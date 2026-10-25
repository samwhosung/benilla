//! The chat line "You are now <rank> with <faction>." end to end: the wire handlers queue it
//! against the store as it stood, and the feed prints it as `CHAT_MSG_SYSTEM` from the VM's own
//! `GlobalStrings.lua` (`0x62c670`). A first contact is `SMSG_SET_FACTION_VISIBLE` (vmangos
//! `ReputationMgr.cpp:298`); a rank change rides `SMSG_SET_FACTION_STANDING`.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::{on_reputation_delta, on_reputations};
use crate::field_watch::{FieldNotice, NoticeLane};
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
        .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0))
        .init_resource::<NoticeLane>();
    // A player (`OBJECT_FIELD_TYPE` 0x19) whose `UNIT_FIELD_SUMMON` (8-9) is [`PET`].
    let me = app
        .world_mut()
        .spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x19),
                (8, PET as u32),
                (9, (PET >> 32) as u32),
                (36, HUMAN_WARRIOR),
            ])),
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

/// The player's pet, a held unit it summoned; a totem it summoned, and a boar nobody owns.
const PET: u64 = 0xF140_0000_0000_0001;
const TOTEM: u64 = 0xF130_0000_0000_0002;
const BOAR: u64 = 0xF130_0000_0000_0003;
/// A party member's guid, which the object manager does not hold.
const FAR_MEMBER: u64 = 0x77;

/// Hold [`PET`], [`TOTEM`] and [`BOAR`] in the object manager (`OBJECT_FIELD_TYPE` 0x09), the
/// first two with `UNIT_FIELD_SUMMONEDBY` (12) the player.
fn hold_units(app: &mut App) {
    for (guid, summoner) in [(PET, ME), (TOTEM, ME), (BOAR, 0)] {
        let e = app
            .world_mut()
            .spawn(ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x09),
                (12, summoner as u32),
            ])))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(guid, e);
    }
}

/// The notices the handlers left, taken.
fn notices(app: &mut App) -> Vec<FieldNotice> {
    app.world_mut()
        .resource_mut::<NoticeLane>()
        .drain()
        .collect()
}

fn faction(guid: u64) -> FieldNotice {
    FieldNotice::Signal {
        guid,
        id: crate::field_watch::id::UNIT_FACTION,
    }
}

fn update_faction() -> FieldNotice {
    FieldNotice::Global {
        id: crate::field_watch::id::UPDATE_FACTION,
    }
}

/// `RefreshReaction` on the active player (`0x606e20`): `UNIT_FACTION` for it and its `SUMMON`,
/// then for each held unit the sweep reaches, its own summons skipped (`0x606f76`-`0x606f9e`), so
/// neither its totem nor a party member the object manager does not hold hears it.
fn refresh() -> Vec<FieldNotice> {
    vec![faction(ME), faction(PET), faction(BOAR)]
}

/// A standing message refreshes the player's reaction once when an entry moved a rank (`0x4d58ac`
/// latch, `0x4d5905` → `0x606e20`), between the sort's `UPDATE_FACTION` and its own
/// (`0x4d58c8`, `0x4d5984`); a move inside a rank refreshes nothing.
#[test]
fn a_rank_change_refreshes_the_players_reaction_once_per_message() {
    use benilla_formats::faction_flags::{HIDDEN, VISIBLE};
    let mut app = synthetic_app();
    hold_units(&mut app);
    let _ = FAR_MEMBER;
    seat(&mut app, VISIBLE, 0);
    let _ = notices(&mut app);
    delta(&mut app, SLOT as u32, 100);
    assert_eq!(notices(&mut app), [update_faction()], "Neutral to Neutral");
    delta(&mut app, SLOT as u32, 3000);
    let mut friendly = refresh();
    friendly.push(update_faction());
    assert_eq!(notices(&mut app), friendly, "Neutral to Friendly");
    send(
        &mut app,
        SessionEvent::ReputationDelta {
            standings: vec![(SLOT as u32, -3500), (SLOT as u32, 9000)],
        },
    );
    // Hostile sets the at-war bit, so the flag byte moved and the list sorts first.
    let mut two = vec![update_faction()];
    two.extend(refresh());
    two.push(update_faction());
    assert_eq!(notices(&mut app), two, "two moves, one refresh");
    // The latch tests no flag (`0x4d5880`): a `HIDDEN` faction moves it too.
    seat(&mut app, VISIBLE | HIDDEN, 0);
    delta(&mut app, SLOT as u32, -1001);
    assert_eq!(notices(&mut app), friendly, "{HIDDEN:#x}");
}

/// The forced table refreshes the player's reaction on every arrival (`0x4d5a5e`), and signals
/// no `UPDATE_FACTION`.
#[test]
fn a_forced_table_refreshes_the_players_reaction_through_the_sweep() {
    let mut app = synthetic_app();
    hold_units(&mut app);
    let table = || SessionEvent::ForcedReactions {
        reactions: vec![(72, 0)],
    };
    app.world_mut()
        .run_system_cached_with(on_reputations, table())
        .unwrap();
    app.world_mut()
        .run_system_cached_with(on_reputations, table())
        .unwrap();
    let mut twice = refresh();
    twice.extend(refresh());
    assert_eq!(notices(&mut app), twice);
}

/// `UPDATE_FACTION` where each message signals it: every standing message at its end, an empty
/// one too (`0x4d597f`), and its sort first when a flag byte moved (`0x4d58c8`); every at-war
/// message whether or not the bit moved (`0x4d56f6`); a visible message only for a slot it lists
/// (`0x4d574a`); the initialize message once without the player held (`0x4d5699`).
#[test]
fn update_faction_fires_where_each_message_signals_it() {
    use benilla_formats::faction_flags::{AT_WAR, VISIBLE};
    let mut app = synthetic_app();
    seat(&mut app, VISIBLE, 0);
    let _ = notices(&mut app);
    send(
        &mut app,
        SessionEvent::ReputationDelta {
            standings: Vec::new(),
        },
    );
    assert_eq!(notices(&mut app), [update_faction()], "an empty message");
    delta(&mut app, 30, 5);
    assert_eq!(
        notices(&mut app),
        [update_faction(), update_faction()],
        "a slot first listed moved its flag byte"
    );
    for wire in [AT_WAR, AT_WAR, 0] {
        send(
            &mut app,
            SessionEvent::ReputationAtWar {
                list_id: SLOT as u32,
                flags: wire,
            },
        );
        assert_eq!(notices(&mut app), [update_faction()], "{wire:#x}");
    }
    visible(&mut app, 40);
    assert_eq!(notices(&mut app), [update_faction()], "newly listed");
    visible(&mut app, 40);
    assert!(notices(&mut app).is_empty(), "listed already");
    let me = app
        .world_mut()
        .query_filtered::<Entity, With<SelfPlayer>>()
        .single(app.world())
        .unwrap();
    app.world_mut().despawn(me);
    app.world_mut()
        .run_system_cached_with(
            on_reputations,
            SessionEvent::Reputations {
                standings: vec![(0, 0); 64],
            },
        )
        .unwrap();
    assert_eq!(
        notices(&mut app),
        [update_faction()],
        "the initialize message"
    );
}

/// With the player held, the initialize message's rebuild sorts twice, each sort signalling: the
/// Inactive header's fold (`0x4d55d9` → `0x4d6457` → `0x4d5dab`) and the sort after it
/// (`0x4d55de`); then the message's own (`0x4d5699`).
#[test]
fn the_initialize_message_signals_both_rebuild_sorts_and_its_own() {
    let mut app = synthetic_app();
    let _ = notices(&mut app);
    app.world_mut()
        .run_system_cached_with(
            on_reputations,
            SessionEvent::Reputations {
                standings: vec![(0, 0); 64],
            },
        )
        .unwrap();
    assert_eq!(
        notices(&mut app),
        [update_faction(), update_faction(), update_faction()]
    );
}

/// Each message hands the pane's list what its handler does to it: a visible message the slot it
/// lists (`0x4d5738`), a standing message the slots its entries made visible and whether a flag
/// byte moved (`0x4d5826`, `0x4d58c8`), an at-war message an edit of nothing, as it sorts nothing
/// (`0x4d56b0`).
#[test]
fn each_message_hands_the_list_its_own_ops() {
    use benilla_formats::faction_flags::{AT_WAR, VISIBLE};
    use benilla_ui::script::ReputationListOp as Op;
    let mut app = synthetic_app();
    seat(&mut app, VISIBLE, 0);
    let ops = |app: &mut App| app.world_mut().resource_mut::<Reputations>().take_ops();
    let _ = ops(&mut app);
    visible(&mut app, 40);
    visible(&mut app, 40);
    assert_eq!(
        ops(&mut app),
        [
            Op::Edit {
                listed: vec![40],
                sort: false
            },
            Op::Edit {
                listed: vec![],
                sort: true
            }
        ],
        "the adder ahead of the flag write, the sort after it"
    );
    send(
        &mut app,
        SessionEvent::ReputationAtWar {
            list_id: SLOT as u32,
            flags: AT_WAR,
        },
    );
    assert_eq!(
        ops(&mut app),
        [Op::Edit {
            listed: vec![],
            sort: false
        }],
        "no row listed, no sort"
    );
    delta(&mut app, 30, 5);
    assert_eq!(
        ops(&mut app),
        [
            Op::Edit {
                listed: vec![30],
                sort: false
            },
            Op::Edit {
                listed: vec![],
                sort: true
            }
        ],
        "the adder at the listing, then the sort once the entries are in"
    );
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

/// `SMSG_SET_FACTION_STANDING` triggers Reputation (`0x4d5973`-`0x4d597a`) with the active
/// player held at level 10 or more (`0x4d596d cmp [ecx+0x70],0xa`), after its entries and
/// before its `UPDATE_FACTION`; below 10 nothing.
#[test]
fn a_standing_change_at_level_ten_triggers_the_reputation_tutorial() {
    for (level, fired) in [(9, vec![]), (10, vec![0x10])] {
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
            .init_resource::<crate::tutorial::Tutorials>()
            .insert_resource(crate::net::NetCommands(crossbeam_channel::unbounded().0));
        app.world()
            .resource::<crate::tutorial::Tutorials>()
            .lock()
            .apply_flags(&[0u8; 32]);
        // `UNIT_FIELD_LEVEL`.
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(ObjectFields::from_pairs(&[
                    (36, HUMAN_WARRIOR),
                    (34, level),
                ])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        app.world_mut()
            .run_system_cached_with(
                on_reputation_delta,
                SessionEvent::ReputationDelta {
                    standings: vec![(0, 100)],
                },
            )
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<crate::tutorial::Tutorials>()
                .take_fired(),
            fired,
            "level {level}"
        );
    }
}
