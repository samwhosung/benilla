//! The one reader of every place benilla fires a Lua event, for the gates that ask what is fired
//! and with what ([`super::event_shape_gate`], `reference_ui`'s producer and ghost gates).
//!
//! The reference dispatches an event only through `SignalEvent` (`0x703e50`, an id alone) and
//! `SignalEvent2` (`0x703f50`, an id and its arguments). Its counterpart here is a *firing
//! function*: a `fn` or closure whose parameters hold `event: &str` and then a `ScriptValue`
//! list. Those are found by that signature, never listed, so a wrapper written tomorrow is
//! scanned tomorrow; and [`every_queue_and_dispatch_sink_sits_in_a_firing_function`] fails on any
//! other way into the host queue or the dispatch, which is what closes the set. A call of a
//! firing function is a [`Site`]. A site whose name and list are the caller's own bare bindings
//! is a relay: inside a firing function it forwards that function's parameters, and elsewhere
//! its origin is a `("EVENT", list)` tuple in the same file, which the scan reads as a site.
//! Test files and `#[cfg(test)] mod` bodies are not read: their fires are not shipped.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// One source file, comments blanked and `#[cfg(test)] mod` bodies blanked; `lit` marks the bytes
/// inside string and char literals, whose brackets and words are not code.
pub(super) struct Src {
    pub path: String,
    pub code: String,
    lit: Vec<bool>,
}

impl Src {
    /// Every event-shaped string literal in the file's code.
    pub fn event_literals(&self) -> BTreeSet<String> {
        let b = self.code.as_bytes();
        let mut out = BTreeSet::new();
        let mut k = 0;
        while k < b.len() {
            if self.lit[k] && b[k] == b'"' && (k == 0 || !self.lit[k - 1]) {
                let end = (k..b.len()).find(|&e| !self.lit[e]).unwrap_or(b.len());
                if let Some(body) = self.code.get(k + 1..end.saturating_sub(1)) {
                    if event_shaped(body) {
                        out.insert(body.to_string());
                    }
                }
                k = end;
            } else {
                k += 1;
            }
        }
        out
    }
}

/// A function, or a closure that fires, with the byte range of its body.
struct Def {
    start: usize,
    end: usize,
    name: String,
    /// Set when the parameters hold `event: &str` and then a `ScriptValue` list.
    firing: Option<Firing>,
}

#[derive(Clone)]
struct Firing {
    /// The call-argument index of `event`, a `self` receiver not counted.
    at: usize,
    closure: bool,
}

/// What a site names.
#[derive(Debug)]
pub(super) enum Name {
    /// Every event-shaped string literal of the name expression: one, or the arms of an `if`.
    Lit(Vec<String>),
    /// A binding or a call: the name is read elsewhere.
    Other(String),
}

/// A call of a firing function, a literal `pending_events.push((name, list))`, or a relay file's
/// `("EVENT", list)` tuple.
#[derive(Debug)]
pub(super) struct Site {
    pub path: String,
    pub line: usize,
    pub name: Name,
    /// The argument-list expression as written.
    pub list: String,
    /// The name and the list are the caller's own bare bindings; the origin is another site.
    pub relay: bool,
    /// The site sits inside a firing function, and forwards what that function was given.
    forward: bool,
    /// Where the site is in its source's code.
    pos: usize,
}

impl Site {
    pub fn at(&self) -> String {
        format!("{}:{}", self.path, self.line)
    }

    /// The kind of each pushed value, `None` when the list is not written out at the site.
    pub fn kinds(&self) -> Option<Vec<char>> {
        kinds_of_list(&self.list)
    }
}

/// Everything the scan found.
pub(super) struct Scan {
    pub sources: Vec<Src>,
    pub sites: Vec<Site>,
    firing: BTreeSet<String>,
    /// Where a queue or dispatch sink sits outside every firing function.
    stray: Vec<String>,
    /// Files with a relay outside a firing function whose origin the scan cannot see.
    unaccounted: Vec<String>,
}

/// The files that drain the host queue: their relay carries what the `pending_events` sites
/// pushed, and those are sites of their own.
const DRAINS: &[&str] = &["crates/benilla-ui/src/script/tick.rs"];

impl Scan {
    /// The event-shaped literals of the file at `path`.
    pub fn file_events(&self, path: &str) -> BTreeSet<String> {
        self.sources
            .iter()
            .find(|s| s.path == path)
            .map(Src::event_literals)
            .unwrap_or_default()
    }
}

pub(super) fn scan() -> &'static Scan {
    static SCAN: OnceLock<Scan> = OnceLock::new();
    SCAN.get_or_init(|| build(read_sources()))
}

/// An event name's shape: upper case, digits and underscores.
pub(super) fn event_shaped(lit: &str) -> bool {
    !lit.is_empty()
        && lit
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn blank(code: &mut [u8], from: usize, to: usize) {
    for b in &mut code[from..to] {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

/// `src` with comments blanked, and a mask of the bytes inside string and char literals.
fn lex(src: &str) -> (Vec<u8>, Vec<bool>) {
    let b = src.as_bytes();
    let mut code = b.to_vec();
    let mut lit = vec![false; b.len()];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            let end = b[i..]
                .iter()
                .position(|&x| x == b'\n')
                .map_or(b.len(), |p| i + p);
            blank(&mut code, i, end);
            i = end;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let (start, mut depth) = (i, 0);
            while i < b.len() {
                if b[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if b[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            blank(&mut code, start, i);
        } else if c == b'r'
            && (i == 0 || !is_ident(b[i - 1]) || b[i - 1] == b'b')
            && b[i + 1..].iter().find(|&&x| x != b'#') == Some(&b'"')
        {
            let hashes = b[i + 1..].iter().take_while(|&&x| x == b'#').count();
            let body = i + 1 + hashes + 1;
            let mut close = vec![b'"'];
            close.extend(std::iter::repeat_n(b'#', hashes));
            let end = (body..b.len())
                .find(|&k| b[k..].starts_with(&close))
                .map_or(b.len(), |k| k + close.len());
            lit[i..end].fill(true);
            i = end;
        } else if c == b'"' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            lit[start..i].fill(true);
        } else if c == b'\'' {
            // A char literal, else a lifetime.
            let rest = &src[i + 1..];
            let end = if rest.starts_with('\\') {
                rest[2..].find('\'').map(|p| i + 1 + 2 + p + 1)
            } else {
                rest.chars()
                    .next()
                    .filter(|ch| rest[ch.len_utf8()..].starts_with('\''))
                    .map(|ch| i + 1 + ch.len_utf8() + 1)
            };
            match end {
                Some(end) => {
                    lit[i..end].fill(true);
                    i = end;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    (code, lit)
}

/// The index of the bracket closing the one opened just before `from`; brackets inside literals
/// do not count.
fn close_of(code: &[u8], lit: &[bool], from: usize) -> usize {
    let mut depth = 1i32;
    for k in from..code.len() {
        if lit[k] {
            continue;
        }
        match code[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// Splits `from..` at its top-level commas, up to the bracket that closes the list or, at depth
/// 0, the byte `stop`; returns the trimmed parts and where it ended. `angle` also tracks `<` and
/// `>`, for a signature's generics.
fn split_top(
    code: &[u8],
    lit: &[bool],
    from: usize,
    stop: Option<u8>,
    angle: bool,
) -> (Vec<String>, usize) {
    let (mut parts, mut depth, mut start, mut k) = (Vec::new(), 0i32, from, from);
    while k < code.len() {
        if lit[k] {
            k += 1;
            continue;
        }
        let c = code[k];
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b'<' if angle => depth += 1,
            b'>' if angle && code[k - 1] != b'-' => depth -= 1,
            b')' | b']' | b'}' if depth == 0 => break,
            b')' | b']' | b'}' => depth -= 1,
            _ if Some(c) == stop && depth == 0 => break,
            b',' if depth == 0 => {
                parts.push(String::from_utf8_lossy(&code[start..k]).trim().to_string());
                start = k + 1;
            }
            _ => {}
        }
        k += 1;
    }
    let k = k.min(code.len());
    parts.push(String::from_utf8_lossy(&code[start..k]).trim().to_string());
    parts.retain(|p| !p.is_empty());
    (parts, k)
}

/// `Some(Firing)` when the parameters hold a `&str` and then a `ScriptValue` list: the event's
/// name and arguments, whatever the binding is called. A `self` receiver has no `:`, so it is
/// left out of the argument index as a call leaves it out.
fn firing_of(params: &[String], closure: bool) -> Option<Firing> {
    let ps: Vec<(String, String)> = params
        .iter()
        .filter_map(|p| {
            let (name, ty) = p.split_once(':')?;
            let ty: String = ty.split_whitespace().collect();
            Some((name.trim().to_string(), ty))
        })
        .collect();
    let list =
        |t: &str| t.contains("ScriptValue") && (t.starts_with("Vec<") || t.starts_with("&["));
    let at = ps
        .iter()
        .zip(ps.iter().skip(1))
        .position(|((_, ty), (_, next))| ty == "&str" && list(next))?;
    Some(Firing { at, closure })
}

fn defs_of(src: &Src) -> Vec<Def> {
    let (code, lit) = (src.code.as_bytes(), &src.lit);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = src.code[from..].find("fn ") {
        let at = from + p;
        from = at + 3;
        if (at > 0 && is_ident(code[at - 1])) || lit[at] {
            continue;
        }
        let name: String = src.code[at + 3..]
            .trim_start()
            .chars()
            .take_while(|&c| is_ident(c as u8))
            .collect();
        let Some(open) = src.code[at..]
            .find(['(', '{', ';'])
            .map(|p| at + p)
            .filter(|&o| code[o] == b'(')
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let (params, close) = split_top(code, lit, open + 1, None, true);
        let end = match src.code[close..].find(['{', ';']).map(|p| close + p) {
            Some(o) if code[o] == b'{' => close_of(code, lit, o + 1),
            Some(o) => o,
            None => close,
        };
        out.push(Def {
            start: at,
            end,
            name,
            firing: firing_of(&params, false),
        });
    }
    // `let name = |params|`, `move` allowed: a scope of its own only when it fires.
    let mut from = 0;
    while let Some(p) = src.code[from..].find("let ") {
        let at = from + p;
        from = at + 4;
        if (at > 0 && is_ident(code[at - 1])) || lit[at] {
            continue;
        }
        let rest = src.code[at + 4..].trim_start();
        let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
        let name: String = rest.chars().take_while(|&c| is_ident(c as u8)).collect();
        let Some(after) = rest[name.len()..].trim_start().strip_prefix('=') else {
            continue;
        };
        let after = after.trim_start();
        let after = after.strip_prefix("move ").unwrap_or(after).trim_start();
        if name.is_empty() || !after.starts_with('|') {
            continue;
        }
        let bar = src.code.len() - after.len();
        let (params, close) = split_top(code, lit, bar + 1, Some(b'|'), true);
        let Some(firing) = firing_of(&params, true) else {
            continue;
        };
        let body = src.code[close + 1..].trim_start();
        let at_body = src.code.len() - body.len();
        let end = if body.starts_with('{') {
            close_of(code, lit, at_body + 1)
        } else {
            split_top(code, lit, at_body, Some(b';'), false).1
        };
        out.push(Def {
            start: at,
            end,
            name,
            firing: Some(firing),
        });
    }
    out.sort_by_key(|d| d.start);
    out
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].bytes().filter(|&b| b == b'\n').count() + 1
}

/// The kind a pushed value has: `s` string, `n` number, `b` boolean, `x` nil, `?` not in the source.
fn kind_of(expr: &str) -> char {
    let e = expr.trim_start_matches('&').trim();
    let Some(p) = e.find("ScriptValue::") else {
        return '?';
    };
    if !e[..p]
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
    {
        return '?';
    }
    match &e[p + "ScriptValue::".len()..] {
        v if v.starts_with("Str") => 's',
        v if v.starts_with("Int") || v.starts_with("Number") => 'n',
        v if v.starts_with("Bool") => 'b',
        v if v.starts_with("Nil") => 'x',
        _ => '?',
    }
}

/// The kinds of a list written out as `vec![…]`, `&[…]`, `Vec::new()` or `Vec::<T>::new()`.
fn kinds_of_list(list: &str) -> Option<Vec<char>> {
    let l = list.trim();
    let inner = l
        .strip_prefix("vec![")
        .or_else(|| l.strip_prefix("&["))
        .and_then(|r| r.strip_suffix(']'));
    if let Some(inner) = inner {
        let (code, lit) = lex(inner);
        let (parts, _) = split_top(&code, &lit, 0, None, false);
        return Some(parts.iter().map(|e| kind_of(e)).collect());
    }
    let compact: String = l.split_whitespace().collect();
    (compact == "Vec::new()" || (compact.starts_with("Vec::<") && compact.ends_with(">::new()")))
        .then(Vec::new)
}

/// A name expression: a literal, or an `if`/`match` whose arms are literals, is read; anything
/// else (a binding, a call, `format!`) is not.
fn name_of(expr: &str) -> Name {
    let e = expr.trim().trim_start_matches('&');
    let (_, lit) = lex(expr);
    let b = expr.as_bytes();
    let mut lits = Vec::new();
    let mut k = 0;
    while k < b.len() {
        if lit[k] && b[k] == b'"' {
            let end = (k + 1..b.len()).find(|&x| b[x] == b'"').unwrap_or(b.len());
            lits.push(expr[k + 1..end].to_string());
            k = end + 1;
        } else {
            k += 1;
        }
    }
    let readable = e.starts_with('"') || e.starts_with("if ") || e.starts_with("match ");
    if readable && !lits.is_empty() && lits.iter().all(|l| event_shaped(l)) {
        Name::Lit(lits)
    } else {
        Name::Other(expr.to_string())
    }
}

/// `expr` is a lone binding, optionally borrowed and with one of `suffix`.
fn bare(expr: &str, suffix: &[&str]) -> bool {
    let e = expr.trim().trim_start_matches('&');
    let e = suffix.iter().find_map(|s| e.strip_suffix(s)).unwrap_or(e);
    !e.is_empty() && e.bytes().all(is_ident) && e.as_bytes()[0].is_ascii_lowercase()
}

fn find_from(code: &[u8], needle: &str, from: usize) -> Option<usize> {
    let n = needle.as_bytes();
    (from..code.len().saturating_sub(n.len() - 1)).find(|&k| code[k..].starts_with(n))
}

impl Src {
    /// `text` as `path`: comments blanked, and every `#[cfg(test)] mod` taken out wherever it sits
    /// in the file.
    pub fn new(path: &str, text: &str) -> Src {
        let (mut code, lit) = lex(text);
        let mut from = 0;
        while let Some(i) = find_from(&code, "#[cfg(test)]", from) {
            from = i + 1;
            let mut j = i + "#[cfg(test)]".len();
            while code.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            let tail = String::from_utf8_lossy(&code[j..(j + 24).min(code.len())]).to_string();
            let tail = tail.strip_prefix("pub(crate) ").unwrap_or(&tail);
            let tail = tail.strip_prefix("pub ").unwrap_or(tail);
            if !tail.starts_with("mod ") {
                continue;
            }
            let end = match (find_from(&code, ";", j), find_from(&code, "{", j)) {
                (Some(s), Some(b)) if s < b => s + 1,
                (_, Some(b)) => close_of(&code, &lit, b + 1) + 1,
                (Some(s), None) => s + 1,
                _ => continue,
            };
            blank(&mut code, i, end);
        }
        Src {
            path: path.to_string(),
            code: String::from_utf8(code).expect("blanking keeps UTF-8"),
            lit,
        }
    }
}

/// The shipped sources: every `.rs` under `crates/` whose path (from the workspace root) does not
/// name a test.
fn read_sources() -> Vec<Src> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let mut out = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            if p.extension().is_some_and(|x| x == "rs") && !rel.contains("test") {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                out.push(Src::new(&rel, &text));
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Every `field.push(` in `src`: its place, and its `(name, list)` tuple when written out.
fn pushes(src: &Src, field: &str) -> Vec<(usize, Option<(String, String)>)> {
    let (code, lit) = (src.code.as_bytes(), &src.lit);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = src.code[from..].find(field) {
        let at = from + p;
        from = at + field.len();
        if (at > 0 && is_ident(code[at - 1])) || lit[at] {
            continue;
        }
        let Some(rest) = src.code[from..]
            .trim_start()
            .strip_prefix('.')
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix("push"))
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('('))
        else {
            continue;
        };
        let rest = rest.trim_start();
        let tuple = rest.strip_prefix('(').and_then(|_| {
            let inner = src.code.len() - rest.len() + 1;
            let (parts, _) = split_top(code, lit, inner, None, false);
            <[String; 2]>::try_from(parts).ok().map(|[n, l]| (n, l))
        });
        out.push((at, tuple));
    }
    out
}

/// Names a `format!` builds, by its template: the five power tokens of `UnitFrame` (`0x51bbb0`'s
/// bridge fires `UNIT_MANA` and `UNIT_MAXMANA` and their kin).
const FORMATTED: &[(&str, &[&str])] = &[
    ("UNIT_{}", &["MANA", "RAGE", "FOCUS", "ENERGY", "HAPPINESS"]),
    (
        "UNIT_MAX{}",
        &["MANA", "RAGE", "FOCUS", "ENERGY", "HAPPINESS"],
    ),
];

impl Src {
    /// The event-shaped literals between `from` and `to`.
    fn event_literals_in(&self, from: usize, to: usize) -> Vec<String> {
        let (b, to) = (self.code.as_bytes(), to.min(self.code.len()));
        let mut out = Vec::new();
        let mut k = from;
        while k < to {
            if self.lit[k] && b[k] == b'"' && (k == 0 || !self.lit[k - 1]) {
                let end = (k + 1..b.len()).find(|&e| b[e] == b'"').unwrap_or(b.len());
                if let Some(body) = self.code.get(k + 1..end) {
                    if event_shaped(body) {
                        out.push(body.to_string());
                    }
                }
                k = end + 1;
            } else {
                k += 1;
            }
        }
        out
    }
}

/// Where the expression starting at `from` ends: the `;`, or with `brace` the `{`, at depth 0.
fn expr_end(src: &Src, from: usize, brace: bool) -> usize {
    let (b, mut depth) = (src.code.as_bytes(), 0i32);
    for (k, &c) in b.iter().enumerate().skip(from) {
        if src.lit[k] {
            continue;
        }
        match c {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b'{' if brace && depth == 0 => return k,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth < 0 {
                    return k;
                }
            }
            b';' if depth == 0 => return k,
            _ => {}
        }
    }
    b.len()
}

fn has_word(hay: &str, word: &str) -> bool {
    hay.match_indices(word).any(|(i, _)| {
        let (b, end) = (hay.as_bytes(), i + word.len());
        (i == 0 || !is_ident(b[i - 1])) && (end >= b.len() || !is_ident(b[end]))
    })
}

/// The event names `expr` can hold at `pos` of source `i`, read from where the enclosing function
/// binds it: `let x = <init>;` (an `if`, `match` or `Some(..)` of literals, or another binding),
/// `for x in <literals> {`, `if let Some(x) = <binding> {`; a `format!` template of [`FORMATTED`];
/// a `const`; or a call, whose callee's body names them. `None` when it is read from nowhere
/// else in the function: a parameter, a field, a collection filled elsewhere.
fn resolve(
    sources: &[Src],
    defs: &[Vec<Def>],
    i: usize,
    pos: usize,
    expr: &str,
    depth: u8,
) -> Option<Vec<String>> {
    let e = expr.trim().trim_start_matches('&').trim_start_matches('*');
    let mut out: Vec<String> = Vec::new();
    if let Some(rest) = e.strip_prefix("format!(") {
        let rest = rest.trim_start();
        let tpl = rest.strip_prefix('"')?.split('"').next()?;
        let (_, fills) = FORMATTED.iter().find(|(t, _)| *t == tpl)?;
        return Some(fills.iter().map(|f| tpl.replace("{}", f)).collect());
    }
    let ident: String = e.chars().take_while(|&c| is_ident(c as u8)).collect();
    if ident.is_empty() || depth == 0 {
        return None;
    }
    let rest = e[ident.len()..].trim_start();
    if rest.starts_with('(') || rest.starts_with("::") {
        // A call: the last path segment before the arguments names the function.
        let callee: String = e[..e.find('(')?].rsplit("::").next()?.trim().to_string();
        for (src, ds) in sources.iter().zip(defs) {
            for d in ds.iter().filter(|d| d.name == callee && d.firing.is_none()) {
                out.extend(src.event_literals_in(d.start, d.end));
            }
        }
    } else if rest.is_empty()
        && ident
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b == b'_' || b.is_ascii_digit())
    {
        let decl = format!("const {ident}:");
        for src in sources {
            for (k, _) in src.code.match_indices(&decl) {
                out.extend(src.event_literals_in(k, expr_end(src, k, false)));
            }
        }
    } else if rest.is_empty() {
        let src = &sources[i];
        let body = defs[i].iter().rev().find(|d| {
            d.start <= pos && pos < d.end && d.firing.as_ref().is_none_or(|f| !f.closure)
        })?;
        for (k, _) in src.code[body.start..body.end]
            .match_indices("let ")
            .map(|(k, m)| (body.start + k, m))
        {
            if k > 0 && is_ident(src.code.as_bytes()[k - 1]) {
                continue;
            }
            let after = k + 4;
            // The pattern runs to the `=` that is not `==`, past a type annotation.
            let Some(eq) = src.code[after..].find(" = ").map(|p| after + p) else {
                continue;
            };
            let pat = src.code[after..eq].split(':').next().unwrap_or("");
            if !has_word(pat, &ident)
                || pat.contains(';')
                || pat.contains('{') && !pat.contains('(')
            {
                continue;
            }
            let conditional = {
                let before = src.code[..k].trim_end();
                before.ends_with("if") || before.ends_with("while") || before.ends_with("&&")
            };
            let (from, to) = (eq + 3, expr_end(src, eq + 3, conditional));
            out.extend(src.event_literals_in(from, to));
            let init = src.code[from..to].trim();
            if init.bytes().all(|b| is_ident(b) || b" &.".contains(&b)) {
                out.extend(resolve(sources, defs, i, k, init, depth - 1).unwrap_or_default());
            }
            // A function the initializer calls names its own events.
            for (at, _) in init.match_indices('(') {
                let callee: String = init[..at]
                    .chars()
                    .rev()
                    .take_while(|&c| is_ident(c as u8))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                for (csrc, ds) in sources.iter().zip(defs) {
                    for d in ds.iter().filter(|d| d.name == callee && d.firing.is_none()) {
                        out.extend(csrc.event_literals_in(d.start, d.end));
                    }
                }
            }
        }
        for (k, _) in src.code[body.start..body.end]
            .match_indices("for ")
            .map(|(k, m)| (body.start + k, m))
        {
            if k > 0 && is_ident(src.code.as_bytes()[k - 1]) {
                continue;
            }
            let after = k + 4;
            let Some(inn) = src.code[after..].find(" in ").map(|p| after + p) else {
                continue;
            };
            if !has_word(&src.code[after..inn], &ident) {
                continue;
            }
            out.extend(src.event_literals_in(inn + 4, expr_end(src, inn + 4, true)));
        }
    }
    out.sort();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

/// The list a call stands for: `expr` is `callee(..)` or `recv.callee(..)`, `callee` is the one
/// function of that name in the sources, and its body ends in a `vec![…]`, which comes back as
/// written. That is the reference's own shape for a shared fire: one helper builds the list
/// (`0x49b0b0` for every `CHAT_MSG_*`), and each call site only names it. `None` for a name two
/// functions carry, a body that does not end in the list, or an expression that is more than the
/// call, or a body with a `return` in it.
fn list_of_call(sources: &[Src], defs: &[Vec<Def>], expr: &str) -> Option<String> {
    let e = expr.trim();
    let open = e.find('(')?;
    let (head, callee) = {
        let head = &e[..open];
        let name: String = head
            .chars()
            .rev()
            .take_while(|&c| is_ident(c as u8))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        (&head[..head.len() - name.len()], name)
    };
    let path_like = head.bytes().all(|b| is_ident(b) || b == b'.' || b == b':');
    let (code, lit) = lex(e);
    if callee.is_empty() || !path_like || close_of(&code, &lit, open + 1) + 1 != e.len() {
        return None;
    }
    let mut found = sources.iter().zip(defs).flat_map(|(src, ds)| {
        ds.iter()
            .filter(|d| d.name == callee)
            .map(move |d| (src, d))
    });
    let (src, d) = found.next().filter(|_| found.next().is_none())?;
    if d.firing.is_some() {
        return None;
    }
    let (code, lit) = (src.code.as_bytes(), &src.lit);
    // An early `return` can hand back another list, so the tail is then not the whole story.
    let body = &src.code[d.start..d.end];
    if body.match_indices("return").any(|(i, _)| {
        let (at, end) = (d.start + i, d.start + i + "return".len());
        !lit[at]
            && (i == 0 || !is_ident(code[at - 1]))
            && (end >= code.len() || !is_ident(code[end]))
    }) {
        return None;
    }
    let at = body.rfind("vec![")? + d.start;
    let close = close_of(code, lit, at + "vec![".len());
    (!lit[at] && src.code[close + 1..d.end].trim().is_empty())
        .then(|| src.code[at..=close].to_string())
}

fn build(sources: Vec<Src>) -> Scan {
    let defs: Vec<Vec<Def>> = sources.iter().map(defs_of).collect();

    // Firing functions by name. A name some other `fn` also carries is ambiguous (`raise` is
    // also "bring to front"), and a call of it counts only with an event-shaped literal name.
    let mut firing: BTreeMap<String, Firing> = BTreeMap::new();
    let mut other: BTreeSet<String> = BTreeSet::new();
    for d in defs.iter().flatten() {
        match &d.firing {
            Some(f) if !f.closure => {
                firing.entry(d.name.clone()).or_insert_with(|| f.clone());
            }
            None => {
                other.insert(d.name.clone());
            }
            _ => {}
        }
    }

    let mut sites = Vec::new();
    let mut stray = Vec::new();
    for (src, ds) in sources.iter().zip(&defs) {
        let (code, lit) = (src.code.as_bytes(), &src.lit);
        // A firing closure is a binding of its own file.
        let local: BTreeMap<&str, &Firing> = ds
            .iter()
            .filter_map(|d| {
                d.firing
                    .as_ref()
                    .filter(|f| f.closure)
                    .map(|f| (&*d.name, f))
            })
            .collect();
        let in_firing = |at: usize| {
            ds.iter()
                .any(|d| d.firing.is_some() && d.start <= at && at < d.end)
        };
        let is_def = |at: usize| {
            let before = src.code[..at].trim_end();
            before.ends_with("fn")
                && !before[..before.len() - 2].ends_with(|c: char| c.is_alphanumeric() || c == '_')
        };

        let callees = firing
            .iter()
            .map(|(n, f)| (n.as_str(), f))
            .chain(local.iter().map(|(n, f)| (*n, *f)));
        for (name, f) in callees {
            let mut from = 0;
            while let Some(p) = src.code[from..].find(name) {
                let at = from + p;
                from = at + name.len();
                let after = src.code[from..].trim_start();
                if (at > 0 && is_ident(code[at - 1]))
                    || !after.starts_with('(')
                    || lit[at]
                    || is_def(at)
                {
                    continue;
                }
                let open = src.code.len() - after.len() + 1;
                let (args, _) = split_top(code, lit, open, None, false);
                let (Some(n), Some(l)) = (args.get(f.at), args.get(f.at + 1)) else {
                    continue;
                };
                let named = name_of(n);
                if other.contains(name)
                    && !local.contains_key(name)
                    && !matches!(named, Name::Lit(_))
                {
                    continue;
                }
                sites.push(Site {
                    path: src.path.clone(),
                    line: line_of(&src.code, at),
                    name: named,
                    list: l.clone(),
                    relay: bare(n, &[]) && bare(l, &[".to_vec()"]),
                    forward: in_firing(at),
                    pos: at,
                });
            }
        }

        for (at, tuple) in pushes(src, "pending_events") {
            match tuple {
                Some((n, l)) => sites.push(Site {
                    path: src.path.clone(),
                    line: line_of(&src.code, at),
                    name: name_of(&n),
                    relay: bare(&n, &[".to_string()"]) && bare(&l, &[]),
                    forward: in_firing(at),
                    pos: at,
                    list: l,
                }),
                None if !in_firing(at) => stray.push(format!(
                    "{}:{} pending_events.push(..)",
                    src.path,
                    line_of(&src.code, at)
                )),
                None => {}
            }
        }
        for (what, needle) in [
            ("a `Raised::Event` queued", "push(Raised::Event("),
            ("a frame's OnEvent dispatched", "fire_event_handler("),
        ] {
            let mut from = 0;
            while let Some(p) = src.code[from..].find(needle) {
                let at = from + p;
                from = at + needle.len();
                if !lit[at] && !is_def(at) && !in_firing(at) {
                    stray.push(format!("{}:{} {what}", src.path, line_of(&src.code, at)));
                }
            }
        }
    }

    // A relay outside a firing function takes its name and list from somewhere in its file: a
    // `("EVENT", list)` tuple. Read each as a site, so its list is gated like any other.
    let relay_files: BTreeSet<String> = sites
        .iter()
        .filter(|s| s.relay && !s.forward)
        .map(|s| s.path.clone())
        .collect();
    let mut tuples = Vec::new();
    for src in sources.iter().filter(|s| relay_files.contains(&s.path)) {
        for (at, _) in src.code.match_indices('(').filter(|(at, _)| !src.lit[*at]) {
            if !src.code[at + 1..].trim_start().starts_with('"') {
                continue;
            }
            let (parts, _) = split_top(src.code.as_bytes(), &src.lit, at + 1, None, false);
            let [n, l] = parts.as_slice() else { continue };
            if let Name::Lit(_) = name_of(n) {
                tuples.push(Site {
                    path: src.path.clone(),
                    line: line_of(&src.code, at),
                    name: name_of(n),
                    list: l.clone(),
                    relay: false,
                    forward: false,
                    pos: at,
                });
            }
        }
    }
    // The host queue's drain relays what the scanned `pending_events` sites pushed; any other
    // relay file must show its origin as a tuple, or it hides a forwarder the scan does not read.
    let unaccounted = relay_files
        .iter()
        .filter(|f| !DRAINS.contains(&f.as_str()) && !tuples.iter().any(|t| &t.path == *f))
        .cloned()
        .collect();
    sites.extend(tuples);

    // A name held in a binding is read from where the function binds it.
    let resolved: Vec<Option<Vec<String>>> = sites
        .iter()
        .map(|site| match &site.name {
            Name::Other(expr) if !site.relay => {
                let i = sources.iter().position(|x| x.path == site.path)?;
                resolve(&sources, &defs, i, site.pos, expr, 3)
            }
            _ => None,
        })
        .collect();
    for (site, names) in sites.iter_mut().zip(resolved) {
        if let Some(names) = names {
            site.name = Name::Lit(names);
        }
    }

    // A list held in a call is read from the one function it names.
    for site in sites.iter_mut().filter(|s| !s.relay) {
        if kinds_of_list(&site.list).is_none() {
            if let Some(list) = list_of_call(&sources, &defs, &site.list) {
                site.list = list;
            }
        }
    }

    Scan {
        sources,
        sites,
        firing: firing.into_keys().collect(),
        stray,
        unaccounted,
    }
}

#[test]
fn the_firing_functions_are_found_by_their_signature() {
    let s = scan();
    for name in [
        "fire_event",
        "fire_event_into",
        "queue_event",
        "raise",
        "fire_global",
    ] {
        assert!(
            s.firing.contains(name),
            "`{name}` takes `event: &str` and a `ScriptValue` list and is not found as a firing \
             function: {:?}",
            s.firing
        );
    }
    assert!(
        s.sites.len() > 300,
        "the scan found only {} fire sites — it stopped matching, which would make every gate \
         that reads it silently vacuous",
        s.sites.len()
    );
}

/// Nothing enters the host queue or reaches a frame's `OnEvent` except inside a firing function
/// or as a literal `pending_events.push((name, list))`, which is a site. So a fire spelled any
/// other way, with a signature the scan does not read, fails here and not unseen.
#[test]
fn every_queue_and_dispatch_sink_sits_in_a_firing_function() {
    let s = scan();
    assert!(
        s.stray.is_empty(),
        "an event is queued or dispatched outside a function taking `event: &str` and a \
         `ScriptValue` list, which the fire scan reads:\n  {}",
        s.stray.join("\n  ")
    );
}

/// The scan of `files` as `(path, text)` instead of the workspace, for the tests that show each
/// gate refuses what it guards.
pub(super) fn scan_of(files: &[(&str, &str)]) -> Scan {
    build(files.iter().map(|(p, t)| Src::new(p, t)).collect())
}

/// A site's names, or `[]` for a name the scan does not read.
#[cfg(test)]
fn names_of(s: &Scan) -> Vec<String> {
    s.sites
        .iter()
        .flat_map(|x| match &x.name {
            Name::Lit(n) => n.clone(),
            Name::Other(_) => Vec::new(),
        })
        .collect()
}

const WRAPPERS: &str = r#"
    impl UiScript {
        pub fn fire_event(&mut self, event: &str, args: Vec<ScriptValue>) {}
        pub fn queue_event(&mut self, event: &str, args: Vec<ScriptValue>) {}
    }
    impl Model {
        pub(crate) fn raise(&mut self, event: &str, args: Vec<ScriptValue>) {}
    }
    fn fire_event_into(lua: &Lua, event: &str, args: Vec<ScriptValue>) {
        self.pending_events.push((event.to_string(), args));
    }
"#;

#[test]
fn every_spelling_of_a_fire_is_a_site() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "a.rs",
            r#"
            fn a(script: &mut UiScript, model: &mut Model, lua: &Lua) {
                script.fire_event("A_ONE", vec![]);
                script.queue_event("A_TWO", vec![]);
                model.raise("A_THREE", Vec::new());
                fire_event_into(lua, "A_FOUR", vec![]);
                model
                    .pending_events
                    .push(("A_FIVE".to_string(), vec![]));
                let fire = |event: &str, args: Vec<ScriptValue>| fire_event_into(lua, event, args);
                fire("A_SIX", vec![]);
                let ev = if x { "A_SEVEN" } else { "A_EIGHT" };
            }
            "#,
        ),
    ]);
    let mut names = names_of(&s);
    names.sort();
    assert_eq!(
        names,
        ["A_FIVE", "A_FOUR", "A_ONE", "A_SIX", "A_THREE", "A_TWO"],
        "{:?}",
        s.sites
    );
    assert!(s.stray.is_empty(), "{:?}", s.stray);
}

/// A wrapper written tomorrow is scanned tomorrow: it is found by its signature, not a list.
#[test]
fn a_new_wrapper_is_found_by_its_signature() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "b.rs",
            r#"
            impl Bar {
                fn notify(&mut self, id: u32, event: &str, args: &[ScriptValue]) {}
            }
            fn b(bar: &mut Bar) { bar.notify(1, "B_ONE", &[]); }
            "#,
        ),
    ]);
    assert_eq!(names_of(&s), ["B_ONE"]);
    let b = s.sites.iter().find(|x| x.path == "b.rs").unwrap();
    assert_eq!(b.kinds(), Some(vec![]));
}

/// A name another `fn` also carries (`raise` is also "bring to front") claims only an
/// event-shaped literal, so the frame verb is no site and the event is.
#[test]
fn an_ambiguous_name_claims_only_an_event_shaped_literal() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "c.rs",
            r#"
            fn raise(model: &mut Model, h: FrameHandle) {}
            fn c(model: &mut Model) { raise(model, h); model.raise("C_ONE", vec![]); }
            "#,
        ),
    ]);
    assert_eq!(names_of(&s), ["C_ONE"]);
}

/// A `#[cfg(test)] mod` is not shipped wherever it sits; a `#[cfg(test)]` item before more code
/// does not end the file.
#[test]
fn a_test_module_is_skipped_and_the_code_after_a_test_item_is_not() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "d.rs",
            r#"
            #[cfg(test)]
            fn only_in_tests() {}
            fn d(script: &mut UiScript) { script.fire_event("D_ONE", vec![]); }
            #[cfg(test)]
            mod tests {
                fn t(script: &mut UiScript) { script.fire_event("D_TEST", vec![]); }
            }
            fn e(script: &mut UiScript) { script.fire_event("D_TWO", vec![]); }
            "#,
        ),
    ]);
    assert_eq!(names_of(&s), ["D_ONE", "D_TWO"]);
}

/// A comment or a string that spells a fire is not one.
#[test]
fn a_comment_or_a_string_is_not_a_fire() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "e.rs",
            r#"
            // script.fire_event("E_COMMENT", vec![]);
            const C: &[&str] = &["fire_event(", "queue_event(\"E_STRING\", vec![])"];
            "#,
        ),
    ]);
    assert!(names_of(&s).is_empty(), "{:?}", s.sites);
}

/// The queue and the dispatch are reached only inside a firing function: another way in is
/// stray, however it is spelled.
#[test]
fn a_sink_outside_a_firing_function_is_stray() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "f.rs",
            r#"
            fn sneaky(model: &mut Model, which: u8, args: Vec<ScriptValue>) {
                let name = pick(which);
                model.pending_events.push(entry);
                fire_event_handler(lua, 0, name, &args);
            }
            "#,
        ),
    ]);
    assert_eq!(s.stray.len(), 2, "{:?}", s.stray);
}

/// A relay outside a firing function forwards a name and a list built elsewhere. Its origin must
/// be in view: the queue's drain, or `("EVENT", list)` tuples in the same file. Otherwise the
/// forwarder is a wrapper this scan does not read, and every fire through it is unseen.
#[test]
fn every_relay_has_its_origin_in_view() {
    let s = scan();
    assert!(
        s.unaccounted.is_empty(),
        "these files forward an event's name and list from bindings the fire scan cannot trace: \
         {:?}",
        s.unaccounted
    );
}

#[test]
fn a_forwarder_the_scan_cannot_trace_is_unaccounted() {
    let s = scan_of(&[
        ("w.rs", WRAPPERS),
        (
            "g.rs",
            r#"
            fn forward(script: &mut UiScript, which: &str, list: Vec<Foo>) {
                let (event, args) = pick(which, list);
                script.fire_event(event, args);
            }
            "#,
        ),
        (
            "h.rs",
            r#"
            fn pick(script: &mut UiScript, e: u8) {
                let (event, args) = match e { 0 => ("H_ONE", vec![]), _ => ("H_TWO", vec![]) };
                script.fire_event(event, args);
            }
            "#,
        ),
    ]);
    assert_eq!(s.unaccounted, ["g.rs"]);
    let mut names = names_of(&s);
    names.sort();
    assert_eq!(names, ["H_ONE", "H_TWO"]);
}
