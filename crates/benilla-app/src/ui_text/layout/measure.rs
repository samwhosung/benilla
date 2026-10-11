//! Measure and fit, in the emit pass's own space ([`super`]): the width law, the wrap walk and the
//! ellipsis seam. A width is a pure function of face, raster size, outline and characters, which
//! lets a measure answer from inside a Lua call where no shaper reaches.

use bevy::math::Rect;

use benilla_ui::script::{JustifyH, JustifyV, Outline};

use super::super::engine::{GlyphRef, TextEngine, UiFontAtlas};
use super::super::markup::{fontstring_lines, parse_markup, ColorRun};
use super::wrap::{greedy_pack, row_limit, tokenize_words};
use super::{overflow, FontSpec, Justify, HEIGHT_LIMIT_MIN, WRAP_MIN_WIDTH};

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
    /// The line gap, whole device pixels in logical px ([`super::line_gap`]).
    pub(super) gap: f32,
    /// The outline's growth of a line's last glyph in device px ([`outline_lead`]).
    pub(super) lead: f32,
    /// What seeds each laid-out row's pen in device px: the outline's lead and the drawn shadow's
    /// x offset ([`FontSpec::shadow_x`], [`shadow_px`]), as the line builder is handed both
    /// (`0x5cddb1`, `0x5c7470` @`0x5c749d`–`0x5c74f2`).
    pub(super) seed: f32,
}

/// `n` lines' height, `n·S + (n−1)·gap` (`0x5c2070`); 0 for none.
pub(super) fn block_height(n: usize, r: Resolved) -> f32 {
    if n == 0 {
        return 0.0;
    }
    n as f32 * r.size + (n - 1) as f32 * r.gap
}

/// Resolve a spec without touching the caches ([`TextEngine::ensure_metrics`] covers the text).
pub(super) fn resolve(e: &mut TextEngine, font: &FontSpec) -> Resolved {
    let face = e.face_for(font.path);
    let ppem = e.ppem(font.height.unwrap_or(super::super::DEFAULT_FONT_SIZE));
    let lead = outline_lead(font.outline);
    Resolved {
        face,
        ppem,
        size: e.logical_size(ppem),
        radius: super::super::outline::radius_of(font.outline),
        step_extra: super::step_extra_of(font.outline),
        gap: super::line_gap(font.spacing, e.dpi()),
        lead,
        seed: lead + shadow_px(font.shadow_x, e.dpi()),
    }
}

/// What an outline adds to a line in device px, twice: the reference seeds every line's pen with
/// it before the first glyph (`0x5c6940` @`0x5c69c0`, `0x5c7470` @`0x5c74d0`), and its rasterizer
/// widens each glyph's bitmap by it (`0x5ca160` → `0x5d1120`, `out[4]`), which the line's last glyph
/// counts. 4 for THICK (font flag `0x8`), 2 for NORMAL (`0x1`).
fn outline_lead(outline: Outline) -> f32 {
    match outline {
        Outline::Thick => 4.0,
        Outline::Normal => 2.0,
        Outline::None => 0.0,
    }
}

/// A shadow's x offset, in logical px, as the pen seed it is in device px: whole pixels when it
/// points right, else nothing (`0x5c69a4`/`0x5c74b4`: `round` then `ceil`, only past 0).
fn shadow_px(shadow_x: f32, dpi: f32) -> f32 {
    if shadow_x > 0.0 {
        (shadow_x * dpi + 0.5).floor()
    } else {
        0.0
    }
}

/// The first glyph `c` draws, the one a step looks at past its own glyph (`0x5c6b70`).
fn first_glyph(e: &TextEngine, r: Resolved, c: char) -> Option<GlyphRef> {
    e.char_cell(r.face, r.ppem, c)
        .and_then(|cell| cell.glyphs.first().copied())
}

/// One line's extent in device px past its seed, with its first and last glyphs. Each glyph steps
/// to the next ([`TextEngine::step`], kerned); the last looks past itself as the reference does
/// (`0x5c6b70`): when any character follows it, in `text` or as `next`, the source character
/// after it, it takes its step to that character, else its ink plus the outline's growth. A
/// character with no glyph steps nothing and keeps the glyph before it, as the kernels skip it.
fn line_extent_dev(
    e: &TextEngine,
    r: Resolved,
    text: impl Iterator<Item = char>,
    next: Option<char>,
) -> (f32, Option<GlyphRef>, Option<GlyphRef>) {
    let mut glyphs: Vec<GlyphRef> = Vec::new();
    // A glyphless character after the last glyph: a code the look-ahead steps to.
    let mut code_after: Option<Option<GlyphRef>> = None;
    for c in text {
        match e.char_cell(r.face, r.ppem, c) {
            Some(cell) if !cell.glyphs.is_empty() => {
                glyphs.extend_from_slice(&cell.glyphs);
                code_after = None;
            }
            _ if !glyphs.is_empty() && code_after.is_none() => code_after = Some(None),
            _ => {}
        }
    }
    let (Some(&first), Some(&last)) = (glyphs.first(), glyphs.last()) else {
        return (0.0, None, None);
    };
    let step = |a: &GlyphRef, b: Option<&GlyphRef>| e.step(r.face, r.ppem, r.step_extra, a, b);
    let mut w: f32 = glyphs.windows(2).map(|p| step(&p[0], Some(&p[1]))).sum();
    let follow = code_after.or_else(|| next.map(|c| first_glyph(e, r, c)));
    let ink = last.ink + r.lead;
    w += match follow {
        // `0x5c6beb`: a zero step falls through to the ink.
        Some(right) => Some(step(&last, right.as_ref()))
            .filter(|s| *s != 0.0)
            .unwrap_or(ink),
        None => ink,
    };
    (w, Some(first), Some(last))
}

/// `GetStringWidth`'s measure in logical px, the kernel `0x5c6940` as `0x772890` calls it: each
/// line is [`line_extent_dev`] with nothing after its last glyph, the widest wins, and nothing
/// rounds it or pads it. The first line's pen is seeded with the outline's lead and the shadow's
/// x offset ([`shadow_px`]); `shadow_x` is in this measure's logical px, which `0x772890` leaves
/// unscaled by the frame (`[fs+0x108]`), unlike the draw's ([`FontSpec::shadow_x`]). The
/// reference's kernel resets the pen at a break but not the previous glyph (`0x5c6aa0`–`0x5c6abe`
/// leave `[ebp-0x10]`), so in its measure each later line opens with the step from the last glyph
/// before it, kerned as any step, and without the seed.
pub(crate) fn string_width(e: &mut TextEngine, text: &str, font: FontSpec, shadow_x: f32) -> f32 {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;
    let dpi = e.dpi();
    let mut widest = 0.0f32;
    let mut carry: Option<GlyphRef> = None;
    for (i, line) in fontstring_lines(text, [1.0, 1.0, 1.0, 1.0])
        .iter()
        .enumerate()
    {
        let chars = line.iter().flat_map(|run| run.text.chars());
        let (extent, first, last) = line_extent_dev(e, r, chars, None);
        let open = if i == 0 {
            r.lead + shadow_px(shadow_x, dpi)
        } else {
            // The previous glyph the break kept (`0x5c6aa0`), stepped to this line's first.
            match (carry, first) {
                (Some(p), Some(f)) => e.step(r.face, r.ppem, r.step_extra, &p, Some(&f)),
                _ => 0.0,
            }
        };
        widest = widest.max(open + extent);
        carry = last.or(carry);
    }
    widest / dpi
}

/// Greedy wrap of one markup line into rows within `max_width` px, the reference's wrap kernel
/// (`0x5c6c50`/`0x5c7780`): break at the last opportunity, else force-break at the last fitting
/// glyph; inter-word whitespace stays verbatim and only the separator at a break drops. A row is
/// priced as its line builder prices it (`0x5c7470`): its seed, then its glyphs' steps, the last
/// looking past itself to the source character after it ([`line_extent_dev`]), and it fits when
/// that is within the width rounded up to a whole device pixel (`0x5c752a`–`0x5c7530`). Each row
/// comes with the source character after it, `None` at the line's end.
///
/// Break opportunities are whitespace only: the reference's classifier (`0x5c7780`) also breaks
/// after `-`, `:` and `/` and by CJK kinsoku classes, and its `nonspacewrap` flag (ui `0x1000`) is
/// not modelled; of the flag's three stock users, only `ScriptErrors_Message` wraps past one line.
pub(super) fn wrap_line(
    e: &TextEngine,
    r: Resolved,
    line: &[ColorRun],
    max_width: f32,
) -> Vec<(Vec<ColorRun>, Option<char>)> {
    let words = tokenize_words(line);
    if words.is_empty() {
        // A blank or all-whitespace line still takes a row.
        return vec![(line.to_vec(), None)];
    }
    let trailing = trailing_whitespace(line);
    greedy_pack(
        words,
        row_limit(max_width, e.dpi()),
        &trailing,
        |t, next| r.seed + line_extent_dev(e, r, t.chars(), next).0,
    )
}

/// The whitespace ending a markup line, which the word walk leaves to the packer's end.
fn trailing_whitespace(line: &[ColorRun]) -> String {
    let mut tail: Vec<char> = Vec::new();
    for run in line.iter().rev() {
        for c in run.text.chars().rev() {
            if !c.is_whitespace() {
                return tail.iter().rev().collect();
            }
            tail.push(c);
        }
    }
    tail.iter().rev().collect()
}

/// A laid-out row's width in logical px, its seed included, as the line builder hands it to the
/// justify (`0x5c7470` → `0x5cddf9`); `next` is the source character after it ([`wrap_line`]).
pub(super) fn row_width(e: &TextEngine, r: Resolved, row: &[ColorRun], next: Option<char>) -> f32 {
    let chars = row.iter().flat_map(|run| run.text.chars());
    (r.seed + line_extent_dev(e, r, chars, next).0) / e.dpi()
}

/// [`wrap_line`] over every source line of `lines`.
pub(super) fn wrap_rows(
    e: &TextEngine,
    r: Resolved,
    lines: &[Vec<ColorRun>],
    max_width: f32,
) -> Vec<(Vec<ColorRun>, Option<char>)> {
    lines
        .iter()
        .flat_map(|line| wrap_line(e, r, line, max_width))
        .collect()
}

/// `text`'s laid-out size, by all lines' height: unwrapped, the width is [`string_width`]'s with
/// the spec's shadow, the one the reference keeps (`[fs+0xfc]`); wrapped at `wrap_width`, it is the
/// widest row the render's own [`wrap_rows`] pass lays. The engine's measure round-trip sizes
/// height-less FontStrings by it ([`benilla_ui::script::UiScript::fontstrings_needing_measure`]).
pub(crate) fn measure_text(
    e: &mut TextEngine,
    text: &str,
    wrap_width: Option<f32>,
    font: FontSpec,
) -> (f32, f32) {
    let r = resolve(e, &font);
    // Metrics only: a string measured and never drawn costs no raster.
    e.ensure_metrics(r.face, r.ppem, text);
    let lines = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    // Height: N lines at the font em with no outline pad, the gap between them (`0x5c2070`:
    // `N·S + (N-1)·gap`); an outlined ring pokes past it, as in the reference.
    let (w, rows) = match wrap_width {
        Some(w) if w > WRAP_MIN_WIDTH => {
            let e = &*e;
            let rows = wrap_rows(e, r, &lines, w);
            let widest = rows
                .iter()
                .map(|(row, next)| row_width(e, r, row, *next))
                .fold(0.0f32, f32::max);
            (widest, rows.len())
        }
        _ => (string_width(e, text, font, font.shadow_x), lines.len()),
    };
    (w, block_height(rows, r))
}

/// `text`'s unwrapped height, its lines and the gaps between them ([`measure_text`]'s, without a
/// width).
pub(crate) fn unwrapped_height(e: &mut TextEngine, text: &str, font: FontSpec) -> f32 {
    let r = resolve(e, &font);
    block_height(fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]).len(), r)
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
        let cap = overflow::lines_fitting(box_h.max(r.size + r.gap), r.size, r.gap) + 1;
        overflow::ellipsize_in_box(text, box_h, r.size, r.gap, |candidate| {
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
            let w = (r.seed + line_extent_dev(e, r, drawn.chars(), None).0) / e.dpi();
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
    // Each glyph with the glyph after it, so a kerned step lands where the draw lands it.
    let glyphs: Vec<(usize, GlyphRef)> = drawn
        .char_indices()
        .flat_map(|(off, ch)| {
            e.char_cell(r.face, r.ppem, ch)
                .map(|c| c.glyphs.iter().map(move |g| (off, *g)).collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect();
    // The render's device-px pen, so a caret boundary is the x the glyph after it is drawn at.
    let mut x_dev = 0.0f32;
    let mut k = 0usize;
    for (off, ch) in drawn.char_indices() {
        while k < glyphs.len() && glyphs[k].0 == off {
            let next = glyphs.get(k + 1).map(|(_, g)| g);
            x_dev += e.step(r.face, r.ppem, r.step_extra, &glyphs[k].1, next);
            k += 1;
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
/// `wrap_width`, by the render's own [`wrap_line`] pass, and the row pitch, the font em plus the
/// line gap (`0x77d133`-`0x77d161`). Never empty (`[0]` for empty text).
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
                .map(|(row, _)| row)
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
    (rows, r.size + r.gap)
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
