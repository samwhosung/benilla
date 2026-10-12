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
        .init_resource::<crate::ui_reputation::StandingLines>()
        .insert_resource(crate::target::Factions::from_catalog(catalog))
        .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0))
        .insert_non_send_resource(script)
        .add_systems(Update, crate::ui_reputation::feed_standing_lines);
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

/// Send one packet's event and run a frame; the lines printed since the last call.
fn send(app: &mut App, ev: SessionEvent) -> Vec<String> {
    // The handler `register` binds to the event's kind, run as the bridge runs it.
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
    app.update();
    let lines = app
        .world()
        .resource::<crate::ui_chat::ChatLog>()
        .pending_lines();
    app.world_mut()
        .insert_resource(crate::ui_chat::ChatLog::default());
    lines
}

#[test]
fn first_contact_a_rank_change_and_a_repeat_through_the_wire_handlers() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    let visible = |app: &mut App| send(app, SessionEvent::ReputationVisible { list_id: slot });
    let delta = |app: &mut App, standing: i32| {
        send(
            app,
            SessionEvent::ReputationDelta {
                standings: vec![(slot, standing)],
            },
        )
    };

    // First contact (what vmangos sends on meeting a faction): the human's base 4000 reads
    // Friendly, and the line names it.
    assert_eq!(visible(&mut app), ["You are now Friendly with Stormwind."]);
    // The same packet again is the dedup return: the slot is already listed.
    assert!(
        visible(&mut app).is_empty(),
        "already listed prints nothing"
    );

    // 4000 → 4100: still Friendly.
    assert!(
        delta(&mut app, 100)
            .iter()
            .all(|l| !l.starts_with("You are now")),
        "no rank change, no line"
    );
    // 4100 → 9100: Honored, a rank change.
    assert_eq!(
        delta(&mut app, 5100)
            .into_iter()
            .filter(|l| l.starts_with("You are now"))
            .collect::<Vec<_>>(),
        ["You are now Honored with Stormwind."]
    );
}

/// A faction a standing packet reveals prints once, and not before the self object streams in.
#[test]
fn the_standing_packet_that_lists_a_faction_prints_it_once() {
    let Some((mut app, slot)) = app() else {
        return;
    };
    let lines = send(
        &mut app,
        SessionEvent::ReputationDelta {
            standings: vec![(slot, 5100)],
        },
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("You are now"))
            .collect::<Vec<_>>(),
        ["You are now Honored with Stormwind."],
        "one line, the rank after the gain, though the rank also moved"
    );
}
