//! **The login queue** — our place in line for a full realm, and the reference's own estimate of
//! how long it will take (decision 1681).
//!
//! The realm server answers a login to a full realm with `SMSG_AUTH_RESPONSE(AUTH_WAIT_QUEUE)` and
//! a position, re-sending as the line moves and ending with `AUTH_OK` when we are admitted. It is
//! a **wait, not an outcome** — which is what benilla got wrong until this module: the world
//! handshake treated the code as a refusal, so a busy server was simply unreachable.
//!
//! Everything here is the reference's arithmetic, transcribed (`CGlueMgr::UpdateQueuePosition`
//! `0x46ae50`). It is worth transcribing rather than reinventing because the estimate is visibly
//! wrong in a *specific* way — it divides before it multiplies, so it truncates per-person seconds
//! before scaling them by the whole queue — and a "better" estimator would disagree with the
//! reference by minutes on a long queue.
//!
//! Three things about it are deliberately NOT reproduced, each named at its site: the unguarded
//! 32-bit overflow, the stale-position redisplay on a truncated packet ([`benilla_protocol`]'s
//! parser), and the leading blank line the reference's composite prints.

/// How many `(tick, position)` samples the estimate is formed from — the reference's ring, exactly
/// (`0xb41cf8..0xb41d14` ticks, `0xb41d18..0xb41d34` positions).
const SAMPLES: usize = 8;

/// A minute, in the milliseconds everything here counts in.
const MINUTE_MS: i64 = 60_000;

/// The reference's queue-estimate ring: `[0]` is the newest sample, `[SAMPLES - 1]` the oldest.
///
/// Sampled **once per queue packet, on no timer at all** — so the estimate's whole notion of time
/// is the server's send cadence. A `tick[SAMPLES - 1]` still zero means fewer than eight packets
/// have arrived and no estimate exists yet.
#[derive(Debug, Default, Clone)]
pub(crate) struct QueueEstimate {
    tick_ms: [u32; SAMPLES],
    position: [u32; SAMPLES],
}

impl QueueEstimate {
    /// Fold one queue packet in at `now_ms`.
    ///
    /// **A position that has not moved does not shift the ring** once it is full — it only restamps
    /// the newest tick. That is the reference's own guard, and it is what stops a chatty server
    /// that keeps repeating the same position from flushing the history the estimate is made of.
    pub(crate) fn sample(&mut self, position: u32, now_ms: u32) {
        let repeat = position == self.position[0] && self.tick_ms[SAMPLES - 1] != 0;
        if !repeat {
            self.tick_ms.rotate_right(1);
            self.position.rotate_right(1);
        }
        self.position[0] = position;
        self.tick_ms[0] = now_ms;
    }

    /// Our latest known position, or `None` before any packet.
    pub(crate) fn position(&self) -> Option<u32> {
        (self.tick_ms[0] != 0).then_some(self.position[0])
    }

    /// The estimated wait in milliseconds, or `None` while one cannot be formed.
    ///
    /// `floor((now - oldest_tick) / (oldest_pos - newest_pos)) * newest_pos` — **divide first**,
    /// truncating, then scale. The reference emits `idiv` at `0x46aed3` and `imul` at `0x46aed5`,
    /// in that order, and the order is load-bearing: at 30 s per person and 100 people ahead,
    /// dividing first gives the same answer, but at 90 s per two people it discards the remainder
    /// once for every person in the queue rather than once overall.
    ///
    /// Two preconditions, both the reference's: the ring must be full, and the queue must have
    /// **moved forward** (the oldest position strictly greater than the newest). A queue that has
    /// not moved, or has moved backwards, yields nothing rather than a negative or infinite wait.
    ///
    /// The arithmetic is done in `i64` where the reference uses 32-bit signed and does not guard
    /// the multiply. That overflow is a defect, not a behaviour: reproducing it would put an
    /// absurd countdown on screen for a queue long enough to trigger it.
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

    /// What the dialog says right now.
    ///
    /// `realm` is the realm's name when we know it — the reference reads the `realmName` CVar it
    /// wrote on the way in, and since that is the only door into the queued state, the **`_NAME`
    /// variants are what its players actually saw**; the plain "Realm is Full" forms need an empty
    /// name and effectively never render.
    ///
    /// What is displayed is `tick[0] + estimate - now`. That reads like a countdown and **is not
    /// one**: `estimate` is itself recomputed from `now`, against a divisor fixed by the last eight
    /// packets, so a silent queue's figure *climbs* — its slope is `places_left / places_moved - 1`,
    /// positive whenever more people are ahead of you than the ring saw move. The next packet
    /// re-anchors it. Verified, reproduced deliberately, and pinned by
    /// `a_silent_queue_inflates_its_own_estimate`: a client that counted down smoothly here would
    /// disagree with the reference by minutes.
    ///
    /// **One stated divergence.** The reference composes this as `"%s\n%s"` with an empty first
    /// half — the status leg it means to print is suppressed by a guard sixteen instructions
    /// earlier — so its dialog carries a leading blank line for two of the three states and not the
    /// third. That is a defect of an empty string, not a layout, and it is not reproduced.
    pub(crate) fn text(
        &self,
        now_ms: u32,
        realm: Option<&str>,
        strings: &crate::glue_strings::GlueStrings,
    ) -> String {
        let position = self.position().unwrap_or(0);
        let named = realm.filter(|r| !r.is_empty());

        // `remaining` is what is left of the estimate that was current when the last packet landed.
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
            // The nameless twin of whichever leg we picked — same text without the leading `%s`.
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

    /// Feed packets `step_ms` apart. Returns the ring and **the instant the last one landed**,
    /// which is the natural moment to read the estimate at.
    fn ring_of(places: &[u32], step_ms: u32) -> (QueueEstimate, u32) {
        let mut q = QueueEstimate::default();
        let mut now = 1_000; // never 0 — that is the "no sample yet" sentinel
        let mut last = now;
        for &p in places {
            q.sample(p, now);
            last = now;
            now += step_ms;
        }
        (q, last)
    }

    /// No estimate until the ring is full — the reference's `tick[7] != 0` precondition.
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

    /// A queue that has not moved yields nothing rather than an infinite wait.
    #[test]
    fn a_stalled_queue_has_no_estimate() {
        let (q, last) = ring_of(&[50; SAMPLES], 1_000);
        assert_eq!(
            q.estimate_ms(last),
            None,
            "it has not moved, so nothing divides"
        );
    }

    /// **A repeat stops shifting the ring once it is full** — the reference's own guard, and what
    /// stops a server restating one position from flushing the history the estimate is made of.
    ///
    /// The guard is `newPos == pos[0] && tick[7] != 0`, so while the ring is still filling a repeat
    /// DOES shift; only once the oldest slot is stamped does a repeat become a restamp.
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

    /// Seven moves of one place, a second apart, read as the eighth lands: 7 000 ms over 7 places
    /// is 1 000 ms each, and 93 are still ahead → 93 000 ms.
    #[test]
    fn the_estimate_is_per_place_times_places_left() {
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert_eq!(q.estimate_ms(last), Some(93_000));
    }

    /// **Divide before multiply**, and a fixture where the two orders actually disagree: 7 places
    /// over 10 000 ms is 1 428 ms each once truncated, times 93. Multiplying first would keep the
    /// remainder and land on a different minute at the boundary.
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

    /// **Waiting makes the estimate go UP, not down** — surprising, verified, and the reference's.
    ///
    /// The estimate is recomputed every frame from a *growing* elapsed against a divisor fixed by
    /// the last eight packets, so while the queue is silent the per-place figure inflates. What is
    /// displayed is `tick[0] + estimate - now`, whose slope is `places_left / places_moved - 1` —
    /// positive whenever more people are ahead of you than the ring saw move, which is the normal
    /// case. So a stalled queue's "estimated time" climbs until the next packet re-anchors it.
    ///
    /// Not reproduced as a *bug fix*: this is what the reference puts on screen, and a client that
    /// counted down smoothly here would disagree with it by minutes.
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

    /// The display legs: Calculating before the ring fills, then a minutes figure.
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

    /// The `< 1 minute` leg, which needs the whole remaining wait under a minute — i.e. few enough
    /// people ahead that the inflation above cannot carry it over the threshold.
    #[test]
    fn a_short_wait_reads_under_a_minute() {
        let strings = GlueStrings::default();
        let (q, last) = ring_of(&[8, 7, 6, 5, 4, 3, 2, 1], 1_000);
        let text = q.text(last, Some("Benilla"), &strings);
        assert!(text.contains("Position in queue: 1"), "{text}");
        assert!(text.contains("< 1 minute"), "{text}");
    }

    /// The nameless twin renders when no realm name is known — and neither form leaves a
    /// placeholder behind.
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
