//! The login cycle, [`super::run`], against a fake realmd and a fake world server on loopback
//! ([`super::fake`]): the real logon, the logon park, world handshake and roster, driven through
//! the parks as the app does.

use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use std::thread;

use benilla_protocol::messages::opcode;
use benilla_protocol::{RealmInfo, SessionEvent, WorldWriter};

use super::fake::{self, FakeRealm, WAIT};
use super::{run, Cycle, LoginRequest, Parks, PingClock};
use crate::net::{CharRequest, RealmRequest};

const REALM: &str = "Fake";

/// The next event, skipping the stage announcements; a login failure fails the test by name.
fn next_event(events: &crossbeam_channel::Receiver<SessionEvent>) -> SessionEvent {
    loop {
        match events.recv_timeout(WAIT).expect("the cycle went quiet") {
            SessionEvent::LoginStage { .. } => continue,
            SessionEvent::LoginFailed { reason, .. } => panic!("the login failed: {reason}"),
            ev => return ev,
        }
    }
}

/// A dial of `realm` asked under generation 0, the cycles' start.
fn enter(realm: RealmInfo) -> RealmRequest {
    RealmRequest::Enter {
        realm,
        generation: 0,
    }
}

/// The next realm list, its realms and whether the query failed.
fn next_list(events: &crossbeam_channel::Receiver<SessionEvent>) -> (Vec<RealmInfo>, bool) {
    match next_event(events) {
        SessionEvent::RealmList { realms, failed } => (realms, failed),
        other => panic!("expected the realm list, got {other:?}"),
    }
}

/// The reference builds `CMSG_CHAR_CREATE` in one place (`0x5aac50`), reached only from the glue's
/// `CreateCharacter`, which the stock glue calls only from the create screen's Okay; so a login to
/// an account with no characters parks at select on the empty list and creates nothing.
#[test]
fn an_empty_account_parks_at_select_without_creating_a_character() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);

    let (events_tx, events) = crossbeam_channel::unbounded();
    let (writer_tx, _writer_rx) = crossbeam_channel::unbounded::<WorldWriter>();
    let (login_tx, login_rx) = crossbeam_channel::unbounded();
    let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
    let (pick_tx, pick_rx) = crossbeam_channel::unbounded();
    let cycle = thread::spawn(move || {
        let parks = Parks {
            login_rx,
            realm_rx,
            pick_rx,
        };
        let ended = run(
            &events_tx,
            &writer_tx,
            &parks,
            &AtomicU64::new(0),
            &Mutex::new(PingClock::default()),
            &mut Default::default(),
        );
        matches!(ended, Ok(Cycle::Repark))
    });

    login_tx
        .send(LoginRequest {
            user: fake::USER.into(),
            pass: fake::PASS.into(),
            host: realmd.addr.clone(),
            generation: 0,
        })
        .unwrap();
    let (realms, failed) = next_list(&events);
    assert!(!failed);
    assert_eq!(realms[0].name, REALM);
    realm_tx.send(enter(realms[0].clone())).unwrap();
    match next_event(&events) {
        SessionEvent::CharacterList { characters, .. } => assert!(
            characters.is_empty(),
            "the roster at select must be the account's, empty: {characters:?}"
        ),
        other => panic!("expected the roster at select, got {other:?}"),
    }
    // Select's Back ends the cycle, so the socket is done and its log complete.
    pick_tx.send(CharRequest::Abandon).unwrap();
    assert!(cycle.join().unwrap(), "Back returns to the login park");
    let sent = world.sent.lock().unwrap().clone();
    assert!(
        !sent.contains(&opcode::CMSG_CHAR_CREATE),
        "the login sent CMSG_CHAR_CREATE on its own: {sent:#06x?}"
    );
}

/// The logon park takes the next login (#833): closing the list ends the logon and the thread
/// parks for credentials; a login while the list stands supersedes it. Either way the next login
/// reaches realmd and its list.
#[test]
fn the_next_login_is_accepted_after_the_realm_list_closes_or_stands() {
    let (realmd, _) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();

    fake::login(&cycles, &realmd.addr);
    assert!(!next_list(&cycles.events).1);
    // `RealmListDialogCancelled` on the login screen: `CLoginMgr::Cancel`.
    cycles.realm.send(RealmRequest::Close).unwrap();
    fake::login(&cycles, &realmd.addr);
    let (realms, failed) = next_list(&cycles.events);
    assert!(!failed && realms[0].name == REALM);
    assert_eq!(realmd.logons(), 2, "the second login logged on");

    // The close button leaves the list's link up; Login again supersedes it.
    fake::login(&cycles, &realmd.addr);
    assert!(!next_list(&cycles.events).1);
    assert_eq!(realmd.logons(), 3);
    let deadline = std::time::Instant::now() + WAIT;
    while realmd.open() > 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "the superseded links stay open: {}",
            realmd.open()
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// A world attempt that fails returns to the logon park with the realmd link up, as the
/// reference's state-2 failure arm drops only the world socket: the list is queried on the same
/// link and another realm dialled, with no second logon.
#[test]
fn a_failed_world_dial_keeps_the_logon_for_the_next_realm() {
    let (realmd, _) = fake::servers(|world| {
        vec![
            FakeRealm::new("Down", &fake::dead_address()),
            FakeRealm::new(REALM, world),
        ]
    });
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    let down = realms.iter().find(|r| r.name == "Down").unwrap().clone();
    let up = realms.iter().find(|r| r.name == REALM).unwrap().clone();

    cycles.realm.send(enter(down)).unwrap();
    loop {
        match cycles
            .events
            .recv_timeout(WAIT)
            .expect("the dial went quiet")
        {
            SessionEvent::LoginFailed { refusal, dial, .. } => {
                assert!(
                    refusal.is_none() && dial.is_none(),
                    "a world transport failure"
                );
                break;
            }
            SessionEvent::LoginStage { .. } => {}
            other => panic!("expected the dial to fail, got {other:?}"),
        }
    }
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    assert!(!next_list(&cycles.events).1, "the link answers the query");
    assert_eq!(realmd.lists(), 2);
    cycles.realm.send(enter(up)).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(realmd.logons(), 1, "one logon throughout");
}

/// With no logon a realm list query fails at once (`REALM_LIST_FAILED`); the dial and the close
/// have nothing to act on.
#[test]
fn a_query_without_a_logon_fails_at_once() {
    let cycles = fake::spawn_cycles();
    cycles.realm.send(RealmRequest::Close).unwrap();
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    let (realms, failed) = next_list(&cycles.events);
    assert!(failed && realms.is_empty());
}

/// A query on a link realmd has closed dials again and answers (`0x5b2200`), so a list left up
/// past vmangos's `MaxSessionDuration` still refreshes.
#[test]
fn a_query_on_a_closed_link_dials_again() {
    let (realmd, _) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    assert!(!next_list(&cycles.events).1);
    realmd.drop_links();
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    let (realms, failed) = next_list(&cycles.events);
    assert!(!failed, "the re-dial answers");
    assert_eq!(realms[0].name, REALM);
    assert_eq!(realmd.logons(), 2);
}

/// A Cancel after the dial was asked ends it, however early it lands: the dial carries the
/// generation the app held when it asked, so the thread drops it and stays at the logon park.
#[test]
fn a_cancel_after_the_dial_was_asked_drops_it() {
    let (realmd, _) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    // The dial was asked under 0, and the Cancel's bump landed before the thread read it.
    cycles
        .abandon
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    assert!(
        !next_list(&cycles.events).1,
        "the logon park answers, with no roster before it"
    );
}

/// A server that lists no realm gets its empty list published and the logon parked, and the
/// screens show it empty, as the reference does.
#[test]
fn an_empty_realm_list_is_published_and_parked() {
    let (realmd, _) = fake::servers(|_| Vec::new());
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, failed) = next_list(&cycles.events);
    assert!(realms.is_empty() && !failed);
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    assert!(!next_list(&cycles.events).1, "parked at the logon park");
    assert_eq!(realmd.lists(), 2);
}

/// Change Realm with the world up drops the session first (`0x46b22b`) and returns to the logon
/// park, realmd still up: the world socket closes before any query.
#[test]
fn leave_drops_the_world_session_and_keeps_the_logon() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(world.open(), 1);
    cycles.realm.send(RealmRequest::Leave).unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while world.open() != 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the world session is still open"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    assert!(!next_list(&cycles.events).1);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(realmd.logons(), 1, "one logon throughout");
}
