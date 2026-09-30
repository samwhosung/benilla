//! The character the active keyboard layout makes on each key, which names a letter or punctuation
//! key in 1.12. The Windows client's translator `0x42d800` sends every `VK_A`-`VK_Z` and
//! `VK_OEM_*` key to `MapVirtualKeyA(vk, MAPVK_VK_TO_CHAR)` (`0x42da39`) against the active layout,
//! and the Mac client fills the same keys from the live layout: `KeyTranslate` (`0x8a3c3`) on the
//! current `KCHR`, re-read whenever it changes (`0x8a345`). Bevy's `KeyboardInput` carries the
//! physical key and a `logical_key` with Shift applied (Shift+1 arrives as `!`), so the unshifted
//! character comes from winit's `key_without_modifiers`, read off the raw key messages.

use std::collections::HashMap;

use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;

/// The character the active layout makes on each physical key with no modifier held, as the OS
/// last reported it: an entry changes with the next message of its key after a layout switch, and
/// every key message is read before any `Update` reader, so a press is named under the layout it
/// was typed in. Empty, every key reads as unreported, where no winit feeds it (a headless run, a
/// test).
#[derive(Resource, Default, Debug)]
pub(crate) struct LayoutChars(HashMap<KeyCode, char>);

impl LayoutChars {
    /// The layout's unshifted character on the physical key `key`, when it makes exactly one
    /// printable character.
    pub(crate) fn get(&self, key: KeyCode) -> Option<char> {
        self.0.get(&key).copied()
    }

    /// What the layout makes on `key` with no modifier held: a dead key counts as its character
    /// (the reference's `and eax,0xffff` at `0x42da42` drops `MapVirtualKeyA`'s dead-key bit), and
    /// anything but one printable character clears the entry.
    pub(crate) fn record(&mut self, key: KeyCode, unshifted: &Key) {
        let one = |s: &str| {
            let mut chars = s.chars();
            chars.next().filter(|_| chars.next().is_none())
        };
        let c = match unshifted {
            Key::Character(s) => one(s),
            Key::Dead(c) => *c,
            _ => None,
        };
        match c.filter(|c| !c.is_control() && !c.is_whitespace()) {
            Some(c) => self.0.insert(key, c),
            None => self.0.remove(&key),
        };
    }
}

/// Keeps [`LayoutChars`] current from the raw key messages (every shipped platform).
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<LayoutChars>();
    // A headless build has no winit to register it.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    app.add_message::<bevy::winit::RawWinitWindowEvent>()
        .add_systems(PreUpdate, record_layout_chars);
}

/// Refreshes [`LayoutChars`] from each raw key message: winit's `key_without_modifiers` reads the
/// live layout on all three platforms (`ToUnicodeEx` on Windows, `UCKeyTranslate` on macOS, the
/// first level of the key's xkb group on Linux).
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn record_layout_chars(
    mut raw: MessageReader<bevy::winit::RawWinitWindowEvent>,
    mut chars: ResMut<LayoutChars>,
) {
    use bevy::winit::converters::{convert_logical_key, convert_physical_key_code};
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    for ev in raw.read() {
        if let winit::event::WindowEvent::KeyboardInput { event, .. } = &ev.event {
            chars.record(
                convert_physical_key_code(event.physical_key),
                &convert_logical_key(&event.key_without_modifiers()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_records_the_one_character_its_layout_makes_unshifted() {
        let mut chars = LayoutChars::default();
        // An azerty Z: the key where a qwerty W sits.
        chars.record(KeyCode::KeyW, &Key::Character("z".into()));
        assert_eq!(chars.get(KeyCode::KeyW), Some('z'));
        // A dead key names its accent, as `MapVirtualKeyA` with its dead bit masked off.
        chars.record(KeyCode::BracketLeft, &Key::Dead(Some('^')));
        assert_eq!(chars.get(KeyCode::BracketLeft), Some('^'));
        chars.record(KeyCode::Quote, &Key::Character("ù".into()));
        assert_eq!(chars.get(KeyCode::Quote), Some('ù'));
        // No single printable character: the key is unreported.
        chars.record(KeyCode::Slash, &Key::Character("ch".into()));
        chars.record(KeyCode::Comma, &Key::Character(" ".into()));
        chars.record(KeyCode::Period, &Key::Dead(None));
        for k in [KeyCode::Slash, KeyCode::Comma, KeyCode::Period] {
            assert_eq!(chars.get(k), None, "{k:?}");
        }
        assert_eq!(chars.get(KeyCode::KeyQ), None, "never pressed");
    }

    #[test]
    fn a_layout_switch_renames_a_key_at_its_next_message() {
        let mut chars = LayoutChars::default();
        chars.record(KeyCode::KeyW, &Key::Character("z".into()));
        chars.record(KeyCode::KeyW, &Key::Character("w".into()));
        assert_eq!(
            chars.get(KeyCode::KeyW),
            Some('w'),
            "back on a qwerty layout"
        );
        // A layout that makes no character there clears what the last one made.
        chars.record(KeyCode::KeyW, &Key::Escape);
        assert_eq!(chars.get(KeyCode::KeyW), None);
    }
}
