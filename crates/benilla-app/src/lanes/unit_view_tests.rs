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
                    | SessionEvent::ItemCreate { .. }
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
                        | SessionEvent::ItemCreate { .. }
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

/// A `MSG_MOVE_HEARTBEAT` from `guid` at the world's `at`, moving by `flags`, aboard `deck` at
/// its deck-local point when one is named.
pub(super) fn heartbeat(
    guid: u64,
    at: [f32; 3],
    flags: u32,
    deck: Option<(u64, [f32; 3])>,
) -> SessionEvent {
    SessionEvent::UnitMove {
        guid,
        position: at,
        orientation: 0.0,
        flags: flags | deck.map_or(0, |_| crate::creature_anim::move_flags::ON_TRANSPORT),
        pitch: 0.0,
        time: 0,
        verb: benilla_protocol::RelayVerb::Heartbeat,
        fall_time: 0,
        jump: None,
        transport: deck.map(|(guid, [x, y, z])| benilla_protocol::TransportPose {
            guid,
            pos: benilla_protocol::wire::Vector3d { x, y, z },
            orientation: 0.0,
        }),
    }
}

/// `create` placed at the world's `at` and facing `facing`, aboard `deck` at its deck-local point
/// when one is named.
pub(super) fn placed(
    mut create: SessionEvent,
    at: [f32; 3],
    facing: f32,
    deck: Option<(u64, [f32; 3])>,
) -> SessionEvent {
    if let SessionEvent::ObjectCreate {
        position,
        orientation,
        transport,
        ..
    } = &mut create
    {
        *position = at;
        *orientation = facing;
        *transport = deck.map(|(guid, [x, y, z])| benilla_protocol::TransportPose {
            guid,
            pos: benilla_protocol::wire::Vector3d { x, y, z },
            orientation: 0.0,
        });
    }
    create
}

/// A create block of `kind` whose fields are `pairs`, as the wire's whole descriptor, with the
/// `OBJECT_FIELD_TYPE` every create carries (object, unit and, for a player, player bits) unless
/// `pairs` sets its own: the resolver's hop and the getters' lookups test it (typemask 8,
/// `0x515a0f`).
pub(super) fn created(guid: u64, kind: EntityKind, pairs: &[(u16, u32)]) -> SessionEvent {
    let as_type = match kind {
        EntityKind::Player => ObjectType::Player,
        _ => ObjectType::Unit,
    };
    const OBJECT_TYPE: u16 = 2;
    let mut pairs = pairs.to_vec();
    if !pairs.iter().any(|&(i, _)| i == OBJECT_TYPE) {
        let mask = match as_type {
            ObjectType::Player => 0x19,
            _ => 0x9,
        };
        pairs.insert(0, (OBJECT_TYPE, mask));
    }
    let pairs = &pairs[..];
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
    // In the world with the interface up, so the dispatch fires each packet's lanes.
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
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
    // The world entry first, as a session has it: the target comes after.
    frame(&mut app, vec![]);
    let world = app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    // The store's token flag on the wolf, as `SetSelection` reports it (`0x49372a`).
    world.non_send_resource_mut::<UiScript>().unit_token_flags(
        WOLF,
        benilla_ui::script::TokenChange::Add(benilla_ui::script::TOKEN_TARGET),
    );
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

/// One frame: the packets through the handler table, each packet's lanes fired after its
/// handlers, then the frame's drain; what the spy saw.
pub(super) fn frame(app: &mut App, packets: Vec<SessionEvent>) -> Vec<String> {
    run_frame(app, wire(packets))
}

/// [`frame`], with `inside` run inside the dispatch past the span's start and ahead of the
/// packets, as a handler of an earlier packet would run it ([`inside_the_dispatch`]).
pub(super) fn frame_after(
    app: &mut App,
    inside: impl FnOnce(&mut World) + Send + Sync + 'static,
    packets: Vec<SessionEvent>,
) -> Vec<String> {
    let events = inside_the_dispatch(app, inside, wire(packets));
    run_frame(app, events)
}

/// A test's step on the world.
type Step = Box<dyn FnOnce(&mut World) + Send + Sync>;

/// What a test runs inside the dispatch, as a packet's handler.
#[derive(Resource, Default)]
struct Inside(Option<Step>);

/// The test's step, run as the handler of the packet [`inside_the_dispatch`] puts first.
fn run_inside(In(_): In<SessionEvent>, world: &mut World) {
    if let Some(step) = world
        .get_resource_mut::<Inside>()
        .and_then(|mut i| i.0.take())
    {
        step(world);
    }
}

/// `events` behind a `SMSG_PONG`, a packet no handler of the client takes, whose handler here runs
/// `step`: a write a handler makes past the span's start, which the dispatch takes at its first
/// packet ([`crate::lanes::span_starts`]).
pub(super) fn inside_the_dispatch(
    app: &mut App,
    step: impl FnOnce(&mut World) + Send + Sync + 'static,
    events: Vec<SessionEvent>,
) -> Vec<SessionEvent> {
    use crate::net::handlers::NetHandlerApp;
    if !app.world().contains_resource::<Inside>() {
        app.init_resource::<Inside>()
            .net_handler(benilla_protocol::SessionEventKind::Pong, run_inside);
    }
    app.world_mut().resource_mut::<Inside>().0 = Some(Box::new(step));
    std::iter::once(SessionEvent::Pong { sequence: 0 })
        .chain(events)
        .collect()
}

/// `events` through the dispatch, then the frame's drain; what the spy saw.
fn run_frame(app: &mut App, events: Vec<SessionEvent>) -> Vec<String> {
    let world = app.world_mut();
    crate::net::handlers::dispatch(world, events);
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

/// The unit tooltip watches the health of the unit it shows on the object itself (`0x52a0a8`
/// registers `0x529560`), so a values block re-seats the plate's bar inside its packet's notify
/// pass, with no token pushed: the target's plate follows each packet's health.
#[test]
fn a_unit_tooltips_bar_follows_its_units_health_at_each_packet() {
    let mut app = client();
    let world = app.world_mut();
    crate::ui_script::test_ui::load_tooltip_template(world.non_send_resource::<UiScript>());
    crate::script_calls::with_hosted_vm(world, |s| {
        s.expect("a VM").run(
            r#"
            Tip = CreateFrame("GameTooltip", "Tip", nil, "TestTooltipTemplate")
            local bar = CreateFrame("StatusBar", "TipStatusBar", Tip)
            bar:SetWidth(100) bar:SetHeight(8)
            Bars = {}
            bar:SetScript("OnValueChanged", function() table.insert(Bars, arg1) end)
            Tip:SetOwner(CreateFrame("Frame"), "ANCHOR_NONE")
            Tip:SetUnit("target")
            "#,
        )
    })
    .expect("the plate shows the target");
    frame(
        &mut app,
        vec![values(WOLF, &health(70)), values(WOLF, &health(40))],
    );
    let bars: Vec<f64> = crate::script_calls::eval_in_vm(app.world(), "return Bars");
    // Each seat sets the range, re-firing the held value, then the packet's (`0x529560`).
    assert_eq!(bars, [100.0, 100.0, 70.0, 70.0, 40.0]);
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

/// The lane's own count of the stores it checked at its end, read and reset.
fn lane_checks(app: &mut App) -> usize {
    let mut views = app
        .world_mut()
        .resource_mut::<crate::ui_unit::views::UnitViews>();
    std::mem::take(&mut views.checked)
}

/// The replay ends on the live stores: moved dwords, a re-create that clears a field, the
/// target's target released out of range and brought back by a create, each with an event
/// between, end each flush with every rewound store equal to the live one (the `debug_assert` in
/// the lane's end) and none left rewound, so the stores read live again between packets.
#[test]
fn the_replay_ends_on_the_live_stores() {
    let mut app = client();
    frame(&mut app, vec![values(WOLF, &targets(BOAR))]);
    seat(&mut app, BOAR);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("targettarget") end"#)
        .unwrap();
    lane_checks(&mut app);
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
        lane_checks(&mut app),
        5,
        "each flush checks its own span: the boar's four, then ours"
    );
    let views = app.world().resource::<crate::ui_unit::views::UnitViews>();
    assert!(
        !views.holds(BOAR) && !views.holds(ME),
        "and read live again"
    );
}

/// Two packets with no event between them are one gap: the flush after the second applies both.
/// A fall ahead of the first reads the stores as they were, one between the second and the third
/// reads after the second, and the last reads the live stores.
#[test]
fn two_packets_with_no_event_between_are_one_gap() {
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth("targettarget") end"#)
        .unwrap();
    lane_checks(&mut app);
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
}

/// A store written past the log fails the rewind's check against the span-start snapshot: here
/// the target's `MAXHEALTH`, a field no entry of the span names.
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

/// A worldport drops every streamed object without an update block (`SMSG_NEW_WORLD`), logged as
/// the stores it takes: the transfer ends the selection, so no seat reads the dropped target's
/// store, and the target the frame creates again is read live.
#[test]
fn a_worldport_is_logged_as_the_stores_it_takes() {
    let mut app = client();
    lane_checks(&mut app);
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
    assert_eq!(views.starts_checked, 0, "no seat reads a dropped store");
    assert!(!views.holds(WOLF), "and read live again");
}

/// A lane that fires nothing, which stops the drain at its item.
#[derive(Resource, Default)]
struct Stuck(Option<crate::lanes::Stamp>);

fn fire_stuck(In(_): In<crate::lanes::Until>) {}

/// A drain that stops before the views lane has replayed its entries leaves no rewound store
/// behind, so the target reads, through the host, the health its last packet wrote.
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
    let health = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM").eval::<f64>("return UnitHealth('target')")
    })
    .unwrap();
    assert_eq!(health, 70.0);
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
    // The packet's end writes the line among the death's legs.
    crate::net::handlers::dispatch(world, wire([values(ME, &health(0))]));
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

/// The death line's listener runs where the health watcher wrote it, inside its update packet
/// (`0x465277` → `0x6046f0` → `0x605860` → `0x62c160`), before the next packet's handler runs
/// (`0x537ae5`): its `SendChatMessage` goes out ahead of the name query the next packet's create
/// asks in its handler (`0x5dec12` → `0x609210`).
#[test]
fn a_death_lines_listener_sends_ahead_of_the_next_packets_handler() {
    const STRANGER: u64 = 0x31;
    let mut app = client();
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    app.world_mut()
        .resource_mut::<crate::net::NetStatus>()
        .connected = true;
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            r#"UNITDIESOTHER = "%s dies."
            local d = CreateFrame("Frame")
            d:RegisterEvent("CHAT_MSG_COMBAT_HOSTILE_DEATH")
            d:SetScript("OnEvent", function() SendChatMessage("down", "SAY") end)"#,
        )
        .unwrap();
    let world = app.world_mut();
    crate::net::handlers::dispatch(
        world,
        wire_packets(vec![
            vec![values(BOAR, &health(0))],
            vec![created(
                STRANGER,
                EntityKind::Player,
                &[(FIELD_UNIT_HEALTH, 100)],
            )],
        ]),
    );
    crate::lanes::fire_in_order(world);
    let order: Vec<&str> = sent
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::Chat { .. } => Some("chat"),
            crate::net::ClientCommand::NameQuery { guid: STRANGER } => Some("name query"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["chat", "name query"]);
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

/// A friend list of one, Ann, her name cached: a lever no log rewinds, since `GetNumFriends` reads
/// the list at the call (`0x5ad000`), so the count a listener reads names the packets ahead of it.
pub(super) fn one_friend(app: &mut App) -> SessionEvent {
    const ANN: u64 = 0x61;
    app.world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ANN, "Ann".into(), None);
    SessionEvent::FriendList {
        friends: vec![benilla_protocol::messages::FriendEntry {
            guid: ANN,
            status: benilla_protocol::messages::friend_status::ONLINE,
            area: 0,
            level: 10,
            class: 1,
        }],
    }
}

/// A say's listener runs at its packet, before the next packet's handler (`0x537ae5`): its
/// `SendChatMessage` goes out ahead of the name query the next packet's player create asks in its
/// handler (`0x5dec12` → `0x609210`).
#[test]
fn a_says_listener_sends_ahead_of_the_next_packets_query() {
    const SPEAKER: u64 = 0x32;
    const STRANGER: u64 = 0x31;
    let mut app = client();
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let world = app.world_mut();
    world.resource_mut::<crate::net::NetStatus>().connected = true;
    world
        .resource_mut::<crate::names::NameCache>()
        .insert_player(SPEAKER, "Ann".into(), None);
    world
        .non_send_resource::<UiScript>()
        .run(
            r#"local d = CreateFrame("Frame")
            d:RegisterEvent("CHAT_MSG_SAY")
            d:SetScript("OnEvent", function() SendChatMessage("hello", "SAY") end)"#,
        )
        .unwrap();
    let say = SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_SAY,
        language: 0,
        sender_guid: SPEAKER,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: "hi".into(),
        chat_tag: 0,
    });
    crate::net::handlers::dispatch(
        world,
        wire_packets(vec![
            vec![say],
            vec![created(
                STRANGER,
                EntityKind::Player,
                &[(FIELD_UNIT_HEALTH, 100)],
            )],
        ]),
    );
    crate::lanes::fire_in_order(world);
    let order: Vec<&str> = sent
        .try_iter()
        .filter_map(|c| match c {
            crate::net::ClientCommand::Chat { .. } => Some("chat"),
            crate::net::ClientCommand::NameQuery { guid: STRANGER } => Some("name query"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["chat", "name query"]);
}

/// Each flush rewinds the views to its own span's start: two target health packets with a friend
/// list between, each `UNIT_HEALTH` listener reading its packet's health and the friends the
/// packets ahead of it left ([`one_friend`]), and each raising flush checking the wolf's store.
#[test]
fn each_flush_rewinds_to_its_own_spans_start() {
    let mut app = client();
    let ann = one_friend(&mut app);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(r#"function Look(event, unit) return UnitHealth(unit) .. "/" .. GetNumFriends() end"#)
        .unwrap();
    lane_checks(&mut app);
    assert_eq!(
        frame(
            &mut app,
            vec![values(WOLF, &health(70)), ann, values(WOLF, &health(40))]
        ),
        ["UNIT_HEALTH:target:70/0", "UNIT_HEALTH:target:40/1"]
    );
    assert_eq!(lane_checks(&mut app), 2, "the wolf, once a raising flush");
}

/// A held line ages once a frame, not once a flush: its age pass is a frame begin
/// ([`crate::ui_chat::ChatLog::begin_frame`]). A stranger's say waits on his name through a frame
/// of more raising packets than its tries, each fall's listener counting the friends the packets
/// ahead of it left ([`one_friend`]), and shows at the name's answer in the next frame.
#[test]
fn a_held_line_ages_by_frames_not_by_flushes() {
    const STRANGER: u64 = 0x31;
    let mut app = client();
    let ann = one_friend(&mut app);
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"function Look(event, unit) return GetNumFriends() end
            local s = CreateFrame("Frame")
            s:RegisterEvent("CHAT_MSG_SAY")
            s:SetScript("OnEvent", function() table.insert(Seen, "SAY:" .. arg2) end)"#,
        )
        .unwrap();
    let say = SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_SAY,
        language: 0,
        sender_guid: STRANGER,
        target_guid: 0,
        sender_name: None,
        channel: None,
        text: "hi".into(),
        chat_tag: 0,
    });
    let mut packets = vec![say, fall(1), ann];
    packets.extend((0..crate::ui_chat::NAME_MAX_TRIES).map(|_| fall(1)));
    let seen = frame(&mut app, packets);
    assert_eq!(seen.len(), usize::from(crate::ui_chat::NAME_MAX_TRIES) + 1);
    assert_eq!(seen[0], "UNIT_COMBAT:player:0");
    assert!(
        seen[1..].iter().all(|s| s == "UNIT_COMBAT:player:1"),
        "the line still waits: {seen:?}"
    );
    let name = SessionEvent::PlayerName {
        guid: STRANGER,
        name: "Zed".into(),
        race: 1,
        gender: 0,
        class: 1,
    };
    assert_eq!(frame(&mut app, vec![name]), ["SAY:Zed"]);
}

/// The dispatch's cost a packet, with each packet's flush and with the frame's one drain alone (the
/// world's interface not up until the drain, so no flush runs): the target's health, which raises
/// `UNIT_HEALTH`; a unit no token names, whose notices fire and signal nothing; and its scale,
/// which queues nothing, so no flush runs.
///
/// Run with `cargo nextest run -p benilla-app --lib flush_cost --run-ignored only --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn flush_cost() {
    const PACKETS: u32 = 200;
    let mut app = client();
    app.world_mut()
        .non_send_resource_mut::<UiScript>()
        .run("function Look() return 0 end")
        .unwrap();
    let kinds: [(&str, u64, u16); 3] = [
        ("raising", WOLF, FIELD_UNIT_HEALTH),
        ("no token's notices", BOAR, FIELD_UNIT_HEALTH),
        ("non-raising", BOAR, 4),
    ];
    for (what, guid, field) in kinds {
        for flush in [true, false] {
            let mut times = Vec::new();
            let mut counts = (0, 0);
            for round in 0..30u32 {
                let packets = wire(
                    (0..PACKETS).map(|n| values(guid, &[(field, 1 + (round * PACKETS + n) % 99)])),
                );
                let world = app.world_mut();
                if !flush {
                    world.insert_resource(crate::ui_script::PendingEntryUiLoad);
                }
                let (f0, k0) = crate::lanes::tally();
                let at = std::time::Instant::now();
                crate::net::handlers::dispatch(world, packets);
                world.remove_resource::<crate::ui_script::PendingEntryUiLoad>();
                crate::lanes::fire_in_order(world);
                times.push(at.elapsed().as_secs_f64() * 1e9 / f64::from(PACKETS));
                let (f1, k1) = crate::lanes::tally();
                counts = (f1 - f0, k1 - k0);
                world
                    .non_send_resource_mut::<UiScript>()
                    .run("Seen = {}")
                    .unwrap();
            }
            times.sort_by(f64::total_cmp);
            eprintln!(
                "{what}, {}: {:.0} ns a packet; a frame of {PACKETS}: flushes={} fired={}",
                if flush {
                    "each packet's flush"
                } else {
                    "the drain alone"
                },
                times[times.len() / 2],
                counts.0,
                counts.1
            );
        }
    }
}
