//! The chord codec (decision 0997): Bevy's physical input ↔ the reference's canonical binding
//! strings — `[ALT-][CTRL-][SHIFT-]<TOKEN>`, where the token set is 1.12's own (`W`, `SPACE`,
//! `NUMPAD0`, `BUTTON4`, `MOUSEWHEELUP`, the bare punctuation characters). These strings are what
//! the table stores, the window displays (through the `KEY_*` GlobalStrings), and the files save.
//!
//! A press is matched against them by string equality — and then, on a miss, **once more** with
//! its leftmost modifier dropped ([`Chord::fallback`], decision 1142; this is the half 0585 got
//! wrong and 0997 carried).
//!
//! Prefix order is ALT-CTRL-SHIFT, verified from 1.12's own capture Lua (`Blizzard_BindingUI.lua`
//! prepends SHIFT, then CTRL, then ALT), its saved cache (`CTRL-SHIFT-PAGEDOWN`), and the engine's
//! own emitter (`0x4b6630` walks the `{bitIndex,name}` table at `0x846bd0` downward). It is not
//! only cosmetic: that order is what decides *which* modifier the fallback drops.
//!
//! The Super/Cmd key is **not** a 1.12 binding modifier: a chord never carries it, a super-modified
//! press never matches (0585's `sup` addition), and a capture with Super held is ignored outright.

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

/// One parsed binding chord: the modifier set + the base input. Equality is how a press is
/// probed; [`Chord::fallback`] is the second and last probe when that misses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Chord {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub key: BindKey,
}

impl Chord {
    /// Parse a canonical chord string (`"ALT-CTRL-SHIFT-F1"`, `"CTRL--"` = Ctrl+minus). Unknown
    /// base tokens return `None` — the table may hold them (a future command's key), the
    /// dispatcher just can't press them.
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

    /// The **one** retry the reference allows after an exact miss — drop the leftmost modifier
    /// present, in the emitted prefix order ALT → CTRL → SHIFT. `None` once there is none left,
    /// which is where the lookup ends (decision 1142).
    ///
    /// `CBindings::ExecuteBinding` (`0x4b7990`) does this by string surgery: on a miss it calls
    /// `strchr(chord, '-')` (`0x4b7a2b`/`0x4b7a2d`) and re-probes the text after the **first**
    /// `'-'` (`0x4b7a49 inc eax`). Both second-probe misses land on `0x4b7b41 xor eax,eax` —
    /// there is no third probe and no loop, so `ALT-CTRL-Z` reaches `CTRL-Z` and stops without
    /// ever seeing `ALT-Z` or bare `Z`. Hence `Option`, not an iterator: the chain is two long.
    ///
    /// Cutting at the first `'-'` *is* dropping the leftmost prefix, byte for byte — including
    /// for the one token that contains a `'-'` of its own, the minus key. With a modifier held
    /// the first `'-'` still terminates that prefix (`SHIFT--` → `-`); bare, the reference's
    /// retry lands on the empty string and can only miss, which is exactly the `None` here.
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

/// Build the canonical chord string from live modifier state + a base token — the capture arm's
/// output (prefix order ALT-CTRL-SHIFT, the 1.12 canon).
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

/// The 1.12 token for a physical key — `None` for keys the reference has no name for (they are
/// ignored for binding, the client's own `UNKNOWN` posture). Modifier keys are deliberately
/// absent: they are chord *prefixes*, never base keys (`IsKeyPressIgnoredForBinding`).
///
/// `NumpadEnter` shares `ENTER` with the main Enter key (1.12 has one token; [`normalize_key`]
/// folds the pair for dispatch too).
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
        // **On a Mac keyboard the print-screen key IS F13 — read out of the Mac binary now, not
        // inferred.** The 1.12.1 Mac slice's key table `0x5bf320` maps Mac virtual keycode `0x69`
        // (F13) to `0x212`, the same `PRINTSCREEN` the Windows table reaches from `VK_SNAPSHOT`.
        // (This arm shipped as an inference off `KEY_PRINTSCREEN_MAC = "F13"`; the Mac table
        // settled it at the bytes.) macOS has no PrintScreen keycode at
        // all, so without this the byte-real `PRINTSCREEN SCREENSHOT` default is a dead key on
        // every Mac — not fidelity, a broken key.
        #[cfg(target_os = "macos")]
        F13 => "PRINTSCREEN",
        // **F13-F24: bindable strings, and `F16` a real reference code.**
        // `IsValidBindingKeyString` arm 3 takes `F` + digits with no ceiling (`0x846c04`), and the
        // Mac table really does reach `F16` — keycode `0x6A` → `0x30d`. Naming only F1-F12 meant
        // `SetBinding("F16", …)` stored a chord this codec could never press: the key vanished
        // into `unpressable chord` at the next seed.
        //
        // **Above F12 this is a benilla extension, deliberately.** The reference cannot reach
        // them: Windows' fixed table stops at `VK_F12` and everything past it falls to
        // `MapVirtualKeyA(vk, MAPVK_VK_TO_CHAR)`, which returns 0 for an F-key and drops the
        // message outright. That is a 2004 lookup table's silence, not a rule about keys.
        #[cfg(not(target_os = "macos"))]
        F13 => "F13",
        // **F14 and F15 are the Mac's ScrollLock and Pause, and those two are unbindable.** The
        // Mac table sends keycode `0x6B` (F14) to `0x210` and `0x71` (F15) to `0x211` — the exact
        // pair the namer calls `UNKNOWN` — so on a Mac they are refused for the same reason
        // ScrollLock and Pause are refused below: they ARE those keys. (1.12's
        // `KEY_SCROLLLOCK_MAC` / `KEY_PAUSE_MAC` are dead labels for the same reason; neither
        // binary carries a `SCROLLLOCK` or `PAUSE` string at all.)
        //
        // The line that draws: a key the reference deliberately maps to `UNKNOWN` stays
        // unbindable; a key it simply never mapped, we may name.
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
        // The Mac numeric keypad's `=` key. It is in the reference's namer (`0x30c`) and in
        // `IsValidBindingKeyString`'s 26-name table, and it was in OUR validator too — but not
        // here, so it was the one token `SetBinding` accepted and the dispatcher could not press.
        NumpadEqual => "NUMPADEQUALS",
        NumLock => "NUMLOCK",
        PrintScreen => "PRINTSCREEN",
        // ScrollLock and Pause are deliberately absent: the reference's namer (`0x4b66b0`) calls
        // their key codes (`0x210`/`0x211`) `UNKNOWN`, so they are unbindable there, and
        // `IsValidBindingKeyString` (`0x4b7890`) would refuse the names anyway — its 26-name table
        // holds neither.
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
        // The Mac print-screen key — see [`key_token`]'s arm for the evidence. Folded here as well
        // as there because the two arms serve different halves: `key_token` names the key the
        // capture arm just swallowed, this one makes a press DISPATCH against a chord parsed from
        // the token. Only both together make `PRINTSCREEN` a working binding on a Mac.
        #[cfg(target_os = "macos")]
        KeyCode::F13 => KeyCode::PrintScreen,
        other => other,
    }
}

/// The 1.12 token for a mouse button. BUTTON4 is winit's `Forward` and BUTTON5 `Back` —
/// deliberately keeping the thumb button that toggled autorun before 0997 (macOS winit maps
/// NSEvent buttonNumber 4 → `Forward`; see the old `player.rs` site) on the 1.12 default
/// (`BUTTON4 TOGGLEAUTORUN`), so the director's in-hand behavior survives the refactor.
pub(crate) fn mouse_token(b: MouseButton) -> Option<&'static str> {
    Some(match b {
        MouseButton::Left => "BUTTON1",
        MouseButton::Right => "BUTTON2",
        MouseButton::Middle => "BUTTON3",
        MouseButton::Forward => "BUTTON4",
        MouseButton::Back => "BUTTON5",
        // **The extra buttons on a real mouse.** The reference names them the same way it names
        // the first five and then some: `0x4b6aa0`'s fallback arm bit-scans from bit 3 upward and
        // `sprintf("BUTTON%{d}", n)`, and `IsValidBindingKeyString` arm 3 takes `BUTTON` +
        // digits with no ceiling — so `BUTTON6` was always a storable, listable, *unpressable*
        // chord here. winit hands the platform's own button number through `Other(n)`, and on
        // macOS that is `NSEvent.buttonNumber` — 0-4 already spoken for by the five named
        // variants, so the sixth physical button arrives as `Other(5)` and is `BUTTON6`.
        MouseButton::Other(n) => return EXTRA_BUTTONS.get(usize::from(n).wrapping_sub(5)).copied(),
    })
}

/// `BUTTON6`…`BUTTON20`, indexed by `Other(n) - 5`. A bounded table rather than a formatted
/// string because these names are `&'static str` on both sides of the codec; twenty buttons is
/// past every mouse anyone ships and the reference's own namer stops at the first gap too.
const EXTRA_BUTTONS: &[&str] = &[
    "BUTTON6", "BUTTON7", "BUTTON8", "BUTTON9", "BUTTON10", "BUTTON11", "BUTTON12", "BUTTON13",
    "BUTTON14", "BUTTON15", "BUTTON16", "BUTTON17", "BUTTON18", "BUTTON19", "BUTTON20",
];

/// Token → base input (the parse side of [`key_token`]/[`mouse_token`], plus the wheel pair).
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
    // **Through the alias fold, and only if the namer agrees.** Two invariants, both of which
    // this codec broke somewhere before 1745:
    //
    // 1. A chord parsed from a token must compare equal to the one a press BUILDS, and a press is
    //    normalized ([`normalize_key`]) — so `ENTER` has to land on the same `KeyCode` the
    //    numpad's Enter folds to.
    // 2. A token this side accepts must be one [`key_token`] would produce, on THIS platform.
    //    Otherwise the two tables drift: on a Mac `F13` is the print-screen key and `F14`/`F15`
    //    are the unbindable ScrollLock/Pause pair, so parsing those strings to their raw
    //    `KeyCode`s would make a chord the capture arm can never write and the window can never
    //    show, reachable only by hand-editing the file. Asking the namer closes it structurally
    //    rather than by remembering to cfg both sides.
    let k = normalize_key(k);
    if key_token(k) != Some(t) {
        return None;
    }
    Some(BindKey::Key(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every token this codec names is one `SetBinding` will actually take** (decision 1295).
    /// The namer here and `IsValidBindingKeyString` in the engine table are two transcriptions of
    /// the same reference, and nothing but this ties them together — a token we name but the
    /// validator refuses is a key the capture seam happily offers and `SetBinding` then drops on
    /// the floor, which is exactly what `SCROLLLOCK` and `PAUSE` were until 1295.
    ///
    /// The registry's default chords are covered exhaustively; `KeyCode` cannot be enumerated, so
    /// the namer is covered by one key of every SHAPE it produces (letter, digit, F-key, numpad
    /// digit, numpad name, arrow, the named editing/lock keys, punctuation).
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
        // The other half of the same law: the reference's namer calls these two `UNKNOWN`
        // (`0x210`/`0x211`), so we must not name them either — nothing downstream could bind them.
        assert_eq!(key_token(KeyCode::ScrollLock), None);
        assert_eq!(key_token(KeyCode::Pause), None);
    }

    /// **And the way back — the direction that actually bit.** The test above walks the codec's
    /// output into the validator; nothing walked the validator's *input space* back into the
    /// codec, and three families lived in the gap: `NUMPADEQUALS` (in the reference's namer at
    /// `0x30c`, in the 26-name table, in our validator — and nowhere here), `F13`-`F24`
    /// (`IsValidBindingKeyString` arm 3 is `F` + digits with no ceiling), and `BUTTON6`+ (arm 3
    /// again, and `0x4b6aa0`'s bit-scan fallback). Each was a key `SetBinding` accepted, the
    /// window listed, the file saved — and the dispatcher could never press, so it came back as
    /// `bindings: <cmd>: unpressable chord` and the player's key did nothing.
    ///
    /// The accept set is infinite (any single character, `F`/`NUMPAD`/`BUTTON` + any digits), so
    /// this pins the families that name a key a real keyboard or mouse HAS. Past those — `F25`,
    /// `BUTTON21`, `NUMPAD11` — the validator still accepts and the codec still refuses, which is
    /// correct and deliberate: there is no physical key to press, and inventing a `KeyCode` for
    /// one would be inventing hardware.
    #[test]
    fn every_token_setbinding_accepts_for_a_real_key_is_one_the_codec_can_press() {
        use benilla_ui::script::keybind::normalize_binding_key;

        let mut tokens: Vec<String> = Vec::new();
        tokens.extend((1..=24).map(|n| format!("F{n}")));
        // On a Mac those three are other keys: F13 IS print-screen (`0x5bf320`: keycode `0x69` →
        // `0x212`) and F14/F15 are the unbindable ScrollLock/Pause pair (`0x210`/`0x211`). The
        // codec refuses them there on purpose, so the list has to as well.
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
        // Every key with a token parses back to itself (through the alias fold).
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
        // The alias fold: both Enters share the token and the parsed key.
        assert_eq!(key_token(KeyCode::NumpadEnter), Some("ENTER"));
        assert_eq!(token_key("ENTER"), Some(BindKey::Key(KeyCode::Enter)));
    }

    /// The Mac print-screen fold (decision 1487). macOS delivers no `PrintScreen` at all — a PC
    /// keyboard's PrtSc arrives as F13 — so BOTH arms have to agree that F13 *is* `PRINTSCREEN`,
    /// or the shipped `PRINTSCREEN SCREENSHOT` default is a key nobody on a Mac can press.
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
        // `CTRL--` is Ctrl + the minus key — the prefix strip never splits the base token.
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
        // The builder emits the same canon the parser reads.
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
