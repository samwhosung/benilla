//! The two message-frame widget classes, and the display machinery they share.
//!
//! 1.12 ships **two** of them, and they are *siblings, not parent and child* (different ctors,
//! different vtables, different Lua type tags, offsets that do not transfer):
//!
//! - [`scrolling`] — `CSimpleMessageScrollFrame` (ctor `0x787670`), the chat window's class: a true
//!   ring of `maxLines`, a scrollback cursor, `AddMessage(text[,r,g,b[,id]])` with alpha forced
//!   opaque.
//! - [`plain`] — `CSimpleMessageFrame` (ctor `0x785640`), `UIErrorsFrame`'s class: no ring, no
//!   scrollback, `insertMode`, and `AddMessage(text[,r,g,b[,a]])` whose fourth numeric is a real
//!   **alpha**.
//!
//! Each gets its **own** Lua method table, consulted by the frame `__index` dispatcher only for its
//! own [`FrameKind`](crate::widget::FrameKind) — so a duck-typing addon (`if f.SetInsertMode then`)
//! sees `nil` everywhere else, exactly as against the client's per-class registrar tables.
//!
//! What *is* shared sits here: both classes display a stack of [`MessageLine`] records, so the
//! wrapped-row measure round-trip, the band emit, and the font/justification read are one code path
//! over [`KindState::message_lines`]. Everything that actually differs stays on the two states.

use crate::layout::Rect;
use crate::order::ZTarget;
use crate::widget::{FrameHandle, InsertMode, KindState, MessageLine};

use super::layout::FramePaint;
use super::{
    ExtractedQuad, FontShadow, JustifyH, JustifyV, LineMeasureRequest, Model, Outline, QuadContent,
    UiScript,
};

mod plain;
mod scrolling;

pub(super) use plain::REG_MESSAGEFRAME_METHODS;
pub(super) use scrolling::REG_SCROLLINGMESSAGEFRAME_METHODS;

/// Install both classes' method tables (and the chat input globals the scrolling one carries).
/// **`AddMessage`'s text argument, and the three ways it silently swallows the whole call.**
/// `None` means *do nothing at all* — no line, no record, no error.
///
/// `CSimpleMessageFrame::AddMessage 0x795590` fetches Lua index 2 through `lua_isstring 0x6f3510`
/// and then `lua_tostring 0x6f3690`, and **three** results jump to the function's own epilogue at
/// `0x79582b` (`pop edi / pop esi / xor eax,eax / pop ebx / leave / ret` — zero Lua values, no
/// push, and the virtual enqueue at `0x795825 call [eax+0x90]` never reached):
///
/// ```text
///   79561b  mov edx,2
///   795625  call 0x6f3510      ; lua_isstring — tags 4 (string) and 3 (number) only
///   79562c  je   0x79582b      ; nil / boolean / table / function  -> silent
///   795642  je   0x79582b      ; lua_tostring returned NULL        -> silent
///   79564b  je   0x79582b      ; cmp byte ptr [ebx],0 — EMPTY string -> silent
/// ```
///
/// `ScrollingMessageFrame:AddMessage 0x792900` is the same shape (gate `0x79299c`, `je 0x792b81` =
/// its own epilogue), so `DEFAULT_CHAT_FRAME:AddMessage(nil)` is silent too.
///
/// **This is not a general argument law and must not be read as one** (1717's rule: the shape is
/// settled per binding). The *colour* arguments beside it behave the opposite way — a bad `r`/`g`/
/// `b`/`a` jumps to `0x79581c`, which still calls the enqueue with the `0xffffffff` opaque white
/// staged at `0x795658`. Only a bad **text** shows nothing. The three *receiver* guards
/// (`0x847ef8` "used '.' instead of ':'", `0x847ec0`, `0x847e98`) do still raise, and they are
/// upstream of this.
///
/// **Why it is load-bearing rather than pedantry.** Stock FrameXML takes this path: 1.12's
/// `ContainerFrame.lua:753` reports a full keyring with
/// `UIErrorsFrame:AddMessage(NO_EMPTY_KEYRING_SLOTS, 1.0, 0.1, 0.1, 1.0)` and **no such
/// GlobalString exists** — the shipped `GlobalStrings.lua` defines only
/// `NO_EMPTY_KEYRING_SLOTS_ERROR`, a rename that updated the definition and not the use. So a
/// player dropping a key onto a full keyring gets a dead click on a real client. Taking a `String`
/// here (which is what this did until 1751's fourth window swapped the bag bar in) raised instead,
/// putting a script-error dialog on an ordinary player gesture that the reference never shows.
///
/// A **number** is accepted and rendered as its decimal text — `0x6f7c80` retags the slot in
/// place — which is exactly [`super::binding_abi::optional_string`]'s coercion.
pub(super) fn message_text(lua: &mlua::Lua, v: &mlua::Value) -> Option<String> {
    super::binding_abi::optional_string(lua, v).filter(|s| !s.is_empty())
}

/// Install `SetJustifyH`/`GetJustifyH`/`SetJustifyV`/`GetJustifyV` on a message-frame method
/// table. Both classes carry all four (see [`crate::widget::ScrollingMessageState::justify`] for
/// the method-table bytes that settled it), and the law they obey is
/// [`crate::justify`]'s — shared with FontString, `<Font>` and EditBox since 1237, so the two
/// verified traps cannot drift apart per widget:
///
///  · an **unrecognised token RAISES** `Usage: %s:SetJustifyH("justify")` rather than defaulting;
///  · a token from the **other axis** parses and then masks to nothing — `SetJustifyH("TOP")`
///    CLEARS the horizontal axis, and `GetJustifyH()` answers `"UNKNOWN"` while the glyphs keep
///    drawing at the paint fallback.
///
/// `GetJustify*` reports the EFFECTIVE value — the frame's own once set, else the font object's —
/// so an untouched frame answers what it actually draws with rather than a ctor constant its
/// `<FontString>` may have overridden.
use crate::script::object::frame_handle_of;

pub(super) fn install_justify(
    lua: &mlua::Lua,
    m: &mlua::Table,
    widget: &'static str,
) -> mlua::Result<()> {
    use crate::justify::{self, Justify};

    fn slot(ks: &mut KindState) -> Option<&mut Option<Justify>> {
        match ks {
            KindState::ScrollingMessage(s) => Some(&mut s.justify),
            KindState::Message(s) => Some(&mut s.justify),
            _ => None,
        }
    }
    fn own(ks: &KindState) -> Option<Justify> {
        match ks {
            KindState::ScrollingMessage(s) => s.justify,
            KindState::Message(s) => s.justify,
            _ => None,
        }
    }

    for (name, mask, horizontal) in [
        ("SetJustifyH", justify::H_MASK, true),
        ("SetJustifyV", justify::V_MASK, false),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, (this, token): (mlua::Table, mlua::Value)| {
                let parsed = token
                    .as_string()
                    .and_then(|t| t.to_str().ok().and_then(|t| justify::parse_bits(&t)))
                    .ok_or_else(|| {
                        if horizontal {
                            justify::usage_h(widget)
                        } else {
                            justify::usage_v(widget)
                        }
                    })?;
                let h = frame_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                let frame = model
                    .arena
                    .frame_mut(h)
                    .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
                let cur = own(&frame.kind_state).unwrap_or_default();
                let next = Justify(justify::set_axis(cur.0, mask, parsed));
                *slot(&mut frame.kind_state)
                    .ok_or_else(|| mlua::Error::runtime("not a message frame"))? = Some(next);
                Ok(())
            })?,
        )?;
    }

    // The two getters differ ONLY in what an untouched frame falls back to, and the asymmetry is
    // real rather than an oversight: horizontally there IS another source — the font object the
    // lines actually draw with — so reporting anything else would have `GetJustifyH` disagree with
    // the glyphs on our own chat window, which declares `justifyH="LEFT"` on its `<FontString>`.
    // Vertically there is no such source: `emit_message_lines` pins each line to the top of its own
    // band deliberately (the band, not a FontString rect, owns the vertical rhythm), so an untouched
    // frame reports the ctor default rather than a value invented from our band math.
    m.set(
        "GetJustifyH",
        lua.create_function(move |lua, this: mlua::Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let bits = match model.arena.frame(h).and_then(|f| own(&f.kind_state)) {
                Some(j) => j.0,
                None => {
                    let font = UiScript::message_frame_font(&model, h);
                    justify::parse_bits(justify::name_h(font.justify_h)).unwrap_or(0)
                }
            };
            Ok(justify::name_of(bits, justify::H_MASK).to_string())
        })?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(move |lua, this: mlua::Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let j = model
                .arena
                .frame(h)
                .and_then(|f| own(&f.kind_state))
                .unwrap_or_default();
            Ok(justify::name_of(j.0, justify::V_MASK).to_string())
        })?,
    )?;
    Ok(())
}

pub(super) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    scrolling::install(lua)?;
    plain::install(lua)
}

/// The font a message frame's lines draw with — its declared `<FontString>` child, which is exactly
/// how both reference frames spell it (`<FontString inherits="ChatFontNormal" justifyH="LEFT"/>` on
/// a chat frame, `<FontString inherits="ErrorFont" justifyH="CENTER"/>` on `UIErrorsFrame.xml`).
/// Shared by the emit and the measure round-trip so bands, measures and glyphs agree. Missing ⇒ the
/// renderer's default face at the ~14px chat default.
#[derive(Clone)]
pub(super) struct MessageFont {
    pub(super) path: Option<String>,
    pub(super) height: Option<f32>,
    pub(super) shadow: Option<FontShadow>,
    pub(super) outline: Outline,
    /// The child's `justifyH`. **Load-bearing, not decoration**: it is the only reason the
    /// reference's error toasts sit centred in their 512-wide frame while chat lines run flush
    /// left, and the class draws its lines through that font instance. A frame with no declared
    /// FontString falls back to LEFT (the chat shape), never to the FontString default of CENTER —
    /// there is no region here whose own default could apply.
    pub(super) justify_h: JustifyH,
}

/// The FontString region a message frame's **style** lives on — found, or created on demand.
///
/// Both classes draw their lines through one font instance, and [`UiScript::message_frame_font`]
/// reads it off the frame's first `FontString` region. That region normally comes from the
/// declared direct-child `<FontString>` (LoadXML's "special" font string — our ChatFrame has one).
/// A frame built by `CreateFrame("MessageFrame")` has none, so the ten shared font-block verbs
/// need somewhere to write: this is that somewhere, and it is created **only** when one of them is
/// actually called, so a frame nobody styles never grows a region.
///
/// **It is seated LEFT deliberately.** A frame with no declared FontString falls back to LEFT (the
/// chat shape) per [`MessageFont::justify_h`]'s law — but a freshly created `RegionData` defaults
/// to the *FontString* default of CENTER. Creating the region at the default would silently
/// re-justify a frame the moment an addon touched its font, which is a visible change nobody
/// asked for.
pub(super) fn ensure_font_region(
    lua: &mlua::Lua,
    fh: FrameHandle,
) -> Option<crate::widget::RegionHandle> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let existing = model.arena.frame(fh).and_then(|frame| {
        frame
            .regions
            .iter()
            .find(|&&rh| {
                matches!(
                    model.arena.region(rh).map(|r| r.kind),
                    Some(crate::widget::RegionKind::FontString)
                )
            })
            .copied()
    });
    if existing.is_some() {
        return existing;
    }
    let rh = model.arena.create_region(
        fh,
        crate::widget::RegionKind::FontString,
        crate::order::DrawLayer::Overlay,
        0,
    )?;
    let mut data = crate::script::RegionData::default();
    data.justify.set_h(JustifyH::Left);
    model.region_data.insert(rh, data);
    model.touch_layout();
    Some(rh)
}

impl UiScript {
    pub(super) fn message_frame_font(model: &Model, fh: FrameHandle) -> MessageFont {
        model
            .arena
            .frame(fh)
            .and_then(|frame| {
                frame
                    .regions
                    .iter()
                    .find(|&&rh| {
                        matches!(
                            model.arena.region(rh).map(|r| r.kind),
                            Some(crate::widget::RegionKind::FontString)
                        )
                    })
                    .and_then(|rh| model.region_data.get(rh))
                    .map(|d| MessageFont {
                        path: d.font_path.clone(),
                        height: d.font_height,
                        shadow: d.font_shadow,
                        outline: d.outline,
                        justify_h: d.justify.paint_h(),
                    })
            })
            .unwrap_or(MessageFont {
                path: None,
                height: None,
                shadow: None,
                outline: Outline::default(),
                justify_h: JustifyH::Left,
            })
    }

    /// How many display **rows** frame `fh`'s resolved rect holds at its line font's pitch — the
    /// client's layout-derived `numLinesDisplayed`. `0` when the frame has no resolved rect.
    ///
    /// Both classes need it and neither can compute it from its own state: it is the page size for
    /// the scrolling class's `PageUp`/`PageDown` and the whole capacity law for the plain one.
    pub(super) fn message_viewport_rows(model: &Model, fh: FrameHandle) -> usize {
        let pitch = Self::message_frame_font(model, fh)
            .height
            .unwrap_or(14.0)
            .max(1.0);
        model.resolved.get(&fh).map_or(0, |r| {
            ((r.top - r.bottom) / pitch).floor().max(0.0) as usize
        })
    }

    /// Message lines whose wrapped **row count** needs a host measurement (a new line, or the
    /// frame's width/font changed under it) — the message-frame half of the measure round-trip.
    /// Call after [`UiScript::resolve`] (widths must be resolved); answer with
    /// [`UiScript::set_message_line_rows`] before extract. Cache keys keep this empty on quiet
    /// frames.
    pub fn message_lines_needing_measure(&mut self) -> Vec<LineMeasureRequest> {
        use std::hash::{Hash, Hasher};
        let mut model = self.model_mut();
        // A plain reborrow so the arena borrow inside the loop and the sweep-token field writes
        // split by FIELD instead of fighting over the RefMut.
        let model = &mut *model;
        let mut out = Vec::new();
        let frames: Vec<(FrameHandle, Rect)> = model
            .resolved
            .iter()
            .filter(|(&fh, _)| {
                model
                    .arena
                    .frame(fh)
                    .is_some_and(|f| f.kind_state.message_lines().is_some() && f.effective_visible)
            })
            .map(|(&fh, &fr)| (fh, fr))
            .collect();
        for (fh, fr) in frames {
            let wrap_width = fr.right - fr.left;
            if wrap_width <= 1.0 {
                continue; // unresolved/degenerate width — nothing meaningful to wrap against
            }
            let scale = model
                .arena
                .frame(fh)
                .map(|f| f.effective_scale)
                .unwrap_or(1.0);
            let font = Self::message_frame_font(model, fh);
            let frame_id = model.frame_id(fh);
            let Some(frame) = model.arena.frame(fh) else {
                continue;
            };
            let Some(lines) = frame.kind_state.message_lines() else {
                continue;
            };
            // The skip token (the W4 fix, 1410's lane one door over): a frame whose line set
            // (lines_gen) and measure environment (font/wrap/scale/outline) both match its last
            // CLEAN sweep can produce nothing — every line's rows_key was minted under exactly
            // these inputs and matched then. Environment changes (a resize, a font swap, a
            // SetScale) miss the env hash; text changes miss the generation. The token is only
            // stored by a zero-request sweep (below), so an unanswered request keeps
            // re-requesting — the same rule the region ledger runs under.
            let lines_gen = frame.kind_state.lines_gen().unwrap_or(0);
            let env = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                font.path.hash(&mut h);
                font.height.map(f32::to_bits).hash(&mut h);
                wrap_width.to_bits().hash(&mut h);
                (font.outline as u8).hash(&mut h);
                scale.to_bits().hash(&mut h);
                h.finish()
            };
            if model.msg_swept.get(&fh) == Some(&(lines_gen, env)) {
                continue;
            }
            let requests_before = out.len();
            for (index, line) in lines.iter().enumerate() {
                model.msg_lines_hashed += 1;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                line.text.hash(&mut hasher);
                font.path.hash(&mut hasher);
                font.height.map(f32::to_bits).hash(&mut hasher);
                wrap_width.to_bits().hash(&mut hasher);
                (font.outline as u8).hash(&mut hasher);
                scale.to_bits().hash(&mut hasher);
                let key = hasher.finish();
                if line.rows_key == key {
                    continue;
                }
                out.push(LineMeasureRequest {
                    frame: frame_id,
                    index: index as u32,
                    font: font.path.clone(),
                    height: font.height,
                    wrap_width,
                    outline: font.outline,
                    scale,
                    text: line.text.clone(),
                    key,
                });
            }
            if out.len() == requests_before {
                model.msg_swept.insert(fh, (lines_gen, env));
            } else {
                model.msg_swept.remove(&fh);
            }
        }
        out
    }

    /// Store host row-count answers for [`LineMeasureRequest`]s (`(frame, index, rows, key)` —
    /// frame/index/key verbatim from the request). The key is stored beside the rows, so a line
    /// whose width/font changed again since the request simply re-requests next frame.
    pub fn set_message_line_rows(&mut self, rows: &[(u32, u32, u16, u64)]) {
        let mut model = self.model_mut();
        for &(frame_id, index, n, key) in rows {
            let Some(&fh) = model.id_to_frame.get(&frame_id) else {
                continue;
            };
            let Some(frame) = model.arena.frame_mut(fh) else {
                continue;
            };
            let Some(lines) = frame.kind_state.message_lines_mut() else {
                continue;
            };
            if let Some(line) = lines.get_mut(index as usize) {
                line.rows = n.max(1);
                line.rows_key = key;
            }
        }
    }

    /// Push one [`QuadContent::Text`] per visible message of either message-frame class (no-op for
    /// any other kind). Each message occupies `rows × pitch` (its host-measured wrapped row count —
    /// the message-line measure round-trip), so a long line pushes its neighbours by its real
    /// height. The pitch is the **font height itself** — the client's own line-step law
    /// (`LayoutLines` 0x5cdc20: step = px(size) + spacing, spacing 0), the same law the app's text
    /// renderer lays wrapped rows at, so band grid and glyph rows coincide by construction. (The
    /// msgframe's own relayout `0x788750/0x788c00` is not fully traced — if the look pass ever
    /// shows the ref spacing chat lines wider than the font height, that residual is the place to
    /// pin.)
    ///
    /// **Which edge the stack grows from is the one place the two classes diverge visibly.** A
    /// ScrollingMessageFrame always stacks bottom-up with `scroll_offset` picking which message
    /// sits on the bottom row. A MessageFrame has no scrollback and instead honours `insertMode`:
    /// BOTTOM (the ctor default) is the same bottom-up stack, TOP hangs the newest message off the
    /// frame's top edge with older ones stepping *down* — the shape `UIErrorsFrame.xml:4` asks for,
    /// and the one every corpus `SetInsertMode` caller asks for. The growth *anchor* per mode is
    /// not settled at the bytes, so this is the INFERRED half: it is the reading that reproduces
    /// both shipped usages, and the place to
    /// re-pin if a reference A/B ever disagrees.
    ///
    /// A message that only partially fits at the far edge still draws — clipped to the frame rect,
    /// so its inner wrapped rows show, never inking outside the frame. A fully-faded scrolling line
    /// draws nothing but still holds its rows (the reference's chat never re-packs as old lines
    /// fade); a faded MessageFrame line is gone from the state entirely by then, because that class
    /// frees its lines instead of holding slots.
    pub(super) fn emit_message_lines(
        model: &Model,
        fh: FrameHandle,
        fr: Rect,
        z: u64,
        paint: FramePaint,
        clip: Option<Rect>,
        out: &mut Vec<ExtractedQuad>,
    ) {
        let Some(frame) = model.arena.frame(fh) else {
            return;
        };
        // (lines, index of the message on the anchored row, which edge it hangs off).
        let (lines, top_index, from_top): (&std::collections::VecDeque<MessageLine>, usize, bool) =
            match &frame.kind_state {
                KindState::ScrollingMessage(smf) => (
                    &smf.lines,
                    smf.lines.len().saturating_sub(1 + smf.scroll_offset),
                    false,
                ),
                KindState::Message(mf) => (
                    &mf.lines,
                    mf.lines.len().saturating_sub(1),
                    matches!(mf.insert_mode, InsertMode::Top),
                ),
                _ => return,
            };
        if lines.is_empty() {
            return;
        }
        let font = Self::message_frame_font(model, fh);
        let own_justify = match &frame.kind_state {
            KindState::ScrollingMessage(s) => s.justify,
            KindState::Message(s) => s.justify,
            _ => None,
        };
        // The band grid lives in the frame's RESOLVED (scale-multiplied) rect, so the row pitch
        // rides the frame scale exactly like the glyphs it must coincide with (the font height
        // itself is frame-local).
        let pitch = font.height.unwrap_or(14.0) * paint.scale;
        if pitch <= 0.0 || fr.top <= fr.bottom {
            return;
        }
        // Message text never inks outside its frame: a partially-fitting outermost message is
        // scissored at the frame edge (intersected with any ScrollFrame ancestor clip).
        let line_clip = Some(match clip {
            Some(c) => super::clip::intersect_rect(c, fr),
            None => fr,
        });
        let mut used = 0.0f32; // rows already consumed, in px in from the anchored edge
        for idx in (0..=top_index).rev() {
            if used >= fr.top - fr.bottom {
                break; // the next band would start outside the frame — nothing more can show
            }
            let line = &lines[idx];
            let band_h = f32::from(line.rows.max(1)) * pitch;
            let (bottom, top) = if from_top {
                (fr.top - used - band_h, fr.top - used)
            } else {
                (fr.bottom + used, fr.bottom + used + band_h)
            };
            used += band_h;
            if line.alpha <= 0.0 {
                continue; // fully faded — holds its place, draws nothing
            }
            out.push(ExtractedQuad {
                target: ZTarget::Frame(fh),
                z,
                rect: Some(Rect::new(bottom, fr.left, top, fr.right)),
                alpha: paint.alpha,
                clip: line_clip,
                content: QuadContent::Text {
                    text: Some(line.text.clone()),
                    color: Some([
                        f32::from(line.color[0]) / 255.0,
                        f32::from(line.color[1]) / 255.0,
                        f32::from(line.color[2]) / 255.0,
                        line.alpha,
                    ]),
                    // The frame's own justify wins once SetJustifyH has been called; until then
                    // the font object's governs, which is what `justifyH=` on the frame's
                    // `<FontString>` set and what this drew with before the verbs existed.
                    justify_h: match own_justify {
                        Some(j) => j.paint_h(),
                        None => font.justify_h,
                    },
                    // The band is our own per-message construct (rows × pitch tall, not a
                    // FontString rect) — keep the block at its top so the band math, not MIDDLE
                    // centering, owns the vertical rhythm; the band height equals the wrapped
                    // block height exactly, so Top means "fills the band".
                    justify_v: JustifyV::Top,
                    font: font.path.clone(),
                    // The font object's own drop shadow (ChatFontNormal's `(1,-1)` black) — what
                    // makes the lines read against the world behind the transparent frame.
                    shadow: font.shadow,
                    font_height: font.height,
                    text_height: None, // message lines are never SetTextHeight'd
                    outline: font.outline,
                    alpha_gradient: None,
                    world_seat: false, // a chat frame is interface, on the interface's grid
                },
                scale: paint.scale,
            });
        }
    }
}
