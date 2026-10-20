//! The chat line "You are now <rank> with <faction>." end to end: the wire handlers queue it
//! against the store as it stood, and the feed prints it as `CHAT_MSG_SYSTEM` from the VM's own
//! `GlobalStrings.lua` (`0x62c670`). A first contact is `SMSG_SET_FACTION_VISIBLE` (vmangos
//! `ReputationMgr.cpp:298`); a rank change rides `SMSG_SET_FACTION_STANDING`.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::{on_reputation_delta, on_reputations};
use crate::net::{ObjectStore, Reputations, SelfGuid, SelfPlayer};
use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;

const ME: u64 = 0x42;
/// `UNIT_FIELD_BYTES_0` of a male human warrior.
const HUMAN_WARRIOR: u32 = 1 | (1 << 8);

/// The handlers and the feed over a logged-in human warrior who has met nothing, on the real
/// DBC; `None` without client data. The second value is Stormwind's wire slot.
fn app() -> Option<(App, u32)> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    let catalog = benilla_formats::load_faction_catalog(&mut chain).expect("factions");
    let slot = u32::try_from(catalog.reputation_faction(72)?.rep_index).ok()?;

    let script = UiScript::new().expect("VM");
    script
        .run(
            r#"FACTION_STANDING_CHANGED = "You are now %s with %s."
               FACTION_STANDING_LABEL4 = "Neutral"
               FACTION_STANDING_LABEL5 = "Friendly"
               FACTION_STANDING_LABEL6 = "Honored""#,
        )
        .unwrap();

    let mut app = App::new();
    app.init_resource::<crate::net::GuidIndex>()
        .insert_resource(SelfGuid(Some(ME)))
        .init_resource::<crate::net::NetStatus>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<crate::items::Items>()
        .init_resource::<crate::spell::Cooldowns>()
        .init_resource::<Reputations>()
        .init_resource::<crate::net::ServerTime>()
        .init_resource::<crate::net::ServerWallClock>()
        .init_resource::<crate::net::HomeBind>()
        .init_resource::<crate::net::Proficiencies>()
        .init_resource::<crate::net::DroppedOpcodes>()
        .init_resource::<crate::ui_quest::QuestGiver>()
        .init_resource::<crate::ui_chat::ChatLog>()
        .insert_resource(crate::target::Factions::from_catalog(catalog))
        .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0))
        .insert_non_send_resource(script);
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[(36, HUMAN_WARRIOR)])),
        ))
        .id();
    app.world_mut()
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(ME, me);
    Some((app, slot))
}

/// Run one packet's handler as the bridge runs it, in wire order; nothing drains the chat log.
fn send(app: &mut App, ev: SessionEvent) {
    // The handler `register` binds to the event's kind.
    match ev {
        SessionEvent::ReputationVisible { .. } => {
            app.world_mut()
                .run_system_cached_with(on_reputations, ev)
                .unwrap();
        }
        SessionEvent::ReputationDelta { .. } => {
            app.world_mut()
                .run_system_cached_with(on_reputation_delta, ev)
                .unwrap();
        }
        _ => panic!("not a reputation event"),
    }
}

fn chat(app: &App) -> &crate::ui_chat::ChatLog {
    app.world().resource::<crate::ui_chat::ChatLog>()
}

fn visible(app: &mut App, slot: u32) {
    send(app, SessionEvent::ReputationVisible { list_id: slot });
}

fn delta(app: &mut App, slot: u32, standing: i32) {
    send(
        app,
        SessionEvent::ReputationDelta {
            standings: vec![(slot, standing)],
        },
    );
}

fn xp_gain(app: &mut App) {
    app.world_mut()
        .resource_mut::<crate::ui_chat::ChatLog>()
        .push_xp_gain(0, 100, 100);
}

fn rank_lines(app: &App) -> Vec<(String, u8, bool)> {
    chat(app).pending_standing()
}

#[test]
fn first_contact_a_rank_change_and_a_repeat_through_the_wire_handlers() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    // First contact (what vmangos sends on meeting a faction): the human's base 4000 reads
    // Friendly, and the line names it.
    visible(&mut app, slot);
    assert_eq!(rank_lines(&app), [("Stormwind".to_string(), 4, false)]);
    // The same packet again is the dedup return: the slot is already listed.
    visible(&mut app, slot);
    assert_eq!(rank_lines(&app).len(), 1, "already listed prints nothing");

    // 4000 → 4100: still Friendly.
    delta(&mut app, slot, 100);
    assert_eq!(rank_lines(&app).len(), 1, "no rank change, no line");
    // 4100 → 9100: Honored, a rank change.
    delta(&mut app, slot, 5100);
    assert_eq!(
        rank_lines(&app).last(),
        Some(&("Stormwind".to_string(), 5, false))
    );
    assert_eq!(rank_lines(&app).len(), 2);
}

/// A faction a standing packet reveals prints once, whatever else the rank did.
#[test]
fn the_standing_packet_that_lists_a_faction_prints_it_once() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    delta(&mut app, slot, 5100);
    assert_eq!(
        rank_lines(&app),
        [("Stormwind".to_string(), 5, false)],
        "one line, the rank after the gain, though the rank also moved"
    );
}

/// The lines keep wire order against the rest of the chat: a standing packet prints "increased
/// by" (`0x4d6330`), then the rank (`0x4d5896`), and an XP award that arrives after it in the same
/// batch prints behind both. On first contact the VISIBLE packet's rank line leads the state's
/// "increased by" (vmangos `ReputationMgr.cpp:264-266`).
#[test]
fn the_rank_line_keeps_its_place_in_the_packet_batch() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    delta(&mut app, slot, 5100);
    xp_gain(&mut app);
    // A rank-moving standing packet for a faction already listed.
    assert_eq!(chat(&app).pending_kinds(), ["combat", "standing", "xp"]);

    let Some((mut app, slot)) = self::app() else {
        return;
    };
    visible(&mut app, slot);
    delta(&mut app, slot, 100);
    assert_eq!(chat(&app).pending_kinds(), ["standing", "combat"]);
}

/// The wire handlers carry the player's real gender to the line.
#[test]
fn a_female_player_asks_for_the_female_label() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    // `UNIT_FIELD_BYTES_0` byte 2 is the gender; a drunk female is still female.
    let me = app
        .world_mut()
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .entity_mut(me)
        .insert(ObjectStore(ObjectFields::from_pairs(&[(
            36,
            HUMAN_WARRIOR | (1 << 16),
        )])));
    visible(&mut app, slot);
    assert_eq!(rank_lines(&app), [("Stormwind".to_string(), 4, true)]);
}
