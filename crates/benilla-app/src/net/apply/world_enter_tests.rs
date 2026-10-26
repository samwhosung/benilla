//! The world-enter sends wait for our own player's create, as the reference's do (`0x5dea50`):
//! none goes out on the login edge alone, which fires before the server seats the player; the
//! create sends `CMSG_SET_ACTIVE_MOVER`, then the cascade's `CMSG_QUERY_TIME`,
//! `MSG_QUERY_NEXT_MAIL_TIME`, `CMSG_BATTLEFIELD_STATUS` and `CMSG 0x296`, and for a dead body
//! `CMSG_REPOP_REQUEST` last; a worldport's fresh create sends all five again.

use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{
    ClientCommand, EnteredWorldMessage, Guid, NetCommands, NetStatus, SelfGuid, ServerWallClock,
    WorldEnterCascadeMessage,
};

const ME: u64 = 0x0000_0000_0000_0042;

/// The self-create edge and the cascade's four senders, in the order the client schedules them
/// (`the_client_schedules_the_sends_in_the_reference_order` pins that), logged in as [`ME`].
fn app() -> (App, Receiver<ClientCommand>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut app = App::new();
    app.insert_resource(NetCommands(tx))
        .insert_resource(SelfGuid(Some(ME)))
        .insert_resource(NetStatus {
            connected: true,
            last_reason: None,
        })
        .init_resource::<ServerWallClock>()
        .init_resource::<crate::ui_mail::MailPending>()
        .init_resource::<crate::ui_mail::MailOpen>()
        .init_resource::<crate::ui_battlefield::Battlefield>()
        .init_resource::<crate::ui_battlefield::BattlefieldLeash>()
        .init_resource::<crate::ui_dialog_verbs::MeetingStone>()
        .add_message::<EnteredWorldMessage>()
        .add_message::<WorldEnterCascadeMessage>()
        .add_systems(
            Update,
            (
                super::tag_self_player,
                super::enter_world_on_self_create,
                crate::net::send_query_time,
                crate::ui_mail::send_query_next_mail_time_on_enter,
                crate::ui_battlefield::reset_on_world_enter,
                crate::ui_dialog_verbs::meeting_stone_enter_world,
                crate::death::repop_on_world_enter,
            )
                .chain(),
        );
    (app, rx)
}

/// The login edge `connected` writes at `CMSG_PLAYER_LOGIN`.
fn log_in(app: &mut App) {
    app.world_mut().write_message(EnteredWorldMessage {
        tutorial_flags: None,
    });
}

/// What `object_create` spawns for our own create block.
fn self_create(app: &mut App) -> Entity {
    app.world_mut().spawn(Guid(ME)).id()
}

fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| match c {
            ClientCommand::SetActiveMover { guid } => format!("SetActiveMover {guid:#x}"),
            other => format!("{other:?}"),
        })
        .collect()
}

fn the_five() -> Vec<String> {
    vec![
        format!("SetActiveMover {ME:#x}"),
        "QueryTime".into(),
        "QueryNextMailTime".into(),
        "BattlefieldStatusRequest".into(),
        "MeetingStoneStatusQuery".into(),
    ]
}

/// cmangos drops each of the five as "the player has not logged in yet" when it beats the load,
/// so the login edge sends none, and the hourly clock resync (the clock never answered, so stale)
/// waits for the create too.
#[test]
fn the_login_edge_alone_sends_nothing() {
    let (mut app, rx) = app();
    log_in(&mut app);
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(sent(&rx), Vec::<String>::new());
}

#[test]
fn the_self_create_sends_the_five_in_the_reference_order() {
    let (mut app, rx) = app();
    log_in(&mut app);
    app.update();
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_five());
    // The same body on later frames is no new create.
    app.update();
    app.update();
    assert_eq!(sent(&rx), Vec::<String>::new());
}

/// The cascade ends by releasing a dead body (`0x490a2d`-`0x490a38`, `0x490a3c` → `0x5e0ae0`):
/// `CMSG_REPOP_REQUEST` after the cascade's other sends, which vmangos answers by making the
/// player a ghost at the graveyard (`MiscHandler.cpp:48-67`). The same cascade runs on a `/reload`
/// (`0x490168`); a live body sends none.
#[test]
fn a_dead_body_is_released_at_the_end_of_the_cascade() {
    use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
    use benilla_protocol::messages::ObjectType;
    use benilla_protocol::ObjectFields;
    let body = |health| {
        crate::net::ObjectStore(
            ObjectFields::from_pairs(&[(FIELD_UNIT_HEALTH, health), (FIELD_UNIT_MAXHEALTH, 100)])
                .into_created(ObjectType::Player),
        )
    };
    let enter = |health| {
        let (mut client, rx) = app();
        log_in(&mut client);
        let me = self_create(&mut client);
        client.world_mut().entity_mut(me).insert(body(health));
        client.update();
        (client, rx)
    };
    let (mut client, rx) = enter(0);
    let mut released = the_five();
    released.push("RepopRequest".into());
    assert_eq!(sent(&rx), released);

    // A `/reload` re-enters the cascade with the body still dead.
    client.world_mut().write_message(WorldEnterCascadeMessage);
    client.update();
    assert_eq!(sent(&rx).last().map(String::as_str), Some("RepopRequest"));

    let (_, rx) = enter(100);
    assert_eq!(sent(&rx), the_five(), "alive: no release");
}

/// A cross-map worldport purges the streamed world and the server creates us afresh
/// (`0x401bc0` rebuilds the object manager; the create takes the login's route).
#[test]
fn a_worldport_create_sends_the_five_again() {
    let (mut app, rx) = app();
    log_in(&mut app);
    let first = self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_five());

    app.world_mut().despawn(first);
    app.update();
    assert_eq!(sent(&rx), Vec::<String>::new(), "nothing between the two");
    self_create(&mut app);
    app.update();
    assert_eq!(sent(&rx), the_five());
}

/// The production schedule: the self-create edge, then the time, mail, battlefield and meeting
/// stone sends (`0x4909a1`, `0x4909f6`, `0x4909fb`, `0x490a14`); and a `/reload` frame's sweep
/// drops the stone's text before its cascade resets it (`0x490c20`, then `0x490168`).
#[test]
fn the_client_schedules_the_sends_in_the_reference_order() {
    use crate::test_support::runs_before;
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    assert!(runs_before(
        &mut app,
        super::tag_self_player,
        super::enter_world_on_self_create
    ));
    assert!(runs_before(
        &mut app,
        super::enter_world_on_self_create,
        crate::net::send_query_time
    ));
    assert!(runs_before(
        &mut app,
        crate::net::send_query_time,
        crate::ui_mail::send_query_next_mail_time_on_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_mail::send_query_next_mail_time_on_enter,
        crate::ui_battlefield::reset_on_world_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::ui_battlefield::reset_on_world_enter,
        crate::ui_dialog_verbs::meeting_stone_enter_world
    ));
    // The release last (`0x490a3c`), after the pet ask in the unit feed (`0x4909c9`), then the
    // arrival's death signal (`0x5deb65`).
    assert!(runs_before(
        &mut app,
        crate::ui_dialog_verbs::meeting_stone_enter_world,
        crate::death::repop_on_world_enter
    ));
    assert!(crate::ui_unit::feed_units_runs_before(
        &mut app,
        crate::death::repop_on_world_enter
    ));
    assert!(runs_before(
        &mut app,
        crate::death::repop_on_world_enter,
        crate::death::feed_death
    ));
}
