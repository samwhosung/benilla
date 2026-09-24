//! Pointer input dispatch: the hit test, and the mouse move, button and wheel entry points that
//! turn host pointer events into FrameScript handler calls. [`cursor`] owns the payload and
//! gesture state; this file dispatches.
//!
//! A frame captures the cursor when it is mouse-enabled and effective-visible and its rect and
//! effective ScrollFrame clip hold the point (the reference's probe `0x76b020`), taking the first
//! in the order of the hover walk (`0x7661cd` in `0x7660d0`), which [`crate::order::hit_test`]
//! implements.

use mlua::Value;

use crate::layout::Rect;
use crate::order;
use crate::widget::{ButtonState, FrameHandle};

use super::clip::{effective_clip, scroll_clip_sources};
use super::{button, cursor, editbox, event, Model, UiScript};

/// The double-click interval, 300 ms, a constant in the Button mouse-up dispatcher `0x7792d0`
/// (`0x77937b cmp ecx, 0x12c` on the `0x42c010` millisecond clock). It is not the OS double-click
/// time (`GetDoubleClickTime` is not imported) and not a CVar, and there is no distance limit:
/// each release only has to hit the frame.
pub(super) const DOUBLE_CLICK_SECS: f64 = 0.300;

/// Install `GetCursorPosition()` and `GetMouseFocus()`. The cursor is in UI units, as 1.12
/// answers it; the engine's y-up UI space is 1:1 logical pixels, so no scale factor applies.
pub(super) fn install(lua: &mlua::Lua) -> mlua::Result<()> {
    lua.globals().set(
        "GetCursorPosition",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<super::Model>().expect("model app_data");
            Ok(model.cursor_pos)
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

/// Whether `h` is the world frame, whose mouse hits belong to the 3D world; it wins the hover walk
/// only when nothing else mouse-enabled is under the cursor.
fn is_world_handle(model: &Model, h: FrameHandle) -> bool {
    model
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == crate::widget::FrameKind::WorldFrame)
}

/// Whether a press or release target is the world: the world frame, the reference's world click
/// target, or no frame where `WorldFrame.xml` is not loaded (most tests).
fn over_world(model: &Model, target: Option<FrameHandle>) -> bool {
    target.is_none_or(|h| is_world_handle(model, h))
}

/// Whether `(x, y)` falls inside one of this frame's hyperlink spans, the engine half of the
/// reference's per-span `CSimpleHyperlinkButton` children.
fn link_span_hit(model: &Model, fh: FrameHandle, x: f32, y: f32) -> bool {
    model
        .link_spans
        .get(&fh)
        .is_some_and(|spans| spans.iter().any(|(r, _, _)| point_in_rect(*r, x, y)))
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
        let model = self.model_ref();
        let sorted = order::traversal(&model.arena);
        let scroll_sources = scroll_clip_sources(&model);
        order::hit_test(&sorted, |fh| {
            // Mouse-enabled, or over one of the frame's hyperlink spans. The chat frame takes no
            // mouse (`[+0xcc] = 0`); the reference gives each span a mouse-enabled
            // `CSimpleHyperlinkButton` child (`0x7a3240`) that routes its scripts to the parent, so
            // a chat window hits only over a link and drags by its tab, not its body.
            (model.arena.is_mouse_enabled(fh) || link_span_hit(&model, fh, x, y))
                // The nameplate's `+0x3c` veto (`0x7cba30`), not its mouse-enabled bit: while a
                // ground-targeted spell is armed a plate refuses the hit before its rect is
                // tested, so the reticle is placed through it.
                && !model.nameplates.vetoes(fh)
                && model.resolved.get(&fh).is_some_and(|r| {
                    point_in_rect(inset_rect(*r, model.arena.hit_rect_insets(fh)), x, y)
                })
                && effective_clip(&model, &scroll_sources, fh)
                    .is_none_or(|c| point_in_rect(c, x, y))
        })
    }

    /// [`Self::hit_test`] over the frames that take the wheel: the same rect and clip rules, gated
    /// on the wheel flag alone.
    fn hit_test_wheel(&self, x: f32, y: f32) -> Option<u32> {
        let model = self.model_ref();
        let sorted = order::traversal(&model.arena);
        let scroll_sources = scroll_clip_sources(&model);
        let fh = order::hit_test(&sorted, |fh| {
            // The wheel flag alone: the wheel index is separate from the mouse's, so the wheel
            // passes a frame that only takes the mouse, as a scroll pane's chrome does.
            model.arena.is_mouse_wheel_enabled(fh)
                && model.resolved.get(&fh).is_some_and(|r| {
                    point_in_rect(inset_rect(*r, model.arena.hit_rect_insets(fh)), x, y)
                })
                && effective_clip(&model, &scroll_sources, fh)
                    .is_none_or(|c| point_in_rect(c, x, y))
        })?;
        model.frame_to_id.get(&fh).copied()
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

    /// Move the cursor to `(x, y)`. When the hovered frame changes, fires `OnLeave(self, true)` on
    /// the old one if still live and `OnEnter(self, true)` on the new; a disabled Button takes the
    /// hover but fires neither ([`button::hover_notify_runs`]). Drags and frame moves advance
    /// first. Returns the hovered frame id.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> Option<u32> {
        // A drag-active EditBox extends its selection on every move (`0x77a860`).
        editbox::drag_update(&self.lua, x, y);
        let new_id = self.hit_test(x, y);
        #[allow(clippy::type_complexity)]
        let (leave_id, drag_start, enter_id, slider_change, color_change): (
            Option<u32>,
            Option<(u32, String)>,
            Option<u32>,
            Option<(u32, f32)>,
            Option<(u32, f64, f64, f64)>,
        ) = {
            let mut model = self.model_mut();
            model.cursor_pos = (x, y);
            // Drags advance on every move, before the hover test: a drag within one frame crosses
            // no hover boundary.
            let slider_change = super::slider::drag_move(&mut model, x, y);
            // The colour picker fires on every move, not only on a change: it has no change gate.
            let color_change = super::colorselect::drag_move(&mut model, x, y);
            super::object::advance_move(&mut model, (x, y));
            super::object::advance_size(&mut model, (x, y));
            let drag_start = cursor::maybe_start_drag(&mut model, (x, y));
            let new_handle = new_id.and_then(|id| model.id_to_frame.get(&id).copied());
            if new_handle == model.mouseover {
                (None, drag_start, None, slider_change, color_change)
            } else {
                let old_id = model.mouseover.and_then(|h| {
                    model
                        .arena
                        .frame(h)
                        .is_some()
                        .then(|| model.frame_to_id.get(&h).copied())
                        .flatten()
                });
                let old_handle = model.mouseover;
                model.mouseover = new_handle;
                // Crossing a button's boundary changes no button state: the enter/leave notifies
                // (`0x779490`/`0x7794e0`) read `[+0x328]` only as a disabled guard, so a press
                // walked off a button stays pushed until the drag start (`0x7793f0`) un-presses a
                // drag-registered one. A disabled Button runs neither script, on either side, and
                // is still the hover: the reference moves `[root+0x7c]` before either notify.
                let old_id = old_id.filter(|_| button::hover_notify_runs(&model, old_handle));
                let enter_id = new_id.filter(|_| button::hover_notify_runs(&model, new_handle));
                (old_id, drag_start, enter_id, slider_change, color_change)
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
        if let Some(oid) = leave_id {
            if let Err(e) =
                event::fire_widget_handler(&self.lua, oid, "OnLeave", vec![Value::Boolean(true)])
            {
                self.push_error(e);
            }
        }
        if let Some(nid) = enter_id {
            if let Err(e) =
                event::fire_widget_handler(&self.lua, nid, "OnEnter", vec![Value::Boolean(true)])
            {
                self.push_error(e);
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
            .and_then(|rh| model.region_resolved.get(&rh))
            .is_some_and(|r| point_in_rect(*r, x, y))
    }

    /// A mouse button press or release at `(x, y)`; `button` is the WoW name (`"LeftButton"`, …).
    /// `OnClick` follows `RegisterForClicks` ([`button::wants_click`], default `{"LeftButtonUp"}`):
    /// a press fires it for `"<Button>ButtonDown"`, a release on the pressed frame for
    /// `"<Button>ButtonUp"`. Its `down` argument is not 1.12, whose firer (`0x779540`) passes only
    /// the button name. Returns whether the UI consumed the event.
    pub fn mouse_button(&mut self, x: f32, y: f32, button: &str, down: bool) -> bool {
        let hit_id = self.hit_test(x, y);
        // ── The raise, first ────────────────────────────────────────────────────────────────────
        //
        // The press handler `0x7662c0` raises the capture (`root+0x80`) else the hover frame
        // (`root+0x7c`) through `0x76a5b0`, called unguarded at `0x766392` (the toplevel walk is
        // inside `0x7650f0`), before the title-region test: a click on a child or a title bar
        // brings its window forward, and a chorded press raises the window being dragged. The
        // release handler `0x766420` never raises.
        if down {
            let target = self.model_ref().mouse_capture;
            if let Some(t) = target.or_else(|| self.hit_test_frame(x, y)) {
                let mut model = self.model_mut();
                super::object::toplevel::raise(&mut model, t);
            }
        }
        // ── The title region swallows the press ─────────────────────────────────────────────────
        //
        // A press inside the title region starts a mode-2 move and ends there: `0x7662c0` tests
        // the region first and only a miss reaches `0x7663e6`, the capture and `OnMouseDown`. So
        // no `OnMouseDown`, drag arm, `OnClick` or double-click stamp, and the event is consumed.
        if down {
            if let Some(h) = self.hit_test_frame(x, y) {
                if self.title_region_hit(h, x, y) {
                    self.model_mut().cursor_pos = (x, y);
                    crate::script::object::movable::start_title_move(&mut self.model_mut(), h);
                    return true;
                }
            }
        }
        // ── The release ends a title move ───────────────────────────────────────────────────────
        //
        // `0x766420` cancels move modes 1 (modifier drag) and 2 (title region) on release and
        // leaves mode 3 (`StartMoving`) to the addon's `StopMovingOrSizing`;
        // `FrameMove::auto_stop` is that distinction.
        if !down
            && self
                .model_ref()
                .moving
                .as_ref()
                .is_some_and(|m| m.auto_stop)
        {
            self.model_mut().moving = None;
        }
        // The press this release belongs to, read before the borrow below drains `mouse_down_on`.
        let captured_id: Option<u32> = if down {
            None
        } else {
            let model = self.model_ref();
            model
                .mouse_down_on
                .get(button)
                .and_then(|h| model.frame_to_id.get(h).copied())
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
        #[allow(clippy::type_complexity)]
        let (
            click_id,
            drag_release,
            world_dropped,
            slider_jump,
            double_id,
            abandoned,
            color_jump,
        ): (
            Option<u32>,
            Option<cursor::DragRelease>,
            bool,
            Option<(u32, f32)>,
            Option<u32>,
            Option<crate::widget::FrameHandle>,
            Option<(u32, f64, f64, f64)>,
        ) = {
            let mut model = self.model_mut();
            let hit_handle = hit_id.and_then(|id| model.id_to_frame.get(&id).copied());
            if down {
                let displaced = match hit_handle {
                    Some(h) => model.mouse_down_on.insert(button.to_string(), h),
                    None => model.mouse_down_on.remove(button),
                };
                // The press edge, `SetButtonState(PUSHED)` (`0x7792b1`), for a button registered
                // for this mouse button up or down (`0x779210` tests `m | m << 8` at `0x77924b`).
                if let Some(h) = hit_handle.filter(|&h| button::wants_press_visual(&model, h, button))
                {
                    button::edge(&mut model, h, ButtonState::on_mouse_down);
                }
                // A press while this mouse button still holds another frame means a release was
                // lost (a focus loss); that frame gets it so it does not stay pushed. The
                // reference's capture never loses one.
                if let Some(h) = displaced {
                    button::edge(&mut model, h, ButtonState::on_mouse_up);
                }
                // `0x7663e6` sets the capture `root+0x80` only when none is held: a second button
                // pressed elsewhere does not displace it.
                model.mouse_capture = model.mouse_capture.or(hit_handle);
                // A press replaces any in-flight gesture; a started one gets its `OnDragStop`
                // below, outside this borrow.
                let abandoned = cursor::abandon_drag(&mut model);
                cursor::arm_drag(&mut model, hit_handle, button, (x, y));
                // A left press on a Slider grabs the thumb, or on the track seats the thumb under
                // the cursor first (the returned value jump).
                let jump = if button == "LeftButton" {
                    super::slider::begin_drag(&mut model, hit_handle, x, y)
                } else {
                    None
                };
                // A left press in a ColorSelect's wheel or value strip also jumps the colour to the
                // click point: its press handler calls its cursor handler (`0x78bf70`).
                let color_jump = if button == "LeftButton" {
                    super::colorselect::begin_drag(&mut model, hit_handle, x, y)
                } else {
                    None
                };
                let wants = format!("{button}Down");
                let click = hit_handle
                    .filter(|&h| button::wants_click(&model, h, &wants))
                    .and(hit_id);
                (click, None, false, jump, None, abandoned, color_jump)
            } else {
                let pressed = model.mouse_down_on.remove(button);
                // `0x7664bb` clears the capture only when no button is still held, so a chorded
                // release keeps it.
                if model.mouse_down_on.is_empty() {
                    model.mouse_capture = None;
                }
                if button == "LeftButton" {
                    super::slider::end_drag(&mut model);
                    super::colorselect::end_drag(&mut model);
                }
                let same_frame = matches!(
                    (pressed, hit_handle),
                    (Some(p), Some(released)) if p == released
                );
                let release = cursor::take_drag(&mut model, button);
                let started = release.as_ref().is_some_and(|r| r.started);
                // The release edge, `SetButtonState(NORMAL)` (`0x7793de`), on the pressed frame
                // wherever the cursor is, skipped after a started drag (`0x7792df` tests
                // `0x76c040`), which already un-pressed it.
                if let (Some(h), false) = (pressed, started) {
                    button::edge(&mut model, h, ButtonState::on_mouse_up);
                }
                // The world drop: a completed left click on the world, never a drag release, which
                // keeps carrying (`0x495300` runs on the WorldFrame click release). The world is
                // `over_world`, not "no frame": the stock `WorldFrame` covers the screen.
                let dropped = button == "LeftButton"
                    && !started
                    && over_world(&model, hit_handle)
                    && over_world(&model, pressed)
                    && cursor::world_drop_click(&mut model);
                let wants = format!("{button}Up");
                let click = if started {
                    None
                } else {
                    hit_handle
                        .filter(|&h| same_frame && button::wants_click(&model, h, &wants))
                        .and(hit_id)
                };
                // The double click, on the release that qualified as a click: `0x77938d` in the
                // mouse-up dispatcher `0x7792d0` fires `OnDoubleClick` instead of the second
                // `OnClick` (`0x77939d jmp 0x7793b5` skips the single leg), only for a frame with
                // an `OnDoubleClick` script (`[+0x4d4] != 0`). What suppresses the click
                // suppresses it too.
                let target = click.and(hit_handle);
                let double = target.filter(|&h| {
                    model
                        .scripts
                        .get(&h)
                        .is_some_and(|s| s.contains("OnDoubleClick"))
                        && model
                            .last_click
                            .get(&h)
                            .is_some_and(|&t| now - t <= DOUBLE_CLICK_SECS)
                });
                if let Some(h) = double {
                    // A double clears the stamp (`0x779393`), so clicks pair: four fast clicks are
                    // click, double, click, double.
                    model.last_click.remove(&h);
                } else if let Some(h) = target {
                    // A single stamps the detector (`0x7793af`) whether or not an `OnClick` script
                    // is bound.
                    model.last_click.insert(h, now);
                }
                let double_id = double.and_then(|h| model.frame_to_id.get(&h).copied());
                let click = if double_id.is_some() { None } else { click };
                (click, release, dropped, None, double_id, None, None)
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
        let script = if down { "OnMouseDown" } else { "OnMouseUp" };
        if let Some(id) = if down { hit_id } else { captured_id } {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, script, vec![btn.clone()]) {
                self.push_error(e);
            }
        }
        // A release in a hyperlink span fires `OnHyperlinkClick(link, markup, button)` on the
        // owning frame, except after a started drag.
        if !down && !drag_release.as_ref().is_some_and(|r| r.started) {
            if let Some(id) = hit_id {
                let span = {
                    let model = self.model_ref();
                    model
                        .id_to_frame
                        .get(&id)
                        .and_then(|fh| model.link_spans.get(fh))
                        .and_then(|spans| {
                            spans
                                .iter()
                                .find(|(r, _, _)| point_in_rect(*r, x, y))
                                .map(|(_, l, m)| (l.clone(), m.clone()))
                        })
                };
                if let Some((link, markup)) = span {
                    let args = match (
                        self.lua.create_string(&link),
                        self.lua.create_string(&markup),
                    ) {
                        (Ok(l), Ok(m)) => {
                            vec![Value::String(l), Value::String(m), btn.clone()]
                        }
                        _ => Vec::new(),
                    };
                    if !args.is_empty() {
                        if let Err(e) =
                            event::fire_widget_handler(&self.lua, id, "OnHyperlinkClick", args)
                        {
                            self.push_error(e);
                        }
                    }
                }
            }
        }
        // A left press in an EditBox places the cursor, starts a selection drag and focuses it
        // whatever its `autoFocus`, before any `OnClick` (`0x77b800`).
        if button == "LeftButton" {
            if down {
                if let Some(id) = hit_id {
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
        if let Some(release) = drag_release.filter(|r| r.started) {
            self.fire_drag_stop(release.source);
            if let Some(id) = hit_id {
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnReceiveDrag", Vec::new())
                {
                    self.push_error(e);
                }
            }
        }
        if let Some(id) = click_id {
            // Shared with `Click()`: gated on the Button being enabled; a CheckButton toggles
            // before `OnClick` fires.
            button::click_button(&self.lua, id, button, down, false);
        }
        // `OnDoubleClick(self, button)` in place of `OnClick` (`0x77938d call [edx+0x98]`), with
        // the same click sound: its firer `0x779650` is the twin of `OnClick`'s `0x779540` and
        // passes only the button name of the completing release.
        if let Some(id) = double_id {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnDoubleClick", vec![btn]) {
                self.push_error(e);
            }
        }
        // A world-frame hit is not the UI consuming the event: the reference runs its
        // `OnMouseDown` without consuming (`0x483c40`), then the `BUTTON1`/`BUTTON2` binding, the
        // world click. Only a world drop counts.
        hit_id.is_some_and(|id| !self.is_world_frame(id)) || world_dropped
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

    /// A wheel spin at `(x, y)`: fires `OnMouseWheel(self, delta)` on the hit frame, or on its
    /// nearest ancestor with the script when it has none; the reference instead lets its wheel
    /// sweep continue past a frame with no script (`0x76c180`). `delta` passes through as given;
    /// the reference passes only its sign, `+1` or `-1` (`0x76c1a2`).
    pub fn mouse_wheel(&mut self, x: f32, y: f32, delta: f32) {
        let target: Option<u32> = {
            // The wheel flag, not the mouse bit: the reference's wheel dispatcher `0x7664f0` walks
            // its own index and ignores hover, capture and `EnableMouse`.
            let hit = self.hit_test_wheel(x, y);
            let model = self.model_ref();
            hit.and_then(|id| {
                let mut h = *model.id_to_frame.get(&id)?;
                loop {
                    if model
                        .scripts
                        .get(&h)
                        .is_some_and(|s| s.contains("OnMouseWheel"))
                    {
                        // A frame carrying a handler was registered through Lua, so it has an id.
                        return model.frame_to_id.get(&h).copied();
                    }
                    h = model.arena.frame(h)?.parent?;
                }
            })
        };
        if let Some(id) = target {
            if let Err(e) = event::fire_widget_handler(
                &self.lua,
                id,
                "OnMouseWheel",
                vec![Value::Number(f64::from(delta))],
            ) {
                self.push_error(e);
            }
        }
    }
}

/// Point-in-rect in UI space (y-up), inclusive on all four edges, as the reference's probe
/// `0x76b020` tests it.
pub(super) fn point_in_rect(r: Rect, x: f32, y: f32) -> bool {
    x >= r.left && x <= r.right && y >= r.bottom && y <= r.top
}

/// A frame's mouse rect: its resolved rect shrunk by its `[left, right, top, bottom]` hit-rect
/// insets (y-up); an over-shrunk rect collapses to empty rather than inverting.
fn inset_rect(r: Rect, [left, right, top, bottom]: [f32; 4]) -> Rect {
    Rect {
        left: r.left + left,
        right: (r.right - right).max(r.left + left),
        top: r.top - top,
        bottom: (r.bottom + bottom).min(r.top - top),
    }
}
