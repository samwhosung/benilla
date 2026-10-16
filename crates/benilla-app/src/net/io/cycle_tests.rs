//! The login cycle, [`super::run`], against a fake realmd and a fake world server on loopback
//! ([`super::fake`]): the real logon, the logon park, world handshake and roster, driven through
//! the parks as the app does.

use std::sync::atomic::AtomicU64;
use std::sync::Mutex;
use std::thread;

use benilla_protocol::messages::opcode;
use benilla_protocol::{RealmInfo, RosterCause, SessionEvent};

use super::fake::{self, FakeRealm, WAIT};
use super::{run, Cycle, LoginRequest, Parks, PingClock, WriterSwap};
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
    let (writer_tx, _writer_rx) = crossbeam_channel::unbounded::<WriterSwap>();
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
            locale: benilla_protocol::auth::LOCALE_TAG_EN_US,
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

/// The park serves a rename in place: the reply goes out as it came, with no roster asked, and
/// the park's own copy of the roster takes the new name, so the world entry names the character
/// by it. A refused rename changes nothing.
#[test]
fn the_park_serves_a_rename_and_follows_it() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let mut bravo = crate::glue_session::roster::test_character(GUID, "Bravo");
    bravo.flags = benilla_protocol::CHARACTER_FLAG_RENAME;
    world.set_roster(&[bravo]);
    let cycles = fake::spawn_cycles_asking();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::WorldAuthenticated { .. }
    ));
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));

    world.answer_renames(0x2f);
    cycles
        .pick
        .send(CharRequest::Rename {
            guid: GUID,
            name: "Newbie".into(),
        })
        .unwrap();
    match next_event(&cycles.events) {
        SessionEvent::CharRenamed { result, renamed } => {
            assert_eq!((result, renamed), (0x2f, None))
        }
        other => panic!("expected the rename's reply, got {other:?}"),
    }
    world.answer_renames(0);
    cycles
        .pick
        .send(CharRequest::Rename {
            guid: GUID,
            name: "Newbie".into(),
        })
        .unwrap();
    match next_event(&cycles.events) {
        SessionEvent::CharRenamed { result, renamed } => {
            assert_eq!((result, renamed), (0, Some((GUID, "Newbie".to_string()))));
        }
        other => panic!("expected the rename's reply, got {other:?}"),
    }
    assert_eq!(world.enumerations(), 1, "no roster behind the rename");
    cycles.pick.send(CharRequest::Enter(GUID)).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::Connected { name, .. } => assert_eq!(name, "Newbie"),
        other => panic!("expected the world entry, got {other:?}"),
    }
}

/// Logs in, dials the one realm and picks `GUID` at character select: the session's epoch once
/// the entry is announced. Under the native policy the park's roster comes first.
fn enter_world(cycles: &fake::Cycles, realmd: &fake::Realmd, asking: bool) -> u64 {
    fake::login(cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    let epoch = match next_event(&cycles.events) {
        SessionEvent::WorldAuthenticated { epoch, .. } => epoch,
        other => panic!("expected the authentication, got {other:?}"),
    };
    if !asking {
        assert!(matches!(
            next_event(&cycles.events),
            SessionEvent::CharacterList {
                cause: RosterCause::Park,
                ..
            }
        ));
    }
    cycles.pick.send(CharRequest::Enter(GUID)).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::Connected { self_guid, .. } => assert_eq!(self_guid, GUID),
        other => panic!("expected the world entry, got {other:?}"),
    }
    epoch
}

/// The character every entry picks.
const GUID: u64 = 0x2a;

/// The world session's end after `ended`: the event itself, then the teardown edge with the link
/// kept, a `Disconnected` that ends in a logout.
fn expect_back(
    events: &crossbeam_channel::Receiver<SessionEvent>,
    ended: fn(&SessionEvent) -> bool,
) {
    let ev = next_event(events);
    assert!(ended(&ev), "expected the world session's end, got {ev:?}");
    match next_event(events) {
        SessionEvent::Disconnected { end, .. } => {
            assert_eq!(end, benilla_protocol::SessionEnd::LoggedOut)
        }
        other => panic!("expected the teardown, got {other:?}"),
    }
}

/// A logout returns to character select on the world link it entered by (`SMSG_LOGOUT_COMPLETE`
/// → `0x5aaeb0` → `0x401ee0(1, 1, 0)`, then `CharacterSelect_OnShow` → `0x46b880`): one more
/// `CMSG_CHAR_ENUM` on the same session, with no realmd logon and no second `CMSG_AUTH_SESSION`.
/// The native screens get the park's enumeration; the stock screen's comes when it asks.
#[test]
fn a_logout_returns_to_character_select_on_the_same_link() {
    for asking in [false, true] {
        let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
        let cycles = if asking {
            fake::spawn_cycles_asking()
        } else {
            fake::spawn_cycles()
        };
        let epoch = enter_world(&cycles, &realmd, asking);
        cycles
            .commands
            .send(crate::net::ClientCommand::Logout)
            .unwrap();
        expect_back(&cycles.events, |ev| matches!(ev, SessionEvent::LoggedOut));
        if asking {
            assert!(
                cycles
                    .events
                    .recv_timeout(std::time::Duration::from_millis(400))
                    .is_err(),
                "the park sent a roster the stock screen did not ask for"
            );
            cycles.pick.send(CharRequest::Enumerate).unwrap();
        }
        match next_event(&cycles.events) {
            SessionEvent::CharacterList {
                cause, epoch: on, ..
            } => {
                let want = if asking {
                    RosterCause::Asked
                } else {
                    RosterCause::Park
                };
                assert_eq!(cause, want, "asking: {asking}");
                assert_eq!(
                    on, epoch,
                    "the roster comes on the link the world was entered by"
                );
            }
            other => panic!("expected the roster at select, got {other:?}"),
        }
        let before = usize::from(!asking);
        assert_eq!(
            world.enumerations_in(0),
            before + 1,
            "one CMSG_CHAR_ENUM after the logout, on the first session"
        );
        assert_eq!(world.authentications(), 1, "no second CMSG_AUTH_SESSION");
        assert_eq!(world.open(), 1, "the world link is kept");
        assert_eq!(realmd.logons(), 1, "no realmd logon");
    }
}

/// A refused character login returns the same way (`SMSG_CHARACTER_LOGIN_FAILED` → `0x5aad70` →
/// `0x401ee0(1, 1, 1)`, then the dialog's `StatusDialogClick` → `0x46b880`): the session is kept,
/// and the native policy enumerates once on it.
#[test]
fn a_refused_login_returns_to_character_select_on_the_same_link() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.refuse_logins();
    let cycles = fake::spawn_cycles();
    let epoch = enter_world(&cycles, &realmd, false);
    expect_back(&cycles.events, |ev| {
        matches!(ev, SessionEvent::CharacterLoginFailed { result: 1 })
    });
    match next_event(&cycles.events) {
        SessionEvent::CharacterList {
            cause, epoch: on, ..
        } => {
            assert_eq!(cause, RosterCause::Park);
            assert_eq!(on, epoch);
        }
        other => panic!("expected the roster at select, got {other:?}"),
    }
    assert_eq!(world.enumerations_in(0), 2);
    assert_eq!(world.authentications(), 1, "no second CMSG_AUTH_SESSION");
    assert_eq!(realmd.logons(), 1, "no realmd logon");
    // Still parked on the link: the next pick goes out on it.
    cycles.pick.send(CharRequest::Enter(GUID)).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::Connected { .. }
    ));
    assert_eq!(world.authentications(), 1);
}

/// `EnterWorld` closes the realmd link (`CLoginMgr::Cancel`, `0x46b70c` → `0x5b3320`); it stays up
/// at character select until then.
#[test]
fn entering_the_world_closes_the_realmd_link() {
    let (realmd, _world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    fake::login(&cycles, &realmd.addr);
    let (realms, _) = next_list(&cycles.events);
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    assert!(matches!(
        next_roster(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(
        realmd.open(),
        1,
        "the realmd link is up at character select"
    );
    cycles.pick.send(CharRequest::Enter(GUID)).unwrap();
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::Connected { .. }
    ));
    wait_until(|| realmd.open() == 0, "the entry closes the realmd link");
}

/// Change Realm after a logout finds the realmd link closed since Enter World and dials it again
/// (`0x46b8d0` → `0x5ab840` → `0x5b2200`), then dials the realm with the new logon's key: a second
/// world session.
#[test]
fn change_realm_after_a_logout_dials_realmd_again() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    let cycles = fake::spawn_cycles();
    let epoch = enter_world(&cycles, &realmd, false);
    cycles
        .commands
        .send(crate::net::ClientCommand::Logout)
        .unwrap();
    expect_back(&cycles.events, |ev| matches!(ev, SessionEvent::LoggedOut));
    assert!(matches!(
        next_event(&cycles.events),
        SessionEvent::CharacterList { .. }
    ));
    assert_eq!(realmd.logons(), 1);
    cycles.realm.send(RealmRequest::Refresh).unwrap();
    let (realms, failed) = next_list(&cycles.events);
    assert!(!failed, "the re-dial answers");
    assert_eq!(realmd.logons(), 2, "the query dialled realmd again");
    cycles.realm.send(enter(realms[0].clone())).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::WorldAuthenticated { epoch: next, .. } => assert_ne!(next, epoch),
        other => panic!("expected the new session's authentication, got {other:?}"),
    }
    assert_eq!(
        world.authentications(),
        2,
        "the change dials a new world session"
    );
}

/// Back from the world the link still carries what the world session left in flight: a pong for
/// a ping sent before the writer came back, a reply vmangos serves an authenticated session. A
/// packet of a known opcode whose body does not parse is skipped at the park, as the world stream
/// skips it, and the roster behind it is read; the link is not lost over it.
#[test]
fn a_packet_left_in_flight_by_the_world_does_not_cost_the_link() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.trail_logouts_with(vec![
        // An `SMSG_PONG` with no sequence: a known opcode that does not parse.
        (opcode::SMSG_PONG, Vec::new()),
        (opcode::SMSG_PONG, 7u32.to_le_bytes().to_vec()),
    ]);
    let cycles = fake::spawn_cycles();
    let epoch = enter_world(&cycles, &realmd, false);
    cycles
        .commands
        .send(crate::net::ClientCommand::Logout)
        .unwrap();
    expect_back(&cycles.events, |ev| matches!(ev, SessionEvent::LoggedOut));
    match next_event(&cycles.events) {
        SessionEvent::CharacterList {
            cause, epoch: on, ..
        } => {
            assert_eq!(cause, RosterCause::Park);
            assert_eq!(on, epoch);
        }
        other => panic!("expected the roster past the leftovers, got {other:?}"),
    }
    assert_eq!(world.open(), 1, "the world link is kept");
}

/// The write thread holds a lost link's dead writer until the next entry installs a new one, and
/// a command can be ready in the same wake-up as that install. Every pending swap is applied
/// before a command is judged, so the command goes out on the new link. The write thread is held
/// inside the first install (on the ping clock's lock) while the second install and the command
/// queue, so both are ready together; without the drain the pick between them is a coin toss,
/// and twelve rounds catch it.
#[test]
fn a_command_ready_with_an_install_goes_out_on_the_new_writer() {
    use super::{writer_loop, WriterSwap};
    use crate::net::ClientCommand;
    const KEY: [u8; benilla_srp::SESSION_KEY_LENGTH] = [9; benilla_srp::SESSION_KEY_LENGTH];
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (keys_tx, keys_rx) = crossbeam_channel::unbounded();
    keys_tx.send(KEY).unwrap();
    let world = fake::world(listener, keys_rx);
    let writer = || {
        benilla_protocol::WorldSession::connect(&addr, fake::USER, KEY)
            .unwrap()
            .into_split()
            .unwrap()
    };

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<ClientCommand>();
    let (swap_tx, swap_rx) = crossbeam_channel::unbounded::<WriterSwap>();
    let clock = std::sync::Arc::new(Mutex::new(PingClock::default()));
    let thread_clock = std::sync::Arc::clone(&clock);
    thread::spawn(move || writer_loop(&cmd_rx, swap_rx, &thread_clock));

    let played_in = |session: usize| {
        world
            .session_packets
            .lock()
            .unwrap()
            .get(session)
            .is_some_and(|packets| {
                packets
                    .iter()
                    .any(|p| p[2..4] == opcode::CMSG_PLAYED_TIME.to_le_bytes())
            })
    };
    let mut readers = Vec::new();
    for round in 0..12 {
        let (old_reader, old) = writer();
        let (new_reader, new) = writer();
        readers.extend([old_reader, new_reader]);
        let held = clock.lock().unwrap();
        swap_tx
            .send(WriterSwap::Install {
                writer: old,
                fresh_link: true,
            })
            .unwrap();
        // The write thread has taken the old writer and waits on the clock inside the install.
        thread::sleep(std::time::Duration::from_millis(5));
        swap_tx
            .send(WriterSwap::Install {
                writer: new,
                fresh_link: true,
            })
            .unwrap();
        cmd_tx.send(ClientCommand::PlayedTime).unwrap();
        drop(held);
        let (old_index, new_index) = (2 * round, 2 * round + 1);
        wait_until(
            || played_in(old_index) || played_in(new_index),
            "the command went out",
        );
        assert!(
            played_in(new_index) && !played_in(old_index),
            "round {round}: the command went out on the writer being replaced"
        );
    }
}

/// Under the stock screens a refused login returns the same way, and the roster comes only when
/// the screen asks (in the reference from the refusal dialog's `StatusDialogClick`, `0x46b767` →
/// `0x46b880`), on the same link.
#[test]
fn a_refused_login_under_the_stock_screens_waits_for_the_screens_ask() {
    let (realmd, world) = fake::servers(|world| vec![FakeRealm::new(REALM, world)]);
    world.refuse_logins();
    let cycles = fake::spawn_cycles_asking();
    let epoch = enter_world(&cycles, &realmd, true);
    expect_back(&cycles.events, |ev| {
        matches!(ev, SessionEvent::CharacterLoginFailed { result: 1 })
    });
    assert!(
        cycles
            .events
            .recv_timeout(std::time::Duration::from_millis(400))
            .is_err(),
        "the park sent a roster the stock screen did not ask for"
    );
    assert_eq!(world.enumerations_in(0), 0);
    cycles.pick.send(CharRequest::Enumerate).unwrap();
    match next_event(&cycles.events) {
        SessionEvent::CharacterList {
            cause, epoch: on, ..
        } => {
            assert_eq!(cause, RosterCause::Asked);
            assert_eq!(on, epoch);
        }
        other => panic!("expected the asked-for roster, got {other:?}"),
    }
    assert_eq!(world.enumerations_in(0), 1);
    assert_eq!(world.authentications(), 1, "no second CMSG_AUTH_SESSION");
    assert_eq!(realmd.logons(), 1, "no realmd logon");
}
