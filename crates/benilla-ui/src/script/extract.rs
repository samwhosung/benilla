//! [`UiScript::extract`] — the render-list builder (decision 0068): the visible-tree
//! [`crate::order::traversal`] zipped with the resolved rects and per-kind region visuals, in the
//! client's painter order. An `impl UiScript` block beside its concern (the `layout.rs` pattern);
//! the shared ScrollFrame clip it walks lives in [`super::clip`].

use crate::layout::Rect;
use crate::order::{self, ZTarget};
use crate::widget::FrameHandle;

use super::clip::{effective_clip, scroll_clip_sources};
use super::{colorselect, slider, ExtractedQuad, FontObject, QuadContent, TexCoords, UiScript};

impl UiScript {
    /// **Every live draw target in the VM**, whether or not it is currently visible — each frame's
    /// own slot followed by its region leaves, in arena order.
    ///
    /// [`Self::extract`] answers *what is being drawn*; this answers *what exists*, and the two
    /// together are what let an instrument say **who drew a quad**. Diff this across a load and the
    /// new targets are exactly the ones that load created: no name prefixes, no heuristics, and no
    /// dependence on an addon naming anything at all. That is the oracle behind the addon
    /// harness's render column, and it is written this way because the obvious alternatives are
    /// both wrong — a with/without **quad-count diff** reads zero for an addon that *replaces* a
    /// window rather than adding one (Bagnon takes the bags over), and a **name-prefix** match
    /// cannot see an anonymous frame or a region an addon hangs off one of ours.
    ///
    /// Handles are generational, so a destroyed-and-reused slot can never be mistaken for a
    /// survivor of the baseline.
    pub fn live_targets(&self) -> Vec<ZTarget> {
        let model = self.model_ref();
        let mut out = Vec::new();
        for (fh, frame) in model.arena.iter_frames() {
            out.push(ZTarget::Frame(fh));
            out.extend(frame.regions.iter().copied().map(ZTarget::Region));
        }
        out
    }

    /// The frame a draw target belongs to — itself for a frame slot, the owner for a region.
    ///
    /// The half of the attribution that separates *created a window of its own* from *painted onto
    /// one of ours*: a new `Region` whose owner frame is **not** new is an addon hooking an
    /// existing frame, which is precisely what `!OmniCC` does to a cooldown and what a check built
    /// only on new frames would score as "drew nothing".
    pub fn target_frame(&self, target: ZTarget) -> Option<FrameHandle> {
        match target {
            ZTarget::Frame(fh) => self.model_ref().arena.frame(fh).map(|_| fh),
            ZTarget::Region(rh) => self.model_ref().arena.region(rh).map(|r| r.owner),
        }
    }

    /// A frame's parent, or `None` for a top-level frame (or a stale handle).
    ///
    /// The other half of the attribution walk: a frame being **new** does not make it the addon's
    /// own window — `!OmniCC` creates a brand-new anonymous frame *parented to one of our action
    /// buttons*, and only the chain tells an overlay from a window.
    pub fn frame_parent(&self, frame: FrameHandle) -> Option<FrameHandle> {
        self.model_ref().arena.frame(frame)?.parent
    }

    /// A frame's name, or `None` — it is anonymous, or the handle is stale.
    ///
    /// For reporting only. Nothing above depends on a frame being named; this is what turns an
    /// attributed handle into a row a human can act on.
    pub fn frame_name(&self, frame: FrameHandle) -> Option<String> {
        self.model_ref().arena.frame(frame)?.name.clone()
    }

    /// The nearest **named** frame at or above `target` — its own frame's name, else the closest
    /// named ancestor.
    ///
    /// An addon's slot buttons are usually named and its inner art usually is not, so a bare
    /// [`Self::frame_name`] would report a hole exactly where the interesting quads are. Walking up
    /// answers "which window is this part of", which is the question a report is asking.
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

    /// The render list in the client's painter order (decision 0068): the visible-tree
    /// [`crate::order::traversal`] zipped with the resolved rects and region visuals. Already sorted
    /// ascending by `ZKey`. Call [`UiScript::resolve`] first for populated rects.
    pub fn extract(&self) -> Vec<ExtractedQuad> {
        let model = self.model_ref();
        let list = order::traversal(&model.arena);
        let mut out = Vec::with_capacity(list.len());
        // The ScrollFrame clip sources (decision 0112): every live ScrollFrame with a resolved rect
        // and a live child, `child handle → the scrollframe's resolved rect`. Built once per extract;
        // [`effective_clip`] walks a quad's owner up through this to find every ancestor ScrollFrame
        // it is clipped by (nested ScrollFrames intersect).
        let scroll_sources = scroll_clip_sources(&model);
        for &(target, zkey) in list.iter() {
            let (rect, alpha, content, clip, scale) = match target {
                ZTarget::Frame(fh) => {
                    let frame = model.arena.frame(fh);
                    let alpha = frame.map(|f| f.effective_alpha).unwrap_or(1.0);
                    let scale = frame.map(|f| f.effective_scale).unwrap_or(1.0);
                    let clip = effective_clip(&model, &scroll_sources, fh);
                    // A Minimap widget's own slot carries its zoom out to the app renderer (the
                    // tile/mask/arrow draw — decision 0203); every other frame slot is bare.
                    let content = match frame.map(|f| &f.kind_state) {
                        Some(crate::widget::KindState::Minimap(m)) => QuadContent::Minimap {
                            zoom: m.zoom,
                            inside_zoom: m.inside_zoom,
                        },
                        // A `<Model>`/`<PlayerModel>` pane's content hole, carrying the pane's own
                        // name so the app can join it to the bake that window keeps (see
                        // [`QuadContent::ModelPane`]). Both widget kinds share `KindState::Model`
                        // because the client's `CGCharacterModelBase` extends `CSimpleModel`, and
                        // both draw the same way here.
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
                    // A draw layer switched off with `Frame:DisableDrawLayer` hides every region
                    // the frame owns in it. Skipped HERE rather than by touching the regions
                    // themselves, so each one keeps its own shown state and re-enabling the layer
                    // restores exactly what was visible before.
                    if let (Some(r), Some(f)) = (region, owner_frame) {
                        if f.disabled_layers & (1 << r.draw_layer.index()) != 0 {
                            continue;
                        }
                    }
                    // Regions clip with their owner frame (decision 0112 §4).
                    let clip = owner.and_then(|o| effective_clip(&model, &scroll_sources, o));
                    let mut rect = owner.and_then(|o| model.resolved.get(&o).copied());
                    // A StatusBar's bar-fill region draws at the value fraction of the frame's rect,
                    // along the orientation axis (horizontal grows rightward, vertical bottom-up). The
                    // bar owns its geometry — it never carries anchors, so it skips the region-rect
                    // precedence below. It also CROPS: see `bar_fill_uv`.
                    let mut bar_fill: Option<&crate::widget::StatusBarState> = None;
                    if let Some(crate::widget::KindState::StatusBar(sb)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        if sb.bar == Some(rh) {
                            rect = rect.map(|r| bar_fill_rect(r, sb));
                            bar_fill = Some(sb);
                        }
                    }
                    // A Slider's thumb draws at the value fraction along the track (decision 0250 §4),
                    // centered on the cross-axis — like the bar-fill, it owns its geometry and skips
                    // the region-rect precedence below.
                    let mut thumb_fill = false;
                    if let Some(crate::widget::KindState::Slider(sl)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        if sl.thumb == Some(rh) {
                            // The thumb's OWN size getters, not its authored `<Size>` — the same
                            // `CSimpleTexture::GetWidth`/`GetHeight` fallback the drag law reads
                            // (`slider::thumb_extent`), so a Lua-built `SetThumbTexture(path)` knob
                            // draws at its art's texel span instead of smeared over the whole track.
                            let tsize = super::region::virtual_span(&model, rh);
                            rect = rect
                                .map(|r| slider::thumb_rect(r, tsize, sl.vertical, sl.fraction()));
                            thumb_fill = true;
                        }
                    }
                    // The colour picker's four sub-textures. The wheel and the strip are ordinary
                    // authored regions — they keep their own anchors and resolve normally; all
                    // they need from here is a content kind that says "the app paints this one".
                    // The two markers are the opposite: their rects are DERIVED from the widget's
                    // HSV, so the pixel you clicked is the pixel the marker lands on
                    // (`colorselect::wheel_thumb_rect` is the exact inverse of the pick law).
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
                            // The strip anchors it; the WHEEL scales it (the client's own
                            // unguarded `[this+0x318]` read — `colorselect::value_thumb_rect`).
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
                    // A Button's state textures show by interaction state (the texture-array
                    // "current" pointer): a non-current state texture emits no quad this frame.
                    // The ButtonText additionally re-points to the current STATE's font instance
                    // (disabled > highlighted > normal) — the client's per-state label font swap
                    // (UIPanelButtonTemplate's gold/white/gray trio).
                    let mut state_font: Option<&FontObject> = None;
                    let mut state_color: Option<[f32; 4]> = None;
                    // The current instance's OWN justify (`<…Font justifyH=>`, a local write on
                    // the embedded font — [`crate::widget::ButtonState::normal_justify_h`]).
                    let mut state_justify: Option<super::JustifyH> = None;
                    // `Button:SetFont` — the face/size/flags written on the button's own embedded
                    // fonts rather than on any object they inherit.
                    let mut button_font: Option<&crate::widget::ButtonFont> = None;
                    if let Some(crate::widget::KindState::Button(bs)) =
                        owner_frame.map(|f| &f.kind_state)
                    {
                        let hovered = owner.is_some() && model.mouseover == owner;
                        // The PRESS is not read here. Which state texture shows is latched on the
                        // transition (`ButtonState::set_state`), so the press reaches the paint
                        // through the press EDGE at the moment the button goes down — not by
                        // being re-derived every frame. `hovered` survives because the Highlight
                        // is not a state texture and carries no latch.
                        if !bs.region_visible(rh, hovered) {
                            continue;
                        }
                        if bs.text == Some(rh) {
                            // The label inherits ONE of the button's three embedded font instances
                            // WHOLE (normal `+0x33c` / highlight `+0x3b8` / disabled `+0x434`): the
                            // client swaps which instance the label reads, it does not merge axes
                            // across them. So the object and the colour are picked as a PAIR, and a
                            // state with no font object of its own is not in force at all — the
                            // label stays on the normal instance, face and colour together.
                            //
                            // The pairing is the load-bearing half, and it is what this used to get
                            // wrong (the colour fell back to `normal_color` while the font did not).
                            // `UIDropDownMenu.lua` sets every row colour through `SetTextColor`
                            // **and** `SetHighlightTextColor` with the same values (l.216-220,
                            // l.829-830) — a second call it would never need if the first reached
                            // the hovered state. The tradeskill list leans on the other direction:
                            // its rows are `SetTextColor`'d to the recipe's difficulty and still
                            // turn white under the cursor, because `<HighlightFont
                            // inherits="GameFontHighlight">` is the instance in force there
                            // (`ClassTrainerFrameTemplates.xml` l.74) and a normal-instance colour
                            // cannot reach it.
                            //
                            // `LockHighlight()` counts as highlighted HERE too, not only for the
                            // HighlightTexture in `region_visible`. `TradeSkillFrame_Update` blanks
                            // a recipe row's highlight texture to `""` and *then* locks the row it
                            // selected (Blizzard_TradeSkillUI.lua l.131/144) — with no texture left
                            // to pin, the lock's only possible effect on that row is this label
                            // swap, which is the white text on the selected recipe. Craft
                            // (l.234) and the class trainer (l.183) lock the same way.
                            //
                            // INFERRED, not byte-verified: that an unset state instance leaves the
                            // label on the normal one. A null slot in the *texture* array draws
                            // nothing (decision 0227) and a font instance with no object cannot
                            // work that way — a disabled button with no `<DisabledFont>` still
                            // shows its label. Every state-colour caller in our own UI ships the
                            // matching font object, so the two readings agree on all of them.
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
                    // A TITLE REGION NEVER DRAWS. It is a hit rectangle, not a visual:
                    // `CreateTitleRegion 0x773910` builds it as a plain Region with no textures at
                    // all. Falling through here would emit the
                    // texture quad the `else` branch below builds — invisible on screen, but the
                    // render report counts quads, so every addon that makes one would read as
                    // "drew something" (1246's lesson about what an instrument is told).
                    if region.map(|r| r.kind) == Some(crate::widget::RegionKind::Title) {
                        continue;
                    }
                    // Region-level Hide (the VisibleRegion bit): no quad at all — checked on the
                    // borrow, BEFORE the clone below. `RegionData` is a fat row (text `String`,
                    // paths, the anchors `Vec`), and paying its clone for a row whose next line
                    // discards it was a per-hidden-region-per-frame allocation tax the extract
                    // walk never noticed it was paying.
                    let data_ref = model.region_data.get(&rh);
                    if data_ref.is_some_and(|d| d.hidden) {
                        continue;
                    }
                    // THE NAMEPLATE GLOW IS SHOWN AND NOT DRAWN — the one region in the engine
                    // whose paint the director replaced with something else (decision 0184: the
                    // lit plate brightens its bar instead of wearing the additive rim). It has to
                    // stay a real, shown, ADD-blended `Nameplate-Glow` region because
                    // `glow:IsShown()` IS the mouseover signal every 1.12 nameplate addon reads —
                    // so the deviation lives here, at the paint, and nowhere in the model.
                    if data_ref.is_some_and(super::nameplate::is_unpainted_glow) {
                        continue;
                    }
                    let mut data = data_ref.cloned().unwrap_or_default();
                    // The single-hop draw multiply (region-combine `0x772180`): the region's own
                    // alpha times its immediate owner's — never a product up the tree, because the
                    // owner's own `effective_alpha` was already overwritten by any ancestor's
                    // `SetAlpha 0x76a690`.
                    let alpha = owner_frame.map(|f| f.effective_alpha).unwrap_or(1.0)
                        * data.alpha.unwrap_or(1.0);
                    if let Some(fo) = state_font {
                        // The font object's paint, **behind the severance mask on every axis** —
                        // the same `font_explicit` gate `font::repaint` applies and the
                        // `button_font` block below names as the law ("it loses to a face the
                        // label FontString set for *itself*, which severs one level further
                        // down"). Three of these six axes did not have it: face and height were
                        // `fo.x.or(data.x)`, which makes the OBJECT outrank an explicit
                        // `SetFont`, and the outline was written unconditionally — so a label
                        // that called `SetFont(path, h, "OUTLINE")` for itself had all three put
                        // back from the object on the very next extract, every frame, forever.
                        // That is the *shape* of the report this was found under (decision 2112:
                        // an addon's `SetFont` silently not taking); MSBT's own strings are not
                        // a button's label and never met it, but any addon that restyles a
                        // `<ButtonText>` did.
                        if !data.font_explicit.face {
                            data.font_path = fo.font.clone().or(data.font_path);
                        }
                        if !data.font_explicit.height {
                            data.font_height = fo.height.or(data.font_height);
                        }
                        // Same severance as the colour below: a region that called
                        // `SetShadowColor`/`SetShadowOffset` keeps its own, or the font object it
                        // inherits would silently overwrite the value the addon just set.
                        if !data.font_explicit.shadow {
                            data.font_shadow = fo.shadow.or(data.font_shadow);
                        }
                        if !data.font_explicit.outline {
                            data.outline = fo.outline;
                        }
                        // Test the severance MASK, which is what the sentence above claims and what
                        // the reference pins, not `vertex_color.is_none()`. The nil-check was an
                        // equivalent proxy for exactly as long as an explicit `SetTextColor` was the
                        // only way a button label's colour could be populated at all; the moment
                        // `SetTextFontObject` began linking the label to its font object (so
                        // `GetFont`/`GetFontObject` stop answering nil), a label carried the NORMAL
                        // object's colour and the disabled state's gray silently stopped applying.
                        // Caught by `button_label_repaints_by_state_font_object`, which is the test
                        // that exists for this.
                        if !data.font_explicit.color {
                            data.vertex_color = fo.color.or(data.vertex_color);
                        }
                        // The instance's justify: its own `<…Font justifyH=>` (a local write on
                        // the embedded font, decision 1996), else the object's. Between frames
                        // the label's own word carries the NORMAL instance's value
                        // (`button::apply_normal_font`, the live link); a hover or a disable swaps
                        // it here the way the client re-links the label to another instance
                        // (`0x779810`) — behind the severance mask like every other axis, so a
                        // label that `SetJustifyH`'d for itself keeps its own.
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
                    // `Button:SetFont` sits BETWEEN the two: it is a local set on the button's own
                    // embedded font, so it outranks the font object that font inherits (a locally
                    // set axis clears the font's `inheritMask` bit, `CSimpleFontString+0xD4`, and
                    // is never restored), but it loses to a face the label FontString set for
                    // *itself*, which severs one level further down. That is what `font_explicit`
                    // is, so it is the gate here too.
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
                    // The button-level state color wins over the font object AND the region's own
                    // explicit color — the client's Button color slots repaint the label outright.
                    if let Some(c) = state_color {
                        data.vertex_color = Some(c);
                    }
                    // A region draws at its RESOLVED rect, and nothing else (decision 1310,
                    // superseding 0068 v1's centered/fill-the-owner fallbacks): every drawable
                    // region carries real anchors — authored, or the creation-path implicit anchor
                    // (`region::implicit_creation_anchor`) — and the real resolver has no
                    // zero-anchor fallback (a failed resolve latches unresolvable, `0x768d55`). A
                    // region with no rect here is a templateless Lua region nobody anchored, and
                    // the reference draws it nowhere. The bar-fill/thumb regions keep their own
                    // fraction geometry (computed off the owner rect above) — they never carry
                    // anchors and skip the resolver entirely, like the reference's own bar path.
                    // The bar-fill CROPS its texture (`SetValue 0x7cc450`→`0x7833c0` drives
                    // `0x770410`): every `SetValue` rewrites the region's 4-corner UV block with
                    // `u1 = fraction` and recomputes the quad as `right = left + frac·width`. So
                    // the art is sliced, never squeezed — a bar texture with a horizontal ramp
                    // (`UI-StatusBar` brightens 124→166 left-to-right) keeps its true gradient at
                    // every fill level.
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
                    // The generated art wins only over an EMPTY slot: an addon that sets a real
                    // texture on the wheel region gets its texture, the way it would on any other
                    // region. (`ColorPickerFrame.xml` sets none, which is the whole point.)
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
                            // A property of the OWNER, not of the string: the plate's own name
                            // and level and an addon's replacements for them are all drawn
                            // inside the same sliding rect.
                            world_seat: owner
                                .is_some_and(|o| super::nameplate::is_world_seated(&model, o)),
                        }
                    } else {
                        // The draw gate is the TEXTURE slot, never the colour (`0x7706e0`: `+0xcc`
                        // empty -> emit NOTHING). A
                        // vertex colour is a tint on whatever texture exists; alone it is not
                        // drawable content — it survives `SetTexture(nil)` by design ("a tint
                        // outlives the art it was tinting") and used to leak out of here as a
                        // solid plate the moment the art was cleared (the white action buttons on
                        // a character switch; the 2026-07-10 grey wells were the same class).
                        // A GRADIENT is drawable content in its own right — the client generates it
                        // into the same texture slot the colour form of `SetTexture` fills, so a
                        // region carrying only a gradient still paints. Folded to its midpoint here
                        // because a quad carries ONE tint; the whole gradient stays on the region
                        // (`RegionData::gradient`) so a renderer that grows a second stop needs no
                        // API change. The approximation is visible, and it is stated there and here
                        // rather than discovered later.
                        let fill = data.fill.or_else(|| data.gradient.map(|g| g.midpoint()));
                        let has_path = data.texture.is_some();
                        let has_texture = has_path || fill.is_some();
                        QuadContent::Texture {
                            path: data.texture,
                            color: has_texture
                                .then(|| texture_color(fill, data.vertex_color))
                                .flatten(),
                            // ADD is the one blend mode the renderer acts on; the other four of
                            // the client's `alphaMode` enum are carried on `RegionData::blend`
                            // (and answered by `GetBlendMode`) but drawn as straight alpha — the
                            // stated v1 gap, see [`crate::script::BlendMode`].
                            additive: data.blend == crate::script::BlendMode::Add,
                            tex_coords: data.tex_coords,
                            circular: data.circular,
                            portrait_unit: data.portrait_unit,
                            rotation: data.rotation,
                            // Only ever set against real ART. A pathless solid has its colour
                            // FOLDED into `color` two lines up (the renderer draws it as a tint on
                            // a 1x1 white texel), so a shader that greys the *texel* would grey
                            // white and change nothing — the flag would read as honoured while
                            // doing nothing at all. No reference consumer desaturates a solid;
                            // every one of them is an icon. Stated here rather than discovered.
                            desaturated: data.desaturated && has_path,
                        }
                    };
                    // A region draws at its owner's scale — same single hop as alpha (a region
                    // has no scale of its own; the `effective_scale 0x76ac90` product lives on
                    // frames).
                    let scale = owner_frame.map(|f| f.effective_scale).unwrap_or(1.0);
                    (rect, alpha, content, clip, scale)
                }
            };
            // **A `Model` frame's scene draws out of its bucket's ARTWORK batch, last:** the batch
            // object carries a third sub-array beside the quads and the text, a render-callback
            // list, and `0x76fb00` drains the three in that order; `0x76d160` registers the model's
            // callback only for `layer == 2` (`0x76d17f cmp ebx,2`). So a model is neither a
            // separate pass nor the frame's own layer-0 slot — it is ARTWORK content, after that
            // layer's quads.
            // This is what puts the world map's player arrow over the zone overlays: both frames
            // sit at `WorldMapFrame.level + 1`, the overlays are ARTWORK quads there, and the
            // arrow's callback drains after them (the director's report). A model's own
            // OVERLAY/HIGHLIGHT regions still draw over it. The
            // CALLBACK rank (1995) is what puts the scene after that layer's font strings too,
            // whatever their link stamps — the content key alone sat at the text rank.
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
            // A ScrollingMessageFrame emits, on top of its own (empty) frame slot, one Text quad per
            // visible ring line — stacked bottom-up (newest at the frame's bottom edge), each carrying
            // its live fade alpha in the text color. The generic region path can't do this (the lines
            // aren't declared FontStrings); the ring lives in the frame's kind state.
            if let (ZTarget::Frame(fh), Some(fr)) = (target, rect) {
                // The frame's own draw slot carries its Backdrop, behind its regions (which sort
                // after the frame slot). A ScrollingMessageFrame's ring lines do NOT: they take the
                // slot's ARTWORK *content* key ([`crate::order::ZKey::content`]) — the layer the
                // client's own message font strings live in — so the frame's BACKGROUND textures
                // stay behind them. At the bare slot the chat window's hover box painted over its
                // own messages. Both clip like the frame's own slot (the frame IS the clipped owner
                // when it's a scroll child).
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

/// The single colour a Texture region draws with: **`texel × vertexColour`**, per channel and
/// **alpha included** (the stage-0 combine preset live at submit, `.data 0x85c250` index 1:
/// `MODULATE(TEXTURE, DIFFUSE)` for both colour and alpha).
///
/// **That law is scoped to a region with no pixel shader bound** (`+0x128 == 0`). A DESATURATED
/// region takes the fragment program instead, which supersedes the whole stage chain and reads
/// the vertex colour's ALPHA only; its RGB never reaches the pixel. The colour computed here
/// still travels — the renderer needs the alpha, and the RGB is simply unread on that branch
/// (`ui_quad.wgsl`) — so nothing changes here.
///
/// `fill` is the region's own solid-colour texture ([`RegionData::fill`] — the client generates a
/// real 8×8 texel block from it), so where it is set it IS the texel and the product is the drawn
/// colour. Where it isn't, the texel comes from `path`'s art and this returns the tint alone for the
/// renderer to modulate the sample by; `None` means untinted.
///
/// The correction this encodes: a `<Color 1,1,1,0.2>` trough later `SetVertexColor(0,0,0.75,0.5)`'d
/// draws at alpha **0.1**. benilla used to store one colour slot and let the second call *replace*
/// the first, which drew it at 0.5.
fn texture_color(fill: Option<[f32; 4]>, vertex: Option<[f32; 4]>) -> Option<[f32; 4]> {
    match (fill, vertex) {
        (Some(f), Some(v)) => Some([f[0] * v[0], f[1] * v[1], f[2] * v[2], f[3] * v[3]]),
        (Some(c), None) | (None, Some(c)) => Some(c),
        (None, None) => None,
    }
}

/// A StatusBar's bar-fill rect: the frame rect scaled by the value fraction — rightward from the
/// left edge (horizontal) or upward from the bottom (vertical), the documented fill directions
/// (reverse-fill is an Era extension, not modeled yet).
fn bar_fill_rect(r: Rect, sb: &crate::widget::StatusBarState) -> Rect {
    let f = sb.fraction();
    if sb.vertical {
        Rect::new(r.bottom, r.left, r.bottom + r.height() * f, r.right)
    } else {
        Rect::new(r.bottom, r.left, r.top, r.left + r.width() * f)
    }
}

/// A StatusBar's bar-fill UV sub-rect: `base` (its `<TexCoords>`/`SetTexCoord`, or the full texture)
/// sliced to the value fraction along the fill axis — `[left, right, top, bottom]`, 0..1, top-left
/// origin.
///
/// The client CROPS rather than scales: `SetValue`
/// (`0x7cc450`→`0x7833c0`) drives `0x770410`, which writes the 4-corner UV block (`+0x104..+0x120`)
/// with `u1 = GetValue()` and recomputes `right = left + frac·width`. Horizontal is the verified
/// case; VERTICAL mirrors it up the `v` axis (bottom-up fill ⇒ the *bottom* edge of the art is
/// pinned) — inferred from the same block, not separately pinned.
fn bar_fill_uv(base: Option<TexCoords>, sb: &crate::widget::StatusBarState) -> TexCoords {
    // The fill crop is inherently the 4-edge form; an affine base (no live StatusBar uses one)
    // contributes its bounding edges.
    let [l, r, t, b] = base.map(|tc| tc.edges()).unwrap_or([0.0, 1.0, 0.0, 1.0]);
    let f = sb.fraction();
    TexCoords::Rect(if sb.vertical {
        [l, r, b - (b - t) * f, b]
    } else {
        [l, l + (r - l) * f, t, b]
    })
}
