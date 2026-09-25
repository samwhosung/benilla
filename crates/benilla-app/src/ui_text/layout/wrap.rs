//! The pure word side of the wrap: [`WrapWord`] tokenizing, the greedy packer and the run rejoin.
//! [`super::wrap_line`] binds the real measure and states the break law; the tests run on stubs.

use crate::ui_text::markup::ColorRun;

/// Slack on the wrap width: `max_width` can arrive a few ulps under the content it was sized to,
/// which must not force-break; far below a glyph step (≥ 3 px), far above that dust (≤ 1e-3).
const WIDTH_EPS: f32 = 0.25;

/// A hyperlink handle, shared by every run or piece the link's visible text splits into.
type Link = Option<std::sync::Arc<crate::ui_text::markup::LinkInfo>>;

/// Two handles name the same link by pointer identity: markup shares one `Arc` per link.
fn same_link(a: &Link, b: &Link) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => std::sync::Arc::ptr_eq(x, y),
        _ => false,
    }
}

/// One break unit and the verbatim whitespace before it (`lead`, empty first on a line), so a
/// double space survives the wrap. A colour or link boundary can fall inside a word
/// (`[Chipped Claw]x2.`), so a word carries styled pieces, not one colour.
pub(super) struct WrapWord {
    /// The styled pieces in source order: never empty, and never an empty piece.
    pieces: Vec<ColorRun>,
    lead: String,
}

impl WrapWord {
    /// The word's visible text, what the packer measures and force-breaks.
    fn text(&self) -> String {
        self.pieces.iter().map(|p| p.text.as_str()).collect()
    }
}

/// Append `ch` under `run`'s style, extending the last piece when the style matches: the one place
/// a piece opens, so none is empty.
fn push_char(pieces: &mut Vec<ColorRun>, ch: char, run: &ColorRun) {
    match pieces.last_mut() {
        Some(last) if last.color == run.color && same_link(&last.link, &run.link) => {
            last.text.push(ch);
        }
        _ => pieces.push(ColorRun {
            text: ch.to_string(),
            color: run.color,
            link: run.link.clone(),
        }),
    }
}

/// Split a word's pieces at byte `at` of their joined text, a char boundary: the force-break point.
fn split_pieces(pieces: &[ColorRun], at: usize) -> (Vec<ColorRun>, Vec<ColorRun>) {
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let mut off = 0usize;
    for p in pieces {
        let end = off + p.text.len();
        if end <= at {
            head.push(p.clone());
        } else if off >= at {
            tail.push(p.clone());
        } else {
            let cut = at - off;
            head.push(ColorRun {
                text: p.text[..cut].to_string(),
                ..p.clone()
            });
            tail.push(ColorRun {
                text: p.text[cut..].to_string(),
                ..p.clone()
            });
        }
        off = end;
    }
    (head, tail)
}

/// Split a markup line's runs into [`WrapWord`]s; whitespace leading the line rides the first
/// word's `lead` and drops at emit. Only whitespace ends a word: a colour or link boundary splits
/// [pieces](WrapWord::pieces), never words, so `[Chipped Claw]x2.` never wraps before the `x`.
pub(super) fn tokenize_words(line: &[ColorRun]) -> Vec<WrapWord> {
    let mut words: Vec<WrapWord> = Vec::new();
    let mut cur: Vec<ColorRun> = Vec::new();
    let mut lead = String::new();
    for run in line {
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                if !cur.is_empty() {
                    words.push(WrapWord {
                        pieces: std::mem::take(&mut cur),
                        lead: std::mem::take(&mut lead),
                    });
                }
                lead.push(ch);
            } else {
                push_char(&mut cur, ch, run);
            }
        }
    }
    if !cur.is_empty() {
        words.push(WrapWord { pieces: cur, lead });
    }
    words
}
/// The greedy packer: fill each line left to right, breaking before the first word that would pass
/// `max_width`; `measure` also prices each separator. Assumes `words` is non-empty.
///
/// A word too wide alone force-breaks at its last fitting glyph, as the reference drops the
/// exceeding glyph and ends a line with no opportunity (`0x5c7780`; `0x5c7623` `fcomp`, `0x5c762b`
/// `je`); when not one glyph fits, its builder bails and the rest of the source line drops.
pub(super) fn greedy_pack<F: FnMut(&str) -> f32>(
    words: Vec<WrapWord>,
    max_width: f32,
    mut measure: F,
) -> Vec<Vec<ColorRun>> {
    // One slack covers the pack test and the force-break walk.
    let max_width = max_width + WIDTH_EPS;
    let mut out: Vec<Vec<WrapWord>> = Vec::new();
    let mut cur: Vec<WrapWord> = Vec::new();
    let mut cur_w = 0.0f32;
    for word in words {
        let ww = measure(&word.text());
        if !cur.is_empty() {
            let candidate = cur_w + measure(&word.lead) + ww;
            if candidate <= max_width {
                cur_w = candidate;
                cur.push(word);
                continue;
            }
            // Break before this word, the last opportunity.
            out.push(std::mem::take(&mut cur));
        }
        // A word too wide alone force-breaks into full-line chunks; the remainder keeps packing.
        let mut word = word;
        let mut ww = ww;
        while ww > max_width {
            let text = word.text();
            let Some(at) = split_at_last_fitting_glyph(&text, max_width, &mut measure) else {
                // Not one glyph fits: bail, dropping the rest of the line, as the reference does.
                return finish_pack(out, cur);
            };
            let (head, rest) = split_pieces(&word.pieces, at);
            out.push(vec![WrapWord {
                pieces: head,
                lead: std::mem::take(&mut word.lead),
            }]);
            word.pieces = rest;
            ww = measure(&word.text());
        }
        cur_w = ww;
        cur.push(word);
    }
    finish_pack(out, cur)
}

/// Close the packer: flush the open line and rejoin every line's words into runs.
fn finish_pack(mut out: Vec<Vec<WrapWord>>, cur: Vec<WrapWord>) -> Vec<Vec<ColorRun>> {
    if !cur.is_empty() {
        out.push(cur);
    }
    out.iter().map(|ws| words_to_runs(ws)).collect()
}

/// The force-break point past the longest glyph prefix within `max_width`, `None` when not even
/// one glyph fits; re-measuring each prefix agrees with a step sum, as the law is additive.
fn split_at_last_fitting_glyph<F: FnMut(&str) -> f32>(
    text: &str,
    max_width: f32,
    measure: &mut F,
) -> Option<usize> {
    let mut fit_end = 0usize;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        if measure(&text[..end]) > max_width {
            break;
        }
        fit_end = end;
    }
    (fit_end > 0).then_some(fit_end)
}

/// Rejoin a wrapped line's words into runs: same colour and link pieces merge, each `lead` joins
/// the run before it, and the line's first word drops its `lead`, the separator at the break.
fn words_to_runs(words: &[WrapWord]) -> Vec<ColorRun> {
    let mut runs: Vec<ColorRun> = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            if let Some(last) = runs.last_mut() {
                last.text.push_str(&word.lead);
            }
        }
        for piece in &word.pieces {
            match runs.last_mut() {
                Some(last) if last.color == piece.color && same_link(&last.link, &piece.link) => {
                    last.text.push_str(&piece.text);
                }
                _ => runs.push(piece.clone()),
            }
        }
    }
    runs
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    /// A stub measure: every char, space included, is one unit wide.
    fn char_measure(s: &str) -> f32 {
        s.chars().count() as f32
    }

    /// A single-piece word with the given separator.
    fn word(text: &str, color: [f32; 4], lead: &str) -> WrapWord {
        WrapWord {
            pieces: vec![ColorRun {
                text: text.to_string(),
                color,
                link: None,
            }],
            lead: lead.to_string(),
        }
    }

    fn words(pairs: &[(&str, [f32; 4])]) -> Vec<WrapWord> {
        // A single-space separator; the first word's `lead` is dropped at emit.
        pairs.iter().map(|(t, c)| word(t, *c, " ")).collect()
    }

    /// A wrapped line's text, colours dropped.
    fn line_text(runs: &[ColorRun]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    #[test]
    fn long_line_breaks_at_word_boundaries_within_width() {
        // Width 12: "Refreshing" (10) plus " Spring" (7) passes it; "Spring Water" is exactly 12.
        let w = words(&[("Refreshing", WHITE), ("Spring", WHITE), ("Water", WHITE)]);
        let lines = greedy_pack(w, 12.0, char_measure);
        assert_eq!(lines.len(), 2, "wraps to two lines");
        assert_eq!(line_text(&lines[0]), "Refreshing");
        assert_eq!(line_text(&lines[1]), "Spring Water");
        for l in &lines {
            assert!(line_text(l).chars().count() <= 12);
        }
    }

    #[test]
    fn short_line_stays_single() {
        let w = words(&[("Buy", WHITE), ("now", WHITE)]);
        let lines = greedy_pack(w, 100.0, char_measure);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "Buy now");
    }

    #[test]
    fn overlong_word_force_breaks_at_the_last_fitting_glyph() {
        let w = words(&[("Supercalifragilistic", WHITE), ("ok", WHITE)]);
        let lines = greedy_pack(w, 8.0, char_measure);
        assert_eq!(lines.len(), 3);
        assert_eq!(line_text(&lines[0]), "Supercal");
        assert_eq!(line_text(&lines[1]), "ifragili");
        // The remainder packs on with the next word.
        assert_eq!(line_text(&lines[2]), "stic ok");
        for l in &lines {
            assert!(
                line_text(l).chars().count() <= 8,
                "no line exceeds the width"
            );
        }
    }

    #[test]
    fn force_break_mid_line_starts_from_the_break_opportunity() {
        let w = words(&[("at", WHITE), ("Supercalifragilistic", WHITE)]);
        let lines = greedy_pack(w, 8.0, char_measure);
        assert_eq!(line_text(&lines[0]), "at");
        assert_eq!(line_text(&lines[1]), "Supercal");
        assert_eq!(line_text(&lines[2]), "ifragili");
        assert_eq!(line_text(&lines[3]), "stic");
    }

    /// A box a few ulps under its content does not force-break; one a glyph too narrow does.
    #[test]
    fn content_exact_width_with_float_dust_does_not_break() {
        // One "word" of two 5.5-unit glyphs (11.0 total) in a box 15 ulps shy of 11.0.
        let measure = |s: &str| s.chars().count() as f32 * 5.5;
        let w = vec![word("34", WHITE, "")];
        let lines = greedy_pack(w, 11.0 - 0.000015, measure);
        assert_eq!(lines.len(), 1, "float dust must not split the digits");
        assert_eq!(line_text(&lines[0]), "34");

        // One glyph short.
        let w2 = vec![word("34", WHITE, "")];
        let lines2 = greedy_pack(w2, 5.5, measure);
        assert_eq!(lines2.len(), 2, "a real overflow still breaks");
    }

    #[test]
    fn sub_glyph_width_bails_without_progress() {
        // Not one glyph fits: the builder bails and drops the line, as the reference's does.
        let w = words(&[("ab", WHITE)]);
        let lines = greedy_pack(w, 0.5, char_measure);
        assert!(lines.is_empty());
    }

    #[test]
    fn tokenize_preserves_internal_whitespace() {
        // A double space after a period survives as the next word's `lead`.
        let line = vec![ColorRun {
            text: "Hello.  World again".to_string(),
            color: WHITE,
            link: None,
        }];
        let ws = tokenize_words(&line);
        assert_eq!(ws.len(), 3);
        assert_eq!(ws[0].text(), "Hello.");
        assert_eq!(ws[0].lead, "", "first word has no separator");
        assert_eq!(ws[1].text(), "World");
        assert_eq!(ws[1].lead, "  ", "the double space is kept verbatim");
        assert_eq!(ws[2].text(), "again");
        assert_eq!(ws[2].lead, " ");
    }

    #[test]
    fn wrap_preserves_double_space_between_words() {
        let w = vec![word("Hello.", WHITE, ""), word("World", WHITE, "  ")];
        let lines = greedy_pack(w, 100.0, char_measure);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "Hello.  World");

        // Width 6 fits "Hello." alone; the separator at the break drops.
        let w2 = vec![word("Hello.", WHITE, ""), word("World", WHITE, "  ")];
        let lines2 = greedy_pack(w2, 6.0, char_measure);
        assert_eq!(lines2.len(), 2);
        assert_eq!(line_text(&lines2[0]), "Hello.");
        assert_eq!(line_text(&lines2[1]), "World");
    }

    #[test]
    fn color_runs_survive_the_wrap() {
        // Same-colour words merge, a new colour starts a run, and the space joins the run before.
        let w = words(&[("aa", WHITE), ("bb", WHITE), ("cc", RED)]);
        let lines = greedy_pack(w, 100.0, char_measure);
        assert_eq!(lines.len(), 1);
        let runs = &lines[0];
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "aa bb ");
        assert_eq!(runs[0].color, WHITE);
        assert_eq!(runs[1].text, "cc");
        assert_eq!(runs[1].color, RED);
    }

    /// Text typed straight after a chat link, no space between: `Bearer]` and the typed text are
    /// one break unit in two pieces, and the typed text keeps its own colour, outside the link.
    #[test]
    fn a_word_straddling_a_color_boundary_keeps_both_colors() {
        let link = std::sync::Arc::new(crate::ui_text::markup::LinkInfo {
            link: "item:13984:0:0:0".into(),
            markup: "|Hitem:13984:0:0:0|h[The Plague Bearer]|h".into(),
        });
        let line = vec![
            ColorRun {
                text: "[The Plague Bearer]".into(),
                color: RED,
                link: Some(link.clone()),
            },
            ColorRun {
                text: "dsfsdfsd".into(),
                color: WHITE,
                link: None,
            },
        ];
        // The colour boundary is no break opportunity: one word, two pieces.
        let ws = tokenize_words(&line);
        assert_eq!(ws.len(), 3, "three whitespace-delimited break units");
        assert_eq!(ws[2].text(), "Bearer]dsfsdfsd");
        assert_eq!(ws[2].pieces.len(), 2);

        let lines = greedy_pack(ws, 100.0, char_measure);
        assert_eq!(lines.len(), 1);
        let runs = &lines[0];
        assert_eq!(runs.len(), 2, "the color boundary survives the round trip");
        assert_eq!(runs[0].text, "[The Plague Bearer]");
        assert_eq!(runs[0].color, RED);
        assert!(runs[0].link.is_some(), "the name stays clickable");
        assert_eq!(runs[1].text, "dsfsdfsd");
        assert_eq!(runs[1].color, WHITE, "typed text keeps the base color");
        assert!(runs[1].link.is_none(), "typed text is not part of the link");
    }

    /// A colour boundary in a force-broken word splits with it; no chunk takes the other's colour.
    #[test]
    fn a_force_break_splits_a_words_pieces_with_it() {
        let line = vec![
            ColorRun {
                text: "[Claw]".into(),
                color: RED,
                link: None,
            },
            ColorRun {
                text: "x2.".into(),
                color: WHITE,
                link: None,
            },
        ];
        // Width 4: "[Cla" | "w]x2" | ".", the colour boundary inside the middle chunk.
        let lines = greedy_pack(tokenize_words(&line), 4.0, char_measure);
        assert_eq!(line_text(&lines[0]), "[Cla");
        assert_eq!(lines[0][0].color, RED);
        assert_eq!(line_text(&lines[1]), "w]x2");
        assert_eq!(lines[1].len(), 2, "the chunk carries both colors");
        assert_eq!(lines[1][0].text, "w]");
        assert_eq!(lines[1][0].color, RED);
        assert_eq!(lines[1][1].text, "x2");
        assert_eq!(lines[1][1].color, WHITE);
        assert_eq!(line_text(&lines[2]), ".");
        assert_eq!(lines[2][0].color, WHITE);
    }

    #[test]
    fn each_wrapped_line_is_an_independent_run_sequence() {
        // The emit pass justifies each line on its own, so each must be a complete run sequence.
        let w = words(&[
            ("one", WHITE),
            ("two", RED),
            ("three", WHITE),
            ("four", RED),
        ]);
        let lines = greedy_pack(w, 10.0, char_measure); // "one two"=7 fits; +three (13) overflows
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "one two");
        assert_eq!(line_text(&lines[1]), "three four");
        // Line 2 opens in its own colour, not line 1's last.
        assert_eq!(lines[1][0].color, WHITE);
    }
}
