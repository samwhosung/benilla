//! The EditBox mouse/selection interaction law — click→char-index, drag-select, the clipboard
//! pair, and the caret blink — a child module over the mother's focus/editing primitives.
//!
//! Byte law:
//!
//! - **Click `0x77b800`**: on a hit — blink reset, click→index (`0x77d0d0`) places the cursor and
//!   collapses the selection, the drag flag raises (`E+0x364 = 1`), then `SetFocus`
//!   UNCONDITIONALLY. No shift+click branch exists in the handler (INFERRED by absence — 1.12
//!   predates extend-on-click).
//! - **Drag `0x77a860`** (mouse move while `+0x364`): map x→index, extend the selection there via
//!   the same helper Shift+arrow uses (`0x77cd10`), cursor following.
//! - **Release** (`0x77afc0` case 1): `+0x364 = 0`.
//! - **Clipboard**: Ctrl+C/X act only with a non-empty selection (`0x77e1d0`); X then
//!   deletes = cut. A password box copies a FIXED placeholder (`0x882748`, runtime literal
//!   unresolved — flagged live-capture), never the real text; benilla stands in the mask run.
//! - **Blink `0x77a790`**: the focused box's accumulator (`E+0x374`) advances by dt; crossing the
//!   period (`E+0x370`, ctor default 0.5 s) toggles the caret and zeroes the accumulator.

use mlua::Lua;

use super::{set_focus_handle, sync_text_region, with_eb};
use crate::script::Model;
use crate::widget::{FrameHandle, FrameKind, KindState};

/// Map a screen point (the engine's y-up UI space — the same space resolved rects and the
/// pointer API live in) to the box's TEXT byte index: x relative to the text region's left edge,
/// shifted into advance space by the scroll window's origin, nearest char boundary (`0x77d0d0`;
/// the rounding is INFERRED nearest). A multiline box also reads y — px down from the region's
/// top pick the wrapped row ([`crate::widget::EditBoxState::index_at_pos`]). Ends up at
/// end-of-text while the advance table is unanswered — the pre-seam behavior, degrading
/// gracefully.
fn index_at_screen_pos(lua: &Lua, h: FrameHandle, x: f32, y: f32) -> usize {
    let corner = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let rh = match model.arena.frame(h).map(|f| &f.kind_state) {
            Some(KindState::EditBox(eb)) => eb.text_region,
            _ => None,
        };
        // The text region's resolved corner; an unanchored region (no insets) draws over its
        // owner, so the frame's own edges stand in.
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

/// A LeftButton press landed on frame `id` (`0x77b800`): if it is an EditBox — blink reset, the
/// cursor to the clicked char (collapsing any selection), drag begins, focus UNCONDITIONALLY.
pub(in crate::script) fn click(lua: &Lua, id: u32, x: f32, y: f32) {
    let Some(h) = editbox_of(lua, id) else { return };
    let idx = index_at_screen_pos(lua, h, x, y);
    with_eb(lua, h, |eb| {
        eb.move_caret_to(idx, false);
        eb.drag_active = true;
    });
    set_focus_handle(lua, h);
}

/// Mouse move while the press is held (`0x77a860`): the focused, drag-active box extends its
/// selection to the hovered index; the cursor follows.
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

/// The press released (`0x77afc0` case 1): the drag ends. Fires for any release — a box that
/// never started a drag ignores it.
pub(in crate::script) fn drag_end(lua: &Lua) {
    if let Some(h) = focused(lua) {
        with_eb(lua, h, |eb| eb.drag_active = false);
    }
}

/// Ctrl+LEFT/RIGHT — the word-granular cursor move (the client walks its per-byte class array on
/// the Ctrl branch, `0x41f8f0(1)`; benilla's alnum-run approximation is INFERRED): `shift` extends
/// the selection from the fixed anchor like the char move; else the caret collapses there.
pub(in crate::script) fn move_word(lua: &Lua, h: FrameHandle, right: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_by_word(right, shift));
}

/// Ctrl+C (`0x77e1d0`): the focused box's selected substring, `None` with no selection. A
/// password box yields the mask run — the client copies a fixed placeholder, NEVER the text.
pub(in crate::script) fn copy_selection(lua: &Lua) -> Option<String> {
    let h = focused(lua)?;
    with_eb(lua, h, |eb| eb.selected_text()).flatten()
}

/// Ctrl+X: copy, then delete the selection (an ordinary edit — the region syncs and
/// `OnTextChanged` fires).
pub(in crate::script) fn cut_selection(lua: &Lua) -> Option<String> {
    let h = focused(lua)?;
    let copied = with_eb(lua, h, |eb| eb.cut_selection()).flatten()?;
    sync_text_region(lua, h);
    super::mark_text_changed(lua, h);
    Some(copied)
}

/// The blink tick (`0x77a790`, run from the host's frame tick): the focused box's accumulator
/// advances; crossing the period toggles the caret and resets. Non-positive period never blinks
/// (a solid caret).
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

/// The focused EditBox handle, if any.
fn focused(lua: &Lua) -> Option<FrameHandle> {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .focused_editbox
}

/// Resolve a frame id to a live EditBox handle.
fn editbox_of(lua: &Lua, id: u32) -> Option<FrameHandle> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let h = model.id_to_frame.get(&id).copied()?;
    matches!(
        model.arena.frame(h).map(|f| f.kind),
        Some(FrameKind::EditBox)
    )
    .then_some(h)
}
