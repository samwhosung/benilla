//! The Text-arm rasterization: one `QuadContent::Text` quad — a region FontString, a
//! message-frame ring line, or the focused editbox's windowed text — into glyph [`UiQuad`]s:
//! the editbox window + selection/caret, the ellipsis-truncate seam, the drop shadow, and the
//! hyperlink span collection. Split out of the extraction pass ([`super`]) when it crossed the
//! size budget; the arm's host-loop context arrives bundled as [`TextHost`].

use bevy::prelude::*;

use benilla_ui::order::ZTarget;
use benilla_ui::script::{EditBoxTextUi, FontShadow, JustifyH, JustifyV, Outline};
use benilla_ui::widget::FrameHandle;

use crate::ui_pass::{UiQuad, UvRect};
use crate::ui_text::{layout_text_quads, layout_text_quads_links, TextSeat, UiFontAtlas};

/// `WOW_TEXT_PROBE=1` — launch-time knob, read once (the check ran per Text quad per frame).
static TEXT_PROBE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WOW_TEXT_PROBE").as_deref() == Ok("1"));

/// The `QuadContent::Text` payload minus the text itself — the region's resolved style, passed
/// verbatim from the destructure at the [`super::paint_script`] call site.
pub(super) struct TextStyle {
    pub color: Option<[f32; 4]>,
    pub justify_h: JustifyH,
    pub justify_v: JustifyV,
    pub font: Option<String>,
    pub font_height: Option<f32>,
    pub text_height: Option<f32>,
    pub shadow: Option<FontShadow>,
    pub outline: Outline,
    pub alpha_gradient: Option<(f32, f32)>,
    /// The region's seat claim (`benilla_ui`'s `RegionData::world_seat`) — true for the V-plate's
    /// name and level, which are rigid to a device-snapped overlay rather than to the UI grid.
    pub world_seat: bool,
}

/// The host-loop context one Text quad draws under: the extracted quad's identity (z/alpha/
/// target), its y-flipped rect + clip, the focused editbox's text-UI geometry (matched to this
/// quad inside), and the screen height for the link-span flip back to engine space.
pub(super) struct TextHost<'a> {
    pub z: u64,
    pub alpha: f32,
    pub target: ZTarget,
    pub rect: Rect,
    pub clip: Option<Rect>,
    /// The focused editbox's text-UI geometry, unfiltered — [`emit`] matches it to this quad
    /// by [`EditBoxTextUi::target`].
    pub ebox: Option<&'a EditBoxTextUi>,
    pub screen_h: f32,
    /// The 768-virtual scale `s = windowH/768` (decision 0582): rects arrive pre-scaled (px);
    /// this converts the remaining unit-space inputs — font heights, shadow offsets, the
    /// engine's editbox caret/selection x-offsets — into the same px space.
    pub scale: f32,
    /// The owning frame's `effective_scale` ([`benilla_ui::script::ExtractedQuad::scale`]): the
    /// rect already carries it; the FONT metrics — glyph raster size and shadow offset, both
    /// frame-local — multiply by it here (the real client's text rides `SetScale`; 0219 §2's
    /// divergence, closed). NOT applied to the editbox caret/selection/advance x-offsets: those
    /// arrive in screen UI units (the advance measure already rode the scale — see
    /// [`benilla_ui::script::EditBoxAdvanceRequest`]'s `scale` doc).
    pub font_scale: f32,
    /// Captures pin the caret blink ON (deterministic pixels); live, the engine's phase decides.
    pub caret_pinned: bool,
}

/// Rasterize one Text quad into `out` (and its hyperlink spans into `link_spans`, message-frame
/// lines only). The glyph quads share the owning region's `z` — see [`layout_text_quads`]'s doc
/// for why that's already the correct total-order slot.
pub(super) fn emit(
    atlas: &mut UiFontAtlas,
    text: &str,
    style: TextStyle,
    host: TextHost<'_>,
    out: &mut Vec<UiQuad>,
    link_spans: &mut Vec<(FrameHandle, benilla_ui::layout::Rect, String, String)>,
) {
    let base_color = style.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
    // Which pixel grid this block's top may land on (decision 2172): the interface's, or none —
    // the WorldFrame overlays carry their own device-pixel seat and their text has to be rigid
    // to it. The shadow pass below inherits it, like every other layout input.
    let seat = if style.world_seat {
        TextSeat::Exact
    } else {
        TextSeat::UiGrid
    };
    let spec = crate::ui_text::FontSpec {
        path: style.font.as_deref(),
        // The drawn px under the two size regimes × the 768-virtual scale × the owner's frame
        // scale (`drawn_px`, decision 0582): one-to-one text unit-caps at 32 (a frame-LOCAL cap,
        // like the seam scale it precedes) then scales; a SetTextHeight override scales uncapped.
        // The shadow twin below inherits the spec (`..spec`).
        height: crate::ui_text::drawn_px(
            style.font_height,
            style.text_height,
            host.scale * host.font_scale,
        ),
        outline: style.outline,
        alpha_gradient: style.alpha_gradient,
    };
    // The focused edit box's text draws WINDOWED (`0x77da80`): the scroll window's substring,
    // left-anchored at the line origin in an unbounded rect (the window never wraps — the box
    // edge clips instead), the selection highlight behind everything, the white caret after.
    // `line_origin` gives the origin + the line cell; the x-offsets inside it are the engine's
    // advance-derived geometry.
    let ebox = host.ebox.filter(|u| u.target == host.target);
    let mut draw_text: &str = text;
    let mut draw_rect = host.rect;
    let mut draw_justify = crate::ui_text::Justify {
        h: style.justify_h,
        v: style.justify_v,
    };
    let mut text_clip = host.clip;
    // The shadow's whole-pixel displacement ([`shadow_offset_px`]), hoisted: the band scissor
    // below has to admit the ink it puts under the fill.
    let shadow_delta = style.shadow.map(|sh| {
        (
            shadow_offset_px(sh.offset[0] * host.scale * host.font_scale),
            shadow_offset_px(-sh.offset[1] * host.scale * host.font_scale),
        )
    });
    // A message-frame ring line's scissor follows the seat, at the bottom edge only.
    //
    // The band ladder stacks UP from the frame's bottom edge, so the newest line is flush with it
    // by construction and can never overflow there — the scissor's real job is the OTHER end, the
    // half-fitting scrollback line at the frame's top, and that edge is untouched. What the bottom
    // edge did cut was ink the renderer had deliberately placed below the band: the seat nudge
    // (0351) lowers every UI text block one px, and the drop shadow sits one further px under the
    // fill. Two of our own dials against a scissor that predates them — and the newest chat line
    // lost the tails of its descenders and the feet of its brackets (director, 2026-07-26).
    if matches!(host.target, ZTarget::Frame(_)) {
        if let Some(c) = text_clip.as_mut() {
            c.max.y += band_clip_slack(shadow_delta.map(|(_, dy)| dy));
        }
    }
    let mut ebox_geom = None;
    if let Some(ui) = ebox {
        let drawn = &text[ui.display_from.min(text.len())..];
        let (x0, top, cell_h) =
            crate::ui_text::line_origin(&mut atlas.lock(), drawn, host.rect, draw_justify, spec);
        ebox_geom = Some((x0, top, cell_h));
        text_clip = Some(host.clip.map_or(host.rect, |c| c.intersect(host.rect)));
        if !ui.multi_line {
            // Single-line: the windowed draw — the scroll window's substring, left-anchored in
            // an unbounded rect (the box edge clips instead of wrapping).
            draw_text = drawn;
            draw_rect = Rect::new(x0, host.rect.min.y, x0 + 100_000.0, host.rect.max.y);
            draw_justify = crate::ui_text::Justify {
                h: JustifyH::Left,
                v: style.justify_v,
            };
        }
        // Multiline draws the whole block through the ordinary wrapped path below (the text
        // region's justify is already the editbox law: TOP/LEFT) — only the caret/selection are
        // seated by `(row, x)` at the same row pitch the wrap answered.
        for &(row, sx0, sx1) in &ui.selection {
            let hc = ui.highlight_color;
            #[allow(clippy::cast_precision_loss)]
            let ry = top + row as f32 * cell_h;
            // The engine's selection x-span is in UI units (its advance table was fed ÷scale).
            out.push(UiQuad {
                rect: Rect::new(
                    x0 + sx0 * host.scale,
                    ry,
                    x0 + sx1 * host.scale,
                    ry + cell_h,
                ),
                z_key: host.z,
                texture: None,
                uv: UvRect::FULL,
                color: [hc[0], hc[1], hc[2], hc[3] * host.alpha],
                additive: false,
                circular: false,
                desaturated: false,
                premultiplied: false,
                gamma_texel: false,
                alpha_test: None,
                uv_clamp: None,
                clip: text_clip,
                rotation: 0.0,
                mask: None,
                corners: None,
            });
        }
    }
    // The height-gated ellipsis-truncate (`ellipsize_to_fit` — CSimpleFontString `0x771ec0`,
    // decision 0292's residue landed): a region FontString whose wrapped text needs more lines
    // than its height-pinned rect allows draws `prefix + "..."` instead — the bag title, the
    // unit-frame names, the minimap zone text. Region FontStrings only, exactly the client's
    // seam: the editbox windows (never truncates), message-frame ring lines size their band FROM
    // the text, and auto-height FontStrings fit by construction (the measure round-trip), so the
    // gate inside is geometric and needs no fixed-vs-measured flag. Computed before the shadow
    // so the shadow pass draws the same display string (the client's shadow is a second draw of
    // the same truncated CGxString).
    let ellipsized = match host.target {
        ZTarget::Region(region) if ebox.is_none() => {
            crate::ui_text::ellipsize_to_fit(atlas, region, draw_text, draw_rect, spec)
        }
        _ => None,
    };
    if let Some(display) = ellipsized.as_deref() {
        draw_text = display;
    }
    // The probe prints the DISPLAY string (post-ellipsis, post-editbox-window) — what this pass
    // actually draws, which is what a truncation report needs to show.
    let probe = *TEXT_PROBE;
    if probe {
        // For the focused edit box, also the two numbers that must agree: where the engine puts the
        // caret (`caret=`, advance-table-derived) and how wide the text this pass actually draws is
        // (`ink=`). They diverge by the width of the markup when the advance table is measured over
        // the raw buffer — the caret-out-in-space report (decision 1075) as a pair of numbers.
        let ebox_geom = ebox.map(|ui| {
            let ink = crate::ui_text::measure_text(&mut atlas.lock(), draw_text, None, spec).0;
            format!(" caret={:.1} ink={ink:.1}", ui.caret_x * host.scale)
        });
        // **The FACE and the FLAGS are on this line for a reason.** It used to print the
        // requested height and nothing else about the font, so the one question a "the text
        // looks wrong" report actually asks — *which face, at what size, with what outline, did
        // this quad draw* — could not be answered from the probe at all; it took a live
        // `SetFont` probe, a control plate and a screenshot to establish for MSBT what these
        // four fields say directly (decisions 2103, 2112). `px` is the DRAWN logical height
        // (`drawn_px`: the requested one through the cap, the 768 seam and the frame scale),
        // which is the number that disagrees with `h` whenever a size looks wrong.
        info!(
            "text probe: [{:.0},{:.0} {:.0}x{:.0}] h={:?} px={:?} flags={:?} face={:?}{} {:?}",
            draw_rect.min.x,
            draw_rect.min.y,
            host.rect.width(),
            host.rect.height(),
            style.font_height,
            spec.height,
            style.outline.as_str(),
            spec.path.unwrap_or("<none — the fallback face>"),
            ebox_geom.unwrap_or_default(),
            &draw_text[..draw_text.len().min(60)]
        );
    }
    // The drop shadow (font object `<Shadow>` — MasterFont's (1,-1) black covers the whole
    // GameFont* family): the same layout at an offset rect in the shadow color, pushed FIRST so
    // the stable z-sort keeps it behind the glyph pass. Offset is WoW y-up (`y="-1"` = down) →
    // y-down screen dy = −y. Markup color codes inside the text tint the shadow run too (v1
    // corner, invisible for solid-color strings). The shadow is a single flat offset copy —
    // never itself outlined — it lays out identically to its fill (a shadow with different steps
    // would smear under long strings) and redraws the same composite cells in the shadow color,
    // where the ring's black is indistinguishable from the shadow's.
    if let (Some(sh), Some((dx, dy))) = (style.shadow, shadow_delta) {
        // WHOLE pixels (`shadow_offset_px`, computed above): the shadow is a rigid copy of the
        // fill, so its displacement must be an integer in the rect's own space — see that fn for
        // why a fractional one makes the offset itself wobble line to line.
        let srect = Rect::new(
            draw_rect.min.x + dx,
            draw_rect.min.y + dy,
            draw_rect.max.x + dx,
            draw_rect.max.y + dy,
        );
        let shadow_spec = crate::ui_text::FontSpec { ..spec };
        let mut sq = layout_text_quads(
            &mut atlas.lock(),
            draw_text,
            srect,
            sh.color,
            draw_justify,
            host.z,
            shadow_spec,
            seat,
        );
        for q in &mut sq {
            // Flatten rgb to the shadow color (markup tints ride the fill only); the alpha is
            // every scalar the fill rides plus the shadow's own — see [`shadow_alpha`], which
            // names the set and the symptom a missing member wears.
            q.color = [
                sh.color[0],
                sh.color[1],
                sh.color[2],
                shadow_alpha(q.color[3], base_color[3], host.alpha),
            ];
            // Every glyph quad inherits the owning Text quad's ScrollFrame clip — the
            // FontString's own extract-time clip, not a per-glyph concept.
            q.clip = text_clip;
        }
        out.extend(sq);
    }
    // `font`/`font_height` come from the region's resolved font object (`Fonts.xml`).
    let mut glyphs = if let ZTarget::Frame(fh) = host.target {
        // A frame-targeted Text quad is a message-frame ring line: collect its hyperlink spans
        // for the engine's click hit-test (y-down → y-up flip).
        let mut spans = Vec::new();
        let g = layout_text_quads_links(
            &mut atlas.lock(),
            text,
            host.rect,
            base_color,
            crate::ui_text::Justify {
                h: style.justify_h,
                v: style.justify_v,
            },
            host.z,
            spec,
            seat,
            &mut spans,
        );
        for sp in spans {
            // px → the engine's y-up UI-unit space (÷scale after the flip).
            link_spans.push((
                fh,
                benilla_ui::layout::Rect::new(
                    (host.screen_h - sp.rect.max.y) / host.scale,
                    sp.rect.min.x / host.scale,
                    (host.screen_h - sp.rect.min.y) / host.scale,
                    sp.rect.max.x / host.scale,
                ),
                sp.link,
                sp.markup,
            ));
        }
        g
    } else {
        layout_text_quads(
            &mut atlas.lock(),
            draw_text,
            draw_rect,
            base_color,
            draw_justify,
            host.z,
            spec,
            seat,
        )
    };
    for q in &mut glyphs {
        q.color[3] *= host.alpha;
        // Every glyph quad inherits the Text quad's ScrollFrame clip (decision 0112) —
        // `ui_pass`'s CPU clip already applies uniformly to any `UiQuad`, glyph or not.
        q.clip = text_clip;
    }
    // The probe's seat line: the drawn INK rows (glyph-quad union, logical px, relative to the
    // rect top) — the measurable half of the vertical-seat law
    // (`0x5d1360`): compare `ink` against the law's `d + ascender` seat
    // when hunting a vertical offset. Fill quads only (the shadow pass above would smear the
    // bounds one px down-right).
    let vpl = style.world_seat && benilla_assets::trace::enabled_for("vpl");
    if (probe || vpl) && !glyphs.is_empty() {
        let (mut y0, mut y1) = (f32::MAX, f32::MIN);
        for q in &glyphs {
            y0 = y0.min(q.rect.min.y);
            y1 = y1.max(q.rect.max.y);
        }
        // **Where the plate's text actually inked**, on the same `vpl` tag the driver's `plate=`
        // seat and the border's `paint=` line ride ([`crate::vplates`], decision 2168's
        // measurement). The question the three lines answer together is not "does the text
        // move?" — it is "does it move WITH the border?", which is a difference of two numbers
        // per frame. Decision 2168 §5 named this line as the missing third; 2172 is what it
        // found when it was built.
        if vpl {
            benilla_assets::trace::line(
                "vpl",
                &format!(
                    "text x={:.3} rect={:.3} ink={:.3} rel={:.3} txt={}",
                    host.rect.min.x,
                    host.rect.min.y,
                    y0,
                    y0 - host.rect.min.y,
                    draw_text.chars().take(16).collect::<String>()
                ),
            );
        }
        if probe {
            info!(
                "seat probe: top={:.2} ink=[{:.2}..{:.2}] (rel {:.2}..{:.2}) {:?}",
                host.rect.min.y,
                y0,
                y1,
                y0 - host.rect.min.y,
                y1 - host.rect.min.y,
                draw_text.chars().take(20).collect::<String>()
            );
        }
    }
    out.extend(glyphs);
    // The focused box's caret: a 1-px WHITE bar (the client's ctor `0xffffffff` caret texture —
    // never the text color) one line cell tall at the engine's advance-derived x, pushed after
    // the glyphs so the stable sort draws it on top.
    if let (Some(ui), Some((x0, top, cell_h))) = (ebox, ebox_geom) {
        if host.caret_pinned || ui.caret_on {
            #[allow(clippy::cast_precision_loss)]
            let top = top + ui.caret_row as f32 * cell_h;
            // caret_x is engine UI units (÷scale advances); the 1-px bar width stays device-thin
            // (the client's 4-unit caret, `0x77b8c0`, is a named residual).
            let cx = x0 + ui.caret_x * host.scale;
            out.push(UiQuad {
                rect: Rect::new(cx, top, cx + 1.0, top + cell_h),
                z_key: host.z,
                texture: None,
                uv: UvRect::FULL,
                color: [1.0, 1.0, 1.0, host.alpha],
                additive: false,
                circular: false,
                desaturated: false,
                premultiplied: false,
                gamma_texel: false,
                alpha_test: None,
                uv_clamp: None,
                clip: text_clip,
                rotation: 0.0,
                mask: None,
                corners: None,
            });
        }
    }
}

/// One axis of the drop shadow's displacement, in the rect's own px space: the font object's
/// unit offset times the seam scale, **rounded to a whole pixel** (never to zero — a shadow the
/// scale shrank below half a pixel still draws one, the reference's look at any window size).
///
/// It must be a whole pixel because the shadow is laid out as a second, offset copy of the fill,
/// and each copy independently takes the client's single vertical anchor snap (`snap_block_top`
/// — `ceil(y − 0.5)`). A snap commutes with an integer translate and *only* with an integer
/// translate: at a fractional offset `d`, `ceil(y + d − 0.5) − ceil(y − 0.5)` is `⌈d⌉` for some
/// fractional parts of `y` and `⌊d⌋` for others — so the shadow's distance from its own glyphs
/// changed with where the string happened to land. The chat window made that visible and
/// permanent: its newest line's band is pinned to the frame's bottom edge, one fixed fractional
/// position, which sat inside the wide-by-one window — so the most recent message, and only it,
/// wore a shadow a pixel further out than every line above it (director, 2026-07-26; measured at
/// 1600×900 ×0.9 UI scale, seam scale 1.055 → the fill/shadow gap alternated 1 px and 2 px).
///
/// The world's floating combat text reached the same conclusion from its own law
/// (`combat_text::law::shadow_offset_px` rounds `0.002·viewport`); this is the UI's half.
/// How far below its band a message-frame line's ink can reach, in the rect's own px space — the
/// slack its scissor's bottom edge has to give back.
///
/// Two renderer offsets put ink under the geometric band, and neither existed when the band model
/// was written: the seat nudge ([`crate::ui_text::UI_SEAT_NUDGE`]) drops every UI text block one
/// px, and the drop shadow ([`shadow_offset_px`]) draws a copy one further px down. A shadow that
/// rises (`dy < 0`) adds nothing at the bottom.
fn band_clip_slack(shadow_dy: Option<f32>) -> f32 {
    crate::ui_text::UI_SEAT_NUDGE + shadow_dy.map_or(0.0, |dy| dy.max(0.0))
}

fn shadow_offset_px(scaled: f32) -> f32 {
    let rounded = scaled.round();
    if rounded == 0.0 && scaled != 0.0 {
        scaled.signum()
    } else {
        rounded
    }
}

/// A shadow glyph's alpha: **every scalar the fill rides, plus its own.**
///
/// Named because the set is the whole law and a missing member is invisible until something fades
/// through the one that was dropped. There are three:
///
/// - `layout` — what the shadow pass laid out with: the font object's `<Shadow>` colour alpha,
///   times the write-on gradient's per-glyph ramp.
/// - `fill` — the FILL colour's alpha (`base_color[3]`): `SetTextColor(r,g,b,a)`, and every
///   MessageFrame line, whose fade is a colour ramp rather than a frame-alpha one.
/// - `host` — the region's effective frame alpha (`SetAlpha` and every parent's).
///
/// `fill` was the one missing. A frame that fades through its colour (UIErrorsFrame,
/// RaidWarningFrame, chat) holds `host` at 1.0 throughout, so the shadow stayed a fully opaque
/// black copy of a vanishing string: the text read as turning BLACK across the ramp and then
/// popping out when the line retired, instead of fading.
fn shadow_alpha(layout: f32, fill: f32, host: f32) -> f32 {
    layout * fill * host
}

#[cfg(test)]
mod tests {
    use super::{band_clip_slack, shadow_alpha, shadow_offset_px};

    /// A drop shadow fades with the string it belongs to — through EITHER fade lane.
    ///
    /// The regression this pins: `UIErrorsFrame`'s "Requires Mining" and every other MessageFrame
    /// line ramps its COLOUR alpha and holds frame alpha at 1.0, so a shadow that only rode
    /// `host` stayed opaque black under a vanishing red fill — the text turned black across the
    /// ramp and then vanished in one frame (director, 2026-08-22). Anything that fades through
    /// `SetTextColor`'s alpha wore the same defect.
    #[test]
    fn a_drop_shadow_fades_through_the_colour_lane_as_well_as_the_frame_lane() {
        // Nothing fading: an opaque shadow, as before.
        assert_eq!(shadow_alpha(1.0, 1.0, 1.0), 1.0);
        // The colour lane alone (a message-frame line mid-ramp): the shadow follows it. This is
        // the assertion that fails without the fix — it used to answer 1.0.
        assert_eq!(shadow_alpha(1.0, 0.25, 1.0), 0.25);
        // The frame lane alone (UIFrameFadeOut, the zone splash): unchanged behaviour.
        assert_eq!(shadow_alpha(1.0, 1.0, 0.25), 0.25);
        // Both lanes compose, and the font object's own semi-transparent shadow rides on top.
        assert_eq!(shadow_alpha(0.8, 0.5, 0.5), 0.2);
        // Fully faded fill leaves NO shadow behind — the black that used to outlive the text.
        assert_eq!(shadow_alpha(1.0, 0.0, 1.0), 0.0);
    }

    /// The newest chat line's band bottom IS the frame's bottom edge, so every px the renderer
    /// adds below the band is a px the scissor would otherwise eat — the descender tails and
    /// bracket feet the director saw sliced off. The slack is the sum of the two offsets that put
    /// ink there, and nothing else.
    #[test]
    fn the_band_scissor_gives_back_exactly_the_seat_and_the_shadow() {
        // The shipped GameFont case: seat nudge 1 + a 1px drop shadow.
        assert_eq!(band_clip_slack(Some(1.0)), 2.0);
        // A font object with no shadow still owes the seat.
        assert_eq!(band_clip_slack(None), 1.0);
        // A shadow that rises adds nothing under the line.
        assert_eq!(band_clip_slack(Some(-1.0)), 1.0);
        // A bigger window scales the shadow, and the slack with it.
        assert_eq!(band_clip_slack(Some(2.0)), 3.0);
    }

    /// The whole-pixel law, at the three scales that matter: the 1:1 seam (offset 1 px), the
    /// director's 1600×900 ×0.9 (1.055 → 1 px, the fix's own case), and a small capture window
    /// whose scale would otherwise round the shadow away entirely.
    #[test]
    fn a_shadow_offset_is_a_whole_pixel_and_never_vanishes() {
        assert_eq!(shadow_offset_px(1.0), 1.0);
        assert_eq!(shadow_offset_px(1.0546875), 1.0);
        assert_eq!(shadow_offset_px(-1.0546875), -1.0);
        assert_eq!(shadow_offset_px(2.109375), 2.0);
        // 377-tall capture window × 0.9: 0.44 px would round to nothing.
        assert_eq!(shadow_offset_px(0.4418), 1.0);
        assert_eq!(shadow_offset_px(-0.4418), -1.0);
        // A font object with no offset on an axis keeps none.
        assert_eq!(shadow_offset_px(0.0), 0.0);
    }

    /// The property the fix exists for: an integer translate commutes with the client's single
    /// vertical snap (`ceil(y − 0.5)`), so the fill→shadow gap is the same at every fractional
    /// position a string can land on. A fractional offset does not — the sweep below is exactly
    /// the wobble the chat window's newest line wore.
    #[test]
    fn a_whole_pixel_offset_survives_the_anchor_snap_at_every_fraction() {
        let snap = |y: f32| (y - 0.5).ceil();
        let d = shadow_offset_px(1.0546875);
        for step in 0..1000 {
            let y = 100.0 + step as f32 / 1000.0;
            assert_eq!(
                snap(y + d) - snap(y),
                d,
                "whole-px offset must translate the snapped block exactly, y={y}"
            );
        }
        // …and the unrounded offset provably does not (both gaps occur over one unit of y).
        let raw = 1.0546875f32;
        let gaps: std::collections::BTreeSet<i32> = (0..1000)
            .map(|step| {
                let y = 100.0 + step as f32 / 1000.0;
                (snap(y + raw) - snap(y)) as i32
            })
            .collect();
        assert_eq!(gaps, [1, 2].into_iter().collect());
    }
}
