//! **The emit pass** — a laid-out string into [`UiQuad`]s.
//!
//! Its other half is [`measure`], which decides widths, wraps and fits. The two used to compute in
//! different spaces with a rescale between them; since decision 1342 the raster size is an exact
//! integer device-pixel size, so both walk the same per-character table and the pen here and the
//! sum there are the same arithmetic. See [`super::engine`] for why that matters more than it
//! sounds like it should.

use bevy::math::Rect;
use bevy::prelude::*;

use benilla_ui::script::{JustifyH, JustifyV, Outline};

use crate::ui_pass::{UiQuad, UvRect};

use super::engine::TextEngine;
use super::markup::{fontstring_lines, ColorRun};

mod measure;
mod overflow;
mod wrap;

/// The width law itself, for the differential test in [`super::engine`] that pins it against
/// shaping whole strings. Not a production seam — every caller goes through [`measure_text`].
#[cfg(test)]
pub(super) use measure::measure_line_width_for_test;
pub(crate) use measure::{
    ellipsize_to_fit, line_advances, line_origin, line_rows, measure_text, measure_wrapped_rows,
};

/// The smallest resolved rect width (px) treated as a **pinned wrap constraint**. A `FontString`
/// with no explicit `<Size>`/`setAllPoints`/opposing anchor resolves through [`crate::ui_text`]'s
/// caller to a near-zero-width rect (the single-point case — the engine's layout derives
/// `left == right` from the pinned center, see `benilla-ui`'s `layout` resolver); such a rect is
/// "no width constraint" and text lays out on one line from its justify point, never wrapped. Only
/// a rect wider than this engages word wrap. No shipped wrappable FontString is this narrow (the
/// narrowest, a merchant row's price field, is tens of px); the degenerate titles are ~0.
const WRAP_MIN_WIDTH: f32 = 1.0;

/// The smallest resolved rect height (px) treated as a **pinned height limit** — the vertical twin
/// of [`WRAP_MIN_WIDTH`]: a degenerate (single-point / pre-measure) rect resolves to ~zero height,
/// which is "no height constraint", never "a zero-line box". Only a taller rect engages the
/// line-stack stop ([`overflow::lines_allowed`]) and the ellipsis gate ([`ellipsize_to_fit`]).
const HEIGHT_LIMIT_MIN: f32 = 1.0;

/// **Which grid a text block's top may land on** — the layout input decision 2172 made explicit.
///
/// The client has one vertical anchor snap ([`snap_block_top`]) and applies it to every
/// `FontString`, because in the reference the interface *is* the framebuffer's pixel grid: the
/// snap and the rect cannot disagree. benilla has frames that live outside that grid — the
/// WorldFrame overlays (V-plates, chat bubbles, overhead names) that slide continuously over the
/// 3-D scene and take their own DEVICE-pixel seat law ([`crate::vplates::device_snap`],
/// 0188/1398). Snapping their text on the UI's coarser LOGICAL grid puts two quantizers on one
/// sliding object, and the text beats against the art it is drawn inside.
///
/// This used to be inferred from the rect: a degenerate (zero-height) rect skipped the snap, and
/// the overlay painters all happened to pass one. Decision 2148 gave the V-plate real FontString
/// regions with real rects, and the inference silently stopped selecting them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextSeat {
    /// The client's law: the block top takes [`snap_block_top`]. Every interface `FontString`.
    UiGrid,
    /// The block top is exactly where `rect` puts it; only the per-glyph device rounding
    /// downstream quantizes it, on the same grid the overlay's own art is snapped to.
    Exact,
}

/// A `FontString`'s two justification axes (`justifyH`/`justifyV`), bundled — they arrive together
/// from the region's paint and travel together into [`layout_text_quads`].
#[derive(Clone, Copy)]
pub(crate) struct Justify {
    pub h: JustifyH,
    pub v: JustifyV,
}

/// The resolved face + size a `FontString` draws with (from its font object / `SetFont`) — bundled
/// so [`layout_text_quads`] stays within one argument budget. `None`s fall back to Friz Quadrata at
/// [`super::DEFAULT_FONT_SIZE`].
#[derive(Clone, Copy)]
pub(crate) struct FontSpec<'a> {
    pub path: Option<&'a str>,
    /// The **logical** height, seam already folded in ([`super::drawn_px`]). The engine rounds it
    /// to whole device pixels ([`TextEngine::ppem`]) — there is no "between sizes" any more.
    pub height: Option<f32>,
    /// The font's TRUE glyph outline ([`Outline::None`] for the common case). It selects the
    /// composite cell variant (ring+fill in one texture) AND, for THICK only, the step law's extra
    /// `+1` (`GlyphStepBase` 0x5ca2b0 biases solely under the THICK flag), so [`measure_text`]
    /// must see it too: a THICK outline changes measured/wrapped width.
    pub outline: Outline,
    /// The write-on reveal (`SetAlphaGradient` — per-FontString paint state, like `outline`):
    /// [`layout_text_quads`] multiplies each glyph's alpha by the ramp at its character position.
    /// Riding the spec means the drop-shadow pass (`..spec`) inherits it for free — a revealing
    /// character's shadow fades with its fill. `None` = draw whole; [`measure_text`] ignores it
    /// (alpha never changes metrics).
    pub alpha_gradient: Option<(f32, f32)>,
}

/// The client's per-glyph **step law**: the rasterizer stores each glyph's step base as
/// `(FT_advance >> 6) + 1.0` — the FreeType advance floored to integer pixels **plus one** (the
/// glyph rasterizer 0x5d1120, `out[5]`); an outlined font biases it another
/// `+1.0` (`GlyphStepBase` 0x5ca2b0); `ComputeStep` 0x5ca2d0 consumes exactly that (plus
/// only-negative kerning scaled by the face's advance-adjust, then rounds). This extra tracking is
/// look-defining — it is why real client text reads wider/denser than the raw font metrics.
///
/// **Answered in PHYSICAL device pixels** (the ppem the face is actually rasterized at); the caller
/// divides back to logical **once**, at the end of the line — because that is where
/// the real client's law lives: `FT_advance >> 6` floors the device-pixel advance and the `+1`/`+1`
/// biases are whole *device* pixels. Flooring in logical px
/// instead (the pre-fix shortcut) over-tracks on any window where `dpi ≠ 1`: at retina a
/// 6.385px-logical (12.77px-physical) ARIALN digit steps `(⌊12.77⌋+2)/2 = 7.0px`, not the logical
/// `⌊6.385⌋+2 = 8.0px` — a full logical pixel of extra gap *per digit*, proportionally huge on small
/// outlined numbers (bag counts, money) and the cause of their "digits too wide apart" look.
///
/// Deliberate v1 simplification (module doc): kerning is dropped entirely (the client applies only
/// *negative* pair kerns and rounds the sum — at UI sizes the rounded contribution is almost always
/// 0).
fn client_step(raw_physical_advance: f32, step_extra: f32) -> f32 {
    raw_physical_advance.floor() + step_extra
}

/// The step-law bias for a font's outline flag: the base `+1` for everyone, and one more `+1` for
/// **THICK only** — `GlyphStepBase 0x5ca2b0` adds its extra pixel solely under the THICK font flag
/// (CORRECTING the earlier "any outline" reading this module shipped with: a NORMAL-outlined font
/// steps exactly like a plain one, its 1px ring riding the base `+1` tracking; mirroring the cell
/// pad onto the advance was the flagged trap). NumberFontNormal digits (money, counts, hotkeys) are
/// the visible beneficiaries: one less pixel of gap per glyph.
fn step_extra_of(outline: Outline) -> f32 {
    match outline {
        Outline::Thick => 2.0,
        _ => 1.0,
    }
}

/// The client's SINGLE vertical anchor snap (`0x5cdf70` @`0x5ce051`): the composed block top — rect
/// top plus the justify offset — rounds ONCE to the integer pixel grid; the per-line ladder and the
/// within-cell ascender are added as exact integers after it, never re-snapped. The client's round
/// runs in the y-UP frame (`round(viewH·anchor.y)`), so a half-pixel tie pushes the block UP on
/// screen — in y-down units that is round-half-DOWN, `ceil(y − 0.5)` (the money row's H=13/S=14
/// MIDDLE tie: offset −1, not 0). Applied in LOGICAL units: the byte law at the 768-tall design
/// resolution (the 0292 posture), so a retina window seats text exactly as the design-resolution
/// layout, scaled.
fn snap_block_top(y: f32) -> f32 {
    snap_block_top_law(y) + UI_SEAT_NUDGE
}

/// The pure byte law of the snap ([`snap_block_top`] minus the taste nudge) — kept separate so the
/// tests pin the verified law itself, independent of the dial.
fn snap_block_top_law(y: f32) -> f32 {
    (y - 0.5).ceil()
}

/// The director's seat nudge: every UI FontString's text block sits one px LOWER than the byte
/// law's row. A deliberate **taste deviation** (the 0104/0235 precedent: the
/// director's eye outranks the byte reading): the law is triple-confirmed, yet UI text
/// consistently reads high on the director's display; this is their call, applied at the one seat
/// every UI FontString shares ([`snap_block_top`] — world text's degenerate rects skip it, keeping
/// nameplates/combat text on their own approved seats). Rendering only — measures, wrap, and the
/// Lua metric echoes are untouched.
///
/// Public to the crate because a scissor that means to admit a block's ink has to know the seat
/// pushed it down — see the message-band clip in [`crate::ui_script`]'s text arm.
pub(crate) const UI_SEAT_NUDGE: f32 = 1.0;

/// The `SetAlphaGradient` ramp at one character position: opaque before `start`, a linear 1→0 fade
/// across the next `length` characters, invisible beyond (hard edge when `length` ≤ 0).
fn gradient_alpha(index: usize, start: f32, length: f32) -> f32 {
    let i = index as f32;
    if i < start {
        1.0
    } else if length > 0.0 && i < start + length {
        1.0 - (i - start) / length
    } else {
        0.0
    }
}

/// A clickable hyperlink span the glyph layout produced: the union rect (y-down screen px, this
/// layout's own space) of one link's glyphs on ONE laid-out line — a wrapped link yields one span
/// per line — plus the link payload and its full `|H…|h…|h` markup for
/// `OnHyperlinkClick(link, markup, button)`.
pub(crate) struct LinkSpan {
    pub(crate) rect: Rect,
    pub(crate) link: String,
    pub(crate) markup: String,
}

/// Lays out `text` within `rect` (screen px, **y-down** — the same space
/// [`crate::ui_script::extract::paint_script`] already flips frame rects into) and returns one
/// [`UiQuad`] per non-blank glyph, textured with the page its cell was packed into. `z_key` is
/// shared by every glyph — the owning `FontString` region's own [`crate::ui_pass`]-order key
/// (regions already sort after their frame and by draw layer/decl, so reusing it keeps text in the
/// client's total order with no extra bookkeeping; ties break by push order, i.e.
/// left-to-right/top-to-bottom, which is what a stable sort preserves). `base_color` is the region's
/// own color (`SetVertexColor`, default opaque white); `|cAARRGGBB`/`|r` runs in `text` override it
/// per the markup rules.
///
/// `justify_h` positions each line's runs horizontally *within* `rect`; `justify_v` positions the
/// line block vertically (default `Middle`, the client's FontString default; degenerate zero-height
/// rects keep top). Each line is measured once (a first pass laying glyphs at a zero line-origin),
/// then shifted by the justification offset.
pub(crate) fn layout_text_quads(
    e: &mut TextEngine,
    text: &str,
    rect: Rect,
    base_color: [f32; 4],
    justify: Justify,
    z_key: u64,
    font: FontSpec,
    seat: TextSeat,
) -> Vec<UiQuad> {
    layout_text_quads_inner(e, text, rect, base_color, justify, z_key, font, seat, None)
}

/// [`layout_text_quads`] that also collects the laid-out [`LinkSpan`]s — the message-frame path
/// (chat lines carry `|H` item/player links; the app feeds the spans back to the engine's click
/// hit-test, `benilla_ui::script::UiScript::set_link_spans`).
pub(crate) fn layout_text_quads_links(
    e: &mut TextEngine,
    text: &str,
    rect: Rect,
    base_color: [f32; 4],
    justify: Justify,
    z_key: u64,
    font: FontSpec,
    seat: TextSeat,
    links_out: &mut Vec<LinkSpan>,
) -> Vec<UiQuad> {
    layout_text_quads_inner(
        e,
        text,
        rect,
        base_color,
        justify,
        z_key,
        font,
        seat,
        Some(links_out),
    )
}

fn layout_text_quads_inner(
    e: &mut TextEngine,
    text: &str,
    rect: Rect,
    base_color: [f32; 4],
    justify: Justify,
    z_key: u64,
    font: FontSpec,
    seat: TextSeat,
    mut links_out: Option<&mut Vec<LinkSpan>>,
) -> Vec<UiQuad> {
    let gradient = font.alpha_gradient;
    let r = measure::resolve(e, &font);
    // **`AllGlyphsCached`** (`0x5c9fa0`): one pass to fill the caches, then the whole layout below
    // is lookups. Markup bytes are ensured along with everything else, which costs a few cells for
    // `|`/`c`/hex digits that a nearby run would have wanted anyway.
    e.ensure_str(r.face, r.ppem, r.radius, text);
    let e = &*e;
    let dpi = e.dpi();
    let sheet = e.sheet_image();
    // **The line rounds ONCE, and the glyphs are integer device offsets from it**.
    //
    // Glyph edges must land on device pixels — the same integer-device-pixel placement the real
    // client does — or the cell is resampled instead of blitted. Everything a glyph contributes to
    // its own position is *already* an integer count of device texels: swash's `bearing_x` /
    // `bearing_top`, the shaper's `y_off`, and the step law's `floor(advance) + extra`. So the pen
    // walks in DEVICE px, the line's origin is rounded once, and each glyph adds its integers to
    // that one rounded number.
    //
    // The obvious spelling — `snap(pen + per_glyph_offset)` with `snap(v) = (v·dpi).round()/dpi` —
    // is the same thing in exact arithmetic and NOT the same thing in `f32`, which is what B232
    // was. At a fractional scale factor (Windows 150 %, a fractional Wayland scale) `bearing/dpi`
    // is inexact, so a sum that is mathematically a half-pixel tie lands either side of the tie
    // depending on the glyph's own bearing: the tall letters round one way, the x-height letters
    // the other, and the word visibly steps. It fires only where the tie actually falls (~2 % of
    // seats at dpi 1.5, ~4 % at 1.75 — measured), which is why it read as "one UI scale is broken
    // and the next is fine" rather than as anything systematic. Rounding the ORIGIN instead of the
    // sum removes the tie from the per-glyph term entirely: a line cannot be anything but flat.
    // (The predecessor defect, 1342's: a rescale ran *after* the snap and multiplied each rounded
    // position by a non-integer factor, giving every letter its own sub-pixel phase.)

    // The render lays the same lines the measure counted, or a MIDDLE-justified block seats
    // against a height it does not have ([`fontstring_lines`]).
    let lines = fontstring_lines(text, base_color);
    // The THICK ink rise: the client's THICK quad shift (−2, `0x5cd0e1`) against its blit's baked
    // seat nets the INK **1 row above** the plain baseline law; NORMAL cancels exactly
    // (outline-invariant). Our composite cell reproduces the blit, so the quad carries the net:
    // THICK cells rise 1 logical px.
    let thick_rise = f32::from(u8::from(matches!(font.outline, Outline::Thick)));

    // Word-wrap within the rect when it carries a pinned width; an unsized single-point FontString
    // resolves to a ~zero-width rect (no constraint) and keeps its single-line behavior.
    //
    // The box is the box: `rect` is in the same space the pen lays out in, because there is only
    // one space. Under the size ladder this pass measured a drawn-px rect against snapped-size
    // glyphs unless it remembered to convert, and forgetting is what put the Main Menu's labels one
    // character short (1339 — "Options" → "Option", with no ellipsis to show for it).
    let mut render_lines: Vec<Vec<ColorRun>> = if rect.width() > WRAP_MIN_WIDTH {
        lines
            .iter()
            .flat_map(|line| measure::wrap_line(e, r, line, rect.width()))
            .collect()
    } else {
        lines
    };
    // The height-limit line stack (regime 2's vertical half, `CGxString+0x40` in `0x5cdc20`): a
    // height-pinned rect stops emitting wrapped lines when the accumulated pitch passes it. A
    // FontString the ellipsis seam ([`ellipsize_to_fit`]) already truncated fits by construction,
    // so this is the belt under any caller that skips that seam — in the client the same split: the
    // ui truncate feeds the gx string, and the gx line stack clamps regardless.
    if rect.height() > HEIGHT_LIMIT_MIN {
        render_lines.truncate(overflow::lines_allowed(rect.height(), r.size));
    }

    let mut quads = Vec::new();
    // `justifyV` positions the line *block* — N lines at the client's pitch, which IS the font
    // height (line-step law: `lineStep = px(size) + spacing`, spacing 0 for all shipped UI;
    // `LayoutLines` 0x5cdc20) — vertically within `rect`. The real FontString default is MIDDLE,
    // which is what seats the money digits on their coin icons' centerline. A rect sized *by* the
    // text (the host-measure round-trip's height-less FontStrings) has `height == block`, so the
    // offset degenerates to 0 there — only explicitly-sized rects shift. A degenerate rect (unsized
    // single-point anchor, pre-measure) keeps the v1 top placement rather than hoisting text above
    // its anchor.
    //
    // NO outline pad in the pitch — the byte law (`0x5cdc20`/`0x5cdf70`): the justify block is
    // `h = N·S`, and the `+2r` lives ONLY in the atlas cell (`[font+0x178]`), which the layout
    // never reads. (The earlier `+2r` here seated every outlined MIDDLE string r px too high — the
    // money-digit bug.)
    let pitch = r.size;
    let block_h = render_lines.len() as f32 * pitch;
    let v_offset = if rect.height() > f32::EPSILON {
        match justify.v {
            JustifyV::Top => 0.0,
            JustifyV::Middle => (rect.height() - block_h) * 0.5,
            JustifyV::Bottom => rect.height() - block_h,
        }
    } else {
        0.0
    };
    // Baseline of the first line: the row the client hangs ink from — the face's pixel ascender
    // `[CGxFont+0x17c] = round(size · asc/(asc+|desc|))`, threaded unchanged into the glyph
    // placement kernel 0x5d1360 as the operand that fixes `baseline = cellTop + ascender`. NOT
    // `asc/upem` ≈ 0.965 — that is the FreeType scaled hhea ascender, which appears nowhere in the
    // placement path; seating with it dropped every line ~3px too low.
    let ascent_ratio = e.ascent_ratio_of(r.face);
    let baseline_in_cell = (f64::from(r.size) * f64::from(ascent_ratio) + 0.5).floor() as f32;
    // The block top takes the client's ONE vertical snap ([`snap_block_top`]) — unless this block
    // seats outside the UI's pixel grid ([`TextSeat::Exact`], the WorldFrame overlays), or its rect
    // is degenerate (a single-point / pre-measure rect has no seat to snap to).
    let block_top = if seat == TextSeat::UiGrid && rect.height() > f32::EPSILON {
        snap_block_top(rect.min.y + v_offset)
    } else {
        rect.min.y + v_offset
    };
    let mut pen_y = block_top + baseline_in_cell;

    // One glyph laid at a line-relative x-origin of 0, pending the line's justification shift.
    // Both offsets are **device px, and integral** — the pen's own walk plus swash's bearings.
    struct PendingGlyph {
        /// Horizontal offset from the line's rounded origin.
        x_dev: f32,
        /// Vertical offset from the line's rounded baseline.
        y_dev: f32,
        uv: Rect,
        px_w: f32,
        px_h: f32,
        color: [f32; 4],
    }

    // The write-on gradient's character counter (`SetAlphaGradient`): a glyph's position is its char
    // index in the emitted run stream — markup codes stripped, wrap-carried whitespace counted where
    // it rides. That drifts from the source string by at most the newline count (the engine-side
    // return condition counts every source char), which only pads the tail of the reveal by a frame
    // or two — the ramp itself is exact per drawn character.
    let mut char_index: usize = 0;
    for line in &render_lines {
        // First pass: lay the line's glyphs at line-origin 0 and measure its total width.
        let mut pending: Vec<PendingGlyph> = Vec::new();
        // This line's hyperlink x-ranges (line-origin space), one entry per distinct link — a
        // link's runs are contiguous, so min/max of its runs' pen extents is its span.
        let mut line_links: Vec<(std::sync::Arc<super::markup::LinkInfo>, f32, f32)> = Vec::new();
        // The pen in DEVICE px: every step is `floor(advance) + extra`, an integer, so the walk
        // is exact and the logical width is one division at the end rather than a sum of quotients.
        let mut pen_dev = 0.0f32;
        for run in line {
            if run.text.is_empty() {
                continue;
            }
            let run_x0 = pen_dev / dpi;
            // The pen walks CHARACTERS, not a shaped buffer. The client's law has no kerning and no
            // neighbour term (`ComputeStep 0x5ca2d0`), so a run's glyph sequence is the
            // concatenation of its characters' — which is exactly what the cache holds, and exactly
            // what `measure_line_width` sums. The two are now the same walk over the same numbers
            // rather than two computations kept in agreement.
            for ch in run.text.chars() {
                let Some(cc) = e.char_cell(r.face, r.ppem, ch) else {
                    // No face shapes it: it draws nothing and steps nothing, which is also what
                    // the measure says. (Under the ladder these disagreed — measure returned 0 and
                    // the pen stepped the shaped advance. One walk, one answer.)
                    char_index += 1;
                    continue;
                };
                let mut color = run.color;
                if let Some((start, length)) = gradient {
                    color[3] *= gradient_alpha(char_index, start, length);
                }
                for g in &cc.glyphs {
                    if let Some(info) = e.cell(r.face, r.ppem, r.radius, g.glyph_id) {
                        pending.push(PendingGlyph {
                            x_dev: pen_dev + info.bearing_x,
                            y_dev: g.y_off - info.bearing_top,
                            uv: info.uv,
                            px_w: info.px_w / dpi,
                            px_h: info.px_h / dpi,
                            color,
                        });
                    }
                    pen_dev += client_step(g.advance, r.step_extra);
                }
                char_index += 1;
            }
            if let Some(info) = &run.link {
                match line_links
                    .iter_mut()
                    .find(|(a, _, _)| std::sync::Arc::ptr_eq(a, info))
                {
                    Some((_, _, x1)) => *x1 = pen_dev / dpi,
                    None => line_links.push((info.clone(), run_x0, pen_dev / dpi)),
                }
            }
        }

        // Second pass: shift the whole line by the justification offset and emit.
        let line_width = pen_dev / dpi;
        let origin_x = match justify.h {
            JustifyH::Left => rect.min.x,
            JustifyH::Center => rect.min.x + (rect.width() - line_width) * 0.5,
            JustifyH::Right => rect.max.x - line_width,
        };
        // The line's two rounded device anchors — computed once, shared by every glyph on it.
        // `thick_rise` is a LOGICAL px (the composite cell's net blit shift), so it rides into the
        // round with the baseline rather than being applied to each glyph after it.
        let origin_dev_x = (origin_x * dpi).round();
        let baseline_dev_y = ((pen_y - thick_rise) * dpi).round();
        for g in &pending {
            let gx = (origin_dev_x + g.x_dev) / dpi;
            let gy = (baseline_dev_y + g.y_dev) / dpi;
            quads.push(UiQuad {
                rect: Rect::new(gx, gy, gx + g.px_w, gy + g.px_h),
                z_key,
                texture: Some(sheet.clone()),
                // Glyph cells are always normalized (never mirrored); pass them straight through.
                uv: UvRect::from_rect(g.uv),
                color: g.color,
                ..default()
            });
        }
        // The line's hyperlink spans, shifted by the same justification origin the glyphs took
        // (hit rects cover the glyph cells: cell top = baseline − ascender, one font-height tall).
        if let Some(out) = links_out.as_deref_mut() {
            let cell_top = pen_y - baseline_in_cell;
            for (info, x0, x1) in line_links.drain(..) {
                out.push(LinkSpan {
                    rect: Rect::new(origin_x + x0, cell_top, origin_x + x1, cell_top + r.size),
                    link: info.link.clone(),
                    markup: info.markup.clone(),
                });
            }
        }
        pen_y += pitch;
    }

    quads
}

#[cfg(test)]
mod seat_tests {
    use super::*;

    /// The composed MIDDLE seat for one line: block top from box top, per the anchor snap
    /// `0x5cdf70` — `d = H − round_half_away((H+h)/2)` in the client's y-up frame, which
    /// [`snap_block_top`] reproduces in y-down over the exact offset.
    fn middle_d(box_h: f32, block_h: f32) -> f32 {
        snap_block_top_law((box_h - block_h) * 0.5)
    }

    #[test]
    fn middle_seat_matches_the_three_verified_cases() {
        // Case 1 — Friz 12 in a 12-tall box (the bag title): d = 0.
        assert_eq!(middle_d(12.0, 12.0), 0.0);
        // Case 2 — Friz 10 in a 10-tall box (unit-frame name): d = 0.
        assert_eq!(middle_d(10.0, 10.0), 0.0);
        // Case 3 — arialn 14 (NO outline pad in the block!) in a 13-tall box (the money row):
        // d = 13 − round(13.5) = −1 — the celltop one px ABOVE the box top, the S>H degenerate.
        assert_eq!(middle_d(13.0, 14.0), -1.0);
    }

    #[test]
    fn the_tie_rounds_up_on_screen() {
        // The client's one snap runs in the y-up frame (round-half-away), so a .5 tie pushes
        // the block UP on screen = down-rounds in y-down units.
        assert_eq!(snap_block_top_law(10.5), 10.0);
        assert_eq!(snap_block_top_law(-0.5), -1.0);
        // Non-ties round normally.
        assert_eq!(snap_block_top_law(10.4), 10.0);
        assert_eq!(snap_block_top_law(10.6), 11.0);
    }

    #[test]
    fn the_seat_is_the_law_plus_the_directors_nudge() {
        // The drawn seat = the byte law + the 1px taste nudge (director's call).
        assert_eq!(snap_block_top(10.5), snap_block_top_law(10.5) + 1.0);
        assert_eq!(snap_block_top(0.0), 1.0);
    }
}

#[cfg(test)]
mod gradient_tests {
    use super::*;

    #[test]
    fn gradient_ramp_is_opaque_then_linear_then_invisible() {
        // SetAlphaGradient(10, 30): chars 0..10 opaque, 10..40 ramp 1→0, 40+ invisible.
        assert_eq!(gradient_alpha(0, 10.0, 30.0), 1.0);
        assert_eq!(gradient_alpha(9, 10.0, 30.0), 1.0);
        assert_eq!(gradient_alpha(10, 10.0, 30.0), 1.0); // ramp start = leading edge, fully lit
        assert!((gradient_alpha(25, 10.0, 30.0) - 0.5).abs() < 1e-6);
        assert_eq!(gradient_alpha(40, 10.0, 30.0), 0.0);
        assert_eq!(gradient_alpha(1000, 10.0, 30.0), 0.0);
        // Degenerate length: a hard reveal edge, never a divide-by-zero.
        assert_eq!(gradient_alpha(5, 10.0, 0.0), 1.0);
        assert_eq!(gradient_alpha(15, 10.0, 0.0), 0.0);
    }
}

#[cfg(test)]
mod measure_fits_render {
    use super::*;
    use crate::ui_text::engine::test_engine;
    use crate::ui_text::markup::parse_markup;

    const FACE: &str = "Fonts\\FRIZQT__.TTF";

    /// Every label the Main Menu draws — the shipped strings, so a regression reads as the
    /// director's screenshot did (`Options` → `Option`, `Return to Game` → `Return to`).
    const LABELS: &[&str] = &[
        "Options",
        "Support",
        "Macros",
        "Logout",
        "Return to Game",
        "Exit Game",
        "Key Bindings",
        "Edit",
        "Main Menu",
    ];

    /// `ERA_WINDOW_SCALE` — the Game Menu and the Options window carry `SetScale(0.78)`, so their
    /// font heights land on nothing round. That was unrepresentable on a fixed size ladder BY
    /// CONSTRUCTION, and it is the case the Main Menu report came from.
    const ERA: f32 = 0.78;

    fn spec(h: f32) -> FontSpec<'static> {
        FontSpec {
            path: Some(FACE),
            height: Some(h),
            outline: Outline::None,
            alpha_gradient: None,
        }
    }

    /// **A string fits the width its own measure reported.** An auto-sized FontString has no width
    /// of its own: the engine's measure round-trip sizes its rect *from* `measure_text`, and the
    /// emit pass then re-wraps inside that rect. If the two ever answer in different spaces the
    /// string wraps inside its own box and the line stack drops the overflow row with no ellipsis
    /// — which is what shipped (1339). Here they are one walk over one table, so this holds at
    /// every size and DPI rather than at the ones somebody remembered to bake.
    #[test]
    fn a_string_fits_the_width_its_own_measure_reported() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // Sizes that a ladder could not carry: the era scale against each shipped menu height,
        // and both DPIs.
        for dpi in [1.0f32, 2.0] {
            e.set_dpi_for_test(dpi);
            for base in [10.0f32, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0] {
                let h = base * ERA;
                let s = spec(h);
                for label in LABELS {
                    let measured = measure_text(&mut e, label, None, s).0;
                    let r = measure::resolve(&mut e, &s);
                    e.ensure_metrics(r.face, r.ppem, label);
                    let runs = parse_markup(label, [1.0, 1.0, 1.0, 1.0]);
                    let line = runs.first().expect("one line");
                    let rows = measure::wrap_line(&e, r, line, measured);
                    assert_eq!(
                        rows.len(),
                        1,
                        "{label:?} wrapped inside its own measured width \
                         ({measured} px, height {h}, dpi {dpi})"
                    );
                }
            }
        }
    }

    /// **A capped quest-log row title ellipsizes on ONE line — it never wraps into the row.**
    ///
    /// Stock `QuestLogFrame.lua:196-203` shrinks a tagged row's title to `275 − 15 − tagWidth` so
    /// it cannot run under the right-flush `(Elite)`. Decision 1873 declined that cap on the
    /// reading that an explicit width is a **wrap** width here, so a long title would spill onto a
    /// second line inside the 16-unit row; 1944 then put the reference's own file on the chain and
    /// the cap went live with it. The reading was wrong about our own engine, and this pins why:
    /// the title FontString carries a DECLARED height (`<ButtonText>` `y="10"`), which arms both
    /// overflow regimes — the ellipsis gate (`boxW > 0 && boxH > 0`, `0x771ec0`;
    /// [`measure::ellipsize_to_fit`]) and the line stack ([`overflow::lines_allowed`]) — and each
    /// on its own holds the paint to one line. The
    /// reference does exactly this: its gate holds on the same declared height and its display
    /// string is `prefix + "..."`.
    ///
    /// The geometry is the reference's own: row `300×16`, title `GameFontNormal` (FRIZQT__ 12) in a
    /// `y="10"` ButtonText (`QuestLogFrame.xml:5-7, 92-103`).
    #[test]
    fn a_capped_quest_log_title_ellipsizes_on_one_line() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // GameFontNormal: FRIZQT__ at 12 (Fonts.xml l.70-75). The row's title box is 10 tall.
        let s = spec(12.0);
        let r = measure::resolve(&mut e, &s);
        const BOX_H: f32 = 10.0;
        // The longest tag `QuestInfo.dbc` ships is "World Event"; "(Dungeon)" is the longest one a
        // quest actually carries in bulk (411 quests, decision 1873's census). Either way the cap
        // is `275 - 15 - tagWidth`.
        for tag in ["(Elite)", "(Dungeon)", "(World Event)"] {
            let cap = 275.0 - 15.0 - measure_text(&mut e, tag, None, s).0;
            for title in [
                // A real 1.11 Dungeon-tagged quest, indented as the row indents it.
                "  The Left Piece of Lord Valthalak's Amulet",
                "  The Right Piece of Lord Valthalak's Amulet",
                "  Bring Me The Head of Nekrum Gutchewer!",
            ] {
                e.ensure_metrics(r.face, r.ppem, title);
                e.ensure_metrics(r.face, r.ppem, "...");
                // The cap bites: uncapped the title is one line, capped it needs more than one.
                assert_eq!(
                    measure::wrapped_rows_for_test(&e, r, title, 275.0),
                    1,
                    "{title:?} fits the uncapped row"
                );
                assert!(
                    measure::wrapped_rows_for_test(&e, r, title, cap) > 1,
                    "{title:?} must overflow the {cap}px cap for this test to mean anything"
                );
                // Regime 2, the line stack: a 10-tall box emits ONE line whatever the wrap says.
                assert_eq!(
                    overflow::lines_allowed(BOX_H, r.size),
                    1,
                    "the row's 10-tall title box stacks exactly one line"
                );
                // Regime 3, the ellipsis: the drawn string is a prefix + "..." that fits that line.
                let cap_rows = overflow::lines_fitting(BOX_H.max(r.size), r.size) + 1;
                let display = overflow::ellipsize_in_box(title, BOX_H, r.size, |candidate| {
                    measure::wrapped_rows_capped_for_test(&e, r, candidate, cap, cap_rows)
                })
                .unwrap_or_else(|| panic!("{title:?} at cap {cap} must ellipsize"));
                assert!(
                    display.ends_with("..."),
                    "the truncation marker is three dots, got {display:?}"
                );
                assert_eq!(
                    measure::wrapped_rows_for_test(&e, r, &display, cap),
                    1,
                    "{display:?} must fit the row's single line at cap {cap}"
                );
            }
        }
    }

    /// **A world-seated block follows its rect; a UI-seated one snaps to the grid.**
    ///
    /// The V-plate slides over the world on the DEVICE pixel grid ([`crate::vplates::device_snap`],
    /// 0188/1398) — half a logical pixel per step at 2×, which is why the border art glides. Its
    /// name and level are drawn *inside* it and have to be rigid to it. Under
    /// [`TextSeat::UiGrid`] the block top takes [`snap_block_top`], a WHOLE-logical-pixel
    /// quantizer, so the ink holds still for one step of the border and then jumps two: the text
    /// beats against the art at half the plate's step rate. That is the "level and name text look
    /// janky af walking up to a mob" the director reported the day after decision 2148 moved the
    /// plate's strings out of a painter that never snapped them (2172).
    ///
    /// The walk is the plate's own: half-pixel steps at dpi 2, the seat the driver actually hands
    /// the widget layer.
    #[test]
    fn a_world_seated_block_tracks_its_rect_and_a_ui_one_snaps() {
        let Some(mut e) = test_engine(2.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        let s = spec(13.0);
        let (mut exact, mut grid) = (Vec::new(), Vec::new());
        for step in 0..24 {
            #[allow(clippy::cast_precision_loss)]
            let top = 100.0 + step as f32 * 0.5;
            let rect = Rect::new(40.0, top, 140.0, top + 14.0);
            for (seat, out) in [(TextSeat::Exact, &mut exact), (TextSeat::UiGrid, &mut grid)] {
                let quads = layout_text_quads(
                    &mut e,
                    "Twilight Avenger",
                    rect,
                    [1.0; 4],
                    Justify {
                        h: JustifyH::Center,
                        v: JustifyV::Middle,
                    },
                    0,
                    s,
                    seat,
                );
                assert!(!quads.is_empty(), "the name drew");
                out.push(quads.iter().map(|q| q.rect.min.y).fold(f32::MAX, f32::min));
            }
        }
        // Rigid: every half-pixel step of the rect is a half-pixel step of the ink.
        for (i, w) in exact.windows(2).enumerate() {
            assert!(
                (w[1] - w[0] - 0.5).abs() < 1e-3,
                "world-seated step {i}: the rect moved 0.5px and the ink moved {}",
                w[1] - w[0]
            );
        }
        // The defect, stated so it cannot come back quietly: the UI grid quantizes this same walk
        // to whole pixels — some steps move nothing and others move a full pixel, none move 0.5.
        assert!(
            grid.windows(2).any(|w| w[1] - w[0] > 0.9),
            "the UI grid must actually quantize, else this test proves nothing: {grid:?}"
        );
        assert!(
            grid.windows(2).all(|w| (w[1] - w[0] - 0.5).abs() > 1e-3),
            "a UI-grid seat never lands between two logical pixels: {grid:?}"
        );
    }

    /// **One line is ONE baseline — at every DPI, size and seat.**
    ///
    /// The emit pass's whole claim is that a line is flat *by construction*: one
    /// `pen_y`, an integer physical `bearing_top` per glyph, so every letter rounds by the same
    /// residual. That is true in exact arithmetic and it was NOT true in `f32` — `snap(pen_y −
    /// bt/dpi)` rounds a per-glyph sum, and at a **fractional** scale factor (Windows 150 %,
    /// a fractional Wayland/KDE scale) `bt/dpi` is inexact, so a value that is mathematically a
    /// half-pixel tie lands either side of it depending on the glyph's own bearing. B232's
    /// "the letters are not vertically aligned" is that tie, one device pixel wide, and it can
    /// only be seen at the sizes and seats where the tie actually falls — which is why one UI
    /// scale showed it and the next did not.
    ///
    /// The invariant is read off the quads: FRIZQT's hinted raster puts **every** cell bottom
    /// exactly on the baseline (`px_h == bearing_top` for every glyph at every ppem 8..64 —
    /// measured, engine sweep), so one baseline means one `rect.max.y`.
    #[test]
    fn one_line_is_one_baseline_at_every_dpi() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // Integer DPIs are the ones this machine can capture; the fractional ones are what the
        // reporters run, and they are exactly where the tie lives.
        for dpi in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            e.set_dpi_for_test(dpi);
            for base in [10.0f32, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0] {
                let s = spec(base * ERA);
                // A quarter-pixel walk over the seat: a frame at 90 % UI scale seats its text at
                // every fractional offset there is.
                for step in 0..320 {
                    let top = step as f32 * 0.25;
                    let rect = Rect::new(17.5, top, 217.5, top + 20.0);
                    let quads = layout_text_quads(
                        &mut e,
                        "Combat",
                        rect,
                        [1.0; 4],
                        Justify {
                            h: JustifyH::Left,
                            v: JustifyV::Middle,
                        },
                        0,
                        s,
                        TextSeat::UiGrid,
                    );
                    let lo = quads
                        .iter()
                        .map(|q| q.rect.max.y)
                        .fold(f32::INFINITY, f32::min);
                    let hi = quads
                        .iter()
                        .map(|q| q.rect.max.y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    assert!(
                        (hi - lo) * dpi < 1e-3,
                        "dpi {dpi}, height {base}, seat {top}: the letters of \"Combat\" \
                         straddle {:.2} device px of baseline",
                        (hi - lo) * dpi
                    );
                    // The same law horizontally: a glyph's left edge is the line's one rounded
                    // origin plus integers, so it is on the device grid too (a letter off it is
                    // resampled — the "uneven stems" half of the same report).
                    for q in &quads {
                        for (axis, dev) in
                            [("top", q.rect.min.y * dpi), ("left", q.rect.min.x * dpi)]
                        {
                            assert!(
                                (dev - dev.round()).abs() < 1e-3,
                                "dpi {dpi}, height {base}, seat {top}: a glyph {axis} landed at \
                                 {dev} device px — off the device grid"
                            );
                        }
                    }
                }
            }
        }
    }

    /// **The pen and the measure are the same number, to the bit.** The emit pass walks characters
    /// and steps; `measure_line_width` sums the same per-character steps. Nothing rounds between
    /// them, so this is equality, not a tolerance — and any future divergence (a re-introduced
    /// rescale, a shaper creeping back into one side) fails here rather than on someone's screen.
    #[test]
    fn the_drawn_line_is_exactly_as_wide_as_the_measure_says() {
        let Some(mut e) = test_engine(2.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        for label in LABELS {
            let s = spec(16.0 * ERA);
            let want = measure_text(&mut e, label, None, s).0 - 1.0; // less the headroom pixel
                                                                     // The emit pass's own line width, read off the quads it produced: the ink of the last
                                                                     // glyph never passes the pen, and the first starts at the origin.
            let quads = layout_text_quads(
                &mut e,
                label,
                Rect::new(0.0, 0.0, 0.0, 0.0),
                [1.0; 4],
                Justify {
                    h: JustifyH::Left,
                    v: JustifyV::Top,
                },
                0,
                s,
                TextSeat::UiGrid,
            );
            let ink = quads.iter().map(|q| q.rect.max.x).fold(0.0f32, f32::max);
            assert!(
                ink <= want.ceil() + 1e-3,
                "{label:?}: ink reaches {ink} but the measure promised {want}"
            );
            assert!(!quads.is_empty(), "{label:?} drew nothing");
        }
    }

    /// **The letters sit on one line.** Every glyph of a single-line string shares a baseline, so
    /// every quad's top edge is an exact device pixel and the whole row rounds by one residual.
    /// This is the director's report in an assertion: under the size ladder the rescale ran after
    /// the snap and multiplied each already-rounded position by a non-integer factor, giving every
    /// letter its own sub-pixel phase.
    #[test]
    fn every_glyph_on_a_line_lands_on_the_device_pixel_grid() {
        let Some(mut e) = test_engine(2.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // Deliberately an awkward height (12.48 logical at dpi 2 → 25 device px) and a fractional
        // rect origin, which is what a `SetScale`d frame actually resolves to.
        let s = spec(16.0 * ERA);
        let quads = layout_text_quads(
            &mut e,
            "Return to Game",
            Rect::new(10.5, 20.0, 400.0, 40.0),
            [1.0; 4],
            Justify {
                h: JustifyH::Left,
                v: JustifyV::Middle,
            },
            0,
            s,
            TextSeat::UiGrid,
        );
        assert!(quads.len() > 8, "the string drew");
        let dpi = 2.0f32;
        for q in &quads {
            for v in [q.rect.min.x, q.rect.min.y, q.rect.max.x, q.rect.max.y] {
                let device = v * dpi;
                assert!(
                    (device - device.round()).abs() < 1e-3,
                    "a glyph edge at {v} logical is {device} device px — off the grid, which is \
                     the sub-pixel scatter that took the letters off their line"
                );
            }
        }
    }
}

#[cfg(test)]
mod ellipsis_cost {
    use super::*;
    use crate::ui_text::engine::test_engine;
    use std::cell::Cell;
    use std::time::Instant;

    /// The reported page verbatim — vmangos `page_text` 2676, the *Alliance Military Ranks* plaque
    /// in Stormwind's Old Town (`GameObject` 3011). 647 bytes.
    const PAGE: &str = concat!(
        "<HTML>\n",
        "<BODY>\n",
        "<H1 align=\"center\">ALLIANCE MILITARY RANKS</H1><BR/>\n",
        "<P align=\"center\">OFFICERS</P><BR/>\n",
        "<P align=\"center\">Grand Marshal</P>\n",
        "<P align=\"center\">Field Marshal</P>\n",
        "<P align=\"center\">Marshal</P>\n",
        "<P align=\"center\">Commander</P>\n",
        "<P align=\"center\">Lieutenant Commander</P>\n",
        "<P align=\"center\">Knight-Champion</P>\n",
        "<P align=\"center\">Knight-Captain</P>\n",
        "<P align=\"center\">Knight-Lieutenant</P>\n",
        "<P align=\"center\">Knight</P><BR/>\n",
        "<P align=\"center\">ENLISTED</P><BR/>\n",
        "<P align=\"center\">Sergeant Major</P>\n",
        "<P align=\"center\">Master Sergeant</P>\n",
        "<P align=\"center\">Sergeant</P>\n",
        "<P align=\"center\">Corporal</P>\n",
        "<P align=\"center\">Private</P>\n",
        "</BODY>\n",
        "</HTML>",
    );

    /// The longest body vmangos actually ships (`page_text` 2880, a Hearthglen letter, 928 bytes)
    /// — **plain prose, no markup at all**. It is here because HTML is the loudest case, not
    /// the boundary: the seam is armed by OVERFLOW, and the longest
    /// pages in the world are plain. `$b` arrives expanded (`npc_text::substitute`, the feed).
    const PLAIN: &str = concat!(
        "Reuben,\n\nI write this letter knowing you may never see it; I simply can't remain idle, ",
        "listening to the constant pounding against the Hearthglen walls. The undead are outside ",
        "our village, unceasing in their assault, and we have been charged with defending the ",
        "townsfolk until reinforcements arrive.\n\nMy leg was broken in the last charge, and so I ",
        "sit, useless, with my sword at my side should there be a breach in our defenses. There is ",
        "no idle banter... only the sounds of fighting and death. The air is thick with fear.\n\n",
        "Prince Arthas is here, fighting on the front lines with the men. Were he not present we ",
        "would have fallen long ago. His love for this land and its people is infectious; I gladly ",
        "serve under him, and will to the end of my days.\n\nThe fighting grows more intense; ",
        "broken leg or not, I cannot sit here. Every sword is needed.  I hope these words find you ",
        "in happier times.\n\nYour friend,\nLeagrem\n\n",
    );

    /// The reader's own wrapper — `ItemTextFrame.xml`'s READY handler frames an authorless page
    /// with a leading and a trailing newline.
    fn page_body(page: &str) -> String {
        format!("\n{page}\n")
    }

    /// `ItemTextPageText`'s geometry and face: 270×304 at `ItemTextFontNormal`
    /// (`Fonts\MORPHEUS.TTF`, height 15). At the 768-tall design window the seam scale is 1, so
    /// these are also the drawn px.
    const FACE: &str = "Fonts\\MORPHEUS.TTF";
    const SIZE: f32 = 15.0;
    const BOX_W: f32 = 270.0;
    const BOX_H: f32 = 304.0;

    #[test]
    #[ignore = "rasterizes a real font from the install; run explicitly"]
    fn a_page_that_overflows_its_box_costs_a_whole_back_off() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        let s = FontSpec {
            path: Some(FACE),
            height: Some(SIZE),
            outline: Outline::None,
            alpha_gradient: None,
        };
        let r = measure::resolve(&mut e, &s);
        let fits = overflow::lines_fitting(BOX_H, r.size);

        for (label, page) in [("html 2676", PAGE), ("plain 2880", PLAIN)] {
            let text = page_body(page);
            e.ensure_metrics(r.face, r.ppem, &text);
            e.ensure_metrics(r.face, r.ppem, "...");
            let full = measure::wrapped_rows_for_test(&e, r, &text, BOX_W);
            eprintln!(
                "[ellipsis-cost] {label:<11} {:>4} chars -> {full:>2} rows in a {fits}-row box",
                text.chars().count(),
            );

            // Uncapped (what shipped) against the client's box-bounded fit walk. Both must return
            // the same display string — the cap decides overflow, it does not decide the answer.
            let mut out = Vec::new();
            for (how, cap) in [("whole string", usize::MAX), ("box-bounded", fits + 1)] {
                let probes = Cell::new(0usize);
                let chars = Cell::new(0usize);
                let t = Instant::now();
                let got = overflow::ellipsize_in_box(&text, BOX_H, r.size, |candidate| {
                    probes.set(probes.get() + 1);
                    chars.set(chars.get() + candidate.chars().count());
                    measure::wrapped_rows_capped_for_test(&e, r, candidate, BOX_W, cap)
                });
                let us = t.elapsed().as_secs_f64() * 1e6;
                eprintln!(
                    "[ellipsis-cost]   {how:<13} {:>4} probes, {:>6} candidate chars, {us:>7.0} us",
                    probes.get(),
                    chars.get(),
                );
                assert!(
                    got.is_some(),
                    "{label} overflows its box — the seam is armed"
                );
                out.push(got);
            }
            assert_eq!(
                out[0], out[1],
                "{label}: the row cap changed the display string"
            );
        }
    }
}
