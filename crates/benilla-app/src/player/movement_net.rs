//! Outbound self-movement → the wire — the mirror of [`crate::net::motion`] (which integrates *remote*
//! movers). [`stream_self_movement`] diffs this frame's CMovement move-flags against last frame's and
//! emits a `MSG_MOVE_*` per movement-*axis* transition (start/stop forward-back, strafe, turn), the
//! jump/fall lifecycle (JUMP launch, FALL_LAND — a jumpless fall opens with nothing, decision 1464),
//! a periodic heartbeat while moving,
//! and a SET_FACING every frame the facing changes off the turn axis — each carrying the live
//! `MovementInfo` (decisions 0052 + 0053 + 0617). Split out of the controller: the wire stream is its
//! own concern.
//!
//! **Invariant — the wire mirrors the avatar's *actual* local motion** (decision 0056). vmangos relays
//! what we send verbatim and observers extrapolate it from the moveFlags, so any divergence strands them
//! on stale state: a flag we set but never clear is a *phantom* walk/spin, and an out-of-range value is
//! silently dropped before relay (vmangos rejects `|orientation| > 4π` in `VerifyMovementInfo`,
//! regardless of anticheat). Three rules keep us honest, all enforced here at the wire boundary:
//! - **Every outbound `orientation` is normalized into `[0, 2π)`** — `face_yaw`/`cam.yaw` are unbounded
//!   accumulators, but the real client always sends a normalized facing and the server's validity gate
//!   demands it.
//! - **When the controller stops driving locomotion, the mover is *parked*** — [`park_mover`] flushes a
//!   Stop and clears our flags on entering free-fly; the held frames of a post-teleport settle stream
//!   zeroed flags (so [`stream_self_movement`]'s own diff emits the Stop) — so observers never
//!   extrapolate motion that isn't happening locally.
//! - **The server's copy of our *position* may never go stale** — the reconcile at the bottom of
//!   [`stream_self_movement`] (decision 0907). A resting body whose resolver settles it a fraction
//!   of a millimetre after the packet that reported the rest used to keep that to itself, and
//!   vmangos — which compares positions with exact float equality — read the next packet's
//!   accumulated delta as movement and cancelled the cast in flight. Drift at rest is news; it goes
//!   out the frame it happens.

use benilla_assets::coords::bevy_to_wow;
use benilla_protocol::{JumpInfo, TransportPose};
use crossbeam_channel::Sender;

use crate::creature_anim::move_flags;
use crate::net::{ClientCommand, MoveKind};

use super::Player;

/// How often (s) we send a `MSG_MOVE_HEARTBEAT` while moving: the local-player send-deadline
/// `mgr+0x130` is armed to `clientTime + 500 ms` (`0x615b80`) — the wire report is the
/// per-transition broadcast plus this ~500 ms-paced heartbeat, independent of the 250 ms physics
/// substeps.
const HEARTBEAT_INTERVAL: f32 = 0.5;
/// The move-flag bits we put on the wire — the base directional / turn / walk set **plus `FALLING`**
/// (= `MOVEFLAG_JUMPING` 0x2000): we serialize the jump tail (`zspeed, cos, sin, xyspeed`) whenever it's
/// set, so observers replay our jump as a ballistic arc (decision 0053) — **`FALLING_FAR`**
/// (`MOVEFLAG_FALLINGFAR` 0x4000, latched mid-arc past the 1/9-yd descent): the real client's live
/// flags carry it and vmangos reads it (anticheat, PointMovementGenerator), so ours does too; it
/// changes no opcode (the axis differ below keys on direction bits only) and just rides whatever
/// packets the arc sends — **and `SWIMMING`** (0x200000): the swim-pitch tail is now serialized
/// symmetrically ([`MovementInfo`](benilla_protocol) — the decision-0052 swim follow-up), so setting
/// the flag no longer desyncs the server's parse; the controller supplies the live pitch alongside it.
const OUTBOUND_FLAG_MASK: u32 = move_flags::FORWARD
    | move_flags::BACKWARD
    | move_flags::STRAFE_LEFT
    | move_flags::STRAFE_RIGHT
    | move_flags::TURN_LEFT
    | move_flags::TURN_RIGHT
    | move_flags::WALK_MODE
    | move_flags::FALLING
    | move_flags::FALLING_FAR
    | move_flags::SWIMMING
    // …**and `LEVITATING`** (0x400, GM flight — decision 0726). Echoing it is not cosmetic: the
    // reference's packet builder reads the same `[cmov+0x40]` the server's flags merged into, so a
    // real client sends it straight back, and vmangos refreshes its `m_movementInfo` from whatever
    // we report. Drop it from our stream and the server's copy loses the bit — then the next
    // server-authored move (a `.go forward`, a forced speed change) echoes a LEVITATING-less word
    // back at us, our merge clears the mode, and we fall out of the sky mid-flight.
    | move_flags::LEVITATING
    // …**and the rest of the granted movement-mode family** (decision 0866) — root, water-walk,
    // safe-fall, hover: the same reasoning one step stronger. These four are the *ack'd* modes, so
    // the server holds an explicit record of granting each one and notices our stream disagreeing.
    // `MOVEFLAG_ROOT` especially: vmangos re-adds it to anything it writes for a rooted mover
    // (`MovementHandler.cpp:1064`), and its absence from an apply-ack is a kick.
    | move_flags::ROOT
    | move_flags::WATER_WALKING
    | move_flags::SAFE_FALL
    | move_flags::HOVER
    | move_flags::ON_TRANSPORT;

/// The bits that mean **the body is genuinely in motion**, and so that its position is expected to
/// change every frame: the direction bits, the airborne arc, swimming, and riding a transport. The
/// position reconcile at the bottom of [`stream_self_movement`] fires only when NONE of them is set
/// — at rest, a position change is news; in motion it is the whole point of the transition +
/// heartbeat stream (0052/0053), which already carries it. Turning in place is deliberately absent:
/// a keyboard turn moves nothing, so a drift under it is still news.
const IN_MOTION: u32 = move_flags::ANY_MOVE
    | move_flags::FALLING
    | move_flags::FALLING_FAR
    | move_flags::SWIMMING
    | move_flags::ON_TRANSPORT;

/// What this frame's **skipped-time report** ([`stream_skipped_time`]) needs: how long the frame
/// was, whether the mover integrated it, and which mover to name.
pub(super) struct SkipClock {
    /// This frame's length in seconds — the budget the mover either integrated or did not.
    pub(super) dt: f32,
    /// **The mover held instead of stepping this frame** ([`super::Player::settling`]): the world
    /// under us has not streamed in, so nothing integrated `dt`.
    pub(super) held: bool,
    /// The mover's own guid — a possessed unit's while we hold its reins, else our own body's.
    /// `None` before the server has named one, which is also before anything could be skipped.
    pub(super) mover: Option<u64>,
}

/// This frame's **arc edges** — the airborne lifecycle as the send law reads it. One struct rather
/// than four adjacent bools in the argument list, where a miscount is silent and the symptom is a
/// wrong opcode on the wire.
pub(super) struct ArcEdges {
    /// A take-off launched this frame — the arc opened.
    pub(super) jumped: bool,
    /// …and it came from the **wire**, not the key: a knockback, or the jump a `SetHover(true)`
    /// grant owes ([`super::Player::hover_launch`]). Suppresses `MSG_MOVE_JUMP`, which is the
    /// *keyboard's* packet and nothing else's: `0x615ed1`, in the move-command drain's jump arm, is
    /// its one live emission site image-wide, and neither wire door passes through it — the hover
    /// handler `0x61a620` sends nothing at all, and the knockback's own arm of that same drain
    /// pushes `0xf0`, its ack. (Decision 1702; this corrects 1620, which streamed a JUMP for the
    /// hover launch.)
    pub(super) wire_launch: bool,
    /// The standstill air nudge fired ([`super::mover::step`]) — the one mid-air press that really
    /// moves us, and so the one that breaks the airborne send silence (decision 0627).
    pub(super) air_nudged: bool,
    /// The arc ended this frame → `MSG_MOVE_FALL_LAND`.
    pub(super) landed: bool,
    /// ms since take-off — the caller's snapshot, because the landing frame's arc bookkeeping has
    /// already cleared `airborne_since` and the FALL_LAND must still report the accumulated time.
    pub(super) fall_time: u32,
}

/// Stream this frame's movement to the server the way the real client does: a `MSG_MOVE_*` per movement-
/// *axis* transition (start/stop forward-back, strafe, turn), a JUMP on take-off, a SET_FACING every
/// frame the facing changes off the turn axis, and a HEARTBEAT every ~500 ms while moving — each
/// carrying the current `MovementInfo`:
/// the move-state-change broadcaster `0x61a820` selects the wire opcode *from the flag delta*
/// (`0x619f00`), and the flag report is exactly "per-transition broadcast + ~500 ms heartbeat" — with
/// the *facing* report its own independent emitter alongside it (decision 0617: in the 1.12.1 sniff
/// SET_FACING outnumbers every other client-sent movement opcode combined, moving or standing).
/// vmangos relays it all to nearby players, who extrapolate from the flags — how they see us walk/turn/
/// strafe. (We claimed the mover with CMSG_SET_ACTIVE_MOVER at login.) **Airborne is its own send law**
/// (VERIFIED, vanilla-sniffs `dwarf_rogue_dun_morogh`): the fwd/back/strafe **transitions** go silent
/// while FALLING — the live flag state rides the packets that do go out — so a jump is JUMP →
/// \[heartbeats/turn/facing\] → FALL_LAND, with the landing flags telling observers what the keys say
/// *now*. The one press that breaks the transition silence is the standstill air nudge (`air_nudged`,
/// decision 0627): the case the reference's own deferral excludes, and the only mid-air input that
/// actually moves us. **The periodic heartbeat is NOT part of that silence** and a jumpless fall opens
/// with no packet at all — both corrected against the bytes and the same capture by decision 1464,
/// which retired this module's two remaining 0053-era inventions. Sends are fire-and-forget; a down thread no-ops. Mutates `player`'s last-sent flags/facing/
/// heartbeat so next frame can diff against them.
// Eight, down from twelve: the arc edges are one struct now (`ArcEdges`). The rest are distinct
// types the compiler can tell apart, so the remaining count is noise rather than a miscount risk.
pub(super) fn stream_self_movement(
    sender: &Sender<ClientCommand>,
    player: &mut Player,
    move_flags_now: u32,
    swim_pitch: f32,
    arc: ArcEdges,
    now: f32,
    speed_acks: &[crate::net::SpeedChangeMessage],
    knock_ack: Option<super::state::PendingKnockback>,
    transport: Option<TransportPose>,
    skip: SkipClock,
) {
    let ArcEdges {
        jumped,
        wire_launch,
        air_nudged,
        landed,
        fall_time,
    } = arc;
    stream_skipped_time(sender, player, skip);
    let wow_pos = bevy_to_wow(player.pos);
    // Normalize the facing into [0, 2π) before it goes on the wire. `face_yaw` is an unbounded
    // accumulator (mouse-look and A/D turning keep growing it), but the real client always sends a
    // normalized orientation, and vmangos's `VerifyMovementInfo` → `IsValidMapCoord` rejects any
    // movement packet with `|o| > 4π`. Past that bound every packet — including the Stop/StopTurn that
    // ends a run or turn — is silently dropped, stranding observers on the last-relayed flags (a
    // phantom spin or run-off that only clears once we turn back in range and emit a fresh transition).
    let facing = player.face_yaw.rem_euclid(std::f32::consts::TAU);
    let wire_flags = move_flags_now & OUTBOUND_FLAG_MASK;
    // The ballistic launch tail, sent on every airborne packet (decision 0053): `zspeed` is the
    // constant take-off vertical speed, the horizontal is the frozen `horiz_vel` mapped to world XY,
    // and `xyspeed` its magnitude. Present iff JUMPING is in `wire_flags` — the serializer gates the
    // tail on the same bit, so the two never disagree. `fall_time` (ms since take-off) is the
    // caller's snapshot: on the landing frame the arc bookkeeping has already cleared
    // `airborne_since`, but the FALL_LAND must still report the accumulated fall time — vmangos
    // `Player::HandleFall` deals fall damage only when the land packet's fallTime ≥ 1229 ms.
    let wire_jump = (wire_flags & move_flags::FALLING != 0).then(|| {
        let v = bevy_to_wow(player.horiz_vel); // WoW velocity [vx, vy, 0] (the transform is linear)
        let xy = v[0].hypot(v[1]);
        let (cos_angle, sin_angle) = if xy > 1.0e-4 {
            (v[0] / xy, v[1] / xy)
        } else {
            (facing.cos(), facing.sin()) // a standing jump: direction is moot (xy 0); use the facing
        };
        JumpInfo {
            // The wire zspeed is DOWN-positive (the real client sends -7.955547 for a rising jump —
            // VERIFIED, vanilla-sniffs), so negate our +up `jump_zspeed`. A real-client observer reads
            // the up-speed as `-zspeed`; sending +up here would make them see us sink (decision 0054).
            zspeed: -player.jump_zspeed,
            cos_angle,
            sin_angle,
            xy_speed: xy,
        }
    });
    // Forced speed changes owe their ack inside the server's ~4 s window: echo kind/guid/counter/
    // speed with EXACTLY this frame's wire payload — the same flags/pose/tails a Move packet would
    // carry, so the server's relocation and anticheat position tests see our honest live state.
    for ack in speed_acks {
        let _ = sender.send(ClientCommand::ForceSpeedAck {
            kind: ack.kind,
            guid: ack.guid,
            counter: ack.counter,
            speed: ack.speed,
            flags: wire_flags,
            pos: wow_pos,
            orientation: facing,
            pitch: swim_pitch,
            fall_time,
            jump: wire_jump,
            transport,
        });
        // The ack's `MovementInfo` relocates the mover server-side exactly like a Move packet, so
        // it is also a position report — the reconcile below must not re-send what it just told.
        player.last_pos = wow_pos;
    }

    // **The knockback ack** (decision 1702) — owed only if the mover actually took off this frame,
    // and carrying the pose it took off from. `apply → send, inside one call` is the reference's own
    // order (`0x61624d call 0x6179c0` → `0x616261 push 0xf0`), which is why this sits here and not
    // back at the opcode: everything on the wire must describe the arc we are already flying.
    //
    // `launch` goes out verbatim rather than re-derived from `wire_jump` below, though the two agree
    // exactly for every real knockback (the mover seeded its velocity from this quad, and the round
    // trip through the coordinate basis is sign flips and a hypot). The reference copies its own
    // live fields as plain dwords (`+0xa0`→`+0x3c`, `+0x68`→`+0x40`, `+0x6c`→`+0x44`,
    // `+0x84`→`+0x48`) so its echo is bit-identical to what arrived, and vmangos's `0.01` tolerance
    // is never exercised. Sending the quad itself is how we keep that true in the one case where a
    // re-derivation could not: `xy_speed == 0`, where a direction has no length to recover and the
    // tail below substitutes the facing.
    if let Some(k) = knock_ack {
        let _ = sender.send(ClientCommand::KnockBackAck {
            guid: k.guid,
            counter: k.counter,
            launch: k.launch,
            flags: wire_flags,
            pos: wow_pos,
            orientation: facing,
            transport,
        });
        // Like a speed ack, this relocates us server-side — so it is a position report too.
        player.last_pos = wow_pos;
    }
    let prev = player.move_flags;
    let added = wire_flags & !prev;
    let removed = prev & !wire_flags;
    let mut sent = false;
    macro_rules! send_move {
        ($kind:expr) => {{
            if *crate::net::CAST_TRACE {
                bevy::log::info!(
                    "cast-trace: SEND move {:?} flags={:#x} pos=[{:.3},{:.3},{:.3}] o={:.3}",
                    $kind,
                    wire_flags,
                    wow_pos[0],
                    wow_pos[1],
                    wow_pos[2],
                    facing
                );
            }
            super::move_trace::sent($kind, wire_flags, facing, wow_pos);
            let _ = sender.send(ClientCommand::Move {
                kind: $kind,
                flags: wire_flags,
                pos: wow_pos,
                orientation: facing,
                // The serializer writes the pitch iff SWIMMING is in `wire_flags`, so a non-swimming
                // packet ignores this; while swimming it's the live swim heading pitch.
                pitch: swim_pitch,
                fall_time,
                jump: wire_jump,
                // Written iff ON_TRANSPORT is in `wire_flags` — the rider's boat-local pose.
                transport,
            });
            sent = true;
            // What the server now believes, for the reconcile below — every packet in this family
            // carries `wow_pos`, and vmangos relocates the mover to it (`HandleMoverRelocation`).
            player.last_pos = wow_pos;
        }};
    }
    const FB: u32 = move_flags::FORWARD | move_flags::BACKWARD;
    const STRAFE: u32 = move_flags::STRAFE_LEFT | move_flags::STRAFE_RIGHT;
    const TURN: u32 = move_flags::TURN_LEFT | move_flags::TURN_RIGHT;
    // Airborne, the fwd/back/strafe axes go SILENT on the wire while the flag state stays live —
    // VERIFIED (vanilla-sniffs `dwarf_rogue_dun_morogh`): a strafe pressed mid-air emits no
    // START_STRAFE yet the landing FALL_LAND carries `(Forward, StrafeLeft)`; an S→W swap mid-air
    // emits no transition yet lands as `(Forward)`. The keys don't move an airborne avatar (the
    // arc's momentum froze at takeoff), so their transitions aren't motion changes — the live bits
    // simply ride every packet that does go out. The TURN axis is the exception (below): turning
    // genuinely works mid-air, and the sniff shows START_TURN_RIGHT/STOP_TURN with `Falling` set.
    //
    // **The standstill air nudge is the other exception** (decision 0627), and it is the *same* rule,
    // not a carve-out: the wire mirrors actual motion (0056). The reference's airborne silence is a
    // real flags-side mechanism — while FALLING, `StartMove 0x7c6ae0` defers a new press into an inert
    // latch (`0x20000`/`0x40000`) instead of flipping the direction bit, "**unless nothing is
    // currently moving**" — and the broadcaster
    // `0x61a820` picks its opcode from the *flag delta*, so a deferred press produces no delta and
    // no packet. In the one non-deferred case the bit really flips, so the transition really
    // broadcasts. That case is exactly our nudge (`mover::step`: airborne, nothing moving, a
    // direction pressed — the one input that genuinely re-seeds the arc's horizontal velocity), and
    // it must go out: the packet carries the fresh `JumpInfo` tail, which is the only way an observer
    // — integrating our arc from a JUMP that said `xy_speed = 0` — can ever learn we started moving
    // before the FALL_LAND lands.
    let falling = wire_flags & move_flags::FALLING != 0;
    // The airborne lifecycle, and it is exactly two opcodes: a JUMP launch (carrying the ballistic
    // tail) and a FALL_LAND that closes the arc. A mid-air key release updated the flag state
    // silently (above), so the landing frame's diff has no direction edge left and the FALL_LAND
    // goes out alone — the real client sends no trailing Stop after it (sniff-verified).
    //
    // **A fall that had no jump opens with NOTHING** — decision 1464, and the third of 0053's
    // inventions to be retired by the bytes. We used to push an immediate heartbeat here "so
    // observers start the arc promptly"; the reference refutes it three ways: the move-state
    // broadcaster `0x61a820` gates every send on the *locomotion nibble* (`61a99d test al,0xf`)
    // and FALLING/`0x2000` lives in `ah`, so a flags change that is only the fall bit never
    // broadcasts at all; every non-jump `StartFalling 0x7c61f0` site seeds `+0xa0 = 0.0f` while
    // only `CMovement::Jump 0x7c6230` seeds the launch speed; and `MSG_MOVE_JUMP` has exactly one
    // emission site image-wide, the move-command drain's jump arm. The 1.12.1 sniff agrees — three
    // separate non-jump falls (a login settle, a forced unroot, and a post-near-teleport arrival)
    // each show only the terminal `FALL_LAND`. The law is: **echo → heartbeats every 500 ms while
    // the fall lasts → FALL_LAND**, and the parenthesis is empty for any fall shorter than the
    // deadline.
    //
    // It was not a harmless invention. It is the only packet in the client that could put
    // `MOVEFLAG_JUMPING` on the wire for a jumpless arc, and vmangos reads any reported moving flag
    // as movement — `Unit::HandleInterruptsOnMovement` answers it with `SetStandState(STAND)`,
    // with a source comment saying the test is there *because* sitting on a chair teleports you.
    // A chair seats you inside the collider we bake for it, where the mover's down-shapecast can
    // report nothing and the body reads airborne while moving `dy=+0.000` for six frames; this
    // packet then told the server we were falling, and stood us up (B79, decision 1458).
    // **…and a WIRE-driven take-off sends no JUMP at all** (decision 1702). Same law read the other
    // way — `MSG_MOVE_JUMP` is the keyboard's packet ([`ArcEdges::wire_launch`] carries the bytes).
    // For a knockback the server agrees from its own side: `CHEAT_TYPE_OVERSPEED_JUMP` is one of the
    // few vmangos checks with **no** knockback exemption (`MovementAnticheat.cpp:650`), and a
    // knockback's horizontal speed is over run speed by design — so a JUMP here would be a logged
    // cheat on every single one.
    if jumped && !wire_launch {
        send_move!(MoveKind::Jump);
    } else if landed {
        send_move!(MoveKind::FallLand);
    }
    // Swim transition: the real client announces entering/leaving the water with a dedicated
    // MSG_MOVE_START_SWIM (0xca) / STOP_SWIM (0xcb) the frame the `SWIMMING` bit flips (the local
    // `0x6030c0` decision enqueues it), rather than letting the flag ride the next heartbeat.
    // Airborne and swimming are mutually exclusive, so this never races the arc lifecycle above.
    if added & move_flags::SWIMMING != 0 {
        send_move!(MoveKind::StartSwim);
    } else if removed & move_flags::SWIMMING != 0 {
        send_move!(MoveKind::StopSwim);
    }
    // The gait toggle: `MSG_MOVE_SET_WALK_MODE` (0xc3) entering walk, `MSG_MOVE_SET_RUN_MODE`
    // (0xc2) leaving it — the same "a dedicated opcode the frame the bit flips" shape as the swim
    // pair above, and for the same reason. It sits OUTSIDE the `if !falling || air_nudged` axis
    // block on purpose: a mode flip is not an axis transition, so the airborne silence (which
    // exists because mid-air direction presses are deferred into an inert latch) does not apply —
    // the reference's `ToggleRun` refuses nothing about being airborne and enqueues its event
    // regardless. And it must fire while standing perfectly still, which is the whole reason the
    // reference gives the toggle its own enqueue rather than letting the move-state broadcaster
    // find it: that broadcaster gates on the locomotion nibble (`0x61a99d test al,0xf`) and would
    // drop it. This differ has no such gate (decision 1752).
    if added & move_flags::WALK_MODE != 0 {
        send_move!(MoveKind::SetWalkMode);
    } else if removed & move_flags::WALK_MODE != 0 {
        send_move!(MoveKind::SetRunMode);
    }
    // Forward/back axis — silent while airborne (the flag state rides the next packet instead),
    // except on the nudge frame. The **stop** arms stay silent while falling no matter what: a
    // mid-air release is the deferred-latch case, always, and the nudge frame is a press by
    // definition — so this only keeps a same-frame release on the *other* axis from slipping out.
    if !falling || air_nudged {
        if added & move_flags::FORWARD != 0 {
            send_move!(MoveKind::StartForward);
        } else if added & move_flags::BACKWARD != 0 {
            send_move!(MoveKind::StartBackward);
        } else if !falling && removed & FB != 0 && wire_flags & FB == 0 {
            send_move!(MoveKind::Stop);
        }
        // Strafe axis — same airborne silence, same one exception.
        if added & move_flags::STRAFE_LEFT != 0 {
            send_move!(MoveKind::StartStrafeLeft);
        } else if added & move_flags::STRAFE_RIGHT != 0 {
            send_move!(MoveKind::StartStrafeRight);
        } else if !falling && removed & STRAFE != 0 && wire_flags & STRAFE == 0 {
            send_move!(MoveKind::StopStrafe);
        }
    }
    // Turn axis (keyboard A/D when not mouse-looking).
    if added & move_flags::TURN_LEFT != 0 {
        send_move!(MoveKind::StartTurnLeft);
    } else if added & move_flags::TURN_RIGHT != 0 {
        send_move!(MoveKind::StartTurnRight);
    } else if removed & TURN != 0 && wire_flags & TURN == 0 {
        send_move!(MoveKind::StopTurn);
    }
    // The facing report: one `MSG_MOVE_SET_FACING` per frame in which the facing actually changed —
    // **the single biggest thing our wire stream was missing** (decision 0617). **VERIFIED** against the
    // real 1.12.1.5875 sniff (`dwarf_rogue_dun_morogh`): 179 of its 336 client-sent movement packets are
    // SET_FACING — more than every other movement opcode combined — streamed at *frame* cadence (median
    // 41 ms between them, p25 23 ms, minimum 17 ms) and, decisively, **while moving**: 116 of the 179
    // carry a direction bit (`Forward` ×68, `StrafeRight` ×12, `Forward+StrafeRight` ×9, `Backward` ×12,
    // `Forward+Falling` ×4, …). There is no rate limit and no angular epsilon — `0x617100`
    // (SetFacing-then-send) reports whenever `0x617170`'s **exact-equality** change detector says the
    // facing differs at all, so a frame that didn't move the mouse sends nothing and a frame that did
    // sends one packet. (Our own `face_yaw` is likewise only written by real input, so the exact
    // comparison neither floods nor misses.)
    //
    // The one exclusion is the **turn axis**: not a single SET_FACING in the capture carries `TURN_LEFT`
    // or `TURN_RIGHT`. A keyboard turn is already fully described by its flag — observers rotate the
    // mover at the turn rate for as long as it's set — so the facing needs no separate carrier, and the
    // STOP_TURN closes the arc with the final angle.
    //
    // Not gated on `sent`: the capture repeatedly shows a SET_FACING sharing a millisecond with a
    // transition (`+1563` SET_FACING & STOP_STRAFE, `+3009` SET_FACING & STOP, `+4971` SET_FACING &
    // START_STRAFE_RIGHT). They are two independent emitters in the reference — the input-phase facing
    // report and the move-state broadcaster — not one prioritized channel.
    if wire_flags & TURN == 0 && facing != player.last_facing {
        send_move!(MoveKind::SetFacing);
    }
    // Board/deboard: the ON_TRANSPORT flip has no axis opcode of its own, so if nothing else went
    // out this frame, a heartbeat announces it promptly — the server learns the new frame (and its
    // local-pose tail, or its absence) the frame it changes rather than on the next natural packet.
    if !sent && (added | removed) & move_flags::ON_TRANSPORT != 0 {
        send_move!(MoveKind::Heartbeat);
    }
    // Heartbeat keeps a moving/turning mover's position + facing flowing between transitions. While
    // riding, ON_TRANSPORT alone keeps this stream alive — the deck carries us, so our world pose
    // really is changing (decision 0056: the wire mirrors actual motion) and observers on reference
    // clients keep a fresh compose anchor.
    //
    // **And it runs while FALLING too** (decision 1464). This arm used to carry `&& !falling`,
    // defended as "the real client sends a normal-length jump with no mid-air packet at all
    // (sniff-verified)" — which the same 1.12.1 capture refutes on both halves.
    // `MSG_MOVE_JUMP` is the **44-byte** form (the jump quad is present, `vz = -7.955547`, matching
    // the `.text` constant `0xc0fe93d8` bit for bit), and mid-air packets are routine: heartbeats
    // and a mid-air SET_FACING, all 44 B. The "untraced trigger" behind the sniff's sparse mid-air
    // sends is traced now, and it is *this deadline*: `[mgr+0x130]` is re-armed to `now + 500 ms`
    // after **every** outbound packet (`0x600a30` → `0x615b80`), which is why one 892 ms jump shows
    // no heartbeat at all — the player's own SET_FACING at `fallTime 460` pushed the deadline out
    // to 960. Our `last_heartbeat` is already stamped on every send below, so dropping the gate
    // reproduces the reference's cadence exactly rather than approximating it.
    //
    // This does not re-open B79: a chair arrival's stall is ~100 ms and the arrival's own reconcile
    // packet re-arms the deadline, so nothing fires inside it. What FALLING must never do is open
    // an arc with a packet of its own — that is the arm above, and it is gone.
    if !sent && wire_flags != 0 && now - player.last_heartbeat >= HEARTBEAT_INTERVAL {
        send_move!(MoveKind::Heartbeat);
    }
    // ── The position reconcile (decision 0907) ── **the server's copy of where we are may never go
    // stale.** Our resolver settles a body that is already at rest by a fraction of a millimetre
    // *after* the packet that reported the rest: a landing reports the touchdown pose and the next
    // frame's step-down snap takes 2e-5 yd off it; a login/teleport lands on a server-authored
    // position our own collision resolves a hair differently. Until 0617 nothing carried that
    // difference, and while standing still nothing else goes out — so the client and the server
    // silently held two different positions, and the next packet of ANY kind delivered the whole
    // accumulated delta at once.
    //
    // vmangos reads that delta as movement, on an **exact float compare**: `Player::SetPosition`
    // sets `positionChanged = old_x != x || old_y != y || old_z != z` and hands it to
    // `Unit::HandleInterruptsOnMovement`, which runs `InterruptSpellsWithInterruptFlags(
    // SPELL_INTERRUPT_FLAG_MOVEMENT)`. No epsilon: 2e-5 yd cancels a cast exactly as a yard would.
    // (Verified in the deployed server's source, `Objects/Player.cpp` + `Objects/Unit.cpp`; the
    // 0.5-yd tolerance elsewhere in `Spell::update` is a different, later test.)
    //
    // The observable was the director's: right-dragging to look around killed a cast. Mouse-look is
    // the one thing that puts packets on the wire while standing still (SET_FACING at frame
    // cadence), so it was the messenger — the first one after a run, a jump, or a login carried a
    // stale-position "you moved" into the middle of the cast. Reporting the drift when it *happens*
    // — at rest, so once per settle rather than per frame — keeps the two copies identical, and a
    // cast then dies to real movement only.
    if !sent && wire_flags & IN_MOTION == 0 && wow_pos != player.last_pos {
        send_move!(MoveKind::Heartbeat);
    }
    if sent {
        player.last_heartbeat = now;
    }
    // The change detector's reference is the **previous frame's** facing, not the last one we reported:
    // the reference compares the new facing against the unit's live facing cell, which its setter
    // updates on every change whether or not a packet went out. So the turn-axis frames above, which
    // deliberately send nothing, still leave no catch-up packet behind when the key releases.
    player.last_facing = facing;
    player.move_flags = wire_flags;
}

/// Park our mover on the wire: flush a single `MSG_MOVE_STOP` (flags cleared) so the server — and the
/// observers extrapolating from it — drop whatever locomotion flags we last reported, then zero our
/// bookkeeping. Called when the controller stops driving the avatar with stale flags still live on the
/// **`CMSG_MOVE_TIME_SKIPPED` — the milliseconds the mover advanced through without integrating**
/// (decision 1935). Accumulate while the mover holds; report the total once, on the release edge.
///
/// **The law is "time the movement simulation advanced through without integrating"** — not "a
/// long frame", which is what this packet is usually described as. The reference has **four**
/// emission sites feeding one builder `0x600be0`, and the long-frame one (`0x616642 cmp esi,0xfa`
/// → send `dt − 250`) is only one of them and not the common one. The two that dominate real
/// traffic are the **no-geometry** pair: the resolve entry's swept query returning nothing
/// (`0x634154`) and the fall integrator's TOI kernel finding no contact (`0x635742`), each
/// reporting the substep budget they could not spend. Across 79 sniffs of the reference,
/// 1277 of 1278 packets carry `lag ≤ 250` — the clamped substep budget those two draw from —
/// and an exact-build 36-minute session sent **three**, at world entry and after a teleport.
///
/// **benilla's no-geometry state is the settle hold**, and that is what this reports: the frames
/// after a login, teleport or worldport where the destination's terrain and building colliders
/// have not streamed in, so the mover freezes rather than falling through a city that isn't there
/// (decision 0737). Same cause, same quantity, same moments the reference's own captures show it.
///
/// **The coalescing is ours, deliberately.** The reference sends once per firing, which for it is
/// once per skipped substep — and its no-geometry window is a substep or two wide, because it
/// loads the world before it places you. Ours is a live stream that can hold for seconds, so
/// per-frame parity would put hundreds of packets on the wire where the reference puts one. The
/// *quantity* is what the wire means and the quantity is preserved whole; the packet count is
/// kept at the reference's. What is NOT copied is the send's `[mgr+0x130] += lag` side effect
/// (the 500 ms heartbeat deadline pushed out by the skipped time): that deadline is only ever read
/// while moving, and a held body streams zeroed flags with its mover parked, so there is no
/// heartbeat inside a settle for it to delay. Applying a multi-second delay to the first heartbeat
/// *after* one would be our invention, not the reference's.
///
/// The other two sites are deliberately unbuilt. The long-frame one has no meaning here: benilla
/// has no 250 ms substep clamp — it integrates the whole frame — so a long frame is time we
/// *spent*, not time we skipped, and reporting it would be a lie about our own simulation. The
/// `[CMovement+0x40] & 0x8000000` free-advance bit's own setter is not modelled at all, and need
/// not be: a client implementing the other three sites matches every packet in the shipped
/// captures.
fn stream_skipped_time(sender: &Sender<ClientCommand>, player: &mut Player, skip: SkipClock) {
    if skip.held {
        player.skipped_ms += skip.dt * 1000.0;
        return;
    }
    // The release edge: report the whole hold at once, then disarm. Sub-millisecond residue is
    // dropped rather than carried — the wire field is whole milliseconds and a `0` lag is a
    // packet that says nothing.
    let lag_ms = player.skipped_ms as u32;
    player.skipped_ms = 0.0;
    if lag_ms == 0 {
        return;
    }
    let Some(guid) = skip.mover else {
        return;
    };
    // The reference's `0x600be0` refuses unless the named mover IS the active mover
    // (`0x600bec`/`0x600bfb` against `[0xc4da98]`/`[0xc4da9c]`); ours is the same statement, made
    // by naming the mover the controller is driving and nothing else.
    super::move_trace::skipped_time(guid, lag_ms);
    let _ = sender.send(ClientCommand::MoveTimeSkipped { guid, lag_ms });
}

/// wire — entering free-fly (`F`), where [`stream_self_movement`] no longer runs each frame, so nothing
/// else would ever clear them and observers would extrapolate a phantom walk/spin until we re-attach.
/// **Idempotent**: a no-op once we've already reported stopped, so it's safe to call every free-fly
/// frame. The frozen pose + `[0, 2π)`-normalized facing follow the module invariant.
pub(super) fn park_mover(sender: &Sender<ClientCommand>, player: &mut Player) {
    if player.move_flags == 0 {
        return;
    }
    let facing = player.face_yaw.rem_euclid(std::f32::consts::TAU);
    let pos = bevy_to_wow(player.pos);
    player.last_pos = pos; // the park is a position report too (decision 0907's reconcile)
                           // Traced like every other outbound move: the park is a real `MSG_MOVE_STOP` on the wire, and
                           // leaving it off `snd` made the trace lie by omission at exactly the edges it was wanted for —
                           // the handover, the free-fly detach, the moment control is taken (decision 1281).
    super::move_trace::sent(MoveKind::Stop, 0, facing, pos);
    let _ = sender.send(ClientCommand::Move {
        kind: MoveKind::Stop,
        flags: 0,
        pos,
        orientation: facing,
        pitch: 0.0, // flags cleared → not swimming → no pitch tail written
        fall_time: 0,
        jump: None,
        transport: None, // flags cleared → no transport tail written
    });
    player.move_flags = 0;
    player.last_facing = facing;
}

/// The rider's boat-local pose for the wire's `ON_TRANSPORT` tail, or `None` off a deck.
/// `bevy_to_wow` is a pure basis rotation, so the boat-local Bevy vector converts directly, and the
/// local orientation is `face_yaw − boat_yaw` (the GetAbsoluteFacing law in reverse), normalized
/// like any wire orientation.
pub(super) fn wire_transport(player: &Player) -> Option<TransportPose> {
    player.ride.as_ref().map(|r| {
        let local = bevy_to_wow(r.local_pos);
        TransportPose {
            guid: r.guid,
            pos: benilla_protocol::wire::Vector3d {
                x: local[0],
                y: local[1],
                z: local[2],
            },
            orientation: (player.face_yaw - r.boat_yaw).rem_euclid(std::f32::consts::TAU),
        }
    })
}

/// **The forced-speed acks owed on a frame the controller does not drive** — a server spline
/// (`server_riding`), a fear (`control_lost`) or a mover hand-off (`reseat`). Those frames return
/// before [`stream_self_movement`], which is where a controlled frame's acks go out, so until this
/// the change was applied locally and never answered: vmangos holds an unacked change for
/// `PendingAckResponseTime` (4 s) before enforcing it, counts it in `OnFailedToAckChange` (a kick
/// under the default anticheat penalty) and blocks the graveyard repop while one is pending. The
/// reference acks from its per-mover drain whoever is driving (`0x616142`/`0x61812d`).
///
/// The payload is the honest state we last reported: our streamed flags (a ride's deliberate
/// FORWARD included) with the transport tail when we are on a deck, minus the airborne pair — no
/// arc is being integrated on these frames, so there is no jump tail to send with them.
pub(super) fn ack_speeds_undriven(
    sender: &Sender<ClientCommand>,
    player: &Player,
    acks: &[crate::net::SpeedChangeMessage],
) {
    let transport = wire_transport(player);
    let mut flags =
        player.move_flags & OUTBOUND_FLAG_MASK & !(move_flags::FALLING | move_flags::FALLING_FAR);
    if transport.is_none() {
        flags &= !move_flags::ON_TRANSPORT; // flag and tail travel together
    }
    for ack in acks {
        let _ = sender.send(ClientCommand::ForceSpeedAck {
            kind: ack.kind,
            guid: ack.guid,
            counter: ack.counter,
            speed: ack.speed,
            flags,
            pos: bevy_to_wow(player.pos),
            orientation: player.face_yaw.rem_euclid(std::f32::consts::TAU),
            pitch: 0.0,
            fall_time: 0,
            jump: None,
            transport: transport.filter(|_| flags & move_flags::ON_TRANSPORT != 0),
        });
    }
}

#[cfg(test)]
mod tests {
    /// A frame that integrated everything it was given — the skipped-time report's no-op input,
    /// which is every frame in these tests but the one that exercises it.
    fn no_skip() -> SkipClock {
        SkipClock {
            dt: 0.0,
            held: false,
            mover: None,
        }
    }

    use super::*;
    use std::f32::consts::TAU;

    /// **The gait toggle announces itself standing perfectly still** — which is the whole reason
    /// **The skipped-time report** (decision 1935): held frames accumulate, the release edge sends
    /// the total ONCE, and nothing is sent for a frame that actually integrated. The three
    /// properties together are the whole contract — a per-frame send would be hundreds of packets
    /// per settle, a lost accumulator would tell the server nothing, and a send with no hold
    /// behind it would be a lie about our own simulation.
    #[test]
    fn a_settle_hold_reports_its_skipped_milliseconds_once_on_release() {
        const MOVER: u64 = 0x0000_0000_0000_002a;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        let held = |dt: f32| SkipClock {
            dt,
            held: true,
            mover: Some(MOVER),
        };
        let released = || SkipClock {
            dt: 0.016,
            held: false,
            mover: Some(MOVER),
        };
        // Three held frames: 0.5 s of simulation advanced through, nothing integrated, and
        // NOTHING on the wire yet — the report is one packet, not one per frame.
        for _ in 0..3 {
            stream_skipped_time(&tx, &mut player, held(0.1));
        }
        stream_skipped_time(&tx, &mut player, held(0.2));
        assert!(
            rx.try_recv().is_err(),
            "a hold in progress sends nothing — the reference's own captures show one packet per \
             skip, not one per frame"
        );
        // The release: one packet, carrying the whole hold.
        stream_skipped_time(&tx, &mut player, released());
        match rx.try_recv().expect("the release edge reports") {
            ClientCommand::MoveTimeSkipped { guid, lag_ms } => {
                assert_eq!(guid, MOVER, "the packet names the mover we were driving");
                assert_eq!(lag_ms, 500, "0.1+0.1+0.1+0.2 s of un-integrated simulation");
            }
            other => panic!("expected MoveTimeSkipped, got {other:?}"),
        }
        // …and the accumulator disarms: a second released frame reports nothing.
        stream_skipped_time(&tx, &mut player, released());
        assert!(
            rx.try_recv().is_err(),
            "the report is an edge; a running mover skips nothing and says nothing"
        );
    }

    /// the reference gives `ToggleRun` its own move-event enqueue instead of letting the
    /// move-state broadcaster find the flag change: that broadcaster gates every send on the
    /// locomotion nibble (`0x61a99d test al,0xf`), so a toggle with no direction bit set would
    /// never leave the client through it. Ours rides the flag differ, which has no such gate.
    /// Observers derive a walker's speed from the relayed bit, so a swallowed packet is a
    /// remote body that keeps running at 7 yd/s while its owner walks (decision 1752).
    #[test]
    fn the_walk_toggle_sends_its_own_opcode_with_no_movement_at_all() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        let arc = || ArcEdges {
            jumped: false,
            wire_launch: false,
            air_nudged: false,
            landed: false,
            fall_time: 0,
        };
        // Entering walk: MSG_MOVE_SET_WALK_MODE (0xc3), and it carries the bit it announces.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("a standing walk toggle still sends")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetWalkMode);
        assert_eq!(kind.opcode(), 0x00C3);
        assert_eq!(flags & move_flags::WALK_MODE, move_flags::WALK_MODE);

        // …and leaving it is the OTHER opcode, not a repeat of the same one.
        stream_self_movement(
            &tx,
            &mut player,
            0,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("the run half sends too")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetRunMode);
        assert_eq!(kind.opcode(), 0x00C2);
        assert_eq!(flags & move_flags::WALK_MODE, 0);

        // A steady walk announces nothing further — it is an edge, not a per-frame report.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.0,
            arc(),
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        while let Ok(ClientCommand::Move { kind, .. }) = rx.try_recv() {
            assert_eq!(kind, MoveKind::SetWalkMode, "the re-entry edge only");
        }
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::WALK_MODE,
            0.1,
            arc(),
            0.1,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(
            rx.try_recv().is_err(),
            "a walk already announced is silent until it flips back"
        );
    }

    /// The toggle is not an axis transition, so the airborne silence must not swallow it: a
    /// mid-air press flips the bit and the packet goes out (the reference's `ToggleRun` refuses
    /// nothing about being airborne, and the fall's own heartbeats would carry the bit anyway).
    #[test]
    fn a_walk_toggle_taken_mid_air_is_not_swallowed_by_the_airborne_silence() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING | move_flags::WALK_MODE,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 400,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert!(
            kinds.contains(&MoveKind::SetWalkMode),
            "the walk edge rides out mid-arc: {kinds:?}"
        );
    }

    #[test]
    fn wire_orientation_is_normalized_into_0_2pi() {
        // `face_yaw` is an unbounded accumulator, but vmangos's `VerifyMovementInfo` rejects any
        // movement packet whose orientation has `|o| > 4π` (≈ 12.566) — so a large yaw must leave the
        // controller wrapped into [0, 2π), matching the real client. A fresh FORWARD press emits a
        // StartForward carrying the current orientation, so we can read back what went on the wire.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            face_yaw: 100.0, // ~15.9 turns — far past the 4π reject bound
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );

        let ClientCommand::Move { orientation, .. } = rx
            .try_recv()
            .expect("a StartForward is sent on first FORWARD")
        else {
            panic!("expected a Move command");
        };
        assert!(
            (0.0..TAU).contains(&orientation),
            "orientation must be normalized into [0, 2π), got {orientation}"
        );
        assert!(
            (orientation - 100.0_f32.rem_euclid(TAU)).abs() < 1e-4,
            "the wrap preserves the angle (100 mod 2π): got {orientation}"
        );
    }

    #[test]
    fn fall_land_reports_the_accumulated_fall_time() {
        // The landing frame's FALL_LAND must carry the arc's accumulated fall clock — vmangos's
        // `Player::HandleFall` only deals fall damage when the land packet's fallTime ≥ 1229 ms, so
        // a zeroed clock silently disables fall damage. The controller snapshots the clock before
        // its arc bookkeeping clears `airborne_since`; this pins that the snapshot — not a re-read
        // of the (already-cleared) arc state — is what goes on the wire.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default(); // airborne_since already cleared, as on a landing frame
        stream_self_movement(
            &tx,
            &mut player,
            0,
            // grounded again: no FALLING/FALLINGFAR on the land packet itself
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: true,
                fall_time: 1700,
            },
            // the snapshot: ~1.7 s of fall (> the 1229 ms damage gate)
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );

        let ClientCommand::Move {
            kind, fall_time, ..
        } = rx.try_recv().expect("a FALL_LAND is sent on landing")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::FallLand);
        assert_eq!(
            fall_time, 1700,
            "the FALL_LAND carries the accumulated fall time, not a cleared clock"
        );
    }

    #[test]
    fn airborne_direction_release_is_silent_and_the_landing_sends_only_fall_land() {
        // The sniff-verified airborne send law: releasing W mid-air emits NO packet (the flag
        // state updates silently and rides the next packet), and the landing then sends exactly
        // one FALL_LAND — never a trailing Stop (the real client sends none; the extra Stop was
        // what re-picked the observer's landing anim away).
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING, // last sent: the JUMP's flags
            ..Default::default()
        };
        // Mid-air, W released: flags drop FORWARD but keep FALLING — silence.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 300,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(rx.try_recv().is_err(), "a mid-air release sends nothing");
        assert_eq!(
            player.move_flags,
            move_flags::FALLING,
            "the flag state still updated silently"
        );
        // The landing frame: grounded, no keys — one FALL_LAND, flags 0, and nothing after it.
        stream_self_movement(
            &tx,
            &mut player,
            0,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: true,
                fall_time: 800,
            },
            0.8,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } = rx.try_recv().expect("the landing packet")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::FallLand);
        assert_eq!(flags, 0, "the FALL_LAND carries the live (released) flags");
        assert!(
            rx.try_recv().is_err(),
            "no trailing Stop after the FALL_LAND"
        );
    }

    /// **A fall that had no jump opens with nothing, and then heartbeats on the ordinary deadline**
    /// — decision 1464, replacing two 0053-era inventions with the law read off the broadcaster
    /// `0x61a820` and reproduced in the 1.12.1 capture: `echo → heartbeats every 500 ms while the
    /// fall lasts → FALL_LAND`, the parenthesis empty for any fall shorter than the deadline.
    ///
    /// The opener is the load-bearing half. It is the only packet that could put `MOVEFLAG_JUMPING`
    /// on the wire for a jumpless arc, and vmangos answers a reported moving flag with
    /// `SetStandState(STAND)` — which is what un-seated a chair-sitter for six frames of
    /// `dy=+0.000` inside the chair's own collider (B79, decision 1458).
    #[test]
    fn a_jumpless_fall_opens_with_nothing_and_heartbeats_on_the_deadline() {
        let (tx, rx) = crossbeam_channel::unbounded();
        // Grounded last frame (flags 0), airborne now with no jump: the walk-off / arrival case.
        let mut player = Player::default();
        let step_off = |now: f32, player: &mut Player| {
            stream_self_movement(
                &tx,
                player,
                move_flags::FALLING,
                0.0,
                ArcEdges {
                    jumped: false,
                    wire_launch: false,
                    air_nudged: false,
                    landed: false,
                    fall_time: (now * 1000.0) as u32,
                },
                now,
                &[],
                None,
                None,
                no_skip(),
            );
        };

        step_off(0.1, &mut player);
        assert!(
            rx.try_recv().is_err(),
            "the arc's first frame puts NOTHING on the wire — this packet is B79's un-seat"
        );
        assert_eq!(
            player.move_flags,
            move_flags::FALLING,
            "the flag state still updated silently, to ride the next packet that does go out"
        );

        // Still inside the 500 ms deadline: silent. A chair arrival's whole stall lives here.
        step_off(0.4, &mut player);
        assert!(rx.try_recv().is_err(), "inside the deadline, still silent");

        // Past it: the ordinary heartbeat runs, carrying the live FALLING bit. This is the half the
        // old `&& !falling` gate suppressed, and the sniff shows mid-air heartbeats plainly.
        step_off(0.7, &mut player);
        let ClientCommand::Move { kind, flags, .. } = rx
            .try_recv()
            .expect("past the deadline the fall heartbeats")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Heartbeat);
        assert_ne!(
            flags & move_flags::FALLING,
            0,
            "and it carries the arc, so observers learn of a long fall in progress"
        );
    }

    #[test]
    fn the_standstill_air_nudge_is_the_one_press_that_breaks_the_airborne_silence() {
        // Decision 0627. The airborne silence is the reference's *deferral* (`StartMove 0x7c6ae0`
        // latches a mid-air press instead of flipping the direction bit) and the deferral has one
        // byte-verified exclusion: "unless nothing is currently moving". That case really flips the
        // bit, so the broadcaster really sends — and it must, because it is the only case where the
        // press changed our motion, and the observer is integrating an arc that says xy_speed = 0.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FALLING, // the standing jump: airborne, no direction
            // Post-nudge: the mover re-seeded it this frame (Bevy −Z = WoW +X, so |xy| = 2.5).
            horiz_vel: bevy::prelude::Vec3::new(0.0, 0.0, -2.5),
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: true,
                landed: false,
                fall_time: 300,
            },
            0.3,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move {
            kind, flags, jump, ..
        } = rx.try_recv().expect("the nudge broadcasts its transition")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StartForward);
        assert_ne!(flags & move_flags::FALLING, 0, "it rides the arc");
        let tail = jump.expect("an airborne packet carries the ballistic tail");
        assert!(
            (tail.xy_speed - 2.5).abs() < 1.0e-3,
            "the tail carries the RE-SEEDED horizontal — the whole point: an observer whose \
             arc says xy_speed = 0 learns the mover started moving, got {}",
            tail.xy_speed
        );
        assert!(rx.try_recv().is_err(), "one packet, not a burst");

        // The other side of the same rule: a press that did NOT nudge (a jump taken with momentum —
        // the reference defers it into an inert latch) stays silent, exactly as the sniff shows.
        let mut moving = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut moving,
            move_flags::FORWARD | move_flags::STRAFE_LEFT | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 300,
            },
            0.3,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(
            rx.try_recv().is_err(),
            "a deferred mid-air press sends nothing"
        );
    }

    #[test]
    fn airborne_turn_transitions_and_facing_still_stream() {
        // The two things that DO go out mid-air (sniff-verified): the turn axis (turning works
        // airborne — START_TURN_RIGHT/STOP_TURN with Falling set) and SET_FACING for a mouse
        // jump-turn — the capture's `8193 (Forward, Falling)` SET_FACINGs, which carry **no** turn
        // bit. No heartbeats stream while FALLING, so the facing needs its own carrier.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::TURN_LEFT | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 200,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } =
            rx.try_recv().expect("a mid-air turn broadcasts")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StartTurnLeft);
        assert_ne!(flags & move_flags::FALLING, 0, "the packet rides the arc");
        assert!(rx.try_recv().is_err(), "the turn axis carries the facing");
        // Mid-air mouse-turn with the turn key released: the StopTurn transition is this frame's
        // send, so the periodic arm does not add a second packet (it is `!sent`-gated, not
        // FALLING-gated — 1464), and the facing streams via SET_FACING once the turn flag is gone.
        player.face_yaw = 1.0;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 900,
            },
            1.5,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![MoveKind::StopTurn, MoveKind::SetFacing],
            "the turn closes, then the facing reports — the frame already sent, so no heartbeat"
        );
    }

    #[test]
    fn facing_streams_while_running() {
        // **The regression this law exists for** (decision 0617). A mouse-turn while running used to
        // put nothing on the wire until the next ~500 ms heartbeat, so observers dead-reckoned us in a
        // stale direction for half a second and then watched us snap. The reference streams SET_FACING
        // *while moving* — 116 of the capture's 179 carry a direction bit — so every frame the facing
        // moves, one packet goes out carrying the live direction flags.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD, // already running: no transition this frame
            ..Default::default()
        };
        player.face_yaw = 0.7;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.05,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, flags, .. } = rx
            .try_recv()
            .expect("a mouse-turn while running reports its facing")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::SetFacing);
        assert_eq!(
            flags,
            move_flags::FORWARD,
            "the facing report carries the live direction flags"
        );
        assert!(
            rx.try_recv().is_err(),
            "exactly one packet per changed frame"
        );
        // A frame that didn't move the mouse sends nothing: the detector is exact equality, not a
        // rate limit — and the heartbeat deadline (0.5 s) has not come round.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.06,
            &[],
            None,
            None,
            no_skip(),
        );
        assert!(rx.try_recv().is_err(), "an unchanged facing is silent");
    }

    #[test]
    fn the_turn_axis_carries_its_own_facing() {
        // Not one SET_FACING in the capture carries TURN_LEFT/TURN_RIGHT: a keyboard turn is fully
        // described by its flag (observers rotate at the turn rate while it's set), so the facing it
        // sweeps needs no packet of its own — and, crucially, the suppressed frames leave no catch-up
        // packet behind, because the change detector tracks the previous frame's facing regardless.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD | move_flags::TURN_RIGHT,
            ..Default::default()
        };
        for (i, yaw) in [0.3_f32, 0.6, 0.9].into_iter().enumerate() {
            player.face_yaw = yaw;
            stream_self_movement(
                &tx,
                &mut player,
                move_flags::FORWARD | move_flags::TURN_RIGHT,
                0.0,
                ArcEdges {
                    jumped: false,
                    wire_launch: false,
                    air_nudged: false,
                    landed: false,
                    fall_time: 0,
                },
                0.05 * (i as f32 + 1.0),
                &[],
                None,
                None,
                no_skip(),
            );
            assert!(
                rx.try_recv().is_err(),
                "the turn axis is silent (frame {i})"
            );
        }
        // Release the turn key with the facing unchanged since the last swept frame: the STOP_TURN
        // goes out and nothing follows it.
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.2,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, .. } = rx.try_recv().expect("the STOP_TURN") else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::StopTurn);
        assert!(
            rx.try_recv().is_err(),
            "no catch-up SET_FACING after a suppressed turn"
        );
    }

    #[test]
    fn a_transition_does_not_swallow_the_facing_report() {
        // The capture repeatedly shows both in one millisecond (`+4971` SET_FACING & START_STRAFE_
        // RIGHT). They are two independent emitters in the reference, so a frame that both turns and
        // changes axis puts both on the wire.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD,
            ..Default::default()
        };
        player.face_yaw = 1.2;
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FORWARD | move_flags::STRAFE_RIGHT,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.05,
            &[],
            None,
            None,
            no_skip(),
        );
        let kinds: Vec<_> = rx
            .try_iter()
            .map(|c| match c {
                ClientCommand::Move { kind, .. } => kind,
                _ => panic!("expected Move commands"),
            })
            .collect();
        assert_eq!(kinds, vec![MoveKind::StartStrafeRight, MoveKind::SetFacing]);
    }

    /// One idle frame at `pos`, nothing else happening — the shape every reconcile test needs.
    fn idle_frame(tx: &Sender<ClientCommand>, player: &mut Player, pos: bevy::prelude::Vec3) {
        player.pos = pos;
        stream_self_movement(
            tx,
            player,
            player.move_flags,
            0.0,
            ArcEdges {
                jumped: false,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
    }

    #[test]
    fn a_resting_body_that_drifts_reports_it_once() {
        // Decision 0907. Our resolver settles a body already at rest a fraction of a millimetre after
        // the packet that reported the rest (a landing, a login, a teleport onto a server-authored
        // pose), and standing still nothing else goes out — so the server's copy of our position went
        // stale, and the next packet of any kind (in practice a right-drag's SET_FACING) delivered the
        // whole delta at once. vmangos compares positions with EXACT float equality and reads any
        // difference as movement, cancelling a movement-interrupt cast. So: at rest, a changed
        // position IS the news, and one heartbeat carries it — then silence again.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default();
        idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(1.0, 2.0, 3.0));
        let ClientCommand::Move {
            kind, flags, pos, ..
        } = rx.try_recv().expect("the drift is reported")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Heartbeat);
        assert_eq!(flags, 0, "at rest — the reconcile invents no motion");
        assert_eq!(pos, bevy_to_wow(bevy::prelude::Vec3::new(1.0, 2.0, 3.0)));
        // The same position again: the server's copy is current, so nothing goes out.
        idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(1.0, 2.0, 3.0));
        assert!(
            rx.try_recv().is_err(),
            "an unchanged resting position is silent — one packet per settle, not per frame"
        );
        // A settle that is one float wide still counts: vmangos's compare has no epsilon.
        idle_frame(
            &tx,
            &mut player,
            bevy::prelude::Vec3::new(1.0, f32::from_bits(2.0f32.to_bits() - 1), 3.0),
        );
        assert!(
            rx.try_recv().is_ok(),
            "a one-ULP settle is exactly what the server's exact compare would read as movement"
        );
    }

    #[test]
    fn the_reconcile_never_fires_while_the_body_is_in_motion() {
        // In motion the position changes every frame BY DESIGN, and the transition + ~500 ms
        // heartbeat stream is what carries it (0052/0053) — a per-frame reconcile there would be a
        // packet flood, and the reference sends nothing of the kind. The gate is the motion mask, so
        // a runner mid-stride and a body mid-arc are both silent between their own packets.
        let (tx, rx) = crossbeam_channel::unbounded();
        for flags in [move_flags::FORWARD, move_flags::FALLING] {
            let mut player = Player {
                move_flags: flags, // already streaming this state: no transition this frame
                ..Default::default()
            };
            idle_frame(&tx, &mut player, bevy::prelude::Vec3::new(9.0, 9.0, 9.0));
            assert!(
                rx.try_recv().is_err(),
                "moving ({flags:#x}): the position rides the movement stream, not a reconcile"
            );
        }
    }

    #[test]
    fn park_mover_flushes_a_stop_and_clears_stale_flags() {
        // We were last streaming FORWARD when the controller stopped driving us (free-fly). Parking must
        // send one Stop with flags cleared (so observers drop the phantom walk) and a normalized facing.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            move_flags: move_flags::FORWARD,
            face_yaw: -3.0, // out-of-range / negative — must still go out normalized
            ..Default::default()
        };
        park_mover(&tx, &mut player);

        let ClientCommand::Move {
            flags, orientation, ..
        } = rx
            .try_recv()
            .expect("a Stop is flushed when flags were stale")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(flags, 0, "the parked Stop clears the move-flags");
        assert!(
            (0.0..TAU).contains(&orientation),
            "the parked facing is normalized into [0, 2π), got {orientation}"
        );
        assert_eq!(player.move_flags, 0, "bookkeeping is zeroed after parking");
    }

    /// **A forced speed change is acked on a frame nobody is driving** — a fear, a server spline,
    /// a mover hand-off. Those frames return before the stream, which is where a controlled
    /// frame's acks go out, so the change was applied and never answered (vmangos enforces it 4 s
    /// late, counts the miss toward its anticheat kick, and blocks the graveyard repop meanwhile).
    /// The payload is the honest reported word: the ride's FORWARD kept, the airborne pair dropped
    /// (no arc tail exists here), and `ON_TRANSPORT` only with the tail that must travel with it.
    #[test]
    fn a_speed_change_is_acked_while_not_driving() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let ack = crate::net::SpeedChangeMessage {
            guid: 7,
            kind: benilla_protocol::SpeedKind::Run,
            counter: 3,
            speed: 3.5,
        };

        // A spline ride reporting FORWARD, off any deck, with a stale ON_TRANSPORT bit.
        let player = Player {
            move_flags: move_flags::FORWARD | move_flags::FALLING | move_flags::ON_TRANSPORT,
            face_yaw: -1.0,
            ..Default::default()
        };
        ack_speeds_undriven(&tx, &player, &[ack]);
        let Ok(ClientCommand::ForceSpeedAck {
            counter,
            speed,
            flags,
            orientation,
            transport,
            jump,
            ..
        }) = rx.try_recv()
        else {
            panic!("the change is acked");
        };
        assert_eq!((counter, speed), (3, 3.5), "the ack echoes the change");
        assert_eq!(
            flags,
            move_flags::FORWARD,
            "FORWARD kept; FALLING and a tailless ON_TRANSPORT dropped"
        );
        assert!(transport.is_none() && jump.is_none());
        assert!((0.0..TAU).contains(&orientation));
        assert!(rx.try_recv().is_err(), "one ack per change");

        // On a deck: the transport bit rides with its tail.
        let player = Player {
            move_flags: move_flags::ON_TRANSPORT,
            ride: Some(super::super::state::PlayerRide {
                entity: bevy::ecs::entity::Entity::PLACEHOLDER,
                guid: 0x1F,
                local_pos: bevy::math::Vec3::ZERO,
                boat_yaw: 0.0,
            }),
            ..Default::default()
        };
        ack_speeds_undriven(&tx, &player, &[ack]);
        let Ok(ClientCommand::ForceSpeedAck {
            flags, transport, ..
        }) = rx.try_recv()
        else {
            panic!("the change is acked");
        };
        assert_eq!(flags, move_flags::ON_TRANSPORT);
        assert_eq!(transport.map(|t| t.guid), Some(0x1F));
    }

    /// **A knockback launch acks, and sends no `MSG_MOVE_JUMP`** (decision 1702).
    ///
    /// Two laws in one frame, and they are the same law read from both ends. `MSG_MOVE_JUMP` has a
    /// single live emission site image-wide — `0x615ed1`, the move-command drain's jump arm — and
    /// the knockback's own arm of that same drain pushes `0xf0`, its ack, instead. So a knockback
    /// announces itself once, on the ack, whose `MovementInfo` is what the server turns into every
    /// observer's `MSG_MOVE_KNOCK_BACK`.
    ///
    /// The regression this pins is not cosmetic: `CHEAT_TYPE_OVERSPEED_JUMP` is one of the few
    /// vmangos checks with no knockback exemption, and it fires on any `MSG_MOVE_JUMP` whose
    /// `xyspeed` exceeds run speed — which a knockback's does by design.
    #[test]
    fn a_knockback_acks_and_sends_no_jump() {
        let (tx, rx) = crossbeam_channel::unbounded();
        // Mid-launch: the mover has seeded the arc from the quad below (Bevy −Z is WoW +X, so a
        // due-west push of 25 yd/s lands as `horiz_vel.z = −25`), and `jump_zspeed` is +up where the
        // wire's is down-positive.
        let launch = benilla_protocol::JumpInfo {
            zspeed: -12.0,
            cos_angle: 1.0,
            sin_angle: 0.0,
            xy_speed: 25.0,
        };
        let mut player = Player {
            horiz_vel: bevy::prelude::Vec3::new(0.0, 0.0, -25.0),
            jump_zspeed: 12.0,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: true, // the arc really opened — the bookkeeping cannot tell the two apart
                wire_launch: true,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            Some(super::super::state::PendingKnockback {
                guid: 0x1234,
                counter: 7,
                launch,
            }),
            None,
            no_skip(),
        );

        let ClientCommand::KnockBackAck {
            guid,
            counter,
            launch: acked,
            flags,
            ..
        } = rx.try_recv().expect("the launch owes an ack")
        else {
            panic!("expected the knockback ack first");
        };
        assert_eq!((guid, counter), (0x1234, 7), "the counter must be echoed");
        assert_eq!(
            acked, launch,
            "the four floats go back bit-for-bit — vmangos matches them within 0.01 before it will \
             relay the knockback to anyone"
        );
        assert_ne!(
            flags & move_flags::FALLING,
            0,
            "the ack's MovementInfo must carry JUMPING, or the jump tail it needs is never written"
        );

        for extra in rx.try_iter() {
            if let ClientCommand::Move { kind, .. } = extra {
                assert_ne!(
                    kind,
                    MoveKind::Jump,
                    "a knockback must not put MSG_MOVE_JUMP on the wire — instant \
                     CHEAT_TYPE_OVERSPEED_JUMP, and the reference's jump arm is a different drain arm"
                );
            }
        }
    }

    /// The other half of the same gate: an ordinary jump on an identical frame still sends its JUMP.
    /// Without this, "no JUMP for a knockback" could be satisfied by never sending one at all.
    #[test]
    fn an_ordinary_jump_still_sends_its_jump() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player {
            jump_zspeed: 7.955547,
            ..Default::default()
        };
        stream_self_movement(
            &tx,
            &mut player,
            move_flags::FALLING,
            0.0,
            ArcEdges {
                jumped: true,
                wire_launch: false,
                air_nudged: false,
                landed: false,
                fall_time: 0,
            },
            0.0,
            &[],
            None,
            None,
            no_skip(),
        );
        let ClientCommand::Move { kind, jump, .. } =
            rx.try_recv().expect("a jump take-off is announced")
        else {
            panic!("expected a Move command");
        };
        assert_eq!(kind, MoveKind::Jump);
        let tail = jump.expect("the JUMP carries the ballistic tail");
        assert!(
            (tail.zspeed + 7.955547).abs() < 1.0e-4,
            "the wire zspeed is down-positive: a rising jump reports negative, got {}",
            tail.zspeed
        );
    }

    #[test]
    fn park_mover_is_a_noop_once_already_stopped() {
        // Idempotent: with no stale flags there's nothing to clear, so no packet goes out — safe to call
        // every free-fly frame.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut player = Player::default(); // move_flags == 0
        park_mover(&tx, &mut player);
        assert!(
            rx.try_recv().is_err(),
            "no Stop is sent when we were already reported stopped"
        );
    }
}
