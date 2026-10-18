//! [`UiScript::extract`], the render list: the visible-tree [`crate::order::traversal`] zipped
//! with the resolved rects and region visuals, in the client's painter order.

use crate::layout::Rect;
use crate::order::{self, ZTarget};
use crate::widget::FrameHandle;

use super::clip::DrawClips;
use super::{colorselect, layer_batch, slider, ExtractedQuad, QuadContent, TexCoords, UiScript};

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
        // The files set since the last draw take their stamps: this draw forces them.
        self.model_mut().texels.flush();
        let model = self.model_ref();
        let list = order::traversal(&model.arena);
        let mut out = Vec::with_capacity(list.len());
        // A scroll child's subtree clips to its ScrollFrames' rects, nested ones intersecting.
        let mut clips = DrawClips::new(&model, &list);
        // Every texture quad's batch and texture, for the batches' End sort below.
        let mut batch_quads: Vec<layer_batch::BatchQuad> = Vec::new();
        for &order::DrawEntry {
            target,
            key: zkey,
            scope,
        } in list.iter()
        {
            if zkey.parts().strata == order::Strata::World.index()
                && super::nameplate::awaits_batch(&model, target)
            {
                continue;
            }
            // Under a ScrollFrame with no rect the callback draws nothing (`0x787253`).
            let Some(clip) = clips.clip(scope) else {
                continue;
            };
            // A region's texture, which ranks it in the batch sort.
            let mut region_tex = None;
            let (rect, alpha, content, clip, scale) = match target {
                ZTarget::Frame(fh) => {
                    let frame = model.arena.frame(fh);
                    let alpha = frame.map_or(1.0, |f| crate::color::alpha_f32(f.alpha));
                    let scale = frame.map(|f| f.effective_scale).unwrap_or(1.0);
                    let content = match frame.map(|f| &f.kind_state) {
                        // The zoom cells are the minimap's statics, one pair for every Minimap.
                        Some(crate::widget::KindState::Minimap(_)) => QuadContent::Minimap {
                            zoom: model.minimap.zoom,
                            inside_zoom: model.minimap.inside_zoom,
                        },
                        // A `<Model>`/`<PlayerModel>` pane's content hole. Both kinds share
                        // `KindState::Model`, as the client's `CGCharacterModelBase` extends
                        // `CSimpleModel`.
                        Some(crate::widget::KindState::Model(m)) => QuadContent::ModelPane {
                            handle: fh,
                            name: frame.and_then(|f| f.name.clone()),
                            character: m.character.is_some(),
                            model: m.path.clone(),
                            facing: m.facing,
                            model_scale: m.scale,
                            position: m.position,
                            own_alpha: frame.map_or(1.0, |f| crate::color::alpha_f32(f.alpha)),
                            icon: m.icon.clone(),
                            camera: m.camera,
                            light: m.light,
                            fog: m.scene_fog(),
                            ffx: m.ffx,
                        },
                        _ => QuadContent::Frame,
                    };
                    (model.frame_rect(fh), alpha, content, clip, scale)
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
                    let mut rect = owner.and_then(|o| model.frame_rect(o));
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
                            // The thumb's own `CSimpleTexture::GetWidth`/`GetHeight` at the
                            // slider's scale, as the drag reads them (`slider::thumb_extent`), not
                            // its authored `<Size>`.
                            let tsize = super::region::screen_span(&model, rh);
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
                        // The markers' authored size, in the picker's units.
                        let scale = owner_frame.map_or(1.0, |f| f.effective_scale);
                        let tsize = model
                            .region_data
                            .get(&rh)
                            .and_then(|d| d.size)
                            .map(|(w, h)| (w * scale, h * scale));
                        if cs.wheel_thumb == Some(rh) {
                            rect = cs
                                .wheel
                                .and_then(|w| model.region_rect(w))
                                .map(|w| colorselect::wheel_thumb_rect(w, tsize, cs.hsv));
                            color_thumb = true;
                        } else if cs.value_thumb == Some(rh) {
                            // The strip anchors it and the wheel scales it: the client's unguarded
                            // `[this+0x318]` read.
                            let wheel = cs.wheel.and_then(|w| model.region_rect(w));
                            rect = cs
                                .value_strip
                                .and_then(|v| model.region_rect(v))
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
                    // A Button's state textures draw by their shown bit like any region. Its
                    // label paints from its own fields, which the state's embedded font pushed
                    // (`script::button`), so the draw and the measure read one font; only the
                    // pushed text offset is the button's.
                    let mut label_offset = [0.0f32; 2];
                    if let Some(crate::widget::KindState::Button(bs)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        let hovered = owner.is_some() && model.mouseover == owner;
                        // The press is not read here: the state texture latches on the transition
                        // (`ButtonState::set_state`). Only the Highlight, unlatched, reads hover.
                        if !bs.highlight_draws(rh, hovered) {
                            continue;
                        }
                        if bs.text == Some(rh) {
                            label_offset = bs.label_offset();
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
                    region_tex = data.tex.id();
                    // One hop (region combine `0x772180`): the region's alpha times its owner's
                    // byte, which `SetAlpha 0x76a690` already carries down the tree.
                    let alpha = owner_frame.map_or(1.0, |f| crate::color::alpha_f32(f.alpha))
                        * data.alpha.map_or(1.0, crate::color::alpha_f32);
                    // A region draws at its resolved rect: drawable regions carry anchors
                    // (authored, or `region::implicit_creation_anchor`), so a region that never
                    // resolved draws nowhere, as in the reference. After a failed resolve
                    // (`0x768d55`) a FontString stops (`0x7730b7` needs the valid bit) and a
                    // Texture keeps the quad its last good resolve wrote (`0x7706e0`). The bar
                    // fill and thumbs keep their fraction geometry.
                    if let Some(sb) = bar_fill {
                        data.tex_coords = Some(bar_fill_uv(data.tex_coords, sb));
                    }
                    let is_text = matches!(
                        region.map(|r| r.kind),
                        Some(crate::widget::RegionKind::FontString)
                    );
                    // A FontString with no font builds no string (`0x7724f8`) and emits nothing
                    // (`0x772e5b`), whatever its text.
                    if is_text
                        && data.font_path.is_none()
                        && !super::font_block::has_font(&model, rh)
                    {
                        continue;
                    }
                    if bar_fill.is_none() && !thumb_fill && !color_thumb {
                        rect = model
                            .region_rect(rh)
                            .or_else(|| (!is_text).then(|| model.region_kept_rect(rh)).flatten());
                    }
                    // A pushed label's text moves by its offset, `x · scale + left`,
                    // `y · scale + bottom` (`0x772100`); its rect, anchors and edges stay.
                    if label_offset != [0.0, 0.0] {
                        let k =
                            model.layout_unit() * owner_frame.map_or(1.0, |f| f.effective_scale);
                        let (dx, dy) = (label_offset[0] * k, label_offset[1] * k);
                        rect = rect.map(|r| {
                            crate::layout::Rect::new(
                                r.bottom + dy,
                                r.left + dx,
                                r.top + dy,
                                r.right + dx,
                            )
                        });
                    }
                    // Generated art fills only an empty slot (`ColorPickerFrame.xml` sets none).
                    let content = if let Some(art) =
                        color_art.filter(|_| data.tex.path().is_none() && data.tex.fill().is_none())
                    {
                        art
                    } else if is_text {
                        let spacing = data.line_gap();
                        let text_height = data.text_height();
                        let text_color = data.vertex_color_or_white();
                        QuadContent::Text {
                            text: data.text,
                            // The slot's colour as `0x77f8c0` answers it: an empty one, or one
                            // set to white that stored nothing (`0x77f766`), draws white.
                            color: Some(text_color.to_f32()),
                            // The gx translator's answer, not the getter's: a cleared axis draws
                            // CENTER/MIDDLE (`0x44d420`), where `GetJustifyH` says "UNKNOWN".
                            justify_h: data.justify.paint_h(),
                            justify_v: data.justify.paint_v(),
                            font: data.font_path,
                            font_height: data.font_height,
                            text_height,
                            shadow: data.font_shadow,
                            outline: data.outline,
                            monochrome: data.monochrome,
                            spacing,
                            alpha_gradient: data.alpha_gradient,
                            // The owner's property: an addon's strings on a plate slide with it.
                            world_seat: owner
                                .is_some_and(|o| super::nameplate::is_world_seated(&model, o)),
                            edit_line: matches!(
                                owner_frame.map(|f| &f.kind_state),
                                Some(crate::widget::KindState::EditBox(eb))
                                    if eb.text_region == Some(rh) && !eb.multi_line
                            ),
                            non_space_wrap: model.non_space_wrap_of(rh),
                            max_lines: data.max_lines,
                        }
                    } else {
                        // The draw gate is the texture slot, never the colour (`0x7706e0`: empty
                        // `+0xcc` emits nothing): a vertex colour or a gradient alone is a tint.
                        let fill = data.tex.fill();
                        let has_path = data.tex.path().is_some();
                        let has_texture = has_path || fill.is_some();
                        let wrap = data.tex.wrap();
                        QuadContent::Texture {
                            path: std::mem::take(&mut data.tex).into_path(),
                            color: has_texture
                                .then(|| {
                                    let mut tint =
                                        data.vertex_color.map(crate::color::Rgba8::to_f32);
                                    if let (Some(t), Some(k)) = (
                                        tint.as_mut(),
                                        super::nameplate::lit_fill_boost(&model, rh),
                                    ) {
                                        t[..3].iter_mut().for_each(|c| *c *= k);
                                    }
                                    texture_color(fill.map(crate::color::Rgba8::to_f32), tint)
                                })
                                .flatten(),
                            // The quad record carries the four colours with stride 4
                            // (`0x77304d`), one per vertex, each times the texel.
                            gradient: data
                                .gradient
                                .filter(|_| has_texture)
                                .map(|g| g.vertex_colors()),
                            blend: drawn_blend(
                                data.blend,
                                owner_frame.map_or(1.0, |f| crate::color::alpha_f32(f.alpha)),
                                data.alpha.map_or(1.0, crate::color::alpha_f32),
                                data.vertex_color.map(crate::color::Rgba8::to_f32),
                                data.gradient.map(|g| g.vertex_colors()),
                            ),
                            tex_coords: data.tex_coords,
                            wrap,
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
            // A slot's texture by its stamp; the colour picker's two images, built when it loads
            // (`0x78b580`, `0x78b8a0`), before anything a script sets.
            let rank = match &content {
                QuadContent::Texture { .. } => region_tex.map(|id| model.texels.stamp(id)),
                QuadContent::ColorWheel | QuadContent::ColorValue { .. } => Some(0),
                _ => None,
            };
            if let (Some(rank), Some(batch)) = (rank, order::quad_batch(z)) {
                batch_quads.push(layer_batch::BatchQuad {
                    index: out.len(),
                    batch,
                    rank,
                });
            }
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
                // The lines take the ARTWORK content key, where the client's message strings
                // live, over the BACKGROUND (hover box) art.
                let paint = super::layout::FramePaint { alpha, scale };
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
        // Each layer batch's End sorts its quads by texture (`0x7731a0`).
        layer_batch::sort_batches(&mut out, &batch_quads);
        out
    }
}

/// The blend a texture's record draws with (`0x76fb00`): a record with a colour array and a mode
/// below `BLEND` draws as `BLEND` (`0x76fb85`-`0x76fba2`). The array is there exactly when it holds
/// an entry (`0x773035`), and the frame-alpha fold (`0x77fac0`) leaves one exactly when the frame's
/// alpha byte is below `0xfe` (`0x77fb6a` builds a white one) or an entry is not opaque white
/// (`0x77faf5`-`0x77fb23`). The entries are the vertex colour, whose alpha byte the region's
/// `SetAlpha` writes (`0x79b580`), or a gradient's four; a solid fill is the texel (`0x770360`).
fn drawn_blend(
    mode: crate::script::BlendMode,
    frame_alpha: f32,
    region_alpha: f32,
    vertex: Option<[f32; 4]>,
    gradient: Option<[[f32; 4]; 4]>,
) -> crate::script::BlendMode {
    use crate::script::BlendMode;
    if !matches!(mode, BlendMode::Disable | BlendMode::AlphaKey) {
        return mode;
    }
    // A channel's byte as the colour setters pack it (`×255 + 0.5`, truncated, `0x40a2b0`).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let opaque_white =
        |c: [f32; 4]| c[..3].iter().all(|&v| byte(v) == 0xff) && byte(c[3] * region_alpha) >= 0xfe;
    let all_white = match (gradient, vertex) {
        (Some(g), _) => g.into_iter().all(opaque_white),
        (None, Some(c)) => opaque_white(c),
        (None, None) => opaque_white([1.0; 4]),
    };
    if byte(frame_alpha) < 0xfe || !all_white {
        BlendMode::Blend
    } else {
        mode
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
