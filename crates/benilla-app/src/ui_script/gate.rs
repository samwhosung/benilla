//! The feed gate: a per-frame UI feed skips its rebuild when no input changed, the
//! change-detection form of the reference's event-driven pushes.
//!
//! The gate sits in the feed's body, never in a `run_if`, and holds every input the body reads:
//! `is_changed()` for a resource, a [`Watch`] on the counter of a generation-counted store (its
//! lazy `&mut` resolves mark it changed every frame), `Changed`/`RemovedComponents` queries, a
//! drained channel's emptiness, and first the VM reset ([`super::VmMemo::get_reset`]). Bind every
//! [`Watch`] before OR-ing them, or a short-circuited one observes a frame late.

/// A remembered counter value; a bool watches as `u64::from(b)`. The default has observed nothing,
/// so the first [`Watch::moved`] reports movement and a VM reset re-opens every watch in its memo.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Watch(Option<u64>);

impl Watch {
    /// Whether `now` differs from the last observation; `now` becomes the next one.
    pub(crate) fn moved(&mut self, now: u64) -> bool {
        self.0.replace(now) != Some(now)
    }
}

/// `WOW_FEED_GATE_CHECK=1`: gated bodies run on a closed gate so [`audit_push`] can check them.
pub(crate) fn auditing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FEED_GATE_CHECK").is_some_and(|v| v != "0"))
}

/// `WOW_FEED_GATE_TRACE=1`: at most once a second per feed, prints which inputs hold it open.
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

/// Called at every push site of a gated feed: a push under a closed gate means a missing input.
#[track_caller]
pub(crate) fn audit_push(gate_closed: bool, feed: &str, what: &str) {
    assert!(
        !gate_closed,
        "feed gate audit: {feed} pushed {what} on a CLOSED gate — \
         an input is missing from its gate"
    );
}

/// A gate verdict: the body runs when `open`, or closed under audit mode (`closed_audit`).
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
        let mut w = Watch::default();
        assert!(w.moved(3));
        assert!(!w.moved(3));
        w = Watch::default();
        assert!(w.moved(3), "the reset memo's watch reports movement again");
    }

    #[test]
    fn a_gate_skips_exactly_when_closed_and_not_auditing() {
        // `auditing()` is off unless the test env sets `WOW_FEED_GATE_CHECK`.
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
