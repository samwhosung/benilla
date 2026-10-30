//! What an item record's answer runs, at its packet's place. `SMSG_ITEM_QUERY_SINGLE_RESPONSE`'s
//! handler (`0x555140`) calls `ItemCacheReceive` (`0x55bdb0`), which stores the record
//! (`0x55bef6`, `0x55befb`) and then runs every callback the lookups that missed it registered
//! (`0x55bf12`-`0x55bf39`), inside the handler: the equip callbacks (`0x5e17a0`, `0x5e1020`) send
//! there, ahead of a later packet's, and the quest log's (`0x4e0830`) and the tooltips'
//! (`0x52e530`, `0x52f820`) fire and rebuild between the frame's neighbouring packets.

use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_PLAYER_INV_SLOT_HEAD};
use benilla_protocol::messages::ObjectType;
use benilla_protocol::{ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::unit_view_tests::{client, fall, frame, values};
use crate::net::ClientCommand;

/// [`client`]'s player.
const ME: u64 = 0x7;
/// A fall's `UNIT_COMBAT`, as [`client`]'s spy records it.
const FELL: &str = "UNIT_COMBAT:player:100";
/// Our item in backpack slot 1, whose record the cache has not seen.
const HELM: u64 = (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | 0x31;
const HELM_ENTRY: u32 = 4_242;
/// The duel flag's guid of our own challenge, which the request's handler accepts at once
/// (`0x4d4a12`).
const ARBITER: u64 = 0xF110_0000_0000_0055;

/// [`client`] with our helm in backpack slot 1 and its record in flight, cursor control, and the
/// sends kept.
fn holding_client() -> (App, Receiver<ClientCommand>) {
    let mut app = client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let helm = SessionEvent::ItemCreate {
        guid: HELM,
        container: false,
        fields: ObjectFields::from_pairs(&[
            (2, 0x3),
            (3, HELM_ENTRY),
            (6, ME as u32),
            (8, ME as u32),
            (FIELD_ITEM_STACK_COUNT, 1),
        ])
        .into_created(ObjectType::Item),
    };
    let seat = FIELD_PLAYER_INV_SLOT_HEAD + 2 * 23;
    frame(
        &mut app,
        vec![
            helm,
            values(ME, &[(seat, HELM as u32), (seat + 1, (HELM >> 32) as u32)]),
        ],
    );
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_player_control(true);
    (app, rx)
}

/// `chunk` with the host lent.
fn hosted(app: &mut App, chunk: &str) {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").run(chunk).expect("the chunk runs")
    });
}

/// The helm's record: plain, no bind on equip, so the replay sends.
fn helm_record() -> SessionEvent {
    let mut t = crate::items::test_template("Plain Helm");
    (t.class, t.inventory_type) = (4, 1);
    SessionEvent::ItemTemplate {
        entry: HELM_ENTRY,
        info: Some(Box::new(t)),
    }
}

/// A duel request we made, whose handler sends `CMSG_DUEL_ACCEPTED` at once.
fn our_duel() -> SessionEvent {
    SessionEvent::DuelRequested {
        arbiter: ARBITER,
        challenger: ME,
    }
}

/// The equip and move sends among `sent`, and the duel's accept, in order.
fn equips_and_duels(sent: &Receiver<ClientCommand>) -> Vec<&'static str> {
    sent.try_iter()
        .filter_map(|c| match c {
            ClientCommand::AutoEquipItem { .. } => Some("AutoEquipItem"),
            ClientCommand::SwapInvItem { .. } | ClientCommand::SwapItem { .. } => Some("Swap"),
            ClientCommand::DuelAccepted { .. } => Some("DuelAccepted"),
            _ => None,
        })
        .collect()
}

/// `AutoEquipCursorItem` on a helm whose record is not cached asks it with the callback
/// `0x5e17a0` (`0x5e1603`); the answer runs the call again inside its handler (`0x5e182b`), whose
/// send (`0x5e1757`) goes out ahead of the next packet's handler's.
#[test]
fn an_equip_replay_sends_before_a_later_packets_handler_send() {
    let (mut app, sent) = holding_client();
    hosted(&mut app, "PickupContainerItem(0, 1) AutoEquipCursorItem()");
    assert!(
        sent.try_iter().any(|c| matches!(
            c,
            ClientCommand::ItemQuery {
                entry: HELM_ENTRY,
                ..
            }
        )),
        "the record is asked"
    );
    frame(&mut app, vec![helm_record(), our_duel()]);
    assert_eq!(equips_and_duels(&sent), ["AutoEquipItem", "DuelAccepted"]);
}

/// The replay's clear signals at its packet's place: its `CURSOR_UPDATE` fires between the
/// falls of the packets around the answer, the cursor read empty there and held before it.
#[test]
fn an_equip_replays_signals_fire_at_its_packets_place() {
    let (mut app, _sent) = holding_client();
    hosted(&mut app, "PickupContainerItem(0, 1) AutoEquipCursorItem()");
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("CURSOR_UPDATE")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. (CursorHasItem() and " held" or " empty"))
            end)
            "#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![fall(5), helm_record(), fall(6)]);
    assert_eq!(
        seen,
        [
            FELL,
            "UNIT_COMBAT held",
            "CURSOR_UPDATE empty",
            FELL,
            "UNIT_COMBAT empty",
        ]
    );
}

/// A swap onto the doll parked on the helm's record (`0x5e0dbe`, the callback `0x5e1020`)
/// replays inside the answer's handler (`0x5e10c2`): its move goes out ahead of the next
/// packet's send, and the parked element is freed (`0x5e10cf`).
#[test]
fn a_parked_swap_sends_before_a_later_packets_handler_send() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, sent) = holding_client();
    let mv = benilla_ui::script::ContainerMove {
        src_bag: 0,
        src_slot: 1,
        dst_bag: benilla_ui::script::EQUIPMENT_BAG,
        dst_slot: 1,
        count: None,
    };
    app.world_mut()
        .run_system_once(
            move |mut gate: crate::ui_bind_confirm::BindGate,
                  items: Res<crate::items::Items>,
                  net: Res<crate::net::NetCommands>| {
                gate.park_swap(&mv, HELM_ENTRY, &items, &net);
            },
        )
        .unwrap();
    frame(&mut app, vec![helm_record(), our_duel()]);
    assert_eq!(equips_and_duels(&sent), ["Swap", "DuelAccepted"]);
    assert_eq!(
        app.world()
            .resource::<crate::ui_bind_confirm::PendingEquips>()
            .live(),
        0,
        "the replay freed the parked move"
    );
}

/// A leaderboard read that missed an objective's item registered the callback `0x4e0830`
/// (`0x4e05e0`); the record's answer signals `QUEST_LOG_UPDATE` (`0x4e083a`) at its packet's
/// place, between the falls of the packets around it.
#[test]
fn a_quest_log_update_off_an_item_answer_fires_between_its_neighbours() {
    use crate::ui_quest_log::fixture::{quest, seat};
    const PELT: u32 = 900;
    let mut app = client();
    {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        let mut pelts = quest(50, "Pelts", 1);
        pelts.record.objectives[0] = benilla_ui::script::QuestObjectiveView {
            item_id: PELT,
            item_count: 5,
            ..Default::default()
        };
        seat(&mut s, &[pelts]);
        s.run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("QUEST_LOG_UPDATE")
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    }
    // Our quest log's first slot holds the quest, as the getter reads it at the call.
    let slot = benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
    frame(&mut app, vec![values(ME, &[(slot, 50)])]);
    // The read, with the host lent, misses the pelt's record and registers the callback.
    let kind: String = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        let s = s.expect("a VM");
        let kind = s
            .eval("local _, kind = GetQuestLogLeaderBoard(1, 2) return kind")
            .expect("the chunk runs");
        s.run("Seen = {}").expect("the chunk runs");
        kind
    });
    assert_eq!(kind, "item");
    let answer = SessionEvent::ItemTemplate {
        entry: PELT,
        info: Some(Box::new(crate::items::test_template("Wolf Pelt"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "QUEST_LOG_UPDATE", FELL]
    );
}

/// A shown spell tooltip whose reagent's record the build missed counts it and keeps the
/// callback `0x52f820` with the tooltip as its cookie (`0x52f3f4`, `0x52f3ff`); the answer builds
/// the spell again at its packet's place (`0x52f862`), so a listener of the packet before reads
/// the tooltip as it stood and one of the packet after reads the reagent's line.
#[test]
fn a_spell_tooltip_rebuilds_at_its_reagents_answer() {
    use benilla_formats::{SpellCatalog, SpellDisplay};
    const FROSTBOLT: u32 = 116;
    /// Arcane Powder.
    const POWDER: u32 = 17020;
    let mut app = client();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (POWDER, 1);
    let frostbolt = SpellDisplay {
        id: FROSTBOLT,
        name: "Frostbolt".into(),
        reagents,
        ..Default::default()
    };
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(std::collections::HashMap::from([(
            FROSTBOLT, frostbolt,
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    app.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
    });
    app.world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(FROSTBOLT);
    crate::ui_script::test_ui::load_tooltip_template(app.world().non_send_resource::<UiScript>());
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            SPELL_REAGENTS = "Reagents: "
            local a = CreateFrame("Button", "B"); a:SetPoint("CENTER", 0, 0)
            a:SetWidth(10); a:SetHeight(10)
            CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_COMBAT")
            f:SetScript("OnEvent", function()
                table.insert(Seen, "lines " .. TT:NumLines())
            end)
            "#,
        )
        .unwrap();
    hosted(
        &mut app,
        r#"TT:SetOwner(B, "ANCHOR_RIGHT"); TT:SetSpell(1, "spell")"#,
    );
    let answer = SessionEvent::ItemTemplate {
        entry: POWDER,
        info: Some(Box::new(crate::items::test_template("Arcane Powder"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "lines 1", FELL, "lines 2"]
    );
}

/// Two swaps parked on one record (`0x5e0dbe`) replay in table order inside the answer's handler
/// (`0x5e1087`-`0x5e10ed`): each `SwapItem` sends in its body (`0x5e0fdb`) ahead of its clear
/// (`0x5e0fe7`), so the second's send waits on no signal of the first's, and both go out ahead
/// of the next packet's.
#[test]
fn two_parked_swaps_on_one_record_both_send_before_a_later_packets_handler_send() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, sent) = holding_client();
    let to = |dst_slot: u32| benilla_ui::script::ContainerMove {
        src_bag: 0,
        src_slot: 1,
        dst_bag: benilla_ui::script::EQUIPMENT_BAG,
        dst_slot,
        count: None,
    };
    let moves = [to(1), to(1)];
    // The helm held, so the first replay's clear signals `CURSOR_UPDATE` ahead of the second.
    hosted(&mut app, "PickupContainerItem(0, 1)");
    app.world_mut()
        .run_system_once(
            move |mut gate: crate::ui_bind_confirm::BindGate,
                  items: Res<crate::items::Items>,
                  net: Res<crate::net::NetCommands>| {
                for mv in &moves {
                    gate.park_swap(mv, HELM_ENTRY, &items, &net);
                }
            },
        )
        .unwrap();
    frame(&mut app, vec![helm_record(), our_duel()]);
    assert_eq!(equips_and_duels(&sent), ["Swap", "Swap", "DuelAccepted"]);
}

/// A refusal destroys the record's node (`0x55bed2`), so an equip on that entry looks it up
/// afresh: it waits and asks again, its callback held for the next answer, never run at once on
/// a record that is not there.
#[test]
fn an_equip_on_a_refused_record_asks_again_and_waits() {
    let (mut app, sent) = holding_client();
    hosted(&mut app, "PickupContainerItem(0, 1) AutoEquipCursorItem()");
    let refusal = SessionEvent::ItemTemplate {
        entry: HELM_ENTRY,
        info: None,
    };
    frame(&mut app, vec![refusal]);
    sent.try_iter().for_each(drop);
    hosted(&mut app, "AutoEquipCursorItem()");
    let asked = sent.try_iter().any(|c| {
        matches!(
            c,
            ClientCommand::ItemQuery {
                entry: HELM_ENTRY,
                ..
            }
        )
    });
    assert!(asked, "the record is asked again");
    assert_eq!(
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .take_waiters(HELM_ENTRY, crate::items::RecordCallback::AutoEquip),
        1,
        "the callback waits for the next answer"
    );
}

/// A record the cache holds ahead of the VM's copy, as a cache file's load lands one outside an
/// answer, is found by the lookup inside the call (`0x5e1603`, `0x5e0db1`): `AutoEquipCursorItem`
/// sends in its body (`0x5e1757`) and a place's `SwapItem` in its (`0x5e0fdb`), before the call
/// returns, with nothing asked or parked for a later replay.
#[test]
fn an_equip_on_a_record_the_cache_holds_sends_inside_the_call() {
    for (chunk, send) in [
        (
            "PickupContainerItem(0, 1) AutoEquipCursorItem()",
            "AutoEquipItem",
        ),
        ("PickupContainerItem(0, 1) PickupInventoryItem(1)", "Swap"),
    ] {
        let (mut app, sent) = holding_client();
        let mut t = crate::items::test_template("Plain Helm");
        (t.class, t.inventory_type) = (4, 1);
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(HELM_ENTRY, Some(t));
        sent.try_iter().for_each(drop);
        hosted(&mut app, chunk);
        let sent: Vec<ClientCommand> = sent.try_iter().collect();
        assert!(
            !sent
                .iter()
                .any(|c| matches!(c, ClientCommand::ItemQuery { .. })),
            "{chunk}: nothing asked"
        );
        let (tx, rx) = crossbeam_channel::unbounded();
        sent.into_iter().for_each(|c| tx.send(c).unwrap());
        assert_eq!(equips_and_duels(&rx), [send], "{chunk}");
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "{chunk}: nothing parked"
        );
    }
}

/// A bind-on-equip record's answer replays `AutoEquipCursorItem` inside its handler, which asks
/// the question (`0x5e163b`, `0x5e16c6`): `AUTOEQUIP_BIND_CONFIRM` fires at the answer's place,
/// between the falls of the packets around it, and nothing is sent.
#[test]
fn a_bind_on_equip_replay_asks_at_its_answers_place() {
    let (mut app, sent) = holding_client();
    hosted(&mut app, "PickupContainerItem(0, 1) AutoEquipCursorItem()");
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("AUTOEQUIP_BIND_CONFIRM")
            f:SetScript("OnEvent", function() table.insert(Seen, event .. " " .. arg1) end)
            "#,
        )
        .unwrap();
    let mut t = crate::items::test_template("Bound Helm");
    (t.class, t.inventory_type, t.bonding) = (4, 1, benilla_ui::script::BIND_WHEN_EQUIPPED);
    let answer = SessionEvent::ItemTemplate {
        entry: HELM_ENTRY,
        info: Some(Box::new(t)),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "AUTOEQUIP_BIND_CONFIRM 0", FELL]
    );
    assert!(equips_and_duels(&sent).is_empty(), "nothing sent");
}

/// A try-on of an item the cache lacks keeps `0x504ba0` on the record (`0x504579`, dedupe 0);
/// the answer tries it on again inside its handler (`0x504bae` → `0x504540`), whose refusal of a
/// non-equippable item shows `ERR_NOT_EQUIPPABLE` (`0x504b2d`) at the packet's place, between
/// the falls of the packets around it.
#[test]
fn a_try_on_refused_at_its_records_answer_shows_at_the_answers_place() {
    const BOX: u32 = 5_555;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            ERR_NOT_EQUIPPABLE = "Not equippable"
            local pane = CreateFrame("Frame", "DressUpFrame", UIParent)
            CreateFrame("DressUpModel", "DressUpModel", pane)
            local f = CreateFrame("Frame")
            f:RegisterEvent("UI_ERROR_MESSAGE")
            f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)
            "#,
        )
        .unwrap();
    hosted(&mut app, &format!("DressUpModel:TryOn({BOX})"));
    let mut t = crate::items::test_template("Box");
    t.inventory_type = 0;
    let answer = SessionEvent::ItemTemplate {
        entry: BOX,
        info: Some(Box::new(t)),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [FELL, "UI_ERROR_MESSAGE:Not equippable", FELL]
    );
}

/// A crafting line whose item the cache lacks waits with the combat log's callback `0x6294b0`
/// (`0x55ba30`, dedupe 0), ahead of a leaderboard read's `0x4e0830` on the same record; the answer
/// runs them in that order (`0x55bf12`-`0x55bf39`), so the line is written ahead of the quest
/// log's update.
#[test]
fn a_combat_lines_callback_runs_at_its_place_in_the_answers_list() {
    use crate::ui_quest_log::fixture::{quest, seat};
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    let mut app = client();
    {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        let mut pelts = quest(50, "Pelts", 1);
        pelts.record.objectives[0] = benilla_ui::script::QuestObjectiveView {
            item_id: PELT,
            item_count: 5,
            ..Default::default()
        };
        seat(&mut s, &[pelts]);
        s.run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("QUEST_LOG_UPDATE")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. (arg1 and (":" .. arg1) or ""))
            end)
            "#,
        )
        .unwrap();
    }
    let craft = SessionEvent::SpellLogExecute(SpellLogExecute {
        caster: ME,
        spell_id: 1,
        effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry: PELT }])],
    });
    let slot = benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
    frame(&mut app, vec![values(ME, &[(slot, 50)]), craft]);
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        let s = s.expect("a VM");
        s.run("GetQuestLogLeaderBoard(1, 2) Seen = {}")
            .expect("the chunk runs");
    });
    let answer = SessionEvent::ItemTemplate {
        entry: PELT,
        info: Some(Box::new(crate::items::test_template("Wolf Pelt"))),
    };
    assert_eq!(
        frame(&mut app, vec![answer]),
        [
            "CHAT_MSG_SPELL_TRADESKILLS:You create Wolf Pelt.",
            "QUEST_LOG_UPDATE"
        ]
    );
}

/// A crafting line whose item the cache lacks looks it up at its packet (`0x629650`) and waits;
/// the answer later in the same span writes it inside its callback (`0x6294b0`), between the
/// falls around the answer, never at its own packet's place off the later record. The span's
/// packets run while the interface is down, as a reload's do, and fire once it is up, so the
/// line's first drain comes after the answer's handler.
#[test]
fn a_combat_line_waits_for_an_answer_later_in_its_span() {
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)
            "#,
        )
        .unwrap();
    let craft = SessionEvent::SpellLogExecute(SpellLogExecute {
        caster: ME,
        spell_id: 1,
        effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry: PELT }])],
    });
    let answer = SessionEvent::ItemTemplate {
        entry: PELT,
        info: Some(Box::new(crate::items::test_template("Wolf Pelt"))),
    };
    use crate::client_state::ClientState;
    app.insert_resource(State::new(ClientState::CharSelect));
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, vec![craft, fall(5), answer, fall(6)]);
    world.insert_resource(State::new(ClientState::InWorld));
    crate::lanes::fire_in_order(world);
    let script = world.non_send_resource::<UiScript>();
    assert_eq!(
        script.eval::<Vec<String>>("return Seen").unwrap(),
        [
            FELL,
            "CHAT_MSG_SPELL_TRADESKILLS:You create Wolf Pelt.",
            FELL
        ]
    );
}

/// Two crafting lines of one kind naming one uncached item, a leaderboard read of it between
/// them: each line keeps its entry in the wait list (`0x6293a0`, the lowest free slot
/// `0x6293f0`) and its own callback. The answer runs the callbacks oldest first, and each writes
/// one line, the last waiting entry of its kind and item (`0x6295b0` scans from the back): the
/// newer line at the first callback, the quest log's between, the older at the last.
#[test]
fn two_waiting_lines_of_one_item_are_written_newest_first_one_per_callback() {
    use crate::ui_quest_log::fixture::{quest, seat};
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    /// [`client`]'s wolf.
    const WOLF: u64 = 0xF130_0000_0044_0001;
    let mut app = client();
    {
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        let mut pelts = quest(50, "Pelts", 1);
        pelts.record.objectives[0] = benilla_ui::script::QuestObjectiveView {
            item_id: PELT,
            item_count: 5,
            ..Default::default()
        };
        seat(&mut s, &[pelts]);
        s.run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            TRADESKILL_LOG_THIRDPERSON = "%s creates %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("QUEST_LOG_UPDATE")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function()
                table.insert(Seen, arg1 and event .. ":" .. arg1 or event)
            end)
            "#,
        )
        .unwrap();
    }
    let slot = benilla_protocol::field::FIELD_PLAYER_QUEST_LOG_1_1;
    frame(&mut app, vec![values(ME, &[(slot, 50)])]);
    let craft = |caster| {
        SessionEvent::SpellLogExecute(SpellLogExecute {
            caster,
            spell_id: 1,
            effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry: PELT }])],
        })
    };
    frame(&mut app, vec![craft(ME)]);
    hosted(&mut app, "GetQuestLogLeaderBoard(1, 2)");
    frame(&mut app, vec![craft(WOLF)]);
    hosted(&mut app, "Seen = {}");
    let answer = SessionEvent::ItemTemplate {
        entry: PELT,
        info: Some(Box::new(crate::items::test_template("Wolf Pelt"))),
    };
    assert_eq!(
        frame(&mut app, vec![fall(5), answer, fall(6)]),
        [
            FELL,
            "CHAT_MSG_SPELL_TRADESKILLS:Unknown Being creates Wolf Pelt.",
            "QUEST_LOG_UPDATE",
            "CHAT_MSG_SPELL_TRADESKILLS:You create Wolf Pelt.",
            FELL
        ]
    );
}

/// A reconnect drops the item callbacks (`net::session`'s teardown) and the wait list with them:
/// a line on the same kind and item after it, in the lowest free slot, is the one its callback
/// frees and writes, never an entry left from before the reconnect in a slot above it.
#[test]
fn a_reconnect_clears_the_combat_logs_wait_list() {
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    const LEATHER: u32 = 901;
    /// [`client`]'s wolf.
    const WOLF: u64 = 0xF130_0000_0044_0001;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            TRADESKILL_LOG_THIRDPERSON = "%s creates %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function() table.insert(Seen, arg1) end)
            "#,
        )
        .unwrap();
    let craft = |caster, item_entry| {
        SessionEvent::SpellLogExecute(SpellLogExecute {
            caster,
            spell_id: 1,
            effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry }])],
        })
    };
    let answer = |entry, name| SessionEvent::ItemTemplate {
        entry,
        info: Some(Box::new(crate::items::test_template(name))),
    };
    // The leather's line takes the first slot and the wolf's pelt the second; the leather's
    // answer frees the first.
    frame(&mut app, vec![craft(ME, LEATHER), craft(WOLF, PELT)]);
    frame(&mut app, vec![answer(LEATHER, "Light Leather")]);
    frame(
        &mut app,
        vec![SessionEvent::Disconnected {
            reason: "lost".into(),
            end: benilla_protocol::SessionEnd::Lost,
        }],
    );
    // The reconnect's world entry: we are created again.
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    frame(
        &mut app,
        vec![super::unit_view_tests::created(
            ME,
            benilla_protocol::EntityKind::Player,
            &[],
        )],
    );
    let me = app.world().resource::<crate::net::GuidIndex>().0[&ME];
    app.world_mut()
        .entity_mut(me)
        .insert(crate::net::SelfPlayer);
    frame(&mut app, vec![craft(ME, PELT)]);
    hosted(&mut app, "Seen = {}");
    let seen = frame(&mut app, vec![answer(PELT, "Wolf Pelt")]);
    assert_eq!(seen, ["You create Wolf Pelt."]);
}

/// The wait list has no timer or count: only an item callback or the session's end frees an
/// entry (`0x6293a0`, `0x6294cc`, `0x629a90`), so a record that lands frames later still writes
/// its line. A refusal's callback frees its entry and writes nothing (`0x6294fd`-`0x629504`), so
/// a record after it finds no line waiting.
#[test]
fn a_waiting_line_outlasts_its_frames_and_a_refusal_frees_it_unwritten() {
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    const GONE: u32 = 99_999;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            TRADESKILL_LOG_FIRSTPERSON = "You create %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function() table.insert(Seen, arg1) end)
            "#,
        )
        .unwrap();
    let craft = |item_entry| {
        SessionEvent::SpellLogExecute(SpellLogExecute {
            caster: ME,
            spell_id: 1,
            effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry }])],
        })
    };
    let answer = |entry, name: Option<&str>| SessionEvent::ItemTemplate {
        entry,
        info: name.map(|n| Box::new(crate::items::test_template(n))),
    };
    frame(&mut app, vec![craft(PELT), craft(GONE)]);
    for _ in 0..300 {
        frame(&mut app, vec![]);
    }
    hosted(&mut app, "Seen = {}");
    assert_eq!(
        frame(&mut app, vec![answer(PELT, Some("Wolf Pelt"))]),
        ["You create Wolf Pelt."]
    );
    hosted(&mut app, "Seen = {}");
    assert!(frame(&mut app, vec![answer(GONE, None)]).is_empty());
    assert!(
        frame(&mut app, vec![answer(GONE, Some("Late Record"))]).is_empty(),
        "the refusal freed the entry"
    );
}

/// The item's callback runs the tradeskill writer again from the entry's guids (`0x629556` →
/// `0x629610`), which names the caster there (`0x6264e0`): a caster whose name arrives between
/// the packet and the item's answer is written by that name.
#[test]
fn a_waiting_tradeskill_line_names_its_caster_at_the_answer() {
    use benilla_protocol::messages::{ExecuteLog, SpellLogExecute};
    const PELT: u32 = 900;
    const CRAFTER: u64 = 0x0000_0000_0000_0042;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            TRADESKILL_LOG_THIRDPERSON = "%s creates %s."
            local f = CreateFrame("Frame")
            f:RegisterEvent("CHAT_MSG_SPELL_TRADESKILLS")
            f:SetScript("OnEvent", function() table.insert(Seen, arg1) end)
            "#,
        )
        .unwrap();
    frame(
        &mut app,
        vec![super::unit_view_tests::created(
            CRAFTER,
            benilla_protocol::EntityKind::Player,
            &[],
        )],
    );
    let craft = SessionEvent::SpellLogExecute(SpellLogExecute {
        caster: CRAFTER,
        spell_id: 1,
        effects: vec![(24, vec![ExecuteLog::CreateItem { item_entry: PELT }])],
    });
    frame(&mut app, vec![craft]);
    frame(
        &mut app,
        vec![SessionEvent::PlayerName {
            guid: CRAFTER,
            name: "Crafter".into(),
            race: 1,
            gender: 0,
            class: 1,
        }],
    );
    hosted(&mut app, "Seen = {}");
    let answer = SessionEvent::ItemTemplate {
        entry: PELT,
        info: Some(Box::new(crate::items::test_template("Wolf Pelt"))),
    };
    assert_eq!(
        frame(&mut app, vec![answer]),
        ["Crafter creates Wolf Pelt."]
    );
}
