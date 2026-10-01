//! The box's own per-frame update (`0x77a790`, vtable `0x81c910` slot `+0x38`), which the tick's
//! walk runs in the box's place, and its flush (`0x77d3e0`), which drains the dirty word.

use mlua::{Lua, Value};

use super::{event, fire_script, frame_id_of, relayout, with_eb, Model, CARET_WIDTH};
use crate::widget::{EditBoxState, FrameHandle};

/// The box's update after its Lua `OnUpdate` (`0x77a79a`): the flush (`0x77a7a1`), then the caret
/// blink (`0x77a7a6`). The walk runs it for every shown box, with an `OnUpdate` or without.
pub(in crate::script) fn update(lua: &Lua, h: FrameHandle, dt: f32) {
    flush(lua, h);
    blink(lua, h, dt);
}

/// The flush `0x77d3e0`. It samples bit 0 at entry (`0x77d3f2`); on bit 0 the relayout runs
/// (`0x77d447`) and the bit clears (`0x77d44c`); on bit 2 the caret leg runs (`0x77d475`), firing
/// `OnCursorChanged`, and only then does the bit clear (`0x77d47a`); last, `OnTextChanged` fires
/// if bit 0 was set at entry (`0x77d481`–`0x77d498`). So a handler's edit raises bits for the next
/// flush, a frame later, and never loops inside one: an `OnTextChanged` that sets its own text
/// fires once more, next frame, and a caret moved by `OnCursorChanged` is cleared with the bit.
/// Besides the walk, the box's `OnKeyDown` (`0x77b1e2`) and `OnMouseDown` (`0x77b819`) run it
/// before the key or click acts.
///
/// Deviation: the flush's promotion of bit 2 to bit 0 when the caret has left the display window
/// (`0x77d3f5`–`0x77d436`), with its IME legs (`0x77d417`–`0x77d431`), and the IME writer's
/// `or 6` (`0x77acde`, in `0x77ac60`) are not modelled: the promotion only relayouts, which
/// re-windows a single-line box whose window benilla keeps host-side, and fires nothing, since
/// bit 0 is sampled before it; benilla has no IME composition.
pub(super) fn flush(lua: &Lua, h: FrameHandle) {
    let Some((entry, owed)) = with_eb(lua, h, |eb| (eb.dirty, eb.relayout_owed)) else {
        return;
    };
    let text_changed = entry & EditBoxState::DIRTY_TEXT != 0;
    if text_changed || owed {
        let done = relayout::relayout(lua, h);
        with_eb(lua, h, |eb| {
            eb.dirty &= !EditBoxState::DIRTY_TEXT;
            eb.relayout_owed = !done;
        });
    }
    if entry & EditBoxState::DIRTY_CURSOR != 0 && caret(lua, h) {
        with_eb(lua, h, |eb| eb.dirty &= !EditBoxState::DIRTY_CURSOR);
    }
    if text_changed {
        fire_script(lua, frame_id_of(lua, h), "OnTextChanged");
    }
}

/// The caret leg `0x77da80`: `OnCursorChanged(x, y, w, h)` (`0x77de1b`, in UI units via
/// `0x77dd5f`), `x` the caret's advance along its line, `y` minus the row index times the row
/// pitch, `w` the constant 4.0 and `h` the line height. The arguments are computed only for a box
/// with the script (`0x77dd8c`), from the box's advance table, which the installed font engine
/// measures here when the text or font moved. `false` while that table is pending, with no engine
/// installed, for the next flush to retry: the host answers the focused box's a tick later.
///
/// Deviation: `h` is the row pitch the advance answer carries, where the reference passes the line
/// height (`0x7727b0`): 0 for a single-line box, whose answer has no pitch, and for a multi-line
/// box taller than the line height only for a font with extra spacing (none in the stock UI).
fn caret(lua: &Lua, h: FrameHandle) -> bool {
    let id = frame_id_of(lua, h);
    if !event::has_widget_handler(lua, id, "OnCursorChanged") {
        return true;
    }
    if !super::seam::advances_current(lua, h) {
        return false;
    }
    let args = with_eb(lua, h, |eb| {
        let display = eb.display();
        if eb.advances.len() != display.len() + 1 {
            return None;
        }
        let cursor_d = eb.text_to_display(eb.cursor).min(display.len());
        let (row, x) = if eb.multi_line {
            eb.caret_row_x(cursor_d)
        } else {
            (0, eb.advances[cursor_d])
        };
        let pitch = eb.cell_h;
        Some((x, -(row as f32) * pitch, pitch))
    })
    .flatten();
    let Some((x, y, pitch)) = args else {
        return false;
    };
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnCursorChanged",
        vec![
            Value::Number(f64::from(x)),
            Value::Number(f64::from(y)),
            Value::Number(f64::from(CARET_WIDTH)),
            Value::Number(f64::from(pitch)),
        ],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    true
}

/// The caret blink (`0x77a7a6`–`0x77a84c`): past the period (0.5 s by default) the focused box's
/// caret toggles and the accumulator resets; only a period of exactly 0.0 keeps the caret solid
/// (`0x77a7b4`).
///
/// Deviation: not gated on the cursor lying inside the scroll window (`0x77a7bd`–`0x77a7d7`),
/// which benilla's host keeps around the caret.
fn blink(lua: &Lua, h: FrameHandle, dt: f32) {
    let focused = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .focused_editbox
        == Some(h);
    if !focused {
        return;
    }
    with_eb(lua, h, |eb| {
        if eb.blink_period != 0.0 {
            eb.blink_accum += dt;
            if eb.blink_accum > eb.blink_period {
                eb.caret_shown = !eb.caret_shown;
                eb.blink_accum = 0.0;
            }
        }
    });
}
