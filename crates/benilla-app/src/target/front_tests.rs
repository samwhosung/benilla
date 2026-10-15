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

const ME: u64 = 0x10;
/// Selected before each writer runs.
const OLD: u64 = 0x21;
/// The unit each writer finds: the plate, the click, the attacker.
const MOB: u64 = 0x31;
/// A spell the library does not know, so any unit binds under its `TARGET_FLAG_UNIT` word.
const HEAL: u32 = 2050;

struct Fix {
    app: App,
    rx: Receiver<ClientCommand>,
    mob: Entity,
}

/// The whole client, headless, with us, `OLD` selected and `MOB` beside; `charmed` has someone
/// charm us (`UNIT_FIELD_CHARMEDBY`, a guid of two fields).
fn fix(charmed: bool) -> Fix {
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
    let mut mine = vec![(2, 0x19), (22, 100), (28, 100)];
    if charmed {
        mine.extend([(10, 0x99), (11, 0)]);
    }
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
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
    fn cursor_up(&mut self) {
        self.app
            .world_mut()
            .resource_mut::<crate::spell::SpellTargeting>()
            .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
    }

    fn cursor_is_up(&self) -> bool {
        self.app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .active()
    }

    fn selected(&self) -> Option<u64> {
        self.app.world().resource::<Selection>().guid
    }

    fn click(&mut self, pick: Hovered) {
        let world = self.app.world_mut();
        world.resource_mut::<PressPick>().hovered = pick;
        world
            .resource_mut::<Messages<WorldClick>>()
            .write(WorldClick);
    }

    fn over_the_mob(&self) -> Hovered {
        Hovered {
            target: Some(self.mob),
            guid: Some(MOB),
            distance: 5.0,
            ..Hovered::default()
        }
    }

    /// The calls pass, whose head binds what the writers parked.
    fn bind(&mut self) {
        self.app
            .world_mut()
            .run_system_once(crate::script_calls::apply_script_calls)
            .expect("the calls apply as a one-shot system");
    }

    /// What went on the wire: the casts' targets and the selections.
    fn wire(&self) -> (Vec<Option<u64>>, Vec<u64>) {
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
    let writers: [(&str, bool, Run, Option<u64>, bool); 6] = [
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
                    .run_system_once(scan::auto_acquire_attacker)
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
                script.run("ClearTarget()").expect("the script runs");
                f.app.world_mut().insert_non_send_resource(script);
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
            let before = if what == "an attacker acquired" {
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
        .run_system_once(scan::auto_acquire_attacker)
        .expect("the acquire runs");
    f.bind();
    assert_eq!(f.wire(), (vec![Some(MOB)], vec![]));
    assert_eq!(f.selected(), None);
    assert!(!f.cursor_is_up());
}
