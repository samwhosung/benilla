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
//! release's stamp, a step in its place whose listeners run inside it ([`run_owed`]).

use std::ops::DerefMut;

use benilla_protocol::{ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::Selection;
use crate::lanes::Stamp;
use crate::net::NetHandlerApp;

/// The teardowns the packet's releases owe, each at its release's stamp.
#[derive(Resource, Default)]
pub(crate) struct OwedReleases(Vec<(u64, Stamp)>);

/// Registers the teardowns after the handlers that release objects (`crate::net` builds first),
/// the selects owed at each packet's end, and the lane that runs the selects a system or a lane
/// owed.
pub(super) fn register(app: &mut App) {
    use crate::lanes::LaneApp;
    crate::net::handlers::add_packet_end(app, super::scan::run_owed_selects);
    app.init_resource::<OwedReleases>()
        .init_resource::<super::scan::OwedSelects>()
        .net_handler(SessionEventKind::ObjectsRemoved, on_objects_removed)
        .net_handler(SessionEventKind::ObjectDestroyed, on_object_destroyed)
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

/// The packet's owed teardowns, each run nested under its release's stamp, so its sends go out in
/// the packet's place and what it signals fires there, among the other lanes' items.
fn run_owed(world: &mut World) {
    let Some(mut owed) = world.get_resource_mut::<OwedReleases>() else {
        return;
    };
    for (guid, at) in std::mem::take(&mut owed.0) {
        crate::lanes::firing(at, || tear_down(world, guid));
    }
}

/// The teardown `0x493910(guid, 1)` ([`super::OwedSelect::Deselect`]), a step in its place
/// ([`in_place`]).
fn tear_down(world: &mut World, guid: u64) {
    in_place(world, |world| {
        let select = super::OwedSelect::Deselect(guid);
        if let Err(err) = world.run_system_cached_with(super::scan::owed_select, select) {
            // A harness without the attack seam's state tears nothing down.
            debug!("release: the teardown did not run: {err}");
        }
    });
}

/// Run `step`, a selection's engine turn inside a packet, as a step in its place
/// ([`crate::lanes::step`]), its unit edits applied as it returns: what was stamped before it
/// fires first, then its signals' listeners run inside it, as the reference's teardown and
/// selects signal inside their packet (`0x493a2a`, `0x493a73`, `0x49389c`).
pub(super) fn in_place(world: &mut World, step: impl FnOnce(&mut World)) {
    crate::lanes::step(world, |world| {
        step(world);
        crate::creature_anim::apply_unit_edits(world);
    });
}
