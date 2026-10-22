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
        SessionEvent::ReputationVisible { .. } | SessionEvent::ReputationAtWar { .. } => {
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
        .push_xp_gain(0, 100, 100, 1.0);
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

/// Stormwind's slot in [`synthetic_app`].
const SLOT: usize = 19;

/// The handlers over a logged-in human warrior and one reputation faction, Stormwind (72) on slot
/// [`SLOT`] at base 0 for a human; runs without client data.
fn synthetic_app() -> App {
    use benilla_formats::FactionCatalog;
    let catalog = FactionCatalog::from_rows(Default::default(), Default::default())
        .with_reputation(72, SLOT as i32, 1, 0);
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
        .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0));
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
    app
}

fn store(app: &App) -> &Reputations {
    app.world().resource::<Reputations>()
}

/// Seat Stormwind's slot at `flags` and wire standing `standing`.
fn seat(app: &mut App, flags: u8, standing: i32) {
    let mut slots = vec![(0, 0); SLOT + 1];
    slots[SLOT] = (flags, standing);
    app.world_mut()
        .resource_mut::<Reputations>()
        .set_slots(&slots);
}

/// `SMSG_SET_FACTION_STANDING`'s war rule (`0x4d5833`-`0x4d5849`), on the rank after the entry
/// (`0x4d63a0`): below 2, Hostile or Hated, sets the at-war bit whatever the rank was; a rank that
/// rose clears it; any other move leaves it as it was, the pane's or the server's.
#[test]
fn a_standing_sets_the_war_bit_below_unfriendly_and_a_rise_clears_it() {
    use benilla_formats::faction_flags::{AT_WAR, VISIBLE};
    let mut app = synthetic_app();
    let war = |app: &App| store(app).flags(SLOT) & AT_WAR != 0;
    for (flags, from, to, at_war, why) in [
        (VISIBLE, 0, -4000, true, "Neutral to Hostile sets it"),
        (
            VISIBLE,
            -9000,
            -8000,
            true,
            "Hated, its rank unmoved, sets it",
        ),
        (
            VISIBLE | AT_WAR,
            -4000,
            -2000,
            false,
            "Hostile up to Unfriendly clears it",
        ),
        (
            VISIBLE | AT_WAR,
            -2000,
            -1500,
            true,
            "Unfriendly unmoved keeps it",
        ),
        (
            VISIBLE | AT_WAR,
            100,
            -100,
            true,
            "Neutral down to Unfriendly keeps it",
        ),
        (
            VISIBLE | AT_WAR,
            -100,
            100,
            false,
            "Unfriendly up to Neutral clears it",
        ),
        (
            VISIBLE,
            -100,
            100,
            false,
            "a rise with the bit clear leaves it clear",
        ),
        (VISIBLE | AT_WAR, 9000, 21000, false, "any rise clears it"),
    ] {
        seat(&mut app, flags, from);
        delta(&mut app, SLOT as u32, to);
        assert_eq!(war(&app), at_war, "{why}");
        assert_eq!(store(&app).standing(SLOT), to);
    }
}

/// The rule reads the faction's rank (`0x4d5620`, `0x4d6370`): a slot no faction holds stands at
/// 0, Neutral, before and after, so its bit stays.
#[test]
fn a_slot_no_faction_holds_keeps_its_war_bit() {
    use benilla_formats::faction_flags::{AT_WAR, VISIBLE};
    let mut app = synthetic_app();
    let mut slots = vec![(0, 0); 31];
    slots[30] = (VISIBLE | AT_WAR, -100);
    app.world_mut()
        .resource_mut::<Reputations>()
        .set_slots(&slots);
    delta(&mut app, 30, -9000);
    assert_eq!(store(&app).flags(30), VISIBLE | AT_WAR);
}

/// A rank change refreshes the active player's reaction once per message (`0x4d58ac` latch,
/// `0x4d5905` → `0x606e20`), however many entries moved; a move inside a rank refreshes nothing.
#[test]
fn a_rank_change_refreshes_the_players_reaction_once_per_message() {
    use benilla_formats::faction_flags::VISIBLE;
    let mut app = synthetic_app();
    seat(&mut app, VISIBLE, 0);
    let refreshes = |app: &App| store(app).player_refreshes();
    let before = refreshes(&app);
    delta(&mut app, SLOT as u32, 100);
    assert_eq!(refreshes(&app), before, "Neutral to Neutral");
    delta(&mut app, SLOT as u32, 3000);
    assert_eq!(refreshes(&app), before + 1, "Neutral to Friendly");
    send(
        &mut app,
        SessionEvent::ReputationDelta {
            standings: vec![(SLOT as u32, -3500), (SLOT as u32, 9000)],
        },
    );
    assert_eq!(refreshes(&app), before + 2, "two moves, one message");
}

/// `SMSG_SET_FACTION_ATWAR` (`0x4d56b0`) copies bit `0x02` of its byte into the slot's at-war bit
/// and touches no other bit (`0x4d56d9`-`0x4d56e9`).
#[test]
fn the_at_war_message_copies_bit_1_alone() {
    use benilla_formats::faction_flags::{AT_WAR, PEACE_FORCED, VISIBLE};
    let mut app = synthetic_app();
    seat(&mut app, VISIBLE, 0);
    let at_war = |app: &mut App, flags: u8| {
        send(
            app,
            SessionEvent::ReputationAtWar {
                list_id: SLOT as u32,
                flags,
            },
        );
    };
    at_war(&mut app, AT_WAR);
    assert_eq!(store(&app).flags(SLOT), VISIBLE | AT_WAR);
    at_war(&mut app, !AT_WAR);
    assert_eq!(
        store(&app).flags(SLOT),
        VISIBLE,
        "the other bits of the byte are read nowhere"
    );
    seat(&mut app, VISIBLE | PEACE_FORCED, 0);
    at_war(&mut app, 0xff);
    assert_eq!(
        store(&app).flags(SLOT),
        VISIBLE | PEACE_FORCED | AT_WAR,
        "the message tests no peace-forced bit"
    );
}
