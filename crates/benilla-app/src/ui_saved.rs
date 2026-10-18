//! The saved-variables host side: the file, the load seam and the write. The declaration set and
//! the serializer are [`benilla_ui::script`]'s.
//!
//! The file is read after the in-game UI's own files and every addon's `ADDON_LOADED`, and before
//! `VARIABLES_LOADED` (`0x4900a3`, `0x4900b2`), so a saved value overrides a file-scope default
//! before any consumer reads it. It is written only at UI shutdown (`0x490bd0`, after
//! `PLAYER_LOGOUT`): the reference has no autosave and no Lua call that forces a write.
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

/// Read the saved-variables file into the globals ([`UiScript::load_flat_saved_variables`]), then
/// `host_settings`, then fire `VARIABLES_LOADED`. An unreadable file warns and is held
/// ([`UiScript::hold_saved_file`]), so the shutdown write leaves it alone.
///
/// `host_settings` seats what the host carries into a `RegisterForSave` global (a plate setting
/// left in `config.toml`): after the file, so it outranks it, and before the event, where the
/// consumers run (`UIParent_OnEvent` → `UpdateNameplates`).
pub(crate) fn load_saved_variables(
    script: &mut UiScript,
    host_settings: impl FnOnce(&mut UiScript),
) {
    // `None` in a capture or with no install: nothing is read, and the event still fires.
    if let Some(path) = crate::local_state::saved_variables_path() {
        match std::fs::read(&path) {
            Ok(bytes) => {
                let lost = script.load_flat_saved_variables(&bytes);
                info!("saved variables: loaded {}", path.display());
                if lost > 0 && contains(&bytes, CHUNK_ERA_HEADER_LINE.as_bytes()) {
                    keep_chunk_era_copy(script, &path, &bytes, lost);
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

/// The header line of a file an earlier benilla wrote and ran as a Lua chunk, which can hold
/// lines the line reader drops (a table, a boolean).
const CHUNK_ERA_HEADER_LINE: &str =
    "-- Written at logout/exit from the live values; executed as a Lua chunk at UI load.";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Copy a chunk-era file whose lines the reader dropped to `saved-variables.lua.bak` before the
/// shutdown write replaces it, once: an existing copy is never overwritten. The copy is written
/// whole beside it and then renamed into place, so a crash never leaves a partial one; if it cannot
/// be written the file is held, so the write leaves it as it is.
fn keep_chunk_era_copy(script: &UiScript, path: &std::path::Path, bytes: &[u8], lost: usize) {
    use std::io::Write as _;
    let bak = path.with_extension("lua.bak");
    if bak.exists() {
        return;
    }
    let tmp = path.with_extension("lua.bak.tmp");
    let copy = std::fs::File::create(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, &bak));
    let _ = std::fs::remove_file(&tmp);
    match copy {
        Ok(()) => warn!(
            "saved variables: {} has {lost} line(s) this build no longer reads; the file as it \
             was is kept at {}",
            path.display(),
            bak.display()
        ),
        Err(e) => {
            warn!(
                "saved variables: cannot keep a copy at {}: {e} — leaving {} as it is",
                bak.display(),
                path.display()
            );
            script.hold_saved_file(path);
        }
    }
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
        Ok(()) => {
            info!(
                "saved variables: wrote {} ({} names)",
                path.display(),
                names.len()
            );
            // A written saved-variables file empties the loose-file index (`0x491865`).
            benilla_formats::install_index().clear();
        }
        Err(e) => warn!("saved variables: cannot write {}: {e}", path.display()),
    }
}

/// Deviation: a header saying what the file is, because the folder is in plain view; the
/// reference's file opens with a bare blank line. Its lines are comments to the reader.
const HEADER: &str = "\
-- benilla saved variables: the UI's own remembered settings.
-- Written at logout/exit from the live values; read back at UI load, line by line.
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
            "VARIABLES_LOADED fires exactly once, after the file is read"
        );

        // A line the reader ignores leaves the default, and the shutdown writes the file anew.
        crate::local_state::write_atomic(&path, "KEPT = = 3\n").unwrap();
        let mut odd = script("1");
        load_saved_variables(&mut odd, |_| {});
        assert_eq!(odd.eval::<i64>("return KEPT").unwrap(), 1);
        save(&mut odd);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("KEPT = 1\n"), "{text}");
        assert!(
            !path.with_extension("lua.bak").exists(),
            "no copy for a current file"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// The header line an earlier benilla wrote, when it ran the file as a chunk.
    const CHUNK_ERA: &str = "-- benilla saved variables: the UI's own remembered settings.\n\
        -- Written at logout/exit from the live values; executed as a Lua chunk at UI load.\n";

    /// A chunk-era file of plain lines reads the same values and needs no copy; one with a line the
    /// reader drops is copied once to `.bak`, and a second load keeps that first copy.
    #[test]
    fn a_chunk_era_file_with_dropped_lines_is_copied_once() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-svera-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let path = tmp.join("saved-variables.lua");
        let bak = tmp.join("saved-variables.lua.bak");

        let plain = format!("{CHUNK_ERA}KEPT = 7\nNONE = nil\nLABEL = \"a\\\"b\"\n");
        crate::local_state::write_atomic(&path, &plain).unwrap();
        let mut s = script("1");
        load_saved_variables(&mut s, |_| {});
        assert_eq!(s.eval::<i64>("return KEPT").unwrap(), 7);
        assert_eq!(s.eval::<String>("return LABEL").unwrap(), "a\"b");
        assert!(!bak.exists(), "every line read: no copy");

        let tabled = format!("{CHUNK_ERA}KEPT = 7\nLIST = {{\n\t[1] = 1,\n}}\n");
        crate::local_state::write_atomic(&path, &tabled).unwrap();
        let mut s = script("1");
        load_saved_variables(&mut s, |_| {});
        assert_eq!(s.eval::<i64>("return KEPT").unwrap(), 7);
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            tabled,
            "the file as it was"
        );

        let again = format!("{CHUNK_ERA}LIST = {{}}\n");
        crate::local_state::write_atomic(&path, &again).unwrap();
        let mut s = script("1");
        load_saved_variables(&mut s, |_| {});
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            tabled,
            "the first copy stays"
        );

        // A current file with a dropped line makes no copy.
        std::fs::remove_file(&bak).unwrap();
        crate::local_state::write_atomic(&path, &format!("{HEADER}LIST = {{}}\n")).unwrap();
        let mut s = script("1");
        load_saved_variables(&mut s, |_| {});
        assert!(!bak.exists());
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
