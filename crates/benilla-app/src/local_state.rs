//! Where benilla keeps local state: one folder, `benilla-config/`, never inside the install, and
//! this module is the only place that computes a path into it; each path fn's doc says what it
//! holds. The 1.12 client is portable the same way, its `WTF/` and `Cache/` in its own folder.
//!
//! Resolution, in order, the same shape as [`benilla_formats::wow_data`]:
//! 1. `$BENILLA_HOME`.
//! 2. `<project folder>/benilla-config/`, dev builds only: a shipped binary must not carry the
//!    build machine's source tree.
//! 3. `<exe dir>/benilla-config/`.
//!
//! With `$WOW_CAPTURE` set every path resolves to `None`: a capture neither reads nor writes
//! player state.

use std::io::Write;
use std::path::{Path, PathBuf};

/// The folder's name: not `benilla`, which beside the binary is the executable itself, and not
/// `WTF`, which a real install already uses.
const STATE_DIR: &str = "benilla-config";

/// The state folder, or `None` when persistence is off (a capture run, or no executable path).
/// It may not exist yet: [`write_atomic`] creates it, and a reader treats a missing file as
/// defaults.
pub(crate) fn home() -> Option<PathBuf> {
    if std::env::var_os("WOW_CAPTURE").is_some() {
        return None; // hermetic: captures neither read nor write player state
    }
    if let Some(over) = std::env::var_os("BENILLA_HOME") {
        return Some(PathBuf::from(over));
    }
    // 2 · the project folder, dev builds only.
    if let Some(root) = dev_project_root() {
        return Some(root.join(STATE_DIR));
    }
    // 3 · beside the binary; a dev build never reaches here.
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(STATE_DIR))
}

/// The project folder a dev build keeps its state in: the primary checkout, whichever worktree
/// built the binary, so every worktree shares one settings folder. Anything unexpected falls back
/// to the crate root's grandparent rather than `None`.
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

/// `benilla-config/config.toml`: the CVar overrides, the `Config.wtf` analog.
pub(crate) fn config_path() -> Option<PathBuf> {
    home().map(|h| h.join("config.toml"))
}

/// `benilla-config/macros/account.txt`: the account-wide macro tab (indices 1..=18).
pub(crate) fn macros_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("macros/account.txt"))
}

/// `benilla-config/macros/<realm>-<character>.txt`: the per-character macro tab (19..=36), the
/// reference's `WTF/Account/<ACC>/<REALM>/<CHAR>/macros-cache.txt` flattened into one folder.
pub(crate) fn macros_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("macros").join(format!("{key}.txt")))
}

/// `benilla-config/bindings/account.txt`: the account-wide key bindings (the `bindings-cache.wtf`
/// analog), a diff against the defaults.
pub(crate) fn bindings_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("bindings/account.txt"))
}

/// `benilla-config/bindings/<realm>-<character>.txt`: the character-specific binding set. Its
/// existence is the "character specific key bindings" state; going back to general deletes it.
pub(crate) fn bindings_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("bindings").join(format!("{key}.txt")))
}

/// `benilla-config/saved-variables.lua`: the flat channel our FrameXML saves through
/// `RegisterForSave` (the `SavedVariables.lua` analog), written whole at logout, run at UI load.
pub(crate) fn saved_variables_path() -> Option<PathBuf> {
    home().map(|h| h.join("saved-variables.lua"))
}

/// `benilla-config/addons/<Realm>-<Character>.txt`: the AddOn enable state, per character in the
/// reference's `AddOns.txt` format (`<AddOnName>: enabled|disabled`), which it writes at the tail
/// of its UI shutdown (`0x490bd0`). An addon absent from the file is enabled.
pub(crate) fn addons_state_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("addons").join(format!("{key}.txt")))
}

/// `benilla-config/saved/`: per-addon saved variables, account scope, one `<Addon>.lua` per addon
/// declaring `## SavedVariables` (the reference's `WTF/Account/<ACC>/SavedVariables/<Addon>.lua`).
pub(crate) fn addon_saved_account_dir() -> Option<PathBuf> {
    home().map(|h| h.join("saved"))
}

/// `benilla-config/saved/<Realm>-<Character>/`: per-addon saved variables, character scope
/// (`## SavedVariablesPerCharacter`), loaded after the account file so it wins.
pub(crate) fn addon_saved_character_dir(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("saved").join(key))
}

/// `benilla-config/camera/<realm>-<character>.txt`: the third-person camera pose (the
/// `<Char>/camera-settings.txt` analog), two lines in the reference's keys and order.
pub(crate) fn camera_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    let key = format!("{}-{}", file_token(realm), file_token(character));
    home().map(|h| h.join("camera").join(format!("{key}.txt")))
}

/// `benilla-config/account`: the account name the login screen remembers
/// (`GetSavedAccountName`/`SetSavedAccountName`; the reference's is `Config.wtf`'s `accountName`).
pub(crate) fn saved_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("account"))
}

/// `benilla-config/chat/<realm>-<character>.txt`: the chat windows' tint, alpha, font size and
/// lock, the four the reference keeps in its per-character `chat-cache.txt`.
pub(crate) fn chat_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    Some(home()?.join("chat").join(format!(
        "{}-{}.txt",
        file_token(realm),
        file_token(character)
    )))
}

/// `benilla-config/layout/<realm>-<character>.txt`: the layout cache, the geometry of every
/// user-placed frame, as the reference's per-character `layout-cache.txt`; [`crate::ui_layout`]
/// owns the shape.
pub(crate) fn layout_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    Some(home()?.join("layout").join(format!(
        "{}-{}.txt",
        file_token(realm),
        file_token(character)
    )))
}

/// `benilla-config/cache/<realm>.tsv`: the player, creature and pet names the server has answered,
/// one file for the reference's `WDB/namecache.wdb`, `creaturecache.wdb` and `petnamecache.wdb`.
///
/// Realm-scoped, where the reference's is not: guids, creature entries and pet numbers are
/// realm-local, so one shared file would serve a realm another realm's names.
pub(crate) fn name_cache_path(realm: &str) -> Option<PathBuf> {
    Some(
        home()?
            .join("cache")
            .join(format!("{}.tsv", file_token(realm))),
    )
}

/// `benilla-config/Logs/`: `WoWChatLog.txt` and `WoWCombatLog.txt`, the reference's names.
pub(crate) fn logs_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Logs"))
}

/// `benilla-config/shots.txt`: the framing instrument's camera poses (`/shot`, dev builds).
pub(crate) fn shots_path() -> Option<PathBuf> {
    home().map(|h| h.join("shots.txt"))
}

/// `benilla-config/Screenshots/`: where the print-screen key writes, each image once, not through
/// [`write_atomic`].
///
/// Deviation: the reference writes `Screenshots\\` inside the install, which benilla never writes
/// to; the folder keeps the reference's name.
pub(crate) fn screenshots_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Screenshots"))
}

/// `benilla-config/Diagnostics/`: the stuck-thread sampler's profiles ([`crate::perf::stall`]) and
/// the FPS journal; `None` on a capture run like every path here, so a capture has no sampler.
pub(crate) fn diagnostics_dir() -> Option<PathBuf> {
    home().map(|h| h.join("Diagnostics"))
}

/// `benilla-config/Diagnostics/fps-journal.csv`: the FPS journal's rows while the `fpsJournal`
/// CVar is on; a capture names its own path through `WOW_FPS_JOURNAL`.
pub(crate) fn fps_journal_path() -> Option<PathBuf> {
    diagnostics_dir().map(|d| d.join("fps-journal.csv"))
}

/// A realm or character name as one path component: anything not a letter or digit becomes `_`.
/// Letters of any script stay, as in the reference's raw-name folders: vmangos accepts non-Latin
/// names (`StrictPlayerNames = 0`) and a character name is letters only, so no two share a file.
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

/// Write a state file atomically, through `<path>.tmp` and a rename, so a crash leaves the old
/// file intact.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    write_atomic_bytes(path, contents.as_bytes())
}

/// [`write_atomic`] for bytes: the saved-variables files carry Lua byte strings as they are.
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

/// Test plumbing for the persistence env vars: `set_var` is process-global, so every such test
/// takes [`ENV_LOCK`] and scopes its overrides in [`EnvGuard`]s.
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

    /// Every per-character file keys on this token; ASCII names map unchanged.
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

    /// The layout under the override, the one step of [`home`] whose answer a test can state.
    #[test]
    fn the_home_law_override_then_the_residents_and_hermetic_captures() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-ls-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        // 1 · the explicit override.
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.join(STATE_DIR).to_str().unwrap());
        assert_eq!(home(), Some(tmp.join(STATE_DIR)));
        assert_eq!(config_path(), Some(tmp.join("benilla-config/config.toml")));
        assert_eq!(
            macros_account_path(),
            Some(tmp.join("benilla-config/macros/account.txt"))
        );
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
        assert_eq!(
            camera_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/camera/Hydraxian_Waterlords-Probeone.txt"))
        );
        assert_eq!(
            chat_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/chat/Hydraxian_Waterlords-Probeone.txt"))
        );
        assert_eq!(
            layout_character_path("Hydraxian Waterlords", "Probeone"),
            Some(tmp.join("benilla-config/layout/Hydraxian_Waterlords-Probeone.txt"))
        );

        assert_eq!(
            saved_account_path(),
            Some(tmp.join("benilla-config/account"))
        );
        assert_eq!(shots_path(), Some(tmp.join("benilla-config/shots.txt")));
        assert_eq!(
            screenshots_dir(),
            Some(tmp.join("benilla-config/Screenshots"))
        );

        // 0 · a capture run resolves nothing, even with an override set.
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

    /// A missing or wrong `$WOW_DATA` does not move or lose the state folder.
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

    /// A player build resolves beside the binary; a dev build to the primary checkout, even from a
    /// linked worktree (whose `.git` is a file).
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
            // Player build: beside the binary.
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
