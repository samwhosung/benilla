//! Measure and fit, in the emit pass's own space ([`super`]): the width law, the wrap walk and the
//! ellipsis seam. A width is a pure function of face, raster size, outline and characters, which
//! lets a measure answer from inside a Lua call where no shaper reaches.

use bevy::math::Rect;

use benilla_ui::script::{JustifyH, JustifyV, Outline};

use super::super::engine::{GlyphRef, TextEngine, UiFontAtlas};
use super::super::markup::{fontstring_lines, parse_markup, ColorRun};
use super::super::outline::Bake;
use super::wrap::{self, row_limit, RowPricer};
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
    /// The cell's bake: the outline's ring and the monochrome raster.
    pub(super) bake: Bake,
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
        bake: Bake::of(font.outline, font.monochrome),
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

/// A row priced as it grows, the step law of [`line_extent_dev`] one character at a time: the
/// wrap's [`RowPricer`], so a row of `n` characters costs `n` steps, not `n²`.
struct EnginePricer<'a> {
    e: &'a TextEngine,
    r: Resolved,
    /// The steps between the glyphs so far, in device px.
    inner: f32,
    first: Option<GlyphRef>,
    last: Option<GlyphRef>,
    /// A glyphless character after the last glyph: a code the look-ahead steps to.
    code_after: Option<Option<GlyphRef>>,
}

impl<'a> EnginePricer<'a> {
    fn new(e: &'a TextEngine, r: Resolved) -> Self {
        Self {
            e,
            r,
            inner: 0.0,
            first: None,
            last: None,
            code_after: None,
        }
    }

    /// The row's extent in device px past its seed, with its first and last glyphs: each glyph
    /// steps to the next ([`TextEngine::step`], kerned); the last looks past itself as the
    /// reference does (`0x5c6b70`): when any character follows it, in the row or as `next`, the
    /// source character after it, it takes its step to that character, else its ink plus the
    /// outline's growth.
    fn extent(&self, next: Option<char>) -> (f32, Option<GlyphRef>, Option<GlyphRef>) {
        let (Some(first), Some(last)) = (self.first, self.last) else {
            return (0.0, None, None);
        };
        let r = self.r;
        let follow = self
            .code_after
            .or_else(|| next.map(|c| first_glyph(self.e, r, c)));
        let ink = last.ink + r.lead;
        let tail = match follow {
            // `0x5c6beb`: a zero step falls through to the ink.
            Some(right) => Some(
                self.e
                    .step(r.face, r.ppem, r.step_extra, &last, right.as_ref()),
            )
            .filter(|s| *s != 0.0)
            .unwrap_or(ink),
            None => ink,
        };
        (self.inner + tail, Some(first), Some(last))
    }
}

impl RowPricer for EnginePricer<'_> {
    fn reset(&mut self) {
        *self = Self::new(self.e, self.r);
    }

    fn push(&mut self, c: char) -> bool {
        let r = self.r;
        match self.e.char_cell(r.face, r.ppem, c) {
            Some(cell) if !cell.glyphs.is_empty() => {
                for g in &cell.glyphs {
                    match &self.last {
                        Some(prev) => {
                            self.inner += self.e.step(r.face, r.ppem, r.step_extra, prev, Some(g));
                        }
                        None => self.first = Some(*g),
                    }
                    self.last = Some(*g);
                }
                self.code_after = None;
                true
            }
            _ => {
                if self.last.is_some() && self.code_after.is_none() {
                    self.code_after = Some(None);
                }
                false
            }
        }
    }

    fn width(&self, next: Option<char>) -> f32 {
        self.r.seed + self.extent(next).0
    }
}

/// One line's extent in device px past its seed, with its first and last glyphs
/// ([`EnginePricer::extent`]). A character with no glyph steps nothing and keeps the glyph before
/// it, as the kernels skip it.
fn line_extent_dev(
    e: &TextEngine,
    r: Resolved,
    text: impl Iterator<Item = char>,
    next: Option<char>,
) -> (f32, Option<GlyphRef>, Option<GlyphRef>) {
    let mut p = EnginePricer::new(e, r);
    for c in text {
        p.push(c);
    }
    p.extent(next)
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

/// Wrap the `lines` of one text into rows within `max_width` px, the reference's line kernel
/// (`0x5c7470`, [`wrap::wrap_text`]): a row ends at the last break opportunity the classifier
/// (`0x5c7780`) found, else is cut at the last fitting glyph; inter-word whitespace stays verbatim
/// and only the whitespace at a break drops. A row is priced as its line builder prices it: its
/// seed, then its glyphs' steps, the last looking past itself to the source character after it
/// ([`EnginePricer`]), and it fits when that is within the width rounded up to a whole device
/// pixel (`0x5c752a`-`0x5c7530`). `non_space_wrap` is the string's flag `0x40`; `max_rows` stops
/// the walk once that many rows are laid.
pub(super) fn wrap_lines(
    e: &TextEngine,
    r: Resolved,
    lines: &[Vec<ColorRun>],
    max_width: f32,
    non_space_wrap: bool,
    max_rows: usize,
) -> wrap::Laid {
    wrap::wrap_text(
        lines,
        row_limit(max_width, e.dpi()),
        non_space_wrap,
        max_rows,
        &mut EnginePricer::new(e, r),
    )
}

/// A laid-out row's width in logical px, its seed included, as the line builder hands it to the
/// justify (`0x5c7470` → `0x5cddf9`); `next` is the source character after it ([`wrap_lines`]).
pub(super) fn row_width(e: &TextEngine, r: Resolved, row: &[ColorRun], next: Option<char>) -> f32 {
    let chars = row.iter().flat_map(|run| run.text.chars());
    (r.seed + line_extent_dev(e, r, chars, next).0) / e.dpi()
}

/// [`wrap_lines`] with each row's runs and the source character after it.
pub(super) fn wrap_rows(
    e: &TextEngine,
    r: Resolved,
    lines: &[Vec<ColorRun>],
    max_width: f32,
    non_space_wrap: bool,
) -> Vec<(Vec<ColorRun>, Option<char>)> {
    wrap_lines(e, r, lines, max_width, non_space_wrap, usize::MAX)
        .rows
        .into_iter()
        .map(|row| (row.runs, row.next))
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
            let rows = wrap_rows(e, r, &lines, w, font.non_space_wrap);
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
    let rows = wrapped_rows(e, r, text, wrap_width, font.non_space_wrap);
    rows.clamp(1, usize::from(u16::MAX)) as u16
}

/// The wrapped row count of `text` at `wrap_width`; an unconstrained width counts the `\n` lines.
fn wrapped_rows(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    non_space_wrap: bool,
) -> usize {
    let lines = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    if wrap_width <= WRAP_MIN_WIDTH {
        return lines.len();
    }
    wrap_lines(e, r, &lines, wrap_width, non_space_wrap, usize::MAX)
        .rows
        .len()
}

/// The row count of the reference's fit walk (`0x5c21c0`), which lays out only `min(needed, fits +
/// 1)` lines: an overflow test passes `allowed + 1` as `cap`, and a count at `cap` is an overflow.
/// The walk also stops after a force-broken line while the string's `nonspacewrap` is off
/// (`0x5c22e8`-`0x5c22f5`, the carry byte it passed down), so the text after it does not fit
/// however many lines the box has left: that too counts `cap`.
fn fit_rows(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    cap: usize,
    non_space_wrap: bool,
) -> usize {
    let lines = fontstring_lines(text, [1.0, 1.0, 1.0, 1.0]);
    if wrap_width <= WRAP_MIN_WIDTH {
        return lines.len().min(cap);
    }
    let laid = wrap_lines(e, r, &lines, wrap_width, non_space_wrap, cap);
    // A row no byte of which fits ends the walk short of the text (`0x5c22c0`), as a force-cut one
    // does while `nonspacewrap` is off.
    let stops = laid.bailed_at.is_some()
        || (!non_space_wrap && laid.rows.iter().any(|row| row.forced && row.more));
    if stops {
        cap
    } else {
        laid.rows.len()
    }
}

/// [`wrapped_rows`], for the tests.
#[cfg(test)]
pub(super) fn wrapped_rows_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    non_space_wrap: bool,
) -> usize {
    wrapped_rows(e, r, text, wrap_width, non_space_wrap)
}

/// [`fit_rows`], for the tests.
#[cfg(test)]
pub(super) fn fit_rows_for_test(
    e: &TextEngine,
    r: Resolved,
    text: &str,
    wrap_width: f32,
    cap: usize,
    non_space_wrap: bool,
) -> usize {
    fit_rows(e, r, text, wrap_width, cap, non_space_wrap)
}

/// The display string under the reference's ellipsis truncate (`CSimpleFontString` `0x771ec0`),
/// remembered per region ([`super::super::EllipsisMemo`]); `None` draws the text raw. See
/// [`ellipsize_text`] for the law. Region FontStrings only: edit boxes, message lines and world
/// text never reach `0x771ec0` in the reference either.
pub(crate) fn ellipsize_to_fit(
    atlas: &mut UiFontAtlas,
    region: benilla_ui::widget::RegionHandle,
    text: &str,
    rect: Rect,
    font: FontSpec,
) -> Option<String> {
    if rect.width() <= WRAP_MIN_WIDTH || (font.max_lines == 0 && rect.height() <= HEIGHT_LIMIT_MIN)
    {
        return None;
    }
    let (box_w, box_h) = (rect.width(), rect.height());
    // The remembered answer ([`super::super::EllipsisMemo`], the reference's `CGxString+0xf8`):
    // the paint runs every frame, and this is by far its costliest step.
    if let Some(hit) = atlas.ellipsis.get(region, text, box_w, box_h, &font) {
        return hit.clone();
    }
    let display = ellipsize_text(&mut atlas.lock(), text, box_w, box_h, font);
    atlas
        .ellipsis
        .put(region, text, box_w, box_h, &font, display.clone());
    display
}

/// The reference's ellipsis truncate (`0x771ec0`) of `text` in a `box_w` by `box_h` box (px of
/// `font`'s space): it runs for any box with a width (`0x771efb`) and either a height
/// (`0x771f19`) or a `maxLines` (`0x771f0f`), which sets the height to that many line pitches
/// (`0x771f48`-`0x771f61`) whatever the box holds; the text that wraps past it backs off a char at
/// a time behind `"..."` until the fit walk takes it ([`fit_rows`]). `None`: it fits.
pub(crate) fn ellipsize_text(
    e: &mut TextEngine,
    text: &str,
    box_w: f32,
    box_h: f32,
    font: FontSpec,
) -> Option<String> {
    let r = resolve(e, &font);
    // Every candidate is a prefix of `text` plus "...", so these two cover the whole walk.
    e.ensure_metrics(r.face, r.ppem, text);
    e.ensure_metrics(r.face, r.ppem, "...");
    let e = &*e;
    let box_h = if font.max_lines > 0 {
        font.max_lines as f32 * (r.size + r.gap)
    } else {
        box_h
    };
    // Overflow is decided one row past the box, the reference's bound ([`fit_rows`]).
    let cap = overflow::lines_fitting(box_h.max(r.size + r.gap), r.size, r.gap) + 1;
    overflow::ellipsize_in_box(text, box_h, r.size, r.gap, |candidate| {
        fit_rows(e, r, candidate, box_w, cap, font.non_space_wrap)
    })
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
/// `wrap_width`, by the render's own [`wrap_lines`] pass, and the row pitch, the font em plus the
/// line gap (`0x77d133`-`0x77d161`). Never empty (`[0]` for empty text). A trailing break leaves
/// the empty row its caret sits on, which the FontString law does not count.
pub(crate) fn line_rows(
    e: &mut TextEngine,
    text: &str,
    wrap_width: f32,
    font: FontSpec,
) -> (Vec<usize>, f32) {
    let r = resolve(e, &font);
    e.ensure_metrics(r.face, r.ppem, text);
    let e = &*e;
    let lines = parse_markup(text, [1.0, 1.0, 1.0, 1.0]);
    let starts: Vec<usize> = if wrap_width > WRAP_MIN_WIDTH {
        // The enumerator records a row's start before it asks the kernel (`0x5c24a9`), so the row
        // that ends the layout counts.
        let laid = wrap_lines(e, r, &lines, wrap_width, font.non_space_wrap, usize::MAX);
        laid.rows
            .iter()
            .map(|row| row.start)
            .chain(laid.bailed_at)
            .collect()
    } else {
        let mut at = 0usize;
        lines
            .iter()
            .map(|line| {
                let start = at;
                // A character, a line break and a code are one element each.
                at += line
                    .iter()
                    .map(|run| run.text.chars().count() + run.marks as usize)
                    .sum::<usize>()
                    + 1;
                start
            })
            .collect()
    };
    let mut rows = raw_offsets(text, &starts);
    // The text's end is no row: the line break that ends the last line is the walk's end. What
    // the enumerator adds for a text ending in LF (`0x5c250b`) is the end offset itself, even when
    // a wrap's whitespace skip ate that LF, and after a bail.
    rows.retain(|&at| at < text.len());
    if text.ends_with('\n') {
        rows.push(text.len());
    }
    if rows.is_empty() {
        rows.push(0);
    }
    (rows, r.size + r.gap)
}

/// The raw byte of each element index in `starts`: the cursor of the token it is, so a row that
/// opens on a glyph with codes before it starts past them, and one that opens on a code, on it.
/// One past the last token is the text's end.
fn raw_offsets(text: &str, starts: &[usize]) -> Vec<usize> {
    let at: Vec<usize> = benilla_ui::markup::tokens(text)
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .collect();
    starts.iter().map(|&c| at[c.min(at.len() - 1)]).collect()
}

#[cfg(test)]
mod row_start_tests {
    use super::*;

    #[test]
    fn a_plain_start_is_its_byte() {
        assert_eq!(raw_offsets("hello world", &[0, 6]), [0, 6]);
        assert_eq!(
            raw_offsets("h\u{e9}llo w", &[0, 6]),
            [0, 7],
            "an element index counts characters"
        );
    }

    /// A code is an element of its own: a start on the glyph after it is past the code, a start on
    /// the code is on it.
    #[test]
    fn a_start_past_a_code_is_past_its_bytes() {
        let text = "a|cffff0000b c";
        // a, the code, b, ' ', c.
        assert_eq!(raw_offsets(text, &[0, 1, 2, 4]), [0, 1, 11, 13]);
        assert_eq!(
            raw_offsets("a||b", &[0, 2]),
            [0, 3],
            "`||` draws one byte from two"
        );
    }

    #[test]
    fn a_start_at_the_end_is_the_texts_end() {
        assert_eq!(raw_offsets("ab\n", &[0, 3]), [0, 3]);
        assert_eq!(raw_offsets("", &[0]), [0]);
    }
}
