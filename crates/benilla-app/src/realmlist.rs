//! The realmlist: the `host[:port]`, or several separated by `;`, benilla dials for the logon
//! handshake.
//!
//! The reference registers the CVar `realmList` (help "Address of realm list server", at
//! `0x5ab6a6`) and loads it from `realmlist.wtf` beside the executable, with no UI. benilla keeps
//! the name, the shape and the help string. Deviation: the value lives in `config.toml` with every
//! other CVar and is edited from a login-screen control, because local state is one folder and one
//! config file.
//!
//! `$WOW_HOST` wins for the session, is never written to the file, and is taken verbatim, not
//! through [`normalize`].

use bevy::prelude::*;

/// Installs [`Realmlist`], which must exist before `cvars::load_config` applies the saved value.
pub(crate) struct RealmlistPlugin;

/// `realmList`'s change callback: a value that is not an address is ignored with a warning.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut realmlist: ResMut<Realmlist>) {
    if !ev.is(CVAR_REALMLIST) {
        return;
    }
    match normalize(&ev.new) {
        Some(address) => realmlist.set(&address),
        None => {
            warn!("cvar realmList: unusable realmlist '{}' ignored", ev.new);
            realmlist.refused = Some(ev.new.clone());
        }
    }
}

impl Plugin for RealmlistPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<Realmlist>();
    }
}

/// The CVar name, the reference's spelling.
pub(crate) const CVAR_REALMLIST: &str = "realmList";

/// Deviation: the local machine's server, not the reference's
/// `us.logon.worldofwarcraft.com:3724`, because that host no longer resolves.
pub(crate) const DEFAULT_REALMLIST: &str = "localhost";

/// The dialog box's `letters` cap. Deviation: 64, where the reference's login boxes cap at 16
/// (`AccountLogin.xml`), because a hostname needs more.
pub(crate) const MAX_LETTERS: usize = 64;

/// The address the next logon attempt dials, as `host[:port]` (the port defaults to
/// [`benilla_protocol::AUTH_PORT`]). It travels on each [`crate::net::LoginRequest`], so an edit
/// never repoints an attempt in flight.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Realmlist {
    address: String,
    /// `$WOW_HOST` owns this session: the address shows, the control is disabled, nothing persists.
    pinned_by_env: bool,
    /// The last `realmList` value that was not an address, for the stock login screen's
    /// realmlist dialog to reopen on ([`crate::ui_script::glue_mgr`]).
    refused: Option<String>,
}

impl Default for Realmlist {
    fn default() -> Self {
        match std::env::var("WOW_HOST") {
            // Verbatim, not normalized.
            Ok(host) if !host.trim().is_empty() => Realmlist {
                address: host,
                pinned_by_env: true,
                refused: None,
            },
            _ => Realmlist {
                address: DEFAULT_REALMLIST.to_string(),
                pinned_by_env: false,
                refused: None,
            },
        }
    }
}

impl Realmlist {
    /// A realmlist with no env pin, for tests, since [`Default`] reads `$WOW_HOST`.
    #[cfg(test)]
    pub(crate) fn unpinned(address: &str) -> Self {
        Realmlist {
            address: address.to_string(),
            pinned_by_env: false,
            refused: None,
        }
    }

    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    pub(crate) fn pinned_by_env(&self) -> bool {
        self.pinned_by_env
    }

    /// The last value refused as no address, taken once.
    pub(crate) fn take_refused(&mut self) -> Option<String> {
        self.refused.take()
    }

    /// Points at an already normalized `address`; ignored while pinned by the env.
    pub(crate) fn set(&mut self, address: &str) {
        if self.pinned_by_env || self.address == address {
            return;
        }
        self.address = address.to_string();
    }
}

/// Normalizes typed or pasted text into a `host[:port]`, checking syntax only; reachability
/// surfaces in the `LOGIN_FAILED` dialog. A pasted `realmlist.wtf` line
/// (`SET realmlist "logon.example.org"`) is unwrapped to its value.
pub(crate) fn normalize(input: &str) -> Option<String> {
    let mut s = input.trim();

    // `SET realmlist <value>` or `set realmlist = <value>`.
    if let Some(rest) = strip_prefix_ci(s, "set") {
        if rest.starts_with(|c: char| c.is_whitespace()) {
            if let Some(value) = strip_prefix_ci(rest.trim_start(), "realmlist") {
                s = value.trim_start().strip_prefix('=').unwrap_or(value).trim();
            }
        }
    }
    // One matched pair of quotes only; `trim_matches` would eat a run of them.
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = &s[1..s.len() - 1];
    }
    let s = s.trim();

    if s.is_empty() || s.chars().count() > MAX_LETTERS {
        return None;
    }
    // Whitespace or control characters mean the paste brought a second word along.
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    // The logon resolves each `;`-separated host up to the first empty one (`0x43b890`), so the
    // first must be there. Each mirrors `host_port`'s split: a single colon must carry a valid
    // port; two or more colons are an IPv6 literal, left intact. Deviation: a host name may carry
    // a port, where the reference resolves a name whole (`0x43ba0e`) and takes a port only on a
    // dotted address (`0x43f3d0`), because a server on a non-default logon port behind a host
    // name is everyday use.
    if s.starts_with(';') {
        return None;
    }
    for piece in s.split(';').take_while(|piece| !piece.is_empty()) {
        if let Some((host, port)) = piece.rsplit_once(':') {
            if !host.contains(':') && (host.is_empty() || port.parse::<u16>().is_err()) {
                return None;
            }
        }
    }
    Some(s.to_string())
}

/// `s` without `prefix`, case-insensitively; `str::get` refuses a non-boundary split.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_host_passes_through() {
        assert_eq!(normalize("localhost").as_deref(), Some("localhost"));
        assert_eq!(
            normalize("  logon.example.org  ").as_deref(),
            Some("logon.example.org"),
        );
        assert_eq!(
            normalize("127.0.0.1:3725").as_deref(),
            Some("127.0.0.1:3725")
        );
    }

    #[test]
    fn a_pasted_wtf_line_is_unwrapped() {
        for line in [
            r#"SET realmlist "logon.example.org""#,
            r#"set realmlist "logon.example.org""#,
            "SET realmlist logon.example.org",
            r#"set realmlist = "logon.example.org""#,
            r#"   SET   realmlist   "logon.example.org"   "#,
        ] {
            assert_eq!(
                normalize(line).as_deref(),
                Some("logon.example.org"),
                "unwrapping {line:?}",
            );
        }
    }

    #[test]
    fn a_host_named_like_the_prefix_is_left_alone() {
        assert_eq!(
            normalize("settings.example.org").as_deref(),
            Some("settings.example.org")
        );
        assert_eq!(normalize("set").as_deref(), Some("set"));
    }

    #[test]
    fn nothing_usable_is_rejected() {
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("   "), None);
        assert_eq!(normalize(r#"SET realmlist """#), None);
        // A paste that brought a second word along.
        assert_eq!(normalize("logon.example.org and more"), None);
        // Longer than the box can hold.
        assert_eq!(normalize(&"a".repeat(MAX_LETTERS + 1)), None);
    }

    #[test]
    fn a_single_colon_must_carry_a_real_port() {
        assert_eq!(normalize("logon.example.org:notaport"), None);
        assert_eq!(normalize("logon.example.org:"), None);
        assert_eq!(normalize("logon.example.org:99999"), None); // past u16
        assert_eq!(normalize(":3724"), None); // no host
        assert_eq!(
            normalize("logon.example.org:3724").as_deref(),
            Some("logon.example.org:3724"),
        );
    }

    /// The logon resolves each `;`-separated host in turn (`0x43b890`), so each one's port is
    /// checked on its own.
    #[test]
    fn each_host_of_a_semicolon_list_is_checked_on_its_own() {
        assert_eq!(
            normalize("10.0.0.1:3725;logon.example.org").as_deref(),
            Some("10.0.0.1:3725;logon.example.org"),
        );
        assert_eq!(normalize("a.example.org;b.example.org:notaport"), None);
        assert_eq!(normalize("a.example.org:notaport;b.example.org"), None);
        assert_eq!(normalize(";a.example.org"), None, "no first host");
    }

    #[test]
    fn an_ipv6_literal_is_left_intact() {
        assert_eq!(normalize("::1").as_deref(), Some("::1"));
        assert_eq!(normalize("fe80::1").as_deref(), Some("fe80::1"));
        // And what normalize passes is what the protocol splits.
        assert_eq!(
            benilla_protocol::host_port("::1", benilla_protocol::AUTH_PORT),
            ("::1", benilla_protocol::AUTH_PORT),
        );
    }

    #[test]
    fn the_default_is_a_host_the_protocol_can_split() {
        let (host, port) =
            benilla_protocol::host_port(DEFAULT_REALMLIST, benilla_protocol::AUTH_PORT);
        assert_eq!(host, "localhost");
        assert_eq!(port, benilla_protocol::AUTH_PORT);
    }

    #[test]
    fn an_env_pinned_realmlist_ignores_writes() {
        let mut r = Realmlist {
            address: "harness.example.org".into(),
            pinned_by_env: true,
            refused: None,
        };
        r.set("somewhere.else.org");
        assert_eq!(r.address(), "harness.example.org");
        assert!(r.pinned_by_env());
    }

    #[test]
    fn an_unpinned_realmlist_takes_writes() {
        let mut r = Realmlist {
            address: DEFAULT_REALMLIST.into(),
            pinned_by_env: false,
            refused: None,
        };
        r.set("logon.example.org");
        assert_eq!(r.address(), "logon.example.org");
    }
}
