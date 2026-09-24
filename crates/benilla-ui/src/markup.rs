//! The 1.12 inline text markup (`|cAARRGGBB`, `|r`, `|H…|h…|h`, `|n`, `||`) and the edit box's
//! cursor law over it, one grammar for the renderer and the edit box. It transcribes the token
//! decoder `0x5c2810` ([`token_at`]: a byte remap at `0x5c2b10` into the jump table at `0x5c2af4`)
//! and the edit box's per-byte class array `E+0x330`, rebuilt by `0x77ba90` after every edit, with
//! the primitives that read it ([`ClassMap`]).
//!
//! 1.12 has no `|T…|t` texture escape: the remap claims only `c`, `h`, `n`, `r` in either case and
//! `|`, so `|TInterface\Icons\Foo:16:16|t` draws literally.
//!
//! The decoder's flags word can disable classes, but no code in the 1.12 client sets the bits for
//! `|c`, `|r`, `|H`, `|h` or `||` (`0x44d670` builds it from a font string's flags). Only `|n` can
//! be switched off, on a single-line edit box (`SetMultiLine`, `0x77a5e2`), and the class map
//! parses it regardless (`0x77ba90` passes 0), so that gate belongs to the renderer's line breaker.
//!
//! A cursor stands only on a token boundary with the adjacent zero-width escapes absorbed: after
//! any trailing `|r`/`|h`, before any leading `|c`/`|H` (`0x77bb30`). Every index here is a byte
//! offset on a UTF-8 character boundary.

use std::ops::Range;

// ── The colour a `|c` token carries ──────────────────────────────────────────────────────────

/// A `|cAARRGGBB` colour as the decoder packs it, `0xFFRRGGBB` (`0x5c2ab2..0x5c2ace`): the `AA` is
/// parsed and discarded, so `|c00ff0000` and `|cffff0000` are the same red.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgba(u32);

impl Rgba {
    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba(0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    pub const fn packed(self) -> u32 {
        self.0
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    pub const fn a(self) -> u8 {
        0xff
    }

    /// `[r, g, b, alpha]` in 0..1: the emitter patches the font string's own alpha over the decoded
    /// `0xff` (`0x5cceb0`, `0x5cceb6`), so a `|c` span fades with its string.
    pub fn to_f32_at(self, alpha: f32) -> [f32; 4] {
        [
            self.r() as f32 / 255.0,
            self.g() as f32 / 255.0,
            self.b() as f32 / 255.0,
            alpha,
        ]
    }
}

// ── Layer 1: the token decoder (`0x5c2810`) ──────────────────────────────────────────────────

/// The token class as `0x5c2810` numbers it, and as a class-map entry stores it (bits 16..23).
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TokenClass {
    /// `|cAARRGGBB`, 10 bytes.
    Color = 0,
    /// `|r` or `|R`, 2 bytes.
    ColorReset = 1,
    /// `\n`, `\r`, `\r\n`, `|n` or `|N`.
    LineBreak = 2,
    /// `||`, 2 bytes, drawing one `|`.
    EscapedPipe = 3,
    /// A hyperlink's whole `|H<payload>|h` prefix.
    LinkOpen = 4,
    /// `|h`, 2 bytes, closing a hyperlink.
    LinkClose = 5,
    /// An ordinary character, its UTF-8 length.
    Char = 6,
}

/// A token and what it carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind<'a> {
    Color(Rgba),
    /// Back to the string's base colour (`0x5cce99` restores `FontString+0x2c`).
    ColorReset,
    LineBreak,
    /// Draws one `|` (`0x5ccec3` looks up the glyph).
    EscapedPipe,
    LinkOpen {
        /// The text between `|H` and `|h`, such as `item:12345:0:0:0`.
        payload: &'a str,
    },
    LinkClose,
    Char(char),
}

impl TokenKind<'_> {
    pub const fn class(&self) -> TokenClass {
        match self {
            TokenKind::Color(_) => TokenClass::Color,
            TokenKind::ColorReset => TokenClass::ColorReset,
            TokenKind::LineBreak => TokenClass::LineBreak,
            TokenKind::EscapedPipe => TokenClass::EscapedPipe,
            TokenKind::LinkOpen { .. } => TokenClass::LinkOpen,
            TokenKind::LinkClose => TokenClass::LinkClose,
            TokenKind::Char(_) => TokenClass::Char,
        }
    }
}

/// One decoded token and its byte length, which `0x5c2810` returns beside the class (`*ioLen`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token<'a> {
    pub kind: TokenKind<'a>,
    /// At least 1.
    pub byte_len: usize,
}

impl Token<'_> {
    pub const fn class(&self) -> TokenClass {
        self.kind.class()
    }
}

/// Decode the token at byte offset `at`, a character boundary: `0x5c2810`. A `|` that opens
/// nothing well-formed is an ordinary character of length 1, as the remap's fall-through makes it.
pub fn token_at(text: &str, at: usize) -> Option<Token<'_>> {
    let rest = &text[at..];
    let b = rest.as_bytes();
    let first = *b.first()?;

    // `\r\n` is one class-2 token; a lone `\r` is one too.
    if first == b'\r' {
        let byte_len = if b.get(1) == Some(&b'\n') { 2 } else { 1 };
        return Some(Token {
            kind: TokenKind::LineBreak,
            byte_len,
        });
    }
    if first == b'\n' {
        return Some(Token {
            kind: TokenKind::LineBreak,
            byte_len: 1,
        });
    }

    if first == b'|' {
        // A lone `|`, or one leading anything the remap (`0x5c2b10`) does not claim.
        let literal_pipe = Token {
            kind: TokenKind::Char('|'),
            byte_len: 1,
        };
        return Some(match b.get(1) {
            Some(b'c' | b'C') => match parse_color(b) {
                Some(rgba) => Token {
                    kind: TokenKind::Color(rgba),
                    byte_len: 10,
                },
                // Fewer than 8 hex digits: not a colour token, so the `|` is just a `|`.
                None => literal_pipe,
            },
            Some(b'r' | b'R') => Token {
                kind: TokenKind::ColorReset,
                byte_len: 2,
            },
            Some(b'n' | b'N') => Token {
                kind: TokenKind::LineBreak,
                byte_len: 2,
            },
            Some(b'|') => Token {
                kind: TokenKind::EscapedPipe,
                byte_len: 2,
            },
            // Uppercase opens and lowercase closes, inferred: the remap sends both to one arm,
            // whose case test is untraced.
            Some(b'H') => match parse_link_open(rest) {
                Some((payload, byte_len)) => Token {
                    kind: TokenKind::LinkOpen { payload },
                    byte_len,
                },
                None => literal_pipe,
            },
            Some(b'h') => Token {
                kind: TokenKind::LinkClose,
                byte_len: 2,
            },
            _ => literal_pipe,
        });
    }

    let c = rest.chars().next().expect("non-empty");
    Some(Token {
        kind: TokenKind::Char(c),
        byte_len: c.len_utf8(),
    })
}

/// The 8 hex digits `AARRGGBB` after `|c`, alpha discarded; fewer leave the `|` literal.
fn parse_color(b: &[u8]) -> Option<Rgba> {
    let digits = b.get(2..10)?;
    let mut v: u32 = 0;
    for &d in digits {
        v = (v << 4) | (d as char).to_digit(16)?;
    }
    Some(Rgba::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The payload after `|H` and the length of the whole `|H<payload>|h` prefix; `None`, a literal
/// `|`, when no `|h` follows, when the payload is empty (`0x5c2972`) or when the visible text is
/// empty (`0x5c2992`). The delimiter scan is a plain byte search that does not skip `||`, inferred
/// from the emitter's own scan (`0x5ccdc8`, for the literal at `0x84453c`).
fn parse_link_open(rest: &str) -> Option<(&str, usize)> {
    let b = rest.as_bytes();
    let delim = (2..b.len().saturating_sub(1)).find(|&i| b[i] == b'|' && b[i + 1] == b'h')?;
    let span = delim + 2;
    if span == 4 {
        return None;
    }
    if b.get(span) == Some(&b'|') && b.get(span + 1) == Some(&b'h') {
        return None;
    }
    Some((&rest[2..delim], span))
}

/// Every token of `text` with the byte offset it starts at.
pub fn tokens(text: &str) -> Tokens<'_> {
    Tokens { text, at: 0 }
}

/// The iterator [`tokens`] returns.
#[derive(Clone, Debug)]
pub struct Tokens<'a> {
    text: &'a str,
    at: usize,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = (usize, Token<'a>);

    fn next(&mut self) -> Option<(usize, Token<'a>)> {
        let token = token_at(self.text, self.at)?;
        let at = self.at;
        self.at += token.byte_len;
        Some((at, token))
    }
}

// ── Layer 2: the per-byte class map (`E+0x330`, built by `0x77ba90`) ─────────────────────────

/// One class-map entry, the client's dword unpacked: byte length in bits 0..15, class in 16..23,
/// "inside a hyperlink" in bit 31. Only a token's first byte is stored (`0x77bb0f`); continuation
/// bytes and the terminator are zero, so they read as no class and not in a link.
///
/// Deviation: the length keeps every bit, where the client masks it to 16 (`0x77baf3`), because a
/// wrapped length would put the cursor inside an `|H` span of 64 KiB or more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    byte_len: u32,
    class: Option<TokenClass>,
    in_link: bool,
}

impl Entry {
    /// A continuation byte, or the terminator: the client's zeroed dword.
    pub const CONTINUATION: Entry = Entry {
        byte_len: 0,
        class: None,
        in_link: false,
    };

    pub const fn byte_len(&self) -> usize {
        self.byte_len as usize
    }

    pub const fn class(&self) -> Option<TokenClass> {
        self.class
    }

    /// Bit 31: set from a link's `|H` through its closing `|h`, both included.
    pub const fn in_link(&self) -> bool {
        self.in_link
    }

    pub const fn is_token_start(&self) -> bool {
        self.byte_len != 0
    }
}

/// The edit box's per-byte class array (`E+0x330`) and the cursor primitives over it: one entry
/// per byte plus a zero terminator (`0x77c670`). It holds no text; the edit box rebuilds it after
/// every edit, as the client calls `0x77ba90` at the end of each insert (`0x77c022`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClassMap {
    entries: Vec<Entry>,
}

impl ClassMap {
    /// `0x77ba90`, which parses every class, `|n` included, whatever the font string's flags
    /// (`K = 0` at `0x77bad9`).
    pub fn new(text: &str) -> ClassMap {
        let mut entries = vec![Entry::CONTINUATION; text.len() + 1];
        // The link bit rises before the store on an open (`0x77baec`) and falls after it on a
        // close (`0x77bb1e`), so the closing `|h` is tagged and the byte after it is not.
        let mut in_link = false;
        for (at, token) in tokens(text) {
            let class = token.class();
            if class == TokenClass::LinkOpen {
                in_link = true;
            }
            entries[at] = Entry {
                byte_len: token.byte_len as u32,
                class: Some(class),
                in_link,
            };
            if class == TokenClass::LinkClose {
                in_link = false;
            }
        }
        ClassMap { entries }
    }

    pub fn text_len(&self) -> usize {
        self.entries.len() - 1
    }

    /// `at == text_len()` is the terminator; past it panics.
    pub fn entry(&self, at: usize) -> Entry {
        self.entries[at]
    }

    /// The next token's start, `0x77bd10`: a fixed point where the length is 0, as the client's
    /// unguarded add is, so a forward walk must test `at < text_len()`.
    pub fn next_token(&self, at: usize) -> usize {
        at + self.entry(at).byte_len()
    }

    /// The previous token's start, `0x77bd30`; `None`, the client's `-1`, at offset 0.
    pub fn prev_token(&self, at: usize) -> Option<usize> {
        let mut i = at.checked_sub(1)?;
        while !self.entry(i).is_token_start() {
            i = i.checked_sub(1)?;
        }
        Some(i)
    }

    /// The letters in `byte_count` bytes from `start`, `0x77bc80`, a per-byte scan that counts
    /// only classes 2, 3 and 6 (`0x77bcb0`): `GetNumLetters` and `SetMaxLetters` see a 48-byte
    /// item link as its 14 visible letters. A token starting inside the budget counts whole.
    pub fn letters(&self, start: usize, byte_count: usize) -> usize {
        let end = start.saturating_add(byte_count).min(self.text_len());
        let mut at = start;
        let mut letters = 0;
        while at < end {
            let entry = self.entry(at);
            let Some(class) = entry.class() else { break };
            if matches!(
                class,
                TokenClass::LineBreak | TokenClass::EscapedPipe | TokenClass::Char
            ) {
                letters += 1;
            }
            at += entry.byte_len();
        }
        letters
    }

    /// The whole string's letter count, `GetNumLetters`.
    pub fn num_letters(&self) -> usize {
        self.letters(0, self.text_len())
    }

    /// The byte offset `steps` token steps from `from`, backward when negative: `0x77bb30`, whose
    /// caller `0x77c6b0` signs its result. `atomic_links` is the keyboard's walk (arrows, word
    /// jumps, HOME/END, BACKSPACE, DELETE), crossing a whole `|H…|h[text]|h` in one step; click,
    /// drag, UP/DOWN, the scroll-window sizing and the IME span stop on each visible character.
    ///
    /// Deviation: a step that runs out of buffer while skipping lands on the far end, because the
    /// client spins forever there (its skip loop re-enters at `0x77bb60`, past the zero check at
    /// `0x77bb56`), so `SetText("|cffffffff")` and one RIGHT hang 1.12.
    pub fn advance(&self, from: usize, steps: isize, atomic_links: bool) -> usize {
        let mut at = from;
        for _ in 0..steps.unsigned_abs() {
            at = if steps > 0 {
                self.step_forward(at, atomic_links)
            } else {
                self.step_back(at, atomic_links)
            };
        }
        at
    }

    /// One step forward: the skip loop at `0x77bb60`, then the absorb at `0x77bb98`.
    fn step_forward(&self, from: usize, atomic_links: bool) -> usize {
        let end = self.text_len();
        let mut at = from;
        while at < end {
            let entry = self.entry(at);
            let Some(class) = entry.class() else { break };
            at += entry.byte_len();
            // `|c` and `|H…|h` are skipped at either atomicity (`0x77bb84`, `0x77bb86`).
            if matches!(class, TokenClass::Color | TokenClass::LinkOpen) {
                continue;
            }
            if !atomic_links {
                break; // one visible token consumed
            }
            if !entry.in_link() {
                break; // bit 31 clear
            }
            if class == TokenClass::LinkClose {
                break; // the close ends the atomic skip
            }
            // Inside a link and not its close: keep skipping.
        }
        // Then absorb any immediately following `|r` / `|h`, so the cursor lands past them.
        while at < end
            && matches!(
                self.entry(at).class(),
                Some(TokenClass::ColorReset | TokenClass::LinkClose)
            )
        {
            at = self.next_token(at);
        }
        at
    }

    /// One step backward, the mirror (`0x77bc13`, `0x77bc1d`, `0x77bc59`).
    fn step_back(&self, from: usize, atomic_links: bool) -> usize {
        let mut at = from;
        // Trailing `|r` and `|h` first.
        while let Some(prev) = self.prev_token(at) {
            match self.entry(prev).class() {
                Some(TokenClass::ColorReset | TokenClass::LinkClose) => at = prev,
                _ => break,
            }
        }
        // One token back, extended when atomic over the whole link to its open.
        while let Some(prev) = self.prev_token(at) {
            let entry = self.entry(prev);
            at = prev;
            if !atomic_links || !entry.in_link() || entry.class() == Some(TokenClass::LinkOpen) {
                break;
            }
        }
        // Then back over any preceding `|c` or `|H`.
        while let Some(prev) = self.prev_token(at) {
            match self.entry(prev).class() {
                Some(TokenClass::Color | TokenClass::LinkOpen) => at = prev,
                _ => break,
            }
        }
        at
    }

    /// Widen a deletion range so it never cuts a hyperlink: `0x77c510`, for BACKSPACE and DELETE
    /// (`0x77c280`), delete-selection (`0x77cd70`) and Clear (`0x77c500`). Touching any byte of a
    /// link removes the whole `|c…|H…|h[text]|h|r` unit. The endpoint tests read the entries at
    /// `start` and `end` (`0x77c543`, `0x77c59d`), so an end that only abuts a link's `|H` still
    /// widens, and the backward walk also stops on a `|c` inside the link text (`0x77c550`).
    pub fn snap_delete_range(&self, range: Range<usize>) -> Range<usize> {
        let Range {
            start: mut lo,
            end: mut hi,
        } = range;

        if self.entry(lo).in_link() {
            while !matches!(
                self.entry(lo).class(),
                Some(TokenClass::Color | TokenClass::LinkOpen)
            ) {
                match self.prev_token(lo) {
                    Some(prev) => lo = prev,
                    None => break,
                }
            }
            while let Some(prev) = self.prev_token(lo) {
                match self.entry(prev).class() {
                    Some(TokenClass::Color | TokenClass::LinkOpen) => lo = prev,
                    _ => break,
                }
            }
        }

        if self.entry(hi).in_link() {
            while hi < self.text_len() && self.entry(hi).class() != Some(TokenClass::LinkClose) {
                hi = self.next_token(hi);
            }
            while hi < self.text_len()
                && matches!(
                    self.entry(hi).class(),
                    Some(TokenClass::ColorReset | TokenClass::LinkClose)
                )
            {
                hi = self.next_token(hi);
            }
        }

        lo..hi
    }

    /// The opening guard of `0x77bee0`: refused when the entry at the cursor and the previous
    /// token's both carry bit 31 (`0x77befb`, `0x77bf0d`), so a link's leading edge still accepts.
    /// Only the mouse (`0x77d0d0`) reaches a cursor inside a link, and the client then drops the
    /// typing (inferred from the guard).
    pub fn insert_allowed(&self, at: usize) -> bool {
        if !self.entry(at).in_link() {
            return true;
        }
        match self.prev_token(at) {
            // Cursor 0 proceeds.
            None => true,
            Some(prev) => !self.entry(prev).in_link(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An epic item link as `0x52adb0` formats one.
    ///
    /// ```text
    ///  0        10                  30            44  46
    ///  |cffa335ee|Hitem:12345:0:0:0|h[Chipped Claw]|h|r
    ///  \--10---/ \-------20-------/ \-----14-----/ \2/\2/
    /// ```
    const LINK: &str = "|cffa335ee|Hitem:12345:0:0:0|h[Chipped Claw]|h|r";

    /// The same link between ordinary text, every offset shifted by 2.
    const AROUND: &str = "ab|cffa335ee|Hitem:12345:0:0:0|h[Chipped Claw]|h|rcd";

    fn kinds(text: &str) -> Vec<(usize, TokenKind<'_>, usize)> {
        tokens(text)
            .map(|(at, t)| (at, t.kind, t.byte_len))
            .collect()
    }

    // ── Layer 1: the decoder ─────────────────────────────────────────────────────────────────

    fn one(text: &str) -> Token<'_> {
        token_at(text, 0).expect("a token")
    }

    #[test]
    fn each_of_the_seven_classes_is_recognised_at_its_own_byte_length() {
        let c = one("|cffa335ee");
        assert_eq!(c.class(), TokenClass::Color);
        assert_eq!(c.byte_len, 10);
        assert_eq!(c.kind, TokenKind::Color(Rgba::from_rgb(0xa3, 0x35, 0xee)));

        for reset in ["|r", "|R"] {
            assert_eq!(one(reset).kind, TokenKind::ColorReset);
            assert_eq!(one(reset).byte_len, 2);
        }

        // Every spelling of class 2, `\r\n` as one token.
        for (text, len) in [("\n", 1), ("\r", 1), ("\r\n", 2), ("|n", 2), ("|N", 2)] {
            assert_eq!(one(text).kind, TokenKind::LineBreak, "{text:?}");
            assert_eq!(one(text).byte_len, len, "{text:?}");
        }

        assert_eq!(one("||").kind, TokenKind::EscapedPipe);
        assert_eq!(one("||").byte_len, 2);

        let open = one("|Hitem:12345:0:0:0|h[Chipped Claw]|h");
        assert_eq!(
            open.kind,
            TokenKind::LinkOpen {
                payload: "item:12345:0:0:0"
            }
        );
        assert_eq!(
            open.byte_len, 20,
            "the whole |H<payload>|h prefix, not just |H"
        );

        assert_eq!(one("|h").kind, TokenKind::LinkClose);
        assert_eq!(one("|h").byte_len, 2);

        assert_eq!(one("a").kind, TokenKind::Char('a'));
        assert_eq!(one("a").byte_len, 1);
        assert_eq!(one("é").kind, TokenKind::Char('é'));
        assert_eq!(one("é").byte_len, 2);
        assert_eq!(one("✚").byte_len, 3);

        assert_eq!(token_at("", 0), None);
        assert_eq!(token_at("ab", 2), None);
    }

    #[test]
    fn a_pipe_leading_anything_unclaimed_is_an_ordinary_character() {
        // The remap table at 0x5c2b10 claims only C/H/N/R and their lowercase siblings.
        for text in ["|x", "|", "| ", "|1"] {
            let t = token_at(text, 0).expect("a token");
            assert_eq!(t.kind, TokenKind::Char('|'), "{text:?}");
            assert_eq!(t.byte_len, 1, "{text:?}");
        }
    }

    #[test]
    fn there_is_no_inline_texture_escape_in_1_12_1() {
        // `|T…|t` is a later client's escape; in 1.12 every byte of it is an ordinary character.
        let text = "|TInterface\\Icons\\Foo:16:16|t";
        let first = token_at(text, 0).expect("a token");
        assert_eq!(first.kind, TokenKind::Char('|'));
        assert_eq!(first.byte_len, 1);
        assert_eq!(
            token_at(text, 1).expect("a token").kind,
            TokenKind::Char('T')
        );
        assert!(tokens(text).all(|(_, t)| t.class() == TokenClass::Char));
    }

    #[test]
    fn a_malformed_colour_escape_is_an_ordinary_character() {
        for text in ["|cffzz0000", "|cff", "|c", "|cffff000"] {
            let t = token_at(text, 0).expect("a token");
            assert_eq!(t.kind, TokenKind::Char('|'), "{text:?}");
            assert_eq!(t.byte_len, 1, "{text:?}");
        }
    }

    #[test]
    fn a_colour_escapes_alpha_is_parsed_and_then_discarded() {
        let transparent = token_at("|c00ff0000", 0).expect("a token").kind;
        let opaque = token_at("|cffff0000", 0).expect("a token").kind;
        assert_eq!(
            transparent, opaque,
            "the AA nibbles cannot reach the colour"
        );
        assert_eq!(transparent, TokenKind::Color(Rgba::from_rgb(0xff, 0, 0)));
        let TokenKind::Color(rgba) = transparent else {
            panic!("a colour")
        };
        assert_eq!(rgba.a(), 0xff);
        assert_eq!(rgba.packed(), 0xffff_0000);
        // The string's alpha reaches the vertex, never the escape's (`0x5cceb0`).
        assert_eq!(rgba.to_f32_at(1.0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(rgba.to_f32_at(0.5), [1.0, 0.0, 0.0, 0.5]);
    }

    #[test]
    fn a_link_open_degrades_to_a_literal_pipe_in_the_three_cases() {
        let degraded = |text: &str| {
            let t = token_at(text, 0).expect("a token");
            assert_eq!(t.kind, TokenKind::Char('|'), "{text:?}");
            assert_eq!(t.byte_len, 1, "{text:?}");
        };
        // 1. No closing `|h` anywhere after it.
        degraded("|Hitem:12345:0:0:0[Chipped Claw]");
        degraded("|H");
        // 2. An empty payload (0x5c2972).
        degraded("|H|h[Chipped Claw]|h");
        // 3. Empty visible text: another `|h` follows at once (0x5c2992).
        degraded("|Hitem:12345:0:0:0|h|h");

        // Near misses still open: one visible character is enough, and the stateless decoder
        // opens a link that never closes.
        assert_eq!(
            token_at("|Hitem:1|h[|h", 0).expect("a token").kind,
            TokenKind::LinkOpen { payload: "item:1" }
        );
        assert_eq!(
            token_at("|Hitem:1|h[Chipped Claw]", 0)
                .expect("a token")
                .kind,
            TokenKind::LinkOpen { payload: "item:1" }
        );
    }

    #[test]
    fn the_token_stream_covers_the_whole_string_exactly_once() {
        let mut next = 0;
        for (at, token) in tokens(AROUND) {
            assert_eq!(at, next, "no gap, no overlap");
            assert!(token.byte_len >= 1);
            assert!(AROUND.is_char_boundary(at));
            next = at + token.byte_len;
        }
        assert_eq!(next, AROUND.len());

        // The link's shape: every index the other tests cite.
        let ks = kinds(LINK);
        assert_eq!(ks[0].0, 0);
        assert_eq!(ks[0].2, 10);
        assert_eq!(
            ks[1],
            (
                10,
                TokenKind::LinkOpen {
                    payload: "item:12345:0:0:0"
                },
                20
            )
        );
        assert_eq!(ks[2], (30, TokenKind::Char('['), 1));
        assert_eq!(ks[16], (44, TokenKind::LinkClose, 2));
        assert_eq!(ks[17], (46, TokenKind::ColorReset, 2));
        assert_eq!(ks.len(), 18);
        assert_eq!(LINK.len(), 48);
    }

    // ── Layer 2: the class map ───────────────────────────────────────────────────────────────

    #[test]
    fn the_link_bit_covers_the_open_through_the_closing_pipe_h_and_stops_there() {
        let map = ClassMap::new(LINK);
        for at in 0..=map.text_len() {
            let entry = map.entry(at);
            // Set from the open at 10 through the close at 44; the `|r` at 46 is outside.
            let expected = entry.is_token_start() && (10..=44).contains(&at);
            assert_eq!(entry.in_link(), expected, "bit 31 at {at}");
        }
        assert!(map.entry(10).in_link(), "the |H open itself");
        assert!(map.entry(44).in_link(), "the closing |h");
        assert!(!map.entry(46).in_link(), "the trailing |r is outside");
        assert!(!map.entry(0).in_link(), "the leading |c is outside");
        assert!(!map.entry(48).in_link(), "the terminator");
    }

    #[test]
    fn a_continuation_byte_and_the_terminator_are_the_zero_entry() {
        let map = ClassMap::new(LINK);
        // Every byte inside the |H…|h span but its first.
        for at in 11..30 {
            assert_eq!(map.entry(at), Entry::CONTINUATION, "byte {at}");
            assert_eq!(map.entry(at).class(), None);
            assert!(!map.entry(at).is_token_start());
        }
        assert_eq!(map.entry(map.text_len()), Entry::CONTINUATION);
        assert_eq!(ClassMap::new("").text_len(), 0);
        assert_eq!(ClassMap::new("").entry(0), Entry::CONTINUATION);
    }

    #[test]
    fn next_and_prev_token_walk_whole_tokens() {
        let map = ClassMap::new(LINK);
        assert_eq!(map.next_token(0), 10);
        assert_eq!(map.next_token(10), 30);
        assert_eq!(map.next_token(44), 46);
        assert_eq!(map.next_token(46), 48);
        assert_eq!(map.next_token(48), 48);

        assert_eq!(map.prev_token(48), Some(46));
        assert_eq!(map.prev_token(46), Some(44));
        assert_eq!(map.prev_token(30), Some(10));
        // From anywhere inside the |H span's continuation bytes, back to its start.
        assert_eq!(map.prev_token(20), Some(10));
        assert_eq!(map.prev_token(10), Some(0));
        assert_eq!(map.prev_token(0), None);
    }

    #[test]
    fn letters_counts_classes_two_three_and_six_and_nothing_else() {
        // The whole 48-byte link is 14 letters: `[Chipped Claw]`, the escapes free.
        let map = ClassMap::new(LINK);
        assert_eq!(map.num_letters(), 14);
        assert_eq!(map.letters(0, 10), 0, "the |c alone");
        assert_eq!(map.letters(0, 30), 0, "the |c and the |H…|h");
        assert_eq!(map.letters(30, 14), 14, "the visible text alone");
        assert_eq!(map.letters(44, 4), 0, "the |h|r tail");

        // A line break and an escaped pipe are one letter each; `\r\n` is one.
        let map = ClassMap::new("a||b\r\nc|nd");
        assert_eq!(map.num_letters(), 7);
        assert_eq!(ClassMap::new(AROUND).num_letters(), 18, "14 + `ab` + `cd`");
        assert_eq!(ClassMap::new("").num_letters(), 0);
    }

    // ── The cursor law ───────────────────────────────────────────────────────────────────────

    /// The reachable sets the reference's `0x77bb30` produced, executed on this buffer: 30, between
    /// `|h` and `[`, is reachable in neither mode, and after the name the stop is 43 in both, the
    /// trailing `|h|r` absorbing forward.
    #[test]
    fn the_reachable_sets_match_the_emulation_oracle() {
        // `[` at 30, `]` at 38, the `d` of the typed text at 43; 51 bytes.
        const BUF: &str = "|cffa335ee|Hitem:11684:0:0:0|h[Ironfoe]|h|rdsfsdfsd";
        let map = ClassMap::new(BUF);

        for (atomic, expected) in [
            (true, vec![0, 43, 44, 45, 46, 47, 48, 49, 50, 51]),
            (
                false,
                vec![
                    0, 31, 32, 33, 34, 35, 36, 37, 38, 43, 44, 45, 46, 47, 48, 49, 50, 51,
                ],
            ),
        ] {
            let mut forward = vec![0usize];
            let mut at = 0;
            while at < BUF.len() {
                let next = map.advance(at, 1, atomic);
                assert!(next > at, "forward walk stalled at {at} (atomic={atomic})");
                forward.push(next);
                at = next;
            }
            assert_eq!(forward, expected, "forward, atomic={atomic}");

            let mut back = vec![BUF.len()];
            let mut at = BUF.len();
            while at > 0 {
                let prev = map.advance(at, -1, atomic);
                assert!(prev < at, "backward walk stalled at {at} (atomic={atomic})");
                back.push(prev);
                at = prev;
            }
            back.reverse();
            assert_eq!(back, expected, "backward, atomic={atomic}");
        }

        // Insertion is refused at exactly 30..=39, the text through the first byte of the closing
        // `|h`; 40 is that token's zero interior slot.
        let refused: Vec<usize> = (0..=BUF.len())
            .filter(|&i| !map.insert_allowed(i))
            .collect();
        assert_eq!(refused, (30..=39).collect::<Vec<_>>());
    }

    /// A buffer ending in `|c`, or in an unclosed link when atomic, where the client spins forever.
    #[test]
    fn a_trailing_escape_lands_on_the_end_where_the_client_would_hang() {
        // (buffer, offset where only skip-class tokens remain, whether the client hangs non-atomic)
        for (buf, from, both_modes) in [
            ("|cffffffff", 0, true),
            ("ab|cffffffff", 2, true),
            ("|Hitem:1|h[Unclosed", 0, false),
        ] {
            let map = ClassMap::new(buf);
            assert_eq!(map.advance(from, 1, true), buf.len(), "{buf:?} atomic");
            if both_modes {
                assert_eq!(map.advance(from, 1, false), buf.len(), "{buf:?} non-atomic");
            }
        }
    }

    #[test]
    fn one_atomic_step_crosses_the_entire_item_link() {
        let map = ClassMap::new(LINK);
        assert_eq!(
            map.advance(0, 1, true),
            48,
            "past the whole link, not into it"
        );
        assert_eq!(map.advance(48, -1, true), 0, "and back again");
        // The atomic walk has exactly two stops in the 48 bytes.
        assert_eq!(map.advance(0, 5, true), 48, "clamped at the end");
        assert_eq!(map.advance(48, -5, true), 0);

        // With text on both sides, the link is still one step, and the plain text is not.
        let map = ClassMap::new(AROUND);
        assert_eq!(map.advance(0, 1, true), 1);
        assert_eq!(map.advance(1, 1, true), 2);
        assert_eq!(map.advance(2, 1, true), 50, "the whole |c…|H…|h|r unit");
        assert_eq!(map.advance(50, 1, true), 51);
        assert_eq!(map.advance(52, -1, true), 51);
        assert_eq!(map.advance(50, -1, true), 2, "back over the whole unit");
    }

    /// The mouse path (`0x77d2f6`): every visible character is a stop, and no index inside
    /// `|cffa335ee`, `|Hitem:…|h`, `|h` or `|r` ever is.
    #[test]
    fn a_non_atomic_walk_stops_on_each_visible_character_and_never_inside_an_escape() {
        let map = ClassMap::new(LINK);

        let mut at = 0;
        let mut stops = vec![at];
        while at < map.text_len() {
            let next = map.advance(at, 1, false);
            assert_ne!(next, at, "the walk must make progress");
            at = next;
            stops.push(at);
        }
        // After `[`, after each of the 12 letters of `Chipped Claw`, then straight past `]|h|r`.
        let expected: Vec<usize> = std::iter::once(0).chain(31..=43).chain([48]).collect();
        assert_eq!(stops, expected);

        let mut at = map.text_len();
        let mut back = vec![at];
        while at > 0 {
            at = map.advance(at, -1, false);
            back.push(at);
        }
        back.reverse();
        assert_eq!(back, expected);

        for &stop in &stops {
            assert!(
                !(1..10).contains(&stop)
                    && !(11..30).contains(&stop)
                    && stop != 45
                    && stop != 47
                    && stop != 10
                    && stop != 30
                    && stop != 44
                    && stop != 46,
                "{stop} is inside an escape"
            );
        }
    }

    /// The reachable set (`0x77bb30`): boundaries not before a `|r`/`|h` nor after a `|c`/`|H`.
    fn reachable_set(text: &str) -> Vec<usize> {
        let map = ClassMap::new(text);
        (0..=map.text_len())
            .filter(|&at| {
                let entry = map.entry(at);
                if !(at == map.text_len() || entry.is_token_start()) {
                    return false; // not a token boundary at all
                }
                // A trailing `|r`/`|h` must have been absorbed, so you are never before one.
                if matches!(
                    entry.class(),
                    Some(TokenClass::ColorReset | TokenClass::LinkClose)
                ) {
                    return false;
                }
                // A leading `|c`/`|H` must have been skipped, so you are never after one.
                !matches!(
                    map.prev_token(at).map(|p| map.entry(p).class()),
                    Some(Some(TokenClass::Color | TokenClass::LinkOpen))
                )
            })
            .collect()
    }

    /// Any walk, either way, at either atomicity, lands only in the reachable set, and the walks
    /// reach all of it.
    #[test]
    fn every_index_any_walk_can_reach_is_a_boundary_with_its_escapes_absorbed() {
        for text in [LINK, AROUND] {
            let map = ClassMap::new(text);
            let canonical = reachable_set(text);

            let mut seen = vec![0, map.text_len()];
            let mut frontier = seen.clone();
            while let Some(at) = frontier.pop() {
                for atomic in [false, true] {
                    for step in [-1, 1] {
                        let to = map.advance(at, step, atomic);
                        assert!(
                            text.is_char_boundary(to),
                            "{text:?}: {at} -{step}/{atomic} landed mid-character at {to}"
                        );
                        assert!(
                            canonical.contains(&to),
                            "{text:?}: {at} -{step}/{atomic} landed at {to}, outside the \
                             reachable set {canonical:?}"
                        );
                        if !seen.contains(&to) {
                            seen.push(to);
                            frontier.push(to);
                        }
                    }
                }
            }
            seen.sort_unstable();
            assert_eq!(
                seen, canonical,
                "{text:?}: the non-atomic walk reaches all of it"
            );
        }
    }

    /// A step that runs out of buffer while skipping lands on the far end, the deviation on
    /// [`ClassMap::advance`].
    #[test]
    fn a_step_that_runs_out_of_buffer_lands_on_the_far_end() {
        let map = ClassMap::new("ab|cffff0000");
        assert_eq!(
            map.advance(2, 1, true),
            12,
            "nothing follows the |c to stop on"
        );
        assert_eq!(map.advance(12, -1, true), 2);
        let map = ClassMap::new("|rab");
        assert_eq!(map.advance(2, -1, true), 0, "nothing precedes the |r");
        assert_eq!(map.advance(0, 1, true), 2);
        let map = ClassMap::new(LINK);
        assert_eq!(map.advance(0, -1, true), 0);
        assert_eq!(map.advance(48, 1, true), 48);
        assert_eq!(map.advance(31, 0, false), 31, "zero steps is the identity");
    }

    // ── Deletion and insertion ───────────────────────────────────────────────────────────────

    #[test]
    fn deleting_any_byte_of_a_link_widens_to_the_whole_unit() {
        let map = ClassMap::new(AROUND);
        // Touching the visible text takes the whole `|c…|r` unit, leaving `abcd`.
        for range in [33..38, 32..46, 33..34, 2..40, 12..46] {
            let snapped = map.snap_delete_range(range.clone());
            assert_eq!(snapped, 2..50, "{range:?}");
            let mut left = AROUND.to_owned();
            left.replace_range(snapped, "");
            assert_eq!(left, "abcd");
        }
        // A selection that touches no link byte is left exactly as it was.
        for range in [0..2, 0..0, 50..52, 51..52, 2..2] {
            assert_eq!(map.snap_delete_range(range.clone()), range, "{range:?}");
        }
        // Only one endpoint inside: the other stays put.
        assert_eq!(map.snap_delete_range(0..35), 0..50);
        assert_eq!(map.snap_delete_range(35..52), 2..52);
    }

    /// An exclusive end on the link's first tagged byte still widens (`0x77c59d`).
    #[test]
    fn a_deletion_ending_on_the_links_open_still_takes_the_link() {
        let map = ClassMap::new(AROUND);
        assert_eq!(map.snap_delete_range(0..12), 0..50);
        // Ending on the untagged `|c` does not.
        assert_eq!(map.snap_delete_range(0..2), 0..2);
    }

    #[test]
    fn insertion_is_refused_strictly_inside_a_links_visible_text() {
        let map = ClassMap::new(LINK);
        assert!(map.insert_allowed(0), "before the leading |c");
        assert!(map.insert_allowed(46), "between the |h and the |r");
        assert!(map.insert_allowed(48), "at the end of the buffer");
        // The leading edge: the |H entry carries bit 31, its predecessor the |c does not.
        assert!(map.insert_allowed(10), "at the link's leading edge");
        // Refused from the first visible character through the tagged closing |h.
        for at in 30..=44 {
            assert!(!map.insert_allowed(at), "inside the link at {at}");
        }
        // Including every position a mouse click can produce.
        for at in reachable_set(LINK) {
            assert_eq!(
                map.insert_allowed(at),
                !(31..=43).contains(&at),
                "reachable position {at}"
            );
        }

        let map = ClassMap::new(AROUND);
        for at in [0, 1, 2, 50, 51, 52] {
            assert!(map.insert_allowed(at), "outside the link at {at}");
        }
    }

    // ── UTF-8 ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn no_primitive_can_return_an_index_inside_a_character() {
        let text = "héllo |cffa335ee|Hitem:1:0:0:0|h[Épée ✚]|h|r né";
        let map = ClassMap::new(text);
        assert_eq!(map.text_len(), text.len());

        for (at, token) in tokens(text) {
            assert!(text.is_char_boundary(at), "token start {at}");
            assert!(text.is_char_boundary(at + token.byte_len), "token end {at}");
        }
        for at in 0..=map.text_len() {
            if !map.entry(at).is_token_start() && at != map.text_len() {
                continue;
            }
            for atomic in [false, true] {
                for step in [-1isize, 1] {
                    let to = map.advance(at, step, atomic);
                    assert!(
                        text.is_char_boundary(to),
                        "advance({at},{step},{atomic}) -> {to}"
                    );
                }
            }
            assert!(text.is_char_boundary(map.next_token(at)));
            if let Some(prev) = map.prev_token(at) {
                assert!(text.is_char_boundary(prev));
            }
            let snapped = map.snap_delete_range(at..map.text_len());
            assert!(text.is_char_boundary(snapped.start));
            assert!(text.is_char_boundary(snapped.end));
            // `replace_range` panics on a range that cuts a character.
            let mut left = text.to_owned();
            left.replace_range(snapped, "");
        }

        // One letter per character: `héllo ` 6, `[Épée ✚]` 8, ` né` 3.
        assert_eq!(map.num_letters(), 17);
        // One atomic step still crosses the whole link.
        let open = text.find("|cffa335ee").expect("the colour push");
        let after = text.find("|r").expect("the reset") + 2;
        assert_eq!(map.advance(open, 1, true), after);
        assert_eq!(map.advance(after, -1, true), open);
    }
}
