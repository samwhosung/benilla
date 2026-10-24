//! The getters' lookups with a callback, each through the whole client's host: a getter that
//! misses asks the cache inside its call, so the query goes out ahead of the chunk's next verb
//! (`RandomRoll`, `0x48c832`), as the reference's lookup sends it on the miss (`0x556aa0`).

use benilla_ui::script::{TradeSideState, TradeSlotItem, TradeState, UiScript, UnitState};
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
                ClientCommand::ItemQuery { entry, .. } => format!("item {entry}"),
                ClientCommand::GuildQuery { guild_id } => format!("guild {guild_id}"),
                _ => return None,
            })
        })
        .collect()
}

fn vm(app: &mut App) -> Mut<'_, UiScript> {
    app.world_mut().non_send_resource_mut::<UiScript>()
}

/// `UnitName` on a unit with no object reads the player-name cache by guid with a callback
/// (`0x5171eb`), which sends `CMSG_NAME_QUERY` on the miss inside the call, once while it is in
/// flight.
#[test]
fn unit_name_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
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

/// A trade getter that finds a filled slot whose template the cache lacks asks the item cache
/// with the callback `0x4c0340` inside its call (`0x4c017f`, `0x4c0582`), so the query goes out
/// ahead of the chunk's next verb, and the callback waits for the record.
#[test]
fn a_trade_getter_asks_a_missed_template_inside_its_call() {
    let (mut app, rx) = client();
    let mut target = TradeSideState::default();
    target.slots[0] = Some(TradeSlotItem {
        item_id: 4306,
        count: 1,
        ..Default::default()
    });
    let mut player = TradeSideState::default();
    player.slots[1] = Some(TradeSlotItem {
        item_id: 2592,
        count: 1,
        ..Default::default()
    });
    vm(&mut app).set_trade(Some(TradeState { player, target }));
    eval::<()>(
        &mut app,
        "GetTradeTargetItemInfo(1) RandomRoll(1, 2) GetTradePlayerItemInfo(2)",
    );
    assert_eq!(sent(&rx), ["item 4306", "roll 1", "item 2592"]);
}

/// `GetGuildInfo` on a player whose guild the cache lacks makes the lookup with its callback
/// inside its call (`0x4c93d2`): `CMSG_GUILD_QUERY` goes out ahead of the chunk's next verb, once
/// while it is in flight.
#[test]
fn get_guild_info_asks_a_missed_guild_inside_its_call() {
    let (mut app, rx) = client();
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
    assert_eq!(sent(&rx), ["name 0x2a"], "the call after the refusal asks again");
}

/// `GetBattlefieldPosition` on a teammate the name cache lacks looks the name up with the
/// callback `0x4ac190` and no dedupe (`0x4ac104`-`0x4ac118`): the query goes out inside the call,
/// once while it is in flight, and again after a refusal.
#[test]
fn a_battlefield_position_asks_a_missed_name_inside_its_call() {
    let (mut app, rx) = client();
    vm(&mut app).set_battlefield_positions(
        vec![benilla_ui::script::BattlefieldPositionView {
            uv: (0.5, 0.5),
            name: None,
            guid: 0x31,
        }],
        None,
        1.0,
    );
    eval::<()>(
        &mut app,
        "GetBattlefieldPosition(1) RandomRoll(1, 2) GetBattlefieldPosition(1)",
    );
    assert_eq!(sent(&rx), ["name 0x31", "roll 1"]);
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(0x31, String::new(), None);
    eval::<()>(&mut app, "GetBattlefieldPosition(1)");
    assert_eq!(sent(&rx), ["name 0x31"], "the call after the refusal asks again");
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
    eval::<()>(
        &mut app,
        r#"GetInventoryItemTexture("player", 0) RandomRoll(1, 2)
           GetInventoryItemQuality("player", 0)"#,
    );
    assert_eq!(sent(&rx), ["item 2512", "roll 1"]);
    assert_eq!(
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .take_ammo_waiters(2512),
        2,
        "a callback per call"
    );
}

/// The item cooldown query `0x6e2ed0` asks a record the cache lacks with the callback `0x6e2f70`,
/// deduped (`0x6e2ef4`-`0x6e2f0d`): `GetContainerItemCooldown` (`0x4f99b0`) and the bag and
/// inventory tooltips (`0x53483a`, `0x533188` → `0x5331ec`) each send the item query inside the
/// call, and the cooldowns await the record.
#[test]
fn the_cooldown_queries_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{ContainerSlot, ContainerState, InvSlotView, InventorySlots};
    let (mut app, rx) = client();
    let pending = |item_id: u32| ContainerSlot {
        item_id,
        count: 1,
        ..Default::default()
    };
    vm(&mut app).set_container(
        0,
        Some(ContainerState {
            name: None,
            num_slots: 16,
            slots: std::collections::HashMap::from([(1, pending(6948)), (2, pending(5976))]),
        }),
    );
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
        ["item 6948", "roll 1", "item 5976", "roll 3", "item 2589", "roll 5"]
    );
    let cooldowns = app.world().resource::<crate::spell::Cooldowns>();
    for entry in [6948, 5976, 2589] {
        assert!(cooldowns.awaits_item_record(entry), "{entry} awaited");
    }
}

/// The trainer's icon and description getters look the product's record up with the callbacks
/// `0x4d9140` and `0x4d9db0` and no dedupe (`0x4d9079`-`0x4d9096`, `0x4d9cea`-`0x4d9d07`): a miss
/// sends the item query inside the call, once while it is in flight, and each call leaves its
/// callback on the window.
#[test]
fn the_trainer_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{TrainerService, TrainerServiceCategory, TrainerState};
    let (mut app, rx) = client();
    vm(&mut app).set_trainer(Some(TrainerState {
        guid: NPC,
        services: vec![TrainerService {
            spell_id: 2018,
            name: Some("Blacksmithing".into()),
            category: TrainerServiceCategory::Available,
            group_key: 164,
            group_name: "Blacksmithing".into(),
            icon_wait: Some(2589),
            description_wait: Some(2590),
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
    assert_eq!(sent(&rx), ["item 2589", "roll 1", "item 2590"]);
    assert_eq!(
        app.world().resource::<crate::ui_trainer::TrainerOpen>().item_waits,
        [2589, 2590, 2589],
        "a callback per call"
    );
}

/// `GetQuestItemInfo` looks a template in flight up with the callback `0x5018c0` and no dedupe
/// (`0x5017ca`-`0x5017de`): the item query goes out inside the call, once while it is in flight.
#[test]
fn get_quest_item_info_asks_a_missed_record_inside_its_call() {
    use benilla_ui::script::{QuestItemView, QuestPanel, QuestState};
    let (mut app, rx) = client();
    let pending = |item_id: u32| QuestItemView {
        item_id,
        count: 1,
        ..Default::default()
    };
    vm(&mut app).set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        rewards: vec![pending(2589)],
        choices: vec![pending(2590)],
        ..Default::default()
    }));
    eval::<()>(
        &mut app,
        r#"GetQuestItemInfo("reward", 1) RandomRoll(1, 2) GetQuestItemInfo("choice", 1)
           GetQuestItemInfo("reward", 1)"#,
    );
    assert_eq!(sent(&rx), ["item 2589", "roll 1", "item 2590"]);
}

/// The craft getters look a record in flight up with the callback `0x4f7710` and no dedupe:
/// `GetCraftReagentInfo` its reagent (`0x4f75dd`), `GetCraftSpellFocus` each totem (`0x4f79f2`).
/// The item query goes out inside the call, once while it is in flight.
#[test]
fn the_craft_getters_ask_a_missed_record_inside_their_calls() {
    use benilla_ui::script::{CraftDifficulty, CraftReagent, CraftRecipe, CraftState, CraftTooltip};
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
    assert_eq!(sent(&rx), ["item 10940", "roll 1", "item 6218"]);
}
