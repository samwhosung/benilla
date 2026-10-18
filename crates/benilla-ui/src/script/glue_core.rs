//! The glue core's login-screen verbs (registrar `0x46ddf0`, table `0x8373b8`): the login, the
//! status dialog's button, the saved account, the four user agreements, the build, the locale,
//! the server, the screen name, the glue music, the movie frame's settings and the quit. Each reads
//! a slot of its own, runs one of the glue manager's functions in place through the host the VM is
//! seated with ([`GlueHost`], a [`GlueCall`]), or leaves the host an effect for its next frame (a
//! [`GlueRequest`]). The screen state machine (`CGlueMgr`, its frame `CGlueMgr::Update` `0x46b930`)
//! is the host's.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, s_str_to_int, string_arg};
use super::{Model, ScriptValue};

/// One of the four user agreements the login screen walks before it shows the login boxes
/// (`AccountLogin_ShowUserAgreements`), each a CVar the glue manager reads once at startup
/// (`0x46a400`): `readTOS`, `readEULA`, `readScanning`, `readContest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlueNotice {
    Tos,
    Eula,
    Scanning,
    Contest,
}

impl GlueNotice {
    /// All four, in the glue manager's seeding order (`0x46a406`-`0x46a48e`).
    pub const ALL: [GlueNotice; 4] = [
        GlueNotice::Tos,
        GlueNotice::Eula,
        GlueNotice::Scanning,
        GlueNotice::Contest,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// The CVar the agreement is read from at startup (`0x46a400`) and set to `"1"` on its accept
    /// (`0x46aef0` and its siblings).
    pub fn cvar(self) -> &'static str {
        match self {
            GlueNotice::Tos => "readTOS",
            GlueNotice::Eula => "readEULA",
            GlueNotice::Scanning => "readScanning",
            GlueNotice::Contest => "readContest",
        }
    }
}

/// One agreement's two flags, as the glue manager seeds them from its CVar's integer value
/// (`0x46a41a`-`0x46a42e` for `readTOS`): accepted is the value being 1, the notice (the agreement
/// changed since it was read) is the value being negative.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoticeState {
    pub accepted: bool,
    pub show_notice: bool,
}

impl NoticeState {
    /// The flags for a CVar value, read as its record's integer ([`s_str_to_int`]).
    pub fn from_cvar(value: &str) -> Self {
        let n = s_str_to_int(value);
        NoticeState {
            accepted: n == 1,
            show_notice: n < 0,
        }
    }
}

/// The first `%d` of the CRT's `sscanf` (`0x73fb5d`): leading whitespace skipped, an optional sign
/// and the leading digits; `None` when no digit follows, which leaves the out-slot untouched; 0
/// for a number past `i64`.
pub fn scanf_d(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    (end > 0).then(|| sign * digits[..end].parse::<i64>().unwrap_or(0))
}

/// `DefaultServerLogin`'s two strings. Never logged: the `Debug` form names the account only.
#[derive(Clone)]
pub struct GlueCredentials {
    pub account: String,
    pub password: String,
}

impl std::fmt::Debug for GlueCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlueCredentials")
            .field("account", &self.account)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The `accountName` CVar's table key, its name lowercased.
const CVAR_ACCOUNT_NAME_KEY: &str = "accountname";

/// A glue verb's effect the host carries out on its next frame, drained by its glue manager: the
/// glue music and the browser. None of them is anything Lua reads back.
#[derive(Clone, Debug)]
pub enum GlueRequest {
    /// `PlayGlueMusic(file)` (`0x46ceb0` → `0x45aeb0`).
    PlayGlueMusic(String),
    /// `StopGlueMusic()` (`0x46cf00`): `0x45b050(1.0)`, a one-second fade.
    StopGlueMusic,
    /// `PlayCreditsMusic()` (`0x46cef0` → `0x45b000`): the theme out, the city playlist in.
    PlayCreditsMusic,
    /// `LaunchURL(url)` (`0x46cf60` → `0x5abd00`) for a URL that passed `launch_url_allowed`
    /// (`0x5abd00` tests `0x5abc10` first): the player's browser opens it (`0x429ff0`).
    LaunchUrl(String),
    /// `LaunchAddOnURL(index)` (`0x46d5e0` → `0x429ff0`, no whitelist) for a URL that passed
    /// [`launch_addon_url`]: already decoded, ready for the player's browser.
    LaunchAddonUrl(String),
}

/// One of the glue manager's own functions a verb calls (`CGlueMgr`, `[0x46a400, 0x476020)`), run
/// in place by the host the VM is seated with ([`GlueHost::call`]): its screen state, its dialog
/// and its other events land before the verb returns; only the network half waits for the host.
#[derive(Clone, Debug)]
pub enum GlueCall {
    /// `DefaultServerLogin(account, password)` (`0x46d160`) past the agreements (`0x46afb0`'s
    /// first two tests, `[0xb41dfc]`, `[0xb41e04]`), the rest of `CGlueMgr::Login`: the idle
    /// test, an `OKAY` dialog for an empty box, else state 1, the `CANCEL` dialog and the logon.
    Login(GlueCredentials),
    /// `StatusDialogClick()` (`0x46d270` → `0x46b720`): the status dialog's button, per screen
    /// state.
    StatusDialogClick,
    /// `DisconnectFromServer()` (`0x46d340`): the world link's flag down and the link dropped if
    /// up (`0x5ab1a0`), then the logon cancelled (`0x5b3320`).
    DisconnectFromServer,
    /// `QuitGame()` (`0x46cea0` → `0x46b870` → `0x401560(0)`).
    QuitGame,
    /// `RequestRealmList([dialog])` (`0x46ecf0` → `0x46b8d0`): screen state 4, the status dialog
    /// when `dialog`, and a realm list query (`0x5ab840`).
    RequestRealmList { dialog: bool },
    /// `CancelRealmListQuery()` (`0x46ed10` → `0x46b7e0`): in screen state 4 only, the query
    /// completes cancelled and the state goes to 0; nothing is sent.
    CancelRealmListQuery,
    /// `RealmListDialogCancelled()` on the login screen (`0x46b810` → `CLoginMgr::Cancel`
    /// `0x5b3320`): the realmd link closes, with no dialog and no screen change.
    CancelLogon,
    /// `ChangeRealm` reaching `ConnectToRealm` (`0x46b210`) with this realm, `realmName` already
    /// set to it (`0x5ab7b0`).
    ChangeRealm(String),
    /// `GetCharacterListUpdate()` (`0x473270` → `0x4723d0` → `0x46b880`): the roster is already
    /// cleared in the VM; the host takes screen state 3, the `CHAR_LIST_RETRIEVING` dialog and
    /// the `CMSG_CHAR_ENUM`.
    GetCharacterList,
    /// `DeleteCharacter(index)` (`0x4734c0` → `0x46b2e0`) for a record with a guid: the host takes
    /// screen state 6, the `CHAR_DELETE_IN_PROGRESS` dialog and the `CMSG_CHAR_DELETE`.
    DeleteCharacter(u64),
    /// `RenameCharacter(index, name)` past its checks and the name check (`0x46b340` at
    /// `0x46b39b`): the host takes screen state 7, the `CHAR_RENAME_IN_PROGRESS` dialog and the
    /// `CMSG_CHAR_RENAME`.
    RenameCharacter { guid: u64, name: String },
    /// `EnterWorld()` (`0x46d3c0` → `CGlueMgr::EnterWorld` `0x46b500`) with the selection on the
    /// roster (`0x46b513`-`0x46b53e`): its 0-based index (`[0x83856c]`), guid (`+0x00`) and flags
    /// (`+0xfc`). The realm and link guards, the flag dialogs and the entry are the host's.
    EnterWorld { index: i32, guid: u64, flags: u32 },
    /// `CreateCharacter(name)` (`0x4719e0` → `0x4711d0`) past the name check: the host takes
    /// screen state 5, the `CHAR_CREATE_IN_PROGRESS` dialog and the `CMSG_CHAR_CREATE`
    /// (`0x46b290`).
    CreateCharacter(super::glue_create::GlueCreateRequest),
}

/// A glue string by its global's key, or as given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlueText {
    Key(&'static str),
    Raw(String),
}

/// One thing the glue manager raises in the glue VM: an event of the glue table, the roster
/// teardown that fires two, a CVar write, or the point where the manager's own next step runs.
/// The host's frame and a verb's in-place call fire them through the same path
/// ([`super::UiScript::fire_glue_signal`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlueSignal {
    /// `OPEN_STATUS_DIALOG(which[, text[, data]])` (event 3), the text resolved at the fire
    /// (`0x703bf0`).
    Open {
        which: &'static str,
        text: Option<GlueText>,
        data: Option<String>,
    },
    /// `UPDATE_STATUS_DIALOG(text[, button])` (event 4).
    Update {
        text: GlueText,
        button: Option<GlueText>,
    },
    /// `CLOSE_STATUS_DIALOG` (event 5).
    Close,
    /// `SET_GLUE_SCREEN(name)` (event 0, `0x46b840`).
    Screen(&'static str),
    /// `SET_GLUE_SCREEN(name)` when the current screen (`[0xb41478]`) is `current`, compared
    /// case-folded as it stands at the fire (`0x46bfa7`, `0x64a4c0`).
    ScreenFrom {
        current: &'static str,
        name: &'static str,
    },
    /// `DISCONNECTED_FROM_SERVER` (event 2, `0x46c62e`).
    Disconnected,
    /// The roster teardown (`0x472580`): `UPDATE_SELECTED_CHARACTER` with 1, then
    /// `CHARACTER_LIST_UPDATE`.
    Teardown,
    /// `SHOW_SERVER_ALERT(text)` (event 21, `0x46b964`).
    ServerAlert(String),
    /// `SELECT_FIRST_CHARACTER` (event 13, `0x46c1a2`).
    SelectFirst,
    /// `SELECT_LAST_CHARACTER` (event 12, `0x46c133`).
    SelectLast,
    /// `FORCE_RENAME_CHARACTER(key)` (event 23): the key, unresolved.
    ForceRename(&'static str),
    /// A CVar the manager set (`CVar::Set`, `0x63df50`), written to the VM's table as the
    /// engine's write.
    Cvar { name: &'static str, value: String },
    /// The manager's own step that follows the signals before it, run back in the host at this
    /// point of the fan-out ([`GlueHost::resume`]): an operation begun after its dialog's handlers
    /// have run, as the reference's verbs begin it after their `SignalEvent`.
    Resume(u32),
}

/// The glue manager a glue VM is seated with: the host side of `CGlueMgr` and the parts of
/// `ClientServices` it reads, which outlive the VM as the reference's C globals outlive its Lua
/// state.
pub trait GlueHost {
    /// Run `call` and return what it raised, in order, for the VM to fire before the verb
    /// returns. The network half it starts is the host's, sent on its next frame.
    fn call(&self, call: GlueCall) -> Vec<GlueSignal>;
    /// Run the step a [`GlueSignal::Resume`] names, once, and return what it raised, for the VM
    /// to fire in its place.
    fn resume(&self, step: u32) -> Vec<GlueSignal>;
    /// The world link's flag (`[svc+0x1b00]`), which `IsConnectedToServer` (`0x46d380`) reads.
    fn connected(&self) -> bool;
    /// The realm type of the realm selected this process (`0x5ab9a0`: `[0xc28134]`, the record at
    /// `0xc27fb8`), which `GetServerName` looks up.
    fn selected_realm_type(&self) -> Option<u32>;
}

/// The `ClientServices` status keys (`0x85cae8`, 83 entries), the `ResponseCodes` the world
/// server's `SMSG_AUTH_RESPONSE` byte indexes directly, and the `CHAR_NAME_*` block the name check
/// answers in (`0x45`-`0x52`).
pub const GLUE_STATUS_KEYS: [&str; 0x53] = [
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

/// `0x5abd20`: a status's key, the empty string (`0x882748`) for 0 and for anything past the table.
pub fn glue_status_key(code: i32) -> &'static str {
    match usize::try_from(code) {
        Ok(i @ 1..0x53) => GLUE_STATUS_KEYS[i],
        _ => "",
    }
}

/// The hosts `LaunchURL` opens, the 10-entry table at `0x85cc34`: a leading `*` stands for one
/// label, so `*.blizzard.com` takes `forums.blizzard.com` and not `a.b.blizzard.com`.
const LAUNCH_HOSTS: [&str; 10] = [
    "*.blizzard.com",
    "*.blizzcon.com",
    "www.battle.net",
    "*.worldofwarcraft.com",
    "www.battlenet.co.kr",
    "*.worldofwarcraft.co.kr",
    "*.blizzard-europe.com",
    "*.wow-europe.com",
    "*.wowchina.com",
    "*.wowtaiwan.com.tw",
];

/// The gate in front of the browser: `0x5abd00` tests `0x5abc10` and `0x429ff0` takes the URL.
/// The URL begins `http://` (`0x85d6bc`, compared case-folded), every byte after it is a letter,
/// a digit, `.`, `-` or `/` (so no query, port, escape or `_`), and its host, the text up to the
/// first `/`, is one of [`LAUNCH_HOSTS`], compared case-folded. `0x429ff0` refuses a URL of 0x400
/// bytes or more (`0x42a01c`). The reference works on a C string, so the verb cuts its argument
/// at a NUL before asking; a URL with a NUL in it is refused here.
///
/// Deviation: a host of 0x80 bytes or more is refused. The reference copies the host into a 0x80-byte
/// buffer (`0x5abc7c`-`0x5abc9c`) and writes its terminator one byte past the end, and the cut
/// host could end in a whitelisted suffix the real one does not; this takes the safe side.
pub fn launch_url_allowed(url: &str) -> bool {
    if url.len() >= 0x400 || url.contains('\0') {
        return false;
    }
    let url = url.as_bytes();
    const SCHEME: &[u8] = b"http://";
    let Some(rest) = url
        .get(..SCHEME.len())
        .filter(|head| head.eq_ignore_ascii_case(SCHEME))
        .map(|_| &url[SCHEME.len()..])
    else {
        return false;
    };
    if !rest
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'/'))
    {
        return false;
    }
    let host = rest.split(|&b| b == b'/').next().unwrap_or_default();
    if host.len() >= 0x80 {
        return false;
    }
    LAUNCH_HOSTS
        .iter()
        .any(|entry| match entry.strip_prefix('*') {
            // `SStrChr(host, '.')`, then the rest against the entry past its `*`.
            Some(suffix) => host
                .iter()
                .position(|&b| b == b'.')
                .is_some_and(|dot| host[dot..].eq_ignore_ascii_case(suffix.as_bytes())),
            None => host.eq_ignore_ascii_case(entry.as_bytes()),
        })
}

/// What the browser is given for an add-on's URL, or `None` for one benilla will not open. The
/// reference hands the server's string to `0x429ff0` with no whitelist (`0x46d5e0`): a C string of
/// fewer than 0x400 bytes, each `%XY` replaced by the byte it spells (table `0x801490`: a
/// non-hex digit reads 0; a `%` with fewer than two bytes after it ends the string), then
/// `ShellExecute "open"`.
///
/// Deviation: benilla keeps that decode and then encodes it again, every byte outside the URL
/// characters [`URL_SAFE`] as `%XY` (a decoded `%` too), because the opener on some platforms is a
/// shell script (`xdg-open`) or takes the raw command tail (`rundll32`) and the string comes off
/// the wire: no space, quote, backtick, `$`, `;`, `<`, `>`, `|`, `^`, `\`, brace or control byte
/// reaches it. The result must be an `http://` or `https://` (case-folded) address with a host,
/// and nothing else; the reference would open whatever the server sent, `file:` and program paths
/// included.
pub fn launch_addon_url(url: &str) -> Option<String> {
    if url.is_empty() || url.len() >= 0x400 {
        return None;
    }
    let hex = |b: u8| -> u8 {
        match b {
            b'1'..=b'9' => b - b'0',
            b'A'..=b'F' => b - b'A' + 10,
            b'a'..=b'f' => b - b'a' + 10,
            _ => 0,
        }
    };
    let bytes = url.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() && bytes[i] != 0 {
        let c = bytes[i];
        i += 1;
        if c != b'%' {
            decoded.push(c);
            continue;
        }
        let (Some(&hi), Some(&lo)) = (bytes.get(i), bytes.get(i + 1)) else {
            break;
        };
        if hi == 0 || lo == 0 {
            break;
        }
        i += 2;
        decoded.push(hex(hi) << 4 | hex(lo));
    }
    // The C string ends at a NUL, a decoded `%00` included.
    let end = decoded
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(decoded.len());
    let mut out = String::with_capacity(end);
    for &b in &decoded[..end] {
        if b != b'%' && URL_SAFE.contains(&b) || b.is_ascii_alphanumeric() {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    launch_addon_url_allowed(&out).then_some(out)
}

/// The bytes a URL may carry besides letters and digits when it is handed to the opener, the
/// unreserved set and the reserved ones no shell reads as syntax.
const URL_SAFE: &[u8] = b"-._~:/?#[]@!&()*+,=";

/// The host's own check of what [`launch_addon_url`] returned, and the whole of its gate: an
/// `http://` or `https://` address (case-folded) with a host that begins with a letter or digit,
/// and only letters, digits, [`URL_SAFE`] bytes and well-formed `%XY` escapes.
pub fn launch_addon_url_allowed(url: &str) -> bool {
    let bytes = url.as_bytes();
    let rest = [&b"http://"[..], &b"https://"[..]]
        .iter()
        .find_map(|scheme| {
            bytes
                .get(..scheme.len())
                .filter(|head| head.eq_ignore_ascii_case(scheme))
                .map(|_| &bytes[scheme.len()..])
        });
    let Some(rest) = rest else {
        return false;
    };
    if !rest.first().is_some_and(u8::is_ascii_alphanumeric) {
        return false;
    }
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        i += 1;
        if b == b'%' {
            let (Some(hi), Some(lo)) = (bytes.get(i), bytes.get(i + 1)) else {
                return false;
            };
            if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
                return false;
            }
            i += 2;
        } else if !(b.is_ascii_alphanumeric() || URL_SAFE.contains(&b)) {
            return false;
        }
    }
    true
}

/// The glue core's slots and queue in the VM's model.
#[derive(Clone, Default)]
pub(crate) struct GlueCore {
    requests: Vec<GlueRequest>,
    /// The glue manager the verbs call in place, once the host has seated this VM.
    host: Option<std::rc::Rc<dyn GlueHost>>,
    /// Calls made before any host was seated, for the one that seats this VM to run.
    calls: Vec<GlueCall>,
    /// The agreements' flag pairs (`[0xb41df8]`-`[0xb41e14]`): read off their CVars when the glue
    /// starts, set by their accepts, and read by the getters and the login's first two tests.
    notices: [NoticeState; 4],
    /// The current glue screen's name (`[0xb41478]`), which `SetCurrentScreen` (`0x46ce60` →
    /// `0x46b860`) copies into a 0x40-byte buffer.
    pub(super) screen: String,
    /// `HideCursor` / `ShowCursor` (`0x46db70`/`0x46db60`, the device's `0x58b590`): whether the
    /// glue has the pointer hidden. The host feeds it to the cursor driver. Its callers are the
    /// credits frame (`CreditsFrame.lua:75`, `CreditsFrame.xml:456`) and the movie frame
    /// (`MovieFrame.lua:36`, `:49`).
    cursor_hidden: bool,
    /// The realm list the realm verbs read ([`super::glue_realm`]).
    pub(super) realms: super::glue_realm::GlueRealmList,
    /// The character roster and the select verbs' cells ([`super::glue_select`]).
    pub(super) roster: super::glue_select::GlueRoster,
    /// The create screen's state ([`super::glue_create`]).
    pub(super) create: super::glue_create::GlueCreate,
    /// The AddOns screen's enable nodes ([`super::glue_addons`]).
    pub(super) addons: super::glue_addons::GlueAddons,
    /// The character name check's lists ([`super::char_name`]), as the host seated them.
    name_lists: Option<std::rc::Rc<dyn super::NameLists>>,
}

impl GlueCore {
    /// The name check's lists; with none seated, nothing is reserved.
    pub(super) fn name_lists(&self) -> std::rc::Rc<dyn super::NameLists> {
        self.name_lists
            .clone()
            .unwrap_or_else(|| std::rc::Rc::new(super::NoNameLists))
    }
}

/// `SetCurrentScreen`'s buffer, 0x40 bytes with its terminator (`0x46b860`, `SStrCopy`).
const SCREEN_NAME_MAX: usize = 0x3f;

impl super::UiScript {
    /// Drain the glue verbs' requests since the last call.
    pub fn take_glue_requests(&mut self) -> Vec<GlueRequest> {
        std::mem::take(&mut self.model_mut().glue.requests)
    }

    /// Seat the glue manager the verbs call in place ([`GlueHost`]).
    pub fn set_glue_host(&mut self, host: std::rc::Rc<dyn GlueHost>) {
        self.model_mut().glue.host = Some(host);
    }

    /// Drain the manager calls made while no host was seated, for the host to run now.
    pub fn take_glue_calls(&mut self) -> Vec<GlueCall> {
        std::mem::take(&mut self.model_mut().glue.calls)
    }

    /// Fire one of the glue manager's signals into this VM, as the manager's frame raises it.
    pub fn fire_glue_signal(&mut self, signal: GlueSignal) {
        fire_signal(&self.lua, signal);
    }

    /// Seat the lists the character name check searches (`0x6c91a0` compiles them at startup).
    pub fn set_glue_name_lists(&mut self, lists: std::rc::Rc<dyn super::NameLists>) {
        self.model_mut().glue.name_lists = Some(lists);
    }

    /// Seed one agreement's flags.
    pub fn set_glue_notice(&mut self, notice: GlueNotice, state: NoticeState) {
        self.model_mut().glue.notices[notice.index()] = state;
    }

    /// One agreement's flags as the VM holds them.
    pub fn glue_notice(&self, notice: GlueNotice) -> NoticeState {
        self.model_ref().glue.notices[notice.index()]
    }

    /// Whether the glue has hidden the pointer (`HideCursor`) and not shown it again.
    pub fn glue_cursor_hidden(&self) -> bool {
        self.model_ref().glue.cursor_hidden
    }

    /// The current glue screen's name, as `SetCurrentScreen` last set it.
    pub fn glue_screen(&self) -> String {
        self.model_ref().glue.screen.clone()
    }

    /// Seed the current glue screen's name. The buffer is the client's (`[0xb41478]`), which a
    /// glue rebuild (`0x46a7b0`) leaves as it was, so the host carries it into the new VM.
    pub fn set_glue_screen_name(&mut self, name: &str) {
        self.model_mut().glue.screen = name.to_string();
    }

    /// A string global's text, `GetGlobalString`'s answer (`0x703bf0` with no gender and no
    /// ordinal takes `0x704350`): the global's string (a number stringified), else the empty
    /// string (`0x882748`).
    pub fn global_text(&self, key: &str) -> String {
        global_text(&self.lua, key)
    }
}

pub(super) fn queue(lua: &Lua, request: GlueRequest) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .glue
        .requests
        .push(request);
}

/// Run one of the glue manager's functions inside the verb: the seated host runs its half, then
/// everything it raised fires before the binding returns, as the reference's verbs call `CGlueMgr`
/// directly and its `SignalEvent` (`0x703e50`, `0x703f50`) walks the listeners in place. A handler
/// that calls another verb runs that call inside this fan-out. With no host seated yet the call
/// waits for the one that seats this VM.
pub(super) fn call(lua: &Lua, call: GlueCall) {
    let host = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        match model.glue.host.clone() {
            Some(host) => host,
            None => {
                model.glue.calls.push(call);
                return;
            }
        }
    };
    for signal in host.call(call) {
        fire_signal(lua, signal);
    }
}

/// Fire one signal, its text resolved through the VM's own strings (`0x703bf0`).
fn fire_signal(lua: &Lua, signal: GlueSignal) {
    let text = |t: &GlueText| match t {
        GlueText::Key(k) => global_text(lua, k),
        GlueText::Raw(s) => s.clone(),
    };
    let fire = |event: &str, args: Vec<ScriptValue>| super::tick::fire_event_into(lua, event, args);
    match signal {
        GlueSignal::Open {
            which,
            text: t,
            data,
        } => {
            let mut args = vec![ScriptValue::Str(which.to_string())];
            if t.is_some() || data.is_some() {
                args.push(t.map_or(ScriptValue::Nil, |t| ScriptValue::Str(text(&t))));
            }
            if let Some(d) = data {
                args.push(ScriptValue::Str(d));
            }
            fire("OPEN_STATUS_DIALOG", args);
        }
        GlueSignal::Update { text: t, button } => {
            let mut args = vec![ScriptValue::Str(text(&t))];
            if let Some(b) = button {
                args.push(ScriptValue::Str(text(&b)));
            }
            fire("UPDATE_STATUS_DIALOG", args);
        }
        GlueSignal::Close => fire("CLOSE_STATUS_DIALOG", Vec::new()),
        GlueSignal::Screen(name) => {
            fire("SET_GLUE_SCREEN", vec![ScriptValue::Str(name.to_string())]);
        }
        GlueSignal::ScreenFrom { current, name } => {
            let here = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .glue
                .screen
                .eq_ignore_ascii_case(current);
            if here {
                fire("SET_GLUE_SCREEN", vec![ScriptValue::Str(name.to_string())]);
            }
        }
        GlueSignal::Disconnected => fire("DISCONNECTED_FROM_SERVER", Vec::new()),
        GlueSignal::Teardown => super::glue_select::teardown_roster(lua),
        GlueSignal::ServerAlert(text) => fire("SHOW_SERVER_ALERT", vec![ScriptValue::Str(text)]),
        GlueSignal::SelectFirst => fire("SELECT_FIRST_CHARACTER", Vec::new()),
        GlueSignal::SelectLast => fire("SELECT_LAST_CHARACTER", Vec::new()),
        GlueSignal::ForceRename(key) => {
            fire(
                "FORCE_RENAME_CHARACTER",
                vec![ScriptValue::Str(key.to_string())],
            );
        }
        GlueSignal::Cvar { name, value } => {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            super::cvars::set_from_engine(&mut model, name, value);
        }
        GlueSignal::Resume(step) => {
            let host = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .glue
                .host
                .clone();
            if let Some(host) = host {
                for signal in host.resume(step) {
                    fire_signal(lua, signal);
                }
            }
        }
    }
}

/// The text of a string global, as [`super::UiScript::global_text`] reads it.
pub(super) fn global_text(lua: &Lua, key: &str) -> String {
    match lua.globals().get::<Value>(key) {
        Ok(v @ (Value::String(_) | Value::Integer(_) | Value::Number(_))) => lua
            .coerce_string(v)
            .ok()
            .flatten()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // `Screenshot` (`0x46d3d0`): the in-game verb's capture ask, answered with the glue's events.
    super::screenshot::install(lua)?;
    let g = lua.globals();

    // Both arguments `is-number-or-string` (`0x46d168`, `0x46d178`), else the usage error
    // (`0x837cbc`); then `CGlueMgr::Login(account, password)` (`0x46d1a0`), whose first two tests
    // are the TOS and EULA flags (`0x46afb0`, `0x46afc0`): either unaccepted, it returns.
    g.set(
        "DefaultServerLogin",
        lua.create_function(|lua, (account, password): (Value, Value)| {
            const USAGE: &str = "Usage: DefaultServerLogin(\"accountName\", \"password\")";
            let ok =
                |v: &Value| matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_));
            if !ok(&account) || !ok(&password) {
                return Err(mlua::Error::RuntimeError(USAGE.into()));
            }
            let account = string_arg(lua, account, USAGE)?;
            let password = string_arg(lua, password, USAGE)?;
            let agreed = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let n = &model.glue.notices;
                n[GlueNotice::Tos.index()].accepted && n[GlueNotice::Eula.index()].accepted
            };
            if agreed {
                call(lua, GlueCall::Login(GlueCredentials { account, password }));
            }
            Ok(())
        })?,
    )?;

    g.set(
        "StatusDialogClick",
        lua.create_function(|lua, _: MultiValue| {
            call(lua, GlueCall::StatusDialogClick);
            Ok(())
        })?,
    )?;

    // The `accountName` CVar's string (`0x46cdd5`, `[[0x8826c8]+0x20]`), the saved, script-visible
    // record registered at `0x402bd2`: `GetCVar("accountName")` reads the same text.
    g.set(
        "GetSavedAccountName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let value = model
                .cvars
                .get(CVAR_ACCOUNT_NAME_KEY)
                .map_or("", |slot| slot.value.as_str());
            lua.create_string(value)
        })?,
    )?;
    // A string argument (`0x46cdf8`) or `Usage: SetSavedAccountName("accountName")` (`0x837c28`),
    // then `CVar::Set(s, 1, 0, 0, 1)` on that record (`0x46ce13`-`0x46ce2e`), a write like
    // `SetCVar`'s that the host persists, so the next `GetSavedAccountName` reads it back at once.
    g.set(
        "SetSavedAccountName",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: SetSavedAccountName(\"accountName\")")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            super::cvars::write_cvar(&mut model, "accountName", name);
            Ok(())
        })?,
    )?;

    // Each agreement: `Show*Notice` and `*Accepted` push 1 or nil off the flag pair the glue
    // manager seeded (`[0xb41df8]`/`[0xb41dfc]` for the TOS, `[0xb41e00]`/`[0xb41e04]` the EULA,
    // `[0xb41e08]`/`[0xb41e0c]` scanning, `[0xb41e10]`/`[0xb41e14]` the contest); `Accept*` sets
    // accepted, clears the notice and sets the CVar to "1" (`0x46aef0`, `0x46af20`, `0x46af50`,
    // `0x46af80`), all in the call.
    for (notice, show, accepted, accept) in [
        (GlueNotice::Tos, "ShowTOSNotice", "TOSAccepted", "AcceptTOS"),
        (
            GlueNotice::Eula,
            "ShowEULANotice",
            "EULAAccepted",
            "AcceptEULA",
        ),
        (
            GlueNotice::Scanning,
            "ShowScanningNotice",
            "ScanningAccepted",
            "AcceptScanning",
        ),
        (
            GlueNotice::Contest,
            "ShowContestNotice",
            "ContestAccepted",
            "AcceptContest",
        ),
    ] {
        let i = notice.index();
        g.set(
            show,
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(flag(model.glue.notices[i].show_notice))
            })?,
        )?;
        g.set(
            accepted,
            lua.create_function(move |lua, _: MultiValue| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(flag(model.glue.notices[i].accepted))
            })?,
        )?;
        g.set(
            accept,
            lua.create_function(move |lua, _: MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.glue.notices[i] = NoticeState {
                    accepted: true,
                    show_notice: false,
                };
                super::cvars::set_from_engine(&mut model, notice.cvar(), "1".to_string());
                Ok(())
            })?,
        )?;
    }

    // Five strings (`0x46cd70`): the version type and build type through `GetGlobalString`
    // (`BETA_BUILD` `0x837c1c`, `RELEASE_BUILD` `0x837c0c`), then the version, the build and the
    // date as the binary spells them. The in-game binding (`0x4884a0`) has the last three only.
    g.set(
        "GetBuildInfo",
        lua.create_function(|lua, _: MultiValue| {
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(global_text(lua, "BETA_BUILD"))?),
                Value::String(lua.create_string(global_text(lua, "RELEASE_BUILD"))?),
                Value::String(lua.create_string(super::client::VERSION)?),
                Value::String(lua.create_string(super::client::BUILD)?),
                Value::String(lua.create_string(super::client::BUILD_DATE)?),
            ]))
        })?,
    )?;

    // The locale name table `0x8558a4` at the locale index `[0xc0e080]` (`0x46ce40`), the same
    // answer as the in-game binding.
    g.set(
        "GetLocale",
        lua.create_function(|lua, _: MultiValue| {
            lua.create_string(super::client::locale_name(lua))
        })?,
    )?;

    // A string argument (`0x46ce68`) or `Usage: SetCurrentScreen("screen")` (`0x837c54`), copied
    // into the 0x40-byte screen buffer (`0x46b860`).
    g.set(
        "SetCurrentScreen",
        lua.create_function(|lua, name: Value| {
            let mut name = string_arg(lua, name, "Usage: SetCurrentScreen(\"screen\")")?;
            if name.len() > SCREEN_NAME_MAX {
                let mut end = SCREEN_NAME_MAX;
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                name.truncate(end);
            }
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .glue
                .screen = name;
            Ok(())
        })?,
    )?;

    // Three values (`0x46d280`): the `realmName` CVar's string, always a string (`0x46d289` →
    // `0x5ab7d0`), then PvP and RP as 1 or nil off the selected realm's `Cfg_Configs` row
    // (`0x46d29c` → `0x5ab9a0`, the walk at `0x46d2a5`-`0x46d30d`: `[row+8]`, `[row+0xc]`), link
    // or no link; nil, nil with no realm selected or no row for its type.
    g.set(
        "GetServerName",
        lua.create_function(|lua, _: MultiValue| {
            let (name, host) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                (
                    super::glue_realm::realm_name(&model),
                    model.glue.host.clone(),
                )
            };
            let selected = host.and_then(|h| h.selected_realm_type());
            let (pvp, rp) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                selected.map_or((false, false), |t| model.glue.realms.type_of(t))
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&name)?),
                flag(pvp),
                flag(rp),
            ]))
        })?,
    )?;

    // The host's link flag as it stands; with no host seated, none.
    g.set(
        "IsConnectedToServer",
        lua.create_function(|lua, _: MultiValue| {
            let host = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .glue
                .host
                .clone();
            Ok(flag(host.is_some_and(|h| h.connected())))
        })?,
    )?;
    g.set(
        "DisconnectFromServer",
        lua.create_function(|lua, _: MultiValue| {
            call(lua, GlueCall::DisconnectFromServer);
            Ok(())
        })?,
    )?;
    g.set(
        "QuitGame",
        lua.create_function(|lua, _: MultiValue| {
            call(lua, GlueCall::QuitGame);
            Ok(())
        })?,
    )?;

    // A string argument (`0x46ceb8`) or `Usage: PlayGlueMusic("filename")` (`0x837c78`).
    g.set(
        "PlayGlueMusic",
        lua.create_function(|lua, file: Value| {
            let file = string_arg(lua, file, "Usage: PlayGlueMusic(\"filename\")")?;
            queue(lua, GlueRequest::PlayGlueMusic(file));
            Ok(())
        })?,
    )?;
    // A string argument (`0x46cf68`) or `Usage: LaunchURL("URL")` (`0x837ca4`), then `0x5abd00`,
    // which opens the browser for a URL `0x5abc10` passes and does nothing for any other. The
    // in-game state has no such verb.
    g.set(
        "LaunchURL",
        lua.create_function(|lua, url: Value| {
            let url = string_arg(lua, url, "Usage: LaunchURL(\"URL\")")?;
            // The C string ends at a NUL (`0x6f3690`), for the gate and for the shell alike.
            let url = url.split('\0').next().unwrap_or_default();
            if launch_url_allowed(url) {
                queue(lua, GlueRequest::LaunchUrl(url.to_string()));
            }
            Ok(())
        })?,
    )?;
    // No argument read and no result (`0x46cef0` calls `0x45b000` and returns 0).
    g.set(
        "PlayCreditsMusic",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::PlayCreditsMusic);
            Ok(())
        })?,
    )?;
    g.set(
        "StopGlueMusic",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StopGlueMusic);
            Ok(())
        })?,
    )?;

    // One value (`0x46dbc0`): 1 when the `movieSubtitle` CVar's integer (`[0x882640]+0x28`) is
    // nonzero, else nil.
    g.set(
        "GetMovieSubtitles",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let on = model
                .cvars
                .get("moviesubtitle")
                .is_some_and(|c| s_str_to_int(&c.value) != 0);
            Ok(flag(on))
        })?,
    )?;
    // One number (`0x46cf10`): the width of the `gxResolution` CVar's string, scanned as `"%dx%d"`
    // (`0x837c9c`). A string with no leading integer leaves the scan's stack slot unwritten in the
    // reference; 0 here.
    g.set(
        "GetMovieResolution",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let width = model
                .cvars
                .get("gxresolution")
                .map_or(0, |c| scanf_d(&c.value).unwrap_or(0));
            Ok(width as f64)
        })?,
    )?;
    // The device's pointer off (`0x46db70` → `0x58b590(0)`) and on (`0x46db60` → `0x58b590(1)`).
    for (name, hidden) in [("HideCursor", true), ("ShowCursor", false)] {
        g.set(
            name,
            lua.create_function(move |lua, _: MultiValue| {
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .glue
                    .cursor_hidden = hidden;
                Ok(())
            })?,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// A host that answers each call with the signals it was given for that call's name, and
    /// records the calls in order.
    #[derive(Default)]
    struct FakeHost {
        calls: std::cell::RefCell<Vec<String>>,
        replies: std::cell::RefCell<Vec<(&'static str, Vec<GlueSignal>)>>,
        connected: std::cell::Cell<bool>,
        selected: std::cell::Cell<Option<u32>>,
    }

    impl GlueHost for FakeHost {
        fn call(&self, call: GlueCall) -> Vec<GlueSignal> {
            let name = format!("{call:?}");
            self.calls.borrow_mut().push(name.clone());
            self.replies
                .borrow()
                .iter()
                .find(|(n, _)| name.starts_with(n))
                .map(|(_, s)| s.clone())
                .unwrap_or_default()
        }

        fn resume(&self, step: u32) -> Vec<GlueSignal> {
            self.calls.borrow_mut().push(format!("resume {step}"));
            Vec::new()
        }

        fn connected(&self) -> bool {
            self.connected.get()
        }

        fn selected_realm_type(&self) -> Option<u32> {
            self.selected.get()
        }
    }

    fn seated(replies: Vec<(&'static str, Vec<GlueSignal>)>) -> (UiScript, std::rc::Rc<FakeHost>) {
        let mut s = UiScript::new_glue().unwrap();
        let host = std::rc::Rc::new(FakeHost::default());
        *host.replies.borrow_mut() = replies;
        s.set_glue_host(host.clone());
        (s, host)
    }

    /// A seated host runs the call inside the verb, and what it raised has run its handlers by
    /// the verb's next Lua line, in order, with nothing left for a tick or a queue.
    #[test]
    fn a_seated_host_runs_the_call_and_its_signals_fire_before_the_verb_returns() {
        let (mut s, host) = seated(vec![(
            "StatusDialogClick",
            vec![
                GlueSignal::Teardown,
                GlueSignal::Open {
                    which: "CANCEL",
                    text: Some(GlueText::Key("CHAR_LIST_RETRIEVING")),
                    data: None,
                },
            ],
        )]);
        s.run("CHAR_LIST_RETRIEVING = 'Retrieving character list'")
            .unwrap();
        super::super::tick::heard::listen(
            &s,
            &[
                "UPDATE_SELECTED_CHARACTER",
                "CHARACTER_LIST_UPDATE",
                "OPEN_STATUS_DIALOG",
            ],
            Some("arg2"),
        );
        let seen: Vec<String> = s
            .eval("StatusDialogClick() local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(
            seen,
            [
                "UPDATE_SELECTED_CHARACTER nil",
                "CHARACTER_LIST_UPDATE nil",
                "OPEN_STATUS_DIALOG Retrieving character list"
            ]
        );
        assert_eq!(*host.calls.borrow(), ["StatusDialogClick"]);
        assert!(s.take_glue_calls().is_empty());
        assert!(s.model_ref().pending_events.is_empty());
    }

    /// A handler of a call's event that calls another verb runs that call inside the fan-out, as
    /// the stock `OKAY` dialog's `OnShow` presses `StatusDialogClick` inside `OPEN_STATUS_DIALOG`.
    #[test]
    fn a_handler_that_calls_a_verb_runs_its_call_inside_the_first() {
        let open = |which| GlueSignal::Open {
            which,
            text: None,
            data: None,
        };
        let (s, host) = seated(vec![
            ("GetCharacterList", vec![open("OKAY")]),
            ("StatusDialogClick", vec![open("CANCEL")]),
        ]);
        s.run(
            "SEEN = {} F = CreateFrame('Frame') F:RegisterEvent('OPEN_STATUS_DIALOG') \
             F:SetScript('OnEvent', function() table.insert(SEEN, arg1) \
               if arg1 == 'OKAY' then StatusDialogClick() end end)",
        )
        .unwrap();
        let seen: Vec<String> = s.eval("GetCharacterListUpdate() return SEEN").unwrap();
        assert_eq!(seen, ["OKAY", "CANCEL"]);
        assert_eq!(
            *host.calls.borrow(),
            ["GetCharacterList", "StatusDialogClick"]
        );
    }

    /// `IsConnectedToServer` reads the host's flag as it stands, nil with no host seated.
    #[test]
    fn is_connected_to_server_reads_the_hosts_flag() {
        let bare = UiScript::new_glue().unwrap();
        assert!(bare
            .eval::<bool>("return IsConnectedToServer() == nil")
            .unwrap());
        let (s, host) = seated(Vec::new());
        assert!(s
            .eval::<bool>("return IsConnectedToServer() == nil")
            .unwrap());
        host.connected.set(true);
        assert_eq!(
            s.eval::<Option<i64>>("return IsConnectedToServer()")
                .unwrap(),
            Some(1)
        );
    }

    /// Every manager verb reaches the host in its call.
    #[test]
    fn every_manager_verb_calls_the_host() {
        let (s, host) = seated(Vec::new());
        s.run(
            "DisconnectFromServer() QuitGame() RequestRealmList(1) CancelRealmListQuery() \
             SetCurrentScreen('login') RealmListDialogCancelled()",
        )
        .unwrap();
        assert_eq!(
            *host.calls.borrow(),
            [
                "DisconnectFromServer",
                "QuitGame",
                "RequestRealmList { dialog: true }",
                "CancelRealmListQuery",
                "CancelLogon"
            ]
        );
    }

    #[test]
    fn a_notice_cvar_reads_as_atoi() {
        let s = |v| NoticeState::from_cvar(v);
        assert_eq!(s("0"), NoticeState::default());
        assert!(s("1").accepted && !s("1").show_notice);
        assert!(s("-1").show_notice && !s("-1").accepted);
        assert!(!s("2").accepted && !s("2").show_notice);
        assert!(s("1x").accepted, "atoi stops at the first non-digit");
        assert_eq!(s("x"), NoticeState::default());
    }

    /// With no host seated the call waits for one; the agreements are the login's first tests
    /// (`0x46afb0`, `0x46afc0`), so with either unaccepted nothing is called.
    #[test]
    fn default_server_login_calls_with_the_strings_or_raises_its_usage() {
        let mut s = UiScript::new_glue().unwrap();
        s.run("DefaultServerLogin('probe', 1234)").unwrap();
        assert!(s.take_glue_calls().is_empty(), "no agreements, no login");
        s.set_glue_notice(GlueNotice::Tos, NoticeState::from_cvar("1"));
        s.run("DefaultServerLogin('probe', 1234)").unwrap();
        assert!(s.take_glue_calls().is_empty(), "the EULA still stands");
        s.set_glue_notice(GlueNotice::Eula, NoticeState::from_cvar("1"));
        s.run("DefaultServerLogin('probe', 1234)").unwrap();
        let calls = s.take_glue_calls();
        match calls.as_slice() {
            [GlueCall::Login(c)] => {
                assert_eq!((c.account.as_str(), c.password.as_str()), ("probe", "1234"));
                assert!(
                    !format!("{c:?}").contains("1234"),
                    "the password is never printed"
                );
            }
            other => panic!("{other:?}"),
        }
        let err = s
            .run("DefaultServerLogin('probe')")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Usage: DefaultServerLogin(\"accountName\", \"password\")"),
            "{err}"
        );
        assert!(s.take_glue_calls().is_empty());
    }

    /// An agreement's flags come off its record's integer (`0x46a41a` reads `+0x28`), Storm's
    /// `SStrToInt`: no whitespace skip, no `+`, a 32-bit wrap.
    #[test]
    fn an_agreement_reads_its_cvar_as_the_records_integer() {
        let accepted = |text: &str| NoticeState::from_cvar(text).accepted;
        assert!(accepted("1"));
        assert!(accepted("1abc"));
        assert!(!accepted(" 1"), "no whitespace skip");
        assert!(!accepted("+1"), "no unary plus");
        assert!(accepted("4294967297"), "the register wraps");
        assert!(NoticeState::from_cvar("-1").show_notice);
        assert!(!NoticeState::from_cvar(" -1").show_notice);
    }

    /// `AcceptEULA` sets its flag pair and its CVar in the call (`0x46af20`).
    #[test]
    fn the_agreements_answer_their_seeded_flags_and_accept_sets_them() {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([("readEULA", "0")]);
        s.set_glue_notice(
            GlueNotice::Eula,
            NoticeState {
                accepted: false,
                show_notice: true,
            },
        );
        s.set_glue_notice(GlueNotice::Tos, NoticeState::from_cvar("1"));
        let read = |s: &UiScript| {
            s.eval::<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)>(
                "return EULAAccepted(), ShowEULANotice(), TOSAccepted(), ScanningAccepted()",
            )
            .unwrap()
        };
        assert_eq!(read(&s), (None, Some(1), Some(1), None));
        s.run("AcceptEULA()").unwrap();
        assert_eq!(s.cvar("readEULA").as_deref(), Some("1"));
        assert_eq!(
            s.take_cvar_changes(),
            [("readEULA".to_string(), "1".to_string())],
            "the write is the engine's, for the CVar sync to persist"
        );
        assert_eq!(read(&s), (Some(1), None, Some(1), None));
        assert!(s.take_glue_requests().is_empty() && s.take_glue_calls().is_empty());
    }

    /// The saved account name is the `accountName` CVar (`0x46cdd0`, `0x46cdf0`): what one side
    /// writes the other reads, and the write queues for the host like a `SetCVar`.
    #[test]
    fn the_saved_account_is_the_account_name_cvar() {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([("accountName", "")]);
        s.set_cvar_host("accountName", "PROBE");
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            "PROBE"
        );
        s.run("SetSavedAccountName('Other')").unwrap();
        assert_eq!(s.cvar("accountName").as_deref(), Some("Other"));
        s.run("SetSavedAccountName('')").unwrap();
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            ""
        );
        assert_eq!(
            s.take_cvar_changes(),
            [("accountName", "Other"), ("accountName", "")]
                .map(|(n, v)| (n.to_string(), v.to_string()))
        );
        assert!(s.take_glue_requests().is_empty());
        let err = s.run("SetSavedAccountName()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: SetSavedAccountName(\"accountName\")"),
            "{err}"
        );
    }

    /// `0x5abc10`: the scheme, the character set past it and the host table.
    #[test]
    fn launch_url_opens_only_the_whitelisted_hosts() {
        let ok = [
            "http://www.worldofwarcraft.com/",
            "HTTP://WWW.WorldOfWarcraft.com/index.html",
            "http://forums.blizzard.com",
            "http://us.blizzard.com/en-us/alert",
            "http://www.battle.net/x",
            "http://www.wow-europe.com/en/info/theburningcrusade.html",
            "http://status.wow-europe.com/en/alert",
        ];
        for url in ok {
            assert!(launch_url_allowed(url), "{url}");
        }
        let refused = [
            "https://www.blizzard.com/",
            "ftp://www.blizzard.com/",
            "www.blizzard.com",
            "http://blizzard.com/",
            "http://a.b.blizzard.com/",
            "http://example.com/",
            "http://example.com/www.blizzard.com",
            "http://www.blizzard.com.evil.net/",
            "http://battle.net/",
            "http://www.blizzard.com/?q=1",
            "http://www.blizzard.com:80/",
            "http://www.blizzard.com/a_b",
            "http://www.blizzard.com/a b",
            "http://www.blizzard.com\n/",
            "",
        ];
        for url in refused {
            assert!(!launch_url_allowed(url), "{url:?}");
        }
        // The gate takes a C string: a NUL inside one is refused outright.
        assert!(!launch_url_allowed("http://www.blizzard.com/\0?q=1"));
        // 0x400 bytes or more is refused (`0x42a01c`); one byte under is not.
        let path = |n: usize| format!("http://www.blizzard.com/{}", "a".repeat(n - 24));
        assert!(launch_url_allowed(&path(0x3ff)));
        assert!(!launch_url_allowed(&path(0x400)));
        // A host of 0x80 bytes or more is refused rather than cut to a whitelisted tail.
        let host = |n: usize| format!("http://{}.blizzard.com/", "a".repeat(n - 13));
        assert!(launch_url_allowed(&host(0x7f)));
        assert!(!launch_url_allowed(&host(0x80)));
        let cut = format!("http://{}.blizzard.com.evil.net/", "a".repeat(0x80 - 13));
        assert!(!launch_url_allowed(&cut));
    }

    #[test]
    fn play_credits_music_queues_its_request_and_returns_nothing() {
        let mut s = UiScript::new_glue().unwrap();
        let n: i64 = s
            .eval("return table.getn({PlayCreditsMusic()})")
            .expect("the binding exists");
        assert_eq!(n, 0, "`0x46cef0` returns 0 results");
        s.run("PlayCreditsMusic('ignored', 1)").unwrap();
        assert!(matches!(
            s.take_glue_requests().as_slice(),
            [GlueRequest::PlayCreditsMusic, GlueRequest::PlayCreditsMusic]
        ));
    }

    #[test]
    fn launch_url_queues_a_passing_url_and_raises_its_usage() {
        let mut s = UiScript::new_glue().unwrap();
        s.run("LaunchURL('http://www.worldofwarcraft.com/')")
            .unwrap();
        s.run("LaunchURL('http://example.com/')").unwrap();
        // The text before a NUL is the argument, for the gate and for the opener.
        s.run("LaunchURL('http://www.blizzard.com/a\\0?q=1')")
            .unwrap();
        s.run("LaunchURL('http://example.com/\\0http://www.blizzard.com/')")
            .unwrap();
        let queued = s.take_glue_requests();
        assert!(
            matches!(
                queued.as_slice(),
                [GlueRequest::LaunchUrl(a), GlueRequest::LaunchUrl(b)]
                    if a == "http://www.worldofwarcraft.com/" && b == "http://www.blizzard.com/a"
            ),
            "{queued:?}"
        );
        let err = s.run("LaunchURL()").unwrap_err().to_string();
        assert!(err.contains("Usage: LaunchURL(\"URL\")"), "{err}");
        assert!(s.take_glue_requests().is_empty());
    }

    #[test]
    fn the_glue_build_info_is_five_strings() {
        let s = UiScript::new_glue().unwrap();
        s.run("BETA_BUILD = 'Version' RELEASE_BUILD = 'Release'")
            .unwrap();
        assert_eq!(
            s.eval::<Vec<String>>("return { GetBuildInfo() }").unwrap(),
            ["Version", "Release", "1.12.1", "5875", "Sep 19 2006"]
        );
    }

    /// The name is the `realmName` CVar's text as it stands, whatever its case; the flags are the
    /// selected realm type's `Cfg_Configs` row, nil, nil with no host, no selection or no row.
    #[test]
    fn the_server_name_is_the_cvar_and_the_flags_the_selected_realms() {
        let bare = UiScript::new_glue().unwrap();
        assert_eq!(
            bare.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap(),
            (String::new(), None, None)
        );
        let (mut s, host) = seated(Vec::new());
        s.register_cvars([("realmName", "")]);
        s.set_glue_realm_setup(
            Vec::new(),
            vec![super::super::GlueRealmType {
                realm_type: 1,
                pvp: true,
                rp: false,
            }],
        );
        s.set_cvar_host("realmName", "nOSTALRIUS");
        let read = |s: &UiScript| {
            s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap()
        };
        assert_eq!(read(&s), ("nOSTALRIUS".to_string(), None, None));
        host.selected.set(Some(1));
        assert_eq!(read(&s), ("nOSTALRIUS".to_string(), Some(1), None));
        host.selected.set(Some(7));
        assert_eq!(read(&s), ("nOSTALRIUS".to_string(), None, None), "no row");
    }

    #[test]
    fn set_current_screen_keeps_the_name_in_its_buffer() {
        let s = UiScript::new_glue().unwrap();
        s.run("SetCurrentScreen('login')").unwrap();
        assert_eq!(s.glue_screen(), "login");
        s.run(&format!("SetCurrentScreen('{}')", "x".repeat(100)))
            .unwrap();
        assert_eq!(s.glue_screen().len(), 0x3f);
        assert!(s.run("SetCurrentScreen()").is_err());
    }
    /// `GetMovieSubtitles` is 1 or nil off the `movieSubtitle` CVar's integer (`0x46dbc0`),
    /// `GetMovieResolution` the width of `gxResolution` (`0x46cf10`), and the cursor pair moves
    /// the hidden flag (`0x46db70`, `0x46db60`).
    #[test]
    fn the_movie_verbs_read_their_cvars_and_move_the_pointer_flag() {
        let mut s = UiScript::new_glue().unwrap();
        s.register_cvars([("movieSubtitle", "0"), ("gxResolution", "1024x768")]);
        assert!(s.eval::<bool>("return GetMovieSubtitles() == nil").unwrap());
        s.set_cvar_host("movieSubtitle", "1");
        assert_eq!(
            s.eval::<Option<i64>>("return GetMovieSubtitles()").unwrap(),
            Some(1)
        );
        s.set_cvar_host("movieSubtitle", "0");
        assert!(s.eval::<bool>("return GetMovieSubtitles() == nil").unwrap());

        assert_eq!(
            s.eval::<f64>("return GetMovieResolution()").unwrap(),
            1024.0
        );
        s.set_cvar_host("gxResolution", "800x600");
        assert_eq!(s.eval::<f64>("return GetMovieResolution()").unwrap(), 800.0);
        // `GetMovieSubtitles` reads the record's integer (`0x46dbc5`, `+0x28`), `SStrToInt`: a
        // leading space or `+` is 0. `GetMovieResolution` scans the text with `sscanf`
        // (`0x46cf34`), which skips whitespace and takes a sign.
        for text in [" 1", "+1"] {
            s.set_cvar_host("movieSubtitle", text);
            assert!(
                s.eval::<bool>("return GetMovieSubtitles() == nil").unwrap(),
                "{text:?}"
            );
        }
        s.set_cvar_host("movieSubtitle", "1x");
        assert_eq!(
            s.eval::<Option<i64>>("return GetMovieSubtitles()").unwrap(),
            Some(1)
        );
        s.set_cvar_host("gxResolution", " 640x480");
        assert_eq!(s.eval::<f64>("return GetMovieResolution()").unwrap(), 640.0);
        s.set_cvar_host("gxResolution", "+1280x960");
        assert_eq!(
            s.eval::<f64>("return GetMovieResolution()").unwrap(),
            1280.0
        );
        assert!(s
            .eval::<bool>("return table.getn({GetMovieResolution()}) == 1")
            .unwrap());

        assert!(!s.glue_cursor_hidden());
        s.run("HideCursor()").unwrap();
        assert!(s.glue_cursor_hidden());
        s.run("ShowCursor()").unwrap();
        assert!(!s.glue_cursor_hidden());
        assert!(s.take_errors().is_empty());
    }

    /// `0x429ff0`'s copy: `%XY` is the byte it spells, a non-hex digit reads 0, a `%` with fewer
    /// than two bytes after it ends the string, a decoded NUL ends it too; then the web-address
    /// rule on the result.
    #[test]
    fn the_addon_url_is_decoded_as_the_launcher_decodes_it_and_must_be_a_web_address() {
        for (url, want) in [
            ("http://a.example/x", Some("http://a.example/x")),
            ("HTTPS://a.example/%7Euser", Some("HTTPS://a.example/~user")),
            ("http://a.example/a%20b", Some("http://a.example/a%20b")),
            ("http://a.example/%2f%2F", Some("http://a.example///")),
            ("http://a.example/x%", Some("http://a.example/x")),
            ("http://a.example/x%4", Some("http://a.example/x")),
            ("http://a.example/x%00y", Some("http://a.example/x")),
            ("http://a.example/x%zzy", Some("http://a.example/x")),
            (
                "http://a.example/x?a=1&b=2#f",
                Some("http://a.example/x?a=1&b=2#f"),
            ),
            ("%68ttp://a.example/", Some("http://a.example/")),
            // A decoded `%` is encoded again, so the opener decodes it no further.
            ("http://a.example/%2541", Some("http://a.example/%2541")),
            ("http://a.example/50%25", Some("http://a.example/50%25")),
            // The bytes a shell or a raw command tail would read are escaped, wherever they came
            // from: typed, or spelled as an escape.
            (
                "http://a.example/a b\"c'd`e$f;g<h>i|j^k\\l{m}n",
                Some("http://a.example/a%20b%22c%27d%60e%24f%3Bg%3Ch%3Ei%7Cj%5Ek%5Cl%7Bm%7Dn"),
            ),
            (
                "http://a.example/%22%27%60%24%3b%3c%3e%7c%5e%5c%7b%7d%20%0d%0a",
                Some("http://a.example/%22%27%60%24%3B%3C%3E%7C%5E%5C%7B%7D%20%0D%0A"),
            ),
            ("http://a.example/\u{e9}", Some("http://a.example/%C3%A9")),
            ("file:///etc/passwd", None),
            ("ftp://a.example/", None),
            ("javascript:alert(1)", None),
            ("%66ile:///x", None),
            // A host is required, and begins with a letter or digit.
            ("http://", None),
            ("http:///x", None),
            ("http://?x", None),
            ("http://.example/", None),
            ("http://-/", None),
            ("", None),
        ] {
            assert_eq!(launch_addon_url(url).as_deref(), want, "{url:?}");
        }
        let long = format!("http://a.example/{}", "a".repeat(0x400));
        assert_eq!(
            launch_addon_url(&long),
            None,
            "0x42a01c refuses 0x400 bytes"
        );
        // The host's own check takes only what the verb makes.
        assert!(launch_addon_url_allowed("http://a.example/a%20b"));
        for bad in [
            "http://a.example/a b",
            "http://a.example/a\tb",
            "http://a.example/a'b",
            "http://a.example/a$b",
            "http://a.example/%2",
            "http://a.example/%zz",
            "http:// a.example/",
            "http://",
        ] {
            assert!(!launch_addon_url_allowed(bad), "{bad:?}");
        }
    }
}
