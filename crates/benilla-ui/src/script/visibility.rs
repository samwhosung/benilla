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
/// `0x530a80`. CharacterModelBase's (`0x505d00`, which asks for a fresh copy of its subject) runs
/// in the arena's show mark ([`crate::widget::WidgetArena::show_mark`]), on every
/// hidden-to-visible edge however the show came; the app builds the copy.
pub(super) fn show(lua: &Lua, h: FrameHandle) {
    match kind(lua, h) {
        Some(FrameKind::GameTooltip) => super::tooltip::show_override(lua, h),
        _ => base_show(lua, h),
    }
}

/// The hide slot (`+0x84`): the kind's override, else the base [`base_hide`]. GameTooltip's is
/// `0x530a60`. CharacterModelBase's (`0x505ce0`, which releases the copy) runs in the arena's hide
/// step ([`crate::widget::WidgetArena::hide_step`]); the next show asks for a new one.
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
/// link order, the toplevel raise (`0x76aeec`), then its own OnShow (`0x76aef5`). Past the gate,
/// and before it is marked visible, the frame's layout undefers and resolves what it owes
/// (`0x76ae58`), so its `OnSizeChanged` fires here, ahead of its regions, children and notify;
/// its regions follow the mark.
pub(super) fn base_show(lua: &Lua, h: FrameHandle) {
    // The frame and its title region resolve before it is marked visible (`0x76ae58`,
    // `0x76ae6c`); a show with no handler to fire leaves its resolve to the next one.
    let roots = {
        let mut model = model_mut(lua);
        if !model.arena.show_gate(h) {
            return;
        }
        let owed = model.layout_on_show_frame(h);
        let roots = model.show_frame_roots(h);
        (owed && model.resolve_fires(&roots)).then_some(roots)
    };
    if let Some(roots) = roots {
        resolve_and_fire(lua, roots);
    }
    let roots = {
        let mut model = model_mut(lua);
        model.arena.show_mark(h);
        // Then each shown region starts drawing (`0x76aeac` → `0x77fcb0`), its own resolve after
        // the mark (`0x76ae7b`).
        let owed = model.layout_on_show_regions(h);
        let roots = model.show_region_roots(h);
        (owed && model.resolve_fires(&roots)).then_some(roots)
    };
    if let Some(roots) = roots {
        resolve_and_fire(lua, roots);
    }
    {
        let mut model = model_mut(lua);
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
        model.layout_on_hide(h);
        // The de-registration ends a move or size on this frame before its index removal
        // (`0x76493a`, then `0x764942`).
        super::object::cancel_on_hide(&mut model, h);
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

/// A resolve run inside a binding, as the reference's resolve-now (`0x7680e0(1)`) runs inside
/// the show (`0x76ae58`) and the getters (`0x7a1a1d`): what `roots` owe resolves now, with each
/// deferred object an anchor read reaches on the way (`0x7a2fc0`), and their `OnSizeChanged` fire
/// here, each pulled object's before its reader's, as each one's `ApplyRect` (`0x76b580`) runs
/// inside the read. Size changes the resolve made elsewhere stay queued for the drain, as the
/// reference resolves those there.
pub(in crate::script) fn resolve_and_fire(lua: &Lua, roots: Vec<u32>) {
    let fire: Vec<(u32, f32, f32)> = {
        let mut model = model_mut(lua);
        let mark = model.pending_size_changed.len();
        model.layout_chain = Some(roots);
        super::UiScript::resolve_layout(&mut model);
        let chain = model.layout_chain.take().unwrap_or_default();
        let mut fire = Vec::new();
        for id in chain {
            if let Some(at) = model.pending_size_changed[mark..]
                .iter()
                .position(|e| e.0 == id)
            {
                fire.push(model.pending_size_changed.remove(mark + at));
            }
        }
        fire
    };
    for (id, w, h) in fire {
        event::fire_size_changed(lua, id, w, h);
    }
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

/// Whether the frame is in the mouse input index when visible: its mouse bit (`[+0xcc]` bit 2).
fn takes_mouse(model: &Model, h: FrameHandle) -> bool {
    model.arena.is_mouse_enabled(h)
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
///
/// The pre-load hook defers the frame's layout (`0x7697c2`–`0x769807`). The post-load hook, once
/// the window is down, undefers a visible one (`0x76a2ff`–`0x76a35b`), whose resolve runs there
/// (`0x76a310` → `0x768060(0)` → `0x7680e0(1)`), so its `OnSizeChanged` fires ahead of its
/// `<Frames>` (`0x76a36f`), its `OnLoad` (`0x76a387`) and its `OnShow` (`0x76a3b3`), as a show's
/// does ([`base_show`]).
pub(crate) fn set_load_window(lua: &Lua, wrapper: &Table, open: bool) {
    let Ok(h) = super::object::frame_handle_of(lua, wrapper) else {
        return;
    };
    let roots = {
        let mut model = model_mut(lua);
        if let Some(f) = model.arena.frame_mut(h) {
            f.load_window = open;
        }
        if open {
            model.layout_load_begin(h);
            return;
        }
        let owed = model.layout_load_end(h);
        let roots = model.load_end_roots(h);
        (owed && model.resolve_fires(&roots)).then_some(roots)
    };
    if let Some(roots) = roots {
        resolve_and_fire(lua, roots);
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
