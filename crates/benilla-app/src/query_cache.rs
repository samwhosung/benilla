//! **`QueryCache<K, V>` — the one ask-once cache**.
//!
//! The reference has one `DBCache<T>` (vtable `0x80912c`): a consumer's miss
//! sends the query and queues a callback; a second lookup of a pending key appends a callback
//! instead of re-sending; the response handler writes the record and fires the callbacks;
//! eviction is explicit only. benilla had that machine hand-copied across eight modules under
//! three names for the in-flight set (`pending`, `querying`, `queried`), the miss and the
//! landing bodies pasted ("the exact twin of `NameCache`"), and the release that keeps an ask
//! sent before the writer thread exists from latching its key for the process lifetime written
//! for two of the eight.
//!
//! **The read that asks takes `&self`.** A miss marks itself in a lock-guarded set and sends
//! its query; the answered map does not move. That is the reason for a type over the copies: a
//! system that only *reads* a name or a template holds the owning resource shared, so two such
//! systems no longer conflict over it — 2287's "pure cache" class measured 556 such undeclared
//! orders — and Bevy's change detection on the owner means "an answer landed" again instead of
//! "someone looked" (the two hand-rolled epochs in `items.rs` were written because it did not).
//!
//! A negative answer is cached as `None` (the server's high-bit miss), so a dead key is never
//! re-asked; [`QueryCache::answered_unknown`] is for the consumer that waits (the cast-fail
//! redisplay) to know when to stop.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Mutex;

use benilla_assets::LockRecover;
use bevy::prelude::*;

use crate::net::EnteredWorldMessage;

pub(crate) struct QueryCache<K, V> {
    answered: HashMap<K, Option<V>>,
    /// The keys in flight. Behind a lock so the read that asks is `&self`; uncontended by
    /// construction (every holder runs on the main thread under the single-threaded executor,
    /// and a miss is the rare path).
    pending: Mutex<HashSet<K>>,
    /// Bumped by every landing, positive or negative — the broadcast a consumer that caches a
    /// view derived from an answer keys on (the reference's `DBCACHECALLBACK` redisplay).
    generation: u64,
}

impl<K: Copy + Eq + Hash, V> Default for QueryCache<K, V> {
    fn default() -> Self {
        Self {
            answered: HashMap::new(),
            pending: Mutex::new(HashSet::new()),
            generation: 0,
        }
    }
}

impl<K: Copy + Eq + Hash, V> QueryCache<K, V> {
    /// The answer for `key` if the server has given one. `None` for a miss — which runs `ask`
    /// once per key per connection — and for a cached negative alike.
    pub(crate) fn get_or_ask(&self, key: K, ask: impl FnOnce()) -> Option<&V> {
        match self.answered.get(&key) {
            Some(answer) => answer.as_ref(),
            None => {
                if self.pending.lock_recover().insert(key) {
                    ask();
                }
                None
            }
        }
    }

    /// The read-only twin: the answer if it is already here, and never an ask.
    pub(crate) fn get(&self, key: K) -> Option<&V> {
        self.answered.get(&key)?.as_ref()
    }

    /// The answer for `key`, mutably — for a record the wire patches in place after it landed
    /// (a guild's rank rename, a petition's new title). Never an ask.
    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.answered.get_mut(&key)?.as_mut()
    }

    /// Has the server answered `key` at all — with a record or with "unknown"?
    pub(crate) fn answered(&self, key: K) -> bool {
        self.answered.contains_key(&key)
    }

    /// Has the server answered `key` with "unknown"? Distinct from a still-pending ask, which
    /// reads `None` from [`Self::get_or_ask`] too.
    pub(crate) fn answered_unknown(&self, key: K) -> bool {
        self.answered.get(&key).is_some_and(Option::is_none)
    }

    #[cfg(test)]
    /// Is an ask for `key` in flight?
    pub(crate) fn is_pending(&self, key: K) -> bool {
        self.pending.lock_recover().contains(&key)
    }

    /// Land an answer; `None` is the server's negative.
    pub(crate) fn insert(&mut self, key: K, answer: Option<V>) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        self.answered.insert(key, answer);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The reference's explicit eviction (a high-bit key, `SMSG_INVALIDATE_PLAYER`): the next
    /// read anywhere re-asks. Returns whether there was anything to evict.
    pub(crate) fn evict(&mut self, key: K) -> bool {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        let was = self.answered.remove(&key).is_some();
        if was {
            self.generation = self.generation.wrapping_add(1);
        }
        was
    }

    /// Forget the in-flight asks — a disconnect may have dropped them on the writer floor, and
    /// an ask sent before the writer existed never left at all. [`register`] runs this on every
    /// world entry for every cache that registers.
    pub(crate) fn clear_pending(&mut self) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Drop everything, answers included (a session-scoped cache on disconnect).
    pub(crate) fn clear(&mut self) {
        self.answered.clear();
        self.clear_pending();
    }

    /// The landing counter — see the field.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Every answered key with its answer (a persisted cache's save walks this).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&K, &Option<V>)> {
        self.answered.iter()
    }

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

/// **The one release-on-enter.** Every cache owner registers here at build; the system runs on
/// the world-enter message and clears the in-flight sets, so a key asked before the io writer
/// existed — or across a disconnect — is asked again. Before 2288 this was written for two of
/// the eight owners (`net::release_ask_once_latches_on_enter`, after the mail send tab's
/// stationery list went missing for a whole session in silence), and the other six were the
/// same dead-feature class, still armed.
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
        cache.insert(7, Some("seven"));
        assert_eq!(cache.get_or_ask(7, || asks += 1), Some(&"seven"));
        assert_eq!(asks, 1);
        assert!(!cache.is_pending(7));
        assert_eq!(cache.generation(), 1);
    }

    #[test]
    fn a_negative_is_cached_and_never_re_asked() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        cache.get_or_ask(1, || asks += 1);
        cache.insert(1, None);
        assert!(cache.get_or_ask(1, || asks += 1).is_none());
        assert_eq!(asks, 1);
        assert!(cache.answered_unknown(1));
        assert!(!cache.answered_unknown(2));
    }

    #[test]
    fn clearing_pending_lets_a_dropped_ask_go_again_and_keeps_the_answers() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        cache.get_or_ask(1, || asks += 1);
        cache.insert(2, Some("two"));
        cache.clear_pending();
        cache.get_or_ask(1, || asks += 1);
        assert_eq!(asks, 2);
        assert_eq!(cache.get(2), Some(&"two"));
    }

    #[test]
    fn eviction_re_asks() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.insert(3, Some("three"));
        assert!(cache.evict(3));
        assert!(!cache.evict(3));
        let mut asks = 0;
        assert!(cache.get_or_ask(3, || asks += 1).is_none());
        assert_eq!(asks, 1);
    }
}
