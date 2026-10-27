//! The census of the sends a call's apply or an engine turn makes while a handler it signalled is
//! held, for test builds. `SignalEvent` (`0x703e50`) runs every listener before the verb goes on,
//! so a send made while one waits goes out ahead of the packets its handler sends in 1.12.1. Each
//! such send is counted under its call's kind, or `turn@` and its turn's caller
//! ([`super::UiScript::host_out`]), from the first handler held to the signal point that runs it
//! ([`super::UiScript::signal_point`]) or the apply's or turn's end, for the engine half's tests to
//! hold to an allowlist that only shrinks. With `BENILLA_SEND_CENSUS` naming a file, each count is
//! also appended to it as `kind count test site`, a whole suite's survey (`test` is the test's name
//! under nextest, else its thread's, and `site` the frames of the app crate that held the first
//! handler). A survey also gives every in-game VM a frame that hears every event, so the whole
//! suite is swept ([`survey_listener`]).
//!
//! A listener that runs late also reads the state the apply wrote after the signal, which no send
//! shows. The host reports each lend of its state (a call applier's, an engine turn's) as it ends,
//! with whether the lend wrote, and the census counts a lend that wrote while a handler was held,
//! under `kind:member` (the owner of the state the lend took), as `after` when the handler was held
//! before the lend began (the write follows the signal) and `within` when it was first held during
//! the lend (the order inside the lend is unobserved, so the hit is a candidate to read). A lend
//! counts as a write when anything it lent was dereferenced mutably, so an applier that takes its
//! state `&mut` without changing it is counted too: each hit is read before it is believed. It watches only where asked
//! ([`watch_writes`], or the survey file named): a lend then takes a change tick of its own to tell
//! its writes from the system's.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static CENSUS: RefCell<Census> = RefCell::default();
}

#[derive(Default)]
struct Census {
    /// What counts the sends made so far, as the host registered it.
    probe: Option<Box<dyn Fn() -> usize>>,
    /// The applies running now, innermost last.
    applying: Vec<Apply>,
    /// The sends counted per kind.
    counts: BTreeMap<String, usize>,
    /// The lends that wrote while a handler was held, counted per kind and member.
    writes: BTreeMap<String, Writes>,
    /// Whether lends take ticks of their own and are scanned for writes: set by [`watch_writes`],
    /// else by the survey file.
    watching: bool,
}

/// An apply or an engine turn running.
struct Apply {
    /// The call's kind, or the turn's caller.
    kind: String,
    /// The sends counted at the first handler it held since its last point.
    mark: Option<usize>,
    /// A handler was held when the lend running now began.
    held_at_lend: bool,
    /// Where the first handler of the window was held, for a survey: the nearest frames of the
    /// app crate.
    site: String,
}

/// The lends of one kind that wrote while a handler was held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    /// Lends that began after a handler was held: the write follows the signal.
    pub after: usize,
    /// Lends in which the first handler was held: the order inside the lend is unobserved.
    pub within: usize,
}

impl Census {
    fn sent(&self) -> usize {
        self.probe.as_ref().map_or(0, |probe| probe())
    }

    /// Count what the innermost apply sent since its first held handler, closing its window.
    fn close(&mut self) {
        let sent = self.sent();
        let Some(apply) = self.applying.last_mut() else {
            return;
        };
        if let Some(mark) = apply.mark.take() {
            let n = sent.saturating_sub(mark);
            if n > 0 {
                *self.counts.entry(apply.kind.clone()).or_default() += n;
                survey(&apply.kind, n, &apply.site);
            }
        }
    }
}

/// Append `kind n test site` to the file `BENILLA_SEND_CENSUS` names, if it names one.
fn survey(kind: &str, n: usize, site: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("BENILLA_SEND_CENSUS") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let test = std::env::var("NEXTEST_TEST_NAME")
            .ok()
            .or_else(|| std::thread::current().name().map(str::to_string))
            .unwrap_or_default();
        let _ = file.write_all(format!("{kind} {n} {test} {site}\n").as_bytes());
    }
}

/// The three nearest frames of the app crate on the stack, as `file:line` joined by `<`.
fn site() -> String {
    let trace = std::backtrace::Backtrace::force_capture().to_string();
    let frames: Vec<&str> = trace
        .lines()
        .filter_map(|l| l.trim().strip_prefix("at "))
        .filter(|at| at.starts_with("./src/") || at.contains("benilla-app/src/"))
        .take(3)
        .map(|at| at.rsplit_once("src/").map_or(at, |(_, rest)| rest))
        .collect();
    frames.join("<")
}

/// With a survey running, a frame that hears every event and does nothing, so each signal an
/// apply makes is held for a listener and the suite's every test is swept, not only those that
/// register one. A test that counts frames or registrations may fail under it: the survey is not a
/// verdict.
pub(crate) fn survey_listener(lua: &mlua::Lua) {
    if std::env::var_os("BENILLA_SEND_CENSUS").is_none() {
        return;
    }
    let _ = lua
        .load(
            r#"local all = CreateFrame("Frame")
            all:RegisterAllEvents()
            all:SetScript("OnEvent", function() end)"#,
        )
        .exec();
}

/// Watch the lends the host reports ([`lend_begins`], [`lend_ends`]) on this thread while the guard
/// lives: a test that counts writes asks for it, and a survey does without.
#[must_use = "the lends are watched while the guard lives"]
pub fn watch_writes() -> WriteWatch {
    CENSUS.with(|c| c.borrow_mut().watching = true);
    WriteWatch(())
}

/// The lends are watched ([`watch_writes`]) until this drops.
pub struct WriteWatch(());

impl Drop for WriteWatch {
    fn drop(&mut self) {
        CENSUS.with(|c| c.borrow_mut().watching = false);
    }
}

/// Whether the host's lends are watched: asked for ([`watch_writes`]), or a survey is running.
pub fn watching_writes() -> bool {
    CENSUS.with(|c| c.borrow().watching) || std::env::var_os("BENILLA_SEND_CENSUS").is_some()
}

/// Count sends with `probe`, the number sent so far on this thread's channel.
pub fn probe(probe: impl Fn() -> usize + 'static) {
    CENSUS.with(|c| c.borrow_mut().probe = Some(Box::new(probe)));
}

/// The sends counted so far, per call kind, taken.
pub fn take() -> BTreeMap<String, usize> {
    CENSUS.with(|c| std::mem::take(&mut c.borrow_mut().counts))
}

/// The lends that wrote while a handler was held so far, per call kind and member, as
/// `kind:member`, taken.
pub fn take_writes() -> BTreeMap<String, Writes> {
    CENSUS.with(|c| std::mem::take(&mut c.borrow_mut().writes))
}

/// A call's apply running, counted under its kind until it drops.
pub(crate) struct Applying(());

impl Applying {
    pub(crate) fn new(call: &super::ScriptCall) -> Self {
        let debug = format!("{call:?}");
        let kind = debug
            .split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or_default()
            .to_string();
        Self::kind(kind)
    }

    /// An engine turn called from `at`, counted as `turn@` and its file and line under `src/`.
    pub(crate) fn turn(at: &std::panic::Location) -> Self {
        let file = at.file();
        let file = file.rsplit_once("src/").map_or(file, |(_, rest)| rest);
        Self::kind(format!("turn@{file}:{}", at.line()))
    }

    fn kind(kind: String) -> Self {
        CENSUS.with(|c| {
            c.borrow_mut().applying.push(Apply {
                kind,
                mark: None,
                held_at_lend: false,
                site: String::new(),
            })
        });
        Applying(())
    }
}

impl Drop for Applying {
    fn drop(&mut self) {
        CENSUS.with(|c| {
            let mut c = c.borrow_mut();
            c.close();
            c.applying.pop();
        });
    }
}

/// A handler was held: the innermost apply's window opens, if it is not open.
pub(crate) fn held() {
    CENSUS.with(|c| {
        let mut c = c.borrow_mut();
        let sent = c.sent();
        if let Some(apply) = c.applying.last_mut() {
            if apply.mark.is_none() && std::env::var_os("BENILLA_SEND_CENSUS").is_some() {
                apply.site = site();
            }
            apply.mark.get_or_insert(sent);
        }
    });
}

/// Whether the innermost apply holds a handler since its last point: a lend that writes now is
/// counted, so the host scans for its writes only then ([`lend_ends`]).
pub fn window_open() -> bool {
    CENSUS.with(|c| c.borrow().applying.last().is_some_and(|a| a.mark.is_some()))
}

/// The innermost apply begins a lend of the host's state.
pub fn lend_begins() {
    CENSUS.with(|c| {
        if let Some(apply) = c.borrow_mut().applying.last_mut() {
            apply.held_at_lend = apply.mark.is_some();
        }
    });
}

/// The innermost apply's lend of `member` ends, having written or not. One that wrote while a
/// handler was held is counted ([`Writes`]).
pub fn lend_ends(wrote: bool, member: &str) {
    CENSUS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(apply) = c.applying.last() else {
            return;
        };
        if !wrote || apply.mark.is_none() {
            return;
        }
        let (kind, after, site) = (
            format!("{}:{member}", apply.kind),
            apply.held_at_lend,
            apply.site.clone(),
        );
        let writes = c.writes.entry(kind.clone()).or_default();
        if after {
            writes.after += 1;
        } else {
            writes.within += 1;
        }
        survey(
            &format!("{kind}~{}", if after { "after" } else { "within" }),
            1,
            &site,
        );
    });
}

/// A signal point runs what the innermost apply held: its window closes.
pub(crate) fn point() {
    CENSUS.with(|c| c.borrow_mut().close());
}

#[cfg(test)]
mod tests {
    use super::super::{CallHost, CallTail, ScriptCall, UiScript};
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    /// A host that signals `PLAYER_TARGET_CHANGED` in each apply, stops at a signal point when
    /// `point`, then sends.
    struct Sending {
        sent: Rc<Cell<usize>>,
        point: bool,
    }

    impl CallHost for Sending {
        fn apply(&mut self, script: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            if self.point {
                script.signal_point(self);
            }
            self.sent.set(self.sent.get() + 1);
            None
        }
    }

    /// A send made while the apply holds a handler is counted under its call's kind; one made
    /// after a signal point ran the handler is not.
    #[test]
    fn a_send_made_while_a_handler_is_held_is_counted_under_its_call() {
        for point in [false, true] {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_TARGET_CHANGED")
                f:SetScript("OnEvent", function() end)"#,
            )
            .unwrap();
            let sent = Rc::new(Cell::new(0));
            let probe = sent.clone();
            super::probe(move || probe.get());
            super::take();
            s.hosted(&mut Sending { sent, point }, |s| s.run("ClearTarget()"))
                .unwrap();
            let counted = super::take();
            if point {
                assert_eq!(counted, BTreeMap::new());
            } else {
                assert_eq!(counted, BTreeMap::from([("ClearTarget".to_string(), 1)]));
            }
        }
    }

    /// An engine turn's send made while a handler it signalled is held is counted under the
    /// turn's caller.
    #[test]
    fn a_turns_send_made_while_a_handler_is_held_is_counted_under_its_caller() {
        let mut s = UiScript::new().unwrap();
        s.run(
            r#"local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_TARGET_CHANGED")
            f:SetScript("OnEvent", function() end)"#,
        )
        .unwrap();
        let sent = Rc::new(Cell::new(0));
        let probe = sent.clone();
        super::probe(move || probe.get());
        super::take();
        let line = line!() + 1;
        s.host_out(|s| {
            s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            sent.set(sent.get() + 1);
        });
        s.refuse_waiting();
        assert_eq!(
            super::take(),
            BTreeMap::from([(format!("turn@script/census.rs:{line}"), 1)])
        );
    }

    /// A host whose apply writes in lends, reported to the census as the host would: one before
    /// the signal, one in which the signal is raised, and one after it; with `point`, a signal
    /// point runs the handler between the second and the third.
    struct Writing {
        point: bool,
    }

    impl CallHost for Writing {
        fn apply(&mut self, script: &mut UiScript, _: ScriptCall) -> Option<CallTail> {
            super::lend_begins();
            super::lend_ends(true, "m");
            super::lend_begins();
            script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
            super::lend_ends(true, "m");
            if self.point {
                script.signal_point(self);
            }
            super::lend_begins();
            super::lend_ends(true, "m");
            super::lend_begins();
            super::lend_ends(false, "m");
            None
        }
    }

    /// A lend that wrote while a handler was held is counted under its call's kind: the one that
    /// began after the signal as a write that follows it, the one that raised it as unordered. A
    /// lend before the signal, one that wrote nothing and one after a signal point are not.
    #[test]
    fn a_lend_that_wrote_while_a_handler_was_held_is_counted_under_its_call() {
        for point in [false, true] {
            let mut s = UiScript::new().unwrap();
            s.run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("PLAYER_TARGET_CHANGED")
                f:SetScript("OnEvent", function() end)"#,
            )
            .unwrap();
            super::take_writes();
            s.hosted(&mut Writing { point }, |s| s.run("ClearTarget()"))
                .unwrap();
            let wrote = if point { 0 } else { 1 };
            assert_eq!(
                super::take_writes(),
                BTreeMap::from([(
                    "ClearTarget:m".to_string(),
                    super::Writes {
                        after: wrote,
                        within: 1
                    }
                )])
            );
        }
    }
}
