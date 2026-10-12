//! The two message-frame classes and the display code they share. In 1.12 they are siblings, not
//! parent and child, each with its own method table, so a method only one carries is nil on the
//! other:
//!
//! - [`scrolling`]: `CSimpleMessageScrollFrame` (ctor `0x787670`), the chat window's class.
//! - [`plain`]: `CSimpleMessageFrame` (ctor `0x785640`), `UIErrorsFrame`'s class.
//!
//! Both display a stack of [`MessageLine`] records, so the row measure, the band emit and the font
//! read are shared here.

use crate::layout::Rect;
use crate::order::ZTarget;
use crate::script::object::frame_handle_of;
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

/// A message frame's update after its `OnUpdate`, in its walk slot, so only while shown: a hidden
/// frame's lines keep their time. The plain class (`0x786200`) first drains its queued messages
/// against the lines that fit (`0x786247`), which [`MessageFrameState::trim_to_viewport`] stands
/// for, then fades (`0x7862b0`); the scrolling class (`0x788460`) only fades.
///
/// [`MessageFrameState::trim_to_viewport`]: crate::widget::MessageFrameState::trim_to_viewport
pub(super) fn update(model: &mut Model, h: FrameHandle, elapsed: f32) {
    let rows = UiScript::message_viewport_rows(model, h);
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Message(mf)) => {
            mf.trim_to_viewport(rows);
            mf.tick(elapsed);
        }
        Some(KindState::ScrollingMessage(smf)) => smf.tick(elapsed),
        _ => {}
    }
}

/// `lua_isnumber` (`0x6f34d0`) then `lua_tonumber` (`0x6f3620`): a number, or a string Lua reads
/// as one. The message-frame verbs gate every colour channel and id on it.
pub(super) fn is_number(lua: &mlua::Lua, v: &mlua::Value) -> Option<f64> {
    lua.coerce_number(v.clone()).ok().flatten()
}

/// A channel the verb has already gated with [`is_number`], as the `f32` it stores.
pub(super) fn channel(lua: &mlua::Lua, v: &mlua::Value) -> f32 {
    is_number(lua, v).map_or(0.0, |n| n as f32)
}

/// `AddMessage`'s text; `None` means the whole call does nothing, no line and no error. Both
/// bindings (`0x795590`, `0x792900`) jump to their epilogue (`0x79582b`, `0x792b81`) when
/// `lua_isstring` (`0x6f3510`) fails, `lua_tostring` (`0x6f3690`) gives NULL or the string is empty
/// (`0x79564b`). Stock `ContainerFrame.lua:753` passes the undefined `NO_EMPTY_KEYRING_SLOTS` on a
/// full keyring and gets a silent click. Only the text is gated so: a bad colour still adds the
/// line in white (`0x79581c`), and the receiver checks (`0x847ef8`) still raise. A number becomes
/// its decimal text (`0x6f7c80`).
pub(super) fn message_text(lua: &mlua::Lua, v: &mlua::Value) -> Option<String> {
    super::binding_abi::optional_string(lua, v).filter(|s| !s.is_empty())
}

/// Install `SetJustifyH`, `GetJustifyH`, `SetJustifyV` and `GetJustifyV`, which both classes carry,
/// under [`crate::justify`]'s rules: an unknown token raises the usage line, and a token of the
/// other axis clears this one, so `GetJustifyH()` then answers `"UNKNOWN"`.
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

    // On an untouched frame `GetJustifyH` answers its font's `justifyH`, what the lines draw with,
    // and `GetJustifyV` the ctor default, as each line sits at the top of its own band.
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

/// The font a message frame's lines draw with: its declared `<FontString>` child, as the stock
/// chat frames and `UIErrorsFrame.xml` declare it, else the default face at 14 px.
#[derive(Clone)]
pub(super) struct MessageFont {
    pub(super) path: Option<String>,
    pub(super) height: Option<f32>,
    pub(super) shadow: Option<FontShadow>,
    pub(super) outline: Outline,
    /// The child's `justifyH`, which centres `UIErrorsFrame`'s lines and keeps chat flush left;
    /// LEFT without a declared FontString, not the FontString default of CENTER.
    pub(super) justify_h: JustifyH,
    /// The font's spacing as set, a negative included (`+0x50`).
    pub(super) spacing: f32,
}

/// `0x766750`: a length snapped to whole pixels, here of the 768-tall root grid, the device pixel
/// of a 768-tall window: a positive one rounds up (`+0.99999`, then truncation), a negative one
/// truncates toward zero.
fn snap_px(v: f32) -> f32 {
    if v > 0.0 {
        (v + 0.999_99).trunc()
    } else {
        v.trunc()
    }
}

impl MessageFont {
    /// The font height, the default 14 when none is set.
    fn height_or_default(&self) -> f32 {
        self.height.unwrap_or(14.0)
    }

    /// The gap between a line string's rows and, in the scrolling class, between messages: each
    /// line string inherits the frame's font (`0x7888fd`) through the clamping setter (`0x7708fd`
    /// → `0x772240`), and the stack snaps its field (`0x78899e` → `0x766750`).
    fn line_gap(&self, scale: f32) -> f32 {
        snap_px(self.spacing.max(0.0) * scale)
    }

    /// The plain class's pitch, its fit and its slot step (`0x785f03`-`0x785f2a`, `0x786184`): the
    /// font's own spacing, unclamped (`0x785f06`), and the height, each snapped.
    fn plain_pitch(&self, scale: f32) -> f32 {
        snap_px(self.height_or_default() * scale) + snap_px(self.spacing * scale)
    }
}

/// The FontString region holding a message frame's font, made on the first font-block call when
/// the frame declared none. It is made LEFT, not CENTER, so styling a frame never re-justifies it.
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
    model.touch_layout_region(rh);
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
                        spacing: d.spacing,
                    })
            })
            .unwrap_or(MessageFont {
                path: None,
                height: None,
                shadow: None,
                outline: Outline::default(),
                justify_h: JustifyH::Left,
                spacing: 0.0,
            })
    }

    /// How many rows frame `fh`'s resolved rect holds at its font's pitch, the client's
    /// `numLinesDisplayed`: the scrolling class's page size and the plain class's capacity. Both
    /// the rect and the pitch are scaled (`0x785f0a`, `0x785f1f`); a pitch of 0 or less, which a
    /// negative spacing can make on the plain class, holds nothing (`0x785f8f`).
    pub(super) fn message_viewport_rows(model: &Model, fh: FrameHandle) -> usize {
        let font = Self::message_frame_font(model, fh);
        let Some(frame) = model.arena.frame(fh) else {
            return 0;
        };
        let scale = frame.effective_scale;
        let pitch = match frame.kind_state {
            KindState::Message(_) => font.plain_pitch(scale),
            _ => font.height_or_default() * scale + font.line_gap(scale),
        };
        if pitch <= 0.0 {
            return 0;
        }
        model.resolved.get(&fh).map_or(0, |r| {
            ((r.top - r.bottom) / pitch).floor().max(0.0) as usize
        })
    }

    /// Message lines whose wrapped row count needs a host measure. Call after
    /// [`UiScript::resolve`]; answer with [`UiScript::set_message_line_rows`] before extract.
    pub fn message_lines_needing_measure(&mut self) -> Vec<LineMeasureRequest> {
        use std::hash::{Hash, Hasher};
        let mut model = self.model_mut();
        // A plain reborrow, so the arena reads and the sweep-token writes borrow disjoint fields.
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
                continue; // no width to wrap against
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
            // Skip a frame whose line generation and measure environment match its last clean
            // sweep. Only a sweep with no requests stores the token, so an unanswered request
            // keeps re-requesting.
            let lines_gen = frame.kind_state.lines_gen().unwrap_or(0);
            let env = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                font.path.hash(&mut h);
                font.height.map(f32::to_bits).hash(&mut h);
                wrap_width.to_bits().hash(&mut h);
                (font.outline as u8).hash(&mut h);
                font.shadow
                    .map_or(0.0, |s| s.offset[0])
                    .to_bits()
                    .hash(&mut h);
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
                font.shadow
                    .map_or(0.0, |s| s.offset[0])
                    .to_bits()
                    .hash(&mut hasher);
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
                    shadow_x: font.shadow.map_or(0.0, |s| s.offset[0]),
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

    /// Store host row counts, `(frame, index, rows, key)` echoed from each [`LineMeasureRequest`].
    /// The key is stored beside the rows, so a line that changed since its request re-requests.
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

    /// Push one [`QuadContent::Text`] per visible message of either class, each `rows·S +
    /// (rows−1)·gap` tall, its wrapped block (`0x5c2070`). The scrolling class anchors each line
    /// string on the previous one's top at its snapped spacing (`0x788954`-`0x788975`) and charges
    /// `spacing + height` per message (`0x78899e`-`0x7889ad`); the plain class steps each row by its
    /// own pitch, the raw spacing included ([`MessageFont::plain_pitch`]). A scrolling frame stacks
    /// up from the bottom, `scroll_offset` choosing the bottom message. A MessageFrame does too for `insertMode` BOTTOM, and for TOP hangs the newest off
    /// the top edge (`UIErrorsFrame.xml:4`); the reference's anchor per mode is untraced, and this
    /// reading reproduces both stock uses. A message that partly fits draws clipped; a faded
    /// scrolling line keeps its rows, as the reference never re-packs chat.
    pub(super) fn emit_message_lines(
        model: &Model,
        fh: FrameHandle,
        fr: Rect,
        z: u128,
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
        // The resolved rect is scaled and the font is frame-local, so the height and gap scale.
        let line_h = font.height_or_default() * paint.scale;
        let gap = font.line_gap(paint.scale);
        // A row's advance: the scrolling class's height plus gap, the plain class's own pitch.
        let row_step = match &frame.kind_state {
            KindState::Message(_) => font.plain_pitch(paint.scale),
            _ => line_h + gap,
        };
        if line_h <= 0.0 || row_step <= 0.0 || fr.top <= fr.bottom {
            return;
        }
        // Clip to the frame, intersected with any ScrollFrame ancestor's clip.
        let line_clip = Some(match clip {
            Some(c) => super::clip::intersect_rect(c, fr),
            None => fr,
        });
        let mut used = 0.0f32; // px used from the anchored edge
        for idx in (0..=top_index).rev() {
            if used >= fr.top - fr.bottom {
                break; // the next band starts outside the frame
            }
            let line = &lines[idx];
            let rows = f32::from(line.rows.max(1));
            let band_h = rows * line_h + (rows - 1.0) * gap;
            let (bottom, top) = if from_top {
                (fr.top - used - band_h, fr.top - used)
            } else {
                (fr.bottom + used, fr.bottom + used + band_h)
            };
            used += rows * row_step;
            if line.alpha <= 0.0 {
                continue; // faded: holds its rows, draws nothing
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
                    // The frame's own justify once set, else its font's.
                    justify_h: match own_justify {
                        Some(j) => j.paint_h(),
                        None => font.justify_h,
                    },
                    // The band is exactly the wrapped block's height, so Top fills it.
                    justify_v: JustifyV::Top,
                    font: font.path.clone(),
                    // The font's own shadow, ChatFontNormal's black at `(1,-1)`.
                    shadow: font.shadow,
                    font_height: font.height,
                    text_height: None, // message lines have no SetTextHeight
                    outline: font.outline,
                    // The line string's own field, clamped; the host snaps it.
                    spacing: font.spacing.max(0.0),
                    alpha_gradient: None,
                    world_seat: false, // interface, not world
                },
                scale: paint.scale,
            });
        }
    }
}
