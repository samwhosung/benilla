//! Headless tests for the cast-edge router [`super::route_cast_visuals`] and the aura and mount
//! watchers, over synthetic visual chains and, where an install is present, the shipped tables.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualKit, VisualStages};

use super::super::{
    CastEvent, CastEventKind, CastHold, EmoteAnim, RangedHold, SheathRequest, WoundAnim,
};
use super::{
    route_cast_visuals, KitPush, MissileSpawn, SpellKitFx, SpellKitShake, SpellKitSound,
    SpellVisuals,
};
use crate::creature_anim::SpellGoTargets;

/// Demon Armor's shipped chain: visual 130 → precast kit 217 with anim 52, an instant self-buff.
const SPELL: u32 = 706;
const VISUAL: u32 = 130;
const PRECAST_KIT: u32 = 217;
const HOLD_ANIM: u16 = 52;

/// A ranged-slot spell (`Attributes & 0x2`) whose own visual's cast kit plays the fire clip.
const RANGED_SPELL: u32 = 19434;
const RANGED_VISUAL: u32 = 3180;
const RANGED_CAST_KIT: u32 = 900;
const FIRE_ANIM: u16 = 46;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                VISUAL,
                VisualStages {
                    precast: PRECAST_KIT,
                    ..Default::default()
                },
            ),
            (
                RANGED_VISUAL,
                VisualStages {
                    cast: RANGED_CAST_KIT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            (
                PRECAST_KIT,
                VisualKit {
                    anim_id: Some(HOLD_ANIM),
                    ..Default::default()
                },
            ),
            (
                RANGED_CAST_KIT,
                VisualKit {
                    anim_id: Some(FIRE_ANIM),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                SPELL,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            ),
            (
                RANGED_SPELL,
                SpellDisplay {
                    visual: RANGED_VISUAL,
                    attributes: 0x2, // USES_RANGED_SLOT, the `0x400` hold's gate
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);
    app
}

fn cast_event(entity: Entity, spell_id: u32, kind: CastEventKind) -> CastEvent {
    CastEvent {
        entity,
        spell_id,
        kind,
        seq: 1,
    }
}

fn hold(app: &App, unit: Entity) -> Option<u32> {
    app.world().entity(unit).get::<CastHold>().map(|h| {
        assert_eq!(h.anim_id, HOLD_ANIM);
        h.spell_id
    })
}

#[test]
fn timed_cast_hold_arms_and_releases_across_frames() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.update();
    assert_eq!(hold(&app, unit), Some(SPELL), "START arms the hold");

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(hold(&app, unit), None, "GO releases it");
}

/// An instant cast's START and GO drain in one frame: the GO must see the hold its own batch
/// inserted, or the hold leaks and the cast pose loops forever.
#[test]
fn same_frame_start_and_go_leave_no_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(
        hold(&app, unit),
        None,
        "the instant cast's hold is released"
    );
}

/// The hold is keyed by spell id, so a proc's GO never drops it, in the same frame or later.
#[test]
fn a_foreign_go_never_drops_the_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, 999, CastEventKind::Go));
    app.update();
    assert_eq!(
        hold(&app, unit),
        Some(SPELL),
        "same-frame foreign GO ignored"
    );

    app.world_mut()
        .write_message(cast_event(unit, 999, CastEventKind::Go));
    app.update();
    assert_eq!(hold(&app, unit), Some(SPELL), "later foreign GO ignored");
}

/// The precast kit's sound (kit field 13) rings once at START, not again at GO. Herb Gathering's
/// shipped chain: visual 91 → precast kit 64, anim 123 (UseStandingLoop), sound 1104 (Gather_Herb).
#[test]
fn precast_kit_sound_rings_once_at_start() {
    const HERB: u32 = 2366;
    const HERB_VISUAL: u32 = 91;
    const HERB_KIT: u32 = 64;
    const HERB_SOUND: u32 = 1104;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            HERB_VISUAL,
            VisualStages {
                precast: HERB_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            HERB_KIT,
            VisualKit {
                anim_id: Some(123),
                sound: Some(HERB_SOUND),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            HERB,
            SpellDisplay {
                visual: HERB_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, HERB, CastEventKind::Start));
    app.update();
    let played: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        matches!(
            played.as_slice(),
            [
                SpellKitSound::StopHold { .. },
                SpellKitSound::Play { kit_sound, .. }
            ] if *kit_sound == HERB_SOUND
        ),
        "START rings the precast kit's sound once (got {played:?})"
    );

    app.world_mut()
        .write_message(cast_event(unit, HERB, CastEventKind::Go));
    app.update();
    let after_go: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        !after_go
            .iter()
            .any(|s| matches!(s, SpellKitSound::Play { .. })),
        "the GO plays no second kit sound (got {after_go:?})"
    );
}

/// The `$TRD` strike sound is the held spell's `SpellVisual` field 14: Mining's visual 93 names
/// 1143 (Mining Impact), Fireball's visual 67 none.
#[test]
fn held_strike_sound_reads_the_visuals_field_14() {
    const MINING: u32 = 2575;
    const FIREBALL: u32 = 133;
    let visuals = SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                93,
                VisualStages {
                    precast: 166,
                    strike_sound: Some(1143),
                    ..Default::default()
                },
            ),
            (
                67,
                VisualStages {
                    precast: 30,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::new(),
    );
    let spells = crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                MINING,
                SpellDisplay {
                    visual: 93,
                    ..Default::default()
                },
            ),
            (
                FIREBALL,
                SpellDisplay {
                    visual: 67,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    };
    assert_eq!(
        super::held_strike_sound(&spells, &visuals, MINING),
        Some(1143)
    );
    assert_eq!(super::held_strike_sound(&spells, &visuals, FIREBALL), None);
    assert_eq!(super::held_strike_sound(&spells, &visuals, 999), None);
}

#[test]
fn same_frame_start_and_fail_leave_no_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Fail));
    app.update();
    assert_eq!(hold(&app, unit), None, "the failed cast's hold is released");
}

/// The ranged weapon-visual merge (`0x60d450`): a spell with `Attributes & 0x2` fills every zero
/// slot of its own row from the caster's weapon visual; a non-ranged spell never looks.
#[test]
fn ranged_spells_merge_the_weapon_visual_into_their_empty_slots() {
    const THROW: u32 = 2764; // Attributes 0x410012, SpellVisual1 0: the shipped Throw row
    const FIREBALL: u32 = 133; // its own visual; the fallback must stay unused
    const MULTI_SHOT: u32 = 2643; // RANGED, own visual 567: impact + missile, no body kits
    const NO_VIS_MELEE: u32 = 772; // no visual, no RANGED attribute: stays silent
    const WEAPON_VISUAL: u32 = 98; // a shipped thrown weapon's ItemDisplayInfo col-10 visual
    const MULTI_SHOT_VISUAL: u32 = 567;

    let visuals = SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                WEAPON_VISUAL,
                VisualStages {
                    precast: 171,
                    cast: 172,
                    ..Default::default()
                },
            ),
            (
                VISUAL,
                VisualStages {
                    precast: PRECAST_KIT,
                    ..Default::default()
                },
            ),
            (
                // The shipped row: impact kit 658, missile 528, gate 1, attach 1, no body kits.
                MULTI_SHOT_VISUAL,
                VisualStages {
                    impact: 658,
                    missile_gate: 1,
                    missile_model: 528,
                    missile_attach: 1,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::new(),
    );
    let spells = crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                THROW,
                SpellDisplay {
                    visual: 0,
                    attributes: 0x410012,
                    ..Default::default()
                },
            ),
            (
                FIREBALL,
                SpellDisplay {
                    visual: VISUAL,
                    attributes: 0x2, // ranged, with its own visual: its own slot wins (`0x60d4b4`)
                    ..Default::default()
                },
            ),
            (
                MULTI_SHOT,
                SpellDisplay {
                    visual: MULTI_SHOT_VISUAL,
                    attributes: 0x10002, // the shipped word: RANGED set, own visual present
                    ..Default::default()
                },
            ),
            (
                NO_VIS_MELEE,
                SpellDisplay {
                    visual: 0,
                    attributes: 0,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    };

    let throw = super::resolve_stages(&spells, &visuals, THROW, || Some(WEAPON_VISUAL));
    assert_eq!(
        throw.map(|s| (s.precast, s.cast)),
        Some((171, 172)),
        "Throw borrows the weapon visual's kits"
    );
    assert!(
        super::resolve_stages(&spells, &visuals, THROW, || None).is_none(),
        "no ranged weapon equipped → still silent"
    );
    assert_eq!(
        super::resolve_stages(&spells, &visuals, FIREBALL, || Some(WEAPON_VISUAL))
            .map(|s| s.precast),
        Some(PRECAST_KIT),
        "a populated slot is never displaced by the weapon's"
    );
    let multi = super::resolve_stages(&spells, &visuals, MULTI_SHOT, || Some(WEAPON_VISUAL))
        .expect("Multi-Shot has its own row");
    assert_eq!(
        (multi.precast, multi.cast),
        (171, 172),
        "the empty body-kit slots fill from the weapon — else the draw/release is missing"
    );
    assert_eq!(
        (multi.impact, multi.missile_model, multi.missile_attach),
        (658, 528, 1),
        "its own impact + missile block survives the merge untouched"
    );
    assert_eq!(
        super::resolve_stages(&spells, &visuals, MULTI_SHOT, || None).map(|s| (s.cast, s.impact)),
        Some((0, 658)),
        "no ranged weapon equipped → the own row stands alone, unfilled"
    );
    assert!(
        super::resolve_stages(&spells, &visuals, NO_VIS_MELEE, || Some(WEAPON_VISUAL)).is_none(),
        "a non-ranged spell never takes the fallback"
    );
}

/// The aura watcher arms a state kit's effects persistent under [`super::FxClass::AuraState`] as
/// the spell enters the slots and reaps them as it leaves. Food: spell 433 → visual 51 → kit 409.
#[test]
fn aura_state_kit_arms_persistent_and_reaps_on_aura_end() {
    use benilla_protocol::messages::ObjectFields;

    const FOOD: u32 = 433;
    const FOOD_VISUAL: u32 = 51;
    const STATE_KIT: u32 = 409;
    const BREAD_FX: u32 = 393;

    #[derive(Resource, Default)]
    struct FxLog(Vec<SpellKitFx>);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>();
    app.add_message::<crate::net::FieldChanged>();
    // The watcher's CharProc and sound writers need their messages; kit 409 carries neither.
    app.add_message::<crate::aura_visual::AuraProc>();
    app.add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>();
    app.init_resource::<FxLog>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables_with_paths(
        HashMap::from([(
            FOOD_VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                // Slot 4 is the spell-hand tag (KIT_SLOT_TAGS[4] = 0x16), bread's shipped slot.
                effect_slots: [
                    None,
                    None,
                    None,
                    None,
                    Some(BREAD_FX),
                    None,
                    None,
                    None,
                    None,
                ],
                ..Default::default()
            },
        )]),
        HashMap::from([(BREAD_FX, "Spells\\Item_Bread.mdx".to_string())]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            FOOD,
            SpellDisplay {
                visual: FOOD_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(
        Update,
        (
            super::arm_aura_state_fx,
            |mut r: MessageReader<SpellKitFx>, mut log: ResMut<FxLog>| {
                log.0.extend(r.read().cloned());
            },
        )
            .chain(),
    );

    // Slot 0: UNIT_FIELD_AURA[0] (field 47) holds the spell id; occupancy is an effect-index bit
    // in the slot's AURAFLAGS nibble (field 95's low nibble).
    let eating = ObjectFields::from_pairs(&[(47, FOOD), (95, 0x0E)]);
    let fasted = ObjectFields::from_pairs(&[(95, 0)]);

    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            eating.into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    {
        let log = &app.world().resource::<FxLog>().0;
        assert_eq!(log.len(), 1, "one Begin on the ADD edge");
        let SpellKitFx::Begin {
            spell_id,
            persistent,
            class,
            effects,
            ..
        } = &log[0]
        else {
            panic!("expected Begin");
        };
        assert_eq!(*spell_id, FOOD);
        assert!(*persistent, "state kit persists for the aura's life");
        assert_eq!(*class, super::FxClass::AuraState);
        assert_eq!(
            effects.as_slice(),
            [super::FxSlot {
                tag: 0x16,
                effect: BREAD_FX,
                path: "Spells\\Item_Bread.mdx".to_string(),
            }],
            "bread at the spell hand"
        );
    }

    app.update();
    assert_eq!(
        app.world().resource::<FxLog>().0.len(),
        1,
        "a held aura re-arms nothing"
    );

    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        fasted.into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.update();
    {
        let log = &app.world().resource::<FxLog>().0;
        assert_eq!(log.len(), 2, "one Reap on the REMOVE edge");
        let SpellKitFx::Reap {
            spell_id, class, ..
        } = &log[1]
        else {
            panic!("expected Reap");
        };
        assert_eq!(*spell_id, FOOD);
        assert_eq!(*class, super::FxClass::AuraState);
    }
}

/// An instant harmful GO lays one severity-0 wound on each hit (`0x6e8bf0` at `0x6e8c89`), a
/// helpful one none; a state kit's wound anim adds nothing (stage 2 never reaches the `[8,10]`
/// test, `0x60f383`); a missile arrival always wounds (`0x61dc50` at `0x61dc74`).
#[test]
fn a_harmful_go_wounds_each_hit_once_and_a_missile_arrival_always() {
    const HARMFUL: u32 = 7386; // Sunder Armor's shape: instant, enemy-targeted
    const HELPFUL: u32 = 139; // Renew's shape: instant, ally-targeted
    const VISUAL_H: u32 = 406;
    const VISUAL_F: u32 = 280;
    const IMPACT_KIT: u32 = 556; // no anim
    const STATE_KIT: u32 = 436; // names CombatWound (9), which must add nothing

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    let stages = VisualStages {
        impact: IMPACT_KIT,
        state: STATE_KIT,
        ..Default::default()
    };
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(VISUAL_H, stages), (VISUAL_F, stages)]),
        HashMap::from([
            (
                IMPACT_KIT,
                VisualKit {
                    anim_id: None,
                    ..Default::default()
                },
            ),
            (
                STATE_KIT,
                VisualKit {
                    anim_id: Some(9),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                HARMFUL,
                SpellDisplay {
                    visual: VISUAL_H,
                    targets: 0x80,
                    ..Default::default()
                },
            ),
            (
                HELPFUL,
                SpellDisplay {
                    visual: VISUAL_F,
                    targets: 0x100,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    let wounds = |app: &mut App| -> Vec<Entity> {
        app.world_mut()
            .resource_mut::<Messages<WoundAnim>>()
            .drain()
            .map(|w| w.entity)
            .collect()
    };
    for (spell_id, expected) in [(HARMFUL, vec![target]), (HELPFUL, Vec::new())] {
        app.world_mut()
            .write_message(cast_event(caster, spell_id, CastEventKind::Go));
        app.world_mut().write_message(SpellGoTargets {
            caster,
            spell_id,
            hits: vec![target],
            misses: Vec::new(),
            dest: None,
            ammo_display_id: None,
            seq: 1,
        });
        app.update();
        assert_eq!(wounds(&mut app), expected, "instant GO of spell {spell_id}");
    }
    app.world_mut().write_message(cast_event(
        target,
        HELPFUL,
        CastEventKind::Impact {
            weapon_visual: None,
        },
    ));
    app.update();
    assert_eq!(wounds(&mut app), vec![target], "missile arrival");
}

/// The GO's release gate (`0x6e7a70`): a Speed>0 spell whose cast kit plays a body animation
/// defers its [`MissileSpawn`] to the animation's release keyframe; any other launches at GO.
#[test]
fn missile_spawn_defers_iff_the_cast_kit_animates() {
    const ANIMATED: u32 = 133; // Fireball's shape: cast kit with anim 53
    const SILENT: u32 = 134; // same chain, cast kit with no anim
    const ANIMATED_VISUAL: u32 = 67;
    const SILENT_VISUAL: u32 = 68;
    const CAST_KIT: u32 = 38;
    const MUTE_KIT: u32 = 39;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([
            (
                ANIMATED_VISUAL,
                VisualStages {
                    cast: CAST_KIT,
                    ..Default::default()
                },
            ),
            (
                SILENT_VISUAL,
                VisualStages {
                    cast: MUTE_KIT,
                    ..Default::default()
                },
            ),
        ]),
        HashMap::from([
            (
                CAST_KIT,
                VisualKit {
                    anim_id: Some(53),
                    ..Default::default()
                },
            ),
            (
                MUTE_KIT,
                VisualKit {
                    anim_id: None,
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                ANIMATED,
                SpellDisplay {
                    visual: ANIMATED_VISUAL,
                    speed: 24.0,
                    ..Default::default()
                },
            ),
            (
                SILENT,
                SpellDisplay {
                    visual: SILENT_VISUAL,
                    speed: 24.0,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let target = app.world_mut().spawn_empty().id();
    for spell_id in [ANIMATED, SILENT] {
        app.world_mut().write_message(SpellGoTargets {
            caster,
            spell_id,
            hits: vec![target],
            misses: Vec::new(),
            dest: None,
            ammo_display_id: None,
            seq: 1,
        });
    }
    app.update();
    let spawns: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .map(|m| (m.spell_id, m.awaits_release))
        .collect();
    assert_eq!(
        spawns,
        vec![(ANIMATED, true), (SILENT, false)],
        "deferred iff the cast kit animates"
    );
}

/// A Speed>0 GO with no hits but a ground point spawns one projectile at it (`0x6e8a50`'s empty-hit
/// arm), whose arrival (`0x61d870`) rings `SpellVisual` field 13's kit sound at the landing point.
#[test]
fn a_targetless_dest_go_spawns_a_ground_missile_whose_arrival_sounds_at_the_point() {
    const GROUND: u32 = 1543; // Flare's shape: speed>0, dest-targeted, empty hit list
    const VISUAL: u32 = 318;
    const AREA_KIT: u32 = 3270;
    const BOOM: u32 = 4100;
    let at = Vec3::new(11.0, 2.0, -3.0);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                // Field 6 ≠ 0: the missile owns the arrival, so the GO plays no dest burst.
                missile_gate: 1,
                area_effect: 3021,
                area_kit: AREA_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            AREA_KIT,
            VisualKit {
                sound: Some(BOOM),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            GROUND,
            SpellDisplay {
                visual: VISUAL,
                speed: 5.0,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(SpellGoTargets {
        caster,
        spell_id: GROUND,
        hits: Vec::new(),
        misses: Vec::new(),
        dest: Some(at),
        ammo_display_id: None,
        seq: 1,
    });
    app.update();
    let spawns: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<MissileSpawn>>()
        .drain()
        .collect();
    assert_eq!(spawns.len(), 1, "one projectile at the point");
    assert_eq!(spawns[0].ground_aim, Some(at));
    assert!(spawns[0].targets.is_empty(), "no unit owns it");
    assert!(
        app.world_mut()
            .resource_mut::<Messages<crate::entities::dest_fx::GroundBurst>>()
            .drain()
            .next()
            .is_none(),
        "field 6 ≠ 0 suppresses the GO's dest one-shot — the missile owns the arrival"
    );

    // The arrival the missile lane writes back.
    app.world_mut().write_message(CastEvent {
        entity: caster,
        spell_id: GROUND,
        kind: CastEventKind::GroundImpact { pos: at },
        seq: 2,
    });
    app.update();
    let sounds: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SpellKitSound>>()
        .drain()
        .collect();
    assert!(
        matches!(
            sounds[..],
            [SpellKitSound::PlayAt { pos, kit_sound }] if pos == at && kit_sound == BOOM
        ),
        "the area kit's sound, at the landing point: {sounds:?}"
    );
}

/// A cast edge for a despawned unit neither panics the hold insert nor resurrects the unit, whether
/// it died in the wire drain applied before this chain, or `model_fade::apply_despawn_fade`,
/// unordered against it, queued the despawn first, which no queue-time check can see.
#[test]
fn a_despawned_subject_never_panics_the_router() {
    {
        let mut app = app();
        let unit = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(unit).despawn();
        app.world_mut()
            .write_message(cast_event(unit, SPELL, CastEventKind::Start));
        app.update(); // window 1: the unit is gone before the edge is read
        assert!(
            app.world().get_entity(unit).is_err(),
            "the hold write must not resurrect a dead subject"
        );
    }
    {
        // Window 2, the fade lane's shape: an ordering edge with no sync point, so both command
        // queues flush together and the despawn applies first.
        let mut app = app();
        let unit = app.world_mut().spawn_empty().id();
        app.add_systems(
            Update,
            (move |mut commands: Commands| {
                commands.entity(unit).try_despawn();
            })
            .before_ignore_deferred(route_cast_visuals),
        );
        app.world_mut()
            .write_message(cast_event(unit, SPELL, CastEventKind::Start));
        app.update();
        assert!(
            app.world().get_entity(unit).is_err(),
            "the same-frame despawn wins; the hold write is dropped"
        );
    }
}

/// The `0x400` weapon-visual hold (`0x60d020`): a ranged spell's visual play inserts [`RangedHold`]
/// on any caster, and a non-ranged play clears it (the stale-visual cleanup `0x6ec39e`).
#[test]
fn ranged_visual_play_arms_the_any_caster_hold_and_a_non_ranged_play_clears_it() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "a ranged GO's visual play sets the hold"
    );

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_none(),
        "a non-ranged visual play clears the hold"
    );

    // A ranged START re-arms it too, but this chain's precast stage is empty, so the GO re-arms it.
    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "the next ranged play re-arms"
    );
}

/// The mount poof, as `0x5ffa50` gives it: only the build leg allocates it, behind
/// `0x5ffa87 je 0x5ffade` on the new value, so a dismount is silent; any changed value puffs; and
/// a unit streaming in already mounted does not.
#[test]
fn the_mount_poof_puffs_on_the_build_leg_only() {
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_MOUNTDISPLAYID` (index 133).
    const FIELD_MOUNTDISPLAYID: u16 = 133;
    /// The shipped path of `SpellVisualEffectName` row 1185, the druid-morph cloud.
    const POOF: &str = "Spells\\DruidMorph_Impact_Base.mdx";
    const POOF_FX: u32 = 1185;
    /// The M2 attach the hardcoded-effect spawn stamps (`0x80c968[6]`).
    const BASE_ATTACH: u16 = 0x13;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>();
    app.add_message::<crate::net::FieldChanged>();
    app.insert_resource(SpellVisuals(
        SpellVisualCatalog::from_tables(HashMap::new(), HashMap::new()).with_hardcoded(
            "HARDCODED Mount Poof",
            POOF_FX,
            POOF,
        ),
    ));
    app.add_systems(Update, super::arm_mount_poof_fx);

    let fields = |v: u32| ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, v)]);
    // Streams in already mounted: a create block is no edge.
    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            fields(2404).into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    let puffs = |app: &mut App| -> Vec<super::FxSlot> {
        let mut out = Vec::new();
        let world = app.world_mut();
        let mut msgs = world.resource_mut::<Messages<SpellKitFx>>();
        for m in msgs.drain() {
            if let SpellKitFx::Begin {
                entity, effects, ..
            } = m
            {
                assert_eq!(entity, unit);
                out.extend(effects);
            }
        }
        out
    };
    assert!(
        puffs(&mut app).is_empty(),
        "a rider that streams into view did not just mount"
    );

    // Dismount: the new value is 0, so the build leg is skipped.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(0));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no poof on the way down");

    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2404));
    app.update();
    assert_eq!(
        puffs(&mut app),
        vec![super::FxSlot {
            tag: BASE_ATTACH,
            effect: POOF_FX,
            path: POOF.to_string(),
        }],
        "the build leg puffs the druid-morph cloud at the base attach"
    );

    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2404));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no re-puff while just riding");

    // A swap (N → N′) is a change: the reference rebuilds and puffs again.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2405));
    app.update();
    assert_eq!(
        puffs(&mut app),
        vec![super::FxSlot {
            tag: BASE_ATTACH,
            effect: POOF_FX,
            path: POOF.to_string(),
        }]
    );
}

/// A kit with a chain `CharProc` emits a `ChainProcPlay` from both dispatcher sites:
/// `PlaySpellVisualKit`'s tail (the cast release) and the channel poll (`0x612b18`), the only way
/// a channelled beam is reached.
#[test]
fn a_kit_with_a_chain_char_proc_asks_for_a_beam_from_both_dispatcher_sites() {
    use benilla_formats::{char_proc_type, CharProc};

    const BEAM_SPELL: u32 = 421; // Chain Lightning
    const BEAM_VISUAL: u32 = 36;
    const BEAM_CAST_KIT: u32 = 321;
    const BEAM_CHANNEL_KIT: u32 = 402;

    let mut app = app();
    // A cast kit with a type-12 proc and a channel kit with a type-0 one, in the shipped shape:
    // chain id 1, one strand, and the flag that splits cast (0) from channel (1).
    let mut visuals = HashMap::from([(
        BEAM_VISUAL,
        VisualStages {
            cast: BEAM_CAST_KIT,
            channel: BEAM_CHANNEL_KIT,
            ..Default::default()
        },
    )]);
    let mut kits = HashMap::new();
    for (kit, ty, flag) in [
        (BEAM_CAST_KIT, char_proc_type::CHAIN_CAST, 0.0),
        (BEAM_CHANNEL_KIT, char_proc_type::CHAIN_CHANNEL, 1.0),
    ] {
        kits.insert(
            kit,
            VisualKit {
                char_proc_slots: [
                    Some(CharProc {
                        ty,
                        params: [1.0, 1.0, flag, 0.0],
                    }),
                    None,
                    None,
                    None,
                ],
                ..Default::default()
            },
        );
    }
    // A beam-less kit, the control.
    kits.insert(PRECAST_KIT, VisualKit::default());
    visuals.insert(
        VISUAL,
        VisualStages {
            cast: PRECAST_KIT,
            ..Default::default()
        },
    );
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(visuals, kits)));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            (
                BEAM_SPELL,
                SpellDisplay {
                    visual: BEAM_VISUAL,
                    ..Default::default()
                },
            ),
            (
                SPELL,
                SpellDisplay {
                    visual: VISUAL,
                    ..Default::default()
                },
            ),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });

    let plays = |app: &mut App| -> Vec<(Entity, u32, bool)> {
        app.world_mut()
            .resource_mut::<Messages<super::ChainProcPlay>>()
            .drain()
            .map(|p| (p.entity, p.spell_id, p.proc.flag))
            .collect()
    };

    // Site 1, the cast release (`0x60f35c`).
    let unit = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(cast_event(unit, BEAM_SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(
        plays(&mut app),
        vec![(unit, BEAM_SPELL, false)],
        "the cast kit's type-12 proc asks for a one-shot beam"
    );

    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert!(plays(&mut app).is_empty(), "a beam-less kit stays silent");

    // Site 2, the channel poll (`0x612b18`), on the rising edge of `UNIT_CHANNEL_SPELL`: the only
    // play of a channel kit.
    let channeller = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(144, BEAM_SPELL)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();
    assert_eq!(
        plays(&mut app),
        vec![(channeller, BEAM_SPELL, true)],
        "the channel kit's type-0 proc asks for a persistent beam"
    );
}

// ── The shipped-table shooter ────────────────────────────────────────────────────────────────
//
// The tests above use bare units, for which `WeaponVisualSrc::caster` (a player's ranged weapon →
// its substitute visual) is always `None`. These run the shipped DBCs, a self-player and a real
// `item_template` row: one `CastEvent` in, the body's clip out.

/// Auto Shot, one `SMSG_SPELL_GO` per shot. Its shipped row: `Attributes` 0x50012 (`& 0x2` set),
/// `AttributesEx2` 0x20 (auto-repeat), Speed 40 and `SpellVisual1` 0, so every clip it plays comes
/// from the weapon's substitute visual through [`super::resolve_stages`]'s merge.
const AUTO_SHOT: u32 = 75;

/// A ranged weapon's vmangos `item_template` row; the display → `ItemDisplayInfo` col 10 →
/// `SpellVisual` → kit → anim chain is re-derived from the shipped DBCs by the test.
struct RealRanged {
    name: &'static str,
    entry: u32,
    display_id: u32,
    /// `ItemClass` 2 (weapon) and its subclass: 2 bow, 3 gun, 18 crossbow.
    class: u32,
    subclass: u32,
    /// The pull, from the weapon visual's precast kit: LoadBow 105 or LoadRifle 106.
    load_anim: u16,
    /// The release, from its cast kit: AttackBow 46 or AttackRifle 49.
    fire_anim: u16,
}

/// A bow (`item_template` 2504, display 8106 → visual 5 → kits 7/164 → 105/46).
const WORN_SHORTBOW: RealRanged = RealRanged {
    name: "Worn Shortbow",
    entry: 2504,
    display_id: 8106,
    class: 2,
    subclass: 2,
    load_anim: 105,
    fire_anim: 46,
};

/// A gun (`item_template` 2508, display 6606 → visual 224 → kits 161/167 → 106/49).
const OLD_BLUNDERBUSS: RealRanged = RealRanged {
    name: "Old Blunderbuss",
    entry: 2508,
    display_id: 6606,
    class: 2,
    subclass: 3,
    load_anim: 106,
    fire_anim: 49,
};

/// A crossbow (`item_template` 12651, display 22929 → visual 743 → kits 803/804 → 106/49):
/// crossbows share the rifle clips.
const BLACKCROW: RealRanged = RealRanged {
    name: "Blackcrow",
    entry: 12651,
    display_id: 22929,
    class: 2,
    subclass: 18,
    load_anim: 106,
    fire_anim: 49,
};

/// `PLAYER_VISIBLE_ITEM_18_0`, the item entry in equipment slot 17 (vmangos
/// `EQUIPMENT_SLOT_RANGED`): `PLAYER_VISIBLE_ITEM_1_CREATOR` (258) + 2 + 12 × 17, spelled out as
/// the base is private to `benilla-protocol`.
const VISIBLE_RANGED_ENTRY_FIELD: u16 = 258 + 2 + 12 * 17;

/// Keeps the item layer's ask-once channel alive, so an unexpected `ItemQuery` (a template not
/// landed, which starves the weapon lookup) is observable.
#[derive(Resource)]
struct AskLog(crossbeam_channel::Receiver<crate::net::ClientCommand>);

/// The cast router over the shipped tables, with a self-player wearing `weapon` in the ranged slot
/// and its template landed, as it is by the time anything shoots; `None` without an install.
fn real_shooter(weapon: &RealRanged) -> Option<(App, Entity)> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open the install's MPQ chain");
    let visuals =
        benilla_formats::load_spell_visual_catalog(&mut chain).expect("SpellVisual/SpellVisualKit");
    let spells = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
    let displays =
        benilla_formats::load_item_display_catalog(&mut chain).expect("ItemDisplayInfo.dbc");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(visuals));
    app.insert_resource(crate::ui_action::Spells {
        catalog: spells,
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::entities::ItemDisplays::icons_for_tests(displays));

    let mut items = crate::items::Items::default();
    let mut template = crate::items::test_template(weapon.name);
    template.class = weapon.class;
    template.subclass = weapon.subclass;
    template.display_info_id = weapon.display_id;
    template.inventory_type = 15; // INVTYPE_RANGED, the real row's; unread by this chain
    items.insert_template(weapon.entry, Some(template));
    app.insert_resource(items);

    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(AskLog(rx));

    // Our own body, a player, with the weapon's entry in the visible-item field.
    let unit = app
        .world_mut()
        .spawn((
            crate::net::SelfPlayer,
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Player,
                display_id: Some(49), // HumanMale, which authors 46/49/105/106
                scale: 1.0,
            },
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(
                VISIBLE_RANGED_ENTRY_FIELD,
                weapon.entry,
            )])),
        ))
        .id();
    app.add_systems(Update, route_cast_visuals);
    Some((app, unit))
}

/// The one-shot clips this frame asked of `unit`'s body.
fn emote_anims(app: &mut App, unit: Entity) -> Vec<u16> {
    app.world_mut()
        .resource_mut::<Messages<EmoteAnim>>()
        .drain()
        .filter(|e| e.entity == unit)
        .map(|e| e.anim_id)
        .collect()
}

/// The entries the item layer asked the server for; any means the weapon lookup starved.
fn asked_entries(app: &App) -> Vec<u32> {
    app.world()
        .resource::<AskLog>()
        .0
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::ItemQuery { entry, .. } => Some(entry),
            _ => None,
        })
        .collect()
}

/// Auto Shot's GO with a bow equipped reaches the body as AttackBow (46): slot 17's template →
/// display 8106 → `ItemDisplayInfo` col 10 (visual 5) → the merge → cast kit 164 → anim 46.
#[test]
fn a_real_bow_shooters_auto_shot_go_plays_attackbow() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert_eq!(
        emote_anims(&mut app, unit),
        vec![WORN_SHORTBOW.fire_anim],
        "Auto Shot's GO plays the bow's release clip (asked the server for {:?})",
        asked_entries(&app)
    );
    assert!(
        asked_entries(&app).is_empty(),
        "the weapon template was already landed — no ask-once miss starved the lookup"
    );
}

/// A gun and a crossbow take the same road to AttackRifle (49), through visuals 224 and 743.
#[test]
fn a_real_gun_or_crossbow_shooters_auto_shot_go_plays_attackrifle() {
    for weapon in [&OLD_BLUNDERBUSS, &BLACKCROW] {
        let Some((mut app, unit)) = real_shooter(weapon) else {
            return;
        };
        app.world_mut()
            .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
        app.update();
        assert_eq!(
            emote_anims(&mut app, unit),
            vec![weapon.fire_anim],
            "{}'s Auto Shot GO plays its release clip",
            weapon.name
        );
    }
}

/// Auto Shot's START, once per auto-repeat activation, arms the LoadBow (105) pull as the cast
/// hold, with [`RangedHold`] and the ranged sheath snap; the GO releases it and fires.
#[test]
fn a_real_bow_shooters_auto_shot_start_arms_the_loadbow_hold() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Start));
    app.update();
    let held = app.world().entity(unit).get::<CastHold>();
    assert_eq!(
        held.map(|h| (h.anim_id, h.spell_id, h.ranged)),
        Some((WORN_SHORTBOW.load_anim, AUTO_SHOT, true)),
        "START arms the bow's pull as the cast hold"
    );
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "…and the any-caster `0x400` weapon-visual hold"
    );
    let sheaths: Vec<_> = app
        .world_mut()
        .resource_mut::<Messages<SheathRequest>>()
        .drain()
        .filter(|s| s.entity == unit)
        .map(|s| s.state)
        .collect();
    assert_eq!(sheaths, vec![2], "…and the ranged stance snaps drawn");

    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<CastHold>().is_none(),
        "the GO releases the pull"
    );
    assert_eq!(
        emote_anims(&mut app, unit),
        vec![WORN_SHORTBOW.fire_anim],
        "…and plays the release"
    );
}

/// The control: with the ranged slot empty Auto Shot resolves no clip, having no visual of its own.
#[test]
fn a_shooter_with_no_ranged_weapon_resolves_no_clip_at_all() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    // Slot 17 empty, as the wire says it.
    crate::net::apply_fields_for_test(
        app.world_mut(),
        unit,
        benilla_protocol::ObjectFields::from_pairs(&[(VISIBLE_RANGED_ENTRY_FIELD, 0)])
            .into_created(benilla_protocol::messages::ObjectType::Unit),
    );
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Start));
    app.world_mut()
        .write_message(cast_event(unit, AUTO_SHOT, CastEventKind::Go));
    app.update();
    assert!(
        emote_anims(&mut app, unit).is_empty(),
        "no weapon, no substitute visual, no clip"
    );
    assert!(
        app.world().entity(unit).get::<CastHold>().is_none(),
        "and no pull to hold"
    );
}

/// A state kit's anim id is a compare, never a play: `0x60edf0`'s one play site
/// (`0x60f3c5 call 0x5fe2f0`) is skipped for stage 2 (`0x60f387 jne`), the stage both field-4 users
/// play (the aura watcher and the impact hand-off `0x61dced`), so the id only feeds `0x60f390`'s
/// compare and a base recompute. Charge (22911) plays Knockdown (121) from impact kit 348; state
/// kit 349's Stun (14) is a recompute, which Knockdown's base-animation lock refuses.
#[test]
fn a_state_kits_anim_is_a_recompute_and_never_a_second_play() {
    const CHARGE: u32 = 22911;
    const CHARGE_VISUAL: u32 = 3783;
    const IMPACT_KIT: u32 = 348;
    const STATE_KIT: u32 = 349;
    const KNOCKDOWN: u16 = 121;
    const STUN: u16 = 14;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<CastEvent>()
        .add_message::<SpellGoTargets>()
        .add_message::<KitPush>()
        .add_message::<EmoteAnim>()
        .add_message::<WoundAnim>()
        .add_message::<SpellKitSound>()
        .add_message::<SpellKitShake>()
        .add_message::<crate::weapon_trail::TrailArm>()
        .add_message::<super::BaseAnimRecompute>()
        .add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<MissileSpawn>()
        .add_message::<crate::entities::dest_fx::GroundBurst>()
        .add_message::<super::ChainProcPlay>()
        .add_message::<SheathRequest>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            CHARGE_VISUAL,
            VisualStages {
                impact: IMPACT_KIT,
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([
            (
                IMPACT_KIT,
                VisualKit {
                    anim_id: Some(KNOCKDOWN),
                    ..Default::default()
                },
            ),
            (
                STATE_KIT,
                VisualKit {
                    anim_id: Some(STUN),
                    ..Default::default()
                },
            ),
        ]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            CHARGE,
            SpellDisplay {
                visual: CHARGE_VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, route_cast_visuals);

    let caster = app.world_mut().spawn_empty().id();
    let victim = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(cast_event(caster, CHARGE, CastEventKind::Go));
    app.world_mut().write_message(SpellGoTargets {
        caster,
        spell_id: CHARGE,
        hits: vec![victim],
        misses: Vec::new(),
        dest: None,
        ammo_display_id: None,
        seq: 1,
    });
    app.update();

    let plays: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<EmoteAnim>>()
        .drain()
        .map(|e| (e.entity, e.anim_id))
        .collect();
    assert_eq!(
        plays,
        vec![(victim, KNOCKDOWN)],
        "the impact kit plays; the state kit must not add a second one-shot"
    );
    let recomputes: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<super::BaseAnimRecompute>>()
        .drain()
        .map(|r| (r.entity, r.anim_id))
        .collect();
    assert_eq!(
        recomputes,
        vec![(victim, STUN)],
        "the state kit's id is spent on the stage-2 recompute instead"
    );
}

/// An anim-only state kit still arms, and its add edge owes a recompute: 15 shipped state kits are
/// anim-only (`benilla-extract kitanim`), kit 586's Stun (14) among them.
#[test]
fn an_anim_only_state_kit_still_arms_on_the_aura_add_edge() {
    use benilla_protocol::messages::ObjectFields;

    const SPELL: u32 = 6902; // Sneezing Fit
    const VISUAL: u32 = 517;
    const STATE_KIT: u32 = 586; // anim 14, no effect slot, no sound, no CharProc
    const STUN: u16 = 14;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_message::<SpellKitFx>()
        .add_message::<crate::net::FieldChanged>()
        .add_message::<crate::aura_visual::AuraProc>()
        .add_message::<SpellKitSound>()
        .add_message::<super::BaseAnimRecompute>();
    app.insert_resource(SpellVisuals(SpellVisualCatalog::from_tables(
        HashMap::from([(
            VISUAL,
            VisualStages {
                state: STATE_KIT,
                ..Default::default()
            },
        )]),
        HashMap::from([(
            STATE_KIT,
            VisualKit {
                anim_id: Some(STUN),
                ..Default::default()
            },
        )]),
    )));
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            SPELL,
            SpellDisplay {
                visual: VISUAL,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.add_systems(Update, super::arm_aura_state_fx);

    let unit = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            ObjectFields::from_pairs(&[(47, SPELL), (95, 0x0E)])
                .into_created(benilla_protocol::messages::ObjectType::Unit),
        ))
        .id();
    app.update();

    let recomputes: Vec<(Entity, u16)> = app
        .world_mut()
        .resource_mut::<Messages<super::BaseAnimRecompute>>()
        .drain()
        .map(|r| (r.entity, r.anim_id))
        .collect();
    assert_eq!(
        recomputes,
        vec![(unit, STUN)],
        "an anim-only state kit is not nothing — its ADD edge owes a recompute"
    );
}
