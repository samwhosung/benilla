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
    (Row::Queue, "::ui_trade::TradeSession", "the trade session"),
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
/// [`FILLED_THROUGH_A_READ`].
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
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::NodeId;
    let mut app = headless_client();
    // The census's pure caches, read off its own build of the schedule, then the graph read here
    // before this one builds, which moves the systems into the executable.
    let caches = census(&mut headless_client(), Update, SyncPoints::Declared)
        .classes
        .caches;
    let queues = handler_queues(app.world(), &caches);
    let (taken, exclusive, systems) = app.world_mut().schedule_scope(Update, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let vm = world
            .components()
            .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
        // The call host's own state, which every feed that lends it holds: what a call touches.
        let host = {
            use bevy::ecs::system::System;
            let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
            System::initialize(&mut host, world)
        };
        let host = host.combined_access();
        let lanes: Vec<ComponentId> = [
            TypeId::of::<crate::field_watch::NoticeLane>(),
            TypeId::of::<crate::ui_chat::ChatLog>(),
        ]
        .into_iter()
        .filter_map(|t| world.components().get_resource_id(t))
        .collect();
        let graph = schedule.graph();
        let in_feed = |node: NodeId| {
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
        };
        let mut taken = BTreeSet::new();
        let mut exclusive = BTreeSet::new();
        let mut systems = BTreeSet::new();
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
        (taken, exclusive, systems)
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
