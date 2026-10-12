//! The glue core's login-screen verbs (registrar `0x46ddf0`, table `0x8373b8`): the login, the
//! status dialog's button, the saved account, the four user agreements, the build, the locale,
//! the server, the screen name, the glue music and the quit. Each reads a slot the host seeds or
//! queues a [`GlueRequest`] the host's glue manager drains; the screen state machine they drive
//! (`CGlueMgr::Update`, `0x46b930`) is the host's.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, string_arg};
use super::Model;

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
    /// The flags for a CVar value, read as `atoi` reads it: anything unparseable is 0.
    pub fn from_cvar(value: &str) -> Self {
        let n = leading_int(value);
        NoticeState {
            accepted: n == 1,
            show_notice: n < 0,
        }
    }
}

/// `atoi`: an optional sign and the leading digits, 0 when there are none.
fn leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'-') => (-1, &s[1..]),
        Some(b'+') => (1, &s[1..]),
        _ => (1, s),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    sign * digits[..end].parse::<i64>().unwrap_or(0)
}

/// `DefaultServerLogin`'s two strings. Never logged: the `Debug` form names the account only.
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

/// A glue verb's effect on the host, drained by its glue manager.
#[derive(Debug)]
pub enum GlueRequest {
    /// `DefaultServerLogin(account, password)` (`0x46d160`), handed to `CGlueMgr::Login`
    /// (`0x46afb0`), whose guards and dialogs are the host's.
    Login(GlueCredentials),
    /// `StatusDialogClick()` (`0x46d270` → `0x46b720`): the status dialog's button, per screen
    /// state.
    StatusDialogClick,
    /// `SetSavedAccountName(name)` (`0x46cdf0`): the `accountName` CVar's new value.
    SetSavedAccountName(String),
    /// `AcceptTOS` and its three siblings (`0x46d000`, `0x46d070`, `0x46d0e0`, `0x46d150`): the
    /// agreement's CVar set to `"1"` (`0x46aef0` and its siblings).
    Accept(GlueNotice),
    /// `PlayGlueMusic(file)` (`0x46ceb0` → `0x45aeb0`).
    PlayGlueMusic(String),
    /// `StopGlueMusic()` (`0x46cf00`): `0x45b050(1.0)`, a one-second fade.
    StopGlueMusic,
    /// `DisconnectFromServer()` (`0x46d340`): the world connection dropped if up, then the logon
    /// cancelled.
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
}

/// The realm `GetServerName` (`0x46d280`) answers: the `realmName` CVar's text, and the
/// selected realm's PvP and RP flags off its `Cfg_Configs` row, nil without a selected realm.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlueServer {
    pub name: String,
    pub pvp: bool,
    pub rp: bool,
}

/// The glue core's slots and queue in the VM's model.
#[derive(Default)]
pub(crate) struct GlueCore {
    requests: Vec<GlueRequest>,
    /// The `accountName` CVar (`[0x8826c8]`), which `GetSavedAccountName` (`0x46cdd0`) reads.
    saved_account: String,
    notices: [NoticeState; 4],
    /// ClientServices' world-connection flag (`[svc+0x1b00]`), which `IsConnectedToServer`
    /// (`0x46d380`) reads.
    connected: bool,
    server: GlueServer,
    /// The current glue screen's name (`[0xb41478]`), which `SetCurrentScreen` (`0x46ce60` →
    /// `0x46b860`) copies into a 0x40-byte buffer.
    pub(super) screen: String,
    /// The realm list the realm verbs read ([`super::glue_realm`]).
    pub(super) realms: super::glue_realm::GlueRealmList,
}

/// `SetCurrentScreen`'s buffer, 0x40 bytes with its terminator (`0x46b860`, `SStrCopy`).
const SCREEN_NAME_MAX: usize = 0x3f;

impl super::UiScript {
    /// Drain the glue verbs' requests since the last call.
    pub fn take_glue_requests(&mut self) -> Vec<GlueRequest> {
        std::mem::take(&mut self.model_mut().glue.requests)
    }

    /// Seed the saved account name `GetSavedAccountName` answers.
    pub fn set_glue_saved_account(&mut self, name: &str) {
        self.model_mut().glue.saved_account = name.to_string();
    }

    /// Seed one agreement's flags.
    pub fn set_glue_notice(&mut self, notice: GlueNotice, state: NoticeState) {
        self.model_mut().glue.notices[notice.index()] = state;
    }

    /// One agreement's flags as the VM holds them.
    pub fn glue_notice(&self, notice: GlueNotice) -> NoticeState {
        self.model_ref().glue.notices[notice.index()]
    }

    /// Seed whether the world connection is up.
    pub fn set_glue_connected(&mut self, connected: bool) {
        self.model_mut().glue.connected = connected;
    }

    /// Seed the realm `GetServerName` answers.
    pub fn set_glue_server(&mut self, server: GlueServer) {
        self.model_mut().glue.server = server;
    }

    /// The current glue screen's name, as `SetCurrentScreen` last set it.
    pub fn glue_screen(&self) -> String {
        self.model_ref().glue.screen.clone()
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

/// The text of a string global, as [`super::UiScript::global_text`] reads it.
fn global_text(lua: &Lua, key: &str) -> String {
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
    let g = lua.globals();

    // Both arguments `is-number-or-string` (`0x46d168`, `0x46d178`), else the usage error
    // (`0x837cbc`); then `CGlueMgr::Login(account, password)` (`0x46d1a0`).
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
            queue(
                lua,
                GlueRequest::Login(GlueCredentials { account, password }),
            );
            Ok(())
        })?,
    )?;

    g.set(
        "StatusDialogClick",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StatusDialogClick);
            Ok(())
        })?,
    )?;

    // The `accountName` CVar's string (`0x46cdd5`, `[cvar+0x20]`).
    g.set(
        "GetSavedAccountName",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            lua.create_string(&model.glue.saved_account)
        })?,
    )?;
    // A string argument (`0x46cdf8`) or `Usage: SetSavedAccountName("accountName")` (`0x837c28`),
    // then `CVar::Set` (`0x46ce2e`), so the next `GetSavedAccountName` reads it back at once.
    g.set(
        "SetSavedAccountName",
        lua.create_function(|lua, name: Value| {
            let name = string_arg(lua, name, "Usage: SetSavedAccountName(\"accountName\")")?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .glue
                .saved_account = name.clone();
            queue(lua, GlueRequest::SetSavedAccountName(name));
            Ok(())
        })?,
    )?;

    // Each agreement: `Show*Notice` and `*Accepted` push 1 or nil off the flag pair the glue
    // manager seeded (`[0xb41df8]`/`[0xb41dfc]` for the TOS, `[0xb41e00]`/`[0xb41e04]` the EULA,
    // `[0xb41e08]`/`[0xb41e0c]` scanning, `[0xb41e10]`/`[0xb41e14]` the contest); `Accept*` sets
    // accepted, clears the notice and sets the CVar to "1" (`0x46aef0`, `0x46af20`, `0x46af50`,
    // `0x46af80`).
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
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .glue
                    .notices[i] = NoticeState {
                    accepted: true,
                    show_notice: false,
                };
                queue(lua, GlueRequest::Accept(notice));
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
    // answer as the in-game binding: benilla is enUS-only.
    g.set(
        "GetLocale",
        lua.create_function(|lua, _: MultiValue| lua.create_string("enUS"))?,
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

    // Three values (`0x46d280`): the `realmName` CVar's string, always a string, then PvP and RP
    // as 1 or nil.
    g.set(
        "GetServerName",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let server = &model.glue.server;
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&server.name)?),
                flag(server.pvp),
                flag(server.rp),
            ]))
        })?,
    )?;

    g.set(
        "IsConnectedToServer",
        lua.create_function(|lua, _: MultiValue| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.glue.connected))
        })?,
    )?;
    g.set(
        "DisconnectFromServer",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::DisconnectFromServer);
            Ok(())
        })?,
    )?;
    g.set(
        "QuitGame",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::QuitGame);
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
    g.set(
        "StopGlueMusic",
        lua.create_function(|lua, _: MultiValue| {
            queue(lua, GlueRequest::StopGlueMusic);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

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

    #[test]
    fn default_server_login_queues_the_strings_or_raises_its_usage() {
        let mut s = UiScript::new_glue().unwrap();
        s.run("DefaultServerLogin('probe', 1234)").unwrap();
        let reqs = s.take_glue_requests();
        match reqs.as_slice() {
            [GlueRequest::Login(c)] => {
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
        assert!(s.take_glue_requests().is_empty());
    }

    #[test]
    fn the_agreements_answer_their_seeded_flags_and_accept_sets_them() {
        let mut s = UiScript::new_glue().unwrap();
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
        assert_eq!(read(&s), (Some(1), None, Some(1), None));
        assert!(matches!(
            s.take_glue_requests().as_slice(),
            [GlueRequest::Accept(GlueNotice::Eula)]
        ));
    }

    #[test]
    fn the_saved_account_reads_back_what_was_set() {
        let mut s = UiScript::new_glue().unwrap();
        s.set_glue_saved_account("PROBE");
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            "PROBE"
        );
        s.run("SetSavedAccountName('')").unwrap();
        assert_eq!(
            s.eval::<String>("return GetSavedAccountName()").unwrap(),
            ""
        );
        assert!(matches!(
            s.take_glue_requests().as_slice(),
            [GlueRequest::SetSavedAccountName(n)] if n.is_empty()
        ));
        let err = s.run("SetSavedAccountName()").unwrap_err().to_string();
        assert!(
            err.contains("Usage: SetSavedAccountName(\"accountName\")"),
            "{err}"
        );
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

    #[test]
    fn the_server_name_is_always_a_string_with_two_flags() {
        let mut s = UiScript::new_glue().unwrap();
        assert_eq!(
            s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap(),
            (String::new(), None, None)
        );
        s.set_glue_server(GlueServer {
            name: "Nostalrius".into(),
            pvp: true,
            rp: false,
        });
        assert_eq!(
            s.eval::<(String, Option<i64>, Option<i64>)>("return GetServerName()")
                .unwrap(),
            ("Nostalrius".to_string(), Some(1), None)
        );
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
}
