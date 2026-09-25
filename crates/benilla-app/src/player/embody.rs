//! **What the client embodies** — which single unit it simulates, animates from input, and streams.
//!
//! Decision 0092 gave this client two answers to "where am I?": the **camera eye** and the
//! **active-player character**. Possession forces a third, and the reference has had it all along
//! as its own global: the **active mover** (`ds:0xc4da98`, written only by `SetActiveMover
//! 0x6006e0`), which the input applier `0x514640` resolves at the top of every tick and *skips the
//! whole tick* when it does not resolve. The three are genuinely independent stores there — the
//! camera never consults the mover to pick its anchor, and neither of them touches "the active
//! player" (`ds:0xb41414`), which is invariant under both far sight and possession.
//!
//! So benilla splits the *markers*, not the identity. [`SelfPlayer`] keeps meaning **my character**
//! — bags, auras, quest log, paper doll, the name over the head — and never moves. [`Embodied`]
//! means **the body I am attached to** (the reference's camera anchor, `camera+0x88`) and
//! [`ActiveMover`] means **the body I may move** (those mover globals). Of ~150 `SelfPlayer` query
//! sites the large majority are identity and are untouched by possession; the ones that changed are
//! exactly those that simulate, animate from input, render the body the camera is inside, or
//! stream it.
//!
//! Three properties of the placement carry the weight:
//!
//! - **Attached is not allowed to move**. A control update that forbids a body does
//!   not detach us from it: [`Embodied`] stays and only [`ActiveMover`] comes off. That is the
//!   reference's own shape — `0x5fa600` zeroes the mover globals and never touches the camera
//!   anchor, so the camera goes on following your feared body, merely smoothed (`0x50d810`).
//!   Collapsing the two cost more than
//!   the camera: the self-spline ride hangs off attachment, and with it the
//!   `CMSG_MOVE_SPLINE_DONE` vmangos arms a wait for at every spline launch for a
//!   player-or-player-possessed unit (`MoveSplineInit::Launch`), dropping **every** movement packet
//!   until it arrives (`HandleMovementOpcodes`). A client that detaches while feared is frozen
//!   server-side long after the fear ends.
//! - **Unresolvable is nobody, never a fallback.** A claimed grant whose object has not streamed in
//!   leaves the marker unplaced, because the alternative — quietly leaving it on our own body —
//!   drives our body under the creature's mover, and outbound `MSG_MOVE_*` carry no guid: the
//!   server writes our pose onto the creature. That is the sharpest trap in this family (decision
//!   1269 §3), and here it is structurally impossible rather than guarded against.
//! - **The reins change hands by GUID, not by entity.** A cross-map worldport despawns and
//!   re-streams our own body under a fresh entity while we never stop driving it, so keying the
//!   handover on the entity would re-seize (and re-settle) on every zone transfer.

use bevy::prelude::*;

use super::follow::FollowState;
use super::state::Player;
use crate::creature_anim::MovementState;
use crate::net::{ActiveMover, Embodied, GuidIndex, RemoteMotion, SelfGuid, SelfPlayer};

/// Keep [`Embodied`] on the one body we are attached to — the possessed unit while we hold its
/// reins, our own otherwise, and nobody at all while neither is streamed — and [`ActiveMover`] on
/// it for as long as that body is also ours to move.
///
/// Runs before the controller, which reads both to decide what — if anything — it is driving.
// A Bevy system's parameter list, not an argument list.
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
    // Which body we are attached to, and which entity that is. Two answers:
    //
    // - **A claimed foreign mover** answers by itself: either its object is streamed and it is the
    //   body we inhabit, or nothing is.
    // - Otherwise our own body.
    //
    // Being *forbidden to move it* is deliberately not a third answer. It is the
    // narrow [`ActiveMover`] marker below that comes off, and `control`'s own gate that stops
    // driving; detaching outright was decision 1279's mistake, and it took the camera, the
    // collision height and — the expensive one — the self-spline ride and its mandatory
    // `CMSG_MOVE_SPLINE_DONE` with it.
    let want_guid = player.foreign_mover.or(self_guid.0);
    let want = match player.foreign_mover {
        Some(guid) => index.0.get(&guid).copied(),
        None => self_body.iter().next(),
    };

    if *held != want_guid {
        *held = want_guid;
        if want_guid.is_some() {
            // Whatever pose and momentum `Player` holds describe the body we just let go of, so the
            // controller has to adopt the new one's before it drives anything. It also drives
            // nothing at all until it has: see [`Player::reseat`].
            player.reseat = true;
            // The reference tears the same thing down on the outgoing mover — `SetActiveMover
            // 0x6006e0` calls `0x6103a0` → `0x60fb60(0, 1)`, cancelling click-to-move and follow.
            // A follow that survived would steer the creature toward whoever our *character* was
            // walking behind.
            follow.stop();
        }
    }

    // The narrow half: whether that body is ours to move *this frame*. It changes on its own edges —
    // a fear lands and lifts without the reins ever changing hands — so it is maintained beside the
    // hand-over below, not inside it.
    let steer = want.filter(|_| !player.control_lost);
    for e in &steering {
        if Some(e) != steer {
            // [`MovementState`] rides with it, because it is `unify`'s **top-precedence** leg: a
            // body that keeps one while nothing is writing it animates from the last view we drove
            // it with, whatever the server is actually doing to it. Dropping it hands the animation
            // to the same source that now owns the motion — the relayed stream, or the spline the
            // server is flee-pathing it along.
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
        // Drop whatever server-replay state the unit carried at the instant we took it. While we
        // drive it the server sends us no relays for it (vmangos excludes the mover's own session
        // from the broadcast), so nothing accumulates — but a move already queued for a future
        // apply would otherwise fire after we let go and yank the unit back to a pose it left.
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

    /// A remote unit's replay state, minimally populated — only its presence is under test.
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

    /// The whole handover, and the two states a casual version gets wrong: a claim we cannot yet
    /// resolve must leave the marker on **nobody**, and coming home must hand the creature back to
    /// the remote path intact.
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

    /// The controller-fed [`MovementState`] follows the reins — including all the way home. It is
    /// `unify`'s top-precedence leg, so whoever holds it decides what the body looks like it is
    /// doing; leaving one on a body we have let go of freezes its animation on the last view we
    /// drove it with, and failing to give it back leaves our own avatar animation-dead for the rest
    /// of the session, and only ever after a possession.
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

    /// A cross-map worldport despawns and re-streams our own body under a fresh entity while we
    /// never stop driving it. Keying the handover on the entity would re-seize — and re-settle —
    /// on every zone transfer.
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
    /// **Forbidden to move it is not letting go of it** — the split the two markers
    /// exist for. Being feared hands the body's *motion* to the server, which is visible and must
    /// be: a mind-controlled player is seen walking where their captor drives them. But it does not
    /// hand back the body, and letting go was expensive in a way nothing on screen showed — the
    /// self-spline ride goes with it, and with the ride the `CMSG_MOVE_SPLINE_DONE` the server
    /// blocks every later movement packet on.
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

        // A possessed creature that has just been feared out of our control behaves the same way.
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
