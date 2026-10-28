//! The dungeon latch `CanShowResetInstances` reads (`0x495c90`), written where the reference
//! writes it: a world transfer's timer (`0x401bc0`, posted by `SMSG_NEW_WORLD` `0x401b00`) makes
//! the new object manager (`0x401c22`), whose creation latches the map left (`0x464ff0` →
//! `0x46500e` → `0x495d10`), at the next tick's start ahead of its packets.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::unit_view_tests::client;

/// Deadmines, a party dungeon, and the Eastern Kingdoms.
const DEADMINES: u32 = 36;
const AZEROTH: u32 = 0;

fn port(map_id: u32, needs_ack: bool) -> SessionEvent {
    SessionEvent::Worldport {
        map_id,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack,
    }
}

fn can_reset(app: &mut App) -> bool {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<Option<f64>>("return CanShowResetInstances()")
            .expect("runs")
    }) == Some(1.0)
}

/// A port out of a dungeon latches it at the port's load, the timer's manager leg, so a listener
/// of the next tick's packets reads the offer; the port's own frame, before its timer, does not.
#[test]
fn the_dungeon_latch_is_written_at_the_ports_load() {
    let mut app = client();
    app.insert_resource(benilla_assets::MapCatalogRes(
        benilla_formats::MapCatalog::from_rows([
            (AZEROTH, "Azeroth", 0),
            (DEADMINES, "DeadminesInstance", 0),
        ])
        .with_instance_types([(DEADMINES, 1)]),
    ));
    let world = app.world_mut();
    // In the dungeon, bound: the login's world load runs in place.
    crate::net::handlers::dispatch(
        world,
        vec![
            port(DEADMINES, false),
            SessionEvent::UpdateInstanceOwnership { owns: true },
        ],
    );
    assert!(!can_reset(&mut app), "standing in the dungeon");
    // Out: the port posts its load, and its own frame still stands in the dungeon.
    crate::net::handlers::dispatch(app.world_mut(), vec![port(AZEROTH, true)]);
    assert!(!can_reset(&mut app), "the port's frame, ahead of its timer");
    app.world_mut().run_schedule(PreUpdate);
    assert!(
        can_reset(&mut app),
        "the next tick's start made the new manager"
    );
}

/// The world entry with the pick's map: `ClientInitializeGame` makes the manager for the
/// character list's map (`0x4015f3`), so a character logged out inside a dungeon and seated at its
/// exit by `SMSG_LOGIN_VERIFY_WORLD`, another map, has left it (`0x401e36`-`0x401ea4`).
#[test]
fn a_login_inside_a_dungeon_latches_it_at_the_world_entry() {
    let mut app = client();
    app.insert_resource(benilla_assets::MapCatalogRes(
        benilla_formats::MapCatalog::from_rows([
            (AZEROTH, "Azeroth", 0),
            (DEADMINES, "DeadminesInstance", 0),
        ])
        .with_instance_types([(DEADMINES, 1)]),
    ));
    const ME: u64 = 0x7;
    let row = benilla_protocol::Character {
        map: DEADMINES,
        ..crate::glue_session::roster::test_character(ME, "Me")
    };
    app.insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
        vec![row],
        ME,
    ));
    crate::net::handlers::dispatch(
        app.world_mut(),
        vec![
            SessionEvent::Connected {
                self_guid: ME,
                name: "Me".into(),
                billing: Default::default(),
                tutorial_flags: None,
                addon_info: None,
            },
            SessionEvent::UpdateInstanceOwnership { owns: true },
        ],
    );
    assert!(
        !can_reset(&mut app),
        "the world entry stands in the dungeon"
    );
    crate::net::handlers::dispatch(app.world_mut(), vec![port(AZEROTH, false)]);
    assert!(can_reset(&mut app), "seated at its exit, another map");
}
