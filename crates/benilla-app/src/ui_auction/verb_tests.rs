//! The auction verbs inside their calls, through the whole client's call host: each refusal's line
//! fires and each packet goes out before the verb returns, as the reference's bindings do them in
//! their own bodies, and the script's next line reads what the verb did.

use benilla_protocol::ObjectFields;
use benilla_ui::script::{
    AuctionItemRow, AuctionListState, AuctionState, ContainerSlot, ContainerState, UiScript, LIST,
    OWNER,
};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::AuctionOpen;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer};
use crate::script_calls::with_hosted_vm;

const ME: u64 = 0x10;
const AUCTIONEER: u64 = 0xF130_0000_0000_0A0C;
/// `PLAYER_FIELD_COINAGE`.
const COINAGE: u16 = 1176;
/// `PLAYER_FIELD_PACK_SLOT_1` and `PLAYER_FIELD_BANK_SLOT_1`, two fields per guid.
const PACK_SLOT_1: u16 = 532;
const BANK_SLOT_1: u16 = 564;
/// `OBJECT_FIELD_ENTRY`, `ITEM_FIELD_STACK_COUNT`.
const ENTRY: u16 = 3;
const STACK: u16 = 14;
/// The browse row's item.
const SILK: u32 = 4306;

/// Commands as their debug text, `ClientCommand` having no `PartialEq`.
fn text(commands: impl IntoIterator<Item = ClientCommand>) -> Vec<String> {
    commands.into_iter().map(|c| format!("{c:?}")).collect()
}

/// The whole client, headless, with us holding `money`, an auctioneer open and a VM holding one
/// browse row (auction 7: 100c start, 500c bid, 25c step, 2000c buyout, someone else's bid) and
/// one owner row (auction 8, a 400c bid), recording each red line and each sell-slot signal.
struct House {
    app: App,
    rx: Receiver<ClientCommand>,
}

impl House {
    fn new(money: u32) -> Self {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        let me = world
            .spawn((
                SelfPlayer,
                Guid(ME),
                ObjectStore(ObjectFields::from_pairs(&[(2, 0x19), (COINAGE, money)])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world.resource_mut::<AuctionOpen>().open(AUCTIONEER, 1);
        let mut script = UiScript::new().expect("a VM");
        let mut lua = String::new();
        for key in [
            "ERR_AUCTION_MIN_BID",
            "ERR_AUCTION_HIGHER_BID",
            "ERR_AUCTION_BID_INCREMENT",
            "ERR_NOT_ENOUGH_MONEY",
            "ERR_ITEM_MAX_COUNT",
            "ERR_AUCTION_REPAIR_ITEM",
        ] {
            lua.push_str(&format!("{key} = \"{key}\"\n"));
        }
        lua.push_str(
            r#"LINES, HEARD = {}, {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:RegisterEvent("NEW_AUCTION_UPDATE")
            f:SetScript("OnEvent", function()
                if event == "UI_ERROR_MESSAGE" then
                    table.insert(LINES, arg1)
                else
                    table.insert(HEARD, event)
                end
            end)"#,
        );
        script.run(&lua).expect("the recorder installs");
        let row = |auction_id: u32, item_id: u32| AuctionItemRow {
            auction_id,
            item_id,
            name: Some("Silk Cloth".into()),
            quality: Some(1),
            count: 1,
            min_bid: 100,
            ..Default::default()
        };
        let mut state = AuctionState::default();
        let mut browse = row(7, SILK);
        (browse.bid_amount, browse.min_increment, browse.buyout_price) = (500, 25, 2000);
        let mut owned = row(8, 2589);
        owned.bid_amount = 400;
        state.lists[LIST] = AuctionListState {
            rows: vec![browse],
            total: 1,
            ..Default::default()
        };
        state.lists[OWNER] = AuctionListState {
            rows: vec![owned],
            total: 1,
            ..Default::default()
        };
        script.set_auction(state);
        world.insert_non_send_resource(script);
        Self { app, rx }
    }

    /// Run `lua` with the host lent, as every entry that runs Lua lends it.
    fn run(&mut self, lua: &str) {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").run(lua))
            .expect("the script runs");
    }

    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, lua: &str) -> T {
        with_hosted_vm(self.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua))
            .expect("the chunk runs")
    }

    /// What went on the wire, the item queries the setup made aside.
    fn sent(&self) -> Vec<ClientCommand> {
        self.rx
            .try_iter()
            .filter(|c| !matches!(c, ClientCommand::ItemQuery { .. }))
            .collect()
    }

    fn house(&mut self) -> Mut<'_, AuctionOpen> {
        self.app.world_mut().resource_mut::<AuctionOpen>()
    }

    /// A stack of `entry` at the player's slot field `field`, as `0x622420` walks it.
    fn carry(&mut self, field: u16, guid: u64, entry: u32) {
        let world = self.app.world_mut();
        let item = world
            .spawn(ObjectStore(ObjectFields::from_pairs(&[
                (ENTRY, entry),
                (STACK, 1),
            ])))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(guid, item);
        let me = world.resource::<GuidIndex>().0[&ME];
        world
            .get_mut::<ObjectStore>(me)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[
                (field, guid as u32),
                (field + 1, (guid >> 32) as u32),
            ]));
    }
}

/// A refused bid shows its line inside the call, so the script's next line counts it, and sends
/// nothing (`0x4cf77a`); an accepted one is on the wire when the verb returns, lowered to the
/// buyout (`0x4cf762`, `0x4cf915`) and remembered for its result's patch (`0x4cf91e`).
#[test]
fn a_bid_refuses_or_sends_inside_the_call() {
    let mut h = House::new(1_000_000);
    h.run(r#"PlaceAuctionBid("list", 1, 50) AFTER = table.getn(LINES)"#);
    assert_eq!(h.eval::<i64>("return AFTER"), 1);
    assert_eq!(h.eval::<String>("return LINES[1]"), "ERR_AUCTION_MIN_BID");
    h.run(r#"PlaceAuctionBid("list", 1, 524)"#);
    assert_eq!(
        h.eval::<String>("return LINES[2]"),
        "ERR_AUCTION_BID_INCREMENT"
    );
    assert!(h.sent().is_empty());

    h.run(r#"PlaceAuctionBid("list", 1, 5000)"#);
    assert_eq!(
        text(h.sent()),
        text([ClientCommand::AuctionPlaceBid {
            auctioneer: AUCTIONEER,
            auction_id: 7,
            price: 2000,
        }])
    );
    assert_eq!(h.house().last_bid, 2000);
    assert_eq!(h.eval::<i64>("return table.getn(LINES)"), 2);
}

/// The money must cover the bid (`0x4cf847`), shown as `ERR_NOT_ENOUGH_MONEY`.
#[test]
fn a_bid_the_purse_cannot_cover_is_refused() {
    let mut h = House::new(524);
    h.run(r#"PlaceAuctionBid("list", 1, 525)"#);
    assert_eq!(h.eval::<String>("return LINES[1]"), "ERR_NOT_ENOUGH_MONEY");
    assert!(h.sent().is_empty());
}

/// A unique item already carried at its limit, the bank counted (scope `0x4f`, `0x4cf892`), is
/// error `0x12` (`0x4cf8a8`).
#[test]
fn a_bid_on_a_unique_item_already_carried_is_refused() {
    let mut h = House::new(1_000_000);
    let mut t = crate::items::test_template("Silk Cloth");
    t.max_count = 1;
    h.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(SILK, Some(t));
    h.carry(BANK_SLOT_1, 0xB1, SILK);
    h.run(r#"PlaceAuctionBid("list", 1, 600)"#);
    assert_eq!(h.eval::<String>("return LINES[1]"), "ERR_ITEM_MAX_COUNT");
    assert!(h.sent().is_empty());

    let mut h = House::new(1_000_000);
    let mut t = crate::items::test_template("Silk Cloth");
    t.max_count = 2;
    h.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(SILK, Some(t));
    h.carry(PACK_SLOT_1, 0xA1, SILK);
    h.run(r#"PlaceAuctionBid("list", 1, 600)"#);
    assert_eq!(h.sent().len(), 1, "under the limit");
}

/// No auctioneer, no bid and no line (`0x4cf75c`), nor a cancel (`0x4d02a5`).
#[test]
fn with_no_auctioneer_the_verbs_do_nothing() {
    let mut h = House::new(0);
    h.house().clear();
    h.run(r#"PlaceAuctionBid("list", 1, 50) CancelAuction(1)"#);
    h.run("GetOwnerAuctionItems() GetBidderAuctionItems()");
    assert_eq!(h.eval::<i64>("return table.getn(LINES)"), 0);
    assert!(h.sent().is_empty());
}

/// With no player object a bid does nothing (`0x4cf740`), and a cancel with a bid shows the money
/// line at once (`0x4d0301`).
#[test]
fn with_no_player_a_bid_does_nothing_and_a_cancel_is_refused() {
    let mut h = House::new(1_000_000);
    let world = h.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world.despawn(me);
    h.run(r#"PlaceAuctionBid("list", 1, 600)"#);
    assert_eq!(h.eval::<i64>("return table.getn(LINES)"), 0);
    h.run("CancelAuction(1)");
    assert_eq!(h.eval::<String>("return LINES[1]"), "ERR_NOT_ENOUGH_MONEY");
    assert!(h.sent().is_empty());
}

/// `CancelAuction` on a bid of 400 needs 20c, 5% truncated (`0x4d0338`-`0x4d0346`): short of it
/// the line shows and nothing goes; with it the cancel is on the wire when the verb returns.
#[test]
fn a_cancel_needs_five_percent_of_the_bid() {
    let mut h = House::new(19);
    h.run("CancelAuction(1) AFTER = table.getn(LINES)");
    assert_eq!(h.eval::<i64>("return AFTER"), 1);
    assert_eq!(h.eval::<String>("return LINES[1]"), "ERR_NOT_ENOUGH_MONEY");
    assert!(h.sent().is_empty());

    let mut h = House::new(20);
    h.run("CancelAuction(1)");
    assert_eq!(
        text(h.sent()),
        text([ClientCommand::AuctionRemoveItem {
            auctioneer: AUCTIONEER,
            auction_id: 8,
        }])
    );
}

/// The owner and bidder verbs send only while their flag is armed (`0x4cec97`, `0x4cece7`), the
/// hello's arming spent by the first; the bidder request carries the open house's outbid ids.
#[test]
fn the_list_verbs_send_once_per_arming() {
    let mut h = House::new(0);
    h.house().outbid_append(1, 31);
    h.house().outbid_append(2, 32);
    h.run("GetOwnerAuctionItems(1) GetOwnerAuctionItems(2) GetBidderAuctionItems() GetBidderAuctionItems()");
    assert_eq!(
        text(h.sent()),
        text([
            ClientCommand::AuctionListOwnerItems {
                auctioneer: AUCTIONEER,
                list_from: 50,
            },
            ClientCommand::AuctionListBidderItems {
                auctioneer: AUCTIONEER,
                list_from: 0,
                auction_ids: vec![31],
            },
        ])
    );
    h.house().owner_armed = true;
    h.run("GetOwnerAuctionItems(3)");
    assert_eq!(
        text(h.sent()),
        text([ClientCommand::AuctionListOwnerItems {
            auctioneer: AUCTIONEER,
            list_from: 150,
        }])
    );
}

/// A browse query is on the wire when the verb returns and arms the throttle, so
/// `CanSendAuctionQuery()` on the next line answers nil and a second query sends nothing
/// (`0x4cec3d`-`0x4cec4c`).
#[test]
fn a_query_sends_and_closes_the_throttle_inside_the_call() {
    let mut h = House::new(0);
    h.run(r#"QueryAuctionItems("silk") CAN = CanSendAuctionQuery() QueryAuctionItems("wool")"#);
    assert!(h.eval::<bool>("return CAN == nil"));
    let sent = h.sent();
    assert!(
        matches!(&sent[..], [ClientCommand::AuctionListItems { searched_name, .. }] if searched_name == "silk"),
        "{sent:?}"
    );
    assert_eq!(h.house().wire.browse_sent, 1);
}

/// The browse page is a number's `_ftol` times 50 in 32 bits, anything else 0
/// (`0x4ceb28`-`0x4ceb44`): page 85,899,346 wraps to offset 4, and page -1 to -50.
#[test]
fn the_browse_offset_wraps_in_32_bits() {
    for (page, offset) in [
        ("85899346", 4),
        ("-1", (-50i32) as u32),
        ("2.9", 100),
        ("'2'", 100),
        ("'x'", 0),
    ] {
        let mut h = House::new(0);
        h.run(&format!(
            r#"QueryAuctionItems("silk", nil, nil, nil, nil, nil, {page})"#
        ));
        let sent = h.sent();
        assert!(
            matches!(&sent[..], [ClientCommand::AuctionListItems { list_from, .. }] if *list_from == offset),
            "page {page}: {sent:?}"
        );
    }
}

/// The level bounds are a number's `_ftol` low byte, anything else 0 (`0x4ce9fd`-`0x4cea3b`):
/// 300 is 44, -1 is 255.
#[test]
fn the_level_bounds_take_the_low_byte() {
    for (min, max, bounds) in [
        ("300", "-1", (44, 255)),
        ("'10'", "20.9", (10, 20)),
        ("'x'", "{}", (0, 0)),
    ] {
        let mut h = House::new(0);
        h.run(&format!(r#"QueryAuctionItems("silk", {min}, {max})"#));
        let sent = h.sent();
        assert!(
            matches!(&sent[..], [ClientCommand::AuctionListItems { level_min, level_max, .. }] if (*level_min, *level_max) == bounds),
            "{min}, {max}: {sent:?}"
        );
    }
}

/// `StartAuction` sends and then clears the sell slot inside the call (`0x4ce925`, `0x4ce92a`):
/// `NEW_AUCTION_UPDATE` has fired by the next line, and a second call finds the slot empty.
#[test]
fn a_start_sends_and_empties_the_slot_inside_the_call() {
    let mut h = House::new(0);
    {
        let mut script = h.app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_container(
            0,
            Some(ContainerState {
                name: None,
                num_slots: 16,
                slots: [(
                    1,
                    ContainerSlot {
                        item_id: 2589,
                        count: 1,
                        quality: Some(1),
                        guid: 0x4001,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            }),
        );
    }
    h.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton() HEARD = {}");
    let _ = h.sent();
    h.run("StartAuction(100, 0, 480) AFTER = table.getn(HEARD) StartAuction(100, 0, 480)");
    assert_eq!(
        h.eval::<i64>("return AFTER"),
        1,
        "the slot's clear signalled in the call"
    );
    assert_eq!(
        text(h.sent()),
        text([ClientCommand::AuctionSellItem {
            auctioneer: AUCTIONEER,
            item_guid: 0x4001,
            bid: 100,
            buyout: 0,
            etime_minutes: 480,
        }]),
        "one send; the second call finds the slot empty"
    );
}

/// A damaged item's `StartAuction` shows `ERR_AUCTION_REPAIR_ITEM` inside the call
/// (`0x4ce80f`-`0x4ce83f`), sends nothing and keeps the slot.
#[test]
fn a_damaged_items_start_is_refused_inside_the_call() {
    let mut h = House::new(0);
    {
        let mut script = h.app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_container(
            0,
            Some(ContainerState {
                name: None,
                num_slots: 16,
                slots: [(
                    1,
                    ContainerSlot {
                        item_id: 2589,
                        count: 1,
                        quality: Some(1),
                        guid: 0x4001,
                        durability: Some((10, 40)),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            }),
        );
    }
    h.run("PickupContainerItem(0, 1) ClickAuctionSellItemButton() HEARD = {}");
    let _ = h.sent();
    h.run("StartAuction(100, 0, 480) AFTER = table.getn(LINES)");
    assert_eq!(h.eval::<i64>("return AFTER"), 1);
    assert_eq!(
        h.eval::<String>("return LINES[1]"),
        "ERR_AUCTION_REPAIR_ITEM"
    );
    assert!(h.sent().is_empty());
    assert_eq!(
        h.eval::<i64>("return table.getn(HEARD)"),
        0,
        "the slot stays"
    );
}
