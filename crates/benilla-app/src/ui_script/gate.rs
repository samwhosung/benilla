//! The **feed gate**: the input-side early-out for a per-frame UI feed.
//!
//! Every VM feed is a rebuild-to-diff: build the fresh snapshot, compare against what was last
//! pushed, push only the difference. The diff half was always cheap; the REBUILD half runs every
//! frame whether or not any input moved, and on a parked frame none do — that waste is what 1435
//! priced and 1436 deferred to its own slice. The gate is the honest fix, not a cache bolted on:
//! the reference client pushes on *events* (its handlers run when a field-watch or a packet says
//! so), and an input-change gate is that same shape written in change detection.
//!
//! **The gate lives in the feed's body, never in a `run_if`.** A run condition is a second
//! parameter list that must mirror the system's own, and nothing ties them together — every
//! param added to the body without its condition twin is a silent stale-window bug. In the body,
//! the gate reads the very params it guards, beside their declarations, and the audit mode below
//! can check it.
//!
//! Three rules for writing one:
//!
//! 1. **Every input the body reads is in the gate.** `Res<T>::is_changed()` for plain resources;
//!    a [`Watch`] over the counter for the generation-counted stores (`Cooldowns`, `Items`,
//!    `NameCache`, `GuildState` — counted precisely because their lazy `&mut` resolves mark the
//!    resource changed every frame, poisoning `is_changed` for everyone); `Changed<T>`/
//!    `RemovedComponents<T>` queries for component inputs; an emptiness check for drained
//!    side-channels.
//! 2. **The VM reset is an input.** [`super::VmMemo::get_reset`]'s flag ORs in first: a fresh VM
//!    re-pushes with every other input unchanged (session.rs's whole failure class).
//! 3. **Evaluate every [`Watch`] unconditionally** (bind `let`s, then OR the bools): a `moved`
//!    short-circuited past doesn't observe its counter and would re-open the gate one frame
//!    late — harmless, but a cascade of them re-runs the feed for nothing, N frames deep.
//!
//! **The audit (`WOW_FEED_GATE_CHECK=1`)** is the validator that makes the pattern safe to
//! extend: a closed gate still runs its body, and any push the diff then finds is a missed
//! input — [`audit_push`] panics on the spot, naming the feed. A gate bug's natural form is a
//! window that goes stale until something else pokes it, which no capture and no test reliably
//! sees; the audit converts it into a crash in any probe leg that drives the state.

/// A remembered counter observation: [`Watch::moved`] answers whether the counter changed since
/// the last call, and observes the new value. Default is "never observed", so the first call
/// always reports movement — which is the safe direction, and exactly right inside a
/// [`super::VmMemo`]-wrapped memory: the VM reset defaults the memo, every watch in it re-opens,
/// and the fresh VM gets its full push with no extra wiring.
///
/// Booleans watch too: `moved(u64::from(present))` turns a presence flip into an edge.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Watch(Option<u64>);

impl Watch {
    /// True when `now` differs from the last observed value (or nothing was ever observed);
    /// either way `now` is the observation the next call compares against.
    pub(crate) fn moved(&mut self, now: u64) -> bool {
        self.0.replace(now) != Some(now)
    }
}

/// `WOW_FEED_GATE_CHECK=1` — audit mode: gated feeds run their bodies even on a closed gate and
/// [`audit_push`] panics if the diff then pushes anything. Read once; a probe leg opts in.
pub(crate) fn auditing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FEED_GATE_CHECK").is_some_and(|v| v != "0"))
}

/// `WOW_FEED_GATE_TRACE=1` — once per second per feed, print WHICH gate inputs read open this
/// frame (silent on fully-closed frames). The first question every gate slice asks when a
/// battery reads flat is "which input is holding the gate open?", and it cannot be answered
/// from outside the body — 1439's own wiring anomaly was run to ground with exactly this.
pub(crate) fn trace(feed: &'static str, inputs: &[(&str, bool)]) {
    use std::sync::Mutex;
    use std::time::Instant;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("WOW_FEED_GATE_TRACE").is_some_and(|v| v != "0")) {
        return;
    }
    if !inputs.iter().any(|&(_, open)| open) {
        return;
    }
    static LAST: Mutex<Option<std::collections::HashMap<&'static str, Instant>>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    let map = last.get_or_insert_with(Default::default);
    let now = Instant::now();
    if map
        .get(feed)
        .is_some_and(|t| now.duration_since(*t).as_secs_f32() < 1.0)
    {
        return;
    }
    map.insert(feed, now);
    let open: Vec<&str> = inputs
        .iter()
        .filter_map(|&(name, open)| open.then_some(name))
        .collect();
    eprintln!("[gate-trace] {feed}: open by {}", open.join("+"));
}

/// The audit assertion: called at every push/fire site of a gated feed, with the feed's
/// gate-closed flag. A push reached while the gate is closed means an input is missing from the
/// gate — the silent-stale-window bug, converted to a crash that names itself.
#[track_caller]
pub(crate) fn audit_push(gate_closed: bool, feed: &str, what: &str) {
    assert!(
        !gate_closed,
        "feed gate audit (1439): {feed} pushed {what} on a CLOSED gate — \
         an input is missing from its gate"
    );
}

/// One system's gate verdict, binding the two flags every gated body threads: `open` (run the
/// rebuild) and `closed_audit` (the gate said skip, but audit mode runs the body to check it).
/// [`Gate::skip`] is the early-return test; [`Gate::audit`] forwards to [`audit_push`].
pub(crate) struct Gate {
    open: bool,
    closed_audit: bool,
}

impl Gate {
    /// Judge a gate from its OR-of-inputs verdict.
    pub(crate) fn new(open: bool) -> Self {
        Self {
            open,
            closed_audit: !open && auditing(),
        }
    }

    /// True when the body should not run at all this frame.
    pub(crate) fn skip(&self) -> bool {
        !self.open && !self.closed_audit
    }

    /// Assert (in audit mode) that a push site was not reached under a closed gate.
    #[track_caller]
    pub(crate) fn audit(&self, feed: &str, what: &str) {
        audit_push(self.closed_audit, feed, what);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watch_opens_first_and_on_every_move_only() {
        let mut w = Watch::default();
        assert!(w.moved(7), "never observed → moved");
        assert!(!w.moved(7), "steady → quiet");
        assert!(w.moved(8), "moved → open");
        assert!(!w.moved(8));
        assert!(w.moved(7), "any change counts, direction-free");
    }

    #[test]
    fn a_defaulted_watch_reopens_like_a_fresh_vm() {
        // The property the VmMemo embedding rests on: reset-to-default re-arms the watch.
        let mut w = Watch::default();
        assert!(w.moved(3));
        assert!(!w.moved(3));
        w = Watch::default();
        assert!(w.moved(3), "the reset memo's watch reports movement again");
    }

    #[test]
    fn a_gate_skips_exactly_when_closed_and_not_auditing() {
        // `auditing()` is off in the test env (no WOW_FEED_GATE_CHECK); Gate::new(false) skips.
        let open = Gate::new(true);
        assert!(!open.skip());
        let closed = Gate::new(false);
        assert!(
            closed.skip() || auditing(),
            "closed and not auditing → skip"
        );
        open.audit("test_feed", "anything"); // an open gate never panics
    }

    #[test]
    #[should_panic(expected = "an input is missing from its gate")]
    fn the_audit_names_the_missed_input_class() {
        audit_push(true, "feed_test", "the snapshot");
    }
}
