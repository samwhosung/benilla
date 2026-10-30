//! The unit callbacks of the update packet's notify pass that act beyond the notice lane: the
//! reference runs each inside the pass (`0x465277` → `0x465330`), at its watch's field, before the
//! packet's handler returns. Here the packet's end walks the field edges and the aura watcher's
//! calls the pass wrote, in the order it wrote them, and runs each callback in place, inside the
//! dispatch:
//!
//! - the combat log's aura, stack and loyalty lines (`0x604d00`, `0x604ea0`, `0x604530` →
//!   `0x5ff860`), [`crate::ui_chat::combat::watch`]'s;
//! - the health watcher's death (`0x6046f0` → `OnDied 0x605860`), its line among its legs, and
//!   the active player's precast end on a selection still held after it (`0x6047e7`),
//!   `crate::target`'s;
//! - the form watcher's latch release (`0x5de07f`) and the lost service flags' closes (`0x604a20`
//!   → `0x60b420`), [`crate::ui_session`]'s;
//! - the pet name stamp's re-ask and `LOCALPLAYER_PET_RENAMED` (`0x604aa0`), [`crate::ui_pet`]'s.
//!
//! What a callback pushes onto a lane is stamped under its edge, so it fires where the watcher ran
//! among what the packets raised. The Lua a callback's cores signal is held as it runs and parked
//! behind it on this module's lane ([`Parked`]), at a stamp taken as it ends. The item cache's
//! answer parks its VM callbacks here too ([`crate::net::objects`]): the equip replays and the
//! quest log's `QUEST_LOG_UPDATE`; and a refused record's walk goes on here behind its signals.

use std::collections::VecDeque;

use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};

use crate::lanes::{Stamp, Until};
use crate::net::{AuraFieldsChanged, FieldChanged, NetHandlerApp};

/// The Lua the callbacks' legs signalled, parked in the VM, each with its stamp, for the field
/// callbacks' lane.
#[derive(Resource, Default)]
pub(crate) struct FieldCallbacks(VecDeque<(Stamp, Item)>);

/// One item of the lane.
#[derive(Clone, Copy)]
pub(crate) enum Item {
    /// A leg's signals, under the VM's session and key.
    Parked(u64, u64),
    /// A refused record's walk, gone on behind its signals
    /// ([`crate::net::continue_refusal_walk`]).
    Walk(u32, crate::lanes::Mark),
}

/// Queue the walk of `entry` that began at `from` to go on behind what it has signalled so far.
pub(crate) fn walk_on(world: &mut World, entry: u32, from: crate::lanes::Mark) {
    let at = crate::lanes::stamp();
    if let Some(mut lane) = world.get_resource_mut::<FieldCallbacks>() {
        lane.0.push_back((at, Item::Walk(entry, from)));
    }
}

/// The VM's end, a reload's or the session's: what the callbacks parked goes with it.
pub(crate) fn drop_parked(world: &mut World) {
    if let Some(mut parked) = world.get_resource_mut::<FieldCallbacks>() {
        parked.0.clear();
    }
}

/// Registers the walk on the update packet's end, after the update handler that writes the edges
/// (the net plugin builds first), and the lane that fires the callbacks' parked signals.
pub(crate) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<FieldCallbacks>()
        .net_handler(SessionEventKind::ObjectUpdateEnd, on_update_end)
        .add_exclusive_lane("field callbacks", peek_parked, fire_parked, |_| {
            Default::default()
        });
}

/// The packet's edges and aura calls, as its notify pass wrote them.
type PassReaders = (
    MessageReader<'static, 'static, FieldChanged>,
    MessageReader<'static, 'static, AuraFieldsChanged>,
);

/// The packet's callbacks, each run in place.
fn on_update_end(
    In(ev): In<SessionEvent>,
    world: &mut World,
    readers: &mut SystemState<PassReaders>,
) {
    let SessionEvent::ObjectUpdateEnd = ev else {
        return;
    };
    let (edges, calls) = {
        let (mut edges, mut calls) = readers.get_mut(world);
        let edges: Vec<FieldChanged> = edges.read().copied().collect();
        let calls: Vec<AuraFieldsChanged> = calls.read().copied().collect();
        (edges, calls)
    };
    run(world, &edges, &calls);
}

/// One packet's callbacks: the lines, which write nothing a callback reads, then the callbacks
/// that act on the world, edge by edge in the pass's order.
pub(crate) fn run(world: &mut World, edges: &[FieldChanged], calls: &[AuraFieldsChanged]) {
    crate::ui_chat::combat::watch::lines(world, edges, calls);
    for &e in edges {
        if e.unit_died() {
            // What the legs stamp nests under the edge, ahead of what the packet raised after it:
            // `OnDied` (`0x6047a3`), then the watcher's precast end for a selection that stayed.
            crate::lanes::firing(e.at, || {
                crate::target::died(world, e);
                crate::target::dead_selection_ends_precast(world, e);
            });
        }
        crate::ui_session::on_field_edge(world, e);
        crate::ui_pet::on_field_edge(world, e);
    }
}

/// A callback's deferred stretch on the world's VM: each leg's signals held as it runs and parked
/// behind it on the field callbacks' lane, at a stamp taken as it ends, after what it pushed onto
/// the other lanes. With no VM nothing signals.
pub(crate) struct Parked(Option<(benilla_ui::script::Deferring, usize)>);

impl Parked {
    pub(crate) fn new(world: &World) -> Self {
        Self(crate::script_calls::defer_vm(world))
    }

    /// [`Self::new`] split as a close's halves: the legs' calls apply at once through the lent
    /// host, their signals and writes park ([`crate::script_calls::defer_vm_split`]).
    pub(crate) fn split(world: &World) -> Self {
        Self(crate::script_calls::defer_vm_split(world))
    }

    /// Run `leg`, then park what it signalled.
    pub(crate) fn leg(&mut self, world: &mut World, leg: impl FnOnce(&mut World)) {
        leg(world);
        let Some((_, mark)) = &self.0 else {
            return;
        };
        if let Some((session, key)) = crate::script_calls::park_since(world, *mark) {
            let at = crate::lanes::stamp();
            if let Some(mut lane) = world.get_resource_mut::<FieldCallbacks>() {
                lane.0.push_back((at, Item::Parked(session, key)));
            }
        }
    }
}

/// The lane's head.
fn peek_parked(world: &World) -> Option<Stamp> {
    world
        .get_resource::<FieldCallbacks>()?
        .0
        .front()
        .map(|(at, _)| *at)
}

/// Each item `until` admits, run as the firing of its stamp: a leg's signals with the host lent,
/// or a walk's next callbacks. The lane stops after one whose firing stamped anything, so what it
/// pushed fires next.
fn fire_parked(In(until): In<Until>, world: &mut World) {
    loop {
        let Some((at, item)) = world
            .resource::<FieldCallbacks>()
            .0
            .front()
            .copied()
            .filter(|(at, _)| until.admits(*at))
        else {
            return;
        };
        world.resource_mut::<FieldCallbacks>().0.pop_front();
        let ((), moved) = crate::lanes::firing(at, || match item {
            Item::Parked(session, key) => crate::script_calls::run_parked(world, session, key),
            Item::Walk(entry, from) => crate::net::continue_refusal_walk(world, entry, from),
        });
        if moved {
            return;
        }
    }
}

/// Every parked signal queued, fired alone, for a test that drives the callbacks without the
/// other lanes.
#[cfg(test)]
pub(crate) fn fire_parked_lane(world: &mut World) {
    while world
        .resource::<FieldCallbacks>()
        .0
        .front()
        .is_some_and(|(at, _)| Until::all().admits(*at))
    {
        world
            .run_system_cached_with(fire_parked, Until::all())
            .expect("the field callbacks' lane fires");
    }
}

/// [`run`] on every edge and aura call written and not yet taken, as a packet's end would: for a
/// test that writes the notify pass's output by hand.
#[cfg(test)]
pub(crate) fn run_written(world: &mut World) {
    let take = |world: &mut World| -> (Vec<FieldChanged>, Vec<AuraFieldsChanged>) {
        let edges = world
            .get_resource_mut::<Messages<FieldChanged>>()
            .map(|mut m| m.drain().collect())
            .unwrap_or_default();
        let calls = world
            .get_resource_mut::<Messages<AuraFieldsChanged>>()
            .map(|mut m| m.drain().collect())
            .unwrap_or_default();
        (edges, calls)
    };
    let (edges, calls) = take(world);
    run(world, &edges, &calls);
}
