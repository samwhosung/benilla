//! The login queue: our place in line for a full realm and the reference's estimate of the wait.
//! `SMSG_AUTH_RESPONSE(AUTH_WAIT_QUEUE)` carries a position, is re-sent as the line moves, and
//! ends with `AUTH_OK`; it is a wait, not a refusal.
//!
//! The arithmetic is the reference's (`CGlueMgr::UpdateQueuePosition`, `0x46ae50`), in its 32
//! bits, divide-before-multiply truncation and unguarded overflow included: each packet computes
//! the estimate once, against its own time, and stores it; the dialog counts the stored figure
//! down. The dialog's text is the reference's too, the blank line before the estimate included.

/// The reference's ring of `(tick, position)` samples (`0xb41cf8..0xb41d14` ticks,
/// `0xb41d18..0xb41d34` positions).
const SAMPLES: usize = 8;

const MINUTE_MS: i32 = 60_000;

/// The reference's queue-estimate ring: `[0]` is the newest sample, `[SAMPLES - 1]` the oldest.
/// Sampled once per queue packet, on no timer, so its only clock is the server's send cadence.
/// A new value is a fresh connect (`0x5aab20` zeroes the ring and the estimate together).
#[derive(Debug, Default, Clone)]
pub(crate) struct QueueEstimate {
    tick_ms: [u32; SAMPLES],
    position: [u32; SAMPLES],
    /// `[0xb41df0]`: the wait in ms the last packet that moved the queue forward computed; 0 until
    /// one has, and kept by a packet that stalls or steps back. A signed 32-bit word, as there.
    estimate_ms: i32,
}

impl QueueEstimate {
    /// Fold one queue packet in at `now_ms`. Once the ring is full, a repeated position only
    /// restamps the newest tick (the reference's guard), so a repeating server keeps the history.
    /// With the oldest sample set and the newest position below it, the estimate is recomputed
    /// against this packet's time and stored (`0x46aeda`); otherwise it keeps its value
    /// (`0x46aec0`, `0x46aece`).
    pub(crate) fn sample(&mut self, position: u32, now_ms: u32) {
        let repeat = position == self.position[0] && self.tick_ms[SAMPLES - 1] != 0;
        if !repeat {
            self.tick_ms.rotate_right(1);
            self.position.rotate_right(1);
        }
        self.position[0] = position;
        self.tick_ms[0] = now_ms;
        if let Some(ms) = self.computed_ms() {
            self.estimate_ms = ms;
        }
    }

    pub(crate) fn position(&self) -> Option<u32> {
        (self.tick_ms[0] != 0).then_some(self.position[0])
    }

    /// The wait the ring gives at its newest tick, in ms: `-(((newest_tick - oldest_tick) /
    /// (newest_pos - oldest_pos)) * newest_pos)`, every step a signed 32-bit word: `idiv` at
    /// `0x46aed3` truncates toward zero before `imul` at `0x46aed5`, which wraps unguarded, then
    /// `neg`. A wait past `i32::MAX` ms therefore wraps, and one that wraps negative reads as no
    /// estimate. None until the ring is full and the queue has moved forward, as in the reference.
    fn computed_ms(&self) -> Option<i32> {
        if self.tick_ms[SAMPLES - 1] == 0 {
            return None; // fewer than SAMPLES packets so far
        }
        // Negative once the queue moved forward (`0x46aec8` `sub`, `0x46aece` `jns`).
        let gained = (self.position[0] as i32).wrapping_sub(self.position[SAMPLES - 1] as i32);
        if gained >= 0 {
            return None; // stalled, or going backwards
        }
        let elapsed = self.tick_ms[0].wrapping_sub(self.tick_ms[SAMPLES - 1]) as i32;
        // `wrapping_div` stands where `i32::MIN / -1` would fault the reference's `idiv`.
        let per_place = elapsed.wrapping_div(gained);
        Some(
            per_place
                .wrapping_mul(self.position[0] as i32)
                .wrapping_neg(),
        )
    }

    /// The stored estimate, if there is one: `[0xb41df0]` above 0, signed (`0x46bcb8` `jle`).
    pub(crate) fn estimate_ms(&self) -> Option<i32> {
        (self.estimate_ms > 0).then_some(self.estimate_ms)
    }

    /// The dialog text, its `QUEUE_*` template looked up by `lookup(key)`: the stock glue screens
    /// read it off the glue VM's own `GlueStrings.lua`. The reference reads `realm` from the `realmName` CVar it wrote on the way
    /// in, so the `_NAME` variants are the ones that render in practice.
    ///
    /// The shown wait is `tick[0] + estimate - now` in 32 bits (`0x46bcc9`-`0x46bcd7`): the
    /// estimate stands between packets, so a silent queue counts down. Once it passes a minute
    /// short the text is "< 1 minute" and stays so at zero and below (`0x46bcd9`, `0x46bcfa`);
    /// with no estimate yet it is "Calculating...".
    ///
    /// The two estimate texts follow a blank line: `0x46bdd7` joins the status text and the queue
    /// line with `"%s\n%s"`, and the status text is empty while the queue's operation runs
    /// (`0x5aa991` to `0x5aaa39`). The unknown text is shown alone (`0x46be09`).
    pub(crate) fn text_by(
        &self,
        now_ms: u32,
        realm: Option<&str>,
        lookup: impl Fn(&str) -> String,
    ) -> String {
        let (key, remaining) = self.key(now_ms, realm);
        let line = self.fill(&lookup(key), realm, remaining);
        if remaining.is_some() {
            format!("\n{line}")
        } else {
            line
        }
    }

    /// The `QUEUE_*` key for this state, and the wait left in ms once there is an estimate.
    fn key(&self, now_ms: u32, realm: Option<&str>) -> (&'static str, Option<i32>) {
        let named = realm.is_some_and(|r| !r.is_empty());
        let remaining = self.estimate_ms().map(|est| {
            (self.tick_ms[0] as i32)
                .wrapping_add(est)
                .wrapping_sub(now_ms as i32)
        });
        let key = match (named, remaining) {
            (true, None) => "QUEUE_NAME_TIME_LEFT_UNKNOWN",
            (true, Some(ms)) if ms < MINUTE_MS => "QUEUE_NAME_TIME_LEFT_SECONDS",
            (true, Some(_)) => "QUEUE_NAME_TIME_LEFT",
            (false, None) => "QUEUE_TIME_LEFT_UNKNOWN",
            (false, Some(ms)) if ms < MINUTE_MS => "QUEUE_TIME_LEFT_SECONDS",
            (false, Some(_)) => "QUEUE_TIME_LEFT",
        };
        (key, remaining)
    }

    /// The template's holes: the realm's name, the position, then the minutes.
    fn fill(&self, template: &str, realm: Option<&str>, remaining: Option<i32>) -> String {
        let position = self.position().unwrap_or(0);
        let mut out = template.to_string();
        if let Some(realm) = realm.filter(|r| !r.is_empty()) {
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

    /// The `QUEUE_*` templates as the English `GlueStrings.lua` has them.
    fn english(key: &str) -> String {
        match key {
            "QUEUE_NAME_TIME_LEFT_UNKNOWN" => {
                "%s is Full\nPosition in queue: %d\nEstimated time: Calculating..."
            }
            "QUEUE_NAME_TIME_LEFT_SECONDS" => {
                "%s is Full\nPosition in queue: %d\nEstimated time: < 1 minute"
            }
            "QUEUE_NAME_TIME_LEFT" => "%s is Full\nPosition in queue: %d\nEstimated time: %d min",
            "QUEUE_TIME_LEFT_UNKNOWN" => {
                "Realm is Full\nPosition in queue: %d\nEstimated time: Calculating..."
            }
            "QUEUE_TIME_LEFT_SECONDS" => {
                "Realm is Full\nPosition in queue: %d\nEstimated time: < 1 minute"
            }
            _ => "Realm is Full\nPosition in queue: %d\nEstimated time: %d min",
        }
        .to_string()
    }

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
            let (q, _) = ring_of(&places, 1_000);
            assert_eq!(q.estimate_ms(), None, "{n} samples is not enough");
        }
        let (q, _) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert!(q.estimate_ms().is_some(), "eight is");
    }

    #[test]
    fn a_stalled_queue_has_no_estimate() {
        let (q, _) = ring_of(&[50; SAMPLES], 1_000);
        assert_eq!(
            q.estimate_ms(),
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
        let (q, _) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert_eq!(q.estimate_ms(), Some(93_000));
    }

    /// A fixture where the two orders disagree: 10 000 ms over 7 places truncates to 1 428 ms.
    #[test]
    fn the_estimate_divides_before_it_multiplies() {
        let mut q = QueueEstimate::default();
        for (i, p) in [100u32, 99, 98, 97, 96, 95, 94, 93].into_iter().enumerate() {
            q.sample(p, 1_000 + (10_000 * i as u32) / 7);
        }
        let last = 1_000 + 10_000;
        let elapsed = last - 1_000;
        assert_eq!(q.estimate_ms(), Some((elapsed / 7) * 93));
        assert_ne!(
            (elapsed / 7) * 93,
            elapsed * 93 / 7,
            "this fixture must actually distinguish the two orders",
        );
    }

    /// The multiply wraps in 32 bits (`0x46aed5`): 50 000 ms a place with 99 993 ahead is
    /// 4 999 650 000 ms, which the reference's word holds as 704 682 704.
    #[test]
    fn a_long_wait_wraps_in_32_bits() {
        let places: Vec<u32> = (99_993..=100_000).rev().collect();
        let (q, last) = ring_of(&places, 50_000);
        assert_eq!(q.estimate_ms(), Some(704_682_704));
        assert_ne!(
            i64::from(q.estimate_ms().unwrap()),
            50_000 * 99_993,
            "the 64-bit figure is not the reference's"
        );
        let text = q.text_by(last, Some("Benilla"), english);
        assert!(text.ends_with("Estimated time: 11744 min"), "{text}");
    }

    /// A wait that wraps negative is no estimate (`0x46bcb8` `jle`): 30 000 ms a place with 99 993
    /// ahead wraps to -1 295 177 296, and the dialog reads "Calculating...".
    #[test]
    fn a_wait_that_wraps_negative_reads_calculating() {
        let places: Vec<u32> = (99_993..=100_000).rev().collect();
        let (q, last) = ring_of(&places, 30_000);
        assert_eq!(q.estimate_ms(), None);
        let text = q.text_by(last, Some("Benilla"), english);
        assert_eq!(
            text,
            "Benilla is Full\nPosition in queue: 99993\nEstimated time: Calculating..."
        );
    }

    /// The two estimate texts follow a blank line, the empty status text joined by `"%s\n%s"`
    /// (`0x46bdd7`); the unknown text stands alone (`0x46be09`).
    #[test]
    fn the_estimate_texts_follow_a_blank_line() {
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert_eq!(
            q.text_by(last, Some("Benilla"), english),
            "\nBenilla is Full\nPosition in queue: 93\nEstimated time: 1 min"
        );
        assert_eq!(
            q.text_by(last + 60_000, None, english),
            "\nRealm is Full\nPosition in queue: 93\nEstimated time: < 1 minute"
        );
        let (q, last) = ring_of(&[100, 99, 98], 1_000);
        assert_eq!(
            q.text_by(last, None, english),
            "Realm is Full\nPosition in queue: 98\nEstimated time: Calculating..."
        );
    }

    /// The estimate is computed once per packet (`0x46aeda`) and stands between packets, so the
    /// shown wait (`tick[0] + estimate - now`, `0x46bcc9`-`0x46bcd7`) counts down by the time
    /// that passed.
    #[test]
    fn a_silent_queue_counts_down_from_the_stored_estimate() {
        // Ten seconds a place with 93 ahead: 930 s, 15 minutes at the packet.
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 10_000);
        assert_eq!(q.estimate_ms(), Some(930_000));
        for (wait, minutes) in [(0, 15), (60_000, 14), (300_000, 10), (600_000, 5)] {
            let text = q.text_by(last + wait, Some("Benilla"), english);
            assert!(
                text.ends_with(&format!("Estimated time: {minutes} min")),
                "{wait} ms after the packet: {text}"
            );
        }
    }

    /// A packet whose position is not below the oldest sample's, a stall or a step back, keeps the
    /// old estimate (`0x46aec0`, `0x46aece`), and the text keeps counting down from it rather than
    /// falling back to "Calculating...".
    #[test]
    fn a_stalled_or_backward_packet_keeps_the_estimate() {
        let places = [100, 99, 98, 97, 96, 95, 94, 93];
        // Each packet's position, then what it leaves: 99 is the oldest sample's own place once
        // the ring shifts, 200 is above it.
        for (position, what) in [(99, "a stall"), (200, "a step back")] {
            let (mut q, last) = ring_of(&places, 1_000);
            assert_eq!(q.estimate_ms(), Some(93_000));
            q.sample(position, last + 1_000);
            assert!(q.position[0] >= q.position[SAMPLES - 1], "{what}");
            assert_eq!(q.estimate_ms(), Some(93_000), "{what}");
            let text = q.text_by(last + 1_000, Some("Benilla"), english);
            assert!(!text.contains("Calculating"), "{what}: {text}");
            assert!(
                text.contains(&format!("Position in queue: {position}")),
                "{what}: {text}"
            );
        }
    }

    /// A repeat of the newest position restamps its tick and, the ring being full, still
    /// recomputes against it (`0x46ae67` skips only the shift): 17 000 ms over 7 places.
    #[test]
    fn a_repeated_position_recomputes_against_its_own_time() {
        let (mut q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        q.sample(93, last + 10_000);
        assert_eq!(q.estimate_ms(), Some(17_000 / 7 * 93));
    }

    /// A packet that moves the queue forward recomputes the estimate against its own time.
    #[test]
    fn a_forward_packet_replaces_the_estimate() {
        let (mut q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        assert_eq!(q.estimate_ms(), Some(93_000));
        // The oldest sample is now 99 at 2 000 ms: 7 places in 7 000 ms, 92 still ahead.
        q.sample(92, last + 1_000);
        assert_eq!(q.estimate_ms(), Some(92_000));
    }

    /// With no estimate yet the dialog says so, however long it waits.
    #[test]
    fn no_estimate_yet_reads_calculating() {
        let (q, last) = ring_of(&[100, 99, 98, 97], 1_000);
        assert_eq!(q.estimate_ms(), None);
        for wait in [0, 60_000, 600_000] {
            let text = q.text_by(last + wait, Some("Benilla"), english);
            assert!(text.contains("Calculating..."), "{wait}: {text}");
        }
    }

    /// The countdown passing zero reads "< 1 minute" and stays there (`0x46bcd9` takes zero and
    /// `jl` at `0x46bcfa` takes every negative), never "Calculating..." and never minutes.
    #[test]
    fn a_countdown_past_zero_stays_under_a_minute() {
        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        for wait in [33_001, 93_000, 93_001, 200_000, 5_000_000] {
            let text = q.text_by(last + wait, Some("Benilla"), english);
            assert!(
                text.ends_with("Estimated time: < 1 minute"),
                "{wait}: {text}"
            );
        }
    }

    #[test]
    fn the_text_walks_calculating_then_minutes() {
        let (q, last) = ring_of(&[100, 99, 98], 1_000);
        let calculating = q.text_by(last, Some("Benilla"), english);
        assert!(calculating.starts_with("Benilla is Full"), "{calculating}");
        assert!(
            calculating.contains("Position in queue: 98"),
            "{calculating}"
        );
        assert!(calculating.contains("Calculating..."), "{calculating}");

        let (q, last) = ring_of(&[100, 99, 98, 97, 96, 95, 94, 93], 1_000);
        let minutes = q.text_by(last, Some("Benilla"), english);
        assert!(minutes.contains("Position in queue: 93"), "{minutes}");
        assert!(minutes.contains("1 min"), "{minutes}");
        assert!(!minutes.contains("Calculating"), "{minutes}");
    }

    /// Few enough ahead that the inflation cannot carry the wait past a minute.
    #[test]
    fn a_short_wait_reads_under_a_minute() {
        let (q, last) = ring_of(&[8, 7, 6, 5, 4, 3, 2, 1], 1_000);
        let text = q.text_by(last, Some("Benilla"), english);
        assert!(text.contains("Position in queue: 1"), "{text}");
        assert!(text.contains("< 1 minute"), "{text}");
    }

    #[test]
    fn a_nameless_realm_uses_the_plain_form() {
        let (q, last) = ring_of(&[100, 99, 98], 1_000);
        let plain = q.text_by(last, None, english);
        assert!(plain.starts_with("Realm is Full"), "{plain}");
        assert!(!plain.contains("%s") && !plain.contains("%d"), "{plain}");
        let named = q.text_by(last, Some("Benilla"), english);
        assert!(!named.contains("%s") && !named.contains("%d"), "{named}");
    }
}
