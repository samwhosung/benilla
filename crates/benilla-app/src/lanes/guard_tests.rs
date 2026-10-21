//! The guard on the lanes: what the packet handlers queue for the interface fires from a lane,
//! in the packets' order (`0x5384d0`, `0x537aa0`), and a feed of the `UiFeed` phase that still
//! fires such a queue itself is named here until its producer moves onto a lane.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bevy::ecs::component::ComponentId;
use bevy::prelude::*;

use crate::game_plugins::schedule_tests::SyncPoints;
use crate::game_plugins::schedule_tests::{census, headless_client, type_names_available};

/// The feeds that still take a queue the packet handlers fill and fire its events themselves, in
/// the frame's system order rather than the packets', keyed by a suffix of the system's name, with
/// the queue each takes. The list only shrinks, a producer at a time as it moves onto a lane; a row
/// whose feed no longer takes such a queue fails as stale.
const NOT_YET_ON_A_LANE: &[(&str, &str)] = &[
    ("::ui_cast::feed_cast_reports", "the cast reports"),
    (
        "::ui_action::state::feed_action_state",
        "the field changes and item notices the action state reads",
    ),
    (
        "::ui_action::feed::feed_actions",
        "the mount and pet-tame refusals and the wire's error texts",
    ),
    (
        "::ui_action::recount::recount_item_actions",
        "the item notices",
    ),
    (
        "::ui_items::feed::feed_containers",
        "the equip errors and the bag opens",
    ),
    (
        "::ui_items::locks::apply_item_releases",
        "the item releases",
    ),
    ("::ui_loot::feed_loot", "the loot window's steps"),
    ("::ui_loot_roll::feed_loot_rolls", "the loot rolls"),
    (
        "::ui_quest_log::feed_quest_log",
        "the item notices and the quest packets' state",
    ),
    (
        "::ui_trade::resignal_trade_items",
        "the item notices of the traded items",
    ),
    ("::ui_trade::feed_trade", "the trade session"),
    (
        "::ui_unit::feed_units",
        "the guild state its unit pushes read",
    ),
    ("::ui_guild::feed::feed_guild", "the guild state"),
    ("::tutorial::feed_tutorials", "the tutorial events"),
    (
        "::ui_dialog_verbs::feed_dialog_verbs",
        "the dialog packets and the tutorial events",
    ),
    (
        "::ui_dialog_verbs::feed_meeting_stone",
        "the meeting stone and the tutorial events",
    ),
    (
        "::ui_mirror::feed_mirror_timers",
        "the mirror timers and the tutorial events",
    ),
    ("::minimap::ping::drive_minimap_ping", "the minimap pings"),
    ("::player::net::feed_player_skinned", "the skinned state"),
    ("::ui_bank::feed_bank", "the bank errors"),
    (
        "::ui_battlefield::feed_battlefield",
        "the battlefield status",
    ),
    (
        "::ui_battlefield::reset_on_world_enter",
        "the battlefield status",
    ),
    (
        "::ui_battlefield_positions::feed_battlefield_positions",
        "the battlefield positions",
    ),
    (
        "::ui_battlefield_score::feed_battlefield_score",
        "the battlefield queue",
    ),
    ("::ui_binder::feed_binder", "the binder confirm"),
    (
        "::ui_chat::feed::played_time_bridge",
        "the played-time answer",
    ),
    ("::ui_duel::feed_duel", "the duel state"),
    ("::ui_honor::feed_honor", "the inspected honor"),
    ("::ui_instance::feed_instance", "the instance state"),
    ("::ui_logout::feed_logout", "the logout state"),
    ("::ui_mail::feed_mail", "the mailbox's replies"),
    (
        "::ui_mail::send_query_next_mail_time_on_enter",
        "the mailbox's replies",
    ),
    ("::ui_merchant::feed_merchant", "the merchant errors"),
    ("::ui_petition::feed::feed_petition", "the petition state"),
    ("::ui_quest::feed_quest", "the quest giver's packets"),
    ("::ui_quest::lines::feed_quest_lines", "the quest lines"),
    ("::ui_quest_share::feed_quest_share", "the quest share"),
    ("::ui_spellbook::feed_spellbook", "the spells learned"),
    ("::ui_stable::feed_stable", "the stable's packets"),
    ("::ui_summon::feed_summon", "the summon state"),
    ("::ui_tabard::feed_tabard", "the tabard vendor's packets"),
    (
        "::ui_talent_wipe::feed_talent_wipe",
        "the talent wipe confirm",
    ),
    ("::ui_taxi::feed_taxi", "the flight master's packets"),
    (
        "::ui_trainer::feed_trainer",
        "the trainer's packets and the field changes",
    ),
];

/// Whether `id` names a `Messages<T>` buffer, which a reader takes from through a read.
fn is_messages(world: &World, id: ComponentId) -> bool {
    world
        .components()
        .get_name(id)
        .is_some_and(|n| n.to_string().contains("Messages<"))
}

/// What the handlers write, by id: the queues among them are what a feed takes.
fn handler_writes(world: &World) -> HashSet<ComponentId> {
    world
        .resource::<crate::net::handlers::NetHandlers>()
        .accesses
        .iter()
        .flat_map(|(_, access)| access.combined_access().resource_writes())
        .collect()
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

/// A feed of the `UiFeed` phase that holds the VM and takes from a queue the packet handlers fill
/// (writes a resource a handler writes, past the call host's own state and the lanes, or reads a
/// message buffer one writes) fires packet-raised events outside a lane, in system order: it is
/// named in [`NOT_YET_ON_A_LANE`] until its producer moves onto one. A feed that finds an edge by
/// comparing state it only reads is not seen here.
#[test]
fn every_feed_that_takes_a_handlers_queue_is_on_a_lane_or_listed() {
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::NodeId;
    let mut app = headless_client();
    let queues = handler_writes(app.world());
    // Read off the graph before it builds, which moves the systems into the executable.
    let (feeds, named): (BTreeMap<String, BTreeSet<ComponentId>>, bool) =
        app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let vm = world
                .components()
                .get_resource_id(TypeId::of::<benilla_ui::script::UiScript>());
            // The call host's own state, which every feed that lends it holds: what a call
            // touches, not a queue the feed takes.
            let host = {
                use bevy::ecs::system::System;
                let mut host = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
                System::initialize(&mut host, world)
            };
            let host = host.combined_access();
            // The lanes themselves: a feed that writes one pushes onto it.
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
            let mut out = BTreeMap::new();
            for (key, system, _) in graph.systems.iter() {
                let Some(with_access) = graph.systems.get(key) else {
                    continue;
                };
                let access = with_access.access.combined_access();
                if !vm.is_some_and(|vm| access.has_resource_write(vm))
                    || !in_feed(NodeId::System(key))
                {
                    continue;
                }
                let takes: BTreeSet<ComponentId> = queues
                    .iter()
                    .filter(|&&id| {
                        Some(id) != vm
                            && !lanes.contains(&id)
                            && !host.has_resource_read(id)
                            && !host.has_resource_write(id)
                    })
                    .filter(|&&id| {
                        access.has_resource_write(id)
                            || (access.has_resource_read(id) && is_messages(world, id))
                    })
                    .map(|&id| id)
                    .collect();
                if !takes.is_empty() {
                    out.insert(system.name().to_string(), takes);
                }
            }
            let named = graph
                .systems
                .iter()
                .any(|(_, s, _)| s.name().to_string().contains("benilla_app::"));
            (out, named)
        });
    if !named {
        eprintln!("skipped: this build carries no type names");
        return;
    }
    // A pure cache, where a miss records itself, is no queue: the census's class.
    let caches = census(&mut app, Update, SyncPoints::Declared)
        .classes
        .caches;
    let world = app.world();
    let feeds: BTreeMap<String, Vec<String>> = feeds
        .into_iter()
        .map(|(name, takes)| {
            let takes: Vec<String> = takes
                .into_iter()
                .filter(|id| !caches.contains(id))
                .map(|id| {
                    world
                        .components()
                        .get_name(id)
                        .map_or_else(|| "?".to_string(), |n| n.to_string())
                })
                .collect();
            (name, takes)
        })
        .filter(|(_, takes)| !takes.is_empty())
        .collect();
    let mut unlisted = Vec::new();
    for (name, takes) in &feeds {
        if !NOT_YET_ON_A_LANE.iter().any(|(s, _)| name.ends_with(s)) {
            unlisted.push(format!("{name}: {takes:?}"));
        }
    }
    let stale: Vec<&str> = NOT_YET_ON_A_LANE
        .iter()
        .filter(|(s, _)| !feeds.keys().any(|n| n.ends_with(s)))
        .map(|(s, _)| *s)
        .collect();
    eprintln!(
        "{} feeds take a handlers' queue outside a lane",
        feeds.len()
    );
    assert!(
        unlisted.is_empty(),
        "these feeds fire what the packet handlers queue outside a lane, in system order: put \
         their producer on a lane (`crate::lanes::LaneApp::add_lane`), or name them in \
         NOT_YET_ON_A_LANE with what moves them:\n  {}",
        unlisted.join("\n  ")
    );
    assert!(stale.is_empty(), "stale rows: {stale:?}");
}
