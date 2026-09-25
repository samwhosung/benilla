//! **What kind of run is this, and whose** — the always-present layer, on the player's side of the
//! `dev` seam (built in 1174).
//!
//! Everything here answers a question about *the run itself*, and every answer has a
//! **player-faithful default**: nobody is driving the camera, the client starts at the login
//! screen, no rig owns the character pick, this checkout belongs to nobody in particular. A dev
//! build's instruments then move those answers off their defaults — which is the one direction the
//! seam allows. Gameplay reads this module; gameplay never reads `capture`, `debug_panel` or
//! `perf`.
//!
//! It exists because the alternative kept losing. 0026 asked for "a player-safe home or default"
//! for exactly this class of fact in June 2026 and named two of them; by August there were 24
//! references from non-dev code into the instruments (1173), because the facts had no home and the
//! nearest thing that already knew the answer was the instrument itself.
//!
//! The engine drew the same line for itself one layer down: [`benilla_world::dev_state`]'s
//! `deterministic_run` reads `$WOW_CAPTURE` on its own account, because "run deterministically" is
//! a property of the world and the world must be able to ask it with no harness above it at all.
//! **One environment variable, several readers, no shared symbol** — that is the pattern, and
//! [`scenario_active`] below is its app-side twin. Keep the two in step.

use bevy::prelude::*;

/// **Something other than the player is authoring the camera and the avatar this run.**
///
/// Inserted by the capture harness, which is the only thing that ever drives them; a player build
/// compiles no inserter, so the resource simply never exists and every `run_if` below reads as
/// "the player is in charge" for free. The *type* lives here rather than in `capture` for one
/// reason: `player::control`, `player::server_ride`, the self-model fade and the UI pass all name
/// it in a run condition, and a run condition that names a dev type is a dependency on dev.
#[derive(Resource)]
pub(crate) struct CaptureMode;

/// The rig's derived character name, when `$WOW_RIG` names a body.
///
/// Inserted by `capture::ProbeRigPlugin` at build time; absent otherwise — which is the player
/// answer, and the answer in any run the rig is not driving. The character-select roster reads it
/// to know that the pick is already spoken for, so it must not also honour `$WOW_CHAR`: the rig may
/// have to *create* its body first, and the `WOW_CHAR` fast path is a one-shot that structurally
/// cannot wait for that. Before 1174 the roster asked the harness directly.
#[derive(Resource)]
pub(crate) struct RigCharacter(pub(crate) String);

/// **Is a capture running?** (`$WOW_CAPTURE` set.)
///
/// Read before any plugin builds — it decides whether the net thread starts, what size the window
/// opens at, and whether clutter/anim-LOD randomness is pinned. All of those are properties of the
/// run, not of the harness, and all of them must have an answer in a build with no harness in it.
/// See the module doc for why this reads the variable rather than asking `capture`.
pub(crate) fn scenario_active() -> bool {
    std::env::var("WOW_CAPTURE").is_ok()
}

/// **Is the player UI opted into this capture?** — the harness's [`crate::capture::ui_opted_in`],
/// forwarded from here for the same reason [`scenario_active`] lives here: three consumers outside
/// the harness need the answer (the UI load, the synthetic unit feed, the window size), and **all
/// of them must still compile in a build with no harness in it**. `mod capture` is
/// `#[cfg(feature = "dev")]`; this module is not.
///
/// `false` in a player build is the correct answer and not a stub: that binary has no scenario
/// table and no `$WOW_CAPTURE` handling, so there is no capture for the UI to be opted into.
#[cfg(feature = "dev")]
pub(crate) fn capture_ui_opted_in() -> bool {
    crate::capture::ui_opted_in()
}

#[cfg(not(feature = "dev"))]
pub(crate) fn capture_ui_opted_in() -> bool {
    false
}

/// **Do the credentials come from the environment?** (`$WOW_USER` and `$WOW_PASS`, both — the
/// login screen's env fast path.)
///
/// The player answer is `false`, and it is the default: with neither set the client opens at the
/// login screen and waits for somebody to type. Setting both says *"log in without waiting for me
/// to type"* — and **that is the only thing it says.** It is not a claim about who is in the room;
/// [`unattended`] is the fact for that, and keeping the two apart is. There is no
/// default account: one credential without the other is a typed login, and `$WOW_CHAR` alone is
/// the roster's fast path after one (`char_select`), not a login.
pub(crate) fn env_login() -> bool {
    ["WOW_USER", "WOW_PASS"]
        .iter()
        .all(|k| std::env::var_os(k).is_some())
}

/// **Is there nobody here?** (`$WOW_UNATTENDED`, or the two runs that are automated by
/// construction: `$WOW_CAPTURE` and `$WOW_RIG`.)
///
/// The player answer is `false` and it is the **default**, like every other answer in this module:
/// a client with nobody at the keyboard is the exception, and an exception declares itself. The
/// direction is the whole point — guessing "attended" when a probe is running costs that probe a
/// timeout, loudly; guessing "unattended" when a person is playing ships them a client that fights
/// another client for their account, silently. Those two are not worth the same, so the doubt goes
/// one way.
///
/// **This is the fact [`env_login`] was mistaken for, twice.** Env credentials mean "log in without
/// typing"; they say nothing about the room — and the director plays with
/// `WOW_USER=… WOW_PASS=… cargo play`, the example line in `.cargo/config.toml`. So
/// every session they played read as "a harness": a typed password's refusal killed the process
/// (fixed on 2026-08-28 by asking `announced` — direct evidence — instead of the environment) and,
/// the half that fix left standing, a kick sent the client racing to take the account back instead
/// of to the login screen. That is decision 1262's reported bug, reappearing three weeks later
/// with not one line of its code changed, because the fact underneath it was never true. 1769.
///
/// `WOW_CAPTURE` and `WOW_RIG` are folded in because they *cannot* be a person: a capture authors
/// the camera and a rig drives the body, so a run that sets either has already said what it is and
/// cannot forget to. Everything else declares — `scripts/smoke.sh`, `cine.sh`, `summon-live.sh`
/// and an ad-hoc probe run (`docs/CONTRIBUTING.md`, "Running it unattended") all pass
/// `WOW_UNATTENDED=1`.
///
/// Read by whatever may act *instead of* a person: the lost-session verdict (the
/// session-loss readers never call this directly, [`crate::net::DisconnectedMessage::new`] asks
/// once at the wire edge and every reader acts on the verdict it carries) and the two `FATAL`
/// exits that keep a driverless run from burning its wall-clock on a dialog.
pub(crate) fn unattended() -> bool {
    ["WOW_UNATTENDED", "WOW_CAPTURE", "WOW_RIG"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
}

/// **This run cannot get past the screen it is on — may the client end it instead of a person?**
/// (decision 1371's `FATAL` marker, resting on 1769's fact.)
///
/// `true` only for a run that has declared itself [`unattended`]: it exits non-zero on the one
/// greppable marker a leg runner keys on, rather than parking a driverless run on a dialog for
/// its whole wall-clock. Otherwise the dialog stays up for whoever is there.
///
/// The `false` arm still speaks, when the credentials came from the environment: a probe that
/// quietly parks for 300 s instead of failing in 5 is precisely what 1769's default trades away,
/// so the log names the declaration that buys the old behaviour back. Both arms live here because
/// three call sites — two in [`crate::login`], one in [`crate::char_select`] — must not drift
/// apart on either the verdict or the marker a leg runner greps for.
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

/// **Which screen the client starts on.** A player starts at the login screen, always; a capture
/// boots straight into the world (no net, no picker), and a *glue* capture boots onto the very
/// screen it photographs.
///
/// The one place this module forwards into the dev half, and deliberately the only one: the answer
/// is needed *before the plugins build* (it is `CharSelectPlugin`'s `start`), so it cannot be a
/// resource an instrument inserts the way [`CaptureMode`] and [`RigCharacter`] are. The forward is
/// one call, in one direction, and the player arm names nothing at all.
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

/// **Does this build offer developer affordances at all?** `false` in a player build.
///
/// The generalisation of 1176's one-door rule, and the thing that record should have written.
/// A dev affordance does not have to live in a dev module: the world map's
/// Alt-click jump is in `ui_world_map`, the `/castvis` family is in `ui_chat`, the dev key plane's
/// cost is in `bindings`. None of them names a dev root, so none of them fails to compile, and all
/// of them shipped to players in 1174 exactly as free-fly did.
///
/// So the question a gameplay module asks is not "is the debug panel compiled in" — it is **"may I
/// offer this at all"**, and there is one place to ask it. Held by
/// [`tests::the_dev_plane_has_exactly_one_door`].
pub(crate) fn dev_affordances() -> bool {
    cfg!(feature = "dev")
}

/// **Did a dev-chord affordance just fire?** `Ctrl`+`Shift`+*key*, and `false` in a player build.
///
/// The one door onto the dev plane, and the reason it exists: the plane is
/// [`benilla_world::modkeys::dev_chord`], which lives in the **engine** (1160 moved it there —
/// "nothing about which two modifiers are the dev plane is a debug-panel opinion"), and the engine
/// is always compiled. So a gameplay module asking for a dev chord creates **no symbol into a dev
/// module**, compiles clean with `--no-default-features`, and ships a live dev affordance to a
/// player. That is exactly what happened: 1174 landed a green player build in which `Ctrl+Shift+F`
/// still flew the camera through the world, `+G` still teleported the avatar, and `+M` still muted
/// the game.
///
/// Routing every non-dev reader through here makes the whole plane dark at once rather than five
/// keys at a time, and makes the next dev chord dark for free. The build gate cannot see this class
/// — nothing fails to compile — so it is held by [`tests::the_dev_plane_has_exactly_one_door`]
/// instead, in the suite the gates already run.
pub(crate) fn dev_chord(keys: &ButtonInput<KeyCode>, key: KeyCode) -> bool {
    dev_affordances() && benilla_world::modkeys::dev_chord(keys, key)
}

/// The free-fly hint the take-control line carries, or empty when there is no chord to advertise.
/// A player build must not offer a key that does nothing. Spelled from
/// [`benilla_world::modkeys::DEV_CHORD`] like every other surface that names the plane, so it
/// cannot drift from what the chord actually listens for.
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

/// **This crate's source directory on a dev build, `None` in a player build** — the seam's second
/// door, and the one [`dev_affordances`] cannot be.
///
/// `dev_affordances()` answers "may I offer this", at runtime. Part of the seam is not about
/// offering anything: it is about a **string being absent from the binary**.
/// `env!("CARGO_MANIFEST_DIR")` names the build machine's home directory, and a runtime `if` does
/// not remove it — the literal is still compiled in, still in `strings`, still shipped. 1174's
/// pool-slot guard made exactly that mistake one level down ("inert in a player build" read as
/// "absent from a player build"), and decision 1175 is entirely about a binary that stops
/// depending on the machine that built it. So this `cfg` has to be real, and it lives here, once,
/// behind a value instead of scattered across the modules that need it.
///
/// Two callers, both resolving content or state and neither of them an affordance:
/// [`crate::ui_script`]'s dev probe into `assets/ui` (so editing FrameXML costs no recompile) and
/// [`crate::local_state`]'s project-folder home. Both get `None` in a player build and fall through
/// to the compiled-in copy and to `<exe dir>` respectively, with no path literal in the binary.
///
/// Held by [`tests::the_dev_plane_has_exactly_one_door`] like the rest of the seam.
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

/// The pre-connect **account guard**, consulted by the login policy's env fast path
/// ([`crate::login`]). Returns `Err(explanation)` when this checkout declares the account its
/// scripted runs log in as ([`declared_identity`]) and the fast path is about to authenticate as
/// anything else: a login KICKS whoever holds the account — a player mid-session, or another
/// checkout's probe, whose kicked client (the 0065 teardown) despawns every net entity, so that
/// run's next sample reads a unit-less world and prints garbage.
///
/// The declaration is a file, never the build directory: a path is nobody's identity. Without
/// one (a plain clone, a player build) the guard is inert — it has no business having an opinion
/// about a login it cannot attribute. That is why this lives here and not in `preflight`
/// (decision 0649's home for it, until 1174 made `preflight` dev-only): gameplay's login policy
/// calls it, and gameplay may not call an instrument.
///
/// `WOW_ALLOW_ACCOUNT=1` is the escape hatch for the rare deliberate cross-account run; it turns the
/// refusal into a warning rather than silence, because the kick still happens.
pub(crate) fn account_guard(user: &str) -> Result<(), String> {
    guard_for(declared_identity().as_ref(), user)
}

/// What `.probe-identity` at the project root declares: the account a scripted run from this
/// checkout logs in as (`WOW_USER=`, `WOW_PASS=`, `WOW_CHAR=`, one per line; never committed —
/// `.gitignore` carries it). A machine that runs several checkouts against one server gives each
/// its own account this way, and `scripts/probe-identity.sh` reads the same file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeclaredIdentity {
    pub(crate) user: String,
    pub(crate) character: String,
}

/// The declaration, read off the project folder — `dev` only, the install resolver's own
/// project-folder rung: a player build has no source tree to name, and no
/// declared identity to keep.
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

/// The file's three lines, as an identity — `None` unless the account and the character are
/// both there (the password is the scripts' business, never this crate's).
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

/// [`account_guard`]'s decision, with the declaration passed in so it is testable without a file.
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

/// The word a rig appends to the bodies it mints for this checkout (`<Race3><Class3><word>[f]`),
/// so that several checkouts against one server never collide on a character name: the declared
/// character, lowercased, without a leading `probe` — a probe fleet names its bodies
/// `Probe<word>`, and the word is the identity. `None` without a declaration; the rig then
/// configures the body it logged in as.
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

    /// **The dev plane has exactly one door**.
    ///
    /// The player-build gate (`cargo build -p benilla --no-default-features`) catches a *symbol*
    /// crossing into a dev module. It cannot catch this: `benilla_world::modkeys::dev_chord` is
    /// engine, always compiled, so a gameplay module that asks it for `Ctrl+Shift+F` compiles
    /// perfectly in a player build and ships free-fly to a player. 1174 landed exactly that, green.
    ///
    /// So the rule is checked instead of remembered, the way 0789 checks the probe clock: outside
    /// the dev roots, `dev_chord` is [`super::dev_chord`]'s to call and nobody else's. A dev module
    /// may call the engine's directly — it does not exist in a player build to be reached.
    ///
    /// This does NOT police [`benilla_world::modkeys::DEV_CHORD`], the display string: naming the
    /// plane in a label is not offering a key.
    #[test]
    fn the_dev_plane_has_exactly_one_door() {
        // Everything that is compiled out by `--no-default-features`, plus this module, which is
        // the door itself. Anything else asking the engine for a dev chord is the bug.
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
        // Assembled at runtime so the checker does not flag its own source — the same trick, and
        // the same proof of teeth, as `capture::probes`' clock test.
        let needle = format!("modkeys::{}(", "dev_chord");
        // The second clause: `#[cfg(feature = "dev")]` itself. Seam knowledge has exactly three
        // addresses — this module (the always-present layer), `dev.rs` (the group), and `lib.rs`
        // (the module declarations). A `cfg` attribute anywhere else means a gameplay module has
        // learned the seam exists, which is the state 1174's whole diff cleared the tree out of,
        // and which spreads one file at a time if nothing objects.
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

    /// **Two questions, two facts** — and the fixture is the line the director
    /// actually types, so a future merge of the two answers fails here rather than in their game.
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
        // `WOW_USER=… WOW_PASS=… cargo play`, `.cargo/config.toml`'s example, and the
        // director's launch line since 2026-08-29. Log them in without typing, yes. Decide things
        // on their behalf, no.
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
        // Our own account: the whole point of the declaration.
        assert!(guard_for(Some(&id), "probe4").is_ok());
        assert!(guard_for(Some(&id), "PROBE4").is_ok()); // vmangos accounts are case-insensitive

        // Anyone else's account — a player's, another checkout's probe — kicks a live session.
        let player = guard_for(Some(&id), "player").unwrap_err();
        assert!(player.contains("probe4") && player.contains("kicks"));
        // The override hint belongs to the caller that can act on it, not to the reason.
        assert!(!player.contains("WOW_ALLOW_ACCOUNT"));
        assert!(guard_for(Some(&id), "probe7").is_err());
        // No declaration, no standing.
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
        // vmangos player names carry no digits, so a probe fleet spells its index: the word is
        // what follows `Probe`, and the rig's `Taudru<word>` keeps one body per checkout.
        assert_eq!(suffix_of("Probefour"), "four");
        assert_eq!(suffix_of("Probezero"), "zero");
        // Any other name is its own word, cut to what a 12-character name has room for.
        assert_eq!(suffix_of("Tester"), "tester");
        assert_eq!(suffix_of("Longcharname"), "longch");
        assert_eq!(suffix_of("Probe"), "probe");
    }
}
