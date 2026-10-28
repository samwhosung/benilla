//! The vendor window's packet handlers: they fill [`MerchantOpen`] and signal on the merchant
//! lane ([`MerchantLane`]) inside themselves.

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{buy_error_key, sell_error_key, MerchantEvent, MerchantLane, MerchantOpen};
use crate::net::NetHandlerApp;

/// Register the vendor handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::VendorInventory, on_inventory)
        .net_handler(K::VendorBuyResult, on_buy_result)
        .net_handler(K::VendorBuyFailed, on_buy_failed)
        .net_handler(K::VendorSellFailed, on_sell_failed)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_inventory(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::VendorInventory { vendor, items } = ev {
        commands.queue(move |world: &mut World| super::open_merchant(world, vendor, items));
    }
}

/// `SMSG_BUY_ITEM` (`0x5dced0`): the stock, then the bought row's item and count through the quest
/// log's walk ([`crate::ui_quest_log::on_item_bought`]), then the window's stock update and its
/// `MERCHANT_UPDATE` (`0x5dd042` → `0x4fae70`).
fn on_buy_result(
    In(ev): In<SessionEvent>,
    mut merchant: ResMut<MerchantOpen>,
    mut merchant_lane: ResMut<MerchantLane>,
    book: crate::ui_quest_log::ItemBook,
    quest_log: ResMut<crate::ui_quest_log::QuestLog>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
) {
    let SessionEvent::VendorBuyResult {
        vendor,
        slot,
        new_count,
        purchase_count,
    } = ev
    else {
        return;
    };
    merchant.settle_world_reset();
    // Another vendor than the list cache's returns at once (`0x5dcf26`).
    if merchant.listed != Some(vendor) {
        return;
    }
    let bought = merchant
        .update_stock(slot, new_count)
        .map(|entry| (entry, purchase_count));
    crate::ui_quest_log::on_item_bought(In(bought), book, quest_log, script);
    // `0x4fae70` signals for the open vendor, a row found or not (`0x4faea5`, `0x4faebd`).
    if merchant.vendor == Some(vendor) {
        merchant_lane.push(MerchantEvent::Update);
    }
}

fn on_buy_failed(
    In(ev): In<SessionEvent>,
    mut merchant: ResMut<MerchantOpen>,
    mut lane: ResMut<MerchantLane>,
) {
    if let SessionEvent::VendorBuyFailed {
        vendor,
        item_entry,
        reason,
    } = ev
    {
        vendor_buy_failed(vendor, item_entry, reason, &mut merchant, &mut lane);
    }
}

fn on_sell_failed(
    In(ev): In<SessionEvent>,
    mut merchant_lane: ResMut<MerchantLane>,
    mut lane: ResMut<crate::item_lane::ItemLane>,
) {
    if let SessionEvent::VendorSellFailed {
        item_guid, reason, ..
    } = ev
    {
        vendor_sell_failed(reason, item_guid, &mut merchant_lane, &mut lane);
    }
}

/// The vendor window closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut merchant: ResMut<MerchantOpen>) {
    merchant.clear_session();
}

/// `SMSG_BUY_FAILED` (`0x5dcd60`): for `ITEM_ALREADY_SOLD` from the list cache's vendor
/// (`0x5dcd94`), each refused row zeroed (`0x5dcdbf`) and, the vendor open, its
/// `MERCHANT_UPDATE` (`0x5dcdc5` → `0x4fae70`); then the message line (`0x5dcdd8`). vmangos sends
/// that code only for a limited row short of stock (`Player.cpp:18541`).
///
/// Deviation: the row is matched by item entry, where the reference matches its vendor slot word
/// (`+0x00`), because vmangos writes the entry in that field (`Player.cpp:11718`).
fn vendor_buy_failed(
    vendor: u64,
    item_entry: u32,
    reason: u8,
    merchant: &mut MerchantOpen,
    lane: &mut MerchantLane,
) {
    debug!("net: buy failed (entry {item_entry}, reason {reason})");
    merchant.settle_world_reset();
    if reason == benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD
        && merchant.listed == Some(vendor)
    {
        let rows = merchant.sold_out(item_entry);
        if merchant.vendor == Some(vendor) {
            for _ in 0..rows {
                lane.push(MerchantEvent::Update);
            }
        }
    }
    if let Some(key) = buy_error_key(reason) {
        lane.push(MerchantEvent::Line(key));
    }
}

/// `SMSG_SELL_ITEM` (`0x5dd1f0`): a zero result is a success and returns at once (`0x5dd221`);
/// any other shows its message line (`0x5dd22c`) and unlocks the item the sale locked
/// (`0x5dd26b`), a zero guid excepted (`0x5dd267`), on the item lane at the packet's place. A
/// success's item leaves its slot, which unlocks it.
fn vendor_sell_failed(
    reason: u8,
    item_guid: u64,
    merchant_lane: &mut MerchantLane,
    lane: &mut crate::item_lane::ItemLane,
) {
    if reason == 0 {
        return;
    }
    debug!("net: sell failed (reason {reason}, item {item_guid:#x})");
    if let Some(key) = sell_error_key(reason) {
        merchant_lane.push(MerchantEvent::Line(key));
    }
    lane.push(crate::item_watch::Run {
        unlocks: (item_guid != 0)
            .then_some(crate::item_watch::Unlock::Item(item_guid))
            .into_iter()
            .collect(),
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
    use benilla_protocol::messages::{QuestObjective, VendorItem};
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::UiScript;

    use super::*;
    use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};

    const VENDOR: u64 = 0xF130_0000_0000_0043;
    const WATER: u32 = 159;

    /// A vendor open on [`VENDOR`] selling water in slot 3, quest 7 in the log needing five
    /// waters, the quest and item records cached, the quest log's lane showing the toasts, and the
    /// shown lines recorded in `SEEN`.
    fn quest_rig() -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::items::Items>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_quest::QuestLines>()
            .init_resource::<crate::ui_quest::QuestGiver>()
            .init_resource::<crate::ui_quest_log::QuestLog>()
            .init_resource::<MerchantOpen>()
            .init_resource::<MerchantLane>()
            .init_resource::<crate::item_lane::ItemLane>()
            .init_resource::<crate::field_watch::NoticeLane>()
            .add_systems(Update, crate::ui_quest_log::fire_quest_events);
        register(&mut app);
        let world = app.world_mut();
        let mut objectives = std::array::from_fn(|_| QuestObjective {
            creature_or_go: 0,
            required_count: 0,
            item_id: 0,
            item_count: 0,
            text: String::new(),
        });
        objectives[0].item_id = WATER;
        objectives[0].item_count = 5;
        let mut quest = crate::ui_quest::test_template(7, "Water", 0);
        quest.objectives = objectives;
        world
            .resource_mut::<crate::ui_quest_log::QuestLog>()
            .insert_template(quest);
        world
            .resource_mut::<crate::items::Items>()
            .insert_template(WATER, Some(crate::items::test_template("Water")));
        world.spawn((
            ObjectStore(ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, 7)])),
            Guid(0x2A),
            SelfPlayer,
        ));
        world.resource_mut::<MerchantOpen>().open(
            VENDOR,
            vec![VendorItem {
                slot: 3,
                entry: WATER,
                display_id: 18084,
                current_count: 9,
                price: 25,
                max_durability: 0,
                buy_count: 1,
            }],
        );
        let mut script = UiScript::new().unwrap();
        crate::ui_quest_log::seat_for_test(world, &mut script);
        script
            .run(
                r#"
                SEEN = ""
                ERR_QUEST_ADD_ITEM_SII = "%s: %d/%d"
                local f = CreateFrame("Frame")
                f:RegisterEvent("UI_INFO_MESSAGE")
                f:SetScript("OnEvent", function() SEEN = SEEN .. arg1 .. ";" end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .fire_world_enter();
        app.update();
        app
    }

    /// The lines one `SMSG_BUY_ITEM` showed.
    fn bought(app: &mut App, vendor: u64, slot: u32, count: u32) -> String {
        let ev = SessionEvent::VendorBuyResult {
            vendor,
            slot,
            new_count: 4,
            purchase_count: count,
        };
        crate::net::handlers::dispatch(app.world_mut(), vec![ev]);
        app.update();
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let seen = script.eval::<String>("return SEEN").unwrap();
        script.run("SEEN = ''").unwrap();
        seen
    }

    /// `SMSG_BUY_ITEM` walks the quest log for the bought row's item (`0x5dcf70`-`0x5dd02e`):
    /// from the open vendor (`0x5dcf20`), on the row its slot names (`0x5dcf42`), with the
    /// packet's bought count (`0x5dd00b`-`0x5dd017`); another vendor or a slot no row holds
    /// shows nothing (`0x5dcf26`, `0x5dcf54`).
    #[test]
    fn a_purchase_reports_the_bought_items_quest_progress() {
        let mut app = quest_rig();
        assert_eq!(bought(&mut app, VENDOR + 1, 3, 2), "");
        assert_eq!(bought(&mut app, VENDOR, 4, 2), "");
        assert_eq!(bought(&mut app, VENDOR, 3, 2), "Water: 2/5;");
        assert_eq!(
            app.world().resource::<MerchantOpen>().items[0].current_count,
            4
        );
    }

    /// A close clears the vendor cell, not the list cache's guid (`[0xc4d498]`, written only by a
    /// list and the world reset), so a purchase answered after the window closed still reports its
    /// progress (`0x5dcf20`).
    #[test]
    fn a_purchase_answered_after_the_close_still_reports_progress() {
        let mut app = quest_rig();
        app.world_mut().resource_mut::<MerchantOpen>().clear();
        assert_eq!(bought(&mut app, VENDOR, 3, 1), "Water: 1/5;");
        app.world_mut()
            .resource_mut::<MerchantOpen>()
            .clear_session();
        assert_eq!(bought(&mut app, VENDOR, 3, 1), "");
    }

    /// A refused sale unlocks the item it names (`0x5dd26b`); a zero result is a success, which
    /// unlocks nothing and shows nothing (`0x5dd221`).
    #[test]
    fn a_refused_sale_unlocks_its_item_and_a_success_does_nothing() {
        const ITEM: u64 = 0x4000_0000_0000_0042;
        let mut lines = MerchantLane::default();
        let mut lane = crate::item_lane::ItemLane::default();
        vendor_sell_failed(0, ITEM, &mut lines, &mut lane);
        assert!(lines.take().is_empty() && lane.drain_stamped().is_empty());
        vendor_sell_failed(1, ITEM, &mut lines, &mut lane);
        assert_eq!(lines.take(), [MerchantEvent::Line("ERR_ITEM_NOT_FOUND")]);
        let runs: Vec<_> = lane.drain_stamped().into_iter().map(|(_, r)| r).collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].unlocks, [crate::item_watch::Unlock::Item(ITEM)]);
    }
}
