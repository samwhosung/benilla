//! Helpers for the crate's **structural tests** — the ones that read this crate's own source
//! and insist on a shape. A structural test exists where the
//! failure is silent at runtime, so the check has to happen at the line; these are the readers
//! they share.

/// Every `.rs` file under `root`, recursively.
pub(crate) fn rust_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    out
}

/// This crate's `src/` directory.
pub(crate) fn src_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// A file's path under `src/`, with forward slashes — the key every `EXEMPT` table uses.
pub(crate) fn rel_path(file: &std::path::Path) -> String {
    let src = src_dir();
    file.strip_prefix(&src)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/")
}

/// One `fn` item as the scanners see it: its name, its parenthesised parameter list, and its
/// body (the text between the outermost braces), each found by bracket matching so a scan
/// sees a signature or a body rather than whatever text happens to follow.
pub(crate) struct FnItem<'a> {
    pub name: &'a str,
    pub params: &'a str,
    pub body: &'a str,
}

/// Every `fn` in `text`. A `fn` with no body (a trait method signature) comes back with an
/// empty body; a generic parameter list before the `(` is tolerated in the shapes this
/// codebase writes.
pub(crate) fn fn_items(text: &str) -> Vec<FnItem<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (i, _) in text.match_indices("fn ") {
        // `fn` must start a word: `…_fn (` and `Fn(` are not declarations.
        if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
            continue;
        }
        let Some(open) = text[i..].find('(').map(|o| i + o) else {
            continue;
        };
        let name = text[i + 3..open].trim();
        let name = name.split('<').next().unwrap_or(name).trim();
        let Some(close) = matching(text, open, b'(', b')') else {
            continue;
        };
        let params = &text[open + 1..close];
        // The body: the first `{` after the parameter list, unless a `;` (a bodiless
        // signature) or another `fn` comes first.
        let tail = &text[close + 1..];
        let body = match (tail.find('{'), tail.find(';')) {
            (Some(b), Some(s)) if s < b => "",
            (Some(b), _) => {
                let bopen = close + 1 + b;
                match matching(text, bopen, b'{', b'}') {
                    Some(bclose) => &text[bopen + 1..bclose],
                    None => "",
                }
            }
            (None, _) => "",
        };
        out.push(FnItem { name, params, body });
    }
    out
}

/// The parenthesised parameter list of every `fn` in `text` — see [`fn_items`].
pub(crate) fn fn_parameter_lists(text: &str) -> Vec<&str> {
    fn_items(text).into_iter().map(|f| f.params).collect()
}

/// The index of the bracket closing the one at `open`, or `None` if it never closes.
fn matching(text: &str, open: usize, lhs: u8, rhs: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in text.as_bytes()[open..].iter().enumerate() {
        if c == lhs {
            depth += 1;
        } else if c == rhs {
            depth -= 1;
            if depth == 0 {
                return Some(open + j);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fn_comes_back_with_its_name_its_params_and_its_body() {
        let text = "fn a(x: u32) -> u32 { x + 1 }\nfn b();\nfn c<T>(t: T) { if t { { } } }";
        let items = fn_items(text);
        assert_eq!(items.len(), 3);
        assert_eq!(
            (items[0].name, items[0].params, items[0].body),
            ("a", "x: u32", " x + 1 ")
        );
        assert_eq!(
            (items[1].name, items[1].params, items[1].body),
            ("b", "", "")
        );
        assert_eq!(
            (items[2].name, items[2].params, items[2].body),
            ("c", "t: T", " if t { { } } ")
        );
    }
}
