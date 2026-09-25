//! The emit pass: a laid-out string into [`UiQuad`]s. [`measure`] decides widths, wraps and fits
//! by the same per-character steps at one raster size, so the pen and its sum agree to the bit.

use bevy::math::Rect;
use bevy::prelude::*;

use benilla_ui::script::{JustifyH, JustifyV, Outline};

use crate::ui_pass::{UiQuad, UvRect};

use super::engine::TextEngine;
use super::markup::{fontstring_lines, ColorRun};

mod measure;
mod overflow;
mod wrap;

/// The width law, for the engine's differential test against shaped strings.
#[cfg(test)]
pub(super) use measure::measure_line_width_for_test;
pub(crate) use measure::{
    ellipsize_to_fit, line_advances, line_origin, line_rows, measure_text, measure_wrapped_rows,
};

/// A rect this narrow is no wrap constraint: an unsized single-point `FontString` resolves to ~0.
const WRAP_MIN_WIDTH: f32 = 1.0;

/// A rect this short is no height limit, never a zero-line box: a single-point rect is ~0 tall.
const HEIGHT_LIMIT_MIN: f32 = 1.0;

/// Which grid a text block's top lands on. The reference snaps every `FontString` to the UI grid
/// ([`snap_block_top`]); benilla's WorldFrame overlays (V-plates, chat bubbles, overhead names)
/// move on the device grid ([`crate::vplates::device_snap`]), where that snap jitters their text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextSeat {
    /// The block top takes [`snap_block_top`]: every interface `FontString`.
    UiGrid,
    /// The block top is exactly where `rect` puts it; only each glyph's device rounding moves it.
    Exact,
}

/// A `FontString`'s `justifyH` and `justifyV`.
#[derive(Clone, Copy)]
pub(crate) struct Justify {
    pub h: JustifyH,
    pub v: JustifyV,
}

/// The face and size a `FontString` draws with; `None`s fall back to Friz Quadrata at
/// [`super::DEFAULT_FONT_SIZE`].
#[derive(Clone, Copy)]
pub(crate) struct FontSpec<'a> {
    pub path: Option<&'a str>,
    /// The logical height, seam already folded in ([`super::drawn_px`]); the engine rounds it to
    /// whole device pixels ([`TextEngine::ppem`]).
    pub height: Option<f32>,
    /// The glyph outline: it picks the ring-and-fill cell and, for THICK only, the step law's
    /// extra `+1` (`GlyphStepBase` `0x5ca2b0`), so it changes measured width too.
    pub outline: Outline,
    /// The `SetAlphaGradient` reveal `(start, length)`; on the spec, shadows fade with their fill.
    pub alpha_gradient: Option<(f32, f32)>,
}

/// The reference's per-glyph step in device px: the FreeType advance floored plus one (glyph
/// rasterizer `0x5d1120`, `out[5]`), one more for THICK (`GlyphStepBase` `0x5ca2b0`), both in
/// `step_extra`. Divided by the DPI once per line: flooring logical px instead over-tracks.
///
/// Deviation: `ComputeStep`'s (`0x5ca2d0`) negative-only pair kern is dropped, because at UI sizes
/// it rounds to 0 almost always.
fn client_step(raw_physical_advance: f32, step_extra: f32) -> f32 {
    raw_physical_advance.floor() + step_extra
}

/// [`client_step`]'s bias: `+1`, or `+2` for THICK, as `GlyphStepBase` (`0x5ca2b0`) tests the THICK
/// flag alone; a NORMAL outline steps like a plain font, though its cell grows.
fn step_extra_of(outline: Outline) -> f32 {
    match outline {
        Outline::Thick => 2.0,
        _ => 1.0,
    }
}

/// The block top a UI `FontString` takes: the reference's snap plus [`UI_SEAT_NUDGE`].
fn snap_block_top(y: f32) -> f32 {
    snap_block_top_law(y) + UI_SEAT_NUDGE
}

/// The reference's one vertical anchor snap (`0x5cdf70` at `0x5ce051`): the block top rounds once,
/// and the line ladder and ascender add as integers after it. Rounding in y-up, a half-pixel tie
/// moves the block up: `ceil(y - 0.5)` in y-down, in logical units on the 768-tall design grid.
fn snap_block_top_law(y: f32) -> f32 {
    (y - 0.5).ceil()
}

/// Deviation: every UI text block sits 1 px below the reference's row, because at that row UI text
/// reads high. It moves the drawn ink only: measure, wrap and the Lua metrics never see it, and the
/// message-band scissor in [`crate::ui_script`] widens by it to admit the ink.
pub(crate) const UI_SEAT_NUDGE: f32 = 1.0;

/// `SetAlphaGradient`'s alpha at a character: opaque before `start`, linear to 0 over `length`.
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

/// One hyperlink's hit rect on one laid-out line (a wrapped link yields one per line), with the
/// payload and full `|H…|h…|h` markup `OnHyperlinkClick(link, markup, button)` receives.
pub(crate) struct LinkSpan {
    pub(crate) rect: Rect,
    pub(crate) link: String,
    pub(crate) markup: String,
}

/// Lays out `text` in `rect` (screen px, y-down), one [`UiQuad`] per non-blank glyph, all on the
/// region's `z_key` so push order breaks ties; `|c`/`|r` runs override `base_color`.
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

/// [`layout_text_quads`], also collecting the [`LinkSpan`]s a message frame's link hit test reads.
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
    // One pass fills the caches, as `AllGlyphsCached` (`0x5c9fa0`); the layout below is lookups.
    e.ensure_str(r.face, r.ppem, r.radius, text);
    let e = &*e;
    let dpi = e.dpi();
    let sheet = e.sheet_image();
    // The pen walks device px, the line's origin rounds once, and each glyph adds integers to it,
    // so every edge lands on a device pixel; rounding each glyph's own `f32` sum instead splits
    // half-pixel ties by bearing at a fractional DPI, and a word's letters step.

    // The same lines the measure counted ([`fontstring_lines`]), or a MIDDLE block seats wrong.
    let lines = fontstring_lines(text, base_color);
    // THICK ink sits 1 logical px above the plain baseline: the reference's THICK quad shift (-2,
    // `0x5cd0e1`) against its blit's baked seat; NORMAL cancels exactly.
    let thick_rise = f32::from(u8::from(matches!(font.outline, Outline::Thick)));

    // Wrap only within a pinned width; `rect` is in the pen's own space, so no conversion.
    let mut render_lines: Vec<Vec<ColorRun>> = if rect.width() > WRAP_MIN_WIDTH {
        lines
            .iter()
            .flat_map(|line| measure::wrap_line(e, r, line, rect.width()))
            .collect()
    } else {
        lines
    };
    // The line stack (`CGxString+0x40` in `0x5cdc20`): a height-pinned rect stops emitting lines
    // once the pitch sum passes it, even for a caller that skips the ellipsis seam.
    if rect.height() > HEIGHT_LIMIT_MIN {
        render_lines.truncate(overflow::lines_allowed(rect.height(), r.size));
    }

    let mut quads = Vec::new();
    // `justifyV` seats the block in `rect` at a pitch of the font height (`LayoutLines` `0x5cdc20`:
    // `px(size) + spacing`, spacing 0 in shipped UI), so `h = N·S`: the outline's `+2r` is only in
    // the atlas cell (`[font+0x178]`), never the block. A degenerate rect keeps the top.
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
    // The first baseline hangs at the face's pixel ascender, `[CGxFont+0x17c] = round(size ·
    // asc/(asc+|desc|))`, as the placement kernel `0x5d1360` sets `baseline = cellTop + ascender`;
    // the FreeType hhea `asc/upem` is not on that path and seats text ~3 px low.
    let ascent_ratio = e.ascent_ratio_of(r.face);
    let baseline_in_cell = (f64::from(r.size) * f64::from(ascent_ratio) + 0.5).floor() as f32;
    let block_top = if seat == TextSeat::UiGrid && rect.height() > f32::EPSILON {
        snap_block_top(rect.min.y + v_offset)
    } else {
        rect.min.y + v_offset
    };
    let mut pen_y = block_top + baseline_in_cell;

    // A glyph laid at line origin 0, pending the justify shift; its offsets are whole device px.
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

    // `SetAlphaGradient`'s character index counts drawn characters, markup stripped; it trails the
    // engine's source count by at most the newline count, a frame or two on the reveal's tail.
    let mut char_index: usize = 0;
    for line in &render_lines {
        // First pass: lay the line at origin 0 and measure its width.
        let mut pending: Vec<PendingGlyph> = Vec::new();
        // The line's link x-ranges, one per link: a link's runs are contiguous.
        let mut line_links: Vec<(std::sync::Arc<super::markup::LinkInfo>, f32, f32)> = Vec::new();
        // The pen in device px: every step is an integer, so the logical width is one division.
        let mut pen_dev = 0.0f32;
        for run in line {
            if run.text.is_empty() {
                continue;
            }
            let run_x0 = pen_dev / dpi;
            // The pen walks characters, not a shaped buffer: with the kern dropped
            // ([`client_step`]) a run is its characters' steps, the sum `measure_line_width` takes.
            for ch in run.text.chars() {
                let Some(cc) = e.char_cell(r.face, r.ppem, ch) else {
                    // No face shapes it: it draws and steps nothing, as the measure says.
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

        // Second pass: shift the line by the justify offset and emit.
        let line_width = pen_dev / dpi;
        let origin_x = match justify.h {
            JustifyH::Left => rect.min.x,
            JustifyH::Center => rect.min.x + (rect.width() - line_width) * 0.5,
            JustifyH::Right => rect.max.x - line_width,
        };
        // The line's rounded device anchors; `thick_rise` (logical px) rounds with the baseline.
        let origin_dev_x = (origin_x * dpi).round();
        let baseline_dev_y = ((pen_y - thick_rise) * dpi).round();
        for g in &pending {
            let gx = (origin_dev_x + g.x_dev) / dpi;
            let gy = (baseline_dev_y + g.y_dev) / dpi;
            quads.push(UiQuad {
                rect: Rect::new(gx, gy, gx + g.px_w, gy + g.px_h),
                z_key,
                texture: Some(sheet.clone()),
                uv: UvRect::from_rect(g.uv),
                color: g.color,
                ..default()
            });
        }
        // Link hit rects cover the glyph cells: one font height down from baseline minus ascender.
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

    /// One line's MIDDLE seat from the box top, `d = H - round_half_away((H+h)/2)` in y-up
    /// (`0x5cdf70`).
    fn middle_d(box_h: f32, block_h: f32) -> f32 {
        snap_block_top_law((box_h - block_h) * 0.5)
    }

    #[test]
    fn middle_seat_matches_the_three_verified_cases() {
        // Friz 12 in the 12-tall bag title.
        assert_eq!(middle_d(12.0, 12.0), 0.0);
        // Friz 10 in the 10-tall unit-frame name.
        assert_eq!(middle_d(10.0, 10.0), 0.0);
        // Arial Narrow 14 in the 13-tall money row, no outline pad: the cell top 1 px above it.
        assert_eq!(middle_d(13.0, 14.0), -1.0);
    }

    #[test]
    fn the_tie_rounds_up_on_screen() {
        // The reference rounds half away in y-up, so a .5 tie moves the block up: down in y-down.
        assert_eq!(snap_block_top_law(10.5), 10.0);
        assert_eq!(snap_block_top_law(-0.5), -1.0);
        assert_eq!(snap_block_top_law(10.4), 10.0);
        assert_eq!(snap_block_top_law(10.6), 11.0);
    }

    #[test]
    fn the_seat_is_the_law_plus_the_directors_nudge() {
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
        assert_eq!(gradient_alpha(10, 10.0, 30.0), 1.0);
        assert!((gradient_alpha(25, 10.0, 30.0) - 0.5).abs() < 1e-6);
        assert_eq!(gradient_alpha(40, 10.0, 30.0), 0.0);
        assert_eq!(gradient_alpha(1000, 10.0, 30.0), 0.0);
        // Zero length: a hard reveal edge, never a divide by zero.
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

    /// The game menu's labels, where a measure/draw mismatch clips `Options` to `Option`.
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

    /// `ERA_WINDOW_SCALE` of benilla's game menu and options window: fonts land on no whole size.
    const ERA: f32 = 0.78;

    fn spec(h: f32) -> FontSpec<'static> {
        FontSpec {
            path: Some(FACE),
            height: Some(h),
            outline: Outline::None,
            alpha_gradient: None,
        }
    }

    /// An auto-sized rect comes from `measure_text` and the draw re-wraps inside it, so a string
    /// must fit its own measured width on one line.
    #[test]
    fn a_string_fits_the_width_its_own_measure_reported() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // The 0.78 scale against each menu height, at both DPIs.
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

    /// A tagged quest log title capped to `275 - 15 - tagWidth` (`QuestLogFrame.lua:197-205`)
    /// ellipsizes on one line: the 300x16 row's 10-tall ButtonText
    /// (`QuestLogFrame.xml:5-7, 92-103`) arms both the ellipsis gate (`boxW > 0 && boxH > 0`,
    /// `0x771ec0`) and the line stack, and the reference draws `prefix + "..."`.
    #[test]
    fn a_capped_quest_log_title_ellipsizes_on_one_line() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // `GameFontNormal` is FRIZQT__ at 12 (`Fonts.xml:70-75`); the title box is 10 tall.
        let s = spec(12.0);
        let r = measure::resolve(&mut e, &s);
        const BOX_H: f32 = 10.0;
        // Tags up to the longest `QuestInfo.dbc` ships, "World Event".
        for tag in ["(Elite)", "(Dungeon)", "(World Event)"] {
            let cap = 275.0 - 15.0 - measure_text(&mut e, tag, None, s).0;
            for title in [
                // Long titles, indented as the row indents them.
                "  The Left Piece of Lord Valthalak's Amulet",
                "  The Right Piece of Lord Valthalak's Amulet",
                "  Bring Me The Head of Nekrum Gutchewer!",
            ] {
                e.ensure_metrics(r.face, r.ppem, title);
                e.ensure_metrics(r.face, r.ppem, "...");
                // The cap bites: uncapped the title is one line, capped it needs more.
                assert_eq!(
                    measure::wrapped_rows_for_test(&e, r, title, 275.0),
                    1,
                    "{title:?} fits the uncapped row"
                );
                assert!(
                    measure::wrapped_rows_for_test(&e, r, title, cap) > 1,
                    "{title:?} must overflow the {cap}px cap for this test to mean anything"
                );
                // The line stack: a 10-tall box emits one line whatever the wrap says.
                assert_eq!(
                    overflow::lines_allowed(BOX_H, r.size),
                    1,
                    "the row's 10-tall title box stacks exactly one line"
                );
                // The ellipsis: the drawn string is a prefix plus "..." that fits that line.
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

    /// The V-plate moves on the device grid ([`crate::vplates::device_snap`]), half a logical px
    /// per step at dpi 2, and its name must move with it; under [`TextSeat::UiGrid`] the
    /// whole-pixel snap would hold the ink for one step and jump it the next.
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
        // The UI grid quantizes the same walk to whole pixels: no step moves 0.5.
        assert!(
            grid.windows(2).any(|w| w[1] - w[0] > 0.9),
            "the UI grid must actually quantize, else this test proves nothing: {grid:?}"
        );
        assert!(
            grid.windows(2).all(|w| (w[1] - w[0] - 0.5).abs() > 1e-3),
            "a UI-grid seat never lands between two logical pixels: {grid:?}"
        );
    }

    /// One line is one baseline at every DPI, size and seat, where rounding each glyph's own sum
    /// splits half-pixel ties by bearing at a fractional DPI. FRIZQT's hinted cells all end on the
    /// baseline (`px_h == bearing_top` at every ppem 8..64), so one baseline is one `rect.max.y`.
    #[test]
    fn one_line_is_one_baseline_at_every_dpi() {
        let Some(mut e) = test_engine(1.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        for dpi in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            e.set_dpi_for_test(dpi);
            for base in [10.0f32, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0] {
                let s = spec(base * ERA);
                // A quarter-pixel walk, as a scaled frame's text takes every offset.
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
                    // Horizontally too: a left edge off the device grid is resampled.
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

    /// The pen and `measure_line_width` sum the same steps, so the ink never passes the measure.
    #[test]
    fn the_drawn_line_is_exactly_as_wide_as_the_measure_says() {
        let Some(mut e) = test_engine(2.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        for label in LABELS {
            let s = spec(16.0 * ERA);
            let want = measure_text(&mut e, label, None, s).0 - 1.0; // less the headroom pixel
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

    /// Every glyph edge of a single-line string lands on the device grid: no per-letter phase.
    #[test]
    fn every_glyph_on_a_line_lands_on_the_device_pixel_grid() {
        let Some(mut e) = test_engine(2.0) else {
            eprintln!("skipping: no install / font chain");
            return;
        };
        // An awkward height (12.48 logical, 25 device px at dpi 2) and a fractional origin, as a
        // `SetScale`d frame resolves.
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

    /// vmangos `page_text` 2676 verbatim, the Alliance Military Ranks plaque in Stormwind's Old
    /// Town (`GameObject` 3011): HTML, 647 bytes.
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

    /// The longest body vmangos ships (`page_text` 2880, a Hearthglen letter, 928 bytes), plain
    /// prose as the longest pages are; `$b` arrives expanded (`npc_text::substitute`).
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

    /// The reader's wrapper: `ItemTextFrame.lua:44` frames an authorless page with a leading and a
    /// trailing newline.
    fn page_body(page: &str) -> String {
        format!("\n{page}\n")
    }

    /// `ItemTextPageText`: 270×304 at `ItemTextFontNormal` (`Fonts\MORPHEUS.TTF`, height 15); at
    /// the 768-tall design window these are drawn px.
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

            // Uncapped against the reference's box-bounded fit walk: the cap decides overflow,
            // never the display string.
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
