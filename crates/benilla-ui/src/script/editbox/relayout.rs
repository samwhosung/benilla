//! The text region's rect and a multi-line box's own height: `0x77b8c0` seats the FontString by
//! the insets, and the flush's relayout `0x77d4d0` sizes a multi-line box to its text.

use mlua::Lua;

use crate::layout::{Anchor, Point, Rect};
use crate::script::{MeasureRequest, Model};
use crate::widget::{FrameHandle, KindState, RegionHandle};

/// `0x77b8c0`, the text FontString's rect. The reference writes nothing while the box's rect is
/// unresolved (`0x77b8c7`); then it clears the points (`0x77b8da`), sets the width
/// `boxW − (right + left)` for either kind (`0x77b905`), the height 0 multi-line (`0x77b91e`) or
/// `boxH − (top + bottom)` single-line (`0x77b940`), and one `TOPLEFT` point at `(left, −top)`
/// (`0x77b96b`).
///
/// Multi-line, benilla writes that shape, the width once the box has a rect: until then only the
/// point is written, and the size is kept.
///
/// Single-line, benilla pins `TOPLEFT` and `BOTTOMRIGHT` at the inset corners and keeps no
/// explicit size. That is a gap: it covers the same rect, but an explicit width here would be a
/// wrap width, since benilla's single-line region holds the whole text where the reference's holds
/// only the window's substring (`0x77d858`). A script sees the difference: `GetPoint` on the
/// region answers two points where the reference answers one, and `GetWidth`/`GetHeight` answer
/// the text's measure where the reference answers the explicit size.
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

/// The EditBox's `OnSizeChanged` override (`0x77a8d0`, vtable `0x81c910` slot `+0x48`): after the
/// base script fire (`0x77a8e0`), `0x77a8e7` re-seats the text region (`0x77b8c0`) on every resize
/// of any box. `ApplyRect` (`0x76b580`) calls it when the width or height moved by the resize
/// epsilon, the first resolve included, against the ctor's zero rect. `true` when a re-seat moved
/// a layout input, for the caller to resolve it in the same pass, as the reference's drain does.
pub(in crate::script) fn reseat_resized(model: &mut Model) -> bool {
    let epoch = model.layout_epoch;
    for i in 0..model.arena.editbox_kinds().len() {
        let h = model.arena.editbox_kinds()[i];
        let Some(&now) = model.resolved.get(&h) else {
            continue; // no rect, no `ApplyRect`
        };
        let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state)
        else {
            continue;
        };
        let before = eb
            .notified_rect
            .replace(now)
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        if !crate::layout::size_changed(before, now) {
            continue;
        }
        // The ctor built the text region; its data is seeded here if nothing has touched it.
        if super::ensure_text_region_in(model, h).is_some() {
            seat_text_region(model, h);
        }
    }
    model.layout_epoch != epoch
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
/// text and a `SetHeight` lasts until the next tick; a single-line box is never sized. The text
/// region's width is the one [`seat_text_region`] last wrote, on a resize ([`reseat_resized`]),
/// `SetTextInsets`, `GetTextInsets`, `SetMultiLine` or a font change.
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
