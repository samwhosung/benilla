//! EditBox mouse and clipboard interaction: click, drag-select, copy, cut and the caret blink.

use mlua::Lua;

use super::{set_focus_handle, sync_text_region, with_eb};
use crate::script::Model;
use crate::widget::{FrameHandle, FrameKind, KindState};

/// A UI-space point (y up) to the box's text byte index at the nearest cursor stop (`0x77d0d0`,
/// whose own rounding is untraced): x from the text region's left edge plus the scroll origin, and
/// in a multi-line box y from its top picks the wrapped row. The end of the text until the advance
/// table is answered.
fn index_at_screen_pos(lua: &Lua, h: FrameHandle, x: f32, y: f32) -> usize {
    let corner = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let rh = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::EditBox(eb)) => eb.text_region,
            _ => None,
        };
        // An unanchored text region draws over its owner, so the frame's own corner stands in.
        rh.and_then(|rh| model.region_resolved.get(&rh))
            .or_else(|| model.resolved.get(&h))
            .map(|r| (r.left, r.top))
    };
    with_eb(lua, h, |eb| {
        let Some((left, top)) = corner else {
            return eb.text.len();
        };
        let win = eb.advances.get(eb.scroll_start).copied().unwrap_or(0.0);
        let d = eb.index_at_pos((x - left) + win, top - y);
        eb.display_to_text(d)
    })
    .unwrap_or(0)
}

/// A LeftButton press on EditBox `id` (`0x77b800`) places the cursor at the clicked char,
/// collapsing the selection, starts a drag and takes focus unconditionally; no shift+click branch.
pub(in crate::script) fn click(lua: &Lua, id: u32, x: f32, y: f32) {
    let Some(h) = editbox_of(lua, id) else { return };
    let idx = index_at_screen_pos(lua, h, x, y);
    with_eb(lua, h, |eb| {
        eb.move_caret_to(idx, false);
        eb.drag_active = true;
    });
    set_focus_handle(lua, h);
}

/// Mouse move with the button held (`0x77a860`): a dragging focused box extends its selection to
/// the hovered index through the Shift+arrow helper (`0x77cd10`).
pub(in crate::script) fn drag_update(lua: &Lua, x: f32, y: f32) {
    let Some(h) = focused(lua) else { return };
    if with_eb(lua, h, |eb| eb.drag_active) != Some(true) {
        return;
    }
    let idx = index_at_screen_pos(lua, h, x, y);
    with_eb(lua, h, |eb| {
        if idx != eb.cursor {
            eb.move_caret_to(idx, true);
        }
    });
}

/// Any button release ends the drag (`0x77afc0` case 1).
pub(in crate::script) fn drag_end(lua: &Lua) {
    if let Some(h) = focused(lua) {
        with_eb(lua, h, |eb| eb.drag_active = false);
    }
}

/// Ctrl+Left/Right, the word move: the client walks its per-byte class array (`0x41f8f0(1)`),
/// benilla stops at alphanumeric runs. `shift` extends the selection from its anchor.
pub(in crate::script) fn move_word(lua: &Lua, h: FrameHandle, right: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_by_word(right, shift));
}

/// Ctrl+C (`0x77e1d0`): the focused box's selection. A password box never yields its text: the
/// client copies the image's empty string (`0x882748`), benilla the mask run.
pub(in crate::script) fn copy_selection(lua: &Lua) -> Option<String> {
    let h = focused(lua)?;
    with_eb(lua, h, |eb| eb.selected_text()).flatten()
}

/// Ctrl+X (`0x77e1d0`): copy, then delete the selection as an ordinary edit.
pub(in crate::script) fn cut_selection(lua: &Lua) -> Option<String> {
    let h = focused(lua)?;
    let copied = with_eb(lua, h, |eb| eb.cut_selection()).flatten()?;
    sync_text_region(lua, h);
    super::mark_text_changed(lua, h);
    Some(copied)
}

/// The caret blink (`0x77a790`), each frame: past the period (0.5 s by default) the focused box's
/// caret toggles and the accumulator resets; a non-positive period keeps the caret solid.
pub(in crate::script) fn tick_blink(lua: &Lua, dt: f32) {
    if let Some(h) = focused(lua) {
        with_eb(lua, h, |eb| {
            if eb.blink_period > 0.0 {
                eb.blink_accum += dt;
                if eb.blink_accum > eb.blink_period {
                    eb.caret_shown = !eb.caret_shown;
                    eb.blink_accum = 0.0;
                }
            }
        });
    }
}

fn focused(lua: &Lua) -> Option<FrameHandle> {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .focused_editbox
}

fn editbox_of(lua: &Lua, id: u32) -> Option<FrameHandle> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let h = model.id_to_frame.get(&id).copied()?;
    matches!(
        model.arena.frame(h).map(|f| f.kind),
        Some(FrameKind::EditBox)
    )
    .then_some(h)
}
