//! The text region's rect and a multi-line box's own height: `0x77b8c0` seats the FontString by
//! the insets, and the flush's relayout `0x77d4d0` sizes a multi-line box to its text.

use mlua::Lua;

use crate::layout::{Anchor, Point};
use crate::script::{MeasureRequest, Model};
use crate::widget::{FrameHandle, KindState, RegionHandle};

/// `0x77b8c0`: the text FontString `TOPLEFT` at `(left, −top)` on the box. Multi-line, it gets
/// the explicit width `boxW − (right + left)` from the box's resolved rect (`0x77b8df`), which
/// waits while the box has none (`0x77b8c7`), and height 0, so it spans its measured text.
/// Single-line, the two inset corners pin the width and the height `boxH − (top + bottom)` it
/// writes, so no explicit size is kept.
pub(super) fn seat_text_region(model: &mut Model, h: FrameHandle) {
    let Some(KindState::EditBox(eb)) = model.arena.frame(h).map(|f| &f.kind_state) else {
        return;
    };
    let (Some(rh), [l, r, t, b], multi_line) = (eb.text_region, eb.text_insets, eb.multi_line)
    else {
        return;
    };
    let owner = model.frame_id(h);
    let top_left = Anchor::new(Point::TopLeft, owner, Point::TopLeft, l, -t);
    let (anchors, size) = if multi_line {
        let width = multi_line_text_width(model, h, [l, r]);
        (vec![top_left], width.map(|w| Some((w, 0.0))))
    } else {
        let bottom_right = Anchor::new(Point::BottomRight, owner, Point::BottomRight, -r, b);
        (vec![top_left, bottom_right], Some(None))
    };
    let data = model.region_data.entry(rh).or_default();
    let same_anchors = data.anchors.len() == anchors.len()
        && data
            .anchors
            .iter()
            .zip(&anchors)
            .all(|(a, b)| crate::script::object::anchor_bits_eq(a, b));
    // `None` keeps the size: a multi-line box with no rect yet.
    let size = size.unwrap_or(data.size);
    let size_moved = !crate::script::region::size_bits_eq(data.size, size);
    if !same_anchors {
        data.anchors = anchors;
    }
    data.size = size;
    if !same_anchors {
        model.touch_layout();
    } else if size_moved {
        model.touch_layout_region(rh);
    }
    if size_moved {
        // The width is the wrap width, which keys the measure.
        model.touch_measure(rh);
    }
}

/// A multi-line box's text width in its own units, `boxW − (right + left)`, from the rect the
/// last resolve gave it; `None` before it has one.
fn multi_line_text_width(model: &Model, h: FrameHandle, [l, r]: [f32; 2]) -> Option<f32> {
    let rect = model.resolved.get(&h)?;
    Some(rect.width() / crate::script::object::eff_scale(model, h) - (r + l))
}

/// The flush's relayout tail (`0x77d4d0` @`0x77d863`–`0x77d8ad`) for every shown multi-line box,
/// run from the tick ahead of the `OnTextChanged` drain, as the flush relayouts before it fires.
/// The box's own height becomes `(insetTop + insetBottom) + h`, `h` its text's measured wrapped
/// height (`0x7729b0`), or one line's (`0x7727b0(fs, 1)`) when that is exactly 0, as for empty
/// text. It reads neither the authored height nor the rect, so the box grows and shrinks with its
/// text and a `SetHeight` lasts until the next tick; a single-line box is never sized. The box's
/// resize re-seats its text (`0x77a8d0` → `0x77b8c0`), done here when the box's width has moved.
///
/// Each tick relayouts every shown multi-line box, where the reference relayouts a box whose dirty
/// bit 0 is set; every input of the height sets it (an edit, a resize, the insets, the font,
/// `SetMultiLine`, the ctor), so both write the same height.
pub(in crate::script) fn relayout_multi_line(lua: &Lua) {
    let boxes: Vec<FrameHandle> = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        // The flush runs from the box's own update, pumped for shown frames only.
        model
            .arena
            .editbox_kinds()
            .iter()
            .copied()
            .filter(|&h| {
                model.arena.frame(h).is_some_and(|f| {
                    f.effective_visible
                        && matches!(&f.kind_state, KindState::EditBox(eb) if eb.multi_line)
                })
            })
            .collect()
    };
    for h in boxes {
        let Some(rh) = super::ensure_text_region(lua, h) else {
            continue;
        };
        {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let insets = match model.arena.frame(h).map(|f| &f.kind_state) {
                Some(KindState::EditBox(eb)) => eb.text_insets,
                _ => continue,
            };
            let seated = model.region_data.get(&rh).and_then(|d| d.size);
            let width = multi_line_text_width(&model, h, [insets[0], insets[1]]);
            let moved = width.is_some_and(|w| seated.is_none_or(|s| s.0.to_bits() != w.to_bits()));
            if moved {
                seat_text_region(&mut model, h);
            }
        }
        // `0x7729b0` measures in the call; with no engine installed the measure lands a tick later.
        crate::script::measure::ensure_measured(lua, rh);
        let Some(text_h) = text_height(lua, h, rh) else {
            continue; // the measure is pending: the height waits for it
        };
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let [_, _, top, bottom] = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::EditBox(eb)) => eb.text_insets,
            _ => continue,
        };
        // `fld top; fadd bottom; fadd st, st(1)`.
        let height = (top + bottom) + text_h;
        let input = model.layout_inputs.entry(h).or_default();
        if input.height.to_bits() != height.to_bits() {
            input.height = height;
            model.touch_layout_frame(h);
        }
    }
}

/// The text's height for the box: its measure under the current key, or one line's when that is
/// exactly 0.0 (`0x77d877` `fcom 0.0` → `0x77d88e`), as it is for empty text, which is never
/// measured (`0x7729ff`). `None` while either measure is pending.
fn text_height(lua: &Lua, h: FrameHandle, rh: RegionHandle) -> Option<f32> {
    let measured = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let d = model.region_data.get(&rh)?;
        if d.text.as_deref().is_none_or(str::is_empty) {
            0.0
        } else {
            let key = d.measure_key(owner_scale(&model, h));
            d.measured.filter(|m| m.key == key)?.h
        }
    };
    if measured == 0.0 {
        line_height(lua, h, rh)
    } else {
        Some(measured)
    }
}

/// Any text that lays out as one line: its measured height is one line's.
const ONE_LINE: &str = "0";

/// `0x7727b0(fs, 1)`, one line's height in the text region's font: the installed engine's measure
/// of one line, the same measure a line of text takes, cached on the box under its key. `None`
/// with no engine installed.
fn line_height(lua: &Lua, h: FrameHandle, rh: RegionHandle) -> Option<f32> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let scale = owner_scale(&model, h);
    let req = {
        let d = model.region_data.get(&rh)?;
        let key = d.measure_key_of(ONE_LINE, scale);
        if let Some(KindState::EditBox(eb)) = model.arena.frame(h).map(|f| &f.kind_state) {
            if let Some((k, lh)) = eb.line_height {
                if k == key {
                    return Some(lh);
                }
            }
        }
        MeasureRequest {
            id: model.region_to_id.get(&rh).copied().unwrap_or(0),
            font: d.font_path.clone(),
            height: d.font_height,
            text_height: d.text_height,
            wrap_width: None,
            outline: d.outline,
            scale,
            text: ONE_LINE.to_string(),
            key,
        }
    };
    // Taken out and put back, as `ensure_measured` does: the engine cannot re-enter the VM.
    let mut engine = model.measurer.take()?;
    let (_, lh, _) = engine.measure(&req);
    model.measurer = Some(engine);
    if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        eb.line_height = Some((req.key, lh));
    }
    Some(lh)
}

/// The scale a measure is keyed under, the owner's effective scale, as the measure loop reads it.
fn owner_scale(model: &Model, h: FrameHandle) -> f32 {
    model
        .arena
        .frame(h)
        .map(|f| f.effective_scale)
        .unwrap_or(1.0)
}
