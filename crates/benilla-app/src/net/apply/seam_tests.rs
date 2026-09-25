//! **Every packet's handler sees what the packets before it did** — the property 2306 measured
//! across the migration's seam (a run boundary was a command flush mid-frame) and the one the
//! whole dispatch rests on now that every packet is its own handler with its commands applied
//! before the next. The first test drives the real drain on the built client
//! over one frame two ways and compares; the rest are 1478's speed-staging laws, which used to
//! be unit tests over a staging map and are now the same laws over consecutive packets.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{ObjectType, SpeedKind, SplineMode};
use benilla_protocol::{EntityKind, MoveSpeeds, ObjectFields, SessionEvent};
use bevy::prelude::*;

use crate::net::{GuidIndex, NetEvents, ObjectStore, UnitMoveModes, UnitSpeeds};

const GUID: u64 = 0xF130_0000_1234_0001;

fn create() -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid: GUID,
        kind: EntityKind::Unit,
        display_id: None,
        position: [1.0, 2.0, 3.0],
        orientation: 0.0,
        scale: 1.0,
        speeds: Some(MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.7,
            swim_back: 2.5,
            turn_rate: std::f32::consts::PI,
        }),
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_LEVEL, 9),
        ])
        .into_created(ObjectType::Unit),
    }
}

/// What the object layer does to a unit born this frame: a values delta, a speed change, a
/// granted mode, a second delta.
fn updates() -> Vec<SessionEvent> {
    vec![
        SessionEvent::ObjectValues {
            guid: GUID,
            fields: ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, 60)]),
        },
        SessionEvent::SpeedChanged {
            guid: GUID,
            kind: SpeedKind::Run,
            speed: 14.0,
        },
        SessionEvent::SplineMoveMode {
            guid: GUID,
            mode: SplineMode::Root,
            apply: true,
        },
        SessionEvent::ObjectValues {
            guid: GUID,
            fields: ObjectFields::from_pairs(&[(FIELD_UNIT_LEVEL, 10)]),
        },
    ]
}

/// A packet of another subsystem's — any would do; the mailbox's needs no open window.
fn claimed() -> SessionEvent {
    SessionEvent::NextMailTime { seconds: -86400.0 }
}

/// One frame of the real drain over `events`; what the unit ended up as.
fn drained(events: Vec<SessionEvent>) -> (Vec<(u16, u32)>, MoveSpeeds, UnitMoveModes) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetEvents(rx));
    for ev in events {
        tx.send(ev).unwrap();
    }
    let world = app.world_mut();
    super::apply_net_updates(world);
    let e = *world
        .resource::<GuidIndex>()
        .0
        .get(&GUID)
        .expect("the create indexed the unit");
    let fields = world
        .get::<ObjectStore>(e)
        .expect("the store landed")
        .0
        .raw_fields()
        .collect();
    let speeds = world.get::<UnitSpeeds>(e).expect("the speeds landed").0;
    let modes = *world.get::<UnitMoveModes>(e).expect("the modes landed");
    (fields, speeds, modes)
}

#[test]
fn a_foreign_packet_between_a_create_and_its_updates_changes_nothing() {
    // The create and everything after it back to back, the mailbox's packet last.
    let mut one_run = vec![create()];
    one_run.extend(updates());
    one_run.push(claimed());
    // A foreign packet after the create and between every update.
    let mut split = vec![create()];
    for ev in updates() {
        split.push(claimed());
        split.push(ev);
    }

    let (fields, speeds, modes) = drained(one_run);
    let (split_fields, split_speeds, split_modes) = drained(split);

    // The frame's own meaning, so the comparison below cannot pass on two empty results.
    assert!(fields.contains(&(FIELD_UNIT_HEALTH, 60)), "{fields:?}");
    assert!(fields.contains(&(FIELD_UNIT_LEVEL, 10)), "{fields:?}");
    assert!(fields.contains(&(FIELD_UNIT_MAXHEALTH, 100)), "{fields:?}");
    assert_eq!(speeds.run, 14.0);
    assert_eq!(speeds.walk, 2.5);
    assert!(modes.rooted());

    assert_eq!(split_fields, fields);
    assert_eq!(split_speeds.run, speeds.run);
    assert_eq!(split_speeds.walk, speeds.walk);
    assert_eq!(split_modes, modes);
}

/// A create block with this run speed, or none.
fn create_with_speeds(speeds: Option<MoveSpeeds>) -> SessionEvent {
    match create() {
        SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id,
            position,
            orientation,
            scale,
            mover,
            transport_progress,
            transport,
            spline,
            fields,
            ..
        } => SessionEvent::ObjectCreate {
            guid,
            kind,
            display_id,
            position,
            orientation,
            scale,
            speeds,
            mover,
            transport_progress,
            transport,
            spline,
            fields,
        },
        _ => unreachable!(),
    }
}

fn force_run(speed: f32) -> SessionEvent {
    SessionEvent::ForceSpeedChange {
        guid: GUID,
        kind: SpeedKind::Run,
        counter: 1,
        speed,
    }
}

/// One frame of the real drain; the unit's live speed set, if any.
fn speeds_after(events: Vec<SessionEvent>) -> Option<MoveSpeeds> {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetEvents(rx));
    for ev in events {
        tx.send(ev).unwrap();
    }
    let world = app.world_mut();
    super::apply_net_updates(world);
    let e = *world.resource::<GuidIndex>().0.get(&GUID)?;
    world.get::<UnitSpeeds>(e).map(|s| s.0)
}

/// **B213, pinned at the drain.** vmangos's `HandleMoveWorldportAckOpcode` sends the self create
/// block (`Map::Add` → `SendInitSelf`, still carrying the mount's 11.2 yd/s) and then, three
/// statements later, strips the mount because the destination map forbids one — so
/// `SMSG_FORCE_RUN_SPEED_CHANGE` 7.0 rides the same tick. In wire order the change wins; before
/// 1478 the create's *deferred* `UnitSpeeds` insert landed last and `.tele` into BWL left the
/// avatar running at mount speed on foot. Under a handler per packet the create's insert has
/// landed before the change's handler runs, and the change edits the live component.
#[test]
fn a_force_change_beats_a_create_from_the_same_frame() {
    let mounted = MoveSpeeds {
        run: 11.2,
        ..MoveSpeeds {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.7,
            swim_back: 2.5,
            turn_rate: std::f32::consts::PI,
        }
    };
    let s = speeds_after(vec![create_with_speeds(Some(mounted)), force_run(7.0)])
        .expect("the create landed a speed set");
    assert_eq!(s.run, 7.0, "the dismount is the newer packet");
    assert_eq!(
        s.walk, 2.5,
        "the other slots still come from the create block"
    );
}

/// The reverse order is just as much the wire's order: a create block is the server's newest
/// snapshot of the mover, so one arriving *after* a change replaces it whole.
#[test]
fn a_create_after_a_change_replaces_it() {
    let on_foot = MoveSpeeds {
        walk: 2.5,
        run: 7.0,
        run_back: 4.5,
        swim: 4.7,
        swim_back: 2.5,
        turn_rate: std::f32::consts::PI,
    };
    let mounted = MoveSpeeds {
        run: 11.2,
        ..on_foot
    };
    let s = speeds_after(vec![
        create_with_speeds(Some(on_foot)),
        force_run(9.0),
        create_with_speeds(Some(mounted)),
    ])
    .expect("the create landed a speed set");
    assert_eq!(s.run, 11.2);
}

/// A one-slot change on a mover that already exists keeps every other slot it isn't addressing.
#[test]
fn a_change_on_a_live_mover_keeps_its_other_slots() {
    let on_foot = MoveSpeeds {
        walk: 2.5,
        run: 7.0,
        run_back: 4.5,
        swim: 4.7,
        swim_back: 2.5,
        turn_rate: std::f32::consts::PI,
    };
    let s = speeds_after(vec![create_with_speeds(Some(on_foot)), force_run(11.2)])
        .expect("the create landed a speed set");
    assert_eq!(s.run, 11.2);
    assert_eq!(s.run_back, on_foot.run_back);
    assert_eq!(s.turn_rate, on_foot.turn_rate);
}

/// A change with nowhere to land — a mover whose create carried no movement block — invents no
/// speed set: nothing is inserted for a set that never existed.
#[test]
fn a_change_with_no_speed_set_invents_none() {
    assert!(speeds_after(vec![create_with_speeds(None), force_run(7.0)]).is_none());
}
