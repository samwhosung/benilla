//! The VM pass: every system that lends the call host ([`crate::script_calls`]), in one declared
//! order, with every other system that touches what a script call reaches and runs among them: a
//! system of the UI's phases whose access meets the host's, a user of the host's shared sinks
//! ([`shared_sinks`]), and a world system placed in [`VmPass::Member`]. The reference runs its
//! script work on one thread, each verb applied inside its call. benilla's hosts are serialized
//! already, the VM being non-`Send` and the host mutable, but the executor picks among the ready
//! ones; [`order_the_pass`] chains them between the pass's two ends ([`VmPass`]), so they run in
//! the same order every frame.
//!
//! The order is the schedule's own: the phases' sets and every declared edge; then the drains of
//! what the Lua asked ([`crate::ui_script::UiDrain`]) after every other system they are free to
//! follow and before the paint, so an ask made anywhere ahead of the paint is drained, and what
//! the drain queued applied, before the frame paints; then, where nothing
//! is declared (the reference's order among them is not known), the order the app registers them
//! in, which only a changed registration moves. The reference's tick runs its due timers, then the
//! queued window input (`0x423920`, called at `0x420d23`), each record whole through its
//! category's handlers (`0x4239a0`, `0x4245b0`), then category 6 (`0x420d55`), where the packet
//! dispatch `0x403620` is registered, then the frame time, category 5 (`0x420f70`); which category
//! the frame's `OnUpdate` walk runs in is not read. benilla runs the net feeds, then the tick, then
//! the UI's input records, and its bindings and world clicks after the input stage: the window
//! input ahead of the net drain is an order it does not keep yet.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use bevy::ecs::component::ComponentId;
use bevy::ecs::schedule::graph::Direction;
use bevy::ecs::schedule::{InternedSystemSet, NodeId, ScheduleLabel, SystemKey, SystemSetKey};
use bevy::ecs::system::{IntoSystem, System};
use bevy::prelude::*;

/// The pass's sets. [`VmPass::Start`] and [`VmPass::End`] hold the chain between them: every
/// system whose declared access covers the call host's ([`crate::script_calls::ScriptCallHost`]),
/// every one that writes the VM, and every other system [`order_the_pass`] takes in. A world
/// system that moves what a script call reads runs before [`VmPass::Start`]; one that reads what
/// a call writes runs after [`VmPass::End`]. [`order_the_pass`] links them to the chain's first
/// and last; the end holds the flush of the unit edits the chain queued
/// ([`crate::creature_anim::flush_unit_edits`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VmPass {
    Start,
    End,
    /// A world system that reads or writes what a script call reaches and must run among the
    /// hosts, not before or after them all: chained in the pass's order as a host is.
    Member,
}

/// The systems of `Update`'s pass in the order it runs them, by system name, for the tests.
#[cfg(test)]
#[derive(Resource, Debug, Default)]
pub(crate) struct PassOrder(pub(crate) Vec<String>);

/// One link of the chain: an empty set after one of its systems and before the next.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PassLink(usize);

/// An empty set after a drain a host follows and before the world's input stage.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DrainBeforeInput(usize);

/// An empty set after a drain no host follows and before the paint.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DrainBeforePaint(usize);

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
        // The unit edits the pass's seams queued, flushed at its end: every system after the pass
        // reads them.
        app.init_resource::<crate::creature_anim::UnitEdits>()
            .add_systems(
                Update,
                crate::creature_anim::flush_unit_edits.in_set(VmPass::End),
            );
    }

    fn cleanup(&self, app: &mut App) {
        let order = order_the_pass(app.world_mut(), Update);
        flush_unit_edits_elsewhere(app.world_mut());
        #[cfg(test)]
        app.insert_resource(PassOrder(order));
        #[cfg(not(test))]
        drop(order);
    }
}

/// Every schedule but `Update` that holds a system able to queue a unit edit (a host's own, a
/// sweep's) flushes the queue after them and after each of its exclusive systems, so the edits
/// apply at that schedule's end, as the commands its systems queue do. An exclusive system
/// applies what it makes as it goes ([`crate::script_calls::with_hosted_vm`],
/// [`crate::script_calls::run_cached`]), and the flush follows it in case it left any.
///
/// The queue applies in the order it was pushed, and every edit is our own player's, so two of
/// its users must run in a declared order wherever both can push. In these schedules none run
/// unordered: the world-leave sweep chains its legs in the reference's call order; `PreUpdate`
/// declares its VM edges (the memory watch, the reload, the glue's, the entry load) in one
/// order; and in `Last` and `OnExit(InWorld)` the one system that fires the shutdown's events
/// is exclusive, beside users that fire none (the chat save reads the VM; the session's ends
/// push state into it, and the chat's end clears the stock windows).
fn flush_unit_edits_elsewhere(world: &mut World) {
    let Some(edits) = world
        .components()
        .get_resource_id(std::any::TypeId::of::<crate::creature_anim::UnitEdits>())
    else {
        return;
    };
    let labels: Vec<_> = world
        .resource::<Schedules>()
        .iter()
        .map(|(_, schedule)| schedule.label())
        .filter(|label| *label != Update.intern())
        .collect();
    for label in labels {
        world.schedule_scope(label, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            let (exclusive, users): (Vec<_>, Vec<_>) = graph
                .systems
                .iter()
                .filter(|(key, system, _)| {
                    graph.systems.get(*key).is_some_and(|s| {
                        system.is_exclusive() || s.access.combined_access().has_resource_read(edits)
                    })
                })
                .filter_map(|(_, system, _)| {
                    let set: InternedSystemSet = system.default_system_sets().into_iter().next()?;
                    Some((system.is_exclusive(), set))
                })
                .partition(|(exclusive, _)| *exclusive);
            if users.is_empty() {
                return;
            }
            // The queue is written at once, so the flush waits on no command of theirs.
            let mut flush = crate::creature_anim::flush_unit_edits.into_configs();
            for (_, set) in users.into_iter().chain(exclusive) {
                flush = flush.after_ignore_deferred(set);
            }
            schedule.add_systems(flush);
        });
    }
}

/// The host's append-only sinks, which it and their other users share by `&self`: the chat log,
/// the refusal keys, the message sounds and the unit edits. Each user's declared access reads
/// them only, so [`order_the_pass`] takes every user in as though it wrote them: one runs at a
/// time, a call's refusals ([`crate::ui_cast::CallReports`]) are the call's alone, and the edits
/// apply in the order they were made.
pub(crate) fn shared_sinks(world: &World) -> Vec<ComponentId> {
    [
        std::any::TypeId::of::<crate::ui_chat::ChatLog>(),
        std::any::TypeId::of::<crate::ui_action::UiErrorKeys>(),
        std::any::TypeId::of::<crate::sound::MessageSounds>(),
        std::any::TypeId::of::<crate::creature_anim::UnitEdits>(),
    ]
    .into_iter()
    .filter_map(|t| world.components().get_resource_id(t))
    .collect()
}

/// Chains the pass of `label` in a linear extension of the schedule's declared order, with
/// [`VmPass::Start`] before them all and [`VmPass::End`] after; returns their names in that order.
/// Where the declared order leaves it free: a drain that no declared order puts ahead of another
/// system of the chain runs after every other but the paint and what follows it, with a sync point
/// before the paint; a drain one does (the input pass's readers wait on it) runs before the
/// world's input stage; and the rest go by sync depth, then registration. The
/// chain takes every host; every system of the UI's phases or of [`VmPass::Member`] whose access
/// meets the host's; and every user of a [`shared_sinks`] sink not already wholly before
/// [`VmPass::Start`] or after [`VmPass::End`]. One that shares its function's set with another
/// system of the schedule cannot be ordered alone and is left out. A link joins two neighbours
/// the declared order leaves unordered and ignores the deferred buffers between them, which apply
/// at the first sync point past the chain. The call host holds none: what its seams change on a
/// unit waits in [`crate::creature_anim::UnitEdits`] for the pass's end, and no link reads it
/// (`the_host_defers_nothing_and_no_link_reads_a_unit_edit`). Two neighbours the declared order
/// already orders keep that order and its sync point. Panics on a cycle in the declared order,
/// the pass's ends included.
pub(crate) fn order_the_pass(world: &mut World, label: impl ScheduleLabel) -> Vec<String> {
    let host = System::initialize(
        &mut IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {}),
        world,
    );
    let sinks = shared_sinks(world);
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
        // The members of `sets`, nested sets included.
        let members = |sets: &[&dyn SystemSet]| -> Vec<bool> {
            let mut of = vec![false; keys.len()];
            let mut stack: Vec<NodeId> = graph
                .system_sets
                .iter()
                .filter(|(_, set, _)| sets.contains(set))
                .map(|(key, _, _)| NodeId::Set(key))
                .collect();
            while let Some(n) = stack.pop() {
                match n {
                    NodeId::System(key) => of[index[&key]] = true,
                    NodeId::Set(_) => stack.extend(
                        graph
                            .hierarchy()
                            .graph()
                            .neighbors_directed(n, Direction::Outgoing),
                    ),
                }
            }
            of
        };
        // The UI's phases and the pass's world members, whose every system that touches what a
        // call reaches joins the chain.
        let in_phase = members(&[
            &crate::ui_script::UiFeed,
            &crate::ui_script::UiInput,
            &crate::ui_script::UiDrain,
            &crate::ui_script::UiPaint,
            &VmPass::Member,
        ]);
        let drain = members(&[&crate::ui_script::UiDrain]);
        // Why each system joins: a host, a phase member that meets the host, a sink's user. A
        // link's own set is its function's, when the system alone carries it.
        #[derive(Clone, Copy, PartialEq)]
        enum Joins {
            No,
            Always,
            Sink,
        }
        let joins: Vec<Joins> = graph
            .systems
            .iter()
            .map(|(key, system, _)| {
                let Some(declared) = graph.systems.get(key).map(|s| &s.access) else {
                    return Joins::No;
                };
                if system.is_exclusive() {
                    return Joins::No;
                }
                let combined = declared.combined_access();
                let mut with_host = declared.clone();
                with_host.extend(host.clone());
                let hosted = with_host.combined_access() == combined;
                let meets = in_phase[index[&key]] && !declared.is_compatible(&host);
                if hosted || meets {
                    Joins::Always
                } else if sinks.iter().any(|id| combined.has_resource_read(*id)) {
                    Joins::Sink
                } else {
                    Joins::No
                }
            })
            .collect();
        let own_set: Vec<Option<InternedSystemSet>> = graph
            .systems
            .iter()
            .map(|(_, system, _)| {
                system
                    .default_system_sets()
                    .into_iter()
                    .find(|set| sharers[set] == 1)
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
        // The pass's ends, which hold the chain between them.
        let end_of = |end: VmPass| {
            graph
                .system_sets
                .iter()
                .find(|(_, set, _)| *set == &end as &dyn SystemSet)
                .map(|(key, _, _)| key)
        };
        let (start, end) = (
            end_of(VmPass::Start).map(|key| node(NodeId::Set(key))),
            end_of(VmPass::End).map(|key| node(NodeId::Set(key))),
        );
        let input = graph
            .system_sets
            .iter()
            .find(|(_, set, _)| {
                *set == &benilla_world::schedule::WorldStage::Input as &dyn SystemSet
            })
            .map(|(key, _, _)| node(NodeId::Set(key)));
        let paint = graph
            .system_sets
            .iter()
            .find(|(_, set, _)| *set == &crate::ui_script::UiPaint as &dyn SystemSet)
            .map(|(key, _, _)| node(NodeId::Set(key)));
        let nodes = keys.len() + 2 * set_nodes.len();
        for i in (keys.len()..nodes).step_by(2) {
            edges.push((i, i + 1));
        }
        // What the declared order already puts wholly before the pass's start or after its end,
        // the ends' own members included: a sink's user there is ordered against every link and
        // stays out of the chain.
        let reach = |from: usize, forward: bool| -> Vec<bool> {
            let mut next: Vec<Vec<usize>> = vec![Vec::new(); nodes];
            for &(x, y) in &edges {
                if forward {
                    next[x].push(y);
                } else {
                    next[y].push(x);
                }
            }
            let mut seen = vec![false; nodes];
            let mut stack = vec![from];
            while let Some(n) = stack.pop() {
                for &m in &next[n] {
                    if !seen[m] {
                        seen[m] = true;
                        stack.push(m);
                    }
                }
            }
            seen
        };
        let before_start = start.map_or(vec![false; nodes], |(_, at)| reach(at, false));
        let after_end = end.map_or(vec![false; nodes], |(at, _)| reach(at, true));
        let links: Vec<Option<InternedSystemSet>> = (0..keys.len())
            .map(|i| match joins[i] {
                Joins::Always => own_set[i],
                Joins::Sink if !before_start[i] && !after_end[i] => own_set[i],
                _ => None,
            })
            .collect();
        // Each node's sync depth in the declared order, the build's own measure: how many
        // command flushes stand ahead of it, a system with deferred params and a link of the chain
        // counting one each, as the chain carries the buffers ahead of a link to the next flush.
        // The chain takes its links in that order where it is free to, so it holds none back
        // behind a deeper one.
        let depth: Vec<u32> = {
            let deferred: Vec<bool> = graph
                .systems
                .iter()
                .map(|(_, system, _)| system.has_deferred())
                .collect();
            let mut next: Vec<Vec<usize>> = vec![Vec::new(); nodes];
            let mut waits = vec![0usize; nodes];
            for &(x, y) in &edges {
                next[x].push(y);
                waits[y] += 1;
            }
            let mut depth = vec![0u32; nodes];
            let mut ready: Vec<usize> = (0..nodes).filter(|&i| waits[i] == 0).collect();
            while let Some(x) = ready.pop() {
                let w = u32::from(x < keys.len() && (deferred[x] || links[x].is_some()));
                for &y in &next[x] {
                    depth[y] = depth[y].max(depth[x] + w);
                    waits[y] -= 1;
                    if waits[y] == 0 {
                        ready.push(y);
                    }
                }
            }
            depth
        };
        // A drain that some other link follows in the declared order (one the reference's order
        // or a same-frame reader puts before a host) keeps that place; every other drain runs
        // after every host the paint does not hold back, so an ask made anywhere ahead of the
        // paint is drained the frame it is made.
        let late: Vec<bool> = (0..keys.len())
            .map(|i| {
                drain[i] && links[i].is_some() && {
                    let ahead = reach(i, true);
                    !(0..keys.len()).any(|j| ahead[j] && links[j].is_some() && !drain[j])
                }
            })
            .collect();
        // A drain no host follows runs before the paint, and a sync point between applies the
        // world commands it queued (a window's close), so the frame paints what the asks left.
        let after_paint = paint.map(|(_, at)| reach(at, true));
        let after_input = input.map(|(_, at)| reach(at, true));
        let mut before_paint: Vec<InternedSystemSet> = Vec::new();
        if let (Some((paint_start, _)), Some(after_paint)) = (paint, after_paint) {
            for i in 0..keys.len() {
                if let (true, false, Some(set)) = (late[i], after_paint[i], links[i]) {
                    edges.push((i, paint_start));
                    before_paint.push(set);
                }
            }
        }
        // A drain a host follows serves the input pass's asks: it runs before the world's input
        // stage reads them, where its declared order lets it.
        let mut early: Vec<(InternedSystemSet, usize)> = Vec::new();
        if let (Some((input_start, _)), Some(after_input)) = (input, after_input) {
            for i in 0..keys.len() {
                if let (true, false, false, Some(set)) =
                    (drain[i], late[i], after_input[i], links[i])
                {
                    edges.push((i, input_start));
                    early.push((set, i));
                }
            }
        }
        for (i, link) in links.iter().enumerate() {
            if link.is_some() {
                if let Some((_, at)) = start {
                    edges.push((at, i));
                }
                if let Some((at, _)) = end {
                    edges.push((i, at));
                }
            }
        }
        let mut after: Vec<Vec<usize>> = vec![Vec::new(); nodes];
        let mut waits = vec![0usize; nodes];
        for &(x, y) in &edges {
            after[x].push(y);
            waits[y] += 1;
        }
        // A set's own nodes go first, as they hold no system; then every system but a late drain,
        // so one waits for every link the paint does not follow; each by sync depth, then
        // registration.
        let rank = |i: usize| (i < keys.len(), i < keys.len() && late[i], depth[i], i);
        let mut ready: BinaryHeap<Reverse<(bool, bool, u32, usize)>> = (0..nodes)
            .filter(|&i| waits[i] == 0)
            .map(|i| Reverse(rank(i)))
            .collect();
        let mut chain = Vec::new();
        let mut placed = 0;
        while let Some(Reverse((_, _, _, i))) = ready.pop() {
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
        // Whether the declared order already runs `from` before `to`. A link there would add no
        // order and would drop that order's sync point: the build keys an edge's deferred
        // buffers by the pair it joins, whichever declaration made it.
        let ordered = |from: usize, to: usize| -> bool {
            let mut seen = vec![false; nodes];
            let mut stack = vec![from];
            while let Some(n) = stack.pop() {
                for &m in &after[n] {
                    if m == to {
                        return true;
                    }
                    if !seen[m] {
                        seen[m] = true;
                        stack.push(m);
                    }
                }
            }
            false
        };
        let linked: Vec<bool> = chain
            .windows(2)
            .map(|pair| !ordered(pair[0].1, pair[1].1))
            .collect();
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
        for (n, (set, _)) in early.into_iter().enumerate() {
            schedule.configure_sets(
                DrainBeforeInput(n)
                    .after(set)
                    .before(benilla_world::schedule::WorldStage::Input),
            );
        }
        for (n, set) in before_paint.into_iter().enumerate() {
            schedule.configure_sets(
                DrainBeforePaint(n)
                    .after(set)
                    .before(crate::ui_script::UiPaint),
            );
        }
        schedule.configure_sets(VmPass::Start.before(first));
        // The end holds the unit edits' flush, which reads only their queue, written at once: the
        // chain's commands apply at the sync point past it, with the flush's own.
        schedule.configure_sets(VmPass::End.after_ignore_deferred(last));
        for (i, pair) in chain.windows(2).enumerate() {
            if linked[i] {
                schedule.configure_sets(
                    PassLink(i)
                        .after_ignore_deferred(pair[0])
                        .before_ignore_deferred(pair[1]),
                );
            }
        }
        names
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crate::game_plugins::schedule_tests::{
        census, headless_client, headless_client_with, Census, SyncPoints,
    };
    use bevy::ecs::component::ComponentId;
    use bevy::ecs::schedule::graph::Direction;
    use bevy::ecs::schedule::{NodeId, SystemKey};
    use bevy::ecs::system::{IntoSystem, System};
    use bevy::prelude::*;

    /// The pass's systems in order, or `None` in a build without type names, which skips.
    fn pass_order(app: &App) -> Option<Vec<String>> {
        let order = app.world().resource::<super::PassOrder>().0.clone();
        if order.iter().any(|n| n.contains("benilla_app::")) {
            Some(order)
        } else {
            eprintln!(
                "skipped: this build carries no type names (bevy/debug rides with the dev feature)"
            );
            None
        }
    }

    /// Where the system whose name ends with `name` runs in the pass.
    fn at(order: &[String], name: &str) -> usize {
        order
            .iter()
            .position(|n| n.ends_with(name))
            .unwrap_or_else(|| panic!("`{name}` is in the pass: {order:?}"))
    }

    /// The systems of `Update` under `set`, nested sets included; read before a census builds it.
    fn members(app: &mut App, set: impl SystemSet) -> HashSet<SystemKey> {
        app.world_mut().schedule_scope(Update, |_, schedule| {
            let graph = schedule.graph();
            let mut out = HashSet::new();
            let mut stack: Vec<NodeId> = graph
                .system_sets
                .iter()
                .filter(|(_, s, _)| *s == &set as &dyn SystemSet)
                .map(|(key, _, _)| NodeId::Set(key))
                .collect();
            while let Some(n) = stack.pop() {
                match n {
                    NodeId::System(key) => {
                        out.insert(key);
                    }
                    NodeId::Set(_) => stack.extend(
                        graph
                            .hierarchy()
                            .graph()
                            .neighbors_directed(n, Direction::Outgoing),
                    ),
                }
            }
            out
        })
    }

    /// Every system the declared order runs after `from`.
    fn after(next: &HashMap<SystemKey, Vec<SystemKey>>, from: SystemKey) -> HashSet<SystemKey> {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            for m in next.get(&n).into_iter().flatten() {
                if seen.insert(*m) {
                    stack.push(*m);
                }
            }
        }
        seen
    }

    fn successors(update: &Census) -> HashMap<SystemKey, Vec<SystemKey>> {
        let mut next: HashMap<SystemKey, Vec<SystemKey>> = HashMap::new();
        for (a, b) in &update.dependencies {
            next.entry(*a).or_default().push(*b);
        }
        next
    }

    /// The pass runs its phases in their declared order: the net feeds (the unit feed), then the
    /// frame's tick and the input pass, then the drains of what their Lua asked, then the paint.
    #[test]
    fn the_pass_runs_the_feeds_then_the_tick_then_the_drains() {
        let app = headless_client();
        let Some(order) = pass_order(&app) else {
            return;
        };
        let at = |name: &str| at(&order, name);
        let (feed, tick, input) = (
            at("::ui_unit::feed_units"),
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

    /// The call host carries no deferred buffer: what its seams change on a unit (a cast's reap,
    /// the auto-repeat arm and clear, [`crate::creature_anim::UnitEdit`]) waits in a queue the
    /// pass's end flushes, and no link of the pass reads what any kind of edit writes, so none
    /// reads it stale. A deferred param new to the host, or a link that reads an edit's component,
    /// fails here.
    #[test]
    fn the_host_defers_nothing_and_no_link_reads_a_unit_edit() {
        let mut app = headless_client();
        let Some(order) = pass_order(&app) else {
            return;
        };
        let order: HashSet<String> = order.into_iter().collect();
        let world = app.world_mut();
        let mut probe = IntoSystem::into_system(|_: crate::script_calls::ScriptCallHost| {});
        System::initialize(&mut probe, world);
        assert!(
            !probe.has_deferred(),
            "the call host holds a deferred param: queue its changes as unit edits instead"
        );
        let mut writes: Vec<(String, ComponentId)> = Vec::new();
        for edit in crate::creature_anim::UnitEdit::every(Entity::PLACEHOLDER) {
            // A component no system names is unregistered, and no link reads it.
            for id in edit
                .writes()
                .into_iter()
                .filter_map(|t| world.components().get_id(t))
            {
                writes.push((format!("{edit:?}"), id));
            }
        }
        let mut found = Vec::new();
        world.schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            for (key, system, _) in graph.systems.iter() {
                let name = system.name().to_string();
                if !order.contains(&name) {
                    continue;
                }
                let declared = &graph.systems.get(key).expect("initialized").access;
                let combined = declared.combined_access();
                for (edit, id) in &writes {
                    if combined.has_component_read(*id)
                        || declared
                            .filtered_accesses()
                            .iter()
                            .any(|fa| fa.contains(*id))
                    {
                        found.push(format!("{name} reads what {edit} writes"));
                    }
                }
            }
        });
        assert!(writes.len() >= 3, "the edits write components: {writes:?}");
        found.sort();
        found.dedup();
        assert!(
            found.is_empty(),
            "a link of the pass reads what a unit edit changes after the pass:\n  {}",
            found.join("\n  ")
        );
    }

    /// A unit edit applies where it is made: every system that can queue one runs ahead of a
    /// flush in its own schedule, `Update`'s at the pass's end. An exclusive system applies its
    /// own ([`crate::creature_anim::apply_unit_edits`]); outside `Update` the flush follows it as
    /// well, so a one-shot it ran that left an edit does not carry it into the next frame.
    #[test]
    fn every_unit_edit_is_flushed_in_the_schedule_that_makes_it() {
        use bevy::ecs::schedule::ScheduleLabel as _;
        #[derive(PartialEq)]
        enum Kind {
            User,
            Flush,
            Exclusive,
        }
        let mut app = headless_client_with(crate::dev::add_every_instrument);
        let edits = app
            .world()
            .components()
            .get_resource_id(std::any::TypeId::of::<crate::creature_anim::UnitEdits>())
            .expect("the queue is registered");
        let labels: Vec<_> = app
            .world()
            .resource::<Schedules>()
            .iter()
            .map(|(_, schedule)| schedule.label())
            .collect();
        // By identity, not by name: a build without type names has none.
        let flush = System::type_id(&IntoSystem::into_system(
            crate::creature_anim::flush_unit_edits,
        ));
        let (mut users, mut exclusives) = (0, 0);
        let mut late = Vec::new();
        for label in labels {
            let in_update = label == Update.intern();
            let queues: Vec<(SystemKey, String, Kind)> =
                app.world_mut().schedule_scope(label, |world, schedule| {
                    schedule.graph_mut().systems.initialize(world);
                    let graph = schedule.graph();
                    graph
                        .systems
                        .iter()
                        .filter_map(|(key, system, _)| {
                            let access = &graph.systems.get(key).expect("initialized").access;
                            let kind = if system.is_exclusive() {
                                Kind::Exclusive
                            } else if System::type_id(&**system) == flush {
                                Kind::Flush
                            } else if access.combined_access().has_resource_read(edits) {
                                Kind::User
                            } else {
                                return None;
                            };
                            Some((key, system.name().to_string(), kind))
                        })
                        .collect()
                });
            if !queues.iter().any(|(_, _, kind)| *kind == Kind::User) {
                continue;
            }
            let schedule = census(&mut app, label, SyncPoints::Declared);
            let next = successors(&schedule);
            let flushes: Vec<SystemKey> = queues
                .iter()
                .filter(|(_, _, kind)| *kind == Kind::Flush)
                .map(|(key, _, _)| *key)
                .collect();
            for (key, name, kind) in &queues {
                match kind {
                    Kind::Flush => continue,
                    Kind::Exclusive if in_update => continue,
                    Kind::Exclusive => exclusives += 1,
                    Kind::User => users += 1,
                }
                if !flushes.iter().any(|f| after(&next, *key).contains(f)) {
                    late.push(format!("{label:?}: {name}"));
                }
            }
        }
        assert!(users > 20, "the census finds the queue's users: {users}");
        assert!(
            exclusives > 10,
            "and the exclusive systems beside them: {exclusives}"
        );
        assert!(
            late.is_empty(),
            "systems that can queue unit edits after their schedule's flush, or where none \
             runs:\n  {}",
            late.join("\n  ")
        );
    }

    /// The host's shared sinks ([`super::shared_sinks`]) are written through `&self`, so their
    /// users declare reads and no conflict orders them: every two users run in a declared order,
    /// so the refusals a call reads back ([`crate::ui_cast::CallReports`]) are its own, and the
    /// lines and sounds land in one order every frame.
    #[test]
    fn every_user_of_a_shared_sink_runs_in_one_order() {
        let mut app = headless_client_with(crate::dev::add_every_instrument);
        let sinks = super::shared_sinks(app.world());
        assert_eq!(sinks.len(), 4, "the sinks are registered");
        let users: Vec<SystemKey> = app.world_mut().schedule_scope(Update, |world, schedule| {
            schedule.graph_mut().systems.initialize(world);
            let graph = schedule.graph();
            graph
                .systems
                .iter()
                .filter(|(key, system, _)| {
                    let combined = graph
                        .systems
                        .get(*key)
                        .expect("initialized")
                        .access
                        .combined_access();
                    !system.is_exclusive() && sinks.iter().any(|id| combined.has_resource_read(*id))
                })
                .map(|(key, _, _)| key)
                .collect()
        });
        let update = census(&mut app, Update, SyncPoints::Declared);
        let next = successors(&update);
        let reach: HashMap<SystemKey, HashSet<SystemKey>> =
            users.iter().map(|u| (*u, after(&next, *u))).collect();
        let mut unordered = Vec::new();
        for (i, a) in users.iter().enumerate() {
            for b in &users[i + 1..] {
                if !reach[a].contains(b) && !reach[b].contains(a) {
                    unordered.push(format!("{} <-> {}", update.name(*a), update.name(*b)));
                }
            }
        }
        unordered.sort();
        assert!(
            users.len() > 50,
            "the census finds the sinks' users: {}",
            users.len()
        );
        assert!(
            unordered.is_empty(),
            "{} pairs of a shared sink's users in no declared order:\n  {}",
            unordered.len(),
            unordered.join("\n  ")
        );
    }

    /// What a host's Lua asks is drained the frame it is made, a world click's included: a drain
    /// in the pass runs ahead of a host that follows the world's input stage (the click chain, the
    /// cinematic's feed) only where the declared order puts it there, as it does for a drain whose
    /// output the input stage reads or one the paint follows. So a click's target change plays
    /// the target frame's sound, and a `PlayMusic` it starts lands, the frame it is made.
    #[test]
    fn a_world_clicks_ask_is_drained_the_frame_it_is_made() {
        let mut app = headless_client();
        let Some(order) = pass_order(&app) else {
            return;
        };
        let input = members(&mut app, benilla_world::schedule::WorldStage::Input);
        let drains = members(&mut app, crate::ui_script::UiDrain);
        let update = census(&mut app, Update, SyncPoints::Declared);
        let next = successors(&update);
        let key: HashMap<&str, SystemKey> = update
            .systems
            .iter()
            .map(|(k, s)| (s.name.as_str(), *k))
            .collect();
        let after_input: HashSet<SystemKey> = input.iter().flat_map(|k| after(&next, *k)).collect();
        let post: Vec<usize> = (0..order.len())
            .filter(|i| {
                let k = key[order[*i].as_str()];
                update.hosted.contains(&k) && after_input.contains(&k) && !drains.contains(&k)
            })
            .collect();
        assert!(post.len() > 10, "hosts follow the input stage: {post:?}");
        let mut early = Vec::new();
        for (i, name) in order.iter().enumerate() {
            let drain = key[name.as_str()];
            if !drains.contains(&drain) {
                continue;
            }
            let ahead = after(&next, drain);
            for j in post.iter().filter(|j| **j > i) {
                if !ahead.contains(&key[order[*j].as_str()]) {
                    early.push(format!("{name} before {}", order[*j]));
                }
            }
        }
        assert!(
            early.is_empty(),
            "drains ahead of a host after the input stage that no declared order puts there:\n  {}",
            early.join("\n  ")
        );
        let asks = [at(&order, "::drain_ui_sounds"), at(&order, "::lua_music")];
        for host in ["::act_on_interact", "::cinematic::feed_ui"] {
            let host = at(&order, host);
            assert!(asks.iter().all(|a| host < *a), "{order:?}");
        }
        let paint = at(&order, "::paint_script");
        assert!(asks.iter().all(|a| *a < paint), "{order:?}");
    }

    /// The one system of `census` whose name ends with `suffix`.
    fn one(census: &Census, suffix: &str) -> SystemKey {
        let mut hits = census
            .systems
            .iter()
            .filter(|(_, s)| s.name.ends_with(suffix));
        match (hits.next(), hits.next()) {
            (Some((key, _)), None) => *key,
            _ => panic!("exactly one system named `…{suffix}` in Update"),
        }
    }

    /// A window the Lua closes hides before the frame paints: the drains that take a close
    /// (`CloseGossip()` and its kin) queue it as a world command, which a sync point applies
    /// ahead of the paint, so the frame the call is made never paints the window.
    #[test]
    fn a_window_the_lua_closes_is_hidden_before_the_paint() {
        let mut app = headless_client();
        if pass_order(&app).is_none() {
            return;
        }
        let built = census(&mut app, Update, SyncPoints::Built);
        let paint = one(&built, "::paint_script");
        let late: Vec<&str> = [
            "ui_gossip::drain_gossip",
            "ui_merchant::drain_merchant",
            "ui_trade::drain_trade",
            "ui_bank::drain_bank",
            "ui_quest::drain_quest",
        ]
        .into_iter()
        .filter(|drain| !built.applies_before(one(&built, drain), paint))
        .collect();
        assert!(
            late.is_empty(),
            "drains whose closes apply after the paint: {late:?}"
        );
    }

    /// A follow the Lua asks (`FollowUnit`, `FollowByName`) starts the frame it is made: the
    /// request's reader runs after the drain that writes it.
    #[test]
    fn a_follow_the_lua_asks_starts_the_frame_it_is_made() {
        let mut app = headless_client();
        if pass_order(&app).is_none() {
            return;
        }
        let update = census(&mut app, Update, SyncPoints::Declared);
        let next = successors(&update);
        let drain = one(&update, "ui_follow::drain_follow");
        let reader = one(&update, "by_name::follow_requests");
        assert!(
            after(&next, drain).contains(&reader),
            "`follow_requests` reads the `FollowRequest` before `drain_follow` writes it"
        );
    }

    /// A `SetCVar` the input pass makes moves the camera the frame it is made: the registry's
    /// sync triggers the observers (the camera's look, zoom and follow knobs) and a sync point
    /// runs them ahead of the controller, as the reference's `CVar::Set` (`0x63df50`) calls the
    /// change callback inside the call (`0x63df7d`).
    #[test]
    fn a_set_cvar_reaches_the_camera_the_frame_it_is_made() {
        let mut app = headless_client();
        if pass_order(&app).is_none() {
            return;
        }
        let built = census(&mut app, Update, SyncPoints::Built);
        let sync = one(&built, "cvars::sync_cvars");
        let control = one(&built, "controller::control");
        assert!(
            built.applies_before(sync, control),
            "the CVar observers run after the controller reads their knobs"
        );
    }

    /// The stream focus's publish and the post-snap hold's release, which write the player's
    /// settle state the hosts read, run among the hosts in the world's order (after the input
    /// stage, around the stream), so a host after them reads this frame's: the zone channels'
    /// walk, whose gate reads it, runs after the release.
    #[test]
    fn the_settle_state_is_published_before_its_readers() {
        let app = headless_client();
        let Some(order) = pass_order(&app) else {
            return;
        };
        let at = |name: &str| at(&order, name);
        let release = at("::release_post_snap_hold");
        assert!(
            at("::publish_view_focus") < release && release < at("::auto_join_zone_channels"),
            "{order:?}"
        );
    }
}
