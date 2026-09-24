//! [`UiScript::extract`], the render list: the visible-tree [`crate::order::traversal`] zipped
//! with the resolved rects and region visuals, in the client's painter order.

use crate::layout::Rect;
use crate::order::{self, ZTarget};
use crate::widget::FrameHandle;

use super::clip::{effective_clip, scroll_clip_sources};
use super::{colorselect, slider, ExtractedQuad, FontObject, QuadContent, TexCoords, UiScript};

impl UiScript {
    /// Every live draw target, visible or not: each frame's slot then its regions, in arena order.
    /// Handles are generational, so a diff across a load names exactly what the load created.
    pub fn live_targets(&self) -> Vec<ZTarget> {
        let model = self.model_ref();
        let mut out = Vec::new();
        for (fh, frame) in model.arena.iter_frames() {
            out.push(ZTarget::Frame(fh));
            out.extend(frame.regions.iter().copied().map(ZTarget::Region));
        }
        out
    }

    /// The frame a draw target belongs to: itself for a frame slot, the owner for a region.
    pub fn target_frame(&self, target: ZTarget) -> Option<FrameHandle> {
        match target {
            ZTarget::Frame(fh) => self.model_ref().arena.frame(fh).map(|_| fh),
            ZTarget::Region(rh) => self.model_ref().arena.region(rh).map(|r| r.owner),
        }
    }

    /// A frame's parent, or `None` for a top-level frame or a stale handle.
    pub fn frame_parent(&self, frame: FrameHandle) -> Option<FrameHandle> {
        self.model_ref().arena.frame(frame)?.parent
    }

    /// A frame's name, or `None` for an anonymous frame or a stale handle.
    pub fn frame_name(&self, frame: FrameHandle) -> Option<String> {
        self.model_ref().arena.frame(frame)?.name.clone()
    }

    /// The name of the nearest named frame at or above `target`.
    pub fn target_owner_name(&self, target: ZTarget) -> Option<String> {
        let model = self.model_ref();
        let mut frame = match target {
            ZTarget::Frame(fh) => Some(fh),
            ZTarget::Region(rh) => Some(model.arena.region(rh)?.owner),
        };
        while let Some(fh) = frame {
            let f = model.arena.frame(fh)?;
            if let Some(name) = &f.name {
                return Some(name.clone());
            }
            frame = f.parent;
        }
        None
    }

    /// The render list in painter order, ascending by `ZKey`; call [`UiScript::resolve`] first.
    pub fn extract(&self) -> Vec<ExtractedQuad> {
        let model = self.model_ref();
        let list = order::traversal(&model.arena);
        let mut out = Vec::with_capacity(list.len());
        // Scroll child → its ScrollFrame's rect; a quad clips to every ancestor ScrollFrame's.
        let scroll_sources = scroll_clip_sources(&model);
        for &(target, zkey) in list.iter() {
            let (rect, alpha, content, clip, scale) = match target {
                ZTarget::Frame(fh) => {
                    let frame = model.arena.frame(fh);
                    let alpha = frame.map(|f| f.effective_alpha).unwrap_or(1.0);
                    let scale = frame.map(|f| f.effective_scale).unwrap_or(1.0);
                    let clip = effective_clip(&model, &scroll_sources, fh);
                    let content = match frame.map(|f| &f.kind_state) {
                        Some(crate::widget::KindState::Minimap(m)) => QuadContent::Minimap {
                            zoom: m.zoom,
                            inside_zoom: m.inside_zoom,
                        },
                        // A `<Model>`/`<PlayerModel>` pane's content hole, named so the app can
                        // join it to that window's bake. Both kinds share `KindState::Model`, as
                        // the client's `CGCharacterModelBase` extends `CSimpleModel`.
                        Some(crate::widget::KindState::Model(m)) => QuadContent::ModelPane {
                            handle: fh,
                            name: frame.and_then(|f| f.name.clone()),
                            model: m.path.clone(),
                            facing: m.facing,
                            model_scale: m.scale,
                            position: m.position,
                            own_alpha: frame.map_or(1.0, |f| f.alpha),
                            icon: m.icon.clone(),
                            camera: m.camera,
                            light: m.light,
                            fog: m.armed_fog(),
                        },
                        _ => QuadContent::Frame,
                    };
                    (
                        model.resolved.get(&fh).copied(),
                        alpha,
                        content,
                        clip,
                        scale,
                    )
                }
                ZTarget::Region(rh) => {
                    let region = model.arena.region(rh);
                    let owner = region.map(|r| r.owner);
                    let owner_frame = owner.and_then(|o| model.arena.frame(o));
                    // A layer switched off by `Frame:DisableDrawLayer` hides its regions; skipped
                    // here so each region keeps its own shown state for when the layer returns.
                    if let (Some(r), Some(f)) = (region, owner_frame) {
                        if f.disabled_layers & (1 << r.draw_layer.index()) != 0 {
                            continue;
                        }
                    }
                    let clip = owner.and_then(|o| effective_clip(&model, &scroll_sources, o));
                    let mut rect = owner.and_then(|o| model.resolved.get(&o).copied());
                    // A StatusBar's fill has no anchors: `bar_fill_rect` places it.
                    let mut bar_fill: Option<&crate::widget::StatusBarState> = None;
                    if let Some(crate::widget::KindState::StatusBar(sb)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        if sb.bar == Some(rh) {
                            rect = rect.map(|r| bar_fill_rect(r, sb));
                            bar_fill = Some(sb);
                        }
                    }
                    // A Slider's thumb likewise: at the value fraction along the track.
                    let mut thumb_fill = false;
                    if let Some(crate::widget::KindState::Slider(sl)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        if sl.thumb == Some(rh) {
                            // The thumb's own `CSimpleTexture::GetWidth`/`GetHeight`, as the drag
                            // reads them (`slider::thumb_extent`), not its authored `<Size>`.
                            let tsize = super::region::virtual_span(&model, rh);
                            rect = rect
                                .map(|r| slider::thumb_rect(r, tsize, sl.vertical, sl.fraction()));
                            thumb_fill = true;
                        }
                    }
                    // The colour picker: the wheel and strip resolve like any region and the app
                    // paints them; the markers' rects derive from the HSV, inverting the pick.
                    let mut color_art: Option<QuadContent> = None;
                    let mut color_thumb = false;
                    if let Some(crate::widget::KindState::ColorSelect(cs)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        let tsize = model.region_data.get(&rh).and_then(|d| d.size);
                        if cs.wheel_thumb == Some(rh) {
                            rect = cs
                                .wheel
                                .and_then(|w| model.region_resolved.get(&w).copied())
                                .map(|w| colorselect::wheel_thumb_rect(w, tsize, cs.hsv));
                            color_thumb = true;
                        } else if cs.value_thumb == Some(rh) {
                            // The strip anchors it and the wheel scales it: the client's unguarded
                            // `[this+0x318]` read.
                            let wheel = cs
                                .wheel
                                .and_then(|w| model.region_resolved.get(&w).copied());
                            rect = cs
                                .value_strip
                                .and_then(|v| model.region_resolved.get(&v).copied())
                                .map(|v| colorselect::value_thumb_rect(v, wheel, tsize, cs.hsv));
                            color_thumb = true;
                        } else if cs.wheel == Some(rh) {
                            color_art = Some(QuadContent::ColorWheel);
                        } else if cs.value_strip == Some(rh) {
                            color_art = Some(QuadContent::ColorValue {
                                hue: cs.hsv[0],
                                sat: cs.hsv[1],
                            });
                        }
                    }
                    // A Button's non-current state textures emit no quad, and its ButtonText takes
                    // the current state's font instance (disabled > highlighted > normal).
                    let mut state_font: Option<&FontObject> = None;
                    let mut state_color: Option<[f32; 4]> = None;
                    let mut state_justify: Option<super::JustifyH> = None;
                    let mut button_font: Option<&crate::widget::ButtonFont> = None;
                    if let Some(crate::widget::KindState::Button(bs)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        let hovered = owner.is_some() && model.mouseover == owner;
                        // The press is not read here: the state texture latches on the transition
                        // (`ButtonState::set_state`). Only the Highlight, unlatched, reads hover.
                        if !bs.region_visible(rh, hovered) {
                            continue;
                        }
                        if bs.text == Some(rh) {
                            // The label takes one embedded font instance whole (normal `+0x33c`,
                            // highlight `+0x3b8`, disabled `+0x434`), so font and colour pair up;
                            // a state lacking a font object stays on normal (inferred, not traced).
                            // A locked highlight counts: the trade-skill list blanks the highlight
                            // texture and locks the selected row (`Blizzard_TradeSkillUI.lua:133`,
                            // `:144`), so this swap is its white text.
                            let highlighted = hovered || bs.locked_highlight;
                            let (name, color, justify) = if !bs.enabled()
                                && bs.disabled_font.is_some()
                            {
                                (
                                    bs.disabled_font.as_ref(),
                                    bs.disabled_color,
                                    bs.disabled_justify_h,
                                )
                            } else if bs.enabled() && highlighted && bs.highlight_font.is_some() {
                                (
                                    bs.highlight_font.as_ref(),
                                    bs.highlight_color,
                                    bs.highlight_justify_h,
                                )
                            } else {
                                (
                                    bs.normal_font.as_ref(),
                                    bs.normal_color,
                                    bs.normal_justify_h,
                                )
                            };
                            state_font = name.and_then(|n| model.font_object(n));
                            button_font = bs.font.as_ref();
                            state_color = color;
                            state_justify = justify;
                        }
                    }
                    // A title region never draws: `CreateTitleRegion 0x773910` builds a plain
                    // Region with no textures, a hit rectangle only.
                    if region.map(|r| r.kind) == Some(crate::widget::RegionKind::Title) {
                        continue;
                    }
                    // Hidden (the VisibleRegion bit): no quad; tested before the clone below.
                    let data_ref = model.region_data.get(&rh);
                    if data_ref.is_some_and(|d| d.hidden) {
                        continue;
                    }
                    // Deviation: the nameplate glow is shown but never drawn, because its additive
                    // rim reads as hard edge lines in our linear-blending pipeline; the lit plate
                    // brightens its bar instead. It stays a real shown ADD region, as
                    // `glow:IsShown()` is the mouseover signal 1.12 nameplate addons read.
                    if data_ref.is_some_and(super::nameplate::is_unpainted_glow) {
                        continue;
                    }
                    let mut data = data_ref.cloned().unwrap_or_default();
                    // One hop (region combine `0x772180`): the region's alpha times its owner's
                    // `effective_alpha`, which `SetAlpha 0x76a690` already carries down the tree.
                    let alpha = owner_frame.map(|f| f.effective_alpha).unwrap_or(1.0)
                        * data.alpha.unwrap_or(1.0);
                    if let Some(fo) = state_font {
                        // The font object's paint, behind the severance mask (`font_explicit`) on
                        // every axis as in `font::repaint`: an axis the label set itself stays.
                        if !data.font_explicit.face {
                            data.font_path = fo.font.clone().or(data.font_path);
                        }
                        if !data.font_explicit.height {
                            data.font_height = fo.height.or(data.font_height);
                        }
                        if !data.font_explicit.shadow {
                            data.font_shadow = fo.shadow.or(data.font_shadow);
                        }
                        if !data.font_explicit.outline {
                            data.outline = fo.outline;
                        }
                        // The mask, not `vertex_color.is_none()`: a label linked to its font object
                        // already carries the normal object's colour.
                        if !data.font_explicit.color {
                            data.vertex_color = fo.color.or(data.vertex_color);
                        }
                        // The instance's own `justifyH`, else the object's; a hover or disable
                        // swaps it here, as the client re-links the label (`0x779810`).
                        if !data.font_explicit.justify_h {
                            if let Some(j) = state_justify.or(fo.justify_h) {
                                data.justify.set_h(j);
                            }
                        }
                        if !data.font_explicit.justify_v {
                            if let Some(j) = fo.justify_v {
                                data.justify.set_v(j);
                            }
                        }
                    }
                    // `Button:SetFont` sits between: a local set on the embedded font outranks its
                    // object (clearing the `inheritMask` bit, `CSimpleFontString+0xD4`, for good)
                    // but loses to the label's own, so `font_explicit` gates it too.
                    if let Some(bf) = button_font {
                        if !data.font_explicit.face {
                            data.font_path = Some(bf.path.clone());
                        }
                        if !data.font_explicit.height {
                            data.font_height = Some(bf.height);
                        }
                        if !data.font_explicit.outline {
                            data.outline = super::Outline::flags(&bf.flags);
                        }
                    }
                    // The state colour repaints the label outright, even over its own colour.
                    if let Some(c) = state_color {
                        data.vertex_color = Some(c);
                    }
                    // A region draws at its resolved rect only: drawable regions carry anchors
                    // (authored, or `region::implicit_creation_anchor`) and a failed resolve
                    // latches unresolvable (`0x768d55`), so an unanchored Lua region draws nowhere,
                    // as in the reference. The bar fill and thumbs keep their fraction geometry.
                    if let Some(sb) = bar_fill {
                        data.tex_coords = Some(bar_fill_uv(data.tex_coords, sb));
                    }
                    if bar_fill.is_none() && !thumb_fill && !color_thumb {
                        rect = model.region_resolved.get(&rh).copied();
                    }
                    let is_text = matches!(
                        region.map(|r| r.kind),
                        Some(crate::widget::RegionKind::FontString)
                    );
                    // Generated art fills only an empty slot (`ColorPickerFrame.xml` sets none).
                    let content = if let Some(art) =
                        color_art.filter(|_| data.texture.is_none() && data.fill.is_none())
                    {
                        art
                    } else if is_text {
                        QuadContent::Text {
                            text: data.text,
                            color: data.vertex_color,
                            // The gx translator's answer, not the getter's: a cleared axis draws
                            // CENTER/MIDDLE (`0x44d420`), where `GetJustifyH` says "UNKNOWN".
                            justify_h: data.justify.paint_h(),
                            justify_v: data.justify.paint_v(),
                            font: data.font_path,
                            font_height: data.font_height,
                            text_height: data.text_height,
                            shadow: data.font_shadow,
                            outline: data.outline,
                            alpha_gradient: data.alpha_gradient,
                            // The owner's property: an addon's strings on a plate slide with it.
                            world_seat: owner
                                .is_some_and(|o| super::nameplate::is_world_seated(&model, o)),
                        }
                    } else {
                        // The draw gate is the texture slot, never the colour (`0x7706e0`: empty
                        // `+0xcc` emits nothing); a vertex colour alone is a tint. A gradient fills
                        // the texture slot, so it draws, at its midpoint since a quad has one tint.
                        let fill = data.fill.or_else(|| data.gradient.map(|g| g.midpoint()));
                        let has_path = data.texture.is_some();
                        let has_texture = has_path || fill.is_some();
                        QuadContent::Texture {
                            path: data.texture,
                            color: has_texture
                                .then(|| texture_color(fill, data.vertex_color))
                                .flatten(),
                            // The renderer acts on ADD only: the other four `alphaMode` values
                            // answer `GetBlendMode` but draw as straight alpha.
                            additive: data.blend == crate::script::BlendMode::Add,
                            tex_coords: data.tex_coords,
                            circular: data.circular,
                            portrait_unit: data.portrait_unit,
                            rotation: data.rotation,
                            // Real art only: a solid here is a tint on a white texel, where the
                            // reference's is a texel block it would grey. No stock caller
                            // desaturates a solid.
                            desaturated: data.desaturated && has_path,
                        }
                    };
                    // A region draws at its owner's scale: only frames carry one (`0x76ac90`).
                    let scale = owner_frame.map(|f| f.effective_scale).unwrap_or(1.0);
                    (rect, alpha, content, clip, scale)
                }
            };
            // A `Model` frame's scene draws in its bucket's ARTWORK batch, last: `0x76fb00` drains
            // quads, text, then render callbacks, and `0x76d160` registers the model's callback
            // for layer 2 only (`0x76d17f cmp ebx,2`). So the world map's player arrow draws over
            // the zone overlays at the same level, below its own OVERLAY and HIGHLIGHT regions.
            let z = match &content {
                QuadContent::ModelPane { .. } => zkey.callback(order::DrawLayer::Artwork).raw(),
                _ => zkey.raw(),
            };
            out.push(ExtractedQuad {
                target,
                z,
                rect,
                alpha,
                content,
                clip,
                scale,
            });
            // A ScrollingMessageFrame adds one Text quad per visible ring line, stacked bottom-up
            // with each line's fade alpha; the ring lives in its kind state, not in FontStrings.
            if let (ZTarget::Frame(fh), Some(fr)) = (target, rect) {
                // The Backdrop draws at the frame slot; the lines take the ARTWORK content key,
                // where the client's message strings live, over the BACKGROUND (hover box) art.
                let paint = super::layout::FramePaint { alpha, scale };
                Self::emit_backdrop(&model, fh, fr, zkey.raw(), paint, clip, &mut out);
                Self::emit_message_lines(
                    &model,
                    fh,
                    fr,
                    zkey.content(order::DrawLayer::Artwork).raw(),
                    paint,
                    clip,
                    &mut out,
                );
            }
        }
        out
    }
}

/// The colour a Texture region draws with: texel × vertex colour per channel, alpha included (the
/// stage-0 combine preset at submit, `.data 0x85c250` index 1: `MODULATE(TEXTURE, DIFFUSE)`). A
/// desaturated region's pixel shader (`+0x128`) reads only the vertex alpha. `fill`, the region's
/// solid colour, is a real 8×8 texel block in the client, so where set the product is the drawn
/// colour; otherwise this is the tint the renderer modulates the art by, `None` for untinted.
fn texture_color(fill: Option<[f32; 4]>, vertex: Option<[f32; 4]>) -> Option<[f32; 4]> {
    match (fill, vertex) {
        (Some(f), Some(v)) => Some([f[0] * v[0], f[1] * v[1], f[2] * v[2], f[3] * v[3]]),
        (Some(c), None) | (None, Some(c)) => Some(c),
        (None, None) => None,
    }
}

/// A StatusBar's fill rect: the frame rect scaled by the value fraction, rightward from the left
/// edge or upward from the bottom (1.12 has no reverse fill).
fn bar_fill_rect(r: Rect, sb: &crate::widget::StatusBarState) -> Rect {
    let f = sb.fraction();
    if sb.vertical {
        Rect::new(r.bottom, r.left, r.bottom + r.height() * f, r.right)
    } else {
        Rect::new(r.bottom, r.left, r.top, r.left + r.width() * f)
    }
}

/// A StatusBar's fill UVs: `base` (its `<TexCoords>`/`SetTexCoord`, or the full texture) cut to
/// the value fraction along the fill axis, `[left, right, top, bottom]` in 0..1 from top-left. The
/// client crops rather than scales: `SetValue` (`0x7cc450` → `0x7833c0`) drives `0x770410`, which
/// writes the 4-corner UV block (`+0x104..+0x120`) with `u1` set to the fill fraction. Vertical
/// mirrors it up `v`, pinning the art's bottom edge (inferred, not traced).
fn bar_fill_uv(base: Option<TexCoords>, sb: &crate::widget::StatusBarState) -> TexCoords {
    // An affine base contributes its bounding edges (no live StatusBar uses one).
    let [l, r, t, b] = base.map(|tc| tc.edges()).unwrap_or([0.0, 1.0, 0.0, 1.0]);
    let f = sb.fraction();
    TexCoords::Rect(if sb.vertical {
        [l, r, b - (b - t) * f, b]
    } else {
        [l, l + (r - l) * f, t, b]
    })
}
