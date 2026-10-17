//! `QueryCache<K, V>`, the ask-once cache: the reference's `DBCache<T>` (vtable `0x80912c`). A
//! lookup that asks (the reference's lookup with a callback, `0x556aa0`) sends the query on a
//! miss, once while it is pending (`0x556b2a`, `0x556bd2`); a lookup that only reads
//! ([`QueryCache::get`], the same function with no callback, `0x556b1c`) never sends.
//!
//! The reads that ask take `&self` (the in-flight set is behind a lock), so read-only systems
//! share the owning resource and its change detection means an answer landed.
//!
//! A refusal (an integer-keyed store's top-bit key, `0x556e67`; a player's, a guild's or a quest's
//! empty name, `0x55f6f0`, `0x561390`, `0x563150`) is never stored as an answer. The reference runs
//! the key's callbacks with status 0 and frees its node, pending or valid (`0x556f05`,
//! `0x556f35`), so the next lookup that asks finds none and asks again; a refusal for a key with
//! no node does nothing (`0x556ec9`). [`QueryCache::answered_unknown`] holds the status-0 answer
//! for the readers until a lookup asks again. A refusal is never persisted: [`QueryCache::iter`]
//! yields records only.
//!
//! Most of the reference's lookups with a callback run on events: a unit's create, a field change,
//! a packet, and the interface events on which the stock UI calls its getters (a window's show,
//! its update event). Some poll: a hovered bag slot's tooltip build each frame, the minimap's blip
//! probe, a popup's or a map's `OnUpdate` getter. A polled lookup asks at each run, so a refused
//! key is asked again about once per answer. Many of benilla's readers build their view every
//! frame, so a read says which it is with [`Ask`]: [`Ask::Event`] on the run that stands for the
//! reference's event or poll, which asks a refused key again at once ([`QueryCache::lookup`]);
//! [`Ask::New`] on the runs in between, which asks only a key the cache holds nothing for, so the
//! key a reader shows is asked when it first appears and a refused one waits for the next event;
//! and [`Ask::Read`] where the reference's lookup passes no callback.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Mutex;

use benilla_assets::LockRecover;
use bevy::prelude::*;

use crate::net::{EnteredWorldMessage, NetCommands};

/// How a read reaches the server on a miss: the reference's lookup with a callback or without, and
/// the runs of a reader that builds its view every frame between the events that stand for the
/// reference's (the module's head).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ask<'a> {
    /// The lookup with no callback (`0x556b1c` → `0x556bf9`): the record if held, never asking.
    Read,
    /// A reader's run between its events: asks a key the cache holds nothing for, once while it
    /// is pending; a refused key reads unknown and asks nothing until an event's lookup.
    New(&'a NetCommands),
    /// The lookup with a callback, at an event (`0x556aa0`): asks a miss once while it is
    /// pending, and a refused key again at once.
    Event(&'a NetCommands),
}

impl<'a> Ask<'a> {
    /// [`Ask::Event`] on a reader's event, [`Ask::New`] on its other runs.
    pub(crate) fn at(event: bool, commands: &'a NetCommands) -> Self {
        if event {
            Self::Event(commands)
        } else {
            Self::New(commands)
        }
    }
}

/// What a refusal found under its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// No node: nothing happened (`0x556ec9` → `0x556fcf`).
    Nothing,
    /// A pending ask, now answered "unknown".
    Pending,
    /// A record, now gone (`0x556f35` frees a valid node too): what the readers showed changes.
    Record,
}

/// The ask-once cache.
pub(crate) struct QueryCache<K, V> {
    answered: HashMap<K, V>,
    /// The keys in flight, locked so the read that asks is `&self`; uncontended on the main
    /// thread.
    pending: Mutex<HashSet<K>>,
    /// The refused keys: the status-0 answer, held until the next lookup with a callback. Locked
    /// like `pending`, since that lookup drops it.
    refused: Mutex<HashSet<K>>,
    /// Bumped by every landing, and by every refusal that found a node. A refused pending node's
    /// callbacks run with status 0 (`0x556f05`); a node that held a record has none left, as they
    /// are freed when it lands (`0x556fa0`-`0x556fc9`), so the bump for [`Refused::Record`] is
    /// benilla's own redisplay of the record's going.
    generation: u64,
}

impl<K: Copy + Eq + Hash, V> Default for QueryCache<K, V> {
    fn default() -> Self {
        Self {
            answered: HashMap::new(),
            pending: Mutex::new(HashSet::new()),
            refused: Mutex::new(HashSet::new()),
            generation: 0,
        }
    }
}

/// Key 0 names no record: each store's lookup returns null for it before anything else (an item's
/// `0x55ba42`, a guild's `0x560e3d`, a name's `0x55f0a5`), so it is never asked and never read.
impl<K: Copy + Eq + Hash + Default, V> QueryCache<K, V> {
    /// The reference's lookup with a callback (`0x556aa0`): the record, or `None` after running
    /// `ask` on a miss, once while the key is pending. A refused key is a miss: the lookup drops
    /// its "unknown" and asks again at once.
    pub(crate) fn lookup(&self, key: K, ask: impl FnOnce()) -> Option<&V> {
        self.ask_if(key, true, ask)
    }

    /// The lookup of a reader's run between its events ([`Ask::New`]): the record, or `None`
    /// after running `ask` for a key the cache holds nothing for, once while it is pending. A
    /// refused key reads unknown and asks nothing: the next [`Self::lookup`] asks it.
    fn lookup_new(&self, key: K, ask: impl FnOnce()) -> Option<&V> {
        self.ask_if(key, false, ask)
    }

    /// The read [`Ask`] says: `send` sends the query, given the commands it carries.
    pub(crate) fn read(&self, key: K, ask: Ask<'_>, send: impl FnOnce(&NetCommands)) -> Option<&V> {
        match ask {
            Ask::Read => self.get(key),
            Ask::New(commands) => self.lookup_new(key, || send(commands)),
            Ask::Event(commands) => self.lookup(key, || send(commands)),
        }
    }

    /// The asking lookup; `relook` says whether a refused key is asked again.
    fn ask_if(&self, key: K, relook: bool, ask: impl FnOnce()) -> Option<&V> {
        if key == K::default() {
            return None;
        }
        if let Some(answer) = self.answered.get(&key) {
            return Some(answer);
        }
        {
            let mut refused = self.refused.lock_recover();
            if refused.contains(&key) {
                if !relook {
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

    /// The lookup that only reads: the record if it is here, never asking (`0x556b1c` →
    /// `0x556bf9`).
    pub(crate) fn get(&self, key: K) -> Option<&V> {
        (key != K::default())
            .then(|| self.answered.get(&key))
            .flatten()
    }

    /// The record, mutably, for one the wire patches in place; never asking.
    pub(crate) fn get_mut(&mut self, key: K) -> Option<&mut V> {
        self.answered.get_mut(&key)
    }

    /// Whether `key` holds a record or a refusal, as opposed to a miss or a pending ask.
    pub(crate) fn answered(&self, key: K) -> bool {
        self.answered.contains_key(&key) || self.answered_unknown(key)
    }

    /// Whether the server refused `key` and nothing has asked since, as opposed to a pending ask:
    /// the status-0 answer the waiters are told.
    pub(crate) fn answered_unknown(&self, key: K) -> bool {
        self.refused.lock_recover().contains(&key)
    }

    /// Whether the cache holds anything for `key`: a record, a pending ask or a refusal. A key it
    /// knows nothing of was never asked, or its ask was dropped ([`Self::clear_pending`]).
    pub(crate) fn knows(&self, key: K) -> bool {
        self.answered.contains_key(&key)
            || self.answered_unknown(key)
            || self.pending.lock_recover().contains(&key)
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

    /// Lands a refusal (`0x556ec0`-`0x556f37`): a key with a node, pending or holding a record,
    /// reads "unknown" and the node goes; a key with none is left as it was. Says which.
    pub(crate) fn refuse(&mut self, key: K) -> Refused {
        let was_pending = self
            .pending
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
        let found = if self.answered.remove(&key).is_some() {
            Refused::Record
        } else if was_pending {
            Refused::Pending
        } else {
            return Refused::Nothing;
        };
        self.refused
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .insert(key);
        self.generation = self.generation.wrapping_add(1);
        found
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
            .remove(&key);
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

/// The cache queries `rx` holds, drained: what a reader's runs asked.
#[cfg(test)]
pub(crate) fn queries(
    rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>,
) -> Vec<crate::net::ClientCommand> {
    use crate::net::ClientCommand as C;
    rx.try_iter()
        .filter(|c| {
            matches!(
                c,
                C::NameQuery { .. }
                    | C::CreatureQuery { .. }
                    | C::PetNameQuery { .. }
                    | C::ItemQuery { .. }
                    | C::NpcTextQuery { .. }
                    | C::GameObjectQuery { .. }
                    | C::PageTextQuery { .. }
                    | C::QuestQuery { .. }
                    | C::ItemTextQuery { .. }
                    | C::GuildQuery { .. }
                    | C::PetitionQuery { .. }
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Key 0 is null before the cache is searched (`0x55ba42`): no mode asks it or finds it.
    #[test]
    fn key_zero_is_never_asked_nor_found() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let send = |c: &NetCommands| {
            let _ = c.0.send(crate::net::ClientCommand::NameQuery { guid: 0 });
        };
        for ask in [Ask::Read, Ask::New(&commands), Ask::Event(&commands)] {
            assert!(cache.read(0, ask, send).is_none());
        }
        assert!(rx.try_iter().next().is_none(), "nothing asked");
        cache.insert(0, "zero");
        for ask in [Ask::Read, Ask::New(&commands), Ask::Event(&commands)] {
            assert!(
                cache.read(0, ask, send).is_none(),
                "a record under 0 is never read"
            );
        }
    }

    #[test]
    fn a_miss_asks_once_and_a_landing_answers_it() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        assert!(cache.lookup_new(7, || asks += 1).is_none());
        assert!(cache.lookup_new(7, || asks += 1).is_none());
        assert_eq!(
            asks, 1,
            "the second lookup of a pending key does not re-send"
        );
        assert!(cache.is_pending(7));
        cache.insert(7, "seven");
        assert_eq!(cache.lookup_new(7, || asks += 1), Some(&"seven"));
        assert_eq!(asks, 1);
        assert!(!cache.is_pending(7));
        assert_eq!(cache.generation(), 1);
    }

    /// `0x556b1c` → `0x556bf9`: the lookup with no callback sends nothing on a miss.
    #[test]
    fn a_read_never_asks() {
        let cache: QueryCache<u32, &str> = QueryCache::default();
        assert!(cache.get(3).is_none());
        assert!(!cache.is_pending(3));
    }

    /// `0x556e20`'s refusal leg: the waiters run with status 0, the node is freed, and the next
    /// lookup with a callback finds none and asks (`0x556aa0`); reads in between send nothing.
    #[test]
    fn a_refusal_tells_the_waiters_unknown_then_the_next_lookup_asks_again() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        assert!(cache.lookup(1, || asks += 1).is_none());
        assert!(!cache.answered_unknown(1), "a pending ask is not unknown");

        assert_eq!(cache.refuse(1), Refused::Pending);
        assert_eq!(cache.generation(), 1, "the status-0 run is a callback run");
        assert!(!cache.is_pending(1), "the refusal answers the ask");
        assert!(cache.answered_unknown(1), "the waiters are told unknown");
        assert!(cache.answered(1));
        assert!(cache.get(1).is_none(), "and nothing is stored");

        for _ in 0..1_000 {
            assert!(cache.get(1).is_none());
        }
        assert!(cache.answered_unknown(1), "reads leave the answer as it is");

        assert!(cache.lookup(1, || asks += 1).is_none());
        assert_eq!(asks, 2, "the next lookup asks again at once");
        assert!(cache.is_pending(1));
        assert!(!cache.answered_unknown(1), "the unknown is gone");
        assert!(cache.lookup(1, || asks += 1).is_none());
        assert_eq!(asks, 2, "never while one is pending");
    }

    /// `0x556ec9` → `0x556fcf`: a refusal for a key with no node does nothing.
    #[test]
    fn a_refusal_with_nothing_pending_marks_nothing() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        assert_eq!(cache.refuse(5), Refused::Nothing);
        assert!(!cache.answered_unknown(5));
        assert!(!cache.answered(5));
        assert_eq!(cache.generation(), 0, "no callback ran");
        let mut asks = 0;
        cache.lookup_new(5, || asks += 1);
        assert_eq!(asks, 1);
    }

    #[test]
    fn a_refusal_is_never_a_persisted_record() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.insert(1, "one");
        cache.lookup_new(2, || {});
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
        assert_eq!(cache.refuse(4), Refused::Record);
        assert!(cache.get(4).is_none());
        assert!(cache.answered_unknown(4));
    }

    #[test]
    fn a_landing_after_a_refusal_answers_it() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.lookup_new(6, || {});
        cache.refuse(6);
        cache.insert(6, "six");
        assert!(!cache.answered_unknown(6));
        assert_eq!(cache.get(6), Some(&"six"));
    }

    /// A reader's runs between its events never ask a refused key, however many there are; its
    /// next event does, once.
    #[test]
    fn a_refusal_waits_for_the_next_event_whatever_the_runs_between() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        cache.lookup_new(9, || asks += 1);
        assert_eq!(asks, 1, "a key the cache holds nothing for is asked");
        cache.refuse(9);
        for _ in 0..600 {
            assert!(cache.lookup_new(9, || asks += 1).is_none());
        }
        assert_eq!(asks, 1, "no run between events asks the refusal");
        assert!(cache.answered_unknown(9));
        cache.lookup(9, || asks += 1);
        assert_eq!(asks, 2, "the event asks it at once");
        for _ in 0..600 {
            cache.lookup_new(9, || asks += 1);
        }
        assert_eq!(asks, 2, "and the runs after it find it pending");
    }

    /// [`Ask`] picks the lookup: no callback, a run between events, an event.
    #[test]
    fn a_read_does_what_its_ask_says() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let send = |c: &NetCommands| {
            let _ =
                c.0.send(crate::net::ClientCommand::ItemQuery { entry: 4, guid: 0 });
        };
        assert!(cache.read(4, Ask::Read, send).is_none());
        assert_eq!(rx.try_iter().count(), 0, "a read never asks");
        cache.read(4, Ask::New(&commands), send);
        assert_eq!(rx.try_iter().count(), 1, "a new key is asked");
        cache.refuse(4);
        cache.read(4, Ask::at(false, &commands), send);
        assert_eq!(rx.try_iter().count(), 0, "a refusal stands between events");
        cache.read(4, Ask::at(true, &commands), send);
        assert_eq!(rx.try_iter().count(), 1, "an event asks it again");
        cache.insert(4, "four");
        assert_eq!(cache.read(4, Ask::Read, send), Some(&"four"));
    }

    #[test]
    fn clearing_pending_lets_a_dropped_ask_go_again_and_keeps_the_answers() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        let mut asks = 0;
        cache.lookup_new(1, || asks += 1);
        cache.insert(2, "two");
        cache.clear_pending();
        cache.lookup_new(1, || asks += 1);
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
        assert!(cache.lookup_new(3, || asks += 1).is_none());
        assert_eq!(asks, 1);
    }

    #[test]
    fn evicting_a_refusal_drops_its_unknown() {
        let mut cache: QueryCache<u32, &str> = QueryCache::default();
        cache.lookup_new(8, || {});
        cache.refuse(8);
        assert!(cache.evict(8));
        assert!(!cache.answered_unknown(8));
    }
}
