//! The item packet handlers: `SMSG_INVENTORY_CHANGE_FAILURE` onto the error line, the item
//! unlocks and the loot latch, and `SMSG_OPEN_CONTAINER` onto the `BAG_OPEN` queue.

use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{EquipError, EquipErrors, BAG_SLOT_FIRST, BANK_BAG_ID_FIRST};
use crate::net::{NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::ui_items::locks::ItemReleases;
use crate::ui_loot::LootLatch;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::InventoryFailure, on_inventory_failure)
        .net_handler(SessionEventKind::OpenContainer, on_open_container)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The queued unlocks die with the session, never to fire into a later VM: the reference's lock
/// is item-object state (`item+0x314`), and the objects are rebuilt at every world entry. The
/// refusals queued with theirs go too, lines and all: they were the ended world's.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut releases: ResMut<ItemReleases>,
    mut equip_errors: ResMut<EquipErrors>,
    held: Option<ResMut<super::locks::HeldItemLocks>>,
) {
    releases.0.clear();
    equip_errors.0.clear();
    if let Some(mut held) = held {
        *held = Default::default();
    }
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
    mut latch: ResMut<LootLatch>,
) {
    if let SessionEvent::InventoryFailure {
        reason,
        required_level,
        item_guid,
        item2_guid,
        bag_slot,
    } = ev
    {
        let failure = Failure {
            reason,
            required_level,
            items: [item_guid, item2_guid],
            bag_slot,
        };
        inventory_failure(failure, &mut equip_errors, &mut latch);
    }
}

/// One `SMSG_INVENTORY_CHANGE_FAILURE`, past the reason-0 filter.
struct Failure {
    reason: u8,
    required_level: Option<u32>,
    /// The two item guids the packet names.
    items: [u64; 2],
    bag_slot: u8,
}

/// A refused inventory operation (reason 0 never arrives: `benilla_protocol::events` drops it):
/// queues the error line, and after it the unlock of both guids the packet names (`0x5e3ac0`,
/// the second only when nonzero, `0x5e3ad3`), which the container feed runs since this site has
/// no `UiScript`.
fn inventory_failure(failure: Failure, equip_errors: &mut EquipErrors, latch: &mut LootLatch) {
    let Failure {
        reason,
        required_level,
        items: [item_guid, item2_guid],
        bag_slot,
    } = failure;
    debug!(
        "net: inventory failure {reason:#04x} (items {item_guid:#x} {item2_guid:#x}, bag slot \
         {bag_slot})"
    );
    // The line shows first (`0x496720`), then both unlock (the feed runs them after it).
    equip_errors.0.push(EquipError {
        reason,
        required_level,
        bag_slot,
        unlocks: [item_guid, item2_guid],
    });
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
        let mut latch = LootLatch(Some(LOCKBOX));
        let failure = |first: u64| Failure {
            reason: 2,
            required_level: None,
            items: [first, 0],
            bag_slot: 0,
        };

        // An unrelated item's failure must not end the session.
        inventory_failure(failure(0x1234), &mut errs, &mut latch);
        assert_eq!(latch.0, Some(LOCKBOX));

        inventory_failure(failure(LOCKBOX), &mut errs, &mut latch);
        assert_eq!(latch.0, None);
    }

    /// A refusal shows its line first (`0x496720`), then unlocks both items it names (`0x5e3ac0`,
    /// `0x5e3ad3`); driven through the real handler and the container feed.
    #[test]
    fn an_inventory_failure_shows_its_line_then_unlocks_both_items() {
        use bevy::ecs::system::RunSystemOnce;
        const A: u64 = 0x4000_0000_0000_00a1;
        const B: u64 = 0x4000_0000_0000_00b2;
        const REASON: u8 = 0x16;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        let key = super::super::equip_error::equip_error_key(REASON);
        script
            .run(&format!(
                "{key} = 'Refused.' SEEN = {{}} local f = CreateFrame('Frame') \
                 f:RegisterEvent('UI_ERROR_MESSAGE') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)"
            ))
            .unwrap();
        script.lock_item(A);
        script.lock_item(B);
        script.run("SEEN = {}").unwrap();
        app.insert_non_send_resource(script);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::InventoryFailure {
                reason: REASON,
                required_level: None,
                item_guid: A,
                item2_guid: B,
                bag_slot: 0xff,
            }],
        );
        app.world_mut()
            .run_system_once(super::super::feed::feed_containers)
            .unwrap();
        let s = app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>();
        assert_eq!(
            s.eval::<Vec<String>>("return SEEN").unwrap(),
            ["UI_ERROR_MESSAGE", "ITEM_LOCK_CHANGED", "ITEM_LOCK_CHANGED"]
        );
        assert!(!s.item_locked(A) && !s.item_locked(B));
    }

    /// A refusal that lands while no VM is up keeps its line for the next one but drops its
    /// unlocks, which would otherwise fire into that VM's locks.
    #[test]
    fn a_refusal_with_no_vm_drops_its_unlocks_and_keeps_its_line() {
        use bevy::ecs::system::RunSystemOnce;
        const A: u64 = 0x4000_0000_0000_00a1;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        app.world_mut()
            .resource_mut::<EquipErrors>()
            .0
            .push(EquipError {
                reason: 0x16,
                required_level: None,
                bag_slot: 0xff,
                unlocks: [A, 0],
            });
        app.world_mut()
            .run_system_once(super::super::feed::feed_containers)
            .unwrap();
        let errs = &app.world().resource::<EquipErrors>().0;
        assert_eq!(errs.len(), 1, "the line waits");
        assert_eq!(errs[0].unlocks, [0, 0]);
    }

    /// Driven through the real registration.
    #[test]
    fn the_session_end_drops_every_queued_unlock() {
        let mut app = App::new();
        app.init_resource::<EquipErrors>()
            .init_resource::<ItemReleases>()
            .init_resource::<BagOpens>()
            .init_resource::<LootLatch>()
            .init_resource::<SelfGuid>();
        register(&mut app);
        app.world_mut()
            .resource_mut::<ItemReleases>()
            .unlock(0x4000_0000_0000_0007);
        app.world_mut()
            .resource_mut::<EquipErrors>()
            .0
            .push(EquipError {
                reason: 0x16,
                required_level: None,
                bag_slot: 0xff,
                unlocks: [0x4000_0000_0000_0008, 0],
            });

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        assert!(
            app.world().resource::<ItemReleases>().0.is_empty(),
            "no ITEM_LOCK_CHANGED for the old session's items"
        );
        assert!(
            app.world().resource::<EquipErrors>().0.is_empty(),
            "nor a refusal's line or unlocks"
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
