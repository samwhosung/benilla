//! The item packet handlers: `SMSG_INVENTORY_CHANGE_FAILURE` onto the error line, the pending
//! item locks and the loot latch, and `SMSG_OPEN_CONTAINER` onto the `BAG_OPEN` queue.

use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{EquipError, EquipErrors, BAG_SLOT_FIRST, BANK_BAG_ID_FIRST};
use crate::net::{NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_loot::LootLatch;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::InventoryFailure, on_inventory_failure)
        .net_handler(SessionEventKind::OpenContainer, on_open_container)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The item locks and the queued unlocks die with the session: the reference's lock is item-object
/// state (`item+0x314`), and the objects are rebuilt at every world entry.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
) {
    pending.clear_session();
    lock_cleared.0.clear();
}

/// Container ids from `SMSG_OPEN_CONTAINER`, for [`super::feed::feed_containers`] to fire as
/// `BAG_OPEN(id)`: this handler has no `UiScript`.
#[derive(Resource, Default)]
pub(crate) struct BagOpens(pub(crate) Vec<i64>);

fn on_open_container(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut opens: ResMut<BagOpens>,
) {
    if let SessionEvent::OpenContainer { item } = ev {
        let store = self_q.single().ok().map(|s| &s.0);
        match container_of(self_guid.0, store, item) {
            Some(id) => opens.0.push(id),
            None => debug!("open container {item:#x}: not one of our bags — no BAG_OPEN"),
        }
    }
}

/// The reference's `BAG_OPEN` raiser (`0x4f9410`): our own guid is the backpack (0), an equipped
/// bag (slots 19..22) is 1..4, a bank bag 5..10, and any other guid fires nothing. vmangos sends
/// the packet when an auto-equip puts a bag in a bag slot.
pub(super) fn container_of(
    self_guid: Option<u64>,
    store: Option<&ObjectFields>,
    item: u64,
) -> Option<i64> {
    if self_guid.is_some_and(|me| me == item) {
        return Some(0);
    }
    let store = store?;
    (0..4u8)
        .find(|i| store.player_inv_slot(BAG_SLOT_FIRST + i) == Some(item))
        .map(|i| i64::from(i) + 1)
        .or_else(|| {
            (0..6u8)
                .find(|j| store.player_bank_bag_slot(*j) == Some(item))
                .map(|j| BANK_BAG_ID_FIRST + i64::from(j))
        })
}

fn on_inventory_failure(
    In(ev): In<SessionEvent>,
    mut equip_errors: ResMut<EquipErrors>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
    mut latch: ResMut<LootLatch>,
) {
    if let SessionEvent::InventoryFailure {
        reason,
        required_level,
        item_guid,
        bag_slot,
    } = ev
    {
        inventory_failure(
            reason,
            required_level,
            item_guid,
            bag_slot,
            &mut equip_errors,
            &mut pending,
            &mut lock_cleared,
            &mut latch,
        );
    }
}

/// A refused inventory operation (reason 0 never arrives: `benilla_protocol::events` drops it):
/// queues the error line and clears the item's pending lock, the unlocked slots waiting in
/// [`LockTransitions`] for [`super::feed::feed_containers`], since this site has no `UiScript`.
fn inventory_failure(
    reason: u8,
    required_level: Option<u32>,
    item_guid: u64,
    bag_slot: u8,
    equip_errors: &mut EquipErrors,
    pending: &mut PendingItemOps,
    lock_cleared: &mut LockTransitions,
    latch: &mut LootLatch,
) {
    debug!("net: inventory failure {reason:#04x} (item {item_guid:#x}, bag slot {bag_slot})");
    equip_errors.0.push(EquipError {
        reason,
        required_level,
        bag_slot,
    });
    lock_cleared.0.extend(pending.clear_by_failure(item_guid));
    // A failure whose first item guid is the looted item (a lockbox) ends the loot (`0x5e3a84`).
    if item_guid != 0 {
        latch.clear_for(item_guid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inventory_failure_on_the_looted_object_clears_the_latch() {
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;
        let mut errs = EquipErrors::default();
        let mut pending = PendingItemOps::default();
        let mut cleared = LockTransitions::default();
        let mut latch = LootLatch(Some(LOCKBOX));

        // An unrelated item's failure must not end the session.
        inventory_failure(
            2,
            None,
            0x1234,
            0,
            &mut errs,
            &mut pending,
            &mut cleared,
            &mut latch,
        );
        assert_eq!(latch.0, Some(LOCKBOX));

        inventory_failure(
            2,
            None,
            LOCKBOX,
            0,
            &mut errs,
            &mut pending,
            &mut cleared,
            &mut latch,
        );
        assert_eq!(latch.0, None);
    }
    /// Driven through the real registration.
    #[test]
    fn the_session_end_drops_every_item_lock() {
        let mut app = App::new();
        app.init_resource::<EquipErrors>()
            .init_resource::<PendingItemOps>()
            .init_resource::<LockTransitions>()
            .init_resource::<BagOpens>()
            .init_resource::<LootLatch>()
            .init_resource::<SelfGuid>();
        register(&mut app);
        app.world_mut()
            .resource_mut::<PendingItemOps>()
            .add([(0, 3, 0x4000_0000_0000_0007, 1)]);
        app.world_mut()
            .resource_mut::<LockTransitions>()
            .0
            .push((0, 5));
        let epoch = app.world().resource::<PendingItemOps>().epoch();

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let pending = app.world().resource::<PendingItemOps>();
        assert!(pending.is_empty() && !pending.contains(0, 3));
        assert_ne!(pending.epoch(), epoch, "the feed's gate sees the set move");
        assert!(
            app.world().resource::<LockTransitions>().0.is_empty(),
            "no ITEM_LOCK_CHANGED for the old session's slots"
        );
    }
}

#[cfg(test)]
mod open_container_tests {
    use super::*;
    use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;

    /// `PLAYER_FIELD_BANK_BAG_SLOT_1`, field 612 (private to the fields module).
    const BANK_BAG_SLOT_1: u16 = 612;

    /// A self descriptor holding the given guids at the given first-dword indices.
    fn store(guids: &[(u16, u64)]) -> ObjectFields {
        let pairs: Vec<(u16, u32)> = guids
            .iter()
            .flat_map(|(i, g)| [(*i, *g as u32), (*i + 1, (*g >> 32) as u32)])
            .collect();
        ObjectFields::from_pairs(&pairs)
    }

    #[test]
    fn the_bag_open_raiser_resolves_the_reference_container_ids() {
        let me = 0x0000_0000_0000_0042;
        let bag_in_slot_20 = 0x4000_0000_0000_0b02; // the second equipped bag → container 2
        let bank_bag_3 = 0x4000_0000_0000_0b44; // the fourth bank bag → container 5 + 3
        let s = store(&[
            (FIELD_PLAYER_INV_SLOT_HEAD + 2 * 20, bag_in_slot_20),
            (BANK_BAG_SLOT_1 + 2 * 3, bank_bag_3),
        ]);
        assert_eq!(
            container_of(Some(me), Some(&s), me),
            Some(0),
            "our own guid is the backpack"
        );
        assert_eq!(container_of(Some(me), Some(&s), bag_in_slot_20), Some(2));
        assert_eq!(container_of(Some(me), Some(&s), bank_bag_3), Some(8));
        assert_eq!(
            container_of(Some(me), Some(&s), 0xdead),
            None,
            "a stranger's guid fires nothing"
        );
        assert_eq!(
            container_of(None, None, me),
            None,
            "no self, no store: nothing to match"
        );
    }
}
