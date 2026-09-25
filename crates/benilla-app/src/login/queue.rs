//! The login queue: our place in line for a full realm and the reference's estimate of the wait.
//! `SMSG_AUTH_RESPONSE(AUTH_WAIT_QUEUE)` carries a position, is re-sent as the line moves, and
//! ends with `AUTH_OK`; it is a wait, not a refusal.
//!
//! The arithmetic is the reference's (`CGlueMgr::UpdateQueuePosition`, `0x46ae50`), including
//! its divide-before-multiply truncation. Three reference defects are not reproduced: the 32-bit
//! overflow, the leading blank line and the stale-position redisplay on a truncated packet
//! ([`benilla_protocol`]'s parser).

/// The reference's ring of `(tick, position)` samples (`0xb41cf8..0xb41d14` ticks,
/// `0xb41d18..0xb41d34` positions).
const SAMPLES: usize = 8;

const MINUTE_MS: i64 = 60_000;

/// The reference's queue-estimate ring: `[0]` is the newest sample, `[SAMPLES - 1]` the oldest.
/// Sampled once per queue packet, on no timer, so its only clock is the server's send cadence.
#[derive(Debug, Default, Clone)]
pub(crate) struct QueueEstimate {
    tick_ms: [u32; SAMPLES],
    position: [u32; SAMPLES],
}

impl QueueEstimate {
    /// Fold one queue packet in at `now_ms`. Once the ring is full, a repeated position only
    /// restamps the newest tick (the reference's guard), so a repeating server keeps the history.
    pub(crate) fn sample(&mut self, position: u32, now_ms: u32) {
        let repeat = position == self.position[0] && self.tick_ms[SAMPLES - 1] != 0;
        if !repeat {
            self.tick_ms.rotate_right(1);
            self.position.rotate_right(1);
        }
        self.position[0] = position;
        self.tick_ms[0] = now_ms;
    }

    pub(crate) fn position(&self) -> Option<u32> {
        (self.tick_ms[0] != 0).then_some(self.position[0])
    }

    /// The estimated wait in ms: `floor((now - oldest_tick) / (oldest_pos - newest_pos)) *
    /// newest_pos`, dividing first (`idiv` at `0x46aed3` before `imul` at `0x46aed5`). None
    /// until the ring is full and the queue has moved forward, as in the reference.
    ///
    /// Computed in `i64`: the reference's 32-bit multiply overflows unguarded, which on a long
    /// queue shows an absurd countdown.
    pub(crate) fn estimate_ms(&self, now_ms: u32) -> Option<i64> {
        if self.tick_ms[SAMPLES - 1] == 0 {
            return None; // fewer than SAMPLES packets so far
        }
        let moved = i64::from(self.position[SAMPLES - 1]) - i64::from(self.position[0]);
        if moved <= 0 {
            return None; // stalled, or going backwards
        }
        let elapsed = i64::from(now_ms.wrapping_sub(self.tick_ms[SAMPLES - 1]));
        let per_place = elapsed / moved;
        Some(per_place * i64::from(self.position[0]))
    }

    /// The dialog text. The reference reads `realm` from the `realmName` CVar it wrote on the way
    /// in, so the `_NAME` variants are the ones that render in practice.
    ///
    /// The shown wait, `tick[0] + estimate - now`, is not a countdown: `estimate` grows with
    /// `now`, so a silent queue's figure climbs (slope `places_left / places_moved - 1`) until
    /// the next packet, as in the reference.
    ///
    /// The reference composes `"%s\n%s"` with an always-empty first half, a leading blank line in
    /// two of three states; that blank line is not reproduced.
    pub(crate) fn text(
        &self,
        now_ms: u32,
        realm: Option<&str>,
        strings: &crate::glue_strings::GlueStrings,
    ) -> String {
        let position = self.position().unwrap_or(0);
        let named = realm.filter(|r| !r.is_empty());

        let remaining = self.estimate_ms(now_ms).and_then(|est| {
            (est > 0).then(|| i64::from(self.tick_ms[0]) + est - i64::from(now_ms))
        });

        let (key, fallback) = match remaining {
            None => (
                "QUEUE_NAME_TIME_LEFT_UNKNOWN",
                "%s is Full\nPosition in queue: %d\nEstimated time: Calculating...",
            ),
            Some(ms) if ms < MINUTE_MS => (
                "QUEUE_NAME_TIME_LEFT_SECONDS",
                "%s is Full\nPosition in queue: %d\nEstimated time: < 1 minute",
            ),
            Some(_) => (
                "QUEUE_NAME_TIME_LEFT",
                "%s is Full\nPosition in queue: %d\nEstimated time: %d min",
            ),
        };
        let (key, fallback) = match named {
            Some(_) => (key, fallback),
            None => match remaining {
                None => (
                    "QUEUE_TIME_LEFT_UNKNOWN",
                    "Realm is Full\nPosition in queue: %d\nEstimated time: Calculating...",
                ),
                Some(ms) if ms < MINUTE_MS => (
                    "QUEUE_TIME_LEFT_SECONDS",
                    "Realm is Full\nPosition in queue: %d\nEstimated time: < 1 minute",
                ),
                Some(_) => (
                    "QUEUE_TIME_LEFT",
                    "Realm is Full\nPosition in queue: %d\nEstimated time: %d min",
                ),
            },
        };

        let mut out = strings.text(key, fallback).to_string();
        if let Some(realm) = named {
            out = out.replacen("%s", realm, 1);
        }
        out = out.replacen("%d", &position.to_string(), 1);
        if let Some(ms) = remaining.filter(|ms| *ms >= MINUTE_MS) {
            out = out.replacen("%d", &(ms / MINUTE_MS).to_string(), 1);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue_strings::GlueStrings;

    /// Feed packets `step_ms` apart; returns the ring and the instant the last one landed.
    fn ring_of(places: &[u32], step_ms: u32) -> (QueueEstimate, u32) {
        let mut q = QueueEstimate::default();
        let mut now = 1_000; // never 0, the "no sample yet" sentinel
        let mut last = now;
        for &p in places {
            q.sample(p, now);
            last = now;
            now += step_ms;
        }
        (q, last)
    }

    /// The reference's `tick[7] != 0` precondition.
    #[test]
    fn an_estimate_needs_eight_samples() {
        for n in 1..SAMPLES {
            let places: Vec<u32> = (0..n as u32).map(|i| 100 - i).collect();
            let (q, last) = ring_of(&places, 1_000);
            assert_eq!(q.estimate_ms(last), None, "{n} samples is not enough");
        }
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert!(q.estimate_ms(last).is_some(), "eight is");
    }

    #[test]
    fn a_stalled_queue_has_no_estimate() {
        let (q, last) = ring_of(&[50; SAMPLES], 1_000);
        assert_eq!(
            q.estimate_ms(last),
            None,
            "it has not moved, so nothing divides"
        );
    }

    /// The reference's guard is `newPos == pos[0] && tick[7] != 0`: a repeat still shifts while
    /// the ring fills.
    #[test]
    fn a_repeated_position_restamps_rather_than_shifting() {
        let (mut q, last) = ring_of(&[50; SAMPLES], 1_000);
        let oldest = q.tick_ms[SAMPLES - 1];
        assert_ne!(
            oldest, 0,
            "eight packets fill the ring even when they repeat"
        );

        q.sample(50, last + 5_000);
        assert_eq!(
            q.tick_ms[SAMPLES - 1],
            oldest,
            "a repeat past a full ring must not push the oldest sample out",
        );
        assert_eq!(q.tick_ms[0], last + 5_000, "but it does restamp the newest");
    }

    /// 7 000 ms over 7 places is 1 000 ms each, times 93 still ahead.
    #[test]
    fn the_estimate_is_per_place_times_places_left() {
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert_eq!(q.estimate_ms(last), Some(93_000));
    }

    /// A fixture where the two orders disagree: 10 000 ms over 7 places truncates to 1 428 ms.
    #[test]
    fn the_estimate_divides_before_it_multiplies() {
        let mut q = QueueEstimate::default();
        for (i, p) in [100u32, 99, 98, 97, 96, 95, 94, 93].into_iter().enumerate() {
            q.sample(p, 1_000 + (10_000 * i as u32) / 7);
        }
        let last = 1_000 + 10_000;
        let elapsed = i64::from(last - 1_000);
        assert_eq!(q.estimate_ms(last), Some((elapsed / 7) * 93));
        assert_ne!(
            (elapsed / 7) * 93,
            elapsed * 93 / 7,
            "this fixture must actually distinguish the two orders",
        );
    }

    /// Waiting raises the estimate, as in the reference: elapsed grows against a fixed divisor.
    #[test]
    fn a_silent_queue_inflates_its_own_estimate() {
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        let at_packet = q.estimate_ms(last).unwrap();
        let much_later = q.estimate_ms(last + 40_000).unwrap();
        assert!(
            much_later > at_packet,
            "elapsed grows while the divisor does not: {at_packet} -> {much_later}",
        );
    }

    #[test]
    fn the_text_walks_calculating_then_minutes() {
        let strings = GlueStrings::default();
        let (q, last) = ring_of(&[100, 99, 98], 1_000);
        let calculating = q.text(last, Some("Benilla"), &strings);
        assert!(calculating.starts_with("Benilla is Full"), "{calculating}");
        assert!(
            calculating.contains("Position in queue: 98"),
            "{calculating}"
        );
        assert!(calculating.contains("Calculating..."), "{calculating}");

        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        let minutes = q.text(last, Some("Benilla"), &strings);
        assert!(minutes.contains("Position in queue: 93"), "{minutes}");
        assert!(minutes.contains("1 min"), "{minutes}");
        assert!(!minutes.contains("Calculating"), "{minutes}");
    }

    /// Few enough ahead that the inflation cannot carry the wait past a minute.
    #[test]
    fn a_short_wait_reads_under_a_minute() {
        let strings = GlueStrings::default();
        let (q, last) = ring_of(&[8, 7, 6, 5, 4, 3, 2, 1], 1_000);
        let text = q.text(last, Some("Benilla"), &strings);
        assert!(text.contains("Position in queue: 1"), "{text}");
        assert!(text.contains("< 1 minute"), "{text}");
    }

    #[test]
    fn a_nameless_realm_uses_the_plain_form() {
        let strings = GlueStrings::default();
        let (q, last) = ring_of(&[100, 99, 98], 1_000);
        let plain = q.text(last, None, &strings);
        assert!(plain.starts_with("Realm is Full"), "{plain}");
        assert!(!plain.contains("%s") && !plain.contains("%d"), "{plain}");
        let named = q.text(last, Some("Benilla"), &strings);
        assert!(!named.contains("%s") && !named.contains("%d"), "{named}");
    }
}
