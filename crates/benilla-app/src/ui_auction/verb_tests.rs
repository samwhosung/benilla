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
        let mut state = AuctionState {
            open: true,
            ..Default::default()
        };
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
