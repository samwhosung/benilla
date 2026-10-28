//! The merchant getters as a listener reads them, and the vendor's events at their packets. The
//! getters read the window's cell and rows, the player's buy-back fields and the vendor's unit at
//! the call (`0x4fb130`, `0x4fb150`, `0x4fc160`, `0x4fadb0`, `0x4fbd60`); the events fire inside
//! the handler or callback that raises them: the list's `MERCHANT_SHOW` (`0x4fad92`), the stock
//! update's `MERCHANT_UPDATE` (`0x4fae70`), a refusal's line (`0x5dcdd8`), a row record's answer
//! (`0x4fb0f0`) and the buy-back watch (`0x4faca0`).

use benilla_protocol::field::FIELD_UNIT_NPC_FLAGS;
use benilla_protocol::messages::VendorItem;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const VENDOR: u64 = 0xF130_0000_0000_0043;
/// `UNIT_NPC_FLAG_VENDOR | UNIT_NPC_FLAG_REPAIR`.
const VENDOR_REPAIRS: u32 = 0x4 | 0x4000;
/// `PLAYER_FIELD_VENDORBUYBACK_SLOT_1`'s low word and `PLAYER_FIELD_BUYBACK_PRICE_1`, absolute.
const BUYBACK_SLOT_1: u16 = 624;
const BUYBACK_PRICE_1: u16 = 1226;
/// A row whose record the cache lacks.
const UNCACHED: u32 = 777;

/// The whole client, headless, in the world.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
    }

    /// The item queries sent since the last call.
    fn item_queries(&mut self) -> usize {
        crate::query_cache::queries(&self.sent)
            .into_iter()
            .filter(|c| matches!(c, ClientCommand::ItemQuery { .. }))
            .count()
    }
}

/// The client in the world with a repairing vendor beside us, and a spy whose line for each
/// merchant event and message line carries its name and first argument.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            ERR_VENDOR_SOLD_OUT = "sold out"
            local f = CreateFrame("Frame")
            for _, e in ipairs({"MERCHANT_SHOW", "MERCHANT_UPDATE", "MERCHANT_CLOSED",
                    "UI_ERROR_MESSAGE", "UI_INFO_MESSAGE", "CHAT_MSG_MONSTER_SAY"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[]),
        created(
            VENDOR,
            EntityKind::Unit,
            &[(FIELD_UNIT_NPC_FLAGS, VENDOR_REPAIRS)],
        ),
    ]]);
    c.frame(vec![]);
    c.seen();
    c.item_queries();
    c
}

fn row(slot: u32, entry: u32) -> VendorItem {
    VendorItem {
        slot,
        entry,
        display_id: 0,
        current_count: 5,
        price: 25,
        max_durability: 0,
        buy_count: 1,
    }
}

fn list(rows: Vec<VendorItem>) -> SessionEvent {
    SessionEvent::VendorInventory {
        vendor: VENDOR,
        items: rows,
    }
}

/// A monster's line, which shows inside its own packet's handler on the chat lane.
fn boar_says(text: &str) -> SessionEvent {
    SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: text.into(),
        chat_tag: 0,
    })
}

/// Each getter reads what the handlers wrote at the call, with no push between: the rows, the
/// vendor's repair flag, the buy-back fields and the repair total; with no host the VM's copy,
/// which nothing pushes, answers as it was.
#[test]
fn the_merchant_getters_read_the_world_at_the_call() {
    let mut c = client();
    c.handle(vec![list(vec![row(1, 159), row(2, 160)])]);
    assert_eq!(c.eval::<i64>(true, "return GetMerchantNumItems()"), 2);
    assert_eq!(c.eval::<i64>(false, "return GetMerchantNumItems()"), 0);
    assert_eq!(
        c.eval::<i64>(
            true,
            "local _, _, price = GetMerchantItemInfo(2) return price"
        ),
        25
    );
    assert!(c.eval::<bool>(true, "return CanMerchantRepair() == 1"));
    // A repairer with nothing damaged: the pair, the flag nil (`0x4fbfb7`).
    assert_eq!(
        c.eval::<(i64, i64)>(
            true,
            "return (function(...) return arg.n end)(GetRepairAllCost()), (GetRepairAllCost())"
        ),
        (2, 0)
    );
    c.handle(vec![values(VENDOR, &[(FIELD_UNIT_NPC_FLAGS, 0x4)])]);
    assert!(
        c.eval::<bool>(true, "return CanMerchantRepair() == nil"),
        "the flag read off the unit as it stands"
    );
    assert_eq!(
        c.eval::<i64>(
            true,
            "return (function(...) return arg.n end)(GetRepairAllCost())"
        ),
        1,
        "no repairer: the one 0 (`0x4fbd76`)"
    );
    c.handle(vec![values(
        ME,
        &[
            (BUYBACK_SLOT_1, 0x0a02),
            (BUYBACK_SLOT_1 + 1, 0x4000_0000),
            (BUYBACK_PRICE_1, 30),
        ],
    )]);
    assert_eq!(c.eval::<i64>(true, "return GetNumBuybackItems()"), 1);
    assert_eq!(c.eval::<i64>(false, "return GetNumBuybackItems()"), 0);
    // The close zeroes the count (`0x4fae22`).
    crate::ui_merchant::close_core(c.app.world_mut());
    assert_eq!(c.eval::<i64>(true, "return GetMerchantNumItems()"), 0);
}

/// The list's `MERCHANT_SHOW`, a purchase's and a sold-out refusal's `MERCHANT_UPDATE` and the
/// refusal's line fire at their packets' places, around a chat line of a packet between them.
#[test]
fn the_merchant_events_fire_at_their_packets() {
    let mut c = client();
    c.frame(vec![
        vec![list(vec![row(1, 159)])],
        vec![boar_says("a")],
        vec![SessionEvent::VendorBuyResult {
            vendor: VENDOR,
            slot: 1,
            new_count: 4,
            purchase_count: 1,
        }],
        vec![boar_says("b")],
        vec![SessionEvent::VendorBuyFailed {
            vendor: VENDOR,
            item_entry: 159,
            reason: benilla_protocol::messages::buy_result::ITEM_ALREADY_SOLD,
        }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "MERCHANT_SHOW:nil",
            "CHAT_MSG_MONSTER_SAY:a",
            "MERCHANT_UPDATE:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "MERCHANT_UPDATE:nil",
            "UI_ERROR_MESSAGE:sold out",
        ]
    );
    assert_eq!(
        c.eval::<i64>(
            true,
            "local _, _, _, _, left = GetMerchantItemInfo(1) return left"
        ),
        0,
        "the refused row zeroed (`0x5dcdbf`)"
    );
}

/// The list's `MERCHANT_SHOW` sits on the merchant lane at its handler's stamp, between the
/// stamps of the handlers before and after it.
#[test]
fn the_show_is_stamped_at_its_handler() {
    let mut c = client();
    let before = crate::lanes::stamp();
    c.handle(vec![list(vec![row(1, 159)])]);
    let after = crate::lanes::stamp();
    let lane = c.app.world().resource::<crate::ui_merchant::MerchantLane>();
    let stamped = lane.stamped();
    assert_eq!(stamped.len(), 1);
    let (at, event) = stamped[0];
    assert_eq!(event, crate::ui_merchant::MerchantEvent::Show);
    assert!(before < at && at < after, "stamped inside the handler");
}

/// A row's missing record is asked by the getter's call with its callback, one wait per call
/// (`0x4faf1a`, `0x4faf23`); the answer runs the callbacks, and the last signals `MERCHANT_UPDATE`
/// at the answer's packet (`0x4fb0f0`). A buy-back field's change signals it at its packet too
/// (`0x4faca0`), a vendor open or not.
#[test]
fn a_records_answer_and_the_buyback_watch_signal_at_their_packets() {
    let mut c = client();
    c.handle(vec![list(vec![row(1, UNCACHED)])]);
    c.frame(vec![]);
    assert_eq!(c.seen(), ["MERCHANT_SHOW:nil"]);
    let name = "return (GetMerchantItemInfo(1))";
    assert_eq!(c.eval::<Option<String>>(true, name), None, "in flight");
    assert_eq!(c.eval::<Option<String>>(true, name), None);
    assert_eq!(c.item_queries(), 1, "asked once while in flight");
    let info = crate::items::test_template("Bread");
    c.frame(vec![
        vec![SessionEvent::ItemTemplate {
            entry: UNCACHED,
            info: Some(Box::new(info)),
        }],
        vec![boar_says("a")],
    ]);
    assert_eq!(
        c.seen(),
        ["MERCHANT_UPDATE:nil", "CHAT_MSG_MONSTER_SAY:a"],
        "the second wait's answer signals, once"
    );
    assert_eq!(
        c.eval::<Option<String>>(true, name).as_deref(),
        Some("Bread")
    );
    crate::ui_merchant::close_core(c.app.world_mut());
    c.seen();
    c.frame(vec![
        vec![boar_says("b")],
        vec![values(ME, &[(BUYBACK_PRICE_1, 30)])],
    ]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_MONSTER_SAY:b", "MERCHANT_UPDATE:nil"],
        "the watch signals with no vendor open"
    );
}

/// A refusal's `MERCHANT_UPDATE` fires inside its walk, while the dying node stands: a listener's
/// getter that asks the row again links its callback to the node and sends nothing. Raised by the
/// last callback, the walk ends there and the node's destroy frees it uncalled (`0x55be9c`,
/// `0x55bed2`); raised by one with a next, the walk reaches it, and its run with status 0 signals
/// again, whose re-ask is then the last's. A later call asks again.
#[test]
fn a_getter_inside_a_refusals_walk_asks_nothing() {
    refusal_walk(false);
    refusal_walk(true);
}

/// [`a_getter_inside_a_refusals_walk_asks_nothing`], with a trade getter's wait after the row's
/// when `behind`.
fn refusal_walk(behind: bool) {
    let mut c = client();
    c.handle(vec![list(vec![row(1, UNCACHED)])]);
    c.frame(vec![]);
    c.seen();
    c.eval::<()>(
        true,
        r#"
        local f = CreateFrame("Frame")
        f:RegisterEvent("MERCHANT_UPDATE")
        f:SetScript("OnEvent", function() ReAsked = (GetMerchantItemInfo(1)) or "none" end)
        GetMerchantItemInfo(1)
        "#,
    );
    assert_eq!(c.item_queries(), 1);
    if behind {
        c.app
            .world()
            .resource::<crate::items::Items>()
            .wait_for_trade_record(UNCACHED);
    }
    c.frame(vec![vec![SessionEvent::ItemTemplate {
        entry: UNCACHED,
        info: None,
    }]]);
    let updates: &[&str] = if behind {
        &["MERCHANT_UPDATE:nil", "MERCHANT_UPDATE:nil"]
    } else {
        &["MERCHANT_UPDATE:nil"]
    };
    assert_eq!(c.seen(), updates, "the re-ask's run signals when reached");
    assert_eq!(c.eval::<String>(false, "return ReAsked"), "none");
    assert_eq!(c.item_queries(), 0, "nothing sent inside the walk");
    let waits = c
        .app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .take_waiters(UNCACHED, crate::items::RecordCallback::Merchant);
    assert_eq!(waits, 0, "nothing kept");
    c.eval::<()>(true, "GetMerchantItemInfo(1)");
    assert_eq!(c.item_queries(), 1, "a later call asks again");
}

/// The item queries sent since the last call, as their bodies go out.
fn query_bodies(c: &mut Client) -> Vec<Vec<u8>> {
    crate::query_cache::queries(&c.sent)
        .into_iter()
        .filter_map(|q| match q {
            ClientCommand::ItemQuery { entry, guid } => {
                Some(benilla_protocol::messages::item_query(entry, guid))
            }
            _ => None,
        })
        .collect()
}

/// A sold item of ours, [`SOLD`], whose record is `entry`, rolled with random property `property`
/// and gift-wrapped when `wrapped`, in buy-back slot 1 at 30 copper.
fn sold(c: &mut Client, entry: u32, property: u32, wrapped: bool) {
    use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
    let mut pairs = vec![
        (2, 0x3),
        (FIELD_OBJECT_ENTRY, entry),
        (FIELD_ITEM_STACK_COUNT, 1),
        (6, ME as u32),
        (8, ME as u32),
        (21, if wrapped { 0x8 } else { 0 }),
        (44, property),
    ];
    pairs.sort_unstable();
    c.handle(vec![
        SessionEvent::ItemCreate {
            guid: SOLD,
            container: false,
            fields: benilla_protocol::ObjectFields::from_pairs(&pairs)
                .into_created(benilla_protocol::messages::ObjectType::Item),
        },
        values(
            ME,
            &[
                (BUYBACK_SLOT_1, SOLD as u32),
                (BUYBACK_SLOT_1 + 1, (SOLD >> 32) as u32),
                (BUYBACK_PRICE_1, 30),
            ],
        ),
    ]);
}

const SOLD: u64 = 0x4000_0000_0000_0a02;

/// A vendor row's record query names the vendor (`0x4faef6`-`0x4faf17`); a buy-back row's names
/// no one, its guid 0 (`0x4fb3f0`-`0x4fb3f7`). Both bodies byte for byte.
#[test]
fn a_vendor_rows_query_names_the_vendor_and_a_buyback_rows_none() {
    let mut c = client();
    c.handle(vec![list(vec![row(1, UNCACHED)])]);
    c.eval::<()>(true, "GetMerchantItemInfo(1)");
    assert_eq!(
        query_bodies(&mut c),
        [vec![0x09, 0x03, 0, 0, 0x43, 0, 0, 0, 0, 0, 0x30, 0xf1]],
        "entry 777, then the vendor's guid"
    );
    sold(&mut c, UNCACHED + 1, 0, false);
    c.eval::<()>(true, "GetBuybackItemInfo(1)");
    assert_eq!(
        query_bodies(&mut c),
        [vec![0x0a, 0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
        "entry 778, then 0"
    );
}

/// A list cancels the old rows' callbacks only while the window holds rows (`0x4fad01`), which a
/// close zeroes (`0x4fae22`): a callback asked before a close still signals `MERCHANT_UPDATE` at
/// its answer under the next list; one asked under an open list is cancelled by the next.
#[test]
fn a_list_cancels_the_old_rows_callbacks_only_while_it_holds_rows() {
    let answer = |c: &mut Client| {
        c.frame(vec![vec![SessionEvent::ItemTemplate {
            entry: UNCACHED,
            info: Some(Box::new(crate::items::test_template("Bread"))),
        }]]);
        c.seen()
            .into_iter()
            .filter(|e| e == "MERCHANT_UPDATE:nil")
            .count()
    };
    let mut c = client();
    c.handle(vec![list(vec![row(1, UNCACHED)])]);
    c.eval::<()>(true, "GetMerchantItemInfo(1)");
    crate::ui_merchant::close_core(c.app.world_mut());
    c.handle(vec![list(vec![row(1, 159)])]);
    c.frame(vec![]);
    c.seen();
    assert_eq!(answer(&mut c), 1, "the stale callback ran");

    let mut c = client();
    c.handle(vec![list(vec![row(1, UNCACHED)])]);
    c.eval::<()>(true, "GetMerchantItemInfo(1)");
    c.handle(vec![list(vec![row(1, 159)])]);
    c.frame(vec![]);
    c.seen();
    assert_eq!(answer(&mut c), 0, "cancelled by the list");
}

/// A buy-back row's name is its object's full name (`0x4fb42a` → `0x5d8bc0`): the record's with
/// the random-property suffix, bare on a wrapped item.
#[test]
fn a_buyback_rows_name_carries_its_suffix() {
    use benilla_formats::{RandomProperty, RandomPropertyCatalog};
    let mut c = client();
    c.app.insert_resource(crate::items::RandomProperties::new(
        RandomPropertyCatalog::from_rows(
            [(
                584,
                RandomProperty {
                    suffix: "of the Monkey".into(),
                    enchants: [74, 71, 0, 0, 0],
                },
            )]
            .into_iter()
            .collect(),
        ),
    ));
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(UNCACHED, Some(crate::items::test_template("Bandit Cloak")));
    let name = "return (GetBuybackItemInfo(1))";
    sold(&mut c, UNCACHED, 584, false);
    assert_eq!(c.eval::<String>(true, name), "Bandit Cloak of the Monkey");
    sold(&mut c, UNCACHED, 584, true);
    assert_eq!(c.eval::<String>(true, name), "Bandit Cloak", "wrapped");
}

/// A trainer's list in the vendor's frame takes the latch, whose release closes the vendor
/// (`0x4930d0` → `0x4fae10`): the close's `MERCHANT_CLOSED` fires at that packet, after the
/// vendor list's `MERCHANT_SHOW`, so the stock frame shows and then hides.
#[test]
fn a_close_in_the_lists_frame_fires_after_its_show() {
    const TRAINER: u64 = 0xF130_0000_0000_0044;
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::ui_trainer::TrainerOpen>()
        .asked = TRAINER;
    let trainer = SessionEvent::TrainerList {
        trainer: TRAINER,
        trainer_type: 0,
        services: Vec::new(),
        greeting: String::new(),
    };
    c.frame(vec![vec![list(vec![row(1, 25)])], vec![trainer]]);
    assert_eq!(c.seen(), ["MERCHANT_SHOW:nil", "MERCHANT_CLOSED:nil"]);
}

/// The verbs that test the purse read the coinage at the call, as `GetMoney` does (`0x518160`,
/// `+0xf70`): `PickupPlayerMoney`'s gate (`0x494cc0`) passes on a coinage its packet wrote, with no
/// push between, and refuses once a later packet lowers it.
#[test]
fn the_purse_tests_read_the_coinage_at_the_call() {
    use benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE;
    let mut c = client();
    let pickup = "ClearCursor() PickupPlayerMoney(400) return GetCursorMoney()";
    c.handle(vec![values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 500)])]);
    assert_eq!(c.eval::<i64>(true, pickup), 400);
    c.handle(vec![values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 300)])]);
    assert_eq!(c.eval::<i64>(true, pickup), 0, "the purse falls short");
}

/// A bag item's repair cost read in a listener of each of two packets in one frame, each packet
/// moving the open vendor's PvP flag and the purse: `SetBagItem` prices the object at the call
/// after the vendor's discount (`0x534948` → `0x4faf30`, `0x4faf8a`), as stock
/// `ContainerFrame.lua:267` reads it in its listeners, so each listener reads the vendor its own
/// packet left, not the frame's last.
#[test]
fn a_repair_cost_reads_as_of_each_packet() {
    use benilla_protocol::field::{
        FIELD_PLAYER_FIELD_COINAGE, FIELD_PLAYER_INV_SLOT_HEAD, FIELD_UNIT_FLAGS,
    };
    /// `OBJECT_FIELD_ENTRY`, `ITEM_FIELD_STACK_COUNT`, `ITEM_FIELD_DURABILITY` and its maximum.
    const ENTRY: u16 = 3;
    const STACK: u16 = 14;
    const DURABILITY: u16 = 46;
    const MAX_DURABILITY: u16 = 47;
    /// `PLAYER_BYTES_3`, absolute: byte 3 is the honor rank the discount steps by.
    const PLAYER_BYTES_3: u16 = 195;
    const SWORD: u64 = 0x4000_0000_0000_0707;
    const SWORD_ENTRY: u32 = 2488;
    let (full, discounted) = crate::ui_merchant::purse_fixture::sword_prices();
    let mut c = client();
    // One level-20 row at 1 copper a point for a sword (weapon subclass 7), quality 1 at 1.
    let mut costs = vec![0u32; 29];
    costs[7] = 1;
    c.app.insert_resource(crate::ui_merchant::RepairTables(
        benilla_formats::DurabilityTables::from_rows([(20, costs)], [1.0; 4]),
    ));
    let mut sword = crate::items::test_template("Sword");
    (sword.class, sword.subclass, sword.quality, sword.item_level) = (2, 7, 1, 20);
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(SWORD_ENTRY, Some(sword));
    let pack_1 = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    c.frame(vec![
        vec![
            SessionEvent::ItemCreate {
                guid: SWORD,
                container: false,
                fields: benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x3),
                    (ENTRY, SWORD_ENTRY),
                    (STACK, 1),
                    (DURABILITY, 35),
                    (MAX_DURABILITY, 75),
                ])
                .into_created(benilla_protocol::messages::ObjectType::Item),
            },
            values(
                ME,
                &[
                    (pack_1, SWORD as u32),
                    (pack_1 + 1, (SWORD >> 32) as u32),
                    (PLAYER_BYTES_3, 8 << 24),
                ],
            ),
        ],
        vec![list(vec![row(1, 159)])],
    ]);
    c.frame(vec![]);
    {
        let script = c.app.world().non_send_resource::<UiScript>();
        crate::ui_script::test_ui::load_tooltip_template(script);
    }
    c.eval::<()>(
        false,
        "Costs = {} \
         CreateFrame('GameTooltip', 'TT', nil, 'TestTooltipTemplate'):SetOwner(CreateFrame('Frame'), 'ANCHOR_NONE') \
         local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_MONEY') \
         f:SetScript('OnEvent', function() local _, cost = TT:SetBagItem(0, 1) table.insert(Costs, cost) end)",
    );
    c.frame(vec![
        vec![
            values(VENDOR, &[(FIELD_UNIT_FLAGS, 0x1000)]),
            values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 1)]),
        ],
        vec![
            values(VENDOR, &[(FIELD_UNIT_FLAGS, 0)]),
            values(ME, &[(FIELD_PLAYER_FIELD_COINAGE, 2)]),
        ],
    ]);
    assert_eq!(
        c.eval::<Vec<i64>>(false, "return Costs"),
        [i64::from(discounted), i64::from(full)]
    );
}
