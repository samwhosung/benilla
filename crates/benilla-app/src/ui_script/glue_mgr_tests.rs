use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::client_state::ClientState;
use crate::net::{
    AddonInfoMessage, AddonInfoReply, LoginAbandon, LoginRequest, LoginSubmit, RosterFailedMessage,
    WorldAuthMessage,
};
use benilla_protocol::{RealmInfo, SessionEnd};

/// Run the steps the outbox holds past their signals and drop them, as the frame's flush does
/// where it reaches them: what a test of the manager alone sees once the frame has run.
fn settle(m: &mut GlueMgr) {
    m.settle();
}

/// A manager as the app starts it.
fn fresh() -> GlueMgr {
    GlueMgr::default()
}

/// The manager the shared handle holds.
fn mgr(app: &App) -> std::cell::Ref<'_, GlueMgr> {
    app.world().non_send_resource::<SharedGlueMgr>().get()
}

fn mgr_mut(app: &App) -> std::cell::RefMut<'_, GlueMgr> {
    app.world().non_send_resource::<SharedGlueMgr>().get_mut()
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
    let mut m = fresh();
    m.login("a".into(), "b".into());
    settle(&mut m);
    m.on_stage(LoginStage::Connecting, true);
    m.tick();
    settle(&mut m);
    m.outbox.clear();
    m.on_failed(&failed(Some(LoginRefusal::LogonProof(0x0a)), false));
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "{:?}", m.outbox);
    assert_eq!(m.state, ScreenState::Logon);
}

/// `CGlueMgr::Login` past the agreements (the VM's): nothing outside state 0, an `OKAY` dialog for
/// an empty box, else the `CANCEL` dialog with no text and the attempt.
#[test]
fn login_guards_then_starts_the_logon() {
    let mut m = fresh();
    m.login(String::new(), "b".into());
    settle(&mut m);
    assert_eq!(m.outbox, [open("OKAY", Some("LOGIN_ENTER_NAME"))]);
    m.outbox.clear();
    m.login("a".into(), String::new());
    settle(&mut m);
    assert_eq!(m.outbox, [open("OKAY", Some("LOGIN_ENTER_PASSWORD"))]);
    assert!(m.login_out.is_empty());
    m.outbox.clear();

    m.login("a".into(), "b".into());
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Logon);
    assert_eq!(m.outbox, [open("CANCEL", None)]);
    assert!(matches!(
        m.login_out.as_slice(),
        [GlueLogin::Submit { account, password }] if account == "a" && password == "b"
    ));
    m.login("a".into(), "b".into());
    settle(&mut m);
    assert_eq!(m.login_out.len(), 1, "a second login waits for state 0");
}

/// The logon's dialogs come off its `(state, result)` pair once per change (`0x46b140`); the
/// authenticated state moves to the world connect on the same tick.
#[test]
fn the_logon_shows_each_state_once_then_connects() {
    let mut m = fresh();
    m.login("a".into(), "b".into());
    settle(&mut m);
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "the cleared cells show nothing");
    m.on_stage(LoginStage::Connecting, true);
    m.tick();
    settle(&mut m);
    m.tick();
    settle(&mut m);
    assert_eq!(m.outbox, [open("CANCEL", Some("LOGIN_STATE_CONNECTING"))]);
    m.outbox.clear();
    m.on_logon_done();
    m.tick();
    settle(&mut m);
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
        let mut m = fresh();
        m.login("a".into(), "b".into());
        settle(&mut m);
        m.outbox.clear();
        m.on_failed(&failed(refusal, false));
        m.tick();
        settle(&mut m);
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

/// A connect waiting on its list, nothing selected this process.
fn connecting() -> GlueMgr {
    let mut m = fresh();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m
}

/// The realm the connect dialled, if any.
fn dialled(m: &GlueMgr) -> Option<&str> {
    m.park_out.iter().find_map(|r| match r {
        ParkRequest::Realm(RealmRequest::Enter { realm, .. }) => Some(realm.name.as_str()),
        _ => None,
    })
}

/// The `realmName` the manager wrote, if any.
fn realm_name_set(m: &GlueMgr) -> Option<&str> {
    m.outbox.iter().find_map(|o| match o {
        Outgoing::Cvar { name, value } if *name == crate::glue_session::realm::CVAR_REALM_NAME => {
            Some(value.as_str())
        }
        _ => None,
    })
}

/// The run's own realm choice on the stock connect (#1433): `WOW_REALM` names a listed realm
/// that is up, ahead of `realmName`; an unattended run with nothing named takes the first realm
/// up with the most characters. Either is a selection: `realmName` is written and a realm is
/// chosen this session. A player's run with no `realmName` fails `REALM_LIST_REALM_NOT_FOUND`
/// into the list, and `realmName`'s own match writes nothing.
#[test]
fn the_runs_realm_choice_answers_the_stock_connect() {
    let mut down = realm("Down");
    down.flags = 0x02;
    let mut played = realm("Played");
    played.characters = 3;
    let list = || vec![down.clone(), realm("Alpha"), realm("Bravo"), played.clone()];
    let connect = |realm_name: &str, run: RunModeRealm| {
        let mut m = connecting();
        m.on_realm_list(list(), false, realm_name, &run);
        m
    };

    let m = connect("alpha", RunModeRealm::new(Some("bravo"), false));
    assert_eq!(dialled(&m), Some("Bravo"), "WOW_REALM outranks realmName");
    assert_eq!(realm_name_set(&m), Some("Bravo"));
    assert_eq!(m.realm_cells.chosen, Some(0));
    assert!(m.cells_moved, "the chosen cell reaches the VM");

    let m = connect("alpha", RunModeRealm::new(Some("Down"), true));
    assert_eq!(
        dialled(&m),
        Some("Alpha"),
        "a WOW_REALM that is down falls through to realmName"
    );
    assert_eq!(
        realm_name_set(&m),
        None,
        "realmName's own match writes nothing"
    );
    assert_eq!(m.realm_cells.chosen, None);

    let m = connect("", RunModeRealm::new(None, true));
    assert_eq!(dialled(&m), Some("Played"), "the unattended pick");
    assert_eq!(realm_name_set(&m), Some("Played"));
    assert_eq!(m.realm_cells.chosen, Some(0));

    let m = connect("Gone", RunModeRealm::default());
    assert_eq!(dialled(&m), None, "a player's run dials nothing");
    assert_eq!(realm_name_set(&m), None);
    assert!(m.op.done && !m.op.ok);
    assert_eq!(m.op.status, STATUS_REALM_LIST_REALM_NOT_FOUND);
    assert_eq!(m.realm_cells.chosen, None);

    let mut m = connecting();
    m.selected = Some(realm("Alpha"));
    m.on_realm_list(list(), false, "", &RunModeRealm::new(Some("Bravo"), true));
    assert_eq!(
        dialled(&m),
        Some("Alpha"),
        "the realm selected this process outranks the run's choice"
    );
    assert_eq!(realm_name_set(&m), None);
}

/// The world connect's failure arm: the status text, `OKAY_WITH_URL` with the URL key for the
/// five codes of `0x803740`, then the realm list over it (`CLOSE_STATUS_DIALOG` and the rebuild);
/// `AUTH_FAILED` instead goes from the character screen back to login.
#[test]
fn a_world_refusal_opens_its_status_and_url() {
    let connected = || {
        let mut m = fresh();
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
    settle(&mut m);
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
    m.on_failed(&failed(Some(LoginRefusal::World(0x0d)), false));
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [
            update("AUTH_FAILED"),
            open("OKAY", Some("AUTH_FAILED")),
            Outgoing::ScreenFrom {
                current: "charselect",
                name: "login"
            }
        ]
    );

    let mut m = connected();
    m.on_failed(&failed(None, false));
    m.tick();
    settle(&mut m);
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
    settle(&mut m);
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
    let mut m = fresh();
    m.login("a".into(), "b".into());
    settle(&mut m);
    m.on_logon_done();
    m.tick();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Connect);
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "the running connect's text is empty");
    m.on_stage(LoginStage::WorldConnected, true);
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [Outgoing::Update {
            text: Text::Key("RESPONSE_CONNECTED"),
            button: None
        }]
    );
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(
        m.outbox.is_empty(),
        "the running authentication's text is empty"
    );
    m.on_world_authenticated(realm("Fake"), 0);
    settle(&mut m);
    m.tick();
    settle(&mut m);
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
    let mut m = fresh();
    m.state = ScreenState::Connect;
    let alpha = || vec![crate::glue_session::roster::test_character(7, "Alpha")];
    m.on_roster(alpha(), RosterCause::Asked, 0);
    m.on_roster(alpha(), RosterCause::Unasked, 0);
    m.tick();
    settle(&mut m);
    assert!(
        m.outbox.is_empty() && m.state == ScreenState::Connect,
        "a roster does not complete the connect: {:?}",
        m.outbox
    );
    m.on_world_authenticated(realm("Fake"), 0);
    settle(&mut m);
    m.tick();
    settle(&mut m);
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
    assert_eq!(m.selected.as_ref().map(|r| r.name.as_str()), Some("Fake"));
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
    let mut m = fresh();
    m.connected = true;
    for (id, name) in [(7, "Alpha"), (8, "Bravo")] {
        m.get_character_list();
        settle(&mut m);
        assert_eq!(m.state, ScreenState::CharList);
        assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_LIST_RETRIEVING"))]);
        assert!(matches!(
            m.park_out.as_slice(),
            [ParkRequest::Char(CharRequest::Enumerate)]
        ));
        m.park_out.clear();
        m.outbox.clear();
        m.tick();
        settle(&mut m);
        assert!(m.outbox.is_empty(), "a running query shows no text");
        // A roster nothing asked for is not this screen's.
        m.on_roster(
            vec![crate::glue_session::roster::test_character(1, "Stray")],
            RosterCause::Unasked,
            0,
        );
        m.tick();
        settle(&mut m);
        assert!(m.outbox.is_empty());
        assert!(m.roster.iter().all(|c| c.name != "Stray"));
        m.on_roster(
            vec![crate::glue_session::roster::test_character(id, name)],
            RosterCause::Asked,
            0,
        );
        m.tick();
        settle(&mut m);
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
    let late = || vec![crate::glue_session::roster::test_character(9, "Late")];
    let mut m = fresh();
    m.connected = true;
    m.get_character_list();
    settle(&mut m);
    m.park_out.clear();
    m.status_dialog_click();
    settle(&mut m);
    m.tick();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::RealmList);
    m.outbox.clear();
    m.on_roster(late(), RosterCause::Asked, 0);
    m.tick();
    settle(&mut m);
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
    let mut m = fresh();
    m.connected = true;
    m.get_character_list();
    settle(&mut m);
    m.status_dialog_click();
    settle(&mut m);
    m.on_roster(late(), RosterCause::Asked, 0);
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.outbox.iter().any(|o| matches!(o, Outgoing::Roster)));
    assert_eq!(m.roster[0].name, "Late");

    // Deviation: the reference's handler completes whichever operation is current; a reply while
    // a realm change's connect is the operation completes nothing here.
    let mut m = fresh();
    m.connected = true;
    m.holder = vec![realm("Fake")];
    m.get_character_list();
    settle(&mut m);
    m.connect_to_realm("fake");
    settle(&mut m);
    m.reconnect();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Connect);
    m.on_roster(late(), RosterCause::Asked, 0);
    m.tick();
    settle(&mut m);
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
    let mut m = fresh();
    m.connected = true;
    m.on_world_authenticated(realm("Fake"), 5);
    settle(&mut m);
    m.get_character_list();
    settle(&mut m);
    m.on_roster(late(), RosterCause::Asked, 4);
    m.tick();
    settle(&mut m);
    assert!(m.roster.is_empty() && !m.op.done);
    m.on_roster(late(), RosterCause::Asked, 5);
    assert!(m.op.done && m.op.ok);
}

/// A server that wants Warden refuses the roster ask with its words (a terminal login failure):
/// the query completes failed and the next tick opens `OKAY` with them (`0x46c173`), where state 3
/// would otherwise stand on its `CANCEL` dialog for ever. The link is gone.
#[test]
fn a_terminal_failure_fails_the_character_list_query_with_its_words() {
    let mut m = fresh();
    m.connected = true;
    m.get_character_list();
    settle(&mut m);
    m.park_out.clear();
    m.outbox.clear();
    m.on_failed(&failed(None, true));
    m.tick();
    settle(&mut m);
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
    let mut m = fresh();
    m.connected = true;
    m.on_world_authenticated(realm("Fake"), 3);
    settle(&mut m);
    m.get_character_list();
    settle(&mut m);
    m.outbox.clear();
    m.on_roster_failed(2);
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty() && !m.op.done, "another link's failure");
    m.on_roster_failed(3);
    m.tick();
    settle(&mut m);
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
    assert!(!mgr(&app).connected);
    app.world_mut().write_message(auth(1));
    app.update();
    assert!(mgr(&app).connected);
}

/// With no world link `0x5aabb0` fails the query `RESPONSE_FAILED_TO_CONNECT` at once, and the
/// arm opens `OKAY` with that text (`0x46c173`) and goes to state 0.
#[test]
fn a_character_list_query_with_no_link_fails_okay() {
    let mut m = fresh();
    m.get_character_list();
    settle(&mut m);
    assert!(m.park_out.is_empty());
    m.outbox.clear();
    m.tick();
    settle(&mut m);
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
    let mut m = fresh();
    m.connected = true;
    m.get_character_list();
    settle(&mut m);
    m.park_out.clear();
    m.outbox.clear();
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.outbox, [Outgoing::Teardown]);
    m.outbox.clear();
    m.tick();
    settle(&mut m);
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

/// Change Realm with the world up drops the session in the call, its flag down at once, and
/// leaves the screen state and the dialog to the drop's handler (`0x46b22b`-`0x46b241`), whose
/// connect then asks realmd behind the drop (`0x46c585`-`0x46c5bf`).
#[test]
fn change_realm_with_the_world_up_leaves_it_first() {
    let mut m = fresh();
    m.holder = vec![realm("Fake")];
    m.connected = true;
    m.connect_to_realm("fake");
    settle(&mut m);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Leave)]
    ));
    assert_eq!(m.state, ScreenState::Idle, "the call changes no state");
    assert!(m.outbox.is_empty(), "and opens no dialog");
    assert!(!m.connected);
    assert_eq!(m.selected.as_ref().map(|r| r.name.as_str()), Some("Fake"));
    m.reconnect();
    settle(&mut m);
    assert!(matches!(
        m.park_out.as_slice(),
        [
            ParkRequest::Realm(RealmRequest::Leave),
            ParkRequest::Realm(RealmRequest::Refresh)
        ]
    ));
    assert_eq!(m.state, ScreenState::Connect);
    assert_eq!(m.outbox, [open("CANCEL", Some("GAME_SERVER_LOGIN"))]);
    m.outbox.clear();
    m.reconnect();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "the handler's reconnect runs once");

    let mut m = fresh();
    m.holder = vec![realm("Fake")];
    m.connect_to_realm("Fake");
    settle(&mut m);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Refresh)]
    ));
}

/// The queue re-texts the dialog every frame while the world connect waits.
#[test]
fn the_queue_retexts_every_frame() {
    let mut m = fresh();
    m.state = ScreenState::Connect;
    m.on_queued(Some(40), Some("Realm".into()), 1000);
    m.tick();
    settle(&mut m);
    m.tick();
    settle(&mut m);
    assert_eq!(m.outbox, [Outgoing::Queue, Outgoing::Queue]);
}

/// The estimate is stored by the packets (`0x46aeda`, `0xb41df0`), counts down between them, and
/// the next connect clears it with the samples (`0x5aab45`).
#[test]
fn the_queue_estimate_stands_between_packets_and_a_connect_clears_it() {
    let mut m = fresh();
    m.state = ScreenState::Connect;
    for (i, place) in (93..=100).rev().enumerate() {
        m.on_queued(Some(place), Some("Realm".into()), 1_000 + 10_000 * i as u32);
    }
    let last = 1_000 + 70_000;
    let shown = |m: &GlueMgr, now| {
        m.queue.text_by(now, Some("Realm"), |key| match key {
            "QUEUE_NAME_TIME_LEFT" => "%s %d %d".to_string(),
            other => other.to_string(),
        })
    };
    assert_eq!(shown(&m, last), "Realm 93 15");
    assert_eq!(shown(&m, last + 300_000), "Realm 93 10");
    assert_eq!(
        shown(&m, last + 1_000_000),
        "QUEUE_NAME_TIME_LEFT_SECONDS",
        "past zero"
    );
    m.begin_connect();
    settle(&mut m);
    assert_eq!(
        shown(&m, last + 300_000),
        "QUEUE_NAME_TIME_LEFT_UNKNOWN",
        "a connect starts the queue over"
    );
}

/// `StatusDialogClick` by state (`0x46b7a8`): the logon is cancelled; the world connect completes
/// `RESPONSE_CANCELLED`, whose failure arm shows no dialog, drops the world attempt alone and
/// raises the realm list; a query is cancelled to state 0; idle does nothing.
#[test]
fn status_dialog_click_cancels_by_state() {
    let mut m = fresh();
    m.status_dialog_click();
    settle(&mut m);
    assert!(m.login_out.is_empty());
    m.state = ScreenState::Logon;
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(matches!(m.login_out.as_slice(), [GlueLogin::Abandon]));

    let mut m = fresh();
    m.holder = vec![realm("Fake")];
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
    m.status_dialog_click();
    settle(&mut m);
    assert!(m.login_out.is_empty(), "the logon stays up");
    assert_eq!(m.state, ScreenState::Connect, "the next tick takes it");
    m.tick();
    settle(&mut m);
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

    let mut m = fresh();
    m.request_realm_list(false);
    settle(&mut m);
    assert_eq!(m.state, ScreenState::RealmList);
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "a cancelled query reports nothing");
}

/// `0x46b2e0` and state 6: the dialog and the `CMSG_CHAR_DELETE`, the dialog re-texted while the
/// reply is out; a success (`0x39`) fires `SELECT_FIRST_CHARACTER` and asks for the list
/// (`0x46c19d`-`0x46c1a7`), a failure opens `OKAY` with its status's text and goes to state 0
/// (`0x46c173`).
#[test]
fn a_delete_runs_state_6_to_the_list_or_to_okay() {
    let mut m = fresh();
    m.connected = true;
    m.delete_character(0x12);
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Delete);
    assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_DELETE_IN_PROGRESS"))]);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Delete(0x12))]
    ));
    m.park_out.clear();
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "no text while the reply is out");
    m.on_char_deleted(0x39);
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("CHAR_DELETE_SUCCESS"),
                button: None,
            },
            Outgoing::SelectFirst,
            open("CANCEL", Some("CHAR_LIST_RETRIEVING")),
        ]
    );
    assert_eq!(m.state, ScreenState::CharList);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Enumerate)]
    ));

    // vmangos refuses a guild master's delete with `CHAR_DELETE_FAILED` (0x3a).
    let mut m = fresh();
    m.connected = true;
    m.delete_character(0x12);
    settle(&mut m);
    m.outbox.clear();
    m.on_char_deleted(0x3a);
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("CHAR_DELETE_FAILED"),
                button: None,
            },
            open("OKAY", Some("CHAR_DELETE_FAILED")),
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
}

/// With no link the delete fails `RESPONSE_FAILED_TO_CONNECT` at once (`0x5ab11e`) and sends
/// nothing; the dialog's button in state 6 cancels it to state 0 (`0x46b776`), and a reply that
/// lands after shows nothing.
#[test]
fn a_delete_with_no_link_fails_and_a_cancelled_one_shows_nothing() {
    let mut m = fresh();
    m.delete_character(0x12);
    settle(&mut m);
    assert!(m.park_out.is_empty());
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox.last(),
        Some(&open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT")))
    );
    assert_eq!(m.state, ScreenState::Idle);

    let mut m = fresh();
    m.connected = true;
    m.delete_character(0x12);
    settle(&mut m);
    m.outbox.clear();
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    m.on_char_deleted(0x39);
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty());
}

/// A create request with the name and the bytes as `CreateCharacter` reads them.
fn create_request(name: &str) -> benilla_ui::script::GlueCreateRequest {
    benilla_ui::script::GlueCreateRequest {
        name: name.into(),
        race: 2,
        class: 3,
        sex: 1,
        skin: 4,
        face: 5,
        hair_style: 6,
        hair_color: 7,
        facial_hair: 0,
    }
}

/// `0x46b290` and state 5: the dialog and the `CMSG_CHAR_CREATE`, the dialog re-texted once the
/// reply is in; a success (`0x2e`) goes to state 0, closes the dialog, fires
/// `SELECT_LAST_CHARACTER` and puts `charselect` up (`0x46c11e`-`0x46c13d`), a failure opens
/// `OKAY` with its status's text and goes to state 0 (`0x46c118` → `0x46c177`).
#[test]
fn a_create_runs_state_5_to_the_select_screen_or_to_okay() {
    let mut m = fresh();
    m.connected = true;
    m.create_character(create_request("Newbie"));
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Create);
    assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_CREATE_IN_PROGRESS"))]);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Create(r))]
            if r.name == "Newbie"
                && (r.race, r.class, r.gender, r.skin, r.face) == (2, 3, 1, 4, 5)
                && (r.hair_style, r.hair_color, r.facial_hair) == (6, 7, 0)
    ));
    m.park_out.clear();
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "no text while the reply is out");
    // Another operation's reply is not the create's.
    m.on_char_deleted(0x39);
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty());
    m.on_char_created(0x2e);
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("CHAR_CREATE_SUCCESS"),
                button: None,
            },
            Outgoing::Close,
            Outgoing::SelectLast,
            Outgoing::Screen("charselect"),
        ]
    );
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.park_out.is_empty(), "the screen asks for the list itself");

    // vmangos's taken name, `CHAR_CREATE_NAME_IN_USE` (0x31), and a name it refuses, by its
    // own `CHAR_NAME_*` code.
    for (code, key) in [
        (0x31, "CHAR_CREATE_NAME_IN_USE"),
        (0x30, "CHAR_CREATE_FAILED"),
        (0x4e, "CHAR_NAME_THREE_CONSECUTIVE"),
    ] {
        let mut m = fresh();
        m.connected = true;
        m.create_character(create_request("Newbie"));
        settle(&mut m);
        m.outbox.clear();
        m.on_char_created(code);
        m.tick();
        settle(&mut m);
        assert_eq!(
            m.outbox,
            [
                Outgoing::Update {
                    text: Text::Key(key),
                    button: None,
                },
                open("OKAY", Some(key)),
            ],
            "{code:#x}"
        );
        assert_eq!(m.state, ScreenState::Idle);
    }
}

/// With no link the create fails `RESPONSE_FAILED_TO_CONNECT` at once (`0x5aac72`) and sends
/// nothing; the dialog's button in state 5 cancels it to state 0 (`0x46b776`), and a reply that
/// lands after shows nothing.
#[test]
fn a_create_with_no_link_fails_and_a_cancelled_one_shows_nothing() {
    let mut m = fresh();
    m.create_character(create_request("Newbie"));
    settle(&mut m);
    assert!(m.park_out.is_empty());
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox.last(),
        Some(&open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT")))
    );
    assert_eq!(m.state, ScreenState::Idle);

    let mut m = fresh();
    m.connected = true;
    m.create_character(create_request("Newbie"));
    settle(&mut m);
    m.outbox.clear();
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    m.on_char_created(0x2e);
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty());
}

/// State 7: the dialog and the `CMSG_CHAR_RENAME` (`0x46b39b`), sent only on a live link; no
/// frame arm; the button goes to state 0 and sends nothing (`0x46b79d`).
#[test]
fn a_rename_takes_state_7_which_only_its_button_leaves() {
    let mut m = fresh();
    m.connected = true;
    m.rename_character(0x12, "Newname".into());
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Rename);
    assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_RENAME_IN_PROGRESS"))]);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Rename { guid: 0x12, name })] if name == "Newname"
    ));
    m.outbox.clear();
    m.park_out.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty() && m.park_out.is_empty());
    assert_eq!(m.state, ScreenState::Rename);
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.outbox.is_empty() && m.park_out.is_empty());

    let mut m = fresh();
    m.rename_character(0x12, "Newname".into());
    settle(&mut m);
    assert!(m.park_out.is_empty(), "no link, nothing sent");
}

/// `0x46b440`: `CLOSE_STATUS_DIALOG` first, then 0 renames by the reply's guid, `0x31` is
/// `CHAR_CREATE_NAME_IN_USE`, anything else `CHAR_RENAME_FAILED`; the state stays.
#[test]
fn the_rename_reply_closes_the_dialog_then_renames_or_asks_again() {
    let mut m = fresh();
    m.state = ScreenState::Rename;
    for (result, renamed, then) in [
        (
            0,
            Some((0x12, "Newname".to_string())),
            Outgoing::Renamed {
                guid: 0x12,
                name: "Newname".into(),
            },
        ),
        (
            0,
            None,
            Outgoing::Renamed {
                guid: 0,
                name: String::new(),
            },
        ),
        (0x31, None, Outgoing::ForceRename("CHAR_CREATE_NAME_IN_USE")),
        (0x2f, None, Outgoing::ForceRename("CHAR_RENAME_FAILED")),
        (0x4b, None, Outgoing::ForceRename("CHAR_RENAME_FAILED")),
    ] {
        m.outbox.clear();
        m.on_char_renamed(result, renamed);
        assert_eq!(m.outbox, [Outgoing::Close, then], "{result:#x}");
        assert_eq!(m.state, ScreenState::Rename);
    }
}

/// A session that is over fires `DISCONNECTED_FROM_SERVER`; an unattended reconnect does not; in
/// the world connect, a completed failed operation opens `OKAY` with its text instead
/// (`0x46c671`-`0x46c68f`), and a running one still fires the event.
#[test]
fn a_lost_session_fires_disconnected() {
    let mut m = fresh();
    m.on_disconnected(SessionEnd::Lost, false);
    assert!(m.outbox.is_empty());
    m.on_disconnected(SessionEnd::Lost, true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = fresh();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m.on_disconnected(SessionEnd::Lost, true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = fresh();
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
    let mut m = fresh();
    m.connected = true;
    m.on_disconnected(SessionEnd::LoggedOut, false);
    assert!(m.connected, "the link is kept");
    assert!(m.outbox.is_empty());
    m.get_character_list();
    settle(&mut m);
    let (_, park) = m.take_asks();
    assert!(
        matches!(park[..], [ParkRequest::Char(CharRequest::Enumerate)]),
        "the ask goes out on the link"
    );
    assert!(!m.op.done, "and waits for its reply");
}

/// The manager on the character screen: a realm selected this process and the world link up.
fn on_select() -> GlueMgr {
    let mut m = fresh();
    m.selected = Some(realm("Alpha"));
    m.connected = true;
    m
}

/// Nothing `EnterWorld` does: no state, no dialog, no event, no ask, no `lastCharacterIndex`.
fn nothing_done(m: &GlueMgr) -> bool {
    m.state == ScreenState::Idle
        && m.outbox.is_empty()
        && m.park_out.is_empty()
        && m.last_character.is_none()
}

/// `EnterWorld` returns having done nothing with no realm selected this process (`0x5ab9a0` at
/// `0x46b506`) or no world link (`[svc+0x1b00]`, `0x46b56c`), whatever the record's flags.
#[test]
fn enter_world_without_a_realm_or_a_link_does_nothing() {
    for (realm, link) in [(false, true), (true, false), (false, false)] {
        for flags in [0, 0x4, 0x4000] {
            let mut m = on_select();
            if !realm {
                m.selected = None;
            }
            m.connected = link;
            m.enter_world(1, 0x12, flags);
            settle(&mut m);
            assert!(
                nothing_done(&m),
                "realm {realm}, link {link}, flags {flags:#x}"
            );
        }
    }
}

/// A record locked for a transfer (`+0xfc` bit `0x4`, `0x46b57e`) opens `OKAY` with status
/// `0x44`'s text and goes no further: nothing sent, no `lastCharacterIndex`, the state as it was.
/// The test comes before the rename flag's.
#[test]
fn enter_world_on_a_character_locked_for_transfer_opens_okay() {
    for flags in [0x4, 0x4004] {
        let mut m = on_select();
        m.enter_world(1, 0x12, flags);
        settle(&mut m);
        assert_eq!(
            m.outbox,
            [open("OKAY", Some("CHAR_LOGIN_LOCKED_FOR_TRANSFER"))],
            "{flags:#x}"
        );
        assert_eq!(m.state, ScreenState::Idle);
        assert!(m.park_out.is_empty() && m.last_character.is_none());
    }
}

/// A record flagged for a rename (`0x4000`, `0x46b5b1`) fires `FORCE_RENAME_CHARACTER` with
/// `CHAR_RENAME_DESCRIPTION` (`0x46b5c2`) and goes no further.
#[test]
fn enter_world_on_a_character_flagged_for_rename_asks_for_a_name() {
    let mut m = on_select();
    m.enter_world(1, 0x12, 0x4000 | 0x2000);
    settle(&mut m);
    assert_eq!(m.outbox, [Outgoing::ForceRename("CHAR_RENAME_DESCRIPTION")]);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.park_out.is_empty() && m.last_character.is_none());
}

/// Past the guards: `lastCharacterIndex` takes the index (`0x46b5fa`), the state goes to 8
/// (`0x46b711`) and the pick goes out behind the asks made before it. Every call repoints the
/// record the first frame of state 8 stashes the login's guid from (`0x46c1d6`-`0x46c1e6`), so the
/// last call of the frame wins both the index and the pick; once the pick has left, a call writes
/// the index and picks nothing (`0x46c272` sends once). The dialog's button in state 8 cancels
/// to state 0 (`0x46b776`).
#[test]
fn enter_world_writes_the_index_and_picks_in_state_8() {
    let mut m = on_select();
    m.get_character_list();
    settle(&mut m);
    m.outbox.clear();
    m.state = ScreenState::Idle;
    m.enter_world(2, 0x13, 0x2000);
    settle(&mut m);
    assert_eq!(m.state, ScreenState::EnterWorld);
    assert_eq!(m.last_character, Some(2));
    assert_eq!(
        m.outbox,
        [Outgoing::Cvar {
            name: "lastCharacterIndex",
            value: "2".into()
        }],
        "the CVar in the call (`0x46b5fa`), and no dialog of its own"
    );
    m.outbox.clear();
    assert!(matches!(
        m.park_out.as_slice(),
        [
            ParkRequest::Char(CharRequest::Enumerate),
            ParkRequest::Char(CharRequest::Enter(0x13))
        ]
    ));
    // A second call in the same frame: the last record wins both.
    m.enter_world(1, 0x12, 0);
    settle(&mut m);
    assert_eq!(m.last_character, Some(1));
    assert!(matches!(
        m.park_out.as_slice(),
        [
            ParkRequest::Char(CharRequest::Enumerate),
            ParkRequest::Char(CharRequest::Enter(0x12))
        ]
    ));
    // The frame's asks leave.
    m.park_out.clear();
    m.last_character = None;
    m.enter_world(0, 0x11, 0);
    settle(&mut m);
    assert_eq!(m.last_character, Some(0));
    assert!(m.park_out.is_empty(), "one login per entry");
    m.outbox.clear();
    m.tick();
    settle(&mut m);
    assert!(m.outbox.is_empty(), "no frame arm for state 8 here");
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.park_out.is_empty());
}

/// `GetStatus` (`0x5aa960`) formats a status past the 83-row table (`0x5aa99d`) as `"(%i)"`
/// (`0x85d5b0`), where the last row, 0x52, still reads its key.
#[test]
fn a_world_status_past_the_table_reads_its_number() {
    let refused = |code| {
        let mut m = fresh();
        m.state = ScreenState::Connect;
        m.holder = vec![realm("Fake")];
        m.on_failed(&failed(Some(LoginRefusal::World(code)), false));
        m.tick();
        settle(&mut m);
        m.outbox
    };
    for (code, text) in [(0xc8u8, "(200)"), (0x53, "(83)"), (0xff, "(255)")] {
        assert_eq!(
            refused(code),
            [
                Outgoing::Update {
                    text: Text::Raw(text.into()),
                    button: None,
                },
                Outgoing::Open {
                    which: "OKAY",
                    text: Some(Text::Raw(text.into())),
                    data: None,
                },
                Outgoing::Teardown,
                Outgoing::Close,
                Outgoing::Rebuild,
            ],
            "status {code:#x}"
        );
    }
    assert_eq!(
        status_text(0x52).0,
        Text::Key(GLUE_STATUS_KEYS[0x52]),
        "the last row is still a key"
    );
}

/// `SMSG_CHARACTER_LOGIN_FAILED`'s byte through `0x5aad70`'s table (`0x5aae08`): 1 to 6 onto
/// `0x3e 0x3f 0x40 0x42 0x43 0x44`, everything else `CHAR_LOGIN_FAILED`.
#[test]
fn a_refused_login_maps_its_byte_through_the_references_table() {
    let got: Vec<&str> = [0u8, 1, 2, 3, 4, 5, 6, 7, 0x41, 0xff]
        .into_iter()
        .map(|b| GLUE_STATUS_KEYS[usize::from(char_login_status(b))])
        .collect();
    assert_eq!(
        got,
        [
            "CHAR_LOGIN_FAILED",
            "CHAR_LOGIN_NO_WORLD",
            "CHAR_LOGIN_DUPLICATE_CHARACTER",
            "CHAR_LOGIN_NO_INSTANCES",
            "CHAR_LOGIN_DISABLED",
            "CHAR_LOGIN_NO_CHARACTER",
            "CHAR_LOGIN_LOCKED_FOR_TRANSFER",
            "CHAR_LOGIN_FAILED",
            "CHAR_LOGIN_FAILED",
            "CHAR_LOGIN_FAILED",
        ]
    );
}

/// The rebuild a refusal comes back to is in state 9 (`0x402075`), where the screen's list ask
/// returns before anything (`0x46b880`); every frame of state 9 opens `OKAY` with the refusal's
/// text (`0x46c283`); the button goes to state 0 and asks for the list on the link (`0x46b767`).
#[test]
fn a_refused_login_takes_state_9_until_its_dialogs_button() {
    let mut m = on_select();
    m.enter_world(1, 0x12, 0);
    settle(&mut m);
    m.park_out.clear();
    m.outbox.clear();
    m.on_char_login_failed(1);
    assert_eq!(m.state, ScreenState::EnterWorld, "the rebuild sets it");
    m.state = ScreenState::Idle;
    m.take_refusal();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::LoginRefused);
    m.get_character_list();
    settle(&mut m);
    assert!(
        m.outbox.is_empty() && m.park_out.is_empty(),
        "no ask in state 9"
    );
    m.tick();
    settle(&mut m);
    m.tick();
    settle(&mut m);
    assert_eq!(
        m.outbox,
        [
            open("OKAY", Some("CHAR_LOGIN_NO_WORLD")),
            open("OKAY", Some("CHAR_LOGIN_NO_WORLD"))
        ]
    );
    m.outbox.clear();
    m.status_dialog_click();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::CharList);
    assert_eq!(m.outbox, [open("CANCEL", Some("CHAR_LIST_RETRIEVING"))]);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Enumerate)]
    ));
    m.take_refusal();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::CharList, "a refusal is taken once");

    // A session lost before the rebuild leaves no refusal for the next glue.
    let mut m = on_select();
    m.on_char_login_failed(6);
    m.on_disconnected(SessionEnd::Lost, true);
    m.take_refusal();
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
}

// ── The verbs run inside their call ─────────────────────────────────────────────────────────────

/// A bare glue VM seated with `shared` as its host, with the strings the manager's dialogs name
/// and a listener that logs each event the verbs raise as `"EVENT arg1 arg2"` into `HEARD`.
fn seated(shared: &SharedGlueMgr) -> UiScript {
    let mut s = UiScript::new_glue().unwrap();
    s.set_glue_host(shared.host());
    s.run(
        "LOGIN_ENTER_NAME = 'Enter your account name' \
         CHAR_LIST_RETRIEVING = 'Retrieving character list' \
         CHAR_DELETE_IN_PROGRESS = 'Deleting character' \
         CHAR_RENAME_IN_PROGRESS = 'Renaming character' \
         REALM_LIST_IN_PROGRESS = 'Retrieving realm list' \
         GAME_SERVER_LOGIN = 'Connecting' \
         CHAR_LOGIN_LOCKED_FOR_TRANSFER = 'Locked' \
         HEARD = {} local f = CreateFrame('Frame') \
         for _, e in ipairs({'OPEN_STATUS_DIALOG', 'UPDATE_SELECTED_CHARACTER', \
             'CHARACTER_LIST_UPDATE', 'FORCE_RENAME_CHARACTER'}) do f:RegisterEvent(e) end \
         f:SetScript('OnEvent', function() \
           table.insert(HEARD, event .. ' ' .. tostring(arg1) .. ' ' .. tostring(arg2)) end)",
    )
    .unwrap();
    s
}

/// What `chunk` raised, read by the chunk's own last line: inside the verb's call, before any
/// frame of the manager.
fn heard_in(s: &UiScript, chunk: &str) -> Vec<String> {
    s.eval(&format!("HEARD = {{}} {chunk} return HEARD"))
        .unwrap()
}

/// One roster record with `guid` and `flags`.
fn record(guid: u64, flags: u32) -> GlueCharacter {
    GlueCharacter {
        guid,
        name: "Alpha".into(),
        race: String::new(),
        class: String::new(),
        level: 1,
        zone: None,
        file_string: String::new(),
        gender: 0,
        flags,
        model: false,
    }
}

/// `DefaultServerLogin` (`0x46d160` → `0x46afb0`): an empty box's `OKAY`, or state 1 and the
/// `CANCEL` dialog with no text (`0x46b028`-`0x46b04d`), before the call returns; the logon is the
/// frame's to send.
#[test]
fn default_server_login_opens_its_dialog_in_the_call() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    s.set_glue_notice(GlueNotice::Tos, NoticeState::from_cvar("1"));
    s.set_glue_notice(GlueNotice::Eula, NoticeState::from_cvar("1"));
    assert_eq!(
        heard_in(&s, "DefaultServerLogin('', 'b')"),
        ["OPEN_STATUS_DIALOG OKAY Enter your account name"]
    );
    assert_eq!(
        heard_in(&s, "DefaultServerLogin('a', 'b')"),
        ["OPEN_STATUS_DIALOG CANCEL nil"]
    );
    let m = shared.get();
    assert_eq!(m.state, ScreenState::Logon);
    assert!(matches!(m.login_out.as_slice(), [GlueLogin::Submit { .. }]));
}

/// `StatusDialogClick` (`0x46d270` → `0x46b720`): in state 9 the list is asked again
/// (`0x46b767` → `0x46b880`), state 3 and its dialog in the call; in state 2 the operation is
/// cancelled and the roster torn down (`0x46b754` → `0x472580`), its two events in the call.
#[test]
fn status_dialog_click_runs_its_state_arm_in_the_call() {
    let shared = SharedGlueMgr::default();
    let s = seated(&shared);
    {
        let mut m = shared.get_mut();
        m.connected = true;
        m.state = ScreenState::LoginRefused;
    }
    assert_eq!(
        heard_in(&s, "StatusDialogClick()"),
        ["OPEN_STATUS_DIALOG CANCEL Retrieving character list"]
    );
    {
        let mut m = shared.get_mut();
        assert_eq!(m.state, ScreenState::CharList);
        assert!(matches!(
            m.park_out.as_slice(),
            [ParkRequest::Char(CharRequest::Enumerate)]
        ));
        m.park_out.clear();
        m.state = ScreenState::Connect;
        m.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
    }
    assert_eq!(
        heard_in(&s, "StatusDialogClick()"),
        [
            "UPDATE_SELECTED_CHARACTER 1 nil",
            "CHARACTER_LIST_UPDATE nil nil"
        ]
    );
    assert_eq!(shared.get().op.status, STATUS_CANCELLED);
}

/// `GetCharacterListUpdate` (`0x473270` → `0x4723d0` → `0x46b880`): state 3 and the
/// `CHAR_LIST_RETRIEVING` dialog in the call; in state 9 nothing (`0x46b887`).
#[test]
fn get_character_list_update_opens_its_dialog_in_the_call() {
    let shared = SharedGlueMgr::default();
    let s = seated(&shared);
    shared.get_mut().connected = true;
    assert_eq!(
        heard_in(&s, "GetCharacterListUpdate()"),
        ["OPEN_STATUS_DIALOG CANCEL Retrieving character list"]
    );
    assert_eq!(shared.get().state, ScreenState::CharList);
    shared.get_mut().state = ScreenState::LoginRefused;
    assert!(heard_in(&s, "GetCharacterListUpdate()").is_empty());
    assert_eq!(shared.get().state, ScreenState::LoginRefused);
}

/// `DeleteCharacter` (`0x4734c0` → `0x46b2e0`): state 6 and the `CHAR_DELETE_IN_PROGRESS`
/// dialog in the call, the `CMSG_CHAR_DELETE` the frame's to send.
#[test]
fn delete_character_opens_its_dialog_in_the_call() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    shared.get_mut().connected = true;
    s.load_glue_roster(vec![record(2, 0)]);
    assert_eq!(
        heard_in(&s, "DeleteCharacter(1)"),
        ["OPEN_STATUS_DIALOG CANCEL Deleting character"]
    );
    let m = shared.get();
    assert_eq!(m.state, ScreenState::Delete);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Delete(2))]
    ));
}

/// `RenameCharacter` (`0x473520` → `0x46b340`): a passing name's state 7 and
/// `CHAR_RENAME_IN_PROGRESS` dialog in the call (`0x46b39b`-`0x46b3c0`), before the verb answers 1.
#[test]
fn rename_character_opens_its_dialog_in_the_call() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    shared.get_mut().connected = true;
    s.load_glue_roster(vec![record(2, benilla_protocol::CHARACTER_FLAG_RENAME)]);
    assert_eq!(
        heard_in(
            &s,
            "local answer = RenameCharacter(1, 'Newname') \
             table.insert(HEARD, 'answer ' .. tostring(answer))"
        ),
        ["OPEN_STATUS_DIALOG CANCEL Renaming character", "answer 1"]
    );
    assert_eq!(shared.get().state, ScreenState::Rename);
}

/// `RequestRealmList(1)` (`0x46ecf0` → `0x46b8d0`): state 4 and its dialog in the call;
/// `CancelRealmListQuery` (`0x46b7e0`) takes state 4 to 0 in its.
#[test]
fn the_realm_list_query_and_its_cancel_run_in_the_call() {
    let shared = SharedGlueMgr::default();
    let s = seated(&shared);
    assert_eq!(
        heard_in(&s, "RequestRealmList(1)"),
        ["OPEN_STATUS_DIALOG CANCEL Retrieving realm list"]
    );
    assert_eq!(shared.get().state, ScreenState::RealmList);
    assert!(
        heard_in(&s, "RequestRealmList()").is_empty(),
        "no flag, no dialog"
    );
    s.run("CancelRealmListQuery()").unwrap();
    assert_eq!(shared.get().state, ScreenState::Idle);
}

/// `ChangeRealm` (`0x46f0e0` → `0x46b210`): with no world link, state 2 and `GAME_SERVER_LOGIN`
/// in the call (`0x46b250`-`0x46b26c`); with one, the call drops it, so `IsConnectedToServer`
/// straight after reads nil, and changes no state and opens no dialog (`0x46b22b`-`0x46b241`).
#[test]
fn change_realm_runs_its_half_in_the_call() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    s.register_cvars([(crate::glue_session::realm::CVAR_REALM_NAME, "")]);
    let (categories, types) = crate::glue_session::realm::RealmTables::us_client().glue_setup();
    s.set_glue_realm_setup(categories, types);
    s.set_glue_realms(vec![glue_realm(&realm("Fake"))]);
    shared.get_mut().holder = vec![realm("Fake")];
    s.rebuild_glue_realms();
    assert_eq!(
        heard_in(&s, "ChangeRealm(1, 1)"),
        ["OPEN_STATUS_DIALOG CANCEL Connecting"]
    );
    assert_eq!(shared.get().state, ScreenState::Connect);

    {
        let mut m = shared.get_mut();
        m.state = ScreenState::Idle;
        m.connected = true;
        m.park_out.clear();
    }
    assert_eq!(
        heard_in(
            &s,
            "ChangeRealm(1, 1) table.insert(HEARD, 'link ' .. tostring(IsConnectedToServer()))"
        ),
        ["link nil"]
    );
    let m = shared.get();
    assert_eq!(m.state, ScreenState::Idle);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Realm(RealmRequest::Leave)]
    ));
}

/// `EnterWorld` (`0x46d3c0` → `0x46b500`): a locked record's `OKAY` (`0x46b5a5`) and a rename's
/// `FORCE_RENAME_CHARACTER` (`0x46b5c2`) fire in the call; a clean one takes state 8 in it.
#[test]
fn enter_world_runs_its_arms_in_the_call() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    {
        let mut m = shared.get_mut();
        m.connected = true;
        m.selected = Some(realm("Fake"));
    }
    for (flags, want) in [
        (0x4, "OPEN_STATUS_DIALOG OKAY Locked"),
        (
            benilla_protocol::CHARACTER_FLAG_RENAME,
            "FORCE_RENAME_CHARACTER CHAR_RENAME_DESCRIPTION nil",
        ),
    ] {
        s.load_glue_roster(vec![record(2, flags)]);
        assert_eq!(heard_in(&s, "EnterWorld()"), [want]);
        assert_eq!(shared.get().state, ScreenState::Idle);
    }
    // `lastCharacterIndex` is written in the call (`0x46b5fa`): the selection's 0-based index.
    s.register_cvars([(crate::glue_session::roster::CVAR_LAST_CHARACTER, "0")]);
    s.load_glue_roster(vec![record(2, 0), record(3, 0)]);
    s.run("SelectCharacter(2)").unwrap();
    assert!(heard_in(&s, "EnterWorld()").is_empty());
    assert_eq!(
        s.cvar(crate::glue_session::roster::CVAR_LAST_CHARACTER)
            .as_deref(),
        Some("1"),
        "no frame has run"
    );
    assert_eq!(shared.get().state, ScreenState::EnterWorld);
}

/// `DisconnectFromServer` (`0x46d340`): the link's flag is down in the call (`0x5ab1a8`), so
/// `IsConnectedToServer` straight after reads nil; `QuitGame` and `RealmListDialogCancelled` leave
/// their asks in theirs.
#[test]
fn the_link_verbs_run_in_the_call() {
    let shared = SharedGlueMgr::default();
    let s = seated(&shared);
    shared.get_mut().connected = true;
    assert_eq!(
        s.eval::<(Option<i64>, Option<i64>)>(
            "local before = IsConnectedToServer() DisconnectFromServer() \
             return before, IsConnectedToServer()"
        )
        .unwrap(),
        (Some(1), None)
    );
    s.run("QuitGame() SetCurrentScreen('login') RealmListDialogCancelled()")
        .unwrap();
    assert!(matches!(
        shared.get().login_out.as_slice(),
        [GlueLogin::Disconnect, GlueLogin::Quit, GlueLogin::Abandon]
    ));
}

/// A handler inside a verb's fan-out that calls another verb runs that verb's call there, and the
/// first verb's operation begins only after its dialog's handlers have run (`0x46b8b7` after the
/// signal at `0x46b8af`): the `CANCEL` dialog's button pressed from its own `OPEN_STATUS_DIALOG`
/// handler cancels the operation before it and tears the roster down (`0x46b754`), and the list
/// query then begins and is asked, not cancelled.
#[test]
fn a_verb_called_inside_another_verbs_fan_out_acts_before_its_operation_begins() {
    let shared = SharedGlueMgr::default();
    let s = seated(&shared);
    {
        let mut m = shared.get_mut();
        m.connected = true;
        m.op = Operation::begin(OpKind::Delete, STATUS_CHAR_DELETE_IN_PROGRESS);
    }
    s.run(
        "local pressed = false local f = CreateFrame('Frame') \
         f:RegisterEvent('OPEN_STATUS_DIALOG') \
         f:SetScript('OnEvent', function() \
           if arg1 == 'CANCEL' and not pressed then pressed = true StatusDialogClick() end end)",
    )
    .unwrap();
    // The nested events run inside `OPEN_STATUS_DIALOG`'s dispatch, whose `arg1` and `arg2` stand
    // where a nested event has fewer: `0x704f10` sets only the arguments it is given
    // (`0x705045`-`0x70508d`) and restores them after the call (`0x7051fc`-).
    assert_eq!(
        heard_in(&s, "GetCharacterListUpdate()"),
        [
            "OPEN_STATUS_DIALOG CANCEL Retrieving character list",
            "UPDATE_SELECTED_CHARACTER 1 Retrieving character list",
            "CHARACTER_LIST_UPDATE CANCEL Retrieving character list"
        ]
    );
    let m = shared.get();
    assert_eq!(m.state, ScreenState::CharList);
    assert_eq!(m.op.kind, OpKind::CharList);
    assert!(!m.op.done, "the query began after the press: {:?}", m.op);
    assert!(matches!(
        m.park_out.as_slice(),
        [ParkRequest::Char(CharRequest::Enumerate)]
    ));
    assert!(m.resumes.is_empty(), "every step ran once");
}

/// `GetServerName` (`0x46d280`) through the seated manager: the `realmName` CVar's text as it
/// stands, and the PvP and RP of the realm selected this process, link or no link
/// (`0x5ab9a0`, the `Cfg_Configs` walk at `0x46d2a5`-`0x46d30d`).
#[test]
fn get_server_name_reads_the_cvar_and_the_selected_realm_with_no_link() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    s.register_cvars([(crate::glue_session::realm::CVAR_REALM_NAME, "aLPHA")]);
    let (categories, types) = crate::glue_session::realm::RealmTables::us_client().glue_setup();
    s.set_glue_realm_setup(categories, types);
    let read = |s: &UiScript| {
        s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
            .unwrap()
    };
    assert_eq!(
        read(&s),
        ("aLPHA".to_string(), None, None),
        "nothing selected"
    );
    let mut pvp = realm("Alpha");
    pvp.realm_type = 1;
    shared.get_mut().selected = Some(pvp);
    assert!(!shared.get().connected);
    assert_eq!(read(&s), ("aLPHA".to_string(), Some(1), None));
    // With the link up the name is still the CVar's, in its own case, not the realm record's.
    shared.get_mut().connected = true;
    assert_eq!(read(&s), ("aLPHA".to_string(), Some(1), None));
    s.set_cvar_host(crate::glue_session::realm::CVAR_REALM_NAME, "Bravo");
    assert_eq!(read(&s).0, "Bravo", "read as it stands, no frame between");
}

/// `DisconnectFromServer` with no world link only cancels the logon (`0x46d34d` past the drop):
/// a connect in flight keeps its state and its operation.
#[test]
fn disconnect_with_no_link_leaves_the_connect_in_flight() {
    let mut m = fresh();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m.disconnect_from_server();
    assert_eq!(m.state, ScreenState::Connect);
    assert_eq!(m.op.kind, OpKind::Connect);
    assert!(!m.op.done);
    assert!(matches!(m.login_out.as_slice(), [GlueLogin::Disconnect]));
}

/// The frame's arm through the seated VM: its signals fired and its steps run in order.
fn frame(shared: &SharedGlueMgr, s: &mut UiScript) {
    shared.get_mut().tick();
    let names = RosterNames {
        areas: None,
        create: None,
        classes: None,
        creatures: None,
    };
    super::pump(shared, Some(s), None, 0, &names, None);
}

/// State 2's failure arm with no realm held: the failure's `OKAY` is pressed by its own `OnShow`
/// while the arm is still in state 2 (`0x46b754`: the operation cancelled, the roster torn
/// down); only then come state 0 (`0x46bef5`) and `RequestRealmList(1)` (`0x46bf86` →
/// `0x46c06e`), whose query is not cancelled, so its reply closes its dialog and raises the list.
#[test]
fn a_world_failure_with_no_realm_held_asks_for_the_list_and_its_reply_closes_it() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    // The stock `OKAY` type's `OnShow` press (`GlueDialog.lua:72`), and the dialog's close.
    s.run(
        "REALM_LIST_FAILED = 'Realm list failed' local f = CreateFrame('Frame') \
         f:RegisterEvent('OPEN_STATUS_DIALOG') f:RegisterEvent('CLOSE_STATUS_DIALOG') \
         f:SetScript('OnEvent', function() \
           if event == 'CLOSE_STATUS_DIALOG' then table.insert(HEARD, 'CLOSE') \
           elseif arg1 == 'OKAY' then StatusDialogClick() end end)",
    )
    .unwrap();
    {
        let mut m = shared.get_mut();
        m.state = ScreenState::Connect;
        m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
        m.op.complete(false, STATUS_REALM_LIST_FAILED);
    }
    s.run("HEARD = {}").unwrap();
    frame(&shared, &mut s);
    let events = |s: &UiScript| -> Vec<String> {
        s.eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
            .unwrap()
            .iter()
            .map(|h| h.split(' ').take(2).collect::<Vec<_>>().join(" "))
            .collect()
    };
    assert_eq!(
        events(&s),
        [
            "OPEN_STATUS_DIALOG OKAY",
            // The press in state 2, inside the dialog's dispatch: the teardown (`0x472580`).
            "UPDATE_SELECTED_CHARACTER 1",
            "CHARACTER_LIST_UPDATE OKAY",
            // The arm's own teardown (`0x46bf1f`), then `RequestRealmList(1)`. Inside the
            // dispatch the outer `arg1` stood for the argument-less event; here it is nil again
            // (`0x704f10` sets only the arguments it has and restores them after).
            "UPDATE_SELECTED_CHARACTER 1",
            "CHARACTER_LIST_UPDATE nil",
            "OPEN_STATUS_DIALOG CANCEL",
        ]
    );
    {
        let m = shared.get();
        assert_eq!(m.state, ScreenState::RealmList, "the query stands");
        assert_eq!(m.op.kind, OpKind::GetRealms);
        assert!(!m.op.done, "and was not cancelled: {:?}", m.op);
        assert!(m.outbox.is_empty() && m.resumes.is_empty());
    }
    shared
        .get_mut()
        .on_realm_list(vec![realm("Fake")], false, "Fake", &RunModeRealm::default());
    frame(&shared, &mut s);
    assert!(
        events(&s).contains(&"CLOSE".to_string()),
        "the query's success closes its dialog (`0x46c0d7`)"
    );
    assert_eq!(shared.get().state, ScreenState::Idle);
}

/// The logon arm's failure: its `OKAY` is pressed by its own `OnShow` while the arm is still in
/// state 1, which cancels the logon (`0x46b73d`); then the arm cancels it and goes to state 0
/// (`0x46bbbf`-`0x46bbcb`). A failed logon has no link left (`0x5b3320` tests `[login+5]`), so
/// both close only what may stand and leave the credentials and the resubmit alone.
#[test]
fn a_failed_logons_dialog_is_pressed_in_state_1() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    s.run(
        "local f = CreateFrame('Frame') f:RegisterEvent('OPEN_STATUS_DIALOG') \
         f:SetScript('OnEvent', function() if arg1 == 'OKAY' then StatusDialogClick() end end)",
    )
    .unwrap();
    {
        let mut m = shared.get_mut();
        m.login("a".into(), "b".into());
        m.settle();
        m.outbox.clear();
        m.login_out.clear();
        m.on_failed(&failed(Some(LoginRefusal::Logon(0x04)), false));
    }
    frame(&shared, &mut s);
    let m = shared.get();
    assert_eq!(m.state, ScreenState::Idle);
    assert!(
        matches!(
            m.login_out.as_slice(),
            [GlueLogin::CloseLink, GlueLogin::CloseLink]
        ),
        "the press's cancel, then the arm's: {:?}",
        m.login_out.len()
    );
}

/// The logon arm reads the latch and the login state after its dialog's handlers (`0x46ba85`,
/// `0x46baaa`), not off what it read before: a failure that lands after the dialog was raised is
/// the one the arm acts on.
#[test]
fn the_logon_arm_reads_its_login_state_after_the_dialog() {
    let mut m = fresh();
    m.login("a".into(), "b".into());
    settle(&mut m);
    m.outbox.clear();
    m.login_out.clear();
    m.on_stage(LoginStage::Connecting, true);
    m.tick();
    // What a handler of the dialog's dispatch would have seen land.
    m.cells.state = LOGIN_FAILED_STATE;
    m.cells.result = RESULT_DISCONNECTED;
    settle(&mut m);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(matches!(m.login_out.as_slice(), [GlueLogin::CloseLink]));
}

/// The world link's flag goes up with the socket's connect (`0x5aaa9c`), so `DisconnectFromServer`
/// between the connect and the authentication takes state 2 down as the drop's handler does
/// (`0x46c55d`).
#[test]
fn a_disconnect_after_the_world_socket_connects_ends_the_connect() {
    let mut m = fresh();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m.on_stage(LoginStage::WorldConnected, true);
    assert!(m.connected, "up with the socket");
    m.disconnect_from_server();
    assert_eq!(m.state, ScreenState::Idle);
    assert_eq!(m.op, Operation::default());
}

/// `AUTH_FAILED` reads the current screen as it stands after the failure's dialog (`0x46bfa7`):
/// a handler of that dialog that puts the character screen's name up sends it back to login.
#[test]
fn an_auth_failure_reads_the_screen_live() {
    let shared = SharedGlueMgr::default();
    let mut s = seated(&shared);
    s.run(
        "SetCurrentScreen('login') SCREENS = {} local f = CreateFrame('Frame') \
         f:RegisterEvent('OPEN_STATUS_DIALOG') f:RegisterEvent('SET_GLUE_SCREEN') \
         f:SetScript('OnEvent', function() \
           if event == 'SET_GLUE_SCREEN' then table.insert(SCREENS, arg1) \
           elseif arg1 == 'OKAY' then SetCurrentScreen('charselect') end end)",
    )
    .unwrap();
    {
        let mut m = shared.get_mut();
        m.state = ScreenState::Connect;
        m.holder = vec![realm("Fake")];
        m.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
        m.op.complete(false, STATUS_AUTH_FAILED);
    }
    frame(&shared, &mut s);
    assert_eq!(s.eval::<Vec<String>>("return SCREENS").unwrap(), ["login"]);
}

/// A VM the manager has not seated holds its verbs' calls, which the manager runs when it seats
/// the VM on its frame.
#[test]
fn a_call_before_the_seat_runs_on_the_seating_frame() {
    let s = UiScript::new_glue().unwrap();
    s.run("RequestRealmList(1)").unwrap();
    let shared = SharedGlueMgr::default();
    assert_eq!(shared.get().state, ScreenState::Idle);
    let mut app = App::new();
    app.init_resource::<Time>()
        .insert_non_send_resource(shared.clone())
        .add_message::<LoginStageMessage>()
        .add_message::<LoginQueuedMessage>()
        .add_message::<LoginFailedMessage>()
        .add_message::<CharListMessage>()
        .add_message::<WorldAuthMessage>()
        .add_message::<RosterFailedMessage>()
        .add_message::<AddonInfoMessage>()
        .add_message::<crate::net::CharActionResultMessage>()
        .add_message::<crate::net::CharRenameMessage>()
        .add_message::<crate::net::CharacterLoginFailedMessage>()
        .add_message::<LoginGuardRefused>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<crate::ui_script::server_alert::LaunchUrl>()
        .add_message::<crate::ui_script::server_alert::LaunchAddonUrl>()
        .add_systems(Update, drive_glue_mgr);
    app.insert_non_send_resource(s);
    app.update();
    assert_eq!(shared.get().state, ScreenState::RealmList);
}

// ── The stock screens over the login engine, end to end ─────────────────────────────────────────

/// An app with the manager, the login engine's glue asks and the stock glue VM on the login
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
        .init_non_send_resource::<SharedGlueMgr>()
        .init_resource::<crate::glue_session::login::LoginIntent>()
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
        .add_message::<AddonInfoMessage>()
        .add_message::<crate::net::CharActionResultMessage>()
        .add_message::<crate::net::CharRenameMessage>()
        .add_message::<crate::net::CharacterLoginFailedMessage>()
        .add_message::<LoginGuardRefused>()
        .init_resource::<AddonInfoReply>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<crate::ui_script::server_alert::LaunchUrl>()
        .add_message::<crate::ui_script::server_alert::LaunchAddonUrl>()
        .add_message::<AppExit>()
        .init_resource::<crate::portrait::GluePreview>()
        .add_systems(
            Update,
            (
                drive_glue_mgr,
                super::super::glue_scene::feed_stock_glue_preview,
                crate::glue_session::login::answer_glue,
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
        let mut mgr = mgr_mut(&app);
        mgr.connected = true;
        mgr.get_character_list();
        settle(&mut mgr);
    }
    vm(&app).run("DisconnectFromServer()").unwrap();
    app.update();
    {
        let m = mgr(&app);
        assert_eq!(m.state, ScreenState::Idle);
        assert!(!m.connected && m.op.kind != OpKind::CharList);
    }

    // With no world connection the verb only cancels the logon (`0x46d34d`): the state stands.
    mgr_mut(&app).state = ScreenState::RealmList;
    vm(&app).run("DisconnectFromServer()").unwrap();
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::RealmList);
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
        let mut mgr = mgr_mut(&app);
        mgr.select_cells = cells;
        mgr.vm = None;
    }
    app.update();
    assert_eq!(vm(&app).glue_select_cells(), cells);
    vm(&app).run("SetCharacterSelectFacing(90)").unwrap();
    app.update();
    let moved = mgr(&app).select_cells;
    assert!(
        (moved.facing - 90f32.to_radians()).abs() < 1e-5,
        "{moved:?}"
    );
}

/// The stock login screen's Login reaches the login engine with the typed credentials, under the
/// `CANCEL` dialog; the logon's stage and a refusal re-text it with the reference's keys; and the
/// agreements gate it.
#[test]
fn the_stock_login_reaches_the_login_engine_and_shows_its_status() {
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
    let req = rx.try_recv().expect("the login engine gets the attempt");
    assert_eq!((req.user.as_str(), req.pass.as_str()), ("probe", "secret"));
    let (which, _) = dialog(&app).expect("the status dialog is up");
    assert_eq!(which, "CANCEL");
    assert_eq!(mgr(&app).state, ScreenState::Logon);

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
    assert_eq!(mgr(&app).state, ScreenState::Idle);
}

/// The account guard's refusal of the run's own credentials, which no network message carries,
/// reaches the stock screen as the stock `OKAY` dialog with the guard's words, the way the Login
/// verb refuses an empty account; nothing logs on and the screen stays idle.
#[test]
fn the_account_guards_refusal_opens_the_stock_okay_dialog() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, rx, _) = stock_app();
    assert_eq!(dialog(&app), None);
    let why = "WOW_USER=other is not the account this checkout declares.";
    app.world_mut().write_message(LoginGuardRefused(why.into()));
    app.update();
    assert_eq!(dialog(&app), Some(("OKAY".into(), why.into())));
    assert_eq!(mgr(&app).state, ScreenState::Idle);
    assert!(rx.try_recv().is_err(), "nothing logs on");
}

/// The stock select shown with no world link names the realm off `realmName` and its type off the
/// realm selected this process (`CharacterSelect_OnShow`, `CharacterSelect.lua:48`), as
/// `GetServerName` (`0x46d280`) answers them link or no link.
#[test]
fn the_stock_select_with_no_link_shows_the_selected_realms_pvp() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _) = stock_app();
    {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.register_cvars([(crate::glue_session::realm::CVAR_REALM_NAME, "aLPHA")]);
        let (categories, types) = crate::glue_session::realm::RealmTables::us_client().glue_setup();
        s.set_glue_realm_setup(categories, types);
    }
    let mut pvp = realm("Alpha");
    pvp.realm_type = 1;
    mgr_mut(&app).selected = Some(pvp);
    vm(&app).run("SetGlueScreen('charselect')").unwrap();
    assert!(
        vm(&app)
            .eval::<bool>(
                "return CharSelectRealmName:GetText() == \
                 'aLPHA\\n(' .. TEXT(SERVER_DOWN) .. ') ' .. PVP_PARENTHESES"
            )
            .unwrap(),
        "{:?}",
        vm(&app).eval::<String>("return CharSelectRealmName:GetText()")
    );
}

/// The stock dialog over a world failure with no realm held: the failure's `OKAY` presses itself
/// in state 2, the `CANCEL` "Retrieving realm list" follows, and the list's reply closes it.
#[test]
fn the_stock_realm_list_dialog_after_a_world_failure_closes_on_its_reply() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _rx, _) = stock_app();
    {
        let mut m = mgr_mut(&app);
        m.state = ScreenState::Connect;
        m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
        m.op.complete(false, STATUS_REALM_LIST_FAILED);
    }
    app.update();
    let retrieving = vm(&app).global_text("REALM_LIST_IN_PROGRESS");
    assert_eq!(dialog(&app), Some(("CANCEL".into(), retrieving)));
    assert_eq!(mgr(&app).state, ScreenState::RealmList);
    app.world_mut().write_message(RealmListMessage {
        realms: vec![realm("Fake")],
        failed: false,
    });
    app.update();
    assert_eq!(dialog(&app), None, "the query's success closes it");
    assert_eq!(mgr(&app).state, ScreenState::Idle);
}

/// The status dialog's Cancel is `StatusDialogClick`, which abandons the attempt in flight.
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
    assert_eq!(mgr(&app).state, ScreenState::Idle);
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

/// The character verbs' replies and the world session's edges, as `net::session` publishes them.
#[derive(bevy::ecs::system::SystemParam)]
struct CharWrites<'w> {
    actions: MessageWriter<'w, crate::net::CharActionResultMessage>,
    renames: MessageWriter<'w, crate::net::CharRenameMessage>,
    entered: MessageWriter<'w, crate::net::EnteredWorldMessage>,
    refused: MessageWriter<'w, crate::net::CharacterLoginFailedMessage>,
    logouts: MessageWriter<'w, crate::net::LoggedOutMessage>,
}

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
    mut addon_infos: MessageWriter<AddonInfoMessage>,
    mut addon_reply: ResMut<AddonInfoReply>,
    mut char_writes: CharWrites,
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
                ..
            } => {
                failures.write(LoginFailedMessage {
                    refusal,
                    reason,
                    terminal,
                });
            }
            E::CharacterList {
                characters,
                realm,
                cause,
                epoch,
                ..
            } => {
                rosters.write(CharListMessage {
                    characters,
                    realm,
                    cause,
                    epoch,
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
            E::AddonInfo { records } => {
                addon_reply.0 = Some(records.clone());
                addon_infos.write(AddonInfoMessage { records });
            }
            E::CharActionResult { action, code } => {
                char_writes
                    .actions
                    .write(crate::net::CharActionResultMessage { action, code });
            }
            E::CharRenamed { result, renamed } => {
                char_writes
                    .renames
                    .write(crate::net::CharRenameMessage { result, renamed });
            }
            E::Connected {
                billing_time_rested,
                tutorial_flags,
                ..
            } => {
                char_writes.entered.write(crate::net::EnteredWorldMessage {
                    billing_time_rested,
                    tutorial_flags,
                });
            }
            E::CharacterLoginFailed { result } => {
                char_writes
                    .refused
                    .write(crate::net::CharacterLoginFailedMessage { result });
            }
            E::LoggedOut => {
                char_writes.logouts.write(crate::net::LoggedOutMessage);
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
    let cycles = fake::spawn_cycles();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_non_send_resource::<SharedGlueMgr>()
        .init_resource::<crate::glue_session::login::LoginIntent>()
        .insert_resource(crate::realmlist::Realmlist::unpinned(&realmd.addr))
        .insert_resource(LoginSubmit(cycles.login.clone()))
        .insert_resource(LoginAbandon(cycles.abandon.clone()))
        .insert_resource(crate::net::RealmChoice(cycles.realm.clone()))
        .insert_resource(crate::net::CharPick(cycles.pick.clone()))
        .insert_resource(crate::net::NetCommands(cycles.commands.clone()))
        .insert_resource(crate::glue_session::realm::RealmTables::us_client())
        .insert_resource(crate::entities::CharCreate(
            benilla_formats::CharCreateCatalog::load(&mut chain).expect("ChrRaces"),
        ))
        .insert_resource(crate::entities::Creatures::from_catalog(
            benilla_formats::load_creature_catalog(&mut chain).expect("creatures"),
        ))
        .insert_resource(crate::chr_classes::ChrClassTable(
            benilla_formats::load_chr_classes(&mut chain).expect("ChrClasses"),
        ))
        .insert_resource(crate::target::Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("factions"),
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
        .add_message::<AddonInfoMessage>()
        .add_message::<crate::net::CharActionResultMessage>()
        .add_message::<crate::net::CharRenameMessage>()
        .add_message::<crate::net::EnteredWorldMessage>()
        .add_message::<crate::net::CharacterLoginFailedMessage>()
        .add_message::<LoginGuardRefused>()
        .add_message::<crate::net::LoggedOutMessage>()
        .init_resource::<AddonInfoReply>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<crate::ui_script::server_alert::LaunchUrl>()
        .add_message::<crate::ui_script::server_alert::LaunchAddonUrl>()
        .add_message::<AppExit>()
        .init_resource::<crate::portrait::GluePreview>()
        .add_systems(
            Update,
            (
                pump,
                drive_glue_mgr,
                super::super::glue_scene::feed_stock_glue_preview,
                crate::glue_session::login::answer_glue,
            )
                .chain(),
        );
    let mut s = UiScript::new_glue().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.register_cvars(crate::cvars::registered_pairs());
    s.set_cvar_host(crate::glue_session::realm::CVAR_REALM_NAME, realm_name);
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
            let m = mgr(app);
            panic!(
                "timed out waiting for {what}: state {:?}, op {:?}, holder {}, screen {:?}, \
                 dialog {:?}, errors {:?}",
                m.state,
                m.op,
                m.holder.len(),
                vm(app).glue_screen(),
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
    let m = mgr(app);
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
    assert_eq!(mgr(&app).state, ScreenState::Idle);
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
            .cvar(crate::glue_session::realm::CVAR_REALM_NAME)
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
    mgr_mut(&app).realm_cells.chosen = Some(0);
    // The fresh VM takes the cells on the next frame.
    mgr_mut(&app).vm = None;
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
            .cvar(crate::glue_session::realm::CVAR_REALM_NAME)
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

/// A scripted run on the stock screens with no `realmName` (#1433), its account holding no
/// characters: an unattended run's pick, or the realm `WOW_REALM` names, is dialled at the
/// logon's list, `realmName` takes it, and the character screen comes up and stays, neither the
/// realm list nor the wizard raised.
#[test]
fn a_scripted_run_reaches_the_character_screen_with_no_realm_name() {
    let _ = benilla_formats::wow_data_or_skip!();
    for (run, want) in [
        (RunModeRealm::new(None, true), "Played"),
        (RunModeRealm::new(Some("bravo"), false), "Bravo"),
    ] {
        let (mut app, realmd) = live_app(
            |world| {
                let mut down = FakeRealm::new("Down", world);
                down.flags = 0x02;
                let mut played = FakeRealm::new("Played", world);
                played.characters = 2;
                vec![
                    down,
                    FakeRealm::new("Alpha", world),
                    FakeRealm::new("Bravo", world),
                    played,
                ]
            },
            "",
        );
        app.insert_resource(run.clone());
        log_in(&mut app);
        until(&mut app, "the character screen and its roster", |app| {
            let m = mgr(app);
            screen_up(app) && m.op.kind == OpKind::CharList && m.op.done
        });
        for _ in 0..20 {
            app.update();
        }
        assert!(
            shown(&app, "CharacterSelect")
                && !shown(&app, "RealmList")
                && !shown(&app, "RealmWizard"),
            "{run:?}: the character screen stands alone"
        );
        assert_eq!(
            vm(&app)
                .cvar(crate::glue_session::realm::CVAR_REALM_NAME)
                .as_deref(),
            Some(want),
            "{run:?}"
        );
        assert_eq!(realmd.logons(), 1, "{run:?}: dialled on the logon's link");
    }
}

// ── The character screen's roster over the IO thread and a fake world server ────────────────────

/// Three characters: a living night elf druid in Teldrassil, a ghost human warrior in Elwynn, and
/// a gnome mage in a zone `AreaTable` has no row for.
fn roster3() -> Vec<benilla_protocol::Character> {
    let mut alpha = crate::glue_session::roster::test_character(0x11, "Alpha");
    (
        alpha.race,
        alpha.class,
        alpha.gender,
        alpha.level,
        alpha.zone,
    ) = (4, 11, 1, 12, 141);
    let mut bravo = crate::glue_session::roster::test_character(0x12, "Bravo");
    (bravo.race, bravo.class, bravo.level, bravo.zone) = (1, 1, 60, 12);
    bravo.flags = benilla_protocol::CHARACTER_FLAG_GHOST;
    let mut charlie = crate::glue_session::roster::test_character(0x13, "Charlie");
    (charlie.race, charlie.class, charlie.level, charlie.zone) = (7, 8, 3, 9999);
    vec![alpha, bravo, charlie]
}

/// The stock login for `roster`, `lastCharacterIndex` as given, a realm chosen or not, with the
/// events the roster fires logged into the VM's `EVENTS`.
///
/// Nothing is stubbed: the screen's `OnShow` reaches the add-on list's verbs first
/// (`UpdateAddonButton`, `AddonList.lua:4`) and the glue VM answers them.
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
        None,
    )
}

/// [`char_select_app`] over `realms`, the account holding `rosters[n]` on the world session
/// that begins `n`th (the last repeats). `reply` seats the `SMSG_ADDON_INFO` every session is
/// sent on the fake world, before the login dials; none is no reply at all.
fn char_select_app_on(
    realms: impl FnOnce(&str) -> Vec<FakeRealm>,
    rosters: &[&[benilla_protocol::Character]],
    last: &str,
    chosen: bool,
    reply: Option<&dyn Fn(&fake::World)>,
) -> (App, fake::Realmd, fake::World) {
    let (mut app, realmd, world) = live_app_with(realms, "Alpha", rosters[0]);
    world.set_rosters(rosters);
    if let Some(seat) = reply {
        seat(&world);
    }
    if chosen {
        mgr_mut(&app).realm_cells.chosen = Some(0);
        // The fresh VM takes the cells on the next frame.
        mgr_mut(&app).vm = None;
    }
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_cvar_engine("lastCharacterIndex", last);
        script
            .run(
                "EVENTS = {} \
                 local f = CreateFrame('Frame', 'RosterListener') \
                 f:SetScript('OnEvent', function() \
                   local a = arg1 \
                   if event == 'ADDON_LIST_UPDATE' then a = GetNumAddOns() end \
                   table.insert(EVENTS, event .. ' ' .. tostring(a)) end) \
                 f:RegisterEvent('ADDON_LIST_UPDATE') \
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
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
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
        seen(app, "CHARACTER_LIST_UPDATE") > before && mgr(app).state == ScreenState::Idle
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
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
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
    let mut dwarf = crate::glue_session::roster::test_character(0x14, "Delta");
    (dwarf.race, dwarf.zone) = (3, 12);
    roster.push(dwarf);
    let (mut app, _realmd, _world) = char_select_app(&roster, "1", false);
    until(&mut app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
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
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert_eq!(p.scene, Some(GlueScene::Orc));
    assert!(p.look.is_none());
    let fog = p.fog.expect("the empty roster's scene is fogged");
    assert_eq!((fog.near, fog.far), (0.0, 270.0));

    let mut tauren = crate::glue_session::roster::test_character(0x41, "Moo");
    (tauren.race, tauren.zone) = (6, 12);
    let (mut app, _realmd, world) = char_select_app(&[tauren], "0", true);
    until(&mut app, "the roster load", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
    });
    app.update();
    let p = preview(&app);
    assert_eq!((p.scene, p.fog), (Some(GlueScene::Tauren), None));
    assert!(p.look.is_some());

    world.set_roster(&[]);
    let before = seen(&app, "CHARACTER_LIST_UPDATE");
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    until(&mut app, "the empty roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > before && mgr(app).state == ScreenState::Idle
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
    let mut c = crate::glue_session::roster::test_character(1, "Alpha");
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
    let mut alpha = crate::glue_session::roster::test_character(0x21, "Oldrealm");
    alpha.zone = 12;
    let mut bravo = crate::glue_session::roster::test_character(0x31, "Newrealm");
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
            None,
        );
        until(&mut app, "the first roster", |app| {
            seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
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
            mgr(app).state == ScreenState::Idle && vm(app).glue_roster_guids() == [0x31]
        });
        assert_eq!(vm(&app).errors(), Vec::<String>::new(), "round {round}");
        assert_eq!(
            vm(&app)
                .cvar(crate::glue_session::realm::CVAR_REALM_NAME)
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
    let before = vec![crate::glue_session::roster::test_character(0x41, "Before")];
    let (mut app, _realmd, world) = char_select_app(&before, "0", true);
    until(&mut app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
    });
    world.set_roster(&[crate::glue_session::roster::test_character(0x42, "Later")]);
    world.delay_rosters(std::time::Duration::from_millis(400));
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::CharList);
    vm(&app).run("StatusDialogClick()").unwrap();
    until(&mut app, "the realm list", |app| shown(app, "RealmList"));
    // The late reply has gone by (the IO thread served the realm list after it).
    assert_eq!(world.enumerations(), 2);
    assert_eq!(
        mgr(&app).roster[0].name,
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
    assert_eq!(mgr(&app).state, ScreenState::Logon);
    assert!(!shown(&app, "ServerAlertFrame"));

    let fetch = crate::ui_script::server_alert::spawn_fetch(url);
    mgr_mut(&app).set_alert_fetch(fetch);
    until(&mut app, "the alert frame", |app| {
        shown(app, "ServerAlertFrame")
    });

    // After the prefix, short of the last byte (`SStrCopy` of the whole body, `0x46a59f`).
    let want = &body[b"SERVERALERT:".len()..body.len() - 1];
    assert_eq!(vm(&app).eval::<String>("return ALERT_ARG").unwrap(), want);
    assert_eq!(abandon.load(Ordering::SeqCst), 1, "the logon was abandoned");
    assert_eq!(mgr(&app).state, ScreenState::Idle);

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
    mgr_mut(&app).set_alert_fetch(fetch);
    until(&mut app, "the fetch to end", |app| {
        mgr(app).alert_fetch.is_none()
    });
    app.update();
    assert!(!shown(&app, "ServerAlertFrame"));
    assert_eq!(abandon.load(Ordering::SeqCst), 0);
    assert_eq!(mgr(&app).state, ScreenState::Logon);
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

// ── The AddOns screen under the stock select (`AddonList.lua`) ──

/// An AddOns folder under a temp `benilla-config`, `BENILLA_HOME` on it while the guard lives.
fn addon_home(tag: &str) -> (std::path::PathBuf, crate::local_state::test_env::EnvGuard) {
    let tmp =
        std::env::temp_dir().join(format!("benilla-glue-addons-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let home = tmp.join("benilla-config");
    std::fs::create_dir_all(home.join("AddOns")).unwrap();
    let guard = crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());
    (home, guard)
}

fn write_addon(home: &std::path::Path, name: &str, toc: &str) {
    let dir = home.join("AddOns").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
}

/// Three addons by title: Alpha ("Alpha Title"), Beta, and Gamma, which is out of date. The
/// realm `Alpha`'s characters: Alpha has Alpha on, Bravo has it off, Charlie has no file.
fn three_addons(home: &std::path::Path) {
    write_addon(
        home,
        "Alpha",
        "## Interface: 11200\n## Title: Alpha Title\n## Notes: the first\n",
    );
    write_addon(
        home,
        "Beta",
        "## Interface: 11200\n## Dependencies: Alpha\n",
    );
    write_addon(home, "Gamma", "## Interface: 11100\n");
    let dir = home.join("addon-state");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Alpha-Alpha.txt"), "Alpha: enabled\n").unwrap();
    std::fs::write(dir.join("Alpha-Bravo.txt"), "Alpha: disabled\n").unwrap();
}

/// Every enable file under `home/addon-state`, by file name.
fn enable_files(home: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(home.join("addon-state"))
        .map(|d| {
            d.flatten()
                .map(|e| {
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        std::fs::read(e.path()).unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Up to the loaded roster.
fn at_the_roster(app: &mut App) {
    until(app, "the roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") >= 2 && mgr(app).state == ScreenState::Idle
    });
}

/// The stock select over `roster3`'s account on realm `Alpha`, `three_addons`' folder under the
/// current `BENILLA_HOME`, and `reply` the server's `SMSG_ADDON_INFO`.
fn three_addon_app(reply: Option<&dyn Fn(&fake::World)>) -> (App, fake::Realmd, fake::World) {
    char_select_app_on(
        |world| vec![FakeRealm::new("Alpha", world)],
        &[&roster3()],
        "1",
        false,
        reply,
    )
}

/// The stock select runs end to end with nothing stubbed: its `OnShow` reaches
/// `GetCharacterListUpdate` past `UpdateAddonButton`, the add-ons button shows for the registry
/// the server's reply left, and the stock `AddonList` opens, shows each row through the glue
/// verbs, toggles through its checkboxes, and Okay writes each character's enable file.
#[test]
fn the_stock_addon_list_opens_toggles_and_saves() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (home, _guard) = addon_home("stock");
    three_addons(&home);

    let (mut app, _realmd, world) = three_addon_app(Some(&|w| w.set_addon_info(&[2; 12], false)));
    at_the_roster(&mut app);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    assert_eq!(world.enumerations(), 1, "OnShow reached the list ask");
    assert!(shown(&app, "CharacterSelect"));
    assert!(
        shown(&app, "CharacterSelectAddonsButton"),
        "three addons are installed: UpdateAddonButton shows the button"
    );
    assert_eq!(
        vm(&app).eval::<f64>("return GetNumAddOns()").unwrap(),
        3.0,
        "the twelve Blizzard addons are hidden by the reply"
    );

    // Open the list: the rows are the glue verbs' answers.
    vm(&app).run("AddonList:Show()").unwrap();
    app.update();
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    let text = |app: &App, name: &str| {
        vm(app)
            .eval::<String>(&format!("return {name}:GetText() or ''"))
            .unwrap()
    };
    assert_eq!(text(&app, "AddonListEntry1Title"), "Alpha Title");
    assert_eq!(text(&app, "AddonListEntry2Title"), "Beta");
    assert_eq!(text(&app, "AddonListEntry3Status"), "Out of date");
    let states = |app: &App| {
        vm(app)
            .eval::<Vec<f64>>(
                "return { AddonListEntry1Enabled.state, AddonListEntry2Enabled.state, \
                          AddonListEntry3Enabled.state }",
            )
            .unwrap()
    };
    // Alpha is on for one character, off for another and, with no file for the third, the
    // others' mixed answer's default: some. Beta and Gamma nobody set: their DefaultState.
    assert_eq!(states(&app), [1.0, 2.0, 2.0]);

    // The boxes toggle for All (nil): the checked mixed box clears, and so does Beta's.
    vm(&app)
        .run("AddonListEntry1Enabled:Click() AddonListEntry2Enabled:Click()")
        .unwrap();
    assert_eq!(states(&app), [0.0, 0.0, 2.0]);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());

    // One character's view: its own state, from the dropdown's selection.
    vm(&app)
        .run(
            "GlueDropDownMenu_SetSelectedValue(AddonCharacterDropDown, 'Alpha') AddonList_Update()",
        )
        .unwrap();
    vm(&app).run("AddonListEntry2Enabled:Click()").unwrap();
    assert_eq!(states(&app), [0.0, 2.0, 2.0], "Alpha alone has Beta on");
    assert_eq!(vm(&app).errors(), Vec::<String>::new());

    // Okay: SaveAddOns writes the changed nodes, then the list hides.
    let before = enable_files(&home);
    vm(&app).run("AddonList_OnOk()").unwrap();
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    assert!(!shown(&app, "AddonList"));
    let stock = enable_files(&home);
    assert_ne!(before, stock);
    let text = |name: &str| String::from_utf8(stock[name].clone()).unwrap();
    assert_eq!(text("Alpha-Alpha.txt"), "Alpha: disabled\nBeta: enabled\n");
    assert_eq!(text("Alpha-Bravo.txt"), "Alpha: disabled\nBeta: disabled\n");
    assert_eq!(
        text("Alpha-Charlie.txt"),
        "Alpha: disabled\nBeta: disabled\n",
        "a character with no file gets one, as every node the All view set is dirty"
    );
}

/// `ADDON_LIST_UPDATE` fires where the reference's handler for `SMSG_ADDON_INFO` does
/// (`0x5b4260`, listener slot `0x5ab2f0`): once, when the reply has been read, with the index it
/// rebuilt already in place, and ahead of the roster. A reply that trails the authentication
/// answer fires it when it is read, behind the first list ask; one that never comes fires nothing
/// and leaves the add-ons button hidden.
#[test]
fn addon_list_update_fires_once_when_the_reply_is_read() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (home, _guard) = addon_home("timing");
    three_addons(&home);
    let events = |app: &App| {
        vm(app)
            .eval::<Vec<String>>("return EVENTS")
            .unwrap()
            .into_iter()
            .filter(|e| {
                e.starts_with("ADDON_LIST_UPDATE") || e.starts_with("CHARACTER_LIST_UPDATE")
            })
            .collect::<Vec<_>>()
    };

    // Ahead of the authentication answer (vmangos builds it in the auth handler): the count the
    // event carries is the rebuilt index's, three, and the entry's teardown update and the
    // roster come after.
    let (mut app, _r, _w) = three_addon_app(Some(&|w| w.set_addon_info(&[2; 12], false)));
    at_the_roster(&mut app);
    assert_eq!(
        events(&app)[..3],
        [
            "ADDON_LIST_UPDATE 3",
            "CHARACTER_LIST_UPDATE nil",
            "CHARACTER_LIST_UPDATE nil"
        ],
        "the reply, then the teardown and the roster"
    );
    assert_eq!(seen(&app, "ADDON_LIST_UPDATE"), 1, "once for the session");
    // A second list ask on the same link does not read it again.
    let before = seen(&app, "CHARACTER_LIST_UPDATE");
    vm(&app).run("GetCharacterListUpdate()").unwrap();
    until(&mut app, "the second roster", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > before && mgr(app).state == ScreenState::Idle
    });
    assert_eq!(
        seen(&app, "ADDON_LIST_UPDATE"),
        1,
        "…and the event does not repeat"
    );
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    drop(app);

    // Behind the answer: read with the first list ask, so after the entry's teardown update
    // (`0x472580`) and before the roster's.
    let (mut app, _r, _w) = three_addon_app(Some(&|w| w.set_addon_info(&[2; 12], true)));
    at_the_roster(&mut app);
    assert_eq!(
        events(&app)[..3],
        [
            "CHARACTER_LIST_UPDATE nil",
            "ADDON_LIST_UPDATE 3",
            "CHARACTER_LIST_UPDATE nil"
        ]
    );
    assert_eq!(seen(&app, "ADDON_LIST_UPDATE"), 1);
    drop(app);

    // No reply: no index space (`[0xbe1b90]` stays 0), no event, no button.
    let (mut app, _r, _w) = three_addon_app(None);
    at_the_roster(&mut app);
    assert_eq!(seen(&app, "ADDON_LIST_UPDATE"), 0);
    assert_eq!(vm(&app).eval::<f64>("return GetNumAddOns()").unwrap(), 0.0);
    assert!(!shown(&app, "CharacterSelectAddonsButton"));
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// The server's `SMSG_ADDON_INFO` records reach the registry the stock list reads (`0x51da70`):
/// the url and the new-version byte beside a listed addon, BANNED for status 0 (refused, no url),
/// and SECURE once the reply has run the signature check, which benilla answers 3 for an addon
/// of the player's own chain that claims `## Secure`. A folder addon the reply never names stays
/// INSECURE.
#[test]
fn the_server_reply_sets_the_security_url_and_new_version() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (home, _guard) = addon_home("reply");
    three_addons(&home);
    // The twelve in the order they were sent: the first listed with a url and a revision, the
    // second banned with a url, the rest hidden.
    let mut records: Vec<(u8, Option<u32>, Option<&str>)> = vec![(2, Some(0), None); 12];
    records[0] = (1, Some(9), Some("http://example.com/auc"));
    records[1] = (0, None, Some("http://example.com/bf"));
    let (mut app, _realmd, _world) =
        three_addon_app(Some(&|w| w.set_addon_records(&records, false)));
    at_the_roster(&mut app);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
    assert_eq!(
        vm(&app).eval::<f64>("return GetNumAddOns()").unwrap(),
        5.0,
        "three folder addons and the two the reply left listed"
    );
    type Row = (
        Option<String>,
        Option<f64>,
        Option<String>,
        String,
        Option<f64>,
    );
    let row = |name: &str| -> Row {
        vm(&app)
            .eval::<Row>(&format!(
                "for i = 1, GetNumAddOns() do \
                   local n, _, _, url, loadable, reason, security, new = GetAddOnInfo(i) \
                   if n == '{name}' then return url, loadable, reason, security, new end \
                 end"
            ))
            .unwrap()
    };
    assert_eq!(
        row("Blizzard_AuctionUI"),
        (
            Some("http://example.com/auc".into()),
            Some(1.0),
            None,
            "SECURE".into(),
            Some(1.0)
        ),
        "revision 9 is past the installed, and the chain's signed addon passes the check"
    );
    assert_eq!(
        row("Blizzard_BattlefieldMinimap"),
        (None, None, Some("BANNED".into()), "BANNED".into(), None),
        "status 0 bans it without the check, and a banned addon has no url"
    );
    assert_eq!(row("Alpha").3, "INSECURE");

    // The stock confirm dialog's `LaunchAddOnURL(AddonList.selectedID)`: the listed addon's url
    // reaches the host's launcher, the banned one's (nil) nothing.
    let index = |name: &str| {
        vm(&app)
            .eval::<f64>(&format!(
                "for i = 1, GetNumAddOns() do \
                   if (GetAddOnInfo(i)) == '{name}' then return i end end"
            ))
            .unwrap()
    };
    let (auction, minimap) = (
        index("Blizzard_AuctionUI"),
        index("Blizzard_BattlefieldMinimap"),
    );
    vm(&app)
        .run(&format!(
            "LaunchAddOnURL({auction}) LaunchAddOnURL({minimap})"
        ))
        .unwrap();
    app.update();
    let launched: Vec<String> = app
        .world()
        .resource::<Messages<crate::ui_script::server_alert::LaunchAddonUrl>>()
        .iter_current_update_messages()
        .map(|m| m.0.clone())
        .collect();
    assert_eq!(launched, ["http://example.com/auc"]);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// Until the reply runs the handler, a chain addon that claims `## Secure` is INSECURE as the
/// constructor leaves it (`0x5205b6`), and loads under its own context; the reply's signature
/// answer makes it SECURE.
#[test]
fn a_secure_claim_reads_insecure_until_the_reply_has_run() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("claim");
    let mut script = UiScript::new_glue().unwrap();
    crate::ui_script::addons::seat_glue_addons(&mut script, None);
    assert!(!script.addon_is_secure("Blizzard_TalentUI"));
    let all: Vec<_> = benilla_protocol::messages::STOCK_SECURE_ADDONS
        .iter()
        .map(|a| benilla_protocol::messages::AddonRecord {
            name: a.name.to_string(),
            status: 2,
            ..Default::default()
        })
        .collect();
    crate::ui_script::addons::seat_glue_addons(&mut script, Some(&all));
    assert!(script.addon_is_secure("Blizzard_TalentUI"));
    assert!(!script.addon_is_secure("Alpha"));
}

// ── The delete and the rename under the stock select (`CharacterSelect.xml`) ──

/// The stock select over `roster` on realm `Alpha`, at the loaded roster, with
/// `SELECT_FIRST_CHARACTER` and `FORCE_RENAME_CHARACTER` logged into `EVENTS` too; `reply` as
/// [`char_select_app_on`]'s.
fn select_screen(
    roster: &[benilla_protocol::Character],
    reply: Option<&dyn Fn(&fake::World)>,
) -> (App, fake::Realmd, fake::World) {
    let (mut app, realmd, world) = char_select_app_on(
        |world| vec![FakeRealm::new("Alpha", world)],
        &[roster],
        "0",
        false,
        reply,
    );
    at_the_roster(&mut app);
    vm(&app)
        .run(
            "RosterListener:RegisterEvent('SELECT_FIRST_CHARACTER') \
             RosterListener:RegisterEvent('FORCE_RENAME_CHARACTER')",
        )
        .unwrap();
    (app, realmd, world)
}

/// The rows' names as the stock list shows them.
fn row_names(app: &App) -> Vec<String> {
    vm(app)
        .eval::<Vec<String>>(
            "local t = {} for i = 1, GetNumCharacters() do \
               t[i] = getglobal('CharSelectCharacterButton'..i..'ButtonTextName'):GetText() end \
             return t",
        )
        .unwrap()
}

/// The stock delete end to end: the confirm dialog's Okay calls `DeleteCharacter`, which shows the
/// `CHAR_DELETE_IN_PROGRESS` dialog and sends `CMSG_CHAR_DELETE` with the selected guid; the
/// reply's success fires `SELECT_FIRST_CHARACTER` and asks for the list again, which comes back
/// without the character, the first row selected.
#[test]
fn the_stock_delete_refetches_the_list_and_selects_the_first_row() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = select_screen(&roster3(), None);
    // The confirm box enables Okay from its `OnTextChanged`, which fires at the frame's flush.
    vm(&app)
        .run(
            "CharacterSelect_SelectCharacter(2) CharacterDeleteDialog:Show() \
             CharacterDeleteEditBox:SetText(DELETE_CONFIRM_STRING)",
        )
        .unwrap();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .tick(0.01);
    vm(&app).run("CharacterDeleteButton1:Click()").unwrap();
    assert!(!shown(&app, "CharacterDeleteDialog"));
    let lists = seen(&app, "CHARACTER_LIST_UPDATE");
    app.update();
    assert_eq!(
        dialog(&app),
        Some(("CANCEL".into(), "Deleting character".into()))
    );
    assert_eq!(mgr(&app).state, ScreenState::Delete);
    until(&mut app, "the list without Bravo", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > lists && mgr(app).state == ScreenState::Idle
    });
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_CHAR_DELETE),
        [0x12u64.to_le_bytes().to_vec()]
    );
    assert_eq!(
        world.enumerations(),
        2,
        "the delete's success asked for the list"
    );
    assert_eq!(seen(&app, "SELECT_FIRST_CHARACTER"), 1);
    assert_eq!(row_names(&app), ["Alpha", "Charlie"]);
    assert_eq!(
        vm(&app)
            .eval::<f64>("return CharacterSelect.selectedIndex")
            .unwrap(),
        1.0
    );
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x11));
    assert_eq!(dialog(&app), None);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A refused delete (vmangos's guild master, `CHAR_DELETE_FAILED`) opens `OKAY` with its text and
/// asks for no list (`0x46c173`).
#[test]
fn a_refused_stock_delete_opens_okay_and_keeps_the_list() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = select_screen(&roster3(), None);
    world.answer_deletes(0x3a);
    vm(&app).run("DeleteCharacter(2)").unwrap();
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::Delete);
    until(&mut app, "the refusal", |app| {
        mgr(app).state == ScreenState::Idle
    });
    assert_eq!(
        dialog(&app),
        Some(("OKAY".into(), "Character deletion failed".into()))
    );
    assert_eq!(world.enumerations(), 1);
    assert_eq!(seen(&app, "SELECT_FIRST_CHARACTER"), 0);
    assert_eq!(row_names(&app), ["Alpha", "Bravo", "Charlie"]);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// The stock rename end to end, on a character the server flagged: a name the check fails
/// re-shows the rename dialog with the check's text and sends nothing; a passing one hides it,
/// shows `CHAR_RENAME_IN_PROGRESS` and sends `CMSG_CHAR_RENAME`; the reply closes the dialog and
/// renames the row in place, with no list asked, the flag clear and the character's files moved
/// to the new name.
#[test]
fn the_stock_rename_renames_the_row_and_moves_its_files() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (home, _guard) = addon_home("rename");
    three_addons(&home);
    let macros = crate::local_state::macros_character_path("Alpha", "Bravo").unwrap();
    crate::local_state::write_atomic(&macros, "MACRO 1 \"M\" Ability_Ambush\n/say hi\nEND\n")
        .unwrap();
    let mut roster = roster3();
    roster[1].flags |= benilla_protocol::CHARACTER_FLAG_RENAME;
    let (mut app, _realmd, world) =
        select_screen(&roster, Some(&|w| w.set_addon_info(&[2; 12], false)));
    // The session's roster, Bravo picked: the in-world files are named from it.
    app.world_mut()
        .insert_resource(crate::glue_session::roster::Roster::with_pending_pick(
            roster, 0x12,
        ));
    let enable = |app: &App, who: &str| {
        vm(app)
            .eval::<f64>(&format!("return GetAddOnEnableState('{who}', 1)"))
            .unwrap()
    };
    assert_eq!(enable(&app, "Bravo"), 0.0, "Bravo has Alpha off");

    // A name the check fails: the dialog again, with the check's text, nothing sent.
    vm(&app)
        .run(
            "CharacterSelect_SelectCharacter(2) CharacterRenameDialog:Show() \
             CharacterRenameEditBox:SetText('X') CharacterRenameButton1:Click()",
        )
        .unwrap();
    assert!(shown(&app, "CharacterRenameDialog"));
    assert_eq!(
        vm(&app)
            .eval::<String>("return CharacterRenameText1:GetText()")
            .unwrap(),
        "Names must be at least 2 characters"
    );
    assert_eq!(seen(&app, "FORCE_RENAME_CHARACTER CHAR_NAME_TOO_SHORT"), 1);
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::Idle);

    // A passing one.
    vm(&app)
        .run("CharacterRenameEditBox:SetText('Newbie') CharacterRenameButton1:Click()")
        .unwrap();
    assert!(!shown(&app, "CharacterRenameDialog"));
    let lists = seen(&app, "CHARACTER_LIST_UPDATE");
    app.update();
    assert_eq!(
        dialog(&app),
        Some(("CANCEL".into(), "Renaming Character...".into()))
    );
    until(&mut app, "the rename's reply", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > lists
    });
    let mut sent = 0x12u64.to_le_bytes().to_vec();
    sent.extend(b"Newbie\0");
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_CHAR_RENAME),
        [sent]
    );
    assert_eq!(dialog(&app), None, "the reply closed the dialog");
    assert_eq!(world.enumerations(), 1, "the record is renamed in place");
    assert_eq!(row_names(&app), ["Alpha", "Newbie", "Charlie"]);
    assert_eq!(
        mgr(&app).state,
        ScreenState::Rename,
        "the reply leaves state 7"
    );
    // The flag is clear: the verb refuses the record now (`0x4735ab`).
    assert_eq!(
        vm(&app)
            .eval::<f64>(
                "local function n(...) return arg.n end return n(RenameCharacter(2, 'Other'))"
            )
            .unwrap(),
        0.0
    );
    let kept = mgr(&app).roster_character(0x12).map(|c| {
        (
            c.name.clone(),
            c.flags & benilla_protocol::CHARACTER_FLAG_RENAME,
        )
    });
    assert_eq!(kept, Some(("Newbie".to_string(), 0)));
    // The world entry names its files by the new name.
    let picked = app
        .world()
        .resource::<crate::glue_session::roster::Roster>();
    assert_eq!(
        crate::ui_macro::identity(picked).map(|(_, name)| name),
        Some("Newbie".to_string())
    );

    // The files moved, and the AddOn state with them.
    let moved = crate::local_state::macros_character_path("Alpha", "Newbie").unwrap();
    assert!(moved.exists() && !macros.exists());
    assert_eq!(
        std::fs::read(home.join("addon-state").join("Alpha-Newbie.txt")).unwrap(),
        b"Alpha: disabled\n"
    );
    assert!(!home.join("addon-state").join("Alpha-Bravo.txt").exists());
    assert_eq!(enable(&app, "Newbie"), 0.0, "the node came with the file");
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A rename the server refuses re-shows the dialog with `CHAR_RENAME_FAILED` (`0x46b49c`), and
/// the row keeps its name.
#[test]
fn a_refused_stock_rename_asks_again() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut roster = roster3();
    roster[1].flags |= benilla_protocol::CHARACTER_FLAG_RENAME;
    let (mut app, _realmd, world) = select_screen(&roster, None);
    world.answer_renames(0x2f);
    vm(&app).run("RenameCharacter(2, 'Newbie')").unwrap();
    until(&mut app, "the refusal", |app| {
        seen(app, "FORCE_RENAME_CHARACTER CHAR_RENAME_FAILED") == 1
    });
    assert!(shown(&app, "CharacterRenameDialog"));
    assert_eq!(
        vm(&app)
            .eval::<String>("return CharacterRenameText1:GetText()")
            .unwrap(),
        "Character rename failed"
    );
    assert_eq!(dialog(&app), None);
    assert_eq!(row_names(&app), ["Alpha", "Bravo", "Charlie"]);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

// ── The world entry under the stock select ──

/// [`select_screen`] over `roster3`'s account with the session's state machine running: the
/// session's edges ([`crate::glue_session::edges::session_edges`]), the world entry's VM edges
/// (`OnEnter(InWorld)` takes the glue VM out of the world, `OnExit(InWorld)` builds the stock glue
/// again on the screen the exit enters), and the CVar registry with its sync, saved under the
/// current `BENILLA_HOME`. With `refuse` the server refuses every character login.
fn entry_app(refuse: bool) -> (App, fake::World) {
    let (mut app, _realmd, world) = select_screen(&roster3(), None);
    if refuse {
        world.refuse_logins();
    }
    // The registry as the player left it: the realm, and the agreements the login screen took.
    let mut cvars = crate::cvars::Cvars::default();
    cvars.set(crate::glue_session::realm::CVAR_REALM_NAME, "Alpha");
    cvars.set("readTOS", "1");
    cvars.set("readEULA", "1");
    {
        let w = app.world_mut();
        w.remove_resource::<State<ClientState>>();
        w.remove_resource::<NextState<ClientState>>();
        w.insert_resource(cvars);
        // The roster the IO thread already published, as the session's edges took it.
        w.insert_resource(crate::glue_session::roster::Roster::with_chars(roster3()));
        w.init_resource::<crate::glue_session::roster::RunPick>();
    }
    app.add_plugins(bevy::state::app::StatesPlugin)
        .insert_state(ClientState::CharSelect)
        .add_systems(
            OnEnter(ClientState::InWorld),
            super::super::lifecycle::arm_entry_ui_load,
        )
        .add_systems(
            OnExit(ClientState::InWorld),
            crate::ui_script::end_ui_session,
        )
        .add_systems(
            Update,
            (
                crate::glue_session::edges::session_edges()
                    .after(pump)
                    .before(drive_glue_mgr),
                crate::cvars::sync_cvars.after(drive_glue_mgr),
            ),
        );
    app.update();
    (app, world)
}

fn client_state(app: &App) -> ClientState {
    *app.world().resource::<State<ClientState>>().get()
}

/// The glue VM standing in the world, if one does.
fn glue_vm(app: &App) -> Option<&UiScript> {
    app.world()
        .get_non_send_resource::<UiScript>()
        .filter(|s| s.vm_kind() == VmKind::Glue)
}

/// Run frames, the glue VM's own tick between them while one stands, until `done` holds.
fn until_entry(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + fake::WAIT;
    while !done(app) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}: client {:?}, glue {:?}, errors {:?}",
            client_state(app),
            mgr(app).state,
            glue_vm(app).map(UiScript::errors)
        );
        app.update();
        if let Some(mut s) = app.world_mut().get_non_send_resource_mut::<UiScript>() {
            s.tick(0.01);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// The `[cvars]` line `config.toml` holds for `name`, if any.
fn saved_cvar(home: &std::path::Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(home.join("config.toml")).ok()?;
    let table: toml::Table = text.parse().ok()?;
    table.get("cvars")?.get(name)?.as_str().map(str::to_string)
}

/// The stock Enter World end to end: the button's `EnterWorld` writes `lastCharacterIndex` and
/// saves it (`0x46b5fa`, `0x46b6f6`), takes state 8 and sends `CMSG_PLAYER_LOGIN` with the
/// selected guid; the entry takes the client into the world and the glue VM out of it.
#[test]
fn the_stock_enter_world_logs_in_and_the_glue_leaves_with_the_screen() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (home, _guard) = addon_home("enter-world");
    let (mut app, world) = entry_app(false);
    vm(&app)
        .run("CharacterSelect_SelectCharacter(2) CharSelectEnterWorldButton:Click()")
        .unwrap();
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::EnterWorld);
    assert_eq!(
        app.world()
            .resource::<crate::cvars::Cvars>()
            .get(crate::glue_session::roster::CVAR_LAST_CHARACTER),
        Some("1")
    );
    assert_eq!(
        saved_cvar(&home, crate::glue_session::roster::CVAR_LAST_CHARACTER).as_deref(),
        Some("1"),
        "saved in the call, not a quiet second later"
    );
    until_entry(&mut app, "the world", |app| {
        client_state(app) == ClientState::InWorld && glue_vm(app).is_none()
    });
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_PLAYER_LOGIN),
        [0x12u64.to_le_bytes().to_vec()]
    );
    assert_eq!(
        app.world()
            .resource::<crate::glue_session::roster::Roster>()
            .pending_row()
            .map(|c| c.name.as_str()),
        Some("Bravo"),
        "the loading screen and the world are the picked record's"
    );
    assert!(
        app.world().get_non_send_resource::<UiScript>().is_none(),
        "no VM of the glue's left in the world"
    );
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::Idle);
}

/// The logout smoke's re-entry on the stock screens is a player's: the first row selected and
/// Enter World pressed, so it goes through the manager's `EnterWorld` (`0x46b500`) and state 8,
/// not around it with a bare pick.
#[test]
fn the_logout_smokes_stock_reentry_goes_through_enter_world() {
    use bevy::ecs::system::RunSystemOnce;
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("smoke-reentry");
    let (mut app, world) = entry_app(false);
    // Off the first row, so the re-entry's own selection shows.
    vm(&app).run("CharacterSelect_SelectCharacter(3)").unwrap();
    app.world_mut()
        .run_system_once(|stock: crate::ui_script::glue::StockGlueProbe| {
            crate::glue_session::roster::press_enter_world(stock.vm(), 0);
        })
        .unwrap();
    app.update();
    assert_eq!(
        mgr(&app).state,
        ScreenState::EnterWorld,
        "the re-entry is the stock Enter World, not a pick behind the manager's back"
    );
    until_entry(&mut app, "the world", |app| {
        client_state(app) == ClientState::InWorld
    });
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_PLAYER_LOGIN),
        [0x11u64.to_le_bytes().to_vec()],
        "the first row, Alpha"
    );
}

/// The run's own pick (`WOW_CHAR`) on the stock screens is a player's too: once the select screen
/// has its roster, the row is selected and Enter World pressed, so the entry takes the manager's
/// `EnterWorld` (`0x46b500`), state 8 and `lastCharacterIndex`, not a pick sent past the screen.
#[test]
fn the_runs_own_stock_pick_goes_through_enter_world() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("run-pick");
    let (mut app, world) = entry_app(false);
    app.world_mut()
        .insert_resource(crate::glue_session::roster::RunPick::armed(0x12));
    app.add_systems(
        Update,
        crate::glue_session::roster::enter_run_pick.after(drive_glue_mgr),
    );
    app.update();
    assert_eq!(
        mgr(&app).state,
        ScreenState::EnterWorld,
        "the pick is the stock Enter World, not a pick behind the manager's back"
    );
    assert_eq!(
        app.world()
            .resource::<crate::cvars::Cvars>()
            .get(crate::glue_session::roster::CVAR_LAST_CHARACTER),
        Some("1")
    );
    until_entry(&mut app, "the world", |app| {
        client_state(app) == ClientState::InWorld
    });
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_PLAYER_LOGIN),
        [0x12u64.to_le_bytes().to_vec()],
        "the armed row, Bravo"
    );
}

/// The run's own selection (`WOW_CHARSELECT_PICK`) on the stock screens is the screen's own: once
/// it has its roster, the row is selected as a click selects it, and the screen stays up.
#[test]
fn the_runs_own_stock_selection_selects_on_the_screen() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("run-select");
    let (mut app, world) = entry_app(false);
    app.world_mut()
        .insert_resource(crate::glue_session::roster::RunPick::selecting(0x12));
    app.add_systems(
        Update,
        crate::glue_session::roster::enter_run_pick.after(drive_glue_mgr),
    );
    app.update();
    assert_eq!(
        vm(&app)
            .eval::<i64>("return CharacterSelect.selectedIndex")
            .unwrap(),
        2,
        "the armed row, Bravo"
    );
    for _ in 0..3 {
        app.update();
    }
    assert_ne!(mgr(&app).state, ScreenState::EnterWorld);
    assert_eq!(client_state(&app), ClientState::CharSelect);
    assert!(world
        .bodies(benilla_protocol::messages::opcode::CMSG_PLAYER_LOGIN)
        .is_empty());
}

/// A refused login (vmangos's result 1) that lands once the client is in the world comes back to
/// the stock select in state 9, on a glue rebuilt with the link and the server carried over: the
/// screen's own list ask sends nothing (`0x46b880`), state 9's frame opens `OKAY` with
/// `CHAR_LOGIN_NO_WORLD`'s text (`0x46c283`), and the button the stock `OnShow` presses
/// (`0x46b767`) asks for the list again on the same session, which comes back with the character
/// entered selected (`lastCharacterIndex`).
#[test]
fn a_refused_stock_login_comes_back_to_the_select_and_lists_again() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("refused-login");
    let (mut app, world) = entry_app(true);
    // Held until the client is in the world, then let go.
    world.hold_refusals();
    vm(&app)
        .run("CharacterSelect_SelectCharacter(2) CharSelectEnterWorldButton:Click()")
        .unwrap();
    until_entry(&mut app, "the world", |app| {
        client_state(app) == ClientState::InWorld
    });
    world.release_refusals();
    // State 9's `OKAY` is pressed by its own `OnShow` inside the dispatch that opens it
    // (`0x46b767` → `0x46b880`), so no frame ends on the refusal's text.
    let refusal = Some(("OKAY".to_string(), "World server is down".to_string()));
    until_entry(&mut app, "a new glue asking again", |app| {
        if glue_vm(app).is_none() {
            return false;
        }
        assert_ne!(
            dialog(app),
            refusal,
            "the refusal is replaced before a frame ends"
        );
        mgr(app).state == ScreenState::CharList
    });
    assert_eq!(client_state(&app), ClientState::CharSelect);
    assert!(shown(&app, "CharacterSelect"));
    assert_eq!(
        realm_label(&app),
        "Alpha ",
        "the build carried the link and the server into the screen's OnShow"
    );
    let retrieving = vm(&app).global_text("CHAR_LIST_RETRIEVING");
    assert_eq!(dialog(&app), Some(("CANCEL".into(), retrieving)));
    until_entry(&mut app, "the list again", |app| {
        assert_ne!(dialog(app), refusal);
        mgr(app).state == ScreenState::Idle && vm(app).glue_roster_guids().len() == 3
    });
    assert_eq!(
        (world.authentications(), world.enumerations_in(0)),
        (1, 2),
        "on the same session: the OnShow's ask, made on the link in state 9, sent nothing"
    );
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x12));
    assert_eq!(dialog(&app), None);
    assert_eq!(row_names(&app), ["Alpha", "Bravo", "Charlie"]);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// The two flag arms under the stock select, nothing sent by either: a character locked for a
/// transfer gets `OKAY` with `CHAR_LOGIN_LOCKED_FOR_TRANSFER`'s text, and one flagged for a
/// rename gets the stock rename dialog with `CHAR_RENAME_DESCRIPTION`'s.
#[test]
fn the_stock_enter_world_stops_at_a_locked_or_renamed_character() {
    let _ = benilla_formats::wow_data_or_skip!();
    let mut roster = roster3();
    roster[0].flags |= 0x4;
    roster[1].flags |= benilla_protocol::CHARACTER_FLAG_RENAME;
    let (mut app, _realmd, world) = select_screen(&roster, None);
    vm(&app)
        .run("CharacterSelect_SelectCharacter(1) CharSelectEnterWorldButton:Click()")
        .unwrap();
    app.update();
    assert_eq!(
        dialog(&app),
        Some((
            "OKAY".into(),
            vm(&app).global_text("CHAR_LOGIN_LOCKED_FOR_TRANSFER")
        ))
    );
    vm(&app).run("GlueDialog:Hide()").unwrap();
    vm(&app)
        .run("CharacterSelect_SelectCharacter(2) CharSelectEnterWorldButton:Click()")
        .unwrap();
    app.update();
    assert!(shown(&app, "CharacterRenameDialog"));
    assert_eq!(
        vm(&app)
            .eval::<String>("return CharacterRenameText1:GetText()")
            .unwrap(),
        vm(&app).global_text("CHAR_RENAME_DESCRIPTION")
    );
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(mgr(&app).state, ScreenState::Idle);
    assert!(world
        .bodies(benilla_protocol::messages::opcode::CMSG_PLAYER_LOGIN)
        .is_empty());
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// The realm line `CharacterSelect_OnShow` wrote: the server's name and type, with
/// `(Server down)` under it when `IsConnectedToServer` said no.
fn realm_label(app: &App) -> String {
    vm(app)
        .eval::<String>("return CharSelectRealmName:GetText() or ''")
        .unwrap()
}

/// A logout from the world comes back to the stock select on the glue the build rebuilds with the
/// link and the server carried over (`0x46a7b0`): its `CharacterSelect_OnShow` finds
/// `IsConnectedToServer` (`0x46d380`) and names the realm, and its `GetCharacterListUpdate` asks
/// for the list on the same session (`0x5aaeb0` closes nothing).
#[test]
fn a_stock_logout_comes_back_to_the_select_and_asks_for_its_list() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("logout");
    let (mut app, world) = entry_app(false);
    vm(&app)
        .run("CharacterSelect_SelectCharacter(3) CharSelectEnterWorldButton:Click()")
        .unwrap();
    until_entry(&mut app, "the world", |app| {
        client_state(app) == ClientState::InWorld
    });
    app.world()
        .resource::<crate::net::NetCommands>()
        .0
        .send(crate::net::ClientCommand::Logout)
        .unwrap();
    until_entry(&mut app, "the select's list", |app| {
        glue_vm(app).is_some()
            && mgr(app).state == ScreenState::Idle
            && vm(app).glue_roster_guids().len() == 3
    });
    assert_eq!(client_state(&app), ClientState::CharSelect);
    assert_eq!(realm_label(&app), "Alpha ");
    assert_eq!(
        (world.authentications(), world.enumerations_in(0)),
        (1, 2),
        "the screen's own ask, on the same session"
    );
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x13));
    assert_eq!(dialog(&app), None);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A refusal in the same drain as the entry it revokes (vmangos refuses before touching the
/// database) is the last word: the client never leaves the stock select, whose glue VM stands, and
/// the refusal runs its state 9 there, to the list again on the same session.
#[test]
fn a_refusal_in_the_entrys_drain_keeps_the_stock_select() {
    let _ = benilla_formats::wow_data_or_skip!();
    let _l = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_home, _guard) = addon_home("refused-drain");
    let (mut app, world) = entry_app(true);
    let session = vm(&app).session();
    vm(&app)
        .run(
            "OPENED = {} local f = CreateFrame('Frame') f:RegisterEvent('OPEN_STATUS_DIALOG') \
             f:SetScript('OnEvent', function() table.insert(OPENED, arg1 .. ' ' .. tostring(arg2)) end)",
        )
        .unwrap();
    vm(&app)
        .run("CharacterSelect_SelectCharacter(2) CharSelectEnterWorldButton:Click()")
        .unwrap();
    app.update();
    // The entry, its refusal and the session's end, all queued before the next drain.
    let deadline = std::time::Instant::now() + fake::WAIT;
    while app.world().resource::<IoEvents>().0.len() < 3 {
        assert!(std::time::Instant::now() < deadline, "no refusal");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // State 9's dialog, on the glue that stood, is opened and pressed by its own `OnShow` in one
    // dispatch: the list's dialog replaces it before the frame ends.
    let refusal = Some(("OKAY".to_string(), "World server is down".to_string()));
    until_entry(&mut app, "the list asked again", |app| {
        assert_ne!(
            dialog(app),
            refusal,
            "the refusal is replaced before a frame ends"
        );
        mgr(app).state == ScreenState::CharList
    });
    let retrieving = vm(&app).global_text("CHAR_LIST_RETRIEVING");
    assert_eq!(dialog(&app), Some(("CANCEL".into(), retrieving.clone())));
    assert_eq!(
        vm(&app).eval::<Vec<String>>("return OPENED").unwrap(),
        [
            format!("CANCEL {retrieving}"),
            "OKAY World server is down".to_string()
        ],
        "the refusal was raised, and replaced inside its own dispatch: `GlueDialog`, registered \
         first, shows it and its `OnShow` raises the next, which this listener, registered last, \
         hears before the outer delivery reaches it"
    );
    until_entry(&mut app, "the list again", |app| {
        assert_ne!(dialog(app), refusal);
        glue_vm(app).is_some()
            && mgr(app).state == ScreenState::Idle
            && vm(app).glue_roster_guids().len() == 3
    });
    assert_eq!(client_state(&app), ClientState::CharSelect);
    assert_eq!(vm(&app).session(), session, "the glue never left");
    assert!(
        app.world()
            .resource::<crate::glue_session::roster::Roster>()
            .pending_row()
            .is_none(),
        "the pick went with the refusal"
    );
    assert_eq!((world.authentications(), world.enumerations_in(0)), (1, 2));
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x12));
    assert_eq!(dialog(&app), None);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

// ── The create under the stock glue (`CharacterCreate.xml`) ──

/// The stock select over `roster3`, at the loaded roster, with `SELECT_LAST_CHARACTER` and the
/// status dialog's opening logged into `EVENTS`; the create screen's first draws are all 0 (the
/// reset's sex, race and five customizations: a male Human).
fn create_screen() -> (App, fake::Realmd, fake::World) {
    let (mut app, realmd, world) = select_screen(&roster3(), None);
    vm(&app)
        .run(
            "RosterListener:RegisterEvent('SELECT_LAST_CHARACTER') \
             RosterListener:RegisterEvent('OPEN_STATUS_DIALOG')",
        )
        .unwrap();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .script_glue_create_draws(&[0; 7]);
    vm(&app)
        .run("CharSelectCreateCharacterButton:Click()")
        .unwrap();
    app.update();
    (app, realmd, world)
}

/// The preview character the create verbs stand.
fn create_look(app: &App) -> benilla_ui::script::GlueCreateLook {
    vm(app).glue_create_look().expect("a character stands")
}

/// The stock create end to end: the select's Create button shows the create screen, whose `OnShow`
/// resets the character (a male Human here) and stands it in the glue booth; a race, a sex and a
/// class picked and a customization cycled on the screen's own buttons change the character and
/// the booth's look; Okay with a name shows `CHAR_CREATE_IN_PROGRESS` and sends
/// `CMSG_CHAR_CREATE` with the name and the character's bytes; the success puts the select back
/// up, which asks for the list again and selects the new last row (`SELECT_LAST_CHARACTER`).
#[test]
fn the_stock_create_makes_the_character_and_selects_it() {
    use crate::portrait::{CreateLook, GlueLook, GlueScene};
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = create_screen();
    assert!(shown(&app, "CharacterCreate") && !shown(&app, "CharacterSelect"));
    let start = create_look(&app);
    assert_eq!(
        (start.race, start.sex, start.class),
        (1, 0, 1),
        "the reset's draws"
    );
    assert_eq!(
        vm(&app)
            .eval::<(String, String, String)>(
                "return CharacterCreateRaceLabel:GetText(), CharacterCreateClassLabel:GetText(), \
                 CharacterCreateFactionLabel:GetText()"
            )
            .unwrap(),
        ("Human".into(), "Warrior".into(), "Alliance".into())
    );
    let p = preview(&app);
    assert!(
        p.look
            == Some(GlueLook::Create(CreateLook {
                race: 1,
                sex: 0,
                class: 1,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
            }))
    );
    assert_eq!(p.scene, Some(GlueScene::Human));
    assert!((p.yaw + 15f32.to_radians()).abs() < 1e-5, "{}", p.yaw);

    // Orc (the list's fifth), female, Hunter (the Orc list's second), one skin colour on.
    vm(&app)
        .run(
            "CharacterCreateRaceButton5:Click() CharacterCreateGenderButtonFemale:Click() \
             CharacterCreateClassButton2:Click()",
        )
        .unwrap();
    let picked = create_look(&app);
    assert_eq!((picked.race, picked.sex, picked.class), (2, 1, 3));
    vm(&app)
        .run("CharacterCustomizationButtonFrame1RightButton:Click()")
        .unwrap();
    let cycled = create_look(&app);
    let skins = app
        .world()
        .resource::<crate::entities::CharCreate>()
        .0
        .ranges(2, 1)
        .unwrap()
        .skin;
    assert_eq!(cycled.skin, (picked.skin + 1) % skins);
    assert_eq!(
        vm(&app)
            .eval::<String>("return CharacterCreateClassLabel:GetText()")
            .unwrap(),
        "Hunter"
    );
    app.update();
    let p = preview(&app);
    assert!(
        p.look
            == Some(GlueLook::Create(CreateLook {
                race: 2,
                sex: 1,
                class: 3,
                skin: cycled.skin,
                face: cycled.face,
                hair_style: cycled.hair_style,
                hair_color: cycled.hair_color,
                facial_hair: cycled.facial_hair,
            }))
    );
    assert_eq!(p.scene, Some(GlueScene::Orc));
    let fog = p.fog.expect("the create scene keeps its fog");
    assert_eq!((fog.near, fog.far), (0.0, 270.0));

    vm(&app)
        .run("CharacterCreateNameEdit:SetText('Newbie') CharCreateOkayButton:Click()")
        .unwrap();
    // `CreateCharacter`'s state 5 and dialog land in its call (`0x46b29e`-`0x46b2c4`), before any
    // frame of the manager; the `CMSG_CHAR_CREATE` waits for the frame.
    assert_eq!(
        dialog(&app),
        Some(("CANCEL".into(), "Creating character".into()))
    );
    assert_eq!(mgr(&app).state, ScreenState::Create);
    assert!(world
        .bodies(benilla_protocol::messages::opcode::CMSG_CHAR_CREATE)
        .is_empty());
    let lists = seen(&app, "CHARACTER_LIST_UPDATE");
    app.update();
    until(&mut app, "the list with Newbie", |app| {
        seen(app, "CHARACTER_LIST_UPDATE") > lists && mgr(app).state == ScreenState::Idle
    });
    let mut sent = b"Newbie\0".to_vec();
    sent.extend([
        2,
        3,
        1,
        cycled.skin,
        cycled.face,
        cycled.hair_style,
        cycled.hair_color,
        cycled.facial_hair,
        0,
    ]);
    assert_eq!(
        world.bodies(benilla_protocol::messages::opcode::CMSG_CHAR_CREATE),
        [sent]
    );
    assert!(shown(&app, "CharacterSelect") && !shown(&app, "CharacterCreate"));
    assert_eq!(
        world.enumerations(),
        2,
        "the select asked for the list again"
    );
    assert_eq!(seen(&app, "SELECT_LAST_CHARACTER"), 1);
    assert_eq!(row_names(&app), ["Alpha", "Bravo", "Charlie", "Newbie"]);
    assert_eq!(
        vm(&app)
            .eval::<f64>("return CharacterSelect.selectedIndex")
            .unwrap(),
        4.0,
        "the new character, the last row, is selected"
    );
    assert_eq!(vm(&app).glue_selected_guid(), Some(0x14));
    assert_eq!(dialog(&app), None);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A create the server refuses (vmangos's taken name, `CHAR_CREATE_NAME_IN_USE`) opens `OKAY`
/// with its text over the create screen, which stays, and asks for no list.
#[test]
fn a_refused_stock_create_shows_its_text() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = create_screen();
    world.answer_creates(0x31);
    vm(&app)
        .run("CharacterCreateNameEdit:SetText('Alpha') CharCreateOkayButton:Click()")
        .unwrap();
    app.update();
    assert_eq!(mgr(&app).state, ScreenState::Create);
    until(&mut app, "the refusal", |app| {
        mgr(app).state == ScreenState::Idle
    });
    assert_eq!(
        dialog(&app),
        Some(("OKAY".into(), "That name is unavailable".into()))
    );
    assert!(shown(&app, "CharacterCreate"));
    assert_eq!(
        world
            .bodies(benilla_protocol::messages::opcode::CMSG_CHAR_CREATE)
            .len(),
        1
    );
    assert_eq!(world.enumerations(), 1);
    assert_eq!(seen(&app, "SELECT_LAST_CHARACTER"), 0);
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}

/// A name the client's check fails (`0x4711e2`) opens `OKAY` with the check's text and sends
/// nothing, with no screen state; so does an empty one.
#[test]
fn a_stock_create_whose_name_fails_the_check_sends_nothing() {
    let _ = benilla_formats::wow_data_or_skip!();
    let (mut app, _realmd, world) = create_screen();
    for (name, text) in [
        (
            "Abbbc",
            "You cannot use the same letter three times consecutively",
        ),
        ("", "Enter a name for your character"),
    ] {
        vm(&app)
            .run(&format!(
                "CharacterCreateNameEdit:SetText('{name}') CharCreateOkayButton:Click()"
            ))
            .unwrap();
        assert_eq!(dialog(&app), Some(("OKAY".into(), text.into())), "{name:?}");
        app.update();
        assert_eq!(mgr(&app).state, ScreenState::Idle);
        vm(&app).run("GlueDialog:Hide()").unwrap();
    }
    for _ in 0..5 {
        app.update();
    }
    assert!(world
        .bodies(benilla_protocol::messages::opcode::CMSG_CHAR_CREATE)
        .is_empty());
    assert!(shown(&app, "CharacterCreate"));
    assert_eq!(vm(&app).errors(), Vec::<String>::new());
}
