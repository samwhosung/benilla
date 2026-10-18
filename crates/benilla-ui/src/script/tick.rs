//! The host runtime loop: event fan-out ([`UiScript::fire_event`]), the per-frame advance
//! ([`UiScript::tick`]), which advances the `GetTime()` clock ([`UiScript::now`]).

use mlua::Lua;

use crate::widget::FrameHandle;

use super::{event, Model, ScriptValue, UiScript};

impl UiScript {
    /// Queue an event for the start of the next tick, behind everything queued earlier and before
    /// that tick's `OnUpdate` pass: the host's lane ([`Model::pending_events`]), for a producer
    /// outside any Lua verb. A drain runs a step behind its input, so firing at once would
    /// overtake events the host already queued. A Lua verb's own events never wait here: they
    /// fire inside it ([`verb`]), so the soulbind confirm for an item placed into a worn slot
    /// follows that placement's `CURSOR_UPDATE`, whose handler hides it (`UIParent.lua:357-361`).
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

impl Model {
    /// Raise `event` from engine code. Inside a [`verb`] segment it fires before the verb
    /// returns, once the segment's borrow drops; anywhere else it joins the host queue
    /// ([`Model::pending_events`]) for the next tick. So a helper shared by a Lua verb and a host
    /// path, such as the cursor's clear, fires in place for the one and queues for the other.
    pub(crate) fn raise(&mut self, event: &str, args: Vec<ScriptValue>) {
        match &mut self.verb_events {
            Some(segment) => segment.push(Raised::Event(event.to_string(), args)),
            None => self.pending_events.push((event.to_string(), args)),
        }
    }

    /// Inside a [`verb`] segment, show `payload` on the cursor to the handlers of the events
    /// raised until [`Model::end_cursor_view`], then put back the state the segment left. A
    /// clear raises its own events this way, as `ClearCursor` signals before it zeroes the mode
    /// (`0x4952bf`). The host queue fires a tick later against the state then, so it takes no
    /// view.
    pub(crate) fn view_cursor(&mut self, payload: Option<super::cursor::CursorPayload>) {
        if let Some(segment) = &mut self.verb_events {
            segment.push(Raised::CursorView(payload));
        }
    }

    /// Set or clear the client's lock on item `guid` ([`Model::item_locks`]). Inside a [`verb`]
    /// segment the change is also recorded, so the segment's handlers see each lock as it stood
    /// when their event was raised, as `LockItem` and `UnlockItem` set the bit before they signal
    /// (`0x495409`, `0x495449`).
    pub(crate) fn set_item_lock(&mut self, guid: u64, locked: bool) {
        let was = if locked {
            !self.item_locks.insert(guid)
        } else {
            self.item_locks.remove(&guid)
        };
        if let Some(segment) = &mut self.verb_events {
            segment.push(Raised::Lock { guid, locked, was });
        }
    }

    /// End the view [`Model::view_cursor`] opened.
    pub(crate) fn end_cursor_view(&mut self) {
        if let Some(segment) = &mut self.verb_events {
            segment.push(Raised::EndCursorView);
        }
    }
}

/// What a [`verb`] segment raised, in order.
#[derive(Clone, Debug)]
pub(crate) enum Raised {
    /// An event, with its arguments.
    Event(String, Vec<ScriptValue>),
    /// Show this payload on the cursor until the matching [`Raised::EndCursorView`].
    CursorView(Option<super::cursor::CursorPayload>),
    /// A lock change on item `guid`: `locked` after it, `was` before it.
    Lock { guid: u64, locked: bool, was: bool },
    /// Put back the cursor the segment left.
    EndCursorView,
}

/// Run one segment of a Lua verb with the model borrowed, then fire every event it raised, in
/// order, after the borrow drops and before the binding returns, as `SignalEvent` (`0x703e50`)
/// and `SignalEvent2` (`0x703f50`) walk the listeners in place. A verb whose reference signals
/// and then changes state a getter reads runs one segment per such step, so each handler sees
/// the state the reference's sees. A handler that calls another verb runs that verb's segments
/// inside this fan-out, the reference's synchronous nesting.
pub(crate) fn verb<R>(lua: &Lua, body: impl FnOnce(&mut Model) -> R) -> R {
    let (out, raised) = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data set");
        let outer = model.verb_events.replace(Vec::new());
        debug_assert!(outer.is_none(), "a verb segment opened inside another");
        let out = body(&mut model);
        (out, model.verb_events.take().unwrap_or_default())
    };
    // The body's lock changes are rolled back, then replayed in place as the absolute writes they
    // were, in the reference's order against the handlers: a nested verb's change to the same item
    // between two of them is overwritten by the later write, as the reference's later `LockItem`
    // or `UnlockItem` would overwrite it. What the body cannot match is a read: it ran before any
    // handler, so it never sees a handler's change.
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data set");
        for raised in raised.iter().rev() {
            if let Raised::Lock { guid, was, .. } = *raised {
                set_lock_bit(&mut model, guid, was);
            }
        }
    }
    // The cursors a view replaced, innermost last.
    let mut hidden = Vec::new();
    for raised in raised {
        match raised {
            Raised::Event(event, args) => fire_event_into(lua, &event, args),
            Raised::CursorView(payload) => {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data set");
                hidden.push(std::mem::replace(&mut model.cursor, payload));
            }
            Raised::EndCursorView => {
                if let Some(real) = hidden.pop() {
                    lua.app_data_mut::<Model>()
                        .expect("model app_data set")
                        .cursor = real;
                }
            }
            Raised::Lock { guid, locked, .. } => {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data set");
                set_lock_bit(&mut model, guid, locked);
            }
        }
    }
    out
}

/// Set item `guid`'s lock bit to `locked`, recording nothing.
fn set_lock_bit(model: &mut Model, guid: u64, locked: bool) {
    if locked {
        model.item_locks.insert(guid);
    } else {
        model.item_locks.remove(&guid);
    }
}

/// The ten `arg1..arg10` of a `CHAT_MSG_SYSTEM` line with no sender: the text, five empty
/// strings, two zeros, an empty string and a zero, the shape the app's chat feed gives its own
/// system lines.
pub fn system_chat_args(text: String) -> Vec<ScriptValue> {
    let empty = || ScriptValue::Str(String::new());
    vec![
        ScriptValue::Str(text),
        empty(),
        empty(),
        empty(),
        empty(),
        empty(),
        ScriptValue::Int(0),
        ScriptValue::Int(0),
        empty(),
        ScriptValue::Int(0),
    ]
}

/// `0x496720` on a chat-kind catalog row from inside a verb: the line reaches the chat frames at
/// once, as `CHAT_MSG_SYSTEM` (`0x496842` into `0x49a870`, whose event is `0x49ac9a`), and waits
/// with its key for the app's log files and the row's sound ([`UiScript::take_shown_chat_lines`]).
pub(crate) fn show_chat_line_into(lua: &Lua, key: &'static str, text: String) {
    fire_event_into(lua, "CHAT_MSG_SYSTEM", system_chat_args(text.clone()));
    lua.app_data_mut::<Model>()
        .expect("model app_data set")
        .shown_chat_lines
        .push((key, text));
}

/// [`UiScript::fire_event`] for a caller holding `&Lua`: `SignalEvent` (`0x703e50`, with args
/// `0x703f50`). An event with no listener touches nothing; otherwise `event` is set once before
/// the walk (`0x703ea4`) and the earlier value put back after it (`0x703f04`), so a handler's
/// assignment to it reaches the next listener. The walk takes each listener's successor before
/// its call (`0x703ee8`): one appended meanwhile is reached unless the running one was the tail.
pub(crate) fn fire_event_into(lua: &Lua, event: &str, args: Vec<ScriptValue>) {
    let model_mut = || lua.app_data_mut::<Model>().expect("model app_data set");
    let mut at = {
        let model = model_mut();
        model
            .event_to_frames
            .get(event)
            .and_then(|l| l.first().copied())
    };
    if at.is_none() {
        return;
    }
    let globals = lua.globals();
    let saved = globals.get::<mlua::Value>("event").ok();
    if let Ok(name) = lua.create_string(event) {
        let _ = globals.set("event", name);
    }
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
        if let Err(e) = event::fire_event_handler(lua, id, &args) {
            model_mut().record_lua_error(&e);
        }
        at = next;
    }
    if let Some(saved) = saved {
        let _ = globals.set("event", saved);
    }
}

impl super::UiScript {
    /// The catalog lines a verb showed in chat since the last call, by message key, for the log
    /// files and the sound the app keeps.
    pub fn take_shown_chat_lines(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.model_mut().shown_chat_lines)
    }

    /// Advance a frame: the `GetTime()` clock and queued events, then the pump `0x765650` and the
    /// draw pass `0x7657d0`, in that order (`0x764340`, `0x764347`). The pump drains the layout
    /// queue (`0x768ed0`, at `0x7656ba`), whose rect applications fire `OnSizeChanged`; runs the
    /// update walk, each linked frame's update in strata, level and link order: its
    /// `OnUpdate(self, elapsed)` (`0x704f10`) and its kind's own update after it (an EditBox's
    /// flush, a tooltip's fade, a model pane's clock, a message frame's line fades, a scroll frame's
    /// range); drains the layout again (`0x765799`); then re-picks the hover against those rects
    /// (`0x7657be`). The draw pass fires `OnUpdateModel`.
    pub fn tick(&mut self, elapsed: f32) {
        self.model_mut().nameplates.begin_update();
        self.model_mut().now += f64::from(elapsed);
        // Events queued since the last tick fire before this tick's OnUpdate.
        let pending = std::mem::take(&mut self.model_mut().pending_events);
        for (event, args) in pending {
            self.fire_event(&event, args);
        }
        // The item tooltips the caches answered since the last tick rebuild (`0x52e530`).
        for e in super::tooltip_item::build::answer_item_caches(&self.lua) {
            self.push_error(e);
        }
        self.resolve();
        self.update_walk(elapsed);
        self.resolve();
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
        let dt_ms = crate::widget::pane_clock_step_ms(elapsed);
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
    /// stratum hid still paints this frame; only the callback's own gates ([`paints`]) are read at
    /// the call. The rects are the ones the pump's last drain (`0x765799`) left: the paint reads
    /// them through `0x768320`, which never resolves.
    ///
    /// A character pane's paint then runs its turn timer (`0x505c50`, after the root step
    /// `0x76d1a0` that fires `OnUpdateModel`), whether or not it has the handler.
    fn paint_model_panes(&mut self) {
        let mut model = self.model_mut();
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
        // The character panes paint whether or not a script listens: their turn timer runs on
        // whatever `+0x318` holds.
        let copies: Vec<FrameHandle> = model
            .arena
            .model_kinds()
            .iter()
            .copied()
            .filter(|&h| {
                model
                    .arena
                    .frame(h)
                    .is_some_and(|f| f.effective_visible && is_character(f) && has_model_file(f))
            })
            .collect();
        if model.on_update_model_frames.is_empty() && copies.is_empty() {
            return;
        }
        drop(model);
        for stratum in crate::order::Strata::ALL {
            let panes: Vec<(FrameHandle, bool)> = {
                let model = self.model_ref();
                let scripted = &model.on_update_model_frames;
                let mut panes: Vec<(u16, u64, FrameHandle, bool)> = scripted
                    .iter()
                    .chain(copies.iter().filter(|h| !scripted.contains(h)))
                    .filter_map(|&h| {
                        let f = model.arena.frame(h)?;
                        (f.strata == stratum && f.effective_visible && has_model_file(f)).then(
                            || {
                                (
                                    f.level,
                                    u64::from(f.insertion_seq),
                                    h,
                                    scripted.contains(&h),
                                )
                            },
                        )
                    })
                    .collect();
                panes.sort_unstable_by_key(|&(level, seq, _, _)| (level, seq));
                panes.into_iter().map(|(_, _, h, s)| (h, s)).collect()
            };
            for (h, scripted) in panes {
                let id = {
                    let mut model = self.model_mut();
                    match model.arena.frame(h) {
                        Some(f) if paints(&model, h, f) => model.frame_id(h),
                        _ => continue,
                    }
                };
                if scripted {
                    if let Err(e) =
                        event::fire_widget_handler(&self.lua, id, "OnUpdateModel", Vec::new())
                    {
                        self.push_error(e);
                    }
                }
                // The turn timer, against the latch the handler may just have set.
                // It reads whatever `+0x318` holds, a file included.
                let mut model = self.model_mut();
                let now_ms = model.now_ms();
                let path = match model.arena.frame(h).map(|f| &f.kind_state) {
                    Some(crate::widget::KindState::Model(m)) if m.character.is_some() => {
                        m.path.clone()
                    }
                    _ => continue,
                };
                let facts = path.and_then(|p| model.model_facts_for(&p));
                if let Some(crate::widget::KindState::Model(m)) =
                    model.arena.frame_mut(h).map(|f| &mut f.kind_state)
                {
                    m.turn_paint(now_ms, facts.as_deref());
                }
            }
        }
    }
}

/// A `PlayerModel`, `DressUpModel` or `TabardModel`.
fn is_character(f: &crate::widget::Frame) -> bool {
    matches!(&f.kind_state, crate::widget::KindState::Model(m) if m.character.is_some())
}

/// A character pane whose copy sits at `+0x318`, rather than a file `SetModel` put there.
fn holds_copy(f: &crate::widget::Frame) -> bool {
    matches!(&f.kind_state, crate::widget::KindState::Model(m)
        if m.path.is_none() && m.character.as_ref().is_some_and(|c| c.copy.is_some()))
}

/// A Model-family pane with a model at `[widget+0x318]` (`0x76d24c`), which its layer emit and its
/// render callback both require: a file set, resident or streaming, or a character pane's copy.
/// On any other kind `OnUpdateModel` is inert.
fn has_model_file(f: &crate::widget::Frame) -> bool {
    holds_copy(f) || matches!(&f.kind_state, crate::widget::KindState::Model(m) if m.path.is_some())
}

/// The paint callback's gates ahead of `OnUpdateModel` (`0x76d240`, firing at `0x76d5ea`): a model
/// set (`0x76d24c`); a resolved rect (`0x768320`, the rect-resolved bit, `0x76d27d`/`0x76d284`);
/// and the viewport, that rect as fractions of the screen (`0x41ade0` divides by its extent) with
/// each edge clamped to 0..1 (`0x76d473`–`0x76d56d`), at least `[0x8029d4]` = 2⁻²² wide and tall
/// (`0x76d574`–`0x76d5a0`, else to the cleanup `0x76d63e`). So a pane with no size, or wholly off
/// screen, paints nothing and fires nothing.
fn paints(model: &super::Model, h: FrameHandle, f: &crate::widget::Frame) -> bool {
    if !has_model_file(f) {
        return false;
    }
    let Some(r) = model.frame_rect(h) else {
        return false;
    };
    let screen = model.screen;
    let (sw, sh) = (screen.width(), screen.height());
    if sw <= 0.0 || sh <= 0.0 {
        return false;
    }
    let fraction = |v: f32, origin: f32, extent: f32| ((v - origin) / extent).clamp(0.0, 1.0);
    let wide = fraction(r.right, screen.left, sw) - fraction(r.left, screen.left, sw);
    let tall = fraction(r.top, screen.bottom, sh) - fraction(r.bottom, screen.bottom, sh);
    let eps = crate::layout::SIZE_EPS;
    f64::from(wide.abs()) >= eps && f64::from(tall.abs()) >= eps
}

/// A Lua listener that logs each delivery in order, for the tests that pin when a verb's events
/// fire and what their handlers read.
#[cfg(test)]
pub(crate) mod heard {
    use super::UiScript;

    /// Register one frame for `events`; each delivery appends `EVENT` or, with a `probe`, `EVENT
    /// probe-value`, the probe being a Lua expression read inside the handler.
    pub(crate) fn listen(s: &UiScript, events: &[&str], probe: Option<&str>) {
        let register: String = events
            .iter()
            .map(|e| format!("HEARD_FRAME:RegisterEvent(\"{e}\")\n"))
            .collect();
        let line = probe.map_or_else(
            || "event".to_string(),
            |p| format!("event .. \" \" .. tostring({p})"),
        );
        s.run(&format!(
            "HEARD = {{}}\n\
             HEARD_FRAME = HEARD_FRAME or CreateFrame(\"Frame\")\n\
             {register}\
             HEARD_FRAME:SetScript(\"OnEvent\", function() table.insert(HEARD, {line}) end)"
        ))
        .unwrap();
    }

    /// The deliveries since the last call, with no tick run: what fired inside the verbs.
    pub(crate) fn take(s: &UiScript) -> Vec<String> {
        s.eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::{heard, verb};
    use crate::script::UiScript;

    /// A verb's events have run their handlers by the verb's next Lua line, in the order raised,
    /// and none is left for the tick.
    #[test]
    fn a_verb_segment_fires_its_events_before_the_binding_returns() {
        let s = UiScript::new().unwrap();
        s.lua
            .globals()
            .set(
                "RaiseTwo",
                s.lua
                    .create_function(|lua, ()| {
                        verb(lua, |model| {
                            model.raise("CURSOR_UPDATE", Vec::new());
                            model.raise("PLAYER_MONEY", Vec::new());
                        });
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();
        heard::listen(&s, &["CURSOR_UPDATE", "PLAYER_MONEY"], None);
        let seen: Vec<String> = s
            .eval("RaiseTwo() local h = HEARD HEARD = {} return h")
            .unwrap();
        assert_eq!(seen, ["CURSOR_UPDATE", "PLAYER_MONEY"]);
        assert!(s.model_ref().pending_events.is_empty());
    }

    /// Outside a segment, a raise is the host's and waits for the next tick.
    #[test]
    fn a_raise_outside_a_verb_queues_for_the_tick() {
        let mut s = UiScript::new().unwrap();
        heard::listen(&s, &["CURSOR_UPDATE"], None);
        s.model_mut().raise("CURSOR_UPDATE", Vec::new());
        assert!(heard::take(&s).is_empty());
        s.tick(0.0);
        assert_eq!(heard::take(&s), ["CURSOR_UPDATE"]);
    }
}
