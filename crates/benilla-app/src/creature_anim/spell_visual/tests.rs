//! Headless integration tests for [`super::route_cast_visuals`] — the cast-edge router run in a
//! minimal app over a synthetic visual chain. First tenant: the instant-cast hold release. An
//! instant cast's START and GO drain from the wire in the same frame, so the GO's spell-id-keyed
//! release must see the hold its own batch's START inserted through deferred `commands` — the
//! stale-query miss left Demon Armor / Ice Armor casters looping the cast pose forever (the
//! director's stuck-cast report, 2026-07-13).

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

/// Demon Armor's real chain shape (5875 `spellvis 706`): visual 130 → precast kit 217, anim 52 —
/// an instant self-buff whose precast kit carries a sustained cast anim.
const SPELL: u32 = 706;
const VISUAL: u32 = 130;
const PRECAST_KIT: u32 = 217;
const HOLD_ANIM: u16 = 52;

/// A ranged-slot spell with its own chain (an Aimed-Shot shape: `Attributes & 0x2` + a real
/// visual whose cast kit plays the fire clip) — the `0x400` hold tests' subject.
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
                    attributes: 0x2, // USES_RANGED_SLOT — the `0x400` hold's gate
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

/// The timed-cast lifecycle: START arms the precast hold, the (later-frame) GO releases it.
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

/// The instant-cast regression: START and GO in the SAME frame (one wire drain) — the GO must see
/// the hold its own batch inserted, or it leaks and the cast pose loops forever.
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

/// The spell-id key survives the overlay: a different spell's GO landing mid-cast (a proc) never
/// drops the held cast — across frames or within one.
#[test]
fn a_foreign_go_never_drops_the_hold() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    // Same frame as the START (the proc-during-instant shape) …
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

    // … and a frame later (the classic mid-cast proc).
    app.world_mut()
        .write_message(cast_event(unit, 999, CastEventKind::Go));
    app.update();
    assert_eq!(hold(&app, unit), Some(SPELL), "later foreign GO ignored");
}

/// The precast kit's own sound (kit field 13) rings at START — the gathering shape: Herb
/// Gathering's real chain (5875 `spellvis 2366`: visual 91 → precast kit 64, anim 123
/// "UseStandingLoop", sound 1104 "Gather_Herb"). The hold arms AND the kit-sound edge fires
/// once; the GO releasing the hold emits no second play.
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

/// The `$TRD` resolver ([`super::held_strike_sound`]): the held spell's `SpellVisual` field-14
/// strike sound — Mining's real shape (visual 93 → 1143 "Mining Impact") rings;
/// a visual without the field (Fireball's 67 shape) and an unknown spell stay `None`.
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

/// A same-frame START→FAIL (an instant refusal) releases like the GO path.
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

/// The ranged weapon-visual MERGE (`0x60d450`, decision 0986 correcting 0370's row-level
/// reading), on [`super::resolve_stages`] directly: a RANGED-attribute spell (`Attributes & 0x2`)
/// fills every ZERO slot of its own row from the caster's weapon visual — so a basic shot with no
/// row at all takes the lot, a hunter shot keeps its impact/missile and gains the body kits, and a
/// non-ranged spell never looks.
#[test]
fn ranged_spells_merge_the_weapon_visual_into_their_empty_slots() {
    const THROW: u32 = 2764; // Attributes 0x410012, SpellVisual1 0 — the real Throw shape
    const FIREBALL: u32 = 133; // its own visual; the fallback must stay unused
    const MULTI_SHOT: u32 = 2643; // RANGED, own visual 567: impact + missile, no body kits
    const NO_VIS_MELEE: u32 = 772; // no visual, no RANGED attribute — stays silent
    const WEAPON_VISUAL: u32 = 98; // the real thrown ItemDisplayInfo col-10 substitute
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
                // The real 5875 row: impact kit 658, missile model 528, gate 1, attach 1 — and
                // both body-kit slots empty, which is the whole of B153.
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
                    attributes: 0x2, // ranged bit set AND an own visual: own wins (`60d4b4`)
                    ..Default::default()
                },
            ),
            (
                MULTI_SHOT,
                SpellDisplay {
                    visual: MULTI_SHOT_VISUAL,
                    attributes: 0x10002, // the real word: RANGED set, own visual present
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
        "the empty body-kit slots fill from the weapon — the missing draw/release (B153)"
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

/// The aura state watcher (`arm_aura_state_fx`): a spell id appearing in a unit's aura slots
/// arms its state kit's effects persistent under [`super::FxClass::AuraState`]; the id leaving
/// the slots reaps them; a slot-hold in between writes nothing. Food's real chain shape
/// (5875: spell 433 → visual 51 → state kit 409 → effect 393 `Spells\Item_Bread.mdx`).
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
    // The watcher's other fan-outs: the kit's CharProc edges (`crate::aura_visual`) and its
    // sound leg. Food's kit 409 carries neither, so nothing is asserted here — the
    // messages just have to exist for the writers.
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
                // Slot 4 = the spell-hand tag (KIT_SLOT_TAGS[4] = 0x16) — bread's real slot.
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

    // The aura lands in slot 0: UNIT_FIELD_AURA[0] = 47 carries the spell id; the slot's
    // AURAFLAGS nibble (field 95, low nibble) needs an effect-index bit (occupancy is the
    // flags test).
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

    // Slot held: no further edges.
    app.update();
    assert_eq!(
        app.world().resource::<FxLog>().0.len(),
        1,
        "a held aura re-arms nothing"
    );

    // The aura leaves the slots: one AuraState reap.
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

/// The GO's **release gate** (the client's `0x6e7a70` flush condition): a Speed>0 spell whose
/// cast kit plays a body animation emits its [`MissileSpawn`] deferred (`awaits_release`) —
/// the launch waits for the animation's release keyframe — while a cast kit with no animation
/// (or none at all) launches at GO.
/// The spell impact body twitch (sharpened by 2063): an instant harmful spell's GO
/// lays ONE severity-0 wound on each hit target (`0x6e8bf0` @ `0x6e8c89`), a helpful one lays none;
/// a state kit naming a wound anim adds nothing on either (the client's stage-2 play never reaches
/// the `[8,10]` test, `0x60f383`); and a missile ARRIVAL wounds unconditionally (`0x61dc50` @
/// `0x61dc74`) — even for the helpful spell.
#[test]
fn a_harmful_go_wounds_each_hit_once_and_a_missile_arrival_always() {
    const HARMFUL: u32 = 7386; // Sunder Armor's shape: instant, enemy-targeted
    const HELPFUL: u32 = 139; // Renew's shape: instant, ally-targeted
    const VISUAL_H: u32 = 406;
    const VISUAL_F: u32 = 280;
    const IMPACT_KIT: u32 = 556; // no anim
    const STATE_KIT: u32 = 436; // names CombatWound(9) — must add nothing

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
    // A missile landing wounds whoever it lands on, hostility untested.
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

/// The **location fallback** and its arrival, end to end through the router (`0x6e8a50`'s empty-hit
/// arm, then the arrival `0x61d870`): a Speed>0 GO whose hit and miss lists are empty but which
/// carries a ground point spawns one projectile aimed at the point — the flight a pure ground cast
/// (Flare, a bomb thrown at empty dirt) shows — and that projectile's ground arrival rings
/// `SpellVisual` field 13's kit sound **at the landing point**, not at the caster.
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

/// **B130's crash** — the second ever reported: a release build panicked on `insert<CastHold>`
/// while flying at high speed through the Wetlands, applying hold commands to a unit that had
/// despawned. Both windows are exercised here, because they fail for different reasons:
///
/// 1. **Already gone when the edge is read.** Every despawn of an indexed unit runs inside the wire
///    drain (`DESTROY_OBJECT`, the out-of-range stream-out, the worldport purge), and those are
///    applied at the sync point this chain sits behind — so a START and its subject's death arrive
///    in one batch and the edge outlives the unit.
/// 2. **Queued this frame, no sync point between.** `model_fade::apply_despawn_fade` is
///    Update-unordered against this chain; its despawn can be queued before ours and applied first,
///    which a queue-time existence check structurally cannot see.
///
/// The pass condition is that the frame completes — and that neither window resurrects the unit.
#[test]
fn a_despawned_subject_never_panics_the_router() {
    {
        let mut app = app();
        let unit = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(unit).despawn();
        app.world_mut()
            .write_message(cast_event(unit, SPELL, CastEventKind::Start));
        app.update(); // window 1: panicked here before the fix
        assert!(
            app.world().get_entity(unit).is_err(),
            "the hold write must not resurrect a dead subject"
        );
    }
    {
        // `before_ignore_deferred` is exactly the fade lane's shape — an ordering edge with no sync
        // point on it, so both command queues flush together and the despawn applies first.
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

/// The `0x400` weapon-visual hold (set by `0x60d020`): a RANGED spell's visual play inserts
/// [`RangedHold`] on ANY caster — what keeps a remote shooter in the drawn Load/Hold idle between
/// shots — and a non-ranged visual play clears it (the client's stale-visual cleanup `0x6ec39e`).
#[test]
fn ranged_visual_play_arms_the_any_caster_hold_and_a_non_ranged_play_clears_it() {
    let mut app = app();
    let unit = app.world_mut().spawn_empty().id();

    // A remote shooter's per-shot GO (cast kit resolves) → the hold arms.
    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "a ranged GO's visual play sets the hold"
    );

    // A later NON-ranged visual play (the buff's precast kit) → the stale-visual cleanup.
    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Start));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_none(),
        "a non-ranged visual play clears the hold"
    );

    // A ranged START (the volley activation's precast play) re-arms it too — but this shape's
    // precast stage is empty, so drive it through the GO again after the clear.
    app.world_mut()
        .write_message(cast_event(unit, RANGED_SPELL, CastEventKind::Go));
    app.update();
    assert!(
        app.world().entity(unit).get::<RangedHold>().is_some(),
        "the next ranged play re-arms"
    );
}

/// **The mount poof** ([`super::arm_mount_poof_fx`]) — the three properties the
/// reference's `UNIT_FIELD_MOUNTDISPLAYID` watcher gives it, each a real fork in `0x5ffa50`:
/// **the build leg only** (the whole allocation sits behind `5ffa87 je 0x5ffade` on the NEW
/// value, so a dismount spawns nothing), **any changed value** (0→N and N→N′ alike), and
/// **first sight silent** (a unit that streams in already mounted did not just mount — the
/// level-up ding's own treatment).
#[test]
fn the_mount_poof_puffs_on_the_build_leg_only() {
    use benilla_protocol::ObjectFields;

    /// `UNIT_FIELD_MOUNTDISPLAYID` (index 133).
    const FIELD_MOUNTDISPLAYID: u16 = 133;
    /// `SpellVisualEffectName` row 1185's shipped path — the druid-morph cloud.
    const POOF: &str = "Spells\\DruidMorph_Impact_Base.mdx";
    const POOF_FX: u32 = 1185; // the shipped `SpellVisualEffectName` row (decision 0927)
    /// The M2 attach the hardcoded-effect spawn stamps (`DAT_0080c968[6]`).
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
    // Streams in ALREADY mounted: the create block is no edge, so silence.
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

    // Dismount — the NEW value is 0, so the build leg (and the whole allocation) is skipped.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(0));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no poof on the way down");

    // Mount: 0 → N.
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

    // A steady mounted frame is not an edge — the watcher fires on the field CHANGING.
    crate::net::apply_fields_for_test(app.world_mut(), unit, fields(2404));
    app.update();
    assert!(puffs(&mut app).is_empty(), "no re-puff while just riding");

    // A swap (N → N′) is a change, and the reference rebuilds and puffs again.
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

/// **The link that makes the beam exist at all** (decision 0955 slice 2): a kit carrying a chain
/// `CharProc` emits a [`ChainProcPlay`] when it plays — from BOTH of the reference's dispatcher
/// call sites, `PlaySpellVisualKit`'s tail (the cast release) and the channel poll (`0x612b18`,
/// which is the only way a channelled beam is ever reached). A kit without one emits nothing.
///
/// This is the test that fails if the whole lane silently does nothing: the geometry, the hop
/// array and the wire can all be right while no kit ever asks for a beam.
#[test]
fn a_kit_with_a_chain_char_proc_asks_for_a_beam_from_both_dispatcher_sites() {
    use benilla_formats::{char_proc_type, CharProc};

    const BEAM_SPELL: u32 = 421; // Chain Lightning
    const BEAM_VISUAL: u32 = 36;
    const BEAM_CAST_KIT: u32 = 321;
    const BEAM_CHANNEL_KIT: u32 = 402;

    let mut app = app();
    // Overlay the beam chain onto the fixture catalog: a cast kit with the real type-12 slot and a
    // channel kit with the real type-0 one. Params are the shipped shape — chain id 1, one strand,
    // and the flag that splits cast (0) from channel (1).
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
    // …plus a beam-less kit, so "emits nothing" is a real control and not an empty catalog.
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

    // Site 1 — the cast release (`0x60f35c`).
    let unit = app.world_mut().spawn_empty().id();
    app.world_mut()
        .write_message(cast_event(unit, BEAM_SPELL, CastEventKind::Go));
    app.update();
    assert_eq!(
        plays(&mut app),
        vec![(unit, BEAM_SPELL, false)],
        "the cast kit's type-12 proc asks for a one-shot beam"
    );

    // The control: a spell whose cast kit carries no chain proc asks for nothing.
    app.world_mut()
        .write_message(cast_event(unit, SPELL, CastEventKind::Go));
    app.update();
    assert!(plays(&mut app).is_empty(), "a beam-less kit stays silent");

    // Site 2 — the channel poll (`0x612b18`). The rising edge of `UNIT_CHANNEL_SPELL` is what
    // reaches Drain Life's kit; nothing else ever plays it.
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

// ── The real-chain shooter pin (bug B307) ────────────────────────────────────────────────────
//
// The symptom: on Auto Shot, with any ranged weapon (bow, crossbow, gun), a shooter plays no
// reload animation and fires from a still pose. Every link above this
// point is tested on SYNTHETIC tables, and every router test spawns a bare unit with no
// `NetEntity`/`ObjectStore` — so [`super::WeaponVisualSrc::caster`], the one lookup that turns a
// PLAYER's equipped ranged weapon into the substitute visual the whole merge hangs on, returns
// `None` in all of them and nothing notices (their ranged spell carries its own cast kit).
// These tests close that hole: the real 5875 DBCs, a real self-player, a real `item_template`
// row, one `CastEvent` in, the body's clip out.

/// Auto Shot — the one spell every ranged auto-attack fires (one `SMSG_SPELL_GO` per shot).
/// Its real 5875 row: `Attributes = 0x50012` (so `& 0x2`, USES_RANGED_SLOT, IS set),
/// `AttributesEx2 = 0x20` (auto-repeat), Speed 40 — and **`SpellVisual1 = 0`**. It authors no
/// visual whatsoever, so every clip it plays comes from the equipped weapon's substitute visual
/// through [`super::resolve_stages`]'s merge. If that lookup fails, the shooter is silent.
const AUTO_SHOT: u32 = 75;

/// One real `item_template` row — what the wire hands the client for an equipped ranged weapon.
/// Entry/display/class/subclass read from the live vmangos `mangos.item_template`; the
/// display → `ItemDisplayInfo` col 10 → `SpellVisual` → kit → `AnimationData` tail is the real
/// 5875 DBCs' and is re-derived by the test, never assumed.
struct RealRanged {
    name: &'static str,
    entry: u32,
    display_id: u32,
    /// `ItemClass` 2 (weapon) and its subclass: 2 bow, 3 gun, 18 crossbow.
    class: u32,
    subclass: u32,
    /// The pull, from the weapon visual's PRECAST kit: LoadBow 105 / LoadRifle 106.
    load_anim: u16,
    /// The release, from its CAST kit: AttackBow 46 / AttackRifle 49.
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

/// A crossbow (`item_template` 12651, display 22929 → visual 743 → kits 803/804 → 106/49 —
/// crossbows share the rifle clips, they do not have their own).
const BLACKCROW: RealRanged = RealRanged {
    name: "Blackcrow",
    entry: 12651,
    display_id: 22929,
    class: 2,
    subclass: 18,
    load_anim: 106,
    fire_anim: 49,
};

/// `PLAYER_VISIBLE_ITEM_18_0` — the public item ENTRY worn in equipment slot 17 (vmangos
/// `EQUIPMENT_SLOT_RANGED`), i.e. `PLAYER_VISIBLE_ITEM_1_CREATOR (258) + 2 + 12 × 17`. Spelled
/// out because that base index is private to `benilla-protocol`; `player_visible_item_entry(17)`
/// is the accessor it feeds, and this test would fail loudly if the two ever disagreed.
const VISIBLE_RANGED_ENTRY_FIELD: u16 = 258 + 2 + 12 * 17;

/// Keeps the item layer's ask-once channel ALIVE for the app's life, so an unexpected
/// `ItemQuery` send (the "template not landed" path — the failure mode that would silently
/// starve the weapon lookup) is observable rather than swallowed by a dropped receiver.
#[derive(Resource)]
struct AskLog(crossbeam_channel::Receiver<crate::net::ClientCommand>);

/// The cast router wired over the **real 5875 tables**, with a self-player wearing `weapon` in
/// the ranged slot and its template already landed in the item cache — which is the live state
/// by the time anything shoots (the equipped weapon rendered through the same ask-once layer
/// long before). `None`, with the skip note printed, when there is no WoW install to read.
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
    template.inventory_type = 15; // INVTYPE_RANGED — the real row's; unread by this chain
    items.insert_template(weapon.entry, Some(template));
    app.insert_resource(items);

    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(AskLog(rx));

    // The shooter: our own body, a PLAYER on the wire, with the weapon's ENTRY in the public
    // visible-item field — exactly what `SMSG_UPDATE_OBJECT` carries for equipment.
    let unit = app
        .world_mut()
        .spawn((
            crate::net::SelfPlayer,
            crate::net::NetEntity {
                kind: benilla_protocol::EntityKind::Player,
                display_id: Some(49), // HumanMale — authors 46/49/105/106 internally
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

/// The entries the item layer had to ask the server for — empty is the expected reading (the
/// template is pre-landed); anything here means the weapon lookup starved on a cold cache.
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

/// **B307's pin, bow half.** One `SMSG_SPELL_GO` for Auto Shot on a self-player with a real bow
/// equipped must reach the body as `EmoteAnim { anim_id: 46 }` (AttackBow) — through the live
/// chain end to end: `Spell.dbc` 75 (`Attributes & 0x2`, no visual of its own) →
/// [`super::WeaponVisualSrc::caster`] (`PLAYER_VISIBLE_ITEM` slot 17 → the item template →
/// display 8106) → `ItemDisplayInfo` col 10 (visual 5) → the merge → `SpellVisual` 5's cast kit
/// 164 → its anim 46.
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

/// **B307's pin, "any ranged weapon" half.** A gun and a crossbow take the SAME road to a
/// different pair of clips (visual 224 / 743 → AttackRifle 49) — so a fix that only ever saw a
/// bow, or a schema that only decodes one display row, is caught here.
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

/// The START arm of the same chain — the **pull**, the reload animation itself:
/// `SMSG_SPELL_START` for Auto Shot arms `CastHold { anim_id: 105 }` (LoadBow) on a bow
/// shooter, plus the ranged-slot marks the driver reads ([`RangedHold`], the sheath snap). One
/// START per auto-repeat activation, so this is the clip that opens a volley.
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

    // The GO releases it and fires — the shot's own clip, on the same body.
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

/// The control that proves the tests above are actually exercising
/// [`super::WeaponVisualSrc::caster`] and not passing for some other reason: the SAME shooter
/// with an EMPTY ranged slot resolves nothing at all — Auto Shot has no visual of its own to
/// fall back on.
#[test]
fn a_shooter_with_no_ranged_weapon_resolves_no_clip_at_all() {
    let Some((mut app, unit)) = real_shooter(&WORN_SHORTBOW) else {
        return;
    };
    // Strip the equipment field — the wire's "nothing in slot 17".
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

/// **A state kit's animation id is a comparison, never a play**: `0x60edf0`'s tail
/// has exactly one site that hands a kit's `AnimID` to the animation primitive
/// (`0x60f3c5 call 0x5fe2f0`) and `0x60f387 jne` diverts stage 2 around it. Both `SpellVisual`
/// field-4 consumers hardcode stage 2 — the aura watcher and this one, the impact hand-off
/// `0x61dced` — so the state kit's id is only ever the right-hand side of `0x60f390`'s compare,
/// spent on a base recompute.
///
/// The subject is the real Silithus chain: a Dredge Striker's **Charge** (22911 → visual 3783)
/// plays `Knockdown`(121) from impact kit 348, and state kit 349 names `Stun`(14). Before this,
/// benilla played the 14 as a second one-shot — a `Stun` pose the reference never shows on
/// anything.
///
/// **Scope, corrected by decision 2096:** the recompute this emits does NOT cut the Knockdown.
/// `Knockdown` takes the base-animation lock when it arms, so the `Stand` the recompute resolves is
/// refused and the clip plays out — which is what the director sees on the reference. The recompute
/// cuts only what holds no lock. What this test pins is the router's half: one play, not two.
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

/// A state kit whose **whole visual is its animation id** still arms. The aura
/// watcher's entry test asks "does this kit do anything we model?", and the anim was missing from
/// it — the B114 shape one level over, where an effects-only test dropped Stealth's proc-only kit.
/// 15 of the shipped state kits are anim-only (`benilla-extract kitanim`), among them kit 586's
/// `Stun`(14) — Sneezing Fit, Smoke Bomb and kin.
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
