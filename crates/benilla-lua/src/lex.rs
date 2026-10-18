//! The lexer: Lua 5.1's `llex.c` with the client's 5.0 long brackets (`GRAMMAR.md`): `[[` opens
//! a long string that nests `[[`, with no `=` levels (`luaX_lex` `0x6ff610`). As the client's
//! lexer does, `\n` alone ends a line and a numeral is read decimal only. Source is bytes.

use crate::chunkid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    // Reserved words, in `token2string` order.
    And,
    Break,
    Do,
    Else,
    Elseif,
    End,
    False,
    For,
    Function,
    If,
    In,
    Local,
    Nil,
    Not,
    Or,
    Repeat,
    Return,
    Then,
    True,
    Until,
    While,
    Name,
    Concat,
    Dots,
    Eq,
    Ge,
    Le,
    Ne,
    Number,
    String,
    Eof,
    /// A single-character token, the byte itself.
    Char(u8),
}

const RESERVED: [(&[u8], Tok); 21] = [
    (b"and", Tok::And),
    (b"break", Tok::Break),
    (b"do", Tok::Do),
    (b"else", Tok::Else),
    (b"elseif", Tok::Elseif),
    (b"end", Tok::End),
    (b"false", Tok::False),
    (b"for", Tok::For),
    (b"function", Tok::Function),
    (b"if", Tok::If),
    (b"in", Tok::In),
    (b"local", Tok::Local),
    (b"nil", Tok::Nil),
    (b"not", Tok::Not),
    (b"or", Tok::Or),
    (b"repeat", Tok::Repeat),
    (b"return", Tok::Return),
    (b"then", Tok::Then),
    (b"true", Tok::True),
    (b"until", Tok::Until),
    (b"while", Tok::While),
];

/// The reserved word `word` is, if any: [`RESERVED`] as a `match`.
fn reserved(word: &[u8]) -> Option<Tok> {
    Some(match word {
        b"and" => Tok::And,
        b"break" => Tok::Break,
        b"do" => Tok::Do,
        b"else" => Tok::Else,
        b"elseif" => Tok::Elseif,
        b"end" => Tok::End,
        b"false" => Tok::False,
        b"for" => Tok::For,
        b"function" => Tok::Function,
        b"if" => Tok::If,
        b"in" => Tok::In,
        b"local" => Tok::Local,
        b"nil" => Tok::Nil,
        b"not" => Tok::Not,
        b"or" => Tok::Or,
        b"repeat" => Tok::Repeat,
        b"return" => Tok::Return,
        b"then" => Tok::Then,
        b"true" => Tok::True,
        b"until" => Tok::Until,
        b"while" => Tok::While,
        _ => return None,
    })
}

impl Tok {
    /// `luaX_token2str`: a token as an error message names it.
    pub fn text(self) -> Vec<u8> {
        let s: &[u8] = match self {
            Tok::Name => b"<name>",
            Tok::Concat => b"..",
            Tok::Dots => b"...",
            Tok::Eq => b"==",
            Tok::Ge => b">=",
            Tok::Le => b"<=",
            Tok::Ne => b"~=",
            Tok::Number => b"<number>",
            Tok::String => b"<string>",
            Tok::Eof => b"<eof>",
            // `luaX_token2str` (`0x6ff550`): a one-byte token as `%c`. The lexer refuses a
            // control byte itself, so none reaches here.
            Tok::Char(c) => return vec![c],
            word => RESERVED.iter().find(|r| r.1 == word).unwrap().0,
        };
        s.to_vec()
    }
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    /// A string's bytes; empty otherwise.
    pub bytes: Vec<u8>,
    pub num: f64,
    /// A name's place in the source, which [`Lexer::name`] reads.
    name: (usize, usize),
}

/// A compile error, rendered as 5.1's `luaX_lexerror` does: `chunkid:line: msg near `tok'`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError(pub Vec<u8>);

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    pub line: u32,
    /// The line of the last token consumed (`lastline`).
    pub lastline: u32,
    chunk: Vec<u8>,
    pub cur: Token,
    ahead: Option<Token>,
    /// 5.0's `buff`: the text of the last string or numeral read, which `luaX_syntaxerror` quotes
    /// for a current string or number token, even when the lookahead has since overwritten it.
    buff: Vec<u8>,
}

const EOZ: i32 = -1;

/// `s` as C's `%s` reads it: up to its first NUL.
pub fn cstr(s: &[u8]) -> &[u8] {
    s.iter().position(|&c| c == 0).map_or(s, |i| &s[..i])
}

/// C's `isspace` in the "C" locale.
fn is_space(c: i32) -> bool {
    matches!(c, 0x20 | 0x09..=0x0D)
}

/// C's `iscntrl` in the "C" locale.
fn is_cntrl(c: i32) -> bool {
    matches!(c, 0x00..=0x1F | 0x7F)
}

fn is_digit(c: i32) -> bool {
    (b'0' as i32..=b'9' as i32).contains(&c)
}

fn is_alpha(c: i32) -> bool {
    (0..256).contains(&c) && (c as u8).is_ascii_alphabetic()
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8], chunkname: &[u8]) -> Result<Self, SyntaxError> {
        let mut lx = Lexer {
            src,
            pos: 0,
            line: 1,
            lastline: 1,
            // `luaO_chunkid` reads the name as a C string.
            chunk: chunkid(cstr(chunkname)),
            cur: Token {
                tok: Tok::Eof,
                bytes: Vec::new(),
                num: 0.0,
                name: (0, 0),
            },
            ahead: None,
            buff: Vec::new(),
        };
        lx.advance()?;
        Ok(lx)
    }

    fn ch(&self) -> i32 {
        self.src.get(self.pos).map_or(EOZ, |&c| c as i32)
    }

    pub fn advance(&mut self) -> Result<(), SyntaxError> {
        self.lastline = self.line;
        self.cur = match self.ahead.take() {
            Some(t) => t,
            None => self.lex()?,
        };
        Ok(())
    }

    /// `luaX_lookahead`: the token after the current one, without consuming it.
    pub fn lookahead(&mut self) -> Result<Tok, SyntaxError> {
        if self.ahead.is_none() {
            self.ahead = Some(self.lex()?);
        }
        Ok(self.ahead.as_ref().unwrap().tok)
    }

    /// `luaX_lexerror`: `chunkid:line: msg`, then ` near `tok'` unless `near` is `None`. C
    /// formats with `%s`, so the near text stops at its first NUL.
    fn error_at(&self, msg: &[u8], near: Option<&[u8]>) -> SyntaxError {
        let mut out = self.chunk.clone();
        out.extend_from_slice(format!(":{}: ", self.line).as_bytes());
        out.extend_from_slice(msg);
        if let Some(near) = near {
            out.extend_from_slice(b" near `");
            out.extend_from_slice(cstr(near));
            out.push(b'\'');
        }
        SyntaxError(out)
    }

    /// `luaX_syntaxerror` (`0x6ff4d0`) at the current token, which it always names.
    pub fn error(&self, msg: &str) -> SyntaxError {
        let near = match self.cur.tok {
            Tok::Name => self.name().to_vec(),
            Tok::String | Tok::Number => self.buff.clone(),
            t => t.text(),
        };
        self.error_at(msg.as_bytes(), Some(&near))
    }

    /// `luaX_lexerror`: an end of input names `<eof>`, anything else quotes the buffer.
    fn lex_error(&self, msg: &str, eof: bool) -> SyntaxError {
        if eof {
            self.error_at(msg.as_bytes(), Some(b"<eof>"))
        } else {
            self.error_at(msg.as_bytes(), Some(&self.buff))
        }
    }

    /// A line end is `\n` alone: the client's lexer switch sends only it to `inclinenumber`
    /// (`0x6ffb94`), and a `\r` is a blank (`0x6ff709`) or, in a string, a byte.
    fn at_newline(&self) -> bool {
        self.ch() == 0x0A
    }

    /// `inclinenumber` (`0x6ffc20`), at a `\n`: one line, with no pairing.
    fn newline(&mut self) -> Result<(), SyntaxError> {
        self.pos += 1;
        self.line += 1;
        // `checklimit(LS, ++LS->linenumber, MAX_INT, "lines in a chunk")` never trips here.
        Ok(())
    }

    fn tok(tok: Tok) -> Token {
        Token {
            tok,
            bytes: Vec::new(),
            num: 0.0,
            name: (0, 0),
        }
    }

    /// The current token's bytes when it is a name, read in the source.
    pub fn name(&self) -> &'a [u8] {
        let (start, end) = self.cur.name;
        &self.src[start..end]
    }

    fn lex(&mut self) -> Result<Token, SyntaxError> {
        loop {
            let c = self.ch();
            match c {
                EOZ => return Ok(Self::tok(Tok::Eof)),
                0x0A => self.newline()?,
                0x2D /* - */ => {
                    self.pos += 1;
                    if self.ch() != b'-' as i32 {
                        return Ok(Self::tok(Tok::Char(b'-')));
                    }
                    self.pos += 1;
                    // A long comment opens at `--[[` alone; anything else, `--[=[` too, is a
                    // line comment from the byte after the last one read (`0x6ff68b`-`0x6ff6c9`).
                    if self.ch() == b'[' as i32 {
                        self.pos += 1;
                        if self.ch() == b'[' as i32 {
                            self.buff.clear();
                            self.long_string(false)?;
                            continue;
                        }
                    }
                    while !self.at_newline() && self.ch() != EOZ {
                        self.pos += 1;
                    }
                }
                0x5B /* [ */ => {
                    // The byte after a `[` is tested once (`0x6ff793`): a second `[` opens a long
                    // string, anything else, `=` too, leaves `[` a token.
                    self.pos += 1;
                    if self.ch() != b'[' as i32 {
                        return Ok(Self::tok(Tok::Char(b'[')));
                    }
                    self.buff.clear();
                    let bytes = self.long_string(true)?;
                    let mut t = Self::tok(Tok::String);
                    t.bytes = bytes;
                    return Ok(t);
                }
                0x3D /* = */ => return Ok(self.two(Tok::Eq, b'=')),
                0x3C /* < */ => return Ok(self.two(Tok::Le, b'<')),
                0x3E /* > */ => return Ok(self.two(Tok::Ge, b'>')),
                0x7E /* ~ */ => return Ok(self.two(Tok::Ne, b'~')),
                0x22 | 0x27 => return self.string(c as u8),
                0x2E /* . */ => {
                    // The client tests each next byte against `.` alone (`0x6ff98a`, `0x6ff9b3`).
                    let dot = |c: i32| c == b'.' as i32;
                    self.pos += 1;
                    if dot(self.ch()) {
                        self.pos += 1;
                        if dot(self.ch()) {
                            self.pos += 1;
                            return Ok(Self::tok(Tok::Dots));
                        }
                        return Ok(Self::tok(Tok::Concat));
                    }
                    if !is_digit(self.ch()) {
                        return Ok(Self::tok(Tok::Char(b'.')));
                    }
                    return self.numeral(true);
                }
                _ => {
                    if is_space(c) {
                        self.pos += 1;
                    } else if is_digit(c) {
                        return self.numeral(false);
                    } else if is_alpha(c) || c == b'_' as i32 {
                        let (src, start) = (self.src, self.pos);
                        let mut end = start + 1;
                        while end < src.len() && (src[end].is_ascii_alphanumeric() || src[end] == b'_')
                        {
                            end += 1;
                        }
                        self.pos = end;
                        let word = &src[start..end];
                        if let Some(r) = reserved(word) {
                            return Ok(Self::tok(r));
                        }
                        let mut t = Self::tok(Tok::Name);
                        t.name = (start, self.pos);
                        return Ok(t);
                    } else if is_cntrl(c) {
                        // `iscntrl` (`0x6ffabb`): a control byte is the lexer's own error, near
                        // `char(%d)` (`0x8721a8`), through `luaX_error` (`0x6ffafd`).
                        let near = format!("char({c})");
                        return Err(self.error_at(b"invalid control char", Some(near.as_bytes())));
                    } else {
                        // Any other byte is a one-byte token for the parser to refuse.
                        self.pos += 1;
                        return Ok(Self::tok(Tok::Char(c as u8)));
                    }
                }
            }
        }
    }

    fn two(&mut self, tok: Tok, single: u8) -> Token {
        self.pos += 1;
        if self.ch() == b'=' as i32 {
            self.pos += 1;
            Self::tok(tok)
        } else {
            Self::tok(Tok::Char(single))
        }
    }

    /// 5.0's `read_long_string` (`0x700010`), at the second `[`: a `[[` inside nests one deeper
    /// and a `]]` closes one, with no `=` levels. The buffer keeps the brackets, which an error
    /// quotes.
    fn long_string(&mut self, keep: bool) -> Result<Vec<u8>, SyntaxError> {
        let mut cont = 0u32;
        self.buff.extend_from_slice(b"[[");
        self.pos += 1;
        if self.at_newline() {
            self.newline()?;
        }
        loop {
            match self.ch() {
                EOZ => {
                    let msg = if keep {
                        "unfinished long string"
                    } else {
                        "unfinished long comment"
                    };
                    return Err(self.lex_error(msg, true));
                }
                // `[` then `[` (`0x700109`): one level deeper.
                0x5B => {
                    self.buff.push(b'[');
                    self.pos += 1;
                    if self.ch() == b'[' as i32 {
                        self.buff.push(b'[');
                        self.pos += 1;
                        cont += 1;
                    }
                }
                // `]` then `]` (`0x700173`): the end at level 0 (`0x70017e`), else one level out.
                0x5D => {
                    self.buff.push(b']');
                    self.pos += 1;
                    if self.ch() == b']' as i32 {
                        self.buff.push(b']');
                        self.pos += 1;
                        if cont == 0 {
                            break;
                        }
                        cont -= 1;
                    }
                }
                // Only `\n` (`0x700234`); a `\r` is kept as a byte.
                0x0A => {
                    self.buff.push(b'\n');
                    self.newline()?;
                    if !keep {
                        self.buff.clear();
                    }
                }
                c => {
                    if keep {
                        self.buff.push(c as u8);
                    }
                    self.pos += 1;
                }
            }
        }
        // `luaZ_buffer + 2`, the length less 4 (`0x70020a`): the text between the brackets.
        // A comment's buffer was reset at each newline and holds no string.
        if !keep {
            return Ok(Vec::new());
        }
        Ok(self.buff[2..self.buff.len() - 2].to_vec())
    }

    /// `read_string` (`0x7002a0`): a `\n` ends it unfinished (`0x700326`), a `\r` is a byte,
    /// `\ddd` is up to three digits.
    fn string(&mut self, delim: u8) -> Result<Token, SyntaxError> {
        self.buff.clear();
        self.buff.push(delim);
        self.pos += 1;
        while self.ch() != delim as i32 {
            match self.ch() {
                EOZ => return Err(self.lex_error("unfinished string", true)),
                0x0A => return Err(self.lex_error("unfinished string", false)),
                0x5C => {
                    self.pos += 1;
                    let e = self.ch();
                    let simple = match e {
                        0x61 => Some(7),
                        0x62 => Some(8),
                        0x66 => Some(12),
                        0x6E => Some(b'\n'),
                        0x72 => Some(b'\r'),
                        0x74 => Some(b'\t'),
                        0x76 => Some(11),
                        _ => None,
                    };
                    if let Some(b) = simple {
                        self.buff.push(b);
                        self.pos += 1;
                    } else if e == 0x0A {
                        // An escaped `\r` is the default arm's byte (`0x700710`).
                        self.buff.push(b'\n');
                        self.newline()?;
                    } else if e == EOZ {
                        // Raised as unfinished on the next pass.
                    } else if !is_digit(e) {
                        self.buff.push(e as u8);
                        self.pos += 1;
                    } else {
                        let mut v = 0i32;
                        let mut i = 0;
                        loop {
                            v = 10 * v + (self.ch() - b'0' as i32);
                            self.pos += 1;
                            i += 1;
                            if i >= 3 || !is_digit(self.ch()) {
                                break;
                            }
                        }
                        if v > 255 {
                            return Err(self.lex_error("escape sequence too large", false));
                        }
                        self.buff.push(v as u8);
                    }
                }
                c => {
                    self.buff.push(c as u8);
                    self.pos += 1;
                }
            }
        }
        self.buff.push(delim);
        self.pos += 1;
        let mut t = Self::tok(Tok::String);
        t.bytes = self.buff[1..self.buff.len() - 1].to_vec();
        Ok(t)
    }

    /// The client's numeral reader (`0x6ffd20`, 5.0's `read_numeral`): digits, one `.` (a second
    /// right after it is its own error, `0x8721dc`), digits, then `e`/`E`, a sign and digits. A
    /// letter after a number starts the next token, so `0x10` is 0 then the name `x10`.
    fn numeral(&mut self, comma: bool) -> Result<Token, SyntaxError> {
        self.buff.clear();
        if comma {
            self.buff.push(b'.');
        }
        let digits = |l: &mut Self| {
            while is_digit(l.ch()) {
                l.buff.push(l.ch() as u8);
                l.pos += 1;
            }
        };
        digits(self);
        if self.ch() == b'.' as i32 {
            self.buff.push(b'.');
            self.pos += 1;
            if self.ch() == b'.' as i32 {
                self.buff.push(b'.');
                self.pos += 1;
                return Err(self.lex_error(
                    "ambiguous syntax (decimal point x string concatenation)",
                    false,
                ));
            }
        }
        digits(self);
        if self.ch() == b'e' as i32 || self.ch() == b'E' as i32 {
            self.buff.push(self.ch() as u8);
            self.pos += 1;
            if self.ch() == b'+' as i32 || self.ch() == b'-' as i32 {
                self.buff.push(self.ch() as u8);
                self.pos += 1;
            }
            digits(self);
        }
        match crate::number::str2d(&self.buff) {
            Some(n) => {
                let mut t = Self::tok(Tok::Number);
                t.num = n;
                Ok(t)
            }
            None => Err(self.lex_error("malformed number", false)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_matches_the_reserved_table() {
        for (word, tok) in RESERVED {
            assert_eq!(reserved(word), Some(tok));
        }
        for word in [&b"self"[..], b"An", b"ends", b"whil", b""] {
            assert_eq!(reserved(word), None);
        }
    }
}
