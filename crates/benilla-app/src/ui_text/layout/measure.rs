//! **Measure and fit** — the width law, the wrap walk, and the ellipsis seam.
//!
//! Split from the emit pass ([`super`]) at decision 1342, which is when the split finally became
//! honest. Under the size ladder these two halves computed in *different spaces*: measure answered
//! in drawn px and the emit pass laid out at the snapped size, with a rescale between them, so
//! every fit decision had to be converted back and forth and each conversion was a place to get it
//! wrong (1339's fault 1 — an auto-sized label wrapping inside its own measured width). With the
//! raster size an exact integer, both halves work in one space over one table, and the seam between
//! them is just "who calls whom".
//!
//! Everything here is a pure function of `(face, ppem, outline bias, characters)`. That is the
//! client's own law — `ComputeStep 0x5ca2d0` has no kerning term and no neighbour term — and it is
//! what lets a measure be answered from inside a Lua call, where a shaper could never reach.

use bevy::math::Rect;

use benilla_ui::script::{JustifyH, JustifyV};

use super::super::engine::{TextEngine, UiFontAtlas};
use super::super::markup::{fontstring_lines, parse_markup, ColorRun};
use super::wrap::{greedy_pack, tokenize_words};
use super::{client_step, overflow, FontSpec, Justify, HEIGHT_LIMIT_MIN, WRAP_MIN_WIDTH};

/// A [`FontSpec`] resolved against the engine — everything downstream needs, looked up once.
///
/// One body, because the three things it produces have to agree: a measure that resolved a
/// different face than the draw, or a different raster size, is exactly the class of bug this
/// decision is about.
#[derive(Clone, Copy)]
pub(super) struct Resolved {
    /// Index into the engine's faces.
    pub(super) face: usize,
    /// The exact integer device-pixel size ([`TextEngine::ppem`]).
    pub(super) ppem: u16,
    /// The **logical** height that ppem draws at — the line pitch, and the em every block-height
    /// law uses. Not the height that was requested: see [`TextEngine::logical_size`].
    pub(super) size: f32,
    /// The outline cell variant (`0` = plain).
    pub(super) radius: u8,
    /// The step law's per-glyph bias ([`super::step_extra_of`]).
    pub(super) step_extra: f32,
}

/// Resolve a spec. Does **not** touch the caches — call [`TextEngine::ensure_metrics`] (or
/// [`TextEngine::ensure_str`], to draw) for the string itself.
pub(super) fn resolve(e: &mut TextEngine, font: &FontSpec) -> Resolved {
    let face = e.face_for(font.path);
    let ppem = e.ppem(font.height.unwrap_or(super::super::DEFAULT_FONT_SIZE));
    Resolved {
        face,
        ppem,
        size: e.logical_size(ppem),
        radius: super::super::outline::radius_of(font.outline),
        step_extra: super::step_extra_of(font.outline),
    }
}

/// One line's total laid-out width (logical px) under the client's step law
/// ([`client_step`]): the sum of per-character steps, read from the cache
/// ([`TextEngine::char_cell`]). This is the client's `GetTextWidth` (a sum of `ComputeStep`s), so
/// measure == render == the real client's width. Empty text is zero-width.
///
/// **No shaper, and that is the point.** The client's law has no kerning term and no neighbour
/// term, so a string's width is a pure function of its characters, face, size and outline — which
/// is what lets the lookup be cached, and therefore what lets this answer from *inside* a Lua call
/// ([`benilla_ui::script::TextMeasure`]), where a `&mut FontSystem` could never reach. Every
/// measurement in the app runs through this one function, so the synchronous answer and the batch
/// answer cannot be different numbers.
///
/// A character no face can shape contributes 0 — and steps the render pen by 0 too, so unlike
/// under the ladder there is no divergence left to document here.
///
/// The caller must have ensured `text`'s metrics; a character that has not been is skipped rather
/// than shaped, because this half is deliberately `&`-only.
pub(super) fn measure_line_width(e: &TextEngine, r: Resolved, step_extra: f32, text: &str) -> f32 {
    // Summed in DEVICE px and divided once, because that is exactly what the render pen does
    // (decision 1644 — it walks in device px so the line's origin can round once). A sum of
    // quotients and a quotient of sums are not the same `f32`, and "measure == render, to the bit"
    // is a property this module is asked to keep, not one it gets to approximate.
    let steps: f32 = text
        .chars()
        .filter_map(|c| e.char_cell(r.face, r.ppem, c))
        .map(|c| c.floor_sum + step_extra * c.glyphs.len() as f32)
        .sum();
    steps / e.dpi()
}

/// The width law itself, for the differential test that pins it to a shaped sum.
#[cfg(test)]
pub(crate) fn measure_line_width_for_test(
    e: &mut TextEngine,
    face: usize,
    ppem: u16,
    step_extra: f32,
    text: &str,
) -> f32 {
    e.ensure_metrics(face, ppem, text);
    let r = Resolved {
        face,
        ppem,
        size: e.logical_size(ppem),
        radius: 0,
        step_extra,
    };
    measure_line_width(e, r, step_extra, text)
}

/// Greedy word-boundary wrap of one markup line (a `\n`-delimited run sequence) into sub-lines that
/// each fit within `max_width` px.
///
/// The break law is the regime-2 wrap (the `0x5c6c50`/`0x5c7780` kernel): break at the last
/// break opportunity; a word with none that
/// overflows alone force-breaks at the last fitting glyph — a rendered line never exceeds the wrap
/// width. Inter-word whitespace is preserved verbatim (a double space after a period stays a double
/// space — see [`tokenize_words`]), and only the trailing separator at a line break drops. Colors
/// are preserved per word (the separating whitespace attaches to the preceding run, since it draws
/// no ink). **Remaining approximation:** break opportunities are ASCII/Unicode whitespace only —
/// the kernel's kinsoku (CJK) opportunity classes and the `nonspacewrap` flag (ui `0x1000`:
/// mid-word breaks become opportunities even when a space exists) are unmodeled; no shipped
/// FontString renders CJK, and the one `nonspacewrap` consumer (MinimapZoneText) sits in a one-line
/// box where opportunity choice cannot change the outcome.
pub(super) fn wrap_line(
    e: &TextEngine,
    r: Resolved,
    line: &[ColorRun],
    max_width: f32,
) -> Vec<Vec<ColorRun>> {
    // Flatten the line's color runs into words, each tagged with its run color and its verbatim
    // inter-word separator (so Blizzard's double spaces survive the wrap).
    let words = tokenize_words(line);
    if words.is_empty() {
        // A blank line (only whitespace / empty) still occupies a row — keep it as one empty line.
        return vec![line.to_vec()];
    }
    greedy_pack(words, max_width, |t| {
        measure_line_width(e, r, r.step_extra, t)
    })
}

/// Measure `text`'s laid-out size — width of the widest line, height of all (wrapped) lines — at
/// the resolved face/size, wrapping at `wrap_width` when given (the same [`wrap_line`] pass
/// rendering uses, so measure == render). The engine's measure round-trip
/// ([`benilla_ui::script::UiScript::fontstrings_needing_measure`]) sizes height-less FontStrings
/// with this — the real client's layout asks its font engine for string metrics the same way.
pub(crate) fn measure_text(
    e: &mut TextEngine,
    text: &str,
    wrap_width: Option<f32>,
    font: FontSpec,
) -> (f32, f32) {
    let r = resolve(e, &font);
    // The one `&mut` moment: everything below reads the cache this fills. Metrics only — a string
    // that is measured and never drawn costs no raster and no GPU.
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;

    let lines = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    let render_lines: Vec<Vec<ColorRun>> = match wrap_width {
        Some(w) if w > WRAP_MIN_WIDTH => lines
            .iter()
            .flat_map(|line| wrap_line(e, r, line, w))
            .collect(),
        _ => lines,
    };
    let mut max_w = 0.0f32;
    for line in &render_lines {
        let mut w = 0.0f32;
        for run in line {
            if !run.text.is_empty() {
                w += measure_line_width(e, r, r.step_extra, &run.text);
            }
        }
        max_w = max_w.max(w);
    }
    // A hair of headroom. This used to be load-bearing: the emit pass re-wrapped inside this same
    // width in a *different* space, and the rescale between them turned float noise into a
    // proportional error (1339). With one raster size there is one space, and the emit pass sums
    // the identical per-character steps in the identical order — the two numbers are bit-equal, so
    // this pixel is now pure belt-and-braces. It stays because removing it narrows every auto-sized
    // FontString by a pixel, which is a look change and therefore the director's call, not a
    // refactor's.
    //
    // Height: N lines at the client's intrinsic pitch — the font em, NO outline pad (the byte law:
    // the text-block height kernel `0x5c2070` = N·S + (N−1)·gap, spacing 0 for all shipped UI; the
    // +2r lives only in the atlas CELL, never the block math). An outlined line's ring pokes past
    // this height by design, exactly as it does in the client.
    (max_w.ceil() + 1.0, render_lines.len() as f32 * r.size)
}

/// How many display rows `text` wraps into at `wrap_width` — the message-line half of the measure
/// round-trip ([`benilla_ui::script::UiScript::message_lines_needing_measure`]): the engine's
/// ScrollingMessageFrame allocates `rows × font-height` per ring line from this. Runs the exact
/// [`wrap_line`] pass rendering uses (same step law, same outline bias), so the band height always
/// equals the drawn block height. Never 0 (empty text is one blank row).
pub(crate) fn measure_wrapped_rows(
    e: &mut TextEngine,
    text: &str,
    wrap_width: f32,
    font: FontSpec,
) -> u16 {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let rows = wrapped_rows(e, r, text, wrap_width);
    rows.clamp(1, usize::from(u16::MAX)) as u16
}

/// The wrapped display-row count of `text` at `wrap_width` — the exact [`wrap_line`] pass the
/// render uses. An unconstrained width counts the `\n` lines.
fn wrapped_rows(e: &TextEngine, r: Resolved, text: &str, wrap_width: f32) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, usize::MAX)
}

/// [`wrapped_rows`] that stops once `cap` rows exist — the client's own fit walk is bounded by the
/// BOX, not by the string: `GxuFont_GetMaxCharsWithinHeight` (`0x5c21c0`) lays out `min(needed,
/// fits + 1)` lines and abandons the rest, because the height test sits downstream of the per-line
/// kernel call. A caller that only needs to know whether the text OVERFLOWS passes `allowed + 1`
/// and never pays for the tail; the chat band's row count, which is a real number, passes
/// [`usize::MAX`].
fn wrapped_rows_capped(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    cap: usize,
) -> usize {
    let lines = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    if wrap_width <= WRAP_MIN_WIDTH {
        return lines.len().min(cap);
    }
    let mut rows = 0usize;
    for line in &lines {
        rows += wrap_line(e, r, line, wrap_width).len();
        if rows >= cap {
            return cap;
        }
    }
    rows
}

/// [`wrapped_rows`], for the ellipsis-cost bench.
#[cfg(test)]
pub(super) fn wrapped_rows_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
) -> usize {
    wrapped_rows(e, r, text, wrap_width)
}

/// [`wrapped_rows_capped`], for the ellipsis-cost bench.
#[cfg(test)]
pub(super) fn wrapped_rows_capped_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    cap: usize,
) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, cap)
}

/// The FontString display string under the height-gated ellipsis-truncate — `CSimpleFontString
/// 0x771ec0`, regime 3 of the overflow law (decision 0292's named residue): when the wrapped text
/// needs more lines than the box allows, the tail is replaced by `"..."`, backed off one char at a
/// time until the candidate fits. `None` = draw the raw text
/// (fits, or the gate fails). The gate is geometric — `boxW > 0 && boxH > 0` on the resolved rect
/// (`maxLines` unmodeled; nothing shipped sets it): an auto-height FontString's rect height IS its
/// wrapped block (the measure round-trip), so it always fits and never truncates — the byte law's
/// intrinsic-height escape. Applies at the region-FontString paint seam only
/// ([`crate::ui_script::extract`]): the client's ellipsis is a FontString mechanism — editbox,
/// message-frame lines, and world text (`combat_text`/nameplates) never take it, exactly as their
/// C++ counterparts never call `0x771ec0`.
///
/// **The box is the box.** Under the ladder this had to divide the rect by the draw's rescale
/// before comparing it against unscaled glyphs, and getting that backwards is what truncated text
/// that fits (B209, decision 0989's named residual). There is no rescale now, so there is no
/// conversion here and none to get backwards.
pub(crate) fn ellipsize_to_fit(
    atlas: &mut UiFontAtlas,
    region: benilla_ui::widget::RegionHandle,
    text: &str,
    rect: Rect,
    font: FontSpec,
) -> Option<String> {
    if rect.width() <= WRAP_MIN_WIDTH || rect.height() <= HEIGHT_LIMIT_MIN {
        return None;
    }
    let (box_w, box_h) = (rect.width(), rect.height());
    // The remembered answer, under exactly these inputs ([`super::super::EllipsisMemo`] — the
    // client's `CGxString+0xf8`). The paint pass runs every frame; this seam is what the client
    // rebuilds only on invalidation, and it is the most expensive thing in the pass by an order of
    // magnitude (B240, decision 1332).
    if let Some(hit) = atlas.ellipsis.get(region, text, box_w, box_h, &font) {
        return hit.clone();
    }
    let display = {
        let mut e = atlas.lock();
        let r = resolve(&mut e, &font);
        // Every candidate is a prefix of `text` plus "..." — so one ensure covers the whole walk.
        e.ensure_metrics(r.face, r.ppem, text);
        e.ensure_metrics(r.face, r.ppem, "...");
        let e = &*e;
        // The fit test only has to decide OVERFLOW, so it stops one row past the box — the client's
        // own bound ([`wrapped_rows_capped`]).
        let cap = overflow::lines_fitting(box_h.max(r.size), r.size) + 1;
        overflow::ellipsize_in_box(text, box_h, r.size, |candidate| {
            wrapped_rows_capped(e, r, candidate, box_w, cap)
        })
    };
    atlas
        .ellipsis
        .put(region, text, box_w, box_h, &font, display.clone());
    display
}

/// The drawn line's origin for the EditBox text-UI overlays: the line's x0 under `justifyH` (where
/// the engine's advance-derived caret/selection x-offsets are measured from), plus the single-line
/// cell's top and height — the same face/size resolution, step law, and justify math as
/// [`super::layout_text_quads`], so the overlays sit exactly on the glyphs. Single-line by
/// construction (the EditBox law; a multiLine box would need the wrapped-line walk).
pub(crate) fn line_origin(
    e: &mut TextEngine,
    drawn: &str,
    rect: Rect,
    justify: Justify,
    font: FontSpec,
) -> (f32, f32, f32) {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, drawn);
    let e = &*e;
    // The line origin under justifyH (LEFT needs no width measure — the chat case).
    let x0 = match justify.h {
        JustifyH::Left => rect.min.x,
        JustifyH::Center | JustifyH::Right => {
            let w = measure_line_width(e, r, r.step_extra, drawn);
            if matches!(justify.h, JustifyH::Center) {
                rect.min.x + ((rect.width() - w) * 0.5).max(0.0)
            } else {
                rect.max.x - w
            }
        }
    };
    // The single-line block's v_offset — [`super::layout_text_quads`]'s law with block_h = 1 line,
    // through the same ONE vertical snap ([`super::snap_block_top`]) so the caret/selection cell
    // sits exactly on the drawn glyphs.
    let top = if rect.height() > f32::EPSILON {
        let v_offset = match justify.v {
            JustifyV::Top => 0.0,
            JustifyV::Middle => (rect.height() - r.size) * 0.5,
            JustifyV::Bottom => rect.height() - r.size,
        };
        super::snap_block_top(rect.min.y + v_offset)
    } else {
        rect.min.y
    };
    (x0, top, r.size)
}

/// The EditBox advance-table answer ([`benilla_ui::script::UiScript::set_editbox_advances`]'s
/// payload): per-BYTE cumulative laid-out widths of `text` under the exact step law
/// [`measure_line_width`] uses — len+1 entries, `[0] = 0`, each character's full step landing on
/// its END boundary and its interior (continuation) bytes holding the lead's value, so a mid-char
/// index degrades to the char's start and char-boundary lookups are exact.
///
/// **Indexed by the box's RAW byte, measured over what is DRAWN** (decision 1075). The box stores
/// the escaped string and draws only the visible one, so every `|c…`/`|r`/`|H…|h`/`|T…|t` byte
/// costs zero width: [`crate::ui_text::markup::visible_map`] carries each drawn byte back to its
/// raw offset, and the forward-fill below hands every escape byte the previous boundary's width.
/// Measuring the raw string instead is what parked the caret half a chat bar right of a
/// shift-clicked item link.
///
/// The old shaped-buffer walk carried a stated approximation — the whole drawn string was shaped as
/// one buffer while the draw shaped each color run separately, so kerning across a color boundary
/// could differ by a fraction of a pixel. Stepping per character removes it: there is no buffer and
/// no kerning anywhere in the law, so this table and the drawn pen are the same arithmetic.
pub(crate) fn line_advances(e: &mut TextEngine, text: &str, font: FontSpec) -> Vec<f32> {
    let mut cum = vec![0.0f32; text.len() + 1];
    if text.is_empty() {
        return cum;
    }
    let (drawn, bounds) = crate::ui_text::markup::visible_map(text);
    if drawn.is_empty() {
        return cum; // pure markup draws nothing — every boundary sits at x = 0
    }
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, &drawn);
    let e = &*e;
    let dpi = e.dpi();

    let mut written = vec![false; text.len() + 1];
    written[0] = true;
    // The same device-px pen the render walks (1644), so a caret boundary is the same number the
    // glyph before it was drawn at.
    let mut x_dev = 0.0f32;
    for (off, ch) in drawn.char_indices() {
        if let Some(c) = e.char_cell(r.face, r.ppem, ch) {
            for g in &c.glyphs {
                x_dev += client_step(g.advance, r.step_extra);
            }
        }
        // This character's end boundary in DRAWN bytes → the RAW boundary it sits on.
        let end = bounds[(off + ch.len_utf8()).min(drawn.len())];
        cum[end] = x_dev / dpi;
        written[end] = true;
    }
    // Forward-fill the unwritten slots (cluster interiors, markup bytes): each carries the
    // previous boundary's value.
    for i in 1..cum.len() {
        if !written[i] {
            cum[i] = cum[i - 1];
        }
    }
    cum
}

/// The multiline-EditBox row answer (the 2-D half of
/// [`benilla_ui::script::UiScript::set_editbox_advances`]'s payload): the byte offset where each
/// wrapped display row of `text` begins at `wrap_width` — the exact [`wrap_line`] pass the render
/// wraps with, so the engine's `(row, x)` caret/click law lands on the drawn rows — plus the row
/// pitch (the font em, the same `N·S` block law [`measure_text`] heights with). Row starts are
/// reconstructed by walking the source string past each wrapped row's verbatim text and the
/// separator the break swallowed (wrap keeps inter-word whitespace verbatim and drops only the
/// trailing separator, so the walk is exact) — in DRAWN bytes, mapped back to RAW ones, since the
/// rows index the same raw buffer [`line_advances`] does (decision 1075). Never empty (`[0]` for
/// empty text).
pub(crate) fn line_rows(
    e: &mut TextEngine,
    text: &str,
    wrap_width: f32,
    font: FontSpec,
) -> (Vec<usize>, f32) {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;
    let mut rows = Vec::new();
    let mut base = 0usize; // byte offset of the current '\n' segment within `text`
    for seg in text.split('\n') {
        let lines = parse_markup(seg, [1.0, 1.0, 1.0, 1.0]);
        let sub: Vec<Vec<ColorRun>> = if wrap_width > WRAP_MIN_WIDTH {
            lines
                .iter()
                .flat_map(|line| wrap_line(e, r, line, wrap_width))
                .collect()
        } else {
            lines
        };
        segment_row_starts(seg, &sub, base, &mut rows);
        base += seg.len() + 1; // + the '\n' itself (swallowed by the split)
    }
    if rows.is_empty() {
        rows.push(0);
    }
    (rows, r.size)
}

/// [`line_rows`]'s reconstruction walk for one `\n` segment: push the byte start of each wrapped
/// sub-line of `seg` (offset by `base`, `seg`'s position in the full string) into `rows`. Wrap
/// keeps inter-word whitespace verbatim inside a row and drops only the separator a break swallows,
/// so each subsequent row starts past the previous row's bytes plus any whitespace (a force-broken
/// word swallowed none — the skip loop stops at its first byte). A blank segment still occupies one
/// row.
fn segment_row_starts(seg: &str, sub: &[Vec<ColorRun>], base: usize, rows: &mut Vec<usize>) {
    if sub.is_empty() {
        rows.push(base);
        return;
    }
    // A wrapped row is made of DRAWN text (the markup is gone by then) while a row start must be a
    // RAW byte — the offset space the box's cursor and [`line_advances`] both live in. So the walk
    // runs in drawn bytes and maps each start back (decision 1075).
    let (drawn, bounds) = crate::ui_text::markup::visible_map(seg);
    let mut p = 0usize; // byte cursor within `drawn`
    for (j, line) in sub.iter().enumerate() {
        if j > 0 {
            while let Some(c) = drawn[p.min(drawn.len())..].chars().next() {
                if c.is_whitespace() {
                    p += c.len_utf8();
                } else {
                    break;
                }
            }
        }
        rows.push(base + bounds[p.min(drawn.len())]);
        p += line.iter().map(|r| r.text.len()).sum::<usize>();
    }
}

#[cfg(test)]
mod row_start_tests {
    use super::*;

    fn runs(texts: &[&str]) -> Vec<Vec<ColorRun>> {
        texts
            .iter()
            .map(|t| {
                vec![ColorRun {
                    text: (*t).to_string(),
                    color: [1.0; 4],
                    link: None,
                }]
            })
            .collect()
    }

    fn starts(seg: &str, sub: &[Vec<ColorRun>], base: usize) -> Vec<usize> {
        let mut out = Vec::new();
        segment_row_starts(seg, sub, base, &mut out);
        out
    }

    #[test]
    fn a_word_break_swallows_its_separator() {
        assert_eq!(starts("hello world", &runs(&["hello", "world"]), 0), [0, 6]);
    }

    #[test]
    fn a_force_broken_word_swallows_nothing() {
        assert_eq!(
            starts(
                "supercalifragilistic",
                &runs(&["supercali", "fragilistic"]),
                0
            ),
            [0, 9]
        );
    }

    #[test]
    fn a_double_space_separator_is_swallowed_whole() {
        assert_eq!(starts("one  two", &runs(&["one", "two"]), 0), [0, 5]);
    }

    #[test]
    fn inner_whitespace_stays_verbatim_inside_a_row() {
        assert_eq!(
            starts("a b  c d", &runs(&["a b  c", "d"]), 0),
            [0, 7],
            "only the BREAK's separator is dropped"
        );
    }

    #[test]
    fn a_blank_segment_still_occupies_a_row_and_base_offsets() {
        assert_eq!(starts("", &[], 12), [12]);
        assert_eq!(starts("hi there", &runs(&["hi", "there"]), 100), [100, 103]);
    }
}
