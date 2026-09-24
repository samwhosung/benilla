//! Per-handler cost attribution (`WOW_UI_HANDLERS=<secs>`) — *which* FrameXML handler is spending
//! the frame. An `impl UiScript` block beside its concern, the `layout.rs` pattern.
//!
//! ## Why it exists
//!
//! Three perf hunts in a row — 1383's autocast shine, 1385's cast bar, 1388's layout graph — each
//! opened with the same question and each answered it **by hand**: read the shipped Lua, guess the
//! busy handler, add a temporary print, re-run. `[ui-cost] tick=` is one aggregate over the whole
//! `OnUpdate` sweep, so it can say *that* the sweep got expensive and never *who*. This is the
//! missing decomposition, and it sits one layer below the one that landed beside it: 1389's HUD
//! measures the frame's cost and latches its tail (`cpu`/`main` ms, the spike badge), which is
//! **when** and **how much**; this is **who**, inside the UI's share of it.
//!
//! ## What it measures
//!
//! Every handler fired through [`super::event::fire`] — the single firing path for `OnUpdate`,
//! `OnEvent`, `OnShow`/`OnHide`, `OnSizeChanged`, and the whole widget set (`OnClick`, `OnEnter`,
//! `OnValueChanged`, …). Not just the tick's `OnUpdate` sweep, deliberately: an event storm (the
//! combat log at 40 lines a second) costs frames the same way a hot `OnUpdate` does, and a `by
//! script:` rollup on the report line is what tells the two apart at a glance.
//!
//! **Self and total, because handler firing nests.** A handler that calls `Show()` fires `OnShow`
//! inside itself, and a handler that resizes something fires `OnSizeChanged`; charging the parent
//! for its children would name the wrong frame, which is the one failure this instrument cannot
//! afford. A stack of per-level child nanos gives both: `total` is what the call cost, `self` is
//! what it cost *itself*. Sorted by `self`.
//!
//! Engine work a handler provokes synchronously **is** its self time, and that is the point — a
//! geometry getter inside an `OnUpdate` calls `settle()`, which is a whole layout resolve
//! (1388's opening finding: `OptionsScroll_Fit` does four per frame). That cost belongs to the
//! handler that asked for it.
//!
//! ## What it costs when off
//!
//! One relaxed atomic load and a not-taken branch per handler fire ([`armed`]). The state lives in
//! its own `app_data` slot rather than in [`super::Model`]: mlua keys `app_data` by `TypeId` with a
//! **`RefCell` per entry** (`mlua-0.11.6`, `types/app_data.rs`), so the profiler's borrow can never
//! collide with a model borrow held across a fire — a hazard that would otherwise be a panic in the
//! hottest path in the client, armed only in the sessions where we are already chasing something.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mlua::Lua;

use super::UiScript;

/// How many rows the periodic report prints before rolling the rest into a `… N more` line.
const REPORT_ROWS: usize = 12;

/// The fast gate: is *any* VM in this process profiling handlers?
///
/// Process-wide and not per-VM on purpose — the check runs per handler fire, and a per-VM answer
/// would cost the `app_data` lookup this is here to avoid. A false positive (one VM armed, another
/// not) costs the unarmed VM a hash lookup that finds `on == false`, which is the only case where
/// the two answers differ and is not a case that ships.
static ARMED: AtomicBool = AtomicBool::new(false);

#[inline]
pub(super) fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// `WOW_UI_HANDLERS=<secs>` — the report period, read once. `WOW_UI_HANDLERS=3` reports every three
/// seconds; anything unparseable or non-positive leaves the instrument off, the `[ui-cost]` /
/// `[layout-prof]` posture.
fn env_period() -> Option<f32> {
    static PERIOD: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *PERIOD.get_or_init(|| {
        std::env::var("WOW_UI_HANDLERS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|secs| *secs > 0.0)
    })
}

/// One `(frame, script)` pair's accumulation over the current window.
struct Slot {
    script: String,
    calls: u32,
    total_ns: u64,
    self_ns: u64,
}

/// The profiler's state, one per VM, in its own `app_data` slot (see the module header).
///
/// Keyed by frame **id**, not by name: the name is a `String` clone and a two-map walk, so it is
/// resolved once at report time rather than on every fire. A frame destroyed before the report
/// reads back as `#<id>` — its cost still counted, which is what a churning pool of frames needs.
#[derive(Default)]
pub(crate) struct HandlerProf {
    /// Is *this* VM recording? [`armed`] is the process-wide gate; this is the per-VM answer.
    on: bool,
    /// Seconds between reports; `0.0` records forever and never prints (what a test wants).
    period: f32,
    /// Wall seconds and frames since the last report — the window every printed rate divides by.
    window: f32,
    frames: u32,
    /// Fires that had at least one nested fire inside them. Printed as the one-number answer to
    /// "should I be reading `self` or `total` here?".
    nested: u32,
    /// Nanos already charged to *nested* fires, one entry per open fire. See the module header.
    stack: Vec<u64>,
    rows: HashMap<u32, Vec<Slot>>,
}

/// An open fire, closed by its `Drop` — the balance of the nesting stack is then a property of the
/// borrow checker rather than of the call site remembering to close what it opened. It also holds
/// under an unwind, which a hand-written enter/exit pair does not: a panic through a handler would
/// otherwise leak a stack level and silently inflate every parent attribution after it.
pub(super) struct Fire<'a> {
    lua: &'a Lua,
    id: u32,
    script: &'a str,
    /// `None` while this VM is not recording — the whole guard is then a no-op.
    started: Option<Instant>,
}

impl<'a> Fire<'a> {
    /// Open a fire on `(id, script)`. Cheap and inert unless this VM is recording.
    pub(super) fn open(lua: &'a Lua, id: u32, script: &'a str) -> Fire<'a> {
        let started = match lua.app_data_mut::<HandlerProf>() {
            Some(mut prof) if prof.on => {
                prof.stack.push(0);
                Some(Instant::now())
            }
            _ => None,
        };
        Fire {
            lua,
            id,
            script,
            started,
        }
    }
}

impl Drop for Fire<'_> {
    /// Charge `total` to `(id, script)`, `total - children` to its self time, and hand `total` up
    /// to the enclosing fire as *its* child time.
    fn drop(&mut self) {
        let Some(started) = self.started else {
            return;
        };
        let total = started.elapsed().as_nanos() as u64;
        let Some(mut prof) = self.lua.app_data_mut::<HandlerProf>() else {
            return;
        };
        let children = prof.stack.pop().unwrap_or(0);
        if let Some(parent) = prof.stack.last_mut() {
            *parent += total;
        }
        if children > 0 {
            prof.nested += 1;
        }
        // `saturating_sub`: children are measured strictly inside this fire, so the subtraction
        // cannot go negative by construction — but a construction argument is not a reason to hand
        // a wrapped `u64` to a report if a clock ever disagrees.
        let self_ns = total.saturating_sub(children);
        let slots = prof.rows.entry(self.id).or_default();
        match slots.iter_mut().find(|s| s.script == self.script) {
            Some(slot) => {
                slot.calls += 1;
                slot.total_ns += total;
                slot.self_ns += self_ns;
            }
            // The only allocation on this path, and only on a `(frame, script)` pair's first fire
            // of the window — steady state is a linear scan over one frame's handful of slots.
            None => slots.push(Slot {
                script: self.script.to_string(),
                calls: 1,
                total_ns: total,
                self_ns,
            }),
        }
    }
}

/// One `(frame, script)` pair's cost over the profiler's current window.
#[derive(Clone, Debug, PartialEq)]
pub struct HandlerRow {
    /// The frame's name, or `#<id>` for an anonymous or already-destroyed frame.
    pub frame: String,
    pub script: String,
    pub calls: u32,
    /// Microseconds spent in this handler *excluding* handlers it fired — what to sort by.
    pub self_us: f64,
    /// Microseconds spent in this handler *including* handlers it fired.
    pub total_us: f64,
}

impl UiScript {
    /// Arm or disarm handler attribution for this VM, reporting every `period_secs` (`0.0` records
    /// with no periodic report — read it with [`UiScript::handler_profile`] instead).
    ///
    /// Call it between ticks. An open [`Fire`] is inert either way (it decided once, at `open`),
    /// but this clears the nesting stack, so a disarm from *inside* a handler would drop a level
    /// its enclosing fires are still counting on.
    pub fn profile_handlers(&self, on: bool, period_secs: f32) {
        if on {
            ARMED.store(true, Ordering::Relaxed);
        }
        let Some(mut prof) = self.lua.app_data_mut::<HandlerProf>() else {
            return;
        };
        prof.on = on;
        prof.period = period_secs;
        prof.window = 0.0;
        prof.frames = 0;
        prof.nested = 0;
        prof.stack.clear();
        prof.rows.clear();
    }

    /// This VM's handler costs over the current window, heaviest **self** time first.
    ///
    /// Window totals, not per-frame rates — the frame count is [`UiScript::handler_profile_frames`],
    /// so a caller that wants a rate divides by the number it also has to report.
    pub fn handler_profile(&self) -> Vec<HandlerRow> {
        // Snapshot under the profiler's borrow and then let it go: labelling reaches into the model
        // *and* back into Lua, neither of which may be touched with a borrow of anything open.
        let raw: Vec<(u32, String, u32, u64, u64)> = match self.lua.app_data_ref::<HandlerProf>() {
            Some(prof) => prof
                .rows
                .iter()
                .flat_map(|(&id, slots)| {
                    slots.iter().map(move |slot| {
                        (
                            id,
                            slot.script.clone(),
                            slot.calls,
                            slot.self_ns,
                            slot.total_ns,
                        )
                    })
                })
                .collect(),
            None => return Vec::new(),
        };
        let mut rows: Vec<HandlerRow> = raw
            .into_iter()
            .map(|(id, script, calls, self_ns, total_ns)| HandlerRow {
                frame: self.handler_label(id, &script),
                script,
                calls,
                self_us: self_ns as f64 / 1000.0,
                total_us: total_ns as f64 / 1000.0,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.self_us
                .total_cmp(&a.self_us)
                .then_with(|| a.frame.cmp(&b.frame))
                .then_with(|| a.script.cmp(&b.script))
        });
        rows
    }

    /// Frames ticked since the last report — [`UiScript::handler_profile`]'s denominator.
    pub fn handler_profile_frames(&self) -> u32 {
        self.lua
            .app_data_ref::<HandlerProf>()
            .map_or(0, |prof| prof.frames)
    }

    /// Advance the profiler's window by one tick and print the report if it is due. Called at the
    /// end of [`UiScript::tick`], so "a frame" means a ticked frame whatever else fired in it.
    pub(super) fn report_handler_profile(&mut self, elapsed: f32) {
        if !armed() {
            return;
        }
        let due = match self.lua.app_data_mut::<HandlerProf>() {
            Some(mut prof) if prof.on => {
                prof.frames += 1;
                prof.window += elapsed;
                prof.period > 0.0 && prof.window >= prof.period
            }
            _ => false,
        };
        if !due {
            return;
        }
        // The rows first, so the model borrow the label walk takes is released before the profiler
        // is borrowed mutably to reset it.
        let rows = self.handler_profile();
        let mut prof = self
            .lua
            .app_data_mut::<HandlerProf>()
            .expect("profiler app_data — checked above");
        print_report(&rows, prof.window, prof.frames, prof.nested);
        prof.window = 0.0;
        prof.frames = 0;
        prof.nested = 0;
        prof.rows.clear();
    }
}

impl UiScript {
    /// A row's label: the frame's name, else **where the handler was written**, else `#<id>`.
    ///
    /// The definition site is not a nicety. `CreateFrame("Frame")` + `OnUpdate` with no name is
    /// *the* addon timer idiom, so the anonymous case is not the rare one — it is exactly the set
    /// of handlers an attribution table exists to catch, and `#4127` is not something anyone can
    /// act on. `Function::info()` hands back the chunk and line, and every addon chunk is named
    /// `@Interface\AddOns\<Folder>\<File>` (see [`super::addon_chunk_name`]), so the label reads as
    /// the addon and the line to open.
    fn handler_label(&self, id: u32, script: &str) -> String {
        let named = {
            let model = self.model_ref();
            model
                .id_to_frame
                .get(&id)
                .and_then(|&h| model.arena.frame(h))
                .and_then(|f| f.name.clone())
        };
        named
            .or_else(|| self.handler_defined_at(id, script))
            .unwrap_or_else(|| format!("#{id}"))
    }

    /// `<short_src>:<line>` for the function bound at `(id, script)`, or `None` if there is no such
    /// binding or the VM cannot say (a C function, a stripped chunk).
    fn handler_defined_at(&self, id: u32, script: &str) -> Option<String> {
        // `raw_get` throughout: the registry's script tables are plain data, and a report must not
        // be able to run a metamethod — that would re-enter Lua from inside an instrument.
        let scripts: mlua::Table = self.lua.named_registry_value(super::REG_SCRIPTS).ok()?;
        let per: mlua::Table = scripts.raw_get(id).ok()?;
        let func: mlua::Function = per.raw_get(script).ok()?;
        let info = func.info();
        Some(format!("{}:{}", info.short_src?, info.line_defined?))
    }
}

/// The `[ui-handlers]` block: the window's shape, the per-script rollup, then the heaviest
/// [`REPORT_ROWS`] handlers by self time with the remainder folded into one line.
///
/// Everything is **per frame**, because that is the unit every other cost in this client is quoted
/// in (`[ui-cost] tick=`, the HUD's `cpu ms`) and the whole use of this instrument is comparing
/// against those.
fn print_report(rows: &[HandlerRow], window: f32, frames: u32, nested: u32) {
    let per_frame = f64::from(frames.max(1));
    let total_self: f64 = rows.iter().map(|r| r.self_us).sum();
    let mut by_script: Vec<(&str, f64)> = Vec::new();
    for row in rows {
        match by_script.iter_mut().find(|(s, _)| *s == row.script) {
            Some((_, us)) => *us += row.self_us,
            None => by_script.push((&row.script, row.self_us)),
        }
    }
    by_script.sort_by(|a, b| b.1.total_cmp(&a.1));
    let rollup: Vec<String> = by_script
        .iter()
        .map(|(script, us)| format!("{script} {:.1}", us / per_frame))
        .collect();
    eprintln!(
        "[ui-handlers] {window:.2}s · {frames} frames · {} handlers · self {:.1} us/f · nested {nested}",
        rows.len(),
        total_self / per_frame,
    );
    eprintln!("[ui-handlers] by script: {}", rollup.join("  "));
    eprintln!("[ui-handlers]   self/f  total/f  calls/f  handler");
    for row in rows.iter().take(REPORT_ROWS) {
        eprintln!(
            "[ui-handlers] {:8.1} {:8.1} {:8.2}  {}:{}",
            row.self_us / per_frame,
            row.total_us / per_frame,
            f64::from(row.calls) / per_frame,
            row.frame,
            row.script,
        );
    }
    if let Some(rest) = rows.len().checked_sub(REPORT_ROWS).filter(|n| *n > 0) {
        let tail: f64 = rows.iter().skip(REPORT_ROWS).map(|r| r.self_us).sum();
        eprintln!(
            "[ui-handlers] {:8.1}                    … {rest} more",
            tail / per_frame
        );
    }
}

/// Install the profiler's `app_data` slot, and arm it from the environment.
///
/// Unconditional (an empty `HandlerProf` is a few words) because mlua refuses to *insert* app data
/// while any app data is borrowed — so the slot has to exist before the VM starts running, or
/// [`UiScript::profile_handlers`] would carry a panic that only fires in the sessions using it.
pub(super) fn install(lua: &Lua) {
    let period = env_period();
    lua.set_app_data(HandlerProf {
        on: period.is_some(),
        period: period.unwrap_or_default(),
        ..HandlerProf::default()
    });
    if period.is_some() {
        ARMED.store(true, Ordering::Relaxed);
    }
}
