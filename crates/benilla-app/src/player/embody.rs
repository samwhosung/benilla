//! Which unit the client simulates, animates from input and streams. The reference keeps apart
//! the active player (`ds:0xb41414`, fixed under far sight and possession), the camera anchor
//! (`camera+0x88`) and the active mover (`ds:0xc4da98`, set only by `SetActiveMover` `0x6006e0`;
//! unresolved, the input applier `0x514640` skips its tick): [`SelfPlayer`] is the character,
//! [`Embodied`] the attached body and camera anchor, [`ActiveMover`] the body we may move.
//!
//! - Losing control removes only [`ActiveMover`]: `0x5fa600` zeroes the mover globals and keeps
//!   the anchor, so the camera follows the feared body, smoothed (`0x50d810`). The spline ride
//!   hangs off [`Embodied`], and vmangos drops all movement until the `CMSG_MOVE_SPLINE_DONE` it
//!   awaits after a player's spline launch (`MoveSplineInit.cpp:131`, `MovementHandler.cpp:295`),
//!   so detaching while feared would freeze us server-side.
//! - An unstreamed claimed mover puts [`Embodied`] on nobody: outbound `MSG_MOVE_*` carry no guid,
//!   so driving our own body would write our pose onto the creature.
//! - The reins change hands by guid, not entity: a worldport re-streams our body under a new one.

use bevy::prelude::*;

use super::follow::FollowState;
use super::state::Player;
use crate::creature_anim::MovementState;
use crate::net::{ActiveMover, Embodied, GuidIndex, RemoteMotion, SelfGuid, SelfPlayer};

/// Keeps [`Embodied`] on the attached body (the possessed unit, else our own, else nobody) and
/// [`ActiveMover`] on it while it is ours to move; runs before the controller.
pub(super) fn maintain_embodiment(
    mut commands: Commands,
    mut player: ResMut<Player>,
    mut follow: ResMut<FollowState>,
    guids: (Res<SelfGuid>, Res<GuidIndex>),
    self_body: Query<Entity, With<SelfPlayer>>,
    attached: Query<Entity, With<Embodied>>,
    steering: Query<Entity, With<ActiveMover>>,
    mut held: Local<Option<u64>>,
) {
    let (self_guid, index) = (&guids.0, &guids.1);
    // A claimed foreign mover is the body if streamed, else nobody; otherwise our own body. Lost
    // control is not a third answer: only [`ActiveMover`] comes off, below.
    let want_guid = player.foreign_mover.or(self_guid.0);
    let want = match player.foreign_mover {
        Some(guid) => index.0.get(&guid).copied(),
        None => self_body.iter().next(),
    };

    if *held != want_guid {
        *held = want_guid;
        if want_guid.is_some() {
            // `Player` holds the old body's pose; the controller adopts the new one's first.
            player.reseat = true;
            // `SetActiveMover` `0x6006e0` cancels click-to-move and follow on the outgoing mover,
            // through `0x6103a0` → `0x60fb60(0, 1)`.
            follow.stop();
        }
    }

    // Steering has its own edges: a fear lands and lifts without a handover.
    let steer = want.filter(|_| !player.control_lost);
    for e in &steering {
        if Some(e) != steer {
            // `MovementState` goes too: it is `unify`'s top-precedence leg, so a stale one would
            // hold the body's animation while the server moves it.
            commands.entity(e).remove::<(ActiveMover, MovementState)>();
        }
    }
    if let Some(e) = steer {
        if !steering.contains(e) {
            commands
                .entity(e)
                .insert((ActiveMover, MovementState::default()));
        }
    }

    if attached.iter().next() == want {
        return;
    }
    for e in &attached {
        commands.entity(e).remove::<Embodied>();
    }
    if let Some(e) = want {
        // Drop the unit's queued server replay, or it fires after we let go. No relays arrive
        // while we drive it: vmangos excludes the mover's session (`MovementHandler.cpp:396`).
        commands.entity(e).insert(Embodied).remove::<RemoteMotion>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::RemoteMotion;

    fn harness() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Player>()
            .init_resource::<FollowState>()
            .init_resource::<SelfGuid>()
            .init_resource::<GuidIndex>()
            .add_systems(Update, maintain_embodiment);
        let me = app.world_mut().spawn(SelfPlayer).id();
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(0xAA);
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0xAA, me);
        (app, me)
    }

    fn mover(app: &mut App) -> Option<Entity> {
        let mut q = app.world_mut().query_filtered::<Entity, With<Embodied>>();
        q.iter(app.world()).next()
    }

    fn steered(app: &mut App) -> Option<Entity> {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<ActiveMover>>();
        q.iter(app.world()).next()
    }

    /// A remote unit's replay state; only its presence is under test.
    fn replaying() -> RemoteMotion {
        RemoteMotion {
            wow_pos: [0.0; 3],
            orientation: 0.0,
            flags: 0,
            pitch: 0.0,
            speed: 0.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            fall_start_z: None,
            pending: std::collections::VecDeque::new(),
            relay: Default::default(),
            last_apply_ms: 0.0,
            last_apply_pos: [0.0; 3],
        }
    }

    #[test]
    fn a_claim_we_cannot_resolve_yet_moves_the_marker_to_nobody_not_to_our_own_body() {
        let (mut app, me) = harness();
        app.update();
        assert_eq!(mover(&mut app), Some(me), "no claim → our own body");

        // Claimed, but the creature has not streamed in.
        app.world_mut().resource_mut::<Player>().foreign_mover = Some(0xBB);
        app.update();
        assert_eq!(
            mover(&mut app),
            None,
            "an unresolvable claim is NOBODY — leaving it on our body drives us under the \
             creature's mover, and outbound moves carry no guid of their own"
        );
        assert!(
            app.world().resource::<Player>().reseat,
            "and the controller is told to drive nothing until it has a pose to adopt"
        );

        // It streams in.
        let boar = app.world_mut().spawn(replaying()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0xBB, boar);
        app.update();
        assert_eq!(mover(&mut app), Some(boar));
        assert!(
            app.world().entity(boar).contains::<MovementState>(),
            "a creature carries no controller-fed movement view of its own — without one it walks \
             with its idle animation playing"
        );
        assert!(
            !app.world().entity(boar).contains::<RemoteMotion>(),
            "and its queued server replay is dropped, or it snaps back the moment we let go"
        );

        // Released.
        app.world_mut().resource_mut::<Player>().foreign_mover = None;
        app.update();
        assert_eq!(mover(&mut app), Some(me), "the reins come home");
        assert!(
            !app.world().entity(boar).contains::<MovementState>(),
            "`unify` reads this leg FIRST, so one left behind freezes the creature's animation on \
             the last view we drove it with, forever"
        );
    }

    #[test]
    fn our_own_body_gets_its_movement_view_back_when_the_reins_come_home() {
        let (mut app, me) = harness();
        app.update();
        assert!(app.world().entity(me).contains::<MovementState>());

        app.world_mut().resource_mut::<Player>().foreign_mover = Some(0xBB);
        let boar = app.world_mut().spawn(replaying()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0xBB, boar);
        app.update();
        assert!(
            !app.world().entity(me).contains::<MovementState>(),
            "while we are driving the boar, nothing is writing our own body's view — a stale one \
             would shadow whatever the server is doing to it"
        );
        assert!(app.world().entity(boar).contains::<MovementState>());

        app.world_mut().resource_mut::<Player>().foreign_mover = None;
        app.update();
        assert!(
            app.world().entity(me).contains::<MovementState>(),
            "and it comes back with the reins — without this our own avatar is animation-dead for \
             the rest of the session, and only ever after a possession"
        );
        assert!(!app.world().entity(boar).contains::<MovementState>());
    }

    /// A cross-map worldport re-streams our body under a fresh entity while we keep driving it.
    #[test]
    fn re_streaming_our_own_body_moves_the_marker_without_calling_it_a_handover() {
        let (mut app, me) = harness();
        app.update();
        app.world_mut().resource_mut::<Player>().reseat = false;

        app.world_mut().entity_mut(me).despawn();
        let reborn = app.world_mut().spawn(SelfPlayer).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0xAA, reborn);
        app.update();

        assert_eq!(
            mover(&mut app),
            Some(reborn),
            "the marker follows the entity"
        );
        assert!(
            !app.world().resource::<Player>().reseat,
            "but the mover GUID never changed, so this is not a handover: re-seizing here would \
             discard the worldport's own snap and re-run the settle"
        );
    }
    #[test]
    fn a_body_we_may_not_move_stays_in_our_hands_and_only_stops_being_steered() {
        let (mut app, me) = harness();
        app.update();
        assert_eq!(mover(&mut app), Some(me));
        assert_eq!(steered(&mut app), Some(me));

        // Feared, or mind-controlled: the server named us with allowMove = 0.
        app.world_mut().resource_mut::<Player>().control_lost = true;
        app.update();
        assert_eq!(
            mover(&mut app),
            Some(me),
            "still our body — the camera, the collision height and the spline ride all hang off \
             this marker, and a client that drops it goes blind while feared"
        );
        assert_eq!(
            steered(&mut app),
            None,
            "but not ours to move: the replay lanes read THIS marker, so the body walks where the \
             server drives it instead of standing frozen"
        );

        // A possessed creature feared out of our control behaves the same way.
        app.world_mut().resource_mut::<Player>().foreign_mover = Some(0xBB);
        let boar = app.world_mut().spawn(replaying()).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0xBB, boar);
        app.update();
        assert_eq!(mover(&mut app), Some(boar), "held, but not ours to move");
        assert_eq!(steered(&mut app), None);

        // Control back.
        app.world_mut().resource_mut::<Player>().control_lost = false;
        app.update();
        assert_eq!(steered(&mut app), Some(boar));
    }
}
