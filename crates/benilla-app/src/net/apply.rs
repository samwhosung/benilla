//! The per-frame wire→ECS bridge systems: [`apply_net_updates`] drains the inbound
//! [`SessionEvent`] channel through the handler table, and [`tag_self_player`] marks our own
//! streamed entity. The parent module owns the channel/type surface and, since 2327, every
//! packet's handler is a subsystem's own or the bridge's own ([`super::session`],
//! [`super::objects`], [`super::world`]); nothing is applied here.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::{Guid, NetEvents, SelfGuid, SelfPlayer};

#[cfg(test)]
mod seam_tests;

// ── The per-frame bridge systems ─────────────────────────────────────────────────────────────────

/// The drain: take this frame's events off the channel and run each through the handler table
/// ([`super::handlers`]) **in wire order**, one after another, each handler a one-shot system
/// over this exclusive world with its commands applied before the next (
/// 2327). One frame, packet order, before anything else in
/// [`benilla_world::schedule::WorldStage::Net`] runs: the property 0006 built and 2265 said
/// every split must keep.
pub(crate) fn apply_net_updates(world: &mut World) {
    let events: Vec<SessionEvent> = world.resource::<NetEvents>().0.try_iter().collect();
    super::handlers::dispatch(world, events);
}

/// Tag our own player's streamed entity with [`SelfPlayer`] once we know our guid — by matching the
/// [`Guid`] component against [`SelfGuid`]. The renderer skips this entity (the controller owns our
/// avatar); the controller reads its transform to take control. Done as its own pass (rather than at
/// spawn) so it's robust to the order our guid and our create packet arrive in.
///
/// The controller's animation motion source (`MovementState`) rides the tag: a cross-map worldport
/// despawns every tracked entity — our avatar included — and the new map re-streams it, so any
/// per-entity state attached only at the one-shot take-control edge is lost on transfer. That was
/// the ".tele to another continent" bug: the re-tagged avatar had no `MovementState`, the anim
/// selector read it as stationary, and it slid around in the Stand pose.
pub(super) fn tag_self_player(
    mut commands: Commands,
    self_guid: Res<SelfGuid>,
    untagged: Query<(Entity, &Guid), Without<SelfPlayer>>,
) {
    let Some(me) = self_guid.0 else {
        return;
    };
    for (entity, guid) in &untagged {
        if guid.0 == me {
            // Identity only. The controller-fed [`crate::creature_anim::MovementState`] used to
            // ride along here, but it belongs to whichever body we are *steering*, which is not
            // always this one — `player::embody` owns it now.
            commands.entity(entity).insert(SelfPlayer);
        }
    }
}
