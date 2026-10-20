//! The VM pass: every system that lends the call host ([`crate::script_calls`]), in one declared
//! order, with every other system of the UI's phases that touches what a script call reaches. The
//! reference runs its script work on one thread in a fixed order: the net drain's handlers, the
//! input records, the frame's `OnUpdate` walk, each verb applied inside its call. benilla's hosts
//! are serialized already, the VM being non-`Send` and the host mutable, but the executor picks
//! among the ready ones. [`order_the_pass`] chains them between the pass's two ends ([`VmPass`]),
//! so they run in the same order every frame.
//!
//! The order is the schedule's own: the phases' sets and every declared edge, then, where nothing
//! is declared between two of them (the reference's order among them is not known), the order the
//! app registers them in, which only a changed registration moves. The input records are the one
//! known order benilla does not keep yet: it runs the frame's UI records, then its bindings, then
//! its world clicks after the input stage, where the reference dispatches each record whole in
//! the order it arrived (`0x423920` → `0x4239a0`, `0x4245b0`).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use bevy::ecs::schedule::graph::Direction;
use bevy::ecs::schedule::{InternedSystemSet, NodeId, ScheduleLabel, SystemKey, SystemSetKey};
use bevy::ecs::system::{IntoSystem, System};
use bevy::prelude::*;

/// The two ends of the pass: the chain of every system whose declared access covers the call
/// host's ([`crate::script_calls::ScriptCallHost`]), every one that writes the VM, and of every
/// other system of the UI's phases whose access meets the host's. A world system that moves what a
/// script call reads runs before [`VmPass::Start`]; one that reads what a call writes runs after
/// [`VmPass::End`]. Each is an empty set [`order_the_pass`] links to the chain's first and last.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VmPass {
    Start,
    End,
}

/// The systems of `Update`'s pass in the order it runs them, by system name, for the tests.
#[cfg(test)]
#[derive(Resource, Debug, Default)]
pub(crate) struct PassOrder(pub(crate) Vec<String>);

/// One link of the chain: an empty set after one of its systems and before the next.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PassLink(usize);

/// Orders the pass once every plugin has added its systems, the probes' included.
pub(crate) struct VmPassPlugin;

impl Plugin for VmPassPlugin {
    fn build(&self, app: &mut App) {
        // The frame's presentation (its sounds, the GameObjects' animations) after the pass, so
        // it plays what this frame's script calls started.
        app.configure_sets(
            Update,
            benilla_world::schedule::WorldStage::Present.after(VmPass::End),
        );
    }

    fn cleanup(&self, app: &mut App) {
        let order = order_the_pass(app.world_mut(), Update);
        #[cfg(test)]
        app.insert_resource(PassOrder(order));
        #[cfg(not(test))]
        drop(order);
    }
}

/// Chains the pass of `label` (every host, and every other system of the UI's phases whose access
/// meets the host's) in a linear extension of the schedule's declared order, with
/// [`VmPass::Start`] before them all and [`VmPass::End`] after, ties going to the earlier
/// registered system; returns their names in that order. One that shares its function's set with
/// another system of the schedule cannot be ordered alone and is left out. Panics on a cycle in
/// the declared order, the pass's ends included.
pub(crate) fn order_the_pass(world: &mut World, label: impl ScheduleLabel) -> Vec<String> {
    let host = System::initialize(
        &mut IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {}),
        world,
    );
    let label = label.intern();
    if !world.resource::<Schedules>().contains(label) {
        return Vec::new();
    }
    world.schedule_scope(label, |world, schedule| {
        schedule.graph_mut().systems.initialize(world);
        let graph = schedule.graph();
        let keys: Vec<SystemKey> = graph.systems.iter().map(|(key, _, _)| key).collect();
        let index: HashMap<SystemKey, usize> =
            keys.iter().enumerate().map(|(i, key)| (*key, i)).collect();
        let mut sharers: HashMap<InternedSystemSet, usize> = HashMap::new();
        for (_, system, _) in graph.systems.iter() {
            for set in system.default_system_sets() {
                *sharers.entry(set).or_default() += 1;
            }
        }
        // The UI's phases, whose every system that touches what a call reaches joins the chain.
        let phases: Vec<&dyn SystemSet> = vec![
            &crate::ui_script::UiFeed,
            &crate::ui_script::UiInput,
            &crate::ui_script::UiDrain,
            &crate::ui_script::UiPaint,
        ];
        let mut in_phase = vec![false; keys.len()];
        let mut stack: Vec<NodeId> = graph
            .system_sets
            .iter()
            .filter(|(_, set, _)| phases.contains(set))
            .map(|(key, _, _)| NodeId::Set(key))
            .collect();
        while let Some(n) = stack.pop() {
            match n {
                NodeId::System(key) => in_phase[index[&key]] = true,
                NodeId::Set(_) => stack.extend(
                    graph
                        .hierarchy()
                        .graph()
                        .neighbors_directed(n, Direction::Outgoing),
                ),
            }
        }
        // A link's own set, when the system alone carries it: every host, and every other system
        // of the phases whose access meets the host's.
        let links: Vec<Option<InternedSystemSet>> = graph
            .systems
            .iter()
            .map(|(key, system, _)| {
                let declared = &graph.systems.get(key)?.access;
                let mut with_host = declared.clone();
                with_host.extend(host.clone());
                let hosted = with_host.combined_access() == declared.combined_access();
                let meets = in_phase[index[&key]] && !declared.is_compatible(&host);
                (!system.is_exclusive() && (hosted || meets)).then(|| {
                    system
                        .default_system_sets()
                        .into_iter()
                        .find(|set| sharers[set] == 1)
                })?
            })
            .collect();

        // The declared order as one graph: a system is a node, a set a pair of nodes that holds
        // its members between them, so an edge to a set reaches every member and one through an
        // empty set still links its neighbours, as the build flattens them.
        let mut set_nodes: HashMap<SystemSetKey, usize> = HashMap::new();
        let mut node = |id: NodeId| -> (usize, usize) {
            match id {
                NodeId::System(key) => (index[&key], index[&key]),
                NodeId::Set(key) => {
                    let next = keys.len() + 2 * set_nodes.len();
                    let at = *set_nodes.entry(key).or_insert(next);
                    (at, at + 1)
                }
            }
        };
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (parent, child) in graph.hierarchy().graph().all_edges() {
            let (p, c) = (node(parent), node(child));
            edges.push((p.0, c.0));
            edges.push((c.1, p.1));
        }
        for (a, b) in graph.dependency().graph().all_edges() {
            edges.push((node(a).1, node(b).0));
        }
        // The pass's ends hold every host between them.
        let end_of = |end: VmPass| {
            graph
                .system_sets
                .iter()
                .find(|(_, set, _)| *set == &end as &dyn SystemSet)
                .map(|(key, _, _)| key)
        };
        let (start, end) = (end_of(VmPass::Start), end_of(VmPass::End));
        for (i, link) in links.iter().enumerate() {
            if link.is_some() {
                if let Some(start) = start {
                    edges.push((node(NodeId::Set(start)).1, i));
                }
                if let Some(end) = end {
                    edges.push((i, node(NodeId::Set(end)).0));
                }
            }
        }
        let nodes = keys.len() + 2 * set_nodes.len();
        let mut after: Vec<Vec<usize>> = vec![Vec::new(); nodes];
        let mut waits = vec![0usize; nodes];
        for i in (keys.len()..nodes).step_by(2) {
            edges.push((i, i + 1));
        }
        for &(x, y) in &edges {
            after[x].push(y);
            waits[y] += 1;
        }
        // A set's own nodes go first, as they hold no system; systems by registration.
        let rank = |i: usize| (i < keys.len(), i);
        let mut ready: BinaryHeap<Reverse<(bool, usize)>> = (0..nodes)
            .filter(|&i| waits[i] == 0)
            .map(|i| Reverse(rank(i)))
            .collect();
        let mut chain = Vec::new();
        let mut placed = 0;
        while let Some(Reverse((_, i))) = ready.pop() {
            placed += 1;
            if let Some(set) = links.get(i).copied().flatten() {
                chain.push((set, i));
            }
            for &j in &after[i] {
                waits[j] -= 1;
                if waits[j] == 0 {
                    ready.push(Reverse(rank(j)));
                }
            }
        }
        if placed != nodes {
            let mut sets: Vec<Option<SystemSetKey>> = vec![None; nodes];
            for (key, at) in &set_nodes {
                sets[*at] = Some(*key);
                sets[*at + 1] = Some(*key);
            }
            let name = |i: usize| match sets[i] {
                Some(key) => graph
                    .system_sets
                    .get(key)
                    .map(|set| format!("{set:?}"))
                    .unwrap_or_default(),
                None => graph
                    .systems
                    .get(keys[i])
                    .map(|s| s.system.name().to_string())
                    .unwrap_or_default(),
            };
            // An unplaced node waits on an unplaced one; walking back from any closes a cycle.
            let mut at = (0..nodes)
                .find(|&i| waits[i] > 0)
                .expect("an unplaced node");
            let mut path = vec![at];
            loop {
                at = edges
                    .iter()
                    .find(|(x, y)| *y == at && waits[*x] > 0)
                    .map(|(x, _)| *x)
                    .expect("an unplaced node waits on an unplaced one");
                if let Some(from) = path.iter().position(|&p| p == at) {
                    path.drain(..from);
                    break;
                }
                path.push(at);
            }
            path.reverse();
            let mut names: Vec<String> = path.into_iter().map(name).collect();
            names.dedup();
            panic!(
                "the declared order has a cycle through the VM pass, a system before \
                 `VmPass::Start` after a host or one after `VmPass::End` before one:\n  {}",
                names.join("\n  -> ")
            );
        }
        // A system's own set takes no configuration, so each link is an empty set between two of
        // the chain's systems, and the pass's ends one before the first and one after the last.
        let names: Vec<String> = chain
            .iter()
            .map(|(_, i)| {
                graph
                    .systems
                    .get(keys[*i])
                    .map(|s| s.system.name().to_string())
                    .unwrap_or_default()
            })
            .collect();
        let chain: Vec<InternedSystemSet> = chain.into_iter().map(|(set, _)| set).collect();
        let (Some(&first), Some(&last)) = (chain.first(), chain.last()) else {
            return names;
        };
        schedule.configure_sets(VmPass::Start.before(first));
        schedule.configure_sets(VmPass::End.after(last));
        for (i, pair) in chain.windows(2).enumerate() {
            schedule.configure_sets(PassLink(i).after(pair[0]).before(pair[1]));
        }
        names
    })
}

#[cfg(test)]
mod tests {
    use crate::game_plugins::schedule_tests::{
        census, headless_client, headless_client_with, SyncPoints,
    };
    use bevy::prelude::*;

    /// The pass keeps the frame's order where the reference's is known: the net drain's handlers
    /// (the chat feed) before the frame's `OnUpdate` walk (the tick) and the input pass, the drains
    /// of what their Lua asked after both, and the paint last.
    #[test]
    fn the_pass_runs_the_feeds_then_the_tick_then_the_drains() {
        let app = headless_client();
        let order = &app.world().resource::<super::PassOrder>().0;
        if !order.iter().any(|n| n.contains("benilla_app::")) {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            return;
        }
        let at = |name: &str| {
            order
                .iter()
                .position(|n| n.ends_with(name))
                .unwrap_or_else(|| panic!("`{name}` is in the pass: {order:?}"))
        };
        let (feed, tick, input) = (
            at("::feed_chat"),
            at("::tick_script"),
            at("::feed_ui_input"),
        );
        let (drain, paint) = (at("::drain_chat_input"), at("::paint_script"));
        assert!(
            feed < tick && tick < drain && input < drain && drain < paint,
            "{order:?}"
        );
    }

    /// Every two hosts are ordered, the probes' and the instruments' too: no two systems that lend
    /// the call host run in an order the executor picks.
    #[test]
    fn every_two_hosts_run_in_a_declared_order() {
        let mut app = headless_client_with(crate::dev::add_every_instrument);
        let update = census(&mut app, Update, SyncPoints::Declared);
        let mut unordered: Vec<String> = update
            .conflicts
            .iter()
            .filter(|(a, b, _)| update.hosted.contains(a) && update.hosted.contains(b))
            .map(|(a, b, _)| format!("{} <-> {}", update.name(*a), update.name(*b)))
            .collect();
        unordered.sort();
        assert!(
            update.hosted.len() > 200,
            "the census finds the hosts: {}",
            update.hosted.len()
        );
        assert!(
            unordered.is_empty(),
            "{} pairs of hosts in no declared order:\n  {}",
            unordered.len(),
            unordered.join("\n  ")
        );
    }
}
