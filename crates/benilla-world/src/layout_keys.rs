//! The keyboard layout's named keys. The Windows client reads a key as the virtual-key code the
//! OS layout produced from the scancode (the WndProc's key arm `0x42d21e` hands `wParam` to the
//! translator `0x42d800`, which never reads the scancode), so a layout that gives a key another
//! named function (X11's `caps:escape`, `caps:swapescape`, `ctrl:nocaps`, Colemak's Caps Lock as
//! Backspace) moves that function in 1.12; under Wine the same keysym becomes the same virtual
//! key. Bevy's `KeyCode` is the physical key, which such a layout does not move: Caps Lock mapped
//! to Escape arrives as `CapsLock` with the logical key `Escape`.
//!
//! Off macOS, each keyboard message whose logical key is a named key other than its physical one
//! takes that named key's code, before Bevy's input collection, so every reader (the bindings,
//! the text boxes, the glue screens, `ButtonInput<KeyCode>`) sees the layout's key. The Mac client
//! reads its non-character keys off a fixed table on the physical keycode (`0x5bf320`), so macOS
//! keeps them physical. A letter, digit or punctuation key keeps its code too: 1.12 names it by
//! the virtual key the layout makes it, a digit as itself (`0x42d81c`) and a letter or punctuation
//! key through `MapVirtualKeyA` (`0x42da39`), which the game's key namer reads, not by a named
//! key. So do the numpad keys, whose Num Lock navigation meanings are not remaps.

#[cfg(any(not(target_os = "macos"), test))]
use bevy::input::keyboard::{Key, KeyCode};
use bevy::prelude::*;

/// Rewrites remapped named keys before Bevy's input collection (all but macOS).
pub struct LayoutKeysPlugin;

impl Plugin for LayoutKeysPlugin {
    #[cfg(not(target_os = "macos"))]
    fn build(&self, app: &mut App) {
        app.add_systems(PreUpdate, remap.before(bevy::input::InputSystems));
    }

    #[cfg(target_os = "macos")]
    fn build(&self, _app: &mut App) {}
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
        // An azerty A, the key where a qwerty Q sits: named by its character, never moved.
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
}
