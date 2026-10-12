//! The login cycle, [`super::run`], against a fake realmd and a fake world server on loopback
//! ([`super::fake`]): the real logon, the logon park, world handshake and roster, driven through
//! the parks as the app does.

use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use std::thread;

use benilla_protocol::messages::opcode;
use benilla_protocol::{RealmInfo, RosterCause, SessionEvent, WorldWriter};

use super::fake::{self, FakeRealm, WAIT};
use super::{run, Cycle, LoginRequest, Parks, PingClock};
use crate::net::{CharRequest, ParkRequest, RealmRequest};

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

/// The next event past the world authentication that opens every park: the roster the native
/// policy fetches.
fn next_roster(events: &crossbeam_channel::Receiver<SessionEvent>) -> SessionEvent {
    loop {
        match next_event(events) {
            SessionEvent::WorldAuthenticated { .. } => continue,
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
    let (park_tx, park_rx) = crossbeam_channel::unbounded();
    let cycle = thread::spawn(move || {
        let parks = Parks {
            login_rx,
            park_rx,
            screen_asks_roster: Default::default(),
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
    park_tx
        .send(ParkRequest::Realm(enter(realms[0].clone())))
        .unwrap();
    assert!(
        matches!(next_event(&events), SessionEvent::WorldAuthenticated { .. }),
        "the world connect completes with the authentication"
    );
    match next_event(&events) {
        SessionEvent::CharacterList {
            characters, cause, ..
        } => {
            assert!(
                characters.is_empty(),
                "the roster at select must be the account's, empty: {characters:?}"
            );
            assert_eq!(
                cause,
                RosterCause::Park,
                "the native screens' own enumeration"
            );
        }
        other => panic!("expected the roster at select, got {other:?}"),
    }
    // Select's Back ends the cycle, so the socket is done and its log complete.
    park_tx
        .send(ParkRequest::Char(CharRequest::Abandon))
        .unwrap();
    assert!(cycle.join().unwrap(), "Back returns to the login park");
    let sent = world.sent.lock().unwrap().clone();
    assert!(
        !sent.contains(&opcode::CMSG_CHAR_CREATE),
        "the login sent CMSG_CHAR_CREATE on its own: {sent:#06x?}"
    );
    assert_eq!(
        sent.iter()
            .filter(|&&op| op == opcode::CMSG_CHAR_ENUM)
            .count(),
        1,
        "the native policy enumerates once on reaching the park: {sent:#06x?}"
    );
}

/// Under the stock screen's policy the park fetches no roster: the world connect ends with the
/// authentication, and the one `CMSG_CHAR_ENUM` goes out when the screen asks, its reply tagged as
/// asked for (`0x5aabb0`).
#[test]
fn the_stock_policy_enumerates_only_when_asked() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::WorldAuthenticated { .. }
    ));
    assert!(
        cycles
            .events
            .recv_timeout(std::time::Duration::from_millis(400))
            .is_err(),
        "the park sent a roster nobody asked for"
    );
    assert_eq!(world.enumerations(), 0);
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::CharacterList { cause, .. } => assert_eq!(cause, RosterCause::Asked),
        other => panic!("expected the asked-for roster, got {other:?}"),
    }
    assert_eq!(world.enumerations(), 1);
    cycles.pick.send(CharRequest::Abandon).unwrap();
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
        next_roster(&cycles.events),
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
        next_roster(&cycles.events),
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
        next_roster(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(realmd.logons(), 1, "one logon throughout");
}

/// The character park's requests and the realm list's are met in the order the app made them, on
/// one channel. Two roster asks and a `Leave` queue while the IO thread is held in an earlier
/// ask's slow reply: both asks go out on the old link before it leaves, none is dropped, and the
/// leave is honored after them. The reference sends `CMSG_CHAR_ENUM` at the verb (`0x46b880`) and
/// `ConnectToRealm` only then closes the link (`0x46b22b`).
#[test]
fn the_park_serves_its_requests_in_the_order_they_were_made() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.delay_rosters(std::time::Duration::from_millis(15));
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::WorldAuthenticated { .. }
    ));
    // The first ask holds the thread; the second and the leave queue behind it together.
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    cycles.realm.send(RealmRequest::Leave).unwrap();
    for ask in 1..=2 {
        match next_event(&cycles.events) {
            SessionEvent::CharacterList { cause, .. } => {
                assert_eq!(cause, RosterCause::Asked, "ask {ask}")
            }
            other => panic!("expected the asked-for roster, got {other:?}"),
        }
    }
    assert_eq!(world.enumerations_in(0), 2);
    wait_until(|| world.open() == 0, "the leave closes the world session");
}

/// Wait for `done`, up to the tests' bound.
fn wait_until(done: impl Fn() -> bool, what: &str) {
    let deadline = std::time::Instant::now() + WAIT;
    while !done() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// A session's authentication and its roster carry the same epoch and the attempt's generation; a
/// realm change dials a new session, which has a new epoch.
#[test]
fn a_world_session_has_one_epoch_and_a_realm_change_a_new_one() {
    let (realmd, _world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    let mut seen = Vec::new();
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    for round in 0..2 {
        let (auth, listed) = match (next_event(&cycles.events), next_event(&cycles.events)) {
            (
                SessionEvent::WorldAuthenticated {
                    epoch, generation, ..
                },
                SessionEvent::CharacterList {
                    epoch: listed,
                    generation: listed_generation,
                    ..
                },
            ) => ((epoch, generation), (listed, listed_generation)),
            other => panic!("round {round}: expected the auth and the roster, got {other:?}"),
        };
        assert_eq!(auth, listed, "round {round}: one session, one epoch");
        assert_eq!(auth.1, 0, "the attempt's generation");
        seen.push(auth.0);
        if round == 0 {
            // Change Realm at the park: a new dial, a new session.
            cycles.realm.send(enter(realms[0].clone())).unwrap();
        }
    }
    assert_ne!(seen[0], seen[1], "a new session has a new epoch");
}

/// A server that wants Warden answers the stock screen's first ask with `SMSG_WARDEN_DATA`: the
/// login fails for good with the server's words, as it does at the park under the native policy,
/// not as a lost stream.
#[test]
fn a_warden_demand_at_the_asked_roster_is_a_terminal_login_failure() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.demand_warden();
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::WorldAuthenticated { .. }
    ));
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    loop {
        match cycles.events.recv_timeout(WAIT).expect("went quiet") {
            SessionEvent::LoginFailed { terminal, .. } => {
                assert!(terminal);
                break;
            }
            SessionEvent::LoginStage { .. } => {}
            other => panic!("expected the terminal login failure, got {other:?}"),
        }
    }
    // The world socket is closed and the realmd link kept, as after any failed world dial.
    wait_until(|| world.open() == 0, "the world socket closes");
    assert_eq!(realmd.open(), 1, "the realmd link is kept");
}

/// A roster ask that fails on the stream (the server cut the connection) tells the screen its
/// query failed (`0x5ab300`'s failure leg) before the session ends as a dropped socket does.
#[test]
fn a_roster_ask_cut_on_the_stream_reports_its_failure_then_the_loss() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.cut_on_roster_ask();
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    let epoch = match next_event(&cycles.events) {
        SessionEvent::WorldAuthenticated { epoch, .. } => epoch,
        other => panic!("expected the authentication, got {other:?}"),
    };
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::RosterFailed { epoch: failed } => assert_eq!(failed, epoch),
        other => panic!("expected the failed roster ask, got {other:?}"),
    }
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::Disconnected { .. }
    ));
}

/// After a create the native policy re-emits the roster before the result; the stock screen asks
/// again itself (`0x46c1a7`), so the IO thread sends it nothing and enumerates nothing.
#[test]
fn only_the_native_policy_re_enumerates_after_a_create() {
    let req = benilla_protocol::CharCreateReq {
        name: "Newone".into(),
        race: 1,
        class: 1,
        gender: 0,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
    };
    // Native: the park's roster, then the create's, tagged as changed, then the result.
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_roster(&cycles.events),
        SessionEvent::CharacterList {
            cause: RosterCause::Park,
            ..
        }
    ));
    cycles.pick.send(CharRequest::Create(req.clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList {
            cause: RosterCause::Changed,
            ..
        }
    ));
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharActionResult { .. }
    ));
    assert_eq!(world.enumerations(), 2);

    // Stock: the result alone.
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::WorldAuthenticated { .. }
    ));
    cycles.pick.send(CharRequest::Create(req)).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharActionResult { .. }
    ));
    assert_eq!(world.enumerations(), 0);
}
