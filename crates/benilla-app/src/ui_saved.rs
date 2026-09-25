//! The saved-variables host side: the file, the load seam and the write. The declaration set and
//! the serializer are [`benilla_ui::script`]'s.
//!
//! The file runs after the in-game UI's own files and before `VARIABLES_LOADED`, the order of the
//! reference's `AddOn_Load` (`0x51f240`), so a saved value overrides a file-scope default before
//! any consumer reads it. It is written only at UI shutdown (`0x490bd0`, after `PLAYER_LOGOUT`):
//! the reference has no autosave and no Lua call that forces a write.
//!
//! Deviation: the write is atomic ([`crate::local_state::write_atomic_bytes`]), because the
//! reference's `.bak` rotation (`MoveFileW` without `MOVEFILE_REPLACE_EXISTING`, its result
//! discarded) protects only the first write, and nothing reads the `.bak`.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

pub(crate) struct UiSavedPlugin;

impl Plugin for UiSavedPlugin {
    fn build(&self, _app: &mut App) {
        // No systems: the write is one ordered step of `crate::ui_script::shutdown_ui_state`.
    }
}

/// Run the saved-variables file into the VM, then `host_settings`, then fire `VARIABLES_LOADED`.
/// A malformed or unreadable file warns and is held ([`UiScript::hold_saved_file`]), so the
/// shutdown write leaves it alone. Read as bytes: the writer keeps a Lua string's raw bytes.
///
/// `host_settings` seats the `RegisterForSave` globals kept in `config.toml` (the nameplate pair):
/// after the chunk, so a stale line here cannot outvote `config.toml`, and before the event, where
/// the consumers run (`UIParent_OnEvent` → `UpdateNameplates`).
pub(crate) fn load_saved_variables(
    script: &mut UiScript,
    host_settings: impl FnOnce(&mut UiScript),
) {
    // `None` in a capture or with no install: nothing is read, and the event still fires.
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
    // Fired whether or not there was a file, as a step of the reference's load sequence.
    script.fire_event("VARIABLES_LOADED", vec![]);
}

/// Write the flat file from the live globals, one step of [`crate::ui_script::shutdown_ui_state`].
/// Nothing registered means the UI never loaded, so nothing is written: that would wipe the file.
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

/// Deviation: a header saying what the file is, because the folder is in plain view; the
/// reference's file opens with a bare blank line.
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

        // A malformed file is left alone and the session runs on defaults.
        crate::local_state::write_atomic(&path, "KEPT = = 3\n").unwrap();
        let mut broken = script("1");
        load_saved_variables(&mut broken, |_| {});
        assert_eq!(broken.eval::<i64>("return KEPT").unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "KEPT = = 3\n");
        // The shutdown write leaves it alone too.
        save(&mut broken);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "KEPT = = 3\n",
            "a file that did not load is not replaced by the defaults the session ran on"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

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

    /// Even with `BENILLA_HOME` writable, a capture neither reads nor writes settings.
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
        // The load reads nothing, but the event still fires.
        let mut fresh = script("1");
        load_saved_variables(&mut fresh, |_| {});
        assert_eq!(fresh.eval::<i64>("return KEPT").unwrap(), 1);
        assert_eq!(fresh.eval::<i64>("return VL_SEEN").unwrap(), 1);
    }

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
