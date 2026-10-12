use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;
use crate::char_select::ClientState;
use crate::net::{LoginAbandon, LoginRequest, LoginSubmit};

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

/// The world connect's failure arm: the status text, `OKAY_WITH_URL` with the URL key for the
/// five codes of `0x803740`, and `AUTH_FAILED` from the character screen back to login.
#[test]
fn a_world_refusal_opens_its_status_and_url() {
    let connected = || {
        let mut m = accepted();
        m.state = ScreenState::Connect;
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
            }
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
            open("OKAY", Some("RESPONSE_FAILED_TO_CONNECT"))
        ]
    );
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

/// The roster ends the world connect: the status text, state 0, the character screen.
#[test]
fn the_roster_puts_the_character_screen_up() {
    let mut m = accepted();
    m.state = ScreenState::Connect;
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
    assert_eq!(m.state, ScreenState::Idle);
    assert!(m.connected);
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

/// `StatusDialogClick` by state: the logon and the world connect are abandoned; idle does nothing.
#[test]
fn status_dialog_click_cancels_by_state() {
    let mut asks = Vec::new();
    let mut m = accepted();
    m.status_dialog_click(&mut asks);
    assert!(asks.is_empty());
    for state in [ScreenState::Logon, ScreenState::Connect] {
        m.state = state;
        m.status_dialog_click(&mut asks);
        assert_eq!(m.state, ScreenState::Idle);
    }
    assert!(matches!(
        asks.as_slice(),
        [GlueLogin::Abandon, GlueLogin::Abandon]
    ));
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
