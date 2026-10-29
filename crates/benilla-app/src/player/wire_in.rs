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
    ClientCommand, ClientControlMessage, KnockBackMessage, MoveModeMessage, NetCommands,
    SelfMoveMessage, SpeedChangeMessage, TeleportMessage, WorldportMessage,
};
use benilla_world::world_map::CurrentMap;

use super::camera::FlyCam;
use super::{movement_net, MoverLock, OwnControlLost, Player, SETTLE_TIMEOUT};

/// What one `SMSG_CLIENT_CONTROL_UPDATE` means for us. It says whether a unit may move, not who the
/// mover is: the server revokes a mind-controlled victim's control by naming the victim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// The movement flags a verdict sets. `control_lost` is the mover we drive being unable to move:
/// our body revoked, or our possessed creature released while feared or confused (the possession
/// standing). Our own `[0xb4b3e4]` (`0x4958e0`), which `0x5fa600` writes for our own character
/// alone (`0x5fa6d0`), is the packet's handler's to store, signal and refresh inside the packet,
/// with the hold its fall drops (`on_client_control`).
fn track_control(lock: &mut MoverLock, verdict: ControlVerdict) {
    match verdict {
        ControlVerdict::Revoked => lock.control_lost = true,
        ControlVerdict::Restored | ControlVerdict::Granted(_) => lock.control_lost = false,
        ControlVerdict::Released(guid) => {
            if lock.foreign_mover == Some(guid) {
                lock.control_lost = true;
            }
        }
    }
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

/// What [`apply_server_moves`] leaves the controller.
pub(super) struct ServerMoves {
    /// The forced-speed changes for the movement stream to ack, except pre-control or detached,
    /// where they are acked here.
    pub(super) speed_acks: Vec<SpeedChangeMessage>,
    /// An ack went out that ends the mover's precast ([`ack_ends_precast`]).
    pub(super) precast_ended: bool,
    /// A root or unroot ack went out, which runs the mover's input tick after its send
    /// (`0x60e33d` → `0x60e347`, for 0xe9 and 0xeb).
    pub(super) input_ticked: bool,
}

/// Whether a mode's ack ends the mover's precast: the water-walk and hover acks go out through
/// `0x60e0a0` (`0x6163c0` push 0x2d0, `0x616316` and `0x616364` push 0xf6, into `0x6163cf`),
/// which ends it ahead of each opcode it does not send at once (`0x60e15d`); the root, unroot and
/// feather-fall acks (0xe9, 0xeb, 0x2cf) are among those (`0x60e0f1`-`0x60e137`).
fn ack_ends_precast(mode: MoveMode) -> bool {
    matches!(mode, MoveMode::WaterWalk | MoveMode::Hover)
}

/// Apply this frame's server-authored movement and send each ack ([`ServerMoves`]).
/// Our own create's transport block, with the transport's entity and pose as they stand: the
/// seat a login aboard a transport takes control in.
#[derive(Clone, Copy)]
pub(super) struct SelfSeat {
    pub(super) entity: Entity,
    pub(super) guid: u64,
    /// The position and facing in the transport's frame, WoW axes.
    pub(super) local_pos: [f32; 3],
    pub(super) local_orientation: f32,
    pub(super) deck: Transform,
}

/// `self_pose` is the streamed self entity's translation and yaw, the take-control target;
/// `self_seat` its create's transport block.
// `control`'s own transports query, passed through.
#[allow(clippy::type_complexity)]
pub(super) fn apply_server_moves(
    time: &Time,
    commands: &mut Commands,
    player: &mut Player,
    lock: &mut MoverLock,
    cam: &mut FlyCam,
    net_cmds: &NetCommands,
    teleports: &mut MessageReader<TeleportMessage>,
    worldports: &mut MessageReader<WorldportMessage>,
    speed_msgs: &mut MessageReader<SpeedChangeMessage>,
    mode_msgs: &mut MessageReader<MoveModeMessage>,
    knock_msgs: &mut MessageReader<KnockBackMessage>,
    self_moves: &mut MessageReader<SelfMoveMessage>,
    control_msgs: &mut MessageReader<ClientControlMessage>,
    mover: &mut crate::net::MoverClaim,
    self_guid: Option<u64>,
    transports: &super::TransportQuery,
    self_pose: Option<(Vec3, f32)>,
    self_seat: Option<SelfSeat>,
) -> ServerMoves {
    // A grant must be claimed: vmangos drops every `MSG_MOVE_*` for an unconfirmed mover
    // (`MovementHandler.cpp:291-293`, `Player.cpp:20257-20272`).
    for c in control_msgs.read() {
        let verdict = control_verdict(c.mover, c.allow_move, self_guid);
        track_control(lock, verdict);
        match verdict {
            // Park, which stops the server extrapolating a run, and say nothing else: the mover has
            // not changed hands, and `CMSG_MOVE_NOT_ACTIVE_MOVER` would strand us.
            // The stopped mover's global goes too (`0x5fa678`-`0x5fa696`, `0x6006e0(0)`).
            ControlVerdict::Revoked => {
                movement_net::park_mover(&net_cmds.0, player);
                if let Some(me) = self_guid {
                    mover.forget(me);
                }
            }
            ControlVerdict::Restored => {
                lock.foreign_mover = None;
                lock.reseat = true;
                // Re-claim ourselves, as login does (after a possession `m_clientMoverGuid` still
                // names the creature), and send nothing else: `GetConfirmedMover` resolves the
                // mismatch to us (`Player.cpp:20267-20269`), so a parting stop for the creature
                // would move our own character to it.
                if mover.claim(c.mover, net_cmds) {
                    super::move_trace::mover_claim("SET_ACTIVE_MOVER", c.mover);
                }
            }
            // Claim it; recording the claim stops our body's pose streaming under its guid.
            ControlVerdict::Granted(guid) => {
                // Yield our own body first, on the same channel, while it is still the confirmed
                // mover, and only once: vmangos re-sends a grant each time a possessed creature
                // stops fleeing (`FearMovementGenerator.cpp:155`, `Unit.cpp:11157`) and rejects a
                // second yield (`MovementHandler.cpp:891-897`).
                if lock.foreign_mover.is_none() {
                    yield_own_body(net_cmds, player, self_guid);
                }
                lock.foreign_mover = Some(guid);
                lock.reseat = true;
                if mover.claim(guid, net_cmds) {
                    super::move_trace::mover_claim("SET_ACTIVE_MOVER", guid);
                }
            }
            // vmangos sends this on unpossess, after `Restored`, and when the possessed unit is
            // feared or confused (`Unit.cpp:11157`). As in the reference (`0x5fa600`), only the
            // unit we drive matters: we keep it but may not move it. Clearing `foreign_mover`
            // would walk our own body while the server applies our steps to the creature.
            // Its mover global is zeroed (`0x5fa678`-`0x5fa696`), so a later grant claims it again.
            ControlVerdict::Released(guid) => mover.forget(guid),
        }
    }
    // Cross-map worldport: snap, bump `CurrentMap` for the terrain streamer, ack if required.
    for w in worldports.read() {
        // A transfer ends any teleport hold, so its loading screen reaches the window.
        player.world_reload = false;
        let riding = w.transport_entry.is_some() && player.ride.is_some();
        if riding {
            // Riding through: the pose is deck-local (`Player.cpp:2114`) and the boat survived the
            // purge. No settle hold: it would drop `MOVEFLAG_ONTRANSPORT` from the next move
            // packet, which the server reads as a deboard mid-ocean.
            let ride = player.ride.as_mut().expect("riding checked above");
            ride.local_pos = wow_to_bevy(w.position);
            if let Ok((boat, ..)) = transports.get(ride.entity) {
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
    let mut precast_ended = false;
    let mut input_ticked = false;
    for m in mode_msgs.read() {
        player.modes.set(m.mode, m.apply);
        if m.mode == MoveMode::Root && m.apply {
            movement_net::park_mover(&net_cmds.0, player);
            // `0x7c734f`'s `and 0xffe07f00` takes the pitch-key bits with the direction bits.
            player.pitch_keys = None;
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
        precast_ended |= ack_ends_precast(m.mode);
        input_ticked |= m.mode == MoveMode::Root;
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
    let seizing = !player.active || lock.reseat;
    if seizing {
        if let Some((pos, yaw)) = self_pose {
            let first = !player.active;
            lock.reseat = false;
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
            let mut yaw = yaw;
            // 0x618b30: our create on a transport puts the mover in its frame, the deck-local
            // position and facing of the status's transport block (`0x7c6900`), boarded
            // (`0x618970`), never the login pose, which vmangos sends deck-local too.
            if let Some(seat) = self_seat.filter(|_| first) {
                let local = wow_to_bevy(seat.local_pos);
                let boat_yaw = seat.deck.rotation.to_euler(EulerRot::YXZ).0;
                player.pos = seat.deck.translation + seat.deck.rotation * local;
                player.fall_start_y = player.pos.y;
                yaw = wrap_pi(seat.local_orientation + boat_yaw);
                player.ride = Some(super::state::PlayerRide {
                    entity: seat.entity,
                    guid: seat.guid,
                    local_pos: local,
                    boat_yaw,
                    placed: false,
                });
                info!("took control aboard transport {:#x}", seat.guid);
            }
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
                match lock.foreign_mover {
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
    ServerMoves {
        speed_acks,
        precast_ended,
        input_ticked,
    }
}

/// Apply one same-map teleport's snap (`MSG_MOVE_TELEPORT_ACK` inbound); the caller echoes it.
fn snap_near_teleport(player: &mut Player, cam_yaw: &mut f32, t: &TeleportMessage, now: f32) {
    // A near teleport arrives detached: vmangos takes this path only off a transport
    // (`Player.cpp:1866-1893`), and the reference's apply (`0x6186b0`) re-parents to the packet's
    // transport guid, zero here. A kept ride would carry us back onto the deck.
    player.ride = None;
    let from = player.pos;
    player.pos = wow_to_bevy(t.position);
    mark_world_reload(player, from, player.pos, now);
    player.face_yaw = t.orientation;
    *cam_yaw = t.orientation;
    player.move_flags = 0;
    // `0x617e90` places the mover through `0x7c69a0`, whose `and 0xfbc07f00` (`0x7c69c2`) clears
    // the pitch-key bits and which zeroes the pitch (`0x7c69cc`).
    player.pitch_keys = None;
    player.mover_pitch = 0.0;
    player.airborne_since = None; // a snap ends any jump arc (no phantom FALL_LAND)
    hold_until_resident(player, now);
    // Voids any self ride: `drive_self_ride` drops it rather than mirror the stale pose over this.
    player.ride_abort = true;
}

/// The teleport apply `0x617e90`'s reload, for every teleport that reaches it (the ack's snap and
/// our own `MSG_MOVE_TELEPORT`): a move of more than 30 yards (`0x80c730`, squared against `d²`)
/// of the unit the world view follows reloads the destination before the frame goes on
/// (`0x630bd0`). `from` and `to` are in the mover's frame, deck-local aboard a transport, as `d²` is
/// taken of the mover's own position (`+0x10`) after the transport is set (`0x617170`). Returns
/// whether it does. Only marks: a later, shorter teleport (vmangos sends `MSG_MOVE_TELEPORT` twice,
/// `MovementHandler.cpp:252,255`) leaves a hold up, which only its release ends; a far one restarts
/// the hold's ceiling.
fn mark_world_reload(player: &mut Player, from: Vec3, to: Vec3, now: f32) -> bool {
    let far = !player.detached && super::world_focus::is_far_reload(from, to);
    if far {
        player.world_reload = true;
        player.reloads = player.reloads.wrapping_add(1);
        player.reload_ceiling = now + SETTLE_TIMEOUT;
    }
    far
}

/// Hold the body (gravity off) until the destination is resident, which releases it.
fn hold_until_resident(player: &mut Player, now: f32) {
    player.settling = true;
    player.settle_since = now;
    player.settle_deadline = now + SETTLE_TIMEOUT;
    // The destination's ground may not have streamed yet.
    player.world_stale = true;
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
    transports: &super::TransportQuery,
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

    // The mover's own frame for the reload's `d²`: deck-local when the packet places us on the
    // deck we ride, else the world.
    let deck_local = player.ride.as_ref().and_then(|r| {
        let t = m.transport.filter(|t| t.guid == r.guid)?;
        Some((r.local_pos, wow_to_bevy([t.pos.x, t.pos.y, t.pos.z])))
    });
    let from = player.pos;
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
        if let Ok((boat, ..)) = transports.get(ride.entity) {
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
    // A teleport's apply, unlike a placing relay's, reloads a far destination (`0x6186b0` →
    // `0x617e90`), so the body waits for it as the ack's snap does.
    let (from, to) = deck_local.unwrap_or((from, player.pos));
    let now = time.elapsed_secs();
    if m.teleport && mark_world_reload(player, from, to, now) {
        hold_until_resident(player, now);
    }
}

/// Our own player's create gives control back (`OnActiveArrival 0x5dea50`), inside the update
/// packet ([`crate::net::objects`]'s arrival): with the manager's `+0xc8` clear (`0x5deb49`; its
/// constructor `0x4650e7` is the only store, so every create), `0x5fa600(1)` (`0x5deb56`) raises
/// `[0xb4b3e4]` through `0x4958e0` (`PLAYER_CONTROL_GAINED` if it was down) and `0x4958a0`
/// (`0x5deb5b`) stores 1 and re-claims our own mover, ending the movement lock and any
/// possession. vmangos sends a battleground's closing `control 0` and no restore
/// (`BattleGround.cpp:905-908`). Whether the flag was down, whose rise signals in the arrival's
/// place (`0x4958f7`, `0x4958fc`, [`crate::ui_world_enter`]): the event, then the usable refresh.
/// Only the arrival runs it: `0x4958a0` has the one caller `0x5deb5b`, and `0x5fa600` is called at
/// `0x5deb56` and `0x603eeb` alone, so a `/reload`'s cascade (`0x490168` → `0x4908c0`) leaves the
/// lock as it was.
pub(crate) fn regain_control_on_arrival(
    lock: Option<&mut MoverLock>,
    own_control: Option<&mut OwnControlLost>,
) -> bool {
    let own = own_control.as_ref().is_some_and(|o| o.0);
    let held = lock
        .as_ref()
        .is_some_and(|l| l.control_lost || l.foreign_mover.is_some());
    if !own && !held {
        return false;
    }
    if let Some(own_control) = own_control {
        own_control.0 = false;
    }
    if let Some(lock) = lock {
        lock.control_lost = false;
        // The mover is ours again, as `ControlVerdict::Restored` seats it.
        lock.foreign_mover = None;
        lock.reseat = true;
    }
    own
}

/// On a `/logout` or a lost session, reset the whole resource to its boot state, so the next login
/// owes nothing to the ended one. Whole, not by field: `/logout` roots us for the countdown
/// (`MiscHandler.cpp:329`) and the next session never unroots, so any kept mover state persists.
/// The reference also keeps no mover state, shutting the game down on logout (`0x491180`).
pub(super) fn release_on_session_end(
    mut logouts: MessageReader<crate::net::LoggedOutMessage>,
    mut lost: MessageReader<crate::net::DisconnectedMessage>,
    mut player: ResMut<Player>,
    mut lock: ResMut<MoverLock>,
    mut own_control: ResMut<OwnControlLost>,
    mut stand: ResMut<super::StandPending>,
) {
    // `|`, not `||`: both readers must drain, or the unread one carries into the next frame.
    if logouts.read().next().is_some() | lost.read().any(|m| m.session_over) {
        // vmangos re-sends the modes the new session holds (`Player.cpp:19297-19311`).
        *player = Player::default();
        *lock = MoverLock::default();
        *own_control = OwnControlLost::default();
        *stand = super::StandPending::default();
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
            .init_resource::<MoverLock>()
            .init_resource::<OwnControlLost>()
            .init_resource::<super::super::StandPending>()
            .add_message::<LoggedOutMessage>()
            .add_message::<DisconnectedMessage>()
            .add_systems(Update, release_on_session_end);
        app
    }

    /// A session that ran: in world, moving, every mode granted, the reins in other hands.
    fn a_session_that_ran(app: &mut App) {
        app.world_mut().resource_mut::<OwnControlLost>().0 = true;
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
        p.server_riding = true;
        p.swimming = true;
        p.move_flags = crate::creature_anim::move_flags::FORWARD;
        p.pos = Vec3::new(10.0, 20.0, 30.0);
        p.owes_worldport_ack = true;
        p.airborne_since = Some(4.0);
        p.wedged = true;
        let mut lock = app.world_mut().resource_mut::<MoverLock>();
        lock.control_lost = true;
        lock.foreign_mover = Some(0xBB);
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
             `/logout` means the character re-enters the world and WASD is dead"
        );
        assert!(
            *p == Player::default(),
            "and nothing else survives either: the session boundary returns the resource to the \
             state `player::setup` inserts at boot"
        );
        assert!(
            !app.world().resource::<OwnControlLost>().0,
            "`[0xb4b3e4]` is back at its boot value, in control (`0x48f626`)"
        );
        assert_eq!(
            *app.world().resource::<MoverLock>(),
            MoverLock::default(),
            "and the reins are ours"
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
        assert_eq!(*app.world().resource::<MoverLock>(), MoverLock::default());
        assert!(!app.world().resource::<OwnControlLost>().0);
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

    /// A possessed creature feared or confused is released, so the mover we drive cannot move;
    /// our own body's verdicts lock and free it.
    #[test]
    fn releasing_a_possessed_creature_locks_the_creature_alone() {
        let mut lock = MoverLock::default();
        track_control(&mut lock, control_verdict(CREATURE, true, Some(ME)));
        assert!(!lock.control_lost);
        lock.foreign_mover = Some(CREATURE);
        track_control(&mut lock, control_verdict(CREATURE, false, Some(ME)));
        assert!(lock.control_lost, "the creature we drive is stuck");
        track_control(&mut lock, control_verdict(ME, false, Some(ME)));
        assert!(lock.control_lost);
        track_control(&mut lock, control_verdict(ME, true, Some(ME)));
        assert!(!lock.control_lost);
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
                placed: true,
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
        player.pitch_keys = Some(true);
        player.mover_pitch = 0.4;
        snap_near_teleport(&mut player, &mut cam_yaw, &t, 10.0);
        assert!(player.ride.is_none(), "the snap deboards");
        assert_eq!(player.pitch_keys, None, "`0x7c69a0` clears the pitch keys");
        assert_eq!(player.mover_pitch, 0.0, "and zeroes the pitch (`0x7c69cc`)");
        assert_eq!(player.pos, wow_to_bevy(t.position));
        assert_eq!((player.face_yaw, cam_yaw), (1.25, 1.25));
        assert!(player.settling && player.ride_abort && player.world_stale);
    }

    /// `0x617e90` reloads the world only for `d² > 900` of the unit the camera follows
    /// (`0x630bd0`): a short hop, or a snap under a free-fly eye, holds no frame.
    #[test]
    fn only_a_snap_past_30_yards_of_the_followed_body_reloads_the_world() {
        let snapped = |from: [f32; 3], to: [f32; 3], detached: bool| {
            let mut player = Player {
                active: true,
                detached,
                pos: wow_to_bevy(from),
                ..Default::default()
            };
            let t = TeleportMessage {
                guid: 0x45,
                counter: 1,
                position: to,
                orientation: 0.0,
            };
            snap_near_teleport(&mut player, &mut 0.0, &t, 10.0);
            player
        };
        let from = [16_222.0, 16_252.0, 12.5];
        let south_seas = [-11_880.0, -4_700.0, 2.0];
        let holds = |to, detached| snapped(from, to, detached).holds_the_frame();
        assert!(holds(south_seas, false), "GM Island to the South Seas");
        assert!(holds([16_222.0, 16_282.5, 12.5], false), "30.5 yards");
        assert!(!holds([16_222.0, 16_281.5, 12.5], false), "29.5 yards");
        assert!(!holds(south_seas, true), "the eye does not follow");

        assert_eq!(
            snapped(from, south_seas, false).reloads,
            1,
            "a far snap marks"
        );
        assert_eq!(snapped(from, [16_222.0, 16_281.5, 12.5], false).reloads, 0);

        let mut player = snapped(from, south_seas, false);
        player.end_settle(true, 11.0);
        assert!(
            !player.holds_the_frame(),
            "the settle release ends the hold"
        );
    }
}

#[cfg(test)]
mod arrival_tests {
    use super::*;

    const ME: u64 = 0x45;

    /// The control-0 packet naming us: its handler's flag and the controller's lock.
    fn lose_control(lock: &mut MoverLock, own: &mut OwnControlLost) {
        own.0 = true;
        track_control(lock, control_verdict(ME, false, Some(ME)));
    }

    /// Whether the arrival's flag rise is owed its signals.
    fn arrive(lock: &mut MoverLock, own: &mut OwnControlLost) -> bool {
        regain_control_on_arrival(Some(lock), Some(own))
    }

    /// A battleground's end sends `control 0` for us and never the restore; the teleport out
    /// creates us afresh and that create gives control back, the movement lock and the bar's
    /// `[0xb4b3e4]` flag both, signalling the flag's rise and the usable refresh.
    #[test]
    fn a_control_loss_does_not_survive_the_next_self_create() {
        let (mut lock, mut own) = (MoverLock::default(), OwnControlLost::default());
        lose_control(&mut lock, &mut own);
        assert!(lock.control_lost && own.0);
        let signals = arrive(&mut lock, &mut own);
        assert!(!own.0, "the bar's flag is back");
        assert!(!lock.control_lost, "and the controller's movement lock");
        assert_eq!(lock.foreign_mover, None);
        assert!(signals, "the flag's rise signals (`0x4958f7`, `0x4958fc`)");
    }

    /// A possession does not outlive the world either: the create claims our own mover, with no
    /// signal, as the flag never fell.
    #[test]
    fn a_create_ends_a_possession_too() {
        let (mut lock, mut own) = (MoverLock::default(), OwnControlLost::default());
        lock.foreign_mover = Some(0x99);
        lock.control_lost = true;
        let signals = arrive(&mut lock, &mut own);
        assert_eq!(lock.foreign_mover, None);
        assert!(!lock.control_lost && lock.reseat);
        assert!(!signals, "the flag never fell");
    }

    /// A login in control touches nothing.
    #[test]
    fn a_create_in_control_changes_nothing() {
        let (mut lock, mut own) = (MoverLock::default(), OwnControlLost::default());
        let signals = arrive(&mut lock, &mut own);
        assert!(!lock.reseat && !signals);
    }
}

#[cfg(test)]
mod self_teleport_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// One bare self-addressed move from GM Island's dock, applied as the controller applies it.
    fn applied(to: [f32; 3], teleport: bool, detached: bool) -> Player {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.insert_resource(Player {
            active: true,
            detached,
            pos: wow_to_bevy([16_222.0, 16_252.0, 12.5]),
            ..Default::default()
        });
        let m = SelfMoveMessage {
            position: to,
            orientation: 0.0,
            flags: 0,
            pitch: 0.0,
            fall_time: 0,
            jump: None,
            transport: None,
            teleport,
        };
        world
            .run_system_once(
                move |time: Res<Time>,
                      mut player: ResMut<Player>,
                      transports: super::super::TransportQuery| {
                    let mut cam = FlyCam {
                        yaw: 0.0,
                        pitch: 0.0,
                        speed: 0.0,
                    };
                    apply_self_move(&m, &mut player, &mut cam, &time, &transports);
                },
            )
            .expect("runs");
        world.remove_resource::<Player>().expect("player")
    }

    /// Our own mover's `MSG_MOVE_TELEPORT` reaches the teleport apply `0x617e90` (`0x6186ee`,
    /// `0x61870c`), so it reloads under the same gate as the ack's snap: past 30 yards, with the
    /// camera following. The other relays never reach it.
    #[test]
    fn a_self_teleport_past_30_yards_holds_the_body_and_the_frame() {
        let south_seas = [-11_880.0, -4_700.0, 2.0];
        let far = applied(south_seas, true, false);
        assert!(
            far.holds_the_frame(),
            "GM Island to the South Seas holds the frame"
        );
        assert!(
            far.settling && far.world_stale,
            "and the body, until resident"
        );
        assert!(
            !applied([16_222.0, 16_281.5, 12.5], true, false).holds_the_frame(),
            "29.5 yards"
        );
        assert!(
            applied([16_222.0, 16_282.5, 12.5], true, false).holds_the_frame(),
            "30.5 yards"
        );
        assert!(
            !applied(south_seas, true, true).holds_the_frame(),
            "the eye does not follow"
        );
        let placed = applied(south_seas, false, false);
        assert!(
            !placed.holds_the_frame() && !placed.settling,
            "a relay that only places the mover reloads nothing"
        );
    }
}

#[cfg(test)]
mod seat_tests {
    use super::*;
    use crate::player::FlyCam;
    use bevy::ecs::message::Messages;
    use bevy::ecs::system::RunSystemOnce;

    const DECK: u64 = 0x1FC0_0000_0000_0042;

    /// Take control at login with our create's transport block on a deck at `deck_at`, yawed a
    /// quarter turn, then run one carry; `armed` puts the deck's path in place.
    /// What the seat leaves: the ride's guid, deck-local feet and placed flag, the feet and the
    /// facing.
    type Seated = (Option<(u64, Vec3, bool)>, Vec3, f32);

    #[allow(clippy::type_complexity)] // the system's message readers
    fn login_aboard(armed: bool) -> (Seated, Transform) {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Player>();
        world.init_resource::<OwnControlLost>();
        let (tx, _rx) = crossbeam_channel::unbounded();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Messages<TeleportMessage>>();
        world.init_resource::<Messages<WorldportMessage>>();
        world.init_resource::<Messages<SpeedChangeMessage>>();
        world.init_resource::<Messages<MoveModeMessage>>();
        world.init_resource::<Messages<KnockBackMessage>>();
        world.init_resource::<Messages<SelfMoveMessage>>();
        world.init_resource::<Messages<ClientControlMessage>>();
        let deck_at = Transform::from_xyz(100.0, 5.0, -40.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let mut deck = world.spawn((deck_at, crate::net::Guid(DECK)));
        if armed {
            deck.insert(crate::transport::Transport::parked_for_test());
        }
        let deck = deck.id();
        world.insert_resource(crate::net::GuidIndex([(DECK, deck)].into_iter().collect()));
        let seat = SelfSeat {
            entity: deck,
            guid: DECK,
            local_pos: [2.0, 1.0, 3.0],
            local_orientation: 0.5,
            deck: deck_at,
        };
        let player = world
            .run_system_once(
                move |time: Res<Time>,
                      mut commands: Commands,
                      mut player: ResMut<Player>,
                      net: Res<NetCommands>,
                      mut readers: (
                    MessageReader<TeleportMessage>,
                    MessageReader<WorldportMessage>,
                    MessageReader<SpeedChangeMessage>,
                    MessageReader<MoveModeMessage>,
                    MessageReader<KnockBackMessage>,
                    MessageReader<SelfMoveMessage>,
                    MessageReader<ClientControlMessage>,
                ),
                      transports: super::super::TransportQuery,
                      index: Res<crate::net::GuidIndex>| {
                    let mut cam = FlyCam {
                        yaw: 0.0,
                        pitch: 0.0,
                        speed: 0.0,
                    };
                    apply_server_moves(
                        &time,
                        &mut commands,
                        &mut player,
                        &mut crate::player::MoverLock::default(),
                        &mut cam,
                        &net,
                        &mut readers.0,
                        &mut readers.1,
                        &mut readers.2,
                        &mut readers.3,
                        &mut readers.4,
                        &mut readers.5,
                        &mut readers.6,
                        &mut crate::net::MoverClaim::default(),
                        Some(1),
                        &transports,
                        // The create's world position, which the seat replaces.
                        Some((Vec3::new(-500.0, 0.0, 0.0), 0.0)),
                        Some(seat),
                    );
                    super::super::ride::carry(&mut player, &mut cam, &transports, &index);
                    let ride = player
                        .ride
                        .as_ref()
                        .map(|r| (r.guid, r.local_pos, r.placed));
                    (ride, player.pos, player.face_yaw)
                },
            )
            .expect("runs");
        (player, deck_at)
    }

    /// `0x618b30`: our create on a transport seats the mover in its frame, from the status's
    /// transport block, whether or not the transport's path is in place yet.
    #[test]
    fn a_login_aboard_a_transport_seats_us_on_its_deck() {
        for armed in [false, true] {
            let ((ride, pos, face_yaw), deck) = login_aboard(armed);
            let (guid, local_pos, placed) = ride.expect("seated on the deck");
            assert_eq!(guid, DECK);
            let local = wow_to_bevy([2.0, 1.0, 3.0]);
            assert_eq!(local_pos, local);
            assert!(
                (pos - (deck.translation + deck.rotation * local)).length() < 1e-4,
                "the feet are the deck's pose composed with the deck-local seat, armed {armed}"
            );
            assert!(
                (face_yaw - wrap_pi(0.5 + std::f32::consts::FRAC_PI_2)).abs() < 1e-5,
                "the facing is the seat's on top of the deck's"
            );
            assert_eq!(placed, armed, "a deck without its path holds the seat");
        }
    }
}

#[cfg(test)]
mod claim_tests {
    use super::*;
    use bevy::ecs::system::SystemId;
    use crossbeam_channel::Receiver;

    const ME: u64 = 0x45;
    const CREATURE: u64 = 0xF130_0000_0000_0077;

    /// The control updates' path in the controller over one world, its mover claim and lock
    /// persisting from round to round, as the client's do.
    struct Rounds {
        world: World,
        system: SystemId,
        sent: Receiver<ClientCommand>,
    }

    impl Rounds {
        #[allow(clippy::type_complexity)] // the system's message readers
        fn new() -> Self {
            let mut world = World::new();
            world.init_resource::<Time>();
            world.init_resource::<Player>();
            world.init_resource::<MoverLock>();
            world.init_resource::<crate::net::MoverClaim>();
            let (tx, sent) = crossbeam_channel::unbounded();
            world.insert_resource(NetCommands(tx));
            world.init_resource::<Messages<TeleportMessage>>();
            world.init_resource::<Messages<WorldportMessage>>();
            world.init_resource::<Messages<SpeedChangeMessage>>();
            world.init_resource::<Messages<MoveModeMessage>>();
            world.init_resource::<Messages<KnockBackMessage>>();
            world.init_resource::<Messages<SelfMoveMessage>>();
            world.init_resource::<Messages<ClientControlMessage>>();
            let system = world.register_system(
                |time: Res<Time>,
                 mut commands: Commands,
                 mut player: ResMut<Player>,
                 mut lock: ResMut<MoverLock>,
                 mut claim: ResMut<crate::net::MoverClaim>,
                 net: Res<NetCommands>,
                 mut readers: (
                    MessageReader<TeleportMessage>,
                    MessageReader<WorldportMessage>,
                    MessageReader<SpeedChangeMessage>,
                    MessageReader<MoveModeMessage>,
                    MessageReader<KnockBackMessage>,
                    MessageReader<SelfMoveMessage>,
                    MessageReader<ClientControlMessage>,
                ),
                 transports: super::super::TransportQuery| {
                    let mut cam = FlyCam {
                        yaw: 0.0,
                        pitch: 0.0,
                        speed: 0.0,
                    };
                    apply_server_moves(
                        &time,
                        &mut commands,
                        &mut player,
                        &mut lock,
                        &mut cam,
                        &net,
                        &mut readers.0,
                        &mut readers.1,
                        &mut readers.2,
                        &mut readers.3,
                        &mut readers.4,
                        &mut readers.5,
                        &mut readers.6,
                        &mut claim,
                        Some(ME),
                        &transports,
                        None,
                        None,
                    );
                },
            );
            Self {
                world,
                system,
                sent,
            }
        }

        /// The claims one round's control updates send, `(mover, allow_move)` each.
        fn claims(&mut self, updates: &[(u64, bool)]) -> Vec<u64> {
            for &(mover, allow_move) in updates {
                self.world
                    .write_message(ClientControlMessage { mover, allow_move });
            }
            self.world.run_system(self.system).expect("the round runs");
            self.sent
                .try_iter()
                .filter_map(|c| match c {
                    ClientCommand::SetActiveMover { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        }
    }

    /// `SetActiveMover` (`0x6006e0`) returns at once for the mover it holds (`0x6006ee`-
    /// `0x6006fb`): a restore with our own body still claimed sends nothing, and one after a
    /// revoke, which zeroes the mover (`0x5fa678`-`0x5fa696`), sends once.
    #[test]
    fn a_restore_claims_only_a_mover_it_does_not_hold() {
        let mut rounds = Rounds::new();
        let net = NetCommands(rounds.world.resource::<NetCommands>().0.clone());
        rounds
            .world
            .resource_mut::<crate::net::MoverClaim>()
            .claim(ME, &net);
        assert_eq!(rounds.claims(&[]), [ME], "the arrival's claim");
        assert_eq!(rounds.claims(&[(ME, true)]), Vec::<u64>::new(), "held");
        assert_eq!(rounds.claims(&[(ME, false)]), Vec::<u64>::new(), "revoked");
        assert_eq!(rounds.claims(&[(ME, true)]), [ME], "restored, once");
        assert_eq!(
            rounds.claims(&[(ME, true)]),
            Vec::<u64>::new(),
            "and not again"
        );
    }

    /// A possession's grant claims the creature; its release under fear zeroes the mover
    /// (`0x5fa678`-`0x5fa696`), so the grant that ends the fear claims it again: two claims.
    #[test]
    fn a_grant_a_release_and_a_grant_claim_twice() {
        let mut rounds = Rounds::new();
        assert_eq!(rounds.claims(&[(CREATURE, true)]), [CREATURE], "granted");
        assert_eq!(
            rounds.claims(&[(CREATURE, false)]),
            Vec::<u64>::new(),
            "released"
        );
        assert_eq!(
            rounds.claims(&[(CREATURE, true)]),
            [CREATURE],
            "granted again"
        );
        assert_eq!(
            rounds.claims(&[(CREATURE, true)]),
            Vec::<u64>::new(),
            "a grant of the claimed creature claims nothing"
        );
    }
}
