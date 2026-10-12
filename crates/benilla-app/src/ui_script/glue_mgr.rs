//! The glue manager, `CGlueMgr` host side, under `WOW_GLUE_STOCK=1`: the screen-state machine of
//! `CGlueMgr::Update` (`0x46b930`, `screenState` at `0xb41da0`) over the native login engine. The
//! IO thread's parks and [`crate::login`]'s policy still do the network; this maps their messages
//! onto the reference's screen states and fires the glue events the stock GlueXML answers: the
//! status dialog (`OPEN_STATUS_DIALOG`, `UPDATE_STATUS_DIALOG`, `CLOSE_STATUS_DIALOG`),
//! `SET_GLUE_SCREEN` and `DISCONNECTED_FROM_SERVER`, and it serves the glue core's verbs
//! ([`benilla_ui::script::GlueRequest`]).
//!
//! The login, in the reference's order:
//!
//! 1. `DefaultServerLogin` reaches `CGlueMgr::Login` (`0x46afb0`). With the TOS and the EULA
//!    accepted (`[0xb41dfc]`, `[0xb41e04]`) and the state idle, an empty account or password
//!    opens an `OKAY` dialog (`LOGIN_ENTER_NAME` `0x83708c`, `LOGIN_ENTER_PASSWORD` `0x8370a0`);
//!    otherwise the logon's cells reset to -1 (`0x46b009`-`0x46b01b`), the state goes to 1
//!    (`0x46b028`), a `CANCEL` dialog opens with no text (`0x46b04d`) and the logon starts.
//! 2. Every frame in state 1 the logon's `(state, result)` pair raises a dialog when it changes
//!    (`0x46b140`): a failure (login state 5) opens `OKAY` with the result's `LOGIN_*` text
//!    (`0x836b78`), `CONNECTION_HELP` for `LOGIN_FAILED`; any other state but 11 and 12 opens
//!    `CANCEL` with its `LOGIN_STATE_*` text (`0x836b40`). Authenticated (login state 4) sets the
//!    final latch (`0x46b12c`), on which the state goes 0 then 2 (`0x46ba8d`, `0x46b1f0`); a
//!    failure cancels the logon and goes to 0 (`0x46bbbf`).
//! 3. State 2, the world connect, runs `ClientServices` operations: the connect (`0x5aab20`,
//!    which fetches the realm list and dials the realm `realmName` names), then the authentication
//!    (`0x5aaba0`, begun at `0x46bfd9` once the connect completes `RESPONSE_CONNECTED`). Every frame
//!    re-texts the dialog with the operation's status text (`0x46be8b`), empty while it runs
//!    (`0x5aaa35`, which `GlueDialog_OnEvent` ignores), so each completion shows; in the queue
//!    (`AUTH_WAIT_QUEUE`) the text is the queue's, with the button relabelled `CHANGE_REALM`
//!    (`0x46bdfc`). The authentication's success puts `charselect` up (`0x46c00e`-`0x46c01e`); a
//!    failure opens `OKAY` with the status's text (`0x85cae8`), or `OKAY_WITH_URL` with the URL key
//!    for the five codes of `0x803740` (`0x46bf63`), and goes to 0.
//!
//! `StatusDialogClick` (`0x46b720`) is the dialog button's verb, a jump on the state: state 1
//! cancels the logon (`0x5b3320`), states 2 and 4 complete the operation `RESPONSE_CANCELLED`.
//!
//! The realm list, the same machine's other half:
//!
//! 1. The connect's first act is a realm list query (`0x5aab20`; after the logon the logon's own
//!    list is the reply). On the reply (`0x5ab800`) a realm already selected this process
//!    (`[0xc28134]`, the record copied to `0xc27fb8`) is dialled; else the `realmName` CVar names
//!    one, first match by name, case folded (`0x5ab6f0`); else the connect fails
//!    `REALM_LIST_REALM_NOT_FOUND`. A query with no answer fails it `REALM_LIST_FAILED`.
//! 2. State 2's failure arm, for any status but `AUTH_FAILED`, raises the list over the screen
//!    (`0x46bf1f`-`0x46bf96`): `CLOSE_STATUS_DIALOG`, which takes the failure's `OKAY` back down,
//!    then the rebuild (`0x46e510`), whose tail fires `GET_PREFERRED_REALM_INFO` (`GlueParent`'s
//!    `SetGlueScreen("realmwizard")`) for an account with no characters that has chosen no realm
//!    this session, else `OPEN_REALM_LIST`. With no realm held it asks for the list instead
//!    (`0x46c06e`). `RESPONSE_CANCELLED` shows no dialog and drops the world socket alone
//!    (`0x46befd`), so the realmd link stays up for the next pick.
//! 3. `RequestRealmList` (`0x46b8d0`) is state 4 and a query, with the `CANCEL` dialog
//!    `REALM_LIST_IN_PROGRESS` when asked. State 4's frame (`0x46c0a5`) re-texts the dialog; a
//!    success goes to 0, closes the dialog and rebuilds (`0x46c0d2`-`0x46c0e2`), which is how the
//!    stock list's five-second refresh redraws; a failure opens `OKAY` and goes to 0.
//! 4. `ChangeRealm` reaches `ConnectToRealm` (`0x46b210`): the realm selected, state 2, the
//!    `CANCEL` dialog `GAME_SERVER_LOGIN`, and the connect. `RealmListDialogCancelled` closes the
//!    realmd link on the login screen only; `CancelRealmListQuery` cancels state 4 only.
//!
//! The list itself, its categories, sort and the verbs that read it are the glue VM's
//! ([`benilla_ui::script::GlueRealm`]); the manager fills its holder and runs its rebuild where
//! `CGlueMgr::Update` does. `SUGGEST_REALM_WRONG_PVP` and `SUGGEST_REALM_WRONG_CATEGORY` are in the
//! event table (`0xb41ebc`, `0xb41ec0`) but no glue code fires them: no `SignalEvent`
//! (`0x703e50`, `0x703f50`) site in `[0x46a400, 0x476020)` passes 19 or 20.
//!
//! The character list, the other half of the character screen's show:
//!
//! 1. The connect's success tears the roster down (`0x46c014`: `UPDATE_SELECTED_CHARACTER` with 1,
//!    `CHARACTER_LIST_UPDATE`) and puts `charselect` up, whose `OnShow` calls
//!    `GetCharacterListUpdate` (`0x4723d0`). The glue VM clears its records in the call; the tail
//!    (`0x46b880`) is state 3, the `CHAR_LIST_RETRIEVING` dialog and the query (`0x5aabb0`), which
//!    is a `CMSG_CHAR_ENUM` on the parked world session.
//! 2. The world connect completes with the authentication response (`0x5ab2b8` → `0x5aab80`),
//!    not with a roster; the IO thread fetches none of its own under the stock screen, so every
//!    call sends, in order with the realm requests on the one park channel. A reply from the
//!    current link (the world session's epoch) to that request completes the query, as
//!    `SetStatus` does (`0x5aa920`), so one that lands before the tick that takes a Cancel turns
//!    it into a load. A roster of another cause or link, or one with no query pending, is dropped
//!    ([`GlueMgr::on_roster`]).
//! 3. State 3's frame (`0x46c03a`) re-texts the dialog and acts on the completed query: success
//!    closes the dialog and loads the roster (`0x4724d0`: the records, `lastCharacterIndex`
//!    selected, `CHARACTER_LIST_UPDATE`); an empty roster with no realm chosen asks for the realm
//!    list instead and fires no update. A cancel (the dialog's button, `0x46b754`) tears the
//!    roster down and raises the realm list; another failure opens `OKAY`. The roster is torn
//!    down on the world connect's failure arm too (`0x46bf1f`).
//!
//! The native login reports a logon as one blocking step, so of the logon's states only
//! `CONNECTING` and `AUTHENTICATED` are seen; its refusals map through the reference's own
//! byte-to-result table ([`realmd_result`]).

use bevy::prelude::*;

use benilla_protocol::{Character, LoginRefusal, LoginStage, RealmInfo, RosterCause};
use benilla_ui::script::{
    GlueCharacter, GlueNotice, GlueRealm, GlueRealmCells, GlueRequest, GlueSelectCells, GlueServer,
    NoticeState, ScriptValue, UiScript, VmKind,
};

use crate::login::queue::QueueEstimate;
use crate::login::GlueLogin;
use crate::net::{
    CharListMessage, CharRequest, DisconnectedMessage, LoginFailedMessage, LoginQueuedMessage,
    LoginStageMessage, ParkRequest, RealmListMessage, RealmRequest, RosterFailedMessage,
    WorldAuthMessage,
};

/// `CGlueMgr`'s `screenState` (`0xb41da0`), named by the functions that set each value: the
/// states the login, the realm list and the character list reach. The reference's others are
/// create, delete and rename (5-7, `0x46b29e`, `0x46b2f8`, `0x46b3a4`), `EnterWorld` (8,
/// `0x46b711`), a refused character login (9, `0x402075`) and the patch and survey downloads (10
/// and 11, `0x473c40`, `0x473da8`), each set by a verb these screens do not have yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScreenState {
    /// No operation (`0x46a7ce`, and every terminal arm).
    #[default]
    Idle = 0,
    /// The realmd logon (`CGlueMgr::Login`, `0x46b028`).
    Logon = 1,
    /// The world connect: realm list, dial and authentication (`0x46b1f9`, `0x46b250`).
    Connect = 2,
    /// The character list query (`0x46b880`, from `GetCharacterListUpdate`).
    CharList = 3,
    /// A realm list query (`RequestRealmList`, `0x46b8d2`).
    RealmList = 4,
}

/// The logon's `LOGIN_STATE_*` keys by login state (`0x836b40`, 13 entries).
const LOGIN_STATE_KEYS: [&str; 13] = [
    "LOGIN_STATE_INITIALIZED",
    "LOGIN_STATE_CONNECTING",
    "LOGIN_STATE_HANDSHAKING",
    "LOGIN_STATE_AUTHENTICATING",
    "LOGIN_STATE_AUTHENTICATED",
    "LOGIN_STATE_FAILED",
    "LOGIN_STATE_DOWNLOADFILE",
    "LOGIN_STATE_PIN",
    "LOGIN_STATE_PIN_WAIT",
    "LOGIN_STATE_CHECKINGVERSIONS",
    "RESPONSE_CONNECTED",
    "LOGIN_STATE_DISCONNECTED",
    "LOGIN_STATE_SURVEY",
];

/// The logon's `LOGIN_*` keys by result (`0x836b78`, 21 entries).
const LOGIN_RESULT_KEYS: [&str; 21] = [
    "LOGIN_OK",
    "LOGIN_INVALID_CHALLENGE_MESSAGE",
    "LOGIN_SRP_ERROR",
    "LOGIN_INVALID_PROOF_MESSAGE",
    "LOGIN_BAD_SERVER_PROOF",
    "LOGIN_INVALID_RECODE_MESSAGE",
    "LOGIN_BAD_SERVER_RECODE_PROOF",
    "LOGIN_UNKNOWN_ACCOUNT",
    "LOGIN_UNKNOWN_ACCOUNT_PIN",
    "LOGIN_UNKNOWN_ACCOUNT_CALL",
    "LOGIN_INCORRECT_PASSWORD",
    "LOGIN_FAILED",
    "LOGIN_SERVER_DOWN",
    "LOGIN_BANNED",
    "LOGIN_BADVERSION",
    "LOGIN_ALREADYONLINE",
    "LOGIN_NOTIME",
    "LOGIN_DBBUSY",
    "LOGIN_SUSPENDED",
    "LOGIN_PARENTALCONTROL",
    "DISCONNECTED",
];

/// The `ClientServices` status keys (`0x85cae8`, 83 entries), the `ResponseCodes` the world
/// server's `SMSG_AUTH_RESPONSE` byte indexes directly.
const STATUS_KEYS: [&str; 0x53] = [
    "RESPONSE_SUCCESS",
    "RESPONSE_FAILURE",
    "RESPONSE_CANCELLED",
    "RESPONSE_DISCONNECTED",
    "RESPONSE_FAILED_TO_CONNECT",
    "RESPONSE_CONNECTED",
    "RESPONSE_VERSION_MISMATCH",
    "CSTATUS_CONNECTING",
    "CSTATUS_NEGOTIATING_SECURITY",
    "CSTATUS_NEGOTIATION_COMPLETE",
    "CSTATUS_NEGOTIATION_FAILED",
    "CSTATUS_AUTHENTICATING",
    "AUTH_OK",
    "AUTH_FAILED",
    "AUTH_REJECT",
    "AUTH_BAD_SERVER_PROOF",
    "AUTH_UNAVAILABLE",
    "AUTH_SYSTEM_ERROR",
    "AUTH_BILLING_ERROR",
    "AUTH_BILLING_EXPIRED",
    "AUTH_VERSION_MISMATCH",
    "AUTH_UNKNOWN_ACCOUNT",
    "AUTH_INCORRECT_PASSWORD",
    "AUTH_SESSION_EXPIRED",
    "AUTH_SERVER_SHUTTING_DOWN",
    "AUTH_ALREADY_LOGGING_IN",
    "AUTH_LOGIN_SERVER_NOT_FOUND",
    "AUTH_WAIT_QUEUE",
    "AUTH_BANNED",
    "AUTH_ALREADY_ONLINE",
    "AUTH_NO_TIME",
    "AUTH_DB_BUSY",
    "AUTH_SUSPENDED",
    "AUTH_PARENTAL_CONTROL",
    "REALM_LIST_IN_PROGRESS",
    "REALM_LIST_SUCCESS",
    "REALM_LIST_FAILED",
    "REALM_LIST_INVALID",
    "REALM_LIST_REALM_NOT_FOUND",
    "ACCOUNT_CREATE_IN_PROGRESS",
    "ACCOUNT_CREATE_SUCCESS",
    "ACCOUNT_CREATE_FAILED",
    "CHAR_LIST_RETRIEVING",
    "CHAR_LIST_RETRIEVED",
    "CHAR_LIST_FAILED",
    "CHAR_CREATE_IN_PROGRESS",
    "CHAR_CREATE_SUCCESS",
    "CHAR_CREATE_ERROR",
    "CHAR_CREATE_FAILED",
    "CHAR_CREATE_NAME_IN_USE",
    "CHAR_CREATE_DISABLED",
    "CHAR_CREATE_PVP_TEAMS_VIOLATION",
    "CHAR_CREATE_SERVER_LIMIT",
    "CHAR_CREATE_ACCOUNT_LIMIT",
    "CHAR_CREATE_SERVER_QUEUE",
    "CHAR_CREATE_ONLY_EXISTING",
    "CHAR_DELETE_IN_PROGRESS",
    "CHAR_DELETE_SUCCESS",
    "CHAR_DELETE_FAILED",
    "CHAR_DELETE_FAILED_LOCKED_FOR_TRANSFER",
    "CHAR_LOGIN_IN_PROGRESS",
    "CHAR_LOGIN_SUCCESS",
    "CHAR_LOGIN_NO_WORLD",
    "CHAR_LOGIN_DUPLICATE_CHARACTER",
    "CHAR_LOGIN_NO_INSTANCES",
    "CHAR_LOGIN_FAILED",
    "CHAR_LOGIN_DISABLED",
    "CHAR_LOGIN_NO_CHARACTER",
    "CHAR_LOGIN_LOCKED_FOR_TRANSFER",
    "CHAR_NAME_NO_NAME",
    "CHAR_NAME_TOO_SHORT",
    "CHAR_NAME_TOO_LONG",
    "CHAR_NAME_INVALID_CHARACTER",
    "CHAR_NAME_MIXED_LANGUAGES",
    "CHAR_NAME_PROFANE",
    "CHAR_NAME_RESERVED",
    "CHAR_NAME_INVALID_APOSTROPHE",
    "CHAR_NAME_MULTIPLE_APOSTROPHES",
    "CHAR_NAME_THREE_CONSECUTIVE",
    "CHAR_NAME_INVALID_SPACE",
    "CHAR_NAME_CONSECUTIVE_SPACES",
    "CHAR_NAME_FAILURE",
    "CHAR_NAME_SUCCESS",
];

/// The five world statuses whose failure dialog carries a help URL, `{code, urlKey}` records at
/// `0x803740` (stride 0x24), walked at `0x46bec0`.
const URL_STATUSES: [(u8, &str); 5] = [
    (0x1c, "AUTH_BANNED_URL"),
    (0x1f, "AUTH_DB_BUSY_URL"),
    (0x1e, "AUTH_NO_TIME_URL"),
    (0x20, "AUTH_SUSPENDED_URL"),
    (0x21, "AUTH_PARENTAL_CONTROL_URL"),
];

/// Login states, the reference's numbers.
const LOGIN_CONNECTING: i32 = 1;
const LOGIN_AUTHENTICATED: i32 = 4;
const LOGIN_FAILED_STATE: i32 = 5;
const LOGIN_DOWNLOADFILE: i32 = 6;
const LOGIN_PIN_WAIT: i32 = 8;

/// Results.
const RESULT_LOGIN_FAILED: i32 = 0x0b;
const RESULT_DISCONNECTED: i32 = 0x14;

/// World statuses.
const STATUS_RESPONSE_FAILURE: u8 = 0x01;
const STATUS_CANCELLED: u8 = 0x02;
const STATUS_FAILED_TO_CONNECT: u8 = 0x04;
const STATUS_RESPONSE_CONNECTED: u8 = 0x05;
const STATUS_CONNECTING: u8 = 0x07;
const STATUS_AUTHENTICATING: u8 = 0x0b;
const STATUS_AUTH_OK: u8 = 0x0c;
const STATUS_AUTH_FAILED: u8 = 0x0d;
const STATUS_WAIT_QUEUE: u8 = 0x1b;
const STATUS_REALM_LIST_IN_PROGRESS: u8 = 0x22;
const STATUS_REALM_LIST_SUCCESS: u8 = 0x23;
const STATUS_REALM_LIST_FAILED: u8 = 0x24;
const STATUS_REALM_LIST_REALM_NOT_FOUND: u8 = 0x26;
const STATUS_CHAR_LIST_RETRIEVING: u8 = 0x2a;
const STATUS_CHAR_LIST_RETRIEVED: u8 = 0x2b;
const STATUS_CHAR_LIST_FAILED: u8 = 0x2c;

/// What a realmd result byte becomes, `(login state, result)`: `Logon::OnAuthResult`'s
/// dispatch (`0x5b2c90`, byte index `0x5b2ea4`, jumps `0x5b2e78`) after the handlers' clamp of a
/// byte past `0x0f` to `0xff` (`0x5bac36`, `0x5bad8d`). 0x00 and 0x0E take the success arm
/// (`0x5b2cc9`, `0x5b2cd4` to `0x5b2e16`), authenticated; the unknown account and the wrong
/// password share one arm (`0x5b2d00`, result 7 with no PIN); a challenge's 0x0A is the patch state
/// (`0x5b2d3b`). A proof's 0x0A never gets here: `Grunt::OnLogonProof` returns on it (`0x5bada2`),
/// so the logon reports nothing and its dialog stands (`None`).
pub(crate) fn realmd_result(byte: u8, reply: benilla_protocol::LogonReply) -> Option<(i32, i32)> {
    if byte == 0x0a && reply == benilla_protocol::LogonReply::Proof {
        return None;
    }
    let byte = if byte > 0x0f { 0xff } else { byte };
    let result = match byte {
        0x00 | 0x0e => return Some((LOGIN_AUTHENTICATED, 0)),
        0x03 => 0x0d,
        0x04 | 0x05 => 0x07,
        0x06 => 0x0f,
        0x07 => 0x10,
        0x08 => 0x11,
        0x09 => 0x0e,
        0x0a => return Some((LOGIN_DOWNLOADFILE, 0)),
        0x0c => 0x12,
        0x0f => 0x13,
        0xff => RESULT_DISCONNECTED,
        // 0x01, 0x02, 0x0b, 0x0d: the default arm (`0x5b2e00`).
        _ => RESULT_LOGIN_FAILED,
    };
    Some((LOGIN_FAILED_STATE, result))
}

/// A `ClientServices` operation (`[svc+0x1b10]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OpKind {
    /// `COP_CONNECT` (2, `0x5aab20`).
    #[default]
    Connect,
    /// The authentication (3, `0x5aaba0`).
    Auth,
    /// `COP_GET_REALMS` (9, `0x5ab840`).
    GetRealms,
    /// The character list (6, `0x5aabb0`).
    CharList,
}

/// The operation record (`BeginOperation` `0x5aa400`, `SetStatus` `0x5aa920`): its status code
/// (`[svc+0x1b14]`), completed (`[svc+0x1b08]`) and succeeded (`[svc+0x1b0c]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Operation {
    kind: OpKind,
    status: u8,
    done: bool,
    ok: bool,
    /// The text of a failure the reference has no status for, shown in place of the status's.
    raw: Option<String>,
    /// A connect or a query whose realm list reply has not come (`0x5abb90` dispatches the reply
    /// on the operation pending).
    awaiting_list: bool,
}

impl Operation {
    fn begin(kind: OpKind, status: u8) -> Self {
        Operation {
            kind,
            status,
            awaiting_list: matches!(kind, OpKind::Connect | OpKind::GetRealms),
            ..Default::default()
        }
    }

    fn complete(&mut self, ok: bool, status: u8) {
        self.status = status;
        self.ok = ok;
        self.done = true;
    }

    /// `GetStatus`'s text: the status's key once completed, none while it runs (`0x5aaa35`).
    fn text(&self) -> Option<Text> {
        if !self.done {
            return None;
        }
        Some(match &self.raw {
            Some(t) => Text::Raw(t.clone()),
            None => status_text(self.status).0,
        })
    }
}

/// A glue string by key or as given.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Text {
    Key(&'static str),
    Raw(String),
}

/// One glue event the manager fires into the glue VM.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outgoing {
    /// `OPEN_STATUS_DIALOG(which[, text[, data]])` (event 3).
    Open {
        which: &'static str,
        text: Option<Text>,
        data: Option<String>,
    },
    /// `UPDATE_STATUS_DIALOG(text[, button])` (event 4).
    Update { text: Text, button: Option<Text> },
    /// The queue's per-frame re-text, composed at the fire from the ring (`0x46bcb8`-`0x46bdfc`).
    Queue,
    /// `SET_GLUE_SCREEN(name)` (event 0, `0x46b840`).
    Screen(&'static str),
    /// `DISCONNECTED_FROM_SERVER` (event 2, `0x46c62e`).
    Disconnected,
    /// `CLOSE_STATUS_DIALOG` (event 5).
    Close,
    /// The realm list rebuild (`0x46e510`) over the holder, which fires `OPEN_REALM_LIST` or
    /// `GET_PREFERRED_REALM_INFO`.
    Rebuild,
    /// The roster teardown (`0x472580`): `UPDATE_SELECTED_CHARACTER` with 1, then
    /// `CHARACTER_LIST_UPDATE`.
    Teardown,
    /// The roster load (`0x4724d0`) over the roster the last reply left.
    Roster,
}

/// The logon's notification cells: the latest `(state, result)` (`0x836b28`, `0x836b24`) and the
/// pair last shown (`0x836b30`, `0x836b2c`), -1 when none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LoginCells {
    state: i32,
    result: i32,
    shown_state: i32,
    shown_result: i32,
}

impl Default for LoginCells {
    fn default() -> Self {
        LoginCells {
            state: -1,
            result: -1,
            shown_state: -1,
            shown_result: -1,
        }
    }
}

/// The host side of `CGlueMgr`.
#[derive(Resource, Default)]
pub(crate) struct GlueMgr {
    pub(crate) state: ScreenState,
    cells: LoginCells,
    /// Authenticated, waiting for the next tick (`[0xb41e18]`).
    auth_final: bool,
    /// The four agreements, read once from their CVars (`0x46a400`).
    notices: Option<[NoticeState; 4]>,
    /// The world connection is up (`[svc+0x1b00]`).
    connected: bool,
    /// The realm this session connected to.
    realm: Option<benilla_protocol::RealmInfo>,
    /// State 2's operation.
    op: Operation,
    queue: QueueEstimate,
    queue_realm: Option<String>,
    /// The current glue screen's name, as the VM's `SetCurrentScreen` last set it.
    screen: String,
    outbox: Vec<Outgoing>,
    /// The glue VM this manager last served, by session.
    vm: Option<u64>,
    /// The realm holder (`[0xc28118]`): the realms the last list reply carried.
    holder: Vec<RealmInfo>,
    /// The holder changed since the VM was last seeded.
    holder_moved: bool,
    /// `[0xc28134]` and its copy at `0xc27fb8`: the realm the next connect dials, set by the first
    /// selection and kept for the process.
    selected: Option<RealmInfo>,
    /// `[svc+0x1ae4]`: the last roster's character count.
    characters: u16,
    /// The roster the last character list reply left in `ClientServices` (`[svc+0x1ae8]`), which
    /// the roster load (`0x4724d0`) walks.
    roster: Vec<Character>,
    /// The number of the world session the link is (`WorldAuthMessage::epoch`): the roster
    /// replies of any other are not this link's.
    link: u64,
    /// The select verbs' cells, carried from one glue VM to the next.
    select_cells: GlueSelectCells,
    /// The realm list's sort and selection cells, carried from one glue VM to the next.
    realm_cells: GlueRealmCells,
    /// The manager moved [`Self::realm_cells`], for the next seed.
    cells_moved: bool,
    /// Realm and character requests for the IO thread in the order they were made, sent at the end
    /// of the frame on its one ordered channel.
    park_out: Vec<ParkRequest>,
    /// Asks of the login engine the per-frame arms made, sent with the verbs' own.
    login_out: Vec<GlueLogin>,
}

impl GlueMgr {
    /// The agreements as read at the startup (`0x46a400`).
    pub(super) fn set_notices(&mut self, notices: [NoticeState; 4]) {
        self.notices = Some(notices);
    }

    fn notice(&self, notice: GlueNotice) -> NoticeState {
        self.notices.map(|n| n[notice as usize]).unwrap_or_default()
    }

    fn open(&mut self, which: &'static str, text: Option<Text>) {
        self.outbox.push(Outgoing::Open {
            which,
            text,
            data: None,
        });
    }

    /// `CGlueMgr::Login` (`0x46afb0`).
    fn login(&mut self, account: String, password: String, asks: &mut Vec<GlueLogin>) {
        if !self.notice(GlueNotice::Tos).accepted
            || !self.notice(GlueNotice::Eula).accepted
            || self.state != ScreenState::Idle
        {
            return;
        }
        if account.is_empty() {
            self.open("OKAY", Some(Text::Key("LOGIN_ENTER_NAME")));
            return;
        }
        if password.is_empty() {
            self.open("OKAY", Some(Text::Key("LOGIN_ENTER_PASSWORD")));
            return;
        }
        self.begin_logon();
        asks.push(GlueLogin::Submit { account, password });
    }

    /// Login's own state edge: the cells cleared, the latch down, state 1, and the `CANCEL`
    /// dialog with no text (`0x46b009`-`0x46b04d`).
    fn begin_logon(&mut self) {
        self.cells = LoginCells::default();
        self.auth_final = false;
        self.state = ScreenState::Logon;
        self.open("CANCEL", None);
    }

    /// `StatusDialogClick` (`0x46b720`, jump table `0x46b7a8`).
    fn status_dialog_click(&mut self, asks: &mut Vec<GlueLogin>) {
        match self.state {
            // `0x46b731`: the pending operation's callback; none is pending outside an operation.
            ScreenState::Idle => {}
            // `0x46b73d`: the logon cancelled, state 0.
            ScreenState::Logon => {
                asks.push(GlueLogin::Abandon);
                self.state = ScreenState::Idle;
            }
            // `0x46b754` (states 2 and 3): the operation completes `RESPONSE_CANCELLED` and the
            // roster is torn down (`0x472580`). A world connect's next tick takes the failure arm
            // without a dialog (`0x46bef2`), dropping the world socket and raising the realm
            // list; a character list's asks for the list again (`0x46c069`).
            ScreenState::Connect | ScreenState::CharList => {
                self.op.complete(false, STATUS_CANCELLED);
                self.outbox.push(Outgoing::Teardown);
            }
            // `0x46b776`: the query cancelled, state 0, nothing sent.
            ScreenState::RealmList => {
                self.op.complete(false, STATUS_CANCELLED);
                self.state = ScreenState::Idle;
            }
        }
    }

    /// The asks the per-frame arms made of the login engine and of the IO thread.
    fn take_asks(&mut self) -> (Vec<GlueLogin>, Vec<ParkRequest>) {
        (
            std::mem::take(&mut self.login_out),
            std::mem::take(&mut self.park_out),
        )
    }

    /// `0x46b880`, the tail of `GetCharacterListUpdate`: state 3, the `CHAR_LIST_RETRIEVING`
    /// dialog and the query (`0x5aabb0`: operation 6 at status `0x2a`, then `CMSG_CHAR_ENUM` on
    /// the world link). With no link the operation fails `RESPONSE_FAILED_TO_CONNECT` at once
    /// (`0x5aabca`). State 9, a refused character login (`0x46b880`'s first test), returns before
    /// any of it; that state is not reached yet.
    fn get_character_list(&mut self) {
        self.state = ScreenState::CharList;
        self.open("CANCEL", Some(Text::Key("CHAR_LIST_RETRIEVING")));
        self.op = Operation::begin(OpKind::CharList, STATUS_CHAR_LIST_RETRIEVING);
        if !self.connected {
            self.op.complete(false, STATUS_FAILED_TO_CONNECT);
        } else {
            self.park_out.push(CharRequest::Enumerate.into());
        }
    }

    /// `0x46b8d0`, `RequestRealmList`'s body: state 4, the dialog when asked, and the query
    /// (`0x5ab840`, `COP_GET_REALMS` at `REALM_LIST_IN_PROGRESS`).
    fn request_realm_list(&mut self, dialog: bool) {
        self.state = ScreenState::RealmList;
        if dialog {
            self.open("CANCEL", Some(Text::Key("REALM_LIST_IN_PROGRESS")));
        }
        self.op = Operation::begin(OpKind::GetRealms, STATUS_REALM_LIST_IN_PROGRESS);
        self.park_out.push(RealmRequest::Refresh.into());
    }

    /// `CancelRealmListQuery` (`0x46b7e0`): in state 4 only, the query completes cancelled and the
    /// state goes to 0; nothing is sent, so a late reply only fills the holder.
    fn cancel_realm_list_query(&mut self) {
        if self.state == ScreenState::RealmList {
            self.op.complete(false, STATUS_CANCELLED);
            self.state = ScreenState::Idle;
        }
    }

    /// `ConnectToRealm` (`0x46b210`) for the realm `ChangeRealm` named, `realmName` already set to
    /// it: the selection (`0x5ab7b0`'s tail, `0x5ab6f0`), state 2, the `GAME_SERVER_LOGIN` dialog
    /// and the connect. With a world connection up the reference drops it first (`0x46b22b` →
    /// `0x5ab1a0`) and begins the same connect from the disconnect handler (`0x46c5bf`); here the
    /// drop is asked first and the connect follows at once, so a failed query finds no session.
    fn connect_to_realm(&mut self, name: &str) {
        if let Some(r) = select_by_name(&self.holder, name) {
            self.selected = Some(r);
        }
        if self.connected {
            // `0x46b22b`: the world connection drops before anything is asked of realmd.
            self.park_out.push(RealmRequest::Leave.into());
        }
        self.connected = false;
        self.state = ScreenState::Connect;
        self.open("CANCEL", Some(Text::Key("GAME_SERVER_LOGIN")));
        self.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
        self.queue = QueueEstimate::default();
        // The connect's own query (`0x5aab20` → `0x5b2220`).
        self.park_out.push(RealmRequest::Refresh.into());
    }

    /// A realm list reply: the holder filled (`0x5b2230`), then the pending operation's arm of
    /// `0x5abb90`. A query's success is `REALM_LIST_SUCCESS`; a connect's dials the selected realm,
    /// or the one `realm_name` names, and completes only through the dial (`0x5ab823` calls no
    /// `SetStatus`); no answer fails either `REALM_LIST_FAILED`.
    fn on_realm_list(&mut self, realms: Vec<RealmInfo>, failed: bool, realm_name: &str) {
        if !failed {
            self.holder = realms;
            self.holder_moved = true;
        }
        if !self.op.awaiting_list || self.op.done {
            return;
        }
        self.op.awaiting_list = false;
        if failed {
            self.op.complete(false, STATUS_REALM_LIST_FAILED);
            return;
        }
        match self.op.kind {
            OpKind::GetRealms => self.op.complete(true, STATUS_REALM_LIST_SUCCESS),
            OpKind::Connect => {
                if self.selected.is_none() {
                    self.selected = select_by_name(&self.holder, realm_name);
                }
                match &self.selected {
                    Some(r) => {
                        info!("glue: the connect dials {:?}", r.name);
                        // Stamped with the abandon generation as it is sent.
                        self.park_out.push(
                            RealmRequest::Enter {
                                realm: r.clone(),
                                generation: 0,
                            }
                            .into(),
                        );
                    }
                    None => {
                        info!("glue: realmName {realm_name:?} names no listed realm");
                        self.op.complete(false, STATUS_REALM_LIST_REALM_NOT_FOUND);
                    }
                }
            }
            OpKind::Auth | OpKind::CharList => {}
        }
    }

    /// A login stage off the IO thread. `glue_up` is whether the stock screens stand: a stage of
    /// an attempt the native policy made on its own (the `WOW_USER` fast path, a resubmit) shows
    /// as a login only there.
    fn on_stage(&mut self, stage: LoginStage, glue_up: bool) {
        match stage {
            LoginStage::Connecting => {
                if self.state == ScreenState::Idle && glue_up {
                    self.begin_logon();
                }
                if self.state == ScreenState::Logon {
                    self.cells.state = LOGIN_CONNECTING;
                    self.cells.result = 0;
                }
            }
            // The SRP exchange is inside the native logon's one blocking call.
            LoginStage::Authenticating => {}
            // The world dial begins: a logon with no realm list to publish is done.
            LoginStage::Handshaking => self.on_logon_done(),
            // The connect completes `RESPONSE_CONNECTED` (`0x5aaa90`).
            LoginStage::WorldConnected => {
                if self.state == ScreenState::Connect && self.op.kind == OpKind::Connect {
                    self.op.complete(true, STATUS_RESPONSE_CONNECTED);
                }
            }
        }
    }

    /// The logon authenticated: login state 4, which arms the final latch (`0x46b0f0`).
    fn on_logon_done(&mut self) {
        if self.state == ScreenState::Logon {
            self.cells.state = LOGIN_AUTHENTICATED;
            self.cells.result = 0;
            self.auth_final = true;
        }
    }

    /// A failure before the roster.
    fn on_failed(&mut self, msg: &LoginFailedMessage) {
        match self.state {
            ScreenState::Logon => {
                let cells = match msg.refusal {
                    Some(LoginRefusal::Logon(byte)) => {
                        realmd_result(byte, benilla_protocol::LogonReply::Challenge)
                    }
                    Some(LoginRefusal::LogonProof(byte)) => {
                        realmd_result(byte, benilla_protocol::LogonReply::Proof)
                    }
                    // A dial that never connected reports `LOGIN_SERVER_DOWN` (`0x5b1bb2`) and then
                    // the unexpected disconnect (`0x5b1bb7` → `0x5b1c30`, result `0xff`), so the
                    // dialog the next tick shows is the last, `DISCONNECTED`; a transport failure
                    // after the connect is that disconnect alone.
                    _ => Some((LOGIN_FAILED_STATE, RESULT_DISCONNECTED)),
                };
                if let Some((state, result)) = cells {
                    self.cells.state = state;
                    self.cells.result = result;
                }
            }
            // The character list query's reply never came: the server refused the roster to a
            // client without Warden (the park's `char_enum`), the one failure it reports. The
            // query completes failed with the words, and the link is gone.
            ScreenState::CharList if self.op.kind != OpKind::CharList || self.op.done => {}
            ScreenState::Connect | ScreenState::CharList => {
                if self.state == ScreenState::CharList {
                    self.connected = false;
                }
                let status = match msg.refusal {
                    Some(LoginRefusal::World(code)) => code,
                    Some(LoginRefusal::Logon(_) | LoginRefusal::LogonProof(_)) => {
                        STATUS_RESPONSE_FAILURE
                    }
                    None if msg.terminal => STATUS_RESPONSE_FAILURE,
                    None => STATUS_FAILED_TO_CONNECT,
                };
                self.op.complete(false, status);
                // A server refusing a client without Warden has no reference status: its own
                // words.
                if msg.terminal && msg.refusal.is_none() {
                    self.op.raw = Some(msg.reason.clone());
                }
            }
            _ => {}
        }
    }

    /// State 2's failure arm (`0x46bea7`-`0x46bfc8`): the dialog unless cancelled, state 0, a
    /// cancel's world socket dropped (`0x46befd`); `AUTH_FAILED` from the character screen goes
    /// back to login (`0x46bfa7`); every other status tears the roster down (`0x46bf1f`) and
    /// raises the realm list (`0x46bf24`-`0x46bf96`), or with no realm held asks for one
    /// (`0x46c06e`).
    fn world_failed(&mut self) {
        let status = self.op.status;
        if status != STATUS_CANCELLED {
            let (text, code) = match self.op.text() {
                Some(Text::Raw(t)) => (Text::Raw(t), None),
                _ => status_text(status),
            };
            match code.and_then(|c| URL_STATUSES.iter().find(|(s, _)| *s == c)) {
                Some((_, url)) => self.outbox.push(Outgoing::Open {
                    which: "OKAY_WITH_URL",
                    text: Some(text),
                    data: Some(url.to_string()),
                }),
                None => self.open("OKAY", Some(text)),
            }
        }
        self.state = ScreenState::Idle;
        self.connected = false;
        if status == STATUS_CANCELLED {
            self.login_out.push(GlueLogin::CancelWorld);
        }
        if status == STATUS_AUTH_FAILED {
            if self.screen.eq_ignore_ascii_case("charselect") {
                self.outbox.push(Outgoing::Screen("login"));
            }
            return;
        }
        self.outbox.push(Outgoing::Teardown);
        if self.holder.is_empty() {
            self.request_realm_list(true);
        } else {
            self.outbox.push(Outgoing::Close);
            self.outbox.push(Outgoing::Rebuild);
        }
    }

    /// One queue packet: the operation is re-armed as the queue's, status `AUTH_WAIT_QUEUE` and
    /// not complete (`0x5ab280`), and the ring takes a sample.
    fn on_queued(&mut self, position: Option<u32>, realm: Option<String>, now_ms: u32) {
        if self.state != ScreenState::Connect {
            return;
        }
        self.op = Operation::begin(OpKind::Auth, STATUS_WAIT_QUEUE);
        self.queue_realm = realm;
        if let Some(p) = position {
            self.queue.sample(p, now_ms);
        }
    }

    /// A roster reply from the current link stores the roster and completes the operation
    /// `CHAR_LIST_RETRIEVED` (`0x5ab300` → `SetStatus`, `0x5aa920`), which overwrites the status
    /// whatever it was: a reply that lands between a Cancel and the tick that takes it turns the
    /// cancel into a load, as the reference's does, and a second reply replaces the first.
    ///
    /// Deviation: the reference's handler completes whichever operation is current; here only the
    /// character list's does, in state 3, and a reply of another cause (a roster nothing of the
    /// screen asked for) or of another link (before a realm change) completes nothing. Those
    /// replies cannot be told from the operation they would be applied to in the reference, which
    /// has one link and one request in flight; here the IO thread's park serves several.
    fn on_roster(&mut self, roster: Vec<Character>, cause: RosterCause, epoch: u64) {
        if cause != RosterCause::Asked
            || epoch != self.link
            || self.state != ScreenState::CharList
            || self.op.kind != OpKind::CharList
        {
            return;
        }
        self.characters = roster.len().min(usize::from(u16::MAX)) as u16;
        self.roster = roster;
        self.op.complete(true, STATUS_CHAR_LIST_RETRIEVED);
    }

    /// The roster ask failed on the stream: the query completes `CHAR_LIST_FAILED` (`0x5ab300`'s
    /// failure leg, `SetStatus(0, 0x2c)`), which the tick opens as `OKAY`. The session ends behind
    /// it as a dropped socket does, so the disconnect handler follows (`0x46c540`).
    fn on_roster_failed(&mut self, epoch: u64) {
        if epoch != self.link
            || self.state != ScreenState::CharList
            || self.op.kind != OpKind::CharList
        {
            return;
        }
        self.op.complete(false, STATUS_CHAR_LIST_FAILED);
    }

    /// The world server accepted the authentication: the link is up, and in state 2 the
    /// authentication completes `AUTH_OK` (`0x5aab80`), which the tick shows and takes down the
    /// success arm.
    fn on_world_authenticated(&mut self, realm: benilla_protocol::RealmInfo, epoch: u64) {
        self.connected = true;
        self.link = epoch;
        self.realm = Some(realm);
        if self.state == ScreenState::Logon {
            // A logon whose world half finished inside one frame.
            self.on_logon_done();
            self.tick();
        }
        if self.state != ScreenState::Connect {
            return;
        }
        self.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
        self.op.complete(true, STATUS_AUTH_OK);
    }

    /// The session ended. A logout or a refused character login ends only the world: the link
    /// is kept (`0x5aaeb0` and `0x5aad70` reach no close), so the screen's roster ask goes out on
    /// it (`IsConnectedToServer`, `0x46d380`). Over (`session_over`), the disconnect handler
    /// (`0x46c540`): state 0, and `DISCONNECTED_FROM_SERVER` (`0x46c62e`), which the stock screens
    /// answer with the login screen and the `DISCONNECTED` dialog. An unattended reconnect is no
    /// disconnect to the screens.
    ///
    /// Out of state 2 it fires the event (`0x46c618`); in state 2 a completed, failed operation
    /// opens `OKAY` with its text instead (`0x46c671`-`0x46c68f`). The handler's first test, that
    /// the dropped connection is `ClientServices`' current one (`0x5abd40`, else "Invalid
    /// disconnect"), has no native counterpart: the IO thread holds one session and reports only
    /// its own end.
    fn on_disconnected(&mut self, end: benilla_protocol::SessionEnd, session_over: bool) {
        if end == benilla_protocol::SessionEnd::LoggedOut {
            return;
        }
        self.connected = false;
        if !session_over {
            return;
        }
        let was_connect = self.state == ScreenState::Connect;
        self.state = ScreenState::Idle;
        let op = std::mem::take(&mut self.op);
        match op.text() {
            Some(text) if was_connect && !op.ok => self.open("OKAY", Some(text)),
            _ => self.outbox.push(Outgoing::Disconnected),
        }
    }

    /// The per-frame arms of `CGlueMgr::Update` that run without a new message.
    fn tick(&mut self) {
        match self.state {
            ScreenState::Logon => {
                // `0x46ba7c`: every login state but PIN_WAIT reaches the dialog arm.
                if self.cells.state != LOGIN_PIN_WAIT {
                    self.show_login_dialog();
                }
                if self.auth_final {
                    // `0x46ba8d`-`0x46ba98`, then `0x46b1f0`'s idle test passes and the connect
                    // begins at `CSTATUS_CONNECTING` (`0x5aab4a`), its ring cleared (`0x5aab30`),
                    // with no dialog of its own; the logon's list is its query's reply.
                    self.state = ScreenState::Connect;
                    self.auth_final = false;
                    self.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
                    self.queue = QueueEstimate::default();
                    return;
                }
                // `0x46bbbf`: a failed logon is cancelled and the state goes to 0. The patch
                // state (`0x46bb64`) waits on a downloader benilla does not have; its dialog's
                // Cancel ends it.
                if self.cells.state == LOGIN_FAILED_STATE {
                    self.state = ScreenState::Idle;
                }
            }
            ScreenState::Connect => self.tick_connect(),
            ScreenState::CharList => self.tick_char_list(),
            ScreenState::RealmList => self.tick_realm_list(),
            ScreenState::Idle => {}
        }
    }

    /// State 4's frame (`0x46c0a5`): the dialog re-texted, then a completed query's arm, success
    /// to state 0, the dialog closed and the rebuild (`0x46c0d2`-`0x46c0e2`), failure to `OKAY`
    /// with its text and state 0 (`0x46c0c9` → `0x46c177`).
    fn tick_realm_list(&mut self) {
        if let Some(text) = self.op.text() {
            self.outbox.push(Outgoing::Update { text, button: None });
        }
        if !self.op.done {
            return;
        }
        self.state = ScreenState::Idle;
        if self.op.ok {
            self.outbox.push(Outgoing::Close);
            self.outbox.push(Outgoing::Rebuild);
        } else {
            let text = self.op.text();
            self.open("OKAY", text);
        }
    }

    /// State 2's frame (`0x46bc49`-`0x46c02e`): the dialog re-texted from the operation, then a
    /// completed operation's arm.
    fn tick_connect(&mut self) {
        if self.op.status == STATUS_WAIT_QUEUE {
            self.outbox.push(Outgoing::Queue);
        } else if let Some(text) = self.op.text() {
            self.outbox.push(Outgoing::Update { text, button: None });
        }
        if !self.op.done {
            return;
        }
        if !self.op.ok {
            self.world_failed();
        } else if self.op.kind == OpKind::Connect {
            // `0x46bfdd`: the connect done, the authentication begins (`0x5aaba0`).
            self.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING);
        } else {
            // `0x46c00e`-`0x46c023`: state 0, the roster torn down, `charselect`, the queue ring
            // cleared. The screen's own `GetCharacterListUpdate` loads the roster.
            self.state = ScreenState::Idle;
            self.op = Operation::default();
            self.queue = QueueEstimate::default();
            self.outbox.push(Outgoing::Teardown);
            self.outbox.push(Outgoing::Screen("charselect"));
        }
    }

    /// State 3's frame (`0x46c03a`-`0x46c0a4`): the dialog re-texted, then a completed query's
    /// arm. A cancel tears the roster down (`0x46c069`) and raises the realm list
    /// (`RequestRealmList(1)`, `0x46c073`); any other failure opens `OKAY` with its text and goes
    /// to state 0 (`0x46c173`); a success goes to state 0, closes the dialog and loads the roster
    /// (`0x46c084`-`0x46c094`).
    fn tick_char_list(&mut self) {
        if let Some(text) = self.op.text() {
            self.outbox.push(Outgoing::Update { text, button: None });
        }
        if !self.op.done {
            return;
        }
        if !self.op.ok {
            if self.op.status == STATUS_CANCELLED {
                self.outbox.push(Outgoing::Teardown);
                self.request_realm_list(true);
            } else {
                let text = self.op.text();
                self.open("OKAY", text);
                self.state = ScreenState::Idle;
            }
            return;
        }
        self.state = ScreenState::Idle;
        self.outbox.push(Outgoing::Close);
        self.outbox.push(Outgoing::Roster);
    }

    /// `0x46b140`: a dialog for the logon's `(state, result)` when it differs from the pair last
    /// shown.
    fn show_login_dialog(&mut self) {
        let c = self.cells;
        if c.state == c.shown_state && c.result == c.shown_result {
            return;
        }
        self.cells.shown_state = c.state;
        self.cells.shown_result = c.result;
        if c.state == LOGIN_FAILED_STATE {
            let Some(key) = usize::try_from(c.result)
                .ok()
                .and_then(|r| LOGIN_RESULT_KEYS.get(r))
            else {
                return;
            };
            let which = if c.result == RESULT_LOGIN_FAILED {
                "CONNECTION_HELP"
            } else {
                "OKAY"
            };
            self.open(which, Some(Text::Key(key)));
            return;
        }
        // `0x46b1ae`-`0x46b1c7`: no dialog outside the table, for DISCONNECTED (11) or SURVEY (12).
        let Some(key) = usize::try_from(c.state)
            .ok()
            .and_then(|s| LOGIN_STATE_KEYS.get(s))
        else {
            return;
        };
        if c.state == 0x0b || c.state == 0x0c {
            return;
        }
        self.open("CANCEL", Some(Text::Key(key)));
    }
}

/// `SelectRealmByName` (`0x5ab6f0`): the first realm in the holder whose name matches, case
/// folded (`0x64a4c0`), whatever its flags.
fn select_by_name(holder: &[RealmInfo], name: &str) -> Option<RealmInfo> {
    holder
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))
        .cloned()
}

/// A realm record as the glue VM's holder takes it.
fn glue_realm(r: &RealmInfo) -> GlueRealm {
    GlueRealm {
        name: r.name.clone(),
        realm_type: r.realm_type,
        flags: r.flags,
        population: r.population,
        characters: r.characters,
        category: r.category,
    }
}

/// A world status's text and its code for the URL walk; a code past the table reads
/// `RESPONSE_FAILURE`.
fn status_text(code: u8) -> (Text, Option<u8>) {
    match STATUS_KEYS.get(usize::from(code)) {
        Some(key) => (Text::Key(key), Some(code)),
        None => (
            Text::Key(STATUS_KEYS[usize::from(STATUS_RESPONSE_FAILURE)]),
            Some(STATUS_RESPONSE_FAILURE),
        ),
    }
}

/// The native login's messages the manager reads, as one parameter.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct LoginNews<'w, 's> {
    stages: MessageReader<'w, 's, LoginStageMessage>,
    failures: MessageReader<'w, 's, LoginFailedMessage>,
    queued: MessageReader<'w, 's, LoginQueuedMessage>,
    rosters: MessageReader<'w, 's, CharListMessage>,
    world_auths: MessageReader<'w, 's, WorldAuthMessage>,
    roster_failures: MessageReader<'w, 's, RosterFailedMessage>,
    realm_lists: MessageReader<'w, 's, RealmListMessage>,
    disconnects: MessageReader<'w, 's, DisconnectedMessage>,
}

/// What the manager asks of the rest of the app.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GlueAsks<'w> {
    login: MessageWriter<'w, GlueLogin>,
    music: MessageWriter<'w, crate::sound::GlueMusicRequest>,
    realm: Option<Res<'w, crate::net::RealmChoice>>,
    abandon: Option<Res<'w, crate::net::LoginAbandon>>,
}

/// The CVars each agreement is read from and written to.
fn notice_cvar(notice: GlueNotice) -> &'static str {
    match notice {
        GlueNotice::Tos => "readTOS",
        GlueNotice::Eula => "readEULA",
        GlueNotice::Scanning => "readScanning",
        GlueNotice::Contest => "readContest",
    }
}

/// The four agreements' states off the CVar registry: the read `0x46a400` makes at `0x402b36`,
/// before the first glue screen is set (`0x402b65`), so a returning player's login screen never
/// sees them unaccepted.
pub(crate) fn read_agreements(cvars: &crate::cvars::Cvars) -> [NoticeState; 4] {
    let mut n = [NoticeState::default(); 4];
    for notice in GlueNotice::ALL {
        if let Some(value) = cvars.get(notice_cvar(notice)) {
            n[notice as usize] = NoticeState::from_cvar(value);
        }
    }
    n
}

/// The glue manager's frame: the VM's requests, the native login's news, the per-frame arms, then
/// the events into the glue VM and its slots seeded. Runs under `WOW_GLUE_STOCK=1` only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_glue_mgr(
    mut mgr: ResMut<GlueMgr>,
    script: Option<NonSendMut<UiScript>>,
    mut news: LoginNews,
    mut asks: GlueAsks,
    realms: Option<Res<crate::realm_select::Realms>>,
    mut realmlist: Option<ResMut<crate::realmlist::Realmlist>>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    create: Option<Res<crate::entities::CharCreate>>,
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
    creatures: Option<Res<crate::entities::Creatures>>,
    state: Option<Res<State<crate::char_select::ClientState>>>,
    time: Res<Time>,
) {
    let now_ms = time.elapsed().as_millis() as u32;
    let mut script = script.filter(|s| s.vm_kind() == VmKind::Glue);
    let in_world = state.is_some_and(|s| *s.get() == crate::char_select::ClientState::InWorld);
    let glue_up = script.is_some() && !in_world;

    // The agreements, read once (`0x46a400`) off the VM's CVar mirror once the sync has seeded
    // it.
    if mgr.notices.is_none() {
        if let Some(s) = script.as_deref() {
            let values: Option<Vec<String>> = GlueNotice::ALL
                .iter()
                .map(|n| s.cvar(notice_cvar(*n)))
                .collect();
            if let Some(values) = values {
                let mut n = [NoticeState::default(); 4];
                for (notice, value) in GlueNotice::ALL.iter().zip(&values) {
                    n[*notice as usize] = NoticeState::from_cvar(value);
                }
                mgr.notices = Some(n);
            }
        }
    }

    // A new glue VM is the glue build (`0x46a7b0`): state 0, its slots seeded, and the realm
    // list's C globals carried over, which outlive the Lua state.
    let mut fresh_vm = false;
    if let Some(s) = script.as_deref_mut() {
        if mgr.vm != Some(s.session()) {
            mgr.vm = Some(s.session());
            mgr.state = ScreenState::Idle;
            mgr.cells = LoginCells::default();
            mgr.auth_final = false;
            s.set_glue_saved_account(&crate::login::load_saved_account());
            s.set_glue_realm_cells(mgr.realm_cells);
            s.set_glue_select_cells(mgr.select_cells);
            mgr.holder_moved = true;
            fresh_vm = true;
        }
    } else if in_world {
        // In the world the glue is torn down, and the entry's second phase leaves state 0
        // (`0x46c252`).
        mgr.state = ScreenState::Idle;
    }
    if let (Some(s), Some(r)) = (script.as_deref_mut(), realms.as_ref()) {
        if fresh_vm || r.is_changed() {
            let (categories, types) = r.glue_setup();
            s.set_glue_realm_setup(categories, types);
        }
    }
    // The name a connect resolves (`0x5ab7d0`).
    let realm_name = script
        .as_deref()
        .and_then(|s| s.cvar(crate::realm_select::CVAR_REALM_NAME))
        .unwrap_or_default();

    // The verbs the last tick's Lua called.
    let mut logins = Vec::new();
    if let Some(s) = script.as_deref_mut() {
        mgr.screen = s.glue_screen();
        for request in s.take_glue_requests() {
            match request {
                GlueRequest::Login(c) => mgr.login(c.account, c.password, &mut logins),
                GlueRequest::StatusDialogClick => mgr.status_dialog_click(&mut logins),
                GlueRequest::SetSavedAccountName(name) => crate::login::save_account(&name),
                GlueRequest::Accept(notice) => {
                    let n = mgr.notices.get_or_insert([NoticeState::default(); 4]);
                    n[notice as usize] = NoticeState {
                        accepted: true,
                        show_notice: false,
                    };
                    // Through the VM's mirror, whose writes the CVar sync persists.
                    s.set_cvar_engine(notice_cvar(notice), "1");
                }
                GlueRequest::PlayGlueMusic(file) => {
                    asks.music.write(crate::sound::GlueMusicRequest::Play(file));
                }
                GlueRequest::StopGlueMusic => {
                    asks.music.write(crate::sound::GlueMusicRequest::Stop);
                }
                GlueRequest::DisconnectFromServer => {
                    // The disconnect handler zeroes the state (`0x46c55d`), the operation in
                    // flight with it, so no reply completes on the login screen.
                    if mgr.connected || mgr.state == ScreenState::Connect {
                        mgr.state = ScreenState::Idle;
                        mgr.op = Operation::default();
                    }
                    mgr.connected = false;
                    logins.push(GlueLogin::Disconnect);
                }
                GlueRequest::QuitGame => logins.push(GlueLogin::Quit),
                GlueRequest::RequestRealmList { dialog } => mgr.request_realm_list(dialog),
                GlueRequest::CancelRealmListQuery => mgr.cancel_realm_list_query(),
                // `CLoginMgr::Cancel` (`0x5b3320`): nothing else moves, so no dialog shows.
                GlueRequest::CancelLogon => logins.push(GlueLogin::Abandon),
                GlueRequest::ChangeRealm(name) => mgr.connect_to_realm(&name),
                // `0x46b880`; the VM cleared the roster in the verb.
                GlueRequest::GetCharacterList => mgr.get_character_list(),
            }
        }
        mgr.realm_cells = s.glue_realm_cells();
    }

    // The native login's news.
    for msg in news.stages.read() {
        mgr.on_stage(msg.stage, glue_up);
    }
    // The logon's list ends the logon, then answers the connect its latch begins.
    let lists: Vec<(Vec<RealmInfo>, bool)> = news
        .realm_lists
        .read()
        .map(|m| (m.realms.clone(), m.failed))
        .collect();
    if !lists.is_empty() {
        mgr.on_logon_done();
    }
    for msg in news.queued.read() {
        mgr.on_queued(msg.position, msg.realm.clone(), now_ms);
    }
    // An authenticated logon moves to the world connect before the world's own news.
    if mgr.auth_final {
        mgr.tick();
    }
    for (realms, failed) in lists {
        mgr.on_realm_list(realms, failed, &realm_name);
    }
    for msg in news.failures.read() {
        mgr.on_failed(msg);
    }
    // A session of an attempt a Cancel or a disconnect has passed is not the screen's.
    let generation = crate::net::LoginAbandon::now(asks.abandon.as_deref());
    for msg in news.world_auths.read() {
        if msg.generation == generation {
            mgr.on_world_authenticated(msg.realm.clone(), msg.epoch);
        }
    }
    for msg in news.roster_failures.read() {
        mgr.on_roster_failed(msg.epoch);
    }
    for msg in news.rosters.read() {
        mgr.on_roster(msg.characters.clone(), msg.cause, msg.epoch);
    }
    for msg in news.disconnects.read() {
        mgr.on_disconnected(msg.end, msg.session_over);
    }
    mgr.tick();
    send_asks(&mut mgr, logins, &mut asks);

    let Some(mut script) = script else {
        return;
    };
    // The realmlist control's typed value that was not an address reopens its dialog.
    if let Some(typed) = realmlist.as_deref_mut().and_then(|r| r.take_refused()) {
        mgr.outbox.push(Outgoing::Open {
            which: "BENILLA_REALMLIST",
            text: Some(Text::Raw(crate::login::REALMLIST_BAD.to_string())),
            data: Some(typed),
        });
    }
    seed(
        &mut mgr,
        &mut script,
        realms.as_deref(),
        realmlist.as_deref(),
    );
    let names = RosterNames {
        areas: areas.as_deref().map(|a| &a.0),
        create: create.as_deref().map(|c| &c.0),
        classes: classes.as_deref().map(|c| &c.0),
        creatures: creatures.as_deref(),
    };
    flush(&mut mgr, &mut script, now_ms, &names);
    mgr.select_cells = script.glue_select_cells();
    // What the outbox asked in turn (the roster load's realm list).
    send_asks(&mut mgr, Vec::new(), &mut asks);
}

/// Send the asks of the frame: the login engine's, and the park requests in the order they were
/// made, the realm requests stamped with the abandon generation as they go.
fn send_asks(mgr: &mut GlueMgr, logins: Vec<GlueLogin>, asks: &mut GlueAsks) {
    let (arms, park_asks) = mgr.take_asks();
    for ask in logins.into_iter().chain(arms) {
        asks.login.write(ask);
    }
    let now = crate::net::LoginAbandon::now(asks.abandon.as_deref());
    if let Some(choice) = asks.realm.as_deref() {
        for mut ask in park_asks {
            if let ParkRequest::Realm(RealmRequest::Enter { generation, .. }) = &mut ask {
                *generation = now;
            }
            let _ = choice.0.send_request(ask);
        }
    }
}

/// The DBC catalogs the roster's strings come from.
struct RosterNames<'a> {
    areas: Option<&'a benilla_formats::AreaTableCatalog>,
    create: Option<&'a benilla_formats::CharCreateCatalog>,
    classes: Option<&'a benilla_formats::ChrClasses>,
    creatures: Option<&'a crate::entities::Creatures>,
}

/// A roster entry as the glue VM's records take it (`0x4732a0`). The race and class names are
/// the `Name_Lang` of the race and class rows (`ChrRaces` `+0x44`, `ChrClasses` `+0x14`) at the
/// enUS slot (the one locale benilla reads, #429), the file string the race row's (`ChrRaces` column 15), and the model flag
/// whether the race row's body for the gender resolves to a model. A race, class or zone with no row reads
/// empty, empty, nil as the reference's does, and so does one whose catalog did not load.
fn glue_character(c: &Character, names: &RosterNames) -> GlueCharacter {
    let text = |s: Option<&str>| s.unwrap_or_default().to_string();
    GlueCharacter {
        guid: c.guid,
        name: c.name.clone(),
        race: text(names.create.and_then(|r| r.race_name(c.race))),
        class: text(names.classes.and_then(|k| k.name(u32::from(c.class)))),
        level: c.level,
        zone: names.areas.and_then(|a| a.name(c.zone)).map(str::to_string),
        file_string: text(names.create.and_then(|r| r.race_file(c.race))),
        gender: c.gender,
        flags: c.flags,
        // `0x5ec480`: the race row's display for the gender, then its model rows.
        model: names
            .create
            .and_then(|r| r.body_display(c.race, c.gender))
            .is_some_and(|display| names.creatures.is_some_and(|k| k.resolves_model(display))),
    }
}

/// The glue VM's slots, from the manager's state.
fn seed(
    mgr: &mut GlueMgr,
    script: &mut UiScript,
    realms: Option<&crate::realm_select::Realms>,
    realmlist: Option<&crate::realmlist::Realmlist>,
) {
    for notice in GlueNotice::ALL {
        let want = mgr.notice(notice);
        if script.glue_notice(notice) != want {
            script.set_glue_notice(notice, want);
        }
    }
    script.set_glue_connected(mgr.connected);
    if std::mem::take(&mut mgr.cells_moved) {
        script.set_glue_realm_cells(mgr.realm_cells);
    }
    if std::mem::take(&mut mgr.holder_moved) {
        script.set_glue_realms(mgr.holder.iter().map(glue_realm).collect());
    }
    script.set_glue_realm_characters(mgr.characters);
    // `realmName` names the realm the session dials (`0x5ab7b0` writes it on the connect); the
    // flags come off the connected realm's `Cfg_Configs` row.
    let (name, pvp, rp) = match &mgr.realm {
        Some(r) if mgr.connected => {
            let (pvp, rp) = realms.map_or((false, false), |x| x.pvp_rp(r.realm_type));
            (r.name.clone(), pvp, rp)
        }
        _ => (
            script
                .cvar(crate::realm_select::CVAR_REALM_NAME)
                .unwrap_or_default(),
            false,
            false,
        ),
    };
    script.set_glue_server(GlueServer { name, pvp, rp });
    if let Some(r) = realmlist {
        seed_realmlist(script, r);
    }
}

/// The realmlist control's two globals (`AccountLoginRealmlist.xml`): the address the next logon
/// dials and whether `$WOW_HOST` fixes it for the session.
fn seed_realmlist(script: &UiScript, realmlist: &crate::realmlist::Realmlist) {
    let g = script.lua().globals();
    let address = realmlist.address();
    if g.get::<Option<String>>("BENILLA_REALMLIST")
        .ok()
        .flatten()
        .as_deref()
        != Some(address)
    {
        let _ = g.set("BENILLA_REALMLIST", address);
    }
    let pinned = realmlist.pinned_by_env().then_some(1);
    if g.get::<Option<i64>>("BENILLA_REALMLIST_PINNED")
        .ok()
        .flatten()
        != pinned
    {
        let _ = g.set("BENILLA_REALMLIST_PINNED", pinned);
    }
}

/// Fire the outbox into the glue VM, the texts resolved through its own strings (`0x703bf0`).
fn flush(mgr: &mut GlueMgr, script: &mut UiScript, now_ms: u32, names: &RosterNames) {
    // The roster load can queue more (the realm list's dialog), which goes out in turn.
    while !mgr.outbox.is_empty() {
        for out in std::mem::take(&mut mgr.outbox) {
            flush_one(mgr, script, now_ms, names, out);
        }
    }
}

fn flush_one(
    mgr: &mut GlueMgr,
    script: &mut UiScript,
    now_ms: u32,
    names: &RosterNames,
    out: Outgoing,
) {
    let text = |script: &UiScript, t: &Text| match t {
        Text::Key(k) => script.global_text(k),
        Text::Raw(s) => s.clone(),
    };
    match out {
        Outgoing::Open {
            which,
            text: t,
            data,
        } => {
            let mut args = vec![ScriptValue::Str(which.to_string())];
            if t.is_some() || data.is_some() {
                args.push(t.map_or(ScriptValue::Nil, |t| ScriptValue::Str(text(script, &t))));
            }
            if let Some(d) = data {
                args.push(ScriptValue::Str(d));
            }
            script.fire_event("OPEN_STATUS_DIALOG", args);
        }
        Outgoing::Update { text: t, button } => {
            let mut args = vec![ScriptValue::Str(text(script, &t))];
            if let Some(b) = button {
                args.push(ScriptValue::Str(text(script, &b)));
            }
            script.fire_event("UPDATE_STATUS_DIALOG", args);
        }
        Outgoing::Queue => {
            let body = mgr
                .queue
                .text_by(now_ms, mgr.queue_realm.as_deref(), |key| {
                    script.global_text(key)
                });
            let button = script.global_text("CHANGE_REALM");
            script.fire_event(
                "UPDATE_STATUS_DIALOG",
                vec![ScriptValue::Str(body), ScriptValue::Str(button)],
            );
        }
        Outgoing::Screen(name) => {
            script.fire_event("SET_GLUE_SCREEN", vec![ScriptValue::Str(name.to_string())]);
        }
        Outgoing::Disconnected => script.fire_event("DISCONNECTED_FROM_SERVER", Vec::new()),
        Outgoing::Close => script.fire_event("CLOSE_STATUS_DIALOG", Vec::new()),
        Outgoing::Rebuild => {
            script.rebuild_glue_realms();
            mgr.realm_cells = script.glue_realm_cells();
        }
        Outgoing::Teardown => script.teardown_glue_roster(),
        Outgoing::Roster => {
            let roster = mgr
                .roster
                .iter()
                .map(|c| glue_character(c, names))
                .collect();
            // `RequestRealmList(1)` at `0x47254b`, when no realm has been chosen.
            if script.load_glue_roster(roster) {
                mgr.request_realm_list(true);
            }
            mgr.realm_cells = script.glue_realm_cells();
        }
    }
}

#[cfg(test)]
#[path = "glue_mgr_tests.rs"]
mod tests;
