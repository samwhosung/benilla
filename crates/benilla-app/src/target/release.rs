//! The selection's object as the object manager holds it, and its end. The selection is a guid
//! (`[0xb4e2d8]`) and the client reads its object by that guid wherever it needs one, so
//! [`Selection::target`] is kept on whatever object the guid names, by the handlers that hold and
//! release objects.
//!
//! A unit's release (`CGUnit_C::Release 0x5fbb60`, its slot 1), destroyed or out of range, runs
//! the teardown `0x493910(guid, 1)` at `0x5fbbe5` unless `IsRaidMemberOrPet` (`0x491900`,
//! `0x5fbbcd`) holds its guid, whose selection stays as the guid alone. The teardown signals
//! inside the release, inside its packet (`0x493a2a`, `0x493a73`). Here the release decides in
//! place ([`unit_released`]), and its packet's own handler then runs each teardown nested under its
//! release's stamp, parking what it signals on the release lane ([`run_owed`]).

use std::collections::VecDeque;
use std::ops::DerefMut;

use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::Selection;
use crate::lanes::{Stamp, Until};
use crate::net::NetHandlerApp;

/// The teardowns the packet's releases owe, each at its release's stamp.
#[derive(Resource, Default)]
pub(crate) struct OwedReleases(Vec<(u64, Stamp)>);

/// The Lua each of a packet's selection changes signalled, parked in the VM, with its stamp and the
/// VM's session and key, for the release lane: a release's teardown, an acquire, the loot
/// response's select ([`super::scan::OwedSelects`]).
#[derive(Resource, Default)]
pub(crate) struct Releases(VecDeque<(Stamp, u64, u64)>);

/// The VM's end, a reload's or the session's: what its releases parked goes with it.
pub(crate) fn drop_parked_releases(world: &mut World) {
    if let Some(mut releases) = world.get_resource_mut::<Releases>() {
        releases.0.clear();
    }
}

/// Registers the teardowns after the handlers that release objects (`crate::net` builds first),
/// the selects owed at each packet's end, the lane that fires their signals, and the lane that
/// runs the selects a system or a lane owed.
pub(super) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    crate::net::handlers::add_packet_end(app, super::scan::run_owed_selects);
    app.init_resource::<OwedReleases>()
        .init_resource::<Releases>()
        .init_resource::<super::scan::OwedSelects>()
        .net_handler(SessionEventKind::ObjectsRemoved, on_objects_removed)
        .net_handler(SessionEventKind::ObjectDestroyed, on_object_destroyed)
        .add_exclusive_lane("releases", peek_releases, fire_releases, |_| {
            Default::default()
        })
        .add_exclusive_lane(
            "owed selects",
            super::scan::peek_owed_selects,
            super::scan::fire_owed_selects,
            |_| Default::default(),
        );
}

/// `0x5fbbcd`-`0x5fbbe5` on the unit `guid` as it is released, in place: nothing unless it is the
/// selection; the guid alone kept, its object gone, when a group row holds it ([`member`]); else
/// the teardown owed at this stamp.
pub(crate) fn unit_released<S: DerefMut<Target = Selection>>(
    selection: Option<&mut S>,
    owed: Option<&mut OwedReleases>,
    guid: u64,
    member: impl FnOnce() -> bool,
) {
    // Tested through `Deref`, so a release of another object leaves the selection unchanged.
    let Some(selection) = selection.filter(|s| s.guid == Some(guid)) else {
        return;
    };
    if member() {
        // The lit object goes with its release.
        selection.target = None;
        selection.lit = None;
    } else if let Some(owed) = owed {
        owed.0.push((guid, crate::lanes::stamp()));
    }
}

/// `IsRaidMemberOrPet` (`0x491900`) on `guid`, our player and each member found through `held`.
pub(crate) fn member<'a>(
    group: &crate::ui_party::GroupState,
    guid: u64,
    me: Option<u64>,
    held: &dyn Fn(u64) -> Option<&'a ObjectFields>,
) -> bool {
    group.member_or_pet(guid, me.map(|g| (g, held(g))), held)
}

/// An object the guid names again, from a create or a reactivation: the one every reader takes
/// (the target circle compares guids each frame, `0x614a96`), unlit, as only `SetSelection`
/// lights (`0x493701`).
pub(crate) fn object_held<S: DerefMut<Target = Selection>>(
    selection: Option<&mut S>,
    guid: u64,
    entity: Entity,
) {
    if let Some(selection) = selection.filter(|s| s.guid == Some(guid)) {
        selection.target = Some(entity);
        selection.lit = None;
    }
}

/// An out-of-range list's owed teardowns ([`run_owed`]).
fn on_objects_removed(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::ObjectsRemoved(_) = ev {
        run_owed(world);
    }
}

/// `SMSG_DESTROY_OBJECT`'s owed teardown ([`run_owed`]).
fn on_object_destroyed(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::ObjectDestroyed(_) = ev {
        run_owed(world);
    }
}

/// The packet's owed teardowns, each run as the firing of its release's stamp, so its sends go
/// out in the packet's place and what it signals fires among the other lanes' items there.
fn run_owed(world: &mut World) {
    let Some(mut owed) = world.get_resource_mut::<OwedReleases>() else {
        return;
    };
    for (guid, at) in std::mem::take(&mut owed.0) {
        crate::lanes::firing(at, || tear_down_parked(world, guid));
    }
}

/// The teardown `0x493910(guid, 1)` ([`super::OwedSelect::Deselect`]) on the world's VM with its
/// signals held and parked ([`park_signals`]).
fn tear_down_parked(world: &mut World, guid: u64) {
    park_signals(world, |world| {
        let select = super::OwedSelect::Deselect(guid);
        if let Err(err) = world.run_system_cached_with(super::scan::owed_select, select) {
            // A harness without the attack seam's state tears nothing down.
            debug!("release: the teardown did not run: {err}");
        }
    });
}

/// Run `step`, a selection's engine turn inside a packet, on the world's VM with its signals held,
/// its unit edits applied as it returns, then park what it signalled on the release lane at a
/// stamp taken as it ends, after what it pushed onto the other lanes. With no VM nothing signals.
pub(super) fn park_signals(world: &mut World, step: impl FnOnce(&mut World)) {
    let deferring = crate::script_calls::defer_vm(world);
    step(world);
    crate::creature_anim::apply_unit_edits(world);
    let Some((_held, mark)) = &deferring else {
        return;
    };
    if let Some((session, key)) = crate::script_calls::park_since(world, *mark) {
        let at = crate::lanes::stamp();
        world
            .get_resource_or_init::<Releases>()
            .0
            .push_back((at, session, key));
    }
}

/// The release lane's head.
fn peek_releases(world: &World) -> Option<Stamp> {
    world
        .get_resource::<Releases>()?
        .0
        .front()
        .map(|(at, ..)| *at)
}

/// Each teardown's signals `until` admits, run as the firing of their stamp with the host lent;
/// the lane stops after one whose firing stamped anything, so what it pushed fires next. It reads
/// nothing beside the host and its own lane.
fn fire_releases(In(until): In<Until>, world: &mut World) {
    loop {
        let Some((at, session, key)) = world
            .resource::<Releases>()
            .0
            .front()
            .copied()
            .filter(|(at, ..)| until.admits(*at))
        else {
            return;
        };
        world.resource_mut::<Releases>().0.pop_front();
        let ((), moved) =
            crate::lanes::firing(at, || crate::script_calls::run_parked(world, session, key));
        if moved {
            return;
        }
    }
}
