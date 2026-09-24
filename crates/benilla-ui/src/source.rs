//! **UI source bytes** — what a `.lua`, `.xml` or `.toc` file *is*, before any parser sees it.
//!
//! One rule, and it is the reference's: **a UI source file is bytes, not text.** `0x704bc0` slurps
//! the file (`0x648620`) with `pad = 0`, so there is not even a trailing NUL, the handle is opened
//! **binary** on every leg of `0x647db0`, and `0x704ae0` hands the pointer and length straight to
//! `luaL_loadbuffer 0x6f5690`. No transcoding, no codepage conversion; the only
//! `WideCharToMultiByte` on the path converts the *path*. Lua 5.0 strings *are* byte strings, so a
//! cp1252 German locale file loads there and its literals carry the raw bytes.
//!
//! Two things the compiler's front door does do, and we do them too because they are the
//! reference's own code rather than Lua's: it **strips a UTF-8 BOM** ([`strip_bom`] — ten
//! instructions Blizzard patched into `luaL_loadbuffer`) and it **eats a leading `#`-line**
//! ([`strip_hashbang`] — Lua 5.0 puts that in `luaX_setinput`, where every chunk passes; 5.1 moved
//! it somewhere mlua never calls).
//!
//! benilla read every one of these files with `read_to_string` until decision 1193, which meant a
//! file that was not valid UTF-8 did not merely lose a glyph — it **did not exist**: the read
//! returned `None` and the loader reported "not found". Measured over a real 218-addon vanilla
//! corpus, that is **80 addons of 218** (37 %): 76 non-UTF-8 `.lua` files, 5 non-UTF-8 `.toc`
//! files (which made those addons invisible to discovery entirely), and 160 files carrying a UTF-8
//! BOM. `AceAddon-2.0.lua` is one of them, and it is embedded in ~30 addons — one library file
//! taking out the whole Ace/FuBar half of the ecosystem.
//!
//! ## Two verbs, because there are two kinds of consumer
//!
//! - [`strip_bom`] + raw bytes → **Lua chunks**. Byte-transparent, exactly as the reference.
//!   `string.len()` on a literal returns what it returns there, and byte-indexing agrees.
//! - [`decode`] → **our own parsers** (FrameXML XML, `.toc`), which need `&str` because Rust
//!   parsers do. This is the only place a guess is made, and it is made where it is cheapest.
//!
//! **Why not transcode the Lua too, so everything downstream is UTF-8?** Because that would
//! silently change `string.len`, `string.sub` and `string.byte` on every non-ASCII literal — a
//! divergence from the verified mechanism, bought for a rendering convenience. The right place to
//! turn bytes into text is the boundary that actually requires text, which is a Rust binding
//! signature, not the file loader. A stray byte should cost one glyph, not the file.

use std::borrow::Cow;

/// The UTF-8 byte-order mark. Not an encoding — a three-byte signature some Windows editors write
/// in front of a UTF-8 file.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// `bytes` without a leading UTF-8 BOM.
///
/// **This is fidelity, not a modernisation** — and that is a correction to this module's first
/// draft, which reasoned that stock Lua 5.0's lexer meets `0xEF` and stops, so a BOM'd file must
/// have failed on the real client too, so stripping it was us being kinder than the original.
/// Reading the disassembly instead of the manual shows it: **Blizzard patched the strip into
/// `luaL_loadbuffer 0x6f5690` themselves.** Ten instructions stock Lua does not have
/// (`0x6f5699`–`0x6f56b4`): a `size >= 3` guard, three byte compares against `EF BB BF`, then `add
/// edx,3` / `sub eax,3` — all before the `LoadS` fill and `lua_load 0x6f4320`. That is the only
/// live door into the compiler, so addon files, FrameXML, XML script bodies and `loadstring` all
/// get it. The `.toc` reader has its own separate skip at `0x6edc71`.
///
/// The lexer reasoning was right and the conclusion drawn from it was wrong: `luaX_lex 0x6ff610`
/// *would* reject `0xEF`, which is precisely why the patch has to exist. A verified mechanism
/// beats a correct-sounding derivation, every time.
///
/// The edge semantics are the reference's, byte for byte: **at most one** mark, **only** at offset
/// 0, and a file shorter than three bytes is untouched (a BOM-only file compiles as an empty
/// chunk). A UTF-16 BOM (`FF FE` / `FE FF`) is left alone — the reference does not handle one
/// either, that file is not a Lua chunk in any encoding it could run, and transcoding it would be
/// inventing content rather than reading it. None exists in the corpus.
pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(BOM).unwrap_or(bytes)
}

/// `bytes` with a leading `#`-line removed — the **hash-bang skip**, which is a dialect difference
/// and not a convenience.
///
/// Lua 5.0 puts this skip in `luaX_setinput` (`0x6ff5cd`–`0x6ff600`), which every chunk goes
/// through: a source file whose first character is `#` loses its whole first line, including one
/// handed to `loadstring`. **Lua 5.1 moved it out** into `luaL_loadfile`, which mlua's
/// `Lua::load` does not call — so a 5.1-family VM silently keeps the line and then fails to
/// compile it.
///
/// Applied after the BOM strip, because that is the order the reference applies them in: the mark
/// is removed inside `luaL_loadbuffer`, and the lexer sees the shortened buffer.
pub fn strip_hashbang(bytes: &[u8]) -> &[u8] {
    match bytes.first() {
        Some(b'#') => match bytes.iter().position(|&b| b == b'\n') {
            // Lua 5.0 leaves the newline for the line counter; so do we.
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

/// Interpret UI source bytes as text, for the parsers that cannot take bytes.
///
/// Valid UTF-8 passes through **borrowed and unchanged** — which is every file we ship and the
/// overwhelming majority of every corpus, so the guess below is never reached for them.
///
/// Otherwise the bytes are decoded as **cp1252** (Windows-1252). That is a guess, and it is the
/// right one to make: the vanilla-era addons that are not UTF-8 are Western-European locale files
/// written on a Windows box — German and French `## Notes:` lines and `<Include>`d localisation
/// documents. cp1252 maps every byte (the five undefined slots become U+FFFD), so the decode
/// cannot fail and the addon cannot vanish. A GBK or EUC-KR file decodes to mojibake instead of
/// disappearing, which is a strictly better failure and one that is visible rather than silent.
///
/// The BOM is stripped first, so a BOM'd UTF-8 file is borrowed too.
pub fn decode(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = strip_bom(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| cp1252(b)).collect()),
    }
}

/// One cp1252 byte as a `char`.
///
/// cp1252 is Latin-1 (where a byte *is* its code point) except for `0x80..=0x9F`, which Latin-1
/// leaves as C1 control codes and Windows fills with the punctuation people actually type — the
/// curly quotes, the em dash, the euro sign. Getting that window wrong is what turns a French
/// addon's `l'objet` into a control character, so the window is spelled out rather than
/// approximated by a Latin-1 cast.
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

    /// A BOM'd file loses exactly three bytes, and a file without one is untouched.
    #[test]
    fn the_utf8_bom_is_removed_and_nothing_else_is() {
        assert_eq!(strip_bom(b"\xEF\xBB\xBFlocal x = 1"), b"local x = 1");
        assert_eq!(strip_bom(b"local x = 1"), b"local x = 1");
        // A lone 0xEF is not a BOM, and a truncated one is not either — neither may be eaten.
        assert_eq!(strip_bom(b"\xEF\xBB"), b"\xEF\xBB");
        assert_eq!(strip_bom(b"\xEF"), b"\xEF");
        assert_eq!(strip_bom(b""), b"");
        // A UTF-16 mark is left alone: that file is not a chunk we can honestly run.
        assert_eq!(strip_bom(b"\xFF\xFEl\0"), b"\xFF\xFEl\0");
    }

    /// Valid UTF-8 is borrowed, not rebuilt — the common path allocates nothing.
    #[test]
    fn valid_utf8_is_borrowed_unchanged() {
        let text = "## Title: Sch\u{e4}tze";
        assert!(matches!(decode(text.as_bytes()), Cow::Borrowed(s) if s == text));
        // ...including through a BOM.
        let bom = [BOM, text.as_bytes()].concat();
        assert_eq!(decode(&bom), text);
    }

    /// A cp1252 `.toc` line decodes to the glyphs its author typed, rather than to nothing.
    ///
    /// `0xE4` is `a`-umlaut in both Latin-1 and cp1252; `0x92` is the right single quote in cp1252
    /// and a C1 control in Latin-1, which is the difference this table exists for.
    #[test]
    fn cp1252_is_decoded_where_utf8_fails() {
        assert_eq!(decode(b"## Notes: Sch\xE4tze"), "## Notes: Sch\u{e4}tze");
        assert_eq!(decode(b"l\x92objet"), "l\u{2019}objet");
        assert_eq!(decode(b"\x80"), "\u{20AC}");
        // Undefined cp1252 slots survive as the replacement char — never as a decode failure.
        assert_eq!(decode(b"\x81"), "\u{FFFD}");
    }

    /// The `#`-line skip is Lua 5.0's, and it is a *dialect* fact rather than a nicety.
    ///
    /// `luaX_setinput 0x6ff5cd` eats it for every chunk the reference compiles, including one
    /// handed to `loadstring`. mlua's 5.1 does not, because 5.1 moved the skip into
    /// `luaL_loadfile` and `Lua::load` never calls that.
    #[test]
    fn a_leading_hash_line_is_eaten_the_way_lua_5_0_eats_it() {
        // The newline survives — 5.0 leaves it so the line counter stays honest.
        assert_eq!(strip_hashbang(b"#!/usr/bin/lua\nreturn 1"), b"\nreturn 1");
        // A `#` with no newline at all leaves nothing behind.
        assert_eq!(strip_hashbang(b"# only a comment"), b"");
        // Anything else is untouched — `#t` mid-expression is a length operator, not a shebang.
        assert_eq!(strip_hashbang(b"local n = #t"), b"local n = #t");
        assert_eq!(strip_hashbang(b""), b"");
        // ...and the two run in the reference's order: mark first, then the line.
        assert_eq!(chunk(b"\xEF\xBB\xBF#line\nreturn 1"), b"\nreturn 1");
    }

    /// The whole point, stated as a property: **no byte sequence makes a source file vanish**.
    #[test]
    fn every_byte_sequence_decodes_to_something() {
        let all: Vec<u8> = (0u8..=255).collect();
        let text = decode(&all);
        assert!(!text.is_empty());
        // ASCII survives verbatim, which is what a parser actually keys on.
        assert!(text.starts_with('\0'));
        assert!(text.contains("ABC"));
    }
}
