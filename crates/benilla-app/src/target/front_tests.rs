//! `SelectTarget`'s front ([`super::front`]) as the selection writers meet it, on the whole client
//! headless: the charmed return ahead of every writer, and the cursor's bind in place of a select.

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::{click, scan, Hovered, PressPick, Selection};
use crate::creature_anim::SwingMessage;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::vplates::PlateClicks;
use benilla_world::interact::WorldClick;

pub(super) const ME: u64 = 0x10;
/// Selected before each writer runs.
pub(super) const OLD: u64 = 0x21;
/// The unit each writer finds: the plate, the click, the attacker.
pub(super) const MOB: u64 = 0x31;
/// A spell the library does not know, so any unit binds under its `TARGET_FLAG_UNIT` word.
pub(super) const HEAL: u32 = 2050;

pub(super) struct Fix {
    pub(super) app: App,
    pub(super) rx: Receiver<ClientCommand>,
    pub(super) mob: Entity,
}

/// The whole client, headless, with us, `OLD` selected and `MOB` beside; `charmed` has someone
/// charm us (`UNIT_FIELD_CHARMEDBY`, a guid of two fields).
pub(super) fn fix(charmed: bool) -> Fix {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let unit = |guid: u64| {
        (
            Guid(guid),
            GlobalTransform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (22, 100),
                (28, 100),
            ])),
        )
    };
    // `UNIT_FLAG_PVP_ATTACKABLE`: with no faction table a plain unit is attackable.
    let mut mine = vec![(2, 0x19), (22, 100), (28, 100), (46, 0x8)];
    if charmed {
        mine.extend([(10, 0x99), (11, 0)]);
    }
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            Transform::default(),
            GlobalTransform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&mine)),
        ))
        .id();
    let old = world.spawn(unit(OLD)).id();
    let mob = world.spawn(unit(MOB)).id();
    let index = &mut world.resource_mut::<GuidIndex>().0;
    index.extend([(ME, me), (OLD, old), (MOB, mob)]);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    *world.resource_mut::<Selection>() = Selection {
        target: Some(old),
        guid: Some(OLD),
        ..Default::default()
    };
    Fix { app, rx, mob }
}

impl Fix {
    pub(super) fn cursor_up(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<crate::spell::SpellTargeting>()
            .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
    }

    pub(super) fn cursor_is_up(&self) -> bool {
        self.app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .active()
    }

    pub(super) fn selected(&self) -> Option<u64> {
        self.app.world().resource::<Selection>().guid
    }

    pub(super) fn click(&mut self, pick: Hovered) {
        let world = self.app.world_mut();
        world.resource_mut::<PressPick>().hovered = pick;
        world
            .resource_mut::<Messages<WorldClick>>()
            .write(WorldClick);
    }

    /// A unit the TAB scan can pick, `x` yards along.
    pub(super) fn scannable_at(&mut self, guid: u64, x: f32) -> Entity {
        let world = self.app.world_mut();
        let e = world
            .spawn((
                Guid(guid),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (22, 100),
                    (28, 100),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(guid, e);
        e
    }

    /// One TAB from a key's body, with the host lent as the bindings pass lends it.
    pub(super) fn tab(&mut self) {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM").run_binding("TargetNearestEnemy()")
        })
        .expect("the key runs");
        self.bind();
    }

    /// The `NpcGreetingRequest`s written since the last look.
    pub(super) fn greetings(&mut self) -> Vec<Entity> {
        self.app
            .world_mut()
            .resource_mut::<Messages<crate::sound::NpcGreetingRequest>>()
            .drain()
            .map(|g| g.npc)
            .collect()
    }

    pub(super) fn over_the_mob(&self) -> Hovered {
        Hovered {
            target: Some(self.mob),
            guid: Some(MOB),
            distance: 5.0,
            ..Hovered::default()
        }
    }

    /// `entity` held as a unit (`OBJECT_FIELD_TYPE` 0x9), alive, with `more` fields.
    pub(super) fn make_unit_with(&mut self, entity: Entity, more: &[(u16, u32)]) {
        let mut fields = vec![(2, 0x9), (22, 100), (28, 100)];
        fields.extend_from_slice(more);
        self.app.world_mut().entity_mut(entity).insert(ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&fields),
        ));
    }

    pub(super) fn make_unit(&mut self, entity: Entity) {
        self.make_unit_with(entity, &[]);
    }

    /// `SMSG_SPELL_START` by `caster` aimed at us, as the spell handler hands it on.
    pub(super) fn cast_at_us(&mut self, caster: Entity) {
        self.app
            .world_mut()
            .resource_mut::<Messages<crate::spell::net::CastAtPlayer>>()
            .write(crate::spell::net::CastAtPlayer { caster, seq: 0 });
    }

    /// The frame's acquire over the swings and casts written since the last, which it consumes as
    /// the frame's reader does.
    pub(super) fn acquire(&mut self) {
        let world = self.app.world_mut();
        world
            .run_system_once(scan::auto_acquire)
            .expect("the acquire runs");
        world
            .resource_mut::<Messages<crate::spell::net::CastAtPlayer>>()
            .clear();
        world.resource_mut::<Messages<SwingMessage>>().clear();
    }

    /// The target chain's bind of what the writers parked.
    pub(super) fn bind(&mut self) {
        self.app
            .world_mut()
            .run_system_once(crate::script_calls::bind_parked_picks)
            .expect("the bind runs as a one-shot system");
    }

    /// What went on the wire: the casts' targets and the selections.
    pub(super) fn wire(&self) -> (Vec<Option<u64>>, Vec<u64>) {
        let sent: Vec<_> = self.rx.try_iter().collect();
        let casts = sent
            .iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } if *spell_id == HEAL => Some(*target),
                _ => None,
            })
            .collect();
        let picks = sent
            .iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(*guid),
                _ => None,
            })
            .collect();
        (casts, picks)
    }
}

/// A left click on a nameplate is the body click's select (`0x7cb910` → `0x4949f0` action 1 →
/// `0x4925d0`), whose `SelectTarget` (`0x49280c`) binds the unit to a standing cursor's cast
/// and returns. The cursor down, the same click selects.
#[test]
fn a_plate_click_with_the_cursor_up_binds_its_unit() {
    let mut up = fix(false);
    up.cursor_up();
    let mob = up.mob;
    up.app
        .world_mut()
        .resource_mut::<PlateClicks>()
        .left
        .push(mob);
    up.app
        .world_mut()
        .run_system_once(click::select_on_plate_click)
        .expect("the plate click runs");
    up.bind();
    assert_eq!(up.wire(), (vec![Some(MOB)], vec![]), "bound, not selected");
    assert_eq!(up.selected(), Some(OLD));
    assert!(!up.cursor_is_up());

    let mut down = fix(false);
    let mob = down.mob;
    down.app
        .world_mut()
        .resource_mut::<PlateClicks>()
        .left
        .push(mob);
    down.app
        .world_mut()
        .run_system_once(click::select_on_plate_click)
        .expect("the plate click runs");
    assert_eq!(down.wire(), (vec![], vec![MOB]));
    assert_eq!(down.selected(), Some(MOB));
}

/// `SelectTarget`'s charmed return (`0x49359c`) is ahead of every writer, deselects included,
/// and ahead of the cursor's bind: each of them leaves a charmed player's target and cast be,
/// and does its work for one who is not.
#[test]
fn a_charmed_player_is_refused_by_every_selector() {
    type Run = fn(&mut Fix);
    let writers: [(&str, bool, Run, Option<u64>, bool); 7] = [
        (
            "a plate click",
            false,
            |f| {
                let mob = f.mob;
                f.app
                    .world_mut()
                    .resource_mut::<PlateClicks>()
                    .left
                    .push(mob);
                f.app
                    .world_mut()
                    .run_system_once(click::select_on_plate_click)
                    .unwrap();
            },
            Some(MOB),
            false,
        ),
        (
            "a click on a unit",
            false,
            |f| {
                let pick = f.over_the_mob();
                f.click(pick);
                f.app
                    .world_mut()
                    .run_system_once(click::select_on_click)
                    .unwrap();
            },
            Some(MOB),
            false,
        ),
        (
            "a click on empty world",
            false,
            |f| {
                f.click(Hovered::default());
                f.app
                    .world_mut()
                    .run_system_once(click::select_on_click)
                    .unwrap();
            },
            None,
            false,
        ),
        (
            "a click with the cursor up",
            true,
            |f| {
                let pick = f.over_the_mob();
                f.click(pick);
                f.app
                    .world_mut()
                    .run_system_once(crate::spell::targeting::commit_object_cast_on_click)
                    .unwrap();
            },
            Some(OLD),
            true,
        ),
        (
            "an attacker acquired",
            false,
            |f| {
                let (me, mob) = (f.app.world().resource::<GuidIndex>().0[&ME], f.mob);
                *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
                f.app
                    .world_mut()
                    .resource_mut::<Messages<SwingMessage>>()
                    .write(SwingMessage {
                        attacker: mob,
                        victim: Some(me),
                        hit_info: 0,
                        victim_state: 1,
                        damage: 1,
                        displayed: true,
                        seq: 0,
                    });
                f.app
                    .world_mut()
                    .run_system_once(scan::auto_acquire)
                    .unwrap();
            },
            Some(MOB),
            false,
        ),
        (
            "a caster at us acquired",
            false,
            |f| {
                *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
                let mob = f.mob;
                f.make_unit(mob);
                f.cast_at_us(mob);
                f.app
                    .world_mut()
                    .run_system_once(scan::auto_acquire)
                    .unwrap();
            },
            Some(MOB),
            false,
        ),
        (
            "ClearTarget",
            false,
            |f| {
                let script = benilla_ui::script::UiScript::new().expect("a VM");
                f.app.world_mut().insert_non_send_resource(script);
                crate::script_calls::with_hosted_vm(f.app.world_mut(), |s| {
                    s.expect("a VM").run("ClearTarget()")
                })
                .expect("the script runs");
                f.bind();
            },
            None,
            false,
        ),
    ];
    for (what, cursor, run, free, casts) in writers {
        for charmed in [false, true] {
            let mut f = fix(charmed);
            if cursor {
                f.cursor_up();
            }
            let before = if what == "an attacker acquired" || what == "a caster at us acquired" {
                None
            } else {
                Some(OLD)
            };
            run(&mut f);
            f.bind();
            let (cast, picks) = f.wire();
            if charmed {
                assert_eq!(f.selected(), before, "{what}: charmed");
                assert_eq!((cast, picks), (vec![], vec![]), "{what}: charmed");
            } else {
                assert_eq!(f.selected(), free, "{what}: free");
                assert_eq!(!cast.is_empty(), casts, "{what}: free, casts");
            }
        }
    }
}

/// `HandleUnitCombatMessage`'s select of the attacker (`0x6259fe`) is `SelectTarget` too, so
/// with a cursor standing it binds the attacker to the cast and the selection stays empty. The
/// packet handler runs in no script, whose context passes the bind's tag 2 (`0x494a5a`).
#[test]
fn an_attacker_acquired_with_the_cursor_up_is_bound() {
    let mut f = fix(false);
    f.cursor_up();
    let (me, mob) = (f.app.world().resource::<GuidIndex>().0[&ME], f.mob);
    *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
    f.app
        .world_mut()
        .resource_mut::<Messages<SwingMessage>>()
        .write(SwingMessage {
            attacker: mob,
            victim: Some(me),
            hit_info: 0,
            victim_state: 1,
            damage: 1,
            displayed: true,
            seq: 0,
        });
    f.app
        .world_mut()
        .run_system_once(scan::auto_acquire)
        .expect("the acquire runs");
    f.bind();
    assert_eq!(f.wire(), (vec![Some(MOB)], vec![]));
    assert_eq!(f.selected(), None);
    assert!(!f.cursor_is_up());
}

/// Another unit casting at us is selected through `SelectTarget` (`0x6e799c`) when it cannot
/// assist us (`CanAssist 0x6066f0` at `0x6e7973`) and nothing is selected (`[0xb4e2d8]`,
/// `0x6e7980`); `PLAYER_TARGET_CHANGED` follows the selection. Our own pet, which can assist us,
/// is not selected, nor is a caster the object manager does not hold as a unit, nor anyone while
/// a target is held.
#[test]
fn a_caster_at_us_is_selected_when_it_cannot_assist_and_nothing_is() {
    const PET: u64 = 0x41;
    let mut f = fix(false);
    let mob = f.mob;
    f.make_unit(mob);
    f.cast_at_us(mob);
    f.acquire();
    assert_eq!(f.selected(), Some(OLD), "a target is held");
    assert_eq!(f.wire(), (vec![], vec![]));

    *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
    // `UNIT_FLAG_PLAYER_CONTROLLED` and `UNIT_FIELD_CREATEDBY` us: friendly, as the party rung
    // counts the active player (`0x6062b0`).
    let pet = f.scannable_at(PET, 5.0);
    f.make_unit_with(pet, &[(46, 0x8), (14, ME as u32), (15, 0)]);
    f.cast_at_us(pet);
    let old = f.app.world().resource::<GuidIndex>().0[&OLD];
    f.cast_at_us(old);
    f.acquire();
    assert_eq!(
        f.selected(),
        None,
        "the pet can assist us, and OLD is no unit"
    );
    assert_eq!(f.wire(), (vec![], vec![]));

    f.cast_at_us(mob);
    f.acquire();
    assert_eq!(f.selected(), Some(MOB));
    assert_eq!(f.wire(), (vec![], vec![MOB]), "CMSG_SET_SELECTION");
}

/// The caster's select is `SelectTarget` too, so a standing cursor binds it to the cast instead.
#[test]
fn a_caster_acquired_with_the_cursor_up_is_bound() {
    let mut f = fix(false);
    f.cursor_up();
    *f.app.world_mut().resource_mut::<Selection>() = Selection::default();
    let mob = f.mob;
    f.make_unit(mob);
    f.cast_at_us(mob);
    f.app
        .world_mut()
        .run_system_once(scan::auto_acquire)
        .expect("the acquire runs");
    f.bind();
    assert_eq!(f.wire(), (vec![Some(MOB)], vec![]));
    assert_eq!(f.selected(), None);
}

/// 1.12.1 stores the cycle's index before `SelectTarget` (`0x4941d4`, `0x4941e6`), so a pick the
/// cursor took, or refused, is behind the next press. Here the nearest enemy is inside the spell's
/// minimum range, which the bind refuses with the cursor standing; the second press must go on
/// to the next, not take the first again.
#[test]
fn a_tab_goes_on_past_a_unit_the_cursor_refused() {
    const TOO_CLOSE: u64 = 0x41;
    const IN_RANGE: u64 = 0x42;
    let mut f = fix(false);
    f.scannable_at(TOO_CLOSE, 3.0);
    f.scannable_at(IN_RANGE, 20.0);
    let mut spells = crate::ui_action::Spells::empty_for_tests();
    spells.catalog = benilla_formats::SpellCatalog::from_displays(
        [(
            HEAL,
            benilla_formats::SpellDisplay {
                range_index: 114,
                ..Default::default()
            },
        )]
        .into(),
    );
    spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(
        [(
            114,
            benilla_formats::SpellRange {
                min: 8.0,
                max: 35.0,
                flags: 0,
            },
        )]
        .into(),
    );
    f.app.world_mut().insert_resource(spells);
    f.app
        .world_mut()
        .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("a VM"));
    f.cursor_up();

    f.tab();
    assert_eq!(
        f.wire(),
        (vec![], vec![]),
        "the first pick is too close to bind"
    );
    assert!(f.cursor_is_up(), "and the cursor stands");
    f.tab();
    assert_eq!(
        f.wire(),
        (vec![Some(IN_RANGE)], vec![]),
        "the second press has moved on to the next"
    );
    assert_eq!(f.selected(), Some(OLD), "the target never moved");
}

/// A left click with the cursor up and no object under the press: the terrain leg
/// (`HandleGroundClick 0x492580`) tries `BindLocation 0x6e60f0`, which for a word with no location
/// bit signals (`0x6e615d`) and returns 0, so the click falls to `OnWorldClick 0x5e0320` and
/// deselects (`0x5e03cf`), sending `CMSG_SET_SELECTION` 0 with the cursor still up; the sky's
/// nothing leg deselects with no signal (`0x492e5d`). A word with a location bit binds and keeps
/// the target.
#[test]
fn a_click_the_cursor_cannot_bind_deselects() {
    for (label, point, word, deselects, changes) in [
        (
            "terrain, a unit word",
            Some(Vec3::new(1.0, 2.0, 3.0)),
            0x0002,
            true,
            1,
        ),
        ("the sky, a unit word", None, 0x0002, true, 0),
        (
            "terrain, a location word",
            Some(Vec3::new(1.0, 2.0, 3.0)),
            0x0040,
            false,
            2,
        ),
    ] {
        let mut f = fix(false);
        let world = f.app.world_mut();
        world.resource_mut::<crate::spell::SpellTargeting>().enter(
            HEAL,
            crate::spell::CastCommit::Spell,
            word,
        );
        world.resource_mut::<PressPick>().occlusion = super::PickOcclusion {
            distance: if point.is_some() { 5.0 } else { f32::INFINITY },
            point,
            ray: None,
        };
        world
            .resource_mut::<crate::ui_cast::CastReports>()
            .0
            .clear();
        f.click(Hovered::default());
        f.app
            .world_mut()
            .run_system_once(click::select_on_click)
            .expect("the click runs");
        f.app
            .world_mut()
            .run_system_once(crate::spell::targeting::commit_ground_cast_on_click)
            .expect("the terrain commit runs");
        let sent: Vec<_> = f.rx.try_iter().collect();
        let selections: Vec<_> = sent
            .iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(*guid),
                _ => None,
            })
            .collect();
        let reports = &f.app.world().resource::<crate::ui_cast::CastReports>().0;
        let signalled = reports
            .iter()
            .filter(|r| matches!(r, crate::ui_cast::CastReport::Changed))
            .count();
        if deselects {
            assert_eq!(f.selected(), None, "{label}");
            assert_eq!(selections, [0], "{label}");
            assert!(f.cursor_is_up(), "{label}: the cursor stays");
        } else {
            assert_eq!(f.selected(), Some(OLD), "{label}");
            assert!(selections.is_empty(), "{label}");
            assert!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::CastSpellAtDest { .. })),
                "{label}: the bind sends"
            );
        }
        assert_eq!(signalled, changes, "{label}");
    }
}

/// The greeter (`PlayGreetingSound`, `0x60c270`) runs at `0x4927fd` for every unit a body click
/// names, ahead of `SelectTarget`, with nothing in between that tests targeting: a click the
/// cursor binds greets too. The click router returns for a charmed player first (`0x481f98`), so
/// that one is not greeted; a nearer GameObject takes the click from the unit.
#[test]
fn a_body_click_greets_with_the_cursor_up_unless_charmed_or_on_an_object() {
    let click = |f: &mut Fix| {
        let pick = f.over_the_mob();
        f.click(pick);
        f.app
            .world_mut()
            .run_system_once(click::select_on_click)
            .expect("the click runs");
        f.greetings()
    };
    let mut up = fix(false);
    up.cursor_up();
    let mob = up.mob;
    assert_eq!(click(&mut up), vec![mob], "cursor up");
    assert_eq!(up.selected(), Some(OLD), "and the click selected nothing");

    let mut down = fix(false);
    let mob = down.mob;
    assert_eq!(click(&mut down), vec![mob], "cursor down, as ever");

    for cursor in [true, false] {
        let mut charmed = fix(true);
        if cursor {
            charmed.cursor_up();
        }
        assert_eq!(click(&mut charmed), vec![], "charmed, cursor {cursor}");
    }

    // A lock word takes GameObjects; the chest is nearer than the unit.
    let mut chest = fix(false);
    chest
        .app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(HEAL, crate::spell::CastCommit::Spell, 0x4800);
    let mob = chest.mob;
    chest.app.world_mut().resource_mut::<PressPick>().object = crate::target::HoveredObject {
        target: Some(mob),
        guid: Some(0xF110_0000_0000_0001),
        distance: 1.0,
    };
    assert_eq!(click(&mut chest), vec![], "the chest has the click");
}

/// A plate click greets before it selects (`0x7cb910` → `0x4949f0` → `0x4925d0` → `0x4927fd`), the
/// cursor up or down; the router's charmed return (`0x481f98`) is the body click's, not the
/// plate's, so a charmed player's plate click greets too.
#[test]
fn a_plate_click_greets_with_the_cursor_up() {
    for (charmed, cursor) in [(false, true), (false, false), (true, true)] {
        let mut f = fix(charmed);
        if cursor {
            f.cursor_up();
        }
        let mob = f.mob;
        f.app
            .world_mut()
            .resource_mut::<PlateClicks>()
            .left
            .push(mob);
        f.app
            .world_mut()
            .run_system_once(click::select_on_plate_click)
            .expect("the plate click runs");
        assert_eq!(
            f.greetings(),
            vec![mob],
            "charmed {charmed}, cursor {cursor}"
        );
    }
}

/// The `Interact` API's attack leg selects through `StartMeleeAttack` (`0x5eccaa`), which is
/// `SelectTarget`: with a targeting cursor up the unit is bound to its cast, and the swing goes on.
#[test]
fn an_interact_attack_with_the_cursor_up_binds_its_unit() {
    const FOE: u64 = 0x43;
    let attack = |cursor: bool| {
        let mut f = fix(false);
        let foe = f.scannable_at(FOE, 3.0);
        f.app.world_mut().init_resource::<crate::player::Player>();
        if cursor {
            f.cursor_up();
        }
        f.app
            .world_mut()
            .resource_mut::<Messages<click::Interact>>()
            .write(click::Interact(foe));
        f.app
            .world_mut()
            .run_system_once(click::act_on_interact)
            .expect("the interact runs");
        f.bind();
        let sent: Vec<_> = f.rx.try_iter().collect();
        let swung = sent
            .iter()
            .any(|c| matches!(c, ClientCommand::AttackSwing { guid: FOE }));
        let (casts, picks): (Vec<_>, Vec<_>) = (
            sent.iter()
                .filter_map(|c| match c {
                    ClientCommand::CastSpell { target, .. } => Some(*target),
                    _ => None,
                })
                .collect(),
            sent.iter()
                .filter_map(|c| match c {
                    ClientCommand::SetSelection { guid } => Some(*guid),
                    _ => None,
                })
                .collect(),
        );
        (f.selected(), swung, casts, picks)
    };
    assert_eq!(
        attack(false),
        (Some(FOE), true, vec![], vec![FOE]),
        "cursor down: it selects and swings"
    );
    assert_eq!(
        attack(true),
        (Some(OLD), true, vec![Some(FOE)], vec![]),
        "cursor up: it binds, selects nothing, and swings on"
    );
}
