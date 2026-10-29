//! Where each raise of an event into the VM comes from, checked in debug builds. The reference
//! signals an event only inside the step that causes it (`SignalEvent` `0x703e50`, called from the
//! packet's handler, the input record's dispatch, the verb's body or the frame pass's walk), so a
//! raise here is right inside a script call (a verb segment, a handler, the host's apply of a
//! call), a lane's firing or the VM's own tick. One made anywhere else is a system raising at its
//! own place in the frame, which the source scans cannot see when it goes through a helper in
//! another module: it fails, naming its caller, unless a frame of its stack is on the host's allow
//! list with its reason. Each raise is judged where it enters the VM, at one of its two doors: the
//! fire (`tick::fire_event_into`) or the push onto the host queue (`Model::queue_event`); a queued
//! one fires later inside the tick, and a held one inside the host's drain.
//!
//! The check runs only while the host has armed it ([`arm`]), around its frame's schedules, so a
//! test that drives a VM by hand is not judged. How an unlisted raise fails is the host's
//! [`Failure`]: a test panics, and a live run reports each site once and goes on. Without
//! `debug_assertions` every piece compiles to nothing.

/// A place a raise is right to come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A script call: a verb segment's fan-out, a handler running, the host applying a call or
    /// running what waited for it.
    Script,
    /// A lane's firing, the host's place for what a packet raised.
    Lane,
    /// The VM's own tick: the queued events, the `OnUpdate` walk and the pump.
    Tick,
}

/// An origin outside the three [`Origin`]s that may raise: a stack frame's function path, matched
/// as a suffix of a frame's symbol (`ui_world_map::feed_landmarks`), with why it may.
#[derive(Clone, Copy, Debug)]
pub struct Allow {
    /// The function, by a suffix of its path.
    pub frame: &'static str,
    /// Why it raises at its own place: a reference fact, or the guard row that names it.
    pub why: &'static str,
}

/// How a raise from an unlisted origin fails.
#[derive(Clone, Copy, Debug)]
pub enum Failure {
    /// A panic naming the event and its caller, for a test.
    Panic,
    /// The same line handed to the host's reporter, once per site (the raise's line and its
    /// callers), for a live run that must go on.
    Report(fn(&str)),
}

/// What the host armed: its allow list and how a raise outside it fails.
type ArmedState = Option<(&'static [Allow], Failure)>;

#[cfg(debug_assertions)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::HashSet;

    use super::{ArmedState, Failure, Origin};

    thread_local! {
        /// The origins open on this thread, innermost last.
        static OPEN: RefCell<Vec<Origin>> = const { RefCell::new(Vec::new()) };
        /// The host's allow list and failure while the check is armed.
        static ARMED: Cell<ArmedState> = const { Cell::new(None) };
        /// The sites reported so far ([`Failure::Report`]).
        static REPORTED: RefCell<Option<HashSet<String>>> = const { RefCell::new(None) };
    }

    pub(super) fn within<R>(origin: Origin, f: impl FnOnce() -> R) -> R {
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                OPEN.with_borrow_mut(|open| open.pop());
            }
        }
        OPEN.with_borrow_mut(|open| open.push(origin));
        let _close = Close;
        f()
    }

    pub(super) fn current() -> Option<Origin> {
        OPEN.with_borrow(|open| open.last().copied())
    }

    pub(super) fn set_armed(armed: ArmedState) -> ArmedState {
        ARMED.replace(armed)
    }

    #[track_caller]
    pub(super) fn check(event: &str, lua: Option<&mlua::Lua>) {
        let Some((allow, failure)) = ARMED.get() else {
            return;
        };
        if current().is_some() {
            return;
        }
        // A Rust function a script called: what it raises is the script call's.
        if lua.is_some_and(|lua| lua.inspect_stack(0, |_| ()).is_some()) {
            return;
        }
        let trace = std::backtrace::Backtrace::force_capture().to_string();
        let symbols: Vec<&str> = trace
            .lines()
            .map(str::trim)
            .filter_map(|l| l.split_once(": ").map(|(_, s)| s))
            .collect();
        if allow.iter().any(|a| allowed(a.frame, &symbols)) {
            return;
        }
        let at = std::panic::Location::caller();
        let from = callers(&symbols);
        let line = format!(
            "`{event}` was raised outside every script call, lane firing and tick, at {at}, \
             from {from}: put it on a lane or inside the call that causes it, or give its origin \
             a row on the host's allow list with its reason"
        );
        match failure {
            Failure::Panic => panic!("{line}"),
            Failure::Report(report) => {
                let fresh = REPORTED.with_borrow_mut(|seen| {
                    seen.get_or_insert_with(HashSet::new)
                        .insert(format!("{at} {from}"))
                });
                if fresh {
                    report(&line);
                }
            }
        }
    }

    /// Whether a frame of `symbols` is `frame`'s function, or a closure inside it.
    pub(super) fn allowed(frame: &str, symbols: &[&str]) -> bool {
        symbols.iter().any(|s| {
            let s = path_of(s);
            s.strip_suffix(frame).is_some_and(|p| p.ends_with("::"))
        })
    }

    /// A symbol's function path: its generic arguments, its closures and its hash cut.
    fn path_of(symbol: &str) -> String {
        let mut out = String::with_capacity(symbol.len());
        let mut depth = 0usize;
        let mut chars = symbol.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                // A turbofish's arguments: `::<` opens them.
                '<' if depth > 0 || out.ends_with("::") => {
                    if depth == 0 {
                        out.truncate(out.len() - 2);
                    }
                    depth += 1;
                }
                '>' if depth > 0 => depth -= 1,
                _ if depth > 0 => {}
                '{' => {
                    // `{closure#0}`, `{{closure}}`: cut with the `::` before it.
                    while let Some(c) = chars.next() {
                        if c == '}' && chars.peek() != Some(&'}') {
                            break;
                        }
                    }
                    if out.ends_with("::") {
                        out.truncate(out.len() - 2);
                    }
                }
                _ => out.push(c),
            }
        }
        match out.rsplit_once("::h") {
            Some((p, hash)) if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
                p.to_string()
            }
            _ => out,
        }
    }

    /// The nearest functions of benilla's own crates past the check, innermost first, each once.
    fn callers(symbols: &[&str]) -> String {
        let mut ours: Vec<String> = Vec::new();
        for s in symbols {
            let path = path_of(s);
            let path = path.trim_start_matches('<');
            // A trait's shim (`<f as FnMut<…>>::call_mut`) repeats the function under it.
            if !path.starts_with("benilla_") || path.contains("::origin::") || path.contains(" as ")
            {
                continue;
            }
            if ours.last().map(String::as_str) != Some(path) {
                ours.push(path.to_string());
            }
        }
        ours.truncate(16);
        ours.join(" < ")
    }
}

/// Run `f` as `origin`: a raise inside it is that origin's.
#[inline]
pub fn within<R>(origin: Origin, f: impl FnOnce() -> R) -> R {
    #[cfg(debug_assertions)]
    {
        imp::within(origin, f)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = origin;
        f()
    }
}

/// The innermost origin open on this thread; `None` without `debug_assertions`.
pub fn current() -> Option<Origin> {
    #[cfg(debug_assertions)]
    {
        imp::current()
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

/// Arm the check on this thread with `allow` and `failure` until the guard drops.
#[must_use = "the check is armed while the guard lives"]
pub fn arm(allow: &'static [Allow], failure: Failure) -> Armed {
    #[cfg(debug_assertions)]
    {
        Armed(imp::set_armed(Some((allow, failure))))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (allow, failure);
        Armed(None)
    }
}

/// Arm the check on this thread with `allow` and `failure` until [`disarm`], for a host whose
/// frame opens and closes in two calls, as the host's schedules around its frame do.
pub fn arm_until_disarmed(allow: &'static [Allow], failure: Failure) {
    #[cfg(debug_assertions)]
    imp::set_armed(Some((allow, failure)));
    #[cfg(not(debug_assertions))]
    let _ = (allow, failure);
}

/// End what [`arm_until_disarmed`] began.
pub fn disarm() {
    #[cfg(debug_assertions)]
    imp::set_armed(None);
}

/// [`arm`]'s guard: puts back what was armed before.
pub struct Armed(ArmedState);

impl Drop for Armed {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        imp::set_armed(self.0);
        #[cfg(not(debug_assertions))]
        let _ = self.0;
    }
}

/// Judge a raise of `event` here: `lua` tells whether a script is running when the caller has it.
#[inline]
#[track_caller]
pub(crate) fn check(event: &str, lua: Option<&mlua::Lua>) {
    #[cfg(debug_assertions)]
    imp::check(event, lua);
    #[cfg(not(debug_assertions))]
    let _ = (event, lua);
}

/// Whether one of `symbols` (a backtrace's frame symbols) is `frame`'s function or a closure in
/// it, as the allow list matches.
#[cfg(test)]
fn frame_matches(frame: &str, symbols: &[&str]) -> bool {
    #[cfg(debug_assertions)]
    {
        imp::allowed(frame, symbols)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (frame, symbols);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::tick::{fire_event_into, verb};
    use crate::script::UiScript;

    /// Armed, a verb segment's raise fires, and its queued and fired forms alike pass; outside
    /// every origin the same raise fails, naming the line that made it.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_raise_inside_a_segment_passes_and_one_outside_every_origin_fails_with_its_caller() {
        let s = UiScript::new().unwrap();
        let _armed = arm(&[], Failure::Panic);
        verb(&s.lua, |model| model.raise("CURSOR_UPDATE", Vec::new()));
        within(Origin::Lane, || {
            fire_event_into(&s.lua, "PLAYER_MONEY", Vec::new())
        });
        let line = line!() + 2;
        let outside = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            fire_event_into(&s.lua, "PLAYER_MONEY", Vec::new());
        }));
        let message = outside.expect_err("an unarmed origin raised");
        let message = message
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains("`PLAYER_MONEY`") && message.contains(&format!("origin.rs:{line}:")),
            "{message}"
        );
    }

    thread_local! {
        static REPORTS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    fn keep(line: &str) {
        REPORTS.with_borrow_mut(|r| r.push(line.to_string()));
    }

    /// Armed for a live run, a raise outside every origin goes on, and each site is reported once
    /// however often it raises.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_live_run_reports_each_site_once_and_goes_on() {
        let s = UiScript::new().unwrap();
        let _armed = arm(&[], Failure::Report(keep));
        for _ in 0..3 {
            fire_event_into(&s.lua, "PLAYER_MONEY", Vec::new());
        }
        fire_event_into(&s.lua, "PLAYER_MONEY", Vec::new());
        let reports = REPORTS.with_borrow(Clone::clone);
        assert_eq!(reports.len(), 2, "{reports:#?}");
        assert!(
            reports.iter().all(|r| r.contains("`PLAYER_MONEY`")),
            "{reports:#?}"
        );
    }

    /// Unarmed, as a VM driven by hand is, nothing is judged.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn an_unarmed_thread_judges_nothing() {
        let mut s = UiScript::new().unwrap();
        s.fire_event("PLAYER_MONEY", Vec::new());
        s.queue_event("PLAYER_MONEY", Vec::new());
    }

    /// A raise whose stack holds an allowed function passes.
    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn an_allowed_frame_passes() {
        const ALLOW: &[Allow] = &[Allow {
            frame: "origin::tests::an_allowed_frame_passes",
            why: "the test's own",
        }];
        let s = UiScript::new().unwrap();
        let _armed = arm(ALLOW, Failure::Panic);
        fire_event_into(&s.lua, "PLAYER_MONEY", Vec::new());
    }

    #[test]
    #[cfg_attr(not(debug_assertions), ignore = "the check compiles out")]
    fn a_frame_matches_its_function_and_its_closures_by_path_suffix() {
        let symbols = [
            "std::rt::lang_start",
            "benilla_app::ui_world_map::feed_landmarks::{{closure}}",
            "benilla_app::lanes::fire_in_order::h0123456789abcdef",
            "benilla_app::script_calls::hosted::<(), benilla_app::ui_social::feed::feed_social::{closure#0}>::{closure#0}",
        ];
        assert!(frame_matches("script_calls::hosted", &symbols));
        assert!(!frame_matches("ui_social::feed::feed_social", &symbols));
        assert!(frame_matches("ui_world_map::feed_landmarks", &symbols));
        assert!(frame_matches("lanes::fire_in_order", &symbols));
        assert!(!frame_matches("map::feed_landmarks", &symbols));
        assert!(!frame_matches("ui_world_map::feed", &symbols));
    }
}
