//! Per-handler cost attribution (`WOW_UI_HANDLERS=<secs>`): self and total time for every handler
//! fired through [`super::event::fire`], since firing nests; engine work a handler provokes, such
//! as a layout resolve, is its self time. Off, it costs a relaxed atomic load per fire ([`armed`]).
//! Its state has its own `app_data` slot, not the model's: mlua keeps a `RefCell` per entry, so the
//! profiler's borrow cannot collide with a model borrow held across a fire.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use mlua::Lua;

use super::UiScript;

/// How many rows the periodic report prints before rolling the rest into a `… N more` line.
const REPORT_ROWS: usize = 12;

/// Whether any VM in the process profiles handlers: process-wide, so the per-fire check needs no
/// `app_data` lookup.
static ARMED: AtomicBool = AtomicBool::new(false);

#[inline]
pub(super) fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// The report period from `WOW_UI_HANDLERS=<secs>`, read once; unparseable or non-positive is off.
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

/// The profiler's state, one per VM, keyed by frame id: names resolve at report time, and a frame
/// destroyed before then still counts, as `#<id>`.
#[derive(Default)]
pub(crate) struct HandlerProf {
    /// Whether this VM records; [`armed`] is the process-wide gate.
    on: bool,
    /// Seconds between reports; `0.0` records and never prints.
    period: f32,
    /// Wall seconds and frames since the last report, the divisor of every printed rate.
    window: f32,
    frames: u32,
    /// Fires that had a nested fire inside them.
    nested: u32,
    /// Nanos already charged to nested fires, one entry per open fire.
    stack: Vec<u64>,
    rows: HashMap<u32, Vec<Slot>>,
}

/// An open fire, closed by its `Drop`, so the nesting stack stays balanced through an unwind too.
pub(super) struct Fire<'a> {
    lua: &'a Lua,
    id: u32,
    script: &'a str,
    /// `None` while this VM is not recording, which makes the guard a no-op.
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
    /// to the enclosing fire as its child time.
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
        // Children run inside this fire; saturating only matters if a clock disagrees.
        let self_ns = total.saturating_sub(children);
        let slots = prof.rows.entry(self.id).or_default();
        match slots.iter_mut().find(|s| s.script == self.script) {
            Some(slot) => {
                slot.calls += 1;
                slot.total_ns += total;
                slot.self_ns += self_ns;
            }
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
    /// The frame's name, else where its handler was defined, else `#<id>`.
    pub frame: String,
    pub script: String,
    pub calls: u32,
    /// Microseconds in this handler, excluding the handlers it fired.
    pub self_us: f64,
    /// Microseconds in this handler, including the handlers it fired.
    pub total_us: f64,
}

impl UiScript {
    /// Arm or disarm attribution for this VM, reporting every `period_secs` (`0.0`: no report, read
    /// [`UiScript::handler_profile`]). Call it between ticks: it clears the nesting stack that open
    /// fires count on.
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

    /// This VM's handler costs over the current window as totals, heaviest self time first.
    pub fn handler_profile(&self) -> Vec<HandlerRow> {
        // Snapshot, then release the borrow: labelling reaches into the model and back into Lua.
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

    /// Frames ticked since the last report, [`UiScript::handler_profile`]'s denominator.
    pub fn handler_profile_frames(&self) -> u32 {
        self.lua
            .app_data_ref::<HandlerProf>()
            .map_or(0, |prof| prof.frames)
    }

    /// Count a ticked frame and print the report when due, at the end of [`UiScript::tick`].
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
        // The rows first: the label walk's model borrow must end before the profiler's mutable one.
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
    /// A row's label: the frame's name, else the handler's `<file>:<line>` (an unnamed frame is the
    /// common addon timer), else `#<id>`.
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

    /// `<short_src>:<line>` of the function bound at `(id, script)`, when the VM can say.
    fn handler_defined_at(&self, id: u32, script: &str) -> Option<String> {
        // `raw_get`, so a report can never run a metamethod.
        let scripts: mlua::Table = self.lua.named_registry_value(super::REG_SCRIPTS).ok()?;
        let per: mlua::Table = scripts.raw_get(id).ok()?;
        let func: mlua::Function = per.raw_get(script).ok()?;
        let info = func.info();
        Some(format!("{}:{}", info.short_src?, info.line_defined?))
    }
}

/// The `[ui-handlers]` block, per frame like the client's other costs: the window, the per-script
/// rollup, then the heaviest [`REPORT_ROWS`] handlers by self time and the rest in one line.
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

/// Install the profiler's `app_data` slot, armed from the environment. Unconditional: mlua refuses
/// to insert app data while any is borrowed, so the slot must exist before the VM runs.
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
