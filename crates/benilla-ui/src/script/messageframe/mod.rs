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
use crate::widget::{FrameHandle, InsertMode, KindState, MessageLine, Viewport};

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
    let viewport = UiScript::message_viewport(model, h);
    let layout_key = UiScript::message_layout_key(model, h);
    match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Message(mf)) => {
            mf.trim_to_viewport(rows);
            mf.tick(elapsed);
        }
        Some(KindState::ScrollingMessage(smf)) => {
            // A resize or a font change relayouts at the current position (`0x788427`,
            // `0x788086`), as a show does (`0x788451`, [`on_visibility`]).
            if smf.laid.replace(layout_key) != Some(layout_key) {
                smf.relayout(viewport);
            }
            smf.tick(elapsed, viewport)
        }
        _ => {}
    }
}

/// A scrolling frame's show relayouts (`0x788451`): forget what it laid out against, so its next
/// update does.
pub(super) fn on_visibility(lua: &mlua::Lua, h: FrameHandle, visible: bool) {
    if !visible {
        return;
    }
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if let Some(KindState::ScrollingMessage(smf)) =
        model.arena.frame_mut(h).map(|f| &mut f.kind_state)
    {
        smf.laid = None;
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

/// The text a line is drawn and measured as. `SetText`'s expand flag swaps the number separators
/// while `SetEuropeanNumbers` holds: a MessageFrame does it when the line is added (`0x7863fe`,
/// [`Model::localized_text`] at the verb), a ScrollingMessageFrame each time it shows the line
/// (`ShowLine`, `0x788b3c`), from the raw text it keeps, so a line reads the switch as it stands.
pub(super) fn shown_text<'a>(
    european_numbers: bool,
    kind: &KindState,
    text: &'a str,
) -> std::borrow::Cow<'a, str> {
    if european_numbers && matches!(kind, KindState::ScrollingMessage(_)) {
        std::borrow::Cow::Owned(crate::strings::swap_number_separators(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
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

    /// The gap between the rows of one message: each line string inherits the frame's font
    /// (`0x7888fd`) through the clamping setter (`0x7708fd` → `0x772240`), and the block height
    /// forms the spacing times the scale (`0x772a07`-`0x772a0d`), which `GxuFontGetTextBlockHeight`
    /// rounds up to a whole pixel (`0x5c2143`-`0x5c215c`, `+0.99999`).
    fn row_gap(&self, scale: f32) -> f32 {
        snap_px(self.spacing.max(0.0) * scale)
    }

    /// The scrolling class's gap between one message and the next: the stack snaps the unscaled
    /// spacing (`0x78899e` → `0x766750`) and anchors the next line at that offset in frame units
    /// (`0x788954`-`0x788975`), so on screen it is the snapped spacing times the scale.
    fn message_gap(&self, scale: f32) -> f32 {
        snap_px(self.spacing.max(0.0)) * scale
    }

    /// What the scrolling class's layout fits messages into: a frame `height` tall, drawn at
    /// `scale`.
    fn viewport(&self, height: f32, scale: f32) -> Viewport {
        Viewport {
            height,
            line_h: self.height_or_default() * scale,
            row_gap: self.row_gap(scale),
            message_gap: self.message_gap(scale),
        }
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

    /// How many rows the plain frame `fh`'s resolved rect holds at its font's pitch, its capacity
    /// (`+0x318`). Both the rect and the pitch are scaled (`0x785f0a`, `0x785f1f`); a pitch of 0
    /// or less, which a negative spacing can make, holds nothing (`0x785f8f`).
    pub(super) fn message_viewport_rows(model: &Model, fh: FrameHandle) -> usize {
        let font = Self::message_frame_font(model, fh);
        let Some(frame) = model.arena.frame(fh) else {
            return 0;
        };
        let pitch = font.plain_pitch(frame.effective_scale);
        if pitch <= 0.0 {
            return 0;
        }
        model.resolved.get(&fh).map_or(0, |r| {
            ((r.top - r.bottom) / pitch).floor().max(0.0) as usize
        })
    }

    /// The viewport of the scrolling frame `fh`'s resolved rect. The relayout divides the rect by
    /// the scale (`0x7887d4`) and the heights it spends are in the same units, so this is the same
    /// quantity scaled; no rect yet is no height, which still shows the anchored message.
    pub(super) fn message_viewport(model: &Model, fh: FrameHandle) -> Viewport {
        let font = Self::message_frame_font(model, fh);
        let scale = model.arena.frame(fh).map_or(1.0, |f| f.effective_scale);
        let height = model
            .resolved
            .get(&fh)
            .map_or(0.0, |r| (r.top - r.bottom).max(0.0));
        font.viewport(height, scale)
    }

    /// What the scrolling frame `fh`'s relayout lays out against: its size and its font.
    pub(super) fn message_layout_key(model: &Model, fh: FrameHandle) -> u64 {
        use std::hash::{Hash, Hasher};
        let font = Self::message_frame_font(model, fh);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(r) = model.resolved.get(&fh) {
            (r.right - r.left).to_bits().hash(&mut hasher);
            (r.top - r.bottom).to_bits().hash(&mut hasher);
        }
        font.path.hash(&mut hasher);
        font.height.map(f32::to_bits).hash(&mut hasher);
        font.spacing.to_bits().hash(&mut hasher);
        (font.outline as u8).hash(&mut hasher);
        hasher.finish()
    }

    /// Message lines whose wrapped row count needs a host measure. Call after
    /// [`UiScript::resolve`]; answer with [`UiScript::set_message_line_rows`] before extract.
    pub fn message_lines_needing_measure(&mut self) -> Vec<LineMeasureRequest> {
        use std::hash::{Hash, Hasher};
        let mut model = self.model_mut();
        // A plain reborrow, so the arena reads and the sweep-token writes borrow disjoint fields.
        let model = &mut *model;
        let mut out = Vec::new();
        let european = model.european_numbers;
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
                european.hash(&mut h);
                h.finish()
            };
            if model.msg_swept.get(&fh) == Some(&(lines_gen, env)) {
                continue;
            }
            let requests_before = out.len();
            for (index, line) in lines.iter().enumerate() {
                model.msg_lines_hashed += 1;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                let text = shown_text(european, &frame.kind_state, &line.text);
                text.hash(&mut hasher);
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
                    text: text.into_owned(),
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
    /// reading reproduces both stock uses. The scrolling class draws the messages its layout
    /// places ([`crate::widget::ScrollingMessageState::displayed_count`]): the anchored one, then
    /// only those that fit whole. A faded one keeps its rows, as the reference never re-packs
    /// chat. The plain class draws its bands as far as the frame reaches.
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
        let viewport = font.viewport(fr.top - fr.bottom, paint.scale);
        // A message's advance to the next: the scrolling class's block and snapped gap, the plain
        // class's own pitch per row.
        let advance = |rows: u16| match &frame.kind_state {
            KindState::Message(_) => f32::from(rows.max(1)) * font.plain_pitch(paint.scale),
            _ => viewport.cost(rows),
        };
        if line_h <= 0.0 || advance(1) <= 0.0 || fr.top <= fr.bottom {
            return;
        }
        // Clip to the frame, intersected with any ScrollFrame ancestor's clip.
        let line_clip = Some(match clip {
            Some(c) => super::clip::intersect_rect(c, fr),
            None => fr,
        });
        let mut used = 0.0f32; // px used from the anchored edge

        // The scrolling class's lines go out from the farthest displayed line to the anchored
        // one, the order its link rebuild walks them (`0x788c8d`-`0x788d7d`), so the spans reach
        // [`super::hyperlink`] in the reference's placement order.
        let first = out.len();
        let (reverse, shown) = match &frame.kind_state {
            KindState::ScrollingMessage(smf) => (true, smf.displayed_count(viewport)),
            _ => (false, usize::MAX),
        };
        for idx in (0..=top_index).rev().take(shown) {
            if !reverse && used >= fr.top - fr.bottom {
                break; // the next band starts outside the frame
            }
            let line = &lines[idx];
            let band_h = viewport.block(line.rows);
            let (bottom, top) = if from_top {
                (fr.top - used - band_h, fr.top - used)
            } else {
                (fr.bottom + used, fr.bottom + used + band_h)
            };
            used += advance(line.rows);
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
                    text: Some(
                        shown_text(model.european_numbers, &frame.kind_state, &line.text)
                            .into_owned(),
                    ),
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
                    edit_line: false,
                    // A line is a FontString with the flag off and no `maxLines` (`0x7865f0`).
                    non_space_wrap: false,
                    max_lines: 0,
                },
                scale: paint.scale,
            });
        }
        if reverse {
            out[first..].reverse();
        }
    }
}
