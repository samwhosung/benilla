//! `QueryCache<K, V>`, the ask-once cache: the reference's `DBCache<T>` (vtable `0x80912c`). A
//! miss sends the query once, a second lookup of a pending key does not re-send, and the response
//! lands the record. Only an explicit eviction, or a refusal, removes a record.
//!
//! The read that asks takes `&self` (the in-flight set is behind a lock), so read-only systems
//! share the owning resource and its change detection means an answer landed.
//!
//! A refusal (an integer-keyed store's top-bit key, `0x556e67`; a player's or a guild's empty
//! name, `0x55f6f0`, `0x561390`) is never stored as an answer. The reference runs the waiting
//! callbacks with status 0 and frees the node (`0x556f05`, `0x556f35`), so the next lookup with a
//! callback finds none and asks again (`0x556aa0`, `0x556b2a`). Its lookups are events (a hover, a
//! window opening); benilla's readers poll, so the callbacks' one run is a held mark:
//! [`QueryCache::answered_unknown`] says "unknown" to every reader that keeps reading the key, and
//! the first read after [`RELOOK_AFTER`] without one is a new lookup, which drops the mark and asks.
//! A mark is never persisted: [`QueryCache::iter`] yields records only.
//!
//! Deviation: a read within [`RELOOK_AFTER`] of the last read of a refused key is the same lookup,
//! where the reference's next lookup asks at once. A reader that polls each frame would otherwise
//! ask again in the frame after every refusal, and a reader that waits for the answer would never
//! be told it.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use benilla_assets::LockRecover;
use bevy::prelude::*;

use crate::net::EnteredWorldMessage;

/// How long a refused key may go unread before a read is a new lookup that asks again. Longer
/// than any waiter's poll period, shorter than the time a person takes to come back to the same
/// item or unit.
pub(crate) const RELOOK_AFTER: Duration = Duration::from_secs(30);

/// The clock the reads stamp refusals with; in a test, the real one moved on by `test_clock`.
fn now() -> Instant {
    let now = Instant::now();
    #[cfg(test)]
    let now = now + test_clock::skew();
    now
}

/// A test's way past [`RELOOK_AFTER`] without waiting: per thread, so one test never moves
/// another's clock.
#[cfg(test)]
pub(crate) mod test_clock {
    use std::cell::Cell;
    use std::time::Duration;

    thread_local! {
        static SKEW: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    }

    pub(crate) fn skew() -> Duration {
        SKEW.with(Cell::get)
    }

    /// Moves this thread's cache clock forward.
    pub(crate) fn advance(by: Duration) {
        SKEW.with(|s| s.set(s.get() + by));
    }

    /// Moves it just past the gap after which a read of a refused key is a new lookup.
    pub(crate) fn past_the_gap() {
        advance(super::RELOOK_AFTER + Duration::from_secs(1));
    }
}

pub(crate) struct QueryCache<K, V> {
    answered: HashMap<K, V>,
    /// The keys in flight, locked so the read that asks is `&self`; uncontended on the main
    /// thread.
    pending: Mutex<HashSet<K>>,
    /// The refused keys, each with the instant of its last read (or its refusal): the reference's
    /// status-0 callback run, held for the readers that poll. Locked like `pending`.
    refused: Mutex<HashMap<K, Instant>>,
    /// Bumped by every landing, refusal or eviction, the reference's `DBCACHECALLBACK` redisplay.
    generation: u64,
}

impl<K: Copy + Eq + Hash, V> Default for QueryCache<K, V> {
    fn default() -> Self {
        Self {
            answered: HashMap::new(),
            pending: Mutex::new(HashSet::new()),
            refused: Mutex::new(HashMap::new()),
            generation: 0,
        }
    }
}

impl<K: Copy + Eq + Hash, V> QueryCache<K, V> {
    /// The answer for `key`; a miss runs `ask` once per key while it is in flight. `None` for a
    /// miss and a refused key alike: a refused key reads as unknown ([`Self::answered_unknown`])
    /// until a read comes [`RELOOK_AFTER`] after the last, which asks again.
    pub(crate) fn get_or_ask(&self, key: K, ask: impl FnOnce()) -> Option<&V> {
        self.get_or_ask_at(key, now, ask)
    }

    /// [`Self::get_or_ask`] with the clock given, read only when `key` is refused.
    fn get_or_ask_at(
        &self,
        key: K,
        now: impl FnOnce() -> Instant,
        ask: impl FnOnce(),
    ) -> Option<&V> {
        if let Some(answer) = self.answered.get(&key) {
            return Some(answer);
        }
        {
            let mut refused = self.refused.lock_recover();
            if let Some(last) = refused.get_mut(&key) {
                let now = now();
                if now.saturating_duration_since(*last) < RELOOK_AFTER {
                    *last = now;
                    return None;
                }
                refused.remove(&key);
            }
        }
        if self.pending.lock_recover().insert(key) {
            ask();
        }
        None
    }

    /// The record if it is already here, never asking.
    pub(crate) fn get(&self, key: K) -> Option<&V> {
        self.answered.get(&key)
    }

    /// The record, mutably, for one the wire patches in place; never asking.
    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.answered.get_mut(&key)
    }

    /// Whether `key` holds a record or a refusal, as opposed to a miss or a pending ask.
    pub(crate) fn answered(&self, key: K) -> bool {
        self.answered.contains_key(&key) || self.answered_unknown(key)
    }

    /// Whether the server refused `key` and no read has since come after a gap, as opposed to a
    /// still-pending ask: the status-0 answer the waiters are told.
    pub(crate) fn answered_unknown(&self, key: K) -> bool {
        self.refused.lock_recover().contains_key(&key)
    }

    #[cfg(test)]
    /// Whether an ask for `key` is in flight.
    pub(crate) fn is_pending(&self, key: K) -> bool {
        self.pending.lock_recover().contains(&key)
    }

    /// Lands a record.
    pub(crate) fn insert(&mut self, key: K, record: V) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        self.refused
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        self.answered.insert(key, record);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Lands a refusal: the waiters read "unknown" and the key is not stored. A record the key
    /// held goes too, as the reference frees the node whatever it held (`0x556f35`).
    pub(crate) fn refuse(&mut self, key: K) {
        self.refuse_at(key, now());
    }

    fn refuse_at(&mut self, key: K, now: Instant) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        self.answered.remove(&key);
        self.refused
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key, now);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The reference's explicit eviction (`SMSG_INVALIDATE_PLAYER` for a player's name): the next
    /// read re-asks. Returns whether there was anything to evict.
    pub(crate) fn evict(&mut self, key: K) -> bool {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        let refused = self
            .refused
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key)
            .is_some();
        let was = self.answered.remove(&key).is_some() || refused;
        if was {
            self.generation = self.generation.wrapping_add(1);
        }
        was
    }

    /// Forgets the in-flight asks, which a disconnect or a send before the writer existed may
    /// have dropped; [`register`] runs it on every world entry.
    pub(crate) fn clear_pending(&mut self) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Drops everything, answers and refusals included (a session-scoped cache on disconnect).
    pub(crate) fn clear(&mut self) {
        self.answered.clear();
        self.refused
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.clear_pending();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Every record with its key, for a persisted cache's save. A refusal is not one.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.answered.iter()
    }

    /// How many records it holds.
    pub(crate) fn len(&self) -> usize {
        self.answered.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.answered.is_empty()
    }
}

/// A resource that owns one or more [`QueryCache`]s: what to forget on world entry.
pub(crate) trait AskOnce: Resource {
    fn clear_pending(&mut self);
}

/// Registers a cache owner to clear its in-flight asks on world entry, so a key asked before the
/// writer existed, or across a disconnect, is asked again.
pub(crate) fn register<T: AskOnce>(app: &mut App) {
    app.add_systems(
        Update,
        release_on_enter::<T>
            .in_set(benilla_world::schedule::WorldStage::Net)
            .after(crate::net::apply_net_updates),
    );
}

fn release_on_enter<T: AskOnce>(
    mut entered: MessageReader<EnteredWorldMessage>,
    mut cache: ResMut<T>,
) {
    if entered.read().count() == 0 {
        return;
    }
    cache.clear_pending();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instant `ms` milliseconds after a fixed origin, the test's clock.
    fn at(ms: u64) -> Instant {
        thread_local! {
            static ORIGIN: Instant = Instant::now();
        }
        ORIGIN.with(|o| *o + Duration::from_millis(ms))
    }

    /// A read of `key` at `ms`, counting the asks it sends in `asks`.
    fn read(cache: &QueryCache<u32, &'static str>, key: u32, ms: u64, asks: &mut u32) -> bool {
        cache.get_or_ask_at(key, || at(ms), || *asks += 1).is_some()
    }

    #[test]
    fn a_miss_asks_once_and_a_landing_answers_it() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        assert!(cache.get_or_ask(7, || asks += 1).is_none());
        assert!(cache.get_or_ask(7, || asks += 1).is_none());
        assert_eq!(
            asks, 1,
            "the second lookup of a pending key does not re-send"
        );
        assert!(cache.is_pending(7));
        cache.insert(7, "seven");
        assert_eq!(cache.get_or_ask(7, || asks += 1), Some(&"seven"));
        assert_eq!(asks, 1);
        assert!(!cache.is_pending(7));
        assert_eq!(cache.generation(), 1);
    }

    /// `0x556e20`'s refusal leg: the waiters run with status 0, the node is freed, and the next
    /// lookup with a callback finds none and asks (`0x556aa0`).
    #[test]
    fn a_refusal_tells_the_waiters_unknown_then_the_next_lookup_asks_again() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        assert!(!read(&cache, 1, 0, &mut asks));
        assert!(cache.is_pending(1));
        assert!(!cache.answered_unknown(1), "a pending ask is not unknown");

        cache.refuse_at(1, at(40));
        assert_eq!(cache.generation(), 1, "the redisplay counter moves");
        assert!(!cache.is_pending(1), "the refusal answers the ask");
        assert!(cache.answered_unknown(1), "the waiters are told unknown");
        assert!(cache.answered(1));
        assert!(cache.get(1).is_none(), "and nothing is stored");

        assert!(!read(&cache, 1, 60, &mut asks));
        assert_eq!(asks, 1, "a waiter's next poll is the same lookup");
        assert!(cache.answered_unknown(1), "and still reads unknown");

        let later = 60 + RELOOK_AFTER.as_millis() as u64;
        assert!(!read(&cache, 1, later, &mut asks));
        assert_eq!(asks, 2, "a read after a gap is a new lookup: it asks again");
        assert!(cache.is_pending(1));
        assert!(!cache.answered_unknown(1), "the mark is gone");

        assert!(!read(&cache, 1, later + 1, &mut asks));
        assert_eq!(asks, 2, "never while one is pending");
    }

    /// A reader that polls every frame, against a server that refuses every ask a frame later:
    /// the literal reference (evict, ask at the next read) would ask in every frame.
    #[test]
    fn a_per_frame_reader_asks_once_per_lookup_not_once_per_frame() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        let mut waited_unknown = 0;
        for frame in 0..1_200u64 {
            let ms = frame * 16;
            if cache.is_pending(5) {
                cache.refuse_at(5, at(ms));
            }
            read(&cache, 5, ms, &mut asks);
            waited_unknown += u32::from(cache.answered_unknown(5));
        }
        assert_eq!(asks, 1, "twenty seconds of frames: one lookup, one ask");
        assert!(
            waited_unknown > 1_100,
            "and every frame after it reads unknown"
        );
    }

    #[test]
    fn a_per_frame_reader_never_asks_twice_while_one_is_pending() {
        let cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        for frame in 0..2_000u64 {
            read(&cache, 9, frame * 16, &mut asks);
        }
        assert_eq!(asks, 1, "no answer in thirty seconds: still the one ask");
    }

    /// Readers that look up now and then, each lookup past the gap, are each one ask: the round
    /// trip bounds a refusing server's cost to one query per lookup.
    #[test]
    fn each_lookup_after_a_gap_asks_once() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        // Past the gap from the last poll of the lookup before (90 ms into it).
        let step = RELOOK_AFTER.as_millis() as u64 + 200;
        let mut asks = 0;
        for lookup in 0..4u64 {
            let ms = lookup * step;
            // Three polls of the same lookup, the server refusing between the first and second.
            read(&cache, 2, ms, &mut asks);
            cache.refuse_at(2, at(ms + 30));
            read(&cache, 2, ms + 60, &mut asks);
            read(&cache, 2, ms + 90, &mut asks);
        }
        assert_eq!(asks, 4);
    }

    #[test]
    fn a_refusal_is_never_a_persisted_record() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.insert(1, "one");
        cache.refuse(2);
        assert_eq!(cache.iter().map(|(&k, _)| k).collect::<Vec<_>>(), [1]);
        assert_eq!(cache.len(), 1);
        assert!(cache.answered_unknown(2));
    }

    /// `0x556f35` frees the node whatever it held.
    #[test]
    fn a_refusal_frees_a_record_the_key_held() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.insert(4, "four");
        cache.refuse(4);
        assert!(cache.get(4).is_none());
        assert!(cache.answered_unknown(4));
    }

    #[test]
    fn a_landing_after_a_refusal_answers_it() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.refuse(6);
        cache.insert(6, "six");
        assert!(!cache.answered_unknown(6));
        assert_eq!(cache.get(6), Some(&"six"));
    }

    #[test]
    fn clearing_pending_lets_a_dropped_ask_go_again_and_keeps_the_answers() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        cache.get_or_ask(1, || asks += 1);
        cache.insert(2, "two");
        cache.clear_pending();
        cache.get_or_ask(1, || asks += 1);
        assert_eq!(asks, 2);
        assert_eq!(cache.get(2), Some(&"two"));
    }

    #[test]
    fn eviction_re_asks() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.insert(3, "three");
        assert!(cache.evict(3));
        assert!(!cache.evict(3));
        let mut asks = 0;
        assert!(cache.get_or_ask(3, || asks += 1).is_none());
        assert_eq!(asks, 1);
    }

    #[test]
    fn evicting_a_refusal_asks_at_the_next_read() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.refuse(8);
        assert!(cache.evict(8));
        let mut asks = 0;
        assert!(!read(&cache, 8, 0, &mut asks));
        assert_eq!(asks, 1);
    }
}
