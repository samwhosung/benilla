//! The host runtime loop: event fan-out ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]), which advances the `GetTime()` clock ([`UiScript::now`]).

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{event, Model, ScriptValue, UiScript};

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
                model_mut().record_lua_error(&e);
            }
            at = next;
        }
    }
    event::fire_all_event_listeners(lua, event, &args);
}

impl super::UiScript {
    /// Advance a frame: the `GetTime()` clock and queued events, then the pump `0x765650` and the
    /// draw pass `0x7657d0`, in that order (`0x764340`, `0x764347`). The pump is the update walk,
    /// each linked frame's update in strata, level and link order: its `OnUpdate(self, elapsed)`
    /// (`0x704f10`) and its kind's own update after it (an EditBox's flush, a tooltip's fade, a
    /// model pane's clock); then the hover re-pick at its tail (`0x7657be`). The draw pass fires
    /// `OnUpdateModel`.
    pub fn tick(&mut self, elapsed: f32) {
        self.model_mut().now += f64::from(elapsed);
        // Events queued since the last tick fire before this tick's OnUpdate.
        let pending = std::mem::take(&mut self.model_mut().pending_events);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        self.update_walk(elapsed);
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
        self.paint_model_panes();
        // `WOW_UI_HANDLERS=<secs>`: last, so its report covers everything this tick fired.
        self.report_handler_profile(elapsed);
    }
}

impl UiScript {
    /// A model pane's update after its `OnUpdate` (`0x76d7f0`): its scene clock advances by
    /// `trunc(elapsed · 1000)` ms, no `+0.5` (`0x76d843`–`0x76d863`, `0x7074b0`), and the advance
    /// tests the armed sequence (`0x7074d8` → `0x719370`) and drains the completions in the same
    /// call (`0x7075af`), so `OnAnimFinished` (`0x76cdc0`) fires here, once per arm, against the
    /// arm the last paint left. A zero advance tests nothing (`0x7074b7`).
    pub(super) fn advance_pane(&mut self, h: FrameHandle, elapsed: f32) {
        let dt_ms = (elapsed * 1000.0).trunc().max(0.0) as u64;
        let finished = {
            let mut model = self.model_mut();
            let model = &mut *model;
            let Some(crate::widget::KindState::Model(m)) =
                model.arena.frame_mut(h).map(|f| &mut f.kind_state)
            else {
                return;
            };
            m.clock_ms += dt_ms;
            let facts = m
                .path
                .as_deref()
                .and_then(|p| model.model_facts.get(&crate::widget::model_key(p)));
            match facts {
                Some(facts) if dt_ms > 0 && m.completion_due(facts) => {
                    if let Some(a) = &mut m.armed {
                        a.finished = true;
                    }
                    Some(model.frame_id(h))
                }
                _ => None,
            }
        };
        if let Some(id) = finished {
            if let Err(e) = event::fire_widget_handler(&self.lua, id, "OnAnimFinished", Vec::new())
            {
                self.push_error(e);
            }
        }
    }

    /// The draw pass's `OnUpdateModel` (`0x7657d0`): each pane's paint fires it (`0x76d1bc`) from
    /// the render callback its layer emit adds to its level's ARTWORK batch (`0x76d160`). Strata
    /// go from 0 (`0x7657ff`); a stratum's batches are emitted before any is drained (`0x765896`,
    /// then `0x7658e2`), level 0 up, a level's frames first linked first (`0x765920`). So the
    /// panes are read per stratum when its turn comes, and a pane an earlier handler of that
    /// stratum hid still paints this frame; only the callback's own gate, a model file set
    /// (`0x76d24c`), is read at the call.
    fn paint_model_panes(&mut self) {
        let mut model = self.model_mut();
        if model.on_update_model_frames.is_empty() {
            return;
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
        drop(model);
        for stratum in crate::order::Strata::ALL {
            let panes: Vec<FrameHandle> = {
                let model = self.model_ref();
                let mut panes: Vec<(u16, u64, FrameHandle)> = model
                    .on_update_model_frames
                    .iter()
                    .filter_map(|&h| {
                        let f = model.arena.frame(h)?;
                        (f.strata == stratum && f.effective_visible && has_model_file(f))
                            .then_some((f.level, u64::from(f.insertion_seq), h))
                    })
                    .collect();
                panes.sort_unstable_by_key(|&(level, seq, _)| (level, seq));
                panes.into_iter().map(|(_, _, h)| h).collect()
            };
            for h in panes {
                let id = {
                    let mut model = self.model_mut();
                    match model.arena.frame(h) {
                        Some(f) if has_model_file(f) => model.frame_id(h),
                        _ => continue,
                    }
                };
                if let Err(e) =
                    event::fire_widget_handler(&self.lua, id, "OnUpdateModel", Vec::new())
                {
                    self.push_error(e);
                }
            }
        }
    }
}

/// A Model-family pane with a file set, resident or streaming (`[widget+0x318] ≠ 0`), which its
/// layer emit and its render callback both require (`0x76d24c`); on any other kind
/// `OnUpdateModel` is inert.
fn has_model_file(f: &crate::widget::Frame) -> bool {
    matches!(&f.kind_state, crate::widget::KindState::Model(m) if m.path.is_some())
}
