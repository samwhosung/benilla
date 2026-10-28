//! The world-enter sends wait for our own player's create, as the reference's do (`0x5dea50`):
//! none goes out on the login edge alone, which fires before the server seats the player; the
//! create's arrival claims the mover inside its packet (`net::objects`'s arrival tests), and its
//! cascade sends `CMSG_QUERY_TIME`, `MSG_QUERY_NEXT_MAIL_TIME`, `CMSG_BATTLEFIELD_STATUS` and
//! `CMSG 0x296`, and for a dead body `CMSG_REPOP_REQUEST` last; each cascade sends all four again.

use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{
    ClientCommand, EnteredWorldMessage, NetCommands, NetStatus, QueryTimeAsks, SelfGuid,
    ServerWallClock,
};

const ME: u64 = 0x0000_0000_0000_0042;

/// The cascade's senders' state and the hourly resync, logged in as [`ME`].
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
        .init_resource::<QueryTimeAsks>()
        .init_resource::<crate::ui_mail::MailPending>()
        .init_resource::<crate::ui_mail::MailOpen>()
        .init_resource::<crate::ui_battlefield::Battlefield>()
        .init_resource::<crate::ui_battlefield::BattlefieldLeash>()
        .init_resource::<crate::ui_dialog_verbs::MeetingStone>()
        .add_message::<EnteredWorldMessage>()
        .add_systems(Update, crate::net::send_query_time);
    (app, rx)
}

/// The login edge `connected` writes at `CMSG_PLAYER_LOGIN`.
fn log_in(app: &mut App) {
    app.world_mut().write_message(EnteredWorldMessage {
        tutorial_flags: None,
    });
}

fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter().map(|c| format!("{c:?}")).collect()
}

fn the_four() -> Vec<String> {
    vec![
        "QueryTime".into(),
        "QueryNextMailTime".into(),
        "BattlefieldStatusRequest".into(),
        "MeetingStoneStatusQuery".into(),
    ]
}

/// cmangos drops each of the four as "the player has not logged in yet" when it beats the load,
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

/// The cascade's sends in its call order (`0x4909a1`, `0x4909f6`, `0x4909fb`, `0x490a14`), and no
/// later frame sends them again.
#[test]
fn the_cascade_sends_the_four_in_the_reference_order() {
    let (mut app, rx) = app();
    log_in(&mut app);
    app.update();
    crate::ui_world_enter::run_cascade(app.world_mut());
    app.update();
    assert_eq!(sent(&rx), the_four());
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
        client
            .world_mut()
            .spawn((crate::net::Guid(ME), crate::net::SelfPlayer, body(health)));
        crate::ui_world_enter::run_cascade(client.world_mut());
        (client, rx)
    };
    let (mut client, rx) = enter(0);
    let mut released = the_four();
    released.push("RepopRequest".into());
    assert_eq!(sent(&rx), released);

    // A `/reload` re-enters the cascade with the body still dead.
    crate::ui_world_enter::run_cascade(client.world_mut());
    assert_eq!(sent(&rx).last().map(String::as_str), Some("RepopRequest"));

    let (_, rx) = enter(100);
    assert_eq!(sent(&rx), the_four(), "alive: no release");
}
