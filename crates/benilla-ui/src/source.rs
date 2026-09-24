//! UI source files are bytes, not text. The reference reads a Lua file whole and binary
//! (`0x704bc0` via `0x648620`, no trailing NUL; binary on every leg of `0x647db0`), and
//! `0x704ae0` hands it untranscoded to `luaL_loadbuffer 0x6f5690`, so a cp1252 file's literals
//! keep their raw bytes. [`chunk`] prepares a Lua chunk byte for byte; [`decode`] gives our own
//! `.xml` and `.toc` parsers the `&str` they need. Transcoding Lua instead would change
//! `string.len`, `string.sub` and `string.byte` on every non-ASCII literal.

use std::borrow::Cow;

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// `bytes` without a leading UTF-8 BOM, a strip the reference added to its
/// `luaL_loadbuffer 0x6f5690` (`0x6f5699`–`0x6f56b4`), so every chunk gets it, `loadstring`
/// included; the `.toc` reader has its own at `0x6edc71`. At most one mark, only at offset 0; a
/// UTF-16 mark is left alone, as the reference leaves it.
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(BOM).unwrap_or(bytes)
}

/// `bytes` without a leading `#` line: Lua 5.0 skips it for every chunk in `luaX_setinput`
/// (`0x6ff5cd`–`0x6ff600`), while 5.1 moved the skip to `luaL_loadfile`, which mlua's
/// `Lua::load` never calls. Runs after the BOM strip, the reference's order.
pub fn strip_hashbang(bytes: &[u8]) -> &[u8] {
    match bytes.first() {
        Some(b'#') => match bytes.iter().position(|&b| b == b'\n') {
            // Lua 5.0 keeps the newline for the line counter.
            Some(nl) => &bytes[nl..],
            None => &bytes[bytes.len()..],
        },
        _ => bytes,
    }
}

/// A chunk exactly as the reference's compiler receives it: [`strip_bom`] then [`strip_hashbang`].
pub fn chunk(bytes: &[u8]) -> &[u8] {
    strip_hashbang(strip_bom(bytes))
}

/// UI source bytes as text for our parsers: after the BOM, valid UTF-8 is borrowed unchanged and
/// anything else is read as cp1252, which the non-UTF-8 vanilla addons (Western-European locale
/// files) are written in; every byte maps, so no file fails to decode.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = strip_bom(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| cp1252(b)).collect()),
    }
}

/// One cp1252 byte as a `char`: Latin-1, except `0x80..=0x9F`, where Latin-1 has C1 controls and
/// cp1252 has punctuation (the five undefined slots become U+FFFD).
fn cp1252(b: u8) -> char {
    const C1: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}',
        '\u{017D}', '\u{FFFD}', '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    match b {
        0x80..=0x9F => C1[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_utf8_bom_is_removed_and_nothing_else_is() {
        assert_eq!(strip_bom(b"\xEF\xBB\xBFlocal x = 1"), b"local x = 1");
        assert_eq!(strip_bom(b"local x = 1"), b"local x = 1");
        // A truncated mark is not a BOM.
        assert_eq!(strip_bom(b"\xEF\xBB"), b"\xEF\xBB");
        assert_eq!(strip_bom(b"\xEF"), b"\xEF");
        assert_eq!(strip_bom(b""), b"");
        // A UTF-16 mark is left alone.
        assert_eq!(strip_bom(b"\xFF\xFEl\0"), b"\xFF\xFEl\0");
    }

    #[test]
    fn valid_utf8_is_borrowed_unchanged() {
        let text = "## Title: Sch\u{e4}tze";
        assert!(matches!(decode(text.as_bytes()), Cow::Borrowed(s) if s == text));
        // ...including through a BOM.
        let bom = [BOM, text.as_bytes()].concat();
        assert_eq!(decode(&bom), text);
    }

    /// `0xE4` is `ä` in Latin-1 and cp1252 alike; `0x92` is cp1252's right single quote but a C1
    /// control in Latin-1.
    #[test]
    fn cp1252_is_decoded_where_utf8_fails() {
        assert_eq!(decode(b"## Notes: Sch\xE4tze"), "## Notes: Sch\u{e4}tze");
        assert_eq!(decode(b"l\x92objet"), "l\u{2019}objet");
        assert_eq!(decode(b"\x80"), "\u{20AC}");
        // An undefined cp1252 slot becomes the replacement char.
        assert_eq!(decode(b"\x81"), "\u{FFFD}");
    }

    #[test]
    fn a_leading_hash_line_is_eaten_the_way_lua_5_0_eats_it() {
        // The newline stays, for the line counter.
        assert_eq!(strip_hashbang(b"#!/usr/bin/lua\nreturn 1"), b"\nreturn 1");
        assert_eq!(strip_hashbang(b"# only a comment"), b"");
        assert_eq!(strip_hashbang(b"local n = #t"), b"local n = #t");
        assert_eq!(strip_hashbang(b""), b"");
        // The mark goes first, then the line: the reference's order.
        assert_eq!(chunk(b"\xEF\xBB\xBF#line\nreturn 1"), b"\nreturn 1");
    }

    #[test]
    fn every_byte_sequence_decodes_to_something() {
        let all: Vec<u8> = (0u8..=255).collect();
        let text = decode(&all);
        assert!(!text.is_empty());
        assert!(text.starts_with('\0'));
        assert!(text.contains("ABC"));
    }
}
