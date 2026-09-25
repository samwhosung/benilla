//! Server-authored movement edges applied to our mover, the inbound mirror of
//! [`super::movement_net`]. [`apply_server_moves`], called by [`super::control`] before input,
//! takes control changes, worldports, near teleports, the granted modes (root, water walk, feather
//! fall, hover, into [`super::state::MoveModes`]), knockbacks, the take-control edge, pre-control
//! speed acks and the bare self-addressed move ([`apply_self_move`]): a `MSG_MOVE_*` naming our
//! own guid, which owes no ack and which the reference applies.

use benilla_protocol::MoveMode;
use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

use crate::creature_anim::{move_flags, wrap_pi};
use crate::net::{
    ClientCommand, ClientControlMessage, Embodied, Guid, KnockBackMessage, MoveModeMessage,
    NetCommands, SelfMoveMessage, SpeedChangeMessage, TeleportMessage, WorldportMessage,
};
use crate::transport::Transport;
use benilla_world::world_map::CurrentMap;

use super::camera::FlyCam;
use super::{movement_net, Player, SETTLE_TIMEOUT};

/// What one `SMSG_CLIENT_CONTROL_UPDATE` means for us. It says whether a unit may move, not who the
/// mover is: the server revokes a mind-controlled victim's control by naming the victim.
#[derive(Debug, PartialEq, Eq)]
enum ControlVerdict {
    /// Somebody else drives our body; we stop moving it.
    Revoked,
    /// Our own body is ours again.
    Restored,
    /// We were handed the reins of another unit, and owe it a mover claim.
    Granted(u64),
    /// Another unit may not move: the one we drive (kept, frozen) or one already given back.
    Released(u64),
}

/// The pitch a login opens at when no saved camera pose names one.
const LOGIN_PITCH: f32 = -0.45;

/// Give our own body back on a hand-over: the stop, then `CMSG_MOVE_NOT_ACTIVE_MOVER` (`0x2D1`).
/// That packet is only ever about our own character: the reference sends it from `SetActiveMover`
/// (`0x6006e0`) for the local player only (`0x5fa6d0`), and vmangos rejects a guid that is not its
/// confirmed mover, or another unit still being moved (`MovementHandler.cpp:891-908`).
///
/// Never on a fear, confuse or mind control, where our body stays the mover: the packet clears
/// `m_clientMoverGuid` (`MovementHandler.cpp:913`), so vmangos refuses the ack each later spline
/// arms (`MoveSplineInit.cpp:131-132`, `MovementHandler.cpp:804`) and drops our movement until a
/// map change (`MovementHandler.cpp:295-296`, `Map.cpp:448`). Skipped while a server spline owns
/// the body, as in the reference (`0x619d50`).
fn yield_own_body(net_cmds: &NetCommands, player: &mut Player, self_guid: Option<u64>) {
    movement_net::park_mover(&net_cmds.0, player);
    let Some(me) = self_guid else { return };
    if player.server_riding {
        return;
    }
    super::move_trace::mover_claim("NOT_ACTIVE_MOVER", me);
    let _ = net_cmds.0.send(ClientCommand::NotActiveMover {
        guid: me,
        flags: 0,
        pos: bevy_to_wow(player.pos),
        orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
        fall_time: 0,
    });
}

/// `self_guid` is `None` before login names us, when no update can be about our body.
fn control_verdict(mover: u64, allow_move: bool, self_guid: Option<u64>) -> ControlVerdict {
    match (self_guid == Some(mover), allow_move) {
        (true, false) => ControlVerdict::Revoked,
        (true, true) => ControlVerdict::Restored,
        (false, true) => ControlVerdict::Granted(mover),
        (false, false) => ControlVerdict::Released(mover),
    }
}

/// Apply this frame's server-authored movement and send each ack. Returns the forced-speed changes
/// for the movement stream to ack, except pre-control or detached, where they are acked here.
/// `self_pose` is the streamed self entity's translation and yaw, the take-control target.
// `control`'s own transports query, passed through.
#[allow(clippy::type_complexity)]
pub(super) fn apply_server_moves(
    time: &Time,
    commands: &mut Commands,
    player: &mut Player,
    cam: &mut FlyCam,
    net_cmds: &NetCommands,
    teleports: &mut MessageReader<TeleportMessage>,
    worldports: &mut MessageReader<WorldportMessage>,
    speed_msgs: &mut MessageReader<SpeedChangeMessage>,
    mode_msgs: &mut MessageReader<MoveModeMessage>,
    knock_msgs: &mut MessageReader<KnockBackMessage>,
    self_moves: &mut MessageReader<SelfMoveMessage>,
    control_msgs: &mut MessageReader<ClientControlMessage>,
    self_guid: Option<u64>,
    transports: &Query<
        (&Transform, &Guid, Option<&avian3d::prelude::ColliderAabb>),
        (With<Transport>, Without<Embodied>, Without<FlyCam>),
    >,
    self_pose: Option<(Vec3, f32)>,
) -> Vec<SpeedChangeMessage> {
    // A grant must be claimed: vmangos drops every `MSG_MOVE_*` for an unconfirmed mover
    // (`MovementHandler.cpp:291-293`, `Player.cpp:20257-20272`).
    for c in control_msgs.read() {
        match control_verdict(c.mover, c.allow_move, self_guid) {
            // Park, which stops the server extrapolating a run, and say nothing else: the mover has
            // not changed hands, and `CMSG_MOVE_NOT_ACTIVE_MOVER` would strand us.
            ControlVerdict::Revoked => {
                player.control_lost = true;
                movement_net::park_mover(&net_cmds.0, player);
            }
            ControlVerdict::Restored => {
                player.control_lost = false;
                player.foreign_mover = None;
                player.reseat = true;
                // Re-claim ourselves, as login does (after a possession `m_clientMoverGuid` still
                // names the creature), and send nothing else: `GetConfirmedMover` resolves the
                // mismatch to us (`Player.cpp:20267-20269`), so a parting stop for the creature
                // would move our own character to it.
                super::move_trace::mover_claim("SET_ACTIVE_MOVER", c.mover);
                let _ = net_cmds
                    .0
                    .send(ClientCommand::SetActiveMover { guid: c.mover });
            }
            // Claim it; recording the claim stops our body's pose streaming under its guid.
            ControlVerdict::Granted(guid) => {
                // Yield our own body first, on the same channel, while it is still the confirmed
                // mover, and only once: vmangos re-sends a grant each time a possessed creature
                // stops fleeing (`FearMovementGenerator.cpp:155`, `Unit.cpp:11157`) and rejects a
                // second yield (`MovementHandler.cpp:891-897`).
                if player.foreign_mover.is_none() {
                    yield_own_body(net_cmds, player, self_guid);
                }
                player.control_lost = false;
                player.foreign_mover = Some(guid);
                player.reseat = true;
                super::move_trace::mover_claim("SET_ACTIVE_MOVER", guid);
                let _ = net_cmds.0.send(ClientCommand::SetActiveMover { guid });
            }
            // vmangos sends this on unpossess, after `Restored`, and when the possessed unit is
            // feared or confused (`Unit.cpp:11157`). As in the reference (`0x5fa600`), only the
            // unit we drive matters: we keep it but may not move it. Clearing `foreign_mover`
            // would walk our own body while the server applies our steps to the creature.
            ControlVerdict::Released(guid) => {
                if player.foreign_mover == Some(guid) {
                    player.control_lost = true;
                }
            }
        }
    }
    // Cross-map worldport: snap, bump `CurrentMap` for the terrain streamer, ack if required.
    for w in worldports.read() {
        let riding = w.transport_entry.is_some() && player.ride.is_some();
        if riding {
            // Riding through: the pose is deck-local (`Player.cpp:2114`) and the boat survived the
            // purge. No settle hold: it would drop `MOVEFLAG_ONTRANSPORT` from the next move
            // packet, which the server reads as a deboard mid-ocean.
            let ride = player.ride.as_mut().expect("riding checked above");
            ride.local_pos = wow_to_bevy(w.position);
            if let Ok((boat, _, _)) = transports.get(ride.entity) {
                let boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
                ride.boat_yaw = boat_yaw;
                player.pos = boat.translation + boat.rotation * ride.local_pos;
                let dyaw = wrap_pi(w.orientation + boat_yaw - player.face_yaw);
                player.face_yaw += dyaw;
                player.model_yaw = wrap_pi(player.model_yaw + dyaw);
                cam.yaw += dyaw;
            } else {
                warn!("worldport: riding but the boat entity is missing — using raw pose");
                player.pos = wow_to_bevy(w.position);
                player.ride = None;
            }
        } else {
            // No transport carried through: a world pose, and any ride is stale.
            player.ride = None;
            player.pos = wow_to_bevy(w.position);
            player.face_yaw = w.orientation;
            player.model_yaw = w.orientation; // the body snaps too, no chase
            cam.yaw = w.orientation;
            player.settling = true; // hold (gravity off) until the new map's ground streams in
            player.settle_since = time.elapsed_secs();
            player.settle_deadline = time.elapsed_secs() + SETTLE_TIMEOUT;
            // The physics world is still the old map's until the streamer swaps it.
            player.world_stale = true;
        }
        player.move_flags = 0;
        player.airborne_since = None; // a snap ends any jump arc (no phantom FALL_LAND)
        commands.insert_resource(CurrentMap(w.map_id));
        if w.needs_ack {
            if riding {
                // A riding crossing never settles, so it acks now.
                let _ = net_cmds.0.send(ClientCommand::WorldportAck);
                info!(
                    "worldport: mapId {} @ {:?} (riding, boat-local pose, acked)",
                    w.map_id, w.position
                );
            } else {
                // The ack waits for the settle release: the reference acks after its blocking
                // load, and vmangos keeps us out of world until the ack, with no timeout.
                player.owes_worldport_ack = true;
                info!(
                    "worldport: mapId {} @ {:?} (world pose, ack deferred to release)",
                    w.map_id, w.position
                );
            }
        } else {
            info!(
                "worldport: initial login on mapId {} @ {:?}",
                w.map_id, w.position
            );
        }
    }
    // Same-map teleport: snap and echo the ack, without which the server freezes our movement.
    for t in teleports.read() {
        snap_near_teleport(player, &mut cam.yaw, t, time.elapsed_secs());
        // The echo is the whole handshake: the reference echoes on its next movement tick
        // (`0x60e0a0`) and sends nothing else, and vmangos relocates us only when it lands
        // (`ExecuteTeleportNear`, `MovementHandler.cpp:237`).
        let _ = net_cmds.0.send(ClientCommand::TeleportAck {
            guid: t.guid,
            counter: t.counter,
        });
        info!("teleport: snapped to {:?}, acked", t.position);
    }
    // Bare self-addressed moves, after the teleports so a same-frame teleport wins the pose.
    for m in self_moves.read() {
        if player.active {
            apply_self_move(m, player, cam, time, transports);
        }
    }

    // The acked modes: apply to our state first, then ack with the flags it rebuilds to, as the
    // reference does. vmangos kicks a root apply-ack without `MOVEFLAG_ROOT`, or defers the root to
    // landing when the ack carries `JUMPING` or `FALLINGFAR` (`MovementHandler.cpp:715-729`).
    //
    // Root also parks the walk stream and ends the arc, as the reference's `SetRoot` (`0x7c7340`)
    // clears the direction bits and stops the fall (`0x7c6290`); moving bits beside ROOT freeze
    // the reference and raise vmangos's `CHEAT_TYPE_ROOT_MOVE`. Turn bits are not moving bits.
    for m in mode_msgs.read() {
        player.modes.set(m.mode, m.apply);
        if m.mode == MoveMode::Root && m.apply {
            movement_net::park_mover(&net_cmds.0, player);
            player.airborne_since = None;
            player.vel_y = 0.0;
            player.fall_far = false;
        }
        // A hover grant jumps you (`Player::hover_launch`): the reference's handler (`0x61a620`)
        // runs `CMovement::Jump` (`0x7c6230`) on enable and `StartFalling` (`0x7c61c0`) on disable
        // before it sets the flag (`0x7c7310`), so Levitate cast while swimming leaves the water.
        // Disable needs nothing: with `hover_offset` at zero the body falls from rest by itself.
        else if m.mode == MoveMode::Hover && m.apply {
            player.hover_launch = true;
        }
        let facing = player.face_yaw.rem_euclid(std::f32::consts::TAU);
        let _ = net_cmds.0.send(ClientCommand::MoveModeAck {
            guid: m.guid,
            counter: m.counter,
            mode: m.mode,
            apply: m.apply,
            flags: player.modes.wire_flags(),
            pos: bevy_to_wow(player.pos),
            orientation: facing,
        });
        info!(
            "mover mode {:?} {} (acked)",
            m.mode,
            if m.apply { "granted" } else { "revoked" }
        );
    }

    // A knockback is only latched here; the mover launches it and acks. The reference enqueues it
    // (`0x617a30`), and its frame update (`0x616620`) applies it (`0x6179c0`, called at `0x61624d`)
    // and then acks (`0x616261`), so the wire carries the post-launch `MovementInfo`.
    //
    // Two in one frame collapse to the last, where the reference queues both: the motion is the
    // same, as `0x6179c0` replaces the velocity, but the first ack is lost (`OnFailedToAckChange`).
    for k in knock_msgs.read() {
        player.knockback = Some(super::state::PendingKnockback {
            guid: k.guid,
            counter: k.counter,
            launch: k.launch,
        });
    }

    // Take control when the server first reports our position, and again on a mover change; only
    // the login (`first`) settles, flags a stale world and seats the camera.
    let seizing = !player.active || player.reseat;
    if seizing {
        if let Some((pos, yaw)) = self_pose {
            let first = !player.active;
            player.reseat = false;
            // A mover change tears down the outgoing mover's state, as the reference's
            // `SetActiveMover` (`0x6006e0`) resets its flags and click-to-move. A login needs none:
            // `release_on_session_end` already reset the whole resource.
            if !first {
                player.vel_y = 0.0;
                player.horiz_vel = Vec3::ZERO;
                player.move_flags = 0;
                player.autorun = false;
                player.airborne_since = None;
                player.fall_far = false;
                player.fall_start_y = pos.y;
                player.ride = None;
                // The new mover's modes arrive in their own packets.
                player.modes = Default::default();
            }
            player.pos = pos;
            player.active = true;
            if first {
                player.settling = true; // settle once the initial ground loads
                player.settle_since = time.elapsed_secs();
                player.settle_deadline = time.elapsed_secs() + SETTLE_TIMEOUT;
                player.world_stale = true;
                // The spawn pose carries the server's facing (the logout orientation): adopt it,
                // camera behind, as the reference does.
                cam.yaw = yaw;
                cam.pitch = player.login_pitch.unwrap_or(LOGIN_PITCH);
            }
            // No camera re-seat on a mover change: Mind Control's far sight already moved it.
            player.face_yaw = yaw;
            player.model_yaw = yaw;
            // The first frame owes no `SET_FACING`: the reference sends none until a turn.
            player.last_facing = yaw.rem_euclid(std::f32::consts::TAU);
            // `MovementState` comes with the `SelfPlayer` tag, not here: a worldport re-streams
            // the entity while `player.active` stays true.
            info!(
                "took control of {} @ {:?} facing {:.3}{}",
                match player.foreign_mover {
                    Some(guid) => format!("possessed unit {guid:#x}"),
                    None => "player".to_string(),
                },
                player.pos,
                yaw.rem_euclid(std::f32::consts::TAU),
                crate::run_mode::free_fly_hint()
            );
        }
    }

    // Forced speed changes: the bridge applied the speed; the ack carries our wire state, which the
    // server relocates us to. Controlled, the movement stream acks; otherwise our state is the
    // parked pose, so ack here.
    let speed_acks: Vec<SpeedChangeMessage> = speed_msgs.read().copied().collect();
    if !player.active || player.detached {
        for ack in &speed_acks {
            let _ = net_cmds.0.send(ClientCommand::ForceSpeedAck {
                kind: ack.kind,
                guid: ack.guid,
                counter: ack.counter,
                speed: ack.speed,
                flags: 0,
                pos: bevy_to_wow(player.pos),
                orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
                pitch: 0.0,
                fall_time: 0,
                jump: None,
                transport: None, // flags 0 → no transport tail
            });
        }
    }
    speed_acks
}

/// Apply one same-map teleport's snap (`MSG_MOVE_TELEPORT_ACK` inbound); the caller echoes it.
fn snap_near_teleport(player: &mut Player, cam_yaw: &mut f32, t: &TeleportMessage, now: f32) {
    // A near teleport arrives detached: vmangos takes this path only off a transport
    // (`Player.cpp:1866-1893`), and the reference's apply (`0x6186b0`) re-parents to the packet's
    // transport guid, zero here. A kept ride would carry us back onto the deck.
    player.ride = None;
    player.pos = wow_to_bevy(t.position);
    player.face_yaw = t.orientation;
    *cam_yaw = t.orientation;
    player.move_flags = 0;
    player.airborne_since = None; // a snap ends any jump arc (no phantom FALL_LAND)
    player.settling = true; // hold (gravity off) until the destination loads
    player.settle_since = now;
    player.settle_deadline = now + SETTLE_TIMEOUT;
    // The destination's ground may not have streamed yet.
    player.world_stale = true;
    // Voids any self ride: `drive_self_ride` drops it rather than mirror the stale pose over this.
    player.ride_abort = true;
}

/// The reference's masked merge of a server-authored `MOVEMENTFLAGS` (`0x618c30` at `0x618deb`:
/// `old ^ ((old ^ wire) & 0x75a07dff)`). `ON_TRANSPORT` is outside the mask, so a server pose never
/// boards or deboards us.
pub(super) fn merge_server_flags(local: u32, wire: u32) -> u32 {
    move_flags::merge_server_authored(local, wire)
}

/// Apply one bare self-addressed `MSG_MOVE_*`, a pose the server wrote for our mover (`.go`,
/// `.cheat fly`, an anticheat snap-back): a hard snap with no ack. The reference writes the pose
/// into its live cell and its integrator base (`0x7c6420`), one cell for us ([`Player::pos`]), and
/// our next heartbeat carries it home.
///
/// Deviation: applied at arrival, because our own avatar has no motion to de-jitter; the reference
/// routes it through the replay chain remotes use, which paces to the sender's cadence and can hold
/// an early packet (up to 1000 ms in `net::motion`). A self-addressed stream, such as bursting
/// anticheat snap-backs, would need that pacing.
// `control`'s own transports query, passed through.
#[allow(clippy::type_complexity)]
fn apply_self_move(
    m: &SelfMoveMessage,
    player: &mut Player,
    cam: &mut FlyCam,
    time: &Time,
    transports: &Query<
        (&Transform, &Guid, Option<&avian3d::prelude::ColliderAabb>),
        (With<Transport>, Without<Embodied>, Without<FlyCam>),
    >,
) {
    let was_falling = player.move_flags & move_flags::FALLING != 0;
    player.move_flags = merge_server_flags(player.move_flags, m.flags);
    let now_falling = player.move_flags & move_flags::FALLING != 0;

    // Lift the merged modes into typed state, as `move_flags` is rebuilt every frame; the
    // reference's merge applies swimming too (`0x61a1af` → `0x61a230`). `.cheat fly` sends
    // SWIMMING with LEVITATING.
    player.swimming = player.move_flags & move_flags::SWIMMING != 0;
    // The walk bit is lifted too, normally our own echoed back; else a server move would clear it
    // under a set latch, and the next frame would re-announce the gait.
    player.walking = player.move_flags & move_flags::WALK_MODE != 0;
    player.modes.merge_from_wire(player.move_flags);
    // Neither mode touches `settling`: the terrain streamer releases it in every mode alike.

    player.pos = wow_to_bevy(m.position);
    // Aim, body and camera turn by the same delta; the reference writes only the mover's facing,
    // and a hard camera set would yank the view on every `.cheat fly` toggle.
    let dyaw = wrap_pi(m.orientation - player.face_yaw);
    player.face_yaw = wrap_pi(player.face_yaw + dyaw);
    player.model_yaw = wrap_pi(player.model_yaw + dyaw);
    cam.yaw += dyaw;
    player.mover_pitch = m.pitch;

    // On a deck the packet moved us within the platform frame (`ON_TRANSPORT` is outside the mask):
    // re-anchor the local pose, or the next carry undoes the snap. Prefer the wire's deck-local
    // pose: at a cross-map seam our boat is still frozen on the source continent while `position`
    // is on the destination, so the subtraction would fling us thousands of yards.
    if let Some(ride) = player.ride.as_mut() {
        if let Ok((boat, _, _)) = transports.get(ride.entity) {
            match m.transport.filter(|t| t.guid == ride.guid) {
                Some(t) => {
                    ride.local_pos = wow_to_bevy([t.pos.x, t.pos.y, t.pos.z]);
                    ride.boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
                    player.pos = boat.translation + boat.rotation * ride.local_pos;
                }
                None => {
                    ride.local_pos = boat.rotation.inverse() * (player.pos - boat.translation);
                    ride.boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
                }
            }
        }
    }

    // The arc follows the merged `FALLING` bit: seeded from the wire when set (`0x7c6490`), ended
    // when it just cleared.
    match (was_falling, now_falling) {
        (_, true) => {
            let (vel_y, xy) = crate::net::jump_seed(m.jump, m.fall_time, player.modes.feather_fall);
            let t = m.fall_time as f32 / 1000.0;
            player.airborne_since = Some(time.elapsed_secs() - t);
            player.jump_zspeed = m.jump.map_or(0.0, |j| -j.zspeed);
            player.vel_y = vel_y;
            player.horiz_vel = wow_to_bevy([xy[0], xy[1], 0.0]);
            // Where this arc launched, recovered from the pose it is at now: `z = z₀ + v₀t − ½gt²`.
            player.fall_start_y =
                player.pos.y - (player.jump_zspeed * t - 0.5 * crate::player::GRAVITY * t * t);
            player.fall_far = player.move_flags & move_flags::FALLING_FAR != 0;
            player.airborne_dirs = player.move_flags & move_flags::ANY_MOVE;
        }
        (true, false) => {
            // Ended by the server (`Unit.cpp:10073` strips `JUMPING|FALLINGFAR`), silently: the
            // packet moved us, so there is no fall height to report.
            player.airborne_since = None;
            player.fall_far = false;
            player.vel_y = 0.0;
        }
        (false, false) => {}
    }
    player.wedged = false;
    player.wedge_still = 0;
}

/// On a `/logout` or a lost session, reset the whole resource to its boot state, so the next login
/// owes nothing to the ended one. Whole, not by field: `/logout` roots us for the countdown
/// (`MiscHandler.cpp:329`) and the next session never unroots, so any kept mover state persists.
/// The reference also keeps no mover state, shutting the game down on logout (`0x491180`).
pub(super) fn release_on_session_end(
    mut logouts: MessageReader<crate::net::LoggedOutMessage>,
    mut lost: MessageReader<crate::net::DisconnectedMessage>,
    mut player: ResMut<Player>,
) {
    // `|`, not `||`: both readers must drain, or the unread one carries into the next frame.
    if logouts.read().next().is_some() | lost.read().any(|m| m.session_over) {
        // vmangos re-sends the modes the new session holds (`Player.cpp:19297-19311`).
        *player = Player::default();
    }
}

#[cfg(test)]
mod session_end_tests {
    use super::*;
    use crate::net::{DisconnectedMessage, LoggedOutMessage};
    use benilla_protocol::SessionEnd;

    fn harness() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Player>()
            .add_message::<LoggedOutMessage>()
            .add_message::<DisconnectedMessage>()
            .add_systems(Update, release_on_session_end);
        app
    }

    /// A session that ran: in world, moving, every mode granted, the reins in other hands.
    fn a_session_that_ran(app: &mut App) {
        let mut p = app.world_mut().resource_mut::<Player>();
        p.active = true;
        p.modes = super::super::state::MoveModes {
            rooted: true,
            water_walking: true,
            feather_fall: true,
            hover: true,
            levitating: true,
        };
        p.autorun = true;
        p.control_lost = true;
        p.foreign_mover = Some(0xBB);
        p.server_riding = true;
        p.swimming = true;
        p.move_flags = crate::creature_anim::move_flags::FORWARD;
        p.pos = Vec3::new(10.0, 20.0, 30.0);
        p.owes_worldport_ack = true;
        p.airborne_since = Some(4.0);
        p.wedged = true;
    }

    /// `/logout` roots us (`MiscHandler.cpp:329`) and the next login is never unrooted:
    /// `SendInitialPacketsAfterAddToMap` re-roots only for a stun (`Player.cpp:19297-19311`).
    /// Checked against `Player::default()` whole, so a new field is covered.
    #[test]
    fn a_logout_takes_every_grant_the_ended_session_made() {
        let mut app = harness();
        a_session_that_ran(&mut app);
        app.world_mut().write_message(LoggedOutMessage);
        app.update();

        let p = app.world().resource::<Player>();
        assert!(
            p.modes == Default::default(),
            "the granted modes belonged to the mover that just ended — a root that survives \
             `/logout` is B306: the character re-enters the world and WASD is dead"
        );
        assert!(
            *p == Player::default(),
            "and nothing else survives either: the session boundary returns the resource to the \
             state `player::setup` inserts at boot (1542)"
        );
    }

    /// A lost session is a different message on a different reader; the `|` draining both matters.
    #[test]
    fn a_lost_session_takes_them_too() {
        let mut app = harness();
        a_session_that_ran(&mut app);
        app.world_mut().write_message(DisconnectedMessage {
            reason: "world stream closed".into(),
            end: SessionEnd::Lost,
            session_over: true,
        });
        app.update();

        assert!(*app.world().resource::<Player>() == Player::default());
    }

    /// `session_over: false` (the logout's own teardown, a seamless reconnect) keeps the body ours.
    #[test]
    fn a_teardown_that_is_not_the_end_keeps_the_avatar() {
        let mut app = harness();
        a_session_that_ran(&mut app);
        app.world_mut().write_message(DisconnectedMessage {
            reason: "logged out".into(),
            end: SessionEnd::LoggedOut,
            session_over: false,
        });
        app.update();

        let p = app.world().resource::<Player>();
        assert!(
            p.active,
            "the session is not over — the avatar is still ours"
        );
        assert!(p.modes.rooted, "and so is everything granted to its mover");
    }
}

#[cfg(test)]
mod self_move_tests {
    use super::merge_server_flags;
    use crate::creature_anim::move_flags as f;

    /// `ON_TRANSPORT` is outside the reference's `0x75a07dff` mask; `FALLING` is inside, which is
    /// how `.go forward` ends an arc.
    #[test]
    fn a_server_move_owns_the_wire_bits_and_leaves_the_transport_bit_alone() {
        // Riding a boat, running forward. The server says: standing still, falling, not on a boat.
        let local = f::ON_TRANSPORT | f::FORWARD;
        let wire = f::FALLING;
        let merged = merge_server_flags(local, wire);
        assert_eq!(
            merged & f::ON_TRANSPORT,
            f::ON_TRANSPORT,
            "the packet must not deboard a rider — bit 25 is the client's"
        );
        assert_eq!(merged & f::FALLING, f::FALLING, "the wire owns FALLING");
        assert_eq!(merged & f::FORWARD, 0, "and it owns the direction bits too");

        assert_eq!(merge_server_flags(0, f::ON_TRANSPORT), 0);
    }

    /// `Player::SetFly` sends `LEVITATING | SWIMMING | MOVED | FLYING` (`Player.cpp:4595`).
    #[test]
    fn the_fly_toggle_arrives_whole() {
        const SET_FLY: u32 = f::LEVITATING | f::SWIMMING | 0x0080_0000 | 0x0100_0000;
        let merged = merge_server_flags(f::FORWARD, SET_FLY);
        assert_eq!(merged & f::LEVITATING, f::LEVITATING);
        assert_eq!(merged & f::SWIMMING, f::SWIMMING);
        // `.cheat fly off` sends flags 0.
        assert_eq!(
            merge_server_flags(merged, 0) & (f::LEVITATING | f::SWIMMING),
            0
        );
    }

    #[test]
    fn every_modelled_flag_but_the_transport_bit_is_server_authored() {
        for (name, bit) in [
            ("LEVITATING", f::LEVITATING),
            ("FORWARD", f::FORWARD),
            ("BACKWARD", f::BACKWARD),
            ("STRAFE_LEFT", f::STRAFE_LEFT),
            ("STRAFE_RIGHT", f::STRAFE_RIGHT),
            ("TURN_LEFT", f::TURN_LEFT),
            ("TURN_RIGHT", f::TURN_RIGHT),
            ("WALK_MODE", f::WALK_MODE),
            ("ROOT", f::ROOT),
            ("FALLING", f::FALLING),
            ("FALLING_FAR", f::FALLING_FAR),
            ("SWIMMING", f::SWIMMING),
            ("WATER_WALKING", f::WATER_WALKING),
        ] {
            assert_eq!(
                bit & f::SERVER_AUTHORED,
                bit,
                "{name} must be inside the mask"
            );
        }
        assert_eq!(f::ON_TRANSPORT & f::SERVER_AUTHORED, 0);
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    const ME: u64 = 0x0000_0000_0000_0045;
    const VICTIM: u64 = 0x0000_0000_0000_0099;
    const CREATURE: u64 = 0xF130_000C_1A00_A2B4;

    #[test]
    fn naming_us_is_never_a_grant_and_naming_another_is_never_about_our_body() {
        assert_eq!(
            control_verdict(ME, false, Some(ME)),
            ControlVerdict::Revoked,
            "the server revokes by naming US — this is the mind-controlled victim's packet"
        );
        assert_eq!(
            control_verdict(ME, true, Some(ME)),
            ControlVerdict::Restored
        );
        assert_eq!(
            control_verdict(CREATURE, true, Some(ME)),
            ControlVerdict::Granted(CREATURE)
        );
        assert_eq!(
            control_verdict(CREATURE, false, Some(ME)),
            ControlVerdict::Released(CREATURE)
        );
    }

    /// vmangos's Mind Control sequences; the caster's end restores before it releases.
    #[test]
    fn the_mind_control_sequences_classify_in_order() {
        // Caster, possession start: one grant naming the victim.
        assert_eq!(
            control_verdict(VICTIM, true, Some(ME)),
            ControlVerdict::Granted(VICTIM)
        );
        // Caster, possession end: `(self, 1)` then `(victim, 0)` (`SpellAuras.cpp:3023-3024`).
        let caster_end = [
            control_verdict(ME, true, Some(ME)),
            control_verdict(VICTIM, false, Some(ME)),
        ];
        assert_eq!(
            caster_end,
            [ControlVerdict::Restored, ControlVerdict::Released(VICTIM)],
            "restore lands BEFORE the release; last-packet-wins would strand the caster"
        );

        // The victim's own client: revoked at the start, restored at the end.
        assert_eq!(
            control_verdict(VICTIM, false, Some(VICTIM)),
            ControlVerdict::Revoked
        );
        assert_eq!(
            control_verdict(VICTIM, true, Some(VICTIM)),
            ControlVerdict::Restored
        );
    }

    #[test]
    fn an_unknown_self_guid_never_revokes_our_own_body() {
        assert_eq!(
            control_verdict(ME, false, None),
            ControlVerdict::Released(ME),
            "with no self guid this is somebody else's unit, not our body being frozen"
        );
        assert_eq!(control_verdict(ME, true, None), ControlVerdict::Granted(ME));
    }
}

#[cfg(test)]
mod teleport_tests {
    use super::*;

    /// Hearthing off a zeppelin or a lift is a same-map teleport.
    #[test]
    fn a_near_teleport_leaves_the_transport() {
        let mut player = Player {
            active: true,
            ride: Some(super::super::state::PlayerRide {
                entity: Entity::PLACEHOLDER,
                guid: 0x1F,
                local_pos: Vec3::new(1.0, 0.0, 2.0),
                boat_yaw: 0.5,
            }),
            ..Default::default()
        };
        let mut cam_yaw = 0.0;
        let t = TeleportMessage {
            guid: 0x45,
            counter: 1,
            position: [-8_913.0, -136.0, 81.0],
            orientation: 1.25,
        };
        snap_near_teleport(&mut player, &mut cam_yaw, &t, 10.0);
        assert!(player.ride.is_none(), "the snap deboards");
        assert_eq!(player.pos, wow_to_bevy(t.position));
        assert_eq!((player.face_yaw, cam_yaw), (1.25, 1.25));
        assert!(player.settling && player.ride_abort && player.world_stale);
    }
}
