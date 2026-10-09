//! The show and hide transitions with their notifies (`0x76ae10`, `0x76ad50`). Each marks its
//! frame before anything runs, walks the live child list re-gating each child as the walk reaches
//! it, and fires its own notify last, so handlers run inside the walk, children before their
//! parent, and a frame an earlier handler hid is never notified.

use mlua::{Lua, Table, Value};

use super::{event, Model};
use crate::widget::{ButtonState, ChildWalk, FrameHandle};

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

/// The show transition (`0x76ae10`, slot `+0x88`): the frame's own step, its children's
/// transitions in link order, the toplevel raise (`0x76aeec`), then its own OnShow (`0x76aef5`).
pub(super) fn show(lua: &Lua, h: FrameHandle) {
    {
        let mut model = model_mut(lua);
        if !model.arena.show_step(h) {
            return;
        }
        // Entering the mouse index schedules a hover re-pick (`0x764b8d`).
        model.hover_repick = true;
    }
    walk_children(lua, h, show);
    super::object::toplevel::raise_on_show(&mut model_mut(lua), h);
    notify(lua, h, true);
}

/// The hide transition (`0x76ad50`, slot `+0x84`): the frame's own step with its hover leave,
/// its children's transitions in link order, then its own OnHide (`0x76adfd`).
pub(super) fn hide(lua: &Lua, h: FrameHandle) {
    let left = {
        let mut model = model_mut(lua);
        if !model.arena.hide_step(h) {
            return;
        }
        leave_if_hovered(&mut model, h)
    };
    if let Some(id) = left {
        let fired = event::fire_widget_handler(lua, id, "OnLeave", vec![Value::Boolean(true)]);
        record(lua, fired);
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

/// The hide's de-registration (`0x764920`, at `0x76ad9d`, before the child walk) ends a hover on
/// this frame: the removal tail (`0x764ba0`) clears the hover and the drag-arm
/// (`+0x100`/`+0x104`), schedules a re-pick (`0x764cbb`) and calls the leave notify with 1
/// (`0x764cce`), so the hovered frame's OnLeave runs before its own OnHide and its ancestors'.
/// The leave is a virtual call: a disabled Button's own `0x7794e0` skips its OnLeave. This leave is
/// how a tooltip closes with its window; the reference has no other. The id whose OnLeave fires.
fn leave_if_hovered(model: &mut Model, h: FrameHandle) -> Option<u32> {
    if model.mouseover != Some(h) {
        return None;
    }
    let notified = super::button::hover_notify_runs(model, Some(h));
    model.mouseover = None;
    if model.drag.as_ref().is_some_and(|d| d.source == h) {
        model.drag = None;
    }
    model.hover_repick = true;
    notified.then(|| model.frame_id(h))
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
        model_mut(lua).record_script_error(e.to_string());
    }
}
