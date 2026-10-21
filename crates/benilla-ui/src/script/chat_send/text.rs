//! The text steps `SendChatMessage` (`0x49f1e0`) and `SendAddonMessage` (`0x49f920`) run on a line
//! before its packet: the `%` expander and the `|`-escape scan.

/// `Invalid escape code in chat message`, `0x844a4c`, which both verbs raise (`0x49f763`,
/// `0x49fa6f`).
pub const INVALID_ESCAPE: &str = "Invalid escape code in chat message";

/// A `|` followed by anything but `|` or `c`: the verb raises [`INVALID_ESCAPE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEscape;

/// The `|`-escape scan, duplicated in the two verbs (`0x49f661`-`0x49f6a8`, `0x49f9bb`-`0x49fa04`).
/// After each `|`: `||` skips both bytes; `|c` skips past the first later `|r` (case-sensitive
/// `strstr` `0x64b4f0`, resuming at the closer plus 2, `0x49f9f4`), leaving the run between
/// unchecked; `|c` with no `|r` cuts `text` at the `|` and ends the scan (`0x49f758`, `0x49fa6a`);
/// anything else, the NUL after a trailing `|` included, is [`InvalidEscape`].
pub fn validate_escapes(text: &mut String) -> Result<(), InvalidEscape> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'|') {
        let pipe = i + off;
        match bytes.get(pipe + 1) {
            Some(b'|') => i = pipe + 2,
            Some(b'c') => {
                // The reference's `strstr` starts at the `|c`, which cannot itself match `|r`.
                match bytes[pipe..]
                    .windows(2)
                    .position(|w| w == b"|r")
                    .map(|p| pipe + p)
                {
                    // Past the closer: its `|` must not reach the loop head, where `|r` raises.
                    Some(closer) => i = closer + 2,
                    // No closer: the reference cuts at the `|` and accepts.
                    None => {
                        text.truncate(pipe);
                        return Ok(());
                    }
                }
            }
            // Anything else, including `None`: the NUL after a trailing `|`.
            _ => return Err(InvalidEscape),
        }
    }
    Ok(())
}

/// `0x5083a0`, the `%` expander `SendChatMessage` copies its text through (`0x49f60f`): each `%`
/// followed by `N`, `T`, `n` or `t` (the byte table `0x50852c`, read through `0x508524`) becomes
/// `target()`'s answer, the selection's name; after any other byte, the end included, the `%` is
/// kept and that byte is read again as text (`0x5084d0`), so `%%t` is a `%` and the name. The text
/// is a C string, so it ends at its first NUL. The copy is not bounded at 255 bytes, as
/// `chat_send`'s deviation says.
pub fn expand_target_tokens(text: &str, mut target: impl FnMut() -> String) -> String {
    let text = &text[..text.find('\0').unwrap_or(text.len())];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        match rest.as_bytes().get(at + 1) {
            Some(b'N' | b'T' | b'n' | b't') => {
                out.push_str(&target());
                rest = &rest[at + 2..];
            }
            _ => {
                out.push('%');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::{expand_target_tokens, validate_escapes};

    /// `0x5083a0` against a target named Bob.
    #[test]
    fn the_expander_names_the_target_for_its_four_letters_only() {
        let bob = || "Bob".to_string();
        for (text, want) in [
            ("heal %t please", "heal Bob please"),
            ("%T%n%N", "BobBobBob"),
            ("100%", "100%"),
            ("%x %s %%", "%x %s %%"),
            ("%%t", "%Bob"),
            ("%%%t", "%%Bob"),
            ("%", "%"),
            ("no tokens", "no tokens"),
            ("%t\0%t", "Bob"),
            ("é%té", "éBobé"),
        ] {
            assert_eq!(expand_target_tokens(text, bob), want, "{text:?}");
        }
    }

    /// `0x5083a0` asks the target's name at each token (`0x50844a`), never for text without one.
    #[test]
    fn the_name_is_read_once_per_token() {
        let mut asked = 0;
        expand_target_tokens("%t and %n", || {
            asked += 1;
            String::new()
        });
        assert_eq!(asked, 2);
        expand_target_tokens("%x", || {
            asked += 1;
            String::new()
        });
        assert_eq!(asked, 2);
    }

    /// The scan of `0x49f661`-`0x49f6a8`: `Ok` with the text it sends, or `Err` for the raise.
    #[test]
    fn the_escape_scan_passes_pipes_and_colors_and_cuts_an_unclosed_color() {
        for (text, want) in [
            ("plain", Some("plain")),
            ("a||b", Some("a||b")),
            ("|cffff0000red|r", Some("|cffff0000red|r")),
            (
                "|cff|Hitem:1|h[x]|h|r after",
                Some("|cff|Hitem:1|h[x]|h|r after"),
            ),
            ("x |cffno closer", Some("x ")),
            ("x |cff|R", Some("x ")),
            ("a|b", None),
            ("|r", None),
            ("|C", None),
            ("trailing |", None),
            ("|||", None),
            ("|cff|r|x", None),
        ] {
            let mut t = text.to_string();
            let got = validate_escapes(&mut t).ok().map(|()| t);
            assert_eq!(got.as_deref(), want, "{text:?}");
        }
    }
}
