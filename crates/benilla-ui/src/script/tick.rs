//! The host runtime loop — an `impl UiScript` block beside its concern (the `layout.rs` pattern):
//! the event fan-out to registered frames ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]: OnUpdate + the engine-side fades and the model panes' clocks), and the FrameXML session clock
//! ([`UiScript::now`]). The low-level handler-firing these drive lives in [`super::event`].

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{editbox, event, tooltip, Model, ScriptValue, UiScript};

impl UiScript {
    /// Queue an event to fire at the **start of the next tick**, into the same
    /// [`super::Model::pending_events`] list the engine's own bindings use — so it lands *after*
    /// everything queued earlier in the frame and before that tick's `OnUpdate` pass.
    ///
    /// **Ordering, not laziness, is the whole reason this is public** (decision 1750). A benilla
    /// drain runs a step behind the input pass that fed it, so an event it fires *immediately*
    /// arrives BEFORE events the same input already queued — the reverse of the reference, where
    /// the deferral happens inside the very call that queued them. That inversion is not
    /// hypothetical: the soulbind confirm raised by placing an item into a worn slot was cancelled
    /// one tick later by that same place's own `CURSOR_UPDATE`, whose `StaticPopup_Hide` is
    /// `UIParent.lua:356-360` transcribed correctly. Queueing restores the reference's relative
    /// order — cursor-change first, question second — while a *later* cursor change still retires
    /// the question, which is exactly what that arm is for.
    ///
    /// Use [`Self::fire_event`] for anything whose ordering against in-flight queued events does
    /// not matter, which is nearly everything.
    pub fn queue_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        self.model_mut()
            .pending_events
            .push((event.to_string(), args));
    }

    /// Fire an event to every frame registered for it, invoking their `OnEvent` handlers
    /// (`0x704d50`/`0x704f10`): both the legacy `this`/`event`/`arg1..argN` globals *and* the
    /// modern `(self, event, ...)` arguments. Events reach registered frames regardless of
    /// visibility, **in registration order** — the client's `SignalEvent 0x703e50` walks the
    /// per-event listener list head-first (tail-append insert `0x7052d0`: FIFO, first-registered
    /// fires first). Cross-frame order is a LAW consumers depend on: both ZoneText frames write
    /// `PVPInfoTextString` on one event — the last writer decides. Handler errors are collected
    /// into [`UiScript::errors`], never panicking.
    ///
    /// **The walk steps by a next-listener SAVED BEFORE the handler runs** (`0x703ee8: mov
    /// edi,[eax+0x4]` — "save the next node before firing"; decision 1324), never by list index.
    /// That is what makes a handler that UNREGISTERS ITSELF mid-dispatch unable to rob its
    /// successor — and it is load-bearing in the wild: AceEvent-2.0 treats `PLAYER_LOGIN` /
    /// `VARIABLES_LOADED` as fire-once events and unregisters its frame *inside* the handler, so
    /// an index walk skipped whichever addon registered right after AceEvent (Bagnon_Forever's
    /// saved-bag DB never initialized — the director's `SaveBagData:76` error dialogs). A frame
    /// registered mid-dispatch tail-appends and is still visited; a handler that unregisters the
    /// walk's SAVED next stops the dispatch there (the reference frees the node and walks into
    /// its zeroed links — memory-accident territory we render as a deterministic stop, a knowing
    /// divergence from an accident, not from a mechanism).
    pub fn fire_event(&mut self, event: &str, args: Vec<ScriptValue>) {
        fire_event_into(&self.lua, event, args);
    }
}

/// [`UiScript::fire_event`] against the VM directly, for a caller that holds `&Lua` rather than
/// `&UiScript` — which is every Lua binding, and therefore `UpdateSpells` (decision 1924).
///
/// Identical behaviour; `fire_event` is the same call with the script's own VM. The `&mut self` on
/// the method was never load-bearing — the body reaches everything through `lua.app_data_mut()` —
/// and this is the shape [`crate::loader::load_into`] already uses for the same reason (1188 §2).
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
            // The saved handle must still be registered — its removal (by the previous handler)
            // ends the walk, per the doc above.
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
    /// Advance time: run `OnUpdate(self, elapsed)` on every *effectively-visible* frame that has one
    /// (`0x704f10`: OnUpdate → `this` + `arg1 = elapsed`). Errors collected, never panicking.
    /// Also advances the `GetTime()` clock — the FrameXML session clock (seconds, monotonic,
    /// arbitrary epoch like the real client's), kept as the `__benilla_now` global so the stdlib
    /// binding reads it without a host round-trip. Reference FrameXML (CastingBarFrame & co.)
    /// anchors durations on GetTime; the clock advancing in the same call that fires OnUpdate
    /// keeps the two views of time consistent within a frame.
    /// The current `GetTime()` value (`__benilla_now`, seconds) — the FrameXML clock. The app
    /// reads it to stamp an absolute expiry into the clock a Lua countdown reads (the aura feed's
    /// `expirationTime`, decision 0257); it is the same value `GetTime()` returns inside the VM.
    pub fn now(&self) -> f64 {
        self.lua.globals().get("__benilla_now").unwrap_or(0.0)
    }

    /// Start this VM's `GetTime()` clock at `secs` rather than at zero — how a host hands a
    /// **freshly built** VM the clock its process is already on (decision 2116).
    ///
    /// The reference's `GetTime` (`0x515ea0`) is `KERNEL32!GetTickCount` scaled by 0.001 (through
    /// the thunk `0x42c010` → `0x42b790`) — an **OS** clock that knows nothing about the Lua VM and
    /// never restarts, which is exactly what lets stock `Cooldown.lua` gate on `start > 0`. Ours
    /// lives inside the VM, so without this a rebuilt VM — every logout/login, every `ReloadUI` —
    /// would restart it at zero and every host value already converted onto it would land in the
    /// past.
    ///
    /// Set once, at construction: from then on only [`Self::tick`] moves the clock, so the
    /// `(Instant, GetTime)` pair a host keeps beside it stays atomic by construction.
    pub fn set_now(&mut self, secs: f64) {
        if let Err(e) = self.lua.globals().set("__benilla_now", secs) {
            self.push_error(e);
        }
    }

    pub fn tick(&mut self, elapsed: f32) {
        let clock = {
            let g = self.lua.globals();
            let now: f64 = g.get("__benilla_now").unwrap_or(0.0);
            g.set("__benilla_now", now + f64::from(elapsed))
        };
        if let Err(e) = clock {
            self.push_error(e);
        }
        // The focused edit box's caret blink (`0x77a790` runs on the client's frame tick).
        editbox::tick_blink(&self.lua, elapsed);
        // …and, from inside that same `0x77a790` (`0x77a7a1`), the dirty-word drain that fires the
        // `OnTextChanged`s an edit only marked (decision 1831). It runs BEFORE this tick's
        // OnUpdate sweep, matching the reference's order: the box's own OnUpdate override drains
        // before the FrameXML handlers that read the box get their turn.
        editbox::drain_text_changed(&self.lua);
        // …and the caret flush's own fire, the second half of `0x77a790`'s update walk
        // (`0x77d3e0` → `0x77da80`): `OnCursorChanged` when the caret has moved. It is what
        // `ScrollingEdit_OnCursorChanged` records and `ScrollingEdit_OnUpdate` then scrolls by,
        // so this is the mail body and the GM ticket box following the caret as you type.
        editbox::drain_cursor_changed(&self.lua);
        // Events queued by Lua bindings last tick (`Model::pending_events` — e.g. `SetMapZoom` →
        // `WORLD_MAP_UPDATE`; the cursor arc's `CURSOR_UPDATE`/`ITEM_LOCK_CHANGED`/
        // `DELETE_ITEM_CONFIRM`, decision 0216) fire first, so handlers see them before this
        // frame's OnUpdate runs.
        let pending = std::mem::take(&mut self.model_mut().pending_events);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        let ids: Vec<u32> = {
            let mut model = self.model_mut();
            // The OnUpdate population is its own list (decision 1446) — maintained by
            // `SetScript`, `scripts`' one writer — so this reads a few hundred handles instead
            // of re-filtering the whole scripts map. A destroyed frame's handle stays until its
            // liveness check fails once, then compacts here.
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
            // **Deterministic order, by frame id — i.e. creation order.**
            //
            // `model.scripts` is a hash map, so this sweep used to fire OnUpdate in an order that
            // varied per process. That was invisible for as long as every geometry getter answered
            // from the PREVIOUS frame's resolve: each handler saw the same stale world regardless
            // of when it ran. The moment the getters began settling on demand (so a handler can
            // observe work an earlier handler did this same sweep), the order became load-bearing
            // and the macro window's tab width started flipping 164/167 run to run.
            //
            // Random order was always wrong — the reference sweeps a defined order — the staleness
            // was merely hiding it. Creation order is stable, matches FrameXML declaration order,
            // and is what a handler reading a sibling's geometry should see.
            ids.sort_unstable();
            ids
        };
        for id in ids {
            if let Err(e) = event::fire_update_handler(&self.lua, id, elapsed) {
                self.push_error(e);
            }
        }
        self.tick_model_panes(elapsed);
        // Advance every ScrollingMessageFrame's per-line fade (the client's OnUpdate `0x788460`).
        // Independent of the frame's own OnUpdate script — the fade is C++ behavior, not Lua. The
        // AtBottom freeze gate lives inside `ScrollingMessageState::tick`.
        let mut model = self.model_mut();
        // The sibling class's OnUpdate (`0x786200`): the same two-phase fade with no scroll gate,
        // plus the capacity law that is this class's stand-in for `maxLines` — the cap is what fits
        // vertically, so it needs the frame's resolved rect and is collected first (the arena walk
        // below holds a mutable borrow that cannot also read `model.resolved`).
        // Both per-kind walks below ride the arena's ticked-kind registry (decision 1446):
        // dozens of handles instead of two full-arena sweeps per tick.
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
        // Advance fading tooltips (FadeOut's ramp + end-of-ramp hide) — engine behavior like the
        // message fade above, decision 0274.
        tooltip::tick_fades(&self.lua);
        // The hover RE-PICK ([`Model::hover_repick`]): the world under a stationary cursor
        // changed this tick (the hovered frame hid, or a frame was shown over the cursor), so
        // re-run the hover walk at the SAVED cursor position — the reference's own pump tail
        // (`0x765650` → `0x7660d0` with the saved event, didn't-move coalesce bypassed).
        // `mouse_move` at the unchanged point is exactly that walk: the hidden frame's OnLeave
        // already fired at hide time and `mouseover` is clear, so only the new winner's
        // boundary fires — no second OnLeave, `OnEnter` with no physical mouse move.
        let repick = {
            let mut model = self.model_mut();
            let due = model.hover_repick;
            model.hover_repick = false;
            due.then_some(model.cursor_pos)
        };
        if let Some((x, y)) = repick {
            self.mouse_move(x, y);
        }
        // `WOW_UI_HANDLERS=<secs>` — who spent the frame (decision 1395). Last, so a report covers
        // everything this tick fired; a no-op unless the instrument is armed.
        self.report_handler_profile(elapsed);
    }
}

impl UiScript {
    /// The model panes' per-frame pass (decision 2007).
    ///
    /// Three things the reference does each frame for a **visible** model pane, in this order:
    /// the widget's own `OnUpdate` (`0x76d7f0`, walked by the UI pump with the Lua OnUpdates the
    /// caller just fired) advances its private scene clock by `trunc(elapsed · 1000)` ms — a
    /// truncation, no `+0.5` (`76d854 __ftol`); the paint (`vt+0x98`, `0x76d1a0`) fires
    /// `OnUpdateModel` before it builds the scene; and the scene's animate runs the completion
    /// callback (`0x76cdc0`) when the armed sequence has run its length — `OnAnimFinished`, on
    /// natural completion only (mode 0), once per arm, for a looping sequence's first pass as
    /// much as for a clamped one's end (the flag test sits after the enqueue, `0x719370`). A hidden
    /// pane gets none of the three: its
    /// clock stands still and it completes nothing, which is what makes the minimap ping "resume
    /// where the last one stopped".
    ///
    /// The completion is read AFTER `OnUpdateModel` because the handler may re-arm (the cooldown
    /// flips to its flash there), and a fresh arm has nothing to complete.
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
            // The handler population, maintained by `SetScript` like `on_update_frames`; a
            // destroyed frame's handle compacts out on its first miss.
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
            // Visible MODEL panes WITH A MODEL INSTALLED: the handler is `CSimpleModel::LoadXML`'s
            // (`+0x3cc`) and the paint that fires it (`76d1bc`) is reached only past the
            // `[widget+0x318] ≠ 0` gate at `76d24c` — a file set, resident or still streaming;
            // a pane with no file paints nothing and fires nothing. A script of that name on any
            // other kind is inert, as it is in the reference.
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
            ids.sort_unstable(); // creation order — the OnUpdate sweep's own law
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
