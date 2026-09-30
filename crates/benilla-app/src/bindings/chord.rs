//! The chord codec: Bevy input to and from the 1.12 binding strings `[ALT-][CTRL-][SHIFT-]<TOKEN>`,
//! with 1.12's own token set. These strings are what the table stores, the window shows and the
//! files save; a press matches by equality, then once more with its leftmost modifier dropped.
//!
//! Prefix order is ALT-CTRL-SHIFT (`Blizzard_BindingUI.lua:176-182`; the emitter `0x4b6630` walks
//! the table at `0x846bd0`), and it decides which modifier the fallback drops. Super/Cmd is not a
//! 1.12 modifier: a chord never carries it and a press with it held never matches.

use std::fmt;

use bevy::input::mouse::MouseButton;
use bevy::prelude::KeyCode;

use super::layout::LayoutChars;

/// A key's 1.12 name: one character (`Z`, `1`, `ù`), or a word from the reference's name table
/// (`F1`, `NUMPAD7`, `SPACE`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum KeyName {
    Char(char),
    Word(&'static str),
}

impl fmt::Display for KeyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyName::Char(c) => write!(f, "{c}"),
            KeyName::Word(w) => f.write_str(w),
        }
    }
}

/// A bindable base input: a keyboard key by its 1.12 name, a mouse button, or one wheel direction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum BindKey {
    Key(KeyName),
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

/// The 1.12 name a key press gets: a letter or punctuation key its active layout's unshifted
/// character, every other key its fixed name. `None` for a key the reference names `UNKNOWN`, for
/// the modifiers, which are only prefixes (`IsKeyPressIgnoredForBinding`), and for an ISO or JIS
/// extra key its layout reports no character for. Both Enters are `ENTER`.
pub(crate) fn key_token(k: KeyCode, layout: &LayoutChars) -> Option<KeyName> {
    match character_key(k) {
        Some(key) => layout_name(key, layout.get(k)).map(KeyName::Char),
        None => fixed_name(k),
    }
}

/// A key the reference names by its layout's character: `0x42d800` sends every `VK_A`-`VK_Z` and
/// `VK_OEM_*` to `MapVirtualKeyA(vk, MAPVK_VK_TO_CHAR)` (`0x42da39`), and the Mac table
/// (`0x5bf320`) leaves the same keys, `-` and `=` among them, to `KeyTranslate`. The digits are
/// fixed on both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CharacterKey {
    /// A letter position, with the US letter there.
    Letter(char),
    /// A punctuation position, with the US character there; the ISO and JIS extras have none.
    Punctuation(Option<char>),
}

fn character_key(k: KeyCode) -> Option<CharacterKey> {
    use CharacterKey::{Letter, Punctuation};
    use KeyCode::*;
    Some(match k {
        KeyA => Letter('A'),
        KeyB => Letter('B'),
        KeyC => Letter('C'),
        KeyD => Letter('D'),
        KeyE => Letter('E'),
        KeyF => Letter('F'),
        KeyG => Letter('G'),
        KeyH => Letter('H'),
        KeyI => Letter('I'),
        KeyJ => Letter('J'),
        KeyK => Letter('K'),
        KeyL => Letter('L'),
        KeyM => Letter('M'),
        KeyN => Letter('N'),
        KeyO => Letter('O'),
        KeyP => Letter('P'),
        KeyQ => Letter('Q'),
        KeyR => Letter('R'),
        KeyS => Letter('S'),
        KeyT => Letter('T'),
        KeyU => Letter('U'),
        KeyV => Letter('V'),
        KeyW => Letter('W'),
        KeyX => Letter('X'),
        KeyY => Letter('Y'),
        KeyZ => Letter('Z'),
        Minus => Punctuation(Some('-')),
        Equal => Punctuation(Some('=')),
        BracketLeft => Punctuation(Some('[')),
        BracketRight => Punctuation(Some(']')),
        Backslash => Punctuation(Some('\\')),
        Semicolon => Punctuation(Some(';')),
        Quote => Punctuation(Some('\'')),
        Comma => Punctuation(Some(',')),
        Period => Punctuation(Some('.')),
        Slash => Punctuation(Some('/')),
        Backquote => Punctuation(Some('`')),
        // `VK_OEM_102`, and the Mac's ISO section key and JIS keys, are layout keys too.
        IntlBackslash | IntlRo | IntlYen => Punctuation(None),
        _ => return None,
    })
}

/// The name of a character key whose layout makes `unshifted` there: that character, a letter
/// uppercased as `MapVirtualKeyA` returns `VK_A`-`VK_Z`, or the US one where the layout reports
/// none. On AZERTY the key where a US W sits is `Z`.
///
/// A letter position that makes a letter of another script keeps its US letter: a Cyrillic,
/// Greek or Hebrew layout puts `VK_A`-`VK_Z` on the US positions, so the Windows client names
/// those keys by that letter and the stock `W` `A` `S` `D` still move. Deviation: for a character
/// outside Latin-1 the Windows key path hands `MapVirtualKeyA`'s ANSI code-page byte to the UTF-8
/// encoder (`0x41abb0`) as a code point, naming the key by another character, and the Mac drops a
/// key whose character does not convert to Latin-1 (`0x8a46b`); here it is named by its own.
fn layout_name(key: CharacterKey, unshifted: Option<char>) -> Option<char> {
    match (key, unshifted) {
        (CharacterKey::Letter(us), Some(c)) if c.is_alphabetic() && !is_latin(c) => Some(us),
        (_, Some(c)) => Some(c.to_ascii_uppercase()),
        (CharacterKey::Letter(us), None) => Some(us),
        (CharacterKey::Punctuation(us), None) => us,
    }
}

/// Is `c` in a Latin-script block: Basic Latin through the IPA Extensions, or Latin Extended
/// Additional, -C or -D?
fn is_latin(c: char) -> bool {
    c <= '\u{2AF}'
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
        || ('\u{2C60}'..='\u{2C7F}').contains(&c)
        || ('\u{A720}'..='\u{A7FF}').contains(&c)
}

/// The fixed half of `0x42d800` and the Mac table: every key that is not a [`character_key`].
fn fixed_name(k: KeyCode) -> Option<KeyName> {
    use KeyCode::*;
    use KeyName::{Char, Word};
    Some(match k {
        Digit1 => Char('1'),
        Digit2 => Char('2'),
        Digit3 => Char('3'),
        Digit4 => Char('4'),
        Digit5 => Char('5'),
        Digit6 => Char('6'),
        Digit7 => Char('7'),
        Digit8 => Char('8'),
        Digit9 => Char('9'),
        Digit0 => Char('0'),
        F1 => Word("F1"),
        F2 => Word("F2"),
        F3 => Word("F3"),
        F4 => Word("F4"),
        F5 => Word("F5"),
        F6 => Word("F6"),
        F7 => Word("F7"),
        F8 => Word("F8"),
        F9 => Word("F9"),
        F10 => Word("F10"),
        F11 => Word("F11"),
        F12 => Word("F12"),
        // On a Mac F13 is print-screen: the Mac key table `0x5bf320` maps keycode `0x69` to
        // `0x212`, `PRINTSCREEN`, and macOS has no PrintScreen keycode.
        #[cfg(target_os = "macos")]
        F13 => Word("PRINTSCREEN"),
        // `IsValidBindingKeyString` accepts `F` + any digits (`0x846c04`) and the Mac table maps
        // keycode `0x6A` to `F16` (`0x30d`). Deviation: the Windows key table stops at `VK_F12`
        // and drops later F-keys; they are named here so a stored `F13`-`F24` can be pressed.
        #[cfg(not(target_os = "macos"))]
        F13 => Word("F13"),
        // On a Mac F14 and F15 are ScrollLock and Pause: keycodes `0x6B` and `0x71` map to
        // `0x210` and `0x211`, which the namer calls `UNKNOWN`, so they stay unbindable there.
        #[cfg(not(target_os = "macos"))]
        F14 => Word("F14"),
        #[cfg(not(target_os = "macos"))]
        F15 => Word("F15"),
        F16 => Word("F16"),
        F17 => Word("F17"),
        F18 => Word("F18"),
        F19 => Word("F19"),
        F20 => Word("F20"),
        F21 => Word("F21"),
        F22 => Word("F22"),
        F23 => Word("F23"),
        F24 => Word("F24"),
        Space => Word("SPACE"),
        Tab => Word("TAB"),
        Enter | NumpadEnter => Word("ENTER"),
        Escape => Word("ESCAPE"),
        Backspace => Word("BACKSPACE"),
        Insert => Word("INSERT"),
        Delete => Word("DELETE"),
        Home => Word("HOME"),
        End => Word("END"),
        PageUp => Word("PAGEUP"),
        PageDown => Word("PAGEDOWN"),
        ArrowUp => Word("UP"),
        ArrowDown => Word("DOWN"),
        ArrowLeft => Word("LEFT"),
        ArrowRight => Word("RIGHT"),
        Numpad0 => Word("NUMPAD0"),
        Numpad1 => Word("NUMPAD1"),
        Numpad2 => Word("NUMPAD2"),
        Numpad3 => Word("NUMPAD3"),
        Numpad4 => Word("NUMPAD4"),
        Numpad5 => Word("NUMPAD5"),
        Numpad6 => Word("NUMPAD6"),
        Numpad7 => Word("NUMPAD7"),
        Numpad8 => Word("NUMPAD8"),
        Numpad9 => Word("NUMPAD9"),
        NumpadAdd => Word("NUMPADPLUS"),
        NumpadSubtract => Word("NUMPADMINUS"),
        NumpadDivide => Word("NUMPADDIVIDE"),
        NumpadMultiply => Word("NUMPADMULTIPLY"),
        NumpadDecimal => Word("NUMPADDECIMAL"),
        // The Mac keypad's `=`: the namer's `0x30c`, and in `IsValidBindingKeyString`'s table.
        NumpadEqual => Word("NUMPADEQUALS"),
        NumLock => Word("NUMLOCK"),
        PrintScreen => Word("PRINTSCREEN"),
        // No ScrollLock or Pause: the namer (`0x4b66b0`) calls `0x210`/`0x211` `UNKNOWN`, and
        // `IsValidBindingKeyString` (`0x4b7890`) has neither name.
        CapsLock => Word("CAPSLOCK"),
        _ => return None,
    })
}

/// Fold the keys the reference gives one code (`NumpadEnter` → `Enter`, both `0x201`), so the
/// pressed-key list and a latch treat them as one key.
pub(crate) fn normalize_key(k: KeyCode) -> KeyCode {
    match k {
        KeyCode::NumpadEnter => KeyCode::Enter,
        other => other,
    }
}

/// `BUTTON6` to `BUTTON20`, indexed by `Other(n) - 5`: the reference names further buttons
/// `BUTTON<n>` (`0x4b6aa0`'s bit-scan fallback), and winit passes the platform's button number, so
/// the sixth button is `Other(5)`.
const EXTRA_BUTTONS: &[&str] = &[
    "BUTTON6", "BUTTON7", "BUTTON8", "BUTTON9", "BUTTON10", "BUTTON11", "BUTTON12", "BUTTON13",
    "BUTTON14", "BUTTON15", "BUTTON16", "BUTTON17", "BUTTON18", "BUTTON19", "BUTTON20",
];

/// Token to base input: the inverse of [`key_token`], the mouse buttons and the wheel pair.
/// BUTTON4 is winit's `Forward` and BUTTON5 its `Back`, so the 1.12 default `BUTTON4
/// TOGGLEAUTORUN` sits on `Forward`; which physical button the reference calls BUTTON4 is untraced.
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
    // One character: a digit, or what some layout's key makes, which the namer writes as UTF-8
    // (`0x4b66b0`'s `[0x21, 0xff]` arm through `0x41abb0`). It never makes a lowercase letter.
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let pressable = !c.is_control() && !c.is_whitespace() && !c.is_ascii_lowercase();
        return pressable.then_some(BindKey::Key(KeyName::Char(c)));
    }
    let k = match t {
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
        _ => return None,
    };
    // Only a name the namer gives on this platform: on a Mac, F13-F15 are other keys.
    match fixed_name(k) {
        Some(name @ KeyName::Word(w)) if w == t => Some(BindKey::Key(name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ties this namer to the engine's `IsValidBindingKeyString`: every chord the install's
    /// `WTF\DefaultBindings.wtf` ships is a key string it accepts and a chord the dispatcher can
    /// press, and one key of each token shape, since `KeyCode` cannot be enumerated.
    #[test]
    fn every_token_the_codec_names_is_one_setbinding_accepts() {
        use benilla_ui::script::keybind::normalize_binding_key;
        let defaults = crate::ui_script::default_bindings();
        if benilla_formats::wow_data().is_some() {
            assert!(
                defaults.len() > 100,
                "the install's defaults: {}",
                defaults.len()
            );
        }
        for (default, command) in &defaults {
            assert_eq!(
                normalize_binding_key(default).as_deref(),
                Some(default.as_str()),
                "the default chord '{default}' ({command}) is not a bindable key string"
            );
            assert!(
                Chord::parse(default).is_some(),
                "{command}: default '{default}' does not parse"
            );
        }
        let us = LayoutChars::default();
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
            let token = key_token(k, &us).expect("named").to_string();
            assert!(
                normalize_binding_key(&token).is_some(),
                "{k:?} names '{token}', which SetBinding refuses"
            );
        }
        for token in [
            "BUTTON1", "BUTTON2", "BUTTON3", "BUTTON4", "BUTTON5", "BUTTON20",
        ] {
            assert!(token_key(token).is_some(), "'{token}' names no button");
            assert!(normalize_binding_key(token).is_some(), "'{token}'");
        }
        for token in ["MOUSEWHEELUP", "MOUSEWHEELDOWN"] {
            assert!(normalize_binding_key(token).is_some());
        }
        // The reference's namer calls these `UNKNOWN` (`0x210`/`0x211`).
        assert_eq!(key_token(KeyCode::ScrollLock, &us), None);
        assert_eq!(key_token(KeyCode::Pause, &us), None);
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
        let us = LayoutChars::default();
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
            let name = key_token(k, &us).unwrap();
            assert_eq!(
                token_key(&name.to_string()),
                Some(BindKey::Key(name)),
                "token {name}"
            );
            checked += 1;
        }
        assert_eq!(checked, 10);
        let enter = Some(KeyName::Word("ENTER"));
        assert_eq!(key_token(KeyCode::NumpadEnter, &us), enter);
        assert_eq!(key_token(KeyCode::Enter, &us), enter);
        assert_eq!(token_key("ENTER"), enter.map(BindKey::Key));
    }

    /// macOS has no `PrintScreen`, so capture and dispatch must both read F13 as `PRINTSCREEN`.
    #[cfg(target_os = "macos")]
    #[test]
    fn f13_is_print_screen_on_a_mac() {
        let name = key_token(KeyCode::F13, &LayoutChars::default());
        assert_eq!(name, Some(KeyName::Word("PRINTSCREEN")));
        assert_eq!(
            token_key("PRINTSCREEN"),
            name.map(BindKey::Key),
            "a chord parsed from the shipped default matches an F13 press"
        );
        assert_eq!(token_key("F13"), None, "no Mac key is F13");
    }

    #[test]
    fn chords_parse_with_the_112_prefix_order_and_punctuation_bases() {
        assert_eq!(
            Chord::parse("ALT-CTRL-SHIFT-F1"),
            Some(Chord {
                alt: true,
                ctrl: true,
                shift: true,
                key: BindKey::Key(KeyName::Word("F1"))
            })
        );
        // `CTRL--` is Ctrl + the minus key.
        assert_eq!(
            Chord::parse("CTRL--"),
            Some(Chord {
                alt: false,
                ctrl: true,
                shift: false,
                key: BindKey::Key(KeyName::Char('-'))
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
            Chord::parse("ALT-SHIFT-PAGEDOWN").unwrap(),
            Chord {
                alt: true,
                ctrl: false,
                shift: true,
                key: BindKey::Key(KeyName::Word("PAGEDOWN"))
            }
        );
    }

    /// A layout as `layout::record_layout_chars` records it from the OS: each key's unshifted
    /// character.
    fn layout(keys: &[(KeyCode, &str)]) -> LayoutChars {
        use bevy::input::keyboard::Key;
        let mut chars = LayoutChars::default();
        for &(k, c) in keys {
            chars.record(k, &Key::Character(c.into()));
        }
        chars
    }

    fn name(k: KeyCode, layout: &LayoutChars) -> Option<String> {
        key_token(k, layout).map(|n| n.to_string())
    }

    /// 1.12 names a letter or punctuation key by the character the active layout makes on it
    /// (`MapVirtualKeyA` at `0x42da39`), and every other key by the fixed table.
    #[test]
    fn a_letter_or_punctuation_key_is_named_by_its_layouts_character() {
        use KeyCode::*;
        let mut azerty = layout(&[
            (KeyW, "z"),
            (KeyZ, "w"),
            (KeyQ, "a"),
            (KeyA, "q"),
            (Semicolon, "m"),
            (KeyM, ","),
            (Comma, ";"),
            (Period, ":"),
            (Slash, "!"),
            (Quote, "ù"),
            (Backquote, "²"),
            (Minus, ")"),
            (BracketRight, "$"),
            (Backslash, "*"),
            (IntlBackslash, "<"),
            // The number row makes `&é"'(-è_çà` unshifted, but its keys are `VK_1`-`VK_0`.
            (Digit1, "&"),
            (Digit2, "é"),
        ]);
        azerty.record(BracketLeft, &bevy::input::keyboard::Key::Dead(Some('^')));
        for (k, expected) in [
            (KeyW, "Z"),
            (KeyZ, "W"),
            (KeyQ, "A"),
            (KeyA, "Q"),
            (Semicolon, "M"),
            (KeyM, ","),
            (Comma, ";"),
            (Period, ":"),
            (Slash, "!"),
            (Quote, "ù"),
            (Backquote, "²"),
            (Minus, ")"),
            (BracketLeft, "^"),
            (BracketRight, "$"),
            (Backslash, "*"),
            (IntlBackslash, "<"),
            (Digit1, "1"),
            (Digit2, "2"),
            (F1, "F1"),
            (Numpad7, "NUMPAD7"),
            (Space, "SPACE"),
        ] {
            assert_eq!(name(k, &azerty).as_deref(), Some(expected), "{k:?}");
            let token = key_token(k, &azerty).unwrap();
            assert_eq!(
                token_key(expected),
                Some(BindKey::Key(token)),
                "the stored '{expected}' is that key's chord"
            );
        }
    }

    /// A US layout, reported or not, names every key as it always has.
    #[test]
    fn a_us_layout_names_its_keys_as_the_us_positions() {
        use KeyCode::*;
        let reported = layout(&[
            (KeyW, "w"),
            (KeyZ, "z"),
            (Quote, "'"),
            (Backquote, "`"),
            (Minus, "-"),
            (IntlBackslash, "\\"),
        ]);
        let unreported = LayoutChars::default();
        for layout in [&reported, &unreported] {
            for (k, expected) in [
                (KeyW, "W"),
                (KeyZ, "Z"),
                (Quote, "'"),
                (Backquote, "`"),
                (Minus, "-"),
                (Digit1, "1"),
            ] {
                assert_eq!(name(k, layout).as_deref(), Some(expected), "{k:?}");
            }
        }
        assert_eq!(name(IntlBackslash, &reported).as_deref(), Some("\\"));
        assert_eq!(
            name(IntlBackslash, &unreported),
            None,
            "an ISO key no layout has named yet"
        );
    }

    /// A non-Latin layout's letter keys keep the US letters Windows gives them as `VK_A`-`VK_Z`,
    /// so `W` `A` `S` `D` still move; its other keys, and a Latin layout's own letters, are named
    /// by their characters.
    #[test]
    fn a_non_latin_layouts_letter_keys_keep_their_us_letters() {
        use KeyCode::*;
        let russian = layout(&[
            (KeyW, "ц"),
            (KeyA, "ф"),
            (KeyS, "ы"),
            (KeyD, "в"),
            (Semicolon, "ж"),
            (Slash, "."),
            (Period, "ю"),
        ]);
        for (k, expected) in [
            (KeyW, "W"),
            (KeyA, "A"),
            (KeyS, "S"),
            (KeyD, "D"),
            (Semicolon, "ж"),
            (Slash, "."),
            (Period, "ю"),
        ] {
            assert_eq!(name(k, &russian).as_deref(), Some(expected), "{k:?}");
        }
        let turkish_f = layout(&[(KeyE, "ğ"), (KeyQ, "f")]);
        assert_eq!(name(KeyE, &turkish_f).as_deref(), Some("ğ"));
        assert_eq!(name(KeyQ, &turkish_f).as_deref(), Some("F"));
    }

    /// Modifiers are prefixes, never part of the name: Shift+1 is `SHIFT-1`, not `!`.
    #[test]
    fn a_chord_names_its_key_unshifted() {
        let azerty = layout(&[(KeyCode::KeyW, "z")]);
        let key = |k| BindKey::Key(key_token(k, &azerty).unwrap());
        assert_eq!(
            Chord::parse("SHIFT-Z").map(|c| (c.shift, c.key)),
            Some((true, key(KeyCode::KeyW)))
        );
        assert_eq!(
            Chord::parse("SHIFT-1").map(|c| (c.shift, c.key)),
            Some((true, key(KeyCode::Digit1)))
        );
        // Nothing presses a lowercase letter, a space or a control character.
        for t in ["w", " ", "\u{7}"] {
            assert_eq!(Chord::parse(t), None, "{t:?}");
        }
        assert_eq!(
            Chord::parse("ù").map(|c| c.key),
            Some(BindKey::Key(KeyName::Char('ù')))
        );
    }
}
