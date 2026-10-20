//! The glue manager, `CGlueMgr` host side, under the stock glue screens: the screen-state machine of
//! `CGlueMgr::Update` (`0x46b930`, `screenState` at `0xb41da0`) over the login engine. The IO
//! thread's parks and [`crate::glue_session::login`]'s policy still do the network; this maps their
//! messages onto the reference's screen states and fires the glue events the stock GlueXML answers:
//! the status dialog (`OPEN_STATUS_DIALOG`, `UPDATE_STATUS_DIALOG`, `CLOSE_STATUS_DIALOG`),
//! `SET_GLUE_SCREEN` and `DISCONNECTED_FROM_SERVER`. Its verbs run inside their Lua call, as the
//! reference's call `CGlueMgr` directly: the glue VM it seats holds it as its
//! [`benilla_ui::script::GlueHost`] ([`SharedGlueMgr`]), so a verb's screen state, its dialog and
//! its other events land before the call returns, and only the network half it starts waits for
//! this frame's sends, as `CGlueMgr::Update` polls the operation each frame.
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
//!    `REALM_LIST_REALM_NOT_FOUND`. A query with no answer fails it `REALM_LIST_FAILED`. A run
//!    that answers the list itself ([`RunModeRealm`]) selects its realm there instead: `WOW_REALM`
//!    ahead of `realmName`, the unattended pick behind it ([`GlueMgr::connect_realm`]).
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
//! 4. `ChangeRealm` reaches `ConnectToRealm` (`0x46b210`): the realm selected, then with no world
//!    link state 2, the `CANCEL` dialog `GAME_SERVER_LOGIN` and the connect, in the call; with one,
//!    the link dropped in the call and the connect begun by the drop's handler (`0x46c585`).
//!    `RealmListDialogCancelled` closes the realmd link on the login screen only;
//!    `CancelRealmListQuery` cancels state 4 only.
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
//! The delete and the rename, the select screen's two writes:
//!
//! 1. `DeleteCharacter` reaches `0x46b2e0`: state 6, the `CANCEL` dialog `CHAR_DELETE_IN_PROGRESS`
//!    and the delete (`0x5ab100`: operation 7 at status `0x38`, then `CMSG_CHAR_DELETE`). The reply
//!    is the status (`0x5b45c0` → `0x5ab0e0`, success on `0x39`). State 6's frame (`0x46c14e`)
//!    re-texts the dialog and on a success fires `SELECT_FIRST_CHARACTER` and asks for the list
//!    again (`0x46c19d`-`0x46c1a7`, `0x46b880`); a failure opens `OKAY` with the status's text and
//!    goes to state 0 (`0x46c173`-`0x46c188`).
//! 2. `RenameCharacter`'s passing name reaches the send at `0x46b39b`: state 7, the `CANCEL` dialog
//!    `CHAR_RENAME_IN_PROGRESS` and the `CMSG_CHAR_RENAME`. State 7 has no frame arm; the reply's
//!    handler (`0x46b440`) closes the dialog and leaves the state as it is. A result of 0 renames
//!    the record (`0x4725f0`): the character's files move to the new name
//!    ([`crate::local_state::move_character_state`]), the VM's record takes it and loses the
//!    rename flag; `0x31` fires `FORCE_RENAME_CHARACTER` with `CHAR_CREATE_NAME_IN_USE`, any other
//!    result with `CHAR_RENAME_FAILED`.
//!
//! The dialog's button in state 6 cancels the operation and goes to state 0 (`0x46b776`), in state
//! 7 it goes to state 0 alone (`0x46b79d`); neither sends anything.
//!
//! The world entry and its refusal:
//!
//! 1. `EnterWorld` reaches `CGlueMgr::EnterWorld` (`0x46b500`) with the selected record. Three
//!    guards return having done nothing: no realm selected this process (`0x5ab9a0`), the
//!    selection off the roster (the VM's, `0x46b53e`), no world link (`[svc+0x1b00]`, `0x46b56c`).
//!    A record flagged `0x4` (`+0xfc`, locked for a transfer, `0x46b57e`) opens `OKAY` with status
//!    `0x44`'s text, `CHAR_LOGIN_LOCKED_FOR_TRANSFER` (`0x46b589`-`0x46b5a5`); one flagged `0x4000`
//!    fires `FORCE_RENAME_CHARACTER` with `CHAR_RENAME_DESCRIPTION` (`0x46b5c2`), which the stock
//!    screen answers with its rename dialog. Either returns there. Otherwise `lastCharacterIndex`
//!    takes the selection's index (`0x46b5fa`), the CVars are saved (`0x46b6f6`) and the state
//!    goes to 8 (`0x46b711`). The record byte the call sets on the way (`+0x118`, `0x46b54a`) is
//!    read nowhere.
//! 2. The rest is the world entry by the session's pick
//!    ([`crate::glue_session::roster::send_pick`]): the loading screen and its tip off the picked
//!    record (`0x406640`, `0x46b614`-`0x46b6e3`), the realmd link closed (`0x46b70c`), the glue
//!    torn down (`0x46ac90`, the VM leaving with the character screen) and `CMSG_PLAYER_LOGIN`
//!    (`0x46c272`), sent here as the call returns rather than once the world has loaded (the
//!    `Deviation:` in [`crate::net::io`]). State 8 has no frame arm here and ends with the glue, as
//!    the entry's second phase ends it (`0x46c252`); its dialog button cancels to state 0
//!    (`0x46b776`).
//! 3. `SMSG_CHARACTER_LOGIN_FAILED` (`0x5aad70`) tears the world down and rebuilds the glue
//!    (`0x401ee0`), setting state 9 before the `charselect` screen goes up (`0x402075`), so the
//!    screen's `GetCharacterListUpdate` asks nothing (`0x46b880`'s first test); the login operation
//!    then completes failed with the refusal's status (table `0x5aae08`, `0x5aaa50`). State 9's
//!    frame (`0x46c283`) opens `OKAY` with its text; the button (`0x46b767`), which the stock
//!    `OKAY` type's `OnShow` presses at once, goes to state 0 and asks for the list again on the
//!    same link (`0x46b880`).
//!
//! The create:
//!
//! 1. `CreateCharacter`'s passing name reaches `0x46b290`: state 5, the `CANCEL` dialog
//!    `CHAR_CREATE_IN_PROGRESS` and the create (`0x5aac50`: operation 5 at status `0x2d`, then
//!    `CMSG_CHAR_CREATE`; with no link it fails `RESPONSE_FAILED_TO_CONNECT` at once). The reply is
//!    the status (`0x5aac30`, success on `0x2e`).
//! 2. State 5's frame (`0x46c0f3`) re-texts the dialog and acts on a completed create: a success
//!    goes to state 0, closes the dialog, fires `SELECT_LAST_CHARACTER` and puts `charselect` up
//!    (`0x46c11e`-`0x46c13d`), whose `OnShow` asks for the list again and whose list update then
//!    selects the last row; a failure opens `OKAY` with the status's text and goes to state 0
//!    (`0x46c118` → `0x46c177`). The dialog's button in state 5 cancels the operation and goes to
//!    state 0 (`0x46b776`), sending nothing.
//!
//! The login engine reports a logon as one blocking step, so of the logon's states only
//! `CONNECTING` and `AUTHENTICATED` are seen; its refusals map through the reference's own
//! byte-to-result table ([`realmd_result`]).

use bevy::prelude::*;

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use benilla_protocol::{Character, LoginRefusal, LoginStage, RealmInfo, RosterCause};
use benilla_ui::script::{
    GlueCall, GlueCharacter, GlueHost, GlueNotice, GlueRealm, GlueRealmCells, GlueRequest,
    GlueSelectCells, GlueSignal, GlueText as Text, NoticeState, ScriptValue, UiScript, VmKind,
    GLUE_STATUS_KEYS,
};

use crate::glue_session::login::{GlueLogin, LoginGuardRefused};
use crate::glue_session::queue::QueueEstimate;
use crate::glue_session::realm::RunModeRealm;
use crate::net::{
    AddonInfoMessage, AddonInfoReply, CharActionResultMessage, CharListMessage, CharRenameMessage,
    CharRequest, CharacterLoginFailedMessage, DisconnectedMessage, LoginFailedMessage,
    LoginQueuedMessage, LoginStageMessage, ParkRequest, RealmListMessage, RealmRequest,
    RosterFailedMessage, WorldAuthMessage,
};

/// `CGlueMgr`'s `screenState` (`0xb41da0`), named by the functions that set each value: the
/// states the login, the realm list, the character list, the create, the delete, the rename and
/// the world entry reach. The reference's others are the patch and survey downloads (10 and 11,
/// `0x473c40`, `0x473da8`), each set by a verb these screens do not have yet.
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
    /// A character create (`0x46b29e`).
    Create = 5,
    /// A character delete (`0x46b2f8`).
    Delete = 6,
    /// A character rename (`0x46b3a4`).
    Rename = 7,
    /// The world entry (`CGlueMgr::EnterWorld`, `0x46b711`), until the world is up.
    EnterWorld = 8,
    /// A refused character login (`0x402075`, inside `0x401ee0` from `0x5aad70`), until the
    /// status dialog's button (`0x46b767`).
    LoginRefused = 9,
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
const STATUS_CHAR_CREATE_IN_PROGRESS: u8 = 0x2d;
const STATUS_CHAR_CREATE_SUCCESS: u8 = 0x2e;
const STATUS_CHAR_DELETE_IN_PROGRESS: u8 = 0x38;
const STATUS_CHAR_DELETE_SUCCESS: u8 = 0x39;
const STATUS_CHAR_LOGIN_IN_PROGRESS: u8 = 0x3c;
const STATUS_CHAR_LOGIN_FAILED: u8 = 0x41;
const STATUS_CHAR_LOGIN_LOCKED_FOR_TRANSFER: u8 = 0x44;

/// The record flags `EnterWorld` tests (`+0xfc`): locked for a paid transfer (`0x46b57e`) and
/// flagged for a rename (`0x46b5b1`).
const FLAG_LOCKED_FOR_TRANSFER: u32 = 0x4;
const FLAG_RENAME: u32 = benilla_protocol::CHARACTER_FLAG_RENAME;

/// `SMSG_CHARACTER_LOGIN_FAILED`'s byte as a status (`0x5aad70`, jump table `0x5aae08`): a 1-based
/// index onto `0x3e 0x3f 0x40 0x42 0x43 0x44`, every other byte the default arm's
/// `CHAR_LOGIN_FAILED` (`0x5aadf7`).
fn char_login_status(result: u8) -> u8 {
    match result {
        1..=3 => 0x3d + result,
        4..=6 => 0x3e + result,
        _ => STATUS_CHAR_LOGIN_FAILED,
    }
}

/// `SMSG_CHAR_RENAME`'s one result the handler tells apart (`0x46b498 cmp al,0x31`).
const RENAME_NAME_IN_USE: u8 = 0x31;

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
    /// A character create (5, `0x5aac50`).
    Create,
    /// A character delete (7, `0x5ab100`).
    Delete,
    /// The character login (8, `COP_LOGIN_CHARACTER`, begun at `0x5aae20`).
    Login,
}

/// The operation record (`BeginOperation` `0x5aa400`, `SetStatus` `0x5aa920`): its status code
/// (`[svc+0x1b14]`), completed (`[svc+0x1b08]`) and succeeded (`[svc+0x1b0c]`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Operation {
    kind: OpKind,
    status: u8,
    done: bool,
    ok: bool,
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
        self.done.then(|| status_text(self.status).0)
    }
}

/// What follows a signal in the reference, run once the signal's handlers have: a verb's
/// operation begun and its request asked (`0x46b8b7`, `0x46b2cd`, `0x46b328`, `0x46b902`,
/// `0x46b274`, the logon at `0x46b05c`), and the state writes an `Update` arm makes after its
/// re-text and its dialogs. So a handler that presses the dialog's button acts on the state and
/// the operation as they stood when the signal was raised. The arm's decisions are the ones it
/// made off the status it read first (`0x46ba5a`), carried in the step.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// The logon, `0x5ab4b0`.
    Logon { account: String, password: Secret },
    /// The character list query, `0x5aabb0`.
    CharList,
    /// The create, `0x5aac50`.
    Create(benilla_ui::script::GlueCreateRequest),
    /// The delete, `0x5ab100`.
    Delete(u64),
    /// The `CMSG_CHAR_RENAME`, `0x5ab630`.
    Rename { guid: u64, name: String },
    /// The realm list query, `0x5ab840`.
    RealmList,
    /// The world connect, `0x5aab20`.
    Connect,
    /// The screen state, written.
    State(ScreenState),
    /// The logon arm past its dialog, which reads the latch (`0x46ba85`) and the login state
    /// (`0x46baaa`) as they stand once the dialog's handlers have run: authenticated, state 0 and
    /// then `0x46b1f0`'s connect at `CSTATUS_CONNECTING` (`0x5aab4a`), its ring cleared
    /// (`0x5aab30`), with no dialog of its own, the logon's list being its query's reply; failed,
    /// the logon cancelled and state 0 (`0x46bbbf`-`0x46bbcb`). The patch state (`0x46bb64`)
    /// waits on a downloader benilla does not have; its dialog's Cancel ends it.
    LogonPastDialog,
    /// The connect done, the authentication begins (`0x46bfdd` → `0x5aaba0`).
    BeginAuth,
    /// The authentication's success (`0x46c00e`): state 0.
    Authenticated,
    /// The queue position cleared after the character screen is up (`0x46c023`). The reference
    /// zeroes `pos[0]` alone; the whole ring is cleared here, which is neutral: it is read only in
    /// state 2, and every connect clears it first (`0x5aab30`).
    ClearQueue,
    /// State 2's failure arm past its dialog, with the status it read (`0x46bef5`-`0x46bfc8`).
    WorldFailed(u8),
    /// The failure arm past its teardown (`0x46bf24`-`0x46bf96`): the realm list raised over the
    /// screen, or with no realm held asked for (`0x46c06e`).
    RaiseRealmList,
    /// `RequestRealmList(1)` (`0x46b8d0`), as an arm calls it.
    RequestRealmList,
    /// `0x46b880`, as the delete's success calls it (`0x46c1a7`).
    GetCharacterList,
}

/// A password in a step, which never prints.
#[derive(Clone, PartialEq, Eq)]
struct Secret(String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// One glue event the manager fires into the glue VM, or its own step that follows one.
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
    /// `SET_GLUE_SCREEN(name)` when the current screen is `current`, read as it stands.
    ScreenFrom {
        current: &'static str,
        name: &'static str,
    },
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
    /// `SHOW_SERVER_ALERT(text)` (event 21, `0x46b964`): the alert's text after its prefix.
    ServerAlert(String),
    /// `SELECT_FIRST_CHARACTER` (event 13, `0x46c1a2`).
    SelectFirst,
    /// `SELECT_LAST_CHARACTER` (event 12, `0x46c133`).
    SelectLast,
    /// `FORCE_RENAME_CHARACTER(key)` (event 23, `0x46b4a8`, `0x46b4cd`): the key, unresolved.
    ForceRename(&'static str),
    /// A CVar the manager set.
    Cvar { name: &'static str, value: String },
    /// The manager's step that follows the signals before it ([`Step`]).
    Begin(Step),
    /// The rename applier (`0x4725f0`) for this guid and name.
    Renamed { guid: u64, name: String },
}

impl Outgoing {
    /// The signal this is, for the VM to fire ([`UiScript::fire_glue_signal`]); the four only
    /// this frame can fire, off what the manager holds, come back as they are.
    fn into_signal(self) -> Result<GlueSignal, Outgoing> {
        Ok(match self {
            Outgoing::Open { which, text, data } => GlueSignal::Open { which, text, data },
            Outgoing::Update { text, button } => GlueSignal::Update { text, button },
            Outgoing::Screen(name) => GlueSignal::Screen(name),
            Outgoing::ScreenFrom { current, name } => GlueSignal::ScreenFrom { current, name },
            Outgoing::Disconnected => GlueSignal::Disconnected,
            Outgoing::Close => GlueSignal::Close,
            Outgoing::Teardown => GlueSignal::Teardown,
            Outgoing::ServerAlert(text) => GlueSignal::ServerAlert(text),
            Outgoing::SelectFirst => GlueSignal::SelectFirst,
            Outgoing::SelectLast => GlueSignal::SelectLast,
            Outgoing::ForceRename(key) => GlueSignal::ForceRename(key),
            Outgoing::Cvar { name, value } => GlueSignal::Cvar { name, value },
            out @ (Outgoing::Begin(_)
            | Outgoing::Queue
            | Outgoing::Rebuild
            | Outgoing::Roster
            | Outgoing::Renamed { .. }) => return Err(out),
        })
    }
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

/// The host side of `CGlueMgr`, and the `ClientServices` state its functions read: one object,
/// which the app's frame ([`drive_glue_mgr`]) and the seated glue VM's verbs ([`GlueHost`]) both
/// run in, through [`SharedGlueMgr`].
#[derive(Default)]
pub(crate) struct GlueMgr {
    pub(crate) state: ScreenState,
    cells: LoginCells,
    /// Authenticated, waiting for the next tick (`[0xb41e18]`).
    auth_final: bool,
    /// The world connection is up (`[svc+0x1b00]`).
    connected: bool,
    /// `ChangeRealm` dropped a live world link to connect to another realm (`[0xb41d9c]`, set at
    /// `0x46b235`): the connect waits for the drop's handler (`0x46c56f`-`0x46c5bf`).
    reconnect: bool,
    /// State 2's operation.
    op: Operation,
    queue: QueueEstimate,
    queue_realm: Option<String>,
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
    /// The agreements' flag cells (`[0xb41df8]`-`[0xb41e14]`), read off their CVars once, when the
    /// first glue VM comes up (`0x46a400`), and seated in each VM after it.
    pub(super) notices: Option<benilla_ui::script::GlueNotices>,
    /// The server alert's fetch while it runs (`alertPending`, `[0xb41e28]`), started once per
    /// process at the glue init.
    alert_fetch: Option<crossbeam_channel::Receiver<Option<Vec<u8>>>>,
    /// The text a finished fetch left for the next frame (`alertReady`, `[0xb41e2c]`, and the
    /// buffer `0xb414f8` past its prefix).
    alert_ready: Option<String>,
    /// A refused character login's status, waiting for the glue rebuild that sets state 9.
    refused: Option<u8>,
    /// The index `EnterWorld` writes to `lastCharacterIndex`, for the host's CVar registry.
    last_character: Option<i32>,
    /// The steps a verb's call left at points of its fan-out ([`GlueSignal::Resume`]), by number.
    resumes: std::collections::HashMap<u32, Step>,
    /// The next such number.
    next_resume: u32,
    /// A world authentication that came inside the logon's frame, waiting on the logon arm's
    /// steps.
    auth_waits: bool,
}

impl GlueMgr {
    /// The record the last character list reply carried for `guid`: what the scene drive builds
    /// the selected character from (`0x472950` reads the select record's own copy).
    pub(crate) fn roster_character(&self, guid: u64) -> Option<&Character> {
        self.roster.iter().find(|c| c.guid == guid)
    }

    /// The alert fetch the glue init began (`0x46a564`).
    pub(super) fn set_alert_fetch(&mut self, fetch: crossbeam_channel::Receiver<Option<Vec<u8>>>) {
        self.alert_fetch = Some(fetch);
    }

    /// A finished fetch's alert becomes ready (`0x46a5f1`); one that found none, or died, ends the
    /// wait (`0x46a5fb`).
    fn poll_alert(&mut self) {
        let Some(fetch) = &self.alert_fetch else {
            return;
        };
        match fetch.try_recv() {
            Ok(alert) => {
                self.alert_fetch = None;
                self.alert_ready = alert.map(|text| String::from_utf8_lossy(&text).into_owned());
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => self.alert_fetch = None,
        }
    }

    /// The top of `CGlueMgr::Update` (`0x46b939`-`0x46b96c`): a ready alert first runs the status
    /// dialog's button (`0x46b947` → `0x46b720`, the screen state's cancel), fires
    /// `SHOW_SERVER_ALERT` with the text, and is spent.
    fn show_alert(&mut self) {
        if let Some(text) = self.alert_ready.take() {
            self.status_dialog_click();
            self.outbox.push(Outgoing::ServerAlert(text));
        }
    }

    /// One of the manager's functions a glue verb calls, run as the verb's own call does: in place
    /// through [`GlueHost`], or for a call made before this manager seated its VM, on this frame.
    fn call(&mut self, call: GlueCall) {
        match call {
            GlueCall::Login(c) => self.login(c.account, c.password),
            GlueCall::StatusDialogClick => self.status_dialog_click(),
            GlueCall::DisconnectFromServer => self.disconnect_from_server(),
            GlueCall::QuitGame => self.login_out.push(GlueLogin::Quit),
            GlueCall::RequestRealmList { dialog } => self.request_realm_list(dialog),
            GlueCall::CancelRealmListQuery => self.cancel_realm_list_query(),
            // `CLoginMgr::Cancel` (`0x5b3320`): nothing else moves, so no dialog shows.
            GlueCall::CancelLogon => self.login_out.push(GlueLogin::Abandon),
            GlueCall::ChangeRealm(name) => self.connect_to_realm(&name),
            // `0x46b880`; the VM cleared the roster in the verb.
            GlueCall::GetCharacterList => self.get_character_list(),
            GlueCall::DeleteCharacter(guid) => self.delete_character(guid),
            GlueCall::RenameCharacter { guid, name } => self.rename_character(guid, name),
            GlueCall::EnterWorld { index, guid, flags } => self.enter_world(index, guid, flags),
            GlueCall::CreateCharacter(request) => self.create_character(request),
        }
    }

    /// `DisconnectFromServer` (`0x46d340`): with the world link up, its flag goes down in the call
    /// (`0x5ab1a8`) and the link is dropped; then, link or not, the logon is cancelled
    /// (`0x46d36c`). Only a real drop reaches the handler (`0x46c540`), which zeroes the state
    /// (`0x46c55d`), the operation in flight with it, so no reply completes on the login screen;
    /// the IO thread reports no such drop, so here the handler's half runs with the drop.
    fn disconnect_from_server(&mut self) {
        if self.connected {
            self.connected = false;
            self.state = ScreenState::Idle;
            self.op = Operation::default();
        }
        self.login_out.push(GlueLogin::Disconnect);
    }

    fn open(&mut self, which: &'static str, text: Option<Text>) {
        self.outbox.push(Outgoing::Open {
            which,
            text,
            data: None,
        });
    }

    /// `CGlueMgr::Login` (`0x46afb0`) past its first two tests, the agreements, which the VM that
    /// holds them made: the idle test (`0x46afcd`), then the dialogs and the logon.
    fn login(&mut self, account: String, password: String) {
        if self.state != ScreenState::Idle {
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
        self.outbox.push(Outgoing::Begin(Step::Logon {
            account,
            password: Secret(password),
        }));
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
    fn status_dialog_click(&mut self) {
        match self.state {
            // `0x46b731`: the pending operation's callback; none is pending outside an operation.
            ScreenState::Idle => {}
            // `0x46b73d`: the logon cancelled, state 0. A logon still under way is abandoned; one
            // that failed has no link left to close (`0x5b3320` tests `[login+5]`), so the press
            // its failure dialog's `OnShow` makes closes only what may stand, the credentials and
            // the paced resubmit kept.
            ScreenState::Logon => {
                self.login_out
                    .push(if self.cells.state == LOGIN_FAILED_STATE {
                        GlueLogin::CloseLink
                    } else {
                        GlueLogin::Abandon
                    });
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
            // `0x46b776` (states 4, 5, 6 and 8): the operation cancelled, state 0, nothing sent.
            ScreenState::RealmList
            | ScreenState::Create
            | ScreenState::Delete
            | ScreenState::EnterWorld => {
                self.op.complete(false, STATUS_CANCELLED);
                self.state = ScreenState::Idle;
            }
            // `0x46b79d`: state 0 alone; the rename's reply still lands (`0x46b440`).
            ScreenState::Rename => self.state = ScreenState::Idle,
            // `0x46b767`: state 0, then the list asked for again on the same link (`0x46b880`).
            ScreenState::LoginRefused => {
                self.state = ScreenState::Idle;
                self.get_character_list();
            }
        }
    }

    /// The asks made of the login engine and of the IO thread since the last send, by the verbs'
    /// calls and the frame's arms, each list in the order made.
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
    /// any of it.
    fn get_character_list(&mut self) {
        if self.state == ScreenState::LoginRefused {
            return;
        }
        self.state = ScreenState::CharList;
        self.open("CANCEL", Some(Text::Key("CHAR_LIST_RETRIEVING")));
        self.outbox.push(Outgoing::Begin(Step::CharList));
    }

    /// `0x46b290`, `CreateCharacter`'s tail past the name check: state 5, the
    /// `CHAR_CREATE_IN_PROGRESS` dialog (`0x46b2c4`) and the create (`0x5aac50`: operation 5 at
    /// status `0x2d`, then `CMSG_CHAR_CREATE` with the name and the nine bytes). With no link it
    /// fails `RESPONSE_FAILED_TO_CONNECT` at once (`0x5aac72`). No guard on the state: a second
    /// call sends again.
    fn create_character(&mut self, request: benilla_ui::script::GlueCreateRequest) {
        self.state = ScreenState::Create;
        self.open("CANCEL", Some(Text::Key("CHAR_CREATE_IN_PROGRESS")));
        self.outbox.push(Outgoing::Begin(Step::Create(request)));
    }

    /// `SMSG_CHAR_CREATE` (`0x5aac30`): the result byte is the create's status, success on
    /// `0x2e`, whatever the screen state (`SetStatus`, `0x5aa920`).
    fn on_char_created(&mut self, code: u8) {
        if self.op.kind == OpKind::Create {
            self.op.complete(code == STATUS_CHAR_CREATE_SUCCESS, code);
        }
    }

    /// `0x46b2e0`, `DeleteCharacter`'s tail: state 6, the `CHAR_DELETE_IN_PROGRESS` dialog
    /// (`0x46b31e`) and the delete (`0x5ab100`: operation 7 at status `0x38`, then
    /// `CMSG_CHAR_DELETE`). With no link it fails `RESPONSE_FAILED_TO_CONNECT` at once
    /// (`0x5ab11e`).
    fn delete_character(&mut self, guid: u64) {
        self.state = ScreenState::Delete;
        self.open("CANCEL", Some(Text::Key("CHAR_DELETE_IN_PROGRESS")));
        self.outbox.push(Outgoing::Begin(Step::Delete(guid)));
    }

    /// `0x46b340` past the name check (`0x46b39b`): state 7, the `CHAR_RENAME_IN_PROGRESS` dialog
    /// and the `CMSG_CHAR_RENAME`, which `0x5ab630` sends only on a link that is up.
    fn rename_character(&mut self, guid: u64, name: String) {
        self.state = ScreenState::Rename;
        self.open("CANCEL", Some(Text::Key("CHAR_RENAME_IN_PROGRESS")));
        self.outbox
            .push(Outgoing::Begin(Step::Rename { guid, name }));
    }

    /// A verb's step past its dialog ([`Step`]). Each begins its operation (`BeginOperation`,
    /// `0x5aa400`) and asks for it; with no link a character operation fails
    /// `RESPONSE_FAILED_TO_CONNECT` at once (`0x5aabca`, `0x5aac72`, `0x5ab11e`) and the rename
    /// sends nothing (`0x5ab630`).
    fn run_step(&mut self, step: Step) {
        let char_op = |m: &mut Self, kind, status, request: CharRequest| {
            m.op = Operation::begin(kind, status);
            if m.connected {
                m.park_out.push(request.into());
            } else {
                m.op.complete(false, STATUS_FAILED_TO_CONNECT);
            }
        };
        match step {
            Step::Logon { account, password } => {
                self.login_out.push(GlueLogin::Submit {
                    account,
                    password: password.0,
                });
            }
            Step::CharList => char_op(
                self,
                OpKind::CharList,
                STATUS_CHAR_LIST_RETRIEVING,
                CharRequest::Enumerate,
            ),
            Step::Create(r) => char_op(
                self,
                OpKind::Create,
                STATUS_CHAR_CREATE_IN_PROGRESS,
                CharRequest::Create(benilla_protocol::CharCreateReq {
                    name: r.name,
                    race: r.race,
                    class: r.class,
                    gender: r.sex,
                    skin: r.skin,
                    face: r.face,
                    hair_style: r.hair_style,
                    hair_color: r.hair_color,
                    facial_hair: r.facial_hair,
                }),
            ),
            Step::Delete(guid) => char_op(
                self,
                OpKind::Delete,
                STATUS_CHAR_DELETE_IN_PROGRESS,
                CharRequest::Delete(guid),
            ),
            Step::Rename { guid, name } => {
                if self.connected {
                    self.park_out
                        .push(CharRequest::Rename { guid, name }.into());
                }
            }
            Step::RealmList => {
                self.op = Operation::begin(OpKind::GetRealms, STATUS_REALM_LIST_IN_PROGRESS);
                self.park_out.push(RealmRequest::Refresh.into());
            }
            Step::Connect => {
                self.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
                self.queue = QueueEstimate::default();
                // The connect's own query (`0x5aab20` → `0x5b2220`).
                self.park_out.push(RealmRequest::Refresh.into());
            }
            Step::State(state) => self.state = state,
            Step::LogonPastDialog => {
                if self.auth_final {
                    self.state = ScreenState::Connect;
                    self.auth_final = false;
                    self.op = Operation::begin(OpKind::Connect, STATUS_CONNECTING);
                    self.queue = QueueEstimate::default();
                } else if self.cells.state == LOGIN_FAILED_STATE {
                    self.login_out.push(GlueLogin::CloseLink);
                    self.state = ScreenState::Idle;
                }
            }
            Step::BeginAuth => self.op = Operation::begin(OpKind::Auth, STATUS_AUTHENTICATING),
            Step::Authenticated => {
                self.state = ScreenState::Idle;
                self.op = Operation::default();
            }
            Step::ClearQueue => self.queue = QueueEstimate::default(),
            Step::WorldFailed(status) => self.world_failed_past_dialog(status),
            Step::RaiseRealmList => {
                if self.holder.is_empty() {
                    self.request_realm_list(true);
                } else {
                    self.outbox.push(Outgoing::Close);
                    self.outbox.push(Outgoing::Rebuild);
                }
            }
            Step::RequestRealmList => self.request_realm_list(true),
            Step::GetCharacterList => self.get_character_list(),
        }
    }

    /// Run `f`, the items it raises going ahead of everything already waiting: a step or an
    /// outbox entry's own work runs inside the arm that raised it, before the arm goes on.
    fn at_front(&mut self, f: impl FnOnce(&mut Self)) {
        let rest = std::mem::take(&mut self.outbox);
        f(self);
        self.outbox.extend(rest);
    }

    /// The outbox's next signal or frame-only entry, every step ahead of it run in order on the
    /// way, what each raises taking its place at the front.
    fn next_out(&mut self) -> Option<Outgoing> {
        loop {
            if self.outbox.is_empty() {
                return None;
            }
            match self.outbox.remove(0) {
                Outgoing::Begin(step) => self.at_front(|m| m.run_step(step)),
                out => return Some(out),
            }
        }
    }

    /// What a verb's call raised, as the VM fires it: its steps become [`GlueSignal::Resume`]s
    /// at their places. Nothing a call raises is one of the frame-only entries; one would wait
    /// for the frame's flush.
    fn vm_signals(&mut self, raised: Vec<Outgoing>) -> Vec<GlueSignal> {
        let mut signals = Vec::with_capacity(raised.len());
        for out in raised {
            match out.into_signal() {
                Ok(signal) => signals.push(signal),
                Err(Outgoing::Begin(step)) => {
                    let id = self.next_resume;
                    self.next_resume = id.wrapping_add(1);
                    self.resumes.insert(id, step);
                    signals.push(GlueSignal::Resume(id));
                }
                Err(out) => self.outbox.push(out),
            }
        }
        signals
    }

    /// Every step the outbox holds run in order, the signals kept for a VM: the frame with no
    /// glue VM to fire into, whose state still moves.
    fn settle(&mut self) {
        let mut kept = Vec::new();
        while let Some(out) = self.next_out() {
            kept.push(out);
        }
        self.outbox = kept;
    }

    /// `CGlueMgr::EnterWorld` (`0x46b500`) for the record the VM's selection named (its guard
    /// `0x46b53e`): the realm and link guards (`0x46b50d`, `0x46b56c`), the two flag arms
    /// (`0x46b57e`, `0x46b5b1`), then `lastCharacterIndex` (`0x46b5fa`, saved at `0x46b6f6`), state 8
    /// (`0x46b711`) and the pick, in order with the park's other requests. Every call repoints the
    /// selected record (`[0xb41df4]`, `0x46b539`), and the first frame of state 8 stashes the guid
    /// the login sends from it (`0x46c1d6`-`0x46c1e6`, sent once at `0x46c272`): here the pick
    /// leaves at the end of the frame, so a later call in the same frame replaces its guid, and
    /// one after it sends nothing more.
    fn enter_world(&mut self, index: i32, guid: u64, flags: u32) {
        if self.selected.is_none() || !self.connected {
            return;
        }
        if flags & FLAG_LOCKED_FOR_TRANSFER != 0 {
            self.open(
                "OKAY",
                Some(Text::Key(benilla_ui::script::glue_status_key(i32::from(
                    STATUS_CHAR_LOGIN_LOCKED_FOR_TRANSFER,
                )))),
            );
            return;
        }
        if flags & FLAG_RENAME != 0 {
            self.outbox
                .push(Outgoing::ForceRename("CHAR_RENAME_DESCRIPTION"));
            return;
        }
        // `0x46b5fa`: the CVar in the call; the save (`0x46b6f6`) is the frame's.
        self.outbox.push(Outgoing::Cvar {
            name: crate::glue_session::roster::CVAR_LAST_CHARACTER,
            value: index.to_string(),
        });
        self.last_character = Some(index);
        let queued = self.park_out.iter_mut().find_map(|r| match r {
            ParkRequest::Char(CharRequest::Enter(g)) => Some(g),
            _ => None,
        });
        match queued {
            Some(g) => *g = guid,
            None if self.state != ScreenState::EnterWorld => {
                self.park_out.push(CharRequest::Enter(guid).into());
            }
            None => {}
        }
        self.state = ScreenState::EnterWorld;
    }

    /// `SMSG_CHARACTER_LOGIN_FAILED` (`0x5aad70`): the status its byte maps to, for the glue
    /// rebuild that leaves the world (`0x401ee0`) to put into state 9 ([`Self::take_refusal`]).
    fn on_char_login_failed(&mut self, result: u8) {
        self.refused = Some(char_login_status(result));
    }

    /// The rebuild's half of a refused login (`0x402075`, then `0x5aaa50`): state 9 and the login
    /// operation completed failed with the refusal's status.
    fn take_refusal(&mut self) {
        if let Some(status) = self.refused.take() {
            self.state = ScreenState::LoginRefused;
            self.op = Operation::begin(OpKind::Login, STATUS_CHAR_LOGIN_IN_PROGRESS);
            self.op.complete(false, status);
        }
    }

    /// `SMSG_CHAR_DELETE` (`0x5b45c0` → `0x5ab0e0`): the result byte is the delete's status,
    /// success on `0x39`, whatever the screen state (`SetStatus`, `0x5aa920`).
    fn on_char_deleted(&mut self, code: u8) {
        if self.op.kind == OpKind::Delete {
            self.op.complete(code == STATUS_CHAR_DELETE_SUCCESS, code);
        }
    }

    /// `SMSG_CHAR_RENAME` (`0x46b440`), in any screen state, which it leaves as it is:
    /// `CLOSE_STATUS_DIALOG` (`0x46b48b`), then a result of 0 renames the record by the reply's
    /// guid (a reply with none has guid 0, which names no record), `0x31` is
    /// `CHAR_CREATE_NAME_IN_USE` (`0x46b4c0`, `0x5abd20(0x31)`) and any other `CHAR_RENAME_FAILED`
    /// (`0x83711c`).
    fn on_char_renamed(&mut self, result: u8, renamed: Option<(u64, String)>) {
        self.outbox.push(Outgoing::Close);
        match result {
            0 => {
                let (guid, name) = renamed.unwrap_or_default();
                self.outbox.push(Outgoing::Renamed { guid, name });
            }
            RENAME_NAME_IN_USE => {
                self.outbox
                    .push(Outgoing::ForceRename(benilla_ui::script::glue_status_key(
                        i32::from(RENAME_NAME_IN_USE),
                    )))
            }
            _ => self
                .outbox
                .push(Outgoing::ForceRename("CHAR_RENAME_FAILED")),
        }
    }

    /// `0x46b8d0`, `RequestRealmList`'s body: state 4, the dialog when asked, and the query
    /// (`0x5ab840`, `COP_GET_REALMS` at `REALM_LIST_IN_PROGRESS`).
    fn request_realm_list(&mut self, dialog: bool) {
        self.state = ScreenState::RealmList;
        if dialog {
            self.open("CANCEL", Some(Text::Key("REALM_LIST_IN_PROGRESS")));
        }
        self.outbox.push(Outgoing::Begin(Step::RealmList));
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
    /// it: the selection (`0x5ab7b0`'s tail, `0x5ab6f0`). With no world link, state 2, the
    /// `GAME_SERVER_LOGIN` dialog and the connect, all in the call (`0x46b246`-`0x46b27b`). With
    /// one up, the call only flags the reconnect (`0x46b230`, `0x46b235`) and drops the link, its
    /// flag down at once (`0x5ab1a0`); the connect is begun by the drop's handler
    /// (`0x46c585`-`0x46c5bf`), which here is this manager's next frame ([`Self::reconnect`]), the
    /// drop asked before anything of realmd on the one ordered channel.
    fn connect_to_realm(&mut self, name: &str) {
        if let Some(r) = select_by_name(&self.holder, name) {
            self.selected = Some(r);
        }
        if self.connected {
            self.park_out.push(RealmRequest::Leave.into());
            self.connected = false;
            self.reconnect = true;
            return;
        }
        self.begin_connect();
    }

    /// The disconnect handler's reconnect (`0x46c585`-`0x46c5bf`): the drop `ChangeRealm` made is
    /// done, and the connect it deferred begins.
    fn reconnect(&mut self) {
        if std::mem::take(&mut self.reconnect) {
            self.begin_connect();
        }
    }

    /// State 2, the `GAME_SERVER_LOGIN` dialog and the connect (`0x5aab20`), as `ConnectToRealm`
    /// and the disconnect handler both begin it.
    fn begin_connect(&mut self) {
        self.state = ScreenState::Connect;
        self.open("CANCEL", Some(Text::Key("GAME_SERVER_LOGIN")));
        self.outbox.push(Outgoing::Begin(Step::Connect));
    }

    /// A realm list reply: the holder filled (`0x5b2230`), then the pending operation's arm of
    /// `0x5abb90`. A query's success is `REALM_LIST_SUCCESS`; a connect's dials the selected realm,
    /// or the one [`Self::connect_realm`] finds, and completes only through the dial (`0x5ab823`
    /// calls no `SetStatus`); no answer fails either `REALM_LIST_FAILED`.
    fn on_realm_list(
        &mut self,
        realms: Vec<RealmInfo>,
        failed: bool,
        realm_name: &str,
        run: &RunModeRealm,
    ) {
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
                    self.selected = self.connect_realm(realm_name, run);
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
            OpKind::Auth | OpKind::CharList | OpKind::Create | OpKind::Delete | OpKind::Login => {}
        }
    }

    /// The realm a connect with none selected this process dials: the one `WOW_REALM` names when
    /// it is listed and up, else the one `realmName` names (`0x5ab6f0`), else the unattended
    /// run's pick ([`RunModeRealm`]). The run's own choice is a selection as `ChangeRealm` makes
    /// one: `realmName` takes its name (`0x5ab7b0`), which `GetServerName` and the character files
    /// read, and the session has chosen a realm (`[0x837e9c]`, `0x46f1d1`), so an account with no
    /// characters on it is not asked for one. A player's run has neither, and the connect fails
    /// `REALM_LIST_REALM_NOT_FOUND` into the list as the reference's does.
    fn connect_realm(&mut self, realm_name: &str, run: &RunModeRealm) -> Option<RealmInfo> {
        let named = run
            .named()
            .and_then(|n| crate::glue_session::realm::first_named_up(&self.holder, [n]).cloned());
        let (realm, why) = match named {
            Some(realm) => (realm, "WOW_REALM"),
            None => {
                if let Some(realm) = select_by_name(&self.holder, realm_name) {
                    return Some(realm);
                }
                (
                    run.unattended_pick(&self.holder)?.clone(),
                    "the unattended pick",
                )
            }
        };
        info!("glue: {why} selects {:?}", realm.name);
        self.outbox.push(Outgoing::Cvar {
            name: crate::glue_session::realm::CVAR_REALM_NAME,
            value: realm.name.clone(),
        });
        if self.realm_cells.chosen.is_none() {
            self.realm_cells.chosen = Some(0);
            self.cells_moved = true;
        }
        Some(realm)
    }

    /// A login stage off the IO thread. `glue_up` is whether the stock screens stand: a stage of
    /// an attempt the run made on its own (the `WOW_USER` fast path, a resubmit) shows
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
            // The SRP exchange is inside the IO thread's one blocking logon call.
            LoginStage::Authenticating => {}
            // The world dial begins: a logon with no realm list to publish is done.
            LoginStage::Handshaking => self.on_logon_done(),
            // The connect completes `RESPONSE_CONNECTED` and the world link's flag goes up with the
            // socket, before the authentication (`0x5aaa90`, `0x5aaa9c`): a drop from here reaches
            // the disconnect handler.
            LoginStage::WorldConnected => {
                if self.state == ScreenState::Connect && self.op.kind == OpKind::Connect {
                    self.op.complete(true, STATUS_RESPONSE_CONNECTED);
                    self.connected = true;
                }
            }
        }
    }

    /// The account guard refused the run's own credentials ([`LoginGuardRefused`]):
    /// benilla's refusal of a scripted login on an account its checkout does not declare, before
    /// any logon. It shows as `CGlueMgr::Login`'s own refusal of an empty account does
    /// (`0x46b090`-`0x46b0ac`): `OPEN_STATUS_DIALOG` with `OKAY` and the words, the state left as
    /// it is.
    fn on_guard_refused(&mut self, why: &str) {
        self.open("OKAY", Some(Text::Raw(why.to_string())));
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
            ScreenState::Connect => {
                let status = match msg.refusal {
                    Some(LoginRefusal::World(code)) => code,
                    Some(LoginRefusal::Logon(_) | LoginRefusal::LogonProof(_)) => {
                        STATUS_RESPONSE_FAILURE
                    }
                    None => STATUS_FAILED_TO_CONNECT,
                };
                self.op.complete(false, status);
            }
            _ => {}
        }
    }

    /// State 2's failure arm (`0x46bea7`-`0x46bfc8`), off the status the frame read: the dialog
    /// unless cancelled (`0x46bee7`, `0x46bf6f`), whose `OnShow` press runs in state 2
    /// (`0x46b754`), then the rest ([`Self::world_failed_past_dialog`]).
    fn world_failed(&mut self, op: &Operation) {
        let status = op.status;
        if status != STATUS_CANCELLED {
            let (text, code) = status_text(status);
            match code.and_then(|c| URL_STATUSES.iter().find(|(s, _)| *s == c)) {
                Some((_, url)) => self.outbox.push(Outgoing::Open {
                    which: "OKAY_WITH_URL",
                    text: Some(text),
                    data: Some(url.to_string()),
                }),
                None => self.open("OKAY", Some(text)),
            }
        }
        self.outbox.push(Outgoing::Begin(Step::WorldFailed(status)));
    }

    /// The failure arm past its dialog: state 0 (`0x46bef5`), a cancel's world socket dropped
    /// (`0x46befd`); `AUTH_FAILED` from the character screen goes back to login (`0x46bfa7`);
    /// every other status tears the roster down (`0x46bf1f`) and then raises the realm list
    /// ([`Step::RaiseRealmList`]).
    fn world_failed_past_dialog(&mut self, status: u8) {
        self.state = ScreenState::Idle;
        self.connected = false;
        if status == STATUS_CANCELLED {
            self.login_out.push(GlueLogin::CancelWorld);
        }
        if status == STATUS_AUTH_FAILED {
            self.outbox.push(Outgoing::ScreenFrom {
                current: "charselect",
                name: "login",
            });
            return;
        }
        self.outbox.push(Outgoing::Teardown);
        self.outbox.push(Outgoing::Begin(Step::RaiseRealmList));
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
    /// success arm. The realm the link reached is the selected one, as the connect dials only
    /// `[0xc28134]`'s record (`0x5ab823`); a dial the run made on its own selects it.
    fn on_world_authenticated(&mut self, realm: benilla_protocol::RealmInfo, epoch: u64) {
        self.connected = true;
        self.link = epoch;
        if self.selected.is_none() {
            self.selected = Some(realm);
        }
        if self.state == ScreenState::Logon {
            // A logon whose world half finished inside one frame: its arm runs, and the
            // authentication completes once the arm's steps have ([`Self::finish_auth`]).
            self.on_logon_done();
            self.tick();
            self.auth_waits = true;
            return;
        }
        self.complete_auth();
    }

    /// The authentication that waited on the logon's arm.
    fn finish_auth(&mut self) {
        if std::mem::take(&mut self.auth_waits) {
            self.complete_auth();
        }
    }

    /// In state 2, the authentication completes `AUTH_OK` (`0x5aab80`).
    fn complete_auth(&mut self) {
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
    /// disconnect"), has no counterpart here: the IO thread holds one session and reports only
    /// its own end.
    fn on_disconnected(&mut self, end: benilla_protocol::SessionEnd, session_over: bool) {
        if end == benilla_protocol::SessionEnd::LoggedOut {
            return;
        }
        self.connected = false;
        if !session_over {
            return;
        }
        self.refused = None;
        let was_connect = self.state == ScreenState::Connect;
        self.state = ScreenState::Idle;
        let op = std::mem::take(&mut self.op);
        match op.text() {
            Some(text) if was_connect && !op.ok => self.open("OKAY", Some(text)),
            _ => self.outbox.push(Outgoing::Disconnected),
        }
    }

    /// The per-frame arms of `CGlueMgr::Update` that run without a new message. Each reads the
    /// operation's status once (`0x46ba5a`) and decides off it; every write it makes after a
    /// signal is a [`Step`] behind that signal, so the signal's handlers run first, as the
    /// reference's do inside its `SignalEvent`.
    fn tick(&mut self) {
        match self.state {
            ScreenState::Logon => {
                // `0x46ba7c`: every login state but PIN_WAIT reaches the dialog arm.
                if self.cells.state != LOGIN_PIN_WAIT {
                    self.show_login_dialog();
                }
                self.outbox.push(Outgoing::Begin(Step::LogonPastDialog));
            }
            ScreenState::Connect => self.tick_connect(),
            ScreenState::CharList => self.tick_char_list(),
            ScreenState::RealmList => self.tick_realm_list(),
            ScreenState::Create => self.tick_create(),
            ScreenState::Delete => self.tick_delete(),
            // `0x46c283`: `OKAY` with the operation's text, every frame the state stands.
            ScreenState::LoginRefused => {
                let text = self.op.text();
                self.open("OKAY", text);
            }
            // No arm for state 7: the compares in `[0x46b930, 0x46c2ac)` never take it. State 8's
            // two phases are the world entry's here (`0x46c1b8`).
            ScreenState::Rename | ScreenState::EnterWorld | ScreenState::Idle => {}
        }
    }

    /// An arm's re-text of the dialog from the operation (`0x46c043`, `0x46c0ae`, `0x46c0fc`,
    /// `0x46c157`), empty while it runs; the operation as the frame read it.
    fn retext(&mut self) -> Operation {
        let op = self.op.clone();
        if let Some(text) = op.text() {
            self.outbox.push(Outgoing::Update { text, button: None });
        }
        op
    }

    /// A completed operation's failure: `OKAY` with the status's text, then state 0 (`0x46c177`,
    /// `0x46c188`).
    fn fail_okay(&mut self, op: &Operation) {
        self.open("OKAY", op.text());
        self.outbox
            .push(Outgoing::Begin(Step::State(ScreenState::Idle)));
    }

    /// State 5's frame (`0x46c0f3`-`0x46c13d`): the dialog re-texted, then a completed create's
    /// arm. A success goes to state 0, closes the dialog, fires `SELECT_LAST_CHARACTER` and puts
    /// `charselect` up (`0x46c11e`-`0x46c13d`); a failure opens `OKAY` with the status's text and
    /// goes to state 0 (`0x46c118` → `0x46c177`).
    fn tick_create(&mut self) {
        let op = self.retext();
        if !op.done {
            return;
        }
        if op.ok {
            self.outbox
                .push(Outgoing::Begin(Step::State(ScreenState::Idle)));
            self.outbox.push(Outgoing::Close);
            self.outbox.push(Outgoing::SelectLast);
            self.outbox.push(Outgoing::Screen("charselect"));
        } else {
            self.fail_okay(&op);
        }
    }

    /// State 6's frame (`0x46c14e`-`0x46c1a7`): the dialog re-texted, then a completed delete's
    /// arm. A success fires `SELECT_FIRST_CHARACTER` and asks for the list again (`0x46b880`, state
    /// 3 and its dialog); a failure opens `OKAY` with the status's text and goes to state 0
    /// (`0x46c173`).
    fn tick_delete(&mut self) {
        let op = self.retext();
        if !op.done {
            return;
        }
        if op.ok {
            self.outbox.push(Outgoing::SelectFirst);
            self.outbox.push(Outgoing::Begin(Step::GetCharacterList));
        } else {
            self.fail_okay(&op);
        }
    }

    /// State 4's frame (`0x46c0a5`): the dialog re-texted, then a completed query's arm, success
    /// to state 0, the dialog closed and the rebuild (`0x46c0d2`-`0x46c0e2`), failure to `OKAY`
    /// with its text and state 0 (`0x46c0c9` → `0x46c177`).
    fn tick_realm_list(&mut self) {
        let op = self.retext();
        if !op.done {
            return;
        }
        if op.ok {
            self.outbox
                .push(Outgoing::Begin(Step::State(ScreenState::Idle)));
            self.outbox.push(Outgoing::Close);
            self.outbox.push(Outgoing::Rebuild);
        } else {
            self.fail_okay(&op);
        }
    }

    /// State 2's frame (`0x46bc49`-`0x46c02e`): the dialog re-texted from the operation, then a
    /// completed operation's arm.
    fn tick_connect(&mut self) {
        let op = self.op.clone();
        if op.status == STATUS_WAIT_QUEUE {
            self.outbox.push(Outgoing::Queue);
        } else if let Some(text) = op.text() {
            self.outbox.push(Outgoing::Update { text, button: None });
        }
        if !op.done {
            return;
        }
        if !op.ok {
            self.world_failed(&op);
        } else if op.kind == OpKind::Connect {
            self.outbox.push(Outgoing::Begin(Step::BeginAuth));
        } else {
            // `0x46c00e`-`0x46c023`: state 0, the roster torn down, `charselect`, the queue
            // position cleared. The screen's own `GetCharacterListUpdate` loads the roster.
            self.outbox.push(Outgoing::Begin(Step::Authenticated));
            self.outbox.push(Outgoing::Teardown);
            self.outbox.push(Outgoing::Screen("charselect"));
            self.outbox.push(Outgoing::Begin(Step::ClearQueue));
        }
    }

    /// State 3's frame (`0x46c03a`-`0x46c0a4`): the dialog re-texted, then a completed query's
    /// arm. A cancel tears the roster down (`0x46c069`) and raises the realm list
    /// (`RequestRealmList(1)`, `0x46c073`); any other failure opens `OKAY` with its text and goes
    /// to state 0 (`0x46c173`); a success goes to state 0, closes the dialog and loads the roster
    /// (`0x46c084`-`0x46c094`).
    fn tick_char_list(&mut self) {
        let op = self.retext();
        if !op.done {
            return;
        }
        if !op.ok {
            if op.status == STATUS_CANCELLED {
                self.outbox.push(Outgoing::Teardown);
                self.outbox.push(Outgoing::Begin(Step::RequestRealmList));
            } else {
                self.fail_okay(&op);
            }
            return;
        }
        self.outbox
            .push(Outgoing::Begin(Step::State(ScreenState::Idle)));
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

/// The one [`GlueMgr`], shared by the app's frame and the glue VM it seats: a NonSend resource,
/// as the VM is. Nothing that holds a borrow of it calls into Lua, since a handler may call a verb
/// that borrows it in turn.
#[derive(Clone, Default)]
pub(crate) struct SharedGlueMgr(Rc<GlueMgrCell>);

/// The cell the manager lives in, which the VM holds as its host.
#[derive(Default)]
struct GlueMgrCell(RefCell<GlueMgr>);

impl SharedGlueMgr {
    pub(crate) fn get(&self) -> Ref<'_, GlueMgr> {
        self.0 .0.borrow()
    }

    pub(crate) fn get_mut(&self) -> RefMut<'_, GlueMgr> {
        self.0 .0.borrow_mut()
    }

    /// The manager as the glue VM's host.
    fn host(&self) -> Rc<dyn GlueHost> {
        self.0.clone()
    }
}

/// A glue verb's call into the manager: its half runs here, and the signals it raised go back to
/// the VM to fire before the verb returns. Everything a call raises is a signal; the four only the
/// frame fires would stay for its flush.
impl GlueHost for GlueMgrCell {
    fn call(&self, call: GlueCall) -> Vec<GlueSignal> {
        let mut mgr = self.0.borrow_mut();
        let from = mgr.outbox.len();
        mgr.call(call);
        let raised = mgr.outbox.split_off(from);
        mgr.vm_signals(raised)
    }

    fn resume(&self, step: u32) -> Vec<GlueSignal> {
        let mut mgr = self.0.borrow_mut();
        let Some(step) = mgr.resumes.remove(&step) else {
            return Vec::new();
        };
        let rest = std::mem::take(&mut mgr.outbox);
        mgr.run_step(step);
        let raised = std::mem::replace(&mut mgr.outbox, rest);
        mgr.vm_signals(raised)
    }

    fn connected(&self) -> bool {
        self.0.borrow().connected
    }

    fn selected_realm_type(&self) -> Option<u32> {
        self.0.borrow().selected.as_ref().map(|r| r.realm_type)
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

/// A world status's text and its code for the URL walk. `GetStatus` (`0x5aa960`) reads a status
/// outside the table's 0x53 rows (`0x5aa99d`) as the number, `"(%i)"` (`0x85d5b0`) into
/// `0xc27eb8`; the byte reaches it unclamped (`0x5aa920`), and the URL walk reads it unchanged.
fn status_text(code: u8) -> (Text, Option<u8>) {
    match GLUE_STATUS_KEYS.get(usize::from(code)) {
        Some(key) => (Text::Key(key), Some(code)),
        None => (Text::Raw(format!("({code})")), Some(code)),
    }
}

/// The login engine's messages the manager reads, as one parameter.
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
    addon_infos: MessageReader<'w, 's, AddonInfoMessage>,
    addon_reply: Option<Res<'w, AddonInfoReply>>,
    char_actions: MessageReader<'w, 's, CharActionResultMessage>,
    renames: MessageReader<'w, 's, CharRenameMessage>,
    login_refusals: MessageReader<'w, 's, CharacterLoginFailedMessage>,
    /// What the login engine decided with no network message behind it.
    notices: MessageReader<'w, 's, LoginGuardRefused>,
    /// The name check's lists, which the glue VM is seated with (`0x6c91a0`).
    names: Option<Res<'w, crate::text_filter::NameFilter>>,
}

/// What the manager asks of the rest of the app.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct GlueAsks<'w> {
    login: MessageWriter<'w, GlueLogin>,
    music: MessageWriter<'w, crate::sound::GlueMusicRequest>,
    launch: MessageWriter<'w, super::server_alert::LaunchUrl>,
    launch_addon: MessageWriter<'w, super::server_alert::LaunchAddonUrl>,
    realm: Option<Res<'w, crate::net::RealmChoice>>,
    abandon: Option<Res<'w, crate::net::LoginAbandon>>,
    /// The session's roster, which names the in-world files: a rename renames it too, and
    /// the world entry's pick is its pending one.
    roster: Option<ResMut<'w, crate::glue_session::roster::Roster>>,
    pick: Option<Res<'w, crate::net::CharPick>>,
    /// The engine-side CVar table `EnterWorld` writes and saves.
    cvars: Option<ResMut<'w, crate::cvars::Cvars>>,
    /// What a new character's entry reads for its intro eye.
    eyes: crate::glue_session::roster::IntroEyes<'w>,
}

/// The four agreements' states off the CVar registry: the read `0x46a400` makes at `0x402b36`,
/// before the first glue screen is set (`0x402b65`), so a returning player's login screen never
/// sees them unaccepted.
pub(crate) fn read_agreements(cvars: &crate::cvars::Cvars) -> [NoticeState; 4] {
    let mut n = [NoticeState::default(); 4];
    for notice in GlueNotice::ALL {
        if let Some(value) = cvars.get(notice.cvar()) {
            n[notice as usize] = NoticeState::from_cvar(value);
        }
    }
    n
}

/// The glue manager's frame: the login engine's news, the per-frame arms, then the events into the
/// glue VM and its slots seeded. The verbs ran in their own calls ([`GlueHost`]); what is left of
/// them here is the effects they leave for the frame and the calls a VM made before it was seated.
/// Runs under the stock screens only.
///
/// No borrow of the manager is held across a call into the VM that can run a handler.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_glue_mgr(
    shared: NonSend<SharedGlueMgr>,
    mut hosted_vm: crate::script_calls::HostedVm,
    mut news: LoginNews,
    mut asks: GlueAsks,
    tables: Option<Res<crate::glue_session::realm::RealmTables>>,
    run: Option<Res<RunModeRealm>>,
    mut realmlist: Option<ResMut<crate::realmlist::Realmlist>>,
    areas: Option<Res<crate::area::AreaTableRes>>,
    sources: super::glue_create::CreateSources,
    state: Option<Res<State<crate::client_state::ClientState>>>,
    time: Res<Time>,
) {
    hosted_vm.run(|script| {
        let shared = shared.clone();
        let now_ms = time.elapsed().as_millis() as u32;
        let mut script = script.filter(|s| s.vm_kind() == VmKind::Glue);
        let in_world = state.is_some_and(|s| *s.get() == crate::client_state::ClientState::InWorld);
        let glue_up = script.is_some() && !in_world;

        // A refused character login, ahead of the glue build it comes back to (`0x5aad70`).
        for msg in news.login_refusals.read() {
            shared.get_mut().on_char_login_failed(msg.result);
        }

        // A glue VM the glue build did not seat ([`seat_glue_build`]) is seated here.
        let mut fresh_vm = false;
        if let Some(s) = script.as_deref_mut() {
            if shared.get().vm != Some(s.session()) {
                seat_glue(
                    &shared,
                    s,
                    news.addon_reply.as_deref().and_then(|r| r.0.as_deref()),
                    news.names.as_deref(),
                    tables.as_deref(),
                    realmlist.as_deref(),
                );
                fresh_vm = true;
            }
            if let Some(names) = news.names.as_ref().filter(|n| !fresh_vm && n.is_changed()) {
                s.set_glue_name_lists(std::rc::Rc::new((**names).clone()));
            }
            // The create screen's catalog, on every new VM (the glue build's walk, `0x470430`) and
            // whenever a catalog behind it arrives.
            if fresh_vm || sources.changed() {
                s.set_glue_create_catalog(std::rc::Rc::new(sources.catalog()));
            }
        } else if in_world {
            // In the world the glue is torn down, and the entry's second phase leaves state 0
            // (`0x46c252`).
            shared.get_mut().state = ScreenState::Idle;
        }
        // A refused login's state 9, on the glue the refusal comes back to, before its screen's
        // list ask is served (`0x402075`). One that finds the glue still standing beat the world it
        // revokes, which here is entered before the verdict (the pick's `Deviation:` in
        // [`crate::net::io`]), and ends the same way.
        if glue_up {
            shared.get_mut().take_refusal();
        }
        if let (Some(s), Some(r)) = (script.as_deref_mut(), tables.as_ref()) {
            if !fresh_vm && r.is_changed() {
                let (categories, types) = r.glue_setup();
                s.set_glue_realm_setup(categories, types);
            }
        }
        // The name a connect resolves (`0x5ab7d0`).
        let realm_name = script
            .as_deref()
            .and_then(|s| s.cvar(crate::glue_session::realm::CVAR_REALM_NAME))
            .unwrap_or_default();

        {
            let mut mgr = shared.get_mut();
            // The alert a finished fetch left, shown before anything else this frame (`0x46b939`).
            mgr.poll_alert();
            if glue_up {
                mgr.show_alert();
            }
            // A world link `ChangeRealm` dropped is down: the connect it deferred begins.
            mgr.reconnect();

            // What the verbs left for the frame, and the calls a VM made before it was seated.
            if let Some(s) = script.as_deref_mut() {
                for call in s.take_glue_calls() {
                    mgr.call(call);
                }
                for request in s.take_glue_requests() {
                    match request {
                        GlueRequest::PlayGlueMusic(file) => {
                            asks.music.write(crate::sound::GlueMusicRequest::Play(file));
                        }
                        GlueRequest::StopGlueMusic => {
                            asks.music.write(crate::sound::GlueMusicRequest::Stop);
                        }
                        GlueRequest::PlayCreditsMusic => {
                            asks.music.write(crate::sound::GlueMusicRequest::Credits);
                        }
                        // `0x5abd00`, past the whitelist the verb applied (`0x5abc10`).
                        GlueRequest::LaunchAddonUrl(url) => {
                            asks.launch_addon
                                .write(super::server_alert::LaunchAddonUrl(url));
                        }
                        GlueRequest::LaunchUrl(url) => {
                            asks.launch.write(super::server_alert::LaunchUrl(url));
                        }
                    }
                }
                mgr.realm_cells = s.glue_realm_cells();
            }
            // `EnterWorld`'s `lastCharacterIndex` (`0x46b5fa`, `"%d"` of the selection) into the
            // engine's table, then `SaveConfig` (`0x46b6f6`).
            if let Some(index) = mgr.last_character.take() {
                if let Some(cvars) = asks.cvars.as_deref_mut() {
                    cvars.set(
                        crate::glue_session::roster::CVAR_LAST_CHARACTER,
                        &index.to_string(),
                    );
                    cvars.save();
                }
            }
        }

        // `SMSG_ADDON_INFO` read (`0x5b4260`): the index rebuilt from the reply (`0x51da70`), then
        // the listener's `ADDON_LIST_UPDATE` (`0x5b4290`).
        for msg in news.addon_infos.read() {
            // Only the glue VM (`script` is filtered to it above); with none up, `AddonInfoReply`
            // holds the list for the next one.
            if let Some(s) = script.as_deref_mut() {
                s.note_addon_info_records(super::addons::addon_replies(&msg.records));
                s.fire_event("ADDON_LIST_UPDATE", Vec::new());
            }
        }

        let names = RosterNames {
            areas: areas.as_deref().map(|a| &a.0),
            create: sources.create.as_deref().map(|c| &c.0),
            classes: sources.classes.as_deref().map(|c| &c.0),
            creatures: sources.creatures.as_deref(),
        };

        let (lists, latched) = {
            let mut mgr = shared.get_mut();
            // The login engine's news.
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
            let latched = mgr.auth_final;
            if latched {
                mgr.tick();
            }
            (lists, latched)
        };
        if latched {
            pump(
                &shared,
                script.as_deref_mut(),
                realmlist.as_deref(),
                now_ms,
                &names,
                asks.roster.as_deref_mut(),
            );
        }
        let auth_waits = {
            let mut mgr = shared.get_mut();
            let player = RunModeRealm::default();
            let run = run.as_deref().unwrap_or(&player);
            for (realms, failed) in lists {
                mgr.on_realm_list(realms, failed, &realm_name, run);
            }
            for msg in news.failures.read() {
                mgr.on_failed(msg);
            }
            for LoginGuardRefused(why) in news.notices.read() {
                mgr.on_guard_refused(why);
            }
            // A session of an attempt a Cancel or a disconnect has passed is not the screen's.
            let generation = crate::net::LoginAbandon::now(asks.abandon.as_deref());
            for msg in news.world_auths.read() {
                if msg.generation == generation {
                    mgr.on_world_authenticated(msg.realm.clone(), msg.epoch);
                }
            }
            mgr.auth_waits
        };
        if auth_waits {
            pump(
                &shared,
                script.as_deref_mut(),
                realmlist.as_deref(),
                now_ms,
                &names,
                asks.roster.as_deref_mut(),
            );
            shared.get_mut().finish_auth();
        }
        {
            let mut mgr = shared.get_mut();
            for msg in news.roster_failures.read() {
                mgr.on_roster_failed(msg.epoch);
            }
            for msg in news.rosters.read() {
                mgr.on_roster(msg.characters.clone(), msg.cause, msg.epoch);
            }
            for msg in news.char_actions.read() {
                match msg.action {
                    benilla_protocol::CharAction::Delete => mgr.on_char_deleted(msg.code),
                    benilla_protocol::CharAction::Create => mgr.on_char_created(msg.code),
                }
            }
            for msg in news.renames.read() {
                mgr.on_char_renamed(msg.result, msg.renamed.clone());
            }
            for msg in news.disconnects.read() {
                mgr.on_disconnected(msg.end, msg.session_over);
            }
        }
        // What the news raised runs out before the frame's arm, as the reference's network handlers
        // signal inside the net pump, ahead of `CGlueMgr::Update`.
        pump(
            &shared,
            script.as_deref_mut(),
            realmlist.as_deref(),
            now_ms,
            &names,
            asks.roster.as_deref_mut(),
        );
        {
            let mut mgr = shared.get_mut();
            mgr.tick();
            // The realmlist control's typed value that was not an address reopens its dialog.
            if script.is_some() {
                if let Some(typed) = realmlist.as_deref_mut().and_then(|r| r.take_refused()) {
                    mgr.outbox.push(Outgoing::Open {
                        which: "BENILLA_REALMLIST",
                        text: Some(Text::Raw(
                            crate::glue_session::login::REALMLIST_BAD.to_string(),
                        )),
                        data: Some(typed),
                    });
                }
            }
        }
        pump(
            &shared,
            script.as_deref_mut(),
            realmlist.as_deref(),
            now_ms,
            &names,
            asks.roster.as_deref_mut(),
        );
        let mut mgr = shared.get_mut();
        if let Some(s) = script.as_deref() {
            mgr.select_cells = s.glue_select_cells();
        }
        // The frame's asks, the verbs' calls since the last frame and the handlers' inside it.
        send_asks(&mut mgr, &mut asks);
    })
}

/// Run what the manager has raised so far: into the glue VM when one stands, its slots seeded
/// first ([`seed`]) and its outbox fired in order ([`flush`]); with none, its steps alone, the
/// signals kept for the next VM.
fn pump(
    shared: &SharedGlueMgr,
    script: Option<&mut UiScript>,
    realmlist: Option<&crate::realmlist::Realmlist>,
    now_ms: u32,
    names: &RosterNames,
    roster: Option<&mut crate::glue_session::roster::Roster>,
) {
    match script {
        Some(s) => {
            seed(&mut shared.get_mut(), s, realmlist);
            flush(shared, s, now_ms, names, roster);
        }
        None => shared.get_mut().settle(),
    }
}

/// The glue build (`0x46a7b0`) on a new glue VM: state 0, then everything the manager holds that
/// the reference keeps in C globals, which outlive the Lua state: the manager itself as the VM's
/// host, the realm list and select cells, the AddOn registry and the reply the
/// session has read, the name check's lists, the realm categories, a refused login's state 9
/// (`0x402075`, set ahead of the screen) and the slots the verbs read.
fn seat_glue(
    shared: &SharedGlueMgr,
    s: &mut UiScript,
    addon_reply: Option<&[benilla_protocol::messages::AddonRecord]>,
    names: Option<&crate::text_filter::NameFilter>,
    tables: Option<&crate::glue_session::realm::RealmTables>,
    realmlist: Option<&crate::realmlist::Realmlist>,
) {
    s.set_glue_host(shared.host());
    let mut mgr = shared.get_mut();
    mgr.vm = Some(s.session());
    mgr.state = ScreenState::Idle;
    mgr.cells = LoginCells::default();
    mgr.auth_final = false;
    s.set_glue_realm_cells(mgr.realm_cells);
    s.set_glue_select_cells(mgr.select_cells);
    super::addons::seat_glue_addons(s, addon_reply);
    mgr.holder_moved = true;
    if let Some(names) = names {
        s.set_glue_name_lists(std::rc::Rc::new(names.clone()));
    }
    if let Some(r) = tables {
        let (categories, types) = r.glue_setup();
        s.set_glue_realm_setup(categories, types);
    }
    mgr.take_refusal();
    seed(&mut mgr, s, realmlist);
}

/// The glue build's seating ([`seat_glue`]) inside the build, before its first `SET_GLUE_SCREEN`:
/// the screen's `OnShow` runs in that event and reads what the build carried over (a logout's
/// `CharacterSelect_OnShow` asks for its list only on a link `IsConnectedToServer` reports, and
/// names the realm `GetServerName` does).
pub(crate) fn seat_glue_build(world: &mut World, script: &mut UiScript) {
    let Some(shared) = world.get_non_send_resource::<SharedGlueMgr>().cloned() else {
        return;
    };
    script.set_glue_create_catalog(std::rc::Rc::new(super::glue_create::world_catalog(world)));
    seat_glue(
        &shared,
        script,
        world
            .get_resource::<AddonInfoReply>()
            .and_then(|r| r.0.as_deref()),
        world.get_resource::<crate::text_filter::NameFilter>(),
        world.get_resource::<crate::glue_session::realm::RealmTables>(),
        world.get_resource::<crate::realmlist::Realmlist>(),
    );
}

/// Send the asks the manager holds: the login engine's, and the park requests in the order they
/// were made, the realm requests stamped with the abandon generation as they go and the world
/// entry's pick through the session's ([`crate::glue_session::roster::send_pick`]).
fn send_asks(mgr: &mut GlueMgr, asks: &mut GlueAsks) {
    let (logins, park_asks) = mgr.take_asks();
    for ask in logins {
        asks.login.write(ask);
    }
    let now = crate::net::LoginAbandon::now(asks.abandon.as_deref());
    if let Some(choice) = asks.realm.as_deref() {
        for mut ask in park_asks {
            match &mut ask {
                ParkRequest::Realm(RealmRequest::Enter { generation, .. }) => *generation = now,
                ParkRequest::Char(CharRequest::Enter(guid)) => {
                    if let (Some(roster), Some(pick)) =
                        (asks.roster.as_deref_mut(), asks.pick.as_deref())
                    {
                        crate::glue_session::roster::send_pick(roster, pick, *guid, &asks.eyes);
                    }
                    continue;
                }
                _ => {}
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
/// install's locale column, the file string the race row's (`ChrRaces` column 15), and the model flag
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
fn seed(mgr: &mut GlueMgr, script: &mut UiScript, realmlist: Option<&crate::realmlist::Realmlist>) {
    if std::mem::take(&mut mgr.cells_moved) {
        script.set_glue_realm_cells(mgr.realm_cells);
    }
    if std::mem::take(&mut mgr.holder_moved) {
        script.set_glue_realms(mgr.holder.iter().map(glue_realm).collect());
    }
    script.set_glue_realm_characters(mgr.characters);
    if let Some(r) = realmlist {
        seed_realmlist(script, r);
    }
}

/// The `realmName` CVar (`0x5ab7d0`), which the session's connect wrote (`0x5ab7b0`): the
/// per-character files are kept under it, as the reference's
/// `WTF\Account\<account>\<realmName>\`.
fn server_name(script: &UiScript) -> String {
    script
        .cvar(crate::glue_session::realm::CVAR_REALM_NAME)
        .unwrap_or_default()
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

/// Fire the outbox into the glue VM, the texts resolved through its own strings (`0x703bf0`). Each
/// entry is taken off the outbox and fired with no borrow of the manager held, since a handler may
/// call a verb, whose call runs in the manager and fires its own signals inside this one.
fn flush(
    shared: &SharedGlueMgr,
    script: &mut UiScript,
    now_ms: u32,
    names: &RosterNames,
    mut roster: Option<&mut crate::glue_session::roster::Roster>,
) {
    // Steps run as the walk reaches them; what an entry raises goes ahead of the rest.
    loop {
        let Some(out) = shared.get_mut().next_out() else {
            return;
        };
        flush_one(shared, script, now_ms, names, roster.as_deref_mut(), out);
    }
}

fn flush_one(
    shared: &SharedGlueMgr,
    script: &mut UiScript,
    now_ms: u32,
    names: &RosterNames,
    roster: Option<&mut crate::glue_session::roster::Roster>,
    out: Outgoing,
) {
    let out = match out.into_signal() {
        Ok(signal) => return script.fire_glue_signal(signal),
        Err(out) => out,
    };
    match out {
        Outgoing::Queue => {
            let body = {
                let mgr = shared.get();
                mgr.queue
                    .text_by(now_ms, mgr.queue_realm.as_deref(), |key| {
                        script.global_text(key)
                    })
            };
            let button = script.global_text("CHANGE_REALM");
            script.fire_event(
                "UPDATE_STATUS_DIALOG",
                vec![ScriptValue::Str(body), ScriptValue::Str(button)],
            );
        }
        Outgoing::Rebuild => {
            script.rebuild_glue_realms();
            shared.get_mut().realm_cells = script.glue_realm_cells();
        }
        Outgoing::Renamed { guid, name } => {
            // `0x4725f0`: the record by guid, else nothing; the files first (`0x47269a`,
            // `0x4726f0`), so the AddOn carry reads the moved file.
            let Some(old) = script.glue_character_name(guid) else {
                return;
            };
            let realm = server_name(script);
            crate::local_state::move_character_state(&realm, &old, &name);
            if let Some(c) = shared.get_mut().roster.iter_mut().find(|c| c.guid == guid) {
                c.name.clone_from(&name);
                c.flags &= !benilla_protocol::CHARACTER_FLAG_RENAME;
            }
            if let Some(roster) = roster {
                roster.rename(guid, &name);
            }
            script.apply_glue_rename(guid, &name);
        }
        Outgoing::Roster => {
            let records = shared
                .get()
                .roster
                .iter()
                .map(|c| glue_character(c, names))
                .collect();
            // `RequestRealmList(1)` at `0x47254b`, when no realm has been chosen, inside the load.
            let ask = script.load_glue_roster(records);
            let mut mgr = shared.get_mut();
            if ask {
                mgr.at_front(|m| m.request_realm_list(true));
            }
            mgr.realm_cells = script.glue_realm_cells();
        }
        // Signals, fired above.
        _ => {}
    }
}

#[cfg(test)]
#[path = "glue_mgr_tests.rs"]
mod tests;
