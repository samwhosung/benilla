//! The pure animation selection: `0x5fd8b0`'s gait picks, the one-shot tables, the rate math.

use benilla_assets::AnimClip;
use bevy::prelude::*;

use crate::net::{RemoteMotion, Spline};

/// Walk speed (yd/s) for a unit whose movement block has none; Run starts above 2× (`0x5fd224`).
pub(super) const DEFAULT_WALK_SPEED: f32 = 2.5;

/// At or above this speed (yd/s) a unit plays Sprint (143) over Run (`[0x80c484]`, `0x5fd202`).
const FAST_RUN_SPEED: f32 = 11.0;

/// Below this ground speed (yd/s) a streamed mover counts as standing still.
const MOVING_EPSILON: f32 = 0.1;

pub(super) const STAND: u16 = 0;

pub(super) const DEATH: u16 = 1;

/// ShuffleLeft (11): once armed it plays out its clip, not ending with the turn (`0x719370`).
pub(super) const SHUFFLE_LEFT: u16 = 11;
/// See [`SHUFFLE_LEFT`].
pub(super) const SHUFFLE_RIGHT: u16 = 12;

/// StealthWalk (119), gated with [`STEALTH_STAND`] on `UNIT_FIELD_BYTES_1` byte 3's CREEP bit
/// (`0x5fd1d3`), ahead of the whole speed tail and not rate-scaled. Translucency is the stealth
/// aura's own kit (CharProc 14); this flag is only the pose.
pub(super) const STEALTH_WALK: u16 = 119;

/// StealthStand (120), from the chain's last resolver `0x5fd830`, so every other one outranks it.
pub(super) const STEALTH_STAND: u16 = 120;

/// Mount (91), the rider's seat whatever the unit does: the client arms it outside the selector
/// (`0x607a00`, `0x5fe2f0`); benilla pins the gait slot, and the locomotion plays on the mount.
pub(super) const MOUNT: u16 = 91;

/// Loot (50), the kneel while a loot window is open: the loot leg `0x5fd260`, after locomotion
/// and ahead of the cast pin (`0x5fd2e0`) and the Ready idle (`0x5fd360`), declining when mounted
/// or without the `[+0xd58] & 0x40` enable. It needs a session (`0x6126b0`: self, the loot latch;
/// remote, [`UNIT_FLAG_LOOTING`]) and a kneelable target (`0x612710`: self, the target's kind;
/// remote, [`UNIT_FLAG_LOOT_SUPPRESS`] clear).
pub(super) const LOOT: u16 = 50;

/// `UNIT_FLAG_LOOTING` (`0x400`), a remote unit's loot-kneel predicate (`0x6126db`). vmangos sets
/// it for corpse loot only (`Player.cpp:8148`), so others never see a chest kneel.
pub(super) const UNIT_FLAG_LOOTING: u32 = 0x400;

/// `UNIT_FIELD_FLAGS` bit `0x1000_0000`, which must be clear for a remote unit's loot kneel
/// (`0x612710`); its name and role are inferred. vmangos never sets it (`UNIT_FLAG_UNK_28`).
pub(super) const UNIT_FLAG_LOOT_SUPPRESS: u32 = 0x1000_0000;

/// MountSpecial (94), the mount rearing: a one-shot on the mount model (`0x5fe2f0` routes it).
pub(crate) const MOUNT_SPECIAL: u16 = 94;

/// Knockdown (121), with [`LIFT_OFF`] and [`LAND`] the ids whose arm takes the base-animation lock
/// (`0x5fdba0`, table `0x5fdd90`): `PlayAnimation` refuses every base request while one holds.
pub(crate) const KNOCKDOWN: u16 = 121;
/// See [`KNOCKDOWN`]: the taxi pair takes the same lock under the reference's other bit.
pub(crate) const LIFT_OFF: u16 = 192;
/// See [`KNOCKDOWN`].
pub(crate) const LAND: u16 = 200;

/// The client's `MOVEMENTFLAGS` bits (cached at `unit+0x9e8`), tested as the binary tests them.
pub(crate) mod move_flags {
    pub const FORWARD: u32 = 0x1;
    pub const BACKWARD: u32 = 0x2;
    pub const STRAFE_LEFT: u32 = 0x4;
    pub const STRAFE_RIGHT: u32 = 0x8;
    /// The keyboard turn keys (with `TURN_RIGHT`), which rotate the facing (`0x7c4f30`).
    pub const TURN_LEFT: u32 = 0x10;
    pub const TURN_RIGHT: u32 = 0x20;
    /// Walk mode: it scales only the rate numerator, never the id choice.
    pub const WALK_MODE: u32 = 0x100;
    /// `MOVEFLAG_LEVITATING`: the swim decision `0x6030c0` bails on it first (`0x6030d2`), so
    /// liquid can neither latch nor clear [`SWIMMING`]; GM flight sets both (`Player.cpp:4595`).
    pub const LEVITATING: u32 = 0x400;
    /// JUMPING/FALLING: set for the whole airborne arc.
    pub const FALLING: u32 = 0x2000;
    /// `MOVEFLAG_FALLINGFAR` (`0x633220`/`0x633240`): a jump latches it 1/9 yd below its launch
    /// height, a step-off fall after 500 ms airborne; only StopFalling (`0x7c6290`) clears it.
    pub const FALLING_FAR: u32 = 0x4000;
    /// `MOVEFLAG_ROOT`: vmangos kicks a root-apply ack without it unless the ack still falls
    /// (`MovementHandler.cpp:715-722`); a moving bit beside it freezes the 1.12 client.
    pub const ROOT: u32 = 0x1000;
    pub const SWIMMING: u32 = 0x20_0000;
    /// `MOVEFLAG_ONTRANSPORT` (`MovementInfo.h:56`, not TBC's 0x200): every packet carrying it
    /// also carries the transport-local pose.
    pub const ON_TRANSPORT: u32 = 0x0200_0000;
    /// `MOVEFLAG_WATERWALKING`: liquid is walkable ground; read by the liquid-mask selector
    /// `0x6315f0` (`0x63160d`, only when not swimming) and the opcode 0x22 apply `0x61a430`.
    pub const WATER_WALKING: u32 = 0x1000_0000;
    /// `MOVEFLAG_SAFE_FALL`, feather fall: its one effect is the terminal fall speed (`0x7c5d23`),
    /// 7.0 yd/s (`[0x87d898]`) instead of 60.148 (`[0x87d894]`).
    pub const SAFE_FALL: u32 = 0x2000_0000;
    /// `MOVEFLAG_HOVER`: the body rests 1.0 yd above the ground (Levitate): the walk resolver
    /// `0x6367b0` adds the `[0x7ff9d8]` offset and the step-down reach widens by it (`0x633e35`).
    pub const HOVER: u32 = 0x4000_0000;

    /// The bits a server-authored move packet owns, the reference's merge mask (`0x618c30` @
    /// `0x618deb`). [`ON_TRANSPORT`] is outside it: a server pose relocates a rider but never
    /// boards or deboards them.
    pub const SERVER_AUTHORED: u32 = 0x75a0_7dff;

    /// Merge a server-authored packet's flags under [`SERVER_AUTHORED`] (`0x618de7`), for our mover
    /// and every watched one: all thirty relay opcodes take this mask (`0x618f20`).
    pub const fn merge_server_authored(local: u32, wire: u32) -> u32 {
        (local & !SERVER_AUTHORED) | (wire & SERVER_AUTHORED)
    }

    /// Any horizontal direction bit: the client's `[9e8] & 0xf` gate.
    pub const ANY_MOVE: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;

    /// The bits that get a mover integrated at all, the client's `0x20ff` (`0x616e20`, `0x6166f5`):
    /// without one a mover keeps its last packet's pose, and a flag-less unit is not even in the
    /// mover list (`0x618940`). Swim and the mode bits are not among them.
    pub const INTEGRATED: u32 = 0xff | FALLING;

    /// The committed-lower-body test routing a one-shot to the masked overlay (`0x5fe6dc`); the
    /// client's separate mouse-turn test (`d58 & 0x1800`) reads a field benilla does not model.
    pub const ROUTE_COMMITTED_MOVE: u32 =
        FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT | TURN_LEFT | TURN_RIGHT | SWIMMING;

    /// The stationary-cast pin's moving test (`0x5fde80`): no turn bits, so a caster turning in
    /// place keeps the pin, where [`ROUTE_COMMITTED_MOVE`]'s turn bits would flap it.
    pub const CAST_PIN_MOVE: u32 = ANY_MOVE | SWIMMING;
}

/// The rate (/s) the strafing yaw eases at: the reference closes a quarter of the gap per frame
/// (`0x607ed0`, `[0x8029b0]`), 17.3/s at 60 fps. Deviation: a time-based rate, because the
/// reference's per-frame step changes with the frame rate.
const STRAFE_BLEND_RATE: f32 = 17.26;

/// Ease the rendered yaw toward `aim + offset` through the aim-relative offset, which swings round
/// the front where the absolute shortest arc ties at the ±90° strafe flip.
pub(crate) fn ease_strafe_yaw(current_yaw: f32, aim: f32, offset: f32, dt: f32) -> f32 {
    let cur = super::wrap_pi(current_yaw - aim);
    let eased = cur + (offset - cur) * (1.0 - (-STRAFE_BLEND_RATE * dt).exp());
    super::wrap_pi(aim + eased)
}

/// The strafe body heading's offset from the aim (`0x607ed0`): ±π/2, ±π/4 with forward or back,
/// left positive, mirrored while backpedaling. Both strafe bits give 0, as they cancel to no
/// lateral motion, where the client's fold keys on the left bit alone: an unobservable edge.
pub(crate) fn strafe_body_offset(flags: u32) -> f32 {
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
    let left = flags & move_flags::STRAFE_LEFT != 0;
    let right = flags & move_flags::STRAFE_RIGHT != 0;
    if left == right {
        return 0.0;
    }
    let diagonal = flags & (move_flags::FORWARD | move_flags::BACKWARD) != 0;
    let magnitude = if diagonal { FRAC_PI_4 } else { FRAC_PI_2 };
    let back = flags & move_flags::BACKWARD != 0;
    if left != back {
        magnitude
    } else {
        -magnitude
    }
}

/// The swim body pitch for every mover with a reported pitch (`0x60a110` to `0x710620`): the
/// reference's `Rz(facing)·Ry(2π − pitch)` is `Ry(yaw)·Rx(pitch)` here, nose up positive, and
/// only SWIMMING with forward or back pitches (`CMovement+0x40 & 3` at `0x60857d`).
pub(crate) fn swim_body_rotation(yaw: f32, flags: u32, pitch: f32) -> Quat {
    if flags & move_flags::SWIMMING != 0
        && flags & (move_flags::FORWARD | move_flags::BACKWARD) != 0
    {
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
    } else {
        Quat::from_rotation_y(yaw)
    }
}

/// The per-frame movement state the selector `0x5fd8b0` reads.
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct MovementState {
    /// Live directional speed (yd/s); for a swimmer the flag-scalar swim speed whatever its pitch.
    pub(crate) speed: f32,
    /// Vertical speed (yd/s, up); its sign on the first airborne frame tells a jump from a fall.
    pub(crate) vertical_speed: f32,
    /// `MOVEMENTFLAGS` ([`move_flags`]).
    pub(crate) flags: u32,
    /// `UNIT_FIELD_BYTES_1` byte 0: 0 Stand, 1 Sit, 3 Sleep, 4/5/6 chair low/medium/high, 8 Kneel.
    pub(crate) stand_state: u8,
    /// The CREEP bit, stamped from the unit's own descriptor every frame, self included.
    pub(crate) stealthed: bool,
    /// On a flying server spline (a taxi): the selector plays Fly 135 on its Flying bit 0x200
    /// (`0x5fd19c`); [`unify`] stamps it from the live [`Spline`].
    pub(crate) flying: bool,
}

/// A bracketed animation state: an enter one-shot, a loop, an exit one-shot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Special {
    /// JumpStart (37), armed directly by `MSG_MOVE_JUMP` (`0x60e480` case 0xbb), then the Jump (38)
    /// hang once 37's clip ends; a step-off fall never enters here.
    Jump,
    /// The Fall (40) loop from the tick FALLINGFAR latches (`0x602c40`), with no enter one-shot.
    Fall,
    /// A stand-state pose (1 Sit, 3 Sleep, 8 Kneel): down, loop, up.
    Pose(u8),
}

impl Special {
    /// The entry one-shot; Fall has none, so its arm here is unreachable.
    pub(super) fn enter(self) -> u16 {
        match self {
            Special::Jump => 37,
            Special::Fall => 40,
            Special::Pose(1) => 96,
            Special::Pose(3) => 99,
            Special::Pose(8) => 114,
            Special::Pose(_) => STAND,
        }
    }
    /// The looping middle.
    pub(super) fn loop_id(self) -> u16 {
        match self {
            Special::Jump => 38,
            Special::Fall => 40,
            Special::Pose(1) => 97,
            Special::Pose(3) => 100,
            Special::Pose(8) => 115,
            Special::Pose(_) => STAND,
        }
    }
    /// A pose's exit one-shot; a landing is [`jump_land_pick`]'s, so the airborne arm is unused.
    pub(super) fn exit(self) -> u16 {
        match self {
            Special::Jump | Special::Fall => STAND,
            Special::Pose(1) => 98,
            Special::Pose(3) => 101,
            Special::Pose(8) => 116,
            Special::Pose(_) => STAND,
        }
    }

    /// Whether moving cuts the exit short: a pose's stand-up yields to the gait cross-fade at once.
    pub(super) fn interruptible_by_move(self) -> bool {
        matches!(self, Special::Pose(_))
    }
}

/// The landing one-shot from the flags at touchdown (`0x602c60`): JumpEnd 39 stopped, none
/// swimming, backpedaling or walking, else JumpLandRun 187. A rooted arc end is no landing:
/// `SetRoot` (`0x7c7340`) stops the fall in mid-air and the reference sends no land packet
/// (`0x602df3` gating opcode `0xc9`), the packet this dispatcher runs on.
pub(super) fn jump_land_pick(flags: u32) -> Option<u16> {
    use move_flags::*;
    if flags & (SWIMMING | ROOT) != 0 {
        None
    } else if flags & ANY_MOVE == 0 {
        Some(39)
    } else if flags & (BACKWARD | WALK_MODE) == 0 {
        Some(187)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Mode {
    /// Cross-faded ground/swim gaits + idle.
    Gait,
    /// Playing a Special's enter one-shot; settles into its loop when the one-shot finishes.
    Entering(Special),
    /// Looping a Special; plays the exit one-shot when its condition clears.
    Looping(Special),
    /// A pose's exit one-shot; back to Gait when done unless a Special or movement interrupts.
    Exiting(Special, u16),
    /// The jump landing ([`jump_land_pick`]), re-picked the instant a movement flag changes; it
    /// plays through to Gait only if they hold.
    Land { id: u16, flags: u32 },
    /// An action one-shot full-body on bone 0 (standing, or a mid-air cast or emote); `under` is
    /// the Special it replaced. Airborne, a finished clip holds and only a Special edge exits (the
    /// FALLINGFAR Fall, once at `0x61a820`, or the landing); grounded, any movement-flag change
    /// since the base was armed re-picks at once, as the client's base re-arm overwrites bone 0.
    Swing { id: u16, under: Option<Special> },
}

/// The ids a unit in [`Mode::Gait`] tries, most specific first: the `0x5fd8b0` chain without its
/// Special states. `ready` is the engaged idle, which locomotion outranks.
pub(super) fn gait_candidates(
    state: &MovementState,
    walk_speed: f32,
    ready: Option<u16>,
    ranged_load: Option<u16>,
) -> &'static [u16] {
    use move_flags::*;
    let f = state.flags;
    // Swimming (`0x5fd100`): turn, then strafe, then backward, then forward.
    if f & SWIMMING != 0 {
        return if f & (TURN_LEFT | TURN_RIGHT) != 0 {
            &[41, 0]
        } else if f & STRAFE_LEFT != 0 {
            &[43, 42, 41, 0]
        } else if f & STRAFE_RIGHT != 0 {
            &[44, 42, 41, 0]
        } else if f & BACKWARD != 0 {
            &[45, 41, 0]
        } else if f & FORWARD != 0 {
            &[42, 41, 0]
        } else {
            &[41, 0]
        };
    }
    // A flying spline, the taxi (`0x5fd19c`): Fly 135 ahead of backward and speed.
    if state.flying {
        return &[135, 0];
    }
    // Backward dominates strafe (`0x5fd1bc`, `[9e8] & 2` → WalkBackwards 13).
    if f & BACKWARD != 0 {
        return &[13, 4, 0];
    }
    // Stealthed and moving, the prowl (`0x5fd1d3`); Walk is row 119's AnimationData fallback.
    if state.stealthed && f & ANY_MOVE != 0 {
        return &[STEALTH_WALK, 4, 0];
    }
    // Ground gaits by live speed (`0x5fd202`/`0x5fd224`): Sprint at 11 and above, Run above twice
    // walk, else Walk. Strafe takes these too: the reference has no ground strafe branch.
    if f & ANY_MOVE != 0 {
        let s = state.speed;
        return if s >= FAST_RUN_SPEED {
            &[143, 5, 4, 0]
        } else if s > 2.0 * walk_speed {
            &[5, 4, 0]
        } else {
            &[4, 0]
        };
    }
    // Turning in place: the shuffle (`0x5fd3f0`, 11 left, 12 right).
    if f & TURN_LEFT != 0 {
        return &[SHUFFLE_LEFT, STAND];
    }
    if f & TURN_RIGHT != 0 {
        return &[SHUFFLE_RIGHT, STAND];
    }
    // Standing and engaged: the Ready idle (`0x5fd360`), gated on engagement, not the sheath.
    if let Some(r) = ready {
        return match r {
            26 => &[26, 25, 0],
            27 => &[27, 25, 0],
            28 => &[28, 25, 0],
            _ => &[25, 0],
        };
    }
    // Auto-repeat in the ranged stance (`0x5fd460`), ahead of the chair loops (`0x5fd550`).
    if let Some(l) = ranged_load {
        // A finished Load becomes its Hold through the completion dispatch (`0x5fc3f0`, fired at
        // `0x7075af`); each Hold falls back to its Load, so a model without it holds full draw.
        return match l {
            105 => &[105, 25, 0],
            106 => &[106, 25, 0],
            109 => &[109, 105, 25, 0],
            110 => &[110, 106, 25, 0],
            111 => &[111, 25, 0],
            112 => &[112, 25, 0],
            _ => &[25, 0],
        };
    }
    // Standing: a chair loop (4/5/6), else the prowl idle, else Stand; state 2 (`SIT_CHAIR`)
    // writes no override in the reference (its row declines, `0x5fd644`), so it stands.
    match state.stand_state {
        4 => &[102, 0],
        5 => &[103, 0],
        6 => &[104, 0],
        _ if state.stealthed => &[STEALTH_STAND, STAND],
        _ => &[STAND],
    }
}

/// The local auto-repeat standing idle by the ranged item's subclass (`0x5fd460`, LUT `0x5fd530`).
pub(super) fn ranged_load_anim(ranged: Option<(u8, u8)>) -> u16 {
    match ranged {
        Some((2, 2)) => 105,           // Bow → LoadBow
        Some((2, 3) | (2, 18)) => 106, // Gun / Crossbow → LoadRifle
        Some((2, 16)) => 112,          // Thrown → LoadThrown
        Some((2, 19)) => 111,          // Wand → HoldThrown
        _ => 25,                       // no/odd ranged item → ReadyUnarmed
    }
}

/// Whether `id` is a ranged Load clip, played once to full draw rather than looped.
pub(super) fn is_ranged_load(id: u16) -> bool {
    matches!(id, 105 | 106 | 112)
}

/// The Hold a finished ranged Load becomes, unconditionally (`0x5fc3f0` slot 11, a bare call at
/// `0x5fc5e9`); the `[+0xd24]` and `[+0xd58] & 0x600` test at `0x5fc5bc` is the Hold's own
/// re-arm (slot 13). The Hold alone in the cycle is authored as a loop.
pub(super) fn ranged_hold_anim(load: u16) -> u16 {
    match load {
        105 => 109,       // LoadBow → HoldBow
        106 => 110,       // LoadRifle → HoldRifle
        112 | 111 => 111, // LoadThrown → HoldThrown; the wand's hold re-arms itself
        other => other,
    }
}

/// Whether the drawn ranged idle owns the standing pose: `0x5fd460` claims on sheath CUR == 2
/// (`0x5fd463`) and the auto-repeat bit `[+0xd58] & 0x200` (`0x5fd476`) alone, a bit only the
/// local cast-send sets (`0x6e593b`). `0x400` ([`super::RangedHold`]) is not tested here, only by
/// `0x5fc3f0`'s Hold sustain.
pub(super) fn ranged_idle_gate(auto_repeat: bool, sheath_cur: Option<u8>) -> bool {
    sheath_cur == Some(2) && auto_repeat
}

/// The victim's defense reaction (`$CPP`, `0x624a01` to `0x60ec00`); a parry picks by the mainhand
/// through the LUT `0x60ec98` and bails with no clip for anything else (`0x60ec3a`).
pub(super) fn defense_anim(victim_state: u32, main: Option<(u8, u8)>) -> Option<u16> {
    match victim_state {
        2 | 8 => Some(30), // DODGE / DEFLECTS → Dodge
        5 => Some(24),     // BLOCKS → ShieldBlock
        3 => match main {
            Some((2, sub)) => match sub {
                0 | 4 | 7 | 0xb | 0xe | 0xf => Some(21), // 1H family + misc + dagger
                1 | 5 | 8 | 0xc => Some(22),             // 2H family
                6 | 0xa | 0x11 => Some(23),              // polearm / staff / spear
                0xd => Some(20),                         // fist → ParryUnarmed
                _ => None,                               // ranged/obsolete/oddball: bail
            },
            _ => None, // empty or non-weapon mainhand: bail
        },
        _ => None,
    }
}

/// `PlayAnimation`'s substitution (`0x5fe2f0` @ `0x5fe3cc`–`0x5fe3e9`): Special1H/2H (57/58)
/// becomes SpecialUnarmed (118) when both hands are empty; a non-weapon in hand keeps the clip.
pub(super) fn unarmed_special(id: u16, main: Option<(u8, u8)>, off: Option<(u8, u8)>) -> u16 {
    if matches!(id, 57 | 58) && main.is_none() && off.is_none() {
        118
    } else {
        id
    }
}

/// The melee swing one-shot ids, the only clips the whiff slow-down touches on the overlay.
pub(super) fn is_swing_id(id: u16) -> bool {
    matches!(id, 16..=19 | 85 | 87 | 88 | 117)
}

/// The client's COMBAT classifier (`0x5fcc10`): a combat clip requested over another is not
/// armed; the playing one's rate doubles and the request defers.
pub(super) fn is_combat_anim(id: u16) -> bool {
    matches!(id, 10 | 16..=24 | 30 | 36 | 57..=59 | 85..=88 | 95 | 117 | 118)
}

/// The client's CAST classifier (`0x5fcbb0`); with [`is_combat_anim`] it is the transplant test
/// (`0x5feae0`): a locomotion request lifts such a bone-0 clip to the key bone, not replacing it.
pub(super) fn is_cast_anim(id: u16) -> bool {
    matches!(id, 2 | 32 | 33 | 53 | 54)
}

/// Whether `cands` is bare Stand, the one slot the state-emote idle (`UNIT_NPC_EMOTESTATE`) may
/// fill. The prowl idle counts as bare: the reference's emote-state resolver (`0x5fd770`) runs
/// before the fallback idle (`0x5fd830`) that picks it.
pub(super) fn is_bare_stand(cands: &[u16]) -> bool {
    cands == [STAND] || cands == [STEALTH_STAND, STAND]
}

/// The state-emote idle's candidates, Stand as the fallback; only for a bare-Stand frame.
pub(super) fn state_emote_gait(emote_anim: u16) -> [u16; 2] {
    [emote_anim, STAND]
}

/// The mainhand swing id (`0x6246a0`); anything but an equipped melee weapon swings unarmed (16).
pub(super) fn swing_anim_main(wielded: Option<(u8, u8)>) -> u16 {
    match wielded {
        Some((2, sub)) => match sub {
            0x0 | 0x4 | 0x7 | 0xb | 0xe => 17, // Attack1H: 1H axe/mace/sword/exotic/misc
            0x1 | 0x5 | 0x8 | 0xc => 18,       // Attack2H: 2H axe/mace/sword/exotic
            0x6 | 0xa | 0x11 | 0x14 => 19,     // Attack2HL: polearm/staff/spear/fishing pole
            0xf => 85,                         // Attack1HPierce: dagger
            _ => 16,                           // AttackUnarmed: fist/ranged/wand/obsolete/unknown
        },
        _ => 16,
    }
}

/// The offhand swing id (HitInfo bit `0x4`): dagger 88, any other weapon 87, otherwise 117.
pub(super) fn swing_anim_off(wielded: Option<(u8, u8)>) -> u16 {
    match wielded {
        Some((2, 0xf)) => 88,
        Some((2, _)) => 87,
        _ => 117,
    }
}

/// The draw/stow clip by sheath type (`0x611930`–`0x611cc6`): HipSheath 90 for 3 and 7, else 89.
pub(super) fn sheath_clip(sheath_type: u8) -> u16 {
    if (1u32 << (sheath_type & 0x1f)) & 0x88 != 0 {
        90
    } else {
        89
    }
}

/// The ranged anims exempt from the `& 0x10` stow while ranged-drawn (`0x5fe180`, called only on
/// that path, `0x5fe04c`). ReadyThrown 108 is not one: it stows, and the driver's snap bracket
/// keeps the thrown wind-up.
const SHEATH_RANGED_EXEMPT: [u16; 9] = [46, 49, 105, 106, 107, 109, 110, 111, 112];

/// The per-animation sheath reconcile (`0x5fdf80`): the state the clip's WeaponFlags force, first
/// match winning, each a snap. `& 4` stows; mounted stows on every recompute (`0x5fdfd9`) and
/// dismount restores nothing (inferred); `& 0x10` stows, bar the ranged exemptions while
/// ranged-drawn; engaged or `& 0x20` draws melee unless ranged-drawn; else a remote unit takes
/// the server's byte (`0x5fe16e`).
pub(super) fn reconcile_sheath(
    cur: u8,
    anim: u16,
    weapon_flags: u32,
    engaged: bool,
    local: bool,
    server_byte: u8,
    mounted: bool,
) -> Option<u8> {
    if weapon_flags & 4 != 0 {
        return Some(0);
    }
    if mounted {
        return Some(0);
    }
    if cur == 2 {
        if weapon_flags & 0x10 != 0 && !SHEATH_RANGED_EXEMPT.contains(&anim) {
            return Some(0);
        }
    } else {
        if weapon_flags & 0x10 != 0 {
            return Some(0);
        }
        if engaged || weapon_flags & 0x20 != 0 {
            return Some(1);
        }
    }
    if !local && cur != server_byte {
        return Some(server_byte);
    }
    None
}

/// Whether a looping base arm keeps the head variation rather than rolling (`0x5fdba0`): with an
/// auto-attack target, a cast or channel held, or a combat, cast or ready clip leaving. The ids
/// here approximate the client's four classifiers (`0x5fcc10`, `0x5fcbb0`, `0x5fde40`, `0x5fde60`).
pub(super) fn arm_forces_head(engaged: bool, casting: bool, outgoing: u16) -> bool {
    engaged || casting || matches!(outgoing, 16..=19 | 25..=29 | 51..=54)
}

/// The victim's wound flinch (`0x60ea70`), by severity and engagement alone: a crit (`HitInfo &
/// 0x80`) CombatCritical, an engaged victim (`[unit+0xc48]` set) CombatWound, else StandWound.
pub(super) fn wound_anim(hit_info: u32, engaged: bool) -> u16 {
    if hit_info & 0x80 != 0 {
        10 // CombatCritical
    } else if engaged {
        9 // CombatWound
    } else {
        8 // StandWound
    }
}

/// Whether the wound overlay takes the full body rather than the upper body: any id over a ready
/// stance (25–29) on bone 0 (`0x60eae8`; `base_anim` is the armed id, so mid-swing it masks), or
/// StandWound (8) stationary and unmounted (`0x60eb9a` to `0x60ebea`); the reference's
/// transport-substate test is not modelled.
pub(super) fn wound_full_body(id: u16, base_anim: u16, flags: u32, mounted: bool) -> bool {
    matches!(base_anim, 25..=29)
        || (id == 8
            && !mounted
            && flags & (move_flags::ANY_MOVE | move_flags::FALLING | move_flags::SWIMMING) == 0)
}

/// The wound overlay's amplitude λ₀, `+0x108 = 0.75` on op4's `linkFlag = 0` path (`0x712682`):
/// the flinch peaks at 75%, never fully replacing the pose.
pub(super) const WOUND_AMPLITUDE: f32 = 0.75;

/// The wound overlay's graph weight: the client blends by `λ = (3 − 2t)·t² · λ₀`, `t` the
/// fraction of the window left (`0x714880`), and Bevy's normalized blend lands on λ at
/// `w = others · λ/(1 − λ)`, `others` the weight of the rest of the subtree (so `w ≤ 3·others`).
pub(super) fn wound_weight(remaining_frac: f32, others: f32) -> f32 {
    let t = remaining_frac.clamp(0.0, 1.0);
    let lambda = (3.0 - 2.0 * t) * t * t * WOUND_AMPLITUDE;
    others * lambda / (1.0 - lambda)
}

/// The client's per-bone blend weight at amplitude 1 (`0x714880`–`0x714921`), `t` the fraction
/// left: the 150 ms fade-to-rest (`0x7123af`) and the blended re-arm (`0x7125f2`).
pub(crate) fn blend_lambda(remaining_frac: f32) -> f32 {
    let t = remaining_frac.clamp(0.0, 1.0);
    (3.0 - 2.0 * t) * t * t
}

/// The engaged idle, `0x5fd360`'s Ready pick (`0x5fcdc0`); fist and dagger ready as 1H.
pub(super) fn ready_anim(main: Option<(u8, u8)>) -> u16 {
    match main {
        Some((2, sub)) => match sub {
            0x0 | 0x4 | 0x7 | 0xb | 0xd | 0xe | 0xf => 26, // Ready1H (incl. fist + dagger)
            0x1 | 0x5 | 0x8 | 0xc => 27,                   // Ready2H
            0x6 | 0xa | 0x11 => 28,                        // Ready2HL
            _ => 25,                                       // ReadyUnarmed: ranged/obsolete/unknown
        },
        _ => 25,
    }
}

/// The unit's Special: airborne, Fall once FALLINGFAR latches, else Jump for an upward launch (a
/// step-off fall keeps its gait, `0x5fd8e8`); still and sitting, sleeping or kneeling, a Pose.
pub(super) fn current_special(mv: &MovementState, jump_arc: bool) -> Option<Special> {
    if mv.flags & move_flags::FALLING != 0 {
        if mv.flags & move_flags::FALLING_FAR != 0 {
            Some(Special::Fall)
        } else if jump_arc {
            Some(Special::Jump)
        } else {
            None
        }
    } else if mv.flags & move_flags::ANY_MOVE == 0 && matches!(mv.stand_state, 1 | 3 | 8) {
        Some(Special::Pose(mv.stand_state))
    } else {
        None
    }
}

/// Where a requested one-shot is routed this play (the route flag `esi`, `0x5fe6c8`..`0x5fe74d`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum OneShotRoute {
    /// The SpineLow overlay ([`AnimClip::upper_node`]): the torso plays it, the legs keep the base.
    Masked,
    /// The base track (bone 0): the clip replaces the base, legs included, as authored.
    FullBody,
}

/// The CLASS_A set (`0x5fed90`), the only ids that may mask; its ranges' interior is inferred,
/// but only swings, emotes and casts reach [`route_oneshot`].
fn is_class_a(id: u16) -> bool {
    matches!(id,
        2 | 8..=10 | 14..=36 | 46..=49 | 51..=90 | 105..=113 | 117..=118 | 122..=138 | 185..=186 | 195)
}

/// The COMBAT set (`0x5fcc10`), the ids an airborne unit masks: every swing, no emote and no
/// cast, so a jump-in-place cast replaces the hang full-body.
fn is_combat(id: u16) -> bool {
    matches!(id, 10 | 16..=24 | 30 | 36 | 57..=59 | 85..=88 | 95 | 117 | 118)
}

/// The ids forced full-body whatever the state: the Death class {1, 6, 131, 132} (`0x5fda90`)
/// and the special attacks {57, 58, 118} (`0x5fec60`).
fn is_forced_full_body(id: u16) -> bool {
    matches!(id, 1 | 6 | 131 | 132 | 57 | 58 | 118)
}

/// Route a one-shot by live state (`0x5fe6c8`..`0x5fe74d`): masked while the lower body is
/// committed (moving, turning, swimming, a non-zero stand state, or a COMBAT id airborne), else
/// full-body, so one Attack1H is full-body standing and masked running.
pub(super) fn route_oneshot(id: u16, flags: u32, stand_state: u8) -> OneShotRoute {
    if is_forced_full_body(id) || !is_class_a(id) {
        return OneShotRoute::FullBody;
    }
    let committed_lower = flags & move_flags::ROUTE_COMMITTED_MOVE != 0
        || stand_state != 0
        || (is_combat(id) && flags & move_flags::FALLING != 0);
    if committed_lower {
        OneShotRoute::Masked
    } else {
        OneShotRoute::FullBody
    }
}

/// The ids whose rate the client scales by speed (`0x5fee80`), also its LOCOMOTION set.
const RATE_SCALED: &[u16] = &[4, 5, 11, 12, 13, 37, 38, 39, 42, 43, 44, 45, 135, 143, 187];

/// Whether a requested clip is locomotion to the transplant tests (`0x5feae0`, `0x5fe912`). Fall
/// (40) and SwimIdle (41) are not: a FALLINGFAR latch mid-cast replaces the cast on bone 0.
pub(super) fn is_locomotion(id: u16) -> bool {
    RATE_SCALED.contains(&id)
}

/// Whether this state's gait pick is locomotion, deciding if a flag-change re-arm transplants the
/// bone-0 clip or overwrites it; the standing idles cannot change the answer. A root wipes the
/// direction bits, the pick is Stand, and a cast on bone 0 is overwritten.
pub(super) fn gait_is_locomotion(state: &MovementState, walk_speed: f32) -> bool {
    gait_candidates(state, walk_speed, None, None)
        .first()
        .is_some_and(|id| is_locomotion(*id))
}

/// A clip's rate, `0x5fe2f0`'s `speed / (moveSpeed · ‖row0‖)` (`0x5fe4be`..`0x5fe550`), 1× unless
/// the divisor is positive and the id rate-scaled. `‖row0‖` is the rendered scale of the model
/// playing it (`CGUnit+0x90`, `OBJECT_FIELD_SCALE_X`, times a mount's display scale `+0x9c`),
/// the mount's model when mounted; the spell-visual factor `+0x94`
/// (`[0.75, 2.0]`) has no producer yet and must join here.
pub(super) fn playback_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> f32 {
    scaled_rate(clip, speed, model_scale).unwrap_or(1.0)
}

/// [`playback_rate`], `Some` only where both guards pass, so the per-frame write spares clips
/// other producers own (the combat 2×, the whiff 0.5×, an airborne 0×). Only the scale takes
/// `abs()`: `moveSpeed` is signed, and a backwards gait (-2.5) must stay at 1×.
pub(super) fn scaled_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> Option<f32> {
    let divisor = clip.move_speed * model_scale.abs();
    (divisor > 0.0 && RATE_SCALED.contains(&clip.anim_id)).then(|| speed / divisor)
}

/// The unified movement view, first present winning: the controller's [`MovementState`], a
/// remote's relayed flags ([`RemoteMotion`]), a creature's spline, else still.
/// `creature_swimming` supplies the `SWIMMING` the wire never sends for a creature; `modes`, the
/// `SMSG_SPLINE_MOVE_*` grants, join the remote and creature flags (one word in the reference).
pub(super) fn unify(
    movement: Option<&MovementState>,
    remote: Option<&RemoteMotion>,
    spline: Option<&Spline>,
    creature_swimming: bool,
    modes: Option<&crate::net::UnitMoveModes>,
) -> MovementState {
    // Every leg reads the flying spline live, as the selector does at select time (`0x5fd19c`).
    let flying = spline.is_some_and(|s| !s.grounded);
    if let Some(m) = movement {
        return MovementState { flying, ..*m };
    }
    let granted = modes.map_or(0, |m| m.0);
    if let Some(r) = remote {
        return MovementState {
            speed: r.speed,
            flags: r.flags | granted,
            vertical_speed: r.vertical_velocity,
            flying,
            ..default()
        };
    }
    let swim = if creature_swimming {
        move_flags::SWIMMING
    } else {
        0
    };
    let s = spline.map(|s| s.speed()).filter(|s| *s > MOVING_EPSILON);
    MovementState {
        speed: s.unwrap_or(0.0),
        flags: granted | swim | if s.is_some() { move_flags::FORWARD } else { 0 },
        flying,
        ..default()
    }
}

#[cfg(test)]
mod tests;
