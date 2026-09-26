//! `QueryCache<K, V>`, the ask-once cache: the reference's `DBCache<T>` (vtable `0x80912c`). A
//! miss sends the query once, a second lookup of a pending key does not re-send, the response
//! lands the record, and eviction is explicit only.
//!
//! The read that asks takes `&self` (the in-flight set is behind a lock), so read-only systems
//! share the owning resource and its change detection means an answer landed.
//!
//! A negative answer is cached as `None` and never re-asked: an integer-keyed store's high-bit miss
//! (`0x556e67`), or a player's empty name.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Mutex;

use benilla_assets::LockRecover;
use bevy::prelude::*;

use crate::net::EnteredWorldMessage;

pub(crate) struct QueryCache<K, V> {
    answered: HashMap<K, Option<V>>,
    /// The keys in flight, locked so the read that asks is `&self`; uncontended on the main
    /// thread.
    pending: Mutex<HashSet<K>>,
    /// Bumped by every landing or eviction, the reference's `DBCACHECALLBACK` redisplay.
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
    /// The answer for `key`; a miss runs `ask` once per key per connection. `None` for a miss
    /// and a cached negative alike.
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

    /// The answer if it is already here, never asking.
    pub(crate) fn get(&self, key: K) -> Option<&V> {
        self.answered.get(&key)?.as_ref()
    }

    /// The answer, mutably, for a record the wire patches in place; never asking.
    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.answered.get_mut(&key)?.as_mut()
    }

    /// Whether the server answered `key`, with a record or "unknown".
    pub(crate) fn answered(&self, key: K) -> bool {
        self.answered.contains_key(&key)
    }

    /// Whether the server answered "unknown", as opposed to a still-pending ask.
    pub(crate) fn answered_unknown(&self, key: K) -> bool {
        self.answered.get(&key).is_some_and(Option::is_none)
    }

    #[cfg(test)]
    /// Whether an ask for `key` is in flight.
    pub(crate) fn is_pending(&self, key: K) -> bool {
        self.pending.lock_recover().contains(&key)
    }

    /// Lands an answer; `None` is the server's negative.
    pub(crate) fn insert(&mut self, key: K, answer: Option<V>) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        self.answered.insert(key, answer);
        self.generation = self.generation.wrapping_add(1);
    }

    /// The reference's explicit eviction (`SMSG_INVALIDATE_PLAYER` for a player's name): the next
    /// read re-asks. Returns whether there was anything to evict.
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

    /// Forgets the in-flight asks, which a disconnect or a send before the writer existed may
    /// have dropped; [`register`] runs it on every world entry.
    pub(crate) fn clear_pending(&mut self) {
        self.pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    /// Drops everything, answers included (a session-scoped cache on disconnect).
    pub(crate) fn clear(&mut self) {
        self.answered.clear();
        self.clear_pending();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Every answered key with its answer, for a persisted cache's save.
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
