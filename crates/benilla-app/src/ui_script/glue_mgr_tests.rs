use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::char_select::ClientState;
use crate::net::{LoginAbandon, LoginRequest, LoginSubmit};
use benilla_protocol::RealmInfo;

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
            Outgoing::Close,
            Outgoing::Rebuild,
        ]
    );
    assert!(
        m.realm_out.is_empty(),
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
            open("CANCEL", Some("REALM_LIST_IN_PROGRESS")),
        ]
    );
    assert_eq!(m.state, ScreenState::RealmList);
    assert!(matches!(m.realm_out.as_slice(), [RealmRequest::Refresh]));
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
    m.characters = 2;
    m.on_roster(None);
    m.tick();
    assert_eq!(
        m.outbox,
        [
            Outgoing::Update {
                text: Text::Key("AUTH_OK"),
                button: None
            },
            Outgoing::Screen("charselect")
        ]
    );
}

/// The roster ends the world connect: the status text, state 0, the character screen, then the
/// roster's arm (`0x4724d0`): characters mark a realm chosen (`0x472556`); an empty roster with
/// no realm chosen asks for the list (`0x47253d` → `0x46b8d0`); one with a choice asks nothing.
#[test]
fn the_roster_puts_the_character_screen_up() {
    let up = [
        Outgoing::Update {
            text: Text::Key("AUTH_OK"),
            button: None,
        },
        Outgoing::Screen("charselect"),
    ];
    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.characters = 3;
    m.on_roster(None);
    m.tick();
    assert_eq!(m.outbox, up);
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.connected);
    assert_eq!(m.realm_cells.chosen, Some(0));
    assert!(m.cells_moved && m.realm_out.is_empty());

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.on_roster(None);
    m.tick();
    let mut want = up.to_vec();
    want.push(open("CANCEL", Some("REALM_LIST_IN_PROGRESS")));
    assert_eq!(
        m.outbox, want,
        "an empty roster with no choice asks for the list"
    );
    assert_eq!(m.state, ScreenState::RealmList);
    assert!(matches!(m.realm_out.as_slice(), [RealmRequest::Refresh]));

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.realm_cells.chosen = Some(5);
    m.on_roster(None);
    m.tick();
    assert_eq!(m.outbox, up, "a realm was chosen: nothing more");
    assert!(m.realm_out.is_empty());
}

/// Change Realm with the world up drops the session before the connect's query (`0x46b22b`).
#[test]
fn change_realm_with_the_world_up_leaves_it_first() {
    let mut m = accepted();
    m.holder = vec![realm("Fake")];
    m.connected = true;
    m.connect_to_realm("fake");
    assert!(matches!(
        m.realm_out.as_slice(),
        [RealmRequest::Leave, RealmRequest::Refresh]
    ));
    assert_eq!(m.state, ScreenState::Connect);
    assert!(!m.connected);
    assert_eq!(m.selected.as_ref().map(|r| r.name.as_str()), Some("Fake"));

    let mut m = accepted();
    m.holder = vec![realm("Fake")];
    m.connect_to_realm("Fake");
    assert!(matches!(m.realm_out.as_slice(), [RealmRequest::Refresh]));
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
            Outgoing::Update {
                text: Text::Key("RESPONSE_CANCELLED"),
                button: None
            },
            Outgoing::Close,
            Outgoing::Rebuild
        ],
        "no failure dialog for a cancel"
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

/// A session that is over fires `DISCONNECTED_FROM_SERVER`; a logout's relist does not; in the
/// world connect, a completed failed operation opens `OKAY` with its text instead
/// (`0x46c671`-`0x46c68f`), and a running one still fires the event.
#[test]
fn a_lost_session_fires_disconnected() {
    let mut m = accepted();
    m.on_disconnected(false);
    assert!(m.outbox.is_empty());
    m.on_disconnected(true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
    m.on_disconnected(true);
    assert_eq!(m.outbox, [Outgoing::Disconnected]);

    let mut m = accepted();
    m.state = ScreenState::Connect;
    m.op.complete(false, STATUS_FAILED_TO_CONNECT);
    m.on_disconnected(true);
    assert_eq!(m.outbox, [open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT"))]);
    assert_eq!(m.state, ScreenState::Idle);
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
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<AppExit>()
        .add_systems(Update, (drive_glue_mgr, crate::login::answer_glue).chain());
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
            E::CharacterList { characters, realm } => {
                rosters.write(CharListMessage { characters, realm });
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
    let (realmd, _) = fake::servers(realms);
    let cycles = fake::spawn_cycles();
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
        .insert_resource(IoEvents(cycles.events.clone()))
        .init_resource::<WorldDials>()
        .insert_resource(State::new(ClientState::Login))
        .init_resource::<NextState<ClientState>>()
        .add_message::<LoginStageMessage>()
        .add_message::<LoginQueuedMessage>()
        .add_message::<LoginFailedMessage>()
        .add_message::<CharListMessage>()
        .add_message::<RealmListMessage>()
        .add_message::<DisconnectedMessage>()
        .add_message::<GlueLogin>()
        .add_message::<crate::sound::GlueMusicRequest>()
        .add_message::<AppExit>()
        .add_systems(
            Update,
            (pump, drive_glue_mgr, crate::login::answer_glue).chain(),
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
    (app, realmd)
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
