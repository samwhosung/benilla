//! The show and hide transitions with their notifies (`0x76ae10`, `0x76ad50`). Each marks its
//! frame before anything runs, walks the live child list re-gating each child as the walk reaches
//! it, and fires its own notify last, so handlers run inside the walk, children before their
//! parent, and a frame an earlier handler hid is never notified.
//!
//! Both are virtual (slots `+0x88` and `+0x84`), and every caller reaches them through the slot:
//! the Lua `Show`/`Hide`, the parent's child walk (`0x76aed5`, `0x76adee`) and `SetParent`
//! (`0x76abfd`, `0x76ab41`). [`show`] and [`hide`] are that dispatch, so a kind's override runs
//! the same however the transition comes.

use mlua::{Lua, Table};

use super::{event, Model};
use crate::widget::{ButtonState, ChildWalk, FrameHandle, FrameKind};

/// The Lua `Show`/`Hide` (`0x775750`/`0x775810`): write the shown bit (`0x7757f1`/`0x7758b1`),
/// then dispatch the transition (`0x7757fb`/`0x7758bb`), which gates itself, so `Show()` on a
/// frame already shown but not visible still runs.
pub(super) fn set_shown(lua: &Lua, h: FrameHandle, shown: bool) {
    match model_mut(lua).arena.frame_mut(h) {
        Some(f) => f.shown = shown,
        None => return,
    }
    if shown {
        show(lua, h);
    } else {
        hide(lua, h);
    }
}

/// The show slot (`+0x88`): the kind's override, else the base [`base_show`]. GameTooltip's is
/// `0x530a80`. CharacterModelBase's (`0x505d00`, which rebuilds the doll from its unit or file)
/// is not modelled.
pub(super) fn show(lua: &Lua, h: FrameHandle) {
    match kind(lua, h) {
        Some(FrameKind::GameTooltip) => super::tooltip::show_override(lua, h),
        _ => base_show(lua, h),
    }
}

/// The hide slot (`+0x84`): the kind's override, else the base [`base_hide`]. GameTooltip's is
/// `0x530a60`. CharacterModelBase's (`0x505ce0`, which releases the doll's model) is not
/// modelled.
pub(super) fn hide(lua: &Lua, h: FrameHandle) {
    match kind(lua, h) {
        Some(FrameKind::GameTooltip) => super::tooltip::hide_override(lua, h),
        _ => base_hide(lua, h),
    }
}

fn kind(lua: &Lua, h: FrameHandle) -> Option<FrameKind> {
    lua.app_data_ref::<Model>()
        .expect("model app_data")
        .arena
        .frame(h)
        .map(|f| f.kind)
}

/// The shown bit alone (`[+0xd0]`), which an override writes before it re-dispatches.
pub(super) fn set_shown_bit(lua: &Lua, h: FrameHandle, shown: bool) {
    if let Some(f) = model_mut(lua).arena.frame_mut(h) {
        f.shown = shown;
    }
}

/// The base show transition (`0x76ae10`): the frame's own step, its children's transitions in
/// link order, the toplevel raise (`0x76aeec`), then its own OnShow (`0x76aef5`).
pub(super) fn base_show(lua: &Lua, h: FrameHandle) {
    {
        let mut model = model_mut(lua);
        if !model.arena.show_step(h) {
            return;
        }
        // Only a frame entering the mouse index schedules a hover re-pick: the show's insert
        // (`0x764780` → `0x76afa0`, flag 0) reaches `0x764aa0` for a kind whose bit is set, and
        // its tail arms `[root+0x1100]` for the mouse kind alone (`0x764b84`, `0x764b8d`). A
        // frame that takes no mouse arms nothing; a parent's mouse-enabled children arm it
        // through their own transitions below.
        if takes_mouse(&model, h) {
            model.hover_repick = true;
        }
    }
    walk_children(lua, h, show);
    super::object::toplevel::raise_on_show(&mut model_mut(lua), h);
    notify(lua, h, true);
}

/// The base hide transition (`0x76ad50`): the frame's own step with its hover leave, its
/// children's transitions in link order, then its own OnHide (`0x76adfd`).
pub(super) fn base_hide(lua: &Lua, h: FrameHandle) {
    let left = {
        let mut model = model_mut(lua);
        if !model.arena.hide_step(h) {
            return;
        }
        leave_mouse_index(&mut model, h)
    };
    if let Some(script) = left {
        super::mouseover::leave(lua, h, script);
    }
    walk_children(lua, h, hide);
    // `CSimpleButton`'s hide notify (`+0x34`, `0x7791e0`) un-presses it, unless disabled or
    // locked, before the base notify, so a button hidden while held comes back unpushed.
    super::button::edge(&mut model_mut(lua), h, ButtonState::on_hide);
    notify(lua, h, false);
}

/// Run `step` on each child of `parent` as a live walk reaches it
/// ([`crate::widget::WidgetArena::next_child`]), with no borrow held across the step.
fn walk_children(lua: &Lua, parent: FrameHandle, step: fn(&Lua, FrameHandle)) {
    let mut walk = ChildWalk::default();
    loop {
        let next = lua
            .app_data_ref::<Model>()
            .expect("model app_data")
            .arena
            .next_child(parent, &mut walk);
        let Some(child) = next else { break };
        step(lua, child);
    }
}

/// Whether the frame is in the mouse input index when visible: its mouse bit (`[+0xcc]` bit 2),
/// or a hyperlink span, which the reference gives a mouse-enabled child of its own (`0x7a3240`).
fn takes_mouse(model: &Model, h: FrameHandle) -> bool {
    model.arena.is_mouse_enabled(h)
        || model
            .link_spans
            .get(&h)
            .is_some_and(|spans| !spans.is_empty())
}

/// The Lua `EnableMouse` (`0x777070`). Enabling inserts with flag -1 and arms nothing
/// (`0x77710d` → `0x76af00(2, -1)`, gate `0x764b82`). Disabling (`0x77711a` → `0x76af50(2)`) runs
/// the removal tail for a frame that was in the mouse index, which is a visible one with the bit
/// set (`0x76af67`, `0x76af6b`); the bit clears after it (`0x76af8e`), so the leave's `OnLeave`
/// still sees the frame mouse-enabled.
pub(super) fn set_mouse_enabled(lua: &Lua, h: FrameHandle, enabled: bool) {
    let left = {
        let mut model = model_mut(lua);
        let in_index = model.arena.is_mouse_enabled(h)
            && model.arena.frame(h).is_some_and(|f| f.effective_visible);
        if enabled || !in_index {
            model.arena.set_mouse_enabled(h, enabled);
            return;
        }
        leave_mouse_index(&mut model, h)
    };
    if let Some(script) = left {
        super::mouseover::leave(lua, h, script);
    }
    model_mut(lua).arena.set_mouse_enabled(h, false);
}

/// The mouse kind's removal tail (`0x764ba0`, kind 2, flag 0), shared by the hide and a mouse
/// disable: the capture and the press it took drop, then the hover ends. `Some` when the frame
/// held the hover, holding the id whose `OnLeave` its leave notify fires.
fn leave_mouse_index(model: &mut Model, h: FrameHandle) -> Option<Option<u32>> {
    drop_capture(model, h);
    leave_if_hovered(model, h)
}

/// The removal tail's capture clear (`0x764ca5`–`0x764cad`): a frame leaving the mouse index
/// that holds the capture slot (`root+0x80`) empties it. `0x766420` delivers a release to the
/// capture alone, so the press this frame took reaches no one: no `OnMouseUp`, no click, and no
/// world click, since the world hook `0x492b50` runs only in freelook. The button stays held
/// until its release, which then finds nothing.
fn drop_capture(model: &mut Model, h: FrameHandle) {
    if model.mouse_capture == Some(h) {
        model.mouse_capture = None;
    }
    for pressed in model.mouse_down_on.values_mut() {
        if *pressed == Some(h) {
            *pressed = None;
        }
    }
    // Its drag arm (`+0x100`) is polled only through the capture and the index
    // (`0x7661ca`, `0x7661cd`), and the frame is out of both: nothing starts or stops it.
    if model.drag.as_ref().is_some_and(|d| d.source == h) {
        model.drag = None;
    }
}

/// The hide's de-registration (`0x764920`, at `0x76ad9d`, before the child walk) ends a hover on
/// this frame: the removal tail (`0x764ba0`) clears the hover, schedules a re-pick (`0x764cbb`)
/// and calls the leave notify with 1 (`0x764cce`), whose base `0x76b6f0` clears the drag-arm
/// (`+0x100`/`+0x104`), so the hovered frame's OnLeave runs before its own OnHide and its
/// ancestors'.
/// The leave is a virtual call ([`super::mouseover::leave`]): a disabled Button's own `0x7794e0`
/// skips its OnLeave. This leave is how a tooltip closes with its window; the reference has no
/// other. `Some` when the frame held the hover, holding the id whose OnLeave fires.
fn leave_if_hovered(model: &mut Model, h: FrameHandle) -> Option<Option<u32>> {
    if model.mouseover != Some(h) {
        return None;
    }
    let notified = super::button::hover_notify_runs(model, Some(h));
    model.mouseover = None;
    if model.drag.as_ref().is_some_and(|d| d.source == h) {
        model.drag = None;
    }
    model.hover_repick = true;
    Some(notified.then(|| model.frame_id(h)))
}

/// The notify slot (`+0x30` OnShow, `+0x34` OnHide): the base fires the script unless the frame is
/// in its load window (`0x76b260`/`0x76b290`), and the EditBox's overrides then take or drop the
/// focus (`0x77a750`/`0x77a780`, base first).
fn notify(lua: &Lua, h: FrameHandle, visible: bool) {
    let id = {
        let mut model = model_mut(lua);
        match model.arena.frame(h) {
            Some(f) if f.load_window => None,
            Some(_) => Some(model.frame_id(h)),
            None => return,
        }
    };
    if let Some(id) = id {
        let script = if visible { "OnShow" } else { "OnHide" };
        let fired = event::fire_widget_handler(lua, id, script, Vec::new());
        record(lua, fired);
    }
    super::editbox::visibility_focus(lua, h, visible);
}

/// Open or close a frame's load window: the pre-load hook's `+0x114 = 1` (`0x7697c2`) and the
/// post-load hook's first write, `+0x114 = 0` (`0x76a2ff`).
pub(crate) fn set_load_window(lua: &Lua, wrapper: &Table, open: bool) {
    let Ok(h) = super::object::frame_handle_of(lua, wrapper) else {
        return;
    };
    if let Some(f) = model_mut(lua).arena.frame_mut(h) {
        f.load_window = open;
    }
}

/// The post-load hook's OnShow (`0x76a38c`–`0x76a3b3`), after the children and the OnLoad: the
/// script alone, through the runner and not the notify slot, so no raise and no EditBox focus,
/// when the frame is visible, has the handler and is out of its load window. It has no OnHide
/// counterpart, and it fires whatever OnShows the frame already had.
pub(crate) fn post_load_show(lua: &Lua, wrapper: &Table) {
    let Ok(h) = super::object::frame_handle_of(lua, wrapper) else {
        return;
    };
    let id = {
        let mut model = model_mut(lua);
        match model.arena.frame(h) {
            Some(f) if f.effective_visible && !f.load_window => model.frame_id(h),
            _ => return,
        }
    };
    let fired = event::fire_widget_handler(lua, id, "OnShow", Vec::new());
    record(lua, fired);
}

fn model_mut(lua: &Lua) -> mlua::AppDataRefMut<'_, Model> {
    lua.app_data_mut::<Model>().expect("model app_data")
}

/// A handler error is recorded in [`Model::errors`], never propagated to the verb.
fn record(lua: &Lua, fired: mlua::Result<()>) {
    if let Err(e) = fired {
        model_mut(lua).record_lua_error(&e);
    }
}
