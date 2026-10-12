use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::char_select::ClientState;
use crate::net::{LoginAbandon, LoginRequest, LoginSubmit, RosterFailedMessage, WorldAuthMessage};
use benilla_protocol::{RealmInfo, SessionEnd};

/// The manager with both agreements accepted, the way a returning player's CVars seed it.
fn accepted() -> GlueMgr {
    let mut n = [NoticeState::default(); 4];
    n[GlueNotice::Tos as usize] = NoticeState::from_cvar("1");
    n[GlueNotice::Eula as usize] = NoticeState::from_cvar("1");
    GlueMgr {
        notices: Some(n),
        ..GlueMgr::default()
    }
}

fn open(which: &'static str, key: Option<&'static str>) -> Outgoing {
    Outgoing::Open {
        which,
        text: key.map(Text::Key),
        data: None,
    }
}

fn failed(refusal: Option<LoginRefusal>, terminal: bool) -> LoginFailedMessage {
    LoginFailedMessage {
        refusal,
        reason: "test".into(),
        terminal,
        dial: None,
    }
}

/// `0x5b2c90`'s arms, after the handlers' clamp past `0x0f`: 0x0E is a success (`0x5b2cd4`), and
/// a proof's 0x0A reports nothing (`0x5bada2`).
#[test]
fn a_realmd_refusal_maps_through_the_references_table() {
    use benilla_protocol::LogonReply::{Challenge, Proof};
    let cases = [
        (0x00, (4, 0x00)),
        (0x01, (5, 0x0b)),
        (0x02, (5, 0x0b)),
        (0x03, (5, 0x0d)),
        (0x04, (5, 0x07)),
        (0x05, (5, 0x07)),
        (0x06, (5, 0x0f)),
        (0x07, (5, 0x10)),
        (0x08, (5, 0x11)),
        (0x09, (5, 0x0e)),
        (0x0a, (6, 0x00)),
        (0x0b, (5, 0x0b)),
        (0x0c, (5, 0x12)),
        (0x0d, (5, 0x0b)),
        (0x0e, (4, 0x00)),
        (0x0f, (5, 0x13)),
        (0x10, (5, 0x14)),
        (0xff, (5, 0x14)),
    ];
    for (byte, want) in cases {
        assert_eq!(realmd_result(byte, Challenge), Some(want), "{byte:#04x}");
    }
    assert_eq!(
        realmd_result(0x0a, Proof),
        None,
        "a proof's 0x0A is swallowed"
    );
    assert_eq!(realmd_result(0x04, Proof), Some((5, 0x07)));
}

/// A proof's 0x0A leaves the logon's dialog as it stands.
#[test]
fn a_proof_0x0a_changes_no_dialog() {
    let mut m = accepted();
    m.login("a".into(), "b".into(), &mut Vec::new());
    m.on_stage(LoginStage::Connecting, true);
    m.tick();
    m.outbox.clear();
    m.on_failed(&failed(Some(LoginRefusal::LogonProof(0x0a)), false));
    m.tick();
    assert!(m.outbox.is_empty(), "{:?}", m.outbox);
    assert_eq!(m.state, ScreenState::Logon);
}

/// `CGlueMgr::Login`'s guards: nothing without both agreements or outside state 0, an `OKAY`
/// dialog for an empty box, else the `CANCEL` dialog with no text and the attempt.
#[test]
fn login_guards_then_starts_the_logon() {
    let mut asks = Vec::new();
    let mut m = GlueMgr::default();
    m.login("a".into(), "b".into(), &mut asks);
    assert!(asks.is_empty() && m.outbox.is_empty(), "no TOS, no login");

    let mut m = accepted();
    m.login(String::new(), "b".into(), &mut asks);
    assert_eq!(m.outbox, [open("OKAY", Some("LOGIN_ENTER_NAME"))]);
    m.outbox.clear();
    m.login("a".into(), String::new(), &mut asks);
    assert_eq!(m.outbox, [open("OKAY", Some("LOGIN_ENTER_PASSWORD"))]);
    assert!(asks.is_empty());
    m.outbox.clear();

    m.login("a".into(), "b".into(), &mut asks);
    assert_eq!(m.state, ScreenState::Logon);
    assert_eq!(m.outbox, [open("CANCEL", None)]);
    assert!(matches!(
        asks.as_slice(),
        [GlueLogin::Submit { account, password }] if account == "a" && password == "b"
    ));
    m.login("a".into(), "b".into(), &mut asks);
    assert_eq!(asks.len(), 1, "a second login waits for state 0");
}

/// The logon's dialogs come off its `(state, result)` pair once per change (`0x46b140`); the
/// authenticated state moves to the world connect on the same tick.
#[test]
fn the_logon_shows_each_state_once_then_connects() {
    let mut m = accepted();
    m.login("a".into(), "b".into(), &mut Vec::new());
    m.outbox.clear();
    m.tick();
    assert!(m.outbox.is_empty(), "the cleared cells show nothing");
    m.on_stage(LoginStage::Connecting, true);
    m.tick();
    m.tick();
    assert_eq!(m.outbox, [open("CANCEL", Some("LOGIN_STATE_CONNECTING"))]);
    m.outbox.clear();
    m.on_logon_done();
    m.tick();
    assert_eq!(
        m.outbox,
        [open("CANCEL", Some("LOGIN_STATE_AUTHENTICATED"))]
    );
    assert_eq!(m.state, ScreenState::Connect);
}

/// A logon refusal raises its `LOGIN_*` text, `CONNECTION_HELP` for `LOGIN_FAILED`, and state 0;
/// a dial that never connected ends on the disconnect it reports last.
#[test]
fn a_logon_refusal_opens_its_result_and_ends_the_logon() {
    for (refusal, which, key) in [
        (
            Some(LoginRefusal::Logon(0x04)),
            "OKAY",
            "LOGIN_UNKNOWN_ACCOUNT",
        ),
        (
            Some(LoginRefusal::Logon(0x05)),
            "OKAY",
            "LOGIN_UNKNOWN_ACCOUNT",
        ),
        (
            Some(LoginRefusal::Logon(0x01)),
            "CONNECTION_HELP",
            "LOGIN_FAILED",
        ),
        (Some(LoginRefusal::Logon(0x03)), "OKAY", "LOGIN_BANNED"),
        (None, "OKAY", "DISCONNECTED"),
    ] {
        let mut m = accepted();
        m.login("a".into(), "b".into(), &mut Vec::new());
        m.outbox.clear();
        m.on_failed(&failed(refusal, false));
        m.tick();
        assert_eq!(m.outbox, [open(which, Some(key))], "{refusal:?}");
        assert_eq!(m.state, ScreenState::Idle);
    }
}

/// One realm, `name`, as realmd lists it.
fn realm(name: &str) -> RealmInfo {
    RealmInfo {
        name: name.into(),
        address: "127.0.0.1:8085".into(),
        population: 1.0,
        characters: 0,
        realm_type: 0,
        flags: 0,
        category: 1,
        id: 1,
    }
}

/// The world connect's failure arm: the status text, `OKAY_WITH_URL` with the URL key for the
/// five codes of `0x803740`, then the realm list over it (`CLOSE_STATUS_DIALOG` and the rebuild);
/// `AUTH_FAILED` instead goes from the character screen back to login.
#[test]
fn a_world_refusal_opens_its_status_and_url() {
    let connected = || {
        let mut m = accepted();
        m.state = ScreenState::Connect;
        m.holder = vec![realm("Fake")];
        m
    };
    let update = |key| Outgoing::Update {
        text: Text::Key(key),
        button: None,
    };
    let mut m = connected();
    m.on_failed(&failed(Some(LoginRefusal::World(0x1c)), false));
    m.tick();
    assert_eq!(
        m.outbox,
        [
            update("AUTH_BANNED"),
            Outgoing::Open {
                which: "OKAY_WITH_URL",
                text: Some(Text::Key("AUTH_BANNED")),
                data: Some("AUTH_BANNED_URL".into()),
            },
            Outgoing::Teardown,
            Outgoing::Close,
            Outgoing::Rebuild,
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);

    let mut m = connected();
    m.screen = "charselect".into();
    m.on_failed(&failed(Some(LoginRefusal::World(0x0d)), false));
    m.tick();
    assert_eq!(
        m.outbox,
        [
            update("AUTH_FAILED"),
            open("OKAY", Some("AUTH_FAILED")),
            Outgoing::Screen("login")
        ]
    );

    let mut m = connected();
    m.on_failed(&failed(None, false));
    m.tick();
    assert_eq!(
        m.outbox,
        [
            update("RESPONSE_FAILED_TO_CONNECT"),
            open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT")),
            Outgoing::Teardown,
            Outgoing::Close,
            Outgoing::Rebuild,
        ]
    );
    assert!(
        m.park_out.is_empty(),
        "the held list is raised, not asked for"
    );

    // With no realm held the arm asks for the list (`0x46c06e` → `0x46b8d0` with 1).
    let mut m = connected();
    m.holder.clear();
    m.on_failed(&failed(None, false));
    m.tick();
    assert_eq!(
        m.outbox,
        [
            update("RESPONSE_FAILED_TO_CONNECT"),
            open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT")),
            Outgoing::Teardown,
            open("CANCEL", Some("REALM_LIST_IN_PROGRESS")),
        ]
    );
    assert_eq!(m.state, ScreenState::RealmList);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Refresh)]
    ));
}

/// The world connect re-texts the dialog from its operation each frame: nothing while the connect
/// runs, `RESPONSE_CONNECTED` when it completes (`0x5aaa90`), nothing while the authentication runs,
/// then `AUTH_OK` and the character screen.
#[test]
fn the_world_connect_shows_each_completion() {
    let mut m = accepted();
    m.login("a".into(), "b".into(), &mut Vec::new());
    m.on_logon_done();
    m.tick();
    assert_eq!(m.state, ScreenState::Connect);
    m.outbox.clear();
    m.tick();
    assert!(m.outbox.is_empty(), "the running connect's text is empty");
    m.on_stage(LoginStage::WorldConnected, true);
    m.tick();
    assert_eq!(
        m.outbox,
        [Outgoing::Update {
            text: Text::Key("RESPONSE_CONNECTED"),
            button: None
        }]
    );
    m.outbox.clear();
    m.tick();
    assert!(
        m.outbox.is_empty(),
        "the running authentication's text is empty"
    );
    m.on_world_authenticated(realm("Fake"), 0);
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("AUTH_OK"),
                button: None
            },
            Outgoing::Teardown,
            Outgoing::Screen("charselect")
        ]
    );
}

/// The authentication response ends the world connect (`0x5ab2b8` → `0x5aab80`): the status text,
/// state 0, the roster torn down (`0x46c014`) and the character screen up. A roster is no part of
/// it, and neither is one a request did not ask for.
#[test]
fn the_authentication_puts_the_character_screen_up() {
    let mut m = accepted();
    m.state = ScreenState::Connect;
    let alpha = || vec![crate::char_select::test_character(7, "Alpha")];
    m.on_roster(alpha(), RosterCause::Asked, 0);
    m.on_roster(alpha(), RosterCause::Park, 0);
    m.tick();
    assert!(
        m.outbox.is_empty() && m.state == ScreenState::Connect,
        "a roster does not complete the connect: {:?}",
        m.outbox
    );
    m.on_world_authenticated(realm("Fake"), 0);
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("AUTH_OK"),
                button: None,
            },
            Outgoing::Teardown,
            Outgoing::Screen("charselect"),
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.connected);
    assert_eq!(m.realm.as_ref().map(|r| r.name.as_str()), Some("Fake"));
    assert!(m.roster.is_empty() && m.park_out.is_empty());
}

/// `GetCharacterListUpdate`'s host half (`0x46b880`): state 3, the `CHAR_LIST_RETRIEVING` dialog
/// and a `CMSG_CHAR_ENUM` on every call; the reply that was asked for completes the query and the
/// next tick closes the dialog and loads the roster (`0x46c084`-`0x46c094`).
#[test]
fn the_character_list_query_loads_the_roster_on_its_reply() {
    let done = [
        Outgoing::Update {
            text: Text::Key("CHAR_LIST_RETRIEVED"),
            button: None,
        },
        Outgoing::Close,
        Outgoing::Roster,
    ];
    let mut m = accepted();
    m.connected = true;
    for (id, name) in [(7, "Alpha"), (8, "Bravo")] {
        m.get_character_list();
        assert_eq!(m.state, ScreenState::CharList);
        assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_LIST_RETRIEVING"))]);
        assert!(matches!(
            m.park_out.as_slice(),
            [ParkRequest::Char(CharRequest::Enumerate)]
        ));
        m.park_out.clear();
        m.outbox.clear();
        m.tick();
        assert!(m.outbox.is_empty(), "a running query shows no text");
        // Rosters nothing asked for are not this screen's.
        for cause in [RosterCause::Park, RosterCause::Changed] {
            m.on_roster(
                vec![crate::char_select::test_character(1, "Stray")],
                cause,
                0,
            );
        }
        m.tick();
        assert!(m.outbox.is_empty());
        assert!(m.roster.iter().all(|c| c.name != "Stray"));
        m.on_roster(
            vec![crate::char_select::test_character(id, name)],
            RosterCause::Asked,
            0,
        );
        m.tick();
        assert_eq!(m.outbox, done);
        assert_eq!(m.state, ScreenState::Idle);
        assert_eq!((m.roster[0].name.as_str(), m.characters), (name, 1));
        m.outbox.clear();
    }
}

/// A reply completes the character list query as `SetStatus` does (`0x5aa920`): a reply that
/// lands between the Cancel and the tick that takes it turns the cancel into a load, as the
/// reference's does. One that comes once the query is over (the realm list the cancel raised), or
/// to an operation that is not the query's (a realm change's connect), or from another link,
/// completes nothing.
#[test]
fn a_late_roster_reply_is_dropped_unless_it_lands_before_the_tick() {
    let late = || vec![crate::char_select::test_character(9, "Late")];
    let mut m = accepted();
    m.connected = true;
    m.get_character_list();
    m.park_out.clear();
    m.status_dialog_click(&mut Vec::new());
    m.tick();
    assert_eq!(m.state, ScreenState::RealmList);
    m.outbox.clear();
    m.on_roster(late(), RosterCause::Asked, 0);
    m.tick();
    assert!(
        m.roster.is_empty(),
        "no roster after the tick took the cancel"
    );
    assert!(
        !m.outbox.iter().any(|o| matches!(o, Outgoing::Roster)),
        "{:?}",
        m.outbox
    );

    // A reply in the frame of the Cancel, before the tick takes it: `SetStatus` overwrites the
    // cancelled status, so the query succeeds and the roster loads (after the teardown the click
    // made).
    let mut m = accepted();
    m.connected = true;
    m.get_character_list();
    m.status_dialog_click(&mut Vec::new());
    m.on_roster(late(), RosterCause::Asked, 0);
    m.outbox.clear();
    m.tick();
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.outbox.iter().any(|o| matches!(o, Outgoing::Roster)));
    assert_eq!(m.roster[0].name, "Late");

    // Deviation: the reference's handler completes whichever operation is current; a reply while
    // a realm change's connect is the operation completes nothing here.
    let mut m = accepted();
    m.connected = true;
    m.holder = vec![realm("Fake")];
    m.get_character_list();
    m.connect_to_realm("fake");
    assert_eq!(m.state, ScreenState::Connect);
    m.on_roster(late(), RosterCause::Asked, 0);
    m.tick();
    assert_eq!(
        m.state,
        ScreenState::Connect,
        "the roster is not the connect's"
    );
    assert!(m.roster.is_empty());
    assert!(
        !m.op.done,
        "the connect is still waiting on its own answer: {:?}",
        m.op
    );

    // Another link's reply (the world session before a realm change) is not this link's.
    let mut m = accepted();
    m.connected = true;
    m.on_world_authenticated(realm("Fake"), 5);
    m.get_character_list();
    m.on_roster(late(), RosterCause::Asked, 4);
    m.tick();
    assert!(m.roster.is_empty() && !m.op.done);
    m.on_roster(late(), RosterCause::Asked, 5);
    assert!(m.op.done && m.op.ok);
}

/// A server that wants Warden refuses the roster ask with its words (a terminal login failure):
/// the query completes failed and the next tick opens `OKAY` with them (`0x46c173`), where state 3
/// would otherwise stand on its `CANCEL` dialog for ever. The link is gone.
#[test]
fn a_terminal_failure_fails_the_character_list_query_with_its_words() {
    let mut m = accepted();
    m.connected = true;
    m.get_character_list();
    m.park_out.clear();
    m.outbox.clear();
    m.on_failed(&failed(None, true));
    m.tick();
    let words = || Text::Raw("test".into());
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: words(),
                button: None,
            },
            Outgoing::Open {
                which: "OKAY",
                text: Some(words()),
                data: None,
            },
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
    assert!(!m.connected);
}

/// A roster ask that failed on the stream completes the query `CHAR_LIST_FAILED`
/// (`0x5ab300`'s failure leg), which the tick opens as `OKAY`, instead of leaving the retrieving
/// dialog up; one of another link changes nothing.
#[test]
fn a_failed_roster_ask_opens_okay_with_char_list_failed() {
    let mut m = accepted();
    m.connected = true;
    m.on_world_authenticated(realm("Fake"), 3);
    m.get_character_list();
    m.outbox.clear();
    m.on_roster_failed(2);
    m.tick();
    assert!(m.outbox.is_empty() && !m.op.done, "another link's failure");
    m.on_roster_failed(3);
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("CHAR_LIST_FAILED"),
                button: None,
            },
            open("OKAY", Some("CHAR_LIST_FAILED")),
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
}

/// The world server's authentication of an attempt a Cancel or a disconnect has passed is not the
/// screen's: the manager does not take the link.
#[test]
fn an_authentication_behind_the_abandon_generation_is_dropped() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, abandon) = stock_app();
    abandon.store(1, Ordering::SeqCst);
    let auth = |generation| WorldAuthMessage {
        realm: realm("Fake"),
        epoch: 1,
        generation,
    };
    app.world_mut().write_message(auth(0));
    app.update();
    assert!(!app.world().resource::<GlueMgr>().connected);
    app.world_mut().write_message(auth(1));
    app.update();
    assert!(app.world().resource::<GlueMgr>().connected);
}

/// With no world link `0x5aabb0` fails the query `RESPONSE_FAILED_TO_CONNECT` at once, and the
/// arm opens `OKAY` with that text (`0x46c173`) and goes to state 0.
#[test]
fn a_character_list_query_with_no_link_fails_okay() {
    let mut m = accepted();
    m.get_character_list();
    assert!(m.park_out.is_empty());
    m.outbox.clear();
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("RESPONSE_FAILED_TO_CONNECT"),
                button: None,
            },
            open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT")),
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
}

/// The dialog's Cancel in state 3 completes the query cancelled and tears the roster down
/// (`0x46b754`); the next tick tears it down again and raises the realm list
/// (`RequestRealmList(1)`, `0x46c069`-`0x46c073`).
#[test]
fn cancelling_the_character_list_raises_the_realm_list() {
    let mut m = accepted();
    m.connected = true;
    m.get_character_list();
    m.park_out.clear();
    m.outbox.clear();
    m.status_dialog_click(&mut Vec::new());
    assert_eq!(m.outbox, [Outgoing::Teardown]);
    m.outbox.clear();
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("RESPONSE_CANCELLED"),
                button: None,
            },
            Outgoing::Teardown,
            open("CANCEL", Some("REALM_LIST_IN_PROGRESS")),
        ]
    );
    assert_eq!(m.state, ScreenState::RealmList);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Refresh)]
    ));
}

/// Change Realm with the world up drops the session before the connect's query (`0x46b22b`).
#[test]
fn change_realm_with_the_world_up_leaves_it_first() {
    let mut m = accepted();
    m.holder = vec![realm("Fake")];
    m.connected = true;
    m.connect_to_realm("fake");
    assert!(matches!(
        m.park_out.as_slice(),
        [
            ParkRequest::Realm(RealmRequest::Leave),
            ParkRequest::Realm(RealmRequest::Refresh)
        ]
    ));
    assert_eq!(m.state, ScreenState::Connect);
    assert!(!m.connected);
    assert_eq!(m.selected.as_ref().map(|r| r.name.as_str()), Some("Fake"));

    let mut m = accepted();
    m.holder = vec![realm("Fake")];
    m.connect_to_realm("Fake");
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Refresh)]
    ));
}

/// The queue re-texts the dialog every frame while the world connect waits.
#[test]
fn the_queue_retexts_every_frame() {
    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.on_queued(Some(40), Some("Realm".into()), 1000);
    m.tick();
    m.tick();
    assert_eq!(m.outbox, [Outgoing::Queue, Outgoing::Queue]);
}

/// `StatusDialogClick` by state (`0x46b7a8`): the logon is cancelled; the world connect completes
/// `RESPONSE_CANCELLED`, whose failure arm shows no dialog, drops the world attempt alone and
/// raises the realm list; a query is cancelled to state 0; idle does nothing.
#[test]
fn status_dialog_click_cancels_by_state() {
    let mut asks = Vec::new();
    let mut m = accepted();
    m.status_dialog_click(&mut asks);
    assert!(asks.is_empty());
    m.state = ScreenState::Logon;
    m.status_dialog_click(&mut asks);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(matches!(asks.as_slice(), [GlueLogin::Abandon]));

    let mut asks = Vec::new();
    let mut m = accepted();
    m.holder = vec![realm("Fake")];
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
    m.status_dialog_click(&mut asks);
    assert!(asks.is_empty(), "the logon stays up");
    assert_eq!(m.state, ScreenState::Connect, "the next tick takes it");
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Teardown,
            Outgoing::Update {
                text: Text::Key("RESPONSE_CANCELLED"),
                button: None
            },
            Outgoing::Teardown,
            Outgoing::Close,
            Outgoing::Rebuild
        ],
        "the click and the failure arm tear the roster down; no failure dialog for a cancel"
    );
    assert!(matches!(m.login_out.as_slice(), [GlueLogin::CancelWorld]));
    assert_eq!(m.state, ScreenState::Idle);

    let mut m = accepted();
    m.request_realm_list(false);
    assert_eq!(m.state, ScreenState::RealmList);
    m.status_dialog_click(&mut asks);
    assert_eq!(m.state, ScreenState::Idle);
    m.tick();
    assert!(m.outbox.is_empty(), "a cancelled query reports nothing");
}

/// A session that is over fires `DISCONNECTED_FROM_SERVER`; an unattended reconnect does not; in
/// the world connect, a completed failed operation opens `OKAY` with its text instead
/// (`0x46c671`-`0x46c68f`), and a running one still fires the event.
#[test]
fn a_lost_session_fires_disconnected() {
    let mut m = accepted();
    m.on_disconnected(SessionEnd::Lost, false);
    assert!(m.outbox.is_empty());
    m.on_disconnected(SessionEnd::Lost, true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m.on_disconnected(SessionEnd::Lost, true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.op.complete(false, STATUS_FAILED_TO_CONNECT);
    m.on_disconnected(SessionEnd::Lost, true);
    assert_eq!(m.outbox, [open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT"))]);
    assert_eq!(m.state, ScreenState::Idle);
}

/// A logout or a refused login keeps the world link (`0x5aaeb0`, `0x5aad70`), so the character
/// screen that comes back up finds it connected (`0x46d380`) and its `GetCharacterListUpdate`
/// sends the roster ask on it (`0x46b880` → `0x5aabb0`), with no dialog of a disconnect.
#[test]
fn a_logout_keeps_the_link_for_the_screens_roster_ask() {
    let mut m = accepted();
    m.connected = true;
    m.on_disconnected(SessionEnd::LoggedOut, false);
    assert!(m.connected, "the link is kept");
    assert!(m.outbox.is_empty());
    m.get_character_list();
    let (_, park) = m.take_asks();
    assert!(
        matches!(park[..], [ParkRequest::Char(CharRequest::Enumerate)]),
        "the ask goes out on the link"
    );
    assert!(!m.op.done, "and waits for its reply");
}

// ── The stock screens over the native login, end to end ─────────────────────────────────────────

/// An app with the manager, the native login's glue asks and the stock glue VM on the login
/// screen. Returns it, the credentials channel and the abandon generation.
fn stock_app() -> (
    App,
    crossbeam_channel::Receiver<LoginRequest>,
    Arc<AtomicU64>,
) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let abandon = Arc::new(AtomicU64::new(0));
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<GlueMgr>()
        .init_resource::<crate::login::LoginIntent>()
        .insert_resource(crate::realmlist::Realmlist::unpinned(
            crate::realmlist::DEFAULT_REALMLIST,
        ))
        .insert_resource(LoginSubmit(tx))
        .insert_resource(LoginAbandon(abandon.clone()))
        .insert_resource(State::new(ClientState::Login))
        .init_resource::<NextState<ClientState>>()
        .add_message::<LoginStageMessage>()
        .add_message::<LoginQueuedMessage>()
        .add_message::<LoginFailedMessage>()
        .add_message::<CharListMessage>()
        .add_message::<WorldAuthMessage>()
        .add_message::<RosterFailedMessage>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<crate::ui_script::server_alert::LaunchUrl>()
        .add_message::<AppExit>()
        .init_resource::<crate::portrait::GluePreview>()
        .add_systems(
            Update,
            (
                drive_glue_mgr,
                super::super::glue_scene::feed_stock_glue_preview,
                crate::login::answer_glue,
            )
                .chain(),
        );
    let mut s = UiScript::new_glue().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let _ = super::super::glue::load_glue(&mut s);
    super::super::glue::set_glue_screen(&mut s, super::super::glue::GlueScreen::Login);
    let _ = s.take_errors();
    app.insert_non_send_resource(s);
    app.update();
    (app, rx, abandon)
}

fn vm(app: &App) -> &UiScript {
    app.world().non_send_resource::<UiScript>()
}

/// The dialog the stock `GlueDialog` shows: its type and text, or `None` when hidden.
fn dialog(app: &App) -> Option<(String, String)> {
    let (shown, which, text) = vm(app)
        .eval::<(bool, String, String)>(
            "return GlueDialog:IsShown() == 1, GlueDialog.which or '', GlueDialogText:GetText() or ''",
        )
        .unwrap();
    shown.then_some((which, text))
}

/// Type into the stock boxes and press Login, as `AccountLogin_Login` does.
fn type_and_login(app: &mut App) {
    vm(app)
        .run(
            "AccountLoginAccountEdit:SetText('probe') AccountLoginPasswordEdit:SetText('secret') \
             AccountLogin_Login()",
        )
        .unwrap();
    app.update();
}

/// `DisconnectFromServer` zeroes the screen state with the operation in flight (`0x46c55d`), so
/// the roster reply of a query the character screen's Exit left behind cannot complete it on the
/// login screen and raise the realm list.
#[test]
fn disconnecting_from_the_server_ends_the_query_in_flight() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _abandon) = stock_app();
    {
        let mut mgr = app.world_mut().resource_mut::<GlueMgr>();
        mgr.connected = true;
        mgr.get_character_list();
    }
    vm(&app).run("DisconnectFromServer()").unwrap();
    app.update();
    let mgr = app.world().resource::<GlueMgr>();
    assert_eq!(mgr.state, ScreenState::Idle);
    assert!(!mgr.connected && mgr.op.kind != OpKind::CharList);

    // With no world connection the verb only cancels the logon (`0x46d34d`): the state stands.
    app.world_mut().resource_mut::<GlueMgr>().state = ScreenState::RealmList;
    vm(&app).run("DisconnectFromServer()").unwrap();
    app.update();
    assert_eq!(
        app.world().resource::<GlueMgr>().state,
        ScreenState::RealmList
    );
}

/// The select verbs' cells are C globals in the reference, so a glue rebuild finds them as they
/// were: the manager seeds a new VM with the last one's, and keeps the VM's as it moves them.
#[test]
fn the_select_cells_outlive_the_glue_vm() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _abandon) = stock_app();
    let cells = GlueSelectCells {
        selected: 2,
        facing: 0.5,
        built: true,
    };
    {
        let mut mgr = app.world_mut().resource_mut::<GlueMgr>();
        mgr.select_cells = cells;
        mgr.vm = None;
    }
    app.update();
    assert_eq!(vm(&app).glue_select_cells(), cells);
    vm(&app).run("SetCharacterSelectFacing(90)").unwrap();
    app.update();
    let moved = app.world().resource::<GlueMgr>().select_cells;
    assert!(
        (moved.facing - 90f32.to_radians()).abs() < 1e-5,
        "{moved:?}"
    );
}

/// The stock login screen's Login reaches the native login with the typed credentials, under the
/// `CANCEL` dialog; the logon's stage and a refusal re-text it with the reference's keys; and the
/// agreements gate it.
#[test]
fn the_stock_login_reaches_the_native_login_and_shows_its_status() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, rx, _) = stock_app();

    // A first launch: the EULA stands, and a login before it is accepted does nothing.
    assert_eq!(
        vm(&app)
            .eval::<(bool, bool)>("return TOSFrame:IsShown() == 1, AccountLoginUI:IsShown() == 1")
            .unwrap(),
        (true, false),
        "readEULA 0 puts the EULA up"
    );
    type_and_login(&mut app);
    assert!(rx.try_recv().is_err(), "no login before the agreements");
    vm(&app).run("AcceptEULA() AcceptTOS()").unwrap();
    app.update();

    type_and_login(&mut app);
    let req = rx.try_recv().expect("the native login gets the attempt");
    assert_eq!((req.user.as_str(), req.pass.as_str()), ("probe", "secret"));
    let (which, _) = dialog(&app).expect("the status dialog is up");
    assert_eq!(which, "CANCEL");
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Logon);

    app.world_mut().write_message(LoginStageMessage {
        stage: LoginStage::Connecting,
    });
    app.update();
    let connecting = vm(&app).global_text("LOGIN_STATE_CONNECTING");
    assert_eq!(dialog(&app), Some(("CANCEL".into(), connecting)));

    app.world_mut()
        .write_message(failed(Some(LoginRefusal::Logon(0x04)), false));
    app.update();
    let unknown = vm(&app).global_text("LOGIN_UNKNOWN_ACCOUNT");
    assert!(!unknown.is_empty());
    assert_eq!(dialog(&app), Some(("OKAY".into(), unknown)));
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Idle);
}

/// The status dialog's Cancel is `StatusDialogClick`, which abandons the native attempt.
#[test]
fn the_status_dialogs_cancel_abandons_the_attempt() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, rx, abandon) = stock_app();
    vm(&app).run("AcceptEULA() AcceptTOS()").unwrap();
    app.update();
    type_and_login(&mut app);
    assert!(rx.try_recv().is_ok());
    vm(&app).run("GlueDialogButton1:Click()").unwrap();
    app.update();
    assert_eq!(
        abandon.load(Ordering::SeqCst),
        1,
        "Cancel bumps the abandon generation"
    );
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Idle);
    assert_eq!(dialog(&app), None);
}

/// benilla's realmlist control on the stock screen: its button opens the stock dialog on the
/// current address, and Okay writes `realmList`.
#[test]
fn the_realmlist_button_writes_realmlist() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _) = stock_app();
    {
        let s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.register_cvars(crate::cvars::registered_pairs());
        s.run("AcceptEULA() AcceptTOS() AccountLogin_ShowUserAgreements()")
            .unwrap();
    }
    app.update();
    // The readout follows the host's global on the frame's `OnUpdate`.
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .tick(0.05);
    assert_eq!(
        vm(&app)
            .eval::<String>("return BenillaRealmlistText:GetText()")
            .unwrap(),
        crate::realmlist::DEFAULT_REALMLIST,
        "the readout shows the address the next login dials"
    );
    assert_eq!(
        vm(&app)
            .eval::<String>("return BenillaRealmlistButton:GetParent():GetName()")
            .unwrap(),
        "AccountLoginUI"
    );
    vm(&app).run("BenillaRealmlistButton:Click()").unwrap();
    assert_eq!(
        vm(&app)
            .eval::<(String, String)>("return GlueDialog.which, GlueDialogEditBox:GetText()")
            .unwrap(),
        (
            "BENILLA_REALMLIST".to_string(),
            crate::realmlist::DEFAULT_REALMLIST.to_string()
        )
    );
    vm(&app)
        .run("GlueDialogEditBox:SetText('logon.example.org') GlueDialogButton1:Click()")
        .unwrap();
    let changes = app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .take_cvar_changes();
    assert!(
        changes
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case("realmList") && v == "logon.example.org"),
        "{changes:?}"
    );
    assert!(vm(&app).errors().is_empty(), "{:?}", vm(&app).errors());
}

// ── The stock screens over the IO thread and a fake realmd ──────────────────────────────────────

use crate::net::io::fake::{self, FakeRealm};

/// The IO thread's events, pumped into the app's messages as `net::session` does.
#[derive(Resource)]
struct IoEvents(crossbeam_channel::Receiver<benilla_protocol::SessionEvent>);

/// World dials the IO thread began (`LoginStage::Handshaking`).
#[derive(Resource, Default)]
struct WorldDials(usize);

fn pump(
    events: Res<IoEvents>,
    mut dials: ResMut<WorldDials>,
    mut stages: MessageWriter<LoginStageMessage>,
    mut lists: MessageWriter<RealmListMessage>,
    mut failures: MessageWriter<LoginFailedMessage>,
    mut rosters: MessageWriter<CharListMessage>,
    mut auths: MessageWriter<WorldAuthMessage>,
    mut failed_rosters: MessageWriter<RosterFailedMessage>,
    mut queued: MessageWriter<LoginQueuedMessage>,
) {
    use benilla_protocol::SessionEvent as E;
    for ev in events.0.try_iter() {
        match ev {
            E::LoginStage { stage } => {
                if stage == LoginStage::Handshaking {
                    dials.0 += 1;
                }
                stages.write(LoginStageMessage { stage });
            }
            E::RealmList { realms, failed } => {
                lists.write(RealmListMessage { realms, failed });
            }
            E::LoginFailed {
                refusal,
                reason,
                terminal,
                dial,
            } => {
                failures.write(LoginFailedMessage {
                    refusal,
                    reason,
                    terminal,
                    dial,
                });
            }
            E::CharacterList {
                characters,
                realm,
                cause,
                epoch,
                generation,
            } => {
                rosters.write(CharListMessage {
                    characters,
                    realm,
                    cause,
                    epoch,
                    generation,
                });
            }
            E::WorldAuthenticated {
                realm,
                epoch,
                generation,
            } => {
                auths.write(WorldAuthMessage {
                    realm,
                    epoch,
                    generation,
                });
            }
            E::RosterFailed { epoch } => {
                failed_rosters.write(RosterFailedMessage { epoch });
            }
            E::LoginQueued { position, realm } => {
                queued.write(LoginQueuedMessage { position, realm });
            }
            _ => {}
        }
    }
}

/// The stock login screen, agreements accepted, over the IO thread's cycle and a fake realmd
/// listing `realms`; `realm_name` is the persisted `realmName`.
fn live_app(realms: impl FnOnce(&str) -> Vec<FakeRealm>, realm_name: &str) -> (App, fake::Realmd) {
    let (app, realmd, _) = live_app_with(realms, realm_name, &[]);
    (app, realmd)
}

/// [`live_app`] over a world server whose account holds `roster`; also returns the world server.
fn live_app_with(
    realms: impl FnOnce(&str) -> Vec<FakeRealm>,
    realm_name: &str,
    roster: &[benilla_protocol::Character],
) -> (App, fake::Realmd, fake::World) {
    let (realmd, world) = fake::servers(realms);
    world.set_roster(roster);
    // The race and class rows the roster's names come from, off the player's own chain.
    let data = benilla_formats::wow_data().expect("callers skip without an install");
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let cycles = fake::spawn_cycles_asking();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<GlueMgr>()
        .init_resource::<crate::login::LoginIntent>()
        .insert_resource(crate::realmlist::Realmlist::unpinned(&realmd.addr))
        .insert_resource(LoginSubmit(cycles.login.clone()))
        .insert_resource(LoginAbandon(cycles.abandon.clone()))
        .insert_resource(crate::net::RealmChoice(cycles.realm.clone()))
        .insert_resource(crate::net::CharPick(cycles.pick.clone()))
        .insert_resource(crate::realm_select::Realms::us_client())
        .insert_resource(crate::entities::CharCreate(
            benilla_formats::CharCreateCatalog::load(&mut chain).expect("ChrRaces"),
        ))
        .insert_resource(crate::entities::Creatures::from_catalog(
            benilla_formats::load_creature_catalog(&mut chain).expect("creatures"),
        ))
        .insert_resource(crate::chr_classes::ChrClassTable(
            benilla_formats::load_chr_classes(&mut chain).expect("ChrClasses"),
        ))
        .insert_resource(crate::area::AreaTableRes(
            benilla_formats::AreaTableCatalog::from_rows(vec![
                (141, area_row("Teldrassil")),
                (12, area_row("Elwynn Forest")),
            ]),
        ))
        .insert_resource(IoEvents(cycles.events.clone()))
        .init_resource::<WorldDials>()
        .insert_resource(State::new(ClientState::Login))
        .init_resource::<NextState<ClientState>>()
        .add_message::<LoginStageMessage>()
        .add_message::<LoginQueuedMessage>()
        .add_message::<LoginFailedMessage>()
        .add_message::<CharListMessage>()
        .add_message::<WorldAuthMessage>()
        .add_message::<RosterFailedMessage>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<crate::ui_script::server_alert::LaunchUrl>()
        .add_message::<AppExit>()
        .init_resource::<crate::portrait::GluePreview>()
        .add_systems(
            Update,
            (
                pump,
                drive_glue_mgr,
                super::super::glue_scene::feed_stock_glue_preview,
                crate::login::answer_glue,
            )
                .chain(),
        );
    let mut s = UiScript::new_glue().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_cvar_host(crate::realm_select::CVAR_REALM_NAME, realm_name);
    let _ = super::super::glue::load_glue(&mut s);
    super::super::glue::set_glue_screen(&mut s, super::super::glue::GlueScreen::Login);
    s.run("AcceptEULA() AcceptTOS() AccountLogin_ShowUserAgreements()")
        .unwrap();
    let _ = s.take_errors();
    app.insert_non_send_resource(s);
    app.update();
    // The cycles' ends stay alive with the app's resources.
    std::mem::forget(cycles);
    (app, realmd, world)
}

/// An `AreaTable` row named `name`, the columns the roster does not read at their defaults.
fn area_row(name: &str) -> benilla_formats::AreaTableRow {
    benilla_formats::AreaTableRow {
        map_id: 0,
        zone_id: 0,
        explore_flag: 0,
        flags: 0,
        faction_group_mask: 0,
        exploration_level: -1,
        name: name.into(),
    }
}

/// Run frames, the VM's own tick between them, until `done` holds.
fn until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + fake::WAIT;
    while !done(app) {
        if std::time::Instant::now() >= deadline {
            let m = app.world().resource::<GlueMgr>();
            panic!(
                "timed out waiting for {what}: state {:?}, op {:?}, holder {}, screen {:?}, \
                 dialog {:?}, errors {:?}",
                m.state,
                m.op,
                m.holder.len(),
                m.screen,
                dialog(app),
                vm(app).errors()
            );
        }
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Type the fake realmd's account into the stock boxes and press Login.
fn log_in(app: &mut App) {
    vm(app)
        .run(&format!(
            "AccountLoginAccountEdit:SetText('{}') AccountLoginPasswordEdit:SetText('{}') \
             AccountLogin_Login()",
            fake::USER,
            fake::PASS
        ))
        .unwrap();
    app.update();
}

fn shown(app: &App, frame: &str) -> bool {
    vm(app)
        .eval::<bool>(&format!("return {frame}:IsShown() == 1"))
        .unwrap()
}

fn screen_up(app: &App) -> bool {
    let m = app.world().resource::<GlueMgr>();
    m.state == ScreenState::Idle && m.connected
}

/// A launch whose `realmName` names no realm, for an account the session has seen no roster of
/// and with no realm chosen: the connect fails `REALM_LIST_REALM_NOT_FOUND`, its `OKAY` comes
/// down, and `GET_PREFERRED_REALM_INFO` puts the realm wizard up (#834). Its location buttons are
/// the Region's categories holding a realm; Suggest with PvP asks `SetPreferredInfo`, whose
/// `SUGGEST_REALM` the character screen turns into the suggestion dialog, and Accept connects.
#[test]
fn a_fresh_account_is_asked_for_a_realm_and_suggested_one() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, realmd) = live_app(
        |world| {
            let mut pvp = FakeRealm::new("Bravo", world);
            pvp.realm_type = 1;
            let mut oz = FakeRealm::new("Oz", world);
            oz.category = 5;
            vec![FakeRealm::new("Alpha", world), pvp, oz]
        },
        "",
    );
    log_in(&mut app);
    until(&mut app, "the realm wizard", |app| {
        shown(app, "RealmWizard")
    });
    assert_eq!(dialog(&app), None, "the not-found OKAY is taken back down");
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Idle);
    assert_eq!(
        vm(&app)
            .eval::<(String, String, bool)>(
                "return RealmWizardLocationButton1Text:GetText(), \
                 RealmWizardLocationButton2Text:GetText(), \
                 RealmWizardLocationButton3:IsShown() == 1"
            )
            .unwrap(),
        ("United States".into(), "Oceanic".into(), false),
        "one location per category holding a realm"
    );

    vm(&app)
        .run(
            "RealmWizardLocationButton1:Click() RealmWizardGameTypeButton2:Click() \
             RealmWizardSuggest:Click()",
        )
        .unwrap();
    let (which, text) = dialog(&app).expect("the suggestion dialog");
    assert_eq!(which, "SUGGEST_REALM");
    assert!(
        text.contains("Bravo"),
        "the PvP realm is suggested: {text:?}"
    );

    vm(&app).run("GlueDialogButton1:Click()").unwrap();
    until(&mut app, "the character screen", screen_up);
    assert_eq!(
        vm(&app)
            .cvar(crate::realm_select::CVAR_REALM_NAME)
            .as_deref(),
        Some("Bravo"),
        "ChangeRealm sets realmName"
    );
    assert_eq!(
        realmd.logons(),
        1,
        "the realm was dialled on the logon's link"
    );
}

/// A launch whose `realmName` names no realm, once a realm has been chosen this session, raises
/// the realm list over the login screen, its tabs the two categories. Cancel there closes the
/// realmd link and the next login is taken (#833); the close button leaves the link up, and the
/// next login is taken too; a row and Okay then reach the character screen.
#[test]
fn closing_the_realm_list_lets_the_next_login_through() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, realmd) = live_app(
        |world| {
            let mut oz = FakeRealm::new("Oz", world);
            oz.category = 5;
            vec![FakeRealm::new("Alpha", world), oz]
        },
        "Gone",
    );
    app.world_mut().resource_mut::<GlueMgr>().realm_cells.chosen = Some(0);
    // The fresh VM takes the cells on the next frame.
    app.world_mut().resource_mut::<GlueMgr>().vm = None;
    app.update();

    log_in(&mut app);
    until(&mut app, "the realm list", |app| shown(app, "RealmList"));
    assert!(
        shown(&app, "AccountLogin"),
        "the list stands over the login screen"
    );
    assert_eq!(dialog(&app), None);
    assert_eq!(
        vm(&app)
            .eval::<(String, String, bool)>(
                "return RealmListTab1:GetText(), RealmListTab2:GetText(), \
                 RealmListTab2:IsShown() == 1"
            )
            .unwrap(),
        ("United States".into(), "Oceanic".into(), true)
    );

    vm(&app).run("RealmListCancelButton:Click()").unwrap();
    until(&mut app, "the realmd link to close", |_| realmd.open() == 0);
    assert!(!shown(&app, "RealmList") && shown(&app, "AccountLogin"));
    log_in(&mut app);
    until(&mut app, "the list after the second login", |app| {
        shown(app, "RealmList")
    });
    assert_eq!(realmd.logons(), 2);

    vm(&app).run("RealmListCloseButton:Click()").unwrap();
    app.update();
    assert_eq!(realmd.open(), 1, "the close button leaves the link up");
    log_in(&mut app);
    until(&mut app, "the list after the third login", |app| {
        shown(app, "RealmList")
    });
    assert_eq!(realmd.logons(), 3);

    vm(&app)
        .run("RealmListRealmButton1:Click() RealmListOkButton:Click()")
        .unwrap();
    until(&mut app, "the character screen", screen_up);
    assert_eq!(
        vm(&app)
            .cvar(crate::realm_select::CVAR_REALM_NAME)
            .as_deref(),
        Some("Alpha")
    );
}

/// A server that lists no realm: the connect fails `REALM_LIST_REALM_NOT_FOUND`, the failure arm
/// finds the holder empty and asks for the list (`0x46bf86` → `0x46c06e`), and the empty reply's
/// rebuild puts the wizard up; nothing dials a fallback.
#[test]
fn an_empty_realm_list_asks_again_and_shows_it() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, realmd) = live_app(|_| Vec::new(), "");
    log_in(&mut app);
    until(&mut app, "the realm wizard", |app| {
        shown(app, "RealmWizard")
    });
    assert_eq!(realmd.lists(), 2, "the logon's list and the arm's query");
    assert_eq!(
        app.world().resource::<WorldDials>().0,
        0,
        "no fallback dial"
    );
}

// ── The character screen's roster over the IO thread and a fake world server ────────────────────

/// Three characters: a living night elf druid in Teldrassil, a ghost human warrior in Elwynn, and
/// a gnome mage in a zone `AreaTable` has no row for.
fn roster3() -> Vec<benilla_protocol::Character> {
    let mut alpha = crate::char_select::test_character(0x11, "Alpha");
    (
        alpha.race,
        alpha.class,
        alpha.gender,
        alpha.level,
        alpha.zone,
    ) = (4, 11, 1, 12, 141);
    let mut bravo = crate::char_select::test_character(0x12, "Bravo");
    (bravo.race, bravo.class, bravo.level, bravo.zone) = (1, 1, 60, 12);
    bravo.flags = benilla_protocol::CHARACTER_FLAG_GHOST;
    let mut charlie = crate::char_select::test_character(0x13, "Charlie");
    (charlie.race, charlie.class, charlie.level, charlie.zone) = (7, 8, 3, 9999);
    vec![alpha, bravo, charlie]
}

/// The stock login for `roster`, `lastCharacterIndex` as given, a realm chosen or not, with the
/// events the roster fires logged into the VM's `EVENTS`.
///
/// `GetNumAddOns` is the add-on list's verb, which this screen's `OnShow` reaches first
/// (`UpdateAddonButton`, `AddonList.lua:4`) and which the glue VM does not have yet; the test
/// defines it empty, or the show raises there and never asks for the list.
fn char_select_app(
    roster: &[benilla_protocol::Character],
    last: &str,
    chosen: bool,
) -> (App, fake::Realmd, fake::World) {
    char_select_app_on(
        |world| vec![FakeRealm::new("Alpha", world)],
        &[roster],
        last,
        chosen,
    )
}

/// [`char_select_app`] over `realms`, the account holding `rosters[n]` on the world session
/// that begins `n`th (the last repeats).
fn char_select_app_on(
    realms: impl FnOnce(&str) -> Vec<FakeRealm>,
    rosters: &[&[benilla_protocol::Character]],
    last: &str,
    chosen: bool,
) -> (App, fake::Realmd, fake::World) {
    let (mut app, realmd, world) = live_app_with(realms, "Alpha", rosters[0]);
    world.set_rosters(rosters);
    if chosen {
        app.world_mut().resource_mut::<GlueMgr>().realm_cells.chosen = Some(0);
        // The fresh VM takes the cells on the next frame.
        app.world_mut().resource_mut::<GlueMgr>().vm = None;
    }
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_cvar_engine("lastCharacterIndex", last);
        script
            .run(
                "function GetNumAddOns() return 0 end \
                 EVENTS = {} \
                 local f = CreateFrame('Frame', 'RosterListener') \
                 f:SetScript('OnEvent', function() \
                   table.insert(EVENTS, event .. ' ' .. tostring(arg1)) end) \
                 f:RegisterEvent('CHARACTER_LIST_UPDATE') \
                 f:RegisterEvent('UPDATE_SELECTED_CHARACTER')",
            )
            .unwrap();
    }
    app.update();
    log_in(&mut app);
    (app, realmd, world)
}

/// How many times the listener saw `event`.
fn seen(app: &App, event: &str) -> usize {
    vm(app)
        .eval::<usize>(&format!(
            "local n = 0 for _, e in ipairs(EVENTS) do \
               if string.find(e, '^{event}') then n = n + 1 end end return n"
        ))
        .unwrap()
}

/// The stock login reaches the character screen, which asks for its list as it shows: the
/// roster is the reply the IO thread enumerated on its own (one `CMSG_CHAR_ENUM` in all), the
/// rows read it through `GetCharacterInfo`, `lastCharacterIndex` is the selection (0-based, so
/// "1" is the second row), and the flow raises no Lua error. A second `GetCharacterListUpdate`
/// clears the list at the call, shows the retrieving dialog and asks the server again with a
/// `CMSG_CHAR_ENUM` that is the six bytes the client sends: size 4, opcode 0x37 in four.
#[test]
fn the_stock_login_loads_the_roster_into_character_select() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = char_select_app(&roster3(), "1", false);
    until(&mut app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    assert!(shown(&app, "CharacterSelect"));
    assert_eq!(dialog(&app), None, "the retrieving dialog is closed");
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    assert_eq!(
        world.enumerations(),
        1,
        "the first ask takes the held reply"
    );

    let text = |app: &App, name: &str| {
        vm(app)
            .eval::<String>(&format!("return {name}:GetText() or ''"))
            .unwrap()
    };
    let row = |app: &App, i: usize| {
        (
            text(app, &format!("CharSelectCharacterButton{i}ButtonTextName")),
            text(app, &format!("CharSelectCharacterButton{i}ButtonTextInfo")),
            text(
                app,
                &format!("CharSelectCharacterButton{i}ButtonTextLocation"),
            ),
        )
    };
    let s = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    assert_eq!(row(&app, 1), s("Alpha", "Level 12 Druid", "Teldrassil"));
    assert_eq!(
        row(&app, 2),
        s("Bravo", "Level 60 Warrior (Ghost)", "Elwynn Forest")
    );
    assert_eq!(row(&app, 3), s("Charlie", "Level 3 Mage", ""));
    assert!(!shown(&app, "CharSelectCharacterButton4"));
    assert_eq!(text(&app, "CharSelectCharacterName"), "Bravo");
    assert_eq!(
        vm(&app)
            .eval::<(i64, f64, String)>(
                "return CharacterSelect.selectedIndex, GetNumCharacters(), \
                 CharacterSelect.currentModel"
            )
            .unwrap(),
        (2, 3.0, "Human".to_string()),
        "lastCharacterIndex 1 selects the second row, and its race file string is the background"
    );
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x12));
    assert_eq!(
        vm(&app).glue_select_background().as_deref(),
        Some("Interface\\Glues\\Models\\UI_Human\\UI_Human.mdx"),
        "SetBackgroundModel reached SetCharSelectBackground for the selected race"
    );

    // Asked again: the list clears at the call, the dialog shows, the reply reloads it.
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    assert_eq!(
        vm(&app).eval::<f64>("return GetNumCharacters()").unwrap(),
        0.0
    );
    let before = seen(&app, "CHARACTER_LIST_UPDATE");
    app.update();
    assert_eq!(
        dialog(&app),
        Some(("CANCEL".into(), "Retrieving character list".into()))
    );
    until(&mut app, "the second roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > before
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    assert_eq!(dialog(&app), None);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    assert_eq!(
        text(&app, "CharSelectCharacterButton3ButtonTextName"),
        "Charlie"
    );
    assert_eq!(world.enumerations(), 2);
    let enums: Vec<Vec<u8>> = world
        .packets
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p[2..4] == [0x37, 0x00])
        .cloned()
        .collect();
    assert_eq!(enums, vec![vec![0x00, 0x04, 0x37, 0x00, 0x00, 0x00]; 2]);
}

/// An empty roster: the teardown at the connect and the load both select -1, which fires
/// `UPDATE_SELECTED_CHARACTER` with 1 (`0x472740`). With no realm chosen this session the load
/// asks for the realm list and fires no `CHARACTER_LIST_UPDATE` (`0x47253d`-`0x47254b`), and the
/// list's rebuild puts the realm wizard up (an account with no characters); with a realm chosen
/// it fires the update and the screen stays, with no rows.
#[test]
fn an_empty_roster_asks_for_the_realm_list_only_with_no_realm_chosen() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, realmd, _) = char_select_app(&[], "0", false);
    until(&mut app, "the realm wizard", |app| {
        shown(app, "RealmWizard")
    });
    assert_eq!(
        seen(&app, "CHARACTER_LIST_UPDATE"),
        1,
        "the teardown's, and not the load's"
    );
    assert_eq!(seen(&app, "UPDATE_SELECTED_CHARACTER"), 2);
    assert_eq!(realmd.lists(), 2, "the logon's list and the load's query");
    assert_eq!(vm(&app).errors(), Vec::<String>::new());

    let (mut app, realmd, _) = char_select_app(&[], "0", true);
    until(&mut app, "the roster load", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    for _ in 0..20 {
        app.update();
    }
    assert!(shown(&app, "CharacterSelect") && !shown(&app, "RealmWizard"));
    assert!(!shown(&app, "RealmList"));
    assert_eq!(seen(&app, "CHARACTER_LIST_UPDATE"), 2);
    assert_eq!(seen(&app, "UPDATE_SELECTED_CHARACTER"), 2);
    assert_eq!(realmd.lists(), 1, "no realm list was asked for");
    assert_eq!(
        vm(&app)
            .eval::<(f64, i64, bool)>(
                "return GetNumCharacters(), CharacterSelect.selectedIndex, \
                 CharSelectCharacterButton1:IsShown() == 1"
            )
            .unwrap(),
        (0.0, 0, false)
    );
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

// ── The stock screens' scenes through the glue booth ────────────────────────────────────────────

fn preview(app: &App) -> &crate::portrait::GluePreview {
    app.world().resource::<crate::portrait::GluePreview>()
}

/// The stock login's `AccountLogin` pane drives the glue booth: `UI_MainMenu` under the pane's
/// own fog, `AccountLogin.xml`'s `<FogColor>` and `fogFar`, with nobody standing in it.
#[test]
fn the_stock_login_stands_the_main_menu_in_the_glue_booth() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (app, _rx, _abandon) = stock_app();
    let p = preview(&app);
    assert_eq!(p.scene, Some(crate::portrait::GlueScene::MainMenu));
    assert!(p.look.is_none());
    let fog = p.fog.expect("the main menu fogs");
    assert_eq!((fog.near, fog.far), (0.0, 1200.0));
    for (got, want) in fog.rgb.iter().zip([0.25, 0.06, 0.015]) {
        assert!((got - want).abs() < 1.0 / 255.0, "{:?}", fog.rgb);
    }
}

/// The selected character stands in the select scene: the roster load's selection, then a
/// row's `CharacterSelect_SelectCharacter`, which sets its race's stage unfogged; the yaw is the
/// facing cell `SetCharacterSelectFacing` writes, and the next select squares it.
#[test]
fn the_stock_select_stands_the_selected_character_at_the_cells_facing() {
    use crate::portrait::{GlueLook, GlueScene, SelectLook};
    let _ = benilla_formats::wow_data_or_skip!();
    let mut roster = roster3();
    let mut dwarf = crate::char_select::test_character(0x14, "Delta");
    (dwarf.race, dwarf.zone) = (3, 12);
    roster.push(dwarf);
    let (mut app, _realmd, _world) = char_select_app(&roster, "1", false);
    until(&mut app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert!(
        p.look == Some(GlueLook::Select(SelectLook::from(&roster[1]))),
        "lastCharacterIndex 1 stands Bravo"
    );
    assert_eq!((p.scene, p.fog), (Some(GlueScene::Human), None));
    assert_eq!(p.yaw, 0.0);

    vm(&app).run("CharacterSelect_SelectCharacter(1)").unwrap();
    app.update();
    let p = preview(&app);
    assert!(p.look == Some(GlueLook::Select(SelectLook::from(&roster[0]))));
    assert_eq!((p.scene, p.fog), (Some(GlueScene::NightElf), None));

    vm(&app).run("SetCharacterSelectFacing(90)").unwrap();
    app.update();
    let yaw = preview(&app).yaw;
    assert!((yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{yaw}");
    vm(&app).run("CharacterSelect_SelectCharacter(3)").unwrap();
    app.update();
    let p = preview(&app);
    assert_eq!(p.yaw, 0.0, "every select zeroes the facing (`0x472a61`)");
    assert_eq!(
        p.scene,
        Some(GlueScene::Dwarf),
        "a gnome stands in Dwarf's stage"
    );
    // The dwarf after the gnome: the same stage, set again by its own file string, is a new
    // instance (`0x76cce0`), so the scene starts over.
    let gnome_stage = p.stage;
    vm(&app).run("CharacterSelect_SelectCharacter(4)").unwrap();
    app.update();
    let p = preview(&app);
    assert_eq!(p.scene, Some(GlueScene::Dwarf));
    assert_ne!(p.stage, gnome_stage);
    // The pane is GlueParent's, the whole 4:3 screen.
    assert_eq!(p.pane, Some([0.0, 1.0, 0.0, 1.0]));
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// #978: an account with no characters shows `CharacterSelect`'s OnLoad scene, `UI_Orc` under
/// Orc's fog. Once a character has set its stage the scene stays, unfogged, when the roster
/// empties: an empty roster sets no background.
#[test]
fn the_stock_select_fogs_the_empty_roster_and_keeps_the_last_stage() {
    use crate::portrait::GlueScene;
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, _world) = char_select_app(&[], "0", true);
    until(&mut app, "the roster load", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert_eq!(p.scene, Some(GlueScene::Orc));
    assert!(p.look.is_none());
    let fog = p.fog.expect("the empty roster's scene is fogged");
    assert_eq!((fog.near, fog.far), (0.0, 270.0));

    let mut tauren = crate::char_select::test_character(0x41, "Moo");
    (tauren.race, tauren.zone) = (6, 12);
    let (mut app, _realmd, world) = char_select_app(&[tauren], "0", true);
    until(&mut app, "the roster load", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert_eq!((p.scene, p.fog), (Some(GlueScene::Tauren), None));
    assert!(p.look.is_some());

    world.set_roster(&[]);
    let before = seen(&app, "CHARACTER_LIST_UPDATE");
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    until(&mut app, "the empty roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > before
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert_eq!((p.scene, p.fog), (Some(GlueScene::Tauren), None));
    assert!(p.look.is_none(), "the records are freed and detached");
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// The roster's race and class names are the DBC rows' `Name_Lang`, the file string the race
/// row's, and a race or class with no row reads empty (`0x473336`, `0x47336e`); with no catalogs
/// loaded every row is missing.
#[test]
fn the_roster_names_come_from_the_dbc_rows() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let create = benilla_formats::CharCreateCatalog::load(&mut chain).expect("ChrRaces");
    let classes = benilla_formats::load_chr_classes(&mut chain).expect("ChrClasses");
    let creatures = crate::entities::Creatures::from_catalog(
        benilla_formats::load_creature_catalog(&mut chain).expect("creatures"),
    );
    let names = RosterNames {
        areas: None,
        create: Some(&create),
        classes: Some(&classes),
        creatures: Some(&creatures),
    };
    let mut c = crate::char_select::test_character(1, "Alpha");
    (c.race, c.class, c.gender) = (5, 9, 1);
    let g = glue_character(&c, &names);
    assert_eq!(
        (
            g.race.as_str(),
            g.class.as_str(),
            g.file_string.as_str(),
            g.model
        ),
        ("Undead", "Warlock", "Scourge", true)
    );
    (c.race, c.class) = (99, 6);
    let g = glue_character(&c, &names);
    assert_eq!(
        (
            g.race.as_str(),
            g.class.as_str(),
            g.file_string.as_str(),
            g.model
        ),
        ("", "", "", false)
    );
    // The race row's display for the gender byte, nothing for any other (`0x5ec4c0`): a model
    // only for 0 and 1; and none without the model rows behind the display.
    (c.race, c.gender) = (1, 2);
    assert!(!glue_character(&c, &names).model);
    c.gender = 0;
    assert!(glue_character(&c, &names).model);
    let no_models = RosterNames {
        creatures: None,
        ..RosterNames {
            areas: None,
            create: Some(&create),
            classes: Some(&classes),
            creatures: None,
        }
    };
    assert!(!glue_character(&c, &no_models).model);
    let none = RosterNames {
        areas: None,
        create: None,
        classes: None,
        creatures: None,
    };
    c.race = 1;
    let g = glue_character(&c, &none);
    assert_eq!(
        (g.race.as_str(), g.file_string.as_str(), g.model),
        ("", "", false)
    );
}

/// The sequence `GlueDialog`'s SUGGEST_REALM and REALM_IS_FULL `OnAccept` run: the character
/// screen is put up, which asks for its list, and `ChangeRealm` follows in the same Lua call. The
/// reference sends that `CMSG_CHAR_ENUM` on the old link at once and `ConnectToRealm` then only
/// drops the link, so the old session sees that ask whichever of the two requests the IO thread
/// meets first, its reply completes nothing, and the new realm's session is asked once by the
/// screen's show after the connect and loads its own roster.
///
/// The IO thread is held in an earlier ask's slow reply while both requests queue, so that both
/// are ready when it comes back and its choice between them is random; repeated, so a lost ask
/// shows.
#[test]
fn showing_the_screen_then_changing_realm_asks_the_old_link_once() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut alpha = crate::char_select::test_character(0x21, "Oldrealm");
    alpha.zone = 12;
    let mut bravo = crate::char_select::test_character(0x31, "Newrealm");
    bravo.zone = 12;
    let (alpha, bravo) = (vec![alpha], vec![bravo]);
    for round in 0..8 {
        let (mut app, _realmd, world) = char_select_app_on(
            |world| {
                vec![
                    FakeRealm::new("Alpha", world),
                    FakeRealm::new("Bravo", world),
                ]
            },
            &[&alpha, &bravo],
            "0",
            true,
        );
        until(&mut app, "the first roster", |app| {
            seen(app, "CHARACTER_LIST_UPDATE") >= 2
                && app.world().resource::<GlueMgr>().state == ScreenState::Idle
        });
        assert_eq!(vm(&app).glue_roster_guids(), [0x21], "round {round}");
        // The list the realm verbs read: built where the wizard's or the list's events build it.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .rebuild_glue_realms();
        // Odd rounds leave the IO thread idle, so it wakes on whichever request is sent first.
        let busy = round % 2 == 0;
        if busy {
            world.delay_rosters(std::time::Duration::from_millis(150));
            vm(&app).run("GetCharacterListUpdate()").unwrap();
            app.update();
        }
        vm(&app)
            .run("SetGlueScreen('charselect') ChangeRealm(1, 2)")
            .unwrap();
        until(&mut app, "the new realm's roster", |app| {
            app.world().resource::<GlueMgr>().state == ScreenState::Idle
                && vm(app).glue_roster_guids() == [0x31]
        });
        assert_eq!(vm(&app).errors(), Vec::<String>::new(), "round {round}");
        assert_eq!(
            vm(&app)
                .cvar(crate::realm_select::CVAR_REALM_NAME)
                .as_deref(),
            Some("Bravo")
        );
        assert_eq!(
            (world.enumerations_in(0), world.enumerations_in(1)),
            (if busy { 3 } else { 2 }, 1),
            "round {round}: the first show, the slow ask when there was one, the show made with \
             the realm change; then the new realm's own show"
        );
    }
}

/// A reply that comes after its query was cancelled is dropped (the dialog's Cancel while the IO
/// thread is still waiting on the server): the roster the screen holds is not replaced, and the
/// realm list the cancel raises comes up.
#[test]
fn a_roster_reply_after_the_dialogs_cancel_is_dropped() {
    let _ = benilla_formats::wow_data_or_skip!();
    let before = vec![crate::char_select::test_character(0x41, "Before")];
    let (mut app, _realmd, world) = char_select_app(&before, "0", true);
    until(&mut app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2
            && app.world().resource::<GlueMgr>().state == ScreenState::Idle
    });
    world.set_roster(&[crate::char_select::test_character(0x42, "Later")]);
    world.delay_rosters(std::time::Duration::from_millis(400));
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    app.update();
    assert_eq!(
        app.world().resource::<GlueMgr>().state,
        ScreenState::CharList
    );
    vm(&app).run("StatusDialogClick()").unwrap();
    until(&mut app, "the realm list", |app| shown(app, "RealmList"));
    // The late reply has gone by (the IO thread served the realm list after it).
    assert_eq!(world.enumerations(), 2);
    assert_eq!(
        app.world().resource::<GlueMgr>().roster[0].name,
        "Before",
        "the late reply replaced the held roster"
    );
    assert_eq!(
        vm(&app).eval::<f64>("return GetNumCharacters()").unwrap(),
        0.0,
        "and nothing loaded it"
    );
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

// ── The server alert ──

/// A body of the shape the 2007 alert had: the prefix, a dated heading, paragraphs split by blank
/// lines, a list of realm names one to a line, and a final newline; plain text, about 1 KB.
fn alert_body() -> String {
    let mut body = String::from(
        "SERVERALERT:\n[January 17, 2007 - 5:00PM PST]\n\n\
         In order to address player queues on highly populated realms, we are offering free \
         character transfer moves to players on the realms listed below.\n\n\
         Please note: if the populations of these realms are not reduced by free character \
         moves, we will be forced to perform realm splits to reduce their population to \
         acceptable levels.\n\n\
         For full details, please check the latest forum sticky posts.\n\n",
    );
    for realm in [
        "Alexstrasza",
        "Aggramar",
        "Argent Dawn",
        "Azjol-Nerub",
        "Blackwater Raiders",
        "Bloodhoof",
        "Bronzebeard",
        "Cenarius",
        "Doomhammer",
        "Thunderhorn",
        "Whisperwind",
        "Zul'jin",
    ] {
        body.push_str(realm);
        body.push('\n');
    }
    body
}

/// A server that answers the next request with `body` and closes; its URL.
fn serve_body(body: &str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    raw.extend_from_slice(body.as_bytes());
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut conn, _)) = listener.accept() {
            // The whole request head first, then the answer.
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && conn.read(&mut byte).unwrap_or(0) == 1 {
                head.push(byte[0]);
            }
            let _ = conn.write_all(&raw);
        }
    });
    format!("http://127.0.0.1:{port}/alert")
}

/// A fetched alert reaches the stock login screen: the status dialog's button runs first
/// (`0x46b947`), so a logon in flight is abandoned, `SHOW_SERVER_ALERT` fires once with the text
/// after the prefix (`0x46b957`), and the stock handler shows `ServerAlertFrame` with it.
#[test]
fn a_fetched_alert_cancels_the_logon_and_shows_the_alert_frame() {
    let _ = benilla_formats::wow_data_or_skip!();
    let body = alert_body();
    let url = serve_body(&body);
    let (mut app, rx, abandon) = stock_app();
    vm(&app)
        .run(
            "probe = CreateFrame('Frame') probe:RegisterEvent('SHOW_SERVER_ALERT') \
             probe:SetScript('OnEvent', function() ALERT_ARG = arg1 \
             ALERT_CALLS = (ALERT_CALLS or 0) + 1 end)",
        )
        .unwrap();
    vm(&app).run("AcceptEULA() AcceptTOS()").unwrap();
    app.update();
    type_and_login(&mut app);
    assert!(rx.try_recv().is_ok());
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Logon);
    assert!(!shown(&app, "ServerAlertFrame"));

    let fetch = crate::ui_script::server_alert::spawn_fetch(url);
    app.world_mut()
        .resource_mut::<GlueMgr>()
        .set_alert_fetch(fetch);
    until(&mut app, "the alert frame", |app| {
        shown(app, "ServerAlertFrame")
    });

    // After the prefix, short of the last byte (`SStrCopy` of the whole body, `0x46a59f`).
    let want = &body[b"SERVERALERT:".len()..body.len() - 1];
    assert_eq!(vm(&app).eval::<String>("return ALERT_ARG").unwrap(), want);
    assert_eq!(abandon.load(Ordering::SeqCst), 1, "the logon was abandoned");
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Idle);

    // Once: nothing refires it.
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(vm(&app).eval::<f64>("return ALERT_CALLS").unwrap(), 1.0);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A body without the prefix, or a fetch that finds none, shows nothing and leaves the logon be.
#[test]
fn a_body_without_the_prefix_shows_no_alert() {
    let _ = benilla_formats::wow_data_or_skip!();
    let url = serve_body("<html>Not an alert.</html>\n");
    let (mut app, rx, abandon) = stock_app();
    vm(&app).run("AcceptEULA() AcceptTOS()").unwrap();
    app.update();
    type_and_login(&mut app);
    assert!(rx.try_recv().is_ok());
    let fetch = crate::ui_script::server_alert::spawn_fetch(url);
    app.world_mut()
        .resource_mut::<GlueMgr>()
        .set_alert_fetch(fetch);
    until(&mut app, "the fetch to end", |app| {
        app.world().resource::<GlueMgr>().alert_fetch.is_none()
    });
    app.update();
    assert!(!shown(&app, "ServerAlertFrame"));
    assert_eq!(abandon.load(Ordering::SeqCst), 0);
    assert_eq!(app.world().resource::<GlueMgr>().state, ScreenState::Logon);
}

/// `LaunchURL` reaches the host as a launch message, for a whitelisted URL only.
#[test]
fn launch_url_reaches_the_host_for_a_whitelisted_url() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _abandon) = stock_app();
    vm(&app)
        .run("LaunchURL('http://example.com/') LaunchURL('http://www.worldofwarcraft.com/news')")
        .unwrap();
    app.update();
    let launched: Vec<String> = app
        .world()
        .resource::<Messages<crate::ui_script::server_alert::LaunchUrl>>()
        .iter_current_update_messages()
        .map(|m| m.0.clone())
        .collect();
    assert_eq!(launched, ["http://www.worldofwarcraft.com/news"]);
}
