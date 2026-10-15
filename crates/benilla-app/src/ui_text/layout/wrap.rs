//! The pure side of the wrap: the reference's line kernel (`0x5c7470`) over a markup line's
//! characters. [`super::measure::wrap_lines`] binds the real width law; the tests run on stubs.

use crate::ui_text::markup::ColorRun;

use super::breaks::{is_space, may_break};

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
            marks: 0,
        }),
    }
}

/// The fit test's bound in device px: the row's width rounded up to a whole pixel, as the line
/// builder takes it (`0x5c752a`-`0x5c7530`, a ceil); a row priced within it fits.
pub(super) fn row_limit(max_width: f32, dpi: f32) -> f32 {
    (max_width * dpi).ceil()
}

/// The width law of a row as it grows, one character at a time: the kernel prices every glyph
/// with its kern and advance against the bound (`0x5c761b`-`0x5c762b`).
pub(super) trait RowPricer {
    /// Open an empty row.
    fn reset(&mut self);
    /// Append `c`; whether it has a glyph. The kernel skips one without (`0x5c759e`-`0x5c75a0`).
    fn push(&mut self, c: char) -> bool;
    /// The row's width so far, its last glyph looking past itself to `next`, the source character
    /// after the row's last one, `None` at the line's end (`0x5c6b70`).
    fn width(&self, next: Option<char>) -> f32;
}

/// One row of a wrapped text.
pub(super) struct Row {
    pub(super) runs: Vec<ColorRun>,
    /// The source character after the row, `None` at a line's end: it prices the row's last glyph
    /// in the justify.
    pub(super) next: Option<char>,
    /// The row was cut mid-run with no break opportunity (`0x5c76ea`).
    pub(super) forced: bool,
    /// Text follows the row, past the whitespace its end skipped.
    pub(super) more: bool,
    /// The index of the row's first element in the text, where a character, a line break and a
    /// code each count one: the cursor the kernel starts it at.
    pub(super) start: usize,
}

/// The rows of a wrapped text, and where the layout ended short of it.
pub(super) struct Laid {
    pub(super) rows: Vec<Row>,
    /// The element index ([`Row::start`]) of the row no byte of which fit, which ended the layout
    /// (`0x5cdddb`, `0x5c22c0`); its row is not in `rows`.
    pub(super) bailed_at: Option<usize>,
}

/// An element of the text: `(char, line, run)`.
type Elem = (char, u32, u32);

/// The run index of a line break, which no run holds.
const BREAK: u32 = u32::MAX;

/// The character of a code (`|c…`, `|r`, a link's): it draws and prices nothing, but the kernel
/// steps over it as a token of its own and a row holding only codes has bytes (`0x5c7580`).
const MARK: char = '\0';

/// The elements of the lines laid end to end: each run's codes, then its characters, and a line
/// break after each line (the last line's is the text's end), read as far as the kernel needs.
struct Text<'a> {
    lines: &'a [Vec<ColorRun>],
    iter: Box<dyn Iterator<Item = Elem> + 'a>,
    buf: Vec<Elem>,
}

impl<'a> Text<'a> {
    fn new(lines: &'a [Vec<ColorRun>]) -> Self {
        let iter = lines.iter().enumerate().flat_map(|(l, line)| {
            let l = l as u32;
            line.iter()
                .enumerate()
                .flat_map(move |(r, run)| {
                    let r = r as u32;
                    (0..run.marks)
                        .map(move |_| (MARK, l, r))
                        .chain(run.text.chars().map(move |c| (c, l, r)))
                })
                .chain(std::iter::once(('\n', l, BREAK)))
        });
        Self {
            lines,
            iter: Box::new(iter),
            buf: Vec::new(),
        }
    }

    fn at(&mut self, i: usize) -> Option<Elem> {
        while self.buf.len() <= i {
            let e = self.iter.next()?;
            self.buf.push(e);
        }
        Some(self.buf[i])
    }

    fn char_at(&mut self, i: usize) -> Option<char> {
        self.at(i).map(|e| e.0)
    }

    /// The character the glyph before `i` looks ahead to: the next one past any codes, none at a
    /// line break (`0x5c6bad`-`0x5c6bba`).
    fn literal_from(&mut self, mut i: usize) -> Option<char> {
        while self.char_at(i) == Some(MARK) {
            i += 1;
        }
        self.char_at(i).filter(|&c| c != '\n')
    }

    /// The runs of `[a, b)`, which `at` has read.
    fn runs(&self, a: usize, b: usize) -> Vec<ColorRun> {
        let mut out = Vec::new();
        for &(c, l, r) in &self.buf[a..b] {
            if c != MARK {
                push_char(&mut out, c, &self.lines[l as usize][r as usize]);
            }
        }
        out
    }

    /// The first index at or after `at` not skipped as whitespace (`0x5c7708`-`0x5c7734`): the
    /// loop reads raw characters, so it crosses line breaks and the next line's indent, and
    /// stops at a code, which is not one.
    fn skip_space(&mut self, mut at: usize) -> usize {
        while self.at(at).is_some_and(|e| is_space(e.0)) {
            at += 1;
        }
        at
    }
}

/// Wrap the `lines` of one text into rows within `limit`, the reference's line kernel
/// (`0x5c7470`) run down the whole text by its caller (`0x5cdd37`...), until `max_rows` are laid.
/// Each row walks glyph by glyph (a code or a character without one is skipped, `0x5c759e`): the
/// pair (previous glyph, this one) asks the classifier ([`may_break`]) for a break before this
/// glyph (`0x5c75db`), then the glyph is priced and, when it passes `limit`, the row ends. It ends
/// at the last break recorded (`0x5c76cc`-`0x5c76e6`), else cut before the glyph that did not
/// fit; the next row opens past the whitespace there. A line break ends a row and is consumed with
/// it (`0x5c764d`); a row of no bytes at all ends the layout (`0x5cdddb`).
///
/// The kernel's carry byte starts set and runs through all the rows: while it is clear, the first
/// break opportunity of a row is not recorded (`0x5c75e4`-`0x5c75f5`). A row cut with no
/// opportunity sets it to `non_space_wrap` (`0x5c76ea`-`0x5c76f5`, the flag `0x40`), so the
/// default leaves the next row without its first one.
pub(super) fn wrap_text<P: RowPricer>(
    lines: &[Vec<ColorRun>],
    limit: f32,
    non_space_wrap: bool,
    max_rows: usize,
    pricer: &mut P,
) -> Laid {
    let mut text = Text::new(lines);
    let mut broke = true;
    let mut rows = Vec::new();
    let mut bailed_at = None;
    let mut start = 0usize;
    while rows.len() < max_rows {
        let Some((first, ..)) = text.at(start) else {
            break;
        };
        if first == '\n' {
            // A blank line takes a row.
            rows.push(Row {
                runs: Vec::new(),
                next: None,
                forced: false,
                more: text.at(start + 1).is_some(),
                start,
            });
            start += 1;
            continue;
        }
        pricer.reset();
        let mut prev: Option<char> = None;
        // The last break opportunity recorded: the character the next row would open on.
        let mut at_break: Option<usize> = None;
        let mut i = start;
        let cut = loop {
            let c = text.char_at(i).expect("a line break ends every line");
            if c == '\n' {
                break None;
            }
            if c == MARK || !pricer.push(c) {
                i += 1;
                continue;
            }
            if may_break(prev, c) {
                if broke || at_break.is_some() {
                    at_break = Some(i);
                }
                broke = true;
            }
            let next = text.literal_from(i + 1);
            if pricer.width(next) > limit {
                break Some(i);
            }
            prev = Some(c);
            i += 1;
        };
        let Some(cut) = cut else {
            // The line's end (`0x5c7744`): the rest is the row, whitespace and all.
            rows.push(Row {
                runs: text.runs(start, i),
                next: None,
                forced: false,
                more: text.at(i + 1).is_some(),
                start,
            });
            start = i + 1;
            continue;
        };
        let (end, forced) = match at_break {
            Some(b) => (b, false),
            None => {
                broke = non_space_wrap;
                (cut, true)
            }
        };
        let after = text.skip_space(end);
        if end == start && (after == start || text.at(after).is_none()) {
            // No byte fits: the builder bails and the layout ends here (`0x5cdddb`), and an
            // empty row with nothing after it is not laid (`0x5cddeb`-`0x5cddf3`).
            bailed_at = Some(start);
            break;
        }
        rows.push(Row {
            runs: text.runs(start, end),
            next: text.char_at(end),
            forced,
            more: text.at(after).is_some(),
            start,
        });
        start = after;
    }
    Laid { rows, bailed_at }
}

#[cfg(test)]
mod wrap_tests {
    use super::*;
    use crate::ui_text::markup::fontstring_lines;

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    /// A stub width law: every character with a glyph, space included, is one unit, and with
    /// `ink` the last glyph before a line's end is half a unit narrower, as ink often is.
    #[derive(Default)]
    struct Units {
        n: f32,
        ink: bool,
        /// Characters this face has no glyph for.
        missing: &'static str,
        /// Characters three units wide.
        wide: &'static str,
        /// How many characters the kernel read.
        pushes: usize,
    }

    impl RowPricer for Units {
        fn reset(&mut self) {
            self.n = 0.0;
        }
        fn push(&mut self, c: char) -> bool {
            if self.missing.contains(c) {
                return false;
            }
            self.pushes += 1;
            self.n += if self.wide.contains(c) { 3.0 } else { 1.0 };
            true
        }
        fn width(&self, next: Option<char>) -> f32 {
            self.n - if self.ink && next.is_none() { 0.5 } else { 0.0 }
        }
    }

    fn laid<P: RowPricer>(
        lines: &[Vec<ColorRun>],
        limit: f32,
        nsw: bool,
        max_rows: usize,
        pricer: &mut P,
    ) -> Vec<Row> {
        wrap_text(lines, limit, nsw, max_rows, pricer).rows
    }

    fn line(text: &str) -> Vec<ColorRun> {
        vec![ColorRun {
            text: text.to_string(),
            color: WHITE,
            link: None,
            marks: 0,
        }]
    }

    fn rows_of(runs: &[ColorRun], limit: f32, nsw: bool) -> Vec<Row> {
        laid(
            &[runs.to_vec()],
            limit,
            nsw,
            usize::MAX,
            &mut Units::default(),
        )
    }

    /// `lines` of one text wrapped at `limit`, one unit a character.
    fn lay(lines: &[&str], limit: f32, nsw: bool) -> Vec<String> {
        let lines: Vec<Vec<ColorRun>> = lines.iter().map(|l| line(l)).collect();
        texts(&laid(&lines, limit, nsw, usize::MAX, &mut Units::default()))
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| line_text(&r.runs)).collect()
    }

    /// A wrapped line's text, colours dropped.
    fn line_text(runs: &[ColorRun]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }

    fn wrap(text: &str, limit: f32) -> Vec<String> {
        texts(&rows_of(&line(text), limit, true))
    }

    #[test]
    fn long_line_breaks_at_word_boundaries_within_width() {
        // Width 12: "Refreshing" (10) plus " Spring" (7) passes it; "Spring Water" is exactly 12.
        let rows = wrap("Refreshing Spring Water", 12.0);
        assert_eq!(rows, ["Refreshing", "Spring Water"]);
        assert!(rows.iter().all(|r| r.chars().count() <= 12));
    }

    #[test]
    fn short_line_stays_single() {
        assert_eq!(wrap("Buy now", 100.0), ["Buy now"]);
    }

    #[test]
    fn overlong_word_force_breaks_at_the_last_fitting_glyph() {
        let rows = rows_of(&line("Supercalifragilistic ok"), 8.0, true);
        // The remainder packs on with the next word.
        assert_eq!(texts(&rows), ["Supercal", "ifragili", "stic ok"]);
        assert!(rows[0].forced && rows[1].forced && !rows[2].forced);
    }

    #[test]
    fn force_break_mid_line_starts_from_the_break_opportunity() {
        assert_eq!(
            wrap("at Supercalifragilistic", 8.0),
            ["at", "Supercal", "ifragili", "stic"]
        );
    }

    /// The bound is the width rounded up to a whole device pixel: a box a few ulps under its
    /// content still fits it, and a half pixel of room counts as a whole one.
    #[test]
    fn the_bound_rounds_the_width_up_to_a_device_pixel() {
        assert_eq!(row_limit(11.0 - 0.000015, 1.0), 11.0);
        assert_eq!(row_limit(10.5, 1.0), 11.0);
        assert_eq!(row_limit(10.25, 2.0), 21.0);
        // A real overflow still breaks.
        struct Six(f32);
        impl RowPricer for Six {
            fn reset(&mut self) {
                self.0 = 0.0;
            }
            fn push(&mut self, _: char) -> bool {
                self.0 += 6.0;
                true
            }
            fn width(&self, _: Option<char>) -> f32 {
                self.0
            }
        }
        let rows = laid(&[line("34")], 11.0, true, usize::MAX, &mut Six(0.0));
        assert_eq!(rows.len(), 2);
    }

    /// The source line's last glyph fits by its ink (`0x5c6b70` sees nothing after it)…
    #[test]
    fn the_source_lines_last_glyph_is_priced_by_its_ink() {
        let mut ink = Units {
            ink: true,
            ..Units::default()
        };
        let rows = laid(&[line("ab cd")], 4.5, true, usize::MAX, &mut ink);
        assert_eq!(rows.len(), 1, "\"ab cd\" is 4.5 by its ink");
        // Whitespace after the last word is characters after it: the word is priced whole, and
        // the space fits after it and rides its row.
        let rows = laid(&[line("ab cd ")], 4.5, true, usize::MAX, &mut ink);
        assert_eq!(texts(&rows), ["ab", "cd "]);
        assert_eq!(
            rows[0].next,
            Some(' '),
            "a broken row looks past itself to the separator"
        );
        assert_eq!(rows[1].next, None);
    }

    /// …and a glyph with text after it is priced whole by every step, even when it ends up last
    /// on its row.
    #[test]
    fn a_glyph_before_more_text_is_priced_whole() {
        let mut ink = Units {
            ink: true,
            ..Units::default()
        };
        let rows = laid(&[line("ab cd ef")], 4.6, true, usize::MAX, &mut ink);
        assert_eq!(texts(&rows), ["ab", "cd ef"]);
        // A force-break prices each prefix with the character after it.
        let rows = laid(&[line("abcdef")], 5.5, true, usize::MAX, &mut ink);
        assert_eq!(texts(&rows), ["abcdef"]);
        let rows = laid(&[line("abcdef ")], 5.5, true, usize::MAX, &mut ink);
        assert_eq!(texts(&rows), ["abcde", "f "]);
        assert_eq!(rows[0].next, Some('f'));
    }

    /// Trailing whitespace that does not fit ends the row before its first character and drops
    /// whole (`0x5c7780`, `0x5c7708`).
    #[test]
    fn trailing_whitespace_that_does_not_fit_drops() {
        let rows = rows_of(&line("ab  "), 2.0, true);
        assert_eq!(texts(&rows), ["ab"]);
        assert_eq!(rows[0].next, Some(' '));
        // One space fits and the second does not: the break is before the first, so both drop.
        let rows = rows_of(&line("ab  "), 3.0, true);
        assert_eq!(texts(&rows), ["ab"]);
        assert_eq!(rows[0].next, Some(' '));
        let rows = rows_of(&line("ab  "), 4.0, true);
        assert_eq!(texts(&rows), ["ab  "], "all of it fits and rides the row");
        assert_eq!(rows[0].next, None);
    }

    #[test]
    fn sub_glyph_width_bails_without_progress() {
        // Not one glyph fits: the builder bails and drops the line, as the reference's does.
        assert!(rows_of(&line("ab"), 0.5, true).is_empty());
    }

    #[test]
    fn a_blank_line_takes_a_row() {
        let rows = rows_of(&line(""), 5.0, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].next, None);
        assert_eq!(
            laid(&[vec![]], 5.0, true, usize::MAX, &mut Units::default()).len(),
            1
        );
    }

    #[test]
    fn wrap_preserves_double_space_between_words() {
        assert_eq!(wrap("Hello.  World", 100.0), ["Hello.  World"]);
        // Width 6 fits "Hello." alone; the separator at the break drops.
        assert_eq!(wrap("Hello.  World", 6.0), ["Hello.", "World"]);
    }

    /// A line's own leading whitespace is part of its first row (`0x5c7470` skips none at the
    /// start, only past a break): an indent survives the wrap.
    #[test]
    fn leading_whitespace_stays_on_the_first_row() {
        assert_eq!(wrap("  The Left Piece", 100.0), ["  The Left Piece"]);
        assert_eq!(wrap("  The Left Piece", 10.0), ["  The Left", "Piece"]);
    }

    #[test]
    fn color_runs_survive_the_wrap() {
        // Same-colour words merge, a new colour starts a run, and the space joins the run before.
        let runs = vec![
            ColorRun {
                text: "aa bb ".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: "cc".into(),
                color: RED,
                link: None,
                marks: 0,
            },
        ];
        let rows = rows_of(&runs, 100.0, true);
        assert_eq!(rows.len(), 1);
        let runs = &rows[0].runs;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "aa bb ");
        assert_eq!(runs[0].color, WHITE);
        assert_eq!(runs[1].text, "cc");
        assert_eq!(runs[1].color, RED);
    }

    /// Text typed straight after a chat link, no space between: `Bearer]` and the typed text are
    /// one break unit in two runs, and the typed text keeps its own colour, outside the link.
    #[test]
    fn a_word_straddling_a_color_boundary_keeps_both_colors() {
        let link = std::sync::Arc::new(crate::ui_text::markup::LinkInfo {
            link: "item:13984:0:0:0".into(),
            markup: "|Hitem:13984:0:0:0|h[The Plague Bearer]|h".into(),
        });
        let runs = vec![
            ColorRun {
                text: "[The Plague Bearer]".into(),
                color: RED,
                link: Some(link.clone()),
                marks: 0,
            },
            ColorRun {
                text: "dsfsdfsd".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
        ];
        // The colour boundary is no break opportunity.
        let rows = rows_of(&runs, 100.0, true);
        assert_eq!(rows.len(), 1);
        let runs = &rows[0].runs;
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
        let runs = vec![
            ColorRun {
                text: "[Claw]".into(),
                color: RED,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: "x2.".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
        ];
        // Width 4: "[Cla" | "w]x2" | ".", the colour boundary inside the middle chunk.
        let rows = rows_of(&runs, 4.0, true);
        assert_eq!(texts(&rows), ["[Cla", "w]x2", "."]);
        assert_eq!(rows[0].runs[0].color, RED);
        assert_eq!(rows[1].runs.len(), 2, "the chunk carries both colors");
        assert_eq!(rows[1].runs[0].text, "w]");
        assert_eq!(rows[1].runs[0].color, RED);
        assert_eq!(rows[1].runs[1].text, "x2");
        assert_eq!(rows[1].runs[1].color, WHITE);
        assert_eq!(rows[2].runs[0].color, WHITE);
    }

    #[test]
    fn each_wrapped_line_is_an_independent_run_sequence() {
        // The emit pass justifies each line on its own, so each must be a complete run sequence.
        let runs = vec![
            ColorRun {
                text: "one ".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: "two ".into(),
                color: RED,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: "three ".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: "four".into(),
                color: RED,
                link: None,
                marks: 0,
            },
        ];
        let rows = rows_of(&runs, 10.0, true); // "one two"=7 fits; +three (13) overflows
        assert_eq!(texts(&rows), ["one two", "three four"]);
        // Row 2 opens in its own colour, not row 1's last.
        assert_eq!(rows[1].runs[0].color, WHITE);
    }

    /// A glyph the face lacks takes no part in the kernel: it prices nothing and asks nothing of
    /// the classifier, so it neither makes a break nor hides one (`0x5c759e`).
    #[test]
    fn a_character_without_a_glyph_is_skipped() {
        let mut p = Units {
            missing: "\u{200b}",
            ..Units::default()
        };
        let rows = laid(&[line("ab\u{200b}c d")], 3.0, true, usize::MAX, &mut p);
        assert_eq!(texts(&rows), ["ab\u{200b}c", "d"]);
    }

    // The break opportunities of the classifier (#217).

    /// A line ends after a `-`: the reference's merchant tutorial reads "...open, right-" and the
    /// next line "clicking an item", where the whitespace-only wrap moved "right-clicking" whole.
    #[test]
    fn a_line_breaks_after_a_hyphen() {
        assert_eq!(
            wrap("open, right-clicking an item", 12.0),
            ["open, right-", "clicking an", "item"]
        );
        // The same word alone on the line: broken at the joiner, not mid-letter.
        assert_eq!(wrap("tip-top shape", 5.0), ["tip-", "top", "shape"]);
    }

    /// The break lands after the joiner even though more of the word would fit on the line.
    #[test]
    fn a_line_breaks_after_a_colon_and_a_slash() {
        assert_eq!(wrap("ab:cdefgh", 5.0), ["ab:", "cdefg", "h"]);
        assert_eq!(wrap("ab/cdefgh", 5.0), ["ab/", "cdefg", "h"]);
        assert_eq!(wrap("dir:abcdefgh ok", 8.0), ["dir:", "abcdefgh", "ok"]);
        // The joiner that ends the word fits as part of it.
        assert_eq!(wrap("SI:7 agents", 4.0), ["SI:7", "agen", "ts"]);
    }

    /// Between two CJK characters a line may end, but never so that it opens on a closing mark
    /// (`。`) or ends on an opening one (`「`).
    #[test]
    fn a_cjk_line_breaks_between_characters_under_the_bracket_exceptions() {
        assert_eq!(wrap("한국어문자열", 3.0), ["한국어", "문자열"]);
        // Hangul breaks between syllables, not only at spaces: the row ends where the syllables
        // stop fitting, not before the run.
        assert_eq!(wrap("ab 한국어문자", 4.0), ["ab 한", "국어문자"]);
        // The closing mark stays with the character before it.
        assert_eq!(wrap("中文。字", 2.0), ["中", "文。", "字"]);
        // The opening bracket goes with the character after it: the break falls before it.
        assert_eq!(wrap("ab\u{300c}中", 3.0), ["ab", "\u{300c}中"]);
        // A Latin letter before a CJK one is a break, whitespace or not.
        assert_eq!(wrap("ab中文", 2.0), ["ab", "中文"]);
    }

    /// The ideographic space is whitespace: a row ends before it and the next opens past it, and
    /// no break lands after it.
    #[test]
    fn the_ideographic_space_ends_a_row_and_is_skipped() {
        assert_eq!(wrap("中文\u{3000}字字", 3.0), ["中文", "字字"]);
        // After an opening bracket the space still breaks, since the space test comes first.
        assert_eq!(wrap("\u{ff08}\u{3000}字", 1.0), ["\u{ff08}", "字"]);
    }

    // The carry of a force-broken line (#218).

    /// With `nonspacewrap` off, the row after a force-broken one does not record its first break
    /// opportunity, so it is cut at the glyph that does not fit; on, it breaks at the space.
    #[test]
    fn a_force_broken_row_leaves_the_next_without_its_first_break_when_nonspacewrap_is_off() {
        let text = line("aaaaaaaa bb cc dd");
        let off = rows_of(&text, 4.0, false);
        assert_eq!(texts(&off), ["aaaa", "aaaa", "bb c", "c dd"]);
        let on = rows_of(&text, 4.0, true);
        assert_eq!(texts(&on), ["aaaa", "aaaa", "bb", "cc", "dd"]);
        assert!(
            off[..3].iter().all(|r| r.forced),
            "cut, cut, and cut at the lost break"
        );
        assert!(
            on[0].forced && !on[1].forced,
            "on, the second row breaks at its space"
        );
    }

    /// The carry crosses lines: the layout hands one byte through all of its rows.
    #[test]
    fn the_carry_crosses_source_lines() {
        // "aaaaaaaa" cuts twice, so "bb cc dd" opens with the byte clear and loses its first break.
        assert_eq!(
            lay(&["aaaaaaaa", "bb cc dd"], 4.0, false),
            ["aaaa", "aaaa", "bb c", "c dd"]
        );
        // On, it breaks at its spaces.
        assert_eq!(
            lay(&["aaaaaaaa", "bb cc dd"], 4.0, true),
            ["aaaa", "aaaa", "bb", "cc", "dd"]
        );
    }

    // What the whitespace skip after a break eats (`0x5c7708`-`0x5c7734`).

    /// The skip reads raw characters: trailing spaces, every line break after them and the next
    /// line's indent all go, so the next row opens on text.
    #[test]
    fn the_skip_after_a_break_crosses_line_breaks_and_the_next_indent() {
        assert_eq!(lay(&["ab cd  ", "", "ef"], 5.0, true), ["ab cd", "ef"]);
        assert_eq!(lay(&["ab cd  ", "  ef"], 5.0, true), ["ab cd", "ef"]);
        assert_eq!(lay(&["ab cd ", "ef"], 5.0, true), ["ab cd", "ef"]);
        // A line that ends within the row keeps its blank lines and its next indent.
        assert_eq!(lay(&["ab", "", "  cd"], 5.0, true), ["ab", "", "  cd"]);
    }

    /// A glyph wider than the row ends the layout, not just its line (`0x5cdddb`).
    #[test]
    fn a_glyph_wider_than_the_row_ends_the_whole_layout() {
        let lines: Vec<Vec<ColorRun>> = ["ab", "W", "cd"].iter().map(|l| line(l)).collect();
        let mut p = Units {
            wide: "W",
            ..Units::default()
        };
        let rows = laid(&lines, 2.0, true, usize::MAX, &mut p);
        assert_eq!(texts(&rows), ["ab"]);
    }

    /// The rows of a markup string, `|r` standing for any code, one unit a character.
    fn marked(text: &str, limit: f32) -> Vec<String> {
        let lines = fontstring_lines(text, WHITE);
        texts(&laid(
            &lines,
            limit,
            true,
            usize::MAX,
            &mut Units::default(),
        ))
    }

    /// A code is a token the skip stops at: the space after it is a glyph that opens the row.
    #[test]
    fn a_colour_code_stops_the_skip() {
        assert_eq!(marked("ab cd |r ef", 5.0), ["ab cd", " ef"]);
        let runs = vec![
            ColorRun {
                text: "ab cd ".into(),
                color: WHITE,
                link: None,
                marks: 0,
            },
            ColorRun {
                text: " ef".into(),
                color: RED,
                link: None,
                marks: 1,
            },
        ];
        let rows = rows_of(&runs, 5.0, true);
        assert_eq!(texts(&rows), ["ab cd", " ef"]);
        assert_eq!(rows[1].runs[0].color, RED);
    }

    /// The skip stops at a code after a line break too, and at one that opens a line.
    #[test]
    fn the_skip_stops_at_a_code_past_a_line_break() {
        assert_eq!(marked("ab |r\ncd", 2.0), ["ab", "", "cd"]);
        assert_eq!(marked("ab \n|r cd", 2.0), ["ab", " c", "d"]);
    }

    /// A row of codes alone has bytes, so it is laid, empty; so is a line of them.
    #[test]
    fn a_row_of_codes_alone_is_an_empty_row() {
        assert_eq!(marked("ab |r", 2.0), ["ab", ""]);
        assert_eq!(marked("ab\n|r", 2.0), ["ab", ""]);
        assert_eq!(marked("|r", 2.0), [""]);
        // A break alone is not a line (`fontstring_lines`), nor is a bare end.
        assert_eq!(marked("ab\n", 2.0), ["ab"]);
    }

    /// A glyph too wide for the row after a code: the code's row is laid and the layout ends.
    #[test]
    fn a_code_before_a_glyph_too_wide_makes_an_empty_row_and_the_layout_bails() {
        let lines = fontstring_lines("|rWcd", WHITE);
        let mut p = Units {
            wide: "W",
            ..Units::default()
        };
        let laid = wrap_text(&lines, 2.0, true, usize::MAX, &mut p);
        assert_eq!(texts(&laid.rows), [""]);
        assert_eq!(laid.bailed_at, Some(1), "the glyph after the code");
        // Without the code nothing is laid, and the row that bailed is the first.
        let lines = fontstring_lines("Wcd", WHITE);
        let laid = wrap_text(&lines, 2.0, true, usize::MAX, &mut p);
        assert!(laid.rows.is_empty());
        assert_eq!(laid.bailed_at, Some(0));
    }

    /// The kernel is asked for no more rows than the caller wants: a long paragraph under a cap
    /// reads a few rows' worth of it.
    #[test]
    fn the_walk_stops_at_the_row_cap() {
        let long = "ab ".repeat(5000);
        let mut p = Units::default();
        let rows = laid(&[line(&long)], 10.0, true, 3, &mut p);
        assert_eq!(rows.len(), 3);
        assert!(rows[2].more);
        assert!(p.pushes < 100, "read {} characters", p.pushes);
    }

    /// The kernel against a straight-line emulation of the reference's: the line kernel
    /// (`0x5c7470`) over the text with line breaks as tokens, and its caller's loop.
    #[test]
    fn the_wrap_matches_an_emulation_of_the_reference_over_random_texts() {
        fn emulate(text: &str, limit: usize, nsw: bool) -> Vec<String> {
            let t: Vec<char> = text.chars().collect();
            let mut out = Vec::new();
            let (mut carry, mut i) = (true, 0usize);
            while i < t.len() {
                if t[i] == '\n' {
                    out.push(String::new());
                    i += 1;
                    continue;
                }
                let (s, mut j, mut w) = (i, i, 0usize);
                let (mut prev, mut brk) = (None, 0usize);
                let (end, nx) = loop {
                    if j == t.len() || t[j] == '\n' {
                        break (j, j + 1);
                    }
                    if t[j] == '§' {
                        j += 1;
                        continue;
                    }
                    if may_break(prev, t[j]) {
                        if carry || brk != 0 {
                            brk = j - s;
                        }
                        carry = true;
                    }
                    if w + 1 > limit {
                        let end = if brk != 0 {
                            s + brk
                        } else {
                            carry = nsw;
                            j
                        };
                        let mut nx = end;
                        while nx < t.len() && is_space(t[nx]) {
                            nx += 1;
                        }
                        break (end, nx);
                    }
                    w += 1;
                    prev = Some(t[j]);
                    j += 1;
                };
                if nx == s {
                    break;
                }
                out.push(t[s..end].iter().filter(|&&c| c != '§').collect());
                i = nx;
            }
            out
        }
        let alphabet: Vec<char> = "ab -:/.(\t\n\u{3000}\u{4e2d}\u{3002}\u{300c}W§§"
            .chars()
            .collect();
        let mut seed: u64 = 12345;
        let mut next = move || {
            seed = seed
                .wrapping_mul(0x5851_f42d_4c95_7f2d)
                .wrapping_add(0x1405_7b7e_f767_814f);
            (seed >> 33) as usize
        };
        for _ in 0..30_000 {
            let text: String = (0..next() % 24)
                .map(|_| alphabet[next() % alphabet.len()])
                .collect();
            let (limit, nsw) = (1 + next() % 5, next() % 2 == 1);
            // `§` stands for a code, which the real parser turns into a mark.
            let lines = fontstring_lines(&text.replace('§', "|r"), WHITE);
            let got = texts(&laid(
                &lines,
                limit as f32,
                nsw,
                usize::MAX,
                &mut Units::default(),
            ));
            assert_eq!(
                got,
                emulate(&text, limit, nsw),
                "{text:?} @{limit} nsw={nsw}"
            );
        }
    }
}
