//! The eight sections' files (`0x85dcf0`): where each lives here, the bytes it is on the wire, and
//! how a file from the server is stored. A section's bytes are the reference's file: sections 4 to
//! 7 are benilla's own files, which keep the reference's grammar; 0 and 1 are kept as they come;
//! the key bindings keep the reference's `bindings-cache.wtf` beside the diff benilla loads a set
//! from ([`crate::bindings`]). A file from the server is kept as it came, so its digest is the
//! server's at the next login.

use std::path::PathBuf;

use benilla_protocol::messages::{AccountDataDigest, ACCOUNT_DATA_SECTIONS};
use benilla_ui::script::keybind::KeyBinding;
use md5::{Digest as _, Md5};

use crate::bindings::store;
use crate::local_state;

/// Each record's scope byte (`+0`): account-wide, else per character.
pub(crate) const ACCOUNT_WIDE: [bool; ACCOUNT_DATA_SECTIONS] =
    [true, false, true, false, true, false, false, false];

/// The account-wide and per-character `bindings-cache.wtf`.
pub(crate) const BINDINGS: [usize; 2] = [2, 3];
/// The account-wide and per-character `macros-cache.txt`.
pub(crate) const MACROS: [usize; 2] = [4, 5];
/// The per-character `layout-cache.txt`.
pub(crate) const LAYOUT: usize = 6;
/// The per-character `chat-cache.txt`.
pub(crate) const CHAT: usize = 7;

/// The character a per-character section belongs to, `(realm, name)`.
pub(crate) type Identity = (String, String);

/// The file benilla keeps section `section` in; `None` with no state folder, or for a per-character
/// section with no character.
pub(crate) fn path(section: usize, identity: Option<&Identity>) -> Option<PathBuf> {
    let character = |f: fn(&str, &str) -> Option<PathBuf>| identity.and_then(|(r, c)| f(r, c));
    match section {
        0 => local_state::config_cache_account_path(),
        1 => character(local_state::config_cache_character_path),
        2 => local_state::bindings_cache_account_path(),
        3 => character(local_state::bindings_cache_character_path),
        4 => local_state::macros_account_path(),
        5 => character(local_state::macros_character_path),
        6 => character(local_state::layout_character_path),
        7 => character(local_state::chat_character_path),
        _ => None,
    }
}

/// Section `section` as the server is sent it, or `None` when its file does not open, which the
/// reference digests as zeros (`0x5af968`) and uploads nothing for (`0x5afb09`).
pub(crate) fn read(
    section: usize,
    identity: Option<&Identity>,
    defaults: &[KeyBinding],
) -> Option<Vec<u8>> {
    let bytes = match read_file(&path(section, identity)?) {
        Some(bytes) => bytes,
        // A set saved before the section's file was kept: its diff, as the whole table.
        None if BINDINGS.contains(&section) => {
            let diff = read_file(&diff_path(section, identity)?)?;
            let diff = store::from_diff(&String::from_utf8_lossy(&diff));
            return Some(store::to_cache(&diff, defaults).into_bytes());
        }
        None => return None,
    };
    Some(match section {
        LAYOUT | CHAT => without_notes(&bytes),
        _ => bytes,
    })
}

/// A file's bytes, or `None` when it does not open.
fn read_file(path: &std::path::Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                bevy::log::warn!("account data: reading {}: {e}", path.display());
            }
            None
        }
    }
}

/// The diff a binding section's set is loaded from ([`crate::bindings`]).
fn diff_path(section: usize, identity: Option<&Identity>) -> Option<PathBuf> {
    match section {
        2 => local_state::bindings_account_path(),
        3 => identity.and_then(|(r, c)| local_state::bindings_character_path(r, c)),
        _ => None,
    }
}

/// Store a section's file from the server, as it came; whether it was stored. A binding section
/// is also read into the diff its set loads from, and a character table binding no key is the
/// account set in use (`0x4b64b0`): the character's diff goes.
pub(crate) fn write(
    section: usize,
    identity: Option<&Identity>,
    defaults: &[KeyBinding],
    bytes: &[u8],
) -> bool {
    let Some(path) = path(section, identity) else {
        return false;
    };
    let result = match section {
        2 | 3 => local_state::write_atomic_bytes(&path, bytes).and_then(|()| {
            let Some(diff_path) = diff_path(section, identity) else {
                return Ok(());
            };
            match store::from_cache(&String::from_utf8_lossy(bytes), defaults) {
                Some(diff) => local_state::write_atomic(&diff_path, &diff),
                None if section == 3 => match std::fs::remove_file(&diff_path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                },
                None => local_state::write_atomic(&diff_path, &store::to_diff(&[], &[])),
            }
        }),
        CHAT => {
            local_state::write_atomic_bytes(&path, &crate::ui_chat::settings::from_server(bytes))
        }
        _ => local_state::write_atomic_bytes(&path, bytes),
    };
    match result {
        Ok(()) => true,
        Err(e) => {
            bevy::log::warn!("account data: storing {}: {e}", path.display());
            false
        }
    }
}

/// The digest of a section's bytes, `0x5af940`'s MD5, or 16 zero bytes when it has no file.
pub(crate) fn digest(bytes: Option<&[u8]>) -> AccountDataDigest {
    bytes.map_or([0; 16], |b| Md5::digest(b).into())
}

/// The file without benilla's own `# ` note lines, which are no part of the grammar the
/// reference's readers take (`0x490380`, `0x498a60`), so a section is that grammar alone.
pub(crate) fn without_notes(bytes: &[u8]) -> Vec<u8> {
    bytes
        .split_inclusive(|&b| b == b'\n')
        .filter(|line| !line.starts_with(b"# "))
        .flatten()
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_digest_is_md5_or_zeros_for_no_file() {
        assert_eq!(digest(None), [0; 16]);
        // The MD5 specification's own test value: MD5("abc").
        assert_eq!(
            digest(Some(b"abc")),
            [
                0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0, 0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1,
                0x7f, 0x72
            ]
        );
        assert_ne!(digest(Some(b"")), [0; 16], "an empty file is a file");
    }

    #[test]
    fn notes_leave_the_section_and_nothing_else_does() {
        assert_eq!(
            without_notes(b"# benilla header\nVERSION 2\n# more\nCHANNELS\n#x\nEND"),
            b"VERSION 2\nCHANNELS\n#x\nEND".to_vec()
        );
        assert_eq!(
            without_notes(b"Frame: A\r\nX: 1\r\n"),
            b"Frame: A\r\nX: 1\r\n"
        );
    }

    #[test]
    fn the_scopes_and_the_files_are_the_reference_table() {
        assert_eq!(
            ACCOUNT_WIDE,
            [true, false, true, false, true, false, false, false]
        );
        let _env = local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _home = local_state::test_env::EnvGuard::set("BENILLA_HOME", "/nonexistent/benilla");
        let _capture = local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let me = ("Realm".to_string(), "Probesix".to_string());
        for (section, account_wide) in ACCOUNT_WIDE.into_iter().enumerate() {
            assert!(
                path(section, Some(&me)).is_some(),
                "section {section} has a file"
            );
            assert_eq!(
                path(section, None).is_some(),
                account_wide,
                "section {section}: only an account-wide file needs no character"
            );
        }
        assert_eq!(path(8, Some(&me)), None);
    }
}
