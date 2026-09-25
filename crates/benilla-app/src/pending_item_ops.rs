//! The client-side pending-operation lock, the reference's `item+0x314` bit 0: a send locks both
//! ends, and the resolving field update or a nonzero `SMSG_INVENTORY_CHANGE_FAILURE` clears it.
//! Engine-free bookkeeping; `ui_items::feed_containers` reads it into `ContainerSlot::locked`.
//!
//! The reference keys the lock on the item and its watcher (`0x5ddcf0`) clears on a slot's guid
//! change alone; here the baseline is `(guid, stack count)` per slot, since a partial split-merge
//! or a partial destroy settles by changing a slot's count while its guid stays the same.

use bevy::prelude::Resource;

/// One outstanding op: `(bag, slot)` to the `(item guid, stack count)` there at send time.
type PendingEntry = Vec<((i64, u32), (u64, u32))>;

/// The pending-operation lock: per op, the live-API `(bag, slot)`s it touches with their
/// `(item guid, stack count)` at send time (`(0, 0)` for an empty slot). A move or split covers
/// both ends, a destroy its one slot.
#[derive(Debug, Default, Resource)]
pub(crate) struct PendingItemOps {
    entries: Vec<PendingEntry>,
    /// Steps on every change to [`Self::entries`], so a feed notices a lock clearing; the frame
    /// the set empties, an `!is_empty()` gate alone would never run again to unlock.
    epoch: u64,
}

impl PendingItemOps {
    /// Records one outstanding op over `(bag, slot, guid at send, count at send)` quadruples,
    /// once per move, split or destroy send.
    pub(crate) fn add(&mut self, slots: impl IntoIterator<Item = (i64, u32, u64, u32)>) {
        self.epoch += 1;
        self.entries.push(
            slots
                .into_iter()
                .map(|(bag, slot, guid, count)| ((bag, slot), (guid, count)))
                .collect(),
        );
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(crate) fn contains(&self, bag: i64, slot: u32) -> bool {
        self.entries
            .iter()
            .any(|e| e.iter().any(|&(pos, _)| pos == (bag, slot)))
    }

    /// The resolving clear (the field-update watcher, `0x5ddcf0`): a whole entry clears once any
    /// of its slots' current `(guid, count)` differs from its baseline. Returns the unlocked slots.
    pub(crate) fn resolve(&mut self, current: impl Fn(i64, u32) -> (u64, u32)) -> Vec<(i64, u32)> {
        let mut unlocked = Vec::new();
        self.entries.retain(|entry| {
            let settled = entry
                .iter()
                .any(|&((bag, slot), baseline)| current(bag, slot) != baseline);
            if settled {
                unlocked.extend(entry.iter().map(|&(pos, _)| pos));
            }
            !settled
        });
        unlocked.sort_unstable();
        unlocked.dedup();
        if !unlocked.is_empty() {
            self.epoch += 1;
        }
        unlocked
    }

    /// A nonzero-reason `SMSG_INVENTORY_CHANGE_FAILURE` (reason 0 is filtered upstream and clears
    /// nothing): clears the entries naming `item_guid`, or every entry when the guid is 0 or
    /// unrecorded, since the failure names an item, not the operation. The reference unlocks both
    /// guids the packet carries (`0x5e3ac0`, `0x5e3ad3`). Returns the unlocked slots.
    pub(crate) fn clear_by_failure(&mut self, item_guid: u64) -> Vec<(i64, u32)> {
        let matched = item_guid != 0
            && self
                .entries
                .iter()
                .any(|e| e.iter().any(|&(_, (guid, _))| guid == item_guid));
        if !matched {
            return self.clear_all();
        }
        self.clear_by_guid(item_guid)
    }

    /// `UnlockItem` (`0x495420`): clears the entries naming `item_guid`; a non-item guid or 0
    /// clears nothing, as the reference's resolve-as-item finds nothing. The loot close calls it
    /// (`0x48f200` at `0x48f299`): an opened lockbox closed with loot left never changes its slot.
    /// Returns the unlocked slots.
    pub(crate) fn clear_by_guid(&mut self, item_guid: u64) -> Vec<(i64, u32)> {
        let mut unlocked = Vec::new();
        if item_guid == 0 {
            return unlocked;
        }
        self.entries.retain(|entry| {
            let hit = entry.iter().any(|&(_, (guid, _))| guid == item_guid);
            if hit {
                unlocked.extend(entry.iter().map(|&(pos, _)| pos));
            }
            !hit
        });
        unlocked.sort_unstable();
        unlocked.dedup();
        if !unlocked.is_empty() {
            self.epoch += 1;
        }
        unlocked
    }

    /// The session end: drops every entry, as the reference's item objects (`item+0x314`) do not
    /// outlive the session. The epoch steps so a feed corrects a pushed `locked: true`.
    pub(crate) fn clear_session(&mut self) {
        self.entries.clear();
        self.epoch += 1;
    }

    /// Drops every entry: [`Self::clear_by_failure`]'s fallback.
    fn clear_all(&mut self) -> Vec<(i64, u32)> {
        let mut unlocked: Vec<(i64, u32)> = self
            .entries
            .drain(..)
            .flat_map(|e| e.into_iter().map(|(pos, _)| pos))
            .collect();
        unlocked.sort_unstable();
        unlocked.dedup();
        if !unlocked.is_empty() {
            self.epoch += 1;
        }
        unlocked
    }
}

/// Slots whose lock cleared this frame, from any clear; `feed_containers` drains it and fires
/// `ITEM_LOCK_CHANGED`. `ui_items::feed::resolve_item_locks` must run ahead of every feed, since
/// `feed_char` runs before `feed_containers` and would push a lock cleared later that frame.
#[derive(Resource, Default)]
pub(crate) struct LockTransitions(pub Vec<(i64, u32)>);

#[cfg(test)]
mod tests {
    use super::PendingItemOps;

    #[test]
    fn add_then_contains_both_ends() {
        let mut p = PendingItemOps::default();
        assert!(!p.contains(0, 1));
        p.add([(0, 1, 100, 5), (0, 5, 0, 0)]);
        assert!(p.contains(0, 1), "the source end");
        assert!(
            p.contains(0, 5),
            "the destination end (0218: the send locks both)"
        );
        assert!(!p.contains(1, 1), "a different bag's slot is untouched");
    }

    #[test]
    fn resolve_clears_the_whole_entry_when_either_slots_guid_moves() {
        let mut p = PendingItemOps::default();
        p.add([(0, 1, 100, 5), (0, 5, 0, 0)]);
        // Neither slot has changed yet: still locked, nothing resolves.
        assert!(p
            .resolve(|bag, slot| if (bag, slot) == (0, 1) {
                (100, 5)
            } else {
                (0, 0)
            })
            .is_empty());
        assert!(p.contains(0, 1) && p.contains(0, 5));

        // The destination changed: both slots unlock together.
        let unlocked = p.resolve(|bag, slot| {
            if (bag, slot) == (0, 1) {
                (100, 5)
            } else {
                (42, 1)
            }
        });
        assert_eq!(unlocked, vec![(0, 1), (0, 5)]);
        assert!(!p.contains(0, 1) && !p.contains(0, 5));
    }

    #[test]
    fn resolve_catches_a_same_guid_stack_count_change() {
        let mut p = PendingItemOps::default();
        // Source item 100 was a 5-stack, destination item 200 a 3-stack.
        p.add([(0, 1, 100, 5), (0, 7, 200, 3)]);
        assert!(p
            .resolve(|bag, slot| if (bag, slot) == (0, 1) {
                (100, 5)
            } else {
                (200, 3)
            })
            .is_empty());

        // The merge landed: 2 and 6, both guids unchanged.
        let unlocked = p.resolve(|bag, slot| {
            if (bag, slot) == (0, 1) {
                (100, 2)
            } else {
                (200, 6)
            }
        });
        assert_eq!(unlocked, vec![(0, 1), (0, 7)]);
    }

    #[test]
    fn clear_by_failure_targets_the_named_guid_only() {
        let mut p = PendingItemOps::default();
        p.add([(0, 1, 100, 5), (0, 5, 0, 0)]); // op A
        p.add([(1, 3, 200, 1)]); // op B, unrelated

        let unlocked = p.clear_by_failure(100);
        assert_eq!(unlocked, vec![(0, 1), (0, 5)]);
        assert!(!p.contains(0, 1) && !p.contains(0, 5), "op A cleared");
        assert!(p.contains(1, 3), "op B untouched — a different guid");
    }

    #[test]
    fn clear_by_guid_clears_only_the_named_item() {
        let mut p = PendingItemOps::default();
        p.add([(0, 3, 100, 1)]); // the opened lockbox
        p.add([(1, 2, 200, 1)]); // an unrelated move

        assert!(p.clear_by_guid(999).is_empty(), "an unrecorded guid");
        assert!(p.clear_by_guid(0).is_empty(), "the zero guid fires nothing");
        assert!(p.contains(0, 3) && p.contains(1, 2));

        let epoch = p.epoch();
        assert_eq!(p.clear_by_guid(100), vec![(0, 3)]);
        assert!(!p.contains(0, 3) && p.contains(1, 2));
        assert!(p.epoch() > epoch, "the feed's gate sees the lock set move");
    }

    #[test]
    fn clear_by_failure_clears_everything_on_zero_or_unmatched_guid() {
        let mut p = PendingItemOps::default();
        p.add([(0, 1, 100, 1)]);
        p.add([(1, 3, 200, 1)]);

        // guid 0 (unattributed): clear-all.
        let unlocked = p.clear_by_failure(0);
        assert_eq!(unlocked, vec![(0, 1), (1, 3)]);
        assert!(!p.contains(0, 1) && !p.contains(1, 3));

        // An unmatched guid (recorded nowhere): same fallback.
        p.add([(0, 1, 100, 1)]);
        let unlocked = p.clear_by_failure(999);
        assert_eq!(unlocked, vec![(0, 1)]);
    }
}
