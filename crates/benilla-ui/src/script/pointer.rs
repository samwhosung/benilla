//! Pointer input dispatch: the hit test, and the mouse move, button and wheel entry points that
//! turn host pointer events into FrameScript handler calls. [`cursor`] owns the payload and
//! gesture state; this file dispatches.
//!
//! A frame captures the cursor when it is in the mouse index, which `0x76af00` enters it into
//! while it has the mouse bit (`[+0xcc] & 4`) and is visible (`[+0xd4]`), and the probe `0x76b020`
//! takes the point: its rect is valid, and its hit rect holds the point, as does, for a frame in a
//! scroll child's subtree, its first unflagged ancestor's ([`super::clip::hit_clip`]). The first
//! such frame in the order of the hover walk (`0x7661cd` in `0x7660d0`) wins, which
//! [`crate::order::hit_test`] implements over [`crate::order::pick_order`].

use mlua::Value;

use crate::layout::Rect;
use crate::order;
use crate::widget::{ButtonState, FrameHandle};

use super::clip::{hit_clip, hit_rect};
use super::tick::verb;
use super::{button, cursor, editbox, event, protected, Model, UiScript};

/// The double-click interval, 300 ms, a constant in the Button mouse-up dispatcher `0x7792d0`
/// (`0x77937b cmp ecx, 0x12c` on the `0x42c010` millisecond clock). It is not the OS double-click
/// time (`GetDoubleClickTime` is not imported) and not a CVar, and there is no distance limit:
/// each release only has to hit the frame.
pub(super) const DOUBLE_CLICK_SECS: f64 = 0.300;

/// The cursor as `GetCursorPosition` pushes it (`0x48b820`, the glue's `0x46dad0`): its fractions
/// of the screen root through the reference's rounding ([`crate::layout::LayoutSpace::cursor`]), in
/// the root's units, y up, with no scale divided out.
pub(super) fn cursor_position(model: &super::Model) -> (f64, f64) {
    let (x, y) = model.cursor_pos;
    model.layout_space().cursor(x, y)
}

/// Install `GetCursorPosition()` and `GetMouseFocus()`. The cursor ([`cursor_position`]) has no
/// scale divided out: a caller divides by its frame's `GetEffectiveScale()`, which carries the UI
/// scale inside `UIParent`.
pub(super) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<super::Model>().expect("model app_data");
            Ok(cursor_position(&model))
        })?,
    )?;

    // `GetMouseFocus()` (`0x48df40`): the hover frame `[root+0x7c]`, a disabled Button included, as
    // its frame object (not its name), or nil.
    lua.globals().set(
        "GetMouseFocus",
        lua.create_function(|lua, ()| {
            let id = {
                let mut model = lua.app_data_mut::<super::Model>().expect("model app_data");
                // A frame destroyed since the hover answers nil.
                model
                    .mouseover
                    .filter(|&h| model.arena.frame(h).is_some())
                    .map(|h| model.frame_id(h))
            };
            match id {
                Some(id) => Ok(Value::Table(super::object::frame_wrapper(lua, id)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    Ok(())
}

/// The frame a release of `button` goes to: the release handler reads only the capture
/// (`0x76642b`), which `mouse_down_on` keeps per button as the press that set it, `None` once that
/// frame hid. A button with no press of its own, one a title or layout press took, releases onto
/// whatever capture is held.
fn release_target(model: &Model, button: &str) -> Option<FrameHandle> {
    match model.mouse_down_on.get(button) {
        Some(&pressed) => pressed,
        None => model
            .mouse_capture
            .filter(|&c| model.arena.frame(c).is_some()),
    }
}

/// Whether `h` is the world frame, whose mouse hits belong to the 3D world; it wins the hover walk
/// only when nothing else mouse-enabled is under the cursor.
fn is_world_handle(model: &Model, h: FrameHandle) -> bool {
    model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == crate::widget::FrameKind::WorldFrame)
}

/// Whether `h` is a `Minimap`, whose probe drops its blip on a miss (`0x4eb0c0`).
fn is_minimap_handle(model: &Model, h: FrameHandle) -> bool {
    model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == crate::widget::FrameKind::Minimap)
}

/// Whether a press or release target is the world: the world frame, the reference's world click
/// target, or no frame where `WorldFrame.xml` is not loaded (most tests).
fn over_world(model: &Model, target: Option<FrameHandle>) -> bool {
    target.is_none_or(|h| is_world_handle(model, h))
}

impl UiScript {
    /// Whether a frame id names the world frame: hit-tested like any frame, so scripts on it fire,
    /// but not counted by the app as over the UI.
    pub fn is_world_frame(&self, id: u32) -> bool {
        let model = self.model_ref();
        model
            .id_to_frame
            .get(&id)
            .is_some_and(|h| is_world_handle(&model, *h))
    }

    /// The handle of the frame that captures `(x, y)`, for a caller that checks it against a
    /// handle set taken earlier (an anonymous frame has no name). Needs [`Self::resolve`] first.
    pub fn hit_test_frame(&self, x: f32, y: f32) -> Option<FrameHandle> {
        self.hover_walk(x, y).0
    }

    /// The hover walk's probes: the capture frame's first (`[root+0x80]`, `0x7661ba`–`0x7661ca`),
    /// unconditionally and its answer discarded, then the sweep (`0x7661cd`–`0x766213`). Returns the
    /// first frame whose probe takes the point, and whether a `Minimap` was probed and missed it,
    /// which then drops its blip (`0x4eb0c0`). A frame that takes the hit ends the sweep, so a
    /// window over the Minimap keeps it from the sweep's probe; a held capture is probed anyway.
    fn hover_walk(&self, x: f32, y: f32) -> (Option<FrameHandle>, bool) {
        let model = self.model_ref();
        let sorted = order::pick_order(&model.arena);
        let blip_held = model.minimap_blip.is_some();
        let minimap_missed = std::cell::Cell::new(
            blip_held
                && model.mouse_capture.is_some_and(|c| {
                    is_minimap_handle(&model, c) && !frame_contains(&model, c, x, y)
                }),
        );
        let hit = order::hit_test(&sorted, |fh| {
            // In the mouse index. The chat frame takes no mouse (`[+0xcc] = 0`): its links are
            // Button children of their own ([`super::hyperlink`]), so a chat window hits only over
            // a link and drags by its tab, not its body.
            let indexed = model.arena.is_mouse_enabled(fh);
            // The nameplate's `+0x3c` veto (`0x7cba30`), not its mouse-enabled bit: while a
            // ground-targeted spell is armed a plate refuses the hit before its rect is tested,
            // so the reticle is placed through it.
            let hit = indexed && !model.nameplates.vetoes(fh) && frame_contains(&model, fh, x, y);
            if blip_held && indexed && !hit && is_minimap_handle(&model, fh) {
                minimap_missed.set(true);
            }
            hit
        });
        (hit, minimap_missed.get())
    }

    /// [`Self::hit_test_frame`] as a frame id, the id the app and the Lua bindings use.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<u32> {
        let hit = self.hit_test_frame(x, y)?;
        self.model_ref().frame_to_id.get(&hit).copied()
    }

    /// The name of the frame [`Self::hit_test`] captures at `(x, y)`, for tests and probes.
    pub fn hit_test_name(&self, x: f32, y: f32) -> Option<String> {
        let id = self.hit_test(x, y)?;
        let model = self.model_ref();
        let fh = model.id_to_frame.get(&id).copied()?;
        model.arena.frame(fh)?.name.clone()
    }

    /// Move the cursor to `(x, y)`. When the hovered frame changes, fires `OnLeave` on the old one
    /// if still live and `OnEnter` on the new, both with the arg globals untouched; a disabled
    /// Button takes the hover but fires neither ([`button::hover_notify_runs`]). A frame move or
    /// size in flight takes the move instead, and nothing else runs. Returns the hovered frame id.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Option<u32> {
        {
            let mut model = self.model_mut();
            // The walk saves the event before its move/size check (`0x76615f`), so the re-pick's
            // position follows the cursor through a drag.
            model.cursor_pos = (x, y);
            // A move or size in flight is pumped and the walk returns (`0x766164`–`0x7661a0`)
            // before any probe (`vtbl+0x3c`): no hit test, no drag start (`0x76b320`), no
            // EditBox, slider or colour thumb drag (`0x77a860`, `0x789ba0`, `0x78bd80`), no
            // Minimap probe (`0x4eb0c0`), and the hover holds until the drop.
            if super::object::pump_move_or_size(&mut model, (x, y)) {
                return model
                    .mouseover
                    .and_then(|h| model.frame_to_id.get(&h).copied());
            }
        }
        // A drag-active EditBox extends its selection on every move (`0x77a860`).
        editbox::drag_update(&self.lua, x, y);
        let (new_handle, minimap_missed) = self.hover_walk(x, y);
        let new_id = new_handle.and_then(|h| self.model_ref().frame_to_id.get(&h).copied());
        // The Minimap's probe runs in the sweep, ahead of the leave and the enter.
        if minimap_missed {
            super::mouseover::minimap_probe_miss(&self.lua);
        }
        #[allow(clippy::type_complexity)]
        let (leave, drag_start, enter_id, slider_change, color_change): (
            Option<(FrameHandle, Option<u32>)>,
            Option<(u32, String)>,
            Option<u32>,
            Option<(u32, f32)>,
            Option<(u32, f64, f64, f64)>,
        ) = {
            let mut model = self.model_mut();
            // Drags advance on every move, before the hover test: a drag within one frame crosses
            // no hover boundary.
            let slider_change = super::slider::drag_move(&mut model, x, y);
            // The colour picker fires on every move, not only on a change: it has no change gate.
            let color_change = super::colorselect::drag_move(&mut model, x, y);
            let drag_start = cursor::maybe_start_drag(&mut model, (x, y));
            let new_handle = new_id.and(new_handle);
            if new_handle == model.mouseover {
                (None, drag_start, None, slider_change, color_change)
            } else {
                // A frame destroyed since it took the hover gets no leave.
                let old_handle = model.mouseover.filter(|&h| model.arena.frame(h).is_some());
                let old_id = old_handle.and_then(|h| model.frame_to_id.get(&h).copied());
                model.mouseover = new_handle;
                // Crossing a button's boundary changes no button state: the enter/leave notifies
                // (`0x779490`/`0x7794e0`) read `[+0x328]` only as a disabled guard, so a press
                // walked off a button stays pushed until the drag start (`0x7793f0`) un-presses a
                // drag-registered one. A disabled Button runs neither script, on either side, and
                // is still the hover: the reference moves `[root+0x7c]` before either notify.
                let old_id = old_id.filter(|_| button::hover_notify_runs(&model, old_handle));
                let enter_id = new_id.filter(|_| button::hover_notify_runs(&model, new_handle));
                let leave = old_handle.map(|h| (h, old_id));
                (leave, drag_start, enter_id, slider_change, color_change)
            }
        };
        // A thumb drag fires `OnValueChanged`, outside the borrow.
        if let Some((id, value)) = slider_change {
            if let Err(e) = event::fire_widget_handler(
                &self.lua,
                id,
                "OnValueChanged",
                vec![Value::Number(f64::from(value))],
            ) {
                self.push_error(e);
            }
        }
        if let Some((id, r, g, b)) = color_change {
            if let Err(e) = super::colorselect::fire_by_id(&self.lua, id, r, g, b) {
                self.push_error(e);
            }
        }
        if let Some((id, button)) = drag_start {
            let btn = match self.lua.create_string(&button) {
                Ok(s) => Value::String(s),
                Err(e) => {
                    self.push_error(e);
                    return new_id;
                }
            };
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnDragStart", vec![btn]) {
                self.push_error(e);
            }
        }
        // The old frame's leave, its class's engine half included ([`super::mouseover::leave`]),
        // runs before the new frame's enter (`0x766229`, then `0x766239`).
        if let Some((h, script)) = leave {
            super::mouseover::leave(&self.lua, h, script);
        }
        if let Some(nid) = enter_id {
            // A hyperlink button's enter is its text frame's `OnHyperlinkEnter` (`0x7a34b0`); the
            // base enter's plain runner (`0x76b6e5` → `0x702690`) sets no arg global.
            let routed = new_handle.is_some_and(|h| super::hyperlink::hover(&self.lua, h, true));
            if !routed {
                if let Err(e) = event::fire_widget_handler(&self.lua, nid, "OnEnter", Vec::new()) {
                    self.push_error(e);
                }
            }
            // An enabled Button's enter refreshes its label after the base enter and its
            // `OnEnter` (`0x76b6a0`, then `0x7794ab`).
            if let Some(h) = new_handle {
                button::refresh_label(&mut self.model_mut(), h);
            }
        }
        new_id
    }

    /// Whether `(x, y)` is inside the frame's title region. A new title region has no anchors and
    /// hits nothing until `SetPoint`/`SetAllPoints` (`CreateTitleRegion 0x773910`).
    fn title_region_hit(&self, h: crate::widget::FrameHandle, x: f32, y: f32) -> bool {
        let model = self.model_ref();
        model
            .arena
            .frame(h)
            .and_then(|f| f.title_region)
            .and_then(|rh| model.region_rect(rh))
            .is_some_and(|r| point_in_rect(r, x, y))
    }

    /// `0x765490`'s walk: strata FULLSCREEN_DIALOG down to WORLD, never TOOLTIP (the loop starts
    /// below `root+0xcf4`, `0x7654a3`), each level high to low, and each level's list from its head
    /// (the terminator's `next`, `[level+0x14]`) along each link's `next` (`+4`, `0x766aa0`). A
    /// frame links at the tail (`0x7648c9` → `0x7668c0` → `0x766921`), so that is the oldest link
    /// first, the hover sweep's order. Every shown frame is probed with `0x76b020` alone, whatever
    /// its mouse bit. The first hit is lifted to its nearest toplevel self-or-ancestor
    /// (`0x766fe0`); one with none gives up the rest of its stratum and the walk goes on in the
    /// next below (`0x76554a` → `0x765532`).
    fn layout_drag_target(&self, x: f32, y: f32) -> Option<FrameHandle> {
        let model = self.model_ref();
        let sorted = order::pick_order(&model.arena);
        let mut below = order::Strata::Tooltip;
        loop {
            let hit = order::hit_test(&sorted, |fh| {
                model.arena.frame(fh).is_some_and(|f| f.strata < below)
                    && frame_contains(&model, fh, x, y)
            })?;
            if let Some(t) = super::object::toplevel::nearest_toplevel(&model, hit) {
                return Some(t);
            }
            below = model.arena.frame(hit)?.strata;
        }
    }

    /// A mouse button press or release at `(x, y)`; `button` is the WoW name (`"LeftButton"`, …).
    /// `OnClick` follows `RegisterForClicks` ([`button::wants_click`], default `{"LeftButtonUp"}`):
    /// a press fires it for `"<Button>ButtonDown"`, a release on the pressed frame for
    /// `"<Button>ButtonUp"`. Its `down` argument is not 1.12, whose firer (`0x779540`) passes only
    /// the button name. Returns whether the UI consumed the event.
    pub fn mouse_button(&mut self, x: f32, y: f32, button: &str, down: bool) -> bool {
        // ── The layout drag, ahead of everything ─────────────────────────────────────────────────
        //
        // With `SetLayoutMode` on, a press with Control (`0x41f8f0(1)`) or Alt (`0x41f8f0(2)`) held
        // goes to `0x765490` as drag mode 1, Control asking for a size, and the handler returns
        // consumed whatever that started (`0x766301`–`0x76635b`): no raise, title-region test,
        // capture or press slot.
        if down {
            let (_, ctrl, alt) = self.model_ref().modifiers;
            if self.model_ref().layout_mode != 0 && (ctrl || alt) {
                let target = self.layout_drag_target(x, y);
                let mut model = self.model_mut();
                model.cursor_pos = (x, y);
                if let Some(h) = target {
                    crate::script::object::movable::start_layout_drag(&mut model, h, (x, y), ctrl);
                }
                return true;
            }
        }
        let hit_id = self.hit_test(x, y);
        // ── The press target: the capture, else the hover ──────────────────────────────────────
        //
        // The press handler `0x7662c0` takes `root+0x80` first (`0x76637f`) and the hover
        // `root+0x7c` only when none is held (`0x766389`); everything after, the raise, the
        // title-region test, the capture store and the press slot (`vt+0x68`: `OnMouseDown`, the
        // drag arm, a Button's press art and Down click), runs on that one frame. So a second
        // button pressed over another frame fires on the held one. `mouse_down_on` keeps the
        // frame per button for the release.
        //
        // The hover is what the last walk left. Every press follows a move to its point, so that
        // is the frame under it, except while a move or size holds the slot: the walk then pumps
        // it and returns before its sweep (`0x766164`), and the hover stays the frame it was when
        // the drag began.
        let mut release_lost = false;
        let (press_target, hooked): (Option<crate::widget::FrameHandle>, bool) = if down {
            let mut model = self.model_mut();
            // A press for a button still held means its release never came (a focus loss); the
            // reference's capture never loses one. That frame gets the release edge so it does not
            // stay pushed, and with no other button down the stale capture goes too, so this
            // press is not aimed at it.
            if let Some(entry) = model.mouse_down_on.remove(button) {
                release_lost = true;
                if let Some(stale) = entry {
                    button::edge(&mut model, stale, ButtonState::on_mouse_up);
                }
            }
            if model.mouse_down_on.is_empty() {
                model.mouse_capture = None;
            }
            let held = model
                .mouse_capture
                .filter(|&c| model.arena.frame(c).is_some());
            let hover = if model.moving.is_some() {
                model.mouseover.filter(|&h| model.arena.frame(h).is_some())
            } else {
                hit_id.and_then(|id| model.id_to_frame.get(&id).copied())
            };
            // The world input hook `0x492b50`, which the handler runs on the event record before
            // it picks a frame, and only with no capture held (`0x76635c`-`0x766370`): a right
            // press with a payload on the cursor or a gift wrap pending clears the cursor and is
            // consumed.
            let hooked = held.is_none()
                && button == "RightButton"
                && (model.cursor.is_some() || model.pending_wrap.is_some());
            (held.or(hover), hooked)
        } else {
            (None, false)
        };
        if hooked {
            // `ClearCursor(1, 1)` at `0x492bb5`; the handler returns without a capture, a raise or
            // a press slot, so no frame sees this press and its release finds none to go to.
            self.model_mut().cursor_pos = (x, y);
            verb(&self.lua, cursor::clear_cursor);
            return true;
        }
        let press_id: Option<u32> =
            press_target.and_then(|h| self.model_ref().frame_to_id.get(&h).copied());
        // ── The raise, first ────────────────────────────────────────────────────────────────────
        //
        // `0x76a5b0` is called unguarded at `0x766392` (the toplevel walk is inside `0x7650f0`),
        // before the title-region test: a click on a child or a title bar brings its window
        // forward, and a chorded press raises the window being dragged. The release handler
        // `0x766420` never raises.
        if let Some(t) = press_target {
            let mut model = self.model_mut();
            super::object::toplevel::raise(&mut model, t);
        }
        // ── The title region swallows the press ─────────────────────────────────────────────────
        //
        // A press inside the target's title region starts a mode-2 move and ends there:
        // `0x7662c0` tests the region before the capture (`0x766397`–`0x7663e5`) and only a miss
        // reaches `0x7663e6`, the capture and `OnMouseDown`. So no `OnMouseDown`, drag arm,
        // `OnClick` or double-click stamp, and the event is consumed.
        if let Some(h) = press_target {
            if self.title_region_hit(h, x, y) {
                self.model_mut().cursor_pos = (x, y);
                crate::script::object::movable::start_title_move(&mut self.model_mut(), h);
                return true;
            }
        }
        // ── The release ends a title move, and that is all it does ─────────────────────────────
        //
        // `0x766420` tests the drag mode before anything else (`0x76646d`–`0x76647c`): any release
        // during mode 1 (modifier drag) or 2 (title region) clears the slot (`0x765640`) and
        // returns consumed, with no `OnMouseUp`, drag stop or click on the capture, which it
        // leaves held for a later release. Mode 3 (`StartMoving`) is left to the addon's
        // `StopMovingOrSizing`; `FrameMove::auto_stop` is that distinction. The button is up all
        // the same, so its own press entry goes.
        if !down
            && self
                .model_ref()
                .moving
                .as_ref()
                .is_some_and(|m| m.auto_stop)
        {
            let mut model = self.model_mut();
            model.moving = None;
            model.mouse_down_on.remove(button);
            return true;
        }
        // The frame this release goes to, read before the borrow below drains `mouse_down_on`.
        let captured_id: Option<u32> = if down {
            None
        } else {
            let model = self.model_ref();
            release_target(&model, button).and_then(|h| model.frame_to_id.get(&h).copied())
        };
        // The `GetTime()` clock for the double-click detector, read before the borrow.
        let now = self.now();
        self.model_mut().cursor_pos = (x, y);
        let btn = match self.lua.create_string(button) {
            Ok(s) => Value::String(s),
            Err(e) => {
                self.push_error(e);
                return hit_id.is_some();
            }
        };
        let mut press_art: Option<crate::widget::FrameHandle> = None;
        #[allow(clippy::type_complexity)]
        let (
            click_id,
            drag_release,
            world_dropped,
            slider_jump,
            released,
            abandoned,
            color_jump,
        ): (
            Option<u32>,
            Option<cursor::DragRelease>,
            bool,
            Option<(u32, f32)>,
            Option<crate::widget::FrameHandle>,
            Option<crate::widget::FrameHandle>,
            Option<(u32, f64, f64, f64)>,
        ) = {
            let mut model = self.model_mut();
            let hit_handle = hit_id.and_then(|id| model.id_to_frame.get(&id).copied());
            if down {
                match press_target {
                    Some(h) => model.mouse_down_on.insert(button.to_string(), Some(h)),
                    None => model.mouse_down_on.remove(button),
                };
                // The Button press slot `0x779210` runs both legs only on an enabled button
                // (`0x77922e`) with the cursor inside it (`0x77925c`) and the mouse button
                // registered for it (`0x77924b` tests `m | m << 8`): the Down click for
                // `…ButtonDown`, then the press edge, `SetButtonState(PUSHED)` (`0x7792b1`), for
                // either registration, applied below after the scripts as the slot orders them.
                let inside = press_target.filter(|&h| {
                    button::is_enabled(&model, h)
                        && button::wants_press_visual(&model, h, button)
                        && frame_contains(&model, h, x, y)
                });
                press_art = inside;
                // `0x7663e9` stores the target as the capture: the one held, else the hover.
                model.mouse_capture = press_target;
                // The drag arm `0x76bf70`; a started gesture it displaces gets its `OnDragStop`
                // below, outside this borrow.
                let abandoned =
                    cursor::press_drag(&mut model, press_target, button, (x, y), release_lost);
                // A left press on a Slider grabs the thumb, or on the track seats the thumb under
                // the cursor first (the returned value jump).
                let jump = if button == "LeftButton" {
                    super::slider::begin_drag(&mut model, press_target, x, y)
                } else {
                    None
                };
                // A left press in a ColorSelect's wheel or value strip also jumps the colour to the
                // click point: its press handler calls its cursor handler (`0x78bf70`).
                let color_jump = if button == "LeftButton" {
                    super::colorselect::begin_drag(&mut model, press_target, x, y)
                } else {
                    None
                };
                let wants = format!("{button}Down");
                let click = inside
                    .filter(|&h| button::wants_click(&model, h, &wants))
                    .and(press_id);
                (click, None, false, jump, None, abandoned, color_jump)
            } else {
                let pressed = release_target(&model, button);
                let entry = model.mouse_down_on.remove(button);
                // The pressed frame hid (`None` entry): the capture is gone, so nothing of this
                // release runs, the world click included (`over_world(None)` would take it).
                let capture_lost = entry == Some(None);
                // `0x7664bb` clears the capture only when no button is still held, so a chorded
                // release keeps it.
                if model.mouse_down_on.is_empty() {
                    model.mouse_capture = None;
                }
                if button == "LeftButton" {
                    super::slider::end_drag(&mut model);
                    super::colorselect::end_drag(&mut model);
                }
                let release = cursor::take_drag(&mut model, button);
                let started = release.as_ref().is_some_and(|r| r.started);
                // The world drop: a completed left click on the world, never a drag release, which
                // keeps carrying (`0x495300` runs on the WorldFrame click release). The world is
                // `over_world`, not "no frame": the stock `WorldFrame` covers the screen.
                let dropped = button == "LeftButton"
                    && !started
                    && over_world(&model, hit_handle)
                    && over_world(&model, pressed)
                    && !capture_lost
                    && cursor::world_drop_click(&mut model);
                // The Button release slot `0x7792d0` (below, after `OnMouseUp`) does nothing on a
                // started drag (`0x7792df` tests `0x76c040`), which already un-pressed the button.
                (None, release, dropped, None, pressed.filter(|_| !started), None, None)
            }
        };
        // A track press's value jump fires `OnValueChanged` first, outside the borrow.
        if let Some((id, value)) = slider_jump {
            if let Err(e) = event::fire_widget_handler(
                &self.lua,
                id,
                "OnValueChanged",
                vec![Value::Number(f64::from(value))],
            ) {
                self.push_error(e);
            }
        }
        if let Some((id, r, g, b)) = color_jump {
            if let Err(e) = super::colorselect::fire_by_id(&self.lua, id, r, g, b) {
                self.push_error(e);
            }
        }
        // ── `OnMouseUp` goes to the capture ─────────────────────────────────────────────────────
        //
        // The release handler `0x766420` reads only the capture `[mgr+0x80]` (`0x76642b`), never
        // the hover frame, so a press on A released over B fires `OnMouseUp` on A, and a press
        // that captured nothing fires nothing (`0x766498`). A resize grip dragged out from under
        // the cursor still gets its `OnMouseUp`, so its resize ends.
        //
        // The frame's mouse-up `0x76c040` runs the drag leg first: a release of a button the frame
        // is registered to drag (`0x76c050`), with the drag started (`0x76c055`), fires
        // `OnDragStop` (`0x76c06e`) and the hover frame's `OnReceiveDrag` (`0x76c081`) and returns
        // 1 (`0x76c08f`) before the `OnMouseUp` leg (`0x76c099`). So a drag release fires no
        // `OnMouseUp`; a release of a button the frame does not drag, or of a drag that never
        // started, still does.
        if down {
            if let Some(id) = press_id {
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnMouseDown", vec![btn.clone()])
                {
                    self.push_error(e);
                }
            }
        } else if let Some(release) = drag_release.as_ref().filter(|r| r.started) {
            self.fire_drag_stop(release.source);
            if let Some(id) = hit_id {
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnReceiveDrag", Vec::new())
                {
                    self.push_error(e);
                }
            }
        } else if let Some(id) = captured_id {
            if let Err(e) =
                event::fire_widget_handler(&self.lua, id, "OnMouseUp", vec![btn.clone()])
            {
                self.push_error(e);
            }
        }
        // A left press in an EditBox places the cursor, starts a selection drag and focuses it
        // whatever its `autoFocus`, before any `OnClick` (`0x77b800`).
        if button == "LeftButton" {
            if down {
                if let Some(id) = press_id {
                    editbox::click(&self.lua, id, x, y);
                }
            } else {
                editbox::drag_end(&self.lua);
            }
        }
        // A gesture this press replaced gets its `OnDragStop` first: its handler's
        // `StopMovingOrSizing` frees the engine's one move slot.
        if let Some(source) = abandoned {
            self.fire_drag_stop(source);
        }
        if let Some(id) = click_id {
            // Shared with `Click()`: a CheckButton toggles, then `OnClick` fires on an enabled
            // Button. The mouse raises the gate's click flag around the slot
            // (`0x779275`-`0x77928b`); `Click()` does not.
            let _click = protected::raise(&self.lua, protected::Event::Click);
            button::click_button(&self.lua, id, button, down, false);
        }
        // The press art last (`0x7792b1` follows the Down click `0x779285`): `OnMouseDown`
        // (`0x76bf70`, at the top of the slot) and a Down `OnClick` see the button unpushed, and
        // the release's pushed gate reads what they left.
        if let Some(h) = press_art {
            button::edge(&mut self.model_mut(), h, ButtonState::on_mouse_down);
        }
        if let Some(h) = released {
            self.release_slot(h, button, (x, y), now, &btn);
        }
        // A world-frame hit is not the UI consuming the event: the reference runs its
        // `OnMouseDown` without consuming (`0x483c40`), then the `BUTTON1`/`BUTTON2` binding, the
        // world click. Only a world drop counts.
        hit_id.is_some_and(|id| !self.is_world_frame(id)) || world_dropped
    }

    /// The Button release slot `0x7792d0` past its base (`0x76c040`: `OnMouseUp`, which the caller
    /// has fired; a started drag returns 1 there and never reaches this), on the frame that took the press: a PUSHED button (`0x7792fc`) with the cursor
    /// inside its own rect (`0x779335`), not "the topmost frame under the cursor", and registered
    /// for this button's Up (`0x77934e`) fires `OnDoubleClick` (`0x77938d`) or `OnClick`
    /// (`0x7793a9`); only then does the tail set NORMAL (`0x7793de`) if the button is unlocked and
    /// not disabled (`0x7793c2`..`0x7793d4`), on every path that got past the pushed gate and
    /// wherever the cursor is. So a handler reads PUSHED, and what it sets (a lock, a disable)
    /// decides the tail. The state is read here, after `OnMouseUp`, so a script that un-pushes the
    /// button there cancels the click.
    fn release_slot(
        &mut self,
        h: crate::widget::FrameHandle,
        button: &str,
        (x, y): (f32, f32),
        now: f64,
        btn: &Value,
    ) {
        let (click, double) = {
            let model = self.model_ref();
            if model.arena.frame(h).is_none() {
                return;
            }
            let clicks = button::release_may_click(&model, h)
                && frame_contains(&model, h, x, y)
                && button::wants_click(&model, h, &format!("{button}Up"));
            // The double click: `0x77938d` fires `OnDoubleClick` instead of the second `OnClick`
            // (`0x77939d jmp 0x7793b5` skips the single leg), only for a frame with an
            // `OnDoubleClick` script (`[+0x4d4] != 0`) whose last click was within the interval.
            // What suppresses the click suppresses it too.
            let double = clicks
                && model
                    .scripts
                    .get(&h)
                    .is_some_and(|s| s.contains("OnDoubleClick"))
                && model
                    .last_click
                    .get(&h)
                    .is_some_and(|&t| now - t <= DOUBLE_CLICK_SECS);
            (clicks, double)
        };
        let id = self.model_ref().frame_to_id.get(&h).copied();
        if let (true, Some(id)) = (click, id) {
            // The release raises the click flag around slot `+0x98` or `+0x94` (`0x779352`-`0x7793b8`).
            let _click = protected::raise(&self.lua, protected::Event::Click);
            if double {
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnDoubleClick", vec![btn.clone()])
                {
                    self.push_error(e);
                }
            } else {
                button::click_button(&self.lua, id, button, false, false);
            }
            // A double clears the stamp (`0x779393`), so clicks pair: four fast clicks are click,
            // double, click, double. A single stamps the detector (`0x7793af`) whether or not an
            // `OnClick` script is bound.
            let mut model = self.model_mut();
            if model.arena.frame(h).is_some() {
                if double {
                    model.last_click.remove(&h);
                } else {
                    model.last_click.insert(h, now);
                }
            }
        }
        // The release edge, `SetButtonState(NORMAL)` (`0x7793de`).
        button::edge(&mut self.model_mut(), h, ButtonState::on_mouse_up);
    }

    /// Fire `OnDragStop` on a gesture's source, the one place a started drag ends; a source that
    /// died since the press fires nothing.
    pub(super) fn fire_drag_stop(&mut self, source: crate::widget::FrameHandle) {
        let id = {
            let model = self.model_ref();
            model
                .arena
                .frame(source)
                .is_some()
                .then(|| model.frame_to_id.get(&source).copied())
                .flatten()
        };
        if let Some(id) = id {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnDragStop", Vec::new()) {
                self.push_error(e);
            }
        }
    }
}

/// The probe `0x76b020` alone: whether `(x, y)` is inside the frame's hit rect and, in a scroll
/// child's subtree, its [`hit_clip`], whatever its mouse bit. The Button click legs test it
/// against the capture (`0x77925c`, `0x779335`).
pub(super) fn frame_contains(model: &Model, fh: FrameHandle, x: f32, y: f32) -> bool {
    let Some(clip) = hit_clip(model, fh) else {
        return false;
    };
    model
        .frame_rect(fh)
        .is_some_and(|r| point_in_rect(hit_rect(model, fh, r), x, y))
        && clip.is_none_or(|c| point_in_rect(c, x, y))
}

/// Point-in-rect in UI space (y-up), inclusive on all four edges, as the reference's probe
/// `0x76b020` tests it.
pub(super) fn point_in_rect(r: Rect, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.bottom && y <= r.top
}
