//! The Text arm: one `QuadContent::Text` quad (a region FontString, a message-frame line or the
//! focused editbox's text) into glyph [`UiQuad`]s, with the editbox's selection and caret, the
//! ellipsis, the drop shadow and the hyperlink spans.

use bevy::prelude::*;

use benilla_ui::order::ZTarget;
use benilla_ui::script::{EditBoxTextUi, FontShadow, JustifyH, JustifyV, Outline};
use benilla_ui::widget::FrameHandle;

use crate::ui_pass::{UiQuad, UvRect};
use crate::ui_text::{layout_text_quads, layout_text_quads_links, TextSeat, UiFontAtlas};

/// `WOW_TEXT_PROBE=1`: log each Text quad's drawn string, font and ink rows; read once.
static TEXT_PROBE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WOW_TEXT_PROBE").as_deref() == Ok("1"));

/// The `QuadContent::Text` payload minus the text: the region's resolved style.
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
    /// The V-plate's name and level, seated on the plate's device-snapped overlay, not the UI grid.
    pub world_seat: bool,
}

/// The extraction loop's context for one Text quad.
pub(super) struct TextHost<'a> {
    pub z: u64,
    pub alpha: f32,
    pub target: ZTarget,
    pub rect: Rect,
    pub clip: Option<Rect>,
    /// The focused editbox's text UI, unfiltered: [`emit`] matches it to this quad by `target`.
    pub ebox: Option<&'a EditBoxTextUi>,
    pub screen_h: f32,
    /// The seam scale `windowH/768 × uiScale`, for the unit-space inputs; rects arrive in px.
    pub scale: f32,
    /// The frame's effective scale, already in the rect: glyph size and shadow offset ride it, as
    /// the 1.12 client's text rides `SetScale`; the editbox x-offsets arrive in screen UI units.
    pub font_scale: f32,
    /// Captures pin the caret on for deterministic pixels; live, the engine's blink decides.
    pub caret_pinned: bool,
}

/// Rasterize one Text quad into `out`, and a message-frame line's hyperlink spans into
/// `link_spans`. Glyph quads take the region's `z` ([`layout_text_quads`]).
pub(super) fn emit(
    atlas: &mut UiFontAtlas,
    text: &str,
    style: TextStyle,
    host: TextHost<'_>,
    out: &mut Vec<UiQuad>,
    link_spans: &mut Vec<(FrameHandle, benilla_ui::layout::Rect, String, String)>,
) {
    let base_color = style.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
    // The block's top snaps to the UI grid, except on a world overlay, which has its own seat.
    let seat = if style.world_seat {
        TextSeat::Exact
    } else {
        TextSeat::UiGrid
    };
    let spec = crate::ui_text::FontSpec {
        path: style.font.as_deref(),
        // One-to-one text caps at 32 frame units, then scales; a `SetTextHeight` size is uncapped.
        height: crate::ui_text::drawn_px(
            style.font_height,
            style.text_height,
            host.scale * host.font_scale,
        ),
        outline: style.outline,
        alpha_gradient: style.alpha_gradient,
    };
    // The focused editbox draws windowed (`0x77da80`): the scroll window's substring from the
    // line origin, unwrapped and clipped at the box edge, the selection behind, the caret after.
    let ebox = host.ebox.filter(|u| u.target == host.target);
    let mut draw_text: &str = text;
    let mut draw_rect = host.rect;
    let mut draw_justify = crate::ui_text::Justify {
        h: style.justify_h,
        v: style.justify_v,
    };
    let mut text_clip = host.clip;
    // Hoisted: the band scissor below admits the ink the shadow puts under the fill.
    let shadow_delta = style.shadow.map(|sh| {
        (
            shadow_offset_px(sh.offset[0] * host.scale * host.font_scale),
            shadow_offset_px(-sh.offset[1] * host.scale * host.font_scale),
        )
    });
    // A message line's scissor gives back the ink the seat nudge and the shadow put below its
    // band, at the bottom edge only: the top edge still clips a half-fitting scrollback line.
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
            draw_text = drawn;
            draw_rect = Rect::new(x0, host.rect.min.y, x0 + 100_000.0, host.rect.max.y);
            draw_justify = crate::ui_text::Justify {
                h: JustifyH::Left,
                v: style.justify_v,
            };
        }
        // A multiline box draws wrapped (its region is TOP/LEFT already); the caret and the
        // selection seat by `(row, x)` at the wrap's row pitch.
        for &(row, sx0, sx1) in &ui.selection {
            let hc = ui.highlight_color;
            #[allow(clippy::cast_precision_loss)]
            let ry = top + row as f32 * cell_h;
            // The engine's selection x-span is in UI units.
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
    // A region FontString whose wrapped text needs more lines than its rect holds draws
    // `prefix + "..."` (`CSimpleFontString` `0x771ec0`); an editbox never truncates and a message
    // line sizes to its text. Before the shadow, which the client draws from the same string.
    let ellipsized = match host.target {
        ZTarget::Region(region) if ebox.is_none() => {
            crate::ui_text::ellipsize_to_fit(atlas, region, draw_text, draw_rect, spec)
        }
        _ => None,
    };
    if let Some(display) = ellipsized.as_deref() {
        draw_text = display;
    }
    // The probe prints the displayed string, after the ellipsis and the editbox window.
    let probe = *TEXT_PROBE;
    if probe {
        // For the focused editbox, the engine's caret x and the drawn width, which must agree.
        let ebox_geom = ebox.map(|ui| {
            let ink = crate::ui_text::measure_text(&mut atlas.lock(), draw_text, None, spec).0;
            format!(" caret={:.1} ink={ink:.1}", ui.caret_x * host.scale)
        });
        // `px` is the drawn height, after the cap, the seam and the frame scale.
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
    // The font object's `<Shadow>` (`MasterFont`'s `(1, -1)` black, `Fonts.xml:55`, which most
    // GameFonts inherit): the fill's layout and cells again at the offset in the shadow colour,
    // pushed first so the stable sort draws it behind. WoW's `y="-1"` is down: screen `dy = −y`.
    if let (Some(sh), Some((dx, dy))) = (style.shadow, shadow_delta) {
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
            // Markup tints ride the fill only; the alpha is [`shadow_alpha`].
            q.color = [
                sh.color[0],
                sh.color[1],
                sh.color[2],
                shadow_alpha(q.color[3], base_color[3], host.alpha),
            ];
            q.clip = text_clip;
        }
        out.extend(sq);
    }
    let mut glyphs = if let ZTarget::Frame(fh) = host.target {
        // A frame-targeted Text quad is a message line: collect its links for the click hit-test.
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
            // Back to the engine's y-up UI units.
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
        q.clip = text_clip;
    }
    // The seat probe: the fill's ink rows relative to the rect top, to compare with the
    // vertical-seat law's `d + ascender` (`0x5d1360`); the shadow's quads are not in `glyphs`.
    let vpl = style.world_seat && benilla_assets::trace::enabled_for("vpl");
    if (probe || vpl) && !glyphs.is_empty() {
        let (mut y0, mut y1) = (f32::MAX, f32::MIN);
        for q in &glyphs {
            y0 = y0.min(q.rect.min.y);
            y1 = y1.max(q.rect.max.y);
        }
        // Where the plate's text inked, beside the `vpl` tag's `plate=` and `paint=` lines.
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
    // The caret: a 1 px bar in the constructor's white (`0xffffffff`), one line cell tall at the
    // engine's x, pushed after the glyphs to draw on top. The reference's is 4 UI units wide
    // (`0x77b8c0`) and takes the edit box's text colour on a font change (`0x77e2a0`).
    if let (Some(ui), Some((x0, top, cell_h))) = (ebox, ebox_geom) {
        if host.caret_pinned || ui.caret_on {
            #[allow(clippy::cast_precision_loss)]
            let top = top + ui.caret_row as f32 * cell_h;
            // `caret_x` is in engine UI units.
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

/// How far below its band a message-frame line's ink reaches, in the rect's px space: the seat
/// nudge ([`crate::ui_text::UI_SEAT_NUDGE`]) plus a falling shadow's offset.
fn band_clip_slack(shadow_dy: Option<f32>) -> f32 {
    crate::ui_text::UI_SEAT_NUDGE + shadow_dy.map_or(0.0, |dy| dy.max(0.0))
}

/// One axis of the drop shadow's offset in the rect's px space, rounded to a whole pixel but
/// never to zero, as the reference's shadow shows at any window size. Fill and shadow each take
/// the client's vertical snap, `ceil(y − 0.5)`, which commutes only with an integer translate: a
/// fractional offset makes the gap between them alternate between 1 and 2 px.
fn shadow_offset_px(scaled: f32) -> f32 {
    let rounded = scaled.round();
    if rounded == 0.0 && scaled != 0.0 {
        scaled.signum()
    } else {
        rounded
    }
}

/// A shadow glyph's alpha, every scalar the fill rides plus its own: `layout` (the `<Shadow>`
/// colour's alpha times the write-on gradient), `fill` (the fill colour's alpha, which
/// `SetTextColor` and a MessageFrame line's fade move) and `host` (the effective frame alpha).
fn shadow_alpha(layout: f32, fill: f32, host: f32) -> f32 {
    layout * fill * host
}

#[cfg(test)]
mod tests {
    use super::{band_clip_slack, shadow_alpha, shadow_offset_px};

    #[test]
    fn a_drop_shadow_fades_through_the_colour_lane_as_well_as_the_frame_lane() {
        assert_eq!(shadow_alpha(1.0, 1.0, 1.0), 1.0);
        // The colour lane alone: a message-frame line mid-fade.
        assert_eq!(shadow_alpha(1.0, 0.25, 1.0), 0.25);
        // The frame lane alone (`UIFrameFadeOut`).
        assert_eq!(shadow_alpha(1.0, 1.0, 0.25), 0.25);
        // Both lanes, under a font object's own semi-transparent shadow.
        assert_eq!(shadow_alpha(0.8, 0.5, 0.5), 0.2);
        assert_eq!(shadow_alpha(1.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn the_band_scissor_gives_back_exactly_the_seat_and_the_shadow() {
        // The shipped GameFont case: seat nudge 1 + a 1px drop shadow.
        assert_eq!(band_clip_slack(Some(1.0)), 2.0);
        assert_eq!(band_clip_slack(None), 1.0);
        assert_eq!(band_clip_slack(Some(-1.0)), 1.0);
        assert_eq!(band_clip_slack(Some(2.0)), 3.0);
    }

    /// `1.0546875` is the seam scale of a 1600×900 window at uiScale 0.9.
    #[test]
    fn a_shadow_offset_is_a_whole_pixel_and_never_vanishes() {
        assert_eq!(shadow_offset_px(1.0), 1.0);
        assert_eq!(shadow_offset_px(1.0546875), 1.0);
        assert_eq!(shadow_offset_px(-1.0546875), -1.0);
        assert_eq!(shadow_offset_px(2.109375), 2.0);
        // 377-tall capture window × 0.9: 0.44 px would round to nothing.
        assert_eq!(shadow_offset_px(0.4418), 1.0);
        assert_eq!(shadow_offset_px(-0.4418), -1.0);
        assert_eq!(shadow_offset_px(0.0), 0.0);
    }

    /// An integer offset commutes with the client's vertical snap, `ceil(y − 0.5)`.
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
        // The unrounded offset gives both a 1 and a 2 px gap over one unit of y.
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
