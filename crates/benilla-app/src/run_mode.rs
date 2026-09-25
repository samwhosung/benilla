//! What kind of run this is and whose: the always-present layer on the player's side of the `dev`
//! seam. Every answer defaults to the player's (nobody drives the camera, the client starts at the
//! login screen, no rig owns the pick); a dev build's instruments only move answers off those
//! defaults. Gameplay reads this module, never `capture`, `debug_panel` or `perf`.
//!
//! [`benilla_world::dev_state`]'s `deterministic_run` reads `$WOW_CAPTURE` on its own, as
//! [`scenario_active`] does here; the two must stay in step.

use bevy::prelude::*;

/// Something other than the player authors the camera and the avatar this run. Only the capture
/// harness inserts it; the type lives here because non-dev run conditions name it.
#[derive(Resource)]
pub(crate) struct CaptureMode;

/// The rig's derived character name when `$WOW_RIG` names a body, inserted by
/// `capture::ProbeRigPlugin`. While present the roster ignores `$WOW_CHAR`: the rig may have to
/// create its body first, and the `WOW_CHAR` one-shot cannot wait for that.
#[derive(Resource)]
pub(crate) struct RigCharacter(pub(crate) String);

/// Is a capture running (`$WOW_CAPTURE` set)? Read before any plugin builds: it decides whether
/// the net thread starts, the window size, and whether clutter and anim-LOD randomness is pinned.
pub(crate) fn scenario_active() -> bool {
    std::env::var("WOW_CAPTURE").is_ok()
}

/// Is the player UI opted into this capture? Forwards [`crate::capture::ui_opted_in`] for readers
/// outside the harness; `false` in a player build, which has no capture to opt into.
#[cfg(feature = "dev")]
pub(crate) fn capture_ui_opted_in() -> bool {
    crate::capture::ui_opted_in()
}

#[cfg(not(feature = "dev"))]
pub(crate) fn capture_ui_opted_in() -> bool {
    false
}

/// Do the credentials come from the environment (`$WOW_USER` and `$WOW_PASS`, both)? That means
/// "log in without typing" and nothing about who is present; [`unattended`] answers that. One
/// credential alone is a typed login, and `$WOW_CHAR` alone is only the roster's fast path.
pub(crate) fn env_login() -> bool {
    ["WOW_USER", "WOW_PASS"]
        .iter()
        .all(|k| std::env::var_os(k).is_some())
}

/// Is nobody at the keyboard? `false` unless the run declares itself with `$WOW_UNATTENDED`, or is
/// a capture (`$WOW_CAPTURE`) or a rig (`$WOW_RIG`), which cannot be a person. Env login
/// credentials alone never make a run unattended: a player may launch with them. The doubt goes
/// one way because a wrong "attended" costs a probe a timeout, while a wrong "unattended" makes a
/// player's client fight another for their account. A scripted run declares itself, as
/// `docs/CONTRIBUTING.md`, "Running it unattended" says.
///
/// Read by what acts instead of a person: [`crate::net::DisconnectedMessage::new`] (the
/// lost-session verdict, taken once at the wire edge) and the `FATAL` exits.
pub(crate) fn unattended() -> bool {
    ["WOW_UNATTENDED", "WOW_CAPTURE", "WOW_RIG"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

/// A run stuck on a dialog: `true` (exit) only when the run is [`unattended`], logging the `FATAL`
/// marker a leg runner greps for; otherwise the dialog stays up, with a hint when the credentials
/// came from the environment. The call sites in [`crate::login`] and [`crate::char_select`] share
/// it so the verdict and the marker cannot drift apart.
pub(crate) fn fatal_when_driverless(why: &str) -> bool {
    if unattended() {
        error!("login: FATAL — {why}; exiting");
        return true;
    }
    if env_login() {
        warn!(
            "login: {why} — leaving the dialog up, because nothing declared this run driverless. \
             Set WOW_UNATTENDED=1 if nobody is here and it should exit non-zero instead \
             (decision 1769)."
        );
    }
    false
}

/// The screen the client starts on: a player at the login screen, a capture straight in the world,
/// a glue capture on the screen it photographs. Needed before the plugins build, so it forwards
/// into the dev half rather than reading a resource.
pub(crate) fn start_state() -> crate::char_select::ClientState {
    #[cfg(feature = "dev")]
    {
        crate::capture::start_state()
    }
    #[cfg(not(feature = "dev"))]
    {
        crate::char_select::ClientState::Login
    }
}

/// Does this build offer developer affordances at all? `false` in a player build. A dev affordance
/// may sit in a gameplay module (the world map's Alt-click jump, `/castvis`), so it asks here.
/// Held by [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_affordances() -> bool {
    cfg!(feature = "dev")
}

/// Did a dev chord (`Ctrl`+`Shift`+key) fire? `false` in a player build. The engine's
/// [`benilla_world::modkeys::dev_chord`] is always compiled, so a gameplay module calling it
/// directly would ship a live dev key to players with no build failure; every non-dev reader comes
/// through here instead. Held by [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_chord(keys: &ButtonInput<KeyCode>, key: KeyCode) -> bool {
    dev_affordances() && benilla_world::modkeys::dev_chord(keys, key)
}

/// The take-control line's free-fly hint, spelled from [`benilla_world::modkeys::DEV_CHORD`];
/// empty in a player build, which must not offer a key that does nothing.
pub(crate) fn free_fly_hint() -> String {
    #[cfg(feature = "dev")]
    {
        format!(
            " ({} + F toggles free-fly)",
            benilla_world::modkeys::DEV_CHORD
        )
    }
    #[cfg(not(feature = "dev"))]
    {
        String::new()
    }
}

/// This crate's source directory on a dev build, `None` in a player build. The `cfg` must be real,
/// not a runtime check: `env!("CARGO_MANIFEST_DIR")` names the build machine, and the literal must
/// be absent from a player binary. Read by [`crate::ui_script`] (live `assets/ui`) and
/// [`crate::local_state`] (the project-folder home); held by
/// [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_source_dir() -> Option<&'static std::path::Path> {
    #[cfg(feature = "dev")]
    {
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
    }
    #[cfg(not(feature = "dev"))]
    {
        None
    }
}

/// The pre-connect account guard on the env fast path ([`crate::login`]): `Err(explanation)` when
/// this checkout declares an account ([`declared_identity`]) and the login is for another, because
/// a login kicks whoever holds the account. Inert without a declaration. `WOW_ALLOW_ACCOUNT=1`
/// turns the refusal into a warning.
pub(crate) fn account_guard(user: &str) -> Result<(), String> {
    guard_for(declared_identity().as_ref(), user)
}

/// What `.probe-identity` at the project root declares: the account a scripted run from this
/// checkout logs in as (`WOW_USER=`, `WOW_PASS=`, `WOW_CHAR=`, one per line, gitignored). Also read
/// by `scripts/probe-identity.sh`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeclaredIdentity {
    pub(crate) user: String,
    pub(crate) character: String,
}

/// The declaration, read off the project folder; a player build has none.
#[cfg(feature = "dev")]
pub(crate) fn declared_identity() -> Option<DeclaredIdentity> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)?;
    parse_identity(&std::fs::read_to_string(root.join(".probe-identity")).ok()?)
}

#[cfg(not(feature = "dev"))]
pub(crate) fn declared_identity() -> Option<DeclaredIdentity> {
    None
}

/// The file as an identity, `None` unless both the account and the character are there; the
/// password is never read.
#[cfg_attr(not(feature = "dev"), allow(dead_code))]
fn parse_identity(text: &str) -> Option<DeclaredIdentity> {
    let field = |key: &str| {
        text.lines()
            .filter_map(|l| l.trim().strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .find(|v| !v.is_empty())
    };
    Some(DeclaredIdentity {
        user: field("WOW_USER")?,
        character: field("WOW_CHAR")?,
    })
}

/// [`account_guard`]'s decision, with the declaration passed in.
fn guard_for(declared: Option<&DeclaredIdentity>, user: &str) -> Result<(), String> {
    let Some(id) = declared else {
        return Ok(());
    };
    if user.eq_ignore_ascii_case(&id.user) {
        return Ok(());
    }
    Err(format!(
        "the env fast path is about to log in as `{user}`, and this checkout's .probe-identity \
         declares `{}` (WOW_CHAR={}). A login kicks whoever holds `{user}` — a player mid-session, \
         or another checkout's probe.",
        id.user, id.character
    ))
}

/// The word a rig appends to this checkout's bodies (`<Race3><Class3><word>[f]`) so checkouts
/// never collide on a name: the declared character, lowercased, without a leading `probe`. `None`
/// without a declaration; the rig then configures the body it logged in as.
pub(crate) fn rig_suffix() -> Option<String> {
    Some(suffix_of(&declared_identity()?.character))
}

fn suffix_of(character: &str) -> String {
    let name = character.to_ascii_lowercase();
    let word = name
        .strip_prefix("probe")
        .filter(|w| !w.is_empty())
        .unwrap_or(&name);
    word.chars()
        .filter(char::is_ascii_alphabetic)
        .take(6)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outside the dev roots, only [`super::dev_chord`] may call the engine's `dev_chord`, and no
    /// file may name the `dev` feature: the player build gate cannot see either. The display
    /// string [`benilla_world::modkeys::DEV_CHORD`] is exempt.
    #[test]
    fn the_dev_plane_has_exactly_one_door() {
        // Compiled out by `--no-default-features`, plus this module, the door itself.
        const DEV_ROOTS: &[&str] = &[
            "capture/",
            "debug_panel/",
            "asset_churn.rs",
            "dev.rs",
            "hover_log.rs",
            "perf/",
            "preflight.rs",
            "probe_shield.rs",
            "lib.rs",
            "run_mode.rs",
        ];
        // Assembled at runtime so the checker does not flag its own source.
        let needle = format!("modkeys::{}(", "dev_chord");
        // `#[cfg(feature = "dev")]` belongs only in this module, `dev.rs` and `lib.rs`.
        let cfg_needle = format!("feature = {}dev{}", '"', '"');

        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src is readable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&src)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if DEV_ROOTS.iter().any(|r| rel.starts_with(r)) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("source is readable");
                for (n, line) in text.lines().enumerate() {
                    // Doc comments name the function constantly; only real calls matter.
                    let t = line.trim();
                    if t.starts_with("//") {
                        continue;
                    }
                    if t.contains(&needle) || t.contains(&cfg_needle) {
                        offenders.push(format!("{rel}:{}  {t}", n + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a dev-chord affordance outside the dev roots must go through `run_mode::dev_chord`, \
             which is `false` in a player build — the engine's `modkeys::dev_chord` is always \
             compiled, so calling it directly ships a live dev key to a player and the build gate \
             cannot see it (decision 1176). And a `#[cfg(feature = \"dev\")]` outside `run_mode`, \
             `dev.rs` and `lib.rs` means a gameplay module has learned the seam exists — ask \
             `run_mode::dev_affordances()` instead (decision 1179). Offenders:\n  {}",
            offenders.join("\n  "),
        );
    }

    /// Env credentials log in without typing but do not make the run unattended.
    #[test]
    fn env_credentials_are_not_a_claim_about_who_is_in_the_room() {
        use crate::local_state::test_env::{EnvGuard, ENV_LOCK};
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _decl = EnvGuard::unset("WOW_UNATTENDED");
        let _cap = EnvGuard::unset("WOW_CAPTURE");
        let _rig = EnvGuard::unset("WOW_RIG");

        // A player build's defaults: nothing set, nothing assumed.
        {
            let _u = EnvGuard::unset("WOW_USER");
            let _p = EnvGuard::unset("WOW_PASS");
            let _c = EnvGuard::unset("WOW_CHAR");
            assert!(!env_login() && !unattended());
        }
        // `WOW_USER=… WOW_PASS=… cargo play`, `.cargo/config.toml`'s example: a player's launch.
        let _u = EnvGuard::set("WOW_USER", "player");
        let _p = EnvGuard::set("WOW_PASS", "secret");
        assert!(env_login(), "the fast path still submits for them");
        assert!(
            !unattended(),
            "credentials in the environment are not an empty chair",
        );
        // The declaration, and the two runs that are automated by construction.
        for key in ["WOW_UNATTENDED", "WOW_CAPTURE", "WOW_RIG"] {
            let _d = EnvGuard::set(key, "1");
            assert!(unattended(), "{key} declares the run driverless");
        }
        assert!(!unattended(), "and each guard puts it back");
    }

    #[test]
    fn the_fast_path_is_the_declared_account_or_refused() {
        let id = parse_identity("WOW_USER=probe4\nWOW_PASS=secret\nWOW_CHAR=Probefour\n").unwrap();
        assert!(guard_for(Some(&id), "probe4").is_ok());
        assert!(guard_for(Some(&id), "PROBE4").is_ok()); // vmangos accounts are case-insensitive

        // Anyone else's account kicks a live session.
        let player = guard_for(Some(&id), "player").unwrap_err();
        assert!(player.contains("probe4") && player.contains("kicks"));
        // The override hint belongs to the caller that can act on it, not to the reason.
        assert!(!player.contains("WOW_ALLOW_ACCOUNT"));
        assert!(guard_for(Some(&id), "probe7").is_err());
        assert!(guard_for(None, "player").is_ok());
    }

    #[test]
    fn the_declaration_needs_the_account_and_the_character() {
        assert_eq!(
            parse_identity("WOW_USER=probe4\nWOW_PASS=x\nWOW_CHAR=\"Probefour\"\n"),
            Some(DeclaredIdentity {
                user: "probe4".into(),
                character: "Probefour".into()
            })
        );
        assert!(parse_identity("WOW_USER=probe4\nWOW_PASS=x\n").is_none());
        assert!(parse_identity("WOW_USER=\nWOW_CHAR=Probefour\n").is_none());
        assert!(parse_identity("").is_none());
    }

    #[test]
    fn the_rig_word_is_the_declared_character_without_its_probe_prefix() {
        // vmangos player names carry no digits, so a probe fleet spells its index.
        assert_eq!(suffix_of("Probefour"), "four");
        assert_eq!(suffix_of("Probezero"), "zero");
        // Any other name is its own word, cut to fit a 12-character name.
        assert_eq!(suffix_of("Tester"), "tester");
        assert_eq!(suffix_of("Longcharname"), "longch");
        assert_eq!(suffix_of("Probe"), "probe");
    }
}
