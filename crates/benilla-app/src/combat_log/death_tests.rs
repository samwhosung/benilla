//! The death line against the XP award that replaces it: `SMSG_LOG_XPGAIN` naming a held victim
//! sets bit 3 of the unit's `+0xc58` (`0x60a0e6`), and the death line `0x62c160` skips a unit with
//! it set (`0x62c232`-`0x62c23e`).

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{ObjectType, XpGain};
use benilla_protocol::SessionEvent;
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::target::relations::fixture::{creature, player, store_of, HUMAN, ORC};
use crate::ui_chat::combat::watch::death_lines;

const ME: u64 = 1;
const MOB: u64 = 2;
const OTHER: u64 = 3;

/// You and two creatures held, the log empty.
fn world() -> App {
    let mut app = App::new();
    app.add_message::<CombatTextSpawn>()
        .add_message::<crate::net::FieldChanged>()
        .init_resource::<GuidIndex>()
        .init_resource::<crate::ui_party::GroupState>()
        .init_resource::<Reputations>()
        .init_resource::<CombatLogRanges>()
        .init_resource::<DamageTextGates>()
        .init_resource::<ChatLog>()
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(LogPeriodicSpells(true));
    for unit in [player(ME, HUMAN), creature(MOB, ORC), creature(OTHER, ORC)] {
        let e = app
            .world_mut()
            .spawn((Guid(unit.guid), store_of(&unit)))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(unit.guid, e);
    }
    app
}

fn award(app: &mut App, victim: u64, total: u32, kill: bool) {
    let packet = SessionEvent::XpGain(XpGain {
        victim,
        total,
        base: total,
        kill,
        group: 1.0,
    });
    app.world_mut()
        .run_system_once_with(on_xp_gain, packet)
        .unwrap();
}

/// `guid` drops from alive to dead; what lines the death watcher then parks, by key.
fn dies(app: &mut App, guid: u64) -> Vec<String> {
    let entity = app.world().resource::<GuidIndex>().0[&guid];
    app.world_mut().write_message(crate::net::FieldChanged {
        entity,
        guid,
        kind: ObjectType::Unit,
        index: FIELD_UNIT_HEALTH,
        old: 40,
        new: 0,
    });
    app.world_mut().run_system_once(death_lines).unwrap();
    app.world()
        .resource::<ChatLog>()
        .pending_combat()
        .into_iter()
        .map(|l| l.family.key(l.variant))
        .collect()
}

/// A kill that awards experience prints the XP line and no death line, even a zero award
/// (`0x60a0c0` sets the bit before `0x6251e0` tests the amount); a kill that awards none, or one
/// the packet names no held unit for, prints its death line.
#[test]
fn a_kill_that_awards_experience_prints_no_death_line() {
    let death = |setup: &dyn Fn(&mut App)| {
        let mut app = world();
        setup(&mut app);
        dies(&mut app, MOB)
    };
    assert_eq!(death(&|_| {}), ["UNITDIESOTHER"], "no award");
    assert!(death(&|app| award(app, MOB, 60, true)).is_empty());
    assert!(
        death(&|app| award(app, MOB, 0, true)).is_empty(),
        "a zero award"
    );
    assert_eq!(
        death(&|app| award(app, 77, 60, true)),
        ["UNITDIESOTHER"],
        "a victim not held"
    );
    assert_eq!(
        death(&|app| award(app, 0, 60, false)),
        ["UNITDIESOTHER"],
        "experience that names no victim"
    );
    assert_eq!(
        death(&|app| award(app, OTHER, 60, true)),
        ["UNITDIESOTHER"],
        "another unit's award"
    );
}
