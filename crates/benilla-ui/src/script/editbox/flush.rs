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
/// (`0x77d447`) and the bit clears (`0x77d44c`); on bit 2 the caret leg runs (`0x77d475`), with
/// the blink and `OnCursorChanged`, and only then does the bit clear (`0x77d47a`); last,
/// `OnTextChanged` fires if bit 0 was set at entry (`0x77d481`–`0x77d498`). So a handler's edit
/// raises bits for the next flush, a frame later, and never loops inside one: an `OnTextChanged`
/// that sets its own text fires once more, next frame, and a caret moved by `OnCursorChanged` is
/// cleared with the bit.
/// A caret that has left the window promotes bit 2 to bit 0 (`0x77d3f5`–`0x77d436`), so the
/// relayout re-windows around it; that fires no `OnTextChanged`, which keys on the bit as sampled.
/// Besides the walk, the box's `OnKeyDown` (`0x77b1e2`) and `OnMouseDown` (`0x77b819`) run it
/// before the key or click acts.
///
/// Deviation: the promotion's IME legs (`0x77d417`–`0x77d431`) and the IME writer's `or 6`
/// (`0x77acde`, in `0x77ac60`) are not modelled: benilla has no IME composition.
pub(super) fn flush(lua: &Lua, h: FrameHandle) {
    let Some((entry, owed, left)) = with_eb(lua, h, |eb| {
        (eb.dirty, eb.relayout_owed, !eb.caret_in_window())
    }) else {
        return;
    };
    let text_changed = entry & EditBoxState::DIRTY_TEXT != 0;
    let promoted = entry & EditBoxState::DIRTY_CURSOR != 0 && left;
    if text_changed || owed || promoted {
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

/// The caret leg `0x77da80`: a caret outside the window is only hidden (`0x77da97` →
/// `0x77de49`); else it restarts the blink (`0x77dd68`), fires `OnCursorChanged`, then shows the
/// caret in the focused box (`0x77de35`) and hides it elsewhere (`0x77de51`). So a box that takes
/// the keyboard, whose focus change raises bit 2 (`0x77afa8`), starts its caret solid. `false`
/// while the advance table is pending, for the next flush to retry.
fn caret(lua: &Lua, h: FrameHandle) -> bool {
    // A window still owed its relayout is stale, and the retry places the caret.
    if with_eb(lua, h, |eb| !eb.relayout_owed && !eb.caret_in_window()) == Some(true) {
        with_eb(lua, h, |eb| eb.caret_shown = false);
        return true;
    }
    with_eb(lua, h, |eb| eb.blink_accum = 0.0);
    let placed = cursor_changed(lua, h);
    let focused = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .focused_editbox
        == Some(h);
    with_eb(lua, h, |eb| eb.caret_shown = focused);
    placed
}

/// `OnCursorChanged(x, y, w, h)` (`0x77de1b`, in the box's own units via `0x77dd5f`), `x` the
/// caret's advance along its line, `y` minus the row index times the row pitch, and `w` and `h`
/// the caret texture's size (`0x77dd86`, `0x77dd74`): the constant 4.0 and one line of the text's
/// font (`0x7727b0(fs, 1)`, set at `0x77ba76`), single-line or multi-line. The arguments are
/// computed only for a box with the script (`0x77dd8c`), from the box's advance table, which the
/// installed font engine measures here when the text or font moved. `false` while that table or
/// the line's measure is pending, with no engine installed: the host answers the focused box's
/// table a tick later.
fn cursor_changed(lua: &Lua, h: FrameHandle) -> bool {
    let id = frame_id_of(lua, h);
    if !event::has_widget_handler(lua, id, "OnCursorChanged") {
        return true;
    }
    if !super::seam::advances_current(lua, h) {
        return false;
    }
    // Already in the box's own units, as the measure answers.
    let Some(line_h) =
        super::ensure_text_region(lua, h).and_then(|rh| relayout::line_height(lua, h, rh))
    else {
        return false;
    };
    let args = with_eb(lua, h, |eb| {
        let display = eb.display();
        if eb.advances.len() != display.len() + 1 {
            return None;
        }
        let cursor_d = eb.text_to_display(eb.cursor).min(display.len());
        // The caret's line in the line table: in a single-line box the window (`0x77d750`,
        // `0x77d3b2`), multi-line its wrapped row, the last ending at the text's end (`0x77d6ef`).
        let len = display.len();
        let (row, line_start, line_end) = if eb.multi_line {
            let row = eb.row_of(cursor_d);
            let (a, b) = eb.row_range(row, len);
            (row, a.min(len), b.min(len))
        } else {
            let a = eb.scroll_start.min(len);
            (0, a, (a + eb.window_len).min(len))
        };
        let a = &eb.advances;
        // `x` by the font string's justifyH, which follows the box's font (`[fs+0x120] & 7`,
        // `0x77db63`): LEFT from the line's start (`0x77dcd8`–`0x77dcfc`), CENTER that less half
        // the line (`0x77dc1d`–`0x77dc7b`), RIGHT minus the rest of the line (`0x77db89`–
        // `0x77dbb0`), each 0 with the caret on its measuring edge; no axis bit leaves it 0
        // (`0x77db83` → `0x77dd5f`).
        let along = if cursor_d == line_start {
            0.0
        } else {
            a[cursor_d] - a[line_start]
        };
        let x = match eb.justify & crate::justify::H_MASK {
            0x01 => along,
            0x02 => along - (a[line_end] - a[line_start]) * 0.5,
            0x04 if cursor_d == line_end => 0.0,
            0x04 => -(a[line_end] - a[cursor_d]),
            _ => 0.0,
        };
        let pitch = eb.cell_h;
        // `y` starts at 0 and loses one pitch per row above the caret (`0x77dae1`, `0x77db31`),
        // so row 0 reports +0, never the -0 that Lua prints as "-0".
        let y = 0.0 - row as f32 * pitch;
        Some((x, y))
    })
    .flatten();
    let Some((x, y)) = args else {
        return false;
    };
    // The advance table is in root units (measured at the box's scale, divided by the seam
    // alone); the handler gets the box's own, the reference dividing by its effective scale
    // (`0x77db23 fdiv [esi+0x7c]`).
    let k = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        crate::script::object::eff_scale(&model, h)
    };
    let (x, y) = (x / k, y / k);
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnCursorChanged",
        vec![
            Value::Number(f64::from(x)),
            Value::Number(f64::from(y)),
            Value::Number(f64::from(CARET_WIDTH)),
            Value::Number(f64::from(line_h)),
        ],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
    true
}

/// The caret blink (`0x77a7a6`–`0x77a84c`): while the caret lies in the window
/// (`0x77a7bd`–`0x77a7d7`), past the period (0.5 s by default) the focused box's caret toggles and
/// the accumulator resets; only a period of exactly 0.0 keeps the caret solid (`0x77a7b4`).
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
        if eb.blink_period != 0.0 && eb.caret_in_window() {
            eb.blink_accum += dt;
            if eb.blink_accum > eb.blink_period {
                eb.caret_shown = !eb.caret_shown;
                eb.blink_accum = 0.0;
            }
        }
    });
}
