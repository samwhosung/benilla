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

/// The Lua each teardown signalled, parked in the VM, with its stamp and the VM's session and key,
/// for the release lane.
#[derive(Resource, Default)]
pub(crate) struct Releases(VecDeque<(Stamp, u64, u64)>);

/// The VM's end, a reload's or the session's: what its releases parked goes with it.
pub(crate) fn drop_parked_releases(world: &mut World) {
    if let Some(mut releases) = world.get_resource_mut::<Releases>() {
        releases.0.clear();
    }
}

/// Registers the teardowns after the handlers that release objects (`crate::net` builds first),
/// and the lane that fires their signals.
pub(super) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<OwedReleases>()
        .init_resource::<Releases>()
        .net_handler(SessionEventKind::ObjectsRemoved, on_objects_removed)
        .net_handler(SessionEventKind::ObjectDestroyed, on_object_destroyed)
        .net_handler(SessionEventKind::Worldport, on_worldport)
        .add_exclusive_lane("releases", peek_releases, fire_releases, |_| {
            Default::default()
        });
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

/// The transfer's owed teardown ([`run_owed`]).
fn on_worldport(In(ev): In<SessionEvent>, world: &mut World) {
    if let SessionEvent::Worldport { .. } = ev {
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

/// [`tear_down`] on the world's VM with its signals held, then parked on the release lane at a
/// stamp taken as it ends, after what it pushed onto the other lanes. With no VM nothing signals.
fn tear_down_parked(world: &mut World, guid: u64) {
    let deferring = crate::script_calls::defer_vm(world);
    if let Err(err) = world.run_system_cached_with(tear_down, guid) {
        // A harness without the attack seam's state tears nothing down.
        debug!("release: the teardown did not run: {err}");
    }
    crate::creature_anim::apply_unit_edits(world);
    let Some((_held, mark)) = &deferring else {
        return;
    };
    if let Some((session, key)) = crate::script_calls::park_since(world, *mark) {
        let at = crate::lanes::stamp();
        world
            .resource_mut::<Releases>()
            .0
            .push_back((at, session, key));
    }
}

/// What a teardown reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
struct Teardown<'w, 's> {
    selection: ResMut<'w, Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
}

/// `0x493910(guid, 1)`: nothing unless `guid` is still the selection (`0x493949`); else the
/// loot close, the attack stop, the send, the usable refresh and `PLAYER_TARGET_CHANGED`, each
/// signal inside it ([`crate::script_calls::ScriptCallHost::engine_select`]). The last-target
/// pair stays: no `SetSelection` runs.
fn tear_down(In(guid): In<u64>, mut vm: crate::script_calls::VmWith<Teardown<'static, 'static>>) {
    let select = {
        let mut t = vm.own();
        if t.selection.guid != Some(guid) {
            return;
        }
        let before = super::click::Standing::of(&t.selection, &t.seam);
        let send = super::click::clear_owing(&mut t.selection, &mut t.seam);
        before.owing(super::scan::Owed { send, swing: None })
    };
    vm.engine_select(select, |mut t, owed| {
        owed.pay(&mut t.selection, &mut t.seam)
    });
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
