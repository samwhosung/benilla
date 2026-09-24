//! The overflow law — the pure side of the FontString's three overflow regimes:
//! the height-limit line stack (regime 2's vertical half, `CGxString+0x40`) and the height-gated
//! ellipsis-truncate (regime 3, `CSimpleFontString 0x771ec0`). Pure (no atlas, no shaping) — the
//! parent binds the row-count closure ([`super::ellipsize_to_fit`]), mirroring how [`super::wrap`]
//! binds its measure; the tests here run on stub row counts.

/// The client's truncation marker: three ASCII dots (`.rdata 0x800188` = `2e 2e 2e 00`) — never
/// the single `…` glyph.
const ELLIPSIS: &str = "...";

/// Absorbs float noise in a resolved rect height before the line-stack division: an auto-height
/// FontString's rect is its own measured block (`rows × pitch`) plus anchor-graph arithmetic, and
/// a stray `+1e-4` must not read as "one more line started". A quarter pixel is far below any
/// real layout delta.
const HEIGHT_EPS: f32 = 0.25;

/// How many wrapped lines a `box_h`-tall box emits — the client's line-stack law: lines stack
/// until the accumulated height *passes* the limit (`0x5cdc20`: stop at `accum ≥ +0x40`, checked
/// after each line), so the count is the smallest `n` with `n·pitch ≥ box_h`, and never 0 (the
/// first line always emits). `pitch` is the line step ([`super::layout_text_quads`]'s: font height
/// + the outlined-cell pad; spacing 0 for all shipped UI).
///
/// **This is the RENDER law, and it is not the fit law** — see [`lines_fitting`]. Using it for both
/// is what let a four-line item name overflow a three-line box.
pub(super) fn lines_allowed(box_h: f32, pitch: f32) -> usize {
    (((box_h - HEIGHT_EPS) / pitch).ceil() as usize).max(1)
}

/// How many wrapped lines *fit inside* a `box_h`-tall box — the client's *other* line-count law,
/// the one the ellipsis-truncate measures against.
///
/// The truncate loop re-measures each backed-off candidate through
/// `0x44d960` → **`0x5c21c0` `GxuFont_GetMaxCharsWithinHeight`**, whose per-line test **breaks**
/// when `boxH + 2⁻²⁰ < accumH + lineH`.
/// A line is therefore admitted only if it lands *wholly within* the box: the largest `n` with
/// `n·pitch ≤ box_h`. That is a **floor**, where [`lines_allowed`]'s render stack is a **ceil** —
/// the render emits a line and *then* notices it overran, so it draws one more line than fits
/// whenever `box_h` is not a whole multiple of `pitch`.
///
/// The two agree on every exact multiple, which is why the split went unnoticed: every consumer
/// decision 0292/0329 verified (bag title 112×12, unit name 100×10, minimap zone 128×12) is one.
/// The loot row's 93×38 name at pitch 12 is the first shipped box that is not — `ceil(38/12) = 4`
/// against `floor(38/12) = 3` — and at exactly four wrapped lines "Schematic: Small Seaforium
/// Charge" slipped through the fit test and then overflowed its 37px row.
///
/// `0` is this floor's honest answer for a box shorter than a single line — but it is NOT what the
/// ellipsis seam may act on: `0x771ec0` clamps its box height to one line pitch *before* the fit
/// test (`boxH := max(boxH, lineH+gap)` when maxLines==0 — bytes `0x771f9e..0x771faa`), so
/// the sub-one-line call this 0 describes never happens in the client. [`ellipsize_in_box`]
/// mirrors the clamp. Decision 0597's "0 lines → the loop backs off to the bare ellipsis" reading
/// missed it and turned every sub-one-line fixed box into three dots — money purses, hotkeys,
/// everywhere. The epsilon is [`HEIGHT_EPS`] rather than the client's `2⁻²⁰` for
/// the same reason it is on the render law — our `box_h` is the anchor graph's arithmetic, not the
/// client's, and an exact multiple arriving as `12.0 - 1e-6` must not read as zero lines.
pub(super) fn lines_fitting(box_h: f32, pitch: f32) -> usize {
    ((box_h + HEIGHT_EPS) / pitch).floor().max(0.0) as usize
}

/// [`ellipsize`] against a *box*, choosing the line-count law for it — the whole point being that
/// the choice lives here, next to both laws and under test, rather than at the atlas-bound call
/// site where picking the wrong one is invisible ([`super::ellipsize_to_fit`] has no unit coverage:
/// it needs a real font atlas). Picking [`lines_allowed`] here instead of [`lines_fitting`] is
/// precisely the bug decision 0597 corrects, and `the_box_ellipsizer_measures_against_the_fit_law`
/// pins the difference on the exact geometry that exposed it.
///
/// The `box_h.max(pitch)`: the client's own **min-one-line height clamp** — `0x771ec0` raises its
/// box height to one line pitch before the fit measure (`boxH := max(boxH, lineH+gap)`, bytes
/// `0x771f9e..0x771faa`; gap is the pixel-quantized spacing, 0 for all shipped UI, so the clamp
/// floor is exactly `pitch`). The first line is therefore always admitted, even into a box shorter
/// than one line — the stock client renders the 36×10 HotKey under its 12px font and benilla's
/// 20×13 money numbers under 14px, and 0597's unclamped floor turned them all into bare `"..."`
/// (`sub_one_line_boxes_render_their_single_line` pins the exact geometries). The
/// clamp is a no-op for any box a full line fits in, so the ≥1-line floor law — 0597's loot fix —
/// is untouched. The render clamp ([`lines_allowed`], floored at one) draws the admitted line.
pub(super) fn ellipsize_in_box<F: FnMut(&str) -> usize>(
    text: &str,
    box_h: f32,
    pitch: f32,
    rows: F,
) -> Option<String> {
    ellipsize(text, lines_fitting(box_h.max(pitch), pitch), rows)
}

/// The height-gated ellipsis-truncate (`0x771ec0`): when `text` wraps into more lines than
/// `allowed`, back off one char at a time (the client skips UTF-8 continuation bytes; Rust's
/// `char` walk is the same boundary) and append [`ELLIPSIS`], until the candidate's wrapped row
/// count (`rows`, bound by the caller over the real wrap walk) fits — or the prefix is empty and
/// the bare `"..."` ships regardless (the client's loop floor). `None` = the text fits untouched
/// (the raw string draws; the common case, decided by one `rows` call).
///
/// The caller owns the GATE (`boxW > 0 && boxH > 0`; `maxLines` unmodeled): an
/// auto-height FontString's rect height IS its wrapped block, so it always fits here and never
/// truncates — the byte law's intrinsic-height escape, geometrically.
pub(super) fn ellipsize<F: FnMut(&str) -> usize>(
    text: &str,
    allowed: usize,
    mut rows: F,
) -> Option<String> {
    if rows(text) <= allowed {
        return None;
    }
    // Back off from the full text: each candidate is one char shorter than the last, + "...".
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
        // The stack stops when accum PASSES the limit: 13 tall at pitch 12 starts a second line.
        assert_eq!(lines_allowed(13.0, 12.0), 2);
        assert_eq!(lines_allowed(24.0, 12.0), 2);
        // Float noise on an exact multiple never buys a phantom line.
        assert_eq!(lines_allowed(24.0 + 1e-4, 12.0), 2);
        // Degenerate small boxes still emit their first line.
        assert_eq!(lines_allowed(1.5, 12.0), 1);
    }

    /// The fit law is a FLOOR where the render law is a CEIL — a line counts only if it lands
    /// wholly inside the box (`0x5c21c0`: break when `boxH + eps < accumH + lineH`).
    #[test]
    fn lines_fitting_is_the_height_fit_law() {
        // The loot row's item name — the box that exposed the split. The render stack draws 4
        // lines in it; only 3 actually fit.
        assert_eq!(lines_fitting(38.0, 12.0), 3);
        assert_eq!(lines_allowed(38.0, 12.0), 4, "the render law, for contrast");

        // The two laws agree on every exact multiple — which is why the split hid for so long.
        for n in 1..=6 {
            let h = 12.0 * n as f32;
            assert_eq!(lines_fitting(h, 12.0), n, "exact multiple {h}");
            assert_eq!(lines_allowed(h, 12.0), n, "exact multiple {h}");
        }

        // A box shorter than one line: the raw floor honestly says 0, the render law floors at 1.
        // The ellipsis seam does NOT act on the 0 — ellipsize_in_box clamps to one; see
        // sub_one_line_boxes_render_their_single_line.
        assert_eq!(lines_fitting(6.0, 12.0), 0);
        assert_eq!(lines_allowed(6.0, 12.0), 1);

        // Float noise on an exact multiple must not cost a line (the mirror of lines_allowed's).
        assert_eq!(lines_fitting(24.0 - 1e-4, 12.0), 2);
    }

    /// A stub row count: ceil(chars / 10) — a 10-char-wide box, every char one unit.
    fn rows10(s: &str) -> usize {
        s.chars().count().div_ceil(10).max(1)
    }

    /// The director's actual overflow, on the loot row's actual geometry (93x38 box, 12px pitch),
    /// with the stub standing in for the real wrap: 33 chars = 4 rows in a box that fits 3.
    ///
    /// The second assertion is the **mutation check, welded in**: under the render law the very
    /// same string is left untouched — which is what shipped, and what the director saw spill out
    /// of the row. If someone swaps the law back, the first assertion fails and this one explains
    /// why.
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
        // Multi-line boxes fit multi-line text raw.
        assert_eq!(ellipsize("a 17-char sentence", 2, rows10), None);
    }

    #[test]
    fn overflow_backs_off_to_the_longest_fitting_prefix() {
        // 17 chars in a one-row (10-char) box: prefix of 7 + "..." = 10 chars = 1 row.
        let got = ellipsize("Small Brown Pouch", 1, rows10);
        assert_eq!(got.as_deref(), Some("Small B..."));
    }

    #[test]
    fn utf8_backs_off_whole_chars() {
        // Multi-byte chars back off at char boundaries (the client skips continuation bytes).
        let got = ellipsize("Ancêtre éternel", 1, rows10);
        assert_eq!(got.as_deref(), Some("Ancêtre..."));
    }

    #[test]
    fn empty_prefix_ships_the_bare_ellipsis() {
        // Nothing fits: the loop floor is "..." itself, shipped even if over (the client's
        // buffer after a full back-off).
        let got = ellipsize("abcdef", 0, |_| 1);
        assert_eq!(got.as_deref(), Some("..."));
    }

    /// The 0605 regression, on the exact shipped geometries that went to three dots everywhere:
    /// a fixed box SHORTER than one line pitch admits its first line anyway — the stock client
    /// renders the 36×10 HotKey under its 12px font and the 20×13 money numbers under 14px, so
    /// single-line text that fits the width must ship raw, never as "...". Decision 0597's
    /// unclamped floor (0 lines fit → back off to bare dots) is the mutation this welds out.
    #[test]
    fn sub_one_line_boxes_render_their_single_line() {
        // The money purse number: "145" in the 20×13 box at NumberFontNormal's 14px pitch.
        assert_eq!(ellipsize_in_box("145", 13.0, 14.0, rows10), None);
        // The action-button hotkey: "1" in the 36×10 box at NumberFontNormalSmallGray's 12px.
        assert_eq!(ellipsize_in_box("1", 10.0, 12.0, rows10), None);
        // The clamp admits exactly ONE line, not more: text wrapping past it still truncates
        // to the one-line prefix — never to the bare dots the unclamped floor produced.
        assert_eq!(
            ellipsize_in_box("Small Brown Pouch", 10.0, 12.0, rows10).as_deref(),
            Some("Small B...")
        );
    }
}
