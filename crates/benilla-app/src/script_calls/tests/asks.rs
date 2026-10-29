//! The getters' lookups with a callback, each through the whole client's host: a getter that
//! misses asks the cache inside its call, so the query goes out ahead of the chunk's next verb
//! (`RandomRoll`, `0x48c832`), as the reference's lookup sends it on the miss (`0x556aa0`).

use benilla_protocol::messages::{TradeItem, TradeStatusExtended, TRADE_SLOT_COUNT};
use benilla_ui::script::{UiScript, UnitState};
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::systems::{client, eval, NPC};
use crate::net::ClientCommand;

/// What went out, in order, of the kinds these chunks send.
fn sent(rx: &Receiver<ClientCommand>) -> Vec<String> {
    rx.try_iter()
        .filter_map(|c| {
            Some(match c {
                ClientCommand::RandomRoll { min, .. } => format!("roll {min}"),
                ClientCommand::NameQuery { guid } => format!("name {guid:#x}"),
                ClientCommand::ItemQuery { entry, guid } => format!("item {entry} {guid:#x}"),
                ClientCommand::GuildQuery { guild_id } => format!("guild {guild_id}"),
                ClientCommand::CreatureQuery { entry, guid } => {
                    format!("creature {entry} {guid:#x}")
                }
                _ => return None,
            })
        })
        .collect()
}

fn vm(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// The active player's guid, which the ammo and cooldown lookups pass as their owner id.
const ME: u64 = 0x2a;

/// The active player: its object, which the inventory getters find held (`0x4944af`), and the
/// VM's guid, as the unit feeds seat it.
fn seat_player(app: &mut App) {
    seat_player_with(app, &[]);
}

/// [`seat_player`], its object holding `pairs`.
fn seat_player_with(app: &mut App, pairs: &[(u16, u32)]) {
    let world = app.world_mut();
    let me = world
        .spawn((
            crate::net::Guid(ME),
            crate::net::SelfPlayer,
            crate::net::ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(pairs)
                    .into_created(benilla_protocol::messages::ObjectType::Player),
            ),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    vm(app).set_unit_guids(&benilla_ui::script::UnitGuids {
        player: ME,
        ..Default::default()
    });
}

/// The owner id the profession and trainer lookups pass: `0xb0000000` over a spell id (`cdq; or
/// edx,0xb0000000`), the row's at a profession (`0x4f75cf`-`0x4f75d2`, `0x4fdb8a`-`0x4fdb99`), the
/// taught spell's at a trainer (`0x4d9074`-`0x4d9087`).
fn spell_owner(spell_id: u32) -> u64 {
    0xb000_0000_0000_0000 | u64::from(spell_id)
}

/// `UnitName` on a unit with no object reads the player-name cache by guid with a callback
/// (`0x5171eb`), which sends `CMSG_NAME_QUERY` on the miss inside the call, once while it is in
/// flight.
#[test]
fn unit_name_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
    // The target the resolver reads (`0x515a86`), and its view.
    app.world_mut()
        .resource_mut::<crate::target::Selection>()
        .guid = Some(0x77);
    vm(&mut app).set_unit(
        "target",
        Some(UnitState {
            guid: 0x77,
            ..Default::default()
        }),
    );
    eval::<()>(
        &mut app,
        r#"UnitName("target") RandomRoll(1, 2) UnitName("target")"#,
    );
    assert_eq!(sent(&rx), ["name 0x77", "roll 1"]);
}

/// A trade with the partner `0x88` open, `theirs` in his slot 1 and `ours` in our slot 2, as the
/// session holds them: the getters read it at the call.
fn open_trade(app: &mut App, theirs: TradeItem, ours: TradeItem) {
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_trade::TradeSession>()
        .begin(0x88);
    crate::ui_trade::open_window(world, None);
    let mut slots = [None; TRADE_SLOT_COUNT];
    slots[0] = Some(theirs);
    let status = TradeStatusExtended {
        their_window: true,
        sequence: 0,
        flag: 0,
        gold: 0,
        enchant_spell_id: 0,
        slots,
    };
    let mut trade = world.resource_mut::<crate::ui_trade::TradeSession>();
    trade.set_offer(&status, &mut crate::ui_trade::TradeLane::default());
    trade.place_own_item(2, ours);
}

/// A trade slot's block holding `count` of `entry`.
fn trade_item(entry: u32, count: u32) -> TradeItem {
    TradeItem {
        entry,
        display_id: 0,
        count,
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
    }
}

/// A trade getter that finds a filled slot whose template the cache lacks asks the item cache
/// with the callback `0x4c0340` inside its call (`0x4c017f`, `0x4c0582`), so the query goes out
/// ahead of the chunk's next verb, and the callback waits for the record.
#[test]
fn a_trade_getter_asks_a_missed_template_inside_its_call() {
    let (mut app, rx) = client();
    // The open sets the partner cell `[0xb71728]` (`0x4bf549`), then the statuses fill the offers.
    open_trade(&mut app, trade_item(4306, 1), trade_item(2592, 1));
    seat_player(&mut app);
    eval::<()>(
        &mut app,
        "GetTradeTargetItemInfo(1) RandomRoll(1, 2) GetTradePlayerItemInfo(2)",
    );
    // The partner's slot passes the partner (`0x4c016c`-`0x4c017c`), ours the active player
    // (`0x4c0573`).
    assert_eq!(sent(&rx), ["item 4306 0x88", "roll 1", "item 2592 0x2a"]);
}

/// A trade slot whose record the cache already holds answers in the getter's call, though the
/// window's open had it uncached: the getter reads the cache as it runs (`0x4c017f`, `0x4c0582`),
/// so it asks nothing and waits on no callback, and the slot never reads empty.
#[test]
fn a_trade_getter_answers_a_cached_record_inside_its_call() {
    let (mut app, rx) = client();
    open_trade(&mut app, trade_item(4306, 3), trade_item(2592, 1));
    seat_player(&mut app);
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(4306, Some(crate::items::test_template("Silk Cloth")));
    let (name, count, quality): (Option<String>, i64, i64) = eval(
        &mut app,
        "local n, _, c, q = GetTradeTargetItemInfo(1) return n, c, q",
    );
    // Quality -1 for a record with no equip slot, as the getter gates it.
    assert_eq!(
        (name.as_deref(), count, quality),
        (Some("Silk Cloth"), 3, -1)
    );
    let link: Option<String> = eval(&mut app, "return GetTradeTargetItemLink(1)");
    assert!(
        link.as_deref().is_some_and(|l| l.contains("[Silk Cloth]")),
        "{link:?}"
    );
    assert!(sent(&rx).is_empty(), "a cached record asks nothing");
}

/// `GetAuctionItemInfo` on a row whose seller the name cache lacks asks inside its call with the
/// list's callback (`0x4cf1c8`), so `CMSG_NAME_QUERY` goes out ahead of the chunk's next verb,
/// and the house lane keeps the list's wait once however often it is read.
#[test]
fn an_auction_row_asks_its_missed_seller_inside_its_call() {
    use benilla_ui::script::{AuctionItemRow, AuctionListState, AuctionState, LIST};
    let (mut app, rx) = client();
    let mut state = AuctionState::default();
    state.lists[LIST] = AuctionListState {
        rows: vec![AuctionItemRow {
            item_id: 2589,
            name: Some("Linen Cloth".into()),
            quality: Some(1),
            count: 1,
            owner_guid: 0x55,
            ..Default::default()
        }],
        total: 1,
        ..Default::default()
    };
    vm(&mut app).set_auction(state);
    eval::<()>(
        &mut app,
        "GetAuctionItemInfo('list', 1) RandomRoll(1, 2) GetAuctionItemInfo('list', 1)",
    );
    assert_eq!(sent(&rx), ["name 0x55", "roll 1"]);
    let lane = app.world().resource::<crate::ui_auction::HouseLane>();
    assert!(
        lane.any_named(|g| g == 0x55),
        "the list waits on the seller"
    );
}

/// A seller and an owned row's bidder the name cache already holds answer inside the call, though
/// the frame's push had them unnamed: the lookup returns the record (`0x4cf167`, `0x4cf1c8`), so
/// nothing is asked and no list waits on them.
#[test]
fn an_auction_row_answers_a_cached_name_inside_its_call() {
    use benilla_ui::script::{
        AuctionHighBidder, AuctionItemRow, AuctionListState, AuctionState, OWNER,
    };
    let (mut app, rx) = client();
    let mut state = AuctionState::default();
    state.lists[OWNER] = AuctionListState {
        rows: vec![AuctionItemRow {
            item_id: 2589,
            name: Some("Linen Cloth".into()),
            quality: Some(1),
            count: 1,
            owner_guid: 0x55,
            named_bidder: 0x56,
            high_bidder: AuctionHighBidder::Nil,
            ..Default::default()
        }],
        total: 1,
        ..Default::default()
    };
    vm(&mut app).set_auction(state);
    {
        let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
        names.insert_player(0x55, "Seller".into(), None);
        names.insert_player(0x56, "Bidder".into(), None);
    }
    let (bidder, owner): (Option<String>, Option<String>) = eval(
        &mut app,
        "local _,_,_,_,_,_,_,_,_,_,b,o = GetAuctionItemInfo('owner', 1) return b, o",
    );
    assert_eq!(
        (bidder.as_deref(), owner.as_deref()),
        (Some("Bidder"), Some("Seller"))
    );
    assert!(sent(&rx).is_empty(), "a cached name asks nothing");
    let lane = app.world().resource::<crate::ui_auction::HouseLane>();
    assert!(!lane.any_named(|_| true), "and no list waits on it");
}

/// `GetGuildInfo` on a player whose guild the cache lacks makes the lookup with its callback
/// inside its call (`0x4c93d2`): `CMSG_GUILD_QUERY` goes out ahead of the chunk's next verb, once
/// while it is in flight.
#[test]
fn get_guild_info_asks_a_missed_guild_inside_its_call() {
    let (mut app, rx) = client();
    // The object the getter reads at the call: a player of guild 9 (fields 191 and 192).
    let target = app
        .world_mut()
        .spawn(crate::net::ObjectStore(
            benilla_protocol::ObjectFields::from_pairs(&[(191, 9), (192, 0)])
                .into_created(benilla_protocol::messages::ObjectType::Player),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(0x77, target);
    *app.world_mut().resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(target),
        guid: Some(0x77),
        ..Default::default()
    };
    vm(&mut app).set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: true,
            is_player: true,
            guid: 0x77,
            guild_id: 9,
            ..Default::default()
        }),
    );
    eval::<()>(
        &mut app,
        r#"GetGuildInfo("target") RandomRoll(1, 2) GetGuildInfo("target")"#,
    );
    assert_eq!(sent(&rx), ["guild 9", "roll 1"]);
}

/// `GetSummonConfirmSummoner` reads the name cache by the latched summoner (`0xb4e358`) with the
/// callback `0x48b710` and no dedupe (`0x48b6bf`-`0x48b6e1`): a miss sends `CMSG_NAME_QUERY`
/// inside the call, once while it is in flight, and again after a refusal.
#[test]
fn the_summoner_getter_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
    app.world_mut()
        .resource_mut::<crate::ui_summon::SummonState>()
        .latch(0x2a, 1519, 120_000, 0.0);
    eval::<()>(
        &mut app,
        "GetSummonConfirmSummoner() RandomRoll(1, 2) GetSummonConfirmSummoner()",
    );
    assert_eq!(sent(&rx), ["name 0x2a", "roll 1"]);
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(0x2a, String::new(), None);
    eval::<()>(&mut app, "GetSummonConfirmSummoner()");
    assert_eq!(
        sent(&rx),
        ["name 0x2a"],
        "the call after the refusal asks again"
    );
}

/// `GetBattlefieldPosition` on a teammate the name cache lacks looks the name up with the
/// callback `0x4ac190` and no dedupe (`0x4ac104`-`0x4ac118`): the query goes out inside the call,
/// once while it is in flight, and again after a refusal.
#[test]
fn a_battlefield_position_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
    app.world_mut()
        .resource_mut::<crate::ui_battlefield_positions::BattlefieldPositions>()
        .apply(benilla_protocol::messages::BattlefieldPositions {
            players: vec![benilla_protocol::messages::BattlefieldPosition {
                guid: 0x31,
                x: 0.0,
                y: 0.0,
            }],
            carrier: None,
        });
    eval::<()>(
        &mut app,
        "GetBattlefieldPosition(1) RandomRoll(1, 2) GetBattlefieldPosition(1)",
    );
    assert_eq!(sent(&rx), ["name 0x31", "roll 1"]);
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(0x31, String::new(), None);
    eval::<()>(&mut app, "GetBattlefieldPosition(1)");
    assert_eq!(
        sent(&rx),
        ["name 0x31"],
        "the call after the refusal asks again"
    );
}

/// The ammo slot's texture and quality getters look a record the cache lacks up with the
/// callback `0x4c84f0` and no dedupe (`0x4c83b2`-`0x4c83bc`, `0x4c89c2`-`0x4c89cf`): the item
/// query goes out inside the call, once while it is in flight, and each call leaves its callback.
#[test]
fn the_ammo_getters_ask_a_missed_record_inside_their_calls() {
    let (mut app, rx) = client();
    let mut slots: benilla_ui::script::InventorySlots = Default::default();
    slots[0] = Some(benilla_ui::script::InvSlotView {
        item_id: 2512,
        count: 200,
        ..Default::default()
    });
    vm(&mut app).set_inventory_slots(slots);
    seat_player(&mut app);
    eval::<()>(
        &mut app,
        r#"GetInventoryItemTexture("player", 0) RandomRoll(1, 2)
           GetInventoryItemQuality("player", 0)"#,
    );
    // The owner id is the unit's guid, the active player's (`0x4c83ad`-`0x4c83c4`).
    assert_eq!(sent(&rx), ["item 2512 0x2a", "roll 1"]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .take_ammo_waiters(2512),
        2,
        "a callback per call"
    );
}

/// The ammo getter's query, as the wire carries it: `CMSG_ITEM_QUERY_SINGLE`'s entry, then the
/// owner id the lookup passed, the active player's guid (`0x4c83ad`-`0x4c83c4`).
#[test]
fn the_ammo_getters_query_body_is_byte_exact() {
    let (mut app, rx) = client();
    let mut slots: benilla_ui::script::InventorySlots = Default::default();
    slots[0] = Some(benilla_ui::script::InvSlotView {
        item_id: 2512,
        count: 200,
        ..Default::default()
    });
    vm(&mut app).set_inventory_slots(slots);
    seat_player(&mut app);
    eval::<()>(&mut app, r#"GetInventoryItemTexture("player", 0)"#);
    let bodies: Vec<Vec<u8>> = rx
        .try_iter()
        .filter_map(|c| match c {
            ClientCommand::ItemQuery { entry, guid } => {
                Some(benilla_protocol::messages::item_query(entry, guid))
            }
            _ => None,
        })
        .collect();
    assert_eq!(bodies, [[0xd0, 0x09, 0, 0, 0x2a, 0, 0, 0, 0, 0, 0, 0]]);
}

/// The item cooldown query `0x6e2ed0` asks a record the cache lacks with the callback `0x6e2f70`,
/// deduped (`0x6e2ef4`-`0x6e2f0d`): `GetContainerItemCooldown` (`0x4f99b0`) and the bag and
/// inventory tooltips (`0x53483a`, `0x533188` → `0x5331ec`) each send the item query inside the
/// call, and the cooldowns await the record.
#[test]
fn the_cooldown_queries_ask_a_missed_record_inside_their_calls() {
    use benilla_protocol::field::{
        FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
    };
    use benilla_ui::script::{InvSlotView, InventorySlots};
    let (mut app, rx) = client();
    // The backpack's first two seats, 23 and 24, hold the two items whose records are missing.
    let held = [
        (23, 0x4000_0000_0000_0051_u64, 6948),
        (24, 0x4000_0000_0000_0052, 5976),
    ];
    let half = |lo: u16, guid: u64| [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)];
    let seats: Vec<(u16, u32)> = held
        .iter()
        .flat_map(|&(n, guid, _)| half(FIELD_PLAYER_INV_SLOT_HEAD + 2 * n, guid))
        .collect();
    seat_player_with(&mut app, &seats);
    // Held as objects, the record still unasked: spawned past the create handler, which asks it.
    for &(_, guid, entry) in &held {
        let mut pairs = vec![
            (2, 0x3),
            (FIELD_OBJECT_ENTRY, entry),
            (FIELD_ITEM_STACK_COUNT, 1),
        ];
        pairs.extend(half(6, ME));
        pairs.extend(half(8, ME));
        let world = app.world_mut();
        let item = world
            .spawn((
                crate::net::Guid(guid),
                crate::net::ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&pairs)
                        .into_created(benilla_protocol::messages::ObjectType::Item),
                ),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, item);
    }
    let mut worn: InventorySlots = Default::default();
    worn[14] = Some(InvSlotView {
        item_id: 2589,
        ..Default::default()
    });
    vm(&mut app).set_inventory_slots(worn);
    eval::<()>(
        &mut app,
        r#"local tip = CreateFrame("GameTooltip", "AskTip")
           tip:SetOwner(CreateFrame("Frame", "AskTipOwner"), "ANCHOR_NONE")
           GetContainerItemCooldown(0, 1) RandomRoll(1, 2)
           tip:SetBagItem(0, 2) RandomRoll(3, 4)
           tip:SetInventoryItem("player", 14) RandomRoll(5, 6)
           GetContainerItemCooldown(0, 1)"#,
    );
    assert_eq!(
        sent(&rx),
        [
            "item 6948 0x2a",
            "roll 1",
            "item 5976 0x2a",
            "roll 3",
            "item 2589 0x2a",
            "roll 5"
        ]
    );
    let mut items = app.world_mut().resource_mut::<crate::items::Items>();
    for entry in [6948, 5976, 2589] {
        assert_eq!(
            items.take_waiters(entry, crate::items::RecordCallback::CooldownQuery),
            1,
            "{entry} awaited"
        );
    }
}

/// The trainer's icon and description getters look the product's record up with the callbacks
/// `0x4d9140` and `0x4d9db0` and no dedupe (`0x4d9079`-`0x4d9096`, `0x4d9cea`-`0x4d9d07`): a miss
/// sends the item query inside the call, once while it is in flight, and each call leaves its
/// callback on the item cache.
#[test]
fn the_trainer_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{ProductWait, TrainerService, TrainerServiceCategory, TrainerState};
    let (mut app, rx) = client();
    vm(&mut app).set_trainer(Some(TrainerState {
        guid: NPC,
        services: vec![TrainerService {
            spell_id: 2018,
            name: Some("Blacksmithing".into()),
            category: TrainerServiceCategory::Available,
            group_key: 164,
            group_name: "Blacksmithing".into(),
            // The taught recipe's id builds the owner, never the wire spell's (`0x4d9074`).
            icon_wait: Some(ProductWait {
                item: 2589,
                taught: 2660,
            }),
            description_wait: Some(ProductWait {
                item: 2590,
                taught: 2660,
            }),
            ..Default::default()
        }],
        ..Default::default()
    }));
    eval::<()>(
        &mut app,
        "for i = 1, GetNumTrainerServices() do GetTrainerServiceIcon(i) end RandomRoll(1, 2) \
         for i = 1, GetNumTrainerServices() do GetTrainerServiceDescription(i) end \
         for i = 1, GetNumTrainerServices() do GetTrainerServiceIcon(i) end",
    );
    let owner = spell_owner(2660);
    assert_eq!(
        sent(&rx),
        [
            format!("item 2589 {owner:#x}"),
            "roll 1".into(),
            format!("item 2590 {owner:#x}")
        ]
    );
    let mut items = app.world_mut().resource_mut::<crate::items::Items>();
    let trainer = [crate::items::RecordCallback::Trainer];
    assert_eq!(
        items.take_callbacks(2589),
        [trainer; 2].concat(),
        "a callback per call"
    );
    assert_eq!(items.take_callbacks(2590), trainer);
}

/// `GetQuestItemInfo` looks a template in flight up with the callback `0x5018c0` and no dedupe
/// (`0x5017ca`-`0x5017de`): the item query goes out inside the call, once while it is in flight.
#[test]
fn get_quest_item_info_asks_a_missed_record_inside_its_call() {
    use benilla_protocol::messages::{QuestOfferReward, QuestRewardItem};
    let (mut app, rx) = client();
    let pending = |item_id: u32| QuestRewardItem {
        item_id,
        count: 1,
        display_id: 0,
    };
    // The panel's source, `[0xbe0810]`, which the panel setter writes (`0x500d86`).
    app.world_mut()
        .resource_mut::<crate::ui_quest::QuestGiver>()
        .open(
            NPC,
            crate::ui_quest::QuestView::Reward(QuestOfferReward {
                npc: NPC,
                quest_id: 1,
                title: String::new(),
                offer_text: String::new(),
                auto_finish: 0,
                choices: vec![pending(2590)],
                rewards: vec![pending(2589)],
                money: 0,
                quest_flags: 0,
                reward_spell: 0,
            }),
        );
    eval::<()>(
        &mut app,
        r#"GetQuestItemInfo("reward", 1) RandomRoll(1, 2) GetQuestItemInfo("choice", 1)
           GetQuestItemInfo("reward", 1)"#,
    );
    assert_eq!(
        sent(&rx),
        [
            format!("item 2589 {NPC:#x}"),
            "roll 1".into(),
            format!("item 2590 {NPC:#x}")
        ]
    );
}

/// The craft getters look a record in flight up with the callback `0x4f7710` and no dedupe:
/// `GetCraftReagentInfo` its reagent (`0x4f75dd`), `GetCraftSpellFocus` each totem (`0x4f79f2`).
/// The item query goes out inside the call, once while it is in flight.
#[test]
fn the_craft_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{
        CraftDifficulty, CraftReagent, CraftRecipe, CraftState, CraftTooltip,
    };
    let (mut app, rx) = client();
    vm(&mut app).set_craft(Some(CraftState {
        name: "Enchanting".into(),
        display_line: None,
        rank: 1,
        max_rank: 75,
        craft_type: 3,
        recipes: vec![CraftRecipe {
            spell_id: 7418,
            name: "Enchant Bracer - Minor Health".into(),
            sub_name: String::new(),
            difficulty: CraftDifficulty::Optimal,
            num_available: 0,
            icon: None,
            description: None,
            needs_item_target: true,
            reagents: vec![CraftReagent {
                item: 10940,
                name: None,
                icon: None,
                need: 1,
                have: 0,
            }],
            tools: Vec::new(),
            tooltip: CraftTooltip::Spell(7418),
            spell_level: 1,
            group: 0,
            skill_line: None,
            training_point_cost: 0,
            required_level: 0,
            tools_pending: vec![6218],
        }],
    }));
    eval::<()>(
        &mut app,
        "for i = 1, GetNumCrafts() do GetCraftReagentInfo(i, 1) end RandomRoll(1, 2) \
         for i = 1, GetNumCrafts() do GetCraftSpellFocus(i) GetCraftReagentInfo(i, 1) end",
    );
    let owner = spell_owner(7418);
    assert_eq!(
        sent(&rx),
        [
            format!("item 10940 {owner:#x}"),
            "roll 1".into(),
            format!("item 6218 {owner:#x}")
        ]
    );
}

/// The trade-skill getters look a record in flight up with the callback `0x4fdc30` and no dedupe:
/// `GetTradeSkillReagentInfo` its reagent (`0x4ff6cd`), `GetTradeSkillTools` each totem
/// (`0x4ffac6`). The item query goes out inside the call, once while it is in flight.
#[test]
fn the_trade_skill_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{
        TradeSkillDifficulty, TradeSkillReagent, TradeSkillRecipe, TradeSkillState,
    };
    let (mut app, rx) = client();
    vm(&mut app).set_trade_skill(Some(TradeSkillState {
        line: 197,
        line_name: "Tailoring".into(),
        rank: 1,
        max_rank: 75,
        recipes: vec![TradeSkillRecipe {
            group: None,
            spell_id: 2963,
            name: "Bolt of Linen Cloth".into(),
            difficulty: TradeSkillDifficulty::Trivial,
            num_available: 0,
            icon: None,
            min_made: 1,
            max_made: 1,
            cooldown: None,
            product_item: 2996,
            product_inv_type: 0,
            product_item_level: 0,
            reagents: vec![TradeSkillReagent {
                item: 2589,
                name: None,
                icon: None,
                need: 2,
                have: 0,
            }],
            tools: Vec::new(),
            icon_pending: None,
            tools_pending: vec![5956],
        }],
    }));
    eval::<()>(
        &mut app,
        "local n = GetNumTradeSkills() \
         for i = 1, n do GetTradeSkillReagentInfo(i, 1) end RandomRoll(1, 2) \
         for i = 1, n do GetTradeSkillTools(i) GetTradeSkillReagentInfo(i, 1) end",
    );
    let owner = spell_owner(2963);
    assert_eq!(
        sent(&rx),
        [
            format!("item 2589 {owner:#x}"),
            "roll 1".into(),
            format!("item 5956 {owner:#x}")
        ]
    );
}

/// The quest log's lookups with a callback and no dedupe: the leaderboard's creature name
/// (`0x4e044c`, callback `0x4e07d0`) and item (`0x4e05d1`, callback `0x4e0830`), and the reward
/// getter's item (`0x4e0b9e`). Each query goes out inside the call, once while it is in flight.
#[test]
fn the_quest_log_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{QuestObjectiveView, QuestRecordView, QuestSlotView, QUEST_LOG_SLOTS};
    let (mut app, rx) = client();
    let mut record = QuestRecordView {
        title: "Pelts".into(),
        level: 1,
        zone_or_sort: 12,
        header: "Elwynn Forest".into(),
        rewards: vec![(2589, 1)],
        ..Default::default()
    };
    record.objectives[0] = QuestObjectiveView {
        creature_or_go: 40,
        required_count: 3,
        ..Default::default()
    };
    record.objectives[1] = QuestObjectiveView {
        item_id: 900,
        item_count: 5,
        ..Default::default()
    };
    let mut slots = [QuestSlotView::default(); QUEST_LOG_SLOTS];
    slots[0] = QuestSlotView {
        quest_id: 50,
        counters: [1, 0, 0, 0],
        state: 0,
        timer: 0,
    };
    {
        let mut s = vm(&mut app);
        s.seed_quest_record(50, record);
        s.set_quest_log_player(true);
        s.set_quest_slots(slots);
        s.quest_log_changed();
    }
    eval::<()>(
        &mut app,
        "GetQuestLogLeaderBoard(1, 2) RandomRoll(1, 2) GetQuestLogLeaderBoard(2, 2) \
         RandomRoll(3, 4) SelectQuestLogEntry(2) GetQuestLogRewardInfo(1) \
         GetQuestLogLeaderBoard(1, 2) GetQuestLogLeaderBoard(2, 2)",
    );
    assert_eq!(
        sent(&rx),
        // Each lookup passes a zero owner id (`0x4e0448`-`0x4e045e`, `0x4e05c7`-`0x4e05ce`,
        // `0x4e0b9c`-`0x4e0baa`).
        [
            "creature 40 0x0",
            "roll 1",
            "item 900 0x0",
            "roll 3",
            "item 2589 0x0"
        ]
    );
}
