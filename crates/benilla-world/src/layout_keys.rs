//! The keyboard layout, as the reference reads it. The Windows client names a key from the
//! virtual-key code the OS layout produced from the scancode: the WndProc's key arm `0x42d21e`
//! hands `wParam` to the translator `0x42d800`, which never reads the scancode. The translator has
//! two tiers, a fixed table for the named keys and the digits, and for every letter and
//! `VK_OEM_*` key the layout's own unshifted character, `MapVirtualKeyA(vk, MAPVK_VK_TO_CHAR)`
//! (`0x42da39`). Bevy's `KeyCode` is the physical key, which a layout moves in neither tier.
//!
//! Named keys: a layout that gives a key another named function (X11's `caps:escape`,
//! `caps:swapescape`, `ctrl:nocaps`, Colemak's Caps Lock as Backspace) moves that function in 1.12;
//! under Wine the same keysym becomes the same virtual key. Caps Lock mapped to Escape arrives as
//! `CapsLock` with the logical key `Escape`. Off macOS, each keyboard message whose logical key is
//! a named key other than its physical one takes that named key's code, before Bevy's input
//! collection, so every reader (the bindings, the text boxes, the glue screens,
//! `ButtonInput<KeyCode>`) sees the layout's key. The Mac client reads its non-character keys off
//! a fixed table on the physical keycode (`0x5bf320`), so macOS keeps them physical, and every
//! platform keeps the numpad keys, whose Num Lock navigation meanings are not remaps.
//!
//! Character keys: [`LayoutChars`] holds the character each key makes under the active layout
//! with no modifier held, which Bevy's `KeyboardInput` drops (its `logical_key` carries Shift, so
//! Shift+1 arrives as `!`). Each key message refreshes its own key before any `Update` reader, so
//! a press is named under the layout it was typed in. The Mac client fills the same keys from the
//! live layout: `KeyTranslate` (`0x8a3c3`) on the current `KCHR`, re-read whenever it changes
//! (`0x8a345`).

use std::collections::HashMap;

use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;

/// Records each key's layout character (every platform) and rewrites remapped named keys before
/// Bevy's input collection (all but macOS).
pub struct LayoutKeysPlugin;

impl Plugin for LayoutKeysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LayoutChars>();
        // A headless build has no winit to register it.
        #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
        app.add_message::<bevy::winit::RawWinitWindowEvent>()
            .add_systems(PreUpdate, record_layout_chars);
        #[cfg(not(target_os = "macos"))]
        app.add_systems(PreUpdate, remap.before(bevy::input::InputSystems));
    }
}

/// The character the active layout makes on each physical key with no modifier held, as the OS
/// last reported it: an entry changes with the next message of its key after a layout switch.
/// Empty, every key reads as unreported, where no winit feeds it (a headless run, a test).
#[derive(Resource, Default, Debug)]
pub struct LayoutChars(HashMap<KeyCode, char>);

impl LayoutChars {
    /// The layout's unshifted character on the physical key `key`, when it makes exactly one
    /// printable character.
    pub fn get(&self, key: KeyCode) -> Option<char> {
        self.0.get(&key).copied()
    }

    /// What the layout makes on `key` with no modifier held: a dead key counts as its character
    /// (the reference's `and eax,0xffff` at `0x42da42` drops `MapVirtualKeyA`'s dead-key bit), and
    /// anything but one printable character clears the entry.
    pub fn record(&mut self, key: KeyCode, unshifted: &Key) {
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

/// Gives each new keyboard message the code of the named key its layout made it.
#[cfg(not(target_os = "macos"))]
fn remap(mut keys: bevy::ecs::message::MessageMutator<bevy::input::keyboard::KeyboardInput>) {
    for key in keys.read() {
        if let Some(code) = layout_code(key.key_code, &key.logical_key) {
            key.key_code = code;
        }
    }
}

/// The code of the named key `logical` stands for when a layout put it on the physical key
/// `physical`, or `None` when the key keeps its code: a character, a numpad key, a named key with
/// no code, or the key already there (either side, for a modifier).
#[cfg(any(not(target_os = "macos"), test))]
fn layout_code(physical: KeyCode, logical: &Key) -> Option<KeyCode> {
    use KeyCode as C;
    if is_numpad(physical) {
        return None;
    }
    let (code, same_family) = match logical {
        Key::Shift => (C::ShiftLeft, [C::ShiftLeft, C::ShiftRight]),
        Key::Control => (C::ControlLeft, [C::ControlLeft, C::ControlRight]),
        Key::Alt => (C::AltLeft, [C::AltLeft, C::AltRight]),
        Key::Super => (C::SuperLeft, [C::SuperLeft, C::SuperRight]),
        _ => {
            let code = named_code(logical)?;
            (code, [code, code])
        }
    };
    (!same_family.contains(&physical)).then_some(code)
}

/// The physical key a non-modifier named key sits on in a plain layout.
#[cfg(any(not(target_os = "macos"), test))]
fn named_code(logical: &Key) -> Option<KeyCode> {
    use KeyCode as C;
    Some(match logical {
        Key::Escape => C::Escape,
        Key::Tab => C::Tab,
        Key::Enter => C::Enter,
        Key::Space => C::Space,
        Key::Backspace => C::Backspace,
        Key::Delete => C::Delete,
        Key::Insert => C::Insert,
        Key::Home => C::Home,
        Key::End => C::End,
        Key::PageUp => C::PageUp,
        Key::PageDown => C::PageDown,
        Key::ArrowUp => C::ArrowUp,
        Key::ArrowDown => C::ArrowDown,
        Key::ArrowLeft => C::ArrowLeft,
        Key::ArrowRight => C::ArrowRight,
        Key::CapsLock => C::CapsLock,
        Key::NumLock => C::NumLock,
        Key::ScrollLock => C::ScrollLock,
        Key::Pause => C::Pause,
        Key::PrintScreen => C::PrintScreen,
        Key::ContextMenu => C::ContextMenu,
        Key::F1 => C::F1,
        Key::F2 => C::F2,
        Key::F3 => C::F3,
        Key::F4 => C::F4,
        Key::F5 => C::F5,
        Key::F6 => C::F6,
        Key::F7 => C::F7,
        Key::F8 => C::F8,
        Key::F9 => C::F9,
        Key::F10 => C::F10,
        Key::F11 => C::F11,
        Key::F12 => C::F12,
        _ => return None,
    })
}

#[cfg(any(not(target_os = "macos"), test))]
fn is_numpad(code: KeyCode) -> bool {
    use KeyCode as C;
    matches!(
        code,
        C::Numpad0
            | C::Numpad1
            | C::Numpad2
            | C::Numpad3
            | C::Numpad4
            | C::Numpad5
            | C::Numpad6
            | C::Numpad7
            | C::Numpad8
            | C::Numpad9
            | C::NumpadAdd
            | C::NumpadSubtract
            | C::NumpadMultiply
            | C::NumpadDivide
            | C::NumpadDecimal
            | C::NumpadComma
            | C::NumpadEnter
            | C::NumpadEqual
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_mapped_to_escape_is_escape() {
        assert_eq!(
            layout_code(KeyCode::CapsLock, &Key::Escape),
            Some(KeyCode::Escape)
        );
        // `caps:swapescape`: the other half of the swap.
        assert_eq!(
            layout_code(KeyCode::Escape, &Key::CapsLock),
            Some(KeyCode::CapsLock)
        );
    }

    #[test]
    fn a_key_where_the_layout_leaves_it_keeps_its_code() {
        assert_eq!(layout_code(KeyCode::Escape, &Key::Escape), None);
        assert_eq!(layout_code(KeyCode::F5, &Key::F5), None);
        assert_eq!(layout_code(KeyCode::ShiftRight, &Key::Shift), None);
        assert_eq!(layout_code(KeyCode::ControlRight, &Key::Control), None);
    }

    #[test]
    fn a_modifier_moved_by_the_layout_is_that_modifier() {
        // `ctrl:nocaps`: Caps Lock is a Control key.
        assert_eq!(
            layout_code(KeyCode::CapsLock, &Key::Control),
            Some(KeyCode::ControlLeft)
        );
    }

    #[test]
    fn characters_and_the_numpad_keep_their_codes() {
        // An azerty A, the key where a qwerty Q sits: named by `LayoutChars`, not moved.
        assert_eq!(
            layout_code(KeyCode::KeyQ, &Key::Character("a".into())),
            None
        );
        // Num Lock off: the numpad 7 means Home, and stays NUMPAD7.
        assert_eq!(layout_code(KeyCode::Numpad7, &Key::Home), None);
        assert_eq!(layout_code(KeyCode::NumpadEnter, &Key::Enter), None);
        // AltGr has no code of its own.
        assert_eq!(layout_code(KeyCode::AltRight, &Key::AltGraph), None);
    }

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
