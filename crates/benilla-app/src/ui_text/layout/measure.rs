//! Measure and fit, in the emit pass's own space ([`super`]): the width law, the wrap walk and the
//! ellipsis seam. A width is a pure function of face, raster size, outline and characters, which
//! lets a measure answer from inside a Lua call where no shaper reaches.

use bevy::math::Rect;

use benilla_ui::script::{JustifyH, JustifyV};

use super::super::engine::{TextEngine, UiFontAtlas};
use super::super::markup::{fontstring_lines, parse_markup, ColorRun};
use super::wrap::{greedy_pack, tokenize_words};
use super::{client_step, overflow, FontSpec, Justify, HEIGHT_LIMIT_MIN, WRAP_MIN_WIDTH};

/// A [`FontSpec`] resolved once, so measure and draw agree on face and raster size.
#[derive(Clone, Copy)]
pub(super) struct Resolved {
    /// Index into the engine's faces.
    pub(super) face: usize,
    /// The exact integer device-pixel size ([`TextEngine::ppem`]).
    pub(super) ppem: u16,
    /// The logical height `ppem` draws at, not the requested one: the pitch and every block em.
    pub(super) size: f32,
    /// The outline cell variant (`0` = plain).
    pub(super) radius: u8,
    /// The step law's per-glyph bias ([`super::step_extra_of`]).
    pub(super) step_extra: f32,
}

/// Resolve a spec without touching the caches ([`TextEngine::ensure_metrics`] covers the text).
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

/// One line's width in logical px, the sum of its characters' steps ([`client_step`]) as the
/// reference's `GetTextWidth` sums them; every measure, the one inside a Lua call
/// ([`benilla_ui::script::TextMeasure`]) included, runs through here. The caller ensures `text`'s
/// metrics first: this is `&`-only, and a character not in the cache counts 0.
pub(super) fn measure_line_width(e: &TextEngine, r: Resolved, step_extra: f32, text: &str) -> f32 {
    // Summed in device px and divided once, as the pen does: a sum of quotients is another `f32`,
    // and measure must equal render to the bit.
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

/// Greedy wrap of one markup line into rows no wider than `max_width` px, the reference's wrap
/// kernel (`0x5c6c50`/`0x5c7780`): break at the last opportunity, else force-break at the last
/// fitting glyph; inter-word whitespace stays verbatim and only the separator at a break drops.
///
/// Break opportunities are whitespace only: the reference's classifier (`0x5c7780`) also breaks
/// after `-`, `:` and `/` and by CJK kinsoku classes, and its `nonspacewrap` flag (ui `0x1000`) is
/// not modelled; of the flag's three stock users, only `ScriptErrors_Message` wraps past one line.
pub(super) fn wrap_line(
    e: &TextEngine,
    r: Resolved,
    line: &[ColorRun],
    max_width: f32,
) -> Vec<Vec<ColorRun>> {
    let words = tokenize_words(line);
    if words.is_empty() {
        // A blank or all-whitespace line still takes a row.
        return vec![line.to_vec()];
    }
    greedy_pack(words, max_width, |t| {
        measure_line_width(e, r, r.step_extra, t)
    })
}

/// `text`'s laid-out size, the widest line by all lines' height, wrapping at `wrap_width` by the
/// render's own [`wrap_line`] pass; the engine's measure round-trip sizes height-less FontStrings
/// by it ([`benilla_ui::script::UiScript::fontstrings_needing_measure`]).
pub(crate) fn measure_text(
    e: &mut TextEngine,
    text: &str,
    wrap_width: Option<f32>,
    font: FontSpec,
) -> (f32, f32) {
    let r = resolve(e, &font);
    // Metrics only: a string measured and never drawn costs no raster.
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
    // Deviation: one pixel of headroom over the reference's measured width, kept because dropping
    // it narrows every auto-sized FontString, a look change.
    // Height: N lines at the font em with no outline pad (`0x5c2070`: `N·S + (N-1)·gap`, gap 0 in
    // shipped UI); an outlined ring pokes past it, as in the reference.
    (max_w.ceil() + 1.0, render_lines.len() as f32 * r.size)
}

/// How many rows `text` wraps into at `wrap_width`, by the render's own [`wrap_line`] pass: the
/// engine's ScrollingMessageFrame allocates `rows × font height` per line from it
/// ([`benilla_ui::script::UiScript::message_lines_needing_measure`]). Never 0.
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

/// The wrapped row count of `text` at `wrap_width`; an unconstrained width counts the `\n` lines.
fn wrapped_rows(e: &TextEngine, r: Resolved, text: &str, wrap_width: f32) -> usize {
    wrapped_rows_capped(e, r, text, wrap_width, usize::MAX)
}

/// [`wrapped_rows`], stopping at `cap` as the reference's fit walk (`0x5c21c0`) lays out only
/// `min(needed, fits + 1)` lines: an overflow test passes `allowed + 1`, the chat band's real count
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

/// [`wrapped_rows`], for the tests.
#[cfg(test)]
pub(super) fn wrapped_rows_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
) -> usize {
    wrapped_rows(e, r, text, wrap_width)
}

/// [`wrapped_rows_capped`], for the tests.
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

/// The display string under the reference's height-gated ellipsis truncate (`CSimpleFontString`
/// `0x771ec0`): wrapped past the box, the text backs off a char at a time behind `"..."` until it
/// fits; `None` draws it raw. The gate is `boxW > 0 && boxH > 0` (`maxLines` is not modelled), so
/// an auto-height rect never truncates. Region FontStrings only: edit boxes, message lines and
/// world text never reach `0x771ec0` in the reference either.
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
    // The remembered answer ([`super::super::EllipsisMemo`], the reference's `CGxString+0xf8`):
    // the paint runs every frame, and this is by far its costliest step.
    if let Some(hit) = atlas.ellipsis.get(region, text, box_w, box_h, &font) {
        return hit.clone();
    }
    let display = {
        let mut e = atlas.lock();
        let r = resolve(&mut e, &font);
        // Every candidate is a prefix of `text` plus "...", so these two cover the whole walk.
        e.ensure_metrics(r.face, r.ppem, text);
        e.ensure_metrics(r.face, r.ppem, "...");
        let e = &*e;
        // Overflow is decided one row past the box, the reference's bound (`wrapped_rows_capped`).
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

/// The EditBox overlays' line origin (x0 under `justifyH`, where caret offsets start) and one-line
/// cell top and height, by [`super::layout_text_quads`]'s own step law and snap. Single-line only.
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
    // The draw's one-line `v_offset` and snap, so the caret cell sits on the drawn glyphs.
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

/// The EditBox advance table ([`benilla_ui::script::UiScript::set_editbox_advances`]): the drawn
/// width at each raw byte boundary of `text`, `len + 1` entries from 0; a char's inner bytes read
/// as its start. Measured over the drawn string ([`crate::ui_text::markup::visible_map`]), so
/// markup costs nothing; measuring the raw string puts the caret right of a shift-clicked link.
pub(crate) fn line_advances(e: &mut TextEngine, text: &str, font: FontSpec) -> Vec<f32> {
    let mut cum = vec![0.0f32; text.len() + 1];
    if text.is_empty() {
        return cum;
    }
    let (drawn, bounds) = crate::ui_text::markup::visible_map(text);
    if drawn.is_empty() {
        return cum; // pure markup draws nothing: every boundary sits at x = 0
    }
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, &drawn);
    let e = &*e;
    let dpi = e.dpi();

    let mut written = vec![false; text.len() + 1];
    written[0] = true;
    // The render's device-px pen, so a caret boundary is the x the glyph before it was drawn at.
    let mut x_dev = 0.0f32;
    for (off, ch) in drawn.char_indices() {
        if let Some(c) = e.char_cell(r.face, r.ppem, ch) {
            for g in &c.glyphs {
                x_dev += client_step(g.advance, r.step_extra);
            }
        }
        // The character's end in drawn bytes, mapped to its raw boundary.
        let end = bounds[(off + ch.len_utf8()).min(drawn.len())];
        cum[end] = x_dev / dpi;
        written[end] = true;
    }
    // Unwritten slots (char interiors, markup bytes) carry the previous boundary's value.
    for i in 1..cum.len() {
        if !written[i] {
            cum[i] = cum[i - 1];
        }
    }
    cum
}

/// The multiline EditBox row table: the raw byte where each wrapped row of `text` starts at
/// `wrap_width`, by the render's own [`wrap_line`] pass, and the row pitch (the font em). Never
/// empty (`[0]` for empty text).
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

/// [`line_rows`]'s walk for one `\n` segment at `base`: each row starts past the previous row's
/// bytes and the whitespace its break dropped (none after a force-break); a blank segment still
/// takes a row.
fn segment_row_starts(seg: &str, sub: &[Vec<ColorRun>], base: usize, rows: &mut Vec<usize>) {
    if sub.is_empty() {
        rows.push(base);
        return;
    }
    // Rows hold drawn text, but a row start is a raw byte, the space the cursor and
    // [`line_advances`] use: walk drawn bytes and map each start back.
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
