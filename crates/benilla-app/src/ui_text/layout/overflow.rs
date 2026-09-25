//! The pure side of the FontString overflow law: the height-limit line stack (`CGxString+0x40`)
//! and the height-gated ellipsis truncate (`CSimpleFontString` `0x771ec0`). The tests run on stub
//! row counts; [`super::ellipsize_to_fit`] binds the real one.

/// The reference's truncation marker, three ASCII dots (`.rdata 0x800188`), not the `…` glyph.
const ELLIPSIS: &str = "...";

/// Slack on a rect height before the line counts: an auto-height rect is its measured block plus
/// anchor-graph arithmetic, and a stray `1e-4` must not start or lose a line.
const HEIGHT_EPS: f32 = 0.25;

/// How many wrapped lines a `box_h`-tall box draws, the reference's line stack (`0x5cdc20`: stop
/// at `accum ≥ +0x40`): the smallest `n` with `n·pitch ≥ box_h`, never 0, `pitch` being the font
/// height. The render law: fitting by it lets a four-line name overflow a three-line box.
pub(super) fn lines_allowed(box_h: f32, pitch: f32) -> usize {
    (((box_h - HEIGHT_EPS) / pitch).ceil() as usize).max(1)
}

/// How many wrapped lines fit wholly inside a `box_h`-tall box, the ellipsis's law:
/// `GxuFont_GetMaxCharsWithinHeight` (`0x5c21c0`, via `0x44d960`) stops when
/// `boxH + 2⁻²⁰ < accumH + lineH`, the largest `n` with `n·pitch ≤ box_h`, so it floors where
/// [`lines_allowed`] ceils. Its 0 under one line is never acted on ([`ellipsize_in_box`]). The
/// slack is [`HEIGHT_EPS`], not `2⁻²⁰`: `box_h` is anchor-graph arithmetic, and `12.0 - 1e-6`
/// must not lose a line.
pub(super) fn lines_fitting(box_h: f32, pitch: f32) -> usize {
    ((box_h + HEIGHT_EPS) / pitch).floor().max(0.0) as usize
}

/// [`ellipsize`] by the fit law ([`lines_fitting`]), chosen here under test as
/// [`super::ellipsize_to_fit`] needs a real atlas. The box first clamps up to one pitch, as
/// `0x771ec0` does with `maxLines` 0 (`boxH := max(boxH, lineH+gap)`, `0x771f9e..0x771faa`, gap 0
/// in shipped UI), so a box under one line still draws it, like the 36×10 action-button hotkey
/// under a 12 px font.
pub(super) fn ellipsize_in_box<F: FnMut(&str) -> usize>(
    text: &str,
    box_h: f32,
    pitch: f32,
    rows: F,
) -> Option<String> {
    ellipsize(text, lines_fitting(box_h.max(pitch), pitch), rows)
}

/// The height-gated ellipsis truncate (`0x771ec0`): past `allowed` rows, back off one char at a
/// time (the reference skips UTF-8 continuation bytes) behind [`ELLIPSIS`] until `rows` fits; an
/// empty prefix ships the bare `"..."`. `None`: the text fits and draws raw. The caller owns the
/// gate (`boxW > 0 && boxH > 0`; `maxLines` is not modelled).
pub(super) fn ellipsize<F: FnMut(&str) -> usize>(
    text: &str,
    allowed: usize,
    mut rows: F,
) -> Option<String> {
    if rows(text) <= allowed {
        return None;
    }
    let mut cut: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    while let Some(end) = cut.pop() {
        let candidate = format!("{}{ELLIPSIS}", &text[..end]);
        if rows(&candidate) <= allowed {
            return Some(candidate);
        }
    }
    Some(ELLIPSIS.to_string())
}

#[cfg(test)]
mod overflow_tests {
    use super::*;

    #[test]
    fn lines_allowed_is_the_accum_law() {
        // One-line boxes: the bag title (112×12 at pitch 12) and the unit name (100×10 at 10).
        assert_eq!(lines_allowed(12.0, 12.0), 1);
        assert_eq!(lines_allowed(10.0, 10.0), 1);
        // 13 tall at pitch 12 starts a second line.
        assert_eq!(lines_allowed(13.0, 12.0), 2);
        assert_eq!(lines_allowed(24.0, 12.0), 2);
        // Float noise on a multiple buys no line.
        assert_eq!(lines_allowed(24.0 + 1e-4, 12.0), 2);
        // A tiny box still draws its first line.
        assert_eq!(lines_allowed(1.5, 12.0), 1);
    }

    /// A line fits only if it lands wholly inside the box (`0x5c21c0`).
    #[test]
    fn lines_fitting_is_the_height_fit_law() {
        // The loot row's 38-tall item name: 4 lines drawn, 3 fit.
        assert_eq!(lines_fitting(38.0, 12.0), 3);
        assert_eq!(lines_allowed(38.0, 12.0), 4, "the render law, for contrast");

        // The laws agree on every exact multiple.
        for n in 1..=6 {
            let h = 12.0 * n as f32;
            assert_eq!(lines_fitting(h, 12.0), n, "exact multiple {h}");
            assert_eq!(lines_allowed(h, 12.0), n, "exact multiple {h}");
        }

        // Under one line: fit 0, which `ellipsize_in_box` clamps away, render 1.
        assert_eq!(lines_fitting(6.0, 12.0), 0);
        assert_eq!(lines_allowed(6.0, 12.0), 1);

        // Float noise on a multiple costs no line.
        assert_eq!(lines_fitting(24.0 - 1e-4, 12.0), 2);
    }

    /// A stub row count: a 10-char-wide box, `ceil(chars / 10)`.
    fn rows10(s: &str) -> usize {
        s.chars().count().div_ceil(10).max(1)
    }

    /// The loot row's 93x38 name box at pitch 12, the stub standing in for the wrap: 33 chars are
    /// 4 rows where 3 fit. By the render law the same string draws untouched and overflows the row.
    #[test]
    fn the_box_ellipsizer_measures_against_the_fit_law() {
        const LONG: &str = "Schematic: Small Seaforium Charge";
        assert_eq!(rows10(LONG), 4, "the string that started this");

        assert_eq!(
            ellipsize_in_box(LONG, 38.0, 12.0, rows10).as_deref(),
            Some("Schematic: Small Seaforium ..."),
            "the fit law allows 3 lines, so it truncates"
        );
        assert_eq!(
            ellipsize(LONG, lines_allowed(38.0, 12.0), rows10),
            None,
            "the render law allows 4 — the bug: nothing truncates and the 4th line overflows"
        );
    }

    #[test]
    fn fitting_text_is_untouched() {
        assert_eq!(ellipsize("Backpack", 1, rows10), None);
        assert_eq!(ellipsize("a 17-char sentence", 2, rows10), None);
    }

    #[test]
    fn overflow_backs_off_to_the_longest_fitting_prefix() {
        // 17 chars in a one-row (10-char) box: a 7-char prefix plus "..." is 10 chars, 1 row.
        let got = ellipsize("Small Brown Pouch", 1, rows10);
        assert_eq!(got.as_deref(), Some("Small B..."));
    }

    #[test]
    fn utf8_backs_off_whole_chars() {
        let got = ellipsize("Ancêtre éternel", 1, rows10);
        assert_eq!(got.as_deref(), Some("Ancêtre..."));
    }

    #[test]
    fn empty_prefix_ships_the_bare_ellipsis() {
        // Nothing fits: the reference's loop floor is "..." itself, shipped even if over.
        let got = ellipsize("abcdef", 0, |_| 1);
        assert_eq!(got.as_deref(), Some("..."));
    }

    /// A fixed box shorter than one pitch still takes its first line: it ships raw, never as "...".
    #[test]
    fn sub_one_line_boxes_render_their_single_line() {
        // The money purse number: "145" in the 20×13 box at NumberFontNormal's 14px pitch.
        assert_eq!(ellipsize_in_box("145", 13.0, 14.0, rows10), None);
        // The action-button hotkey: "1" in the 36×10 box at NumberFontNormalSmallGray's 12px.
        assert_eq!(ellipsize_in_box("1", 10.0, 12.0, rows10), None);
        // The clamp admits one line only: longer text truncates to a one-line prefix.
        assert_eq!(
            ellipsize_in_box("Small Brown Pouch", 10.0, 12.0, rows10).as_deref(),
            Some("Small B...")
        );
    }
}
