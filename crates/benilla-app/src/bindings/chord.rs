//! The chord codec: Bevy input to and from the 1.12 binding strings `[ALT-][CTRL-][SHIFT-]<TOKEN>`,
//! with 1.12's own token set. These strings are what the table stores, the window shows and the
//! files save; a press matches by equality, then once more with its leftmost modifier dropped.
//!
//! Prefix order is ALT-CTRL-SHIFT (`Blizzard_BindingUI.lua:176-182`; the emitter `0x4b6630` walks
//! the table at `0x846bd0`), and it decides which modifier the fallback drops. Super/Cmd is not a
//! 1.12 modifier: a chord never carries it and a press with it held never matches.

use bevy::input::mouse::MouseButton;
use bevy::prelude::KeyCode;

/// A bindable base input: a keyboard key, a mouse button, or one wheel direction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum BindKey {
    Key(KeyCode),
    Mouse(MouseButton),
    WheelUp,
    WheelDown,
}

/// One parsed binding chord: the modifier set and the base input, matched by equality.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Chord {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub key: BindKey,
}

impl Chord {
    /// Parse a canonical chord string (`"CTRL--"` is Ctrl+minus); an unknown base token is `None`,
    /// held in the table but never pressable.
    pub(crate) fn parse(s: &str) -> Option<Chord> {
        let (mut alt, mut ctrl, mut shift) = (false, false, false);
        let mut rest = s;
        loop {
            if let Some(r) = rest.strip_prefix("ALT-") {
                alt = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("CTRL-") {
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("SHIFT-") {
                shift = true;
                rest = r;
            } else {
                break;
            }
        }
        Some(Chord {
            alt,
            ctrl,
            shift,
            key: token_key(rest)?,
        })
    }

    /// The one retry after an exact miss: drop the leftmost modifier (ALT, then CTRL, then SHIFT).
    /// `0x4b7990` re-probes the text after the first `'-'` (`0x4b7a2b`-`0x4b7a49`) and never
    /// again (`0x4b7b41`), so `ALT-CTRL-Z` reaches `CTRL-Z` and stops. A bare `-` retries the
    /// empty string there and misses, the `None` here.
    pub(crate) fn fallback(self) -> Option<Chord> {
        if self.alt {
            Some(Chord { alt: false, ..self })
        } else if self.ctrl {
            Some(Chord {
                ctrl: false,
                ..self
            })
        } else if self.shift {
            Some(Chord {
                shift: false,
                ..self
            })
        } else {
            None
        }
    }
}

/// Build the canonical chord string from live modifier state and a base token.
pub(crate) fn chord_string(alt: bool, ctrl: bool, shift: bool, token: &str) -> String {
    let mut s = String::new();
    if alt {
        s.push_str("ALT-");
    }
    if ctrl {
        s.push_str("CTRL-");
    }
    if shift {
        s.push_str("SHIFT-");
    }
    s.push_str(token);
    s
}

/// The 1.12 token for a physical key; `None` for a key the reference names `UNKNOWN` and for the
/// modifiers, which are only prefixes (`IsKeyPressIgnoredForBinding`). Both Enters are `ENTER`.
pub(crate) fn key_token(k: KeyCode) -> Option<&'static str> {
    use KeyCode::*;
    Some(match k {
        KeyA => "A",
        KeyB => "B",
        KeyC => "C",
        KeyD => "D",
        KeyE => "E",
        KeyF => "F",
        KeyG => "G",
        KeyH => "H",
        KeyI => "I",
        KeyJ => "J",
        KeyK => "K",
        KeyL => "L",
        KeyM => "M",
        KeyN => "N",
        KeyO => "O",
        KeyP => "P",
        KeyQ => "Q",
        KeyR => "R",
        KeyS => "S",
        KeyT => "T",
        KeyU => "U",
        KeyV => "V",
        KeyW => "W",
        KeyX => "X",
        KeyY => "Y",
        KeyZ => "Z",
        Digit1 => "1",
        Digit2 => "2",
        Digit3 => "3",
        Digit4 => "4",
        Digit5 => "5",
        Digit6 => "6",
        Digit7 => "7",
        Digit8 => "8",
        Digit9 => "9",
        Digit0 => "0",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        // On a Mac F13 is print-screen: the Mac key table `0x5bf320` maps keycode `0x69` to
        // `0x212`, `PRINTSCREEN`, and macOS has no PrintScreen keycode.
        #[cfg(target_os = "macos")]
        F13 => "PRINTSCREEN",
        // `IsValidBindingKeyString` accepts `F` + any digits (`0x846c04`) and the Mac table maps
        // keycode `0x6A` to `F16` (`0x30d`). Deviation: the Windows key table stops at `VK_F12`
        // and drops later F-keys; they are named here so a stored `F13`-`F24` can be pressed.
        #[cfg(not(target_os = "macos"))]
        F13 => "F13",
        // On a Mac F14 and F15 are ScrollLock and Pause: keycodes `0x6B` and `0x71` map to
        // `0x210` and `0x211`, which the namer calls `UNKNOWN`, so they stay unbindable there.
        #[cfg(not(target_os = "macos"))]
        F14 => "F14",
        #[cfg(not(target_os = "macos"))]
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        F21 => "F21",
        F22 => "F22",
        F23 => "F23",
        F24 => "F24",
        Space => "SPACE",
        Tab => "TAB",
        Enter | NumpadEnter => "ENTER",
        Escape => "ESCAPE",
        Backspace => "BACKSPACE",
        Insert => "INSERT",
        Delete => "DELETE",
        Home => "HOME",
        End => "END",
        PageUp => "PAGEUP",
        PageDown => "PAGEDOWN",
        ArrowUp => "UP",
        ArrowDown => "DOWN",
        ArrowLeft => "LEFT",
        ArrowRight => "RIGHT",
        Numpad0 => "NUMPAD0",
        Numpad1 => "NUMPAD1",
        Numpad2 => "NUMPAD2",
        Numpad3 => "NUMPAD3",
        Numpad4 => "NUMPAD4",
        Numpad5 => "NUMPAD5",
        Numpad6 => "NUMPAD6",
        Numpad7 => "NUMPAD7",
        Numpad8 => "NUMPAD8",
        Numpad9 => "NUMPAD9",
        NumpadAdd => "NUMPADPLUS",
        NumpadSubtract => "NUMPADMINUS",
        NumpadDivide => "NUMPADDIVIDE",
        NumpadMultiply => "NUMPADMULTIPLY",
        NumpadDecimal => "NUMPADDECIMAL",
        // The Mac keypad's `=`: the namer's `0x30c`, and in `IsValidBindingKeyString`'s table.
        NumpadEqual => "NUMPADEQUALS",
        NumLock => "NUMLOCK",
        PrintScreen => "PRINTSCREEN",
        // No ScrollLock or Pause: the namer (`0x4b66b0`) calls `0x210`/`0x211` `UNKNOWN`, and
        // `IsValidBindingKeyString` (`0x4b7890`) has neither name.
        CapsLock => "CAPSLOCK",
        Minus => "-",
        Equal => "=",
        BracketLeft => "[",
        BracketRight => "]",
        Backslash => "\\",
        Semicolon => ";",
        Quote => "'",
        Comma => ",",
        Period => ".",
        Slash => "/",
        Backquote => "`",
        _ => return None,
    })
}

/// Fold key aliases that share one 1.12 token (`NumpadEnter` → `Enter`) so a chord parsed from
/// `ENTER` matches either physical key.
pub(crate) fn normalize_key(k: KeyCode) -> KeyCode {
    match k {
        KeyCode::NumpadEnter => KeyCode::Enter,
        // The Mac print-screen key; must agree with `key_token`, which names it for capture.
        #[cfg(target_os = "macos")]
        KeyCode::F13 => KeyCode::PrintScreen,
        other => other,
    }
}

/// The 1.12 token for a mouse button: BUTTON4 is winit's `Forward` and BUTTON5 its `Back`, so the
/// 1.12 default `BUTTON4 TOGGLEAUTORUN` sits on `Forward`. Which physical button the reference
/// calls BUTTON4 is untraced.
pub(crate) fn mouse_token(b: MouseButton) -> Option<&'static str> {
    Some(match b {
        MouseButton::Left => "BUTTON1",
        MouseButton::Right => "BUTTON2",
        MouseButton::Middle => "BUTTON3",
        MouseButton::Forward => "BUTTON4",
        MouseButton::Back => "BUTTON5",
        // The reference names further buttons `BUTTON<n>` (`0x4b6aa0`'s bit-scan fallback).
        // winit passes the platform's button number, so the sixth button is `Other(5)`.
        MouseButton::Other(n) => return EXTRA_BUTTONS.get(usize::from(n).wrapping_sub(5)).copied(),
    })
}

/// `BUTTON6` to `BUTTON20`, indexed by `Other(n) - 5`.
const EXTRA_BUTTONS: &[&str] = &[
    "BUTTON6", "BUTTON7", "BUTTON8", "BUTTON9", "BUTTON10", "BUTTON11", "BUTTON12", "BUTTON13",
    "BUTTON14", "BUTTON15", "BUTTON16", "BUTTON17", "BUTTON18", "BUTTON19", "BUTTON20",
];

/// Token to base input: the inverse of [`key_token`] and [`mouse_token`], plus the wheel pair.
fn token_key(t: &str) -> Option<BindKey> {
    use KeyCode::*;
    if let Some(b) = match t {
        "BUTTON1" => Some(MouseButton::Left),
        "BUTTON2" => Some(MouseButton::Right),
        "BUTTON3" => Some(MouseButton::Middle),
        "BUTTON4" => Some(MouseButton::Forward),
        "BUTTON5" => Some(MouseButton::Back),
        _ => EXTRA_BUTTONS
            .iter()
            .position(|n| *n == t)
            .and_then(|i| u16::try_from(i + 5).ok())
            .map(MouseButton::Other),
    } {
        return Some(BindKey::Mouse(b));
    }
    match t {
        "MOUSEWHEELUP" => return Some(BindKey::WheelUp),
        "MOUSEWHEELDOWN" => return Some(BindKey::WheelDown),
        _ => {}
    }
    let k = match t {
        "A" => KeyA,
        "B" => KeyB,
        "C" => KeyC,
        "D" => KeyD,
        "E" => KeyE,
        "F" => KeyF,
        "G" => KeyG,
        "H" => KeyH,
        "I" => KeyI,
        "J" => KeyJ,
        "K" => KeyK,
        "L" => KeyL,
        "M" => KeyM,
        "N" => KeyN,
        "O" => KeyO,
        "P" => KeyP,
        "Q" => KeyQ,
        "R" => KeyR,
        "S" => KeyS,
        "T" => KeyT,
        "U" => KeyU,
        "V" => KeyV,
        "W" => KeyW,
        "X" => KeyX,
        "Y" => KeyY,
        "Z" => KeyZ,
        "1" => Digit1,
        "2" => Digit2,
        "3" => Digit3,
        "4" => Digit4,
        "5" => Digit5,
        "6" => Digit6,
        "7" => Digit7,
        "8" => Digit8,
        "9" => Digit9,
        "0" => Digit0,
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "F13" => F13,
        "F14" => F14,
        "F15" => F15,
        "F16" => F16,
        "F17" => F17,
        "F18" => F18,
        "F19" => F19,
        "F20" => F20,
        "F21" => F21,
        "F22" => F22,
        "F23" => F23,
        "F24" => F24,
        "SPACE" => Space,
        "TAB" => Tab,
        "ENTER" => Enter,
        "ESCAPE" => Escape,
        "BACKSPACE" => Backspace,
        "INSERT" => Insert,
        "DELETE" => Delete,
        "HOME" => Home,
        "END" => End,
        "PAGEUP" => PageUp,
        "PAGEDOWN" => PageDown,
        "UP" => ArrowUp,
        "DOWN" => ArrowDown,
        "LEFT" => ArrowLeft,
        "RIGHT" => ArrowRight,
        "NUMPAD0" => Numpad0,
        "NUMPAD1" => Numpad1,
        "NUMPAD2" => Numpad2,
        "NUMPAD3" => Numpad3,
        "NUMPAD4" => Numpad4,
        "NUMPAD5" => Numpad5,
        "NUMPAD6" => Numpad6,
        "NUMPAD7" => Numpad7,
        "NUMPAD8" => Numpad8,
        "NUMPAD9" => Numpad9,
        "NUMPADPLUS" => NumpadAdd,
        "NUMPADMINUS" => NumpadSubtract,
        "NUMPADDIVIDE" => NumpadDivide,
        "NUMPADMULTIPLY" => NumpadMultiply,
        "NUMPADDECIMAL" => NumpadDecimal,
        "NUMPADEQUALS" => NumpadEqual,
        "NUMLOCK" => NumLock,
        "PRINTSCREEN" => PrintScreen,
        "CAPSLOCK" => CapsLock,
        "-" => Minus,
        "=" => Equal,
        "[" => BracketLeft,
        "]" => BracketRight,
        "\\" => Backslash,
        ";" => Semicolon,
        "'" => Quote,
        "," => Comma,
        "." => Period,
        "/" => Slash,
        "`" => Backquote,
        _ => return None,
    };
    // Fold aliases so a parsed chord equals a normalized press, and accept only a token the
    // namer produces on this platform (on a Mac, F13-F15 are other keys).
    let k = normalize_key(k);
    if key_token(k) != Some(t) {
        return None;
    }
    Some(BindKey::Key(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ties this namer to the engine's `IsValidBindingKeyString`: every default chord, and one key
    /// of each token shape, since `KeyCode` cannot be enumerated.
    #[test]
    fn every_token_the_codec_names_is_one_setbinding_accepts() {
        use benilla_ui::script::keybind::normalize_binding_key;
        for spec in super::super::commands::SPECS {
            for default in [spec.d1, spec.d2].into_iter().flatten() {
                assert_eq!(
                    normalize_binding_key(default).as_deref(),
                    Some(default),
                    "the default chord '{default}' ({}) is not a bindable key string",
                    spec.name
                );
            }
        }
        for k in [
            KeyCode::KeyW,
            KeyCode::Digit0,
            KeyCode::F1,
            KeyCode::F12,
            KeyCode::Numpad7,
            KeyCode::NumpadAdd,
            KeyCode::NumpadDecimal,
            KeyCode::ArrowUp,
            KeyCode::Space,
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Backspace,
            KeyCode::Tab,
            KeyCode::Insert,
            KeyCode::Delete,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
            KeyCode::NumLock,
            KeyCode::CapsLock,
            KeyCode::PrintScreen,
            KeyCode::Minus,
            KeyCode::Equal,
            KeyCode::BracketLeft,
            KeyCode::Backslash,
            KeyCode::Semicolon,
            KeyCode::Quote,
            KeyCode::Comma,
            KeyCode::Period,
            KeyCode::Slash,
            KeyCode::Backquote,
        ] {
            let token = key_token(k).expect("named");
            assert!(
                normalize_binding_key(token).is_some(),
                "{k:?} names '{token}', which SetBinding refuses"
            );
        }
        for b in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Forward,
            MouseButton::Back,
        ] {
            let token = mouse_token(b).expect("named");
            assert!(normalize_binding_key(token).is_some(), "{b:?} → '{token}'");
        }
        for token in ["MOUSEWHEELUP", "MOUSEWHEELDOWN"] {
            assert!(normalize_binding_key(token).is_some());
        }
        // The reference's namer calls these `UNKNOWN` (`0x210`/`0x211`).
        assert_eq!(key_token(KeyCode::ScrollLock), None);
        assert_eq!(key_token(KeyCode::Pause), None);
    }

    /// The validator's accept set is infinite, so this covers the tokens a real keyboard or mouse
    /// has; `F25`, `BUTTON21` and `NUMPAD11` stay unpressable, as no such key exists.
    #[test]
    fn every_token_setbinding_accepts_for_a_real_key_is_one_the_codec_can_press() {
        use benilla_ui::script::keybind::normalize_binding_key;

        let mut tokens: Vec<String> = Vec::new();
        tokens.extend((1..=24).map(|n| format!("F{n}")));
        // On a Mac F13 is print-screen and F14/F15 are the unbindable ScrollLock and Pause.
        if cfg!(target_os = "macos") {
            tokens.retain(|t| !matches!(t.as_str(), "F13" | "F14" | "F15"));
        }
        tokens.extend((0..=9).map(|n| format!("NUMPAD{n}")));
        tokens.extend((1..=20).map(|n| format!("BUTTON{n}")));
        tokens.extend(
            [
                "SPACE",
                "NUMPADPLUS",
                "NUMPADMINUS",
                "NUMPADMULTIPLY",
                "NUMPADDIVIDE",
                "NUMPADDECIMAL",
                "NUMPADEQUALS",
                "ESCAPE",
                "ENTER",
                "BACKSPACE",
                "TAB",
                "LEFT",
                "UP",
                "RIGHT",
                "DOWN",
                "INSERT",
                "DELETE",
                "HOME",
                "END",
                "PAGEUP",
                "PAGEDOWN",
                "NUMLOCK",
                "CAPSLOCK",
                "PRINTSCREEN",
                "MOUSEWHEELDOWN",
                "MOUSEWHEELUP",
            ]
            .map(str::to_string),
        );
        tokens.extend(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-=[]\\;',./`"
                .chars()
                .map(String::from),
        );

        for token in tokens {
            assert_eq!(
                normalize_binding_key(&token).as_deref(),
                Some(token.as_str()),
                "the test's own list has a token SetBinding refuses: '{token}'"
            );
            assert!(
                Chord::parse(&token).is_some(),
                "'{token}' is a key SetBinding stores and this codec cannot press"
            );
        }
    }

    #[test]
    fn the_codec_round_trips_every_keyboard_token() {
        let mut checked = 0;
        for k in [
            KeyCode::KeyW,
            KeyCode::Digit0,
            KeyCode::F11,
            KeyCode::Space,
            KeyCode::NumLock,
            KeyCode::NumpadDivide,
            KeyCode::ArrowUp,
            KeyCode::Minus,
            KeyCode::Backquote,
            KeyCode::Quote,
        ] {
            let t = key_token(k).unwrap();
            assert_eq!(
                token_key(t),
                Some(BindKey::Key(normalize_key(k))),
                "token {t}"
            );
            checked += 1;
        }
        assert_eq!(checked, 10);
        assert_eq!(key_token(KeyCode::NumpadEnter), Some("ENTER"));
        assert_eq!(token_key("ENTER"), Some(BindKey::Key(KeyCode::Enter)));
    }

    /// macOS has no `PrintScreen`, so capture and dispatch must both read F13 as `PRINTSCREEN`.
    #[cfg(target_os = "macos")]
    #[test]
    fn f13_is_print_screen_on_a_mac() {
        assert_eq!(
            key_token(KeyCode::F13),
            Some("PRINTSCREEN"),
            "the capture arm"
        );
        assert_eq!(
            normalize_key(KeyCode::F13),
            KeyCode::PrintScreen,
            "the dispatch arm"
        );
        assert_eq!(
            token_key("PRINTSCREEN"),
            Some(BindKey::Key(normalize_key(KeyCode::F13))),
            "a chord parsed from the shipped default matches an F13 press"
        );
    }

    #[test]
    fn chords_parse_with_the_112_prefix_order_and_punctuation_bases() {
        assert_eq!(
            Chord::parse("ALT-CTRL-SHIFT-F1"),
            Some(Chord {
                alt: true,
                ctrl: true,
                shift: true,
                key: BindKey::Key(KeyCode::F1)
            })
        );
        // `CTRL--` is Ctrl + the minus key.
        assert_eq!(
            Chord::parse("CTRL--"),
            Some(Chord {
                alt: false,
                ctrl: true,
                shift: false,
                key: BindKey::Key(KeyCode::Minus)
            })
        );
        assert_eq!(
            Chord::parse("SHIFT-MOUSEWHEELUP"),
            Some(Chord {
                alt: false,
                ctrl: false,
                shift: true,
                key: BindKey::WheelUp
            })
        );
        assert_eq!(
            Chord::parse("BUTTON4"),
            Some(Chord {
                alt: false,
                ctrl: false,
                shift: false,
                key: BindKey::Mouse(MouseButton::Forward)
            })
        );
        assert_eq!(Chord::parse("BOGUS"), None);
        assert_eq!(
            chord_string(true, false, true, "PAGEDOWN"),
            "ALT-SHIFT-PAGEDOWN"
        );
        assert_eq!(
            Chord::parse("ALT-SHIFT-PAGEDOWN").unwrap(),
            Chord {
                alt: true,
                ctrl: false,
                shift: true,
                key: BindKey::Key(KeyCode::PageDown)
            }
        );
    }
}
