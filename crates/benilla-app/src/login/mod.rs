//! The native login screen, the reference's `AccountLogin` glue: the form, its input, its caret
//! and its dialogs; the layout is [`screen`]. The login engine it drives, the credentials, the
//! asks and the success edge, is [`crate::glue_session::login`]'s, which serves the stock screens
//! too; this screen shows what it decides ([`LoginNotice`]). All of it runs on the native screens
//! alone. Deviation: the Credits, Cinematics and TOS side of the reference screen is absent,
//! because the screen keeps only what logging in needs.

mod screen;
mod smoke;

use benilla_ui::widget::EditBoxState;
use bevy::input::keyboard::KeyboardInput;

use crate::textinput::{self, HostClipboard};
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_protocol::{DialFailure, LoginRefusal, LoginStage};

use crate::client_state::ClientState;
use crate::glue::dialog::{DialogKind, GlueDialog};
use crate::glue_session::login::{
    close_logon, load_saved_account, save_account, Attempt, LoginIntent, LoginNotice, LoginPolicy,
    QuitArm, REALMLIST_BAD,
};
use crate::glue_strings::GlueStrings;
use crate::net::{LoginAbandon, LoginQueuedMessage, LoginStageMessage};
use crate::portrait::{GluePreview, GlueScene};
use crate::sound::GlueSound;

pub(crate) use screen::LoginAction;

/// How long `gsTitleQuit` plays before `AppExit` drops the mixer.
const QUIT_GRACE_SECS: f32 = 0.4;
/// `letters="16"` on both edit boxes (`AccountLogin.xml`).
const MAX_LETTERS: usize = 16;

/// The native login screen.
pub(crate) struct LoginPlugin;

impl Plugin for LoginPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoginForm>()
            .add_systems(
                OnEnter(ClientState::Login),
                (enter_login, show_main_menu).run_if(crate::ui_script::glue::native_glue),
            )
            .add_systems(
                OnExit(ClientState::Login),
                (screen::exit_login, screen::clear_login_scene)
                    .run_if(crate::ui_script::glue::native_glue),
            )
            .add_systems(
                Update,
                (
                    // Every state: a lost session's dialog opens wherever the session died.
                    show_login_notices,
                    (
                        screen::materialize_screen,
                        login_input,
                        tick_login_caret,
                        screen::refresh_boxes,
                        screen::refresh_checkbox,
                        // The shared glue dialog stays inside this chain: ordering it from
                        // outside would order a shared painter against its own
                        // `SystemTypeSet`, which panics at schedule build, at boot, where no
                        // unit test sees it.
                        crate::glue::dialog::drive_glue_dialog,
                        answer_dialog,
                        // After the driver: it spawns the dialog's edit box, and a realmlist
                        // Okay changes the address repainted here.
                        (
                            crate::glue::dialog::refresh_dialog_box,
                            screen::refresh_realmlist,
                        ),
                    )
                        .chain()
                        .before(crate::glue::GlueVisuals)
                        // `answer_dialog` holds the VM, and every VM holder in `Update`
                        // declares its side of the UI tick; a glue screen has nothing the
                        // tick must see.
                        .after(crate::ui_script::UiInput)
                        .run_if(in_state(ClientState::Login)),
                    smoke::debug_login_smoke,
                )
                    .chain()
                    .run_if(crate::ui_script::glue::native_glue)
                    // What the login engine decided this frame, shown this frame.
                    .after(LoginPolicy)
                    .after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// The login engine's decisions on this screen, in every state: the connecting dialog's stage
/// text, the queue dialog, and the dialog each [`LoginNotice`] opens or closes.
fn show_login_notices(
    mut notices: MessageReader<LoginNotice>,
    mut stages: MessageReader<LoginStageMessage>,
    mut queued: MessageReader<LoginQueuedMessage>,
    realm_list_up: Res<crate::realm_select::Realms>,
    mut dialog: ResMut<GlueDialog>,
    strings: Option<Res<GlueStrings>>,
    time: Res<Time>,
) {
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);
    for msg in stages.read() {
        if matches!(dialog.kind, Some(DialogKind::Status)) {
            dialog.set_text(stage_text(strings, msg.stage));
        }
    }
    // Reaching the realm list moves the login state past `LOGIN_STATE_CONNECTING`, and
    // `CGlueMgr::UpdateGlueDialog` (`0x46b140`) then clears the status dialog.
    if realm_list_up.shown && matches!(dialog.kind, Some(DialogKind::Status)) {
        dialog.close();
    }
    // Each queue packet is one sample; the first turns the connecting dialog into the queue
    // dialog.
    for msg in queued.read() {
        if !matches!(dialog.kind, Some(DialogKind::Queued)) {
            dialog.open_queued(msg.realm.clone());
        }
        if let Some(position) = msg.position {
            dialog.queue.sample(position, queue_now_ms(&time));
        }
    }
    // The estimate moves between packets, so the text is recomputed every frame.
    if matches!(dialog.kind, Some(DialogKind::Queued)) {
        let text = dialog.queue.text(
            queue_now_ms(&time),
            dialog.queue_realm.clone().as_deref(),
            strings,
        );
        dialog.set_text(&text);
    }
    for notice in notices.read() {
        match notice {
            LoginNotice::GuardRefused(why) => dialog.open_error(why),
            LoginNotice::Terminal(reason) => dialog.open_error(reason),
            LoginNotice::Refused(refusal) => {
                dialog.open_error(&fail_text(strings, Some(*refusal), None));
            }
            LoginNotice::Unreachable(dial) => {
                dialog.open_error(&fail_text(strings, None, dial.as_ref()));
            }
            LoginNotice::SessionOver => {
                dialog.open_error(strings.text("DISCONNECTED", "Disconnected from server"));
            }
            // Only the dialogs the edge answers: a refused character login's roster follows it
            // on the kept link, and its `Error` is dismissed by its Okay, never by a packet.
            LoginNotice::LoggedOn => {
                if matches!(dialog.kind, Some(DialogKind::Status | DialogKind::Queued)) {
                    dialog.close();
                }
            }
        }
    }
}

/// The queue ring's clock, standing in for the reference's `GetTickCount`: only differences
/// matter, and `u32` keeps the reference's wrapping width.
fn queue_now_ms(time: &Time) -> u32 {
    time.elapsed().as_millis() as u32
}

/// What a Login press should do: the reference's two empty-field guards, as a verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginPress {
    NeedAccount,
    NeedPassword,
    Submit,
}

/// The account is checked first, so an empty form asks for it, the reference's order.
fn login_press(form: &LoginForm) -> LoginPress {
    if form.account.text.is_empty() {
        LoginPress::NeedAccount
    } else if form.password.text.is_empty() {
        LoginPress::NeedPassword
    } else {
        LoginPress::Submit
    }
}

/// The connecting dialog's `LOGIN_STATE_*` text for a stage.
fn stage_text(strings: &GlueStrings, stage: LoginStage) -> &str {
    match stage {
        LoginStage::Connecting => strings.text("LOGIN_STATE_CONNECTING", "Connecting"),
        LoginStage::Authenticating => strings.text("LOGIN_STATE_AUTHENTICATING", "Authenticating"),
        // The native dialog keeps the handshake's text through the world socket's connect.
        LoginStage::Handshaking | LoginStage::WorldConnected => {
            strings.text("LOGIN_STATE_HANDSHAKING", "Handshaking")
        }
    }
}

/// The failure text for a login result; a transport failure (`None`) reads `LOGIN_FAILED`.
///
/// Deviation: a dial that never opened a socket reads `AUTH_LOGIN_SERVER_NOT_FOUND` or
/// `LOGIN_SERVER_DOWN` plus the address, because with an editable realmlist "Unable to connect"
/// cannot tell a bad name from a server that is down.
fn fail_text(
    strings: &GlueStrings,
    refusal: Option<LoginRefusal>,
    dial: Option<&DialFailure>,
) -> String {
    if let Some(dial) = dial {
        let headline = if dial.unresolved {
            strings.text("AUTH_LOGIN_SERVER_NOT_FOUND", "Invalid Login Server")
        } else {
            strings.text("LOGIN_SERVER_DOWN", "Login Server Down")
        };
        return format!("{headline}\n{}", dial.address);
    }
    let authored = match refusal {
        Some(LoginRefusal::World(code)) => world_refusal_text(strings, code),
        Some(LoginRefusal::Logon(code) | LoginRefusal::LogonProof(code)) => {
            logon_refusal_text(strings, Some(code))
        }
        None => logon_refusal_text(strings, None),
    };
    without_dead_url(authored).into_owned()
}

/// The world server's `SMSG_AUTH_RESPONSE` refusal text: the client's dispatch (`0x5aa960`,
/// key table `0x85cae8`), numbered as `AuthResponseCodes` in cmangos `SharedDefines.h:1721+`.
fn world_refusal_text(strings: &GlueStrings, code: u8) -> &str {
    use benilla_protocol::messages as m;
    let (key, fallback): (&str, &str) = match code {
        m::AUTH_FAILED => ("AUTH_FAILED", "Authentication failed"),
        m::AUTH_REJECT => ("AUTH_REJECT", "Login unavailable"),
        m::AUTH_BAD_SERVER_PROOF => ("AUTH_BAD_SERVER_PROOF", "Server is not valid"),
        m::AUTH_UNAVAILABLE => (
            "AUTH_UNAVAILABLE",
            "System unavailable - Please try again later",
        ),
        m::AUTH_SYSTEM_ERROR => ("AUTH_SYSTEM_ERROR", "System Error"),
        m::AUTH_BILLING_ERROR => ("AUTH_BILLING_ERROR", "Billing system error"),
        m::AUTH_BILLING_EXPIRED => ("AUTH_BILLING_EXPIRED", "Account billing has expired"),
        m::AUTH_VERSION_MISMATCH => ("AUTH_VERSION_MISMATCH", "Wrong client version"),
        m::AUTH_UNKNOWN_ACCOUNT => ("AUTH_UNKNOWN_ACCOUNT", "Unknown account"),
        m::AUTH_INCORRECT_PASSWORD => ("AUTH_INCORRECT_PASSWORD", "Incorrect Password"),
        m::AUTH_SESSION_EXPIRED => ("AUTH_SESSION_EXPIRED", "Session Expired"),
        m::AUTH_SERVER_SHUTTING_DOWN => ("AUTH_SERVER_SHUTTING_DOWN", "Server Shutting Down"),
        m::AUTH_ALREADY_LOGGING_IN => ("AUTH_ALREADY_LOGGING_IN", "Already Logging In"),
        m::AUTH_LOGIN_SERVER_NOT_FOUND => ("AUTH_LOGIN_SERVER_NOT_FOUND", "Invalid Login Server"),
        // All but `AUTH_ALREADY_ONLINE` below are the rows of the client's `OKAY_WITH_URL` table
        // (`0x803740`). Deviation: no URL button on the native screen, because `LaunchURL`'s
        // whitelist (`0x85cc34`) names only Blizzard domains that no longer serve these pages.
        m::AUTH_BANNED => (
            "AUTH_BANNED",
            "This account has been banned for violating the Terms of Use Agreement",
        ),
        m::AUTH_ALREADY_ONLINE => ("AUTH_ALREADY_ONLINE", "This character is still logged on"),
        m::AUTH_NO_TIME => (
            "AUTH_NO_TIME",
            "Your World of Warcraft subscription has expired",
        ),
        m::AUTH_DB_BUSY => ("AUTH_DB_BUSY", "This session has timed out"),
        m::AUTH_SUSPENDED => (
            "AUTH_SUSPENDED",
            "This account has been temporarily suspended for violating the Terms of Use Agreement",
        ),
        m::AUTH_PARENTAL_CONTROL => (
            "AUTH_PARENTAL_CONTROL",
            "Access to this account has been blocked by parental controls.",
        ),
        // `AUTH_OK` and `AUTH_WAIT_QUEUE` never reach here; anything else is unknown.
        _ => ("AUTH_FAILED", "Authentication failed"),
    };
    strings.text(key, fallback)
}

/// Cut a failure string at the clause (comma or full stop) before its first web address.
/// Deviation: the reference prints the address, but those Blizzard pages no longer serve and a
/// private realm's player is not served by them. With no boundary before the address (as in
/// `AUTH_BANNED`) the string is left whole.
fn without_dead_url(text: &str) -> std::borrow::Cow<'_, str> {
    let Some(url) = ["www.", "http://", "https://"]
        .iter()
        .filter_map(|p| text.find(p))
        .min()
    else {
        return std::borrow::Cow::Borrowed(text);
    };
    let head = &text[..url];
    let Some(cut) = head.rfind([',', '.']) else {
        return std::borrow::Cow::Borrowed(text);
    };
    let kept = head[..cut].trim_end();
    if kept.is_empty() {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(format!("{kept}."))
}

/// The realmd logon-proof refusal text, the long `LOGIN_*` family: realmd results resolve
/// through key table `0x836b78` (`Logon::OnAuthResult` `0x5b2c90`, byte-index table `0x5b2ea4`,
/// jump table `0x5b2e78`), the world server's through `0x85cae8` ([`world_refusal_text`]).
///
/// 0x04 and 0x05 share one arm, `LOGIN_UNKNOWN_ACCOUNT`: the reference never shows
/// `LOGIN_INCORRECT_PASSWORD` and has no lockout. Codes past 0x0F saturate to the arm that shows
/// `DISCONNECTED`.
fn logon_refusal_text(strings: &GlueStrings, code: Option<u8>) -> &str {
    let (key, fallback): (&str, &str) = match code {
        // No code, unknown0/1, invalid server and no access all read `LOGIN_FAILED`.
        None | Some(0x01) | Some(0x02) | Some(0x0B) | Some(0x0D) => {
            ("LOGIN_FAILED", "Unable to connect")
        }
        Some(0x03) => (
            "LOGIN_BANNED",
            "This World of Warcraft account has been closed and is no longer available for use.  \
             Please go to http://www.worldofwarcraft.com/misc/banned.html for further information. ",
        ),
        // The unknown account and the wrong password alike.
        Some(0x04) | Some(0x05) => (
            "LOGIN_UNKNOWN_ACCOUNT",
            "The information you have entered is not valid.  Please check the spelling of the \
             account name and password.  If you need help in retrieving a lost or stolen password \
             and account, see www.worldofwarcraft.com for more information.",
        ),
        Some(0x06) => (
            "LOGIN_ALREADYONLINE",
            "This account is already logged into World of Warcraft.  Please check the spelling and \
             try again.",
        ),
        Some(0x07) => (
            "LOGIN_NOTIME",
            "You have used up your prepaid time for this account. Please purchase more to continue \
             playing",
        ),
        Some(0x08) => (
            "LOGIN_DBBUSY",
            "Could not log in to World of Warcraft at this time.  Please try again later.",
        ),
        Some(0x09) => (
            "LOGIN_BADVERSION",
            "Unable to validate game version.  This may be caused by file corruption or the \
             interference of another program.  Please visit www.blizzard.com/support/wow/ for more \
             information and possible solutions to this issue.",
        ),
        Some(0x0C) => (
            "LOGIN_SUSPENDED",
            "This World of Warcraft account has been temporarily suspended.  Please go to \
             http://www.worldofwarcraft.com/misc/banned.html for further information.",
        ),
        Some(0x0F) => (
            "LOGIN_PARENTALCONTROL",
            "Access to this account has been blocked by parental controls.  Your settings may be \
             changed in your account preferences at http://www.worldofwarcraft.com.",
        ),
        // 0x0A (version update) is ignored on the proof (`0x5bada2`); every remaining code takes
        // the disconnect arm.
        Some(_) => ("DISCONNECTED", "Disconnected from server"),
    };
    strings.text(key, fallback)
}

// ── The screen's form state + input ──────────────────────────────────────────────────────────────

/// Which edit box has the focus.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    #[default]
    Account,
    Password,
}

/// The typed form: both boxes, the focus and the Remember checkbox. Each box is the shared
/// [`EditBoxState`], caret blink (the client's 0.5 s period) included.
#[derive(Resource)]
pub(crate) struct LoginForm {
    pub(super) account: EditBoxState,
    pub(super) password: EditBoxState,
    pub(super) focus: Field,
    pub(super) save: bool,
}

impl Default for LoginForm {
    fn default() -> Self {
        LoginForm {
            account: textinput::field(MAX_LETTERS, false),
            // Masks the display; the real text is never rendered or copied.
            password: textinput::field(MAX_LETTERS, true),
            focus: Field::default(),
            save: false,
        }
    }
}

impl LoginForm {
    /// Give `field` the keyboard as the stock `AccountLogin.xml` boxes take it: the box left drops
    /// its selection (`OnEditFocusLost`, `HighlightText(0, 0)`), and the box gaining it selects
    /// all (`OnEditFocusGained`, `HighlightText()`), which leaves its caret where it was
    /// (`0x77cca0`). The focus change raises the caret bit (`0x77afa8`), whose flush restarts the
    /// blink (`0x77dd68`, `0x77de35`), so the caret opens solid.
    fn focus(&mut self, field: Field) {
        self.focused().highlight_text(0, 0);
        self.focus = field;
        let f = self.focused();
        f.highlight_text(0, -1);
        f.reset_blink();
    }

    fn focused(&mut self) -> &mut EditBoxState {
        match self.focus {
            Field::Account => &mut self.account,
            Field::Password => &mut self.password,
        }
    }
}

/// `AccountLogin_OnShow`: prefill the saved account, clear the password, focus the first empty
/// box, check Remember when a name is saved.
fn enter_login(mut form: ResMut<LoginForm>) {
    let saved = load_saved_account();
    form.save = !saved.is_empty();
    form.focus = if saved.is_empty() {
        Field::Account
    } else {
        Field::Password
    };
    form.account.set_text(&saved);
    form.password.set_text("");
    // Focus starts the caret solid (`set_text` no-ops on an unchanged name) and selects, so a
    // remembered name is typed over.
    let focus = form.focus;
    form.focus(focus);
}

/// The native login's scene, `AccountLogin.xml`'s `ModelFFX`: `UI_MainMenu` under its fog.
fn show_main_menu(mut preview: ResMut<GluePreview>) {
    preview.scene = Some(GlueScene::MainMenu);
    preview.look = None;
    preview.yaw = 0.0;
    preview.fog = Some(GlueScene::MainMenu.stock_fog());
    // `AccountLogin`'s OnShow (`0x46fa60`).
    preview.shown = preview.shown.wrapping_add(1);
}

/// The screen's input: typing, Tab, Enter submits, Esc quits (an open dialog takes it first),
/// and clicks on the boxes, buttons and checkbox.
#[allow(clippy::type_complexity)]
fn login_input(
    realms: Res<crate::realm_select::Realms>,
    presses: Query<(Entity, &LoginAction, Ref<Interaction>)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut keyboard: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    // The pasteboard, and the layout that names the clipboard chords' letters.
    (mut clipboard, layout): (NonSendMut<HostClipboard>, Res<crate::bindings::LayoutNames>),
    raw_handle: Query<&bevy::window::RawHandleWrapper, With<bevy::window::PrimaryWindow>>,
    mut form: ResMut<LoginForm>,
    mut attempt: Attempt,
    mut dialog: ResMut<GlueDialog>,
    strings: Option<Res<GlueStrings>>,
    mut sounds: MessageWriter<GlueSound>,
    mut quit: Local<bool>,
    mut commands: Commands,
    time: Res<Time>,
) {
    // The reference's `RealmList` is a DIALOG-strata frame over this screen, so while it is up
    // everything here is inert, Escape included (it is the list's Cancel), through the frame
    // its own Escape closes it in.
    if realms.owns_input() {
        return;
    }
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);

    let mut do_login = false;
    let mut do_quit = false;

    let dialog_open = dialog.kind.is_some();

    for (entity, action, interaction) in &presses {
        if dialog_open {
            continue;
        }
        // The edit boxes focus on the press: `CEditBox`'s OnMouseDown (`0x77b800`) takes focus
        // unconditionally, without waiting for a release.
        if interaction.is_changed() && *interaction == Interaction::Pressed {
            match action {
                LoginAction::FocusAccount => form.focus(Field::Account),
                LoginAction::FocusPassword => form.focus(Field::Password),
                _ => {}
            }
        }
        // Everything else here is a Button, which fires on the release.
        if !clicks.hit(entity) {
            continue;
        }
        match action {
            LoginAction::FocusAccount | LoginAction::FocusPassword => {} // focused on the press
            LoginAction::Login => do_login = true,
            LoginAction::Quit => do_quit = true,
            LoginAction::Realmlist => {
                // `gsLoginNewAccount`, which the reference plays for this corner's other buttons
                // (`AccountLogin_ManageAccount`, `AccountLogin_LaunchCommunitySite`).
                sounds.write(GlueSound("gsLoginNewAccount"));
                if attempt.realmlist.pinned_by_env() {
                    // `$WOW_HOST` owns the address for the session; say so rather than open an
                    // editor whose Okay would do nothing.
                    dialog.open_error(&format!(
                        "$WOW_HOST is set for this session, so the realmlist is fixed at {}.",
                        attempt.realmlist.address(),
                    ));
                } else {
                    dialog.open_realmlist(
                        // The reference's registered help text for the `realmList` CVar.
                        "Address of realm list server",
                        attempt.realmlist.address(),
                    );
                }
            }
            LoginAction::ToggleSave => {
                form.save = !form.save;
                // As `AccountLoginSaveAccountName`'s OnClick: checked plays the "Off" kit,
                // unchecked the "On" kit.
                sounds.write(GlueSound(if form.save {
                    "igMainMenuOptionCheckBoxOff"
                } else {
                    "igMainMenuOptionCheckBoxOn"
                }));
                if !form.save {
                    save_account("");
                }
            }
        }
    }

    let mods = textinput::mods_now(&keys);
    let wl = textinput::wayland_display(raw_handle.iter().next());
    for ev in keyboard.read() {
        // An open dialog owns the keyboard (`GlueDialog` is `toplevel`, `enableKeyboard`);
        // Enter and Escape are read by [`crate::glue::dialog::drive_glue_dialog`] as its buttons.
        if dialog_open {
            if dialog.kind.is_some_and(DialogKind::has_edit_box) {
                textinput::feed_key(
                    &mut dialog.edit,
                    ev,
                    mods,
                    &layout,
                    &mut clipboard,
                    wl,
                    textinput::CharFilter::Any,
                );
            }
            continue;
        }
        // The shared edit box first; what it leaves unclaimed is the screen's (Tab here, Enter
        // and Escape below).
        if textinput::feed_key(
            form.focused(),
            ev,
            mods,
            &layout,
            &mut clipboard,
            wl,
            textinput::CharFilter::Any,
        ) == textinput::FieldKey::Consumed
        {
            if form.focus == Field::Account {
                on_account_edited(&mut form);
            }
            continue;
        }
        if ev.state == ButtonState::Pressed && ev.key_code == KeyCode::Tab {
            let next = match form.focus {
                Field::Account => Field::Password,
                Field::Password => Field::Account,
            };
            form.focus(next);
        }
    }

    if !dialog_open
        && (keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter))
    {
        do_login = true;
    }
    if !dialog_open && keys.just_pressed(KeyCode::Escape) {
        do_quit = true;
    }

    if do_login && !attempt.intent.in_flight() {
        // `gsLogin` on every press: `AccountLogin_Login` plays it before `DefaultServerLogin`,
        // so an empty-box refusal clicks too.
        sounds.write(GlueSound("gsLogin"));
        match login_press(&form) {
            // The reference's empty-box guards: a dialog, nothing on the wire.
            LoginPress::NeedAccount => {
                dialog.open_error(
                    strings.text("LOGIN_ENTER_NAME", "Please enter your account name."),
                );
            }
            LoginPress::NeedPassword => {
                dialog.open_error(
                    strings.text("LOGIN_ENTER_PASSWORD", "Please enter your password."),
                );
            }
            LoginPress::Submit => {
                // `AccountLogin_Login`: save or clear the name per the checkbox, then clear the
                // password box.
                if form.save {
                    save_account(&form.account.text);
                } else {
                    save_account("");
                }
                let (user, pass) = (form.account.text.clone(), form.password.text.clone());
                form.password.set_text("");
                dialog.open_status(strings.text("LOGIN_STATE_CONNECTING", "Connecting"));
                attempt.send(&user, &pass, true);
            }
        }
    }
    if do_quit && !*quit {
        *quit = true;
        sounds.write(GlueSound("gsTitleQuit"));
        commands.insert_resource(QuitArm::at(time.elapsed_secs() + QUIT_GRACE_SECS));
    }
}

/// The focused box's caret clock, on the shared 0.5 s period. A dialog with an edit box takes
/// the blink, so only one caret is live; any other dialog leaves the form's caret running.
fn tick_login_caret(mut form: ResMut<LoginForm>, mut dialog: ResMut<GlueDialog>, time: Res<Time>) {
    let dt = time.delta_secs();
    let in_dialog = dialog.kind.is_some_and(DialogKind::has_edit_box);
    textinput::tick_caret(form.focused(), !in_dialog, dt);
    if in_dialog {
        textinput::tick_caret(&mut dialog.edit, true, dt);
    }
}

/// Editing the account box away from the saved name clears the save and unchecks
/// (`OnTextChanged`).
fn on_account_edited(form: &mut LoginForm) {
    if form.save {
        let saved = load_saved_account();
        if !saved.is_empty() && saved != form.account.text {
            save_account("");
            form.save = false;
        }
    }
}

// ── The GlueDialog (connecting / error) ──────────────────────────────────────────────────────────

/// What a glue-dialog press means on this screen; the widget
/// ([`crate::glue::dialog::drive_glue_dialog`]) owns the tree, keys and sound, and ends an `Error`.
fn answer_dialog(
    mut commands: Commands,
    mut answers: MessageReader<crate::glue::dialog::GlueDialogAnswer>,
    mut dialog: ResMut<GlueDialog>,
    mut intent: ResMut<LoginIntent>,
    mut realmlist: ResMut<crate::realmlist::Realmlist>,
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    abandon: Res<LoginAbandon>,
    realm: Option<Res<crate::net::RealmChoice>>,
) {
    for answer in answers.read() {
        match answer.kind {
            // Already dismissed by the widget.
            DialogKind::Error => {}
            DialogKind::Realmlist => {
                // Okay takes the address or stays open with the text as typed, the one press
                // that does not end its dialog. Cancel (or Escape) closes.
                if answer.button1 {
                    let typed = dialog.edit.text.clone();
                    if accept_realmlist(&typed, &mut realmlist, script.as_deref_mut()) {
                        dialog.dismiss(&mut commands);
                    } else {
                        dialog.set_text(REALMLIST_BAD);
                    }
                } else if answer.button2 {
                    dialog.dismiss(&mut commands);
                }
            }
            // Cancel, as the reference's state-1 Cancel (`0x46b73d` → `CLoginMgr::Cancel`).
            DialogKind::Status | DialogKind::Queued => {
                close_logon(&abandon, &mut intent, realm.as_deref());
                dialog.dismiss(&mut commands);
            }
        }
    }
}

/// Take the realmlist dialog's Okay: normalize, point the session at it, and write the
/// `realmList` CVar so it persists; `false` keeps the dialog open. A CVar the VM has not
/// registered yet is not saved, but the session value stands.
fn accept_realmlist(
    typed: &str,
    realmlist: &mut crate::realmlist::Realmlist,
    script: Option<&mut benilla_ui::script::UiScript>,
) -> bool {
    let Some(address) = crate::realmlist::normalize(typed) else {
        return false;
    };
    realmlist.set(&address);
    let name = crate::realmlist::CVAR_REALMLIST;
    match script {
        Some(script) => {
            script.set_cvar_engine(name, &address);
            if script.cvar(name).is_some() {
                info!("login: realmlist -> {address}");
            } else {
                warn!(
                    "login: realmlist -> {address} for this session, but the VM has not registered \
                     {name} yet, so it was not saved"
                );
            }
        }
        None => {
            warn!("login: realmlist -> {address} for this session only — no UI VM to persist it")
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::glue_session::login::{answer_glue, GlueLogin};
    use crate::net::LoginSubmit;

    /// Escape on the realm list over this screen closes the list and nothing else: the press is
    /// the list's (`Realms::owns_input`), so the login screen does not also quit, in either
    /// order the two run.
    #[test]
    fn escape_on_the_realm_list_does_not_also_quit() {
        for list_first in [true, false] {
            let (tx, _requests) = crossbeam_channel::unbounded();
            let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .init_resource::<crate::glue::GlueClicks>()
                .init_resource::<ButtonInput<KeyCode>>()
                .init_resource::<crate::bindings::LayoutNames>()
                .insert_non_send_resource(HostClipboard::default())
                .init_resource::<LoginForm>()
                .init_resource::<LoginIntent>()
                .init_resource::<GlueDialog>()
                .init_resource::<crate::realm_select::Realms>()
                .insert_resource(crate::realmlist::Realmlist::unpinned(
                    crate::realmlist::DEFAULT_REALMLIST,
                ))
                .insert_resource(LoginSubmit(tx))
                .insert_resource(LoginAbandon(std::sync::Arc::new(
                    std::sync::atomic::AtomicU64::new(0),
                )))
                .insert_resource(crate::net::RealmChoice(realm_tx.into()))
                .add_message::<KeyboardInput>()
                .add_message::<GlueSound>()
                .add_message::<GlueLogin>()
                .add_message::<bevy::input::mouse::MouseWheel>()
                .add_systems(PreUpdate, crate::realm_select::take_input);
            let list = crate::realm_select::list_keys;
            if list_first {
                app.add_systems(Update, (list, login_input).chain());
            } else {
                app.add_systems(Update, (login_input, list).chain());
            }
            // The list stands over this screen, as the realm park raises it after logon.
            app.world_mut()
                .resource_mut::<crate::realm_select::Realms>()
                .shown = true;
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Escape);
            app.update();

            let order = if list_first {
                "list first"
            } else {
                "screen first"
            };
            assert!(
                !app.world().resource::<crate::realm_select::Realms>().shown,
                "{order}: the list's Cancel closed it"
            );
            let asks = app
                .world()
                .resource::<bevy::ecs::message::Messages<GlueLogin>>();
            assert!(
                asks.iter_current_update_messages()
                    .any(|a| matches!(a, GlueLogin::Abandon)),
                "{order}: and ended the attempt, which closes the logon"
            );
            assert!(
                realm_rx.try_recv().is_err(),
                "{order}: the login engine sends the close"
            );
            assert!(
                !app.world().contains_resource::<QuitArm>(),
                "{order}: the press was the list's, so the login screen does not also quit"
            );
        }
    }

    /// The status dialog's Cancel is `CLoginMgr::Cancel` (`0x46b73d` → `0x5b3320`): the attempt
    /// ends at its next boundary and the logon park closes, so a logon that already finished is
    /// not left parked.
    #[test]
    fn the_status_cancel_closes_the_logon() {
        let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
        let abandon = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut app = App::new();
        app.init_resource::<GlueDialog>()
            .insert_resource(LoginIntent::test_in_flight("player"))
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                crate::realmlist::DEFAULT_REALMLIST,
            ))
            .insert_resource(LoginAbandon(abandon.clone()))
            .insert_resource(crate::net::RealmChoice(realm_tx.into()))
            .add_message::<crate::glue::dialog::GlueDialogAnswer>()
            .add_systems(Update, answer_dialog);
        app.world_mut()
            .write_message(crate::glue::dialog::GlueDialogAnswer {
                kind: DialogKind::Status,
                button1: true,
                button2: false,
            });
        app.update();
        assert_eq!(abandon.load(Ordering::SeqCst), 1);
        assert!(matches!(
            realm_rx.try_recv(),
            Ok(crate::net::RealmRequest::Close)
        ));
        assert!(!app.world().resource::<LoginIntent>().in_flight());
    }

    /// The realm list's Cancel over the login screen ends the attempt (#833): the logon closes
    /// and the next Login is sent, where the attempt left in flight refused it.
    #[test]
    fn the_realm_lists_cancel_lets_the_next_login_through() {
        let (tx, requests) = crossbeam_channel::unbounded();
        let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<crate::glue::GlueClicks>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<crate::bindings::LayoutNames>()
            .insert_non_send_resource(HostClipboard::default())
            .init_resource::<LoginForm>()
            .insert_resource(LoginIntent::test_in_flight("probe"))
            .init_resource::<GlueDialog>()
            .init_resource::<crate::realm_select::Realms>()
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                crate::realmlist::DEFAULT_REALMLIST,
            ))
            .insert_resource(LoginSubmit(tx))
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(0),
            )))
            .insert_resource(crate::net::RealmChoice(realm_tx.into()))
            .insert_resource(State::new(ClientState::Login))
            .add_message::<KeyboardInput>()
            .add_message::<GlueSound>()
            .add_message::<GlueLogin>()
            .add_message::<bevy::input::mouse::MouseWheel>()
            .add_systems(PreUpdate, crate::realm_select::take_input)
            .add_systems(
                Update,
                (crate::realm_select::list_keys, answer_glue, login_input).chain(),
            );
        {
            let w = app.world_mut();
            let mut form = w.resource_mut::<LoginForm>();
            form.account.text = "probe".into();
            form.password.text = "secret".into();
            // The attempt reached the realm park, which raised the list over this screen.
            w.resource_mut::<crate::realm_select::Realms>().shown = true;
        }
        let press = |app: &mut App, key: KeyCode| {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        };
        press(&mut app, KeyCode::Escape);
        assert!(
            matches!(realm_rx.try_recv(), Ok(crate::net::RealmRequest::Close)),
            "Cancel closes the logon"
        );
        assert!(!app.world().resource::<LoginIntent>().in_flight());
        app.update(); // the list's input ownership settles
        press(&mut app, KeyCode::Enter);
        let req = requests.try_recv().expect("the next Login is sent");
        assert_eq!(req.user, "probe");
    }

    /// [`show_login_notices`] alone, on the native screens' dialog.
    fn notice_app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<GlueDialog>()
            .init_resource::<crate::realm_select::Realms>()
            .add_message::<LoginNotice>()
            .add_message::<LoginStageMessage>()
            .add_message::<LoginQueuedMessage>()
            .add_systems(Update, show_login_notices);
        app
    }

    /// Each notice opens its own words; the strings table is empty, so the fallbacks show.
    #[test]
    fn each_notice_opens_its_dialog() {
        let down = DialFailure {
            address: "127.0.0.1:3724".into(),
            unresolved: false,
        };
        for (notice, text) in [
            (
                LoginNotice::GuardRefused("not this checkout's account".into()),
                "not this checkout's account".to_string(),
            ),
            (
                LoginNotice::Terminal("the server is full".into()),
                "the server is full".to_string(),
            ),
            (
                LoginNotice::Refused(LoginRefusal::Logon(0x06)),
                fail_text(
                    &GlueStrings::default(),
                    Some(LoginRefusal::Logon(0x06)),
                    None,
                ),
            ),
            (
                LoginNotice::Unreachable(Some(down)),
                "Login Server Down\n127.0.0.1:3724".to_string(),
            ),
            (
                LoginNotice::Unreachable(None),
                "Unable to connect".to_string(),
            ),
            // The same words as `GlueStrings.lua`'s `DISCONNECTED`.
            (
                LoginNotice::SessionOver,
                "Disconnected from server".to_string(),
            ),
        ] {
            let mut app = notice_app();
            app.world_mut()
                .resource_mut::<GlueDialog>()
                .open_status("Connecting");
            app.world_mut().write_message(notice.clone());
            app.update();
            let dialog = app.world().resource::<GlueDialog>();
            assert_eq!(dialog.kind, Some(DialogKind::Error), "{notice:?}");
            assert_eq!(dialog.text, text, "{notice:?}");
        }
    }

    /// A refused character login's roster comes on the kept link right behind the refusal; its
    /// edge must not close the refusal's `Error`.
    #[test]
    fn the_login_edge_closes_the_connecting_dialog_but_not_an_error() {
        let mut app = notice_app();
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_error("World server is down");
        app.world_mut().write_message(LoginNotice::LoggedOn);
        app.update();
        let dialog = app.world().resource::<GlueDialog>();
        assert_eq!(dialog.kind, Some(DialogKind::Error));
        assert_eq!(dialog.text, "World server is down");

        for open in [
            |d: &mut GlueDialog| d.open_status("Connecting"),
            |d: &mut GlueDialog| d.open_queued(Some("Fake".into())),
        ] {
            open(&mut app.world_mut().resource_mut::<GlueDialog>());
            app.world_mut().write_message(LoginNotice::LoggedOn);
            app.update();
            assert!(app.world().resource::<GlueDialog>().kind.is_none());
        }
    }

    /// A pasted `realmlist.wtf` line becomes the session's address; with no VM, nothing persists.
    #[test]
    fn the_realmlist_dialog_takes_what_was_typed() {
        let mut realmlist = crate::realmlist::Realmlist::unpinned("localhost");
        assert!(accept_realmlist(
            r#"  SET realmlist "logon.example.org"  "#,
            &mut realmlist,
            None,
        ));
        assert_eq!(realmlist.address(), "logon.example.org");
    }

    #[test]
    fn a_bad_address_leaves_the_realmlist_alone() {
        let mut realmlist = crate::realmlist::Realmlist::unpinned("localhost");
        for typed in ["", "   ", "logon.example.org and more", "host:notaport"] {
            assert!(
                !accept_realmlist(typed, &mut realmlist, None),
                "{typed:?} is not an address",
            );
            assert_eq!(realmlist.address(), "localhost");
        }
    }

    /// The wiring from the widget's published press to this screen's answer.
    #[test]
    fn a_published_press_reaches_the_login_screens_answer() {
        use crate::glue::dialog::GlueDialogAnswer;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(LoginIntent::test_in_flight("player"))
            .init_resource::<GlueDialog>()
            .insert_resource(crate::realmlist::Realmlist::unpinned("localhost"))
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(0),
            )))
            .add_message::<GlueDialogAnswer>()
            .add_systems(Update, answer_dialog);

        // The status dialog's Cancel abandons the attempt and forgets the credentials.
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_status("Connecting");
        app.world_mut().write_message(GlueDialogAnswer {
            kind: DialogKind::Status,
            button1: true,
            button2: false,
        });
        app.update();
        let intent = app.world().resource::<LoginIntent>();
        assert!(
            !intent.in_flight(),
            "the cancelled attempt is no longer in flight"
        );
        assert!(
            intent.account().is_none(),
            "and it does not silently resubmit"
        );
        assert_eq!(
            app.world()
                .resource::<LoginAbandon>()
                .0
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the abandon generation moved, so the attempt still in flight is discarded",
        );
        assert!(app.world().resource::<GlueDialog>().kind.is_none());

        // The realmlist editor's Okay: the typed address becomes the session's.
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_realmlist("Address of realm list server", "localhost");
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .edit
            .set_text("logon.example.org");
        app.world_mut().write_message(GlueDialogAnswer {
            kind: DialogKind::Realmlist,
            button1: true,
            button2: false,
        });
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::realmlist::Realmlist>()
                .address(),
            "logon.example.org",
        );
        assert!(app.world().resource::<GlueDialog>().kind.is_none());

        // Okay over a bad address keeps the dialog up with the prompt replaced.
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_realmlist("Address of realm list server", "logon.example.org");
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .edit
            .set_text("not an address at all");
        app.world_mut().write_message(GlueDialogAnswer {
            kind: DialogKind::Realmlist,
            button1: true,
            button2: false,
        });
        app.update();
        let dialog = app.world().resource::<GlueDialog>();
        assert_eq!(dialog.kind, Some(DialogKind::Realmlist), "it stays open");
        assert_eq!(dialog.text, REALMLIST_BAD, "saying why");
        assert_eq!(
            app.world()
                .resource::<crate::realmlist::Realmlist>()
                .address(),
            "logon.example.org",
            "and the address it refused is unchanged",
        );
    }

    /// The editor opens on the current address, selected whole.
    #[test]
    fn opening_the_editor_preselects_the_current_address() {
        let mut dialog = GlueDialog::default();
        dialog.open_realmlist("Address of realm list server", "logon.example.org");
        assert_eq!(dialog.kind, Some(DialogKind::Realmlist));
        assert_eq!(dialog.edit.text, "logon.example.org");
        assert_eq!(
            dialog.edit.selected_text().as_deref(),
            Some("logon.example.org")
        );
        assert_eq!(dialog.edit.max_letters, crate::realmlist::MAX_LETTERS);
        assert!(!dialog.edit.password, "an address is not a secret");
    }

    #[test]
    fn a_dead_url_is_cut_at_its_clause() {
        // `LOGIN_UNKNOWN_ACCOUNT`, verbatim from `GlueStrings.lua`.
        assert_eq!(
            without_dead_url(
                "The information you have entered is not valid.  Please check the spelling of the \
                 account name and password.  If you need help in retrieving a lost or stolen \
                 password and account, see www.worldofwarcraft.com for more information.",
            ),
            "The information you have entered is not valid.  Please check the spelling of the \
             account name and password.  If you need help in retrieving a lost or stolen password \
             and account.",
        );
        assert_eq!(
            without_dead_url(
                "This World of Warcraft account has been closed and is no longer available for \
                 use.  Please go to http://www.worldofwarcraft.com/misc/banned.html for further \
                 information. ",
            ),
            "This World of Warcraft account has been closed and is no longer available for use.",
        );
        // Nothing to cut: borrowed, not rebuilt.
        let clean = "You have used up your prepaid time for this account.";
        assert!(matches!(
            without_dead_url(clean),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(without_dead_url(clean), clean);
    }

    /// `AUTH_BANNED`'s address sits behind a dash, with no clause to cut at.
    #[test]
    fn an_address_with_no_clause_boundary_is_left_alone() {
        let banned = "This account has been banned for violating the Terms of Use Agreement - \
                      www.worldofwarcraft.com/termsofuse.shtml. Please contact our GM department.";
        assert_eq!(without_dead_url(banned), banned);
        // The degenerate case cannot produce an empty dialog.
        assert_eq!(without_dead_url("www.example.com"), "www.example.com");
        assert_eq!(without_dead_url(". www.example.com"), ". www.example.com");
    }

    #[test]
    fn the_login_press_asks_for_the_account_before_the_password() {
        let mut form = LoginForm::default();
        assert_eq!(login_press(&form), LoginPress::NeedAccount);
        form.account.set_text("player");
        assert_eq!(login_press(&form), LoginPress::NeedPassword);
        form.password.set_text("secret");
        assert_eq!(login_press(&form), LoginPress::Submit);
        let mut only_pass = LoginForm::default();
        only_pass.password.set_text("secret");
        assert_eq!(login_press(&only_pass), LoginPress::NeedAccount);
    }

    #[test]
    fn focusing_a_box_selects_all_of_it() {
        let mut form = LoginForm::default();
        form.account.set_text("remembered");
        form.password.set_text("secret");

        // Mid-blink, caret off and mid-text: the focus restarts it solid, where it was.
        form.account.caret_shown = false;
        form.account.blink_accum = 0.3;
        form.account.cursor = 3;
        form.focus(Field::Account);
        assert_eq!(form.focus, Field::Account);
        assert_eq!(form.account.selected_text().as_deref(), Some("remembered"));
        assert_eq!(form.account.cursor, 3, "HighlightText leaves the caret");
        assert!(form.account.caret_shown, "a fresh focus starts solid");
        assert_eq!(form.account.blink_accum, 0.0, "with a full half-period");

        // Checked as a range: a password box's `selected_text` returns the `*` mask.
        form.focus(Field::Password);
        assert_eq!(
            (form.password.sel_start, form.password.sel_end),
            (0, form.password.text.len()),
            "the whole password is selected even though it cannot be read back",
        );

        assert_eq!(
            form.account.selected_text(),
            None,
            "the box that lost the keyboard keeps no highlight",
        );

        form.focused().insert("x");
        assert_eq!(form.password.text, "x");

        form.focus(Field::Account);
        assert_eq!(
            form.password.selected_text(),
            None,
            "and back the other way"
        );
    }

    /// The realmd map, against the client's table (`0x5b2c90`, key table `0x836b78`).
    #[test]
    fn fail_text_maps_the_verified_codes() {
        let strings = GlueStrings::default(); // empty table: the fallback literals
        let logon = |b| fail_text(&strings, Some(LoginRefusal::Logon(b)), None);

        // 0x04 and 0x05 share one jump-table arm.
        assert!(logon(0x04).starts_with("The information you have entered is not valid."));
        assert_eq!(logon(0x05), logon(0x04), "one arm, byte-identical");
        assert!(logon(0x04).contains("account name and password"));

        assert_eq!(fail_text(&strings, None, None), "Unable to connect");
        for code in [0x01, 0x02, 0x0B, 0x0D] {
            assert_eq!(logon(code), "Unable to connect", "code {code:#04x}");
        }

        assert!(logon(0x03).contains("has been closed"));
        assert!(logon(0x06).contains("already logged into"));
        assert!(logon(0x09).contains("Unable to validate game version"));
        assert!(logon(0x0C).contains("temporarily suspended"));
        assert!(logon(0x0F).contains("parental controls"));

        // Past 0x0F the byte-index table saturates onto the disconnect arm.
        for code in [0x10, 0x11, 0x12, 0xEE] {
            assert_eq!(logon(code), "Disconnected from server", "code {code:#04x}");
        }
    }

    /// The realmd and world result enums share bytes with different meanings: 0x0C is
    /// `AUTH_LOGON_FAILED_SUSPENDED` to realmd and `AUTH_OK` to the world server.
    #[test]
    fn the_two_auth_enums_do_not_share_a_byte() {
        use benilla_protocol::messages as m;
        let strings = GlueStrings::default();
        let logon = |b| fail_text(&strings, Some(LoginRefusal::Logon(b)), None);
        let world = |b| fail_text(&strings, Some(LoginRefusal::World(b)), None);

        assert!(logon(0x0C).contains("temporarily suspended"));
        assert_eq!(world(m::AUTH_BILLING_ERROR), "Billing system error");
        assert_ne!(logon(0x0C), world(0x0C));

        // The client's world dispatch (`0x5aa960`).
        assert_eq!(world(m::AUTH_INCORRECT_PASSWORD), "Incorrect Password");
        assert_eq!(world(m::AUTH_SESSION_EXPIRED), "Session Expired");
        assert_eq!(world(m::AUTH_SERVER_SHUTTING_DOWN), "Server Shutting Down");
        assert_eq!(world(m::AUTH_ALREADY_LOGGING_IN), "Already Logging In");
        assert_eq!(
            world(m::AUTH_UNAVAILABLE),
            "System unavailable - Please try again later"
        );
        // Rows of the reference's `OKAY_WITH_URL` table.
        assert!(world(m::AUTH_BANNED).contains("banned"));
        assert!(world(m::AUTH_SUSPENDED).contains("temporarily suspended"));
        assert!(world(m::AUTH_PARENTAL_CONTROL).contains("parental controls"));
        // An unknown code lands on the catch-all.
        assert_eq!(world(0xEE), "Authentication failed");
    }

    /// Each dial verdict gets the reference's string for its condition, with the address under it.
    #[test]
    fn a_dial_failure_says_which_failure_it_was() {
        let strings = GlueStrings::default();
        let down = DialFailure {
            address: "127.0.0.1:3724".into(),
            unresolved: false,
        };
        assert_eq!(
            fail_text(&strings, None, Some(&down)),
            "Login Server Down\n127.0.0.1:3724",
            "the address resolved and nothing answered — editing the address will not help",
        );
        let missing = DialFailure {
            address: "logon.nonesuch.example:3724".into(),
            unresolved: true,
        };
        assert_eq!(
            fail_text(&strings, None, Some(&missing)),
            "Invalid Login Server\nlogon.nonesuch.example:3724",
            "the name resolved to nothing — this one IS the address",
        );
        // A refusal keeps its own string.
        assert!(fail_text(&strings, Some(LoginRefusal::Logon(0x05)), None)
            .starts_with("The information you have entered is not valid."));
    }

    /// The focused box blinks on the shared 0.5 s period; the other is left alone.
    #[test]
    fn the_focused_box_caret_blinks() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<LoginForm>()
            .init_resource::<GlueDialog>()
            .add_systems(Update, tick_login_caret);

        let past_the_period = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(600));
            app.update();
            app.world().resource::<LoginForm>().account.caret_shown
        };

        // Account has the focus by default.
        assert!(!past_the_period(&mut app), "the first period turns it off");
        assert!(past_the_period(&mut app), "the second turns it back on");
        // The unfocused box never accumulates, so focusing it lands on a solid caret.
        assert_eq!(
            app.world().resource::<LoginForm>().password.blink_accum,
            0.0
        );
    }
}
