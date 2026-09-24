//! Server-authored movement edges applied to our mover — the inbound mirror of
//! [`super::movement_net`] (which streams our own movement out). One entry point,
//! [`apply_server_moves`], called by [`super::control`] before input integrates: cross-map
//! worldports (incl. the riding-through-the-seam branch, decision 0455), same-map teleports, the
//! **granted movement-mode family** (root, water-walk, feather-fall, hover — decisions 0308/0866,
//! applied to [`super::state::MoveModes`] and acked here), the one-shot take-control edge, the
//! pre-control forced-speed acks (the controlled branch answers those through the movement
//! stream's own per-frame payload instead — the returned list), and the **bare self-addressed
//! move** ([`apply_self_move`], decision 0725).
//!
//! All but the last carry a **mandatory ack**, which is what made the last one easy to miss: a
//! `MSG_MOVE_*` the server addresses to our own guid arrives with no handshake and owes no answer,
//! and this client used to discard it. The real client applies it — its inbound move path has no
//! mover-guid gate at all — and sends nothing back; the next ordinary heartbeat carries the
//! server's own pose home, which is what makes `.go forward` stick.

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

/// What one `SMSG_CLIENT_CONTROL_UPDATE` means for us — the four cases its two fields make, named.
///
/// Worth a type because the packet is genuinely ambiguous read casually: it is a statement *about a
/// unit*, not an assignment, and the naive reading ("the mover is now `mover`") silently inverts
/// the mind-controlled victim — the case where the server names **us** to say we may no longer
/// move. That victim is half of B211, so the inversion would have shipped as the bug it was meant
/// to fix.
#[derive(Debug, PartialEq, Eq)]
enum ControlVerdict {
    /// Somebody is driving our body. We stop moving it — nothing else will stop us.
    Revoked,
    /// Our own body is ours again.
    Restored,
    /// We were handed the reins of another unit, and owe it a mover claim.
    Granted(u64),
    /// A unit we were driving was taken back, and owes a parting pose.
    Released(u64),
}

/// Give our own body back to the server on a **hand-over**: the stop, then the parting pose.
///
/// `CMSG_MOVE_NOT_ACTIVE_MOVER` (`0x2D1`) is **only ever about our own character**. The reference
/// sends it from `SetActiveMover 0x6006e0` for the outgoing mover *when that mover was the local
/// player* (`0x5fa6d0`), and vmangos agrees from the other side: `HandleMoveNotActiveMover` rejects
/// any guid that is not the session's currently-confirmed mover, and rejects again if the guid names
/// something other than the player while the player's mover still is it. There is no such packet for
/// a creature we are handing back.
///
/// **And none for a body merely frozen, either** (decision 1281). Being feared, confused or
/// mind-controlled is not a mover change: our own body is still the mover, and the server still
/// expects us to answer for it. What this packet does server-side is clear `m_clientMoverGuid`, and
/// that single field is the key to two doors — `HandleMoveSplineDone` refuses any acknowledgement
/// whose unit is not the confirmed mover, while `MoveSplineInit::Launch` arms
/// `HasPendingSplineDone` on every spline it starts for a player, and `HandleMovementOpcodes` drops
/// **every** movement packet until that acknowledgement arrives. Sending it on a fear therefore
/// locks the client out of its own body for good: the flee splines can no longer be acked, so the
/// server keeps our position and orientation frozen at the moment of the fear long after it lifts,
/// and every spell we cast answers "you are facing the wrong way" (director, 2026-08-13). Only a
/// map change clears it (`Map::AddPlayerToMap`), which is why relogging appeared to fix it.
///
/// Skipped while a server-authored spline owns the body: the reference returns early on that edge
/// (`0x619d50`), and vmangos would discard it anyway (`HasPendingSplineDone`).
/// The pitch a login opens at when no saved camera pose names one.
const LOGIN_PITCH: f32 = -0.45;

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

/// Classify a control update. `self_guid` is `None` only before login has named us, where nothing
/// can be "about me" yet.
fn control_verdict(mover: u64, allow_move: bool, self_guid: Option<u64>) -> ControlVerdict {
    match (self_guid == Some(mover), allow_move) {
        (true, false) => ControlVerdict::Revoked,
        (true, true) => ControlVerdict::Restored,
        (false, true) => ControlVerdict::Granted(mover),
        (false, false) => ControlVerdict::Released(mover),
    }
}

/// Drain this frame's server-authored movement messages, apply them to the mover, and send each
/// ack. Returns the frame's forced-speed changes: pre-control/detached they were acked here with
/// the parked pose; controlled, the caller's movement stream acks them with its live payload.
/// `self_pose` is the streamed self entity's current translation + facing yaw (the take-control
/// snap target).
// One system phase's full input set (the spawner precedent); the transports query type is
// `control`'s own param shape passed through.
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
    // The control handoff (B211). Two questions, and they are NOT the same one: "is this about my
    // own body?" and "may that unit move?". The server revokes by naming *us* with allowMove=0 —
    // which is what a mind-controlled player receives about themselves — and grants by naming
    // somebody else, so a single "the mover is now `mover`" reading gets the victim backwards.
    //
    // The claim reply is not optional. vmangos discards every `MSG_MOVE_*` for a mover it has not
    // confirmed (`Player::GetConfirmedMover`), so a grant we never answer leaves the possessed unit
    // frozen server-side while our client cheerfully walks it around locally.
    for c in control_msgs.read() {
        match control_verdict(c.mover, c.allow_move, self_guid) {
            // Somebody is driving our body. We stop driving it and say **nothing** — the mover has
            // not changed hands, only the permission to move it, and the one packet that looks
            // right here (`CMSG_MOVE_NOT_ACTIVE_MOVER`) is the one that strands us: see
            // [`yield_own_body`]. The flush below is the whole of our answer, and it matters — we
            // may have been mid-run when the fear landed, and the server would otherwise keep
            // extrapolating that run for every observer.
            ControlVerdict::Revoked => {
                player.control_lost = true;
                movement_net::park_mover(&net_cmds.0, player);
            }
            ControlVerdict::Restored => {
                player.control_lost = false;
                player.foreign_mover = None;
                player.reseat = true;
                // Re-claim ourselves as the mover — the same claim login makes. After a possession
                // it is mandatory: the server's `m_clientMoverGuid` still names the creature we
                // were driving. After a plain fear it is a no-op re-assert, because nothing ever
                // took our own claim away (see [`yield_own_body`]).
                //
                // And send **nothing else**. A parting stop for the creature we were driving looks
                // right and is a teleport: by the time this packet is written the server has
                // already pointed `m_mover` back at us while `m_clientMoverGuid` still names the
                // creature, so `GetConfirmedMover` falls through its mismatch branch to *us* — and
                // a stop carrying the creature's pose would relocate our own character to wherever
                // the creature was standing. The creature's own stop is the server's to send; it
                // owns that unit again.
                super::move_trace::mover_claim("SET_ACTIVE_MOVER", c.mover);
                let _ = net_cmds
                    .0
                    .send(ClientCommand::SetActiveMover { guid: c.mover });
            }
            // Claim them, or the server drops everything we send for that unit. Recording the
            // claim is what stops the controller streaming OUR body's pose under their guid —
            // outbound moves carry none of their own, so that would teleport them onto us.
            ControlVerdict::Granted(guid) => {
                // Hand our own body over FIRST, while it is still the mover the server attributes
                // to. One command channel, so this is genuinely ordered ahead of the claim — and
                // it has to be, in both directions: vmangos rejects a
                // `CMSG_MOVE_NOT_ACTIVE_MOVER` whose guid is not the mover it currently has
                // confirmed, and a `MSG_MOVE_STOP` sent after the claim would carry our pose under
                // the creature's guid.
                // …but only if it still IS the mover the server attributes to us. A grant is not
                // always a first grant: vmangos re-sends one every time a possessed creature stops
                // fleeing (`Unit::UpdateControl` off the fear generator's `Finalize`), and by then
                // our own body was handed over long ago. Yielding it twice sends a
                // `CMSG_MOVE_NOT_ACTIVE_MOVER` naming a unit that is not the confirmed mover, which
                // vmangos rejects with an error log every time the creature is feared.
                if player.foreign_mover.is_none() {
                    yield_own_body(net_cmds, player, self_guid);
                }
                player.control_lost = false;
                player.foreign_mover = Some(guid);
                player.reseat = true;
                super::move_trace::mover_claim("SET_ACTIVE_MOVER", guid);
                let _ = net_cmds.0.send(ClientCommand::SetActiveMover { guid });
            }
            // "That unit may not move" — about a unit that is not us. Genuinely ambiguous on the
            // wire, because vmangos sends it from two places: the unpossess path (right after the
            // `Restored` that already took us home) and, while a possession is still very much in
            // force, whenever the possessed unit becomes feared or confused.
            //
            // The reference resolves both with one rule, and so do we: *is this the unit I am
            // driving?* If it is, we are still holding it and merely forbidden to move it — the
            // mover global goes to nobody, exactly as `0x5fa600` zeroes it when the named unit is
            // the current one. If it is not, `Restored` already ran and there is nothing to do.
            //
            // Clearing `foreign_mover` here — the obvious reading — would send us back to driving
            // our own body while the server still has `m_mover` pointed at the creature, so every
            // step we took would be applied to the creature instead.
            ControlVerdict::Released(guid) => {
                if player.foreign_mover == Some(guid) {
                    player.control_lost = true;
                }
            }
        }
    }
    // Cross-map worldport (`.tele Orgrimmar`, initial-login map, a boat crossing the sea): the net
    // bridge surfaced it as a message earlier this frame (WorldStage::Net). Snap the avatar, bump
    // `CurrentMap` so the terrain streamer swaps ADTs on the next frame, and ack if required (the
    // ack unblocks the new map stream).
    for w in worldports.read() {
        let riding = w.transport_entry.is_some() && player.ride.is_some();
        if riding {
            // Riding through the transfer (decision 0455): the pose is BOAT-LOCAL (vmangos
            // `SendNewWorld` sends the rider's `GetTransportPos()`), and the boat entity was
            // spared through the worldport purge — recompose the world pose through its live
            // transform. NO settle hold: the deck is the support and its collider never
            // unloaded — and settling ("held") would drop MOVEFLAG_ONTRANSPORT from the first
            // post-crossing move packet (the 0447 flag law is `ride && !held`), which the
            // server reads as a deboard mid-ocean.
            let ride = player.ride.as_mut().expect("riding checked above");
            ride.local_pos = wow_to_bevy(w.position);
            if let Ok((boat, _, _)) = transports.get(ride.entity) {
                let boat_yaw = boat.rotation.to_euler(EulerRot::YXZ).0;
                ride.boat_yaw = boat_yaw;
                player.pos = boat.translation + boat.rotation * ride.local_pos;
                // Local wire orientation + boat yaw = world facing (the GetAbsoluteFacing law);
                // carry the body and camera to match, as the deck turn does (rigid for the
                // whole rider — a lone aim carry leaves the body-chase to sweep + shuffle).
                let dyaw = wrap_pi(w.orientation + boat_yaw - player.face_yaw);
                player.face_yaw += dyaw;
                player.model_yaw = wrap_pi(player.model_yaw + dyaw);
                cam.yaw += dyaw;
            } else {
                // The spared boat is gone (shouldn't happen — the spare predicate keys on the
                // ride's own path). Land at the local pose read as world: wrong but bounded;
                // the server's post-ack stream corrects us.
                warn!("worldport: riding but the boat entity is missing — using raw pose");
                player.pos = wow_to_bevy(w.position);
                player.ride = None;
            }
        } else {
            // A transfer the server did NOT carry a transport through (GM `.tele`, dungeon
            // port): world pose — and any ride is stale, the server detached us (without this
            // the next frame's carry would yank the avatar back onto the boat).
            player.ride = None;
            player.pos = wow_to_bevy(w.position);
            player.face_yaw = w.orientation;
            player.model_yaw = w.orientation; // a teleport snaps the body — no chase across a loading screen
            cam.yaw = w.orientation;
            player.settling = true; // hold (gravity off) until the new map's ground streams in
            player.settle_since = time.elapsed_secs();
            player.settle_deadline = time.elapsed_secs() + SETTLE_TIMEOUT;
            // …and the physics world under the new position is still the OLD map's until the
            // streamer swaps it, one stage from now. See [`Player::world_stale`].
            player.world_stale = true;
        }
        player.move_flags = 0;
        player.airborne_since = None; // a snap ends any in-progress jump arc (no phantom FALL_LAND)
                                      // `insert_resource` replaces if it exists; terrain_stream watches it for diff vs the
                                      // loaded map. If terrain setup never ran (no `./WoW/`), inserting is harmless.
        commands.insert_resource(CurrentMap(w.map_id));
        if w.needs_ack {
            if riding {
                // A riding crossing never settles (the deck is the support, 0455) — ack now,
                // exactly as before 1340.
                let _ = net_cmds.0.send(ClientCommand::WorldportAck);
                info!(
                    "worldport: mapId {} @ {:?} (riding, boat-local pose, acked)",
                    w.map_id, w.position
                );
            } else {
                // The ack rides the settle release (decision 1340): the real client sends
                // MSG_MOVE_WORLDPORT_ACK only after its blocking destination load completes, and
                // vmangos keeps us out-of-world — dropping everything we'd send — until the ack
                // lands, with no load timeout. See [`Player::owes_worldport_ack`].
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
    // Same-map teleport (the bridge only emits ours). Snap + echo the ack — without it the server
    // freezes our movement until relog.
    for t in teleports.read() {
        snap_near_teleport(player, &mut cam.yaw, t, time.elapsed_secs());
        // The echo goes now, and it is the WHOLE near-teleport handshake (decision 1340). The
        // real client echoes on the very next movement tick (the 0xC7 drain applies the
        // snap, then `0x60e0a0` sends guid+counter+time) and sends nothing else — no
        // `MSG_MOVE_STOP` exists anywhere in its chain. vmangos holds us at the OLD position
        // until the echo lands; processing it runs the full relocation + visibility refresh
        // (`ExecuteTeleportNear` → `TeleportPositionRelocation`), and the destination's units
        // stream within ~1 s of this send — measured with the `pop` pulse, four legs, no
        // stall. The invented echo-time Stop this arm used to send served nothing; if a
        // silent-player pop-in ever resurfaces, `WOW_MOVE_TRACE_TAGS=pop` decides in one run.
        let _ = net_cmds.0.send(ClientCommand::TeleportAck {
            guid: t.guid,
            counter: t.counter,
        });
        info!("teleport: snapped to {:?}, acked", t.position);
    }
    // A bare server-authored move for our own mover (decision 0725) — no handshake, no ack. Drained
    // after the teleport arms deliberately: a teleport in the same frame is the larger edge (it
    // swaps maps, holds the settle and owes an ack), so it wins the pose.
    for m in self_moves.read() {
        if player.active {
            apply_self_move(m, player, cam, time, transports);
        }
    }

    // **The ack'd movement-mode family** (decisions 0308, 0866) — root, water-walk, feather-fall,
    // hover. Apply the mode to our typed state FIRST, then ack with the flag word that state
    // rebuilds to: that ordering is the real client's, and it is also the server's law — an
    // apply-ack whose `MovementInfo` lacks the mode bit un-grants the very mode it is accepting,
    // and for root it is an outright KICK (vmangos `HandleMoveRootAck:715-723`, live-verified
    // against the deploy's `Movement.log`).
    //
    // Root additionally **parks the walk stream and ends the arc**: moving bits must never
    // accompany `MOVEFLAG_ROOT` (they freeze the real client, and vmangos raises
    // `CHEAT_TYPE_ROOT_MOVE`), and the reference's own `SetRoot 0x7c7340` clears the direction bits
    // and calls stop-fall `0x7c6290` at apply. Turn bits are *not* moving bits —
    // `MOVEFLAG_MASK_MOVING` excludes them — the same asymmetry that keeps turning live while
    // rooted.
    for m in mode_msgs.read() {
        player.modes.set(m.mode, m.apply);
        if m.mode == MoveMode::Root && m.apply {
            movement_net::park_mover(&net_cmds.0, player);
            player.airborne_since = None;
            player.vel_y = 0.0;
            player.fall_far = false;
        }
        // **A hover grant jumps you** ([`Player::hover_launch`], decision 1620). The reference's
        // handler for this very opcode is `0x61a620`, and setting the flag is its *last* act:
        // `61a62c je` splits enable from disable, enable runs `CMovement::Jump(force = 0)`
        // (`61a62e push 0; 61a630 call 0x7c6230`) and disable runs `StartFalling` (`61a637 call
        // 0x7c61c0`), and only then does `61a646 call 0x7c7310` write `0x40000000`. Casting
        // Levitate while swimming therefore launches the body clear of the water — the director's
        // report against the reference client, which 1616 had recorded as faithfully inert
        // because its census looked inside the resolver and the lift is a layer above it.
        //
        // The revoke arm needs nothing of ours: `StartFalling` sets FALLING with velocity 0, and
        // dropping `hover_offset` to zero already leaves the body a yard over its floor with the
        // ground probe out of reach, so the next step falls from rest by itself.
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

    // **The knockback** (decision 1702) — the one server-authored movement edge here that is neither
    // a snap nor a granted mode: a ballistic launch we fly ourselves, with the ack owed only if we
    // actually fly it. Nothing is applied here and nothing is acked here, and both halves are the
    // reference's own shape: `0x617a30` merely **enqueues** the knockback as a due-timed record, and
    // the frame update `0x616620` drains it apply-then-send inside one call
    // (`0x61624d call 0x6179c0` → `0x616261 push 0xf0`), so the `MovementInfo` that goes on the wire
    // is always the post-launch one. Arming a latch the take-off site consumes is that ordering,
    // written the way our mover works — and it is the same latch shape the hover launch uses, for
    // the same reason (decision 1620): our mover re-derives ground contact from probes every frame
    // and would zero a velocity written from out here on its very next step.
    //
    // **Two in one frame collapse to the last, and that is ours, not the reference's.** Its queue
    // holds both, sorted by due time, and drains them in order; our latch holds one. The *motion* is
    // the same either way — `0x6179c0` replaces the velocity outright, so flying both in sequence
    // inside one frame ends exactly where flying only the second does — but the first one's ack is
    // lost, which the server counts as `OnFailedToAckChange`. It takes two knockbacks inside 17 ms
    // to reach, and sub-frame launch timing is already the open item in 1702.
    for k in knock_msgs.read() {
        player.knockback = Some(super::state::PendingKnockback {
            guid: k.guid,
            counter: k.counter,
            launch: k.launch,
        });
    }

    // Take control once the server first reports our position (the streamed mover entity, whose
    // transform is already in Bevy space). From here the controller drives that entity directly;
    // the entity renderer attaches its body model (0041) the same way it does for any other player.
    //
    // The same edge serves a *mover change* — a possessed creature arriving in our hands, or our
    // own body coming back (decision 1277). It is one path deliberately: seizing a body means the
    // same thing either way, and the resource's pose describes whatever we were driving a moment
    // ago. What differs is only the login-once half below (`first`), which owns the settle, the
    // stale-world flag and the initial camera seat.
    let seizing = !player.active || player.reseat;
    if seizing {
        if let Some((pos, yaw)) = self_pose {
            let first = !player.active;
            player.reseat = false;
            // Momentum, movement bits and the platform under our feet all belonged to the body we
            // just let go of. The reference's `SetActiveMover 0x6006e0` tears exactly this down on
            // the outgoing mover — resetting movement flags and cancelling click-to-move — and
            // carrying any of it across would have the new body sprinting, falling or standing on a
            // boat it is nowhere near.
            //
            // `first` scopes this to a mover change **inside one session**, and that gate is
            // right: on a login there is no outgoing mover here to tear down, because the session
            // that owned it ended at `release_on_session_end`, which since decision 1542 takes the
            // whole resource. Until it did, the two lists were the same law kept in two places and
            // this one was the longer — the reason B306's root outlived a `/logout` was that the
            // only teardown naming `modes` was the one a login skips.
            if !first {
                player.vel_y = 0.0;
                player.horiz_vel = Vec3::ZERO;
                player.move_flags = 0;
                player.autorun = false;
                player.airborne_since = None;
                player.fall_far = false;
                player.fall_start_y = pos.y;
                player.ride = None;
                // The granted modes (root, water-walk, feather-fall, hover) were granted to the
                // *previous* mover; the new one's arrive on its own `SMSG_FORCE_*`/mode packets.
                player.modes = Default::default();
            }
            player.pos = pos;
            player.active = true;
            if first {
                player.settling = true; // settle onto the initial ground once it loads (don't fall through)
                player.settle_since = time.elapsed_secs();
                player.settle_deadline = time.elapsed_secs() + SETTLE_TIMEOUT;
                player.world_stale = true; // nothing is streamed yet — see [`Player::world_stale`]
                                           // The streamed spawn pose carries the server's facing (the character's logout
                                           // orientation on a fresh login) — adopt it whole, camera seated behind, like
                                           // the reference. Zeroing here made every login face due north regardless.
                cam.yaw = yaw;
                cam.pitch = player.login_pitch.unwrap_or(LOGIN_PITCH);
            }
            // The camera is deliberately NOT re-seated on a mover change: it is already on the new
            // body (Mind Control sets `PLAYER_FARSIGHT` to the victim alongside the handoff, so
            // far sight moved it there before the reins arrived), and snapping its yaw would spin
            // the view for a change the player made happen on purpose.
            player.face_yaw = yaw;
            player.model_yaw = yaw;
            // Seed the facing-change detector with the adopted facing — the server gave it to us,
            // so the first controlled frame owes no `SET_FACING` (the real client sends nothing at
            // login until the mouse actually turns).
            player.last_facing = yaw.rem_euclid(std::f32::consts::TAU);
            // The avatar's `MovementState` (the animation selector's motion source) is NOT inserted
            // here: it rides the `SelfPlayer` tag (`net::apply::tag_self_player`), because a
            // cross-map worldport despawns and re-streams this entity while `player.active` stays
            // true — per-entity state attached only on this one-shot edge would be lost on transfer.
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

    // Forced speed changes (aura/mount/GM `.modify speed`): the net bridge already applied the new
    // value to our `UnitSpeeds`; the mandatory ack is ours to send, carrying our live wire state
    // (the server relocates us to it — the TeleportMessage pattern). In the controlled branch the
    // ack rides the movement stream's exact per-frame payload (`stream_self_movement`); detached or
    // pre-control, our honest wire state IS the parked pose (flags 0), so answer here directly.
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

/// **Apply one same-map teleport's snap** (`MSG_MOVE_TELEPORT_ACK` inbound) — everything but the
/// echo, which the caller sends. `cam_yaw` is the fly-cam's yaw, carried to the new facing.
fn snap_near_teleport(player: &mut Player, cam_yaw: &mut f32, t: &TeleportMessage, now: f32) {
    // A near teleport always arrives detached: vmangos takes this path only with no `m_transport`
    // (`Player::TeleportTo`, after `RemovePassenger`), and the reference's apply (`0x6186b0`)
    // re-parents to the packet's transport guid — zero here. A ride left standing let this same
    // frame's `ride::carry` compose the old deck pose back over the snap, and the airborne settle
    // kept it attached, so a hearth from a zeppelin or a lift landed us back on the deck.
    player.ride = None;
    player.pos = wow_to_bevy(t.position);
    player.face_yaw = t.orientation;
    *cam_yaw = t.orientation;
    // Stop any in-progress walk — server now sees us at the new spot.
    player.move_flags = 0;
    player.airborne_since = None; // a snap ends any in-progress jump arc (no phantom FALL_LAND)
    player.settling = true; // hold (gravity off) until the destination's ground/buildings load
    player.settle_since = now;
    player.settle_deadline = now + SETTLE_TIMEOUT;
    // A far same-map teleport relocates us over ground that has not streamed either — the tiles we
    // were standing on are still the resident ones. See [`Player::world_stale`].
    player.world_stale = true;
    // The relocation voids any in-progress self server-ride (the taxi flight-end teleport beats our
    // own spline end by ~latency): `drive_self_ride` takes this flag next frame and drops the ride
    // instead of mirroring the stale flight pose back over this snap (decision 0501 — the 4-yd
    // hover + full-6s settle at every taxi landing).
    player.ride_abort = true;
}

/// Merge a server-authored packet's `MOVEMENTFLAGS` into our own — the reference's masked merge
/// (`0x618c30 @0x618deb`: `new = old ^ ((old ^ wire) & 0x75a07dff)`), not an assignment. Pure, so
/// the omission that actually bites is pinned by test: `ON_TRANSPORT` sits **outside** the mask, so
/// a server-authored pose can relocate a rider but never board or deboard them. See
/// [`move_flags::SERVER_AUTHORED`].
pub(super) fn merge_server_flags(local: u32, wire: u32) -> u32 {
    move_flags::merge_server_authored(local, wire)
}

/// Apply one bare self-addressed `MSG_MOVE_*` — a pose the *server* wrote for our own mover, with
/// no handshake (decision 0725). `.go forward`/`up`/`relative`,
/// `.cheat fly`/`fixedz` and the movement anticheat's snap-back all land here.
///
/// **A hard snap, and nothing goes back.** The reference writes the wire pose into both its live
/// position cell and its integrator *base* (`0x7c6420`), which is what makes the snap persist —
/// the next frame integrates forward from the server's point instead of rubber-banding off it. For
/// us those are one cell ([`Player::pos`]), so the snap is the whole of it. It sends no ack and
/// opens no suppression window: our ordinary heartbeat then carries the server's own pose home,
/// which is precisely why `.go forward` sticks.
///
/// **Applied inline, not scheduled** — the one place this departs from the reference's shape, and
/// it is a deliberate divergence rather than a shortcut. The reference has a single move machine
/// and routes a self-addressed packet through the same replay chain as a remote's (decisions
/// 0601/0615). Measured against that chain (`net::motion::tests`'
/// `the_chain_paces_a_server_authored_self_move_and_would_hold_an_early_one`) it does **not** always
/// come back due: the first one fires at arrival, but once the chain has stamps to pace against, a
/// packet arriving ahead of the sender's cadence is deliberately *held* — bounded at +1000 ms.
///
/// That holding is the chain's whole purpose and it is right for a remote: replaying a mover on the
/// sender's own spacing is what stops a watched player stuttering between packets. It buys nothing
/// here. Our avatar is not something we extrapolate between packets — there is no motion to
/// de-jitter — so preserving the "cadence" between one GM command and the next would only delay a
/// correction to our own pose behind a queue the controller would have to yield to. We take the
/// snap at arrival. The trigger to revisit is a self-addressed *stream*: `Anticheat.Enable = 1`,
/// whose snap-backs can burst, is the one sender that would produce one.
// The transports query type is `control`'s own param shape passed through, like the caller's.
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

    // **Lift the granted mover MODES out of the merged word into typed state** (decision 0726).
    // `move_flags` is our last-streamed wire bookkeeping and is rebuilt from state every frame, so a
    // bit parked there alone would be gone before the mover ever read it; the modes live as fields,
    // the way `rooted` does. The reference needs no such step — it has one `[cmov+0x40]` that is both
    // the state and the wire word — and it has the matching apply anyway: the same inbound merge also
    // runs `0x61a1af → 0x61a230 → SetSwim`, the local unit's server echo included. This pair is
    // GM flight: `.cheat fly` sends SWIMMING + LEVITATING together.
    player.swimming = player.move_flags & move_flags::SWIMMING != 0;
    // …and the walk gait rides out on the same lift (decision 1752). `0x100` is inside the
    // `SERVER_AUTHORED` mask like the modes above, so a move the server authors for our mover
    // carries whatever walk bit *it* last saw — normally our own, echoed back from the
    // `m_movementInfo` our last packet refreshed, which makes this idempotent. Dropping the lift
    // instead would let one server-authored move silently clear the bit out of the word while the
    // latch stayed set, and the next frame would rebuild the word and re-announce it: a
    // SET_RUN_MODE / SET_WALK_MODE pair per teleport.
    player.walking = player.move_flags & move_flags::WALK_MODE != 0;
    player.modes.merge_from_wire(player.move_flags);
    // Neither mode touches `settling`: the settle release is the terrain streamer's, keyed on the
    // destination's residency in every mover mode alike (decision 0737). The pre-0737 special case
    // here (swim clears the hold) existed only because the old release was a walk-mover ground
    // probe a swimmer/flyer never reached.

    player.pos = wow_to_bevy(m.position);
    // Facing turns **rigidly** — aim, rendered body and camera all take the same delta (the
    // transport carry's idiom). The reference writes only the mover's own facing cells; hard-setting
    // the camera the way the teleport arm does would yank the view on every `.cheat fly` toggle,
    // whose heartbeat carries nothing but the server's slightly-stale copy of our own orientation.
    let dyaw = wrap_pi(m.orientation - player.face_yaw);
    player.face_yaw = wrap_pi(player.face_yaw + dyaw);
    player.model_yaw = wrap_pi(player.model_yaw + dyaw);
    cam.yaw += dyaw;
    player.mover_pitch = m.pitch;

    // Riding a deck: `ON_TRANSPORT` is outside the merge mask, so this packet did not deboard us —
    // it moved us **within** the platform frame. Re-anchor the local pose, then let the carry
    // recompose from it; without the re-anchor the next frame's carry reinstates the stale one and
    // undoes the snap.
    //
    // **Take the wire's own deck-local pose when the packet carried one** (decision 2026). The
    // subtraction below is only the server's answer while client and server agree about where the
    // boat is, and at a cross-map seam they provably do not: our path clock has not crossed yet, so
    // [`crate::transport::tick_transports`] is holding the boat's transform frozen at the *source*
    // continent's pose while this packet's `position` is a world position on the *destination*. The
    // difference is then not a deck offset but the distance between two continents — stable while
    // the boat stays frozen (the carry just reproduces the server's own position, so nothing looks
    // wrong), and a ~13 000 yd fling off the deck the instant the boat writes an honest pose again.
    // The server sent us the right number in the same packet; use it.
    if let Some(ride) = player.ride.as_mut() {
        if let Ok((boat, _, _)) = transports.get(ride.entity) {
            match m.transport.filter(|t| t.guid == ride.guid) {
                Some(t) => {
                    ride.local_pos = wow_to_bevy([t.pos.x, t.pos.y, t.pos.z]);
                    // World facing = local + boat yaw (the GetAbsoluteFacing law), so the carried
                    // aim stays right even when the boat's own heading is the frozen one.
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

    // The airborne arc. The reference gates the fall tail on the *merged* `FALLING` bit — a data
    // gate, not an identity one — seeding the arc from the wire when it is set (`0x7c6490`) and
    // running the landing reaction when it just cleared.
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
            // The arc is over by server decree (`NearLandTo` strips `JUMPING|FALLINGFAR` before it
            // sends). Ended **silently**: our landing report is a fall *height*, and the same packet
            // just moved the body an arbitrary distance, so there is no descent left to measure.
            player.airborne_since = None;
            player.fall_far = false;
            player.vel_y = 0.0;
        }
        (false, false) => {}
    }
    player.wedged = false;
    player.wedge_still = 0;
}

/// **The avatar went away with the session** — the whole resource goes back to the state the
/// process booted in (`player::setup` inserts `Player::default()`), so the next login re-takes
/// control from its own streamed `SelfPlayer` (possibly a different character on a different map
/// — the boot path) owing nothing to the session that ended. The entity itself is despawned by the
/// net drain the same frame these messages are written.
///
/// Two edges, one answer (decision 1262): a confirmed `/logout` (decision 0193), and a **lost**
/// session, which since 1262 takes the avatar too — there is no reconnect left for it to be the
/// puppet of. Missing the second edge would leave `Player.active` true over a despawned entity: a
/// controller driving nothing, which is the shape of the free camera this arc is about.
///
/// **Everything on this resource belonged to the mover that just ended, so all of it dies here**
/// (decision 1542). It used to clear a hand-picked six
/// fields, and the field it did not name was `modes`: `/logout` has vmangos root us for the
/// countdown (`MiscHandler.cpp` `SetRooted(true)`), we ack the grant, and the next session never
/// hears an unroot — the fresh server-side `Player` was never rooted, so there is nothing for it
/// to revoke. `modes.rooted` therefore survived into the new world and killed WASD for the rest of
/// the run. The list was the bug, not the missing line: the take-control edge above keeps its own
/// list for the same law (a mover *change* tears the old mover down — `SetActiveMover 0x6006e0`),
/// the two drifted apart, and only one of them ran here. So this end takes the resource whole —
/// a field added tomorrow is mover state by default, which is the safe direction.
///
/// It is also what the reference does, by construction rather than by list: the server's logout
/// confirm runs `ShutdownGame 0x491180` (`0x5aaeb0` → `0x401ee0` → `0x402039`, ~35 subsystem
/// shutdowns, the Lua VM replaced twice), so
/// the real client has no per-mover state left to carry across a character-select round trip. Ours
/// is a long-lived resource; this is where it pays that back.
pub(super) fn release_on_session_end(
    mut logouts: MessageReader<crate::net::LoggedOutMessage>,
    mut lost: MessageReader<crate::net::DisconnectedMessage>,
    mut player: ResMut<Player>,
) {
    // Both readers drain unconditionally — `|`, not `||`: a short-circuit would leave the other
    // message unread, and its cursor would carry it into the next frame.
    if logouts.read().next().is_some() | lost.read().any(|m| m.session_over) {
        // Nothing is carried across. Among what this clears that a named list kept missing: the
        // granted modes (B306's root, and water-walk/feather-fall/hover with it — vmangos re-sends
        // every one the new session really holds, `Player::SendInitialPacketsAfterAddToMap`
        // re-applies the aura family), the reins (`control_lost`, `foreign_mover`) which would
        // otherwise leave the next login driving a guid that no longer exists, the autorun latch,
        // and — as before — `active`, the movement flags, the fall/wedge state and the worldport
        // ack debt (decision 1340: an ack from the old transfer is rejected by a player who is
        // already in world, and the server force-acked it at logout anyway).
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

    /// A session that actually ran: in world, moving, with every mode the server can grant on our
    /// mover — and the reins in somebody else's hands. Every one of these was granted TO THE MOVER
    /// this session owned, which is the whole of why none of it may outlive it.
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

    /// **B306, at its own edge** (decision 1542). `/logout` has
    /// vmangos root the player for the countdown (`MiscHandler.cpp:329 SetRooted(true)`), which
    /// reaches us as the ack'd `SMSG_FORCE_MOVE_ROOT` above; the next login gets no unroot, because
    /// server-side the fresh `Player` was never rooted (`SendInitialPacketsBeforeAddToMap` re-sends
    /// the water-walk/feather-fall/hover aura family and roots only for a stun aura). So anything
    /// the ended session left on this resource is permanent, and `modes.rooted` is WASD.
    ///
    /// The assertion is deliberately against `Player::default()` whole rather than a list of
    /// fields: a list is what failed — this reset named six fields, the take-control edge's own
    /// teardown named nine, and only the second list named `modes`. A field added to `Player`
    /// tomorrow is covered by this test on the day it is added.
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

    /// The second edge (decision 1262) is the same answer: a lost session takes the avatar too, so
    /// it takes everything granted to it. Not a variant of the above — it is a different message
    /// on a different reader, and the `|` that drains both is load-bearing.
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

    /// **A teardown that is not the end must not wipe a live avatar.** Both cases that reach this
    /// system with `session_over: false` are ones where the body stays ours: the `/logout`'s own
    /// teardown disconnect (the roster relist *inside* one session — its `LoggedOutMessage` is the
    /// edge, above) and an unattended run's seamless reconnect (0065). Resetting on the mere
    /// arrival of a `DisconnectedMessage` would drop control from under a probe mid-run.
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

    /// The merge is not an assignment, and the bit that proves it is `ON_TRANSPORT`: it sits
    /// outside the reference's `0x75a07dff` mask, so a server-authored pose relocates a rider on
    /// the deck without ever boarding or deboarding them. Everything else we model is inside, and
    /// the wire owns it — including `FALLING`, whose clearing is how `.go forward` ends an arc.
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

        // The other direction: the wire claiming ON_TRANSPORT cannot board us either.
        assert_eq!(merge_server_flags(0, f::ON_TRANSPORT), 0);
    }

    /// **`.cheat fly` survives the merge intact** (decision 0726). vmangos's `Player::SetFly` sends
    /// `LEVITATING | SWIMMING | MOVED | FLYING`; the two we model must both be inside the mask, or
    /// GM flight arrives half-applied — SWIMMING without LEVITATING is a swimmer on dry land that
    /// the water decision clears on the next frame, and LEVITATING without SWIMMING is a walker
    /// whose swim latch has simply been frozen.
    #[test]
    fn the_fly_toggle_arrives_whole() {
        const SET_FLY: u32 = f::LEVITATING | f::SWIMMING | 0x0080_0000 | 0x0100_0000;
        let merged = merge_server_flags(f::FORWARD, SET_FLY);
        assert_eq!(merged & f::LEVITATING, f::LEVITATING);
        assert_eq!(merged & f::SWIMMING, f::SWIMMING);
        // …and `.cheat fly off` (flags 0) takes both away again, which is what lands us.
        assert_eq!(
            merge_server_flags(merged, 0) & (f::LEVITATING | f::SWIMMING),
            0
        );
    }

    /// Every flag benilla models except `ON_TRANSPORT` is inside the mask — a guard against the
    /// mask and our constants drifting apart as new bits get modelled.
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

    /// The victim's inversion trap, stated as a test: the packet that revokes control names **us**,
    /// so "the mover is now `mover`" would read it as *gaining* control of ourselves at the exact
    /// moment we lost it — and a mind-controlled player would keep walking, which is the reported
    /// bug (B211's second half).
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

    /// vmangos's real Mind Control sequences, in order, from both sides of the spell. The caster's
    /// *end* sequence is the one worth pinning: it restores us BEFORE releasing the victim, so a
    /// handler that keyed off "the last packet wins" would leave the caster unable to move.
    #[test]
    fn the_mind_control_sequences_classify_in_order() {
        // Caster, possession start: one grant naming the victim. (`Unit::UpdateControl`.)
        assert_eq!(
            control_verdict(VICTIM, true, Some(ME)),
            ControlVerdict::Granted(VICTIM)
        );
        // Caster, possession end: `(self, 1)` then `(victim, 0)` — restore first, release second.
        let caster_end = [
            control_verdict(ME, true, Some(ME)),
            control_verdict(VICTIM, false, Some(ME)),
        ];
        assert_eq!(
            caster_end,
            [ControlVerdict::Restored, ControlVerdict::Released(VICTIM)],
            "restore lands BEFORE the release; last-packet-wins would strand the caster"
        );

        // Victim's own client, from ITS point of view (self_guid == VICTIM): revoked at the start,
        // restored at the end. Both name the victim; only the byte differs.
        assert_eq!(
            control_verdict(VICTIM, false, Some(VICTIM)),
            ControlVerdict::Revoked
        );
        assert_eq!(
            control_verdict(VICTIM, true, Some(VICTIM)),
            ControlVerdict::Restored
        );
    }

    /// Before login names us, nothing can be about our body — and in particular a packet must not
    /// classify as `Revoked` and silently freeze the character the moment control starts.
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

    /// **A near teleport lands us where the server says, not back on the deck.** Hearthing off a
    /// zeppelin or a lift is a same-map teleport, and vmangos removes the passenger before it
    /// takes that path; the snap has to drop the ride too, or the same frame's platform carry
    /// recomposes the old deck pose over it.
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
