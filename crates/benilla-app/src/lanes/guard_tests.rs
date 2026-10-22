//! The guard on the lanes: what the packet handlers queue for the interface fires from a lane,
//! in the packets' order (`0x5384d0`, `0x537aa0`). What still fires outside one is named here,
//! by the queue the handlers fill, by the feed that finds an edge by comparing state, or by the
//! exclusive system, so the list is the whole of the remaining work and only shrinks.

use std::any::TypeId;
use std::collections::{BTreeSet, HashMap};

use bevy::ecs::component::ComponentId;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::SyncPoints;
use crate::game_plugins::schedule_tests::{census, headless_client, type_names_available};

/// What a row names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// A queue the handlers fill that a feed of the `UiFeed` phase takes and fires itself, by a
    /// suffix of its type's name.
    Queue,
    /// A feed that raises an event by comparing state the handlers write, by a suffix of its
    /// system's name: no queue shows it.
    Edge,
    /// An exclusive system of the phase, whose access bevy does not report, by a suffix of its
    /// system's name.
    Exclusive,
    /// A queue the handlers fill that the call host holds too, by a suffix of its type's name:
    /// every feed that lends the host holds it, so no access tells the feed that takes it apart,
    /// and the row is found by reading the feed, not by this guard. It is stale once the host or
    /// the handlers no longer hold the queue.
    HostHeld,
}

/// What still fires in the frame's system order rather than the packets', with what it is. A
/// queue's row stays while any feed takes that queue; each goes when its producer moves onto a
/// lane, and a row whose queue, feed or system is gone fails as stale.
const NOT_YET_ON_A_LANE: &[(Row, &str, &str)] = &[
    (
        Row::Queue,
        "::ui_action::errors::UiErrorKeys",
        "the refusals the handlers push, which the action feed shows",
    ),
    (
        Row::Queue,
        "::tutorial::Tutorials",
        "the tutorial banks a handler fires",
    ),
    (
        Row::Queue,
        "Messages<benilla_app::tutorial::TutorialEvent>",
        "the tutorial events",
    ),
    (
        Row::Queue,
        "Messages<benilla_app::item_watch::ItemNotice>",
        "the item notices",
    ),
    (
        Row::Queue,
        "Messages<benilla_app::net::FieldChanged>",
        "the field changes",
    ),
    (
        Row::Queue,
        "::ui_action::errors::MountErrors",
        "the mount refusals",
    ),
    (
        Row::Queue,
        "::ui_action::errors::PetTameFailures",
        "the pet-tame refusals",
    ),
    (
        Row::Queue,
        "::ui_action::errors::UiErrorTexts",
        "the wire's error texts",
    ),
    (
        Row::Queue,
        "::minimap::ping::MinimapPing",
        "the minimap pings",
    ),
    (
        Row::Queue,
        "::net::PlayedTimeAnswer",
        "the played-time answer",
    ),
    (
        Row::Queue,
        "::player::net::SkinnedFeed",
        "the skinned state",
    ),
    (Row::Queue, "::ui_bank::BankErrors", "the bank errors"),
    (
        Row::Queue,
        "::ui_battlefield_positions::BattlefieldPositions",
        "the battlefield positions",
    ),
    (
        Row::Queue,
        "::ui_battlefield::Battlefield",
        "the battlefield status",
    ),
    (Row::Queue, "::ui_binder::BinderState", "the binder confirm"),
    (
        Row::Queue,
        "::ui_dialog_verbs::AreaSpiritHealer",
        "the area spirit healer",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::BattlefieldQueue",
        "the battlefield queue",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::InstanceBoot",
        "the instance boot",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::MeetingStone",
        "the meeting stone",
    ),
    (
        Row::Queue,
        "::ui_dialog_verbs::PetUnlearnState",
        "the pet unlearn confirm",
    ),
    (Row::Queue, "::ui_duel::DuelState", "the duel state"),
    (Row::Queue, "::ui_guild::GuildState", "the guild state"),
    (
        Row::Queue,
        "::ui_honor::InspectHonor",
        "the inspected honor",
    ),
    (
        Row::Queue,
        "::ui_instance::InstanceState",
        "the instance state",
    ),
    (Row::Queue, "::ui_items::EquipErrors", "the equip errors"),
    (
        Row::Queue,
        "::ui_items::locks::ItemReleases",
        "the item releases",
    ),
    (Row::Queue, "::ui_items::net::BagOpens", "the bag opens"),
    (Row::Queue, "::ui_logout::LogoutState", "the logout state"),
    (Row::Queue, "::ui_loot_roll::LootRolls", "the loot rolls"),
    (
        Row::Queue,
        "::ui_loot::LootMoveStart",
        "the loot window's steps",
    ),
    (
        Row::Queue,
        "::ui_merchant::MerchantErrors",
        "the merchant errors",
    ),
    (
        Row::Queue,
        "::ui_mirror::MirrorTimerFeed",
        "the mirror timers",
    ),
    (
        Row::Queue,
        "::ui_petition::PetitionState",
        "the petition state",
    ),
    (
        Row::Queue,
        "::ui_quest_log::QuestLog",
        "the quest packets' log state",
    ),
    (
        Row::Queue,
        "::ui_quest_share::QuestShare",
        "the quest share",
    ),
    (
        Row::Queue,
        "::ui_quest::lines::QuestLines",
        "the quest lines",
    ),
    (
        Row::Queue,
        "::ui_quest::QuestGiver",
        "the quest giver's packets",
    ),
    (
        Row::Queue,
        "::ui_spellbook::LearnedInTab",
        "the spells learned",
    ),
    (Row::Queue, "::ui_stable::StableErrors", "the stable errors"),
    (
        Row::Queue,
        "::ui_stable::StableOpen",
        "the stable's packets",
    ),
    (Row::Queue, "::ui_summon::SummonState", "the summon state"),
    (
        Row::Queue,
        "::ui_tabard::TabardOpen",
        "the tabard vendor's packets",
    ),
    (
        Row::Queue,
        "::ui_talent_wipe::TalentWipeState",
        "the talent wipe confirm",
    ),
    (
        Row::Queue,
        "::ui_taxi::TaxiState",
        "the flight master's packets",
    ),
    (
        Row::HostHeld,
        "::ui_trade::TradeSession",
        "the trade session, which `feed_trade` fires from; the trade verbs, acting in their \
         calls, hold it in the host",
    ),
    (
        Row::Queue,
        "::ui_trainer::TrainerErrors",
        "the trainer errors",
    ),
    (
        Row::Queue,
        "::ui_trainer::TrainerOpen",
        "the trainer's packets",
    ),
    (
        Row::Edge,
        "::ui_party::feed::feed_party",
        "the roster's diff",
    ),
    (
        Row::Edge,
        "::ui_cast::feed_cast_reports",
        "the form's change",
    ),
    (
        Row::Edge,
        "::ui_unit::feed_units",
        "the combo points' change",
    ),
    (
        Row::Edge,
        "::ui_unit::feed_units",
        "the selection's change, `PLAYER_TARGET_CHANGED`",
    ),
];

/// The resources the handlers fill through a shared reference, which their declared access shows
/// as a read: the error keys a refusal pushes and the tutorial banks a watcher fires. The lanes
/// and the host's two other sinks (the message sounds and the unit edits, which raise no event)
/// are not queues here.
const FILLED_THROUGH_A_READ: &[&str] =
    &["::ui_action::errors::UiErrorKeys", "::tutorial::Tutorials"];

/// Every other resource a handler reads and does not write: state, tables and caches it
/// consults, none of them a queue a feed takes. A new one fails until it is placed here or in
/// [`FILLED_THROUGH_A_READ`]. The list is kept by hand: an entry a handler in fact fills through a
/// shared reference leaves that queue out of [`handler_queues`], so a feed that takes it and fires
/// in system order, or a system that pushes onto a lane off it at its own place, passes every
/// check here unseen.
const READ_ONLY_TO_HANDLERS: &[&str] = &[
    "::AreaTableRes",
    "::DamageTextGates",
    "::AttackStopInFlight",
    "::EnvDamageTable",
    "::SpellVisuals",
    "::Creatures",
    "::ItemDisplays",
    "::GameObjectTemplates",
    "::Enchants",
    "::Items",
    "::RandomProperties",
    "::ChrClassTable",
    "::ChrRaceTable",
    "::NameCache",
    "::ServerTime",
    "::GuidIndex",
    "::NetCommands",
    "::Reputations",
    "::SelfGuid",
    "::OwnControlLost",
    "::Player",
    "::BarUnitOutdoors",
    "::PlayerOutdoors",
    "::EmoteSounds",
    "::ExplorationSounds",
    "::ScriptCastPermit",
    "::AutoSelfCast",
    "::GroupRoster",
    "::ActiveChannel",
    "::SpellModifiers",
    "::AssistAttack",
    "::Hovered",
    "::HoveredObject",
    "::Factions",
    "::CreatureTypes",
    "::LastEnemy",
    "::Selection",
    "::TextFilter",
    "::TextFilterSwitches",
    "::PlayerActions",
    "::SpellMechanics",
    "::Spells",
    "::CombatLogRanges",
    "::LogPeriodicSpells",
    "::GuildMemberNotify",
    "::ItemSubClasses",
    "::LootConfig",
    "::LootKneel",
    "::MailOpen",
    "::Stationery",
    "::MerchantOpen",
    "::GroupState",
    "::PetBar",
    "::QuestLog",
    "::UiClock",
    "::InteractNpc",
    "::SocialState",
    "::SkillLines",
    "::SpellFocus",
    "::FrameStep",
    "::CurrentMap",
    "bevy_time::time::Time",
    "bevy_time::time::Time<bevy_time::real::Real>",
    // The host's two sinks that raise no event.
    "::sound::message::MessageSounds",
    "::creature_anim::edits::UnitEdits",
];

/// Whether `id` names a `Messages<T>` buffer, which a reader takes from through a read.
fn is_messages(world: &World, id: ComponentId) -> bool {
    name(world, id).contains("Messages<")
}

fn name(world: &World, id: ComponentId) -> String {
    world
        .components()
        .get_name(id)
        .map_or_else(|| "?".to_string(), |n| n.to_string())
}

fn ends_with_any(name: &str, rows: &[&str]) -> bool {
    rows.iter().any(|s| name.ends_with(s))
}

/// The queues the handlers fill, by id: each resource one writes, past the pure caches, and each
/// resource in [`FILLED_THROUGH_A_READ`] one reads.
fn handler_queues(
    world: &World,
    caches: &std::collections::HashSet<ComponentId>,
) -> Vec<ComponentId> {
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let mut out = BTreeSet::new();
    for (_, access) in &handlers.accesses {
        let access = access.combined_access();
        out.extend(access.resource_writes().filter(|id| !caches.contains(id)));
        out.extend(
            access
                .resource_reads()
                .filter(|id| ends_with_any(&name(world, *id), FILLED_THROUGH_A_READ)),
        );
    }
    out.into_iter().collect()
}

/// The call host's own state, which every system that lends it holds: what a call touches.
fn call_host_access(world: &mut World) -> bevy::ecs::query::Access {
    use bevy::ecs::system::System;
    let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
    System::initialize(&mut host, world)
        .combined_access()
        .clone()
}

/// Whether `node` sits in the `UiFeed` phase, through any set.
fn in_feed(graph: &bevy::ecs::schedule::ScheduleGraph, node: bevy::ecs::schedule::NodeId) -> bool {
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::NodeId;
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        for parent in graph
            .hierarchy()
            .graph()
            .neighbors_directed(n, Direction::Incoming)
        {
            if let NodeId::Set(set) = parent {
                if graph
                    .system_sets
                    .get(set)
                    .is_some_and(|s| format!("{s:?}") == "UiFeed")
                {
                    return true;
                }
                stack.push(parent);
            }
        }
    }
    false
}

/// Every resource a handler reads without writing is known: one the handlers fill through a
/// shared reference is a queue the guard below follows, and the rest are named as read-only.
#[test]
fn every_resource_a_handler_reads_is_a_known_queue_or_read_only() {
    let app = headless_client();
    let world = app.world();
    let handlers = world.resource::<crate::net::handlers::NetHandlers>();
    let vm = world
        .components()
        .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
    let lanes = [
        world
            .components()
            .get_resource_id(TypeId::of::<crate::ui_chat::ChatLog>()),
        world
            .components()
            .get_resource_id(TypeId::of::<crate::field_watch::NoticeLane>()),
    ];
    let mut reads = BTreeSet::new();
    for (_, access) in &handlers.accesses {
        let access = access.combined_access();
        for id in access.resource_reads() {
            if !access.has_resource_write(id) && Some(id) != vm && !lanes.contains(&Some(id)) {
                reads.insert(name(world, id));
            }
        }
    }
    if !reads.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let unknown: Vec<&String> = reads
        .iter()
        .filter(|n| {
            !ends_with_any(n, FILLED_THROUGH_A_READ) && !ends_with_any(n, READ_ONLY_TO_HANDLERS)
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "a handler reads these without writing them; name each in FILLED_THROUGH_A_READ if a \
         handler fills it through a shared reference, else in READ_ONLY_TO_HANDLERS:\n  {}",
        unknown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    let stale: Vec<&str> = READ_ONLY_TO_HANDLERS
        .iter()
        .chain(FILLED_THROUGH_A_READ)
        .filter(|s| !reads.iter().any(|n| n.ends_with(*s)))
        .copied()
        .collect();
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The drain runs once a frame after every feed and before the tick, gated as the feeds of the
/// in-game interface are, and holds the chat and notice lanes.
#[test]
fn the_drain_runs_after_the_feeds_and_before_the_tick() {
    let mut app = headless_client();
    let mut lanes = app.world().resource::<super::Lanes>().names();
    lanes.sort_unstable();
    assert_eq!(lanes, ["chat", "field notices"]);
    let c = census(&mut app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let (drain, info) = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::lanes::fire_in_order"))
        .expect("the drain is in Update");
    assert!(
        info.conditions
            .iter()
            .any(|c| c.ends_with("::ingame_ui_up")),
        "{:?}",
        info.conditions
    );
    let mut before: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        before.entry(*b).or_default().push(*a);
    }
    let mut ahead = std::collections::HashSet::new();
    let mut stack = vec![*drain];
    while let Some(n) = stack.pop() {
        for m in before.get(&n).into_iter().flatten() {
            if ahead.insert(*m) {
                stack.push(*m);
            }
        }
    }
    let late: Vec<&str> = c
        .systems
        .iter()
        .filter(|(k, s)| s.sets.iter().any(|set| set == "UiFeed") && !ahead.contains(*k))
        .map(|(_, s)| s.name.as_str())
        .collect();
    assert!(
        late.is_empty(),
        "feeds not ordered ahead of the drain: {late:?}"
    );
    let tick = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::extract::tick_script"))
        .map(|(k, _)| *k)
        .expect("the tick");
    let mut after = std::collections::HashSet::new();
    let mut stack = vec![*drain];
    let mut next: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        next.entry(*a).or_default().push(*b);
    }
    while let Some(n) = stack.pop() {
        for m in next.get(&n).into_iter().flatten() {
            if after.insert(*m) {
                stack.push(*m);
            }
        }
    }
    assert!(after.contains(&tick), "the drain runs before the tick");
}

/// The remaining work, whole. A queue the handlers fill ([`handler_queues`]) that a VM-holding
/// system of the `UiFeed` phase takes (writes it, past the call host's own state and the lanes;
/// reads it, when it is a message buffer or filled through a read) fires its events outside a
/// lane; so may an exclusive system of the phase, whose access shows nothing. Each needs its row,
/// as each edge-comparing feed has one, and a row whose subject is gone is stale.
#[test]
fn every_handler_queue_a_feed_takes_is_on_a_lane_or_listed() {
    use bevy::ecs::schedule::NodeId;
    let mut app = headless_client();
    // The census's pure caches, read off its own build of the schedule, then the graph read here
    // before this one builds, which moves the systems into the executable.
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let (taken, exclusive, systems, host_held) =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
            let host = call_host_access(world);
            let lanes: Vec<ComponentId> = [
                TypeId::of::<crate::field_watch::NoticeLane>(),
                TypeId::of::<crate::ui_chat::ChatLog>(),
            ]
            .into_iter()
            .filter_map(|t| world.components().get_resource_id(t))
            .collect();
            let graph = schedule.graph();
            let in_feed = |node: NodeId| in_feed(graph, node);
            let mut taken = BTreeSet::new();
            let mut exclusive = BTreeSet::new();
            let mut systems = BTreeSet::new();
            let host_held: BTreeSet<String> = queues
                .iter()
                .filter(|&&id| Some(id) != vm && !lanes.contains(&id))
                .filter(|&&id| !is_messages(world, id))
                .filter(|&&id| !ends_with_any(&self::name(world, id), FILLED_THROUGH_A_READ))
                .filter(|&&id| host.has_resource_read(id))
                .map(|&id| self::name(world, id))
                .collect();
            for (key, system, _) in graph.systems.iter() {
                let name = system.name().to_string();
                systems.insert(name.clone());
                if !in_feed(NodeId::System(key)) {
                    continue;
                }
                if system.is_exclusive() {
                    exclusive.insert(name);
                    continue;
                }
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                if !vm.is_some_and(|vm| access.has_resource_write(vm)) {
                    continue;
                }
                for &id in &queues {
                    if Some(id) == vm || lanes.contains(&id) {
                        continue;
                    }
                    let n = self::name(world, id);
                    let through_a_read = ends_with_any(&n, FILLED_THROUGH_A_READ);
                    let takes = if through_a_read {
                        access.has_resource_read(id)
                    } else if is_messages(world, id) {
                        // A reader, not a writer: the host's own messages are pushes.
                        access.has_resource_read(id)
                            && !access.has_resource_write(id)
                            && !host.has_resource_read(id)
                    } else {
                        access.has_resource_write(id)
                            && !host.has_resource_read(id)
                            && !host.has_resource_write(id)
                    };
                    if takes {
                        taken.insert(n);
                    }
                }
            }
            (taken, exclusive, systems, host_held)
        });
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let rows = |kind: Row| -> Vec<&str> {
        NOT_YET_ON_A_LANE
            .iter()
            .filter(|(k, _, _)| *k == kind)
            .map(|(_, s, _)| *s)
            .collect()
    };
    let mut unlisted: Vec<String> = taken
        .iter()
        .filter(|n| !ends_with_any(n, &rows(Row::Queue)))
        .map(|n| format!("queue {n}"))
        .collect();
    unlisted.extend(
        exclusive
            .iter()
            .filter(|n| !ends_with_any(n, &rows(Row::Exclusive)))
            .map(|n| format!("exclusive {n}")),
    );
    let mut stale: Vec<&str> = rows(Row::Queue)
        .into_iter()
        .filter(|s| !taken.iter().any(|n| n.ends_with(s)))
        .collect();
    stale.extend(
        rows(Row::HostHeld)
            .into_iter()
            .filter(|s| !host_held.iter().any(|n| n.ends_with(s))),
    );
    stale.extend(
        rows(Row::Exclusive)
            .into_iter()
            .filter(|s| !exclusive.iter().any(|n| n.ends_with(s))),
    );
    stale.extend(
        rows(Row::Edge)
            .into_iter()
            .filter(|s| !systems.iter().any(|n| n.ends_with(s))),
    );
    assert!(
        unlisted.is_empty(),
        "these fire what the packet handlers queue outside a lane, in system order: put the \
         producer on a lane (`crate::lanes::LaneApp::add_lane`), or give it a row in \
         NOT_YET_ON_A_LANE:\n  {}",
        unlisted.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The handler queues whose items carry their place among what the packets queued, so a system
/// that pushes onto a lane off one pushes at the item's place, not its own: the field edges'
/// `at`. A wrong entry lets a system that pushes at its own place, behind every packet of the
/// frame, pass [`every_lane_push_off_a_handler_queue_keeps_its_place`].
fn placed_queues() -> [TypeId; 1] {
    [TypeId::of::<
        bevy::ecs::message::Messages<crate::net::FieldChanged>,
    >()]
}

/// The systems that take a handler queue and may push onto a lane at their own place, by a
/// suffix of the system's name, with why that place is right.
const AT_THEIR_OWN_PLACE: &[(&str, &str)] = &[
    (
        "::creature_anim::impact::route_swing_impacts",
        "a melee swing's record waits for its animation's impact key, which dispatches it in the \
         frame that crosses the key (`0x6247d0`), not at the packet",
    ),
    (
        "::ui_script::glue_mgr::drive_glue_mgr",
        "the login and character screens' answers, which come before the in-game interface: \
         the drain runs only with it up",
    ),
    (
        "::ui_chat::channels::auto_join_zone_channels",
        "world entry only arms the zone-channel walk, an engine turn that runs no Lua",
    ),
    (
        "::ui_chat::channels::end_session_channels_on_disconnect",
        "a dead socket ends the channel session, and no packet follows it",
    ),
];

/// The systems outside the phase that lend the call host and take a handler queue, firing off it
/// in system order: remaining work, which only shrinks, by a suffix of the system's name.
const NOT_YET_PLACED: &[(&str, &str)] = &[
    (
        "::ui_trade::drain_trade",
        "the trade records' `TRADE_UPDATE`, off the item notices after the tick",
    ),
    (
        "::ui_tradeskill::drain_trade_skill",
        "the repeat craft's hook, which the reference runs inside the cast's go (`0x6e81d3`), \
         off the cast events",
    ),
];

/// A system of any phase that takes a handler queue and pushes onto a lane stamps the push where
/// it runs, behind every packet of the frame, unless the queue's items carry their place
/// ([`placed_queues`]). Each such system needs a row in [`AT_THEIR_OWN_PLACE`] or
/// [`NOT_YET_PLACED`].
#[test]
fn every_lane_push_off_a_handler_queue_keeps_its_place() {
    let mut app = headless_client();
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let (pushes, systems) = app.world_mut().schedule_scope(Update, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let host = call_host_access(world);
        let id = |t: TypeId| world.components().get_resource_id(t);
        let vm = id(TypeId::of::<benilla_ui::script::UiScript>());
        let lanes: Vec<ComponentId> = [
            TypeId::of::<crate::field_watch::NoticeLane>(),
            TypeId::of::<crate::ui_chat::ChatLog>(),
        ]
        .into_iter()
        .filter_map(id)
        .collect();
        let placed: Vec<ComponentId> = placed_queues().into_iter().filter_map(id).collect();
        let graph = schedule.graph();
        let mut pushes = BTreeSet::new();
        let mut systems = BTreeSet::new();
        for (key, system, _) in graph.systems.iter() {
            systems.insert(system.name().to_string());
            let Some(with_access) = graph.systems.get(key) else {
                continue;
            };
            let access = with_access.access.combined_access();
            let fires = vm.is_some_and(|vm| access.has_resource_write(vm));
            // A feed of the phase that fires is the other guard's, and an exclusive one is its
            // row; a system that lends the call host outside the phase is judged here, by the
            // queues it reads: a plain resource it writes is its verbs' own state.
            if system.is_exclusive()
                || (fires && in_feed(graph, bevy::ecs::schedule::NodeId::System(key)))
                || !lanes.iter().any(|&l| access.has_resource_read(l))
            {
                continue;
            }
            let name = system.name().to_string();
            let lends = host.is_subset_resources(access);
            for &q in &queues {
                if Some(q) == vm || lanes.contains(&q) || placed.contains(&q) {
                    continue;
                }
                let n = self::name(world, q);
                // The host's own state is what a call touches, never a queue taken.
                let takes = if ends_with_any(&n, FILLED_THROUGH_A_READ) {
                    access.has_resource_read(q) && !host.has_resource_read(q)
                } else if is_messages(world, q) {
                    access.has_resource_read(q)
                        && !access.has_resource_write(q)
                        && !host.has_resource_read(q)
                } else {
                    !lends
                        && access.has_resource_write(q)
                        && !host.has_resource_read(q)
                        && !host.has_resource_write(q)
                };
                if takes {
                    pushes.insert((name.clone(), format!("{name} takes {n}")));
                }
            }
        }
        (pushes, systems)
    });
    if !systems.iter().any(|n| n.contains("benilla_app::")) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let rows: Vec<&str> = AT_THEIR_OWN_PLACE
        .iter()
        .chain(NOT_YET_PLACED)
        .map(|(s, _)| *s)
        .collect();
    let unlisted: Vec<&String> = pushes
        .iter()
        .filter(|(system, _)| !ends_with_any(system, &rows))
        .map(|(_, line)| line)
        .collect();
    assert!(
        unlisted.is_empty(),
        "these push onto a lane off a handler queue at their own place: stamp each push at its \
         item's place, or give the system a row in AT_THEIR_OWN_PLACE or NOT_YET_PLACED:\n  {}",
        unlisted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    let stale: Vec<&str> = rows
        .into_iter()
        .filter(|s| !pushes.iter().any(|(system, _)| system.ends_with(s)))
        .collect();
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}

/// The drain is exclusive, so bevy lists its pairs with every system no declared order places
/// against it with no component. What those systems meet of the drain's (its lanes' firings'
/// access, and the unit edits it applies in place after each: every component a change writes,
/// the cast events and their sequence, the precast tail's loot release and what it reads) is a
/// pure cache at most, which commutes: the ambiguity ceiling's reason for these pairs. Commands a
/// firing defers, which the drain applies after it, are outside the model. An exclusive system
/// shows bevy no access, so each one needs a declared order against the drain.
#[test]
fn the_systems_unordered_against_the_drain_meet_it_only_on_a_pure_cache() {
    use bevy::ecs::query::{Access, AccessConflicts};
    let mut app = headless_client();
    let mut drain = Access::new();
    for a in &app.world().resource::<super::Lanes>().1 {
        drain.extend(a.combined_access());
    }
    {
        use crate::creature_anim::UnitEdit;
        let world = app.world_mut();
        let writes: BTreeSet<TypeId> = UnitEdit::every(Entity::PLACEHOLDER)
            .iter()
            .flat_map(UnitEdit::writes)
            .collect();
        for t in writes {
            if let Some(id) = world.components().get_id(t) {
                drain.add_component_write(id);
            }
        }
        let player = world.register_component::<crate::net::SelfPlayer>();
        drain.add_component_read(player);
        for t in [
            TypeId::of::<crate::creature_anim::PlaySeq>(),
            TypeId::of::<bevy::ecs::message::Messages<crate::creature_anim::CastEvent>>(),
            TypeId::of::<crate::ui_loot::LootMoveStart>(),
            TypeId::of::<crate::ui_loot::LootCloseAtRest>(),
        ] {
            if let Some(id) = world.components().get_resource_id(t) {
                drain.add_resource_write(id);
            }
        }
        if let Some(id) = world
            .components()
            .get_resource_id(TypeId::of::<crate::player::Player>())
        {
            drain.add_resource_read(id);
        }
    }
    // Each system's access, read before the census builds the schedule.
    let accesses: HashMap<_, (Access, bool)> =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            graph
                .systems
                .iter()
                .filter_map(|(k, system, _)| {
                    let access = graph.systems.get(k)?.access.combined_access().clone();
                    Some((k, (access, system.is_exclusive())))
                })
                .collect()
        });
    let c = census(&mut app, Update, SyncPoints::Declared);
    if !type_names_available(&c) {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    let (drain_key, _) = c
        .systems
        .iter()
        .find(|(_, s)| s.name.ends_with("::lanes::fire_in_order"))
        .expect("the drain is in Update");
    let mut next: HashMap<_, Vec<_>> = HashMap::new();
    let mut prev: HashMap<_, Vec<_>> = HashMap::new();
    for (a, b) in &c.dependencies {
        next.entry(*a).or_default().push(*b);
        prev.entry(*b).or_default().push(*a);
    }
    let reach = |edges: &HashMap<_, Vec<_>>| {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![*drain_key];
        while let Some(n) = stack.pop() {
            for m in edges.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    };
    let (after, before) = (reach(&next), reach(&prev));
    let world = app.world();
    let mut hazards = Vec::new();
    for (k, (a, exclusive)) in &accesses {
        if k == drain_key || after.contains(k) || before.contains(k) {
            continue;
        }
        if *exclusive {
            hazards.push(format!("{}: exclusive, its access unknown", c.name(*k)));
            continue;
        }
        if a.is_compatible(&drain) {
            continue;
        }
        let met: Vec<String> = match a.get_conflicts(&drain) {
            AccessConflicts::All => vec!["everything".to_string()],
            AccessConflicts::Individual(ids) => ids
                .ones()
                .map(ComponentId::new)
                .filter(|id| !c.classes.caches.contains(id))
                .map(|id| name(world, id))
                .collect(),
        };
        if !met.is_empty() {
            hazards.push(format!("{}: {met:?}", c.name(*k)));
        }
    }
    assert!(
        hazards.is_empty(),
        "these run in no declared order against the drain and meet what it touches; order them:\n  {}",
        hazards.join("\n  ")
    );
}
