//! WoW's inline text markup — the grammar every 1.12.1 string carries (`|cAARRGGBB`, `|r`,
//! `|H…|h…|h`, `|n`, `||`), and the cursor law an edit box builds on top of it.
//!
//! Pure: `&str` in, indices out. No Lua, no Bevy, no engine types, no I/O — so the renderer and the
//! edit box can share **one** owner of the grammar instead of drifting apart on where an escape
//! starts and ends.
//!
//! ## Ground truth
//!
//! Two client mechanisms are transcribed here, and every claim below carries its address:
//!
//! - **`0x5c2810`** — the token decoder. Given a pointer it
//!   returns a class 0..6 and the token's byte length, dispatching off a byte remap at `0x5c2b10`
//!   into the 7-arm jump table at `0x5c2af4`. That is [`token_at`] / [`tokens`].
//! - **`0x77ba90`** — the edit box's per-byte class array `E+0x330`, rebuilt in full after every
//!   mutation, plus the five primitives that read it (`0x77bd10`, `0x77bd30`, `0x77bc80`,
//!   `0x77bb30`, `0x77c510`, `0x77bee0`). That is [`ClassMap`].
//!
//! ## The seven classes — the client's own numbering
//!
//! - **0** `|cAARRGGBB` — 10 bytes. The `AA` is parsed and then *discarded*: `0x5c2ab2–0x5c2ace`
//!   builds `0xFF << 24 | RR << 16 | GG << 8 | BB`. See [`Rgba`].
//! - **1** `|r` / `|R` — 2 bytes.
//! - **2** `\n` · `\r` · `\r\n` · `|n` / `|N` — 1 or 2 bytes.
//! - **3** `||` — 2 bytes, draws one literal `|`.
//! - **4** hyperlink OPEN — the **entire** `|H<payload>|h` prefix, however long.
//! - **5** `|h` close — 2 bytes.
//! - **6** an ordinary character — its UTF-8 length. Everything the remap doesn't claim lands here,
//!   *including* a `|` followed by anything outside `{c C h H n N r R |}`.
//!
//! **1.12.1 has no `|T…|t` inline-texture escape at all.** The remap table at `0x5c2b10` (index =
//! `ch - 'C'`) sends every byte but `C`/`H`/`N`/`R` and their lowercase siblings to the
//! ordinary-character arm, so `|TInterface\Icons\Foo:16:16|t` draws literally, pipe and all. Inline
//! textures are a later-expansion feature; this module must not invent one.
//!
//! ## What the parse gates do *not* do
//!
//! `0x5c2810` takes a flags word `K` that can disable classes, and `0x44d670` translates the
//! FontString's `F` into it. An origination census over the whole image found **no writer anywhere**
//! for the bits behind `K & 0x100` / `0x400` / `0x800`, so `|c`, `|r`, `|H`, `|h` and `||` are parsed
//! unconditionally by every `CSimpleFontString` in build 5875. Only `|n` is switchable (`F & 0x4000`,
//! set on a single-line edit box by `SetMultiLine` `0x77a5e2`) — and `0x77ba90` builds the class map
//! with `K = 0` regardless, so the *cursor* model always sees `|n` as one token. This module
//! therefore takes no flags word: the one live gate belongs to the renderer's line breaker, not to
//! the grammar (the resulting caret drift in a single-line box carrying `|n` is inferred from the
//! verified byte mechanism, not confirmed live, and is the renderer's to reproduce or to fix).
//!
//! ## The law this module exists to hold
//!
//! **The reachable cursor set is "a token boundary with every adjacent zero-width escape absorbed"
//! — after any trailing `|r`/`|h`, before any leading `|c`/`|H`** (`0x77bb30`). No index inside an
//! escape is representable as an output of [`ClassMap::advance`], at either atomicity, in either
//! direction;
//! atomicity adds exactly one thing, that the visible text between `|H…|h` and its closing `|h`
//! becomes uncrossable in a single step. Every index taken or returned by anything here is a byte
//! offset into the original string, and every one lands on a UTF-8 character boundary.
//!
//! ## Inferred, not verified
//!
//! Flagged here as well as at each site:
//!
//! - **`|H` opens and `|h` closes, discriminated by case.** The remap sends both to one arm and the
//!   case test itself was not disassembled; the class table above (class 4 is the whole
//!   `|H<payload>|h`, class 5 is `|h`) plus the empty-visible-text guard comparing against
//!   lowercase `0x68`
//!   (`80 7e 01 68` @`0x5c2992`) is the whole basis.
//! - **The delimiter search is a plain forward byte scan for `|h`** that does not skip `||`. The
//!   emitter's own scan (`0x5ccdc8`, against the literal at `0x84453c`) is described the same way;
//!   the decoder arm's scan was not disassembled.
//! - **`0x77bc80`'s second argument is a byte budget**, not a token count — from the `(start, count)`
//!   signature and the byte-budget shape of its sibling kernel `0x5c6940`.
//! - **The end-of-buffer guard on [`ClassMap::advance`] is ours.** The disassembled excerpt of
//!   `0x77bb30` shows no bound, and its class-0/class-4 "keep skipping" arms would spin on the zero
//!   terminator.
//! - **The worked example is itself inferred, not independently verified** — it follows from the
//!   verified mechanism above.

use std::ops::Range;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The colour a `|c` token carries
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The colour decoded from `|cAARRGGBB`, packed exactly as the client packs it —
/// `0xFF << 24 | RR << 16 | GG << 8 | BB` (`0x5c2ab2–0x5c2ace`).
///
/// **The `AA` nibbles are parsed and then thrown away**: there is no way to spell a
/// translucent colour in 1.12.1 markup. A type that cannot carry alpha is how this module refuses to
/// invent one — `|c00ff0000` and `|cffff0000` are the same opaque red.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Rgba(u32);

impl Rgba {
    /// The opaque colour with these components — the only way to build one.
    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba(0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
    }

    /// The client's packed dword, `0xFFRRGGBB`.
    pub const fn packed(self) -> u32 {
        self.0
    }

    /// Red.
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }

    /// Green.
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }

    /// Blue.
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// Alpha — always `0xff`. Kept as an accessor so the discard is visible at the call site.
    pub const fn a(self) -> u8 {
        0xff
    }

    /// `[r, g, b, alpha]` in 0..1 — the escape's colour drawn at the **FontString's own alpha**.
    ///
    /// The decoder forcing `0xff` is only half the law: the emitter then patches the FontString's
    /// alpha byte over it (`mov cl,[edi+0x2f]; mov [ebp-0x39],cl` @`0x5cceb0`/`0x5cceb6`, written
    /// into the outColor slot *before* `mov edx,[ebp-0x3c]` reads it). So a `|c` span fades with
    /// the string it sits in; drawing it opaque would leave a chat link burning at full alpha while
    /// its own line faded out.
    ///
    /// Taking `alpha` as an argument rather than storing one keeps both halves true: the *escape*
    /// still cannot carry alpha (`|c00ff0000` and `|cffff0000` are the same colour), and the alpha
    /// that reaches the vertex is always the string's.
    pub fn to_f32_at(self, alpha: f32) -> [f32; 4] {
        [
            self.r() as f32 / 255.0,
            self.g() as f32 / 255.0,
            self.b() as f32 / 255.0,
            alpha,
        ]
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Layer 1 — the token decoder (`0x5c2810`)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The token class, with the client's own numbering as the discriminant. Returned by `0x5c2810` in
/// `eax`, and stored in bits
/// 16..23 of every class-map entry.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TokenClass {
    /// 0 — `|cAARRGGBB`, 10 bytes.
    Color = 0,
    /// 1 — `|r` / `|R`, 2 bytes.
    ColorReset = 1,
    /// 2 — `\n`, `\r`, `\r\n`, `|n` / `|N`.
    LineBreak = 2,
    /// 3 — `||`, 2 bytes, drawing one literal `|`.
    EscapedPipe = 3,
    /// 4 — hyperlink OPEN: the entire `|H<payload>|h` prefix.
    LinkOpen = 4,
    /// 5 — `|h`, the close, 2 bytes.
    LinkClose = 5,
    /// 6 — an ordinary character, its UTF-8 length.
    Char = 6,
}

/// What a token *is*, with whatever it carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TokenKind<'a> {
    /// Class 0 — switch the draw colour. Opaque by construction; see [`Rgba`].
    Color(Rgba),
    /// Class 1 — back to the string's base colour (`0x5cce99` restores `FontString+0x2c`).
    ColorReset,
    /// Class 2 — a line break, whichever of the four spellings produced it.
    LineBreak,
    /// Class 3 — `||`, which draws one `|` (`0x5ccec3` looks up the glyph).
    EscapedPipe,
    /// Class 4 — open a hyperlink over the visible text that follows. `payload` is the text between
    /// `|H` and the delimiting `|h` (`item:12345:0:0:0`, `player:Bob`).
    LinkOpen {
        /// The link payload, delimiters excluded.
        payload: &'a str,
    },
    /// Class 5 — the closing `|h`.
    LinkClose,
    /// Class 6 — an ordinary character. A `|` that opened nothing well-formed arrives here too.
    Char(char),
}

impl TokenKind<'_> {
    /// The client's class number for this token.
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

/// One decoded token: what it is, and how many bytes of the source it spans.
///
/// The length is not derivable from the kind — `\r\n` and `\n` are both [`TokenKind::LineBreak`],
/// and a [`TokenKind::LinkOpen`] spans its whole `|H<payload>|h` prefix — so `0x5c2810` returns it
/// separately (`*ioLen`), and so do we.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token<'a> {
    /// What this token is.
    pub kind: TokenKind<'a>,
    /// Its byte length. Always ≥ 1.
    pub byte_len: usize,
}

impl Token<'_> {
    /// The client's class number for this token.
    pub const fn class(&self) -> TokenClass {
        self.kind.class()
    }
}

/// Decode the token starting at byte offset `at` — the whole of `0x5c2810`.
///
/// `None` at (or past) the end of the string. `at` must be a UTF-8 character boundary of `text`;
/// every offset this module produces is one.
///
/// The degradations are law, not tolerance: a `|` that opens nothing well-formed is an *ordinary
/// character* of length 1, exactly as the remap table's fall-through arm makes it.
pub fn token_at(text: &str, at: usize) -> Option<Token<'_>> {
    let rest = &text[at..];
    let b = rest.as_bytes();
    let first = *b.first()?;

    // The two bare line breaks. `\r\n` is ONE class-2 token; a lone `\r` is one as well.
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
        // The ordinary-character fall-through: a lone `|`, or one leading anything the remap table
        // at `0x5c2b10` does not claim.
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
            // INFERRED (see the module doc): uppercase opens, lowercase closes. The remap sends both
            // to one arm and the case test itself was not disassembled.
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

/// `b` starts with `|c` or `|C`. Parse the 8 following hex digits as `AARRGGBB` and **discard the
/// alpha** (`0x5c2ab2–0x5c2ace`). `None` when fewer than 8 hex digits follow — a malformed colour is
/// an ordinary character, not a swallowed escape.
fn parse_color(b: &[u8]) -> Option<Rgba> {
    let digits = b.get(2..10)?;
    let mut v: u32 = 0;
    for &d in digits {
        v = (v << 4) | (d as char).to_digit(16)?;
    }
    Some(Rgba::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// `rest` starts with `|H`. Returns the payload and the byte length of the **whole**
/// `|H<payload>|h` prefix, or `None` for each of the three degradations that make the `|H` a literal
/// `|` instead:
///
/// - no closing `|h` anywhere after it — the forward scan finds no delimiter;
/// - the span is exactly 4 bytes, i.e. an empty payload (`83 f8 04; 75 1b` @`0x5c2972`);
/// - the visible text is empty, i.e. the span is immediately followed by another `|h`
///   (`80 f9 7c; 75 21; 80 7e 01 68; 75 1b` @`0x5c2992`).
///
/// The scan is a plain forward byte search that does not skip `||` (INFERRED — the emitter's own
/// scan at `0x5ccdc8` is described the same way). Searching bytes is safe in UTF-8: `|` and `h` are
/// ASCII and can never appear inside a multi-byte character.
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

/// Every token of `text`, each with the byte offset it starts at — `char_indices`, one grammar up.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Layer 2 — the per-byte class map (`E+0x330`, built by `0x77ba90`)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// One entry of the class map — the client's packed dword, unpacked:
///
/// - bits 0..15, the token's **byte length**, non-zero only at the token's *first* byte;
/// - bits 16..23, the token class;
/// - bit 31, **"inside a hyperlink"**.
///
/// Every continuation byte, and the terminator past the end, is the all-zero dword: `0x77ba90`
/// `rep stos`-zeroes the array and then stores only at each token's first byte
/// (`mov [classArr + 4*ebx], ecx` @`0x77bb0f`). So [`Entry::class`] is `None` exactly where
/// [`Entry::byte_len`] is 0, and [`Entry::in_link`] reads false on a continuation byte even inside
/// a link — which is the client's own reading (`0x77c510` tests the whole dword's sign bit), and
/// harmless, since no cursor can be at one.
///
/// The client's length field is 16 bits (`and len,0xffff` @`0x77baf3`); ours is wider, so a
/// pathological `|H` span ≥ 64 KiB simply does not wrap.
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

    /// The token's byte length — 0 at a continuation byte and at the terminator.
    pub const fn byte_len(&self) -> usize {
        self.byte_len as usize
    }

    /// The token class, or `None` at a continuation byte / the terminator.
    pub const fn class(&self) -> Option<TokenClass> {
        self.class
    }

    /// Bit 31 — is this token part of a hyperlink? Raised on the class-4 open **inclusive**, lowered
    /// only *after* the class-5 close, so the close is tagged too.
    pub const fn in_link(&self) -> bool {
        self.in_link
    }

    /// Does a token start here? (Equivalently: is this not a continuation byte?)
    pub const fn is_token_start(&self) -> bool {
        self.byte_len != 0
    }
}

/// The per-byte token-class array the edit box keeps at `E+0x330`, and the cursor primitives that
/// read it.
///
/// One entry per byte of the source string plus a terminating entry
/// (`classArr[len] = 0` @`0x77c670`), so an index equal to the string length is always valid — the
/// cursor's home at the end of the buffer.
///
/// It holds no text: every primitive here reads only the entries, which is what lets an edit box own
/// its `String` and rebuild the map beside it after each mutation, exactly as `0x77ba90` is called
/// at the tail of every insert (`0x77c022`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClassMap {
    entries: Vec<Entry>,
}

impl ClassMap {
    /// Build the map for `text` — `0x77ba90`.
    ///
    /// The client passes `K = 0` here (`push 0` @`0x77bad9`), so the cursor model always parses
    /// *every* class, `|n` included, whatever the FontString's flags say.
    pub fn new(text: &str) -> ClassMap {
        let mut entries = vec![Entry::CONTINUATION; text.len() + 1];
        // The sticky link bit: raised BEFORE the store on a class-4 open (`or [ebp-4],0x80000000`
        // @`0x77baec`), lowered AFTER the store on a class-5 close (`and [ebp-4],0x7fffffff`
        // @`0x77bb1e`) — which is why the closing `|h` is tagged and the byte after it is not.
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

    /// The byte length of the string this map was built from.
    pub fn text_len(&self) -> usize {
        self.entries.len() - 1
    }

    /// The entry for byte `at`. `at == text_len()` is the terminator; anything past it panics.
    pub fn entry(&self, at: usize) -> Entry {
        self.entries[at]
    }

    /// The next token's start — `0x77bd10`, which is `i + (classArr[i] & 0xffff)` and nothing else.
    ///
    /// A fixed point at the terminator and at any continuation byte, where the length is 0; the
    /// client's add is unguarded in the same way, so a forward walk tests `at < text_len()` rather
    /// than relying on this to move.
    pub fn next_token(&self, at: usize) -> usize {
        at + self.entry(at).byte_len()
    }

    /// The previous token's start — `0x77bd30`: walk back from `at - 1` while the entry is a
    /// continuation. `None` when there is none (the client's `-1`), i.e. at offset 0.
    pub fn prev_token(&self, at: usize) -> Option<usize> {
        let mut i = at.checked_sub(1)?;
        while !self.entry(i).is_token_start() {
            i = i.checked_sub(1)?;
        }
        Some(i)
    }

    /// How many **letters** the `byte_count` bytes from `start` hold — `0x77bc80`, which counts
    /// classes 2, 3 and 6 only (`83 fa 02/03/06` @`0x77bcb0`).
    ///
    /// This is what `GetNumLetters` reports and what `SetMaxLetters` caps, so **classes 0/1/4/5 are
    /// not letters**: a 48-byte item link costs 14, its visible `[Chipped Claw]` and nothing more.
    ///
    /// INFERRED: that the second argument is a byte budget (only its `(start, count)` signature is
    /// known; the sibling kernel `0x5c6940` consumes bytes the same way). A token straddling the
    /// end of the budget is counted whole and ends the walk — deliberately *not* the sibling
    /// kernel's run-to-the-NUL behaviour, which is a latent trap with no upside here.
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

    /// The whole string's letter count — `GetNumLetters`.
    pub fn num_letters(&self) -> usize {
        self.letters(0, self.text_len())
    }

    /// Move the cursor `steps` token-steps from `from` — `0x77bb30`, the heart of the cursor model.
    /// Negative `steps` walk backward. Returns the resulting byte offset (the client returns
    /// the *magnitude* of the delta and its caller `0x77c6b0` signs it; same thing, one fewer trap).
    ///
    /// `atomic_links` is the whole of the difference between the keyboard and the mouse: every
    /// arrow, word-jump, HOME/END, BACKSPACE and DELETE path passes 1 and crosses a whole
    /// `|H…|h[text]|h` in one step, while click, drag, UP/DOWN, the scroll-window sizing and the IME
    /// span all pass 0 and stop on each visible character. Neither ever stops *inside* an escape.
    ///
    /// **The end-of-buffer guard is a deliberate divergence: the real client HANGS here.** Its inner
    /// skip loop re-enters at `0x77bb60`, past the zero guard at `0x77bb56`, and the terminator slot
    /// decodes as class 0 with length 0 — a class the loop skips unconditionally — so a walk still
    /// skipping at the end of the buffer spins forever consuming no bytes. Confirmed by executing
    /// `0x77bb30` under an emulation oracle. It is reachable: a trailing `|c` hangs in
    /// both modes and an unclosed link hangs at `atomic_links = true`, so `SetText("|cffffffff")`
    /// plus one RIGHT freezes 1.12.1. A user cannot type it (`|` becomes `||`) but any script can
    /// set it. We bound the loop by the buffer and land the step on the far end instead —
    /// reproducing a freeze is not fidelity.
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

    /// One step forward — the inner loop at `0x77bb60`, then the post-step absorb at `0x77bb98`.
    fn step_forward(&self, from: usize, atomic_links: bool) -> usize {
        let end = self.text_len();
        let mut at = from;
        while at < end {
            let entry = self.entry(at);
            let Some(class) = entry.class() else { break };
            at += entry.byte_len();
            // `|c` and `|H…|h` are skipped whatever the atomicity: a cursor never sits after a
            // leading escape (`74 da` @`0x77bb84`, `74 d5` @`0x77bb86`).
            if matches!(class, TokenClass::Color | TokenClass::LinkOpen) {
                continue;
            }
            if !atomic_links {
                break; // `84 db; 74 09` — one visible token consumed
            }
            if !entry.in_link() {
                break; // `85 d2; 79 05` — bit 31 clear
            }
            if class == TokenClass::LinkClose {
                break; // the close is what ends the atomic skip
            }
            // Inside a link and not the close: keep skipping (`83 ff 05; 75 c8`) — the atomicity.
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

    /// One step backward — the mirror at `0x77bc13` / `0x77bc1d` / `0x77bc59`.
    fn step_back(&self, from: usize, atomic_links: bool) -> usize {
        let mut at = from;
        // Trailing `|r` / `|h` first (`83 fa 01; 74 ca` / `83 fa 05; 74 c5`).
        while let Some(prev) = self.prev_token(at) {
            match self.entry(prev).class() {
                Some(TokenClass::ColorReset | TokenClass::LinkClose) => at = prev,
                _ => break,
            }
        }
        // One token back — extended, when atomic, over the whole link to its class-4 open
        // (`8a 5d 10; 84 db; 74 09; 85 f6; 79 05; 83 fa 04; 75 b5`).
        while let Some(prev) = self.prev_token(at) {
            let entry = self.entry(prev);
            at = prev;
            if !atomic_links || !entry.in_link() || entry.class() == Some(TokenClass::LinkOpen) {
                break;
            }
        }
        // Then back over any preceding `|c` / `|H` (`85 f6; 74 05; 83 fe 04; 75 08`).
        while let Some(prev) = self.prev_token(at) {
            match self.entry(prev).class() {
                Some(TokenClass::Color | TokenClass::LinkOpen) => at = prev,
                _ => break,
            }
        }
        at
    }

    /// Widen a half-open deletion range so it cannot cut a hyperlink in half — `0x77c510`,
    /// called by BACKSPACE/DELETE (`0x77c280`), by delete-selection (`0x77cd70`) and by Clear
    /// (`0x77c500`).
    ///
    /// If `start` is inside a link, walk back to the class-4 open and then over the whole preceding
    /// `|c`/`|H` run; if `end` is inside a link, walk forward past the closing `|h` and any following
    /// `|r`. So **any deletion touching any byte of a link removes the whole
    /// `|c…|H…|h[text]|h|r` unit** — a second, independent guarantee on top of the atomic walk, and
    /// what makes even a mouse-drag partial selection delete cleanly.
    ///
    /// Two faithfully-kept edges. The endpoint tests read `classArr[start]` / `classArr[end]`
    /// directly (`83 3c 96 00; 79 50` @`0x77c543`, `83 3c 86 00; 79 55` @`0x77c59d`), so an *end*
    /// that merely abuts a link's first byte — the byte it does not include — still widens; and the
    /// backward walk stops on class 0 as well as class 4 (both `jz`s @`0x77c550` land on the same
    /// target), so a `|c` sitting *inside* a link's visible text ends it early. 1.12 content puts the
    /// colour outside the link (`0x52adb0`'s `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"`), so the second edge
    /// is unreachable in practice; it is recorded rather than smoothed away.
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

    /// May text be inserted at `at`? — the opening guard of `0x77bee0`.
    ///
    /// Refused exactly when the entry at the cursor carries bit 31 **and** the previous token's entry
    /// carries it too (`79 16` @`0x77befb`, `0f 88 …` @`0x77bf0d`). That second test is what still
    /// permits an insert *at* a link's leading edge, where the `|H` entry has the bit but its
    /// predecessor — the `|c`, or whatever precedes it — does not.
    ///
    /// A strictly-interior cursor is reachable only by mouse (`0x77d0d0`, `atomic_links = 0`), and
    /// the client then silently swallows the typing — inferred from the verified guard, not
    /// independently confirmed.
    pub fn insert_allowed(&self, at: usize) -> bool {
        if !self.entry(at).in_link() {
            return true;
        }
        match self.prev_token(at) {
            // `85 c0; 7e 12` — cursor <= 0 proceeds.
            None => true,
            Some(prev) => !self.entry(prev).in_link(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An epic item link exactly as `0x52adb0` formats one.
    ///
    /// ```text
    ///  0        10                  30            44  46
    ///  |cffa335ee|Hitem:12345:0:0:0|h[Chipped Claw]|h|r
    ///  \--10---/ \-------20-------/ \-----14-----/ \2/\2/
    /// ```
    const LINK: &str = "|cffa335ee|Hitem:12345:0:0:0|h[Chipped Claw]|h|r";

    /// The same link with ordinary text on both sides — every offset above shifted by 2.
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

        // All four spellings of class 2, and `\r\n` as ONE token rather than two.
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
        // `|T…|t` is a later-expansion feature. Here every byte of it is an ordinary character —
        // the pipes draw, and a client that stripped the run would be inventing a 2.x escape.
        let text = "|TInterface\\Icons\\Foo:16:16|t";
        let first = token_at(text, 0).expect("a token");
        assert_eq!(first.kind, TokenKind::Char('|'));
        assert_eq!(first.byte_len, 1);
        assert_eq!(
            token_at(text, 1).expect("a token").kind,
            TokenKind::Char('T')
        );
        // Nothing in the whole run is anything but an ordinary character.
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
        // The alpha that reaches the vertex is the STRING's, never the escape's: `|c00ff0000`
        // inside a half-faded FontString draws at 0.5, not 0.0 and not 1.0 (`0x5cceb0`).
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
        // 2. The span is exactly 4 bytes — an empty payload (0x5c2972).
        degraded("|H|h[Chipped Claw]|h");
        // 3. The visible text is empty — another `|h` immediately follows the span (0x5c2992).
        degraded("|Hitem:12345:0:0:0|h|h");

        // And the near-misses that must still parse: one visible character is enough, and an
        // unterminated *link* (delimiter present, no close) still opens — the decoder is stateless.
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

        // The link buffer's shape, spelled out — every index the rest of these tests cite.
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
            // Raised on the class-4 open (10) inclusive, lowered only after the class-5 close, whose
            // own entry (44) is therefore still tagged. The `|r` at 46 is not.
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
        // A fixed point at the terminator, exactly like the client's unguarded add.
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

        // A line break and an escaped pipe are each ONE letter; `\r\n` is one, not two.
        let map = ClassMap::new("a||b\r\nc|nd");
        assert_eq!(map.num_letters(), 7);
        assert_eq!(ClassMap::new(AROUND).num_letters(), 18, "14 + `ab` + `cd`");
        assert_eq!(ClassMap::new("").num_letters(), 0);
    }

    // ── The cursor law ───────────────────────────────────────────────────────────────────────

    /// Cursor 0, one RIGHT, and the whole link is behind you.
    /// **The oracle cross-check.** The real `0x77bb30` was *executed* under Unicorn on this exact
    /// buffer, reporting the reachable index sets below verbatim — the strongest evidence this
    /// module can carry, because they came from the binary running rather than from anyone reading
    /// it.
    ///
    /// Three things the cross-check pins that a reading could plausibly have got backwards: index
    /// **0 is** reachable (the leading `|c`+`|H…|h` absorb into the first *step*, not off the
    /// origin); index **30 is reachable in neither mode** — classes 0 and 4 are skipped
    /// unconditionally, so a step arriving from the left consumes the `[` in the same step, and the
    /// cursor can never sit between `|h` and `[`; and after the name the stop is **43 in both
    /// modes**, never 39 or 41, because the trailing `|h|r` absorb **forward** rather than backward
    /// onto the `]`.
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

            // Backward visits the identical set and returns exactly to 0.
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

        // The refusal predicate, over every offset: exactly 30..=39 — the display text through the
        // FIRST byte of the closing `|h`. The link's own boundaries accept (10, the `|H` slot, and
        // 41, the `|r`), and so does 40, a token-interior byte whose slot is zero.
        let refused: Vec<usize> = (0..=BUF.len())
            .filter(|&i| !map.insert_allowed(i))
            .collect();
        assert_eq!(refused, (30..=39).collect::<Vec<_>>());
    }

    /// The client freezes where we stop: its inner skip loop re-enters at `0x77bb60`, past the zero
    /// guard at `0x77bb56`, and the terminator decodes as a skip class, so a buffer whose last
    /// token is `|c` — or an unclosed link, when atomic — spins forever. `SetText("|cffffffff")`
    /// plus one RIGHT hangs 1.12.1. We land on the far end instead; reproducing a freeze is not
    /// fidelity.
    #[test]
    fn a_trailing_escape_lands_on_the_end_where_the_client_would_hang() {
        // Each entry is (buffer, the offset where only skip-class tokens remain, whether the
        // non-atomic walk hangs there too — an unclosed link only traps the atomic one).
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
        // Not ~50 presses: the visible text is 14 characters and the buffer is 48 bytes, yet the
        // atomic walk has exactly two stops.
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

    /// The mouse path (`atomic_links = 0`, `0x77d2f6`): every visible character is its own stop, and
    /// no index inside `|cffa335ee`, `|Hitem:…|h`, `|h` or `|r` ever is.
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

        // And the same set walking back.
        let mut at = map.text_len();
        let mut back = vec![at];
        while at > 0 {
            at = map.advance(at, -1, false);
            back.push(at);
        }
        back.reverse();
        assert_eq!(back, expected);

        // Spelled out: no stop lands inside any escape's byte span.
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

    /// The canonical reachable set (`0x77bb30`): **a token boundary with every adjacent zero-width
    /// escape absorbed — after any trailing `|r`/`|h`, before any leading `|c`/`|H`.**
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

    /// The property sweep: whatever sequence of steps you take, in either direction, at either
    /// atomicity, you can only ever land in the set above — so no index inside an escape is
    /// representable as a cursor position. And non-atomically the whole set is reachable.
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

    /// The two degenerate ends the guard owns: a step that runs out of buffer while skipping escapes
    /// lands on the far end rather than spinning (ours, not the reference's — see
    /// [`ClassMap::advance`]).
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
        // And at the extremes nothing moves.
        let map = ClassMap::new(LINK);
        assert_eq!(map.advance(0, -1, true), 0);
        assert_eq!(map.advance(48, 1, true), 48);
        assert_eq!(map.advance(31, 0, false), 31, "zero steps is the identity");
    }

    // ── Deletion and insertion ───────────────────────────────────────────────────────────────

    #[test]
    fn deleting_any_byte_of_a_link_widens_to_the_whole_unit() {
        let map = ClassMap::new(AROUND);
        // Anything touching the visible text takes the |c, the |H…|h, the |h and the |r with it,
        // leaving exactly `abcd`.
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

    /// The endpoint tests read the class array directly, so an exclusive end that merely abuts the
    /// link's first tagged byte still widens. Faithful to `83 3c 86 00` @`0x77c59d`.
    #[test]
    fn a_deletion_ending_on_the_links_open_still_takes_the_link() {
        let map = ClassMap::new(AROUND);
        assert_eq!(map.snap_delete_range(0..12), 0..50);
        // Whereas ending on the `|c` — which is not tagged — does not.
        assert_eq!(map.snap_delete_range(0..2), 0..2);
    }

    #[test]
    fn insertion_is_refused_strictly_inside_a_links_visible_text() {
        let map = ClassMap::new(LINK);
        // Everywhere outside the link.
        assert!(map.insert_allowed(0), "before the leading |c");
        assert!(map.insert_allowed(46), "between the |h and the |r");
        assert!(map.insert_allowed(48), "at the end of the buffer");
        // The leading edge: the |H entry carries bit 31, its predecessor the |c does not.
        assert!(map.insert_allowed(10), "at the link's leading edge");
        // Strictly inside: refused at every byte from the first visible character through the
        // closing |h, whose entry is still tagged.
        for at in 30..=44 {
            assert!(!map.insert_allowed(at), "inside the link at {at}");
        }
        // And in particular at every cursor position a mouse click can actually produce.
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
        // Multi-byte text before, inside and after the link — 2- and 3-byte characters throughout.
        let text = "héllo |cffa335ee|Hitem:1:0:0:0|h[Épée ✚]|h|r né";
        let map = ClassMap::new(text);
        assert_eq!(map.text_len(), text.len());

        // Every token start, and every index any primitive yields, is a character boundary.
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
            // `String::replace_range` panics unless both ends are character boundaries, so a
            // snapped range that cut a character in half fails here rather than corrupting.
            let mut left = text.to_owned();
            left.replace_range(snapped, "");
        }

        // The multi-byte characters are letters like any other: `héllo ` = 6, `[Épée ✚]` = 8,
        // ` né` = 3.
        assert_eq!(map.num_letters(), 17);
        // And an atomic step still crosses the whole link in one, from `héllo `'s end to ` né`'s
        // start, whatever the byte widths in between.
        let open = text.find("|cffa335ee").expect("the colour push");
        let after = text.find("|r").expect("the reset") + 2;
        assert_eq!(map.advance(open, 1, true), after);
        assert_eq!(map.advance(after, -1, true), open);
    }
}
