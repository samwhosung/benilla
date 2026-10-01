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
//! - Editing: an insert deletes the selection, then `numeric` aborts a whole insert on any
//!   non-digit; caps trim from the end (`maxBytes`, then `maxLetters`).
//! - Events: an edit, a caret move, a focus change or a re-seat of the text only raises the box's
//!   dirty bits (`[E+0x31c]`); the box's flush, run after its own `OnUpdate` in the tick's walk
//!   ([`update`]) and before a key or click acts on it, fires `OnCursorChanged` and then
//!   `OnTextChanged`.
//!
//! Mouse, selection and caret (click to index `0x77d0d0`, drag `0x77a860`) live in [`interact`]
//! and [`seam`]; the OS clipboard is host-side, [`paste`] in and `UiScript::editbox_copy` or
//! `editbox_cut` out. The host's per-OS keymap sends editing keys as [`EditAction`]s ([`action`]).
//!
//! Deviation: word and edge deletes (`Delete{Word,Edge}`), which 1.12 does not have, exist because
//! the host's OS-native chords (Option/Cmd+Backspace, Ctrl+Backspace/Delete) expect them.

use mlua::{Lua, Table};

use super::object::frame_handle_of;
use super::types::{EditAction, EditUnit};
use super::{event, Model, RegionData};
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
    // The char's own key-down came first, and `OnKeyDown` flushes (`0x77b1e2`).
    flush::flush(lua, h);
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
    flush::flush(lua, h); // Ctrl+V is a key-down (`0x77b1e2`)
    if with_eb(lua, h, |eb| eb.paste(text)).is_some_and(|o| o.text_changed) {
        sync_text_region(lua, h);
    }
    true
}

/// A named key: only Enter, Escape and Tab act, and a focused box consumes every key.
pub(super) fn key_input(lua: &Lua, key: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    // `OnKeyDown` flushes the box before it handles the key (`0x77b1e2`), so the events of the
    // edits ahead of the key fire first, and per key.
    flush::flush(lua, h);
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
    flush::flush(lua, h); // a key-down (`0x77b1e2`)
    match a {
        EditAction::Move { unit, back, extend } => match unit {
            // The alt-arrow gate lives in the host (`editbox_alt_arrow_mode`), which declines the
            // arrow codes before a chord is built.
            EditUnit::Char => move_horizontal(lua, h, !back, extend),
            // Ctrl (Option on macOS) moves by word (the client's Ctrl check, `0x41f8f0(1)`).
            EditUnit::Word => interact::move_word(lua, h, !back, extend),
            EditUnit::Line => {
                with_eb(lua, h, |eb| eb.move_to_line_edge(!back, extend));
            }
            EditUnit::Edge => move_to_edge(lua, h, !back, extend),
            // UP/DOWN fork on `multiLine` (`0x77b64e`, `0x77b675`): a row in a multi-line box,
            // else history recall (`historyLines`): older, newer, then back to the live draft.
            // On an alt-arrow box, as the stock chat box is, that is Alt+Up/Down; the reference's
            // history controller is untraced.
            EditUnit::Row => {
                if with_eb(lua, h, |eb| eb.multi_line).unwrap_or(false) {
                    // The rows the move reads, measured now when an engine is installed.
                    seam::advances_current(lua, h);
                    with_eb(lua, h, |eb| eb.move_by_row(!back, extend));
                } else {
                    history_step_key(lua, h, back);
                }
            }
        },
        EditAction::Delete { unit, back } => match unit {
            EditUnit::Char => delete_dir(lua, h, !back),
            EditUnit::Word => delete_span(lua, h, |eb| eb.word_boundary(!back)),
            EditUnit::Edge => delete_span(lua, h, |eb| if back { 0 } else { eb.text.len() }),
            // No keymap deletes by line or row.
            EditUnit::Line | EditUnit::Row => {}
        },
        EditAction::SelectAll => highlight_text(lua, h, 0, -1),
    }
    true
}

// Mouse, selection and clipboard (`interact`); the host's advance round trip and text geometry
// (`seam`); the text region's rect and a multi-line box's height (`relayout`); the per-frame
// update and its flush (`flush`).
mod flush;
mod interact;
mod relayout;
mod seam;
pub(in crate::script) use flush::update;
pub(super) use interact::{click, copy_selection, cut_selection, drag_end, drag_update};
pub(in crate::script) use relayout::reseat_resized;

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
        if let Some(o) = old {
            raise_cursor(&mut model, o);
        }
        raise_cursor(&mut model, h);
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

/// The focus transition's caret write (`0x77afa8`, `or 4`), on the box that gains the keyboard
/// and on the one that loses it, whose next flush places or hides the caret.
fn raise_cursor(model: &mut Model, h: FrameHandle) {
    if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        eb.dirty |= EditBoxState::DIRTY_CURSOR;
    }
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
        raise_cursor(&mut model, h);
        model.frame_id(h)
    };
    fire_script(lua, id, "OnEditFocusLost");
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Editing primitives: mutate under one borrow, then sync and fire
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `Insert` (`0x77bee0`): delete the selection, abort on a non-digit when `numeric`, splice,
/// enforce caps, raise the dirty bits for the flush, and with `fire_space` fire one
/// `OnSpacePressed` per space.
fn insert(lua: &Lua, h: FrameHandle, ins: &str, fire_space: bool) {
    let Some(out) = with_eb(lua, h, |eb| eb.insert(ins)) else {
        return; // not an EditBox
    };
    if out.text_changed {
        sync_text_region(lua, h);
    }
    if !out.inserted {
        // Refused: a numeric abort leaves at most the selection's deletion, whose text bit fires
        // `OnTextChanged` at the flush, and returns before `OnChar` (`0x77bf6d` → `0x77c1b8`).
        return;
    }
    let id = frame_id_of(lua, h);
    // Insert fires the generic `OnChar` with the spliced string as `arg1` (`0x77c13c`, through the
    // varargs firer `0x7026f0`), ahead of the flush's `OnTextChanged`; `SetText` does not.
    let on_char = lua
        .create_string(ins)
        .and_then(|s| event::fire_widget_handler(lua, id, "OnChar", vec![mlua::Value::String(s)]));
    if let Err(e) = on_char {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    if fire_space {
        for _ in 0..out.spaces {
            fire_script(lua, id, "OnSpacePressed");
        }
    }
}

/// `SetText` (`0x77be00`): nothing when unchanged; else clear the selection, replace, cursor to
/// the end, enforce caps, raise the dirty bits for the flush and fire `OnTextSet`.
fn set_text(lua: &Lua, h: FrameHandle, s: &str) {
    let changed = with_eb(lua, h, |eb| eb.set_text(s));
    if changed == Some(true) {
        sync_text_region(lua, h);
        let id = frame_id_of(lua, h);
        fire_script(lua, id, "OnTextSet");
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
    }
}

/// Backspace and Delete: the selection if any, else one char before or after the cursor.
fn delete_dir(lua: &Lua, h: FrameHandle, forward: bool) {
    let changed = with_eb(lua, h, |eb| eb.delete_dir(forward));
    if changed == Some(true) {
        sync_text_region(lua, h);
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

/// Left/Right: `shift` extends the selection, else collapse it or move one char.
fn move_horizontal(lua: &Lua, h: FrameHandle, right: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_by_char(right, shift));
}

/// Ctrl+Home/End and Cmd+arrow, to the text's edge; `shift` extends the selection.
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

/// `SetTextInsets(l, r, t, b)` (`0x77a6e0`): store the insets and re-seat the text region by
/// them (`0x77a707` → `0x77b8c0`).
fn set_text_insets(lua: &Lua, h: FrameHandle, l: f32, r: f32, t: f32, b: f32) {
    if with_eb(lua, h, |eb| eb.text_insets = [l, r, t, b]).is_none() {
        return;
    }
    reseat_text_region(lua, h);
}

/// Run `0x77b8c0` on the box's text region, creating the region's data if nothing has yet.
fn reseat_text_region(lua: &Lua, h: FrameHandle) {
    if ensure_text_region(lua, h).is_none() {
        return;
    }
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    relayout::seat_text_region(&mut model, h);
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
    let Some(multi_line) = with_eb(lua, h, |eb| {
        eb.text_region = Some(rh);
        eb.multi_line
    }) else {
        return Ok(()); // not an EditBox
    };
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    relayout::seat_text_region(&mut model, h);
    // Always `Some` text: an empty box still emits its Text quad for the host's caret.
    let data = model.region_data.entry(rh).or_default();
    data.text.get_or_insert_with(String::new);
    apply_text_region_justify(data, multi_line);
    Ok(())
}

/// Get or create the box's text FontString; an XML-declared one is wired by [`adopt_text_region`].
pub(super) fn ensure_text_region(lua: &Lua, h: FrameHandle) -> Option<RegionHandle> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    ensure_text_region_in(&mut model, h)
}

/// [`ensure_text_region`] on a borrowed model.
fn ensure_text_region_in(model: &mut Model, h: FrameHandle) -> Option<RegionHandle> {
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

/// Re-apply the text region's justify and rect for the current `multiLine` flag, as
/// `SetMultiLine` re-seats it (`0x77a63f` → `0x77b8c0`): LoadXML adopts the `<FontString>` before
/// it applies the box's flags.
pub(super) fn refresh_text_region(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(KindState::EditBox(eb)) = model.arena.frame(h).map(|f| &f.kind_state) else {
        return Ok(());
    };
    let (Some(rh), multi_line) = (eb.text_region, eb.multi_line) else {
        return Ok(());
    };
    apply_text_region_justify(model.region_data.entry(rh).or_default(), multi_line);
    relayout::seat_text_region(&mut model, h);
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

/// The caret width `OnCursorChanged` reports: 4.0 UI units on every raster (`0x77b8c0` sets it,
/// `0x77ba67`).
const CARET_WIDTH: f32 = 4.0;

// The EditBox Lua methods, consulted before the shared frame table for EditBox frames.
mod methods;
pub(super) use methods::install;

#[cfg(test)]
mod flush_tests;
#[cfg(test)]
mod relayout_tests;
#[cfg(test)]
mod tests;
