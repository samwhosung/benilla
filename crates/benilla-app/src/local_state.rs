//! Where benilla keeps local state: one folder, `benilla-config/`, never inside the install, and
//! this module is the only place that computes a path into it; each path fn's doc says what it
//! holds. The 1.12 client is portable the same way, its `WTF/` and `Cache/` in its own folder.
//!
//! Resolution, in order, the same shape as [`benilla_formats::wow_data`]:
//! 1. `$BENILLA_HOME`.
//! 2. An unattended run ([`crate::run_mode::unattended`]): a fresh folder of its own for the run,
//!    under the system temp folder, so a probe neither reads the player's settings nor writes back
//!    over them.
//! 3. `<project folder>/benilla-config/`, dev builds only ([`benilla_formats::project_folder`], the
//!    launcher's): a shipped binary must not carry the build machine's source tree. A linked
//!    worktree resolves to its primary checkout's folder, unless it declares its own probe account
//!    ([`shared_root`]).
//! 4. `<exe dir>/benilla-config/`.
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
    home_in(cfg!(test))
}

/// [`home`], with the unit-test stop as an argument so a test can walk the steps behind it.
fn home_in(unit_test: bool) -> Option<PathBuf> {
    if std::env::var_os("WOW_CAPTURE").is_some() {
        return None; // hermetic: captures neither read nor write player state
    }
    if let Some(over) = std::env::var_os("BENILLA_HOME") {
        return Some(PathBuf::from(over));
    }
    if unit_test {
        return None; // a unit test reaches no real state folder unless it pins `$BENILLA_HOME`
    }
    if crate::run_mode::unattended() {
        return Some(run_home().to_path_buf());
    }
    resident_home()
}

/// Step 2 of [`home`]: the unattended run's own folder, one per process, created on first write.
/// Left behind for whoever reads what the run wrote; the system cleans its temp folder.
pub(crate) fn run_home() -> &'static Path {
    static RUN_HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    RUN_HOME.get_or_init(|| run_home_in(&std::env::temp_dir(), std::process::id()))
}

/// [`run_home`]'s path under `temp` for process `pid`.
fn run_home_in(temp: &Path, pid: u32) -> PathBuf {
    temp.join(format!("benilla-run-{pid}")).join(STATE_DIR)
}

/// Steps 3 and 4 of [`home`]: the folder the build itself lives in.
fn resident_home() -> Option<PathBuf> {
    // 3 · the project folder, dev builds only.
    if let Some(root) = dev_project_root() {
        return Some(root.join(STATE_DIR));
    }
    // 4 · beside the binary; a dev build never reaches here.
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .map(|dir| dir.join(STATE_DIR))
}

/// The project folder a dev build keeps its state in ([`benilla_formats::project_folder`]), through
/// [`shared_root`].
fn dev_project_root() -> Option<PathBuf> {
    Some(shared_root(benilla_formats::project_folder()?))
}

/// `here`, or for a linked worktree its primary checkout, whichever worktree built the binary, so
/// a developer's worktrees share one settings folder. A worktree that declares an account of its
/// own in `.probe-identity` is a probe's checkout and keeps its own folder, so no run from it,
/// attended or not, reads or writes the player's settings. Anything unexpected keeps `here`.
fn shared_root(here: &Path) -> PathBuf {
    let dot_git = here.join(".git");
    // A linked worktree: `.git` is a file pointing at `<primary>/.git/worktrees/<name>`.
    if dot_git.is_file() && !here.join(".probe-identity").exists() {
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
                    return primary.to_path_buf();
                }
            }
        }
    }
    here.to_path_buf()
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
    character_path(CharacterFile::Macros, realm, character)
}

/// `benilla-config/bindings/account.txt`: the account-wide key bindings (the `bindings-cache.wtf`
/// analog), a diff against the defaults.
pub(crate) fn bindings_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("bindings/account.txt"))
}

/// `benilla-config/bindings/<realm>-<character>.txt`: the character-specific binding set. Its
/// existence is the "character specific key bindings" state; going back to general deletes it.
pub(crate) fn bindings_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    character_path(CharacterFile::Bindings, realm, character)
}

/// `benilla-config/saved-variables.lua`: the flat channel our FrameXML saves through
/// `RegisterForSave` (the `SavedVariables.lua` analog), written whole at logout, read at UI load.
pub(crate) fn saved_variables_path() -> Option<PathBuf> {
    home().map(|h| h.join("saved-variables.lua"))
}

/// The folder of the AddOn enable files. Not `addons`, which names the AddOns root (`AddOns`) on a
/// case-insensitive filesystem, so the two shared one folder there.
const ADDON_STATE_DIR: &str = "addon-state";

/// `benilla-config/addon-state/<Realm>-<Character>.txt`: the AddOn enable state, per character in
/// the reference's `AddOns.txt` format (`<AddOnName>: enabled|disabled`), which it writes at the
/// tail of its UI shutdown (`0x490bd0`). An addon absent from the file is enabled. The files a
/// build before this one kept in `benilla-config/addons/` are moved here the first time a path is
/// asked for ([`migrate_addon_state`]).
pub(crate) fn addons_state_path(realm: &str, character: &str) -> Option<PathBuf> {
    migrate_addon_state(&home()?);
    character_path(CharacterFile::AddonState, realm, character)
}

/// Move the enable files an earlier build wrote to `<home>/addons/` into [`ADDON_STATE_DIR`], once
/// per process and home. A file qualifies when it is named `<Realm>-<Character>.txt`, holds at
/// least one row and every non-blank line of it reads `<AddOn>: enabled` or `<AddOn>: disabled`,
/// so a loose `.txt` of the player's in a folder that is also the AddOns root (case-insensitive
/// filesystems) stays where it is, and so does a file whose new name is already taken. An old
/// folder left empty goes, unless it is the AddOns root.
fn migrate_addon_state(home: &Path) {
    static DONE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    {
        let mut done = DONE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if done.iter().any(|h| h == home) {
            return;
        }
        done.push(home.to_path_buf());
    }
    let old = home.join("addons");
    let Ok(entries) = std::fs::read_dir(&old) else {
        return;
    };
    let new = home.join(ADDON_STATE_DIR);
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_state_name = path.is_file()
            && name.strip_suffix(".txt").is_some_and(|stem| {
                stem.matches('-').count() == 1 && !stem.starts_with('-') && !stem.ends_with('-')
            });
        if !is_state_name {
            continue;
        }
        // At least one row, and nothing but rows: an empty file, or a player's note, stays.
        let looks_like_state = std::fs::read(&path).is_ok_and(|bytes| {
            let text = benilla_ui::source::decode(&bytes);
            let row = |line: &str| {
                line.split_once(':').is_some_and(|(addon, state)| {
                    !addon.trim().is_empty()
                        && matches!(
                            state.trim().to_ascii_lowercase().as_str(),
                            "enabled" | "disabled"
                        )
                })
            };
            text.lines().any(row) && text.lines().all(|l| l.trim().is_empty() || row(l))
        });
        let target = new.join(&name);
        if !looks_like_state || target.exists() {
            continue;
        }
        if std::fs::create_dir_all(&new).is_ok() && std::fs::rename(&path, &target).is_ok() {
            bevy::log::info!("addons: moved {} to {}", path.display(), target.display());
        }
    }
    // An emptied folder goes, unless it is the AddOns root under another case, which the player
    // drops addons into: that is told by the two being one folder, not by `AddOns` existing, as
    // on a case-sensitive filesystem both can be there.
    remove_emptied(&old, &home.join("AddOns"));
}

/// Remove `old` if it is empty and not the same folder as `root`.
fn remove_emptied(old: &Path, root: &Path) {
    let same = match (std::fs::canonicalize(old), std::fs::canonicalize(root)) {
        (Ok(a), Ok(b)) => a == b,
        (_, Err(_)) => false,
        (Err(_), _) => true,
    };
    if !same {
        let _ = std::fs::remove_dir(old);
    }
}

/// `benilla-config/saved/`: per-addon saved variables, account scope, one `<Addon>.lua` per addon
/// declaring `## SavedVariables` (the reference's `WTF/Account/<ACC>/SavedVariables/<Addon>.lua`).
pub(crate) fn addon_saved_account_dir() -> Option<PathBuf> {
    home().map(|h| h.join("saved"))
}

/// `benilla-config/saved/<Realm>-<Character>/`: per-addon saved variables, character scope
/// (`## SavedVariablesPerCharacter`), loaded after the account file so it wins.
pub(crate) fn addon_saved_character_dir(realm: &str, character: &str) -> Option<PathBuf> {
    character_path(CharacterFile::SavedVariables, realm, character)
}

/// `benilla-config/camera/<realm>-<character>.txt`: the third-person camera pose (the
/// `<Char>/camera-settings.txt` analog), two lines in the reference's keys and order.
pub(crate) fn camera_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    character_path(CharacterFile::Camera, realm, character)
}

/// `benilla-config/account`, where earlier builds kept the account name the login screen
/// remembers. It is now the `accountName` CVar in `config.toml`, as the reference's is in
/// `Config.wtf`: [`crate::cvars`] reads this file once into it and deletes it after the save.
pub(crate) fn legacy_account_path() -> Option<PathBuf> {
    home().map(|h| h.join("account"))
}

/// `benilla-config/chat/<realm>-<character>.txt`: the chat windows' tint, alpha, font size and
/// lock, the four the reference keeps in its per-character `chat-cache.txt`.
pub(crate) fn chat_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    character_path(CharacterFile::Chat, realm, character)
}

/// `benilla-config/layout/<realm>-<character>.txt`: the layout cache, the geometry of every
/// user-placed frame, as the reference's per-character `layout-cache.txt`; [`crate::ui_layout`]
/// owns the shape.
pub(crate) fn layout_character_path(realm: &str, character: &str) -> Option<PathBuf> {
    character_path(CharacterFile::Layout, realm, character)
}

/// The state benilla keeps per character, the analog of the reference's per-character folder
/// `WTF\Account\<account>\<realm>\<character>\`, each a file or folder named
/// `<realm>-<character>` in a folder of its kind. Every per-character path goes through
/// [`character_path`], so a rename ([`move_character_state`]) moves all of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharacterFile {
    /// [`addons_state_path`].
    AddonState,
    /// [`macros_character_path`].
    Macros,
    /// [`bindings_character_path`].
    Bindings,
    /// [`addon_saved_character_dir`], a folder.
    SavedVariables,
    /// [`camera_character_path`].
    Camera,
    /// [`chat_character_path`].
    Chat,
    /// [`layout_character_path`].
    Layout,
}

impl CharacterFile {
    pub(crate) const ALL: [CharacterFile; 7] = [
        CharacterFile::AddonState,
        CharacterFile::Macros,
        CharacterFile::Bindings,
        CharacterFile::SavedVariables,
        CharacterFile::Camera,
        CharacterFile::Chat,
        CharacterFile::Layout,
    ];

    /// The folder under the home, and the suffix of the key (none for a folder).
    fn place(self) -> (&'static str, &'static str) {
        match self {
            CharacterFile::AddonState => (ADDON_STATE_DIR, ".txt"),
            CharacterFile::Macros => ("macros", ".txt"),
            CharacterFile::Bindings => ("bindings", ".txt"),
            CharacterFile::SavedVariables => ("saved", ""),
            CharacterFile::Camera => ("camera", ".txt"),
            CharacterFile::Chat => ("chat", ".txt"),
            CharacterFile::Layout => ("layout", ".txt"),
        }
    }
}

/// `benilla-config/<kind>/<realm>-<character>[.txt]`, the one place a per-character path is made.
fn character_path(file: CharacterFile, realm: &str, character: &str) -> Option<PathBuf> {
    let (dir, suffix) = file.place();
    let key = format!("{}-{}{suffix}", file_token(realm), file_token(character));
    Some(home()?.join(dir).join(key))
}

/// A rename's move of the character's state, the reference's `MoveFileW` of its folder from the
/// old name to the new (`0x4725f0`: `0x47269a` for `WTF\Account\<account>\<realm>\<name>`,
/// `0x4726f0` for the older `WTF\Account\<account>\<name>`). The reference's folder moves
/// whole or not at all: a move onto a name that exists fails and leaves everything under the old
/// name. So when the new name has any per-character file or folder, nothing moves.
pub(crate) fn move_character_state(realm: &str, old: &str, new: &str) {
    let pairs: Option<Vec<(PathBuf, PathBuf)>> = CharacterFile::ALL
        .iter()
        .map(|&file| {
            Some((
                character_path(file, realm, old)?,
                character_path(file, realm, new)?,
            ))
        })
        .collect();
    let Some(pairs) = pairs else {
        return;
    };
    if pairs.iter().any(|(from, to)| from == to) {
        return;
    }
    if let Some((_, taken)) = pairs.iter().find(|(_, to)| to.exists()) {
        bevy::log::warn!(
            "rename: {} exists, so {old}'s files stay under that name",
            taken.display()
        );
        return;
    }
    for (from, to) in pairs.into_iter().filter(|(from, _)| from.exists()) {
        match std::fs::rename(&from, &to) {
            Ok(()) => bevy::log::info!("rename: moved {} to {}", from.display(), to.display()),
            Err(e) => bevy::log::warn!("rename: cannot move {}: {e}", from.display()),
        }
    }
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

    /// A rename moves every per-character file and folder of the realm's character to the new
    /// name, and none of another realm's or character's; when the new name has any of its own,
    /// nothing moves.
    #[test]
    fn a_rename_moves_every_per_character_file() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-ls-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let paths = |realm: &str, character: &str| {
            [
                addons_state_path(realm, character),
                macros_character_path(realm, character),
                bindings_character_path(realm, character),
                addon_saved_character_dir(realm, character),
                camera_character_path(realm, character),
                chat_character_path(realm, character),
                layout_character_path(realm, character),
            ]
            .map(Option::unwrap)
        };
        // The public path functions are the enum's, kind for kind.
        assert_eq!(
            paths("Realm", "Old").to_vec(),
            CharacterFile::ALL
                .iter()
                .map(|&f| character_path(f, "Realm", "Old").unwrap())
                .collect::<Vec<_>>()
        );
        let write = |p: &Path, what: &str| {
            if p.extension().is_some() {
                write_atomic(p, what).unwrap();
            } else {
                write_atomic(&p.join("Addon.lua"), what).unwrap();
            }
        };
        let read = |p: &Path| {
            let p = if p.extension().is_some() {
                p.to_path_buf()
            } else {
                p.join("Addon.lua")
            };
            std::fs::read_to_string(p).ok()
        };
        for p in paths("Realm", "Old") {
            write(&p, "old");
        }
        for p in paths("Other Realm", "Old") {
            write(&p, "other realm");
        }
        move_character_state("Realm", "Old", "New");
        let (old, new) = (paths("Realm", "Old"), paths("Realm", "New"));
        for (o, n) in old.iter().zip(&new) {
            assert_eq!(read(n).as_deref(), Some("old"), "{}", n.display());
            assert!(!o.exists(), "{}", o.display());
        }
        for p in paths("Other Realm", "Old") {
            assert_eq!(read(&p).as_deref(), Some("other realm"));
        }

        // The new name already has a layout: nothing moves, as the reference's one folder move
        // fails whole onto a name that exists.
        for p in paths("Realm", "Two") {
            write(&p, "two");
        }
        write(&layout_character_path("Realm", "Three").unwrap(), "taken");
        move_character_state("Realm", "Two", "Three");
        for p in paths("Realm", "Two") {
            assert_eq!(read(&p).as_deref(), Some("two"), "{}", p.display());
        }
        assert_eq!(
            read(&layout_character_path("Realm", "Three").unwrap()).as_deref(),
            Some("taken")
        );
        assert!(!macros_character_path("Realm", "Three").unwrap().exists());
        let _ = std::fs::remove_dir_all(&tmp);
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
            legacy_account_path(),
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
            legacy_account_path(),
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

    /// An unattended run keeps its state in a folder of its own, never the player's, unless
    /// `$BENILLA_HOME` pins one; a capture still resolves none.
    #[test]
    fn an_unattended_run_keeps_its_own_state_folder() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _r = EnvGuard::unset("WOW_RIG");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let _u = EnvGuard::set("WOW_UNATTENDED", "1");
        let run = home_in(false).expect("an unattended run has a state folder");
        assert_eq!(run, run_home());
        assert_ne!(
            Some(&run),
            resident_home().as_ref(),
            "never the player's folder"
        );
        assert_eq!(
            home_in(false),
            Some(run.clone()),
            "one folder for the whole run"
        );
        assert_eq!(
            run_home_in(Path::new("/tmp"), 42),
            Path::new("/tmp/benilla-run-42/benilla-config")
        );

        // A rig is unattended too.
        let _u2 = EnvGuard::unset("WOW_UNATTENDED");
        let _r2 = EnvGuard::set("WOW_RIG", "tauren druid 60");
        assert_eq!(home_in(false), Some(run));

        // The override still wins, and a capture still resolves nothing.
        let _h2 = EnvGuard::set("BENILLA_HOME", "/tmp/benilla-pinned");
        assert_eq!(home_in(false), Some(PathBuf::from("/tmp/benilla-pinned")));
        let _c2 = EnvGuard::set("WOW_CAPTURE", "ui-options");
        assert_eq!(home_in(false), None);
    }

    /// A unit test that does not pin `$BENILLA_HOME` resolves no state folder, so no test can write
    /// the real one.
    #[test]
    fn a_test_without_an_override_has_no_state_folder() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        assert_eq!(home(), None);
        assert_eq!(config_path(), None);
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
        let h =
            resident_home().expect("a broken install path must not cost the player their config");
        assert!(
            !h.starts_with("/nonexistent"),
            "home() still reads $WOW_DATA: {}",
            h.display()
        );
        assert!(h.ends_with(STATE_DIR), "{}", h.display());
    }

    /// A player build resolves beside the binary; a dev build to the primary checkout, even from a
    /// linked worktree (whose `.git` is a file), unless that worktree declares a probe account.
    #[test]
    fn the_state_folder_lands_where_the_build_says() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::unset("BENILLA_HOME");
        let h = resident_home().expect("the resident folder always resolves");
        assert!(h.ends_with(STATE_DIR), "{}", h.display());

        let Some(here) = benilla_formats::project_folder() else {
            // Player build: beside the binary.
            let exe_dir = std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf();
            assert_eq!(h, exe_dir.join(STATE_DIR));
            return;
        };

        if here.join(".git").is_file() && !here.join(".probe-identity").exists() {
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

    /// A project folder that is a linked worktree keeps its state in the primary checkout, so every
    /// worktree shares one, unless it declares a probe account; any other folder, a crate on top of
    /// benilla's included, keeps its own.
    #[test]
    fn the_state_root_is_the_folder_or_its_primary_checkout() {
        let tmp = std::env::temp_dir().join(format!("benilla-root-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let primary = tmp.join("benilla");
        std::fs::create_dir_all(primary.join(".git/worktrees/pool-3")).unwrap();
        let slot = tmp.join("pool-3");
        std::fs::create_dir_all(&slot).unwrap();
        let gitdir = primary.join(".git/worktrees/pool-3");
        std::fs::write(slot.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(shared_root(&slot), primary);
        assert_eq!(shared_root(&primary), primary);
        // A worktree that logs in as its own account never shares the player's settings.
        std::fs::write(
            slot.join(".probe-identity"),
            "WOW_USER=probe3\nWOW_PASS=unused\nWOW_CHAR=Probethree\n",
        )
        .unwrap();
        assert_eq!(shared_root(&slot), slot);
        std::fs::write(primary.join(".probe-identity"), "WOW_USER=probe0\n").unwrap();
        assert_eq!(
            shared_root(&primary),
            primary,
            "the primary keeps its own either way"
        );
        let hello_mod = tmp.join("hello-mod");
        std::fs::create_dir_all(hello_mod.join(".git")).unwrap();
        assert_eq!(shared_root(&hello_mod), hello_mod);
        let plain = tmp.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(shared_root(&plain), plain);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A launcher's recorded folder is where a dev build keeps its state and reads its probe
    /// identity; a player build resolves neither from it. Recording is process-wide, so the parent
    /// runs this same test in a child process of the test binary, which records and resolves.
    #[test]
    fn a_recorded_launcher_folder_holds_the_state_and_the_identity() {
        const CHILD: &str = "BENILLA_TEST_LAUNCHER_FOLDER";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            benilla_formats::set_project_folder(dir.to_str().unwrap());
            if crate::run_mode::dev_affordances() {
                assert_eq!(resident_home(), Some(dir.join(STATE_DIR)));
                assert_eq!(
                    crate::run_mode::declared_identity(),
                    Some(crate::run_mode::DeclaredIdentity {
                        user: "probe9".into(),
                        character: "Modchar".into()
                    })
                );
            } else {
                let exe_dir = std::env::current_exe().unwrap();
                assert_eq!(
                    resident_home(),
                    Some(exe_dir.parent().unwrap().join(STATE_DIR))
                );
                assert_eq!(crate::run_mode::declared_identity(), None);
            }
            return;
        }
        let tmp = std::env::temp_dir().join(format!("benilla-launcher-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join(".probe-identity"),
            "WOW_USER=probe9\nWOW_PASS=unused\nWOW_CHAR=Modchar\n",
        )
        .unwrap();
        let name =
            "local_state::tests::a_recorded_launcher_folder_holds_the_state_and_the_identity";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env(CHILD, &tmp)
            .env_remove("BENILLA_HOME")
            .env_remove("WOW_CAPTURE")
            .output()
            .unwrap();
        let report = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success() && report.contains("1 passed"),
            "the child run:\n{report}"
        );
        std::fs::remove_dir_all(&tmp).ok();
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

    /// The enable files live where no AddOns root can be: the old `addons/` folder is the AddOns
    /// root under another case on a case-insensitive filesystem. A file of the earlier layout is
    /// moved once; a loose `.txt` of the player's that is no enable state stays, and so does a
    /// file whose new name is taken.
    #[test]
    fn the_enable_files_leave_the_addon_folder_and_old_ones_move_once() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-enable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        let old = home.join("addons");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("Realm-One.txt"), "A: disabled\r\nB: enabled\n").unwrap();
        std::fs::write(old.join("Realm-Two.txt"), "A: disabled\n").unwrap();
        std::fs::write(old.join("Notes-Todo.txt"), "buy milk\n").unwrap();
        std::fs::write(old.join("README.txt"), "A: enabled\n").unwrap();
        std::fs::write(old.join("Realm-Empty.txt"), "").unwrap();
        std::fs::write(old.join("Realm-Blank.txt"), "\n  \n").unwrap();
        let _g = EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

        let path = addons_state_path("Realm", "One").unwrap();
        assert_ne!(
            path.parent()
                .unwrap()
                .to_str()
                .unwrap()
                .to_ascii_lowercase(),
            old.to_str().unwrap().to_ascii_lowercase(),
            "no case spelling of the AddOns root"
        );
        assert_ne!(
            path.parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_ascii_lowercase(),
            "addons"
        );
        let new = path.parent().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "A: disabled\r\nB: enabled\n",
            "moved whole"
        );
        assert!(new.join("Realm-Two.txt").is_file());
        assert!(!old.join("Realm-One.txt").exists());
        assert!(
            old.join("Notes-Todo.txt").is_file() && old.join("README.txt").is_file(),
            "a file that is no enable state stays"
        );
        assert!(
            old.join("Realm-Empty.txt").is_file() && old.join("Realm-Blank.txt").is_file(),
            "a file with no row stays"
        );
        // Once: a file written to the old place afterwards is not taken, and the new is not
        // overwritten.
        std::fs::write(old.join("Realm-Three.txt"), "A: enabled\n").unwrap();
        let _ = addons_state_path("Realm", "Three");
        assert!(old.join("Realm-Three.txt").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// With nothing but enable files in an `addons/` that is not the AddOns root, the emptied
    /// folder goes; with `AddOns/` beside it (different folders on a case-sensitive filesystem) it
    /// goes too, and the root stays whatever its case.
    #[test]
    fn an_emptied_old_folder_goes_unless_it_is_the_addons_root() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-enable-rm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        let (old, root) = (home.join("addons"), home.join("AddOns"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        // One folder under two spellings, or two folders.
        let one_folder =
            std::fs::canonicalize(&old).unwrap() == std::fs::canonicalize(&root).unwrap();
        std::fs::write(old.join("Realm-One.txt"), "A: disabled\n").unwrap();
        let _g = EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());
        let path = addons_state_path("Realm", "One").unwrap();
        assert!(path.is_file());
        assert!(root.is_dir(), "the AddOns root is never removed");
        assert_eq!(old.exists(), one_folder, "an emptied old folder goes");

        // Two folders by any spelling: the emptied one goes, the other stays.
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        remove_emptied(&a, &b);
        assert!(!a.exists() && b.is_dir());
        // The same folder, or one with something in it, stays.
        remove_emptied(&b, &b);
        assert!(b.is_dir());
        std::fs::create_dir_all(tmp.join("c")).unwrap();
        std::fs::write(tmp.join("c/keep.txt"), "x").unwrap();
        remove_emptied(&tmp.join("c"), &b);
        assert!(tmp.join("c/keep.txt").is_file());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
