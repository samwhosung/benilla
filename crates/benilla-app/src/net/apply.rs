//! The per-frame wire→ECS bridge systems: [`apply_net_updates`] drains the inbound
//! [`SessionEvent`] channel through the handler table, [`tag_self_player`] marks our own
//! streamed entity, and [`enter_world_on_self_create`] runs what its create starts. Nothing is
//! applied here: every packet's handler lives with its subsystem.

use benilla_protocol::SessionEvent;
use bevy::prelude::*;

use super::{
    ClientCommand, Guid, NetCommands, NetEvents, SelfGuid, SelfPlayer, WorldEnterCascadeMessage,
};

#[cfg(test)]
mod seam_tests;
#[cfg(test)]
mod world_enter_tests;

// ── The per-frame bridge systems ─────────────────────────────────────────────────────────────────

/// Runs this frame's events through the handler table ([`super::handlers`]) in wire order, each
/// handler's commands applied before the next, before anything else in
/// [`benilla_world::schedule::WorldStage::Net`] runs.
pub(crate) fn apply_net_updates(world: &mut World) {
    let events: Vec<SessionEvent> = world.resource::<NetEvents>().0.try_iter().collect();
    super::handlers::dispatch(world, events);
}

/// Tags our own streamed entity with [`SelfPlayer`] by matching [`Guid`] against [`SelfGuid`].
/// A pass of its own, not at spawn, so either arrival order of our guid and our create block works;
/// a cross-map worldport re-streams the avatar and it is tagged again.
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
            // Identity only: `MovementState` belongs to the body we steer (`player::embody`).
            commands.entity(entity).insert(SelfPlayer);
        }
    }
}

/// Our own player's create, the reference's `0x5dea50` (reached `0x465dbc` → `0x5debe0` →
/// `0x5dec7b`): `SetActiveMover` (`0x6006e0`, sending `CMSG_SET_ACTIVE_MOVER` at `0x6007ae`), then
/// the world-enter cascade (`0x5deb60 call 0x4908c0`). `SMSG_LOGIN_VERIFY_WORLD` and
/// `SMSG_NEW_WORLD` purge the streamed world (`session::worldport`), so a login, a reconnect and
/// every cross-map worldport create us afresh and tag us again; a same-map teleport creates
/// nothing. The server has seated the player by then, so nothing sent here is dropped.
pub(crate) fn enter_world_on_self_create(
    created: Query<&Guid, Added<SelfPlayer>>,
    net: Res<NetCommands>,
    mut cascades: MessageWriter<WorldEnterCascadeMessage>,
) {
    for guid in &created {
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "mvr",
                &format!("SET_ACTIVE_MOVER guid={:#x} (self create)", guid.0),
            );
        }
        let _ = net.0.send(ClientCommand::SetActiveMover { guid: guid.0 });
        cascades.write(WorldEnterCascadeMessage);
    }
}
