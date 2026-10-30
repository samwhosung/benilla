//! The zone walk and the guild recruitment cascade run where the reference runs them, each on the
//! state as its packet left it:
//!
//! - the zone commit's walk waits while our taxi flag is set (`0x494908`-`0x49491a`), and the
//!   flag's clear walks inside the update packet (`0x5ff565`-`0x5ff56e`);
//! - `SetGuildRecruitmentMode(1)` runs the cascade on our guild as it stands at the call
//!   (`0x49ea7b` → `0x49eb01`), and the `PLAYER_GUILDID` watcher and our create's `PostInit` run it
//!   inside the update packet (`0x5e279a`, `0x5dec1e`).

use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::Receiver;

use super::channels::ZoneChannelWalk;
use super::edit::{ChannelSlot, ChannelState};
use crate::net::ClientCommand;
use crate::player::UNIT_FLAG_TAXI_FLIGHT as TAXI;

const ME: u64 = 0x7;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo count (`0x5ddff0`): the packet whose
/// `PLAYER_COMBO_POINTS` listener reads the channels.
const PLAYER_FIELD_BYTES: u16 = 188 + 0x1028 / 4;
/// `PLAYER_GUILDID`.
const PLAYER_GUILDID: u16 = 191;
const ELWYNN: u32 = 12;
const WESTFALL: u32 = 40;
const STORMWIND: u32 = 1519;
/// The city word's row (`0x200`).
const CITY: u32 = 3459;
/// An area whose row has no name, and one named as Elwynn in capitals.
const NAMELESS: u32 = 5000;
const SHOUTING: u32 = 5001;

/// The client's world, driven a packet at a time through the dispatch, which fires each packet's
/// lanes after its handlers, and the frame's end, the zone commit's poll.
struct Client {
    app: App,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame: `packets`, each one packet, then the poll.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        let world = self.app.world_mut();
        crate::net::handlers::dispatch(world, wire(packets));
        crate::lanes::fire_in_order(world);
        // The poll's tenth tick (`0x5db906`).
        for _ in 0..10 {
            world
                .run_system_cached(crate::area::feed_zone_events)
                .expect("the poll runs");
        }
    }

    /// What went out, joins and leaves by name, emptied.
    fn wire(&self) -> Vec<String> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::JoinChannel { name, .. } => Some(format!("join {name}")),
                ClientCommand::LeaveChannel { name } => Some(format!("leave {name}")),
                _ => None,
            })
            .collect()
    }

    /// What the combo listener heard, emptied.
    fn heard(&mut self) -> Vec<String> {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            let s = s.expect("a VM");
            let heard = s.eval::<Vec<String>>("return Heard").expect("the spy");
            s.run("Heard = {}").expect("the reset");
            heard
        })
    }

    fn channels(&mut self) -> Mut<'_, ChannelState> {
        self.app.world_mut().resource_mut::<ChannelState>()
    }
}

/// The client in the world as [`ME`] (held when `held` carries our fields), standing in `zone`
/// with the session's walk live and `zone` committed, the chat cache seated, General and
/// LocalDefense in the mask and the recruitment row in the table; a VM whose `PLAYER_COMBO_POINTS`
/// listener runs the `Combo` function a test sets.
fn client(zone: u32, held: Option<&[(u16, u32)]>) -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    let world = app.world_mut();
    benilla_world::world_point::init_world_point_resources(world);
    world.insert_resource(benilla_world::terrain_stream::CurrentArea(Some(zone)));
    let mut player = crate::player::Player::default();
    player.active = true;
    world.insert_resource(player);
    let row = |flags, name: &str| benilla_formats::AreaTableRow {
        map_id: 0,
        zone_id: 0,
        explore_flag: 0,
        flags,
        faction_group_mask: 0,
        exploration_level: 0,
        name: name.into(),
    };
    world.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(vec![
            (ELWYNN, row(0, "Elwynn Forest")),
            (WESTFALL, row(0, "Westfall")),
            (STORMWIND, row(0x138, "Stormwind City")),
            (CITY, row(0x200, "City")),
            (NAMELESS, row(0, "")),
            (SHOUTING, row(0, "ELWYNN FOREST")),
        ]),
    ));
    let catalog = benilla_formats::ChatChannelsCatalog::from_rows(
        [
            (1, 0x0_0003, "General - %s", "General"),
            (22, 0x1_0003, "LocalDefense - %s", "LocalDefense"),
            (25, 0x2_0032, "GuildRecruitment - %s", "GuildRecruitment"),
        ]
        .into_iter()
        .map(
            |(id, flags, pattern, shortcut)| benilla_formats::ChatChannelRow {
                id,
                flags,
                pattern: pattern.into(),
                shortcut: shortcut.into(),
            },
        )
        .collect(),
    );
    // The zone's channels already joined, as the first commit's walk finds them.
    let zone_name = ["Elwynn Forest", "Westfall", "Stormwind City"][match zone {
        ELWYNN => 0,
        WESTFALL => 1,
        _ => 2,
    }];
    world.insert_resource(ChannelState {
        channels: catalog,
        zone_mask: Some(1 | 1 << 21),
        joined: vec![
            ChannelSlot::confirmed(&format!("General - {zone_name}"), 1).into(),
            ChannelSlot::confirmed(&format!("LocalDefense - {zone_name}"), 22).into(),
        ],
        ..Default::default()
    });
    ZoneChannelWalk::seat_for_test(world, zone);
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    if let Some(fields) = held {
        crate::test_support::hold_self_player(world, ME, fields);
    }
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Heard = {}
            function Combo() end
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_COMBO_POINTS")
            f:SetScript("OnEvent", function() Combo() end)
            "#,
        )
        .expect("the spy");
    world.insert_non_send_resource(script);
    Client { app, sent }
}

/// Each of `packets` one packet: its events, then the update end when it holds a block.
fn wire(packets: Vec<Vec<SessionEvent>>) -> Vec<SessionEvent> {
    packets
        .into_iter()
        .flat_map(|events| {
            let block = events.iter().any(|ev| {
                matches!(
                    ev,
                    SessionEvent::ObjectCreate { .. } | SessionEvent::ObjectValues { .. }
                )
            });
            events
                .into_iter()
                .chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

/// Our create, a player's (`OBJECT_FIELD_TYPE` object, unit and player), with no other field.
fn created() -> SessionEvent {
    SessionEvent::ObjectCreate {
        guid: ME,
        kind: EntityKind::Player,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(&[(2, 0x19)])
            .into_created(benilla_protocol::messages::ObjectType::Player),
    }
}

/// Our values packet of `pairs`.
fn ours(pairs: &[(u16, u32)]) -> Vec<SessionEvent> {
    vec![SessionEvent::ObjectValues {
        guid: ME,
        fields: ObjectFields::from_pairs(pairs),
    }]
}

/// A packet that sets our combo count to `n`, whose listener runs `Combo`.
fn combo(n: u32) -> Vec<SessionEvent> {
    ours(&[(PLAYER_FIELD_BYTES, n << 8)])
}

/// Set the listener's body.
fn on_combo(c: &mut Client, body: &str) {
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run(&format!("function Combo() {body} end"))
            .expect("the body")
    });
}

/// Aloft, the zone under us changes and the walk waits (`0x494908`-`0x49491a`); the taxi flag's
/// clear walks inside its packet (`0x5ff56e`), so the next packet's listener finds the renamed
/// slot and the leaves and joins went out at the clear.
#[test]
fn the_walk_waits_aloft_and_runs_at_the_taxi_flags_clear() {
    let mut c = client(ELWYNN, Some(&[(FIELD_UNIT_FLAGS, TAXI)]));
    c.channels().joined = vec![
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
        ChannelSlot::confirmed("LocalDefense - Elwynn Forest", 22).into(),
    ];
    c.app
        .world_mut()
        .insert_resource(benilla_world::terrain_stream::CurrentArea(Some(WESTFALL)));
    c.frame(vec![]);
    assert!(c.wire().is_empty(), "aloft over Westfall: no walk");
    on_combo(
        &mut c,
        r#"table.insert(Heard, tostring((GetChannelName("General - Westfall"))))"#,
    );
    c.frame(vec![ours(&[(FIELD_UNIT_FLAGS, 0)]), combo(2)]);
    assert_eq!(
        c.heard(),
        ["1"],
        "the listener after the clear finds the renamed slot"
    );
    assert_eq!(
        c.wire(),
        [
            "leave General - Elwynn Forest",
            "join General - Westfall",
            "leave LocalDefense - Elwynn Forest",
            "join LocalDefense - Westfall",
        ]
    );
    c.frame(vec![]);
    assert!(c.wire().is_empty(), "the poll finds the zone walked");
}

/// The taxi arm's walk runs at the flags watcher's place in the notify walk (`0x5ff56e`), after
/// the watchers of lower fields: in one block that sets our health and clears the taxi flag, the
/// health listener reads the slots as they stood, and the next packet's listener the renamed one.
#[test]
fn a_lower_fields_listener_reads_the_slots_ahead_of_the_taxi_walk() {
    let mut c = client(
        ELWYNN,
        Some(&[(FIELD_UNIT_FLAGS, TAXI), (FIELD_UNIT_HEALTH, 100)]),
    );
    c.channels().joined = vec![
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
        ChannelSlot::confirmed("LocalDefense - Elwynn Forest", 22).into(),
    ];
    c.app
        .world_mut()
        .insert_resource(benilla_world::terrain_stream::CurrentArea(Some(WESTFALL)));
    c.frame(vec![]);
    assert!(c.wire().is_empty(), "aloft over Westfall: no walk");
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        let s = s.expect("a VM");
        s.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            held: [(ME, 0)].into_iter().collect(),
            ..Default::default()
        });
        s.run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                table.insert(Heard, "health " .. tostring((GetChannelName("General - Westfall"))))
            end)
            "#,
        )
        .expect("the health listener")
    });
    on_combo(
        &mut c,
        r#"table.insert(Heard, "combo " .. tostring((GetChannelName("General - Westfall"))))"#,
    );
    c.frame(vec![
        ours(&[(FIELD_UNIT_HEALTH, 90), (FIELD_UNIT_FLAGS, 0)]),
        combo(2),
    ]);
    assert_eq!(c.heard(), ["health 0", "combo 1"]);
    assert_eq!(
        c.wire().len(),
        4,
        "the leaves and joins went out at the clear"
    );
}

/// `SetGuildRecruitmentMode(1)` in a listener runs the cascade on our guild as its packet left it
/// (`0x49eb01`): a leave of the guild ahead of it joins, where the plan the frame before left was
/// a guilded leave; the guild's change after it runs the cascade inside its own packet
/// (`0x5e279a`), on AUTO now, and leaves. Off AUTO the watcher does nothing (`0x49ea96`).
#[test]
fn the_cascade_reads_our_guild_at_the_call_and_at_the_watcher() {
    let mut c = client(STORMWIND, Some(&[(PLAYER_GUILDID, 5)]));
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        let s = s.expect("a VM");
        s.set_guild_recruitment_mode(0);
        s.set_guild_recruitment_plan(benilla_ui::script::RecruitmentPlan::Leave(
            "GuildRecruitment - City".into(),
        ));
    });
    on_combo(&mut c, "SetGuildRecruitmentMode(1)");
    c.frame(vec![
        ours(&[(PLAYER_GUILDID, 0)]),
        combo(2),
        ours(&[(PLAYER_GUILDID, 6)]),
    ]);
    assert_eq!(
        c.wire(),
        [
            "join GuildRecruitment - City",
            "leave GuildRecruitment - City"
        ]
    );
}

/// Our create's `PostInit` runs the cascade behind the guild tag, inside the update packet
/// (`0x5dec19`, `0x5dec1e`): unguilded in a capital on AUTO, its join takes its slot there, so
/// the next packet's listener finds it.
#[test]
fn our_create_runs_the_cascade_inside_its_packet() {
    let mut c = client(STORMWIND, None);
    on_combo(
        &mut c,
        r#"table.insert(Heard, tostring((GetChannelName("GuildRecruitment - City"))))"#,
    );
    c.frame(vec![vec![created()], combo(2)]);
    assert_eq!(c.heard(), ["3"], "after General and LocalDefense");
    assert_eq!(c.wire(), ["join GuildRecruitment - City"]);
}

/// The poll's ticks (`0x5db900`), the tenth committing.
fn tick(c: &mut Client, n: usize) {
    for _ in 0..n {
        c.app
            .world_mut()
            .run_system_cached(crate::area::feed_zone_events)
            .expect("the feed runs");
    }
}

/// Elwynn committed and walked, its slots joined, the wire empty.
fn settled_in_elwynn() -> Client {
    let mut c = client(ELWYNN, Some(&[]));
    c.channels().joined = vec![
        ChannelSlot::confirmed("General - Elwynn Forest", 1).into(),
        ChannelSlot::confirmed("LocalDefense - Elwynn Forest", 22).into(),
    ];
    tick(&mut c, 10);
    c.wire();
    c
}

fn stand_in(c: &mut Client, area: u32) {
    c.app
        .world_mut()
        .insert_resource(benilla_world::terrain_stream::CurrentArea(Some(area)));
}

/// The walk is `UpdateZoneText`'s (`0x494931`), run by the poll's tenth tick: a zone line crossed
/// and crossed back between two polls changes nothing on the wire.
#[test]
fn a_zone_line_crossed_and_back_between_polls_walks_nothing() {
    let mut c = settled_in_elwynn();
    stand_in(&mut c, WESTFALL);
    tick(&mut c, 3);
    stand_in(&mut c, ELWYNN);
    tick(&mut c, 7);
    assert!(c.wire().is_empty(), "the poll saw Elwynn both times");
}

/// The channels change at the commit's tick, not on the frame the area moved.
#[test]
fn the_walk_runs_at_the_commits_tick_not_before_it() {
    let mut c = settled_in_elwynn();
    stand_in(&mut c, WESTFALL);
    tick(&mut c, 9);
    assert!(c.wire().is_empty(), "nine ticks return");
    tick(&mut c, 1);
    assert_eq!(
        c.wire(),
        [
            "leave General - Elwynn Forest",
            "join General - Westfall",
            "leave LocalDefense - Elwynn Forest",
            "join LocalDefense - Westfall",
        ]
    );
}

/// An area with no row commits nothing (the commit keeps the last zone), so the zone
/// stays walked and the guild-recruitment cascade reads the last zone's row (`0x49ead7`).
#[test]
fn an_area_with_no_row_keeps_the_last_zone_and_the_cascade_reads_it() {
    let mut c = client(STORMWIND, Some(&[]));
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").set_guild_recruitment_mode(0);
    });
    tick(&mut c, 10);
    c.wire();
    stand_in(&mut c, 4242);
    tick(&mut c, 10);
    assert!(c.wire().is_empty(), "no row, no walk");
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run("SetGuildRecruitmentMode(1)")
            .expect("the verb")
    });
    assert_eq!(
        c.wire(),
        ["join GuildRecruitment - City"],
        "the cascade finds Stormwind, a capital, committed"
    );
}

/// The walk comes before the commit's `ZONE_CHANGED_NEW_AREA` (`0x494931` before `0x49493f`): its
/// listener finds the renamed slot.
#[test]
fn the_walk_runs_before_the_commits_new_area_listeners() {
    let mut c = settled_in_elwynn();
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM")
            .run(
                r#"
                local f = CreateFrame("Frame")
                f:RegisterEvent("ZONE_CHANGED_NEW_AREA")
                f:SetScript("OnEvent", function()
                    table.insert(Heard, tostring((GetChannelName("General - Westfall"))))
                end)
                "#,
            )
            .expect("the listener")
    });
    tick(&mut c, 9);
    stand_in(&mut c, WESTFALL);
    tick(&mut c, 1);
    assert_eq!(c.heard(), ["1"]);
}

const WESTFALL_WALK: [&str; 4] = [
    "leave General - Elwynn Forest",
    "join General - Westfall",
    "leave LocalDefense - Elwynn Forest",
    "join LocalDefense - Westfall",
];

/// A cinematic running (`0x49491c`) or latched (`0x494928` → `0x5efc90`) holds the commit's walk
/// off; the commit still stores the zone, and EndCinematic's walk (`0x48f1d0`) walks it.
#[test]
fn a_cinematic_holds_the_commits_walk_and_its_end_walks_the_committed_zone() {
    for held in [
        crate::cinematic::Cinematic::playing_for_test(),
        crate::cinematic::Cinematic::pending_for_test(41),
    ] {
        let mut c = settled_in_elwynn();
        c.app.world_mut().insert_resource(held);
        stand_in(&mut c, WESTFALL);
        tick(&mut c, 10);
        assert!(c.wire().is_empty(), "no walk under a cinematic");
        assert_eq!(
            c.app.world().resource::<crate::area::CommittedZone>().0,
            Some(WESTFALL),
            "the zone is stored all the same"
        );
        c.app
            .world_mut()
            .insert_resource(crate::cinematic::Cinematic::ended_for_test());
        c.app
            .world_mut()
            .run_system_cached(super::channels::walk_when_cinematic_ends)
            .expect("the end walk runs");
        assert_eq!(
            c.wire(),
            WESTFALL_WALK,
            "EndCinematic walks the stored zone"
        );
    }
}

/// A reload builds a new VM and keeps the stored zone and the real zone text (`0xb4b404`,
/// `0x495664` → `0x490bd0`): its first commit finds no change, so it walks nothing.
#[test]
fn a_reload_keeps_the_real_zone_text_and_walks_nothing() {
    let mut c = settled_in_elwynn();
    c.channels().joined = vec![ChannelSlot::confirmed("General - Stale", 1).into()];
    c.app
        .world_mut()
        .insert_non_send_resource(UiScript::new().expect("a VM"));
    tick(&mut c, 10);
    assert!(c.wire().is_empty(), "Elwynn was walked before the reload");
}

/// An empty name leaves the real zone text as it was (`0x49489a` → `0x494936`), so the next
/// name is compared with the one before it, ignoring case (`0x64a4c0`).
#[test]
fn an_empty_or_recased_name_walks_nothing() {
    let mut c = settled_in_elwynn();
    // Stale names, so that any walk shows on the wire.
    c.channels().joined = vec![ChannelSlot::confirmed("General - Stale", 1).into()];
    stand_in(&mut c, NAMELESS);
    tick(&mut c, 10);
    stand_in(&mut c, ELWYNN);
    tick(&mut c, 10);
    assert!(c.wire().is_empty(), "Elwynn again after an unnamed zone");
    stand_in(&mut c, SHOUTING);
    tick(&mut c, 10);
    assert!(c.wire().is_empty(), "the same name in capitals");
}

/// A hit group the indoor dedup passes never reaches `0x494780` (`0x67e510`): the zone is not
/// stored.
#[test]
fn the_indoor_dedup_stores_no_zone() {
    use benilla_world::wmo_portal::{CurrentAreaInterior, WmoInteriorKeys};
    let mut c = settled_in_elwynn();
    let keys = WmoInteriorKeys {
        wmo_id: 1,
        name_set: 0,
        group_area_id: 7,
    };
    let room = benilla_formats::WmoArea {
        id: 1,
        sound_provider: [0; 2],
        ambience: 0,
        zone_music: 0,
        intro_sound: 0,
        area_table_id: 0,
        name: String::new(),
    };
    let world = c.app.world_mut();
    benilla_world::wmo_portal::seat_wmo_areas(
        world,
        benilla_formats::WmoAreaCatalog::from_rows([(1, 0, 7, room)]),
    );
    world.insert_resource(CurrentAreaInterior(Some(keys)));
    tick(&mut c, 10);
    stand_in(&mut c, WESTFALL);
    tick(&mut c, 10);
    assert_eq!(
        c.app.world().resource::<crate::area::CommittedZone>().0,
        Some(ELWYNN),
        "the same room: the update is skipped"
    );
}
