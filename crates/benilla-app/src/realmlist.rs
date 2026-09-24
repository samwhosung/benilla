//! The **realmlist** — the address benilla dials for the logon (realmd) handshake, and the one
//! setting a player cannot start the game without (decision 1667).
//!
//! The reference client has no UI for this at all: it registers a CVar named `realmList` — help
//! text *"Address of realm list server"*, default `us.logon.worldofwarcraft.com:3724` — and loads
//! it from a plain-text `realmlist.wtf` beside the executable, which every private server's setup
//! page tells you to open in Notepad. (All four strings are byte-verified in `WoW.exe`, adjacent
//! in the string table at the CVar's registration site, `0x5ab6a6`.) benilla keeps the
//! **name, the `host[:port]` shape and the help string**, and replaces the text editor with a
//! control on the login screen.
//!
//! **It is a CVar, in `config.toml` with every other setting — not a `realmlist.wtf` of our own.**
//! The reference splits the file for reasons that are entirely its own (the installer and the
//! patcher write the realmlist without touching a player's `Config.wtf`), and benilla has neither.
//! Decision 0954's law is one folder and one config file; a second file would buy nothing and
//! would fork the atomic-write, unknown-key-preservation and debounce machinery that already
//! exists. `GetCVar("realmList")` answers, which is also what the reference's own console does.
//!
//! **`$WOW_HOST` still wins for the session** and never reaches the file — the same law
//! `WOW_UI_SCALE`/`WOW_FARCLIP` run under (`crate::cvars`' module doc). Every probe, smoke run and
//! harness leg sets it, so the env path is the one that must not change behaviour: its value is
//! taken **verbatim**, not through [`normalize`], so nothing that connects today can start
//! failing a syntax check.

use bevy::prelude::*;

/// Installs [`Realmlist`]. Its own plugin (rather than a line in `CvarPlugin`) because every other
/// CVar knob resource is owned by the module it belongs to, and because the resource has to exist
/// before `cvars::load_config` applies the saved value at `Startup` — `lib.rs` orders it there.
pub(crate) struct RealmlistPlugin;

/// `realmList`'s change callback (1667, 2303): a string row — a value that is not an address
/// is consumed with a warn, and the resource keeps its truth.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut realmlist: ResMut<Realmlist>) {
    if !ev.is(CVAR_REALMLIST) {
        return;
    }
    match normalize(&ev.new) {
        Some(address) => realmlist.set(&address),
        None => warn!("cvar realmList: unusable realmlist '{}' ignored", ev.new),
    }
}

impl Plugin for RealmlistPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<Realmlist>();
    }
}

/// The CVar name — the reference's own spelling, `realmList`.
pub(crate) const CVAR_REALMLIST: &str = "realmList";

/// benilla's registered default.
///
/// **Not the reference's** `us.logon.worldofwarcraft.com:3724`, which is a knowing divergence and
/// a small one: that host has not resolved since 2019, so shipping it would mean every first
/// launch begins with a DNS failure. benilla is a client for servers you run or choose, and the
/// one it can assume is the one on the machine it is running on — which is also what every
/// existing `WOW_HOST`-less dev and capture run already dials.
pub(crate) const DEFAULT_REALMLIST: &str = "localhost";

/// The dialog box's `letters` cap. The reference's login boxes are 16 (`AccountLogin.xml`); a
/// hostname needs far more, and 64 covers any real DNS name (253 is the protocol limit, but
/// nothing a player types by hand approaches it) while still bounding the field.
pub(crate) const MAX_LETTERS: usize = 64;

/// The address the next logon attempt dials, as `host[:port]` — [`benilla_protocol::host_port`]
/// supplies [`benilla_protocol::AUTH_PORT`] when the port is left off.
///
/// The session's live value, so a change takes effect on the **next attempt** with no relaunch.
/// It is deliberately not read by the IO thread: the host travels on each
/// [`crate::net::LoginRequest`], exactly as the credentials have since decision 0539 — an attempt
/// carries everything about itself, and a mid-flight edit cannot repoint the attempt already on
/// the wire.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub(crate) struct Realmlist {
    address: String,
    /// `$WOW_HOST` owns this session. The screen still shows the address (a harness run that
    /// dialed the wrong server should say so on its face), but the control is disabled and
    /// nothing is written to `config.toml`.
    pinned_by_env: bool,
}

impl Default for Realmlist {
    fn default() -> Self {
        match std::env::var("WOW_HOST") {
            // Verbatim, not normalized — see the module doc.
            Ok(host) if !host.trim().is_empty() => Realmlist {
                address: host,
                pinned_by_env: true,
            },
            _ => Realmlist {
                address: DEFAULT_REALMLIST.to_string(),
                pinned_by_env: false,
            },
        }
    }
}

impl Realmlist {
    /// A realmlist at `address` with **no env pin**, whatever the ambient `$WOW_HOST` says.
    ///
    /// For tests only, and it exists because [`Default`] reads the environment: a suite run from a
    /// shell that exports `WOW_HOST` (which every probe recipe in this repo does) would otherwise
    /// assert against that shell's value and refuse every write.
    #[cfg(test)]
    pub(crate) fn unpinned(address: &str) -> Self {
        Realmlist {
            address: address.to_string(),
            pinned_by_env: false,
        }
    }

    /// What the next attempt dials.
    pub(crate) fn address(&self) -> &str {
        &self.address
    }

    /// Whether `$WOW_HOST` pinned this session (the control is disabled, nothing persists).
    pub(crate) fn pinned_by_env(&self) -> bool {
        self.pinned_by_env
    }

    /// Point at `address` — already through [`normalize`]. Ignored while pinned by the env, so a
    /// stray `SetCVar` cannot repoint a harness leg mid-run.
    pub(crate) fn set(&mut self, address: &str) {
        if self.pinned_by_env || self.address == address {
            return;
        }
        self.address = address.to_string();
    }
}

/// Normalize what a player typed or pasted into a `host[:port]`, or `None` if there is nothing
/// usable in it.
///
/// **Syntax only, never reachability** — the deliberate line every comparable client draws
/// (Veloren, ClassiCube, Terraria, Minecraft): a name that does not resolve, a closed port and a
/// firewalled box are all indistinguishable without dialing, and a pre-connect probe would report
/// a false negative on exactly the LAN/VPN-hosted vmangos box this client exists for. Those
/// failures surface where they always have — the authored `LOGIN_FAILED` dialog, with the address
/// on screen behind it.
///
/// It accepts a **pasted `realmlist.wtf` line** verbatim (`SET realmlist "logon.example.org"`),
/// because that is the literal string every private server's setup page tells a player to copy,
/// and pasting it is what they will try first.
pub(crate) fn normalize(input: &str) -> Option<String> {
    let mut s = input.trim();

    // `SET realmlist <value>` / `set realmlist = <value>` — the .wtf line, unwrapped to its value.
    if let Some(rest) = strip_prefix_ci(s, "set") {
        if rest.starts_with(|c: char| c.is_whitespace()) {
            if let Some(value) = strip_prefix_ci(rest.trim_start(), "realmlist") {
                s = value.trim_start().strip_prefix('=').unwrap_or(value).trim();
            }
        }
    }
    // One matched pair of quotes (the .wtf line quotes its value); `trim_matches` would eat a
    // run of them, which is a different string than the one the player pasted.
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = &s[1..s.len() - 1];
    }
    let s = s.trim();

    if s.is_empty() || s.chars().count() > MAX_LETTERS {
        return None;
    }
    // No whitespace and no control characters: an address with a space in it is a paste that
    // brought a second word along, not a host.
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    // Mirror `host_port`'s own split exactly (`benilla-protocol`): a **single** colon means the
    // suffix was meant as a port, so a non-numeric one is a typo worth naming here rather than a
    // DNS failure thirty seconds later. Two or more colons is an IPv6 literal, which `host_port`
    // deliberately leaves intact — so this leaves it intact too rather than inventing a second,
    // stricter law for the same string.
    if let Some((host, port)) = s.rsplit_once(':') {
        if !host.contains(':') && (host.is_empty() || port.parse::<u16>().is_err()) {
            return None;
        }
    }
    Some(s.to_string())
}

/// `s` without `prefix`, matched case-insensitively. `str::get` returns `None` on a non-boundary
/// index, so a multi-byte character straddling the split can never be sliced through.
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

    /// The line a private server's setup page tells you to paste into `realmlist.wtf` — in every
    /// spelling those pages actually use.
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

    /// A host that merely *starts* with the letters of the prefix is not a .wtf line.
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

    /// The port half of `host_port`'s law, enforced forward: a single colon means a port.
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

    /// An IPv6 literal keeps `host_port`'s own answer: more than one colon is a raw address and
    /// comes back intact, with the default port.
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

    /// Every value this module can hand out survives `host_port` — the one consumer downstream.
    #[test]
    fn the_default_is_a_host_the_protocol_can_split() {
        let (host, port) =
            benilla_protocol::host_port(DEFAULT_REALMLIST, benilla_protocol::AUTH_PORT);
        assert_eq!(host, "localhost");
        assert_eq!(port, benilla_protocol::AUTH_PORT);
    }

    /// A pinned session refuses to be repointed — the harness guard.
    #[test]
    fn an_env_pinned_realmlist_ignores_writes() {
        let mut r = Realmlist {
            address: "harness.example.org".into(),
            pinned_by_env: true,
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
        };
        r.set("logon.example.org");
        assert_eq!(r.address(), "logon.example.org");
    }
}
