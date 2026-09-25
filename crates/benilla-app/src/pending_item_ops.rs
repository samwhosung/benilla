//! The client-side pending-operation lock (byte-verified by decision 0218 §3 —
//! `item+0x314` bit0: "the send locks both ends", cleared by the resolving field update or a
//! non-zero `SMSG_INVENTORY_CHANGE_FAILURE"). [`PendingItemOps`] is a Resource wrapping plain
//! bookkeeping — every method is engine-free (no ECS/Bevy types past the `#[derive(Resource)]`
//! marker), so the whole lifecycle is unit-tested below without a `World`. The app's per-frame
//! container feed (`ui_items::feed_containers`) reads it into each pushed
//! `ContainerSlot::locked`, and the move/split/destroy drains (`ui_items::drain_container_moves`/
//! `drain_container_destroys`) are the only writers of [`PendingItemOps::add`].
//!
//! **Baseline is `(guid, stack count)`, not guid alone** — a deliberate widening of 0218's literal
//! "the resolving field-update watcher" (`0x5ddcf0`, cited generically over "the inventory-slot
//! field update", not specifically the guid field). Guid-only tracking has a real stuck-lock gap:
//! a partial split-merge (`SplitContainerItem` onto an existing same-item stack — already a live,
//! tested path, `container.rs::pickup_place_onto_same_item_merges_and_clears`) and a partial
//! destroy (`DeleteCursorItem` off a split carry) both settle by changing a slot's **stack count**
//! while its guid stays exactly the same (the source/destination item is the same object,
//! narrower/wider). Watching count too closes that gap without weakening anything guid-only
//! tracking already caught.

use bevy::prelude::Resource;

/// One outstanding op's slot set: live-API `(bag, slot)` → the `(item guid, stack count)` that sat
/// there when the op was sent — named so clippy doesn't read the nested tuple as "very complex"
/// (the `PendingItemOps` struct doc explains the shape).
type PendingEntry = Vec<((i64, u32), (u64, u32))>;

/// The client-side pending-operation lock. Each outstanding op records the live-API `(bag, slot)`
/// positions it touches, paired with the `(item guid, stack count)` that sat there when the op was
/// **sent** (`(0, 0)` = the slot was empty then — a split placed onto an empty destination, say).
/// A move/split covers both its source and destination ("a send locks both ends"); a destroy
/// covers only the one slot it touches (there is no displaced item).
#[derive(Debug, Default, Resource)]
pub(crate) struct PendingItemOps {
    entries: Vec<PendingEntry>,
    /// One step per change to [`Self::entries`] — the gate input a feed needs to notice that a
    /// lock **cleared**. `!is_empty()` alone cannot: the frame an op resolves, the set goes empty
    /// and every "is anything in flight?" gate closes, so a feed that had pushed `locked: true`
    /// never runs again to correct it. That is the stuck-dark bank bag button of 1771.
    epoch: u64,
}

impl PendingItemOps {
    /// Record one outstanding op covering `slots` — `(bag, slot, guid_at_send_time,
    /// count_at_send_time)` quadruples. Called once per outbound move/split/destroy send,
    /// covering every slot that send touches.
    pub(crate) fn add(&mut self, slots: impl IntoIterator<Item = (i64, u32, u64, u32)>) {
        self.epoch += 1;
        self.entries.push(
            slots
                .into_iter()
                .map(|(bag, slot, guid, count)| ((bag, slot), (guid, count)))
                .collect(),
        );
    }

    /// No outstanding ops at all — the container feed's gate reads this: while
    /// anything is in flight the resolving walk must run every frame, and once nothing is, the
    /// last resolve's own change tick already covered the final unlock.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// [`Self::epoch`] — the "did the lock set move?" counter a feed's gate watches beside
    /// `!is_empty()`.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether `(bag, slot)` is covered by any outstanding op — the container feed reads this
    /// building each pushed `ContainerSlot::locked`.
    pub(crate) fn contains(&self, bag: i64, slot: u32) -> bool {
        self.entries
            .iter()
            .any(|e| e.iter().any(|&(pos, _)| pos == (bag, slot)))
    }

    /// The resolving clear ("the field-update watcher"): an entry clears the moment ANY of
    /// its slots' CURRENT `(guid, count)` (`current` — the same descriptor walk the container feed
    /// already does) differs from what was recorded at send time (see the module doc for why count
    /// joins guid). The WHOLE entry clears together (every slot it covered unlocks at once) rather
    /// than slot-by-slot: a swap's two mutations land in the same field-update batch in practice,
    /// so waiting for either is the simplest correct read. Returns the deduplicated `(bag, slot)`
    /// pairs that just unlocked.
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

    /// `SMSG_INVENTORY_CHANGE_FAILURE` (reason ≠ 0 always — reason 0 never reaches
    /// `benilla_protocol::SessionEvent::InventoryFailure` in the first place, filtered at
    /// `benilla_protocol::events`'s `if reason != 0` guard, matching 0218's "reason-0 clears
    /// nothing"): clear every entry naming `item_guid`, or — when the guid is 0 or matches no
    /// outstanding entry — clear EVERYTHING. INTERIM: the failure event names one item, not the
    /// operation, so a guid this bookkeeping never recorded (or the ack's own 0) can't be
    /// attributed to a specific entry; moves are serial in practice, so a blanket clear is the
    /// safe over-approximation rather than a slot stuck dark forever. Returns the deduplicated
    /// `(bag, slot)` pairs that unlocked.
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

    /// **`UnlockItem 0x495420`** — clear every entry naming `item_guid`, and nothing else: a guid
    /// no entry recorded (a corpse, a chest, 0) unlocks nothing, exactly as the reference's
    /// resolve-as-ITEM (typemask 2) finds nothing to clear for a non-item. The loot close is the
    /// caller that needs it: an opened lockbox/clam is locked at the `CMSG_OPEN_ITEM` send
    /// and, closed with loot left, never changes its slot — so neither [`Self::resolve`] nor a
    /// failure ever clears it (`0x48f200` @ `48f299`). Returns the
    /// deduplicated `(bag, slot)` pairs that unlocked.
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

    /// The session end: drop every outstanding entry and report nothing. The lock is item-object
    /// state in the reference (`item+0x314`), and those objects do not outlive the session; an op
    /// still in flight when the socket died never gets the field update or failure that would
    /// settle it, so without this its slots stayed locked for the whole next session. The epoch
    /// steps so a feed that pushed `locked: true` runs once more to correct it.
    pub(crate) fn clear_session(&mut self) {
        self.entries.clear();
        self.epoch += 1;
    }

    /// Drop every outstanding entry, unconditionally — [`Self::clear_by_failure`]'s
    /// unmatched/zero fallback.
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

/// `(bag, slot)` pairs whose app-lock cleared this frame, from ANY clear: the server failure
/// (`ui_items::net::inventory_failure`, which owns the wire event but has no `UiScript` to
/// fire `ITEM_LOCK_CHANGED` through), the loot close's item unlock (`ui_loot::net`, the same
/// shape) and the resolving field-update watch
/// (`ui_items::feed::resolve_item_locks`, which runs ahead of every feed so that no feed can push
/// a `locked` the clear has already invalidated). `feed_containers` drains this and fires — the
/// exact shape `ui_items::EquipErrors` already uses for the same reason.
///
/// **Why the resolve is not just done inside the feed that fires** (1771): `feed_char` owns the
/// doll's and the bank bags' `locked`, and is ordered `.before(feed_containers)` for an unrelated
/// reason (Bagnon's `BAG_UPDATE` handlers read the inventory surface). With the resolve living in
/// `feed_containers`, `feed_char` read a lock set that was about to be cleared *later in the same
/// frame*, pushed `locked: true`, and then never ran again — leaving a bank bag button greyed
/// until the window was reopened.
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

        // The destination's guid now differs (the swap/split landed) — the WHOLE entry clears,
        // both slots unlock together even though the source's baseline didn't move this check.
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

    /// The gap guid-only tracking would miss (the module doc): a partial split-merge leaves BOTH
    /// ends' guids unchanged — only their stack counts move. Count must join guid in the baseline
    /// or this op would stay locked forever.
    #[test]
    fn resolve_catches_a_same_guid_stack_count_change() {
        let mut p = PendingItemOps::default();
        // Source: item 100, was a 5-stack; destination: item 200 (same item id, different guid —
        // impossible in the real client, but the bookkeeping only ever compares within one slot),
        // was a 3-stack. A partial split-merge leaves both guids exactly where they were.
        p.add([(0, 1, 100, 5), (0, 7, 200, 3)]);
        assert!(p
            .resolve(|bag, slot| if (bag, slot) == (0, 1) {
                (100, 5)
            } else {
                (200, 3)
            })
            .is_empty());

        // The merge landed: source dropped to a 2-stack (guid unchanged), destination rose to 6
        // (guid unchanged) — a guid-only comparison would see no change at all.
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

    /// Unlike the failure clear, the guid clear has no clear-all fallback: an unmatched or zero
    /// guid (a corpse's release) must leave every other lock standing.
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
