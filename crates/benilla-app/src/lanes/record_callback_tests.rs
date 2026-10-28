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
        // The read misses the pelt's record and holds the callback.
        let kind: String = s
            .eval("local _, kind = GetQuestLogLeaderBoard(1, 2) return kind")
            .unwrap();
        assert_eq!(kind, "item");
        s.run("Seen = {}").unwrap();
    }
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
