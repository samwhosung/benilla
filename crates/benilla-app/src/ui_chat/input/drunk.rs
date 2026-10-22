//! The drunk transform `0x508680`, which `SendChatMessage` runs a drunk player's line through after
//! the `%` expander and before the escape scan (`0x49f614`-`0x49f656`).

/// `0x508680(text, dst, cap, strength)`: copy `text` into `cap` bytes (`0x5086b7` stops with one
/// byte left), slurring by `strength`, the drunk fraction. A word starts at the line's start or after
/// a space (`0x5086eb`). An `s` or `S` starting a word gets an `h` after it when the next byte is
/// not a space, `s` or `S` (`0x508ad0`), a draw falls below `strength` (`0x50883f`) and `masks`, the
/// profanity masker `0x4a1a60` (`0x508891`), finds nothing in `sh` and the rest of the word
/// (`0x50884d`-`0x508889`); one ending a word before a space gets an `h` on a draw alone
/// (`0x5088b1`-`0x5089b5`). A `|c` with a later `|r` is copied whole, unread (`0x5086f2`-`0x50873b`).
/// Last, with a draw below a quarter of `strength` (`0x5089f7`-`0x508a04`), the line becomes
/// `slurred_speech` (`SLURRED_SPEECH`, `0x84f3bc`) filled with it (`0x508a3a`), cut to `cap`.
///
/// Each draw is `word()`'s low 23 bits as a float in `[0, 1)`: `0x4531e0`'s word over the mantissa
/// of 1.0, less 1.0 (`0x50881e`-`0x50882f`, `0x5089de`-`0x5089ee`).
pub(super) fn slur(
    text: &str,
    strength: f32,
    cap: usize,
    mut word: impl FnMut() -> u32,
    mut masks: impl FnMut(&mut String) -> bool,
    slurred_speech: &str,
) -> String {
    let mut draw = || f32::from_bits((word() & 0x7f_ffff) | 0x3f80_0000) - 1.0;
    let src = text.as_bytes();
    let src = &src[..src.iter().position(|&b| b == 0).unwrap_or(src.len())];
    let mut out = Vec::with_capacity(src.len() + 8);
    // `edi`, the bytes left in the destination, its NUL's included.
    let mut room = cap;
    let mut word_start = true;
    let mut i = 0;
    while i < src.len() && room > 1 {
        let c = src[i];
        out.push(c);
        i += 1;
        room -= 1;
        match c {
            b' ' => {
                word_start = true;
                continue;
            }
            b'|' if src.get(i) == Some(&b'c') => {
                // `SStrStr2` from the `c` (`0x508702`); the run through the `|r` is copied, cut to
                // the room (`0x50871c`-`0x508729`), and the read goes on after the closer.
                if let Some(at) = src[i..].windows(2).position(|w| w == b"|r") {
                    let n = (at + 2).min(room - 1);
                    out.extend_from_slice(&src[i..i + n]);
                    room -= n;
                    i += at + 2;
                }
            }
            b's' | b'S' if word_start => {
                let next = src.get(i).copied().unwrap_or(0);
                if room > 1 && !matches!(next, b' ' | b's' | b'S') && draw() < strength {
                    let tail = src[i..].split(|&b| b == b' ').next().unwrap_or_default();
                    let mut probe = String::from("sh");
                    probe.push_str(&String::from_utf8_lossy(tail));
                    if !masks(&mut probe) {
                        out.push(b'h');
                        room -= 1;
                    }
                }
            }
            b's' | b'S' if room > 1 && src.get(i) == Some(&b' ') && draw() < strength => {
                out.push(b'h');
                room -= 1;
            }
            _ => {}
        }
        word_start = false;
    }
    let mut line = utf8_prefix(out);
    if draw() < strength * 0.25 {
        line = super::super::away::fill_one(slurred_speech, &line);
        cut_to(&mut line, cap.saturating_sub(1));
    }
    line
}

/// The bytes as text, an incomplete character a cut left at the end dropped.
fn utf8_prefix(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| {
        let valid = e.utf8_error().valid_up_to();
        let mut bytes = e.into_bytes();
        bytes.truncate(valid);
        String::from_utf8(bytes).unwrap_or_default()
    })
}

/// `text` cut to at most `max` bytes, backed off to a char boundary.
fn cut_to(text: &mut String, max: usize) {
    if text.len() > max {
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
}

#[cfg(test)]
mod tests {
    use super::slur;

    /// A word source whose draws are `draws` in turn, each as the float `0x508680` compares.
    fn words(draws: &[f32]) -> impl FnMut() -> u32 + '_ {
        let mut it = draws.iter();
        move || (it.next().expect("a draw") + 1.0).to_bits() & 0x7f_ffff
    }

    fn slurred(text: &str, strength: f32, draws: &[f32]) -> String {
        slur(text, strength, 0x100, words(draws), |_| false, "%s ...hic!")
    }

    /// An `s` starting a word or ending one before a space takes an `h` on a draw below the
    /// strength (`0x50883f`, `0x5089a1`); a draw at it does not (`fcomp` `jp`); the last draw, below
    /// a quarter of it, wraps the line (`0x5089f7`-`0x508a04`).
    #[test]
    fn an_s_at_a_words_edge_slurs_on_a_draw_below_the_strength() {
        // "sorry" (start), "its " (end before a space), "glass" (end of line: no draw).
        assert_eq!(
            slurred("sorry its glass", 0.5, &[0.1, 0.2, 0.9]),
            "shorry itsh glass"
        );
        assert_eq!(slurred("sorry", 0.5, &[0.5, 0.9]), "sorry", "a draw at it");
        assert_eq!(
            slurred("Sorry", 0.5, &[0.0, 0.124]),
            "Shorry ...hic!",
            "a draw below a quarter wraps"
        );
        assert_eq!(slurred("Sorry", 0.5, &[0.0, 0.125]), "Shorry");
    }

    /// No draw for an `s` that starts a word before a space, `s` or `S` (`0x508ad0`), one inside
    /// a word, or one ending the line; the word's start is the line's or a space's (`0x5086eb`).
    #[test]
    fn only_a_words_edge_draws() {
        for text in ["s x", "ss", "sSx", "ask as"] {
            assert_eq!(slurred(text, 1.0, &[0.9]), text, "the last draw alone");
        }
        assert_eq!(slurred("  so", 1.0, &[0.0, 0.9]), "  sho");
        assert_eq!(slurred("s", 1.0, &[0.0, 0.9]), "sh", "the NUL is no stop");
        assert_eq!(
            slurred("x,so", 1.0, &[0.9]),
            "x,so",
            "a comma starts no word"
        );
    }

    /// The masker sees `sh` and the rest of the word (`0x50884d`-`0x508889`), and a match keeps
    /// the `h` out; the word's end takes no mask test.
    #[test]
    fn a_masked_sh_word_takes_no_h() {
        let mut seen = Vec::new();
        let out = slur(
            "sit sap its x",
            1.0,
            0x100,
            words(&[0.0, 0.0, 0.0, 0.9]),
            |probe| {
                seen.push(probe.clone());
                probe == "shit"
            },
            "%s ...hic!",
        );
        assert_eq!(out, "sit shap itsh x");
        assert_eq!(seen, ["shit", "shap"]);
    }

    /// A `|c` run through its `|r` is copied unread (`0x5086f2`-`0x50873b`); a `|c` with no closer,
    /// and any other `|`, is plain text.
    #[test]
    fn a_colour_run_is_copied_whole() {
        assert_eq!(slurred("|cffs s|r so", 1.0, &[0.0, 0.9]), "|cffs s|r sho");
        assert_eq!(slurred("|cso", 1.0, &[0.9]), "|cso");
    }

    /// The copy stops with one byte of `cap` left (`0x5086b7`), and the wrap is cut to it
    /// (`0x508a3a`).
    #[test]
    fn the_copy_and_the_wrap_keep_to_the_capacity() {
        let out = slur("so so", 1.0, 5, words(&[0.0, 0.9]), |_| false, "%s!");
        assert_eq!(out, "sho ");
        let out = slur("ab", 1.0, 4, words(&[0.0]), |_| false, "%s!!");
        assert_eq!(out, "ab!");
    }
}
