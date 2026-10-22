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
    use benilla_ui::script::UiScript;

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

    /// The client in the world as our player with empty bags pushed, and a VM whose
    /// `UI_ERROR_MESSAGE`, `PLAYER_ENTERING_WORLD` and `BAG_UPDATE_COOLDOWN` handlers each note
    /// `HasKey()` behind their letter.
    fn keyring_client() -> App {
        let mut app = keyring_client_without_the_record();
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(KEY, Some(key_record()));
        app
    }

    /// The key's record: a key-family item.
    fn key_record() -> benilla_protocol::messages::ItemInfo {
        benilla_protocol::messages::ItemInfo {
            bag_family: super::super::BAG_FAMILY_KEYS,
            ..crate::items::test_template("Key")
        }
    }

    /// [`keyring_client`] before the key's record is known.
    fn keyring_client_without_the_record() -> App {
        const ME: u64 = 0x7;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let world = app.world_mut();
        let me = world
            .spawn((
                crate::net::Guid(ME),
                crate::net::SelfPlayer,
                crate::net::ObjectStore(
                    benilla_protocol::ObjectFields::default()
                        .into_created(benilla_protocol::messages::ObjectType::Player),
                ),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let script = benilla_ui::script::UiScript::new().unwrap();
        let line = super::super::equip_error::equip_error_key(REFUSAL);
        script
            .run(&format!(
                "{line} = 'Refused.' SEEN = '' local f = CreateFrame('Frame') \
                 local tag = {{UI_ERROR_MESSAGE = 'E', PLAYER_ENTERING_WORLD = 'W', \
                 BAG_UPDATE_COOLDOWN = 'C'}} \
                 for e in pairs(tag) do f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function() \
                 SEEN = SEEN .. tag[event] .. (HasKey() and 1 or 0) end)"
            ))
            .unwrap();
        world.insert_non_send_resource(script);
        world
            .run_system_cached(super::super::feed::feed_containers)
            .unwrap();
        app
    }

    const KEY: u32 = 5396;
    const REFUSAL: u8 = 0x16;

    /// One frame's refusal (`SMSG_INVENTORY_CHANGE_FAILURE`) and a key landing in backpack slot 1
    /// (its create, then our player's `PLAYER_FIELD_PACK_SLOT_1`, 532), the key first when
    /// `key_first`, through the handlers; then the bag feed, `between` (what the feeds after it
    /// fire), and the lanes' drain.
    fn refusal_and_key_frame(app: &mut App, key_first: bool, between: fn(&mut UiScript)) {
        frame(app, Vec::new(), packets_of(key_first), between);
    }

    /// The refusal, and the key's create and seat, in that order or the key first.
    fn packets_of(key_first: bool) -> Vec<SessionEvent> {
        let guid = u64::from(benilla_protocol::guid::HIGH_ITEM) << 48 | 0xA1;
        let refusal = SessionEvent::InventoryFailure {
            reason: REFUSAL,
            required_level: None,
            item_guid: 0,
            item2_guid: 0,
            bag_slot: 0xff,
        };
        let key = [
            SessionEvent::ItemCreate {
                guid,
                container: false,
                fields: benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x3),
                    (3, KEY),
                    (6, 0x7),
                    (8, 0x7),
                    (14, 1),
                ])
                .into_created(benilla_protocol::messages::ObjectType::Item),
            },
            SessionEvent::ObjectUpdateEnd,
            SessionEvent::ObjectValues {
                guid: 0x7,
                fields: benilla_protocol::ObjectFields::from_pairs(&[
                    (532, guid as u32),
                    (533, (guid >> 32) as u32),
                ]),
            },
            SessionEvent::ObjectUpdateEnd,
        ];
        if key_first {
            key.into_iter().chain([refusal]).collect()
        } else {
            std::iter::once(refusal).chain(key).collect()
        }
    }

    /// The refusal alone.
    fn refusal() -> SessionEvent {
        packets_of(false).swap_remove(0)
    }

    /// One frame: `ahead`, then `packets`, through the handlers; the bag feed, `between`, and the
    /// lanes' drain.
    fn frame(
        app: &mut App,
        ahead: Vec<SessionEvent>,
        packets: Vec<SessionEvent>,
        between: fn(&mut UiScript),
    ) {
        let world = app.world_mut();
        crate::net::handlers::dispatch(world, ahead.into_iter().chain(packets).collect());
        world
            .run_system_cached(super::super::feed::feed_containers)
            .unwrap();
        between(&mut world.non_send_resource_mut::<UiScript>());
        crate::lanes::fire_in_order(world);
    }

    /// One frame's refusal and a key landing in the backpack, the key first when `key_first`,
    /// over bags pushed empty; then what `HasKey()` read at the refusal's `UI_ERROR_MESSAGE`, and
    /// after the frame.
    fn refusal_and_key(key_first: bool) -> (String, bool) {
        let mut app = keyring_client();
        refusal_and_key_frame(&mut app, key_first, |_| {});
        let s = app.world().non_send_resource::<UiScript>();
        (
            s.eval::<String>("return SEEN").unwrap(),
            s.eval::<bool>("return HasKey() == 1").unwrap(),
        )
    }

    /// The failure's handler shows its line and unlocks its items (`0x496720`, `0x5e3ac0`,
    /// `0x5e3ad3`) inside its dispatch, so its handlers read the bags as the packets before it
    /// left them: a key that lands after it is not there yet, and one that landed before it is.
    #[test]
    fn an_inventory_failures_handlers_read_the_bags_the_packets_before_it_left() {
        assert_eq!(
            refusal_and_key(false),
            ("E0".into(), true),
            "the key came after"
        );
        assert_eq!(
            refusal_and_key(true),
            ("E1".into(), true),
            "the key came first"
        );
    }

    /// A write of our player that the bags do not read, its health's regeneration, stamps no
    /// swap: the refusal after it still reads the bags before the key that follows it.
    #[test]
    fn a_write_the_bags_do_not_read_leaves_the_refusal_on_the_old_bags() {
        let mut app = keyring_client();
        let regen = SessionEvent::ObjectValues {
            guid: 0x7,
            fields: benilla_protocol::ObjectFields::from_pairs(&[(22, 80)]),
        };
        frame(
            &mut app,
            vec![regen, SessionEvent::ObjectUpdateEnd],
            packets_of(false),
            |_| {},
        );
        let s = app.world().non_send_resource::<UiScript>();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "E0");
    }

    /// The record answer for a key already in the backpack changes what the bags show of it
    /// (`HasKey()` reads the key family off the record): a refusal ahead of the answer reads the
    /// bags before it.
    #[test]
    fn a_record_answer_after_a_refusal_swaps_the_bags_at_the_answer() {
        let mut app = keyring_client_without_the_record();
        let key = packets_of(true);
        frame(&mut app, Vec::new(), key[..4].to_vec(), |_| {});
        let s = app.world_mut().non_send_resource_mut::<UiScript>();
        assert!(!s.eval::<bool>("return HasKey() == 1").unwrap());
        s.run("SEEN = ''").unwrap();
        let answer = SessionEvent::ItemTemplate {
            entry: KEY,
            info: Some(Box::new(key_record())),
        };
        frame(&mut app, Vec::new(), vec![refusal(), answer], |_| {});
        let s = app.world().non_send_resource::<UiScript>();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "E0");
        assert!(s.eval::<bool>("return HasKey() == 1").unwrap());
    }

    /// The feeds' own events after the bag feed read this frame's bags, whatever the lanes owe:
    /// the world-enter cascade `feed_units` fires after the seats it reads, and the cooldown
    /// flush's `BAG_UPDATE_COOLDOWN` `fire_action_events` fires after the feed, while the refusal
    /// a key followed reads the bags before the key.
    #[test]
    fn the_feeds_events_after_the_bag_feed_read_this_frames_bags() {
        use crate::game_plugins::schedule_tests::{census, SyncPoints};
        let mut app = keyring_client();
        refusal_and_key_frame(&mut app, false, |s| {
            s.fire_world_enter();
            s.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
        });
        let s = app.world().non_send_resource::<UiScript>();
        assert_eq!(s.eval::<String>("return SEEN").unwrap(), "W1C1E0");
        let c = census(
            &mut crate::game_plugins::schedule_tests::headless_client(),
            Update,
            SyncPoints::Declared,
        );
        if !crate::game_plugins::schedule_tests::type_names_available(&c) {
            eprintln!("skipped: this build carries no type names");
            return;
        }
        let key = |suffix: &str| {
            c.systems
                .iter()
                .find(|(_, i)| i.name.ends_with(suffix))
                .map(|(k, _)| *k)
                .unwrap_or_else(|| panic!("{suffix} is in Update"))
        };
        let feed = key("::ui_items::feed::feed_containers");
        let mut after = std::collections::HashSet::new();
        let mut stack = vec![feed];
        while let Some(n) = stack.pop() {
            for (a, b) in &c.dependencies {
                if *a == n && after.insert(*b) {
                    stack.push(*b);
                }
            }
        }
        for reader in [
            "::ui_unit::feed_units",
            "::ui_action::state::fire_action_events",
        ] {
            assert!(
                after.contains(&key(reader)),
                "{reader} runs after the bag feed"
            );
        }
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
