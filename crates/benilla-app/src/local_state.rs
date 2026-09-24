//! Where benilla keeps **local state** — THE LAW (decision 0954, placement amended by 1175). Every
//! file benilla persists on a player's machine — config today; realmlist, caches, per-character
//! state as they arrive — lives in **one visible folder, `benilla-config/`, beside the benilla binary**.
//! Never scattered into the install, never hidden in a platform config dir: the 1.12 client is
//! itself fully portable (`WTF/`, `realmlist.wtf`, `Cache/` all in-folder — verified on the live
//! install), its community expects the drop-in-a-folder shape, and one folder answers "what here
//! is benilla's?" at a glance (the director's call, 2026-08-04; prior art: Dolphin's
//! `portable.txt`, DREAMM's config-next-to-exe).
//!
//! **0954 put this folder inside the WoW install and 1175 moved it out**, on the director's word:
//! *the benilla folder should be outside the wow folder … in root of the project or same folder as
//! the benilla binary.* 0954's reasoning was sound for what it knew — `$WOW_DATA` was the only
//! root the client reliably had — but it made benilla's own state a *guest* of the install, so
//! pointing at a second install silently swapped your keybinds and a data path that failed to
//! resolve took your config with it. benilla reads a WoW install; it does not live in one. One
//! behaviour improves as a result: the state folder no longer depends on finding the install at
//! all. Everything else about 0954 stands, including this module being the only place in the tree
//! that may compute a persistence path (grep `local_state::` for every resident) — a new file gets
//! a path fn here with a doc comment saying what it holds and why it is scoped the way it is.
//! **Those doc comments are the layout table** (decision 1689): 0954's own table is a
//! point-in-time snapshot of the law it set and, like every decision record, immutable — it lists
//! none of the residents added since, and it should not.
//!
//! Resolution, in order — deliberately the **same three-step shape** as
//! [`benilla_formats::wow_data`], because it is one law over two folders:
//! 1. **`$BENILLA_HOME`** — explicit override (tests point it at a tempdir; a shared-config setup
//!    points it wherever it likes).
//! 2. **`<project folder>/benilla-config/`** — `#[cfg(feature = "dev")]` only, so dev runs across the
//!    worktrees of the repo keep one predictable place. Gated for the same reason the install resolver's
//!    project-folder probe is: a shipped binary must not carry the build machine's source tree.
//!    (`.gitignore` carries `/benilla-config` for it.)
//! 3. **`<exe dir>/benilla-config/`** — the release answer: your settings sit next to the program that
//!    wrote them.
//!
//! **Capture/probe runs are hermetic**: with `$WOW_CAPTURE` set every path resolves to `None` —
//! a deterministic capture must not read one machine's saved settings or write anything back
//! (the harness owns its inputs; decision 0008's reproducibility posture).

use std::io::Write;
use std::path::{Path, PathBuf};

/// The folder's name, everywhere. **Not** `benilla/`, which is what 0954 called it and what 1175
/// §4 assumed it would keep: beside the binary that name collides with the binary itself, which is
/// also `benilla`. The bare-directory falsifier caught it immediately — `<exe dir>/benilla/`
/// resolved onto the executable and every read failed with `Not a directory (os error 20)`, so a
/// player's settings had nowhere to live. 0954 could name it for the program because it sat inside
/// somebody else's folder and had to announce whose it was; beside our own binary that reason is
/// gone and only the collision is left. The reference client has the same shape and the same
/// answer — `WoW.exe` keeps its state in `WTF/`, named for what it is (and `WTF/` is exactly why we
/// cannot borrow that name: drop benilla into a real install and it is already taken).
/// The director's call, 2026-08-10.
const STATE_DIR: &str = "benilla-config";

/// The one benilla-state folder, or `None` when persistence is off (a hermetic capture run, or a
/// platform with no discoverable executable path). Existence is NOT guaranteed — [`write_atomic`]
/// creates it on first write; readers just try their file and treat absent as defaults.
pub(crate) fn home() -> Option<PathBuf> {
    if std::env::var_os("WOW_CAPTURE").is_some() {
        return None; // hermetic: captures neither read nor write player state
    }
    if let Some(over) = std::env::var_os("BENILLA_HOME") {
        return Some(PathBuf::from(over));
    }
    // 2 · the project folder, dev builds only — see [`dev_project_root`] for why that is the
    // PRIMARY checkout and not the worktree this binary was built in. `None` in a player build,
    // where there is no source tree to name (`run_mode::dev_source_dir`).
    if let Some(root) = dev_project_root() {
        return Some(root.join(STATE_DIR));
    }
    // 3 · beside the binary. A dev build never reaches here — that is what step 2 means by "one
    // predictable place", and it is why the release path is proven with a player build, not a
    // dev one.
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(STATE_DIR))
}

/// The project folder a dev build keeps its state in: **the primary checkout**, whichever worktree
/// this binary was actually built in.
///
/// This is the one place 1175's §4 needed a correction on contact with how we work. `benilla/` used
/// to hang off `$WOW_DATA`, and every worktree of the repo points `WoW` at the same install — so
/// there has always been exactly ONE settings folder no matter which worktree built the binary.
/// Resolving to `CARGO_MANIFEST_DIR` instead would give one per worktree, and the player's
/// keybinds, macros and camera pose would appear to reset whenever a session happened to build
/// in a different one. That is a silent, recurring surprise, and it is not what "one predictable
/// place" meant.
///
/// A linked worktree's `.git` is a **file** reading `gitdir: <primary>/.git/worktrees/<slot>`, so
/// the primary is derivable with no git binary and no build script: walk up to the common dir and
/// take its parent. In the primary checkout `.git` is an ordinary directory and the answer is the
/// crate root's grandparent, unchanged. Anything unexpected falls back to that same answer rather
/// than to `None` — a wrong-but-present settings folder beats losing persistence entirely.
fn dev_project_root() -> Option<PathBuf> {
    let here = crate::run_mode::dev_source_dir()?.ancestors().nth(2)?;
    let dot_git = here.join(".git");
    // A linked worktree: `.git` is a file pointing at `<primary>/.git/worktrees/<name>`.
    if dot_git.is_file() {
        if let Some(gitdir) = std::fs::read_to_string(&dot_git).ok().and_then(|s| {
            s.trim()
                .strip_prefix("gitdir:")
                .map(|p| p.trim().to_owned())
        }) {
            // …/.git/worktrees/<name> → …/.git → the primary checkout.
            if let Some(primary) = Path::new(&gitdir)
                .ancestors()
                .nth(2)
                .and_then(|d| d.parent())
            {
                if primary.is_dir() {
                    return Some(primary.to_path_buf());
                }
            }
        }
    }
    Some(here.to_path_buf())
}

/// `benilla-config/config.toml` — the CVar overrides (the `Config.wtf` analog; see `crate::cvars`).
pub(crate) fn config_path() -> Option<PathBuf> {
    home().map(|h| h.join("config.toml"))
}

/// `benilla-config/macros/account.txt` — the account-wide macro tab (indices 1..=18; decision 0983). One
/// file per scope rather than one file with two sections, because the two have different lifetimes:
/// the account tab follows the install, a character tab dies with its character.
pub(crate) fn macros_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("macros/account.txt"))
}

/// `benilla-config/macros/<realm>-<character>.txt` — the per-character macro tab (indices 19..=36). The
/// reference nests these (`WTF/Account/<ACC>/<REALM>/<CHAR>/macros-cache.txt`); benilla flattens to
/// one `macros/` folder because [`home`] is already account-scoped by the install it sits beside.
pub(crate) fn macros_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("macros").join(format!("{key}.txt")))
}

/// `benilla-config/bindings/account.txt` — the account-wide key bindings (decision 0997; the
/// `bindings-cache.wtf` analog, command-centric diff-vs-defaults so a growing command set keeps
/// its new defaults).
pub(crate) fn bindings_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("bindings/account.txt"))
}

/// `benilla-config/bindings/<realm>-<character>.txt` — the character-specific binding set (decision
/// 0997). Its existence IS the "character specific key bindings" state: the window's checkbox
/// writes it, Okay-back-to-general deletes it (the reference's confirmed permanent delete).
pub(crate) fn bindings_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("bindings").join(format!("{key}.txt")))
}

/// `benilla-config/saved-variables.lua` — the Lua saved-variables file (decision 1128; the
/// `WTF/Account/<ACC>/SavedVariables.lua` analog). Install-scoped, like the reference's account
/// scope: written whole at logout/exit, executed as a chunk at UI load. One file, because our
/// ported UI is one FrameXML tree rather than a set of addons — when third-party addons land they
/// get `benilla-config/saved/<Addon>.lua`, the reference's per-addon shape.
pub(crate) fn saved_variables_path() -> Option<PathBuf> {
    home().map(|h| h.join("saved-variables.lua"))
}

/// `benilla-config/addons/<Realm>-<Character>.txt` — the AddOn **enable state** (1188 phase 2).
///
/// Per character, like the reference: it writes `WTF/Account/<ACC>/<Realm>/<Char>/AddOns.txt` as
/// the last step of its UI shutdown (`0x490bd0`'s tail, after the saved-variables files). The
/// file's own format is the reference's too — one `<AddOnName>: enabled|disabled` per line,
/// confirmed against a real 1.12 install rather than remembered. An addon absent from the file is
/// enabled, which is what makes a freshly-dropped-in folder just work.
pub(crate) fn addons_state_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("addons").join(format!("{key}.txt")))
}

/// `benilla-config/saved/` — **per-addon** saved variables, account scope (1188 phase 3): one
/// `<Addon>.lua` per addon that declares `## SavedVariables`.
///
/// The reference's shape, one level flatter: it writes
/// `WTF/Account/<ACC>/SavedVariables/<Addon>.lua`, and our whole state folder is already
/// per-install, so the folder IS the account scope (0954, and 1128 reserved this exact path —
/// spelled `benilla/saved/` before 1180 renamed the folder).
///
/// Distinct from [`saved_variables_path`], which is the *flat* channel our own FrameXML uses
/// through `RegisterForSave`. The reference has both too, and they are not the same mechanism.
pub(crate) fn addon_saved_account_dir() -> Option<PathBuf> {
    home().map(|h| h.join("saved"))
}

/// `benilla-config/saved/<Realm>-<Character>/` — per-addon saved variables, character scope
/// (`## SavedVariablesPerCharacter`). Loaded **after** the account file, so it wins.
pub(crate) fn addon_saved_character_dir(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("saved").join(key))
}

/// `benilla-config/camera/<realm>-<character>.txt` — the third-person camera pose (decision 1131; the
/// `<Char>/camera-settings.txt` analog, and character-scoped for the same reason it is there: a
/// gnome rogue and a tauren warrior want different zooms). Two lines, the reference's own keys and
/// order, so the file stays readable beside its ancestor.
pub(crate) fn camera_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("camera").join(format!("{key}.txt")))
}

/// `benilla-config/account` — the account name the login screen remembers (decision 0539 §4; the
/// reference's `GetSavedAccountName`/`SetSavedAccountName`, whose own store is `WTF/Config.wtf`'s
/// `accountName`). Install-scoped like every other resident here.
///
/// It resolved to `$HOME/.benilla/account` until decision 1181, through a second path function
/// that `login` had grown before this module's law existed (0539 predates 0954, whose layout table
/// never mentioned it). That split a player's state across two folders and — the part that
/// actually bit — skipped [`home`]'s hermetic guard, so a **capture read the account name off
/// whatever machine it ran on** and rendered it into the login screen it was photographing.
pub(crate) fn saved_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("account"))
}

/// `benilla-config/chat/<realm>-<character>.txt` — the chat windows' saved state (decision 1589):
/// the background tint, the background alpha, the font size a chat tab's right-click menu sets,
/// and the window's lock. **Character-scoped**, where the reference keeps the same four inside its
/// per-character `chat-cache.txt` — a raid alt and a questing alt want different chat boxes. See
/// [`crate::ui_chat`]'s `settings` module for the file's shape and why it is a subset of its
/// ancestor.
pub(crate) fn chat_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    Some(home()?.join("chat").join(format!(
        "{}-{}.txt",
        file_token(realm),
        file_token(character)
    )))
}

/// `benilla-config/layout/<realm>-<character>.txt` — the **layout cache**: where every frame the
/// player has moved or resized sits, so a dragged chat window is still there after a relog.
///
/// The reference's own resident is `WTF/Account/<ACC>/<REALM>/<CHAR>/layout-cache.txt`, written by
/// the engine for exactly the frames carrying the userPlaced bit — so the scope is its scope and
/// the name is its name, one folder flatter like every other resident here ([`home`] is already
/// account-scoped by the binary it sits beside). Character-scoped because the state IS per
/// character: a raid alt wants the combat log wide where a questing alt wants it out of the way.
///
/// The file's shape, and the seam that fills it, are [`crate::ui_layout`]'s.
pub(crate) fn layout_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    Some(home()?.join("layout").join(format!(
        "{}-{}.txt",
        file_token(realm),
        file_token(character)
    )))
}

/// `benilla-config/cache/<realm>.tsv` — the **name cache**: the player, creature and pet names the
/// server has already answered for, kept across sessions (decision 1689).
///
/// The reference's residents are `WDB/namecache.wdb`, `creaturecache.wdb` and `petnamecache.wdb`
/// — three files **inside the install**, which is exactly where benilla may not write (the install
/// is read-only, `docs/METHOD.md`'s hard rule), so ours lives here like every other thing we persist.
/// One file rather than three because our three stores share a lifetime and a realm; the
/// reference's split follows its `DBCache<T>` template instantiation, not a property of the data.
///
/// **Realm-scoped, where the reference's is not** — and that is a correctness fix, not a
/// preference. Every key here is realm-local: a player guid, a creature template entry and a pet
/// number all mean something different on another realm, so one shared file would serve a second
/// realm another realm's names. The reference gets away with it because a 1.12 install typically
/// saw one realm.
pub(crate) fn name_cache_path(realm: &str) -> Option<PathBuf> {
    Some(
        home()?
            .join("cache")
            .join(format!("{}.tsv", file_token(realm))),
    )
}

/// `benilla-config/shots.txt` — the framing instrument's appended camera poses (decision 0600). A dev
/// affordance (`/shot`, compiled out by `--no-default-features` since 1179), but it persists on a
/// real machine, so it resolves here like everything else rather than through a private path.
/// `Logs/` — `WoWChatLog.txt` and `WoWCombatLog.txt`, the reference's two names beside its
/// `WTF`, kept here beside ours ([`crate::ui_chat`]'s logging).
pub(crate) fn logs_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Logs"))
}

pub(crate) fn shots_path() -> Option<PathBuf> {
    home().map(|h| h.join("shots.txt"))
}

/// `benilla-config/Screenshots/` — where the print-screen key writes (decisions 1486, 1487).
///
/// **The reference writes `Screenshots\\` inside the install and we deliberately do not.** benilla
/// reads a WoW install; it never writes to one (decision 1486, the director's rule) — the folder is
/// somebody else's, it is shared with other tools on this machine, and a client that
/// scatters its output through it makes "what here is benilla's?" unanswerable. So the reference's
/// own folder NAME is kept, capital S and all, and only its parent moves: a player who knows where
/// WoW put screenshots finds the same folder one level over.
///
/// Unlike every other resident here this is a DIRECTORY, not a file, and it is not written through
/// [`write_atomic`] — an image is bytes, not a settings diff, and it is written once and never
/// rewritten, so the tmp-then-rename dance buys nothing. It resolves through [`home`] like
/// everything else, which is what makes a capture run hermetic ([`home`] answers `None`).
pub(crate) fn screenshots_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Screenshots"))
}

/// `benilla-config/Diagnostics/` — where the stuck-thread self-sampler drops its profiles
/// ([`crate::perf::stall`]) and the FPS journal its rows ([`fps_journal_path`]).
///
/// **This was `~/Library/Logs/benilla/` until 2026-08-27, hand-built from `$HOME`** — a platform
/// log directory, which is the exact shape the one-folder rule names as forbidden ("never a
/// hand-built path, never a platform config dir"). It had been that way since decision 0713, and it
/// went unnoticed because the sampler only writes when something is already wrong, so the stray
/// folder appeared on the bad days and nobody was looking at paths on a bad day. It surfaced while
/// proving out the MSAA clamp (1631): three probe runs stalled, and the stall lines named a
/// directory that had no business existing.
///
/// It resolves through [`home`] like every other resident, which costs the sampler its capture
/// runs (`home` answers `None` there, deliberately) and is the right trade: a `WOW_CAPTURE` run is
/// bounded by the harness timeout and already has the probe backstop's `_exit(0)`, while the runs
/// this instrument was built for — the director's, and the long probe rounds — keep it. The
/// alternative, an ungated second accessor, would put "which paths are exempt from hermetic?" back
/// into someone's head, which is what the single rule exists to prevent.
pub(crate) fn diagnostics_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Diagnostics"))
}

/// `benilla-config/Diagnostics/fps-journal.csv` — the FPS journal's rows while the `fpsJournal`
/// CVar is on (decision 2008): the file a reporter attaches. `None` on a hermetic run like
/// everything here; the harness names its own path through `WOW_FPS_JOURNAL` instead.
pub(crate) fn fps_journal_path() -> Option<PathBuf> {
    diagnostics_dir().map(|d| d.join("fps-journal.csv"))
}

/// Make an arbitrary realm/character name safe as one path component: anything that is not a
/// letter or digit becomes `_`, so a realm called `Hydraxian Waterlords` or one with a slash cannot
/// escape the folder or collide with the path separator.
///
/// **Letters are any script's, kept as they are** — the reference names its per-character folder
/// with the raw name (`WTF/Account/<ACC>/<REALM>/<CHAR>/`). This was ASCII-only, which folded every
/// other letter to `_`: vmangos accepts extended-Latin, Cyrillic and East-Asian names by default
/// (`StrictPlayerNames = 0`), so `Zoë` and `Zoé` — or any two Cyrillic names of one length — shared
/// one set of macros, bindings, addon state and SavedVariables, and each login saved over the
/// other's. A character name is letters only (vmangos `isValidString`), so keeping every letter
/// makes the token one-to-one for them; an ASCII name maps exactly as before, so no existing file
/// moves.
fn file_token(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if t.is_empty() {
        "unknown".into()
    } else {
        t
    }
}

/// Write a state file atomically: parent dirs created, contents to `<path>.tmp`, then rename —
/// a crash mid-write leaves the old file intact, never a truncated one. The one write path for
/// every resident of the folder.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes())
}

/// [`write_atomic`] for a resident that is bytes, not text — the saved-variables files, which
/// carry a Lua byte string's bytes as they are.
pub(crate) fn write_atomic_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Shared test plumbing for anything that toggles the persistence env vars: `std::env::set_var`
/// is process-global and cargo runs tests in threads, so every such test takes [`ENV_LOCK`] and
/// scopes its overrides in [`EnvGuard`]s (restore-on-drop). Used here and by `crate::cvars`'s
/// end-to-end test.
#[cfg(test)]
pub(crate) mod test_env {
    pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) struct EnvGuard(&'static str, Option<std::ffi::OsString>);
    impl EnvGuard {
        pub(crate) fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self(key, old)
        }
        pub(crate) fn unset(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            std::env::remove_var(key);
            Self(key, old)
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.1 {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_env::{EnvGuard, ENV_LOCK};
    use super::*;

    /// **Two characters never share a file.** Every per-character resident keys on this token, so
    /// a collision is one character's macros, bindings and SavedVariables loaded and then saved
    /// over by another. ASCII names — every file already on disk — keep their exact paths.
    #[test]
    fn distinct_names_never_share_a_token() {
        assert_ne!(file_token("Вася"), file_token("Петя"));
        assert_ne!(file_token("Zoë"), file_token("Zoé"));
        assert_eq!(file_token("Zoë"), "Zoë");
        assert_eq!(file_token("Onehunter"), "Onehunter");
        assert_eq!(file_token("Hydraxian Waterlords"), "Hydraxian_Waterlords");
        assert_eq!(
            file_token("a/b\\c:d"),
            "a_b_c_d",
            "separators never survive"
        );
        assert_eq!(
            file_token("realm-name"),
            "realm_name",
            "the key's own `-` stays unforgeable"
        );
    }

    /// The residents' layout, exercised through the override — which is the *only* step of
    /// [`home`] whose answer a test can state, since the other two are the machine's project
    /// folder and the test runner's own exe directory. Those two get their own tests below; what
    /// this one owns is 0954's layout table, unchanged by 1175's move.
    #[test]
    fn the_home_law_override_then_the_residents_and_hermetic_captures() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-ls-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // 1 · the explicit override, and every resident hanging off it.
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.join(STATE_DIR).to_str().unwrap());
        assert_eq!(home(), Some(tmp.join(STATE_DIR)));
        assert_eq!(config_path(), Some(tmp.join("benilla-config/config.toml")));
        // The macro residents (decision 0983): one account file, one per character, and every
        // realm/character name reduced to a safe single path component.
        assert_eq!(
            macros_account_path(),
            Some(tmp.join("benilla-config/macros/account.txt"))
        );
        // The saved-variables resident (decision 1128) — one install-scoped file.
        assert_eq!(
            saved_variables_path(),
            Some(tmp.join("benilla-config/saved-variables.lua"))
        );
        assert_eq!(
            macros_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/macros/Hydraxian_Waterlords-Probeone.txt"))
        );
        assert_eq!(
            macros_character_path("../evil", "a/b"),
            Some(tmp.join("benilla-config/macros/___evil-a_b.txt")),
            "no name can escape the folder"
        );
        // The camera-pose resident (decision 1131) — character-scoped, same key shape.
        assert_eq!(
            camera_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/camera/Hydraxian_Waterlords-Probeone.txt"))
        );
        // The chat windows' saved look (decision 1589) — character-scoped, same key shape again.
        assert_eq!(
            chat_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/chat/Hydraxian_Waterlords-Probeone.txt"))
        );
        // The layout cache — the userPlaced frames' geometry, character-scoped like its neighbours
        // and like the reference's own `layout-cache.txt`.
        assert_eq!(
            layout_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/layout/Hydraxian_Waterlords-Probeone.txt"))
        );

        // The login screen's remembered account name (decision 0539 §4) and the framing
        // instrument's pose log (0600) — the two residents that computed their own path in
        // `login::config_base` (`$HOME/.benilla`) until decision 1181 folded them in here.
        assert_eq!(
            saved_account_path(),
            Some(tmp.join("benilla-config/account"))
        );
        assert_eq!(shots_path(), Some(tmp.join("benilla-config/shots.txt")));
        // The print-screen folder (decisions 1486/1487) — the one resident that is a DIRECTORY,
        // and the one whose reference lives inside the install we refuse to write to.
        assert_eq!(
            screenshots_dir(),
            Some(tmp.join("benilla-config/Screenshots"))
        );

        // 0 · hermetic: a capture run resolves nothing, even with an override set. The account
        // name is the reason this line now matters more than it did: `config_base` had no such
        // guard, so a login-screen capture read whatever account the host machine had saved and
        // photographed it into the frame (decision 1181).
        let _c2 = EnvGuard::set("WOW_CAPTURE", "ui-options");
        assert_eq!(home(), None);
        assert_eq!(
            saved_account_path(),
            None,
            "a capture reads no saved account"
        );
        assert_eq!(shots_path(), None);
        assert_eq!(
            screenshots_dir(),
            None,
            "a capture writes no player screenshots"
        );
        assert_eq!(
            chat_character_path("Hydraxian Waterlords", "Probeone"),
            None,
            "a capture reads no player's chat look"
        );
        assert_eq!(
            layout_character_path("Hydraxian Waterlords", "Probeone"),
            None,
            "a capture reads no player's window layout"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// **The move itself** (1175 §4): the state folder no longer hangs off the WoW install, so a
    /// missing or wrong `$WOW_DATA` cannot take a player's settings with it. This is the exact
    /// case that used to return `None` — the one behaviour 1175 changes, and the reason it
    /// changed.
    #[test]
    fn the_state_folder_no_longer_depends_on_finding_the_install() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let _d = EnvGuard::set("WOW_DATA", "/nonexistent/benilla-test/Data");
        let h = home().expect("a broken install path must not cost the player their config");
        assert!(
            !h.starts_with("/nonexistent"),
            "home() still reads $WOW_DATA: {}",
            h.display()
        );
        assert!(h.ends_with(STATE_DIR), "{}", h.display());
    }

    /// **Where the folder lands, in both builds** — 1175's falsifier, as far as a test can take it.
    ///
    /// One test rather than a `#[cfg]`-ed pair, because the seam is `run_mode::dev_source_dir()`'s
    /// to know and nothing else's (1179): a player build has no source dir, so the state folder
    /// must sit beside the binary; a dev build has one, and must resolve to the PRIMARY checkout
    /// so every worktree keeps sharing a single settings folder — exactly as they did when it
    /// hung off the shared install. The dev half is asserted structurally (a `.git` *file* means a
    /// linked worktree, and then the answer must be somewhere else), so it says the same thing
    /// whether it runs in the primary or in a slot.
    #[test]
    fn the_state_folder_lands_where_the_build_says() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let h = home().expect("home() always resolves outside a capture");
        assert!(h.ends_with(STATE_DIR), "{}", h.display());

        let Some(here) = crate::run_mode::dev_source_dir().and_then(|d| d.ancestors().nth(2))
        else {
            // Player build: beside the binary, and nowhere near a source tree.
            let exe_dir = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            assert_eq!(h, exe_dir.join(STATE_DIR));
            return;
        };

        if here.join(".git").is_file() {
            assert_ne!(
                h,
                here.join(STATE_DIR),
                "a linked worktree must not get its own settings folder — the worktrees share one"
            );
            assert!(
                h.parent().unwrap().join(".git").is_dir(),
                "resolved to {}, which is not a primary checkout",
                h.display()
            );
        } else {
            assert_eq!(h, here.join(STATE_DIR));
        }
    }

    #[test]
    fn write_atomic_creates_dirs_and_replaces_whole_files() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-wa-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let path = tmp.join("nested/config.toml");
        write_atomic(&path, "a = \"1\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = \"1\"\n");
        write_atomic(&path, "a = \"2\"\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a = \"2\"\n");
        assert!(!path.with_extension("tmp").exists(), "tmp renamed away");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
