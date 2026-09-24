//! The last [`CAPACITY`] log lines, kept in memory so a crash report carries what the client was
//! doing when it died. Installed as [`bevy::log::LogPlugin::custom_layer`] by
//! [`crate::boot::tuned_default_plugins`], under the same `EnvFilter` as stderr, so it holds
//! exactly the lines stderr showed. Read with [`recent`].

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bevy::log::tracing::field::{Field, Visit};
use bevy::log::tracing::{Event, Subscriber};
use bevy::log::tracing_subscriber::layer::Context;
use bevy::log::tracing_subscriber::Layer;

/// How many lines the ring keeps: about a minute of an ordinary session's `info` stream.
pub const CAPACITY: usize = 200;

static RING: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static START: OnceLock<Instant> = OnceLock::new();

/// The layer; its state is a static so the panic hook, which has no handle to the subscriber, can
/// read it.
pub struct LogRing;

impl<S: Subscriber> Layer<S> for LogRing {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let start = START.get_or_init(Instant::now);
        let meta = event.metadata();
        let mut line = format!(
            "+{:9.3}s {:5} {}: ",
            start.elapsed().as_secs_f64(),
            meta.level(),
            meta.target()
        );
        event.record(&mut MessageVisitor(&mut line));
        let mut ring = RING.lock().unwrap_or_else(|p| p.into_inner());
        if ring.len() >= CAPACITY {
            ring.pop_front();
        }
        ring.push_back(line);
    }
}

/// Renders an event's fields as the stderr formatter does: `message` bare, the rest as
/// ` name=value`.
struct MessageVisitor<'a>(&'a mut String);

impl Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

/// A snapshot of the ring, oldest first.
pub fn recent() -> Vec<String> {
    RING.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::log::tracing_subscriber::layer::SubscriberExt;

    /// One test for the whole contract, since the ring is a process-wide static.
    #[test]
    fn the_ring_keeps_the_last_lines_in_order() {
        let subscriber = bevy::log::tracing_subscriber::registry().with(LogRing);
        bevy::log::tracing::subscriber::with_default(subscriber, || {
            for i in 0..(CAPACITY + 5) {
                bevy::log::tracing::info!(n = i, "ring line");
            }
        });
        let lines = recent();
        assert_eq!(lines.len(), CAPACITY, "bounded at CAPACITY");
        let first = &lines[0];
        assert!(
            first.contains("ring line n=5"),
            "oldest surviving line: {first}"
        );
        assert!(first.contains(" INFO "), "level rendered: {first}");
        assert!(
            lines
                .last()
                .unwrap()
                .contains(&format!("n={}", CAPACITY + 4)),
            "newest last: {}",
            lines.last().unwrap()
        );
    }
}
