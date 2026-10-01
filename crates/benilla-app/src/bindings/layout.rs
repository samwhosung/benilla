//! What the active keyboard layout names each letter, digit and punctuation key, which 1.12 binds
//! by. The Windows client names a key from the virtual-key code the layout produced: the
//! translator `0x42d800` keeps `VK_0`-`VK_9` as themselves (`0x42d81c`) and sends every other key
//! outside its fixed table, `VK_A`-`VK_Z` and `VK_OEM_*` among them, to
//! `MapVirtualKeyA(vk, MAPVK_VK_TO_CHAR)` (`0x42da39`), which returns `VK_A`-`VK_Z`'s own letter
//! whatever the layout types there. The Mac client fills its letter and punctuation keys from the
//! live layout, `KeyTranslate` (`0x8a3c3`) on the current `KCHR` (re-read when it changes,
//! `0x8a345`), and fixes its digits by position (`0x5bf320`). Bevy's `KeyboardInput` carries the
//! physical key and a `logical_key` with Shift applied, so each platform's name is read off the
//! raw winit key messages here, before any `Update` reader.

use std::collections::HashMap;

use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;

/// A layout-named key's place on a US keyboard, with the US name there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Position {
    Letter(char),
    Digit(char),
    /// The ISO and JIS extra keys have no US name.
    Punctuation(Option<char>),
}

impl Position {
    fn us(self) -> Option<char> {
        match self {
            Position::Letter(c) | Position::Digit(c) => Some(c),
            Position::Punctuation(c) => c,
        }
    }
}

/// The keys a layout names: the letters, the digits, the punctuation and the ISO and JIS extras.
pub(crate) fn position(k: KeyCode) -> Option<Position> {
    use KeyCode::*;
    use Position::{Digit, Letter, Punctuation};
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
        Digit1 => Digit('1'),
        Digit2 => Digit('2'),
        Digit3 => Digit('3'),
        Digit4 => Digit('4'),
        Digit5 => Digit('5'),
        Digit6 => Digit('6'),
        Digit7 => Digit('7'),
        Digit8 => Digit('8'),
        Digit9 => Digit('9'),
        Digit0 => Digit('0'),
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
        IntlBackslash | IntlRo | IntlYen => Punctuation(None),
        _ => return None,
    })
}

/// A layout-named key's name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LayoutName {
    Char(char),
    /// None: `MapVirtualKeyA` returned 0 and the reference drops the key's message (`0x42da49`).
    Dropped,
}

/// Each layout-named key's name under the active layout, as its platform last reported it: an
/// entry changes with the next message of its key after a layout switch, so a press is named under
/// the layout it was typed in. Empty where no winit feeds it (a headless run, a test).
#[derive(Resource, Default, Debug)]
pub(crate) struct LayoutNames(HashMap<KeyCode, LayoutName>);

impl LayoutNames {
    /// The name of `key` if a layout names it, `None` for a key the fixed table names.
    ///
    /// Deviation: a key its platform reports no character for keeps its US name. The reference
    /// drops such a key, on Windows (`0x42da49`) and on the Mac (`0x8a4aa`); here a key never
    /// reported (a headless run) and a Linux dead key not yet pressed bare stay bindable.
    pub(crate) fn name(&self, key: KeyCode) -> Option<LayoutName> {
        let position = position(key)?;
        Some(match self.0.get(&key) {
            Some(name) => *name,
            None => position.us().map_or(LayoutName::Dropped, LayoutName::Char),
        })
    }

    /// Set what the layout names `key`; `None` clears it back to the US name.
    pub(crate) fn set(&mut self, key: KeyCode, name: Option<LayoutName>) {
        match name {
            Some(name) => self.0.insert(key, name),
            None => self.0.remove(&key),
        };
    }
}

/// Keeps [`LayoutNames`] current from the raw key messages (every shipped platform).
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<LayoutNames>();
    // A headless build has no winit to register it.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    app.add_message::<bevy::winit::RawWinitWindowEvent>()
        .add_systems(PreUpdate, record_layout_names);
}

/// Names each key a raw key message reports, by its platform's rule. On the main thread, the
/// window's: Windows reads that thread's layout, and macOS's input-source calls want it.
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn record_layout_names(
    mut raw: MessageReader<bevy::winit::RawWinitWindowEvent>,
    mut names: ResMut<LayoutNames>,
    #[cfg(target_os = "linux")] mut mods: Local<winit::keyboard::ModifiersState>,
    _main_thread: bevy::ecs::system::NonSendMarker,
) {
    use bevy::winit::converters::convert_physical_key_code;
    use winit::event::WindowEvent;
    #[cfg(target_os = "macos")]
    let mut ascii_capable = None;
    for ev in raw.read() {
        let event = match &ev.event {
            WindowEvent::KeyboardInput { event, .. } => event,
            #[cfg(target_os = "linux")]
            WindowEvent::ModifiersChanged(m) => {
                *mods = m.state();
                continue;
            }
            _ => continue,
        };
        let key = convert_physical_key_code(event.physical_key);
        let Some(position) = position(key) else {
            continue;
        };
        #[cfg(target_os = "windows")]
        {
            let _ = position;
            names.set(key, windows::name(event.physical_key));
        }
        // The Mac and Linux keep their digits by position.
        #[cfg(target_os = "macos")]
        if !matches!(position, Position::Digit(_)) {
            use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
            let capable = *ascii_capable.get_or_insert_with(mac::current_is_ascii_capable);
            let current = one_char(&event.key_without_modifiers());
            let name = mac_name(capable, current, || {
                mac::ascii_capable_char(event.physical_key)
            });
            names.set(key, name);
        }
        #[cfg(target_os = "linux")]
        if !matches!(position, Position::Digit(_)) {
            use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
            let unshifted = event.key_without_modifiers();
            if let Some(name) =
                linux_name(position, &unshifted, &event.logical_key, mods.is_empty())
            {
                names.set(key, name);
            }
        }
    }
}

/// The one printable character a winit key stands for.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn one_char(key: &winit::keyboard::Key) -> Option<char> {
    let winit::keyboard::Key::Character(s) = key else {
        return None;
    };
    let mut chars = s.chars();
    chars
        .next()
        .filter(|c| chars.next().is_none() && !c.is_control() && !c.is_whitespace())
}

/// The reference's name for the virtual key `vk`, given `MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR)`;
/// `None` for a key `0x42d800`'s fixed table names.
///
/// Deviation: a character above U+00FF names the key by itself. The reference's `MapVirtualKeyA`
/// returns an ANSI code-page byte, which `0x42da4b` drops past `0xff` and the namer's UTF-8
/// encoder (`0x41abb0`) otherwise reads as Latin-1, so such a key is mislabelled or dropped; named
/// by what it types, it stays bindable and readable.
#[cfg(any(target_os = "windows", test))]
fn windows_name(vk: u32, to_char: u32) -> Option<LayoutName> {
    match vk {
        // `VK_A`-`VK_Z`, named by their letter, and `VK_0`-`VK_9`, kept as themselves.
        0x41..=0x5a | 0x30..=0x39 => Some(LayoutName::Char(char::from(vk as u8))),
        _ if fixed_vk(vk) => None,
        // The dynamic arm: `and eax,0xffff` (`0x42da42`) drops the dead-key bit, and 0 drops the
        // key (`0x42da49`).
        _ => Some(match char::from_u32(to_char & 0xffff) {
            Some(c) if !c.is_control() && !c.is_whitespace() => LayoutName::Char(c),
            _ => LayoutName::Dropped,
        }),
    }
}

/// `0x42d800`'s fixed table, read from its byte remap at `0x42dafc` and jump table at `0x42da60`:
/// the keys it names itself, never by character. F1-F12 take their own arm (`0x42d800`).
#[cfg(any(target_os = "windows", test))]
fn fixed_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x08 | 0x09
            | 0x0d
            | 0x10..=0x14
            | 0x1b
            | 0x20..=0x28
            | 0x2c..=0x2e
            | 0x60..=0x6b
            | 0x6d..=0x7b
            | 0x90
            | 0x91
    )
}

#[cfg(target_os = "windows")]
mod windows {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, MapVirtualKeyExW, MAPVK_VK_TO_CHAR, MAPVK_VSC_TO_VK_EX,
    };

    /// The reference's own path for the key at `physical`: the virtual key the window thread's
    /// layout gives its scancode, then that key's name (`super::windows_name`).
    pub(super) fn name(physical: winit::keyboard::PhysicalKey) -> Option<super::LayoutName> {
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let scancode = physical.to_scancode()?;
        // SAFETY: plain Win32 calls with no pointers. The caller runs on the main thread, the
        // window's, so thread 0's layout is the one the window's key messages were made under.
        let (vk, to_char) = unsafe {
            let hkl = GetKeyboardLayout(0);
            let vk = MapVirtualKeyExW(scancode, MAPVK_VSC_TO_VK_EX, hkl);
            (vk, MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, hkl))
        };
        if vk == 0 {
            return None;
        }
        super::windows_name(vk, to_char)
    }
}

/// The Mac name of a letter or punctuation key: the layout's own unshifted character, as
/// `KeyTranslate` gives the reference, or none.
///
/// Deviation: a layout that is not ASCII-capable (Hebrew, Greek, Russian, Arabic, Thai) names its
/// keys through the ASCII-capable layout macOS pairs with it, as macOS names its own shortcuts. The
/// reference reads a non-Roman `KCHR`'s byte as MacRoman (`0x8a40f`), which names no real key.
#[cfg(any(target_os = "macos", test))]
fn mac_name(
    ascii_capable: bool,
    current: Option<char>,
    ascii_layout_char: impl FnOnce() -> Option<char>,
) -> Option<LayoutName> {
    let c = if ascii_capable {
        current
    } else {
        ascii_layout_char()
    };
    c.map(LayoutName::Char)
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;
    use std::os::raw::c_ulong;

    type CFTypeRef = *const c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFBooleanGetValue(boolean: CFTypeRef) -> u8;
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyInputSourceIsASCIICapable: CFTypeRef;
        static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(
            layout: *const u8,
            virtual_key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            key_translate_options: u32,
            dead_key_state: *mut u32,
            max_string_length: c_ulong,
            actual_string_length: *mut c_ulong,
            unicode_string: *mut u16,
        ) -> i32;
    }

    const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
    const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;

    /// Is the current keyboard layout one macOS types shortcuts with directly?
    pub(super) fn current_is_ascii_capable() -> bool {
        // SAFETY: the copied source is released here; the property is a borrowed CFBoolean.
        unsafe {
            let source = TISCopyCurrentKeyboardLayoutInputSource();
            if source.is_null() {
                return true;
            }
            let value = TISGetInputSourceProperty(source, kTISPropertyInputSourceIsASCIICapable);
            let capable = value.is_null() || CFBooleanGetValue(value) != 0;
            CFRelease(source);
            capable
        }
    }

    /// The unshifted character the ASCII-capable layout macOS pairs with the current input
    /// source makes on `physical`, dead keys off.
    pub(super) fn ascii_capable_char(physical: winit::keyboard::PhysicalKey) -> Option<char> {
        use winit::platform::scancode::PhysicalKeyExtScancode;
        let keycode = u16::try_from(physical.to_scancode()?).ok()?;
        let mut buf = [0u16; 4];
        let mut len: c_ulong = 0;
        // SAFETY: the copied source owns the layout bytes and is released after the translate;
        // `buf` and `len` are valid for the length passed.
        let status = unsafe {
            let source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
            if source.is_null() {
                return None;
            }
            let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
            let status = if data.is_null() {
                -1
            } else {
                let mut dead = 0u32;
                UCKeyTranslate(
                    CFDataGetBytePtr(data),
                    keycode,
                    K_UC_KEY_ACTION_DISPLAY,
                    0,
                    u32::from(LMGetKbdType()),
                    K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
                    &mut dead,
                    buf.len() as c_ulong,
                    &mut len,
                    buf.as_mut_ptr(),
                )
            };
            CFRelease(source);
            status
        };
        if status != 0 {
            return None;
        }
        let units = buf.get(..usize::try_from(len).ok()?)?;
        let mut chars = char::decode_utf16(units.iter().copied());
        let c = chars.next()?.ok()?;
        (chars.next().is_none() && !c.is_control() && !c.is_whitespace()).then_some(c)
    }
}

/// The Linux name of a letter or punctuation key, `None` to keep what the key had. No 1.12 client
/// ran on Linux, so this is the closest consistent rule: a letter position takes an ASCII letter or
/// ASCII punctuation the layout types there, and anything else there (another script's letter, an
/// accented letter, a mark) keeps its US letter, as Windows keeps `VK_A`-`VK_Z` there on nearly
/// every such layout; any other key takes its character. A dead keysym has no character
/// (`key_without_modifiers` is `Unidentified`), so it takes the dead key's own character from a
/// press with no modifier held, and otherwise keeps what it had.
#[cfg(any(target_os = "linux", test))]
fn linux_name(
    position: Position,
    unshifted: &winit::keyboard::Key,
    logical: &winit::keyboard::Key,
    bare: bool,
) -> Option<Option<LayoutName>> {
    use winit::keyboard::Key;
    let c = match (unshifted, logical) {
        (Key::Unidentified(_), Key::Dead(Some(c))) if bare => Some(*c),
        (Key::Unidentified(_), _) => return None,
        _ => one_char(unshifted),
    };
    Some(c.map(|c| {
        LayoutName::Char(match position {
            Position::Letter(us) if !(c.is_ascii_alphabetic() || c.is_ascii_punctuation()) => us,
            _ => c,
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout key's name under `names`, its ASCII letters folded up as the lookup folds them.
    fn named(names: &LayoutNames, k: KeyCode) -> Option<char> {
        match names.name(k) {
            Some(LayoutName::Char(c)) => Some(c.to_ascii_uppercase()),
            _ => None,
        }
    }

    const VK_OEM_1: u32 = 0xba;
    const VK_OEM_COMMA: u32 = 0xbc;
    const VK_OEM_3: u32 = 0xc0;
    const VK_OEM_6: u32 = 0xdd;
    const VK_OEM_7: u32 = 0xde;

    /// Windows: the reference's path on rows of the layouts' own tables (the key, its virtual
    /// key, its unshifted character), as `MapVirtualKeyExW` returns them.
    #[test]
    fn windows_names_a_key_by_its_virtual_key_as_the_reference_does() {
        let vk = |c: u8| u32::from(c);
        let rows: &[(&str, KeyCode, u32, u32, char)] = &[
            // Hebrew: the W key types `'` but is `VK_W`; Q types `/` and is `VK_Q`.
            ("hebrew W", KeyCode::KeyW, vk(b'W'), '\''.into(), 'W'),
            ("hebrew Q", KeyCode::KeyQ, vk(b'Q'), '/'.into(), 'Q'),
            ("greek Q", KeyCode::KeyQ, vk(b'Q'), ';'.into(), 'Q'),
            ("lithuanian Q", KeyCode::KeyQ, vk(b'Q'), 'ą'.into(), 'Q'),
            ("lithuanian W", KeyCode::KeyW, vk(b'W'), 'ž'.into(), 'W'),
            ("turkish ı", KeyCode::KeyI, vk(b'I'), 'ı'.into(), 'I'),
            // Turkish Q's `VK_OEM_7` types `i`: one name with `I`, as the lookup folds it.
            ("turkish i", KeyCode::Quote, VK_OEM_7, 'i'.into(), 'I'),
            // Bulgarian: `VK_Q` on the Period key, `VK_OEM_COMMA` on Q.
            ("bulgarian .", KeyCode::Period, vk(b'Q'), 'л'.into(), 'Q'),
            ("bulgarian Q", KeyCode::KeyQ, VK_OEM_COMMA, ','.into(), ','),
            // Hungarian: `VK_0` on Backquote, `VK_OEM_3` `ö` on the 0 key.
            ("hungarian `", KeyCode::Backquote, vk(b'0'), '0'.into(), '0'),
            ("hungarian 0", KeyCode::Digit0, VK_OEM_3, 'ö'.into(), 'ö'),
            // Dvorak's Q position is `VK_OEM_7`, `'`.
            ("dvorak Q", KeyCode::KeyQ, VK_OEM_7, '\''.into(), '\''),
            ("us ;", KeyCode::Semicolon, VK_OEM_1, ';'.into(), ';'),
            // A dead key's bit (`0x80000000`) is masked off: French `^`.
            ("french ^", KeyCode::BracketLeft, VK_OEM_6, 0x8000_005e, '^'),
            // Above U+00FF the key is named by what it types (the deviation).
            ("russian ж", KeyCode::Semicolon, VK_OEM_1, 'ж'.into(), 'ж'),
        ];
        let mut names = LayoutNames::default();
        for &(what, key, vk, to_char, expected) in rows {
            names.set(key, windows_name(vk, to_char));
            assert_eq!(named(&names, key), Some(expected), "{what}");
        }
        // `MapVirtualKeyA` returning 0 drops the key (`0x42da49`): Japanese 106's
        // `VK_OEM_AUTO` on Backquote.
        assert_eq!(windows_name(0xf3, 0), Some(LayoutName::Dropped));
        names.set(KeyCode::Backquote, windows_name(0xf3, 0));
        assert_eq!(names.name(KeyCode::Backquote), Some(LayoutName::Dropped));
        // A key the fixed table names is no layout key's: F1, Space, Numpad 0.
        for vk in [0x70, 0x20, 0x60] {
            assert_eq!(windows_name(vk, 0), None, "{vk:#x}");
        }
    }

    /// The fixed table is `0x42d800`'s, decoded from its remap bytes: the 60 entries less the
    /// digits, which take their own arm.
    #[test]
    fn the_fixed_virtual_keys_are_the_translators_table() {
        let fixed: Vec<u32> = (0..=0xffu32)
            .filter(|&vk| fixed_vk(vk) && !(0x70..=0x7b).contains(&vk))
            .collect();
        assert_eq!(
            fixed,
            [
                0x08, 0x09, 0x0d, 0x10, 0x11, 0x12, 0x13, 0x14, 0x1b, 0x20, 0x21, 0x22, 0x23, 0x24,
                0x25, 0x26, 0x27, 0x28, 0x2c, 0x2d, 0x2e, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66,
                0x67, 0x68, 0x69, 0x6a, 0x6b, 0x6d, 0x6e, 0x6f, 0x90, 0x91,
            ]
        );
    }

    /// macOS: a Latin layout's own character; a layout that is not ASCII-capable names through
    /// its ASCII-capable partner.
    #[test]
    fn a_mac_layout_that_is_not_ascii_capable_names_through_its_ascii_partner() {
        // Hebrew: W types `'`; its U.S. partner types `w` there.
        assert_eq!(
            mac_name(false, Some('\''), || Some('w')),
            Some(LayoutName::Char('w'))
        );
        // French: W types `z`, and the partner is never asked.
        assert_eq!(
            mac_name(true, Some('z'), || panic!(
                "an ASCII-capable layout names itself"
            )),
            Some(LayoutName::Char('z'))
        );
        // No character: the US name stands in (the fallback deviation).
        let mut names = LayoutNames::default();
        names.set(KeyCode::KeyW, mac_name(true, None, || None));
        assert_eq!(named(&names, KeyCode::KeyW), Some('W'));
    }

    /// Linux: a letter position keeps its US letter unless the layout types an ASCII letter or
    /// ASCII punctuation there; any other key takes its character.
    #[test]
    fn linux_names_a_letter_position_by_its_ascii_character_or_its_us_letter() {
        use winit::keyboard::{Key, NativeKey};
        let ch = |c: &str| Key::Character(c.into());
        for (key, typed, expected) in [
            (KeyCode::KeyW, "z", 'z'),
            (KeyCode::KeyM, ",", ','),
            (KeyCode::KeyQ, "'", '\''),
            (KeyCode::KeyQ, "ą", 'Q'),
            (KeyCode::KeyI, "ı", 'I'),
            (KeyCode::KeyW, "ц", 'W'),
            // Thai's tone mark on H, INSCRIPT's virama on D.
            (KeyCode::KeyH, "\u{e48}", 'H'),
            (KeyCode::KeyD, "\u{94d}", 'D'),
            (KeyCode::Semicolon, "ж", 'ж'),
            (KeyCode::Quote, "ù", 'ù'),
        ] {
            let got = linux_name(position(key).unwrap(), &ch(typed), &ch(typed), true);
            assert_eq!(
                got,
                Some(Some(LayoutName::Char(expected))),
                "{key:?} typing {typed}"
            );
        }
        // A dead keysym: its character from a bare press, kept through a modified one.
        let none = Key::Unidentified(NativeKey::Unidentified);
        let bracket = position(KeyCode::BracketLeft).unwrap();
        assert_eq!(
            linux_name(bracket, &none, &Key::Dead(Some('^')), true),
            Some(Some(LayoutName::Char('^')))
        );
        assert_eq!(
            linux_name(bracket, &none, &Key::Dead(Some('¨')), false),
            None
        );
    }

    #[test]
    fn a_layout_switch_renames_a_key_at_its_next_message() {
        let mut names = LayoutNames::default();
        names.set(KeyCode::KeyW, Some(LayoutName::Char('z')));
        assert_eq!(named(&names, KeyCode::KeyW), Some('Z'));
        names.set(KeyCode::KeyW, Some(LayoutName::Char('w')));
        assert_eq!(
            named(&names, KeyCode::KeyW),
            Some('W'),
            "back on a US layout"
        );
        // A layout that makes no character there falls back to the US name.
        names.set(KeyCode::KeyQ, Some(LayoutName::Char('a')));
        names.set(KeyCode::KeyQ, None);
        assert_eq!(named(&names, KeyCode::KeyQ), Some('Q'));
        // Digits and punctuation name themselves until a layout says otherwise; an ISO key no
        // layout reported has no name.
        assert_eq!(named(&names, KeyCode::Digit1), Some('1'));
        assert_eq!(named(&names, KeyCode::Quote), Some('\''));
        assert_eq!(
            names.name(KeyCode::IntlBackslash),
            Some(LayoutName::Dropped)
        );
        assert_eq!(names.name(KeyCode::F1), None, "the fixed table's");
    }
}
