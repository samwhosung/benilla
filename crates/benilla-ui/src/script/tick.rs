//! The host runtime loop: event fan-out ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]) and the `GetTime()` clock ([`UiScript::now`]).

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{editbox, event, tooltip, Model, ScriptValue, UiScript};

impl UiScript {
    /// Queue an event for the start of the next tick, behind everything queued earlier and before
    /// that tick's `OnUpdate` pass. A drain runs a step behind its input, so firing at once would
    /// overtake events that input already queued: the soulbind confirm for an item placed into a
    /// worn slot must follow that placement's `CURSOR_UPDATE`, whose handler hides it
    /// (`UIParent.lua:357-361`).
    pub fn queue_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        self.model_mut()
            .pending_events
            .push((event.to_string(), args));
    }

    /// Fire an event to every frame registered for it, shown or not, setting both the
    /// `this`/`event`/`arg1..argN` globals and the `(self, event, ...)` arguments (`0x704d50`,
    /// `0x704f10`); handler errors go to [`UiScript::errors`]. Frames fire in registration order,
    /// as `SignalEvent` (`0x703e50`) walks its tail-appended list (`0x7052d0`), and consumers rely
    /// on it: both ZoneText frames write `PVPInfoTextString` on one event, and the last one wins.
    ///
    /// The walk steps by the next listener saved before each handler runs (`0x703ee8`), so a
    /// handler that unregisters itself does not skip its successor. Deviation: a handler that
    /// unregisters the saved next ends the dispatch, because the reference then reads a freed node.
    pub fn fire_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        fire_event_into(&self.lua, event, args);
    }
}

/// [`UiScript::fire_event`] for a caller holding `&Lua`, such as a Lua binding.
pub(crate) fn fire_event_into(lua: &Lua, event: &str, args: Vec<ScriptValue>) {
    let model_mut = || lua.app_data_mut::<Model>().expect("model app_data set");
    {
        let mut at = {
            let model = model_mut();
            model
                .event_to_frames
                .get(event)
                .and_then(|l| l.first().copied())
        };
        while let Some(h) = at {
            let mut model = model_mut();
            // A saved next that the previous handler unregistered ends the walk.
            let Some(pos) = model
                .event_to_frames
                .get(event)
                .and_then(|l| l.iter().position(|&x| x == h))
            else {
                break;
            };
            let next = model
                .event_to_frames
                .get(event)
                .and_then(|l| l.get(pos + 1).copied());
            let id = model.frame_id(h);
            drop(model);
            if let Err(e) = event::fire_event_handler(lua, id, event, &args) {
                model_mut().record_script_error(e.to_string());
            }
            at = next;
        }
    }
    event::fire_all_event_listeners(lua, event, &args);
}

impl super::UiScript {
    /// The current `GetTime()` value in seconds; the app stamps absolute expiries with it, such as
    /// an aura's `expirationTime`.
    pub fn now(&self) -> f64 {
        self.lua.globals().get("__benilla_now").unwrap_or(0.0)
    }

    /// Start this VM's `GetTime()` clock at `secs`, so a rebuilt VM (a relog, a `ReloadUI`) keeps
    /// the process's clock. The reference's `GetTime` (`0x515ea0`, through `0x42c010` and
    /// `0x42b790`) is `GetTickCount` scaled by 0.001, an OS clock that never restarts, which stock
    /// `Cooldown.lua` relies on (`start > 0`). Set once, at construction; after that only
    /// [`Self::tick`] moves it.
    pub fn set_now(&mut self, secs: f64) {
        if let Err(e) = self.lua.globals().set("__benilla_now", secs) {
            self.push_error(e);
        }
    }

    /// Advance a frame: the `GetTime()` clock, the edit boxes and queued events, then
    /// `OnUpdate(self, elapsed)` on every visible frame that has one (`0x704f10`), then the
    /// engine's own fades, model panes and hover.
    pub fn tick(&mut self, elapsed: f32) {
        let clock = {
            let g = self.lua.globals();
            let now: f64 = g.get("__benilla_now").unwrap_or(0.0);
            g.set("__benilla_now", now + f64::from(elapsed))
        };
        if let Err(e) = clock {
            self.push_error(e);
        }
        // The focused edit box's caret blink (`0x77a790`, on the client's frame tick).
        editbox::tick_blink(&self.lua, elapsed);
        // Then `0x77a790`'s drain of the `OnTextChanged`s an edit only marked (`0x77a7a1`), before
        // the OnUpdate sweep, as in the reference.
        editbox::drain_text_changed(&self.lua);
        // Then the caret flush (`0x77d3e0` → `0x77da80`): `OnCursorChanged` when the caret moved,
        // which `ScrollingEdit_OnUpdate` scrolls by.
        editbox::drain_cursor_changed(&self.lua);
        // Events queued since the last tick fire before this tick's OnUpdate.
        let pending = std::mem::take(&mut self.model_mut().pending_events);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        let ids: Vec<u32> = {
            let mut model = self.model_mut();
            // `SetScript` keeps the OnUpdate list; a destroyed frame's handle compacts out here.
            let frames: Vec<FrameHandle> = model
                .on_update_frames
                .iter()
                .copied()
                .filter(|&h| model.arena.frame(h).is_some_and(|f| f.effective_visible))
                .collect();
            if model
                .on_update_frames
                .iter()
                .any(|&h| model.arena.frame(h).is_none())
            {
                let arena = &model.arena;
                let live: Vec<FrameHandle> = model
                    .on_update_frames
                    .iter()
                    .copied()
                    .filter(|&h| arena.frame(h).is_some())
                    .collect();
                model.on_update_frames = live;
            }
            let mut ids: Vec<u32> = frames.into_iter().map(|h| model.frame_id(h)).collect();
            // Creation order, by frame id: getters settle on demand, so a handler sees what an
            // earlier one did this sweep, and the order must be stable. The reference walks each
            // strata level's list of shown frames (`0x765650`).
            ids.sort_unstable();
            ids
        };
        for id in ids {
            if let Err(e) = event::fire_update_handler(&self.lua, id, elapsed) {
                self.push_error(e);
            }
        }
        self.tick_model_panes(elapsed);
        // The engine's line fades, apart from any OnUpdate script: ScrollingMessageFrame's
        // (`0x788460`) and MessageFrame's (`0x786200`), which also keeps only the lines that fit
        // its height, so its rows are read from the resolved rect before the mutable walk.
        let mut model = self.model_mut();
        let ticked: Vec<FrameHandle> = model.arena.ticked_kinds().to_vec();
        let message_frames: Vec<(FrameHandle, usize)> = ticked
            .iter()
            .copied()
            .filter(|&h| {
                model
                    .arena
                    .frame(h)
                    .is_some_and(|f| matches!(f.kind_state, crate::widget::KindState::Message(_)))
            })
            .map(|h| (h, Self::message_viewport_rows(&model, h)))
            .collect();
        for (h, viewport_rows) in message_frames {
            if let Some(crate::widget::KindState::Message(mf)) =
                model.arena.frame_mut(h).map(|f| &mut f.kind_state)
            {
                mf.tick(elapsed);
                mf.trim_to_viewport(viewport_rows);
            }
        }
        for &h in &ticked {
            let Some(frame) = model.arena.frame_mut(h) else {
                continue;
            };
            if let crate::widget::KindState::ScrollingMessage(smf) = &mut frame.kind_state {
                smf.tick(elapsed);
            }
        }
        drop(model);
        // Tooltip fades: `FadeOut`'s ramp and the hide at its end.
        tooltip::tick_fades(&self.lua);
        // A frame hid or showed under a still cursor: re-run the hover walk at the saved position,
        // as the reference's pump tail does (`0x765650` → `0x7660d0`). The hidden frame's OnLeave
        // already fired, so only the new winner's `OnEnter` fires.
        let repick = {
            let mut model = self.model_mut();
            let due = model.hover_repick;
            model.hover_repick = false;
            due.then_some(model.cursor_pos)
        };
        if let Some((x, y)) = repick {
            self.mouse_move(x, y);
        }
        // `WOW_UI_HANDLERS=<secs>`: last, so its report covers everything this tick fired.
        self.report_handler_profile(elapsed);
    }
}

impl UiScript {
    /// The model panes' per-frame pass, for visible panes, in the reference's order: the pane's
    /// `OnUpdate` (`0x76d7f0`) advances its clock by `trunc(elapsed · 1000)` ms, the paint
    /// (`0x76d1a0`) fires `OnUpdateModel`, and the animate fires `OnAnimFinished` (`0x76cdc0`) once
    /// per arm when the armed sequence completes naturally, a loop's first pass included
    /// (`0x719370`). A hidden pane's clock stands still. Completion is read after `OnUpdateModel`,
    /// whose handler may re-arm.
    fn tick_model_panes(&mut self, elapsed: f32) {
        let dt_ms = (elapsed * 1000.0).trunc().max(0.0) as u64;
        let update_ids: Vec<u32> = {
            let mut model = self.model_mut();
            let ticked: Vec<FrameHandle> = model.arena.ticked_kinds().to_vec();
            for h in ticked {
                let Some(f) = model.arena.frame_mut(h) else {
                    continue;
                };
                if !f.effective_visible {
                    continue;
                }
                if let crate::widget::KindState::Model(m) = &mut f.kind_state {
                    m.clock_ms += dt_ms;
                }
            }
            // Kept by `SetScript`; a destroyed frame's handle compacts out on its first miss.
            if model
                .on_update_model_frames
                .iter()
                .any(|&h| model.arena.frame(h).is_none())
            {
                let arena = &model.arena;
                let live: Vec<FrameHandle> = model
                    .on_update_model_frames
                    .iter()
                    .copied()
                    .filter(|&h| arena.frame(h).is_some())
                    .collect();
                model.on_update_model_frames = live;
            }
            // Only visible Model panes with a file set, resident or streaming: the paint that fires
            // the handler (`0x76d1bc`) is gated on `[widget+0x318] ≠ 0` (`0x76d24c`). On any other
            // kind the script is inert.
            let frames: Vec<FrameHandle> = model
                .on_update_model_frames
                .iter()
                .copied()
                .filter(|&h| {
                    model.arena.frame(h).is_some_and(|f| {
                        f.effective_visible
                            && matches!(&f.kind_state, crate::widget::KindState::Model(m) if m.path.is_some())
                    })
                })
                .collect();
            let mut ids: Vec<u32> = frames.into_iter().map(|h| model.frame_id(h)).collect();
            ids.sort_unstable(); // creation order, as in the OnUpdate sweep
            ids
        };
        for id in update_ids {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnUpdateModel", Vec::new()) {
                self.push_error(e);
            }
        }
        let finished_ids: Vec<u32> = {
            let mut model = self.model_mut();
            let ticked: Vec<FrameHandle> = model.arena.ticked_kinds().to_vec();
            let mut due: Vec<FrameHandle> = Vec::new();
            for h in ticked {
                let Some(f) = model.arena.frame(h) else {
                    continue;
                };
                if !f.effective_visible {
                    continue;
                }
                let crate::widget::KindState::Model(m) = &f.kind_state else {
                    continue;
                };
                let Some(path) = m.path.as_deref() else {
                    continue;
                };
                let Some(facts) = model.model_facts.get(&crate::widget::model_key(path)) else {
                    continue;
                };
                if m.completion_due(facts) {
                    due.push(h);
                }
            }
            let mut ids = Vec::with_capacity(due.len());
            for h in due {
                if let Some(crate::widget::KindState::Model(m)) =
                    model.arena.frame_mut(h).map(|f| &mut f.kind_state)
                {
                    if let Some(a) = &mut m.armed {
                        a.finished = true;
                    }
                }
                ids.push(model.frame_id(h));
            }
            ids.sort_unstable();
            ids
        };
        for id in finished_ids {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnAnimFinished", Vec::new())
            {
                self.push_error(e);
            }
        }
    }
}
