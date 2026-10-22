//! The item packet handlers: `SMSG_INVENTORY_CHANGE_FAILURE` onto the refusal lane, the item
//! unlocks and the loot latch, and `SMSG_OPEN_CONTAINER`'s `BAG_OPEN` onto the item lane.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{EquipError, EquipErrors};
use crate::net::{NetHandlerApp, SelfGuid};
use crate::ui_loot::LootLatch;

pub(super) fn register(app: &mut App) {
    app.net_handler(SessionEventKind::InventoryFailure, on_inventory_failure)
        .net_handler(SessionEventKind::OpenContainer, on_open_container)
        .net_handler(SessionEventKind::Disconnected, on_session_end);
}

/// The queued refusals die with the session, lines and unlocks, never to fire into a later VM:
/// the reference's lock is item-object state (`item+0x314`), and the objects are rebuilt at every
/// world entry.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut equip_errors: ResMut<EquipErrors>,
    held: Option<ResMut<super::locks::HeldItemLocks>>,
) {
    equip_errors.clear();
    if let Some(mut held) = held {
        *held = Default::default();
    }
}

/// `SMSG_OPEN_CONTAINER`'s handler (`0x5e3b35`): `0x4f9410` with the guid (`0x5e3b49`), whose
/// `BAG_OPEN`s ride the item lane at the packet's place. vmangos sends the packet when an
/// auto-equip puts a bag in a bag slot.
fn on_open_container(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    bags: Res<crate::item_watch::BagCache>,
    mut lane: ResMut<crate::item_lane::ItemLane>,
) {
    if let SessionEvent::OpenContainer { item } = ev {
        let opens = bags.opens(self_guid.0, item);
        if opens.is_empty() {
            debug!("open container {item:#x}: not one of our bags — no BAG_OPEN");
        }
        lane.push(crate::item_watch::Run {
            bags: opens
                .into_iter()
                .map(crate::item_watch::BagEvent::Open)
                .collect(),
            ..Default::default()
        });
    }
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
/// the second only when nonzero, `0x5e3ad3`), on the refusal lane at the packet's place.
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
    // The line shows first (`0x496720`), then both unlock.
    equip_errors.push(EquipError {
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
    /// `0x5e3ad3`); driven through the real handler and the lanes' drain.
    #[test]
    fn an_inventory_failure_shows_its_line_then_unlocks_both_items() {
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
        crate::lanes::fire_in_order(app.world_mut());
        let s = app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>();
        assert_eq!(
            s.eval::<Vec<String>>("return SEEN").unwrap(),
            ["UI_ERROR_MESSAGE", "ITEM_LOCK_CHANGED", "ITEM_LOCK_CHANGED"]
        );
        assert!(!s.item_locked(A) && !s.item_locked(B));
    }

    /// A refusal that lands while no in-game interface is up is dropped by the frame, line and
    /// unlocks, never to fire into a later VM's locks.
    #[test]
    fn a_refusal_with_no_interface_is_dropped_by_the_frame() {
        let mut app = App::new();
        super::super::equip_error::add_equip_error_lane(&mut app);
        app.world_mut()
            .resource_mut::<EquipErrors>()
            .push(EquipError {
                reason: 0x16,
                required_level: None,
                bag_slot: 0xff,
                unlocks: [0x4000_0000_0000_00a1, 0],
            });
        app.update();
        assert!(app.world().resource::<EquipErrors>().queued().is_empty());
    }

    /// Driven through the real registration.
    #[test]
    fn the_session_end_drops_every_queued_unlock() {
        let mut app = App::new();
        app.init_resource::<EquipErrors>()
            .init_resource::<crate::item_lane::ItemLane>()
            .init_resource::<crate::item_watch::BagCache>()
            .init_resource::<LootLatch>()
            .init_resource::<SelfGuid>();
        register(&mut app);
        app.world_mut()
            .resource_mut::<EquipErrors>()
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
            app.world().resource::<EquipErrors>().queued().is_empty(),
            "no refusal's line or unlocks for the old session's items"
        );
    }
}
