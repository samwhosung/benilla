//! The trade verbs through the whole client's host: each sends its packet and changes the trade
//! inside its call, as the reference's bindings do (`0x4c06e0`, `0x48aa60`, `0x48aa70`,
//! `0x4c0820`, `0x48a120`, `0x4bfa70`); the window's open moves what the cursor holds only into a
//! trade started by a drop (`0x5d3f80` at `0x4bf5f1`); and `DropItemOnUnit` on another player
//! starts or fills a trade (`0x48d960`).

use benilla_protocol::messages::TradeStatus;
use benilla_protocol::ObjectFields;
use benilla_protocol::SessionEvent;
use benilla_ui::script::{UiScript, UnitGuids};
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
}

/// Our object with `extra` fields: `OBJECT_FIELD_TYPE` a player, alive, the human template,
/// player-controlled, and the two pack slots.
fn me_fields(extra: &[(u16, u32)]) -> ObjectFields {
    let mut pairs = vec![
        (2, 0x19),
        (22, 100),
        (35, fixture::HUMAN),
        (46, fixture::PC),
        // The purse the money verbs test against (`PLAYER_FIELD_COINAGE`).
        (benilla_protocol::field::FIELD_PLAYER_FIELD_COINAGE, 10_000),
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
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
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
    script.set_unit_guids(&UnitGuids {
        player: ME,
        target: BOB,
        ..Default::default()
    });
    script
        .run(
            r#"HEARD = ""
            local f = CreateFrame("Frame")
            for _, e in ipairs({"TRADE_ACCEPT_UPDATE", "PLAYER_TRADE_MONEY", "PLAYER_MONEY",
                    "TRADE_MONEY_CHANGED", "TRADE_SHOW", "TRADE_PLAYER_ITEM_CHANGED",
                    "TRADE_REPLACE_ENCHANT"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                HEARD = HEARD .. event .. "(" .. tostring(arg1) .. "," .. tostring(arg2) .. ") "
            end)"#,
        )
        .expect("the recorder installs");
    app.world_mut().insert_non_send_resource(script);
    Trade { app, rx }
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
    crate::target::record_error_lines(world);
    crate::target::release_click(world, benilla_ui::script::ClickMode::Select);
}

/// The refusals shown inside a click's call, then those queued, since the last read.
fn errors(t: &Trade) -> Vec<&'static str> {
    let world = t.app.world();
    let mut keys = crate::target::take_error_lines(world);
    keys.extend(
        world
            .resource::<crate::ui_action::UiErrorKeys>()
            .take()
            .into_iter()
            .map(|e| e.key),
    );
    keys
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

/// The server's `status`, through the handler table, then the lanes.
fn status(t: &mut Trade, status: TradeStatus) {
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![SessionEvent::TradeStatus { status }],
    );
    drain(t);
}

/// What the handlers left, applied, then every lane fired, as the drain fires them.
fn drain(t: &mut Trade) {
    t.app.world_mut().flush();
    crate::lanes::fire_in_order(t.app.world_mut());
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
            "AcceptTrade { flag: 0 }",
            "BeginTrade",
            "CancelTrade",
            "SetTradeGold { copper: 500 }",
            "InitiateTrade { target: 7 }",
            "RandomRoll { min: 1, max: 2 }",
        ]
    );
}

/// `0x4bf230(1)` signals `TRADE_ACCEPT_UPDATE` with both flags, ours first, inside the call
/// (`0x4bf260`); the lanes that follow do not announce the same change again.
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
    drain(&mut t);
    assert_eq!(
        heard(&mut t),
        "",
        "the lanes do not repeat the call's signal"
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
    assert!(
        sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeItem { trade_slot: 1, .. })),
        "a drop on the open partner fills the next slot"
    );
    assert_eq!(heard(&mut t), "TRADE_PLAYER_ITEM_CHANGED(2,nil) ");
    assert!(eval::<bool>(&mut t, "return CursorHasItem() == nil"));
}

/// `DropItemOnUnit` does its legs inside its call (`0x48d960`): on the open partner the held item
/// is placed and the cursor cleared before the script's next line (`0x48da50` → `0x4bfc50`), and
/// with no trade open `CMSG_INITIATE_TRADE` goes out ahead of a send the next line makes
/// (`0x48da62`).
#[test]
fn a_drop_on_a_player_trades_inside_its_call() {
    let mut t = client();
    open_with_bob(&mut t);
    assert_eq!(
        eval::<(bool, Option<String>)>(
            &mut t,
            "PickupContainerItem(0, 1) DropItemOnUnit('target') \
             return CursorHasItem() == nil, (GetTradePlayerItemInfo(1))"
        ),
        (true, Some("Linen Cloth".to_string())),
        "the next line reads the item placed"
    );
    assert_eq!(heard(&mut t), "TRADE_PLAYER_ITEM_CHANGED(1,nil) ");

    let mut t = client();
    run(
        &mut t,
        "PickupContainerItem(0, 1) DropItemOnUnit('target') RandomRoll(1, 2)",
    );
    assert!(
        matches!(
            sent(&t)[..],
            [
                ClientCommand::InitiateTrade { target: BOB },
                ClientCommand::RandomRoll { .. }
            ]
        ),
        "the ask goes out in call order"
    );
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

/// A placement whose send refuses (here for an item whose object went after the pickup) returns 0
/// from `0x4bfa70`
/// (`0x4bfab3`): the slot is not written or signalled and nothing is counted, and the cursor's
/// put-back unlocks the held item (`0x4c004a`), leaving the cursor empty.
#[test]
fn a_refused_placement_writes_nothing_and_drops_the_item_unlocked() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "PickupContainerItem(0, 1)");
    assert!(eval::<bool>(&mut t, "return CursorHasItem() == 1"), "held");
    let world = t.app.world_mut();
    if let Some(cloth) = world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .remove(&CLOTH)
    {
        world.despawn(cloth);
    }
    run(&mut t, "ClickTradeButton(1)");
    assert!(sent(&t).is_empty(), "nothing went out");
    assert_eq!(heard(&mut t), "", "no slot signal");
    assert_eq!(
        eval::<(bool, bool)>(
            &mut t,
            "return GetTradePlayerItemInfo(1) == nil, CursorHasItem() == nil"
        ),
        (true, true)
    );
    assert!(
        !t.app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(CLOTH),
        "the put-back unlocks it"
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
                flag: 7,
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
/// `PLAYER_TRADE_MONEY` once (`0x4bf630`), `PLAYER_MONEY` (`0x4bf63a`), then `TRADE_SHOW`; the cache of our money takes the
/// offer (`0x4bf62b`), so no later status fires it again.
#[test]
fn a_drop_started_trade_folds_held_coins_with_one_money_signal() {
    let mut t = client();
    run(&mut t, "PickupPlayerMoney(300)");
    let _ = heard(&mut t);
    t.app
        .world_mut()
        .resource_mut::<super::TradeSession>()
        .initiate(BOB, true);
    status(&mut t, TradeStatus::OpenWindow);
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) PLAYER_MONEY(nil,nil) TRADE_SHOW(nil,nil) "
    );
    assert!(sent(&t)
        .iter()
        .any(|c| matches!(c, ClientCommand::SetTradeGold { copper: 300 })));
    assert_eq!(
        eval::<(i64, i64)>(&mut t, "return GetCursorMoney(), GetPlayerTradeMoney()"),
        (0, 300)
    );
    drain(&mut t);
    assert_eq!(heard(&mut t), "", "the fold fires once");
}

/// The open leg sends before it clears the cursor: the coins' offer (`0x5d4500` at `0x4bf608`)
/// ahead of `ClearCursor(1, 0)` (`0x4bf61c`), an item's placement (`0x5d4380` at `0x4bf6be`) ahead
/// of `ClearCursor(0, 1)` (`0x4bf6ca`): nothing sends while the clear's `CURSOR_UPDATE` handler
/// waits, and that handler's packet follows the send, ahead of the leg's own signals.
#[test]
fn a_drop_started_trade_sends_its_offer_before_it_clears_the_cursor() {
    for pickup in ["PickupPlayerMoney(300)", "PickupContainerItem(0, 1)"] {
        let mut t = client();
        run(
            &mut t,
            r#"ORDER = "" local f = CreateFrame("Frame") f:RegisterEvent("CURSOR_UPDATE")
            f:RegisterEvent("PLAYER_TRADE_MONEY") f:RegisterEvent("TRADE_SHOW")
            f:SetScript("OnEvent", function()
                if not ROLL then return end
                ORDER = ORDER .. event .. " "
                if event == "CURSOR_UPDATE" then RandomRoll(9, 9) end
            end)"#,
        );
        run(&mut t, pickup);
        run(&mut t, "ROLL = 1");
        let _ = sent(&t);
        t.app
            .world_mut()
            .resource_mut::<super::TradeSession>()
            .initiate(BOB, true);
        benilla_ui::script::census::take();
        status(&mut t, TradeStatus::OpenWindow);
        assert_eq!(
            benilla_ui::script::census::take(),
            Default::default(),
            "{pickup}: nothing sends while the clear's handlers are held"
        );
        let order: Vec<&str> = sent(&t)
            .iter()
            .filter_map(|c| match c {
                ClientCommand::SetTradeGold { .. } | ClientCommand::SetTradeItem { .. } => {
                    Some("offer")
                }
                ClientCommand::RandomRoll { .. } => Some("roll"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["offer", "roll"], "{pickup}");
        let heard_order: String = eval(&mut t, "return ORDER");
        assert!(
            heard_order.starts_with("CURSOR_UPDATE "),
            "{pickup}: the clear's signal ahead of the leg's own: {heard_order}"
        );
        assert!(eval::<bool>(
            &mut t,
            "return CursorHasItem() == nil and GetCursorMoney() == 0"
        ));
    }
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
    let _ = heard(&mut t);
    click_on(&mut t, BOB);
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::InitiateTrade { target: BOB }]
    ));
    assert_eq!(eval::<i64>(&mut t, "return GetCursorMoney()"), 300);
    status(&mut t, TradeStatus::OpenWindow);
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) PLAYER_MONEY(nil,nil) TRADE_SHOW(nil,nil) "
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
    // The click goes on to select her (`0x49280c`); the trade leg sends nothing.
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::SetSelection { guid: ALICE }]
    ));
    assert_eq!(errors(&t), ["ERR_ALREADY_TRADING"]);
}

/// The click's own gates, each alone: a bag with an item in it (`0x4926f2`), a dead active
/// player (`0x492742`), a spell awaiting a target (`0x492668`) and an empty cursor
/// (`0x492662`) start nothing.
#[test]
fn the_click_refuses_a_full_bag_and_a_dead_player_and_skips_a_targeting_cursor() {
    let mut t = client();
    // Backpack slot 1 a bag with two of its four seats filled.
    {
        use benilla_protocol::field::FIELD_CONTAINER_SLOT_1;
        let world = t.app.world_mut();
        let cloth = world.resource::<crate::net::GuidIndex>().0[&CLOTH];
        let mut pairs = vec![(2, 0x7), (3, 2589), (14, 1), (48, 4)];
        for (i, guid) in [
            (0u16, 0x4000_0000_0000_0c01_u64),
            (2, 0x4000_0000_0000_0c02),
        ] {
            pairs.push((FIELD_CONTAINER_SLOT_1 + 2 * i, guid as u32));
            pairs.push((FIELD_CONTAINER_SLOT_1 + 2 * i + 1, (guid >> 32) as u32));
        }
        world
            .entity_mut(cloth)
            .insert(crate::net::ObjectStore(ObjectFields::from_pairs(&pairs)));
    }
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

/// `CMSG_ACCEPT_TRADE` carries the partner's flag `[0xc4be7c]` (`0x5d4260`): 1 from the window's
/// open (`0x5d4834`), then the second header dword of the partner's last extended status
/// (`0x5d49d5`), which vmangos fills with the slot count.
#[test]
fn an_accept_sends_the_partners_last_flag() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "AcceptTrade()");
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::AcceptTrade { flag: 1 }]
    ));
    partner_slot_7(&mut t, 0);
    run(&mut t, "AcceptTrade()");
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::AcceptTrade { flag: 7 }]
    ));
}

/// The partner's column with `gold` offered and no items.
fn partner_gold(t: &mut Trade, gold: u32) {
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![SessionEvent::TradeStatusExtended {
            state: Box::new(benilla_protocol::messages::TradeStatusExtended {
                their_window: true,
                sequence: 7,
                flag: 7,
                gold,
                enchant_spell_id: 0,
                slots: [None; benilla_protocol::messages::TRADE_SLOT_COUNT],
            }),
        }],
    );
    drain(t);
}

/// The money signals come from a status's compare (`0x4bf2b0`), not from our own edit: an offer
/// we set signals nothing until the next extended status finds it moved against the cache
/// (`0x4bf494`), then `PLAYER_TRADE_MONEY` and `PLAYER_MONEY`; the partner's moved offer is
/// `TRADE_MONEY_CHANGED` (`0x4bf4d6`); a status bouncing our coins (`0x4bf9fd` →
/// `0x4bfbd0(0xff)`) signals ours whatever the cache held.
#[test]
fn the_money_signals_follow_a_status_not_our_edit() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "SetTradeMoney(500)");
    drain(&mut t);
    assert_eq!(heard(&mut t), "", "our edit signals nothing");
    partner_gold(&mut t, 0);
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) PLAYER_MONEY(nil,nil) "
    );
    partner_gold(&mut t, 0);
    assert_eq!(heard(&mut t), "", "nothing moved");
    partner_gold(&mut t, 300);
    assert_eq!(heard(&mut t), "TRADE_MONEY_CHANGED(nil,nil) ");
    status(&mut t, TradeStatus::OnlyConjured { slot: 0xff });
    assert_eq!(
        heard(&mut t),
        "PLAYER_TRADE_MONEY(nil,nil) PLAYER_MONEY(nil,nil) "
    );
    assert_eq!(eval::<i64>(&mut t, "return GetPlayerTradeMoney()"), 0);
}

/// `0x4bf230` signals before it sends (`0x4bf260`, then `0x4bf26d`): a `TRADE_ACCEPT_UPDATE`
/// handler's own packet leaves ahead of the accept's, and an unchanged flag does neither.
#[test]
fn the_accept_signal_runs_before_its_packet() {
    let mut t = client();
    open_with_bob(&mut t);
    run(
        &mut t,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_ACCEPT_UPDATE")
        f:SetScript("OnEvent", function() RandomRoll(arg1, 9) end)"#,
    );
    run(&mut t, "AcceptTrade() AcceptTrade() CancelTradeAccept()");
    let sent: Vec<String> = sent(&t).into_iter().map(|c| format!("{c:?}")).collect();
    assert_eq!(
        sent,
        [
            "RandomRoll { min: 1, max: 9 }",
            "AcceptTrade { flag: 1 }",
            "RandomRoll { min: 0, max: 9 }",
            "UnacceptTrade",
        ]
    );
}

/// A drop on the open partner places inside an engine turn, where its call waits: a send that
/// refuses (`0x4bfab3`, here for an item whose object went after the pickup) still writes
/// nothing, signals nothing and leaves the item on the cursor, as `0x4bfc50` clears it only past
/// a placement that went (`0x4bfcd4`).
#[test]
fn a_refused_drop_placement_writes_nothing_and_keeps_the_cursor() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "PickupContainerItem(0, 1)");
    let world = t.app.world_mut();
    if let Some(cloth) = world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .remove(&CLOTH)
    {
        world.despawn(cloth);
    }
    run(&mut t, "DropItemOnUnit('target')");
    assert!(sent(&t).is_empty(), "nothing went out");
    assert_eq!(heard(&mut t), "", "no slot signal");
    assert_eq!(
        eval::<(bool, bool)>(
            &mut t,
            "return GetTradePlayerItemInfo(1) == nil, CursorHasItem() == 1"
        ),
        (true, true)
    );
}

/// `AcceptTrade` from Lua an engine turn runs: its call waits for the turn's end, and the signal
/// and the send follow the store wherever it applies (`0x4bf25a`, `0x4bf260`, `0x4bf26d`).
#[test]
fn an_accept_made_in_an_engine_turn_signals_and_sends_when_it_applies() {
    let mut t = client();
    open_with_bob(&mut t);
    fn accept_in_turn(mut vm: crate::script_calls::VmWith<Res<'static, super::TradeSession>>) {
        vm.turn(|script, _| {
            script
                .expect("a VM")
                .run("AcceptTrade() SEEN = HEARD")
                .expect("the chunk runs")
        });
    }
    let turn = t.app.world_mut().register_system(accept_in_turn);
    run_system(&mut t, turn);
    assert_eq!(eval::<String>(&mut t, "return SEEN"), "", "the call waited");
    assert_eq!(heard(&mut t), "TRADE_ACCEPT_UPDATE(1,0) ");
    assert!(matches!(
        sent(&t)[..],
        [ClientCommand::AcceptTrade { flag: 1 }]
    ));
}

/// `BACK_TO_TRADE` with both flags up (`0x4bf81c`, `0x4bf823`): our drop signals, then the
/// un-accept goes out, then the partner's drop signals; a handler's packet in the first signal
/// leaves ahead of the un-accept.
#[test]
fn back_to_trade_signals_ours_sends_then_signals_theirs() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "AcceptTrade()");
    status(&mut t, TradeStatus::Accept);
    let _ = sent(&t);
    let _ = heard(&mut t);
    run(
        &mut t,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_ACCEPT_UPDATE")
        f:SetScript("OnEvent", function() RandomRoll(arg2, 9) end)"#,
    );
    status(&mut t, TradeStatus::BackToTrade);
    assert_eq!(
        heard(&mut t),
        "TRADE_ACCEPT_UPDATE(0,1) TRADE_ACCEPT_UPDATE(0,0) "
    );
    let sent: Vec<String> = sent(&t).into_iter().map(|c| format!("{c:?}")).collect();
    assert_eq!(
        sent,
        [
            "RandomRoll { min: 1, max: 9 }",
            "UnacceptTrade",
            "RandomRoll { min: 0, max: 9 }",
        ]
    );
}

/// An extended status drops both accepts through `0x4bf230(0)` and `0x4bf280(0)` before its
/// compares (`0x4bf2b6`, `0x4bf2bd`): the signal, the un-accept, then the slot's own signal.
#[test]
fn an_extended_status_unaccepts_with_its_signal_and_send() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "AcceptTrade()");
    let _ = sent(&t);
    let _ = heard(&mut t);
    partner_gold(&mut t, 300);
    assert_eq!(
        heard(&mut t),
        "TRADE_ACCEPT_UPDATE(0,0) TRADE_MONEY_CHANGED(nil,nil) "
    );
    assert!(matches!(sent(&t)[..], [ClientCommand::UnacceptTrade]));
}

/// Codes 3 and 13 signal `PLAYER_MONEY` in the status handler, ahead of the dispatcher
/// (`0x5d4892`-`0x5d48a8`): `CANCELED` before its `TRADE_REQUEST_CANCEL` and the close, code 13
/// alone with the window left up. `COMPLETE` and `CLOSE_WINDOW` share only the clear
/// (`0x5d48eb`).
#[test]
fn canceled_and_code_13_signal_the_purse_ahead_of_the_dispatcher() {
    let mut t = client();
    run(
        &mut t,
        r#"ORDER = ""
        local f = CreateFrame("Frame")
        for _, e in ipairs({"PLAYER_MONEY", "TRADE_REQUEST_CANCEL", "TRADE_CLOSED"}) do
            f:RegisterEvent(e)
        end
        f:SetScript("OnEvent", function() ORDER = ORDER .. event .. " " end)"#,
    );
    let order = |t: &mut Trade| eval::<String>(t, "local o = ORDER ORDER = \"\" return o");
    open_with_bob(&mut t);
    let _ = order(&mut t);
    status(&mut t, TradeStatus::Unknown13);
    assert_eq!(order(&mut t), "PLAYER_MONEY ");
    assert!(t.app.world().resource::<super::TradeSession>().is_open());
    status(&mut t, TradeStatus::Canceled);
    assert!(
        order(&mut t).starts_with("PLAYER_MONEY TRADE_REQUEST_CANCEL TRADE_CLOSED "),
        "the purse first, then the arm and its close"
    );
    open_with_bob(&mut t);
    let _ = order(&mut t);
    status(&mut t, TradeStatus::Complete);
    assert!(!order(&mut t).contains("PLAYER_MONEY"));
}

/// The open leg's item path asks for the held item's guid and its source container's
/// (`0x4bf667`, `0x4bf674`): an item from bag 1 with no bag known in inventory slot 20 leaves the
/// cursor and the offer alone, and the world click asks no trade for it; with the bag seated
/// both go on. The bag is in the world either way; the doll's slot holds it only when seated.
#[test]
fn the_open_leg_takes_no_item_whose_bag_has_no_guid() {
    for seated in [false, true] {
        let mut t = client();
        {
            use benilla_protocol::field::{FIELD_CONTAINER_SLOT_1, FIELD_PLAYER_INV_SLOT_HEAD};
            const BAG: u64 = 0x4000_0000_0000_0b01;
            let world = t.app.world_mut();
            crate::items::test_spawn_item(
                world,
                BAG,
                ObjectFields::from_pairs(&[
                    (2, 0x7),
                    (3, 4496),
                    (14, 1),
                    (48, 4),
                    (FIELD_CONTAINER_SLOT_1, CLOTH as u32),
                    (FIELD_CONTAINER_SLOT_1 + 1, (CLOTH >> 32) as u32),
                ]),
                true,
            );
            let me = world.resource::<crate::net::GuidIndex>().0[&ME];
            let seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 19;
            world
                .get_mut::<crate::net::ObjectStore>(me)
                .expect("our store")
                .0
                .merge(ObjectFields::from_pairs(&[
                    (532, 0),
                    (533, 0),
                    (seat, BAG as u32),
                    (seat + 1, (BAG >> 32) as u32),
                ]));
        }
        with_hosted_vm(t.app.world_mut(), |vm| {
            let vm = vm.expect("a VM");
            if seated {
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[20] = Some(benilla_ui::script::InvSlotView {
                    item_id: 4496,
                    guid: 0x4000_0000_0000_0b01,
                    ..Default::default()
                });
                vm.set_inventory_slots(slots);
            }
        });
        run(&mut t, "PickupContainerItem(1, 1)");
        // The world click's trade leg asks the same of the payload (`0x49264d`).
        click_on(&mut t, BOB);
        assert_eq!(
            matches!(sent(&t)[..], [ClientCommand::InitiateTrade { .. }]),
            seated,
            "seated {seated}: the click"
        );
        t.app
            .world_mut()
            .resource_mut::<super::TradeSession>()
            .initiate(BOB, true);
        status(&mut t, TradeStatus::OpenWindow);
        let placed = sent(&t)
            .iter()
            .any(|c| matches!(c, ClientCommand::SetTradeItem { trade_slot: 0, .. }));
        assert_eq!(placed, seated, "seated {seated}");
        assert_eq!(
            eval::<bool>(&mut t, "return CursorHasItem() == 1"),
            !seated,
            "seated {seated}"
        );
    }
}

/// Our slot an own-side extended status empties signals first: the handler runs
/// `InvalidateTradeSlot` as it stores the records (`0x5d4ad3`, `0x5d4b33`), then `0x4bf2b0`
/// drops the accepts (`0x5d4b67` → `0x4bf2b6`).
#[test]
fn an_emptied_own_slot_signals_before_the_accept_drop() {
    let mut t = client();
    open_with_bob(&mut t);
    run(
        &mut t,
        "PickupContainerItem(0, 1) ClickTradeButton(1) AcceptTrade()",
    );
    let _ = sent(&t);
    let _ = heard(&mut t);
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![SessionEvent::TradeStatusExtended {
            state: Box::new(benilla_protocol::messages::TradeStatusExtended {
                their_window: false,
                sequence: 7,
                flag: 7,
                gold: 0,
                enchant_spell_id: 0,
                slots: [None; benilla_protocol::messages::TRADE_SLOT_COUNT],
            }),
        }],
    );
    drain(&mut t);
    assert_eq!(
        heard(&mut t),
        "TRADE_PLAYER_ITEM_CHANGED(1,nil) TRADE_ACCEPT_UPDATE(0,0) "
    );
}

/// Code 13 zeroes both offers before it signals `PLAYER_MONEY` (`0x5d489c`, `0x5d48a2`, then
/// `0x5d48a8`), so a handler reads an offer of 0.
#[test]
fn code_13_signals_the_purse_over_zeroed_offers() {
    let mut t = client();
    open_with_bob(&mut t);
    run(&mut t, "SetTradeMoney(500)");
    drain(&mut t);
    run(
        &mut t,
        r#"READ = nil
        local f = CreateFrame("Frame")
        f:RegisterEvent("PLAYER_MONEY")
        f:SetScript("OnEvent", function() READ = GetPlayerTradeMoney() end)"#,
    );
    assert_eq!(eval::<i64>(&mut t, "return GetPlayerTradeMoney()"), 500);
    status(&mut t, TradeStatus::Unknown13);
    assert_eq!(eval::<i64>(&mut t, "return READ"), 0);
}

/// `TRADE_UPDATE` has one producer, the getters' record callback `0x4c0340` (`0x4c034f`): an
/// offer or gold change signals its slot and money events and nothing else, and a getter that
/// finds an unanswered template is called back once per call when the record lands, to a handler
/// that reads it answered through the host. A getter that hits is never called back.
#[test]
fn trade_update_comes_only_from_a_landed_record_a_getter_asked_for() {
    use crate::item_watch::ItemNotice;
    const SWORD: u32 = 4306;
    let mut t = client();
    run(
        &mut t,
        r#"UPDATES, SLOTS, SEEN = 0, 0, {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_UPDATE")
        f:RegisterEvent("TRADE_TARGET_ITEM_CHANGED")
        f:SetScript("OnEvent", function()
            if event == "TRADE_UPDATE" then
                UPDATES = UPDATES + 1
                table.insert(SEEN, tostring((GetTradeTargetItemInfo(1))))
            else
                SLOTS = SLOTS + 1
            end
        end)"#,
    );
    open_with_bob(&mut t);
    let counts = |t: &mut Trade| eval::<(i64, i64)>(t, "return UPDATES, SLOTS");
    let offer = |t: &mut Trade, gold: u32| {
        let mut theirs = [None; benilla_protocol::messages::TRADE_SLOT_COUNT];
        theirs[0] = Some(benilla_protocol::messages::TradeItem {
            entry: SWORD,
            display_id: 0,
            count: 1,
            wrapped: false,
            gift_creator: 0,
            perm_enchant: 0,
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
                    flag: 7,
                    gold,
                    enchant_spell_id: 0,
                    slots: theirs,
                }),
            }],
        );
        drain(t);
    };
    offer(&mut t, 0);
    offer(&mut t, 900);
    assert_eq!(
        counts(&mut t),
        (0, 1),
        "the slot event once, and no TRADE_UPDATE for the offer or the gold"
    );

    // Two getter calls on the unanswered slot register two callbacks (`0x55bbb3`).
    run(
        &mut t,
        "GetTradeTargetItemInfo(1) GetTradeTargetItemInfo(1)",
    );
    drain(&mut t);
    assert_eq!(
        counts(&mut t).0,
        0,
        "nothing signals until the record lands"
    );

    // The record lands as the object handlers write it: the cache, then one notice per waiter.
    let waiting = {
        let mut items = t.app.world_mut().resource_mut::<crate::items::Items>();
        items.insert_template(SWORD, Some(crate::items::test_template("Iron Sword")));
        items.take_trade_waiters(SWORD)
    };
    assert_eq!(waiting, 2, "one per call");
    for _ in 0..waiting {
        t.app
            .world_mut()
            .resource_mut::<crate::item_lane::ItemLane>()
            .push(crate::item_watch::Run {
                notice: Some(ItemNotice::TradeRecord { entry: SWORD }),
                signals: Vec::new(),
                ..Default::default()
            });
    }
    drain(&mut t);
    assert_eq!(
        counts(&mut t),
        (2, 1),
        "one TRADE_UPDATE per registered callback, and no slot event: the landing changed the \
         row, not the offer (`0x4bf2b0` runs on a status)"
    );
    assert_eq!(
        eval::<Vec<String>>(&mut t, "return SEEN"),
        ["Iron Sword", "Iron Sword"],
        "the handler reads the answered row"
    );

    // Answered now, a getter asks nothing and nothing signals.
    run(&mut t, "GetTradeTargetItemInfo(1)");
    drain(&mut t);
    assert_eq!(counts(&mut t).0, 2);
    assert_eq!(
        t.app
            .world_mut()
            .resource_mut::<crate::items::Items>()
            .take_trade_waiters(SWORD),
        0
    );
}

/// The money and the item getters read the session at the call: our offer the instant
/// `SetTradeMoney` stores it, inside the same chunk, and a `TRADE_CLOSED` handler both offers as
/// the close left them, the money zeroed by the ending status (`0x5d4884`) and ours behind the
/// cleared cell (`0x4c0706`).
#[test]
fn the_getters_read_the_session_inside_the_calls_and_the_close() {
    let mut t = client();
    open_with_bob(&mut t);
    assert_eq!(
        eval::<i64>(&mut t, "SetTradeMoney(500) return GetPlayerTradeMoney()"),
        500
    );
    run(&mut t, "PickupContainerItem(0, 1) ClickTradeButton(1)");
    run(
        &mut t,
        r#"CLOSED = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_CLOSED")
        f:SetScript("OnEvent", function()
            table.insert(CLOSED, tostring((GetTradePlayerItemInfo(1))) .. "/" ..
                GetPlayerTradeMoney())
        end)"#,
    );
    status(&mut t, TradeStatus::Complete);
    let closed: Vec<String> = eval(&mut t, "return CLOSED");
    assert!(!closed.is_empty());
    assert!(
        closed.iter().all(|c| c == "Linen Cloth/0"),
        "the handler read {closed:?}"
    );
}

/// A close core run by a handler while the lane still holds an earlier packet's signals waits
/// behind them ([`super::lane::close_step`]): the accept, then the close and its line.
#[test]
fn a_close_fires_behind_the_lanes_earlier_items() {
    let mut t = client();
    open_with_bob(&mut t);
    run(
        &mut t,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_CLOSED")
        f:SetScript("OnEvent", function() HEARD = HEARD .. event .. " " end)"#,
    );
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![
            SessionEvent::TradeStatus {
                status: TradeStatus::Accept,
            },
            SessionEvent::TradeStatus {
                status: TradeStatus::Complete,
            },
        ],
    );
    t.app.world_mut().flush();
    assert_eq!(heard(&mut t), "", "nothing fired in the handlers");
    drain(&mut t);
    assert_eq!(
        heard(&mut t),
        "TRADE_ACCEPT_UPDATE(0,1) TRADE_CLOSED TRADE_CLOSED "
    );
}

/// One frame's `OPEN_WINDOW` then `CANCELED` on a trade a drop started: the open places the held
/// item at its packet (`0x4bf652`-`0x4bf6fa`, locked in our table), and the cancel after it
/// unlocks it (`0x4bf837`), shuts the cell (`0x4bf516`) and sends its `CMSG_CANCEL_TRADE` after
/// the placement's packet (`0x4bf527`): the item ends unlocked, and no longer in a trade.
#[test]
fn a_cancel_behind_a_drop_started_open_undoes_what_the_open_placed() {
    let mut t = client();
    run(&mut t, "PickupContainerItem(0, 1)");
    let _ = sent(&t);
    t.app
        .world_mut()
        .resource_mut::<super::TradeSession>()
        .initiate(BOB, true);
    crate::net::handlers::dispatch(
        t.app.world_mut(),
        vec![
            SessionEvent::TradeStatus {
                status: TradeStatus::OpenWindow,
            },
            SessionEvent::TradeStatus {
                status: TradeStatus::Canceled,
            },
        ],
    );
    drain(&mut t);
    let order: Vec<&str> = sent(&t)
        .iter()
        .filter_map(|c| match c {
            ClientCommand::SetTradeItem { .. } => Some("place"),
            ClientCommand::CancelTrade => Some("cancel"),
            _ => None,
        })
        .collect();
    // The partner holds the latch, so the release runs the core once more inside it (`0x493365`).
    assert_eq!(
        order,
        ["place", "cancel", "cancel"],
        "the cancels follow the placement"
    );
    assert!(
        !t.app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(CLOTH),
        "the cancel unlocks what the open placed"
    );
    // An entry's arrival unlocks the item unless it sits in an open trade (`0x4bfd20`), which
    // tests the cell first: shut, it frees the item the table still names.
    run(&mut t, "PickupContainerItem(0, 1)");
    let mut vm = t.app.world_mut().non_send_resource_mut::<UiScript>();
    vm.unlock_entry_items(&[CLOTH]);
    assert!(!vm.item_locked(CLOTH), "the window is shut");
}

/// `CLOSE_WINDOW` unlocks, prints its line, then runs the close core (`0x4bf87e`, `0x4bf88e`,
/// `0x4bf89c`): the line's message ahead of `TRADE_CLOSED`, the cancel's and the complete's after.
#[test]
fn close_window_prints_its_line_before_the_close() {
    let mut t = client();
    open_with_bob(&mut t);
    run(
        &mut t,
        r#"ERR_TRADE_BAG_FULL = "full" ERR_TRADE_CANCELLED = "cancelled"
        local f = CreateFrame("Frame")
        f:RegisterEvent("TRADE_CLOSED") f:RegisterEvent("UI_ERROR_MESSAGE")
        f:RegisterEvent("UI_INFO_MESSAGE")
        f:SetScript("OnEvent", function()
            HEARD = HEARD .. event .. "(" .. tostring(arg1) .. ") "
        end)"#,
    );
    status(
        &mut t,
        TradeStatus::CloseWindow {
            result: 4,
            side: 1,
            item_limit_category: 0,
        },
    );
    assert_eq!(
        heard(&mut t),
        "UI_ERROR_MESSAGE(full) TRADE_CLOSED(nil) TRADE_CLOSED(nil) "
    );
    open_with_bob(&mut t);
    status(&mut t, TradeStatus::Canceled);
    assert!(
        heard(&mut t).ends_with("TRADE_CLOSED(nil) UI_INFO_MESSAGE(cancelled) "),
        "the cancel's line follows its close"
    );
}

/// `InitiateTrade` with the partner's name cached prints `ERR_INITIATE_TRADE_S` inside its call
/// (`0x5d4042` → `0x496720`), a chat-surface line whose `CHAT_MSG_SYSTEM` the composer fires at
/// once (`0x49a870`), ahead of the chunk's next line.
#[test]
fn a_cached_initiate_prints_its_line_inside_the_call() {
    let mut t = client();
    run(
        &mut t,
        r#"ERR_INITIATE_TRADE_S = "Requested to trade with %s."
        local f = CreateFrame("Frame")
        f:RegisterEvent("CHAT_MSG_SYSTEM")
        f:SetScript("OnEvent", function() HEARD = HEARD .. event .. "(" .. arg1 .. ") " end)
        HEARD = ""
        InitiateTrade("target")
        HEARD = HEARD .. "next""#,
    );
    assert_eq!(
        heard(&mut t),
        "CHAT_MSG_SYSTEM(Requested to trade with Bob.) next"
    );
}
