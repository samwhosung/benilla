//! The avatar's state, [`Player`] and its resources, and the movement constants; no systems.

use avian3d::prelude::*;
use benilla_protocol::{JumpInfo, MoveMode};
use bevy::prelude::*;

/// Backpedal over run, the stock 4.5 / 7.0, for the synthetic speed set used under a
/// `$WOW_MOVE_SPEED` override or before the create block brings the server's speeds.
pub(super) const RUN_BACK_RATIO: f32 = 4.5 / 7.0;

/// Walk over run, the stock 2.5 / 7.0, for the same synthetic speed set as [`RUN_BACK_RATIO`].
pub(super) const WALK_RATIO: f32 = 2.5 / 7.0;

/// Turn rate (rad/s) until the mover's own arrives: the live rate is its sixth speed,
/// `CMovement+0x9c` (set by `0x7c6ff0`, read by `GetYawRate` `0x7c5c50`), which the ctor `0x7c4850`
/// zeroes, so π is the server's value and the client keeps no default.
pub(super) const TURN_RATE: f32 = std::f32::consts::PI;

/// The mouse-look pitch clamp, ±89° (`0x8089d8`, on the camera-to-`SetPitch` path); the ±π/2 clamp
/// belongs to the pitch-key integrator `0x7c4f80`, whose keys are unbound by default.
pub(super) const MOUSELOOK_PITCH_CLAMP: f32 = 1.553_343;

/// Water walking sees the water only while the mover pitch is strictly above −37° (`[0x80dfe8]`,
/// the third gate of the trace-mask arm `0x6315f0`); aiming lower sinks a water-walker in.
pub(super) const WATER_WALK_PITCH_FLOOR: f32 = -0.645_771_8;
/// Turn-rate scale while translating or falling (`flags & 0x200f`).
pub(super) const TURN_RATE_MOVING: f32 = 0.75;

/// Once steering stops, the rendered body closes on the aim at `turnRate × 8` rad/s, clamped to the
/// gap (`0x607ed0`); while steering, only the 90° ceiling moves it (`0x60818a`-`0x6081bf`).
pub(super) const STATIONARY_CHASE_RATE: f32 = 8.0;

// ── Character controller ─────────────────────────────────────────────────────────

/// Capsule radius (yd), the `CMovement` ctor's placeholder 1/3. The reference overwrites it per
/// model from `CreatureModelData` (`0x6174b0`: human male 0.30555, female 0.20835); not built here.
pub(crate) const CAPSULE_RADIUS: f32 = 1.0 / 3.0;
/// Capsule total height (yd), one for every body: the swept shape and the head and feet offsets.
/// Numerically [`DEFAULT_COLLISION_HEIGHT`], but a unit's depth lines use its per-model
/// [`crate::entities::CollisionHeight`] instead.
pub(crate) const CAPSULE_HEIGHT: f32 = 2.027_777_7;

/// The `CMovement` ctor's collision height (`0x616fd8`), before `0x6174b0` sets the model's: the
/// depth lines' fallback for a unit whose display id has no `CreatureModelData` row.
pub(crate) const DEFAULT_COLLISION_HEIGHT: f32 = 2.027_777_7;
/// Gravity (yd/s²), the reference's and vmangos's `Movement::gravity`; also avian's `Gravity`, and
/// [`crate::net`] replays relayed jump arcs under it so an observer's arc matches the mover's.
pub(crate) const GRAVITY: f32 = 19.291_105;
/// Jump take-off speed (yd/s), the reference's.
pub(super) const JUMP_SPEED: f32 = 7.955_547;
/// Terminal fall speed (yd/s), `[0x87d894]` and vmangos's `terminalVelocity`; [`crate::net`]'s arc
/// replay caps at it too.
pub(crate) const TERMINAL_VELOCITY: f32 = 60.148_003;
/// Terminal fall speed under feather fall (yd/s), the whole of Slow Fall: `0x7c5d20` and
/// `0x7c5f50` pick `[0x87d898]` over `[0x87d894]` while `MOVEFLAG_SAFE_FALL` is set (vmangos's
/// `terminalSavefallVelocity`).
pub(crate) const FEATHER_TERMINAL_VELOCITY: f32 = 7.0;
/// Hover's clearance (yd), the whole of Hover: the walk resolver probes `[0x7ff9d8]` further down
/// (`0x636dd2`) and writes the body's z a yard clear of the floor (`0x636e81`-`0x636ea9`).
pub(crate) const HOVER_HEIGHT: f32 = 1.0;
/// Hover's rise rate (yd/s): the snap only lowers the body (`0x636e52`), so the rise is a separate
/// rate-limited pass (`0x636fa1`-`0x6370f1`) at `0x7c61b0`'s `[0x87d898]`, the feather-fall 7.0.
pub(crate) const HOVER_CLIMB_RATE: f32 = 7.0;
/// Walkable iff the surface normal is within 50° of up, the reference's limit; steeper slides back.
pub(super) const GROUND_COS: f32 = 0.642_788;
/// Downward probe distance (yd) to decide whether we're standing on ground.
pub(super) const GROUND_PROBE: f32 = 0.2;
/// The snap's slope ratio, `[0x80c740]` (atan ≈ 61.6°): the reference's step-vs-fall election
/// `0x6367b0` probes `travel · ratio + slack` below the moved body, plus the rise budget `H` only
/// on a steep support (`0x4000000`, `0x636dfc`). The foot cone's waist (`0x631c0b`) shares it.
pub(super) const STEP_SLOPE_RATIO: f32 = 1.849_399;
/// The snap reach's fixed slack (yd), `[0x7ff9d0]` = 1/36.
pub(super) const STEP_SNAP_SLACK: f32 = 0.027_777_8;
/// The step-up rise ceiling (yd), the reference's `H`: `0x617430` returns `CMovement+0xb8`, the
/// ratio `max(SCALE_X / CreatureModelScale, 1)`, which is 1.0 for the local player.
pub(crate) const STEP_UP_HEIGHT: f32 = 1.0;
/// The rise ceiling of a body the reference does not treat as player-controlled, `[0x801628]` on
/// `0x5fa550`'s false leg of `0x617430`: a creature, a pet, or a charmed, possessed, feared,
/// confused or rooted player.
pub(crate) const CREATURE_STEP_UP_HEIGHT: f32 = 2.0;
/// The step-up certify advance (yd), a floor under the frame's travel: the reference steps
/// `max(H·tan50°, radius + 1/720)` into the face, 1.1918 yd at `H` = 1.0. The raised forward sweep
/// is clipped by anything in the way, so the advance reaches only clear air.
pub(super) const STEP_UP_ADVANCE: f32 = 1.191_753_6;
/// The certify reach per yard of rise budget, `tan 50°` (`0x636147`-`0x636190`); a creature's 2.0
/// reaches 2.38 yd.
pub(super) const STEP_UP_ADVANCE_PER_YARD: f32 = STEP_UP_ADVANCE / STEP_UP_HEIGHT;
/// The foot cone's height (yd). The reference's solid (`0x631440`) is a box over a cone whose
/// bevels rise `radius · [0x80c740]` from a point at the foot, so an edge below this is ridden up
/// the 61.6° skirt (`0x635c00`) and one above meets the box and takes the step-up. With the
/// placeholder [`CAPSULE_RADIUS`] ours is 0.616 yd; a live human male's is 0.565, female's 0.385.
pub(super) const FOOT_CONE_HEIGHT: f32 = CAPSULE_RADIUS * STEP_SLOPE_RATIO;
/// The landing probe (yd): airborne, walk mode resumes only this close to the floor, so the arc
/// ends where the slide contacts rather than [`GROUND_PROBE`] early.
pub(super) const LAND_PROBE: f32 = 0.05;
/// Stalled airborne frames (see [`WEDGE_STALL_RATIO`]) that count as resting wedged between two
/// steep faces: the fall ends there, and nothing becomes walkable by it.
pub(super) const WEDGE_STILL_FRAMES: u8 = 3;
/// A frame stalls when its descent is under this fraction of gravity's intended `vel_y·dt` while
/// falling faster than [`WEDGE_MIN_FALL`]: free fall achieves ~100%, a steep slide at least 75%.
pub(super) const WEDGE_STALL_RATIO: f32 = 0.15;
/// Fall speed (yd/s) before stalled frames count: a jump apex never qualifies, a wedge soon does.
pub(super) const WEDGE_MIN_FALL: f32 = 1.0;
/// The one-shot air nudge (yd/s) a standstill jump may steer by. The reference's is the live
/// `min(walk, run)` (`0x7c4c90(1)`, `0x7c4d19`); this is the default walk speed, not the live one.
pub(super) const AIR_NUDGE_SPEED: f32 = 2.5;
/// The FALLINGFAR distance leg (yd): a jump arc (launch vz ≠ 0) latches it once this far below
/// its launch height (`0x633240`, `[0x80dff8]`), and the anim swaps to Fall(40); a flat jump never
/// does.
pub(super) const FALL_FAR_DROP: f32 = 0.111_11;
/// The FALLINGFAR timer leg (s): a step-off fall (launch vz = 0) latches it after 500 ms airborne
/// (`0x633240`, `0x1f4`), about 2.41 yd of free fall from rest.
pub(super) const FALL_FAR_TIME: f32 = 0.5;
/// Skin width (yd) kept between the capsule and geometry on casts.
pub(super) const SKIN_WIDTH: f32 = 0.02;

/// Seconds of stalled streaming before the post-snap hold gives up: the streamer pushes the
/// deadline on while the world is stale or any load counter moves, so only a dead stream hits it.
pub(crate) const SETTLE_TIMEOUT: f32 = 6.0;

/// The player body's collision capsule, swept by avian's `MoveAndSlide`: origin at its centre, the
/// player's `pos` at its feet. Every player body sweeps it, remote ones too, as the reference runs
/// every mover through one controller (`0x616620`); the reference sizes each from its model.
#[derive(Resource)]
pub(crate) struct PlayerCapsule(pub(crate) Collider);

/// The run-speed fallback, the stock 7.0 until the server's speeds stream in, or the
/// `$WOW_MOVE_SPEED` dev override (`env_override`), which replaces the server's speeds outright.
#[derive(Resource)]
pub(super) struct MoveSpeed {
    pub(super) value: f32,
    pub(super) env_override: bool,
}

/// The mover modes the server grants, as typed state: the outbound flag word is rebuilt from these
/// every frame, and a mode missing from it is cleared by the server's next echo. Four arrive
/// through the acked [`benilla_protocol::MoveMode`] opcodes, `levitating` unacked in a
/// server-authored move; Levitate (spell 1706) grants three at once.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MoveModes {
    /// Rooted (`SMSG_FORCE_MOVE_ROOT`): translation and jumps die but turning stays live, since the
    /// input tick's allow-list (`0x615c71`, table `0x618054`) permits turn, pitch and facing. A
    /// stun, which takes the pivot too, is [`crate::player::UNIT_FLAG_STUNNED`].
    pub(crate) rooted: bool,
    /// Water walking (`SMSG_MOVE_WATER_WALK`): the liquid surface is ground. It does not stop the
    /// swim latch (`0x6030c0` tests only LEVITATING); swimming turns it off instead (`0x631617`).
    pub(crate) water_walking: bool,
    /// Feather fall (`SMSG_MOVE_FEATHER_FALL`): the terminal velocity drops to
    /// [`FEATHER_TERMINAL_VELOCITY`], and nothing else changes.
    pub(crate) feather_fall: bool,
    /// Hover (`SMSG_MOVE_SET_HOVER`): ground contact sits [`HOVER_HEIGHT`] up and the jump is
    /// refused (the first test in `CMovement::Jump` `0x7c6230`); the grant itself jumps the body
    /// ([`Player::hover_launch`]). With water walking it seems to leave a swimmer no way up, since
    /// swimming turns water walking off and hover refuses the breach: each gate is traced, the sum
    /// is not.
    pub(crate) hover: bool,
    /// `MOVEFLAG_LEVITATING` (GM `.cheat fly`), merged unacked from a server-authored move: the
    /// swim decision does not run while it is set (`0x6030d2`), so a server-set swim is flight.
    pub(crate) levitating: bool,
}

impl MoveModes {
    /// Grant or revoke one acked mode; `levitating` has no opcode and arrives by the wire merge.
    pub(crate) fn set(&mut self, mode: MoveMode, apply: bool) {
        match mode {
            MoveMode::Root => self.rooted = apply,
            MoveMode::WaterWalk => self.water_walking = apply,
            MoveMode::FeatherFall => self.feather_fall = apply,
            MoveMode::Hover => self.hover = apply,
        }
    }

    /// The granted modes as `MOVEMENTFLAGS` bits, which every outbound packet must carry: the
    /// reference's wire word is its live state (`[cmov+0x40]`), and a mode we drop is cleared by
    /// the server's next echo.
    pub(crate) fn wire_flags(&self) -> u32 {
        use crate::creature_anim::move_flags as f;
        let mut flags = 0;
        if self.rooted {
            flags |= f::ROOT;
        }
        if self.water_walking {
            flags |= f::WATER_WALKING;
        }
        if self.feather_fall {
            flags |= f::SAFE_FALL;
        }
        if self.hover {
            flags |= f::HOVER;
        }
        if self.levitating {
            flags |= f::LEVITATING;
        }
        flags
    }

    /// Lift the granted modes out of a server-authored flag word; the reference's merge mask
    /// `0x75a0_7dff` holds all five. Deviation: root is not merged, because a bare pose unrooting
    /// us while the server still roots us streams motion into `CHEAT_TYPE_ROOT_MOVE`; vmangos keeps
    /// a rooted mover's root bit (`MovementHandler.cpp:1064`), so the reference's merge of it is a
    /// no-op anyway.
    pub(crate) fn merge_from_wire(&mut self, wire: u32) {
        use crate::creature_anim::move_flags as f;
        self.water_walking = wire & f::WATER_WALKING != 0;
        self.feather_fall = wire & f::SAFE_FALL != 0;
        self.hover = wire & f::HOVER != 0;
        self.levitating = wire & f::LEVITATING != 0;
    }
}

/// The precondition both movement-input predicates test first, the reference's `0x5144e0`. Of its
/// five terms, health ([`MoverInput::dead`]) and the far-sight latch
/// ([`MoverInput::view_is_out`]) are here; the mover resolving is structural (the controller
/// returns early on `control_lost` and `reseat`), and the aux gate `[[mover+0x118]+0xa4]`
/// (`0x514516`) and the Knockdown animation lockout `0x60f5b0` are not built.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MoverInput {
    /// The health term: the driven mover's health is not above 0 (`0x5144fd`, off `[mover+0x110]`).
    /// A released ghost has health 1 from the server, so it passes and keeps every input.
    pub(crate) dead: bool,
    /// The far-sight term, held true while our view is out on a far-sight object and we drive our
    /// own body: `[mover+0x1c70] & 1`, read only when the mover is the active player (`0x514537`).
    /// The latch sets only once the subject resolves (`0x5ee3f6`), so this follows the resolved
    /// [`super::view_subject::ViewSubject`], not the raw `PLAYER_FARSIGHT` field.
    pub(crate) view_is_out: bool,
}

/// The far-sight term: the view is out and the body is our own. Mind Control sets the same field,
/// so without `driving_own_body` (the active-player test `0x5fa6d0`) it would freeze the victim.
pub(crate) fn view_is_out(driving_own_body: bool, far_sight_engaged: bool) -> bool {
    driving_own_body && far_sight_engaged
}

impl MoverInput {
    /// `0x5144e0`, over the terms modelled here.
    fn ready(self) -> bool {
        !self.dead && !self.view_is_out
    }

    /// `0x514560`, the gate on the forward/back and strafe emitters (`0x5146c1`): the precondition
    /// and no root (`MOVEMENTFLAGS & 0x1200`); its stand-state-7 term is one vmangos never sets.
    pub(crate) fn may_translate(self, rooted: bool) -> bool {
        self.ready() && !rooted
    }

    /// `0x5145b0`, the gate on the turn and pitch emitters (`0x514755`): the precondition and no
    /// stun (`UNIT_FIELD_FLAGS & 0x40000`). The mouse's gate `0x5145e0` calls it first, so a corpse
    /// turns by neither keys nor mouse.
    pub(crate) fn may_turn(self, stunned: bool) -> bool {
        self.ready() && !stunned
    }

    /// `0x5145e0`, the gate on the mouse's camera-to-body hand-off (`0x514474`, `0x51495a`): the
    /// stun predicate and a standing body (`0x51460c`). A right-drag while seated turns only the
    /// camera and does not stand you up; `stand_state` is the predicted one `0x5ed570` returns.
    pub(crate) fn mouse_may_turn_body(self, stunned: bool, stand_state: u8) -> bool {
        self.may_turn(stunned) && stand_state == 0
    }

    /// The input tick's teardown leg (`0x5146d6`, ending click-to-move and `/follow`), taken only
    /// with both predicates down: death, far sight, or a root and a stun together.
    pub(crate) fn torn_down(self, rooted: bool, stunned: bool) -> bool {
        !self.may_translate(rooted) && !self.may_turn(stunned)
    }
}

/// Strip from a freshly built flag word what a gated mover cannot hold: the direction bits while
/// translation is gated (the allow-list `0x618054` drops the move commands and `SetRoot` `0x7c7340`
/// masks `0xffe07f00`, so the anim resolver's `0x5fd10c` sees no motion) and the turn bits while
/// turning is gated (`0x514755`). The reference's emitters refuse at the source; ours rebuilds the
/// word each frame. Modes and SWIMMING ride on.
pub(crate) fn incapacitated_flags(flags: u32, translate_gated: bool, turn_gated: bool) -> u32 {
    use crate::creature_anim::move_flags as f;
    let mut out = flags;
    if translate_gated {
        out &= !f::ANY_MOVE;
    }
    if turn_gated {
        out &= !(f::TURN_LEFT | f::TURN_RIGHT);
    }
    out
}

/// Our avatar: until `active` the camera free-flies, then we drive the body the server placed.
/// `PartialEq` serves the test that the session end resets the whole of it to `Player::default()`.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Player {
    pub(crate) active: bool,
    /// The modes the server has granted our mover.
    pub(super) modes: MoveModes,
    /// Autorun: the reference's input bit `0x1000` at `[MOVE+4]`, flipped by `ToggleAutoRun`
    /// `0x513de0`. The emitter `0x514da0` adds it to the forward axis, but a W or S key-down clears
    /// it first ([`autorun_cancelled`]), so it is not a held W: autorun then S walks backward.
    pub(super) autorun: bool,
    /// Walk mode, `MOVEFLAG_WALK_MODE` (`0x100`), toggled by `ToggleRun` `0x513d50` from the word's
    /// current bit (`0x60e080`). Observers read our gait off it, and it is in the server-authored
    /// merge mask, so a server move can flip it ([`super::wire_in`]); it owes no ack.
    pub(super) walking: bool,
    /// `/follow` holding forward this frame: the reference's follow pushes W's own bit
    /// (`0x60e790`), so this is a [`forward_axis`] term. Rewritten each frame by
    /// [`super::follow::steer_follow`] just before the controller reads it.
    pub(super) follow_forward: bool,
    /// Free-fly (`F`): the camera moves on its own and the avatar/server position is frozen.
    pub(crate) detached: bool,
    /// `SMSG_CLIENT_CONTROL_UPDATE` with `allowMove = 0` named the unit we drive: us, while someone
    /// mind-controls us, or our possessed creature when feared or confused, the possession
    /// standing. The reference zeroes its mover; vmangos neither roots a charmed player nor refuses
    /// its movement, so nothing else holds the body. Unlike a root, it stops turning too.
    pub(crate) control_lost: bool,
    /// The unit we drive in place of our body (a mind-controlled creature, Eye of Kilrogg), `None`
    /// for our own. Our body must then send nothing: outbound `MSG_MOVE_*` carry no guid, so the
    /// server applies them to whatever we last claimed.
    pub(crate) foreign_mover: Option<u64>,
    /// The driven body changed, and we drive nothing until its streamed pose is adopted, since
    /// `pos` and the facing still describe the last one. Set when the mover guid changes, cleared
    /// at the take-control edge.
    pub(crate) reseat: bool,
    /// Feet position in Bevy coords, converted to WoW coords only for the wire.
    pub(crate) pos: Vec3,
    /// Vertical velocity (yd/s, Bevy +Y up), zeroed while grounded.
    pub(super) vel_y: f32,
    /// Horizontal velocity (yd/s): from input while grounded, the frozen take-off momentum while
    /// airborne, bar one [`AIR_NUDGE_SPEED`] steer after a standstill jump.
    pub(super) horiz_vel: Vec3,
    /// The movement flags we last streamed; this frame's are diffed against them to emit a
    /// `MSG_MOVE_*` per axis transition.
    pub(super) move_flags: u32,
    /// Last frame's facing: any change off the turn axis streams `MSG_MOVE_SET_FACING`, moving or
    /// not (the reference's detector `0x617170`, exact equality).
    pub(super) last_facing: f32,
    /// The exact position floats we last sent (WoW coords). vmangos interrupts a cast on any change
    /// to its stored position (`Player.cpp:6092`), so the resolver's tiny settle after a reported
    /// rest is sent on its own ([`super::movement_net::stream_self_movement`]) before the next
    /// facing change can carry it as a move.
    pub(super) last_pos: [f32; 3],
    /// A stand state committed locally, sent or from `SMSG_STANDSTATE_UPDATE`, whose echo into
    /// `UNIT_FIELD_BYTES_1` has not landed: the reference's predicted cache `[player+0x1d68]`
    /// (`0x6127b0`).
    pub(super) stand_pending: Option<u8>,
    /// Holding the body still, gravity off, after a teleport, summon or login until the
    /// destination's colliders stream in. Released by the terrain streamer on residency or at
    /// [`SETTLE_TIMEOUT`], never by ground contact, which a flyer or a swimmer never makes.
    pub(crate) settling: bool,
    /// When to give up settling (`Time::elapsed_secs`), pushed on while the world is stale or
    /// still loading.
    pub(crate) settle_deadline: f32,
    /// When the current settle hold began (`Time::elapsed_secs`), which the `sett` trace reports.
    pub(crate) settle_since: f32,
    /// The colliders under us may still be the map we left: the snap runs a whole `WorldStage`
    /// before the streamer swaps maps. Set at every snap, cleared once the destination is resident;
    /// while set, the streamer judges no release and pushes [`Player::settle_deadline`] on.
    pub(crate) world_stale: bool,
    /// The camera pitch the login seize seats: the saved pose's `cameraPitch` when
    /// [`super::camera_saved`] restored one, else the stock opening pitch.
    pub(super) login_pitch: Option<f32>,
    /// A same-map teleport voided the self server-ride: vmangos ignores the spline-done ack while
    /// the teleport is pending (`MovementHandler.cpp:819`), so `drive_self_ride` drops the ride
    /// without mirroring its stale pose and sends no `CMSG_MOVE_SPLINE_DONE`.
    pub(super) ride_abort: bool,
    /// A `MSG_MOVE_WORLDPORT_ACK` owed at the settle release: the reference sends it only after its
    /// blocking load returns (`0x401cae`, after `0x66fbe0`), and vmangos has no load timeout. A
    /// riding crossing never settles and acks at once.
    pub(crate) owes_worldport_ack: bool,
    /// `Time::elapsed_secs` when we last sent a heartbeat.
    pub(super) last_heartbeat: f32,
    /// Milliseconds of movement the settle hold skipped, sent as `CMSG_MOVE_TIME_SKIPPED` on the
    /// release edge; fractional because it accrues a frame `dt` at a time.
    pub(super) skipped_ms: f32,
    /// When the current airborne phase began (`Time::elapsed_secs`): the wire `fall_time`, and the
    /// take-off and landing edges that emit `MSG_MOVE_JUMP` and `MSG_MOVE_FALL_LAND`.
    pub(super) airborne_since: Option<f32>,
    /// At rest wedged between steep faces: standing while a close down-probe still finds support,
    /// until real ground, a jump or walking off into open air.
    pub(super) wedged: bool,
    /// Consecutive stalled airborne frames (see [`WEDGE_STALL_RATIO`]).
    pub(super) wedge_still: u8,
    /// On a certified steep contact rather than a walkable floor, the reference's `0x4000000`:
    /// riding a low edge's foot cone up, or following a surface down off a ledge. Counts as
    /// standing, since the straight-down ground probe sees only the steep face; re-earned each
    /// frame.
    pub(super) steep_support: bool,
    /// A hover grant owes a jump: the reference's handler `0x61a620` calls `CMovement::Jump(0)`
    /// (`0x61a630`) before setting the flag, and its disable arm calls `StartFalling` (`0x61a637`).
    /// Latched for the take-off site, since our mover re-derives ground contact every frame and
    /// would cancel a jump committed here.
    pub(super) hover_launch: bool,
    /// The knockback waiting to be flown, armed by `SMSG_MOVE_KNOCK_BACK` and taken at the take-off
    /// site with the jump and the hover launch.
    pub(super) knockback: Option<PendingKnockback>,
    /// The arc's take-off vertical speed (yd/s, WoW +Z up), the client's `StartFalling` argument
    /// (`+0xa0`) and the jump tail's `zspeed`: [`JUMP_SPEED`] for a jump, exactly 0 for a step-off.
    pub(super) jump_zspeed: f32,
    /// The launch vertical speed, recorded before this frame's gravity: [`Self::jump_zspeed`] is
    /// seeded from this, since `vel_y` is already a step down the arc when [`super::arc`] runs.
    pub(super) launch_vz: f32,
    /// Whether the arc's direction nibble (`[CMovement+0x40] & 0xf`) is set: air control opens only
    /// while it is clear (`0x7c6afc`), so a knockback, which plants FORWARD (`0x617a18`), never
    /// steers. Seeded at take-off, set by the nudge, cleared when the arc ends.
    pub(super) arc_dirs_set: bool,
    /// This arc is a knockback's, so its planted FORWARD rides the wire the whole arc (`0x617a18`;
    /// the send mask `0x75a07dff` keeps bit 0).
    pub(super) knock_arc: bool,
    /// Airborne at the end of the last mover step: the mover's own record, since
    /// [`Self::airborne_since`] is written by [`super::flags`], which runs after the mover.
    pub(super) airborne_prev: bool,
    /// The direction bits the current arc launched with: mid-air they are the actual motion, so the
    /// live flags read them instead of the keys; re-seeded by the air nudge, stale while grounded.
    pub(super) airborne_dirs: u32,
    /// Launch height (Bevy Y), the client's `StartFalling` z snapshot (`+0x7c`): the FALLINGFAR
    /// distance leg measures descent below it.
    pub(super) fall_start_y: f32,
    /// `MOVEFLAG_FALLINGFAR` latched for this arc ([`FALL_FAR_DROP`], [`FALL_FAR_TIME`]); only
    /// landing clears it, as the client's `StopFalling` does.
    pub(super) fall_far: bool,
    /// The facing (Bevy yaw, radians): the aim sent as orientation and the basis WASD move in.
    /// Right-drag and movement sync it to the camera, left-drag does not; the body is `model_yaw`.
    pub(super) face_yaw: f32,
    /// Swimming (`MOVEFLAG_SWIMMING`): the water over the feet crossed the swim-enter depth,
    /// latched with hysteresis ([`super::swim::update_swimming`]).
    pub(crate) swimming: bool,
    /// Our body's collision height, of which every swim depth line is a fraction. The component's
    /// own type, so `Player::default()` gives [`DEFAULT_COLLISION_HEIGHT`] rather than 0, which
    /// would swim on dry land.
    pub(crate) collision_height: crate::entities::CollisionHeight,
    /// The liquid surface over our feet as the last movement tick stored it (Bevy Y). The camera's
    /// water corridor reads this a frame late, as the reference's camera reads the tick's cache
    /// through the field accessor `0x670630`; the lag keeps its depth off a band edge.
    pub(crate) liquid_surface: Option<f32>,
    /// The mover pitch (radians, +up), `CMovement+0x20`, live in every mode: mouse-look sets it
    /// straight to the camera pitch within ±89° (`SetPitch` `0x7c6f70`, which stores before any
    /// swim test), and it holds when unsteered; only stop-swim and teleport zero it (`0x7c6e80`).
    /// Swimming reads it for travel, pose and the wire; on land, water walking does (`0x63161e`).
    pub(super) mover_pitch: f32,
    /// The camera pitch the last pitch push carried: the reference pushes only on mouse motion
    /// (`0x514400`), so a push here needs the aim to differ, and a still mouse leaves the drunk
    /// wobble and the stop-swim levelling (`0x7c6e80`) in place.
    pub(super) aim_pitch_seen: f32,
    /// This frame's flag-derived swim speed (yd/s), 0 without swim input: the stroke's playback
    /// numerator, as `0x5fe2f0` divides `GetCurrentSpeed` by the clip's speed. Stale on land.
    pub(super) swim_stroke_speed: f32,
    /// The rendered body heading (Bevy yaw): eased to the strafe offset while strafing
    /// ([`crate::creature_anim::strafe_body_offset`]), snapped to `face_yaw` while otherwise
    /// moving, and chasing at [`STATIONARY_CHASE_RATE`] while standing.
    pub(super) model_yaw: f32,
    /// A server spline drives the body (`SMSG_MONSTER_MOVE` for our guid: a charge, a taxi):
    /// input, physics and the movement stream yield, and its end sends `CMSG_MOVE_SPLINE_DONE`.
    pub(super) server_riding: bool,
    /// The `splineId` of the ride in progress (echoed in `CMSG_MOVE_SPLINE_DONE` when it ends).
    pub(super) ride_spline_id: u32,
    /// Whether the ride is a ground path (no `FLYING`), kept because its last frame has lost the
    /// spline and must still be grounded; a taxi's endpoint keeps its altitude.
    pub(super) ride_grounded: bool,
    /// Standing on a transport, the mover in its frame: attached on a
    /// [`crate::transport::Transport`] support, kept through deck jumps, detached on world ground,
    /// in water or when the transport despawns.
    pub(super) ride: Option<PlayerRide>,
}

/// The player's attachment to a transport's frame ([`Player::ride`]).
#[derive(PartialEq)]
pub(super) struct PlayerRide {
    /// The transport's entity, whose collider is the support that attached us.
    pub(super) entity: Entity,
    /// The transport's guid, which the wire's transport tail carries.
    pub(super) guid: u64,
    /// Feet in the transport's frame (Bevy axes), snapshotted at frame end; the next frame's carry
    /// recomposes the world position from it before input integrates.
    pub(super) local_pos: Vec3,
    /// The transport's yaw at the snapshot: the carry turns `face_yaw` by its per-frame delta, and
    /// the wire's local orientation is `face_yaw − boat_yaw`.
    pub(super) boat_yaw: f32,
}

/// A knockback aimed at our mover (`SMSG_MOVE_KNOCK_BACK`), latched until the mover flies it. The
/// ack is owed only if the launch happened and echoes `launch` as its jump tail: vmangos matches
/// all four floats within 0.01 (`Unit.cpp:7096-7100`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PendingKnockback {
    /// Our mover guid, echoed in the ack as a full u64 (it arrives packed).
    pub(super) guid: u64,
    /// The server's movement counter, echoed or the server rejects the ack.
    pub(super) counter: u32,
    /// The launch: world-XY direction, horizontal speed and vertical speed, down-positive
    /// (negative is up).
    pub(super) launch: JumpInfo,
}

impl Player {
    /// Take the jump a hover grant owes, clearing the latch: `Jump(0)` refuses only ROOT and
    /// FALLING (`0x7c625c`), skipping the hover test (`0x7c6236`), and a refused jump is dropped,
    /// not retried.
    pub(super) fn take_wire_jump(&mut self) -> bool {
        let fire = self.hover_launch && !self.modes.rooted && self.airborne_since.is_none();
        self.hover_launch = false;
        fire
    }

    /// Take the knockback, clearing the latch; the mover may still decline it (the settle hold, a
    /// root), and then it is dropped, not deferred, and no ack is sent.
    pub(super) fn take_knockback(&mut self) -> Option<PendingKnockback> {
        self.knockback.take()
    }

    /// Turn the aim by `radians`, the scripted mouse turn (`capture::probe_look`): the same
    /// `face_yaw` a real mouse turn writes and the facing stream diffs.
    pub(crate) fn turn_aim(&mut self, radians: f32) {
        self.face_yaw += radians;
    }

    /// Set the mover pitch, the scripted dive (`capture::probe_pitch`), under `SetPitch`'s ±89°
    /// clamp (`0x7c6f70`) but without its 2^-20 deadband (`[0x8026bc]`); returns the value stored.
    pub(crate) fn aim_pitch(&mut self, radians: f32) -> f32 {
        self.mover_pitch = radians.clamp(-MOUSELOOK_PITCH_CLAMP, MOUSELOOK_PITCH_CLAMP);
        // Mouse-look pushes on a camera-pitch edge; parking the edge here keeps a still camera
        // from overwriting the script next frame.
        self.aim_pitch_seen = self.mover_pitch;
        self.mover_pitch
    }

    /// The facing (Bevy yaw) sent as orientation, not the rendered `model_yaw`; the 3D-audio
    /// listener faces it, not the camera (`0x483430`).
    pub(crate) fn facing(&self) -> f32 {
        self.face_yaw
    }

    /// End the settle hold, called by the terrain streamer: `resident` is false when
    /// [`SETTLE_TIMEOUT`] fired first, which the `sett` trace records.
    pub(crate) fn end_settle(&mut self, resident: bool, now: f32) {
        self.settling = false;
        let waited = now - self.settle_since;
        super::move_trace::settle(resident, waited, self.pos);
    }

    /// The movement flags as last streamed; the water foam tests `& 0xf` (translating) and `& 0x30`
    /// (turning) on them, as the reference's ripple driver `0x5fa760` does.
    pub(crate) fn move_flags(&self) -> u32 {
        self.move_flags
    }

    /// The states on this resource that stop WASD, comma-joined, or `"none"`: an instrument for the
    /// log after a session boundary, not a gate. A stun lives on the descriptor and does not show.
    pub(crate) fn movement_suppressors(&self) -> String {
        let named = [
            (!self.active, "inactive"),
            (self.detached, "free-fly"),
            (self.modes.rooted, "root"),
            (self.control_lost, "control-lost"),
            (self.server_riding, "server-ride"),
            (self.foreign_mover.is_some(), "possession"),
            (self.reseat, "reseat-pending"),
        ];
        let list: Vec<&str> = named
            .into_iter()
            .filter_map(|(set, name)| set.then_some(name))
            .collect();
        if list.is_empty() {
            "none".to_string()
        } else {
            list.join(",")
        }
    }

    /// The commanded planar speed (yd/s), the reference's `[[player+0x118]+0x84]` from `0x7c4c90`:
    /// exactly 0 with no direction bit set, and live, not averaged like the weather wind's
    /// (`0x67c150`). The precipitation slab's tilt reads it (`0x67bf8b`).
    pub(crate) fn planar_speed(&self) -> f32 {
        if self.move_flags & 0xf == 0 {
            return 0.0;
        }
        self.horiz_vel.with_y(0.0).length()
    }

    /// The guid of the transport we stand on, for instruments.
    pub(crate) fn riding(&self) -> Option<u64> {
        self.ride.as_ref().map(|r| r.guid)
    }

    /// The transport we stand on as its entity, whose transform is the deck frame a rider's effects
    /// are stored in (`benilla_world::ride_frame`).
    pub(crate) fn ride_entity(&self) -> Option<Entity> {
        self.ride.as_ref().map(|r| r.entity)
    }

    /// Whether a server spline drives the avatar ([`super::server_ride`]), for instruments. Not
    /// `UnitOnTaxi`, which reads [`crate::player::UNIT_FLAG_TAXI_FLIGHT`] (`0x517a86`).
    pub(crate) fn server_riding(&self) -> bool {
        self.server_riding
    }
}

/// The forward/back axis, summed as the reference's emitter `0x514da0` sums it: autorun, forward
/// and both-button run +1 each, backward −1; its sign picks the START, and zero is a STOP.
pub(super) fn forward_axis(
    forward: bool,
    backward: bool,
    both_buttons: bool,
    autorun: bool,
) -> i32 {
    i32::from(forward) + i32::from(both_buttons) + i32::from(autorun) - i32::from(backward)
}

/// Whether this frame's input clears autorun: the four of the reference's six writers of `0x1000`
/// with an analog here. `fwd_down` and `back_down` are key-down edges (`0x514a5a`; a release
/// restores nothing), `both_engaged` the edge into both-button run (`0x514a73`), and `lost_mover` a
/// level, the translate gate down (`0x514748`). A jump, a chat box taking focus and a zone change
/// leave autorun on; mounting's effect in the reference is untraced, and here it leaves it on.
pub(super) fn autorun_cancelled(
    fwd_down: bool,
    back_down: bool,
    both_engaged: bool,
    lost_mover: bool,
) -> bool {
    fwd_down || back_down || both_engaged || lost_mover
}

/// Whether the reference's `SetStandState` (`0x5ed430`) refuses this change, returning before the
/// packet is built. A body that reads dead is refused either way; otherwise standing up never is,
/// and any other state is refused while translating or swimming (`0x20000f`, `0x5ed4f8`), SLEEP
/// also while turning (`0x5ed4e6`, which falls through into that test). FALLING is in neither
/// mask. It is the one setter, so the X key, the posture emotes and the AFK sit all go through it.
pub(super) fn stand_state_refused(reads_dead: bool, move_flags: u32, new_state: u8) -> bool {
    use crate::creature_anim::move_flags as f;
    // Health <= 0 (`0x5ed4ac`) or `UNIT_DYNAMIC_FLAGS & 0x20`, a feigner (`0x5ed4b2`), refuses in
    // both directions, so it comes before the stand-up exit.
    if reads_dead {
        return true;
    }
    // Standing up skips the movement word (`0x5ed4f0`).
    if new_state == 0 {
        return false;
    }
    // SLEEP's turn test falls through into the shared one (`0x5ed4ec`).
    if new_state == 3 && move_flags & (f::TURN_LEFT | f::TURN_RIGHT) != 0 {
        return true;
    }
    move_flags & (f::ANY_MOVE | f::SWIMMING) != 0
}

#[cfg(test)]
mod stand_state_tests {
    use super::stand_state_refused;
    use crate::creature_anim::move_flags as f;

    #[test]
    fn a_body_that_reads_dead_can_neither_sit_nor_stand() {
        for state in [0u8, 1, 2, 3, 8] {
            assert!(
                stand_state_refused(true, 0, state),
                "stand state {state} refused on a body that reads dead — standing up included, \
                 which is the one case the movement-word gate below would have let through"
            );
        }
        assert!(
            !stand_state_refused(false, 0, 1),
            "and the same still body, alive, is granted its sit — the guard is the death, not the \
             standing still"
        );
    }

    #[test]
    fn a_swimmer_cannot_sit_but_can_always_stand() {
        // Floating still: SWIMMING alone refuses.
        assert!(
            stand_state_refused(false, f::SWIMMING, 1),
            "sit refused mid-swim"
        );
        assert!(
            stand_state_refused(false, f::SWIMMING | f::FORWARD, 1),
            "swimming forward too"
        );
        // SIT, SIT_CHAIR and KNEEL.
        for state in [1u8, 2, 8] {
            assert!(
                stand_state_refused(false, f::SWIMMING, state),
                "state {state} refused mid-swim"
            );
        }
        // Standing up (0) skips the word.
        assert!(!stand_state_refused(false, f::SWIMMING | f::FORWARD, 0));
        assert!(!stand_state_refused(false, f::ANY_MOVE | f::SWIMMING, 0));
    }

    /// The turn bits are outside `0x20000f`.
    #[test]
    fn sitting_is_refused_while_translating_and_allowed_while_merely_turning() {
        assert!(
            !stand_state_refused(false, 0, 1),
            "standing still: sit granted"
        );
        for bit in [f::FORWARD, f::BACKWARD, f::STRAFE_LEFT, f::STRAFE_RIGHT] {
            assert!(
                stand_state_refused(false, bit, 1),
                "translating: sit refused"
            );
        }
        for bit in [f::TURN_LEFT, f::TURN_RIGHT] {
            assert!(
                !stand_state_refused(false, bit, 1),
                "turning in place: sit granted"
            );
        }
        // Mode bits are not movement.
        for bit in [f::ROOT, f::WATER_WALKING, f::FALLING, f::WALK_MODE] {
            assert!(
                !stand_state_refused(false, bit, 1),
                "mode bit {bit:#x} is not a move"
            );
        }
    }

    /// SLEEP (3) adds the `0x30` turn test and falls through into `0x20000f` (`0x5ed4ec`).
    #[test]
    fn sleep_takes_both_tests_so_it_is_the_strictest_posture() {
        // The turn test is SLEEP's alone.
        assert!(
            stand_state_refused(false, f::TURN_LEFT, 3),
            "turning: /sleep refused"
        );
        assert!(stand_state_refused(false, f::TURN_RIGHT, 3));
        assert!(
            !stand_state_refused(false, f::TURN_LEFT, 1),
            "turning blocks ONLY the sleep — a sit is granted"
        );
        // The shared test it falls into.
        assert!(
            stand_state_refused(false, f::SWIMMING, 3),
            "swimming: /sleep refused"
        );
        assert!(
            stand_state_refused(false, f::FORWARD, 3),
            "walking: /sleep refused"
        );
        // SLEEP's effective mask is the one-shot route's `0x20003f`.
        assert_eq!(
            f::TURN_LEFT | f::TURN_RIGHT | f::ANY_MOVE | f::SWIMMING,
            f::ROUTE_COMMITTED_MOVE,
            "`0x20003f`, byte-verified at 0x5fe6dc as well as 0x5ed4e6+0x5ed4f8"
        );
        // Standing up from SLEEP is still ungated.
        assert!(!stand_state_refused(false, f::ROUTE_COMMITTED_MOVE, 0));
        // FALLING is in neither mask.
        assert!(
            !stand_state_refused(false, f::FALLING, 3),
            "falling: /sleep granted"
        );
    }
}

#[cfg(test)]
mod autorun_tests {
    use super::{autorun_cancelled, forward_axis};

    /// The reference's axis table (`0x514da0`).
    #[test]
    fn the_axis_reproduces_the_verified_state_table() {
        // Autorun alone: +1.
        assert_eq!(forward_axis(false, false, false, true), 1);
        // Autorun, then W: the key-down cleared the bit, so the axis sees W alone; nothing is sent.
        assert_eq!(forward_axis(true, false, false, false), 1);
        // W, then autorun: 2 (`0x514da5`); only the sign is consumed.
        assert_eq!(forward_axis(true, false, false, true), 2);
        // Autorun, then S: the key-down cleared the bit, so -1.
        assert_eq!(forward_axis(false, true, false, false), -1);
        // S, then autorun: the toggle skips the clear and both stay live, so 0.
        assert_eq!(forward_axis(false, true, false, true), 0);
    }

    #[test]
    fn the_two_orders_differ_and_only_one_resumes() {
        // Autorun, then S.
        let mut autorun = true;
        if autorun_cancelled(false, true, false, false) {
            autorun = false;
        }
        assert_eq!(
            forward_axis(false, true, false, autorun),
            -1,
            "walks backward"
        );
        // Releasing S restores no bit.
        assert_eq!(
            forward_axis(false, false, false, autorun),
            0,
            "stops, does not resume"
        );

        // S held, then autorun: the toggle is not a directional set, so nothing clears it.
        let autorun = true;
        assert!(!autorun_cancelled(false, false, false, false));
        assert_eq!(
            forward_axis(false, true, false, autorun),
            0,
            "STOP with S still held"
        );
        // Releasing S resumes: the bit survived.
        assert_eq!(
            forward_axis(false, false, false, autorun),
            1,
            "resumes forward"
        );
    }

    #[test]
    fn both_button_run_shares_the_axis() {
        assert_eq!(forward_axis(false, false, true, false), 1);
        assert_eq!(
            forward_axis(false, true, true, false),
            0,
            "S nets the both-button run to a stop"
        );
        // Engaging both-button run clears autorun.
        assert!(autorun_cancelled(false, false, true, false));
    }

    #[test]
    fn a_jump_or_a_chat_line_is_not_in_the_cancel_set() {
        assert!(!autorun_cancelled(false, false, false, false));
        // Losing the mover clears it, as a level.
        assert!(autorun_cancelled(false, false, false, true));
    }
}

#[cfg(test)]
mod move_mode_tests {
    use super::{
        incapacitated_flags, view_is_out, MoveModes, MoverInput, Player, FEATHER_TERMINAL_VELOCITY,
        GRAVITY, HOVER_CLIMB_RATE, HOVER_HEIGHT, TERMINAL_VELOCITY,
    };
    use crate::creature_anim::move_flags as f;
    use benilla_protocol::MoveMode;

    #[test]
    fn each_granted_mode_rides_the_wire_word_as_its_own_bit() {
        for (mode, bit) in [
            (MoveMode::Root, f::ROOT),
            (MoveMode::WaterWalk, f::WATER_WALKING),
            (MoveMode::FeatherFall, f::SAFE_FALL),
            (MoveMode::Hover, f::HOVER),
        ] {
            let mut modes = MoveModes::default();
            assert_eq!(modes.wire_flags(), 0);
            modes.set(mode, true);
            assert_eq!(
                modes.wire_flags(),
                bit,
                "{mode:?} must ride the wire as {bit:#x}"
            );
            assert_eq!(
                mode.flag(),
                bit,
                "{mode:?}: our bit and the protocol's agree"
            );
            modes.set(mode, false);
            assert_eq!(modes.wire_flags(), 0, "{mode:?} revokes cleanly");
        }
    }

    /// Levitate (spell 1706) grants feather fall, hover and water walk at once.
    #[test]
    fn levitate_carries_its_three_modes_at_once() {
        let mut modes = MoveModes::default();
        for mode in [MoveMode::FeatherFall, MoveMode::Hover, MoveMode::WaterWalk] {
            modes.set(mode, true);
        }
        assert_eq!(
            modes.wire_flags(),
            f::SAFE_FALL | f::HOVER | f::WATER_WALKING
        );
        modes.set(MoveMode::Hover, false);
        assert_eq!(modes.wire_flags(), f::SAFE_FALL | f::WATER_WALKING);
    }

    #[test]
    fn the_wire_merge_never_unroots_us() {
        let mut modes = MoveModes {
            rooted: true,
            ..Default::default()
        };
        modes.merge_from_wire(0); // a pose claiming no modes at all
        assert!(modes.rooted, "only the ack'd opcode may unroot");
        assert!(!modes.hover && !modes.feather_fall && !modes.water_walking && !modes.levitating);

        // The other four follow the wire both ways, which is how `.cheat fly` arrives.
        modes.merge_from_wire(f::SAFE_FALL | f::HOVER | f::WATER_WALKING | f::LEVITATING);
        assert!(modes.hover && modes.feather_fall && modes.water_walking && modes.levitating);
        assert!(modes.rooted, "still ours alone");
    }

    /// Feather fall swaps the terminal velocity and leaves gravity alone (`0x7c5d20`).
    #[test]
    fn feather_fall_caps_the_descent_without_softening_gravity() {
        let dt = 1.0 / 60.0;
        let step = |v: f32, terminal: f32| (v - GRAVITY * dt).max(-terminal);

        assert_eq!(
            step(0.0, FEATHER_TERMINAL_VELOCITY),
            step(0.0, TERMINAL_VELOCITY),
            "the first frame of a slow fall is an ordinary fall"
        );

        let settle = |terminal: f32| {
            let mut v = 0.0;
            for _ in 0..600 {
                v = step(v, terminal);
            }
            v
        };
        assert_eq!(
            settle(FEATHER_TERMINAL_VELOCITY),
            -FEATHER_TERMINAL_VELOCITY
        );
        assert_eq!(settle(TERMINAL_VELOCITY), -TERMINAL_VELOCITY);

        // The cap is reached at t = 7.0 / 19.291105 ≈ 0.363 s.
        let mut v = 0.0f32;
        let mut frames = 0;
        while v > -FEATHER_TERMINAL_VELOCITY {
            v = step(v, FEATHER_TERMINAL_VELOCITY);
            frames += 1;
        }
        assert!(
            (frames as f32 * dt - 0.363).abs() < 0.02,
            "capped after {frames} frames ({:.3} s), expected ≈0.363 s",
            frames as f32 * dt
        );
    }

    #[test]
    fn the_mode_constants_are_the_verified_ones() {
        assert_eq!(FEATHER_TERMINAL_VELOCITY, 7.0); // [0x87d898]
        assert_eq!(HOVER_HEIGHT, 1.0); // [0x7ff9d8]
        assert_eq!(TERMINAL_VELOCITY, 60.148_003); // [0x87d894]
    }

    /// Feather fall is the slower cap.
    const _: () = assert!(FEATHER_TERMINAL_VELOCITY < TERMINAL_VELOCITY);

    /// The stun is `UNIT_FIELD_FLAGS` bit 18 (vmangos `UnitDefines.h:563`, the reference's
    /// `shr eax, 0x12`), not a movement flag.
    #[test]
    fn a_stun_is_not_a_root() {
        assert_eq!(crate::player::UNIT_FLAG_STUNNED, 1 << 18);
        assert_eq!(crate::player::UNIT_FLAG_STUNNED, 0x0004_0000);
        for mode in [
            MoveMode::Root,
            MoveMode::WaterWalk,
            MoveMode::FeatherFall,
            MoveMode::Hover,
        ] {
            assert_ne!(
                mode.flag(),
                crate::player::UNIT_FLAG_STUNNED,
                "{mode:?} is a MOVEMENTFLAGS bit; the stun gate is a descriptor bit"
            );
        }
        // A root alone is only the movement bit.
        let mut modes = MoveModes::default();
        modes.set(MoveMode::Root, true);
        assert_eq!(modes.wire_flags(), crate::creature_anim::move_flags::ROOT);
    }

    #[test]
    fn hover_climbs_to_its_clearance_rather_than_popping() {
        assert_eq!(HOVER_CLIMB_RATE, 7.0); // [0x87d898], via 0x7c61b0
        let dt = 1.0 / 60.0;
        let mut clearance = 0.0_f32;
        let mut frames = 0;
        while clearance < HOVER_HEIGHT {
            clearance = (clearance + HOVER_CLIMB_RATE * dt).min(HOVER_HEIGHT);
            frames += 1;
        }
        assert!(
            frames > 1,
            "a snap would arrive in one frame; this must not"
        );
        assert!(
            (frames as f32 * dt - HOVER_HEIGHT / HOVER_CLIMB_RATE).abs() < 0.02,
            "climbed in {frames} frames, expected ≈0.143 s"
        );
    }

    /// `Jump(0)` at `0x61a630` is refused by ROOT and FALLING (`0x7c625c`), never by hover
    /// (`0x7c6236`).
    #[test]
    fn a_hover_grant_owes_one_jump_and_only_root_or_falling_eats_it() {
        let granted = || Player {
            hover_launch: true,
            modes: MoveModes {
                hover: true,
                ..Default::default()
            },
            ..Default::default()
        };

        let mut p = granted();
        assert!(p.take_wire_jump(), "hover does not refuse its own grant");
        assert!(
            !p.take_wire_jump(),
            "one opcode, one jump — the latch is consumed, not level-triggered"
        );

        let mut rooted = granted();
        rooted.modes.rooted = true;
        assert!(!rooted.take_wire_jump(), "ROOT — `0x7c625c test ah,0x30`");
        assert!(
            !rooted.hover_launch,
            "a refused Jump returns failure and nothing retries it (`0x7c6288 xor eax,eax`)"
        );

        let mut falling = granted();
        falling.airborne_since = Some(0.0);
        assert!(
            !falling.take_wire_jump(),
            "FALLING — the same test's other bit"
        );

        let mut ungranted = Player::default();
        assert!(!ungranted.take_wire_jump(), "no opcode, no jump");
    }

    /// Stand states 1-6: sitting, the chairs (the client's 2, the server's 4, 5 and 6) and sleep.
    #[test]
    fn a_seated_body_refuses_the_mouse_turn_and_keeps_the_keyboard_one() {
        let m = MoverInput {
            dead: false,
            view_is_out: false,
        };
        for seated in [1u8, 2, 3, 4, 5, 6] {
            assert!(
                !m.mouse_may_turn_body(false, seated),
                "stand state {seated}: the mouse hand-off is refused at `0x51460c`"
            );
            assert!(
                m.may_turn(false),
                "stand state {seated}: the keyboard turn is not refused — it stands the body"
            );
        }
        assert!(
            m.mouse_may_turn_body(false, 0),
            "standing: the hand-off runs"
        );
        assert!(
            !m.mouse_may_turn_body(true, 0),
            "stunned: refused through `0x5145b0` before the stand state is even read"
        );
        assert!(
            !MoverInput {
                dead: true,
                view_is_out: false
            }
            .mouse_may_turn_body(false, 0),
            "dead: refused through the shared precondition `0x5144e0`"
        );
    }

    #[test]
    fn death_drops_both_movement_input_predicates() {
        // (dead, rooted, stunned) -> (may_translate, may_turn)
        let gate = |dead, rooted, stunned| {
            let m = MoverInput {
                dead,
                view_is_out: false,
            };
            (m.may_translate(rooted), m.may_turn(stunned))
        };

        assert_eq!(
            gate(false, false, false),
            (true, true),
            "alive and free: both predicates pass and every input applies"
        );
        assert_eq!(
            gate(false, true, false),
            (false, true),
            "a PURE root (Frost Nova) takes translation and leaves the pivot — `0x514560` alone"
        );
        assert_eq!(
            gate(false, false, true),
            (true, false),
            "a stun takes the pivot and, by itself, nothing else — `0x5145b0` alone"
        );
        assert_eq!(
            gate(true, false, false),
            (false, false),
            "DEAD, with neither a root nor a stun granted anywhere: both predicates fail their \
             shared precondition `0x5144e0` on health at `0x5144f8`. The turn half is the bug — a \
             corpse is stunned as far as the input tick is concerned, so it cannot be spun with \
             the turn keys or a right-drag, and it does not need the server's root to say so."
        );
        assert_eq!(
            gate(true, true, true),
            (false, false),
            "and death is not additive with either: it is already both"
        );
    }

    /// The teardown leg (`0x5146d6`).
    #[test]
    fn only_both_predicates_down_tears_the_follow_down() {
        assert!(
            !MoverInput::default().torn_down(true, false),
            "a PURE root does NOT end a follow — translate is down, the turn is still up"
        );
        assert!(
            !MoverInput::default().torn_down(false, true),
            "and neither does a pure stun — the emitter never consults `0x5145b0`"
        );
        assert!(
            MoverInput {
                dead: true,
                ..Default::default()
            }
            .torn_down(false, false),
            "DEATH ends it, with nothing granted: it takes both predicates down by itself"
        );
        assert!(
            MoverInput::default().torn_down(true, true),
            "and so does Ice Block, which is root and stun at once"
        );
    }

    /// `0x5144e0`'s far-sight term, read only when the mover is the active player (`0x514537`).
    #[test]
    fn far_sight_freezes_your_own_body_but_never_a_possessed_one() {
        let driving = |driving_own_body, far_sight_engaged| {
            let m = MoverInput {
                dead: false,
                view_is_out: view_is_out(driving_own_body, far_sight_engaged),
            };
            (m.may_translate(false), m.may_turn(false))
        };

        assert_eq!(
            driving(true, false),
            (true, true),
            "my own body, no far sight: the ordinary frame, both predicates pass"
        );
        assert_eq!(
            driving(true, true),
            (false, false),
            "MIND VISION — my view is out and the body is mine, so `0x5144e0` fails and takes the \
             walk and the turn together, exactly as death does"
        );
        assert_eq!(
            driving(false, true),
            (true, true),
            "MIND CONTROL — the latch is engaged (possession sets the same field), but the mover \
             is not the active player, so `0x51453e` returns the conjunct satisfied and the victim \
             stays drivable. Dropping the `IsActivePlayer` half freezes it."
        );
        assert!(
            MoverInput {
                dead: false,
                view_is_out: true,
            }
            .torn_down(false, false),
            "and it ends a follow, with neither a root nor a stun anywhere — `0x5146d6` needs both \
             predicates down, and a term in the precondition they SHARE is both by construction. \
             Engaging Mind Vision mid-follow stops you following, for the same reason dying does."
        );
    }

    #[test]
    fn the_incapacitate_suppressions_take_exactly_their_own_bits() {
        let keys = f::FORWARD | f::STRAFE_LEFT | f::TURN_LEFT;
        let modes = f::ROOT | f::SWIMMING | f::SAFE_FALL | f::HOVER | f::WATER_WALKING;

        assert_eq!(
            incapacitated_flags(keys | modes, false, false),
            keys | modes,
            "neither gate: the word passes through untouched"
        );
        assert_eq!(
            incapacitated_flags(keys | modes, true, false),
            f::TURN_LEFT | modes,
            "a PURE root (Frost Nova) takes the direction bits and leaves the pivot — turn is on \
             the reference's allow-list on purpose"
        );
        assert_eq!(
            incapacitated_flags(keys, false, true),
            f::FORWARD | f::STRAFE_LEFT,
            "the two are keyed on different state: the stun bit alone takes only the pivot"
        );
        // A stun is both: vmangos's `HandleAuraModStun` sets `UNIT_FLAG_STUNNED` and roots
        // (`SpellAuras.cpp:3523`, `:3542`).
        let blocked = incapacitated_flags(keys | modes, true, true);
        assert_eq!(
            blocked, modes,
            "Ice Block leaves NO reported motion — which is what starves the anim resolver"
        );
        // The modes ride on, or the server's next echo clears them.
        assert_eq!(blocked & f::ROOT, f::ROOT, "the root bit itself rides on");
        assert_eq!(
            blocked & f::SWIMMING,
            f::SWIMMING,
            "a rooted swimmer is still swimming — `0xffe07f00` preserves 0x200000"
        );
    }
}
