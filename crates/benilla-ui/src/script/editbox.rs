//! The `EditBox` runtime — keyboard focus, the text buffer + cursor + selection, editing, and the
//! key/char dispatch (`CSimpleEditBox`, factory `0x6eec70`).
//!
//! Byte-verified against the reference client:
//!
//! - **Focus:** a single class-owned focus owner ([`Model::focused_editbox`], the client's
//!   `DAT_00cf4dc8`). `SetFocus` gates on effective-visibility, is a no-op if already focused, and
//!   fires `OnEditFocusLost` on the old box then `OnEditFocusGained` on the new. A LeftButton click
//!   focuses UNCONDITIONALLY — and **collapses the selection to the clicked byte index** on its way
//!   (`0x77b86f call 0x77ccf0`, immediately before `0x77b881 SetFocus`), so a fresh click-focus leaves
//!   an EMPTY selection at the click point, never a select-all. `SetFocus` itself writes no selection
//!   field, and `0x77e3f6` is the only instruction image-wide that gives a box focus — so *every*
//!   focus gain, whatever triggers it, is selection-neutral.
//!
//!   **`autoFocus` DOES focus on show** (corrected 2026-08-29, `0x81c910` slot +0x30 `0x77a750`):
//!   the OnShow override tail-jumps `SetFocus` when nothing else holds focus, and the OnHide
//!   mirror tail-jumps `ClearFocus`. The old "verified by absence" negative came from a `call`-only
//!   census that could not see a tail-`jmp`. Both overrides are [`visibility_focus`] here, and the
//!   construction default is `true` off the bytes (see
//!   [`EditBoxState::auto_focus`](crate::widget::EditBoxState::auto_focus)). The
//!   self-acquire-on-first-key half stands beside it.
//! - **Routing:** a focused box processes and CONSUMES every key/char (`return 1` past the
//!   guard); an unfocused non-autoFocus box ignores input. The override fires ONLY the specialized
//!   scripts (Enter/Escape/Space/Tab/TextChanged/TextSet/focus), never generic `OnKeyDown`/`OnChar`.
//! - **Text/editing:** every insert replaces the selection first; `numeric` aborts an insert
//!   wholesale on any non-digit; caps trim from the end (`maxBytes` then `maxLetters`); `SetText`
//!   short-circuits when unchanged; `HighlightText(0,-1)` selects all with the client's clamp.
//!
//! The mouse/selection/caret half (click→index `0x77d0d0`, drag `0x77a860`, the clipboard pair,
//! the 0.5 s blink, the char-granular scroll window) lives in [`interact`]/[`seam`] over the
//! host-answered advance table — decision 0298. The OS clipboard itself stays host-side (this
//! crate is engine-free): paste text arrives via [`paste`], copy/cut strings return through
//! `UiScript::editbox_copy`/`editbox_cut`; Ctrl+A arrives as the SOH control char.
//!
//! Editing keys reach the box as semantic [`EditAction`]s ([`action`]) — the *host's* per-OS
//! keymap decides which physical chord means which action (decision 0301), while the effect of
//! each action stays this module's byte-verified law. [`key_input`] keeps only the three
//! box-event keys (ENTER/ESCAPE/TAB).
//!
//! ## Stated divergences / gaps
//! - **`OnCursorChanged`** (the 4-float caret-pos fire) is not fired — no shipped XML consumes it.
//! - Small INFERRED corners (click rounding, word classes, no shift+click, the password-copy
//!   placeholder literal) are flagged where they live — decision 0298's list.
//! - **Word/edge deletes** (`Delete{Word,Edge}`) have no 1.12 counterpart at all — they exist for
//!   the host's OS-native keymaps (Option/Cmd+Backspace on macOS, Ctrl+Backspace/Delete on
//!   Windows/Linux; decision 0301) and reuse the byte-verified word classes + selection-first
//!   delete law.

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
// Public entry points (called from UiScript::char_input / key_input / mouse_button)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A typed character. Routed per the focus/routing rules above; on a focused box, Ctrl+A
/// (delivered as the SOH control char) selects all, other C0 control chars are consumed-but-inert,
/// and printable text is inserted. Always consumes when a box is (or becomes) focused.
pub(super) fn char_input(lua: &Lua, text: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    if text == "\u{1}" {
        // Ctrl+A → select-all (the client's keydown case 0x41/0x61; here via char_input since a
        // Ctrl-modified letter arrives as its control code).
        highlight_text(lua, h, 0, -1);
    } else if text.chars().any(|c| (c as u32) >= 0x20) {
        // A printable char (or string); pure control input is consumed but not inserted.
        //
        // A typed `|` goes in as `||` — OnChar `0x77c200` pushes the literal at `0x879cac`, which
        // is `"||"` (decision 1077). That is why real markup can only enter a box through
        // `SetText`, `Insert`, a paste or the C++ link-insert path: you cannot type an escape, and
        // the doubled form draws as one `|` and counts as one letter.
        insert(lua, h, &text.replace('|', "||"), true);
    }
    true
}

/// Paste an OS-clipboard string into the focused box. The engine-free runtime can't reach the OS
/// clipboard itself, so the host reads it and hands the text here. Sanitized
/// the same way a typed char is gated — every C0 control char is dropped, except a newline in a
/// `multiLine` box — then inserted as one edit (selection-replace + numeric/caps rules via [`insert`],
/// no `OnSpacePressed` fire: a paste is not a typed space). Always consumes when a box is focused.
pub(super) fn paste(lua: &Lua, text: &str) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    // The sanitation + insert is the shared law ([`EditBoxState::paste`]); only the event fire is
    // this layer's.
    if with_eb(lua, h, |eb| eb.paste(text)).is_some_and(|o| o.text_changed) {
        sync_text_region(lua, h);
        mark_text_changed(lua, h);
    }
    true
}

/// A non-character key by name — only the three *box-event* keys act (their FrameXML scripts);
/// every editing key reaches the box as a semantic [`EditAction`] via [`action`] instead. Follows
/// the same focus/routing rules; a focused box consumes the key even when it does nothing with it.
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
        // Escape fires the script but does NOT release focus (FrameXML calls ClearFocus itself).
        "ESCAPE" => fire_script(lua, id, "OnEscapePressed"),
        "TAB" => fire_script(lua, id, "OnTabPressed"),
        // Any other named key: a focused box still consumes it (past the guard), doing nothing.
        _ => {}
    }
    true
}

/// One semantic editing operation — the host's per-OS keymap output (decision 0301). Same
/// routing/consumption law as [`key_input`]: the focused (or self-acquiring if needed) box
/// processes it; no box → not consumed.
pub(super) fn action(lua: &Lua, a: EditAction) -> bool {
    let Some(h) = route(lua) else {
        return false;
    };
    match a {
        EditAction::Move { unit, back, extend } => match unit {
            // No alt-arrow test here: the gate is on the KEY and lives in the host, which declines
            // the four arrow codes before the chord is ever built (`editbox_alt_arrow_mode`).
            // Anything reaching this arm was Alt-held or was not an arrow, and the reference moves
            // the caret for both.
            EditUnit::Char => move_horizontal(lua, h, !back, extend),
            // Ctrl/Option picks the word-granular cursor helper (the client's Ctrl check,
            // `0x41f8f0(1)`).
            EditUnit::Word => interact::move_word(lua, h, !back, extend),
            EditUnit::Edge => move_to_edge(lua, h, !back, extend),
        },
        EditAction::Delete { unit, back } => match unit {
            EditUnit::Char => delete_dir(lua, h, !back),
            EditUnit::Word => delete_span(lua, h, |eb| eb.word_boundary(!back)),
            EditUnit::Edge => delete_span(lua, h, |eb| if back { 0 } else { eb.text.len() }),
        },
        EditAction::SelectAll => highlight_text(lua, h, 0, -1),
        // History recall (the chat box's `historyLines`): prev = older, next = newer, live draft
        // restored past the newest. Single-line only (benilla's multiLine box has no vertical
        // caret nav — survey gap — a multiLine box consumes the step inert). The recall chords
        // are the host keymap's plain Up/Down (the reference's own history controller is
        // untraced; decision 0301). The alt-arrow gate DOES cover them, upstream: UP and DOWN are
        // two of the four codes it declines, so on a flagged box — which the reference's own chat
        // box is — history recall is **Alt**+Up/Down and a plain Up/Down turns the camera. This
        // file used to say the opposite ("`ignoreArrows` does not gate them"), which followed
        // from reading the flag as consume-but-inert; both halves above are now corrected.
        EditAction::HistoryPrev | EditAction::HistoryNext => {
            if !with_eb(lua, h, |eb| eb.multi_line).unwrap_or(false) {
                history_step_key(lua, h, a == EditAction::HistoryPrev);
            }
        }
    }
    true
}

// The mouse/selection interaction law (click→index, drag-select, clipboard, blink) — a child
// module over this file's focus/editing primitives — and the host-facing seam (`impl UiScript`:
// the advance round trip + text-UI geometry).
mod interact;
mod seam;
pub(super) use interact::{
    click, copy_selection, cut_selection, drag_end, drag_update, tick_blink,
};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Focus model
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Resolve the box that should process this key/char event, self-acquiring if needed.
///
/// - A live, effectively-visible focused box → process it.
/// - A focused box that is hidden (still alive) → `None`: the client's guard returns 0 and the focus
///   global stays set, so no other box self-acquires while it "holds" focus.
/// - A focused handle that went stale → cleared, then fall through to self-acquire.
/// - No focus → the topmost effectively-visible `autoFocus` EditBox self-acquires (firing
///   `OnEditFocusGained`) and processes this same event; otherwise `None` (not consumed).
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

/// The topmost-drawn effectively-visible `autoFocus` EditBox (the keyboard self-acquire target), or
/// `None`. Walks the draw order top-down like [`crate::order::hit_test`].
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

/// `SetFocus` (`0x77e3d0`): gate on effective-visibility, no-op if already focused; else fire
/// `OnEditFocusLost` on the old box, move focus, fire `OnEditFocusGained` on the new.
///
/// `0x77e3f6` (inside this) is the ONLY instruction image-wide that grants a box the focus, and it
/// writes no selection field — so every focus gain in the client, whatever triggers it, is
/// selection-neutral.
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
        // A fresh focus starts a fresh history session: the browse position must not leak from
        // the box's previous open (programmatic SetText deliberately preserves it — the chat
        // live parse's recall rewrite — so this is where a stale walk gets dropped).
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

/// The EditBox's own **OnShow/OnHide vtable overrides** (`0x81c910` slots +0x30/+0x34), run by
/// [`crate::script::event::fire_visibility_changes`] after the frame's Lua handler — the order the
/// reference has, since both overrides call the base notify *first* and only then act.
///
/// - **Show** (`0x77a750`): `if ([0xcf4dc8] == 0 && (flags & 1)) SetFocus(this)` — an `autoFocus`
///   box grabs the keyboard when it appears, **iff nothing else holds it**. Not a
///   "topmost/best" choice: whichever box's show runs first while the focus is free takes it.
/// - **Hide** (`0x77a780`): tail-jumps `ClearFocus`, whose own guard makes it per-box — hiding a
///   box that does not hold the keyboard writes nothing and fires nothing.
///
/// Both were missing until 2026-08-29 (decision 1686).
pub(super) fn visibility_focus(lua: &Lua, h: FrameHandle, visible: bool) {
    if !visible {
        // The guard lives in `clear_focus_handle`, exactly as it does in `0x77e410` — so this is
        // called unconditionally here, like the reference's own unconditional tail-jmp.
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
        // `SetFocus` re-checks effective-visibility itself, which is the reference's gate too.
        set_focus_handle(lua, h);
    }
}

/// `ClearFocus` (`0x77e410`): only if this box holds focus; fire `OnEditFocusLost`. The guard is
/// `mov eax,ecx; mov ecx,[0xcf4dc8]; cmp ecx,eax; jne ret` — verified per-box, which is what makes
/// the OnHide override's *unconditional* tail-jmp into it harmless.
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
// Editing primitives — mutate state under one borrow, then sync + fire
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Insert `ins` at the cursor (`0x77bee0`): replace any selection first; `numeric` aborts wholesale on
/// a non-digit; splice + advance; enforce caps; fire `OnTextChanged`, and (when `fire_space`) one
/// `OnSpacePressed` per space in `ins`.
fn insert(lua: &Lua, h: FrameHandle, ins: &str, fire_space: bool) {
    let Some(out) = with_eb(lua, h, |eb| eb.insert(ins)) else {
        return; // not an EditBox
    };
    if !out.text_changed {
        return; // numeric-aborted
    }
    sync_text_region(lua, h);
    let id = frame_id_of(lua, h);
    // **The EditBox DOES fire generic `OnChar`** — with the spliced string as `arg1`, from inside
    // Insert itself (`0x77c13c`, the varargs firer `0x7026f0` with fmt `"%s"`), before the dirty
    // flush gets round to `OnTextChanged`. The "never generic `OnKeyDown`/`OnChar`" rule holds
    // only for `OnKeyDown` (`+0x188`, corrected 2026-08-29) — it missed `OnChar`'s varargs-firer
    // path, distinct from the sibling fixed-arity firer `0x702690`. It rides the one choke point
    // every insert path goes through — typed char, the `|`→`||` escape, the multiLine Enter
    // newline, the Lua `Insert` — and so is absent from `SetText`, which is where the reference
    // has it absent too.
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

/// `SetText` (`0x77be00`): short-circuit when unchanged (no events); else clear selection, replace,
/// cursor to end, enforce caps, fire `OnTextSet` then `OnTextChanged`.
fn set_text(lua: &Lua, h: FrameHandle, s: &str) {
    let changed = with_eb(lua, h, |eb| eb.set_text(s));
    if changed == Some(true) {
        sync_text_region(lua, h);
        let id = frame_id_of(lua, h);
        fire_script(lua, id, "OnTextSet");
        mark_text_changed(lua, h);
    }
}

/// Word/edge deletes (`Delete{Word,Edge}` — no 1.12 counterpart; the OS-native keymap family):
/// the selection first when one exists, else the span from the cursor to `target_of(eb)`; sync +
/// `OnTextChanged` when anything was removed.
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

/// BACKSPACE / DELETE: delete the selection if non-empty, else the char before (`forward=false`) or
/// after (`forward=true`) the cursor; fire `OnTextChanged` if anything was removed.
fn delete_dir(lua: &Lua, h: FrameHandle, forward: bool) {
    let changed = with_eb(lua, h, |eb| eb.delete_dir(forward));
    if changed == Some(true) {
        sync_text_region(lua, h);
        mark_text_changed(lua, h);
    }
}

/// One UP/DOWN history-recall step: ask the state for the line to show and route it through the
/// internal `set_text` path (region sync + `OnTextSet`/`OnTextChanged` — a recall resets FrameXML
/// tab-complete state exactly like any other text change). `set_text` (internal AND Lua) leaves
/// the browse position intact; typed edits, `AddHistoryLine`, and focus gain end browsing.
fn history_step_key(lua: &Lua, h: FrameHandle, older: bool) {
    let recalled = with_eb(lua, h, |eb| eb.history_step(older)).flatten();
    if let Some(text) = recalled {
        set_text(lua, h, &text);
    }
}

/// LEFT/RIGHT one char: `shift` extends the selection from the fixed anchor; otherwise the caret
/// collapses to the selection edge (if any) or moves one char. Fires no script (a cursor/selection
/// move; `OnCursorChanged` geometry is out of scope).
fn move_horizontal(lua: &Lua, h: FrameHandle, right: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_by_char(right, shift));
}

/// HOME/END: move the caret to 0 / `len`; `shift` extends from the fixed anchor.
fn move_to_edge(lua: &Lua, h: FrameHandle, end: bool, shift: bool) {
    with_eb(lua, h, |eb| eb.move_to_edge(end, shift));
}

/// `HighlightText` (`0x77cca0`), the client's exact clamp: `start = clamp(start, 0..=len)`;
/// `end = (end < 0 || end > len) ? len : end`; then `if end < start { end = len }` — so `(0, -1)`
/// selects all. Offsets are bytes, snapped to char boundaries.
fn highlight_text(lua: &Lua, h: FrameHandle, start: i64, end: i64) {
    with_eb(lua, h, |eb| eb.highlight_text(start, end));
}

// ── text-region sync (the '*' mask lives here) ────────────────────────────────────────────────

/// Write the *display* string into the text region's [`RegionData::text`], creating the region lazily
/// (the implicit FontString) if none exists. `password` shows one `'*'` per **character** (the mask,
/// never the real text).
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

/// `SetTextInsets(l, r, t, b)`: store the insets and re-anchor the text region inside the box by
/// them (y-up: the top inset pulls the region's top DOWN, the bottom inset pulls its bottom UP).
/// The chat edit box calls this per header change (`ChatEdit_UpdateHeader`'s
/// `SetTextInsets(15 + headerWidth, 13, 0, 0)`), so typed text starts past the "Say:" header.
fn set_text_insets(lua: &Lua, h: FrameHandle, l: f32, r: f32, t: f32, b: f32) {
    if with_eb(lua, h, |eb| eb.text_insets = [l, r, t, b]).is_none() {
        return;
    }
    let Some(rh) = ensure_text_region(lua, h) else {
        return;
    };
    write_inset_anchors(lua, h, rh, [l, r, t, b]);
}

/// Anchor the text region inside its box by the insets — the two corner anchors that pin its rect
/// (y-up: the top inset pulls the region's top DOWN, the bottom inset pulls its bottom UP).
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

/// The wrapper for an EditBox's **embedded** text FontString — the region its ctor built at
/// `0x779bee` (`Arena::build_editbox_engine_regions`). `None` when `frame` is not a live EditBox.
///
/// The loader's special-`<FontString>` pass uses this instead of `CreateFontString`: the
/// reference's EditBox LoadXML (`0x779fb0`) treats `<FontString>` as the box's *embedded* font
/// string (its `bytes` attr writes the box's own `maxBytes`), so the element DECLARES the ctor's
/// object rather than adding a region. Creating a second one would leave an orphan on the frame
/// and push the authored `<Layers>` regions one place down the creation-ordered list `GetRegions`
/// walks.
pub(crate) fn editbox_text_region_wrapper(lua: &Lua, frame: &Table) -> Option<Table> {
    let h = frame_handle_of(lua, frame).ok()?;
    let rh = ensure_text_region(lua, h)?;
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.region_id(rh)
    };
    super::region::region_wrapper(lua, id).ok()
}

/// The loader's special-`<FontString>` slot assignment: adopt `region` as `frame`'s text region,
/// anchored by the current insets. The engine ASSIGNS an EditBox's font string at LoadXML — it
/// never searches the region list (a find-first here once grabbed a `<Layers>` FontString, the
/// chat header, so typing overwrote "Say:" and the insets clobbered the header's anchor).
pub(crate) fn adopt_text_region(lua: &Lua, frame: &Table, region: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, frame)?;
    let rh = super::region::region_handle_of(lua, region)?;
    let Some((insets, multi_line)) = with_eb(lua, h, |eb| {
        eb.text_region = Some(rh);
        (eb.text_insets, eb.multi_line)
    }) else {
        return Ok(()); // not an EditBox — the loader guards on the tag, but stay safe
    };
    write_inset_anchors(lua, h, rh, insets);
    // A live text region always carries Some(text) — an empty box still emits its Text quad, so
    // the host's caret has a styled quad to ride before the first keystroke.
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let data = model.region_data.entry(rh).or_default();
    data.text.get_or_insert_with(String::new);
    apply_text_region_justify(data, multi_line);
    Ok(())
}

/// Get-or-create the implicit text FontString (the EditBox's ButtonText analogue). Returns `None` if
/// `h` is not a live EditBox. An XML-declared `<FontString>` is wired explicitly by the loader's
/// special pass ([`adopt_text_region`]); the lazy create covers CreateFrame'd boxes.
pub(super) fn ensure_text_region(lua: &Lua, h: FrameHandle) -> Option<RegionHandle> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let (existing, multi_line) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::EditBox(eb)) => (eb.text_region, eb.multi_line),
        _ => return None,
    };
    // The ctor already built it (`Arena::build_editbox_engine_regions` — the client's own
    // `0x779bee`, the first of the five regions a CSimpleEditBox is born with). The REGION exists
    // from birth; its `RegionData` does not, because the arena has no access to it — so seed that
    // here, once, on whichever call arrives first.
    let rh = match existing {
        Some(rh) => rh,
        // A box whose ctor pass could not run (a dead handle, or a non-EditBox that slipped the
        // guard above) still gets a region rather than silently rendering nothing.
        None => model
            .arena
            .create_region(h, RegionKind::FontString, DrawLayer::Overlay, 0)?,
    };
    if let std::collections::hash_map::Entry::Vacant(slot) = model.region_data.entry(rh) {
        let mut data = RegionData {
            // Some("") from birth — the empty box still emits its Text quad for the host caret.
            text: Some(String::new()),
            ..RegionData::default()
        };
        apply_text_region_justify(&mut data, multi_line);
        slot.insert(data);
        model.touch_layout(); // a region entered the layout gate's read set (decision 0740)
    }
    if let Some(KindState::EditBox(eb)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        eb.text_region = Some(rh);
    }
    Some(rh)
}

/// Re-seat an already-wired text region's justify by the box's CURRENT multiLine flag — the
/// `SetMultiLine` rider: the loader adopts the declared `<FontString>` (LoadXML step 5·b) before
/// it applies the editbox flags (5b), so `multiLine="true"` must re-run the law or the body box
/// keeps the single-line MIDDLE seat.
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

/// The EditBox text-anchoring law: the box's text region lays out LEFT-justified (the client's
/// editbox draw `0x77da80` is left-anchored at the insets rect regardless of the font string's
/// declared justification — focused or not), from the TOP for a multiline box, vertically
/// centered for a single-line one. Without this, a bare
/// `<FontString inherits="ChatFontNormal"/>` inherits the FontString CENTER/MIDDLE defaults and
/// an empty focused box parks its caret mid-box (the mail send tab's original sin).
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

/// The stable id of a frame handle (minting one if needed).
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

/// **The caret flush's fire — `OnCursorChanged(x, y, w, h)`** (`0x77da80`).
///
/// The reference recomputes the caret's anchor inside `0x77da80` and, having done so, fires this
/// with four floats, each scaled by `f` so all four are in FrameXML UI units (`0x77dd5f`):
///
/// * `x` — the caret's advance from its line's start, the same number the paint anchors by
///   ([`EditBoxState::caret_row_x`]).
/// * `y` — `−lineIndex · (snap(H·scale) + snap(spacing·scale))/scale`, i.e. minus the row index
///   times the row pitch. **Negative-downward**, which is what `ScrollingEdit_OnCursorChanged`'s
///   `cursorOffset = y` then `-this.cursorOffset` reads as a positive distance down the page.
/// * `w` — **`≡ 4.0`**, a constant in the reference (the caret texture's own width, read through
///   its layout vtable `+0x1c`).
/// * `h` — the font line height, read through `+0x20`.
///
/// Ours takes the row pitch [`EditBoxState::cell_h`] for both the `y` factor and `h`. The two are
/// the same number whenever the font declares no extra `spacing`, which is every box in the
/// shipped 1.12 UI; a box that *does* set spacing would report an `h` a little tall. Named rather
/// than guessed at — the alternative is carrying a second measured number through the advance
/// answer for a case nothing in the corpus produces.
///
/// **The edge is a CHANGE, not a frame.** `0x77da80` has exactly one caller, `0x77d475`, gated on
/// dirty bit 2, so the handler runs when the caret has actually moved — which is what makes
/// `ScrollingEdit_OnUpdate`'s `if (this.cursorOffset)` idiom terminate. [`EditBoxState::cursor_fired`]
/// is that bit's memory here.
///
/// **Scoped to the focused box**, because the caret geometry rides the advance table and only the
/// focused box asks the host for one ([`super::editbox::seam`]). The reference flushes any dirty
/// box; the difference is invisible for the customer this exists for — you are typing in the box
/// you are looking at — and an unfocused box's caret is never drawn either way.
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
        // No answered advance table yet ⇒ no caret geometry to report. The next tick after the
        // host answers carries it, exactly as the reference's next flush would.
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

/// The caret's width as `OnCursorChanged` reports it — **a constant in the reference**, not a
/// measurement: `w ≡ 4.0` (`+0x1c` in the layout vtable).
const CARET_WIDTH: f32 = 4.0;

// The Lua method surface (SetText/GetText/HighlightText/SetFocus/…, consulted before the shared
// frame table only for EditBox frames) — a child module over this file's focus/editing
// primitives, like `interact`/`seam`.
mod methods;
pub(super) use methods::install;

#[cfg(test)]
mod tests;

/// Raise the `textChanged` dirty bit (`[E+0x31c]` bit 0) rather than firing — **`OnTextChanged` is
/// deferred**, and this is the whole of decision 1831's second half.
///
/// The reference never fires it from an edit. `SetText 0x77be00`, `Insert 0x77bee0` and the deletes
/// all only OR the bit; the single fire site is `0x77d498`, inside the dirty-word drain `0x77d3e0`,
/// whose three callers are the box's own `OnUpdate` (`0x77a790`), its `OnKeyDown` (`0x77b160`) and
/// its `OnMouseDown` (`0x77b800`). So a Lua caller gets control back with the handler still
/// unrun, and repeated changes **coalesce**: the box appears once on the pending list however many
/// times it is written, and the one fire carries the final text.
pub(super) fn mark_text_changed(lua: &Lua, h: FrameHandle) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if !model.dirty_editboxes.contains(&h) {
        model.dirty_editboxes.push(h);
    }
}

/// The dirty-word drain (`0x77d3e0`): fire the pending `OnTextChanged`s.
///
/// Called from the frame tick — the first of the reference's three callers (`0x77a7a1`, inside the
/// box's own `OnUpdate` `0x77a790`, which is also where our caret blink lives). Its other two, the
/// box's `OnKeyDown` and `OnMouseDown`, are **not** wired: they only make a pending fire arrive
/// sooner within a frame, and our key and mouse paths reach the tick anyway. Decision 1831 says so
/// out loud rather than leaving it to be discovered.
///
/// A box that is **not effectively visible stays pending**: `Hide`
/// splices it out of the chain the update walk follows, so its fire waits for a `Show` (verified
/// chain mechanics; the "hidden ⇒ no OnUpdate" step is inferred, not confirmed in the binary).
///
/// A handler may itself write to a box — including this one — so the list is taken before any Lua
/// runs and anything re-marked during the sweep lands on the NEXT drain rather than extending this
/// one. That is the reference's shape too: its bit is cleared before its fire.
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
