//! The `EditBox` runtime (`CSimpleEditBox`, factory `0x6eec70`): keyboard focus, the text buffer,
//! cursor and selection, editing, and key and char dispatch.
//!
//! - Focus: one owner ([`Model::focused_editbox`], the client's `DAT_00cf4dc8`). A left click
//!   focuses and collapses the selection to the clicked byte (`0x77b86f call 0x77ccf0`, before
//!   `SetFocus` at `0x77b881`), so it never selects all. An `autoFocus` box takes the focus on show
//!   when none is held and on the first key, and a hidden box drops it ([`visibility_focus`]).
//! - Routing: a focused box consumes every key and char and fires only its specialized scripts
//!   (Enter, Escape, Space, Tab, TextChanged, TextSet, focus), never a generic `OnKeyDown`; an
//!   unfocused box that is not `autoFocus` ignores input.
//! - Editing: an insert replaces the selection; `numeric` aborts a whole insert on any non-digit;
//!   caps trim from the end (`maxBytes`, then `maxLetters`).
//!
//! Mouse, selection and caret (click to index `0x77d0d0`, drag `0x77a860`) live in [`interact`]
//! and [`seam`]; the OS clipboard is host-side, [`paste`] in and `UiScript::editbox_copy` or
//! `editbox_cut` out. The host's per-OS keymap sends editing keys as [`EditAction`]s ([`action`]).
//!
//! Deviation: word and edge deletes (`Delete{Word,Edge}`), which 1.12 does not have, exist because
//! the host's OS-native chords (Option/Cmd+Backspace, Ctrl+Backspace/Delete) expect them.

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::types::{EditAction, EditUnit};
use super::{event, Model, RegionData};
use crate::layout::{Anchor, Point};
use crate::order::{self, DrawLayer, ZTarget};
use crate::widget::{EditBoxState, FrameHandle, FrameKind, KindState, RegionHandle, RegionKind};

/// Registry key of the EditBox method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_EDITBOX_METHODS: &str = "__benilla_editbox_methods";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Public entry points
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A typed character: Ctrl+A (arriving as SOH) selects all, other C0 controls are consumed inert,
/// printable text is inserted.
pub(super) fn char_input(lua: &Lua, text: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    if text == "\u{1}" {
        // Ctrl+A selects all (the client's keydown case 0x41/0x61).
        highlight_text(lua, h, 0, -1);
    } else if text.chars().any(|c| (c as u32) >= 0x20) {
        // A typed `|` goes in as `||` (`0x77c200` pushes the literal `"||"` at `0x879cac`), so
        // markup cannot be typed; the pair draws as one `|` and counts as one letter.
        insert(lua, h, &text.replace('|', "||"), true);
    }
    true
}

/// Paste host clipboard text into the focused box as one edit: C0 controls are dropped, except a
/// newline in a `multiLine` box, and no `OnSpacePressed` fires.
pub(super) fn paste(lua: &Lua, text: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    if with_eb(lua, h, |eb| eb.paste(text)).is_some_and(|o| o.text_changed) {
        sync_text_region(lua, h);
        mark_text_changed(lua, h);
    }
    true
}

/// A named key: only Enter, Escape and Tab act, and a focused box consumes every key.
pub(super) fn key_input(lua: &Lua, key: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    let id = frame_id_of(lua, h);
    match key.to_ascii_uppercase().as_str() {
        // multiLine: Enter inserts a newline (no OnSpacePressed); else fire OnEnterPressed.
        "ENTER" => {
            if with_eb(lua, h, |eb| eb.multi_line).unwrap_or(false) {
                insert(lua, h, "\n", false);
            } else {
                fire_script(lua, id, "OnEnterPressed");
            }
        }
        // Escape does not release focus: FrameXML calls ClearFocus itself.
        "ESCAPE" => fire_script(lua, id, "OnEscapePressed"),
        "TAB" => fire_script(lua, id, "OnTabPressed"),
        _ => {}
    }
    true
}

/// One semantic editing operation from the host's per-OS keymap, routed like [`key_input`].
pub(super) fn action(lua: &Lua, a: EditAction) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    match a {
        EditAction::Move { unit, back, extend } => match unit {
            // The alt-arrow gate lives in the host (`editbox_alt_arrow_mode`), which declines the
            // arrow codes before a chord is built.
            EditUnit::Char => move_horizontal(lua, h, !back, extend),
            // Ctrl (Option on macOS) moves by word (the client's Ctrl check, `0x41f8f0(1)`).
            EditUnit::Word => interact::move_word(lua, h, !back, extend),
            EditUnit::Edge => move_to_edge(lua, h, !back, extend),
        },
        EditAction::Delete { unit, back } => match unit {
            EditUnit::Char => delete_dir(lua, h, !back),
            EditUnit::Word => delete_span(lua, h, |eb| eb.word_boundary(!back)),
            EditUnit::Edge => delete_span(lua, h, |eb| if back { 0 } else { eb.text.len() }),
        },
        EditAction::SelectAll => highlight_text(lua, h, 0, -1),
        // History recall (`historyLines`): older, newer, then back to the live draft. A multiLine
        // box consumes it inert (it has no vertical caret movement). On an alt-arrow box, as the
        // stock chat box is, recall is Alt+Up/Down; the reference's history controller is untraced.
        EditAction::HistoryPrev | EditAction::HistoryNext => {
            if !with_eb(lua, h, |eb| eb.multi_line).unwrap_or(false) {
                history_step_key(lua, h, a == EditAction::HistoryPrev);
            }
        }
    }
    true
}

// Mouse, selection, clipboard and blink (`interact`); the host's advance round trip and text
// geometry (`seam`).
mod interact;
mod seam;
pub(super) use interact::{
    click, copy_selection, cut_selection, drag_end, drag_update, tick_blink,
};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Focus model
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The box that processes a key or char. A hidden focused box blocks self-acquire (the client's
/// guard returns 0 with the focus still set); with no focus, the topmost visible `autoFocus` box
/// takes it and processes this same event.
fn route(lua: &Lua) -> Option<FrameHandle> {
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if let Some(h) = model.focused_editbox {
            match model.arena.frame(h) {
                Some(f) if f.effective_visible && f.kind == FrameKind::EditBox => return Some(h),
                Some(_) => return None, // alive but hidden: block self-acquire, don't consume
                None => model.focused_editbox = None, // stale: drop and self-acquire below
            }
        }
    }
    let h = topmost_autofocus(lua)?;
    set_focus_handle(lua, h);
    Some(h)
}

/// The topmost-drawn visible `autoFocus` EditBox, the keyboard self-acquire target.
fn topmost_autofocus(lua: &Lua) -> Option<FrameHandle> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let sorted = order::traversal(&model.arena);
    for (target, _) in sorted.iter().rev() {
        if let ZTarget::Frame(fh) = *target {
            if let Some(KindState::EditBox(eb)) = model.arena.frame(fh).map(|f| &f.kind_state) {
                if eb.auto_focus {
                    return Some(fh);
                }
            }
        }
    }
    None
}

/// `SetFocus` (`0x77e3d0`): needs effective visibility, a no-op when already focused; fires
/// `OnEditFocusLost` on the old box, then `OnEditFocusGained`. Its store at `0x77e3f6` is the
/// client's only focus grant and writes no selection, so no focus gain changes the selection.
fn set_focus_handle(lua: &Lua, h: FrameHandle) {
    let (old_id, new_id) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let focusable = matches!(
            model.arena.frame(h),
            Some(f) if f.effective_visible && f.kind == FrameKind::EditBox
        );
        if !focusable || model.focused_editbox == Some(h) {
            return;
        }
        let old = model.focused_editbox;
        model.focused_editbox = Some(h);
        // A focus gain ends history browsing (`SetText` keeps it for the chat parser's rewrite).
        if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            eb.end_history_browse();
        }
        (old.map(|o| model.frame_id(o)), model.frame_id(h))
    };
    if let Some(oid) = old_id {
        fire_script(lua, oid, "OnEditFocusLost");
    }
    fire_script(lua, new_id, "OnEditFocusGained");
}

/// The EditBox's OnShow/OnHide vtable overrides (`0x81c910` slots +0x30/+0x34), run after the
/// frame's Lua handler, since both call the base notify first. Show (`0x77a750`) focuses an
/// `autoFocus` box when nothing holds the focus, first come first served; hide (`0x77a780`)
/// tail-jumps `ClearFocus`.
pub(super) fn visibility_focus(lua: &Lua, h: FrameHandle, visible: bool) {
    if !visible {
        // Unconditional, like the reference's tail-jump: the guard is in `clear_focus_handle`.
        clear_focus_handle(lua, h);
        return;
    }
    let wants = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.focused_editbox.is_none()
            && matches!(
                model.arena.frame(h).map(|f| &f.kind_state),
                Some(KindState::EditBox(eb)) if eb.auto_focus,
            )
    };
    if wants {
        // `SetFocus` checks effective visibility itself.
        set_focus_handle(lua, h);
    }
}

/// `ClearFocus` (`0x77e410`): only when this box holds the focus (`[0xcf4dc8]`), then fires
/// `OnEditFocusLost`.
fn clear_focus_handle(lua: &Lua, h: FrameHandle) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.focused_editbox != Some(h) {
            return;
        }
        model.focused_editbox = None;
        model.frame_id(h)
    };
    fire_script(lua, id, "OnEditFocusLost");
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Editing primitives: mutate under one borrow, then sync and fire
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `Insert` (`0x77bee0`): replace the selection, abort on a non-digit when `numeric`, splice,
/// enforce caps, mark `OnTextChanged`, and with `fire_space` fire one `OnSpacePressed` per space.
fn insert(lua: &Lua, h: FrameHandle, ins: &str, fire_space: bool) {
    let Some(out) = with_eb(lua, h, |eb| eb.insert(ins)) else {
        return; // not an EditBox
    };
    if !out.text_changed {
        return; // numeric-aborted
    }
    sync_text_region(lua, h);
    let id = frame_id_of(lua, h);
    // Insert fires the generic `OnChar` with the spliced string as `arg1` (`0x77c13c`, through the
    // varargs firer `0x7026f0`), before the deferred `OnTextChanged`; `SetText` does not.
    let on_char = lua
        .create_string(ins)
        .and_then(|s| event::fire_widget_handler(lua, id, "OnChar", vec![mlua::Value::String(s)]));
    if let Err(e) = on_char {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    mark_text_changed(lua, h);
    if fire_space {
        for _ in 0..out.spaces {
            fire_script(lua, id, "OnSpacePressed");
        }
    }
}

/// `SetText` (`0x77be00`): nothing when unchanged; else clear the selection, replace, cursor to
/// the end, enforce caps, fire `OnTextSet` and mark `OnTextChanged`.
fn set_text(lua: &Lua, h: FrameHandle, s: &str) {
    let changed = with_eb(lua, h, |eb| eb.set_text(s));
    if changed == Some(true) {
        sync_text_region(lua, h);
        let id = frame_id_of(lua, h);
        fire_script(lua, id, "OnTextSet");
        mark_text_changed(lua, h);
    }
}

/// Word and edge deletes (no 1.12 counterpart): the selection if any, else the cursor to
/// `target_of(eb)`.
fn delete_span(lua: &Lua, h: FrameHandle, target_of: impl FnOnce(&EditBoxState) -> usize) {
    let changed = with_eb(lua, h, |eb| {
        let t = target_of(eb);
        eb.delete_to(t)
    });
    if changed == Some(true) {
        sync_text_region(lua, h);
        mark_text_changed(lua, h);
    }
}

/// Backspace and Delete: the selection if any, else one char before or after the cursor.
fn delete_dir(lua: &Lua, h: FrameHandle, forward: bool) {
    let changed = with_eb(lua, h, |eb| eb.delete_dir(forward));
    if changed == Some(true) {
        sync_text_region(lua, h);
        mark_text_changed(lua, h);
    }
}

/// One history-recall step through `set_text`, so a recall fires `OnTextSet` like any text change.
/// `set_text` keeps the browse position; typing, `AddHistoryLine` and a focus gain end it.
fn history_step_key(lua: &Lua, h: FrameHandle, older: bool) {
    let recalled = with_eb(lua, h, |eb| eb.history_step(older)).flatten();
    if let Some(text) = recalled {
        set_text(lua, h, &text);
    }
}

/// Left/Right: `shift` extends from the anchor, else collapse the selection or move one char.
fn move_horizontal(lua: &Lua, h: FrameHandle, right: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_by_char(right, shift));
}

/// Home/End; `shift` extends from the anchor.
fn move_to_edge(lua: &Lua, h: FrameHandle, end: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_to_edge(end, shift));
}

/// `HighlightText` (`0x77cca0`): `start = clamp(start, 0..=len)`; `end = (end < 0 || end > len) ?
/// len : end`; then `if end < start { end = len }`, so `(0, -1)` selects all. Offsets are bytes,
/// snapped to char boundaries.
fn highlight_text(lua: &Lua, h: FrameHandle, start: i64, end: i64) {
    with_eb(lua, h, |eb| eb.highlight_text(start, end));
}

// ── text-region sync ─────────────────────────────────────────────────────────────────────────

/// Write the display string into the text region; a `password` box shows one `*` per character.
fn sync_text_region(lua: &Lua, h: FrameHandle) {
    let Some(rh) = ensure_text_region(lua, h) else {
        return;
    };
    let display = with_eb(lua, h, |eb| {
        if eb.password {
            "*".repeat(eb.text.chars().count())
        } else {
            eb.text.clone()
        }
    });
    if let Some(display) = display {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.region_data.entry(rh).or_default().text = Some(display);
        model.touch_measure(rh);
    }
}

/// `SetTextInsets(l, r, t, b)`: store the insets and re-anchor the text region by them.
fn set_text_insets(lua: &Lua, h: FrameHandle, l: f32, r: f32, t: f32, b: f32) {
    if with_eb(lua, h, |eb| eb.text_insets = [l, r, t, b]).is_none() {
        return;
    }
    let Some(rh) = ensure_text_region(lua, h) else {
        return;
    };
    write_inset_anchors(lua, h, rh, [l, r, t, b]);
}

/// Pin the text region's corners inside the box by the insets (y-up: the top inset moves its top
/// down, the bottom inset its bottom up).
fn write_inset_anchors(lua: &Lua, h: FrameHandle, rh: RegionHandle, [l, r, t, b]: [f32; 4]) {
    let owner = frame_id_of(lua, h);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let pair = [
        Anchor::new(Point::TopLeft, owner, Point::TopLeft, l, -t),
        Anchor::new(Point::BottomRight, owner, Point::BottomRight, -r, b),
    ];
    let data = model.region_data.entry(rh).or_default();
    let same = data.anchors.len() == 2
        && data
            .anchors
            .iter()
            .zip(&pair)
            .all(|(a, b)| super::object::anchor_bits_eq(a, b));
    if !same {
        data.anchors = pair.to_vec();
        model.touch_layout();
    }
}

/// The wrapper for an EditBox's embedded text FontString, built by its ctor (`0x779bee`). The
/// loader uses it for `<FontString>`: the reference's EditBox LoadXML (`0x779fb0`) declares the
/// ctor's font string rather than adding a region (its `bytes` sets the box's `maxBytes`).
pub(crate) fn editbox_text_region_wrapper(lua: &Lua, frame: &Table) -> Option<Table> {
    let h = frame_handle_of(lua, frame).ok()?;
    let rh = ensure_text_region(lua, h)?;
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.region_id(rh)
    };
    super::region::region_wrapper(lua, id).ok()
}

/// Adopt `region` as `frame`'s text region, anchored by the current insets: LoadXML assigns an
/// EditBox's font string, never searching the region list.
pub(crate) fn adopt_text_region(lua: &Lua, frame: &Table, region: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, frame)?;
    let rh = super::region::region_handle_of(lua, region)?;
    let Some((insets, multi_line)) = with_eb(lua, h, |eb| {
        eb.text_region = Some(rh);
        (eb.text_insets, eb.multi_line)
    }) else {
        return Ok(()); // not an EditBox
    };
    write_inset_anchors(lua, h, rh, insets);
    // Always `Some` text: an empty box still emits its Text quad for the host's caret.
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let data = model.region_data.entry(rh).or_default();
    data.text.get_or_insert_with(String::new);
    apply_text_region_justify(data, multi_line);
    Ok(())
}

/// Get or create the box's text FontString; an XML-declared one is wired by [`adopt_text_region`].
pub(super) fn ensure_text_region(lua: &Lua, h: FrameHandle) -> Option<RegionHandle> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let (existing, multi_line) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::EditBox(eb)) => (eb.text_region, eb.multi_line),
        _ => return None,
    };
    // The ctor built the region (`0x779bee`, the first of a CSimpleEditBox's five regions); its
    // `RegionData` is seeded here, on the first call.
    let rh = match existing {
        Some(rh) => rh,
        // A box whose ctor pass did not run still gets a region.
        None => model
            .arena
            .create_region(h, RegionKind::FontString, DrawLayer::Overlay, 0)?,
    };
    if let std::collections::hash_map::Entry::Vacant(slot) = model.region_data.entry(rh) {
        let mut data = RegionData {
            // Some("") from birth: the empty box still emits its Text quad for the host caret.
            text: Some(String::new()),
            ..RegionData::default()
        };
        apply_text_region_justify(&mut data, multi_line);
        slot.insert(data);
        model.touch_layout(); // a region entered the layout gate's read set
    }
    if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        eb.text_region = Some(rh);
    }
    Some(rh)
}

/// Re-apply the text region's justify for the current `multiLine` flag: LoadXML adopts the
/// `<FontString>` before it applies the box's flags.
pub(super) fn refresh_text_region_justify(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(KindState::EditBox(eb)) = model.arena.frame(h).map(|f| &f.kind_state) else {
        return Ok(());
    };
    let (Some(rh), multi_line) = (eb.text_region, eb.multi_line) else {
        return Ok(());
    };
    apply_text_region_justify(model.region_data.entry(rh).or_default(), multi_line);
    Ok(())
}

/// Left-justified whatever the font string declares (the draw `0x77da80` anchors left at the
/// insets), from the top in a multiLine box and vertically centered otherwise.
fn apply_text_region_justify(data: &mut RegionData, multi_line: bool) {
    data.justify.set_h(crate::script::JustifyH::Left);
    data.justify.set_v(if multi_line {
        crate::script::JustifyV::Top
    } else {
        crate::script::JustifyV::Middle
    });
}

// ── small shared helpers ─────────────────────────────────────────────────────────────────────

/// Run `f` over a frame's EditBox state under one short write borrow; `None` if not a live EditBox.
fn with_eb<T>(lua: &Lua, h: FrameHandle, f: impl FnOnce(&mut EditBoxState) -> T) -> Option<T> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    match model.arena.frame_mut(h).map(|fr| &mut fr.kind_state) {
        Some(KindState::EditBox(eb)) => Some(f(eb)),
        _ => None,
    }
}

fn frame_id_of(lua: &Lua, h: FrameHandle) -> u32 {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .frame_id(h)
}

/// Fire one specialized EditBox script (no args beyond `self`); errors go to [`Model::errors`].
fn fire_script(lua: &Lua, id: u32, name: &str) {
    if let Err(e) = event::fire_widget_handler(lua, id, name, Vec::new()) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
}

/// `OnCursorChanged(x, y, w, h)` from the caret flush (`0x77da80`, in UI units via `0x77dd5f`): `x`
/// is the caret's advance along its line, `y` minus the row index times the row pitch, `w` the
/// constant 4.0 and `h` the line height. It fires only when the caret moved: the flush's one
/// caller, `0x77d475`, is gated on dirty bit 2.
///
/// Deviation: `h` is the row pitch, which is taller than the line height only for a font with
/// extra spacing (none in the stock UI), because the host's advance answer carries one measure.
///
/// Deviation: only the focused box fires, where the reference flushes any dirty box, because only
/// the focused box gets an advance table from the host.
pub(in crate::script) fn drain_cursor_changed(lua: &Lua) {
    let fire = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(h) = model.focused_editbox else {
            return;
        };
        if !model.arena.frame(h).is_some_and(|f| f.effective_visible) {
            return;
        }
        let id = model.frame_id(h);
        let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state)
        else {
            return;
        };
        // No advance table yet: the tick after the host answers reports it.
        let display = eb.display();
        if eb.advances.len() != display.len() + 1 {
            return;
        }
        let cursor_d = eb.text_to_display(eb.cursor).min(display.len());
        let (row, x) = if eb.multi_line {
            eb.caret_row_x(cursor_d)
        } else {
            (0, eb.advances[cursor_d])
        };
        if eb.cursor_fired == Some((row, x)) {
            return;
        }
        eb.cursor_fired = Some((row, x));
        let pitch = eb.cell_h;
        (id, x, -(row as f32) * pitch, pitch)
    };
    let (id, x, y, h) = fire;
    if let Err(e) = event::fire_widget_handler(
        lua,
        id,
        "OnCursorChanged",
        vec![
            Value::Number(f64::from(x)),
            Value::Number(f64::from(y)),
            Value::Number(f64::from(CARET_WIDTH)),
            Value::Number(f64::from(h)),
        ],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
}

/// The caret width `OnCursorChanged` reports, a constant in the reference.
const CARET_WIDTH: f32 = 4.0;

// The EditBox Lua methods, consulted before the shared frame table for EditBox frames.
mod methods;
pub(super) use methods::install;

#[cfg(test)]
mod tests;

/// Raise the `textChanged` dirty bit (`[E+0x31c]` bit 0): the reference's edits only mark it, and
/// its one fire site (`0x77d498`) is in the drain `0x77d3e0`, so a Lua caller gets control back
/// before the handler runs and repeated changes coalesce into one fire with the final text.
pub(super) fn mark_text_changed(lua: &Lua, h: FrameHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if !model.dirty_editboxes.contains(&h) {
        model.dirty_editboxes.push(h);
    }
}

/// The dirty-word drain (`0x77d3e0`): fire the pending `OnTextChanged`s from the frame tick, as the
/// box's `OnUpdate` (`0x77a790`) does. A hidden box stays pending until shown: `Hide` unlinks it
/// from the update chain (that it then gets no `OnUpdate` is inferred). The list is taken before
/// any Lua runs, so a box a handler re-marks fires on the next drain, as the reference clears its
/// bit before the fire.
///
/// Deviation: not drained from `OnKeyDown` (`0x77b160`) or `OnMouseDown` (`0x77b800`), because
/// those only bring the fire sooner within a frame and our key and mouse paths reach the tick.
pub(in crate::script) fn drain_text_changed(lua: &Lua) {
    let pending = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if model.dirty_editboxes.is_empty() {
            return;
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut model.dirty_editboxes)
            .into_iter()
            .partition(|&h| model.arena.frame(h).is_some_and(|f| f.effective_visible));
        // A frame that has been destroyed drops out of both halves.
        model.dirty_editboxes = waiting
            .into_iter()
            .filter(|&h| model.arena.frame(h).is_some())
            .collect();
        ready
    };
    for h in pending {
        fire_script(lua, frame_id_of(lua, h), "OnTextChanged");
    }
}
