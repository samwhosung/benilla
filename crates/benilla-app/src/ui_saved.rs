//! The **saved-variables** host side (decision 1128) — the file, the load seam, the write
//! triggers. The engine half (the `RegisterForSave` declaration set and the serializer) is
//! [`benilla_ui::script`]'s `saved` module; this is the part that touches the disk and the clock.
//!
//! **Load** — one chunk, executed straight into the VM immediately after the in-game UI's XML has
//! loaded and before anything runs against it, then `VARIABLES_LOADED`. That ordering is the whole
//! mechanism: the reference's `AddOn_Load 0x51f240` runs the addon's own files (step 2 — where
//! the file-scope `TRAINER_FILTER_* = 1` defaults are assigned), *then* executes the saved file
//! over the top (step 4), *then* fires the load event (step 6). Defaults first, saved values
//! second, consumers third — reverse any two and the saved value can never win.
//!
//! **Write** — `OnExit(InWorld)` (a `/logout` or a disconnect) and `AppExit`, the two edges our
//! session has. The reference writes from exactly one place, the UI shutdown `0x490bd0`, reached
//! from five roots (logout to character select, quit, disconnect, application exit, `/reload`),
//! with `PLAYER_LOGOUT` fired just before; it has no autosave, no dirty bit, and no Lua binding
//! that can force a write. We keep that shape rather than debouncing like [`crate::cvars`] does:
//! these are a handful of scalars a player toggles a few times a session, and the file is written
//! whole from the live globals, so there is nothing an intermediate write would preserve.
//!
//! Divergence, disclosed (1128): the reference rotates the old file to `.bak` and then truncates
//! in place — a one-shot, not crash safety (`MoveFileW` without `MOVEFILE_REPLACE_EXISTING`, its
//! result discarded, so after the first rotation it overwrites forever, and nothing ever reads a
//! `.bak`). We write through [`crate::local_state::write_atomic`] instead, which is what that dance
//! was reaching for.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

pub(crate) struct UiSavedPlugin;

impl Plugin for UiSavedPlugin {
    fn build(&self, _app: &mut App) {
        // **The write is no longer this plugin's own edge.** It is one step of the ordered
        // shutdown tail in [`crate::ui_script::shutdown_ui_state`], because the reference has
        // exactly one shutdown (`0x490bd0`) reached from five roots and its steps are ordered
        // against each other: `PLAYER_LOGOUT` fires before any write, the flat file is written
        // before the per-addon files, and `AddOns.txt` is last. Three independent systems on the
        // same edge cannot express that.
    }
}

/// Execute the saved-variables file into the VM, let the host restore what it keeps elsewhere,
/// and fire `VARIABLES_LOADED`.
///
/// Called from the in-game UI load ([`crate::ui_script`]) at the reference's own seam — after the
/// XML, before anything consumes it. A missing file is the normal first-run case (defaults stand);
/// a malformed or unreadable one warns and is **held** ([`UiScript::hold_saved_file`]), so a hand
/// edit that fails to parse costs this session's settings and not the file. Read as **bytes**
/// (1193): the writer keeps a Lua byte string's bytes, so the file need not be UTF-8.
///
/// **`host_settings` is the third store's turn** (decision 2132). A handful of the reference's
/// `RegisterForSave` globals are settings benilla persists in `config.toml` instead — the
/// nameplate pair is the first — and those have to land in the VM too, or the stock file reads
/// nil and acts on it. The seat is exact and both edges are load-bearing: **after** the chunk, so
/// a stale line in this file cannot outvote `config.toml` (0954's store), and **before** the
/// event, because `VARIABLES_LOADED` is precisely where the consumers run
/// (`UIParent_OnEvent` → `UpdateNameplates`).
pub(crate) fn load_saved_variables(
    script: &mut UiScript,
    host_settings: impl FnOnce(&mut UiScript),
) {
    // `None` = hermetic capture, or no install — session-only state, and the event below still fires.
    if let Some(path) = crate::local_state::saved_variables_path() {
        match std::fs::read(&path) {
            Ok(bytes) => {
                if let Err(e) = script.run_chunk(&bytes) {
                    warn!(
                        "saved variables: {} did not load ({e}) — running on defaults, and \
                         leaving the file as it is",
                        path.display()
                    );
                    script.hold_saved_file(&path);
                } else {
                    info!("saved variables: loaded {}", path.display());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!("saved variables: cannot read {}: {e}", path.display());
                script.hold_saved_file(&path);
            }
        }
    }
    host_settings(script);
    // `VARIABLES_LOADED` fires whether or not there was a file: it means "the settings are now what
    // they are going to be", which is as true on a first run or a hermetic capture as after a real
    // restore — the reference fires it as a step of the load sequence, not conditionally. A window
    // that waits on it (GameTime.xml, BagFrame.xml) must not be left uninitialized in a capture.
    script.fire_event("VARIABLES_LOADED", vec![]);
}

/// Write the flat file from the live globals — step 3 of the shutdown tail
/// ([`crate::ui_script::shutdown_ui_state`]). No-op when nothing is registered — the UI never loaded
/// (a glue-only run, a capture), and an empty write would be a wipe rather than a save. The
/// reference *does* delete the file when an addon declares nothing, which is a different case:
/// there, the addon loaded and its declaration list is genuinely empty.
pub(crate) fn save(script: &mut UiScript) {
    let names = script.saved_variable_names();
    if names.is_empty() {
        return;
    }
    let Some(path) = crate::local_state::saved_variables_path() else {
        return;
    };
    if script.saved_file_held(&path) {
        warn!(
            "saved variables: {} did not load this session — left as it is",
            path.display()
        );
        return;
    }
    let mut body = HEADER.as_bytes().to_vec();
    body.extend(script.saved_variables_bytes());
    for w in script.take_warnings() {
        warn!("saved variables: {w}");
    }
    match crate::local_state::write_atomic_bytes(&path, &body) {
        Ok(()) => info!(
            "saved variables: wrote {} ({} names)",
            path.display(),
            names.len()
        ),
        Err(e) => warn!("saved variables: cannot write {}: {e}", path.display()),
    }
}

/// The file's own header. The reference writes no header at all (its files open with a bare blank
/// line); ours says what the file is, because a visible folder invites a look.
const HEADER: &str = "\
-- benilla saved variables (decision 1128) — the UI's own remembered settings.
-- Written at logout/exit from the live values; executed as a Lua chunk at UI load.
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    /// A VM with one declared global and a `VARIABLES_LOADED` witness.
    fn script(value: &str) -> UiScript {
        let s = UiScript::new().unwrap();
        s.run(&format!(
            "KEPT = {value} RegisterForSave(\"KEPT\") \
             VL_SEEN = 0 \
             local f = CreateFrame(\"Frame\") \
             f:RegisterEvent(\"VARIABLES_LOADED\") \
             f:SetScript(\"OnEvent\", function() VL_SEEN = VL_SEEN + 1 end)"
        ))
        .unwrap();
        s
    }

    /// The disk half, end to end: the file lands in the folder, carries its header, and a *different*
    /// VM's value is replaced by the saved one — then `VARIABLES_LOADED` fires, once, after it.
    #[test]
    fn the_file_round_trips_through_the_folder_and_then_fires_variables_loaded() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-sv-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut s = script("7");
        save(&mut s);
        let path = tmp.join("saved-variables.lua");
        let text = std::fs::read_to_string(&path).expect("the file was written");
        assert!(text.starts_with("-- benilla saved variables"), "{text}");
        assert!(text.contains("KEPT = 7"), "{text}");

        // The restart: this VM's own default is 1, the file says 7, and the file wins.
        let mut fresh = script("1");
        load_saved_variables(&mut fresh, |_| {});
        assert_eq!(fresh.eval::<i64>("return KEPT").unwrap(), 7);
        assert_eq!(
            fresh.eval::<i64>("return VL_SEEN").unwrap(),
            1,
            "VARIABLES_LOADED fires exactly once, after the chunk"
        );

        // A malformed file is left alone, and this session simply runs on defaults.
        crate::local_state::write_atomic(&path, "KEPT = = 3\n").unwrap();
        let mut broken = script("1");
        load_saved_variables(&mut broken, |_| {});
        assert_eq!(broken.eval::<i64>("return KEPT").unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEPT = = 3\n");
        // …and the shutdown write leaves it alone too. This is the half that used to fail: the
        // load kept its promise, and the save then wrote this session's default over the file.
        save(&mut broken);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "KEPT = = 3\n",
            "a file that did not load is not replaced by the defaults the session ran on"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// **The bytes survive the folder.** A value that is not UTF-8 is written raw and read back
    /// raw — the load reads bytes, so the file it now writes is one it can always read (a
    /// `read_to_string` here would call it unreadable and the next save would replace it).
    #[test]
    fn a_byte_string_survives_the_file() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svbytes-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut s = script("string.char(65, 233, 255)");
        save(&mut s);
        let mut fresh = script("'x'");
        load_saved_variables(&mut fresh, |_| {});
        assert!(fresh
            .eval::<bool>(
                "return string.len(KEPT) == 3 and string.byte(KEPT, 2) == 233 \
                 and string.byte(KEPT, 3) == 255"
            )
            .unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Hermetic: a capture run neither reads a machine's settings nor writes any back, even with
    /// `BENILLA_HOME` pointing somewhere writable (0954's law, and the reason captures are stable).
    #[test]
    fn a_capture_run_neither_reads_nor_writes() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svcap-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let _c = EnvGuard::set("WOW_CAPTURE", "ui-saved");

        let mut s = script("7");
        save(&mut s);
        assert!(!tmp.exists(), "a capture must not plant a settings file");
        // The load is a no-op too — but the event still fires, so a window waiting on it is not stuck.
        let mut fresh = script("1");
        load_saved_variables(&mut fresh, |_| {});
        assert_eq!(fresh.eval::<i64>("return KEPT").unwrap(), 1);
        assert_eq!(fresh.eval::<i64>("return VL_SEEN").unwrap(), 1);
    }

    /// Nothing registered = the UI never loaded: writing would be a wipe, so it is skipped.
    #[test]
    fn an_empty_declaration_set_writes_nothing() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svempty-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());

        let mut bare = UiScript::new().unwrap();
        save(&mut bare);
        assert!(!tmp.exists());
    }
}
