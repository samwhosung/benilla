//! The trade verbs through the whole client's host: each sends its packet and changes the trade
//! inside its call, as the reference's bindings do (`0x4c06e0`, `0x48aa60`, `0x48aa70`,
//! `0x4c0820`, `0x48a120`, `0x4bfa70`); the window's open moves what the cursor holds only into a
//! trade started by a drop (`0x5d3f80` at `0x4bf5f1`); and `DropItemOnUnit` on another player
//! starts or fills a trade (`0x48d960`).

use benilla_protocol::messages::TradeStatus;
use benilla_protocol::ObjectFields;
use benilla_protocol::SessionEvent;
use benilla_ui::script::{ContainerSlot, ContainerState, UiScript, UnitGuids};
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use crate::net::{ClientCommand, NetCommands};
use crate::script_calls::with_hosted_vm;
use crate::target::relations::fixture;

/// Us and the player we trade with, both human.
const ME: u64 = 0x10;
const BOB: u64 = 0x7;
/// Our Linen Cloth in backpack slot 1, and our Tough Jerky in slot 2.
const CLOTH: u64 = 0x4000_0000_0000_0a01;
const JERKY: u64 = 0x4000_0000_0000_0a02;

struct Trade {
    app: App,
    rx: Receiver<ClientCommand>,
    /// The trade feed and the drop's system, registered once so their memories persist.
    feed: SystemId,
    drop: SystemId,
    click: SystemId,
}

/// Our object with `extra` fields: `OBJECT_FIELD_TYPE` a player, the human template,
/// player-controlled, and the two pack slots.
fn me_fields(extra: &[(u16, u32)]) -> ObjectFields {
    let mut pairs = vec![
        (2, 0x19),
        (35, fixture::HUMAN),
        (46, fixture::PC),
        (532, CLOTH as u32),
        (533, (CLOTH >> 32) as u32),
        (534, JERKY as u32),
        (535, (JERKY >> 32) as u32),
    ];
    pairs.extend_from_slice(extra);
    ObjectFields::from_pairs(&pairs)
}

/// The whole client, headless, with us and Bob streamed as players of one faction group, Bob
/// selected, two items in our backpack and 1g in the purse.
fn client() -> Trade {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    app.insert_resource(fixture::factions());
    let world = app.world_mut();
    let me = crate::net::ObjectStore(me_fields(&[]));
    let me = world
        .spawn((crate::net::SelfPlayer, crate::net::Guid(ME), me))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.insert_resource(crate::net::SelfGuid(Some(ME)));
    crate::items::test_spawn_object(
        world,
        BOB,
        fixture::store_of(&fixture::player(BOB, fixture::HUMAN)).0,
    );
    for (guid, entry) in [(CLOTH, 2589), (JERKY, 117)] {
        crate::items::test_spawn_item(
            world,
            guid,
            ObjectFields::from_pairs(&[(3, entry), (14, 1)]),
            false,
        );
    }
    let mut items = world.resource_mut::<crate::items::Items>();
    items.insert_template(2589, Some(crate::items::test_template("Linen Cloth")));
    items.insert_template(117, Some(crate::items::test_template("Tough Jerky")));
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(BOB, "Bob".to_string(), None);
    world.resource_mut::<crate::target::Selection>().guid = Some(BOB);
    let mut script = UiScript::new().expect("a VM");
    script.set_money(10_000);
    script.set_unit_guids(&UnitGuids {
        player: ME,
        target: BOB,
        ..Default::default()
    });
    let mut bag = ContainerState {
        num_slots: 16,
        ..Default::default()
    };
    for (slot, guid, item_id, name) in [
        (1, CLOTH, 2589, "Linen Cloth"),
        (2, JERKY, 117, "Tough Jerky"),
    ] {
        bag.slots.insert(
            slot,
            ContainerSlot {
                item_id,
                guid,
                count: 1,
                link: Some(format!("|cffffffff|Hitem:{item_id}:0:0:0|h[{name}]|h|r")),
                ..Default::default()
            },
        );
    }
    script.set_container(0, Some(bag));
    script
        .run(
            r#"HEARD = ""
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TRADE_ACCEPT_UPDATE", "PLAYER_TRADE_MONEY", "TRADE_SHOW",
                    "TRADE_PLAYER_ITEM_CHANGED", "TRADE_REPLACE_ENCHANT"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                HEARD = HEARD .. event .. "(" .. tostring(arg1) .. "," .. tostring(arg2) .. ") "
            end)"#,
        )
        .expect("the recorder installs");
    app.world_mut().insert_non_send_resource(script);
    let feed = app.world_mut().register_system(super::feed_trade);
    let drop = app
        .world_mut()
        .register_system(crate::ui_action::drop_item::drop_item_on_unit);
    let click = app
        .world_mut()
        .register_system(crate::ui_action::drop_item::trade_on_world_click);
    Trade {
        app,
        rx,
        feed,
        drop,
        click,
    }
}

/// A left click on `guid`'s model, through the world click's trade leg.
fn click_on(t: &mut Trade, guid: u64) {
    let world = t.app.world_mut();
    let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
    world.resource_mut::<crate::target::PressPick>().hovered = crate::target::Hovered {
        target: Some(entity),
        guid: Some(guid),
        ..Default::default()
    };
    world
        .resource_mut::<Messages<benilla_world::interact::WorldClick>>()
        .write(benilla_world::interact::WorldClick);
    let click = t.click;
    run_system(t, click);
}

/// The refusals queued since the last read.
fn errors(t: &Trade) -> Vec<&'static str> {
    t.app
        .world()
        .resource::<crate::ui_action::UiErrorKeys>()
        .take()
        .into_iter()
        .map(|e| e.key)
        .collect()
}

fn run(t: &mut Trade, lua: &str) {
    with_hosted_vm(t.app.world_mut(), |vm| vm.expect("a VM").run(lua)).expect("the chunk runs");
}

fn eval<T: benilla_ui::mlua::FromLuaMulti>(t: &mut Trade, lua: &str) -> T {
    with_hosted_vm(t.app.world_mut(), |vm| vm.expect("a VM").eval::<T>(lua))
        .expect("the chunk runs")
}

/// The events the recorder heard since the last read.
fn heard(t: &mut Trade) -> String {
    eval(t, "local h = HEARD HEARD = \"\" return h")
}

/// The packets sent since the last read.
fn sent(t: &Trade) -> Vec<ClientCommand> {
    t.rx.try_iter().collect()
}

fn run_system(t: &mut Trade, id: SystemId) {
    t.app.world_mut().run_system(id).expect("the system runs");
}

/// The server's `status`, through the handler table, then the feed.
fn status(t: &mut Trade, status: TradeStatus) {
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![SessionEvent::TradeStatus { status }],
    );
    t.app.world_mut().flush();
    let feed = t.feed;
    run_system(t, feed);
}

/// A trade Bob asked for and we accepted, its window open.
fn open_with_bob(t: &mut Trade) {
    t.app
        .world_mut()
        .resource_mut::<super::TradeSession>()
        .begin(BOB);
    status(t, TradeStatus::OpenWindow);
    let _ = sent(t);
    let _ = heard(t);
}

/// `AcceptTrade` (`0x4c06e0`), `BeginTrade` (`0x48aa60`), `CancelTrade` (`0x48aa70`),
/// `SetTradeMoney` (`0x4c0820` → `0x5d4680`) and `InitiateTrade` (`0x48a120` → `0x5d3fb0`) each
/// send inside the call, so a chunk's packets leave in call order, `RandomRoll`'s among them.
#[test]
fn the_trade_verbs_send_inside_their_calls() {
    let mut t = client();
    run(
        &mut t,
        "AcceptTrade() BeginTrade() CancelTrade() SetTradeMoney(500) \
         InitiateTrade('target') RandomRoll(1, 2)",
    );
    let sent: Vec<String> = sent(&t)
        .into_iter()
        .map(|c| format!("{c:?}"))
        .filter(|c| !c.starts_with("NameQuery"))
        .collect();
    assert_eq!(
        sent,
        [
            "AcceptTrade",
            "BeginTrade",
            "CancelTrade",
            "SetTradeGold { copper: 500 }",
            "InitiateTrade { target: 7 }",
            "RandomRoll { min: 1, max: 2 }",
        ]
    );
}

/// `0x4bf230(1)` signals `TRADE_ACCEPT_UPDATE` with both flags, ours first, inside the call
/// (`0x4bf260`); the feed that follows does not announce the same change again.
#[test]
fn an_accept_signals_inside_its_call_once() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "AcceptTrade() SEEN = HEARD");
    assert_eq!(
        eval::<String>(&mut t, "return SEEN"),
        "TRADE_ACCEPT_UPDATE(1,0) "
    );
    let _ = heard(&mut t);
    let feed = t.feed;
    run_system(&mut t, feed);
    assert_eq!(
        heard(&mut t),
        "",
        "the feed does not repeat the call's signal"
    );
}

/// `ClickTradeButton` sends `CMSG_SET_TRADE_ITEM` and `CMSG_CLEAR_TRADE_ITEM` inside its call
/// (`0x4bfa70`), and `AddTradeMoney` (`0x4c07f0` → `0x5d4500`) its `CMSG_SET_TRADE_GOLD`.
#[test]
fn a_slot_click_and_the_money_add_send_inside_their_calls() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "PickupContainerItem(0, 1) ClickTradeButton(1)");
    assert!(
        matches!(
            sent(&t)[..],
            [ClientCommand::SetTradeItem { trade_slot: 0, .. }]
        ),
        "the placement goes out in the call"
    );
    run(&mut t, "ClickTradeButton(1) ClearCursor()");
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::ClearTradeItem { trade_slot: 0 }]
    ));
    run(&mut t, "PickupPlayerMoney(300) AddTradeMoney()");
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::SetTradeGold { copper: 300 }]
    ));
}

/// A trade another player asked for opens with the coins still on the cursor and the offer at 0:
/// the open leg moves them only for a trade we started by a drop (`0x5d3f80`, `0x4bf5f8`).
#[test]
fn held_coins_stay_on_the_cursor_when_a_trade_opens_that_no_drop_started() {
    let mut t = client();
    run(&mut t, "PickupPlayerMoney(300)");
    t.app
        .world_mut()
        .resource_mut::<super::TradeSession>()
        .begin(BOB);
    status(&mut t, TradeStatus::OpenWindow);
    assert_eq!(
        eval::<(i64, i64)>(&mut t, "return GetCursorMoney(), GetPlayerTradeMoney()"),
        (300, 0)
    );
    assert!(
        !sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeGold { .. })),
        "nothing is offered"
    );
}

/// `DropItemOnUnit("target")` on another player of our faction group with no trade open asks to
/// trade (`0x48da62`) and keeps the item on the cursor; the window's open then puts it in slot 1
/// (`0x4bf6be`) and clears the cursor keeping the lock (`0x4bf6ca`). A second drop on the open
/// partner fills the next empty slot (`0x4bfc50`) and signals it (`0x4bfaef`).
#[test]
fn an_item_dropped_on_a_player_starts_and_fills_a_trade() {
    let mut t = client();
    run(&mut t, "PickupContainerItem(0, 1) DropItemOnUnit('target')");
    let drop = t.drop;
    run_system(&mut t, drop);
    assert!(
        matches!(
            sent(&t)[..],
            [ClientCommand::InitiateTrade { target: BOB }, ..]
        ),
        "the drop asks to trade"
    );
    assert!(eval::<bool>(&mut t, "return CursorHasItem() == 1"));

    status(&mut t, TradeStatus::OpenWindow);
    assert!(
        sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeItem { trade_slot: 0, .. })),
        "the open places the held item in slot 1"
    );
    assert_eq!(
        eval::<(bool, String)>(
            &mut t,
            "return CursorHasItem() == nil, (GetTradePlayerItemInfo(1))"
        ),
        (true, "Linen Cloth".to_string())
    );
    let _ = heard(&mut t);

    run(&mut t, "PickupContainerItem(0, 2) DropItemOnUnit('target')");
    let drop = t.drop;
    run_system(&mut t, drop);
    assert!(
        sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeItem { trade_slot: 1, .. })),
        "a drop on the open partner fills the next slot"
    );
    assert_eq!(heard(&mut t), "TRADE_PLAYER_ITEM_CHANGED(2,nil) ");
    assert!(eval::<bool>(&mut t, "return CursorHasItem() == nil"));
}

/// No sender tests the window (`0x5d4680`, `0x4bfa70`, `0x5d4380`, `0x5d4480`): money, a placement
/// and a clear go out with none open, and vmangos drops them (`TradeHandler.cpp:668-735`). Each
/// edit sent moves the own-edit counter (`0x5d4442`, `0x5d44bc`, `0x5d4712`), two money verbs in
/// one chunk included.
#[test]
fn each_offer_edit_goes_out_with_or_without_a_window_and_is_counted() {
    let mut t = client();
    let edits = |t: &Trade| t.app.world().resource::<super::TradeSession>().own_edits;
    run(
        &mut t,
        "SetTradeMoney(500) PickupContainerItem(0, 1) ClickTradeButton(1) ClickTradeButton(1) \
         ClearCursor() SetTradeMoney(777)",
    );
    let sent: Vec<String> = sent(&t).into_iter().map(|c| format!("{c:?}")).collect();
    assert_eq!(
        sent,
        [
            "SetTradeGold { copper: 500 }",
            "SetTradeItem { trade_slot: 0, bag: 255, slot: 23 }",
            "ClearTradeItem { trade_slot: 0 }",
            "SetTradeGold { copper: 777 }",
        ]
    );
    assert_eq!(edits(&t), 4);
    open_with_bob(&mut t);
    assert_eq!(edits(&t), 1, "the open starts the count at 1");
    run(
        &mut t,
        "SetTradeMoney(5) PickupContainerItem(0, 1) ClickTradeButton(1)",
    );
    assert_eq!(edits(&t), 3);
}

/// A placement whose send refuses (here a bag slot with no wire form) returns 0 from `0x4bfa70`
/// (`0x4bfab3`): the slot is not written or signalled and nothing is counted, and the cursor's
/// put-back unlocks the held item (`0x4c004a`), leaving the cursor empty.
#[test]
fn a_refused_placement_writes_nothing_and_drops_the_item_unlocked() {
    let mut t = client();
    open_with_bob(&mut t);
    // Slot 30 of the backpack, which has no wire position.
    with_hosted_vm(t.app.world_mut(), |vm| {
        let vm = vm.expect("a VM");
        let mut bag = ContainerState {
            num_slots: 32,
            ..Default::default()
        };
        bag.slots.insert(
            30,
            ContainerSlot {
                item_id: 2589,
                guid: CLOTH,
                count: 1,
                ..Default::default()
            },
        );
        vm.set_container(0, Some(bag));
    });
    run(&mut t, "PickupContainerItem(0, 30) ClickTradeButton(1)");
    assert!(sent(&t).is_empty(), "nothing went out");
    assert_eq!(heard(&mut t), "", "no slot signal");
    assert_eq!(
        eval::<(bool, bool, bool)>(
            &mut t,
            "local _, _, l = GetContainerItemInfo(0, 30) \
             return GetTradePlayerItemInfo(1) == nil, CursorHasItem() == nil, l == nil"
        ),
        (true, true, true)
    );
    assert_eq!(
        t.app.world().resource::<super::TradeSession>().own_edits,
        1,
        "an edit that never went out is not counted"
    );
}

/// An enchant spell (effect 53, `SPELL_EFFECT_ENCHANT_ITEM`) on the targeting cursor, its word
/// wanting an item, with the enchant rows it and the partner's item name.
const ENCHANT_SPELL: u32 = 7418;
const CRUSADER: u32 = 1900;
const MINOR_STAMINA: u32 = 66;

fn enchanting(t: &mut Trade) {
    let world = t.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(
            [(
                ENCHANT_SPELL,
                benilla_formats::SpellDisplay {
                    effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM, 0, 0],
                    effect_misc_value: [MINOR_STAMINA as i32, 0, 0],
                    ..Default::default()
                },
            )]
            .into(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    world.insert_resource(crate::items::Enchants(
        benilla_formats::EnchantCatalog::from_rows(
            std::collections::HashMap::new(),
            [
                (CRUSADER, "Crusader".to_string()),
                (MINOR_STAMINA, "Minor Stamina".to_string()),
            ]
            .into_iter()
            .collect(),
            [(CRUSADER, 0), (MINOR_STAMINA, 0)].into_iter().collect(),
        ),
    ));
    world.resource_mut::<crate::spell::SpellTargeting>().enter(
        ENCHANT_SPELL,
        crate::spell::CastCommit::Spell,
        0x0010,
    );
}

/// The partner's window with `item` in its slot 7, the non-traded slot.
fn partner_slot_7(t: &mut Trade, perm_enchant: u32) {
    let mut slots = [None; benilla_protocol::messages::TRADE_SLOT_COUNT];
    slots[6] = Some(benilla_protocol::messages::TradeItem {
        entry: 2589,
        display_id: 0,
        count: 1,
        wrapped: false,
        gift_creator: 0,
        perm_enchant,
        creator: 0,
        charges: 0,
        suffix_factor: 0,
        random_prop_id: 0,
        lock_id: 0,
        max_durability: 0,
        durability: 0,
    });
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![SessionEvent::TradeStatusExtended {
            state: Box::new(benilla_protocol::messages::TradeStatusExtended {
                their_window: true,
                sequence: 7,
                gold: 0,
                enchant_spell_id: 0,
                slots,
            }),
        }],
    );
}

/// The casts at the partner's slot sent since the last read, and whether the cursor still waits.
fn trade_casts(t: &mut Trade) -> (Vec<(u32, u8)>, bool) {
    let casts = sent(t)
        .into_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpellTradeItem { spell_id, slot } => Some((spell_id, slot)),
            _ => None,
        })
        .collect();
    let waiting = t
        .app
        .world()
        .resource::<crate::spell::SpellTargeting>()
        .active();
    (casts, waiting)
}

/// `ClickTargetTradeButton(7)` with an enchant on the cursor (`0x4c00dd` → `0x496170`): over an
/// enchant already on the partner's item it signals `TRADE_REPLACE_ENCHANT` with the old name and
/// the new one (`0x496230`) and casts nothing; `ReplaceTradeEnchant` (`0x48d330`) then binds the
/// slot and sends the cast at trade slot 6 (`0x6e6900`), and the cursor is down.
#[test]
fn an_enchant_over_the_partners_enchant_asks_then_casts() {
    let mut t = client();
    open_with_bob(&mut t);
    partner_slot_7(&mut t, CRUSADER);
    enchanting(&mut t);
    run(&mut t, "ClickTargetTradeButton(7)");
    assert_eq!(
        heard(&mut t),
        "TRADE_REPLACE_ENCHANT(Crusader,Minor Stamina) "
    );
    assert_eq!(
        trade_casts(&mut t),
        (vec![], true),
        "the cursor waits for the answer"
    );
    run(&mut t, "ReplaceTradeEnchant()");
    assert_eq!(trade_casts(&mut t), (vec![(ENCHANT_SPELL, 6)], false));
}

/// With nothing on the partner's item the clash check binds at once (`0x496211`); another slot
/// of their column, or an empty slot 7, casts nothing (`0x4c00db`, `0x496175`).
#[test]
fn an_enchant_on_the_partners_bare_item_casts_at_once() {
    let mut t = client();
    open_with_bob(&mut t);
    enchanting(&mut t);
    run(&mut t, "ClickTargetTradeButton(7)");
    assert_eq!(trade_casts(&mut t), (vec![], true), "slot 7 is empty");
    partner_slot_7(&mut t, 0);
    run(&mut t, "ClickTargetTradeButton(1)");
    assert_eq!(trade_casts(&mut t), (vec![], true), "slot 1 runs no check");
    run(&mut t, "ClickTargetTradeButton(7)");
    assert_eq!(heard(&mut t), "");
    assert_eq!(trade_casts(&mut t), (vec![(ENCHANT_SPELL, 6)], false));
}

/// A trade we started by dropping coins (`0x5d3f80`) folds them in as it opens and fires
/// `PLAYER_TRADE_MONEY` once (`0x4bf630`), then `TRADE_SHOW`; the cache of our money takes the
/// offer (`0x4bf62b`), so no later feed fires it again.
#[test]
fn a_drop_started_trade_folds_held_coins_with_one_money_signal() {
    let mut t = client();
    run(&mut t, "PickupPlayerMoney(300)");
    t.app
        .world_mut()
        .resource_mut::<super::TradeSession>()
        .initiate(BOB, true);
    status(&mut t, TradeStatus::OpenWindow);
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) TRADE_SHOW(nil,nil) "
    );
    assert!(sent(&t)
        .iter()
        .any(|c| matches!(c, ClientCommand::SetTradeGold { copper: 300 })));
    assert_eq!(
        eval::<(i64, i64)>(&mut t, "return GetCursorMoney(), GetPlayerTradeMoney()"),
        (0, 300)
    );
    let feed = t.feed;
    run_system(&mut t, feed);
    assert_eq!(heard(&mut t), "", "the feed's money diff saw the fold");
}

/// `CloseTrade` (`0x4bfdc0`) signals `PLAYER_TRADE_MONEY` while the done flag is 0, which every
/// close leaves while the partner holds the latch: the release's inner run stores 0 over the
/// `COMPLETE` arm's 1 (`0x4bf4fd`, `0x493363`).
#[test]
fn close_trade_signals_the_money_after_any_close() {
    let mut t = client();
    open_with_bob(&mut t);
    status(&mut t, TradeStatus::Complete);
    let _ = heard(&mut t);
    run(&mut t, "CloseTrade()");
    assert_eq!(heard(&mut t), "PLAYER_TRADE_MONEY(nil,nil) ");
}

/// A left click on another player with coins held asks to trade in the cursor-drop mode
/// (`0x49278e`), keeping the coins; the window's open then folds them (`0x4bf5f1`).
#[test]
fn a_click_on_a_player_with_coins_starts_a_trade_that_folds_them() {
    let mut t = client();
    run(&mut t, "PickupPlayerMoney(300)");
    click_on(&mut t, BOB);
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::InitiateTrade { target: BOB }]
    ));
    assert_eq!(eval::<i64>(&mut t, "return GetCursorMoney()"), 300);
    status(&mut t, TradeStatus::OpenWindow);
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) TRADE_SHOW(nil,nil) "
    );
    assert!(sent(&t)
        .iter()
        .any(|c| matches!(c, ClientCommand::SetTradeGold { copper: 300 })));
    assert_eq!(
        eval::<(i64, i64)>(&mut t, "return GetCursorMoney(), GetPlayerTradeMoney()"),
        (0, 300)
    );
}

/// On our open partner a held item goes to a slot through `0x4bfc50` (`0x49276b`), and coins
/// do nothing there; another player with our trade open is `ERR_ALREADY_TRADING` (`0x492777`).
#[test]
fn a_click_on_the_partner_offers_the_item_and_on_anyone_else_refuses() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "PickupContainerItem(0, 1)");
    click_on(&mut t, BOB);
    assert!(
        sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeItem { trade_slot: 0, .. })),
        "the click places the held item"
    );
    assert_eq!(heard(&mut t), "TRADE_PLAYER_ITEM_CHANGED(1,nil) ");
    assert!(eval::<bool>(&mut t, "return CursorHasItem() == nil"));

    run(&mut t, "PickupPlayerMoney(300)");
    click_on(&mut t, BOB);
    assert!(sent(&t).is_empty(), "coins take no slot");
    assert_eq!(eval::<i64>(&mut t, "return GetCursorMoney()"), 300);

    const ALICE: u64 = 0x8;
    crate::items::test_spawn_object(
        t.app.world_mut(),
        ALICE,
        fixture::store_of(&fixture::player(ALICE, fixture::HUMAN)).0,
    );
    click_on(&mut t, ALICE);
    assert!(sent(&t).is_empty());
    assert_eq!(errors(&t), ["ERR_ALREADY_TRADING"]);
}

/// The click's own gates, each alone: a bag with an item in it (`0x4926f2`), a dead active
/// player (`0x492742`), a spell awaiting a target (`0x492668`) and an empty cursor
/// (`0x492662`) start nothing.
#[test]
fn the_click_refuses_a_full_bag_and_a_dead_player_and_skips_a_targeting_cursor() {
    let mut t = client();
    // Backpack slot 1 a bag with two of its seats filled.
    with_hosted_vm(t.app.world_mut(), |vm| {
        let mut bag = ContainerState {
            num_slots: 16,
            ..Default::default()
        };
        bag.slots.insert(
            1,
            ContainerSlot {
                item_id: 2589,
                guid: CLOTH,
                count: 1,
                occupied_seats: Some(2),
                ..Default::default()
            },
        );
        bag.slots.insert(
            2,
            ContainerSlot {
                item_id: 117,
                guid: JERKY,
                count: 1,
                ..Default::default()
            },
        );
        vm.expect("a VM").set_container(0, Some(bag));
    });
    run(&mut t, "PickupContainerItem(0, 1)");
    click_on(&mut t, BOB);
    assert!(sent(&t).is_empty());
    assert_eq!(errors(&t), ["ERR_TRADE_BAG"]);
    run(&mut t, "ClearCursor()");

    run(&mut t, "PickupContainerItem(0, 2)");
    t.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(1, crate::spell::CastCommit::Spell, 0x0010);
    click_on(&mut t, BOB);
    assert!(sent(&t).is_empty());
    assert!(errors(&t).is_empty(), "the cursor's click is the cast's");
    *t.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>() = Default::default();

    // `UNIT_FIELD_HEALTH` 0 of a `UNIT_FIELD_MAXHEALTH` 100: dead.
    let me = t.app.world().resource::<crate::net::GuidIndex>().0[&ME];
    t.app
        .world_mut()
        .entity_mut(me)
        .insert(crate::net::ObjectStore(me_fields(&[(22, 0), (28, 100)])));
    click_on(&mut t, BOB);
    assert!(sent(&t).is_empty());
    assert_eq!(errors(&t), ["ERR_PLAYER_DEAD"]);

    run(&mut t, "ClearCursor()");
    click_on(&mut t, BOB);
    assert!(sent(&t).is_empty() && errors(&t).is_empty(), "nothing held");
}
