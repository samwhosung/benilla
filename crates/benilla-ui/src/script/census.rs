//! The census of the sends a call's apply makes while a handler it signalled is held, for test
//! builds. `SignalEvent` (`0x703e50`) runs every listener before the verb goes on, so a send made
//! while one waits goes out ahead of the packets its handler sends in 1.12.1. Each such send is
//! counted under its call's kind, from the first handler the apply held to the signal point that
//! runs it ([`super::UiScript::signal_point`]) or the apply's return, for the engine half's tests
//! to hold to an allowlist that only shrinks.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static CENSUS: RefCell<Census> = RefCell::default();
}

#[derive(Default)]
struct Census {
    /// What counts the sends made so far, as the host registered it.
    probe: Option<Box<dyn Fn() -> usize>>,
    /// The applies running now, innermost last: the call's kind, and the sends counted at the
    /// first handler it held since its last point.
    applying: Vec<(String, Option<usize>)>,
    /// The sends counted per kind.
    counts: BTreeMap<String, usize>,
}

impl Census {
    fn sent(&self) -> usize {
        self.probe.as_ref().map_or(0, |probe| probe())
    }

    /// Count what the innermost apply sent since its first held handler, closing its window.
    fn close(&mut self) {
        let sent = self.sent();
        let Some((kind, mark)) = self.applying.last_mut() else {
            return;
        };
        if let Some(mark) = mark.take() {
            let n = sent.saturating_sub(mark);
            if n > 0 {
                *self.counts.entry(kind.clone()).or_default() += n;
            }
        }
    }
}

/// Count sends with `probe`, the number sent so far on this thread's channel.
pub fn probe(probe: impl Fn() -> usize + 'static) {
    CENSUS.with(|c| c.borrow_mut().probe = Some(Box::new(probe)));
}

/// The sends counted so far, per call kind, taken.
pub fn take() -> BTreeMap<String, usize> {
    CENSUS.with(|c| std::mem::take(&mut c.borrow_mut().counts))
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
        CENSUS.with(|c| c.borrow_mut().applying.push((kind, None)));
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
        if let Some((_, mark)) = c.applying.last_mut() {
            mark.get_or_insert(sent);
        }
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
}
