//! What a listener reads of a unit, as the packets before it left it. One packet's handler runs
//! whole before the next (`0x537aa0` calls it at `0x537ae5`, `0x5384d0` takes them in order), its
//! watchers fire inside it (`0x465570` → `0x4655dc`), and the getters read the live object:
//! `UnitHealth` `[obj+0x110]+0x40` (`0x5174d0`, at `0x517553`), `UnitMana` the power its type
//! byte picks (`0x517670`, at `0x5176fc`). So each listener reads its own packet's value, however
//! many packets the frame holds.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{EnvironmentalDamageLog, ObjectType};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::{UiScript, UnitGuids};
use bevy::prelude::*;

const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;
const BOAR: u64 = 0xF130_0000_0045_0002;
/// `UNIT_FIELD_POWER1`, mana, which `UnitMana` reads for power type 0 (`0x5176fc`).
const MANA: u16 = 23;
/// `UNIT_FIELD_MAXPOWER1`.
const MAX_MANA: u16 = 29;
/// `UNIT_FIELD_TARGET`, the hop `"targettarget"` makes; no watch of the bridge covers it.
const TARGET: u16 = 16;

/// `events` one packet each: every update block closed as its own packet.
fn wire(events: impl IntoIterator<Item = SessionEvent>) -> Vec<SessionEvent> {
    events
        .into_iter()
        .flat_map(|ev| {
            let block = matches!(
                ev,
                SessionEvent::ObjectCreate { .. }
                    | SessionEvent::ObjectValues { .. }
                    | SessionEvent::ObjectsRemoved(_)
            );
            std::iter::once(ev).chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

/// Each of `packets` one packet: its events, then the update end when it holds a block.
pub(super) fn wire_packets(packets: Vec<Vec<SessionEvent>>) -> Vec<SessionEvent> {
    packets
        .into_iter()
        .flat_map(|events| {
            let block = events.iter().any(|ev| {
                matches!(
                    ev,
                    SessionEvent::ObjectCreate { .. }
                        | SessionEvent::ObjectValues { .. }
                        | SessionEvent::ObjectsRemoved(_)
                )
            });
            events
                .into_iter()
                .chain(block.then_some(SessionEvent::ObjectUpdateEnd))
        })
        .collect()
}

/// A create block of `kind` whose fields are `pairs`, as the wire's whole descriptor.
pub(super) fn created(guid: u64, kind: EntityKind, pairs: &[(u16, u32)]) -> SessionEvent {
    let as_type = match kind {
        EntityKind::Player => ObjectType::Player,
        _ => ObjectType::Unit,
    };
    SessionEvent::ObjectCreate {
        guid,
        kind,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: ObjectFields::from_pairs(pairs).into_created(as_type),
    }
}

pub(super) fn values(guid: u64, pairs: &[(u16, u32)]) -> SessionEvent {
    SessionEvent::ObjectValues {
        guid,
        fields: ObjectFields::from_pairs(pairs),
    }
}

/// `UNIT_FIELD_TARGET` naming `guid`.
pub(super) fn targets(guid: u64) -> [(u16, u32); 2] {
    [(TARGET, guid as u32), (TARGET + 1, (guid >> 32) as u32)]
}

/// A fall that hurts us: its `UNIT_COMBAT` `WOUND` on `"player"` (`0x624f83`), queued in the chat
/// drain at its packet, an event of another lane between two update packets.
pub(super) fn fall(damage: u32) -> SessionEvent {
    SessionEvent::EnvironmentalDamageLog(EnvironmentalDamageLog {
        victim: ME,
        damage_type: 2,
        damage,
        absorb: 0,
        resist: 0,
    })
}

/// The health in `UNIT_FIELD_HEALTH`.
pub(super) fn health(n: u32) -> [(u16, u32); 1] {
    [(FIELD_UNIT_HEALTH, n)]
}

/// The client in the world as [`ME`], 100 health and 100 mana, targeting [`WOLF`] (100 health),
/// with [`BOAR`] (50 health) beside it; a VM whose spy records `UNIT_HEALTH`, `UNIT_MANA` and
/// `UNIT_COMBAT`, each with its token and what `Look(event, token)` reads as it fires.
pub(super) fn client() -> App {
    let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
    let (tx, _rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let script = UiScript::new().unwrap();
    script
        .run(
            r#"
            Seen = {}
            function Look(event, unit)
                if event == "UNIT_MANA" then return UnitMana(unit) end
                return UnitHealth(unit)
            end
            local f = CreateFrame("Frame")
            for _, e in ipairs({"UNIT_HEALTH", "UNIT_MANA", "UNIT_COMBAT"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. arg1 .. ":" .. tostring(Look(event, arg1)))
            end)
            "#,
        )
        .unwrap();
    world.insert_non_send_resource(script);
    let mob = |hp: u32| [(FIELD_UNIT_HEALTH, hp), (FIELD_UNIT_MAXHEALTH, 100)];
    frame(
        &mut app,
        vec![
            created(
                ME,
                EntityKind::Player,
                &[
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (MANA, 100),
                    (MAX_MANA, 100),
                ],
            ),
            created(WOLF, EntityKind::Unit, &mob(100)),
            created(BOAR, EntityKind::Unit, &mob(50)),
        ],
    );
    let world = app.world_mut();
    // Our create tagged as ours, as the net stage's pass does (`net::apply`).
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::net::SelfPlayer);
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    seat(&mut app, 0);
    frame(&mut app, vec![]);
    app
}

/// The resolver's seat: us, the wolf as `"target"`, and the wolf's target `tot`.
pub(super) fn seat(app: &mut App, tot: u64) {
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit_guids(&UnitGuids {
            player: ME,
            target: WOLF,
            held: [(ME, 0), (WOLF, tot), (BOAR, 0)].into_iter().collect(),
            ..Default::default()
        });
}

/// One frame: the packets through the handler table, the unit feed's push of the frame's state,
/// then the lanes' drain; what the spy saw.
pub(super) fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    frame_after(app, |_| {}, packets)
}

/// [`frame`], with `before` run on the world between the frame-start snapshot and the packets.
pub(super) fn frame_after(
    app: &mut App,
    before: impl FnOnce(&mut World),
    packets: Vec<SessionEvent>,
) -> Vec<String> {
    let world = app.world_mut();
    #[cfg(debug_assertions)]
    {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(crate::ui_unit::views::mark_frame_start)
            .unwrap();
    }
    before(world);
    crate::net::handlers::dispatch(world, wire(packets));
    crate::ui_unit::run_feed_units_once(world);
    crate::lanes::fire_in_order(world);
    let s = world.non_send_resource_mut::<UiScript>();
    let out = s.eval::<Vec<String>>("return Seen").unwrap();
    s.run("Seen = {}").unwrap();
    let errors = s.errors();
    assert!(errors.is_empty(), "script errors: {errors:?}");
    out
}

/// Two health packets for the target in one frame: each `UNIT_HEALTH` listener reads its own
/// packet's value (`0x5174d0` reads the live object), not the frame's last.
#[test]
fn each_unit_health_listener_reads_its_own_packets_target_health() {
    let mut app = client();
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), values(WOLF, &health(40))]
        ),
        ["UNIT_HEALTH:target:70", "UNIT_HEALTH:target:40"]
    );
}

/// A combat-log `UNIT_COMBAT` raised ahead of the update packet that writes the hit reads the
/// health before it: the combat-log handler runs whole first (`0x624f83`), and the health write
/// comes in the later packet.
#[test]
fn a_unit_combat_ahead_of_its_update_reads_the_health_before_the_hit() {
    let mut app = client();
    assert_eq!(
        frame(&mut app, vec![fall(30), values(ME, &health(70))]),
        ["UNIT_COMBAT:player:100", "UNIT_HEALTH:player:70"]
    );
}

/// Two power packets for the player in one frame: each `UNIT_MANA` listener reads its own
/// packet's mana (`0x517670`, `[+0x44 + type*4]` at `0x5176fc`).
#[test]
fn each_unit_mana_listener_reads_its_own_packets_mana() {
    let mut app = client();
    assert_eq!(
        frame(
            &mut app,
            vec![values(ME, &[(MANA, 70)]), values(ME, &[(MANA, 40)])]
        ),
        ["UNIT_MANA:player:70", "UNIT_MANA:player:40"]
    );
}

/// The lane's own count of the builds it made, the views it pushed and the stores it checked at
/// its end, read and reset.
fn lane_counts(app: &mut App) -> (usize, usize, usize) {
    let mut views = app
        .world_mut()
        .resource_mut::<crate::ui_unit::views::UnitViews>();
    let counts = (views.builds, views.pushes, views.checked);
    views.builds = 0;
    views.pushes = 0;
    views.checked = 0;
    counts
}

/// The replay ends on the live stores: a frame of moved dwords, a re-create that clears a field,
/// the target's target released out of range and brought back by a create, each with an event
/// between, ends with every rewound store equal to the live one (the `debug_assert` in the
/// lane's end) and none left rewound, so the stores read live again once the drain is done.
#[test]
fn the_replay_ends_on_the_live_stores() {
    let mut app = client();
    frame(&mut app, vec![values(WOLF, &targets(BOAR))]);
    seat(&mut app, BOAR);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("targettarget") end"#)
        .unwrap();
    lane_counts(&mut app);
    let seen = frame(
        &mut app,
        vec![
            fall(1),
            values(BOAR, &health(40)),
            fall(1),
            // The re-create sends no `MAXHEALTH`: the descriptor's create clears it.
            created(BOAR, EntityKind::Unit, &health(30)),
            fall(1),
            SessionEvent::ObjectsRemoved(vec![BOAR]),
            fall(1),
            created(BOAR, EntityKind::Unit, &health(20)),
            fall(1),
            values(ME, &health(90)),
        ],
    );
    assert_eq!(
        seen.iter()
            .filter(|s| s.starts_with("UNIT_COMBAT"))
            .collect::<Vec<_>>(),
        [
            "UNIT_COMBAT:player:50",
            "UNIT_COMBAT:player:40",
            "UNIT_COMBAT:player:30",
            "UNIT_COMBAT:player:0",
            "UNIT_COMBAT:player:20",
        ],
        "the target's target as each packet left it: gone between its release and its create"
    );
    assert_eq!(
        lane_counts(&mut app).2,
        2,
        "the boar and we were rewound and checked"
    );
    let views = app.world().resource::<crate::ui_unit::views::UnitViews>();
    assert!(
        !views.holds(BOAR) && !views.holds(ME),
        "and read live again"
    );
}

/// Two packets with no event between them are one gap: the drain applies both and builds the
/// views once. A fall ahead of the first reads the frame's start, one between the second and the
/// third reads after the second, and the last packet's views are the feed's own, built by none.
#[test]
fn two_packets_with_no_event_between_make_one_build() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("targettarget") end"#)
        .unwrap();
    lane_counts(&mut app);
    let seen = frame(
        &mut app,
        vec![
            fall(1),
            values(WOLF, &targets(BOAR)),
            values(WOLF, &targets(ME)),
            fall(1),
            values(WOLF, &targets(BOAR)),
            fall(1),
        ],
    );
    assert_eq!(
        seen,
        [
            "UNIT_COMBAT:player:0",
            "UNIT_COMBAT:player:100",
            "UNIT_COMBAT:player:50"
        ]
    );
    assert_eq!(
        lane_counts(&mut app).0,
        2,
        "the frame's start and the one gap"
    );
}

/// A frame whose events all follow its last write needs no older view than the feed's push:
/// the lane builds and pushes nothing.
#[test]
fn a_frame_whose_events_follow_every_write_pushes_no_old_view() {
    let mut app = client();
    lane_counts(&mut app);
    assert_eq!(
        frame(&mut app, vec![values(WOLF, &health(70)), fall(1)]),
        ["UNIT_HEALTH:target:70", "UNIT_COMBAT:player:100"]
    );
    let (builds, pushes, _) = lane_counts(&mut app);
    assert_eq!((builds, pushes), (0, 0));
}

/// A store written past the log fails the rewind's check against the frame-start snapshot: here
/// the target's `MAXHEALTH`, a field no entry of the frame names.
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "a write the log missed")]
fn a_write_the_log_missed_fails_the_rewind() {
    let mut app = client();
    frame_after(
        &mut app,
        |world| {
            let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
            let mut store = world.get_mut::<crate::net::ObjectStore>(wolf).unwrap();
            store
                .0
                .merge(ObjectFields::from_pairs(&[(FIELD_UNIT_MAXHEALTH, 90)]));
        },
        vec![values(WOLF, &health(70))],
    );
}

/// A worldport drops every streamed object without an update block (`SMSG_NEW_WORLD`): logged as
/// the stores it takes, the rewind of the target the frame creates again lands on the store the
/// frame started from.
#[test]
fn a_worldport_is_logged_as_the_stores_it_takes() {
    let mut app = client();
    lane_counts(&mut app);
    app.world_mut()
        .resource_mut::<crate::ui_unit::views::UnitViews>()
        .starts_checked = 0;
    frame(
        &mut app,
        vec![
            SessionEvent::Worldport {
                map_id: 1,
                position: [0.0; 3],
                orientation: 0.0,
                needs_ack: false,
            },
            created(WOLF, EntityKind::Unit, &health(60)),
        ],
    );
    let views = app.world().resource::<crate::ui_unit::views::UnitViews>();
    #[cfg(debug_assertions)]
    assert_eq!(
        views.starts_checked, 1,
        "the target, checked against the snapshot"
    );
    assert!(!views.holds(WOLF), "and read live again");
}

/// A lane that fires nothing, which stops the drain at its item.
#[derive(Resource, Default)]
struct Stuck(Option<crate::lanes::Stamp>);

fn fire_stuck(In(_): In<crate::lanes::Until>) {}

/// A drain that stops before the views lane has replayed its entries leaves no rewound store
/// behind: the next frame's feed reads the live store, and the next drain puts the feed's views
/// back, so the target reads the health its last packet wrote.
#[test]
fn a_drain_that_stops_early_leaves_no_rewound_store() {
    use crate::lanes::LaneApp;
    let mut app = client();
    app.init_resource::<Stuck>().add_lane(
        "stuck",
        |world| world.get_resource::<Stuck>()?.0,
        fire_stuck,
        None,
    );
    let stopped = frame_after(
        &mut app,
        |world| world.resource_mut::<Stuck>().0 = Some(crate::lanes::stamp()),
        vec![values(WOLF, &health(70))],
    );
    assert!(stopped.is_empty(), "the drain stopped at the stuck item");
    let views = app.world().resource::<crate::ui_unit::views::UnitViews>();
    assert!(!views.holds(WOLF), "the stop left nothing rewound");
    app.world_mut().resource_mut::<Stuck>().0 = None;
    assert_eq!(frame(&mut app, vec![]), ["UNIT_HEALTH:target:70"]);
    let health = app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<f64>("return UnitHealth('target')")
        .unwrap();
    assert_eq!(health, 70.0);
}

/// Updates to units no view reads, between events, build nothing: the lane rewinds only a unit a
/// token can name, and a gap whose entries name none is no build.
#[test]
fn updates_to_units_no_view_reads_build_nothing() {
    const FAR: [u64; 3] = [
        0xF130_0000_0050_0001,
        0xF130_0000_0050_0002,
        0xF130_0000_0050_0003,
    ];
    let mut app = client();
    let mob = |hp: u32| [(FIELD_UNIT_HEALTH, hp), (FIELD_UNIT_MAXHEALTH, 100)];
    frame(
        &mut app,
        FAR.iter()
            .map(|&g| created(g, EntityKind::Unit, &mob(100)))
            .collect(),
    );
    lane_counts(&mut app);
    let mut packets = vec![fall(1)];
    for (i, &g) in FAR.iter().enumerate() {
        packets.push(values(g, &health(90 - i as u32)));
        packets.push(fall(1));
    }
    frame(&mut app, packets);
    let (builds, pushes, _) = lane_counts(&mut app);
    assert_eq!((builds, pushes), (0, 0));
}

/// Our own death signals `PLAYER_DEAD` inside its packet (`0x6046f0` at `0x6047a3`, then
/// `0x6058ae` → `0x6251cc` → `0x495850`), after the packets before it ran whole: it fires after
/// the combat line an earlier packet of the frame queued, and `UnitHealth("player")` reads the
/// health its packet left, 0.
#[test]
fn player_dead_reads_its_packets_health_after_an_earlier_packets_line() {
    let mut app = client();
    app.world_mut()
        .resource_mut::<crate::net::NetStatus>()
        .connected = true;
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"local d = CreateFrame("Frame") d:RegisterEvent("PLAYER_DEAD")
            d:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. UnitHealth("player"))
            end)"#,
        )
        .unwrap();
    let seen = frame(&mut app, vec![fall(30), values(ME, &health(0))]);
    let at = |prefix: &str| seen.iter().position(|s| s.starts_with(prefix));
    let (line, dead) = (at("UNIT_COMBAT:player"), at("PLAYER_DEAD"));
    assert!(line.is_some() && dead.is_some(), "{seen:?}");
    assert!(line < dead, "the earlier packet's line first: {seen:?}");
    assert_eq!(seen[dead.unwrap()], "PLAYER_DEAD:0", "{seen:?}");
}

/// Our own death's list signals ahead of its death line and `PLAYER_DEAD` after it: `0x5e99b0`'s
/// deselect (`0x5e9a45`) at `0x60588d`, then `PresentUnitDeath` (`0x6058ae`), which writes the
/// line (`0x62c160`) and ends in `SignalDeathState` (`0x6251cc` → `0x495850`).
#[test]
fn our_deaths_list_signals_before_its_line_and_player_dead_after() {
    use bevy::ecs::system::RunSystemOnce;

    let mut app = client();
    app.world_mut()
        .resource_mut::<crate::net::NetStatus>()
        .connected = true;
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"UNITDIESSELF = "You die." UNITDIESOTHER = "%s dies."
            local d = CreateFrame("Frame")
            for _, e in ipairs({"PLAYER_TARGET_CHANGED", "CHAT_MSG_COMBAT_FRIENDLY_DEATH",
                    "CHAT_MSG_COMBAT_HOSTILE_DEATH", "PLAYER_DEAD"}) do
                d:RegisterEvent(e)
            end
            d:SetScript("OnEvent", function()
                local death = string.find(event, "^CHAT_MSG_COMBAT_.*_DEATH$")
                table.insert(Seen, death and "death line" or event)
            end)"#,
        )
        .unwrap();
    let world = app.world_mut();
    #[cfg(debug_assertions)]
    world
        .run_system_once(crate::ui_unit::views::mark_frame_start)
        .unwrap();
    crate::net::handlers::dispatch(world, wire([values(ME, &health(0))]));
    // The death line's watcher, which the frame runs ahead of the drain.
    world
        .run_system_once(crate::ui_chat::combat::watch::death_lines)
        .unwrap();
    crate::ui_unit::run_feed_units_once(world);
    crate::lanes::fire_in_order(world);
    let seen = world
        .non_send_resource::<UiScript>()
        .eval::<Vec<String>>("return Seen")
        .unwrap();
    let ours: Vec<&str> = seen
        .iter()
        .map(String::as_str)
        .filter(|s| !s.starts_with("UNIT_"))
        .collect();
    assert_eq!(ours, ["PLAYER_TARGET_CHANGED", "death line", "PLAYER_DEAD"]);
}

/// A spy on the level-up's events, each with its arguments, into the client's `Seen`.
fn spy_level_up(app: &mut App) {
    app.world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 32]);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"
            local f = CreateFrame("Frame")
            f:RegisterEvent("PLAYER_LEVEL_UP")
            f:RegisterEvent("TUTORIAL_TRIGGER")
            f:SetScript("OnEvent", function()
                local args = {}
                for _, v in ipairs({arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9}) do
                    table.insert(args, tostring(v))
                end
                table.insert(Seen, event .. ":" .. table.concat(args, ","))
            end)
            "#,
        )
        .unwrap();
}

/// `SMSG_LEVELUP_INFO`'s arm of the notice handler `0x5e38c0`, while the active player exists
/// (`0x5e4071`): the level tutorials the message's level reaches (`0x5e4086`-`0x5e410e`), then
/// `PLAYER_LEVEL_UP` with the packet's level, health, first power, `level >= 10` and five stats
/// (`0x5e413e`), each at its packet's place among the other lanes.
#[test]
fn the_level_up_message_signals_its_tutorials_then_player_level_up_at_its_packet() {
    let mut app = client();
    spy_level_up(&mut app);
    let ding = SessionEvent::LevelUp(benilla_protocol::messages::LevelUpInfo {
        level: 10,
        health: 20,
        powers: [15, 0, 0, 0, 0],
        stats: [1, 2, 3, 4, -5],
    });
    let seen = frame(&mut app, vec![fall(1), ding, fall(1)]);
    // Each published id is the tutorial's own plus one (`0x4b5390`).
    let tutorial = |id: u32| format!("TUTORIAL_TRIGGER:{}", id + 1);
    let mut want = vec!["UNIT_COMBAT:player:100".to_string()];
    want.extend([0x16, 0x0d, 0x02, 0x26, 0x15, 0x25, 0x20, 0x0c].map(tutorial));
    want.push("PLAYER_LEVEL_UP:10,20,15,1,1,2,3,4,-5".into());
    want.push("UNIT_COMBAT:player:100".into());
    assert_eq!(seen, want);

    // Below 10 no talent point, and only the tutorials the level reaches, each once.
    let ding = SessionEvent::LevelUp(benilla_protocol::messages::LevelUpInfo {
        level: 4,
        health: 9,
        powers: [0; 5],
        stats: [0; 5],
    });
    assert_eq!(
        frame(&mut app, vec![ding]),
        ["PLAYER_LEVEL_UP:4,9,0,0,0,0,0,0,0"],
        "the tutorials fire once"
    );
}

/// A level written to our descriptor signals no `PLAYER_LEVEL_UP`: the event's one source is the
/// message (`0x5e413e`), so a GM's level change or a demotion with no `SMSG_LEVELUP_INFO` is
/// silent.
#[test]
fn a_descriptor_level_change_alone_signals_no_level_up() {
    const LEVEL: u16 = 34;
    let mut app = client();
    spy_level_up(&mut app);
    frame(&mut app, vec![values(ME, &[(LEVEL, 10)])]);
    let seen = frame(&mut app, vec![values(ME, &[(LEVEL, 11)])]);
    assert!(
        !seen.iter().any(|e| e.starts_with("PLAYER_LEVEL_UP")),
        "{seen:?}"
    );
}
