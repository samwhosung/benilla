//! The login screen (decision 0539) — the faithful `AccountLogin` glue, functional core only:
//! the `UI_MainMenu` scene with its authored fog/fires, the account/password boxes, Remember
//! Account Name, Login/Quit, the version block, and the connecting/error dialogs. The Credits/
//! Cinematics/TOS side of the reference screen is deliberately cut (the director's call).
//!
//! This module owns the **credential policy** — the 0193 §3 mirror for the IO thread's pre-logon
//! park: the env fast path (`WOW_USER` and `WOW_PASS` both set auto-submits
//! with the old `one`/`pone` defaults, so every probe/smoke invocation keeps working), the
//! pending-credentials resubmit (paced at the flat 3 s, app-side — the IO thread never sleeps),
//! and the director's typed submit. A *refused* code (bad password) clears the intent and shows
//! the authored `AUTH_*` dialog — never an auto-retry against a refusal.
//!
//! **A session that is lost is over** (decision 1262): the reference's `GlueParent.lua` answers
//! `DISCONNECTED_FROM_SERVER` with `SetGlueScreen("login")` + `GlueDialog_Show("DISCONNECTED")`,
//! and so does this. 0065's seamless reconnect survives only where the run has *declared* that
//! nobody is here ([`crate::run_mode::unattended`], decision 1769) — because a client that
//! re-authenticates on its own takes the account back off whoever just displaced it.
//!
//! Module split: this file (state, policy, input, dialogs, the saved-account persistence),
//! [`screen`] (the authored layout, transcribed from `AccountLogin.xml`), [`smoke`] (the
//! `WOW_LOGIN_SMOKE` headless prover).

pub(crate) mod queue;
mod screen;
mod smoke;

use std::sync::atomic::Ordering;

use benilla_ui::widget::EditBoxState;
use bevy::input::keyboard::KeyboardInput;

use crate::textinput::{self, HostClipboard};
use bevy::input::ButtonState;
use bevy::prelude::*;

use benilla_protocol::{DialFailure, LoginRefusal, LoginStage};

use crate::char_select::ClientState;
use crate::glue::dialog::{DialogKind, GlueDialog};
use crate::glue_strings::GlueStrings;
use crate::net::{
    CharListMessage, DisconnectedMessage, LoginAbandon, LoginFailedMessage, LoginQueuedMessage,
    LoginRequest, LoginStageMessage, LoginSubmit,
};
use crate::portrait::{GluePreview, GlueScene};
use crate::sound::GlueSound;

pub(crate) use screen::LoginAction;
pub(crate) use smoke::smoke_character;

/// The flat resubmit pacing after a transport failure with pending credentials (decision 0065's
/// reconnect cadence, moved app-side by 0539 — the IO thread never sleeps).
const RETRY_DELAY_SECS: f32 = 3.0;
/// The quit grace: `gsTitleQuit` gets this long to be audible before `AppExit` drops the mixer.
const QUIT_GRACE_SECS: f32 = 0.4;
/// The ref's `letters="16"` on both edit boxes.
const MAX_LETTERS: usize = 16;

pub(crate) struct LoginPlugin;

impl Plugin for LoginPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoginIntent>()
            .init_resource::<LoginForm>()
            .add_systems(OnEnter(ClientState::Login), enter_login)
            .add_systems(OnExit(ClientState::Login), screen::exit_login)
            .add_systems(
                Update,
                (
                    // Policy + transitions run in EVERY state: the reconnect resubmit fires while
                    // `InWorld`, and the roster edge lands wherever it lands.
                    (drive_policy, to_select_on_roster, drive_quit).chain(),
                    (
                        screen::materialize_screen,
                        login_input,
                        tick_login_caret,
                        screen::refresh_boxes,
                        screen::refresh_checkbox,
                        // The shared glue dialog (2084) — the widget is `crate::glue::dialog`'s
                        // and the select screen runs the same system over the same resource; what
                        // a press *means* here is [`answer_dialog`]'s, right behind it. It sits
                        // inside this chain rather than being hoisted out with a `before`/`after`
                        // pair, because the shared painters below are in this chain too and
                        // ordering across it makes a painter transitively ordered against its own
                        // `SystemTypeSet` — rejected at schedule build, so it panics at boot and
                        // no unit test sees it (2071).
                        crate::glue::dialog::drive_glue_dialog,
                        answer_dialog,
                        // Both after the driver: it is what spawns the dialog's edit box, and a
                        // realmlist Okay is what changes the address this repaints.
                        (
                            crate::glue::dialog::refresh_dialog_box,
                            screen::refresh_realmlist,
                        ),
                    )
                        .chain()
                        .before(crate::glue::GlueVisuals)
                        // After the UI tick: one member holds the VM (`answer_dialog` writes
                        // into it), and every VM holder in `Update` declares its side of the
                        // tick (decision 2304). A glue screen has no push the tick must see,
                        // so the whole chain takes the drain side.
                        .after(crate::ui_script::UiInput)
                        .run_if(in_state(ClientState::Login)),
                    (smoke::debug_login_smoke, screen::debug_login_shot),
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

// ── The credential policy ────────────────────────────────────────────────────────────────────────

/// Where the IO thread's read loop currently is, as far as the app can tell — the policy submits
/// credentials only while it's parked pre-logon.
#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum IoPark {
    /// Parked at the pre-logon park (boot, a failure, a disconnect, a Back).
    #[default]
    AtLogin,
    /// Past logon — parked at select or streaming the world.
    Active,
}

/// The credential policy's memory (the 0193 §3 mirror): the last credentials this session
/// authenticated (or asked) with, the in-flight/park bookkeeping, and the resubmit timer.
#[derive(Resource, Default)]
pub(crate) struct LoginIntent {
    /// The session's credentials — kept while in-world so the logout relist and an unattended
    /// run's reconnect re-authenticate silently (0065); cleared by select's Back, a refusal code,
    /// a Cancel, and by a lost session (1262 — they are the session's, and the session is over).
    creds: Option<(String, String)>,
    /// A submit is in flight (between our send and its LoginFailed/CharacterList answer).
    in_flight: bool,
    /// Whether the in-flight submit came from the screen (it announced a connecting dialog and
    /// wants its failure surfaced) or from the silent auto path.
    announced: bool,
    park: IoPark,
    /// `Time::elapsed_secs` deadline for the next silent resubmit (`None` = no retry scheduled).
    retry_at: Option<f32>,
    /// Env fast path read latch (checked once, on the first policy run).
    env_read: bool,
}

impl LoginIntent {
    /// Forget the session's credentials and any scheduled retry (select's Back, a refusal).
    pub(crate) fn clear(&mut self) {
        self.creds = None;
        self.retry_at = None;
    }

    /// The account this session authenticated as, however it got there — the env fast path or the
    /// login screen. The one honest answer to "whose body is this?", which is what decides whether
    /// the probe shield has any business touching it (decision 0677).
    pub(crate) fn account(&self) -> Option<&str> {
        self.creds.as_ref().map(|(user, _)| user.as_str())
    }
}

/// **Everything one login attempt is made of**, as a single [`SystemParam`]: the policy's memory,
/// the channel to the parked IO thread, the abandon generation a Cancel bumps, and — since
/// decision 1667 — the realmlist it dials.
///
/// A bundle rather than four parameters, for the reason the CVar host's old knob bundle had
/// (retired by 2303): adding the realmlist put
/// [`login_input`] at **seventeen** parameters, one past Bevy's ceiling, and the three systems
/// that submit were already re-typing the same four names. Now a submit is one call on one param,
/// and the next thing an attempt needs is one field here instead of a fourth signature to widen.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Attempt<'w> {
    pub(super) intent: ResMut<'w, LoginIntent>,
    submit: Res<'w, LoginSubmit>,
    abandon: Res<'w, LoginAbandon>,
    realmlist: Res<'w, crate::realmlist::Realmlist>,
}

impl Attempt<'_> {
    /// Send one login attempt to the parked IO thread, stamped with the current abandon
    /// generation.
    ///
    /// The realmlist is read **at submit time** (decision 1667) rather than by the IO thread, so a
    /// resubmit fired after the player repointed the client dials the new server while an attempt
    /// already on the wire keeps the one it started with.
    fn send(&mut self, user: &str, pass: &str, announced: bool) {
        self.intent.creds = Some((user.to_string(), pass.to_string()));
        self.intent.in_flight = true;
        self.intent.announced = announced;
        self.intent.retry_at = None;
        let _ = self.submit.0.send(LoginRequest {
            user: user.to_string(),
            pass: pass.to_string(),
            host: self.realmlist.address().to_string(),
            generation: self.abandon.0.load(Ordering::SeqCst),
        });
    }
}

/// The millisecond clock the queue ring is stamped with — the app's own elapsed time, which is
/// what the reference's `GetTickCount` is here. Only differences matter to the estimate, so an
/// arbitrary epoch is fine; `u32` keeps the wrapping arithmetic the same width the reference used.
fn queue_now_ms(time: &Time) -> u32 {
    time.elapsed().as_millis() as u32
}

/// What a Login press should do — the reference's two guards, as a verdict.
///
/// Extracted from [`login_input`] so the validation can be tested without standing a screen up,
/// and so the click sound has somewhere to live that is not inside one of its branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginPress {
    NeedAccount,
    NeedPassword,
    Submit,
}

/// The account box is checked first, so a wholly empty form asks for the account name — which is
/// the order the reference's dialogs come in.
fn login_press(form: &LoginForm) -> LoginPress {
    if form.account.text.is_empty() {
        LoginPress::NeedAccount
    } else if form.password.text.is_empty() {
        LoginPress::NeedPassword
    } else {
        LoginPress::Submit
    }
}

/// The `LOGIN_STATE_*` glue string for a stage (the connecting dialog's text).
fn stage_text(strings: &GlueStrings, stage: LoginStage) -> &str {
    match stage {
        LoginStage::Connecting => strings.text("LOGIN_STATE_CONNECTING", "Connecting"),
        LoginStage::Authenticating => strings.text("LOGIN_STATE_AUTHENTICATING", "Authenticating"),
        LoginStage::Handshaking => strings.text("LOGIN_STATE_HANDSHAKING", "Handshaking"),
    }
}

/// The authored failure string for an auth result byte (the vmangos-verified map, decision 0539
/// §6): each row is the client's own `GlueStrings` text; a transport failure (`None`) reads
/// `LOGIN_FAILED` ("Unable to connect").
///
/// **A dial that never opened a socket answers ahead of all of them**, because it is the only
/// failure the player can act on and — since 1667 made the address editable — the only one they can
/// misread. "Unable to connect" cannot distinguish *that name does not exist* from *the address is
/// fine and nothing is listening*, so a player whose server is simply down edits a correct address
/// until they give up. That is what the editor's first live use produced. Both replacement strings
/// are the reference's own (`AUTH_LOGIN_SERVER_NOT_FOUND` / `LOGIN_SERVER_DOWN`, so they localize);
/// the address line under them is ours, because the reference never had an address to get wrong.
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
        Some(LoginRefusal::Logon(code)) => logon_refusal_text(strings, Some(code)),
        None => logon_refusal_text(strings, None),
    };
    without_dead_url(authored).into_owned()
}

/// **The world server's** `SMSG_AUTH_RESPONSE` refusal, in the client's own words.
///
/// A straight transcription of the client's own dispatch over this enum (`0x5aa960`, through the
/// key table `0x85cae8`) — each case loads exactly the `GlueStrings` key named below,
/// and the numbering is `AuthResponseCodes` in cmangos `SharedDefines.h:1721+`. Nothing here is a
/// judgement call; where the client picks a string, so do we.
///
/// Every one of these used to arrive as a generic "Unable to connect", because the world result
/// was formatted into an error string in `WorldSession::connect` and the byte thrown away.
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
        // The tail past the queue. These five are exactly the rows of the client's `OKAY_WITH_URL`
        // table (`0x803740`), which is why the URL dialog is reachable only from THIS enum: the
        // lookup is a linear search over a ClientServices code and realmd's results never enter it.
        // benilla shows the text without the URL button — `LaunchURL` is a shell-out we do not do,
        // and its whitelist (`0x85cc34`) is ten worldofwarcraft.com/blizzard.com domains that have
        // not served these pages in fifteen years.
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
        // `AUTH_OK` never reaches here (it is the success path) and `AUTH_WAIT_QUEUE` is not a
        // refusal at all; anything else is a code this client does not know.
        _ => ("AUTH_FAILED", "Authentication failed"),
    };
    strings.text(key, fallback)
}

/// Cut the **dead web address** off the tail of an authored failure string (director's call,
/// 2026-08-28).
///
/// Every long `LOGIN_*`/`AUTH_*` string ends by pointing the player at a Blizzard page —
/// `www.worldofwarcraft.com`, `worldofwarcraft.com/misc/banned.html`. None has served anything in
/// fifteen years, and none was ever about *this* server: benilla's player is on somebody's private
/// realm, where "see www.worldofwarcraft.com for more information" is not merely stale but
/// misdirection. The rest of the sentence is still worth reading, so the address is cut at its
/// **clause**, not its sentence — `"…a lost or stolen password and account, see
/// www.worldofwarcraft.com for more information."` becomes `"…a lost or stolen password and
/// account."`
///
/// Find the first web address, walk back to the clause boundary before it (a comma or a full
/// stop), end there. **With no boundary to walk back to the string is left exactly as it is** —
/// that guard is what stops `AUTH_BANNED` ("…violating the Terms of Use Agreement -
/// www.worldofwarcraft.com/termsofuse.shtml…"), whose address sits behind a dash with no clause
/// break before it, from being trimmed away to nothing.
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

/// The **realmd** logon-proof refusal, in the client's own words — the LONG `LOGIN_*` family.
///
/// **This table was wrong in every row until decision 1679**, and wrong in an instructive way: the
/// codes were right and the *string family* was not. Decision 0539 §6 built it by reading vmangos'
/// `AuthCodes.h`, which names the wire bytes but says nothing about what the client displays, and
/// `GlueStrings.lua` happens to define two plausible families whose keys read alike. So a bad
/// password showed the terse `AUTH_UNKNOWN_ACCOUNT` ("Unknown account") — a real reference string,
/// in the wrong slot.
///
/// The client keeps **two** login-status enums with **two** key tables, and they are conflated
/// precisely because both raise the dialog through `OPEN_STATUS_DIALOG`. realmd's results resolve
/// against table `0x836b78` — the long `LOGIN_*` family; the world server's resolve against
/// `0x85cae8` — the short `AUTH_*` family ([`world_refusal_text`]). The chain is grunt opcode table
/// `0x85e278` → `Logon::OnAuthResult 0x5b2c90` (byte-index table `0x5b2ea4` + jump table
/// `0x5b2e78`) → `CGlueMgr::OnLoginState 0x46b0f0` → `CGlueMgr::UpdateGlueDialog 0x46b140`.
///
/// Two consequences worth stating outright:
///
/// **0x04 and 0x05 share one jump-table arm** — byte-identical, both `LOGIN_UNKNOWN_ACCOUNT`. The
/// reference client structurally *cannot* say "wrong password" on a logon refusal, and
/// `LOGIN_INCORRECT_PASSWORD` is dead surface it never reaches. That also retires the emulator lore
/// that the client locks out after 0x05 (vmangos sends 0x04 for both to avoid it): there is no
/// lockout — no disabled button, no delay, nothing persisted. Its one counter is gated on a
/// `securityFlags` byte that is zero on vmangos and cmangos, and only escalates the same string
/// toward its `_PIN`/`_CALL` variants.
///
/// **Anything past 0x0F is clamped**, not passed through: the byte-index table saturates at 0xFF,
/// which lands on the arm that shows `DISCONNECTED`. So 0x10/0x11/0x12 and every unknown future
/// code read as a disconnect rather than as an authentication message.
fn logon_refusal_text(strings: &GlueStrings, code: Option<u8>) -> &str {
    let (key, fallback): (&str, &str) = match code {
        // A transport failure never had a code; the client's own answer for the codes that mean
        // "no usable connection" is the same string, so they share this row.
        // 0x01/0x02 (unknown0/1), 0x0B (invalid server), 0x0D (no access).
        None | Some(0x01) | Some(0x02) | Some(0x0B) | Some(0x0D) => {
            ("LOGIN_FAILED", "Unable to connect")
        }
        Some(0x03) => (
            "LOGIN_BANNED",
            "This World of Warcraft account has been closed and is no longer available for use.  \
             Please go to http://www.worldofwarcraft.com/misc/banned.html for further information. ",
        ),
        // BOTH the unknown account and the wrong password — see the doc above.
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
        // 0x0A (version update) is IGNORED on the proof (`0x5bada2`) and never reaches a dialog;
        // it only means anything on the challenge, where it is the patch-download state. Every
        // remaining code clamps to the disconnect arm.
        Some(_) => ("DISCONNECTED", "Disconnected from server"),
    };
    strings.text(key, fallback)
}

/// The policy tick + the net-message reactions. Runs in every state (the reconnect path fires
/// while `InWorld`); the screen's own submit comes through [`login_input`], which calls
/// [`send_login`] with `announced = true`.
fn drive_policy(
    mut attempt: Attempt,
    realm_list_up: Res<crate::realm_select::Realms>,
    mut dialog: ResMut<GlueDialog>,
    strings: Option<Res<GlueStrings>>,
    time: Res<Time>,
    mut stages: MessageReader<LoginStageMessage>,
    mut queued: MessageReader<LoginQueuedMessage>,
    mut failures: MessageReader<LoginFailedMessage>,
    mut disconnects: MessageReader<DisconnectedMessage>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = time.elapsed_secs();
    // A harness run (env creds, and not the smoke — the smoke owns its own verdict) may have
    // nobody at the keyboard: a login failure no resubmit can change would leave it parked on a
    // dialog for its whole wall-clock, and every retry a runner grants it is spent the same way.
    // Those failures exit non-zero instead, on one greppable marker — "login: FATAL" — that
    // a leg runner keys on (decision 1371).
    //
    // **Nobody is here only if the run says so** (decision 1769). Whether the client may end the
    // run itself is [`crate::run_mode::fatal_when_driverless`]'s to answer; the two facts this
    // scope adds are local ones. The smoke owns its own verdict, so it is never ours to end.
    let smoke = std::env::var_os("WOW_LOGIN_SMOKE").is_some();
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);

    // The env fast path, once (decision 0539 §3): WOW_USER and WOW_PASS both set → auto-submit
    // them, so every probe/smoke/harness invocation keeps working. There is no default account.
    // The login smoke drives its own credentials instead.
    if !attempt.intent.env_read {
        attempt.intent.env_read = true;
        // Purely "are the credentials in the environment?" (decision 1769) — whether anybody is
        // here to *react* is a different fact with a different home, `run_mode::unattended`.
        if crate::run_mode::env_login() && std::env::var_os("WOW_LOGIN_SMOKE").is_none() {
            let user = std::env::var("WOW_USER").unwrap_or_default();
            let pass = std::env::var("WOW_PASS").unwrap_or_default();
            // The account guard (decision 0649): a vmangos login KICKS whoever holds the account,
            // so an unattended run authenticates as the account its checkout declares
            // (`.probe-identity`) and nothing else. Only the *automated* path is gated — a typed
            // login is the player's own and is never second-guessed.
            match crate::run_mode::account_guard(&user) {
                Ok(()) => {
                    info!("login: env fast path — auto-submitting as {user}");
                    attempt.intent.creds = Some((user, pass));
                    attempt.intent.retry_at = Some(now);
                }
                Err(why) if std::env::var_os("WOW_ALLOW_ACCOUNT").is_some() => {
                    warn!("login: {why} — WOW_ALLOW_ACCOUNT is set, going ahead anyway");
                    attempt.intent.creds = Some((user, pass));
                    attempt.intent.retry_at = Some(now);
                }
                Err(why) => {
                    error!("login: REFUSING the env fast path — {why} Set WOW_ALLOW_ACCOUNT=1 if the cross-account login is deliberate.");
                    dialog.open_error(&why);
                    // The refusal is deterministic — the slot is baked into the binary — so the
                    // run can never get past this screen (the 1371 legs burned 3 × timeout on it).
                    if crate::run_mode::fatal_when_driverless(
                        "the account guard refused the only credentials this run has",
                    ) {
                        exit.write(AppExit::error());
                    }
                }
            }
        }
    }

    for msg in stages.read() {
        if matches!(dialog.kind, Some(DialogKind::Status)) {
            dialog.set_text(stage_text(strings, msg.stage));
        }
    }
    // The realm list answers the same question the status dialog is asking ("what are we
    // connecting to?"), and the reference does not stack them: reaching the realm list means the
    // login state moved past `LOGIN_STATE_CONNECTING`, and `CGlueMgr::UpdateGlueDialog` clears
    // the dialog when it does. Ours would otherwise sit behind the list saying "Connecting".
    if realm_list_up.shown && matches!(dialog.kind, Some(DialogKind::Status)) {
        dialog.close();
    }
    // **The queue** (decision 1681): each packet is one sample, and the first one turns the
    // connecting dialog into the queue dialog. A queue is not a failure — the attempt is still in
    // flight and `in_flight` deliberately stays set, so nothing resubmits underneath it.
    for msg in queued.read() {
        if !matches!(dialog.kind, Some(DialogKind::Queued)) {
            dialog.open_queued(msg.realm.clone());
        }
        if let Some(position) = msg.position {
            dialog.queue.sample(position, queue_now_ms(&time));
        }
        info!(
            "login: queued for {} at position {}",
            msg.realm.as_deref().unwrap_or("the realm"),
            msg.position
                .map_or_else(|| "unknown".to_string(), |p| p.to_string()),
        );
    }
    // The countdown is live between packets, so the text is recomputed every frame it is up.
    if matches!(dialog.kind, Some(DialogKind::Queued)) {
        let text = dialog.queue.text(
            queue_now_ms(&time),
            dialog.queue_realm.clone().as_deref(),
            strings,
        );
        dialog.set_text(&text);
    }
    for msg in failures.read() {
        attempt.intent.in_flight = false;
        attempt.intent.park = IoPark::AtLogin;
        // A terminal failure names something no resubmit can change (the server requires Warden,
        // say) — show the server's own words and drop the credentials so nothing retries.
        // **Did a person submit the attempt that just failed?** `announced` is set by the login
        // screen's own submit and by nothing else — the env fast path and every paced resubmit
        // leave it false — so it is direct evidence rather than an inference about the room, and
        // `LoginIntent::clear` below deliberately does not reset it. A typed attempt is attended
        // by definition, and an attended failure gets the reference's answer: the dialog, and
        // another go. Only an attempt nobody typed may kill the process.
        let typed = attempt.intent.announced;
        let may_end_the_run = !typed && !smoke;
        if msg.terminal {
            warn!("login: {}", msg.reason);
            attempt.intent.clear();
            dialog.open_error(&msg.reason);
            if may_end_the_run
                && crate::run_mode::fatal_when_driverless(&format!(
                    "terminal login failure with nobody at the keyboard ({})",
                    msg.reason
                ))
            {
                exit.write(AppExit::error());
            }
            continue;
        }
        match msg.refusal {
            Some(refusal) => {
                // A refusal — from EITHER server: surface it (even on the silent path, since the
                // credentials went stale) and never auto-retry against it.
                let byte = refusal.byte();
                warn!("login: refused ({refusal:?}, {byte:#04x}) — {}", msg.reason);
                attempt.intent.clear();
                dialog.open_error(&fail_text(strings, Some(refusal), None));
                if may_end_the_run
                    && crate::run_mode::fatal_when_driverless(&format!(
                        "refused ({refusal:?}, {byte:#04x}) and no resubmit can change it"
                    ))
                {
                    exit.write(AppExit::error());
                }
            }
            None if attempt.intent.announced => {
                warn!("login: {}", msg.reason);
                dialog.open_error(&fail_text(strings, None, msg.dial.as_ref()));
            }
            None => {
                // Silent transport failure with pending intent: schedule the paced resubmit.
                debug!("login: transport failure ({}) — retrying", msg.reason);
                if attempt.intent.creds.is_some() {
                    attempt.intent.retry_at = Some(now + RETRY_DELAY_SECS);
                }
            }
        }
    }
    for msg in disconnects.read() {
        // The IO thread is heading back to its pre-logon park.
        attempt.intent.park = IoPark::AtLogin;
        attempt.intent.in_flight = false;
        if msg.session_over {
            // The reference's `DISCONNECTED_FROM_SERVER` (decision 1262): `GlueParent.lua` answers
            // it with `SetGlueScreen("login")` + `GlueDialog_Show("DISCONNECTED")` — the account
            // screen and one Okay button. Nothing retries, and the credentials go with the session:
            // a client that re-authenticates on its own steals the account back from whoever just
            // displaced it, which is the ping-pong the report described.
            warn!(
                "login: {} — session over, back to the login screen",
                msg.reason
            );
            attempt.intent.clear();
            dialog.open_error(strings.text("DISCONNECTED", "Disconnected from server"));
            continue;
        }
        // Otherwise the session continues through the park and the re-auth is silent: immediate
        // after a clean logout (the roster IS the select screen the app now shows), paced after a
        // stream death an unattended run must recover from on its own (0065, paced app-side).
        if attempt.intent.creds.is_some() {
            let delay = if msg.end == benilla_protocol::SessionEnd::LoggedOut {
                0.0
            } else {
                RETRY_DELAY_SECS
            };
            attempt.intent.retry_at = Some(now + delay);
        }
    }

    // The silent (re)submit tick.
    if attempt.intent.park == IoPark::AtLogin
        && !attempt.intent.in_flight
        && attempt.intent.retry_at.is_some_and(|t| now >= t)
    {
        if let Some((user, pass)) = attempt.intent.creds.clone() {
            attempt.send(&user, &pass, false);
        } else {
            attempt.intent.retry_at = None;
        }
    }
}

/// The roster's arrival is the login flow's success edge: the attempt settled, the IO thread is
/// parked at select — leave the login screen for CharSelect (only from `Login`; a reconnect's
/// roster lands while `InWorld` and must not flip the screen).
fn to_select_on_roster(
    mut msgs: MessageReader<CharListMessage>,
    mut intent: ResMut<LoginIntent>,
    mut dialog: ResMut<GlueDialog>,
    state: Res<State<ClientState>>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_none() {
        return;
    }
    intent.in_flight = false;
    intent.park = IoPark::Active;
    intent.retry_at = None;
    // **Only the dialogs the roster ANSWERS.** This used to close whatever was up, which was
    // right while the only dialog that could be up here was this screen's own "Connecting…". It
    // stopped being right when a refused *character* login started raising an `Error` on the
    // select screen: the refusal's relist produces a roster a second later, and closing on it
    // would take the message off the screen before the player had read it — the same silent
    // refusal, one layer up. An `Error` is dismissed by its Okay, never by an arriving packet.
    if matches!(dialog.kind, Some(DialogKind::Status | DialogKind::Queued)) {
        dialog.close();
    }
    if *state.get() == ClientState::Login {
        next.set(ClientState::CharSelect);
    }
}

// ── The screen's form state + input ──────────────────────────────────────────────────────────────

/// Which edit box has the focus.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    #[default]
    Account,
    Password,
}

/// The typed form: both boxes, the focus, and the Remember checkbox. Each box is a real
/// [`EditBoxState`] — the same byte-verified model the chat box uses — so the login fields get
/// caret movement, selection, Ctrl+A and the clipboard from the shared law rather than the
/// three-case imitation they used to carry (decision 0704). The caret clock lives in the box too
/// (`blink_accum`/`caret_shown`), so it blinks on the client's own 0.5 s period.
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
            // `password` masks the *display* only; the real text is never rendered or copied.
            password: textinput::field(MAX_LETTERS, true),
            focus: Field::default(),
            save: false,
        }
    }
}

impl LoginForm {
    /// Give `field` the keyboard **and select everything already in it** — dropping the selection
    /// on the box being left.
    ///
    /// **A knowing divergence** (director's call, 2026-08-28), and the reference half of it is
    /// byte-settled rather than inferred. The reference does the *opposite* on a click:
    /// `OnMouseDown` (`0x77b800`) hit-tests the click to a byte index, **collapses** the selection
    /// onto it (`0x77b86f call 0x77ccf0`) and only then calls `SetFocus` — so a fresh click-focus
    /// leaves an EMPTY selection at the character you clicked. And `SetFocus` itself writes no
    /// selection field at all; `0x77e3f6` being the only instruction image-wide that grants focus
    /// makes *every* focus gain selection-neutral, TAB included. Losing focus likewise touches
    /// nothing (`0x77af50` raises only the cursor dirty bit).
    ///
    /// So we diverge in both directions, deliberately: the reference collapses where we select, and
    /// leaves stale where we collapse. The reason is the same one for both — the thing a player does
    /// after clicking into a login field is *replace* what is there, a remembered account name or a
    /// mistyped password, and having to select-all or backspace it out first is friction the
    /// reference only avoids by being what everyone was used to in 2006.
    ///
    /// **A divergence owes both halves.** 1682 shipped only the select; nothing ever unselected,
    /// so the box you left kept a grey highlight behind you and the screen showed two selections at
    /// once (the director's report, 2026-08-29). Selecting on focus is only coherent if the
    /// selection *means* "this is the box the keyboard is in" — which makes collapsing the outgoing
    /// box not a second feature but the other half of this one. It runs the client's own
    /// collapse-to-cursor (`0x77ccf0`), the same primitive every delete path uses.
    ///
    /// The selection uses the client's own `HighlightText(0, -1)` (`0x77cca0`), which resets the
    /// blink on its way, so the box still opens on a solid caret — the property the old
    /// `reset_blink()` calls at these sites existed to preserve.
    fn focus(&mut self, field: Field) {
        self.focused().collapse();
        self.focus = field;
        self.focused().highlight_text(0, -1);
    }

    /// The box that currently owns the keyboard.
    fn focused(&mut self) -> &mut EditBoxState {
        match self.focus {
            Field::Account => &mut self.account,
            Field::Password => &mut self.password,
        }
    }
}

/// The armed quit (`gsTitleQuit` needs [`QUIT_GRACE_SECS`] to be audible before `AppExit`).
#[derive(Resource, Default)]
struct QuitArm(Option<f32>);

/// Entering the login screen: the ref's `AccountLogin_OnShow` — prefill the saved account name,
/// clear the password, focus account when empty / password otherwise, checkbox = saved-name
/// exists — and stand the `UI_MainMenu` scene up.
fn enter_login(mut form: ResMut<LoginForm>, mut preview: ResMut<GluePreview>) {
    let saved = load_saved_account();
    form.save = !saved.is_empty();
    form.focus = if saved.is_empty() {
        Field::Account
    } else {
        Field::Password
    };
    form.account.set_text(&saved);
    form.password.set_text("");
    // `SetFocus` starts the caret solid — the screen never opens mid-blink-off (`set_text` alone
    // wouldn't do it: it no-ops when the saved name is already in the box) — and selects, so a
    // remembered account name is typed over rather than appended to.
    let focus = form.focus;
    form.focus(focus);
    preview.scene = Some(GlueScene::MainMenu);
    preview.look = None;
    preview.yaw = 0.0;
}

/// The screen's input: typing into the focused box (the ref's 16-letter cap), Tab cycling, Enter
/// submits, Esc quits (dialog-first — an open dialog's Esc is its Cancel/Okay), clicks focus the
/// boxes / press the buttons / toggle the checkbox.
#[allow(clippy::type_complexity)]
fn login_input(
    realms: Res<crate::realm_select::Realms>,
    presses: Query<(Entity, &LoginAction, Ref<Interaction>)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut keyboard: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    // The host pasteboard + the window handle the Wayland backend needs (decision 0702).
    mut clipboard: NonSendMut<HostClipboard>,
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
    // The realm list stands **over** this screen rather than replacing it (the reference's
    // `RealmList` is a DIALOG-strata frame, not a glue screen), so while it is up the boxes,
    // buttons and keys underneath are inert — including ESCAPE, which is its Cancel, not our Quit.
    if realms.shown {
        return;
    }
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);

    let mut do_login = false;
    let mut do_quit = false;

    // While a dialog is up it owns the input; the box/button surface underneath is inert.
    let dialog_open = dialog.kind.is_some();

    // **The edit boxes focus on the PRESS**, and only they. The reference's `CEditBox` takes focus
    // from its own OnMouseDown handler (`0x77b800`), unconditionally and autoFocus-independent —
    // an edit box is not a Button and does not wait for the release. Every *button* on this
    // screen fires from the release loop below (1533).
    for (entity, action, interaction) in &presses {
        if dialog_open {
            continue;
        }
        // **The edit boxes focus on the PRESS**, and only they: the reference's `CEditBox` takes
        // focus from its own OnMouseDown handler (`0x77b800`), unconditionally and
        // autoFocus-independent — an edit box is not a Button and does not wait for the release.
        // `Ref` supplies the press *edge* the old `Changed<Interaction>` filter gave, without
        // costing this system a second query.
        if interaction.is_changed() && *interaction == Interaction::Pressed {
            match action {
                // A click takes the focus the same way TAB does — the solid caret, and the
                // select-all that makes the next keystroke replace what is in the box.
                LoginAction::FocusAccount => form.focus(Field::Account),
                LoginAction::FocusPassword => form.focus(Field::Password),
                _ => {}
            }
        }
        // Everything else on this screen is a Button, and a Button fires on the RELEASE (1533).
        if !clicks.hit(entity) {
            continue;
        }
        match action {
            LoginAction::FocusAccount | LoginAction::FocusPassword => {} // focused on the press
            LoginAction::Login => do_login = true,
            LoginAction::Quit => do_quit = true,
            // The realmlist control (1667) — the button and the address readout under it are
            // the same action, so clicking either opens the editor.
            LoginAction::Realmlist => {
                // `gsLoginNewAccount` — what the reference plays for the other buttons in this
                // corner (`AccountLogin_ManageAccount`, `AccountLogin_LaunchCommunitySite`), which
                // is the closest authored answer for a button it does not have. It replaces a
                // `gsClick` I invented in 1667: no such SoundEntries kit exists, so that press was
                // silently playing nothing at all.
                sounds.write(GlueSound("gsLoginNewAccount"));
                if attempt.realmlist.pinned_by_env() {
                    // A harness/dev run owns the address for the session (`cvars`' env-override
                    // law). Say so rather than opening an editor whose Okay would be a silent
                    // no-op — the trap that shape would set.
                    dialog.open_error(&format!(
                        "$WOW_HOST is set for this session, so the realmlist is fixed at {}.",
                        attempt.realmlist.address(),
                    ));
                } else {
                    dialog.open_realmlist(
                        // The reference's own registered help text for `realmList`, byte-verified
                        // in `WoW.exe` beside the CVar's name and default.
                        "Address of realm list server",
                        attempt.realmlist.address(),
                    );
                }
            }
            LoginAction::ToggleSave => {
                form.save = !form.save;
                // Verbatim ref quirk (`AccountLoginSaveAccountName` OnClick): checked plays the
                // "Off" kit, unchecked the "On" kit.
                sounds.write(GlueSound(if form.save {
                    "igMainMenuOptionCheckBoxOff"
                } else {
                    "igMainMenuOptionCheckBoxOn"
                }));
                if !form.save {
                    save_account("");
                }
            } // The dialog's own buttons are `crate::glue::dialog`'s, and this loop is
              // skipped entirely while one is open.
        }
    }

    let mods = textinput::mods_now(&keys);
    let wl = textinput::wayland_display(raw_handle.iter().next());
    for ev in keyboard.read() {
        // A dialog with an edit box owns the keyboard while it is up — the ref's `GlueDialog` is
        // `toplevel` with `enableKeyboard="true"`, so the boxes behind it hear nothing. ENTER and
        // ESCAPE still come back unclaimed; [`crate::glue::dialog::drive_glue_dialog`] reads
        // them as its two buttons.
        if dialog_open {
            if dialog.kind.is_some_and(DialogKind::has_edit_box) {
                textinput::feed_key(
                    &mut dialog.edit,
                    ev,
                    mods,
                    &mut clipboard,
                    wl,
                    textinput::CharFilter::Any,
                );
            }
            continue;
        }
        // The shared law first (editing, caret, selection, the clipboard trio); only what it
        // hands back unclaimed is the screen's own — TAB cycles the two boxes, ENTER/ESCAPE are
        // handled below off `just_pressed`.
        if textinput::feed_key(
            form.focused(),
            ev,
            mods,
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

    if do_login && !attempt.intent.in_flight {
        // **`gsLogin` first, always** — `AccountLogin_Login` plays it before it calls
        // `DefaultServerLogin`, so the reference clicks even when the attempt is about to be
        // refused for an empty box. Ours played it only on the path that reached the wire, so a
        // Login press with a field still blank was silent and the dialog appeared out of nowhere
        // (the director's "the sound of the popup is sometimes missing"). It sits OUTSIDE the
        // match on purpose: a sound that is structurally unconditional cannot go missing on some
        // branch again.
        sounds.write(GlueSound("gsLogin"));
        match login_press(&form) {
            // The ref's own guards: empty account / empty password get their dialog, no wire.
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
                // `AccountLogin_Login`: save or clear the account name per the checkbox, clear the
                // password box after grabbing it.
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
        commands.insert_resource(QuitArm(Some(time.elapsed_secs() + QUIT_GRACE_SECS)));
    }
}

/// The caret's clock. Exactly one box owns the keyboard, so the focused one is the one that blinks
/// — on the shared law's 0.5 s period, so the login caret keeps time with the create-name box, the
/// delete dialog and the chat box (decision 0704). It keeps blinking under an open dialog: a dialog
/// eats the keys, not the clock.
///
/// Its own system, not a line inside [`login_input`]: a blink is a clock, not input, and as a
/// system it can be run on its own in a test — which is the only thing that can catch this tick
/// going missing again. It went missing once already, and nothing but an eye noticed: the box's
/// `caret_shown` simply never left its `true` default, so the login caret was the one glue caret
/// that sat solid.
///
/// **A dialog with an edit box takes the focus with the keys** (1667): its box blinks and the
/// form's stops, so the screen never shows two live carets at once. Every other dialog leaves the
/// form's caret running — a dialog eats the keys, not the clock.
fn tick_login_caret(mut form: ResMut<LoginForm>, mut dialog: ResMut<GlueDialog>, time: Res<Time>) {
    let dt = time.delta_secs();
    let in_dialog = dialog.kind.is_some_and(DialogKind::has_edit_box);
    textinput::tick_caret(form.focused(), !in_dialog, dt);
    if in_dialog {
        textinput::tick_caret(&mut dialog.edit, true, dt);
    }
}

/// Editing the account box away from the saved name clears the save + unchecks (the ref's
/// `OnTextChanged`).
fn on_account_edited(form: &mut LoginForm) {
    if form.save {
        let saved = load_saved_account();
        if !saved.is_empty() && saved != form.account.text {
            save_account("");
            form.save = false;
        }
    }
}

/// Fire the armed quit once its grace elapsed (so `gsTitleQuit` is heard).
fn drive_quit(arm: Option<Res<QuitArm>>, time: Res<Time>, mut exit: MessageWriter<AppExit>) {
    if let Some(arm) = arm {
        if arm.0.is_some_and(|t| time.elapsed_secs() >= t) {
            exit.write(AppExit::Success);
        }
    }
}

// ── The GlueDialog (connecting / error) ──────────────────────────────────────────────────────────

/// What the realmlist dialog says when the box holds something that is not an address. It replaces
/// the prompt in place and **leaves the typed text alone**, so the fix is an edit rather than a
/// retype — the reason a bad value does not close the dialog or become an error dialog of its own.
const REALMLIST_BAD: &str =
    "That is not a server address.\nTry  logon.example.org  or  127.0.0.1:3724";

/// **What a glue-dialog press means, on this screen.** The widget itself
/// ([`crate::glue::dialog::drive_glue_dialog`]) owns the tree, the keys and the click sound, and
/// ends an `Error` on its own; everything below is behaviour only the login screen can define, and
/// none of it is reachable from any other screen — `Status`, `Queued` and `Realmlist` can only be
/// *opened* from here.
fn answer_dialog(
    mut commands: Commands,
    mut answers: MessageReader<crate::glue::dialog::GlueDialogAnswer>,
    mut dialog: ResMut<GlueDialog>,
    mut intent: ResMut<LoginIntent>,
    mut realmlist: ResMut<crate::realmlist::Realmlist>,
    mut script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    abandon: Res<LoginAbandon>,
) {
    for answer in answers.read() {
        match answer.kind {
            // The widget's own — it has already dismissed it.
            DialogKind::Error => {}
            DialogKind::Realmlist => {
                // Okay: take the typed address, or say why it cannot be taken and stay open with
                // the text as typed — the fix is then an edit, not a retype. **The only press
                // that does not end its dialog**, which is exactly why the widget cannot own
                // this arm. Cancel (button 2, or ESCAPE, which this kind routes to it) closes.
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
            DialogKind::Status | DialogKind::Queued => {
                // Cancel: the next stage boundary discards the attempt; a canceled manual attempt
                // must not silently resubmit later.
                abandon.0.fetch_add(1, Ordering::SeqCst);
                intent.in_flight = false;
                intent.clear();
                dialog.dismiss(&mut commands);
            }
        }
    }
}

/// Take the realmlist dialog's Okay: normalize what was typed, point the session at it, and mirror
/// it into the `realmList` CVar so it is still there next launch. `false` = the box holds nothing
/// usable and the caller should keep the dialog open.
///
/// The persistence leg is `char_select`'s `lastCharacterIndex` pattern exactly (1131/1293): an
/// **engine-side** write rides the change queue like a Lua `SetCVar`, so `cvars::sync_cvars` folds
/// it into the knob and marks the file dirty, and `save_config` writes `config.toml`. A write to a
/// name the VM has not registered yet is a deliberate silent no-op there, so this reports that
/// case rather than claiming a save that did not happen — the **session** value stands either way,
/// which is the half that matters for the login about to be attempted.
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

// ── The saved account name (decision 0539 §4) ────────────────────────────────────────────────────

/// Read the saved account name from `base` (missing file/dir = empty). Takes the *file* rather
/// than resolving one, so the round-trip is testable from a tempdir.
fn load_saved_account_from(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Write (or, for an empty name, remove) the saved account name at `path`.
fn save_account_to(path: &std::path::Path, name: &str) {
    if name.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    if let Err(e) = std::fs::write(path, name) {
        warn!("login: saving account name failed: {e}");
    }
}

/// The ref's `GetSavedAccountName`. The path is [`crate::local_state`]'s — this module computed its
/// own until decision 1181, which is how the account name ended up in a different folder from every
/// other setting, and how a capture came to read one off the host machine.
fn load_saved_account() -> String {
    crate::local_state::saved_account_path()
        .map(|p| load_saved_account_from(&p))
        .unwrap_or_default()
}

/// The ref's `SetSavedAccountName` (empty clears).
fn save_account(name: &str) {
    if let Some(path) = crate::local_state::saved_account_path() {
        save_account_to(&path, name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand `drive_policy` up on its own with the env fast path already spent, so the policy
    /// under test is the disconnect arm and nothing else — and so an ambient `WOW_USER` in
    /// whatever shell runs the suite cannot seed credentials behind the assertions.
    fn policy_app() -> (App, crossbeam_channel::Receiver<LoginRequest>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<LoginIntent>()
            // The dialog is `GluePlugin`'s resource now (2084), so a harness that stands this
            // screen's systems up without that plugin has to seat it.
            .init_resource::<GlueDialog>()
            // The policy asks whether the realm list is standing over this screen; in a harness
            // that never raises one the answer is a default `Realms` — always "no".
            .init_resource::<crate::realm_select::Realms>()
            // Literal, not Default: `Realmlist::default()` reads `$WOW_HOST`, and every probe
            // recipe in this repo exports it — a suite run from such a shell would otherwise
            // assert against whatever that shell happened to be pointing at.
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                crate::realmlist::DEFAULT_REALMLIST,
            ))
            .insert_resource(LoginSubmit(tx))
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(0),
            )))
            .add_message::<LoginStageMessage>()
            .add_message::<LoginQueuedMessage>()
            .add_message::<LoginFailedMessage>()
            .add_message::<DisconnectedMessage>()
            .add_systems(Update, drive_policy);
        app.world_mut().resource_mut::<LoginIntent>().env_read = true;
        // The receiver is RETURNED rather than leaked: a dropped one turns every submit into an
        // `Err` and hides a policy that sent one, and holding it is also what lets a test read
        // back the request that went out.
        (app, rx)
    }

    /// **A lost session does not log itself back in** (decision 1262).
    ///
    /// This is the whole of the displacement report: log into the same account from the reference
    /// client, vmangos kicks us with a bare socket close, and 0065's paced resubmit — which cannot
    /// see *why* the socket died, because nothing on the wire says — re-authenticated three seconds
    /// later and kicked the client that had just displaced us. The account ping-ponged. The
    /// reference's `GlueParent.lua` answers `DISCONNECTED_FROM_SERVER` with the login screen and a
    /// one-button dialog, and retries nothing.
    #[test]
    fn a_lost_session_clears_the_credentials_and_shows_the_dialog() {
        let (mut app, _requests) = policy_app();
        app.world_mut().resource_mut::<LoginIntent>().creds = Some(("one".into(), "pone".into()));
        app.world_mut().write_message(DisconnectedMessage {
            reason: "disconnected: world stream closed: failed to fill whole buffer".into(),
            end: benilla_protocol::SessionEnd::Lost,
            session_over: true,
        });
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(
            intent.creds.is_none(),
            "the session's credentials die with the session — keeping them is what won the \
             account back off the client that displaced us",
        );
        assert!(intent.retry_at.is_none(), "and nothing is scheduled");
        let dialog = app.world().resource::<GlueDialog>();
        assert_eq!(dialog.kind, Some(DialogKind::Error));
        // The fallback literal: this App has no GlueStrings, and the table's own row is
        // `DISCONNECTED = "Disconnected from server";` (GlueStrings.lua) — the same words.
        assert_eq!(dialog.text, "Disconnected from server");
    }

    /// A **clean logout's** teardown rides the same message and must keep its silent relist: the
    /// IO thread returns to the pre-logon park, and the roster it comes back with IS the character
    /// select the player asked for. Breaking this would strand `/logout` on the login screen.
    #[test]
    fn a_logout_teardown_still_relists_at_once() {
        let (mut app, _requests) = policy_app();
        app.world_mut().resource_mut::<LoginIntent>().creds = Some(("one".into(), "pone".into()));
        app.world_mut().write_message(DisconnectedMessage {
            reason: "logged out".into(),
            end: benilla_protocol::SessionEnd::LoggedOut,
            session_over: false,
        });
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(
            intent.creds.is_some(),
            "a logout keeps the account signed in"
        );
        assert!(
            intent.in_flight,
            "and the same tick resubmits — the delay for a logout is 0, so the roster comes \
             straight back",
        );
        assert!(
            app.world().resource::<GlueDialog>().kind.is_none(),
            "with no dialog: nothing went wrong",
        );
    }

    /// **A roster does not take a refusal off the screen.**
    ///
    /// [`to_select_on_roster`] closes the dialog because a roster is the answer to this screen's
    /// "Connecting…" — but since a refused *character* login raises an `Error` on the SELECT
    /// screen, and its own relist produces a roster a second later, an unscoped close would wipe
    /// the message before it could be read. That is the original bug (a refusal nobody sees) one
    /// layer up, and nothing else would catch it: the build is green either way and the window is
    /// a second long.
    #[test]
    fn an_arriving_roster_closes_the_connecting_dialog_but_not_an_error() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<LoginIntent>()
            .init_resource::<GlueDialog>()
            .add_message::<CharListMessage>()
            .add_systems(Update, to_select_on_roster);

        // The refusal's dialog survives its own relist.
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_error("World server is down");
        app.world_mut().write_message(CharListMessage {
            characters: Vec::new(),
            realm: None,
        });
        app.update();
        let dialog = app.world().resource::<GlueDialog>();
        assert_eq!(dialog.kind, Some(DialogKind::Error));
        assert_eq!(dialog.text, "World server is down");

        // …and the connecting dialog the roster IS the answer to still closes.
        app.world_mut()
            .resource_mut::<GlueDialog>()
            .open_status("Connecting");
        app.world_mut().write_message(CharListMessage {
            characters: Vec::new(),
            realm: None,
        });
        app.update();
        assert!(app.world().resource::<GlueDialog>().kind.is_none());
    }

    /// An **unattended** run keeps 0065's paced reconnect on a lost session — the verdict rides
    /// the message, so the policy honours it without re-reading the environment.
    #[test]
    fn an_unattended_run_still_reconnects_on_its_own() {
        let (mut app, _requests) = policy_app();
        app.world_mut().resource_mut::<LoginIntent>().creds =
            Some(("probe1".into(), "pprobe1".into()));
        app.world_mut().write_message(DisconnectedMessage {
            reason: "disconnected: connection reset".into(),
            end: benilla_protocol::SessionEnd::Lost,
            session_over: false,
        });
        app.update();

        let intent = app.world().resource::<LoginIntent>();
        assert!(intent.creds.is_some());
        assert_eq!(
            intent.retry_at,
            Some(RETRY_DELAY_SECS),
            "paced by the flat 3 s off a zeroed clock, not fired on the spot",
        );
        assert!(app.world().resource::<GlueDialog>().kind.is_none());
    }

    /// **The submitted attempt dials the configured realmlist** (decision 1667) — the whole
    /// point of the setting. Before this, the address was latched out of `$WOW_HOST` once at
    /// process start and the request had no say in it; now the request carries it, so a change
    /// made between attempts is the one the next attempt uses.
    #[test]
    fn a_submitted_attempt_carries_the_configured_realmlist() {
        let (mut app, requests) = policy_app();
        app.world_mut()
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                "logon.example.org:3725",
            ));
        // Credentials pending with the retry due: the policy's silent submit tick.
        {
            let mut intent = app.world_mut().resource_mut::<LoginIntent>();
            intent.creds = Some(("one".into(), "pone".into()));
            intent.retry_at = Some(0.0);
        }
        app.update();

        let sent = requests.try_recv().expect("the policy submitted");
        assert_eq!(sent.user, "one");
        assert_eq!(
            sent.host, "logon.example.org:3725",
            "the attempt dials what the realmlist says, not a value latched at spawn",
        );

        // And a change between attempts is picked up by the next one, with no relaunch.
        app.world_mut()
            .insert_resource(crate::realmlist::Realmlist::unpinned(
                "elsewhere.example.org",
            ));
        {
            let mut intent = app.world_mut().resource_mut::<LoginIntent>();
            intent.in_flight = false;
            intent.retry_at = Some(0.0);
        }
        app.update();
        assert_eq!(
            requests.try_recv().expect("resubmitted").host,
            "elsewhere.example.org",
        );
    }

    /// The dialog's Okay: what the box holds becomes the session's address, including the
    /// `realmlist.wtf` line a player pastes off a server's setup page. No VM here, so the
    /// persistence leg is the `None` arm — the session value is what this asserts, and it is the
    /// half the next login attempt reads.
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

    /// …and a box holding something that is not an address changes nothing and reports it, so the
    /// caller keeps the dialog open over the text as typed rather than closing on a silent no-op.
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

    /// **The seam the move opened** (2084): the widget publishes a press, this screen answers it.
    ///
    /// `accept_realmlist` and the cancel bookkeeping are each covered on their own above, and the
    /// widget's keys and kinds are covered in `crate::glue::dialog` — but nothing covered the
    /// *wiring* between them, which is the one thing the move could break silently. A dialog whose
    /// press reaches nobody looks exactly like a dialog that works until you click it.
    #[test]
    fn a_published_press_reaches_the_login_screens_answer() {
        use crate::glue::dialog::GlueDialogAnswer;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<LoginIntent>()
            .init_resource::<GlueDialog>()
            .insert_resource(crate::realmlist::Realmlist::unpinned("localhost"))
            .insert_resource(LoginAbandon(std::sync::Arc::new(
                std::sync::atomic::AtomicU64::new(0),
            )))
            .add_message::<GlueDialogAnswer>()
            .add_systems(Update, answer_dialog);

        // The status dialog's Cancel: the attempt is abandoned and the credentials forgotten, so
        // nothing resubmits behind the player's back.
        app.world_mut().resource_mut::<LoginIntent>().creds = Some(("one".into(), "pone".into()));
        app.world_mut().resource_mut::<LoginIntent>().in_flight = true;
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
            !intent.in_flight,
            "the cancelled attempt is no longer in flight"
        );
        assert!(intent.creds.is_none(), "and it does not silently resubmit");
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

        // …and its Okay over a bad address keeps the dialog up, with the text replaced — the one
        // press in the whole widget that does not end its own dialog.
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

    /// Opening the editor seats the current address in the box, selected whole — so typing a new
    /// server replaces the old one instead of appending to it.
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

    /// Did this update ask the app to quit?
    fn exited(app: &mut App, cursor: &mut bevy::ecs::message::MessageCursor<AppExit>) -> bool {
        let msgs = app.world().resource::<Messages<AppExit>>();
        cursor.read(msgs).next().is_some()
    }

    /// Drive one refusal through the policy and report whether it killed the process.
    fn refusal_exits(typed: bool) -> bool {
        let (mut app, _requests) = policy_app();
        let mut cursor = bevy::ecs::message::MessageCursor::<AppExit>::default();
        // Drain whatever the first update writes before the message under test.
        app.update();
        let _ = exited(&mut app, &mut cursor);
        app.world_mut().resource_mut::<LoginIntent>().announced = typed;
        app.world_mut().write_message(LoginFailedMessage {
            refusal: Some(LoginRefusal::Logon(0x05)),
            reason: "server rejected logon: result 0x05".into(),
            terminal: false,
            dial: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<GlueDialog>().kind,
            Some(DialogKind::Error),
            "every refusal shows the dialog, exit or not",
        );
        exited(&mut app, &mut cursor)
    }

    /// **A typo must not kill the client**, and **env credentials alone must not either.**
    ///
    /// The reported crash: `login: FATAL — refused (code 0x04) … exiting`, on a password typed at
    /// the screen, because `$WOW_CHAR` was set and the old predicate read any env credential as
    /// "a harness". Two answers, both here, because they close the hole from both ends: whether
    /// an *attempt* was typed is direct evidence (`announced`, set by the screen's own submit and
    /// by nothing else), and whether the *run* is driverless is a declaration the run makes
    /// (`WOW_UNATTENDED`, decision 1769) — never an inference off credentials the director's own
    /// launch line carries.
    #[test]
    fn a_typed_password_refusal_never_exits() {
        let _lock = crate::local_state::test_env::ENV_LOCK.lock();
        let _smoke = crate::local_state::test_env::EnvGuard::unset("WOW_LOGIN_SMOKE");

        // 1 · The director's actual launch line, verbatim from `.cargo/config.toml`'s example.
        //     Nothing here says anybody is absent, so nothing may end the run — typed or not.
        let _user = crate::local_state::test_env::EnvGuard::set("WOW_USER", "one");
        let _pass = crate::local_state::test_env::EnvGuard::set("WOW_PASS", "pone");
        let _char = crate::local_state::test_env::EnvGuard::set("WOW_CHAR", "One");
        let _decl = crate::local_state::test_env::EnvGuard::unset("WOW_UNATTENDED");
        let _cap = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _rig = crate::local_state::test_env::EnvGuard::unset("WOW_RIG");
        assert!(
            crate::run_mode::env_login() && !crate::run_mode::unattended(),
            "the fixture must be the env the director plays in: credentials, nobody declared away",
        );
        assert!(
            !refusal_exits(false),
            "an env-credentialled run with a person in it shows the dialog, it does not exit",
        );

        // 2 · A run that declares itself driverless keeps decision 1371's leg guarantee.
        let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
        assert!(
            refusal_exits(false),
            "an attempt nobody typed, in a run nobody is in, still exits non-zero",
        );
        assert!(
            !refusal_exits(true),
            "a password typed at the screen is attended by direct evidence — dialog, another go",
        );
    }

    /// **The dead web address comes off** (director's call) — at the clause, so the sentence in
    /// front of it survives.
    #[test]
    fn a_dead_url_is_cut_at_its_clause() {
        // The reported one, verbatim from `GlueStrings.lua`.
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
        // A whole trailing sentence goes when the address is what the sentence is for.
        assert_eq!(
            without_dead_url(
                "This World of Warcraft account has been closed and is no longer available for \
                 use.  Please go to http://www.worldofwarcraft.com/misc/banned.html for further \
                 information. ",
            ),
            "This World of Warcraft account has been closed and is no longer available for use.",
        );
        // Nothing to cut: left exactly alone, and borrowed rather than rebuilt.
        let clean = "You have used up your prepaid time for this account.";
        assert!(matches!(
            without_dead_url(clean),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(without_dead_url(clean), clean);
    }

    /// The guard that stops the trim eating a string whose address has no clause in front of it —
    /// `AUTH_BANNED`, where the URL sits behind a dash. Better untrimmed than truncated to nothing.
    #[test]
    fn an_address_with_no_clause_boundary_is_left_alone() {
        let banned = "This account has been banned for violating the Terms of Use Agreement - \
                      www.worldofwarcraft.com/termsofuse.shtml. Please contact our GM department.";
        assert_eq!(without_dead_url(banned), banned);
        // …and the degenerate case cannot produce an empty dialog.
        assert_eq!(without_dead_url("www.example.com"), "www.example.com");
        assert_eq!(without_dead_url(". www.example.com"), ". www.example.com");
    }

    /// The Login press's two guards, in the reference's order: a wholly empty form asks for the
    /// account name first.
    #[test]
    fn the_login_press_asks_for_the_account_before_the_password() {
        let mut form = LoginForm::default();
        assert_eq!(login_press(&form), LoginPress::NeedAccount);
        form.account.set_text("one");
        assert_eq!(login_press(&form), LoginPress::NeedPassword);
        form.password.set_text("pone");
        assert_eq!(login_press(&form), LoginPress::Submit);
        // And a password-only form still asks for the account, not the password.
        let mut only_pass = LoginForm::default();
        only_pass.password.set_text("pone");
        assert_eq!(login_press(&only_pass), LoginPress::NeedAccount);
    }

    /// **Focusing a box selects it** (director's call, diverging from the reference) — and leaves
    /// the caret solid, which is what the `reset_blink` calls this replaced were for.
    #[test]
    fn focusing_a_box_selects_all_of_it() {
        let mut form = LoginForm::default();
        form.account.set_text("remembered");
        form.password.set_text("secret");

        form.focus(Field::Account);
        assert_eq!(form.focus, Field::Account);
        assert_eq!(form.account.selected_text().as_deref(), Some("remembered"));
        assert!(form.account.caret_shown, "a fresh focus starts solid");

        // TAB to the other box selects that one whole too. Checked as a RANGE, not as text: a
        // password box's `selected_text` hands back the `*` mask, because the real characters are
        // never rendered or copied (decision 0704's box law) — which is the correct answer and
        // exactly why the assertion cannot ask for them.
        form.focus(Field::Password);
        assert_eq!(
            (form.password.sel_start, form.password.sel_end),
            (0, form.password.text.len()),
            "the whole password is selected even though it cannot be read back",
        );

        // **And the box we left is no longer selected.** The other half of the divergence: with
        // the selection standing for "the keyboard is here", exactly one box can carry one.
        assert_eq!(
            form.account.selected_text(),
            None,
            "the box that lost the keyboard keeps no highlight",
        );

        // Typing then replaces rather than appends — the point of the divergence.
        form.focused().insert("x");
        assert_eq!(form.password.text, "x");

        form.focus(Field::Account);
        assert_eq!(
            form.password.selected_text(),
            None,
            "and back the other way"
        );
    }

    /// The code→string map quotes the client's own strings for the vmangos-verified rows.
    /// The realmd map, against the client's table (`0x5b2c90` → key table `0x836b78`).
    ///
    /// Every row of this changed in decision 1679: the codes were always right and the string
    /// FAMILY was always wrong, so each of these used to answer with the terse `AUTH_*` twin of
    /// the string it now gives.
    #[test]
    fn fail_text_maps_the_verified_codes() {
        let strings = GlueStrings::default(); // empty table → the fallback literals
        let logon = |b| fail_text(&strings, Some(LoginRefusal::Logon(b)), None);

        // 0x04 and 0x05 share one jump-table arm — the client cannot say "wrong password", and
        // this is the string a player actually gets for a typo.
        assert!(logon(0x04).starts_with("The information you have entered is not valid."));
        assert_eq!(logon(0x05), logon(0x04), "one arm, byte-identical");
        assert!(logon(0x04).contains("account name and password"));

        // The four "no usable connection" codes share `LOGIN_FAILED` with a bare transport failure.
        assert_eq!(fail_text(&strings, None, None), "Unable to connect");
        for code in [0x01, 0x02, 0x0B, 0x0D] {
            assert_eq!(logon(code), "Unable to connect", "code {code:#04x}");
        }

        assert!(logon(0x03).contains("has been closed"));
        assert!(logon(0x06).contains("already logged into"));
        assert!(logon(0x09).contains("Unable to validate game version"));
        assert!(logon(0x0C).contains("temporarily suspended"));
        assert!(logon(0x0F).contains("parental controls"));

        // Past 0x0F the byte-index table saturates onto the disconnect arm — so an unknown code
        // reads as a disconnect, not as an authentication message.
        for code in [0x10, 0x11, 0x12, 0xEE] {
            assert_eq!(logon(code), "Disconnected from server", "code {code:#04x}");
        }
    }

    /// **The two result enums are different enums**, and the same byte must not mean the same
    /// thing in both. 0x0C is `AUTH_LOGON_FAILED_SUSPENDED` to realmd and `AUTH_OK` to the world
    /// server; 0x05 is a bad password to realmd and nothing at all to the world server. A single
    /// `Option<u8>` could not tell them apart, which is why `LoginRefusal` exists.
    #[test]
    fn the_two_auth_enums_do_not_share_a_byte() {
        use benilla_protocol::messages as m;
        let strings = GlueStrings::default();
        let logon = |b| fail_text(&strings, Some(LoginRefusal::Logon(b)), None);
        let world = |b| fail_text(&strings, Some(LoginRefusal::World(b)), None);

        // 0x0C: "suspended" to realmd, and to the world server it is AUTH_OK — success, which
        // never reaches a refusal string at all.
        assert!(logon(0x0C).contains("temporarily suspended"));
        assert_eq!(world(m::AUTH_BILLING_ERROR), "Billing system error");
        assert_ne!(logon(0x0C), world(0x0C));

        // Every world row is the client's own dispatch (`0x5aa960`), transcribed.
        assert_eq!(world(m::AUTH_INCORRECT_PASSWORD), "Incorrect Password");
        assert_eq!(world(m::AUTH_SESSION_EXPIRED), "Session Expired");
        assert_eq!(world(m::AUTH_SERVER_SHUTTING_DOWN), "Server Shutting Down");
        assert_eq!(world(m::AUTH_ALREADY_LOGGING_IN), "Already Logging In");
        assert_eq!(
            world(m::AUTH_UNAVAILABLE),
            "System unavailable - Please try again later"
        );
        // The five codes the reference's URL dialog keys on — reachable only from this enum.
        assert!(world(m::AUTH_BANNED).contains("banned"));
        assert!(world(m::AUTH_SUSPENDED).contains("temporarily suspended"));
        assert!(world(m::AUTH_PARENTAL_CONTROL).contains("parental controls"));
        // A code this client does not know still lands on the authored catch-all.
        assert_eq!(world(0xEE), "Authentication failed");
    }

    /// **The report that prompted this** — a dead local server read as a bad address, twice, because
    /// "Unable to connect" cannot tell the two apart. Each dial verdict now gets the reference's own
    /// string for that exact condition, with the address under it so there is nothing left to guess.
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
        // A refusal still wins its own authored string; the dial verdict only speaks for the dial.
        assert!(fail_text(&strings, Some(LoginRefusal::Logon(0x05)), None)
            .starts_with("The information you have entered is not valid."));
    }

    /// Save → load → clear round-trips through the dot-file (the ref's Get/SetSavedAccountName).
    #[test]
    fn saved_account_round_trips() {
        // A FILE, not a folder: since decision 1181 these two take the resolved path
        // (`local_state::saved_account_path`) rather than a base to join `account` onto.
        let dir = std::env::temp_dir().join(format!(
            "benilla-login-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir.join("account");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load_saved_account_from(&path), "");
        // The write creates the folder on its way, exactly as a first run must.
        save_account_to(&path, "ONE");
        assert_eq!(load_saved_account_from(&path), "ONE");
        save_account_to(&path, "");
        assert_eq!(load_saved_account_from(&path), "");
        assert!(!path.exists(), "clearing the name removes the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The clock system toggles the FOCUSED box on the shared 0.5 s period and leaves the other
    /// one alone. It guards the system's behaviour, not its registration (that is the one
    /// `tick_login_caret` line in [`LoginPlugin`], and a login screen with no clock is
    /// indistinguishable from one whose caret is in its ON half forever — `caret_shown` defaults
    /// to `true`, which is exactly how this went unnoticed).
    #[test]
    fn the_focused_box_caret_blinks() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<LoginForm>()
            .init_resource::<GlueDialog>()
            // The clock now asks which box owns the focus (1667): a dialog with an edit box takes
            // it. No dialog is open here, so the form's box keeps it — which is the case this
            // asserts.
            .add_systems(Update, tick_login_caret);

        let past_the_period = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(600));
            app.update();
            app.world().resource::<LoginForm>().account.caret_shown
        };

        // Account has the focus by default; one period each way is on → off → on.
        assert!(!past_the_period(&mut app), "the first period turns it off");
        assert!(past_the_period(&mut app), "the second turns it back on");
        // The box that doesn't own the keyboard never accumulates, so switching focus to it lands
        // on a solid caret rather than wherever its own clock would have drifted to.
        assert_eq!(
            app.world().resource::<LoginForm>().password.blink_accum,
            0.0
        );
    }
}
