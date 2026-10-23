//! The merchant verbs through the whole client's host: `BuyMerchantItem` (`0x4fb850`),
//! `PickupMerchantItem`'s sale (`0x4fb760`), `BuybackItem` (`0x4fb950`) and `RepairAllItems`
//! (`0x4fbfe0`) send inside their calls, so a chunk's packets leave in call order with every
//! other verb's.

use benilla_protocol::field::{FIELD_PLAYER_FIELD_COINAGE, FIELD_UNIT_NPC_FLAGS};
use benilla_protocol::ObjectFields;
use benilla_ui::script::{ContainerSlot, ContainerState};
use crossbeam_channel::Receiver;

use super::*;
use crate::net::ClientCommand;
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const VENDOR: u64 = 0xF130_0000_0000_0043;
/// Our Linen Cloth, five of them in backpack slot 1.
const CLOTH: u64 = 0x4000_0000_0000_0a01;
/// The absolute indices of `PLAYER_FIELD_VENDORBUYBACK_SLOT_1`'s low word and
/// `PLAYER_FIELD_BUYBACK_PRICE_1`.
const BUYBACK_SLOT_1: u16 = 624;
const BUYBACK_PRICE_1: u16 = 1226;

/// The whole client, headless, with a repairing vendor's window open on [`VENDOR`] selling
/// Refreshing Spring Water (159) in row 1, our cloth in the backpack and one sold item in the
/// buyback list.
fn client() -> (App, Receiver<ClientCommand>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (532, CLOTH as u32),
                (533, (CLOTH >> 32) as u32),
                (BUYBACK_SLOT_1, 0x0a02),
                (BUYBACK_SLOT_1 + 1, 0x4000_0000),
                (BUYBACK_PRICE_1, 30),
                (FIELD_PLAYER_FIELD_COINAGE, 10_000),
            ])),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    let vendor = world
        .spawn((
            Guid(VENDOR),
            ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_NPC_FLAGS,
                NPC_FLAG_REPAIR,
            )])),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(VENDOR, vendor);
    crate::items::test_spawn_item(
        world,
        CLOTH,
        ObjectFields::from_pairs(&[(3, 2589), (14, 5)]),
        false,
    );
    world.resource_mut::<MerchantOpen>().open(
        VENDOR,
        vec![VendorItem {
            slot: 1,
            entry: 159,
            display_id: 18084,
            current_count: STOCK_UNLIMITED,
            price: 25,
            max_durability: 0,
            buy_count: 5,
        }],
    );
    let mut script = UiScript::new().expect("a VM");
    script.set_money(10_000);
    script.set_merchant(Some(MerchantState {
        items: vec![MerchantItem {
            name: Some("Refreshing Spring Water".into()),
            price: 25,
            quantity: 5,
            num_available: -1,
            item_id: 159,
            ..Default::default()
        }],
        buyback: vec![MerchantItem {
            name: Some("Tough Jerky".into()),
            item_id: 117,
            ..Default::default()
        }],
        can_repair: true,
    }));
    let mut bag = ContainerState {
        num_slots: 16,
        ..Default::default()
    };
    bag.slots.insert(
        1,
        ContainerSlot {
            item_id: 2589,
            guid: CLOTH,
            count: 5,
            ..Default::default()
        },
    );
    script.set_container(0, Some(bag));
    world.insert_non_send_resource(script);
    (app, rx)
}

fn run(app: &mut App, lua: &str) {
    with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

/// The packets a chunk sent, the queries left out.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.contains("Query"))
        .collect()
}

/// Each merchant verb sends inside its call, so the rolls between them leave between them:
/// `BuyMerchantItem` names the row's item (`0x4fb933`-`0x4fb93c`), `BuybackItem` the absolute
/// slot 69 (`0x4fba02`), and `RepairAllItems` the vendor with guid 0 (`0x4fc0d5`).
#[test]
fn the_merchant_verbs_send_inside_their_calls() {
    let (mut app, rx) = client();
    let _ = sent(&rx);
    run(
        &mut app,
        "BuyMerchantItem(1, 3) RandomRoll(1, 2) BuybackItem(1) RandomRoll(3, 4) \
         RepairAllItems() RandomRoll(5, 6)",
    );
    assert_eq!(
        sent(&rx),
        [
            format!("BuyItem {{ vendor: {VENDOR}, entry: 159, count: 3 }}"),
            "RandomRoll { min: 1, max: 2 }".to_string(),
            format!("BuybackItem {{ vendor: {VENDOR}, slot: 69 }}"),
            "RandomRoll { min: 3, max: 4 }".to_string(),
            format!("RepairItem {{ vendor: {VENDOR}, item_guid: 0 }}"),
            "RandomRoll { min: 5, max: 6 }".to_string(),
        ]
    );
}

/// `PickupMerchantItem` with an item on the cursor sells it inside the call (`0x4fb7a7` →
/// `0x5e1d50`): the cursor's own guid (`0x494c60`), its split count (`[0xb4b40c]`, 0 for the
/// whole stack), and the vendor cell as it stands, 0 with no window open, which it never tests.
#[test]
fn a_cursor_sale_sends_inside_the_call_with_the_split_count_and_the_cell() {
    let (mut app, rx) = client();
    let _ = sent(&rx);
    run(
        &mut app,
        "SplitContainerItem(0, 1, 2) PickupMerchantItem(0) RandomRoll(1, 2)",
    );
    assert_eq!(
        sent(&rx),
        [
            format!("SellItem {{ vendor: {VENDOR}, item_guid: {CLOTH}, count: 2 }}"),
            "RandomRoll { min: 1, max: 2 }".to_string(),
        ]
    );
    // The sale keeps the item locked until the server answers, so a fresh client sells again.
    let (mut app, rx) = client();
    let _ = sent(&rx);
    app.world_mut().resource_mut::<MerchantOpen>().clear();
    run(&mut app, "PickupContainerItem(0, 1) PickupMerchantItem(0)");
    assert_eq!(
        sent(&rx),
        [format!(
            "SellItem {{ vendor: 0, item_guid: {CLOTH}, count: 0 }}"
        )]
    );
}

/// `BuyMerchantItem` clears the cursor first (`0x4fb89f`, `ClearCursor(1, 1)`), whatever it then
/// buys: a held item goes back to its slot.
#[test]
fn buy_merchant_item_clears_the_cursor_first() {
    let (mut app, _rx) = client();
    let held: bool = with_hosted_vm(app.world_mut(), |vm| {
        vm.expect("a VM")
            .eval("PickupContainerItem(0, 1) BuyMerchantItem(9) return CursorHasItem() ~= nil")
    })
    .expect("the chunk runs");
    assert!(!held);
}

/// A held vendor row dropped on a bag slot (`0x4f9f35`) or a doll slot (`0x4c78fd`) sends
/// `CMSG_BUY_ITEM_IN_SLOT` inside the drop's call, count 1, so the roll after it leaves after it.
#[test]
fn a_held_rows_drop_buys_into_its_slot_inside_the_call() {
    let (mut app, rx) = client();
    let _ = sent(&rx);
    run(
        &mut app,
        "PickupMerchantItem(1) PickupContainerItem(0, 2) RandomRoll(1, 2) \
         PickupMerchantItem(1) PickupInventoryItem(16) RandomRoll(3, 4)",
    );
    let sent = sent(&rx);
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(
        sent[0].starts_with(&format!(
            "BuyItemInSlot {{ vendor: {VENDOR}, entry: 159, bag_guid: {ME}, bag_slot: 24"
        )),
        "{sent:?}"
    );
    assert_eq!(sent[1], "RandomRoll { min: 1, max: 2 }");
    assert!(
        sent[2].starts_with(&format!(
            "BuyItemInSlot {{ vendor: {VENDOR}, entry: 159, bag_guid: {ME}, bag_slot: 15"
        )),
        "{sent:?}"
    );
    assert_eq!(sent[3], "RandomRoll { min: 3, max: 4 }");
}

/// A held vendor row dropped on a bank bag's slot buys into that bag (`0x4f9f32`): the slot's
/// list is the bag's own, so `CMSG_BUY_ITEM_IN_SLOT` names the bag's guid, read from
/// `PLAYER_FIELD_BANK_BAG_SLOT_1`, and the 0-based slot.
#[test]
fn a_held_rows_drop_on_a_bank_bag_slot_buys_into_that_bag() {
    /// The bag in the first bank bag slot, container 5.
    const BANK_BAG: u64 = 0x4000_0000_0000_0b01;
    /// `PLAYER_FIELD_BANK_BAG_SLOT_1`'s low word.
    const BANK_BAG_SLOT_1: u16 = 612;
    let (mut app, rx) = client();
    {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut store = q.single_mut(world).expect("the player");
        store.0.merge(ObjectFields::from_pairs(&[
            (BANK_BAG_SLOT_1, BANK_BAG as u32),
            (BANK_BAG_SLOT_1 + 1, (BANK_BAG >> 32) as u32),
        ]));
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.set_bank(Some(benilla_ui::script::BankState::default()));
        script.set_container(
            5,
            Some(ContainerState {
                num_slots: 8,
                ..Default::default()
            }),
        );
    }
    let _ = sent(&rx);
    run(&mut app, "PickupMerchantItem(1) PickupContainerItem(5, 2)");
    let sent = sent(&rx);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].starts_with(&format!(
            "BuyItemInSlot {{ vendor: {VENDOR}, entry: 159, bag_guid: {BANK_BAG}, bag_slot: 1"
        )),
        "{sent:?}"
    );
}
