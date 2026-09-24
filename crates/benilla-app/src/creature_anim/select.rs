//! The pure animation-selection logic (the `0x5fd8b0` selector's and decision 0073's tables,
//! movement/Special state, gait/swing/ready picks, playback-rate math) — kept in its own file as it
//! carries the bulk of the unit-tested selector logic, separate from the Bevy driver systems in
//! [`super::driver`].

use benilla_assets::AnimClip;
use bevy::prelude::*;

use crate::net::{RemoteMotion, Spline};

/// Fallback walk speed (yd/s) for a unit whose movement block didn't carry one — vanilla's default
/// creature walk speed. The run boundary is 2× this (`RecomputeBaseAnim`, the `0x5fd224` compare).
pub(super) const DEFAULT_WALK_SPEED: f32 = 2.5;

/// Fast-run cutoff (yd/s): at or above it the unit plays the sprint animation (id 143) ahead of Run
/// (`[0x80c484]` in the real selector).
const FAST_RUN_SPEED: f32 = 11.0;

/// Below this ground speed (yd/s) a streamed mover counts as standing still — guards a near-zero residual.
const MOVING_EPSILON: f32 = 0.1;

pub(super) const STAND: u16 = 0;

pub(super) const DEATH: u16 = 1;

/// ShuffleLeft (11) / ShuffleRight (12) — the **turn-in-place foot-shuffle**, picked below when
/// the unit turns without translating. Named because their *lifecycle* is unlike any other gait:
/// once armed they are held to their own clip window rather than released when the turn ends
/// (decision 1655; the clip-end watchdog `0x719370`), and the driver has to be able to say so.
pub(super) const SHUFFLE_LEFT: u16 = 11;
/// See [`SHUFFLE_LEFT`].
pub(super) const SHUFFLE_RIGHT: u16 = 12;

/// StealthWalk (119) — the **prowl creep**, and with [`STEALTH_STAND`] the entire difference
/// stealth makes to a body's *pose*. Both ids are gated by the same descriptor bit —
/// `[[unit+0x110]+0x213] & 2`, `UNIT_FIELD_BYTES_1` byte 3's CREEP flag
/// ([`crate::net::ObjectStore`]'s `unit_is_stealthed`) — 119 from the core cascade at `0x5fd1d3`,
/// tested **after** the backward branch and **before the whole speed tail**, so a prowling unit
/// never plays Run or Sprint however fast it travels.
///
/// No aura, no visual kit, no spell id is involved. Stealth's *translucency* is its aura state
/// kit's CharProc-14 (decision 0806) and its *pose* is this flag: two independent mechanisms off
/// one server byte, which is exactly why the body could go translucent and still stand bolt
/// upright. (The same flag's other three read sites are nameplate/marker suppression, among them
/// `0x6070a0` and `0x60f600` — it drives no body render at all.)
///
/// **119 is NOT rate-scaled.** The client's locomotion-rate whitelist `0x5fee80` is exactly
/// `{4,5,11,12,13,37,38,39,42,43,44,45,135,143,187}` and 119 is absent — the creep cycle plays at
/// 1× whatever the live speed, so [`playback_rate`] leaves it alone.
pub(super) const STEALTH_WALK: u16 = 119;

/// StealthStand (120) — the prowl idle, from the **last** resolver in the chain (the fallback idle
/// `0x5fd830`: swim-idle → `[110]+0x213&2` → jump/float → plain Stand). Being last is its
/// precedence: every other resolver — the sit/sleep/kneel and chair stand-states, the turn-in-place
/// shuffle, the combat Ready idle, loot — outranks it. See [`STEALTH_WALK`] for the shared gate.
pub(super) const STEALTH_STAND: u16 = 120;

/// Mount (91) — the rider's seated pose while a mount model is attached (decision 0441): held
/// **unconditionally** — moving, turning, airborne. The client arms it outside the base selector
/// (once at attach by `0x607a00`, then re-forced on every `PlayAnimation` by `0x5fe2f0`'s mounted
/// branch — there is NO mount leg in the `0x5fd8b0` chain); benilla pins the gait slot instead, the
/// same rendered result. The locomotion the selector would pick plays on the mount model (same
/// `0x5fd100` thresholds), via the mount child's own driver pass. Variations roll as any base loop
/// (HumanMale authors 3).
pub(super) const MOUNT: u16 = 91;

/// Loot (50) — the kneel-and-rummage held while a unit's loot window is open (decisions 0515 /
/// 1471 / 1477): the `0x5fd8b0` chain's loot leg `0x5fd260` → 0x32 = 50, chain order locomotion →
/// **LOOT** → standState → combat/channel (movement outranks by position — the leg itself reads no
/// movement state), leg gates `[+0xdc]==0` (never mounted) + the `[+0xd58]&0x40` enable.
///
/// The leg needs **two** predicates, and both split on IsActivePlayer:
/// - **A `0x6126b0` — is a session open.** Self: the client-local loot-target latch
///   (`[player+0x1d28]`, ours [`crate::ui_loot::LootLatch`]) — armed at the `CMSG_LOOT` send for a
///   corpse, at `SMSG_SPELL_GO` for a chest, and at an admitted `SMSG_LOOT_RESPONSE` otherwise.
///   Remote: [`UNIT_FLAG_LOOTING`] set and [`UNIT_FLAG_LOOT_SUPPRESS`] clear.
/// - **B `0x612710` — is that KIND of target knelt at.** Self: a per-object-class filter, ours
///   [`crate::ui_loot::LootKneel`]. It is why a fishing bobber arms the latch and still does not
///   kneel, and 1471 shipped without it.
///
/// One consequence worth stating plainly, because it looks like a bug: on vmangos, other players
/// never see us kneel at a chest. `Player::SendLoot` sets `UNIT_FLAG_LOOTING` only for
/// `LOOT_CORPSE`, and the remote half has nothing else to read. The clip is authored
/// **clamp** (HumanMale: one 0.5 s sequence, no variations): kneel down, freeze in the rummage
/// pose; the rise back is the ordinary cross-fade to Stand when the trigger drops. Weapons stow
/// for free — row 50 carries the `WeaponFlags & 4` force-stow the per-animation sheath
/// reconcile already applies.
pub(super) const LOOT: u16 = 50;

/// `UNIT_FIELD_FLAGS` bit `0x400` — `UNIT_FLAG_LOOTING` (vmangos `UnitDefines.h`: "Displays loot
/// animation", set by `Player::SendLoot` for corpse loot, removed at `DoLootRelease`): the
/// **remote** half of the loot-kneel predicate (`0x6126db shr 0xa; and 1` — see [`LOOT`]); the
/// self unit reads its latch instead.
pub(super) const UNIT_FLAG_LOOTING: u32 = 0x400;

/// `UNIT_FIELD_FLAGS` bit `0x1000_0000` — must be CLEAR for a remote unit's loot kneel
/// (`0x612710`; its one inferred residue is this bit's Blizzard *name*, not
/// its role). vmangos never sets it on players, so the gate is inert on our wire — carried for
/// the faithful predicate shape.
pub(super) const UNIT_FLAG_LOOT_SUPPRESS: u32 = 0x1000_0000;

/// MountSpecial (94) — the mounted flourish (the horse rears; decision 0441 P2). Plays on the
/// MOUNT model as a one-shot (MountSpecial routes to the mount via the same `0x5fe2f0` op4 target
/// as its locomotion), never on the rider — the rider holds [`MOUNT`] throughout. Fired by the
/// space-bar gate (self, locally at send time) and by `SMSG_MOUNTSPECIAL_ANIM` (observed riders;
/// our own echo is dropped in the net drain).
pub(crate) const MOUNT_SPECIAL: u16 = 94;

/// The three ids whose arm takes the **base-animation lock** — `0x5fdba0`'s tail keyed on the id
/// actually armed, byte table `0x5fdd90`. While one of these holds bone 0, `PlayAnimation` refuses
/// every base request outright, which is what lets a stunned victim's `Knockdown` play out over the
/// root's own `Stand` recompute.
///
/// They are the clips with a definite start and end pose — the ones a re-pick mid-flight would
/// leave the body wrong: knocked flat, or halfway off the ground.
pub(crate) const KNOCKDOWN: u16 = 121;
/// See [`KNOCKDOWN`] — the taxi/lift pair, which take the same lock under the reference's other bit.
pub(crate) const LIFT_OFF: u16 = 192;
/// See [`KNOCKDOWN`].
pub(crate) const LAND: u16 = 200;

/// Movement direction/mode flag bits, matching the client's CMovement `MOVEMENTFLAGS` (cached at
/// `unit+0x9e8`). The selector tests these exactly as the binary does, so a streamed unit can
/// eventually drop the server's raw `u32` straight in.
pub(crate) mod move_flags {
    pub const FORWARD: u32 = 0x1;
    pub const BACKWARD: u32 = 0x2;
    pub const STRAFE_LEFT: u32 = 0x4;
    pub const STRAFE_RIGHT: u32 = 0x8;
    /// TURN-left / TURN-right — the keyboard turn keys (vanilla A/D when not mouse-looking). The client
    /// rotates the facing while these are set (`0x7c4f30` heading integrate) and plays the turn-in-place
    /// foot-shuffle (11/12) when turning with no translation. Strafe (above) slides without turning.
    pub const TURN_LEFT: u32 = 0x10;
    pub const TURN_RIGHT: u32 = 0x20;
    /// WALK-mode (vs run). In the client this only scales the rate numerator (walk vs run speed), never
    /// the id choice. The net bridge reads it to extrapolate a `/walk`-toggled remote mover at walk speed.
    pub const WALK_MODE: u32 = 0x100;
    /// MOVEFLAG_LEVITATING — **the free-flight bit, and it works by SUPPRESSION.** It is the very
    /// first test in the client's per-frame swim decision `0x6030c0` (`0x6030d2 test ah,4` → bail to
    /// `0x6031fa`): set, and the whole water/depth decision is skipped — neither the ENTER arm nor
    /// the STOP arm runs, so liquid can no longer latch *or unlatch* [`SWIMMING`].
    ///
    /// That suppression is the entirety of GM flight. vmangos's `.cheat fly` sends
    /// `LEVITATING | SWIMMING | MOVED | FLYING` (`Player::SetFly`): `SWIMMING` puts the mover in the
    /// 3-D floating regime (gravity bypassed, vertical from the aim pitch), and `LEVITATING` stops
    /// the dry-land depth test from clearing it again the very next frame. `MOVED` (`0x800000`) and
    /// `FLYING` (`0x1000000`) carry no behaviour on this build — vmangos's own header marks both
    /// doubtful — and we model neither. Decision 0726.
    pub const LEVITATING: u32 = 0x400;
    /// JUMPING/FALLING — set for the whole airborne arc; drives the jump Special state.
    pub const FALLING: u32 = 0x2000;
    /// MOVEFLAG_FALLINGFAR — the arc has become a **far fall** (`0x633220`/`0x633240`; legs
    /// corrected by decision 0179): a *jump* (launch vz ≠ 0) latches once it descends 1/9 yd below
    /// its launch height; a *step-off fall* (launch vz = 0) latches at 500 ms airborne (≈ 2.41 yd
    /// of free fall — a fence hop never latches, a wagon-height drop latches just before the
    /// floor). Cleared only with FALLING at StopFalling (`0x7c6290`). Mid-air it swaps the pose to
    /// Fall(40) and it opens the landing-anim gate; a flat jump never descends below its takeoff,
    /// so its hang stays Jump(38).
    pub const FALLING_FAR: u32 = 0x4000;
    /// MOVEFLAG_ROOT — the server rooted this mover (death, until release). The controller MUST
    /// carry it in the root-apply ack's MovementInfo (vmangos `HandleMoveRootAck:715` KICKS a
    /// root-apply ack without it — live-verified 2026-07-11, `Movement.log`), and moving bits
    /// must never accompany it (they freeze the real client).
    pub const ROOT: u32 = 0x1000;
    pub const SWIMMING: u32 = 0x20_0000;
    /// MOVEFLAG_ONTRANSPORT — standing on a boat/zepp (1.12 bit 25, vmangos `MovementInfo.h:56`
    /// `0x02000000`; NOT the TBC-era 0x200). Set while the mover rides a transport's platform
    /// frame; every packet carrying it also carries the local-pose tail (decision 0438 phase 2).
    pub const ON_TRANSPORT: u32 = 0x0200_0000;
    /// MOVEFLAG_WATERWALKING — the liquid surface counts as walkable ground (Water Walking,
    /// Levitate, and the ghost form; decisions 0308/0866). Reference consumers: the liquid-mask
    /// selector `0x6315f0` (`0x63160d test eax,0x10000000`, taken *only* when not swimming) and the
    /// opcode-0x22 apply `0x61a430`.
    pub const WATER_WALKING: u32 = 0x1000_0000;
    /// MOVEFLAG_SAFE_FALL — **feather fall** (Slow Fall, Levitate; decision 0866). It has exactly
    /// one effect: the fall-velocity query `0x7c5d20` picks its terminal clamp on this bit
    /// (`0x7c5d23 test [ecx+0x40],0x20000000`) — 7.0 yd/s `[0x87d898]` instead of the ordinary
    /// 60.148 `[0x87d894]`. A bytes-only reading labels it "in-water; selects swim gravity", which
    /// is the bit's *shape* read without the server-side name: vmangos sets it only from
    /// `SPELL_AURA_FEATHER_FALL`, and swimming is the separate [`SWIMMING`] (0x200000).
    pub const SAFE_FALL: u32 = 0x2000_0000;
    /// MOVEFLAG_HOVER — the body rests 1.0 yd above the ground (Levitate; decision 0866). Hover
    /// rather than a wade bit: the WALK resolver `0x6367b0` gates a `[0x7ff9d8]` = +1.0-yd surface
    /// offset on it, and the step-down reach widens by the same yard (`0x633e35`).
    pub const HOVER: u32 = 0x4000_0000;

    /// The bits a **server-authored move packet owns**, and the only ones it may write — the
    /// reference's flag *merge* mask, at `0x618c30 @0x618deb`
    /// (`new = old ^ ((old ^ wire) & 0x75a07dff)`; decision 0725).
    /// Applying a `MSG_MOVE_*` is not an assignment: bits outside this mask are kept from local
    /// state, because they are the client's own (`0x618c30` also holds a second mask,
    /// `0x75a01dff`, for a caller arm that passes a non-zero 4th arg — the plain state family
    /// passes zero and takes this one).
    ///
    /// Two of ours land outside it and the omissions are the point. **[`ON_TRANSPORT`]** (bit 25)
    /// is client-owned: a server-authored pose relocates you, it never boards or deboards you —
    /// which is why the self-move arm re-anchors the ride rather than dropping it. Bit 27 (the
    /// reference's free-advance selector) and bit 31 (its clock baseline) are CMovement internals
    /// with no benilla analog. Every bit we *do* model besides `ON_TRANSPORT` — the direction and
    /// turn bits, walk mode, root, [`FALLING`]/[`FALLING_FAR`], swim, water-walk — is inside.
    pub const SERVER_AUTHORED: u32 = 0x75a0_7dff;

    /// Merge a server-authored packet's `MOVEMENTFLAGS` into a mover's own — the reference's masked
    /// merge (`0x618c30 @0x618de7-df3`: `new = old ^ ((old ^ wire) & 0x75a07dff)`), **not** an
    /// assignment. One law, both movers: our own (`player::wire_in`'s self-addressed pose) and every
    /// watched one (`net::motion::remote::apply_move`), because the reference runs this merge inside
    /// the one scheduler both go through and the mask is arm-invariant across all thirty relay
    /// opcodes (`0x618f20` always takes the wide arm; decision 2064).
    ///
    /// The omission that bites is pinned by test on the self lane: [`ON_TRANSPORT`] sits **outside**
    /// the mask, so a server-authored pose can relocate a rider but never board or deboard them.
    pub const fn merge_server_authored(local: u32, wire: u32) -> u32 {
        (local & !SERVER_AUTHORED) | (wire & SERVER_AUTHORED)
    }

    /// Any horizontal-movement direction bit (forward/back/strafe) — the client's `[9e8] & 0xf` gate.
    pub const ANY_MOVE: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;

    /// **Whether this mover is integrated at all** — the client's `0x20ff`, and the gate a mover
    /// must pass before any physics runs on it. `CMovement::Update 0x616de0` opens its substep loop
    /// with `0x616e20 test dword [esi+0x40],0x20ff; je 0x616f49` (none set ⇒ finalize, having
    /// integrated nothing), and the movement manager tests the same mask before it ever calls the
    /// integrator (`0x6166f5`) — the "idle gate" decision 0059 named by address. The consequence
    /// for a watched player: for a **STANDING remote** the integrator does NOT run, and a flag-less
    /// unit is not even in the mover list (`0x618940`).
    ///
    /// So a mover with none of these bits keeps the pose its last packet wrote, verbatim, until a
    /// packet or a flag moves it — which is what decision 1545 makes benilla do. The bits: the four
    /// direction bits, the two keyboard turn bits, and the two pitch bits benilla does not model
    /// (together `0xff`), plus [`FALLING`] (`0x2000`). Note what is **absent**: [`SWIMMING`]
    /// (`0x200000`) and every mode bit — a floating swimmer holding no direction is not integrated
    /// either.
    pub const INTEGRATED: u32 = 0xff | FALLING;

    /// The **committed-lower-body** move mask that routes a one-shot to the masked overlay (decision
    /// 0087): the client's `[9e8] & 0x20003f` test at `0x5fe6dc` — every direction bit (`0x3f`,
    /// which folds in the keyboard turn keys) plus swim (`0x200000`). Any of these set ⇒ the legs are
    /// committed, so a swing/emote plays masked over them rather than full-body. (The client's
    /// *separate* facing-delta turn test `d58 & 0x1800` — a mouse turn-in-place with no move flag —
    /// is a field benilla does not model; keyboard turning is already covered here via `0x30`.)
    pub const ROUTE_COMMITTED_MOVE: u32 =
        FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT | TURN_LEFT | TURN_RIGHT | SWIMMING;

    /// The stationary-cast pin's "moving" test — the client's `[9e8] & 0x20000f` in the `0x5fde80`
    /// cast-override gate: the direction bits plus swim, and NOTHING else. The turn bits are absent
    /// — a caster turning in place (keys or a mouselook body-step) keeps the full-body pin.
    /// Distinct from [`ROUTE_COMMITTED_MOVE`]'s `0x20003f` (the one-shot route test at `0x5fe6dc` —
    /// a different byte site, a different mask); using that mask here made the pin flap at
    /// mouse-event cadence — the transient chase-step TURN flags demoted the hold to
    /// shuffle+overlay on every mouse-delta frame and re-pinned on every quiet one (decision 0491,
    /// the frostbolt right-drag jitter).
    pub const CAST_PIN_MOVE: u32 = ANY_MOVE | SWIMMING;
}

/// How fast a strafing unit's rendered root yaw eases toward its offset heading — the client blends
/// the display facing a **quarter of the remaining gap per frame** (`0x607ed0` tail, `×0.25`
/// `[0x8029b0]`); at the reference 60 fps that is the continuous exponential rate
/// `−ln(0.75) × 60 ≈ 17.3 /s` (we take the time-based form rather than aping the frame-rate
/// dependence). Applied by [`ease_strafe_yaw`], the pose owners' one blend.
const STRAFE_BLEND_RATE: f32 = 17.26;

/// Advance a unit's rendered yaw toward `aim + offset` by easing the **aim-relative offset** as a
/// plain scalar ([`STRAFE_BLEND_RATE`] exponential). For a stationary aim this is the same curve as
/// easing the absolute yaw — EXCEPT at the strafe flip (±90° → ∓90°), where the absolute
/// shortest-arc is an exact 180° tie that float noise resolves to either side ("sometimes it spins
/// around the back" — director). Offset space has no tie: the swing always passes through the aim,
/// i.e. around the front, and the spine/head twist gap unwinds through 0 instead of snap-flipping
/// at ±180°.
pub(crate) fn ease_strafe_yaw(current_yaw: f32, aim: f32, offset: f32, dt: f32) -> f32 {
    let cur = super::wrap_pi(current_yaw - aim);
    let eased = cur + (offset - cur) * (1.0 - (-STRAFE_BLEND_RATE * dt).exp());
    super::wrap_pi(aim + eased)
}

/// The strafe **body-heading offset** (radians) a unit's rendered root yaw sits from its aim — the
/// client's display-facing strafe blend (`0x607ed0` tail): ±π/2 for a pure strafe, ±π/4 when
/// forward/back is also held (`flags & 3`). Yaw is left-positive (WoW orientation and our Bevy yaw
/// alike), so strafe-left is `+` — **mirrored while backpedaling** (the client's sign fold
/// `((flags>>1)&2) == (flags&2)`): a back-left diagonal faces the body forward-right and backpedals
/// along the movement line, legs never crossing. `0` when not strafing (both strafe bits set cancel
/// — no lateral motion results, unlike the client's left-bit-only fold, an unobservable edge). The
/// [`super::twist::BodyTwist`] counter-twist then walks the upper body back toward the aim.
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

/// The **swim body pitch** — the one render law, for every mover that has a reported pitch
/// (decision 0464 §1; `0x60a110`→`0x710620`, the Euler-ZYX builder).
///
/// The reference composes `T·S·Rz(facing)·Ry(2π − pitch)`: yaw about WoW +Z, pitch about WoW +Y
/// from `CMovement+0x20`, +pitch = nose up. That is this expression, worked through the basis
/// rather than assumed — WoW +Z → Bevy +Y (so `Rz(facing)` is `Quat::from_rotation_y`, the mapping
/// `net::motion::wire_yaw` already rests on, no 180° offset) and WoW +Y → Bevy **−X**
/// (`coords::wow_bevy_basis_is_golden`), so `Ry(2π − pitch)` = `Ry(−pitch)` about −X is `+pitch`
/// about Bevy +X. Same multiplication order, so the pitch stays body-local; and `Rx(+θ)` carries
/// Bevy forward (−Z) to `(0, sin θ, −cos θ)`, i.e. up — nose-up positive on both sides.
///
/// The facing it reads is `+0xc94`, the **render** facing rather than the raw movement one
/// (`+0x1c`); for a swimmer the two coincide, because SWIMMING is on the display-facing SNAP list
/// (`mov [esi+0xc94],[esi+0xc98]` in the `0x607ed0` tail) — the same gate our callers
/// apply before handing a yaw in.
///
/// The gate is `SWIMMING` **and** a forward/back bit: strafe-only, idle, and grounded all render
/// **level** (the ground path). It is `CMovement+0x40 & 3`, read at `0x60857d` off the very word
/// the create-block apply and every relayed `MSG_MOVE_*` merge into under `0x75a07dff` — **not**
/// a `CM2Model+0x3c & 3` dirty gate, which is what an earlier reading of this law named and which
/// does not exist (`0x710620` is an unconditional 16-dword copy).
///
/// One function rather than one per pose owner: the law has three consumers — our own avatar
/// ([`crate::player::body_pose`]), an observed relay mover
/// ([`crate::net::motion::extrapolate_remote_units`]), and the create-block seed
/// ([`crate::net::objects`]) — and in the reference all of them present the *same*
/// pitch. Held apart, they drift: the remote lane shipped in July 2026 with no test and no trace
/// of its own, so "does an observed swimmer tilt?" had no answer short of the director's eye.
pub(crate) fn swim_body_rotation(yaw: f32, flags: u32, pitch: f32) -> Quat {
    if flags & move_flags::SWIMMING != 0
        && flags & (move_flags::FORWARD | move_flags::BACKWARD) != 0
    {
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
    } else {
        Quat::from_rotation_y(yaw)
    }
}

/// The per-frame movement descriptor that drives a unit's animation (what the selector `0x5fd8b0`
/// reads). The player controller fills it for our avatar from input; a streamed unit's is derived
/// from its [`Spline`].
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct MovementState {
    /// Live movement speed (yd/s) — `0` while standing. The rate-scaler's numerator and the
    /// walk/run threshold read this; it is already the *directional* speed (runBack when
    /// backpedaling). On the ground it's the horizontal speed; for a swimmer it's the flag-scalar
    /// 3D travel speed (swim/swimBack regardless of pitch — a vertical stroke plays at full rate).
    pub(crate) speed: f32,
    /// Vertical speed (yd/s, +up); non-zero while airborne. Its sign on the arc's FIRST airborne
    /// frame splits a **jump** (upward — the JumpStart/Jump bracket) from a **step-off fall** (the
    /// gait freeze) — the information the real client carries as the MSG_MOVE_JUMP event vs a bare
    /// StartFalling, which our flag-collapsed machine reads from the launch velocity instead.
    pub(crate) vertical_speed: f32,
    /// CMovement `MOVEMENTFLAGS` subset (see [`move_flags`]).
    pub(crate) flags: u32,
    /// Stand-state (`UNIT_FIELD_BYTES_1` byte 0): 0 Stand · 1 Sit · 3 Sleep · 4/5/6 chair low/med/high ·
    /// 8 Kneel — drives the idle pose / the sit-sleep-kneel transition while standing.
    pub(crate) stand_state: u8,
    /// **Stealthed** — `UNIT_FIELD_BYTES_1` byte 3's CREEP bit, the gate on [`STEALTH_WALK`] /
    /// [`STEALTH_STAND`]. Stamped by the driver from the unit's OWN descriptor every frame, self
    /// and remote alike (the client reads `[[unit+0x110]+0x213]` at select time), so [`unify`]'s
    /// legs never touch it: unlike the stand state there is nothing to predict controller-side —
    /// the crouch waits on the server's aura, exactly as the reference does.
    pub(crate) stealthed: bool,
    /// Riding a **flying** server spline (the taxi flight): the client's selector reads the active
    /// CMovement's spline flags (`[[unit+0x118]+0xa4]+0x18`) and plays Fly 135 when the wire Flying
    /// bit (0x200) is set — `0x5fd19c`. Stamped by [`unify`] from the entity's live
    /// [`Spline`] each frame (the same read-through the client does); the controller's stored
    /// component leaves it `false`.
    pub(crate) flying: bool,
}

/// A transition-bracketed animation state — a one-shot **enter** → **loop** → one-shot **exit**, as
/// opposed to the directly-cross-faded gaits. The ids are from the reference selector + the
/// HumanMale.m2 sequence table (decisions 0047/0049).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Special {
    /// Airborne from a **jump** (the arc launched upward): JumpStart(37) → Jump(38) hang.
    /// `0x60e480` case 0xbb (MSG_MOVE_JUMP) arms **37 directly**, local and observed alike, and 38
    /// follows only by 37's own 833 ms window elapsing — a short arc (the ~0.24 s deep swim hop)
    /// never shows 38, which the Entering→`oneshot_finished` machine reproduces. (`0x5fc3f0`'s
    /// events 0x25/0x26 → 38 is the separate emote/anim-event dispatcher, not the jump path.)
    /// FALLINGFAR latching mid-arc hands off to [`Special::Fall`]; the landing is
    /// [`jump_land_pick`]. A **step-off fall** never enters here — its gait freezes (the base
    /// selector's keep-current, via the takeoff-frozen flags/speed) until FALLINGFAR latches.
    Jump,
    /// Airborne and **fallen far** (FALLINGFAR latched): the Fall(40) loop, entered *directly* —
    /// the client plays 40 the tick the flag latches (`0x602c40`), no enter one-shot — so
    /// `enter_special` skips [`Mode::Entering`] for it. Lands like Jump, via [`jump_land_pick`].
    Fall,
    /// A stand-state pose keyed on the standState value (1 Sit · 3 Sleep · 8 Kneel): down → loop → up.
    Pose(u8),
}

impl Special {
    /// The one-shot played on entry (JumpStart / SitDown / SleepDown / KneelDown). Fall has no
    /// enter — `enter_special` sends it straight to its loop; its arm here is unreachable, kept
    /// for the exhaustive match.
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
    /// The looping middle (Jump hang / Fall / Sit / Sleep / Kneel).
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
    /// The one-shot played on a *pose's* exit (SitUp / SleepUp / KneelUp). An airborne state's
    /// landing never routes here — it is [`jump_land_pick`] (the `0x602c60` dispatcher) via
    /// `Mode::Land`; the Jump/Fall arms are unreachable by flow, kept for the exhaustive match.
    pub(super) fn exit(self) -> u16 {
        match self {
            Special::Jump | Special::Fall => STAND,
            Special::Pose(1) => 98,
            Special::Pose(3) => 101,
            Special::Pose(8) => 116,
            Special::Pose(_) => STAND,
        }
    }

    /// Whether starting to move cuts this state's exit short. A pose's stand-up (SitUp / …) is abandoned
    /// the instant the unit moves — you don't watch yourself stand up when you walk off; the gait
    /// cross-fade carries the half-pose into the walk. Jump/Fall never reach [`Mode::Exiting`] any
    /// more — their landing is [`Mode::Land`], a freely-overwritten pick (decisions 0083/0087) — so
    /// their `false` here is vestigial, kept for the exhaustive match.
    pub(super) fn interruptible_by_move(self) -> bool {
        matches!(self, Special::Pose(_))
    }
}

/// The jump-landing one-shot, from the flags at touchdown — the client's land dispatcher
/// `0x602c60`, transcribed: swimming → no landing anim (the recompute picks the swim gait); stopped
/// (`flags & 0xf == 0`) → JumpEnd **39**; moving — unless BACKWARD (`0x2`) or WALK-mode (`0x100`) —
/// → JumpLandRun **187**; a **backpedaling or walking** landing plays **no one-shot at all**,
/// dropping straight into the gait (the reference backpedals the instant it touches down; 187 is a
/// forward-run footplant and never plays backward — the "forward run flash after a
/// jump-then-hold-S" bug). `None` = no landing clip, go straight to `Mode::Gait`.
///
/// **A ROOTED arc end is not a landing** (decision 0880). A root or a stun caught mid-air ends the
/// fall where the body hangs — `SetRoot 0x7c7340`'s `StopFalling` clears FALLING with no ground
/// involved — and the reference suppresses the land packet on exactly that state
/// (`0x602df3 test ah,0x10` gating opcode `0xc9`). This dispatcher *is* that packet's consumer
/// (`0x602c60` runs on the FALL_LAND apply, local and observed alike), so no packet means no pick:
/// the pose starves to Stand instead of playing JumpEnd in mid-air. Same suppression, same byte
/// site, as the wire half in [`crate::player`]'s arc bookkeeping.
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
    /// Playing a *pose's* exit one-shot (SitUp/SleepUp/KneelUp — the `Special` being left + the exit
    /// clip id). Returns to Gait when it finishes — unless a new Special, or movement, interrupts it
    /// first. (Jump's landing is [`Mode::Land`], not this — its bracket was killed per decision 0083.)
    Exiting(Special, u16),
    /// The jump **landing** as a plain, freely-overwritten pick (decisions 0083/0087 (d)):
    /// [`jump_land_pick`] chooses from the input *at touchdown* (`flags`) — JumpEnd 39 stopped,
    /// JumpLandRun 187 moving forward; a backpedal/walk landing plays no clip and never enters this
    /// mode — and re-picks the instant any movement flag changes, so land-then-press runs
    /// immediately, land-then-release stands immediately, and a direction flip at landing shows no
    /// stale-direction flash. Plays through to Gait only if the input holds steady. Not a
    /// non-preemptible bracket.
    Land { id: u16, flags: u32 },
    /// Playing an action one-shot **full-body on the base track** (the bone-0 route of decision
    /// 0087): a standing melee swing (decision 0073), a standing `/cheer`-class emote — or a
    /// **mid-air cast/emote** (decision 0864: the airborne route test masks only COMBAT ids, so a
    /// jump-in-place cast replaces the hang on bone 0, exactly the ref's full-body mid-air cast).
    /// `under` is the Special whose base clip this play replaced (`None`: it replaced a gait).
    /// While `under` is airborne the one-shot obeys the client's airborne-freeze: a finished clip
    /// clamps and holds its last frame, flag changes are keep-current no-ops, and only a Special
    /// *edge* exits — the FALLINGFAR latch's Fall (played ONCE, at the latch edge
    /// `0x61a820` — 0864's per-tick re-assert was refuted, decision 0868; a clip armed
    /// after the latch holds like any other) and the `0x602c60` land pick at touchdown. Grounded
    /// (`under == None`) it returns to Gait when it finishes; a new one restarts it — and any
    /// movement-flag change since the **base** was last armed
    /// ([`AnimDriver::gait_flags`](crate::creature_anim::AnimDriver)) re-picks immediately: the
    /// client's base re-arm overwrites bone 0 blindly on the change (the same re-arm decision 0280
    /// named), so a shot fired standing yields to the run the instant the player moves instead of
    /// sliding the clip out. It carried its own arm-time flags until 0894, which made the edge
    /// invisible whenever the one-shot and the movement change landed on the same frame. The
    /// *masked* route never enters here — it plays as an overlay beside `Mode`, leaving the base
    /// machine (this enum) running underneath.
    Swing { id: u16, under: Option<Special> },
}

/// The gait an idle/moving unit should play in [`Mode::Gait`], most-specific id first with fallbacks so a
/// model lacking the ideal clip steps down rather than snapping (the `0x5fd8b0` chain's core, minus
/// the Special states — jump and the sit/sleep/kneel poses are handled by the state machine).
/// `walk_speed` is the unit's own walk speed (the run boundary is 2× it). `ready` is the engaged
/// combat idle (decision 0073), played only when standing on the ground — locomotion outranks it,
/// and a swimming engaged unit treads water (the client's own gate order).
pub(super) fn gait_candidates(
    state: &MovementState,
    walk_speed: f32,
    ready: Option<u16>,
    ranged_load: Option<u16>,
) -> &'static [u16] {
    use move_flags::*;
    let f = state.flags;
    // Swimming (`[9e8] & 0x200000`) — the `0x5fd100` cascade: **TURN > STRAFE > BACKWARD >
    // FORWARD**, ids turn→41, strafeL(0x4)→43 / strafeR(0x8)→44 (SwimLeft/SwimRight — names
    // byte-read from AnimationData.dbc), back→45, fwd→42, idle→41. So a turning swimmer treads
    // water whatever its travel bits, and a strafe diagonal (fwd+strafe, back+strafe) plays the
    // side-stroke, not 42/45.
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
    // A flying server spline — the taxi ride (`0x5fd19c`, between the swim block and the
    // backward test in the byte cascade): the spline's wire Flying bit selects Fly 135 outright,
    // before the backward and speed branches — a 32 yd/s taxi plays Fly, never Sprint 143. The
    // one-step fallback mirrors AnimationData (Fly → 0 Stand); every shipped taxi mount authors
    // real Fly clips (the gryphon carries two variations).
    if state.flying {
        return &[135, 0];
    }
    // Backward dominates strafe (`0x5fd1bc`, `[9e8] & 2` → WalkBackwards 13).
    if f & BACKWARD != 0 {
        return &[13, 4, 0];
    }
    // Stealthed and moving — the prowl (`0x5fd1d3`, sitting between the backward branch and
    // the speed tail; [`STEALTH_WALK`]). The gate outranks the ENTIRE speed tail, so a stealthed
    // unit creeps at any speed, while a *backpedaling* one plays WalkBackwards — backward is tested
    // first. The Walk 4 fallback is `AnimationData.dbc`'s own Fallback for row 119, which is also
    // what the baked PlayableAnimationLookup resolves 119 to on a model lacking the clip: HumanMale
    // and NightElfFemale author both stealth clips, `druidcat.m2` authors NEITHER (119→4, 120→0), so
    // a prowling cat shows its ordinary walk — in the reference too, through the same resolver.
    if state.stealthed && f & ANY_MOVE != 0 {
        return &[STEALTH_WALK, 4, 0];
    }
    // Ground gaits by live speed (`0x5fd202`/`0x5fd224`): fast-run ≥ 11, run > 2× walk, else walk.
    // (Strafe currently routes here to Run/Walk; whether it has a dedicated shuffle is open.)
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
    // Turning in place (the turn keys with no translation): the foot-shuffle (`0x5fd3f0` →
    // ShuffleLeft 11 / ShuffleRight 12). Only reached when not moving — moving with a turn curves the
    // run path and plays the gait above.
    if f & TURN_LEFT != 0 {
        return &[SHUFFLE_LEFT, STAND];
    }
    if f & TURN_RIGHT != 0 {
        return &[SHUFFLE_RIGHT, STAND];
    }
    // Standing + engaged in melee: the weapon-class Ready idle (decision 0073 — gated on engagement,
    // not sheath state; outranks the chair poses, since a fighting unit isn't seated).
    if let Some(r) = ready {
        return match r {
            26 => &[26, 25, 0],
            27 => &[27, 25, 0],
            28 => &[28, 25, 0],
            _ => &[25, 0],
        };
    }
    // Standing in the ranged stance with auto-repeat armed (the client's `0x5fd460`, gate
    // `sheath CUR == 2 && [+0xd58] & 0x200` — local player only, decision 0099 phase 5): the
    // ranged weapon's Load/Hold idle ([`ranged_load_anim`]). Placed after the engaged Ready
    // (they can't co-occur — auto-shot never sets the engaged GUID) and before the chair loops
    // (OUR ordering; the client's call-site order vs the chair resolver isn't pinned).
    if let Some(l) = ranged_load {
        // The Hold twins 109/110 ARE reachable (decision 1544): `0x5fd460`'s own jump table writes
        // only 105/106/111/112, but a finished Load is promoted to its Hold by the completion
        // dispatch (`0x5fc3f0` slot 11/12/15), and the caller passes whichever of the pair
        // currently owns the pose. 0994 left them out on an absence proof — that the dispatcher is
        // never reached for a bow id — which is false: the completion dispatcher has a second,
        // deferred fire site (`0x7075af`). Each Hold falls back to its own Load first: a model that
        // authors the pull but not the hold should freeze at full draw, not drop to ReadyUnarmed.
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
    // Standing: a chair-sit loop (server stand-states 4/5/6 — no down/up triple) else Stand. The
    // transition-bracketed poses (1 Sit / 3 Sleep / 8 Kneel) are Special, handled before this is called.
    // State 2 (generic SIT_CHAIR) is verified VESTIGIAL (decision 0280): the client's resolver row
    // for 2 writes no override (`0x5fd644` returns a bool; the 0xd0 sentinel stands), so falling to
    // plain Stand here is the faithful render, not a gap.
    // Standing stealthed: the prowl idle ([`STEALTH_STAND`]) — placed *after* the chair loops, since
    // its resolver (`0x5fd830`) is the last in the chain and the stand-state resolver precedes it.
    match state.stand_state {
        4 => &[102, 0],
        5 => &[103, 0],
        6 => &[104, 0],
        _ if state.stealthed => &[STEALTH_STAND, STAND],
        _ => &[STAND],
    }
}

/// The local auto-repeat **standing idle** id — the client's `0x5fd460` → LUT `0x5fd530`: the
/// RANGED-slot item's subclass picks a held Load/Hold clip — Bow → LoadBow 105 · Gun/Crossbow →
/// LoadRifle 106 · Thrown → LoadThrown 112 · Wand → HoldThrown 111 · anything else →
/// ReadyUnarmed 25. **Not** ReadyBow/AttackBow: no code in the client plays those rows.
///
/// (This doc used to carry the claim "the per-shot caster fire animation is a verified NEGATIVE —
/// the shot shows only the missile". That was REFUTED twice over: by the weapon-visual merge, which
/// plays the fire clip off the weapon's own substitute visual (decision 0370/0986), and by the
/// completion dispatch below (decision 1544). It is gone rather than hedged — a stale negative in a
/// doc is how a session concludes the missing animation is correct, which is exactly what happened
/// to bug B307.)
pub(super) fn ranged_load_anim(ranged: Option<(u8, u8)>) -> u16 {
    match ranged {
        Some((2, 2)) => 105,           // Bow → LoadBow
        Some((2, 3) | (2, 18)) => 106, // Gun / Crossbow → LoadRifle
        Some((2, 16)) => 112,          // Thrown → LoadThrown
        Some((2, 19)) => 111,          // Wand → HoldThrown
        _ => 25,                       // no/odd ranged item → ReadyUnarmed
    }
}

/// Whether `id` is one of the ranged Load clips the drawn idle arms — the clips that must play
/// **once and freeze at full draw** rather than looping ([`ranged_load_anim`]'s outputs, minus
/// the wand's HoldThrown 111, which is already a hold pose).
pub(super) fn is_ranged_load(id: u16) -> bool {
    matches!(id, 105 | 106 | 112)
}

/// The **Hold** clip a finished ranged Load promotes to — the completion dispatch `0x5fc3f0`'s
/// slot 11/12/15 arms: LoadBow 105 → HoldBow 109 · LoadRifle 106 → HoldRifle 110 · LoadThrown 112
/// → HoldThrown 111. The wand's HoldThrown 111 is already the hold and re-arms itself, and
/// anything else (ReadyUnarmed 25) holds nothing and stays put.
///
/// **The promotion is UNCONDITIONAL** (decision 1544): the `[+0xd24]` ranged-prop and
/// `[+0xd58] & 0x600` test at `0x5fc5bc` belongs to slot **13** — the Hold's own re-arm — not to
/// the Load's slot 11, which is the bare `mov eax,0x6d ; push eax ; call 0x5fe2f0` at `0x5fc5e9`.
/// Reading that gate onto the Load is the mistake that would leave a shooter frozen at full draw.
///
/// The Hold is the only clip in the ranged cycle authored as a **loop** (asset flag, verified on
/// five shipped character models); the fire and Load clips are clamp. That is why it can sit
/// between shots at all, and why nothing needs to re-arm it per frame.
pub(super) fn ranged_hold_anim(load: u16) -> u16 {
    match load {
        105 => 109,       // LoadBow → HoldBow
        106 => 110,       // LoadRifle → HoldRifle
        112 | 111 => 111, // LoadThrown → HoldThrown; the wand's hold re-arms itself
        other => other,
    }
}

/// Does the drawn ranged Load idle own this unit's standing pose?
///
/// `0x5fd460` — tier 9 of `ComputeAnimation 0x5fd8b0`, and the **only** writer of 105/106/111/112
/// image-wide — claims on exactly two tests and nothing else:
///
/// ```text
/// 5fd463: cmp dword [ecx+0xd40], 2   ; sheath CUR == RANGED
/// 5fd476: test ah, 0x2               ; [+0xd58] & 0x200 = auto-repeat active
/// ```
///
/// `0x200`'s only writer image-wide is the LOCAL cast-send `0x6e593b`, gated `AttributesEx2 &
/// 0x20` — so entry means *this client's player is actively auto-repeating*. **`0x400`
/// ([`super::RangedHold`]) is never tested in this function.** It appears only in `0x5fc3f0`'s
/// sustain gates (`test ah,0x6` for 109, `test ah,0x4` for 110/111/112), which never
/// run. Folding it into this gate is what let one Serpent Sting / Multi-Shot — any ranged-slot
/// spell whose visual sets the hold bit — leave the shooter aiming a drawn bow with nothing able
/// to clear it (director-reported 2026-08-05).
///
/// A REMOTE shooter never runs the local cast-send, so it never enters here. It is not simply
/// Stand, though: it plays **LoadBow(105) once** at the volley's single `SMSG_SPELL_START`
/// (`0x6e7901`), through the PrecastKit — our [`super::CastHold`] path — then AttackBow per GO over
/// its ordinary idle. Never a sustained aim pose.
///
/// The caller applies the tier order: locomotion (tier 4) outranks tier 9, so a moving unit is
/// never in the idle whatever these bits say.
pub(super) fn ranged_idle_gate(auto_repeat: bool, sheath_cur: Option<u8>) -> bool {
    sheath_cur == Some(2) && auto_repeat
}

/// The victim **defense-reaction** id (decision 0279 — the `$CPP` dispatch, byte-verified
/// `0x624a01` → `0x60ec00`): DODGE/DEFLECTS → Dodge(30) · BLOCKS → ShieldBlock(24) · PARRY keys
/// the victim's own **mainhand** through the `0x60ec98` LUT (read off `WoW.exe` `.text`, its
/// third distinct weapon bucketing after the swing and Ready tables): 1H axes/maces/swords/
/// exotic1H/misc **and dagger** → Parry1H(21) · 2H axes/maces/swords/exotic2H → Parry2H(22) ·
/// polearm/staff/spear → Parry2HL(23) · **fist → ParryUnarmed(20)** · ranged/obsolete/
/// subclass>0x11/non-weapon/empty → the client bails, no clip (`0x60ec3a`/bucket 4 → ret).
/// Every other victimState plays nothing.
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

/// The **play-time unarmed-special substitution** — the third weapon substitution in the client,
/// and the only one that lives inside `PlayAnimation` itself rather than in a selector
/// (`0x5fe2f0` @ `0x5fe3cc`–`0x5fe3e9`).
/// A requested **Special1H(57) / Special2H(58)** — the weapon-remapped spin a spell kit asks for,
/// Eviscerate's among them — becomes **SpecialUnarmed(118)** when `GetWeapon(0, 0)` *and*
/// `GetWeapon(1, 0)` are both NULL, i.e. both hands are empty to the combat reading.
/// `SpecialUnarmed`'s own `AnimationData.dbc` fallback column is 57, closing the ring.
///
/// The test is emptiness, not weapon-ness: a hand holding a non-weapon is non-NULL and keeps the
/// armed clip. Because it sits at the play seam it covers every requester — a genuinely
/// weaponless rogue and a disarmed one reach it by the same route (decision 1863).
pub(super) fn unarmed_special(id: u16, main: Option<(u8, u8)>, off: Option<(u8, u8)>) -> u16 {
    if matches!(id, 57 | 58) && main.is_none() && off.is_none() {
        118
    } else {
        id
    }
}

/// The per-packet melee swing one-shot ids (decision 0073's tables) — what the whiff slow-down
/// (decision 0279) is allowed to touch when it finds them on the masked overlay.
pub(super) fn is_swing_id(id: u16) -> bool {
    matches!(id, 16..=19 | 85 | 87 | 88 | 117)
}

/// The client's COMBAT-anim classifier (`0x5fcc10` — the exhaustive byte-decoded table): the gate
/// on `PlayAnimation`'s combat fast-path (decision 0406). A combat clip requested while another
/// combat clip is playing is NOT armed — the current clip's rate doubles and the request defers.
/// Members: the swings (16–19 main, 85/86 dagger, 87/88/117 off), the specials (57/58/118 —
/// Eviscerate's weapon-remapped spin among them — and 59), the defenses (20–23 parry, 24
/// shield-block, 30 dodge), 10, 36, and Kick 95. The Ready idles (25–28) are NOT combat — a
/// swing over a ready idle arms normally.
pub(super) fn is_combat_anim(id: u16) -> bool {
    matches!(id, 10 | 16..=24 | 30 | 36 | 57..=59 | 85..=88 | 95 | 117 | 118)
}

/// The client's **CAST** classifier (`0x5fcbb0`): the spell-cast release anims
/// `{2, 32, 33, 53, 54}`. Together with [`is_combat_anim`] it is what the **transplant** predicate
/// (`0x5feae0`) tests on the *currently armed bone-0 clip*: a locomotion request over one of these
/// does not replace it — the clip moves up onto the key-bone at its live play position
/// ([`OneShotRoute`]'s two slots, decision 0878). NOT the ReadySpell holds (51/52) — those are
/// their own set (`0x5fde40`), and a jump over a standing hold really does take the whole body.
pub(super) fn is_cast_anim(id: u16) -> bool {
    matches!(id, 2 | 32 | 33 | 53 | 54)
}

/// Whether `cands` (this frame's [`gait_candidates`] pick) is bare `[STAND]` — the one precedence
/// slot the looping **state-emote idle** (`UNIT_NPC_EMOTESTATE`: `/dance`, NPC cooking/sweeping
/// flavor loops) is allowed to fill instead, exactly where Stand itself would otherwise sit.
/// Everything that already outranks plain Stand keeps outranking it: swim/backward/moving/turning
/// return their own arrays above, the Ready idle requires `ready.is_some()` (i.e. engaged), and a
/// chair-loop stand-state (4/5/6) has its own arm before the default. `current_special` (jump, the
/// sit/sleep/kneel poses) is checked by the caller *before* [`gait_candidates`] is even invoked, so
/// this predicate never has to account for Specials itself — they simply never reach here. Movement
/// starting again drops straight out of the state-emote idle for the same reason Stand does: the
/// very next frame's `cands` is no longer bare.
///
/// The **prowl idle** counts as bare too: [`STEALTH_STAND`] comes from `0x5fd830`, the *last*
/// resolver in the client's chain, so anything with a resolver of its own outranks it. (OUR
/// ordering, on that structural argument — where the emote-state idle sits in the real chain is
/// unpinned; a stealthed unit holding an emote state is degenerate either way.)
pub(super) fn is_bare_stand(cands: &[u16]) -> bool {
    cands == [STAND] || cands == [STEALTH_STAND, STAND]
}

/// The candidate array for a state-emote idle occupying the bare-Stand slot: the resolved anim id
/// first, `STAND` as the fallback should the model lack it — resolved through the same
/// [`super::find_resolved`] path as every other gait candidate (decision 0082). Call only when
/// [`is_bare_stand`] held for this frame's `cands`.
pub(super) fn state_emote_gait(emote_anim: u16) -> [u16; 2] {
    [emote_anim, STAND]
}

/// The mainhand swing animation id (decision 0073 — byte-verified `0x6246a0`): keyed on the wielded
/// item's `(class, subclass)`. Anything that isn't an equipped melee weapon — empty hand, non-weapon
/// item, fist weapon, every ranged/wand, obsolete(9), unknown subclasses — swings AttackUnarmed(16).
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

/// The offhand swing animation id (decision 0073, HitInfo bit `0x4`): a dagger stabs (88
/// AttackOffPierce), any other equipped weapon swings 87 (AttackOff), an empty/non-weapon offhand
/// punches (117 AttackUnarmedOff).
pub(super) fn swing_anim_off(wielded: Option<(u8, u8)>) -> u16 {
    match wielded {
        Some((2, 0xf)) => 88,
        Some((2, _)) => 87,
        _ => 117,
    }
}

/// The draw/stow clip for one weapon slot, by the item's **sheathe-type** byte — the client's
/// pick (8 byte-identical sites, `0x611930`–`0x611cc6`): `(1 << (type & 0x1f)) & 0x88` →
/// HipSheath(90) for types {3, 7}, Sheath(89) for everything else (back-mounts, shields, staves).
/// The slot's own byte is the *only* input — mainhand, offhand, shield and ranged all run the same
/// test, each on its own record.
pub(super) fn sheath_clip(sheath_type: u8) -> u16 {
    if (1u32 << (sheath_type & 0x1f)) & 0x88 != 0 {
        90
    } else {
        89
    }
}

/// The ranged-handling anims exempt from the `&0x10` force-stow while ranged-drawn — the
/// exemption is the predicate `0x5fe180` (sole caller `0x5fe04c`, the **CUR==2 path only**;
/// index table `0x5fe1a8`, targets `0x5fe1a0`), returning true for exactly these nine ids —
/// the ranged Load/Hold/Attack family. **ReadyThrown 108 is NOT exempt** and genuinely stows;
/// the real thrown wind-up survives by the snap **bracket** around every ranged kit play
/// (`driver`'s CastHold-ranged override), not by an exemption. The explicit trio compare at
/// `0x5fe037`–`0x5fe04a` (105/106/112) is a fast-path subset.
const SHEATH_RANGED_EXEMPT: [u16; 9] = [46, 49, 105, 106, 107, 109, 110, 111, 112];

/// The per-animation sheath reconcile (decision 0080 — the client's `0x5fdf80`, run after every
/// animation pick): given the unit's current sheath state and the playing clip's
/// `AnimationData.dbc` WeaponFlags, the state the policy *forces*, or `None` to leave it alone.
/// Priority-ordered, first match wins, every force is a SNAP:
///
/// 1. flag `&4` → stow (casts, swim, mount, sit-chair, loot);
/// 2. mounted → stow — the persistent mounted draw-block (`0x5fdfd9`: the reconcile forces
///    state 0 on every recompute while a mount model is attached, so weapons sit at their sheath
///    points and the manual toggle can never stick; dismount does NOT restore the pre-mount state);
/// 3. flag `&0x10` → stow (emotes, unarmed attacks, sit-ground/kneel, bow/rifle/thrown shots) —
///    except the three ranged-handling anims while ranged-drawn;
/// 4. engaged **or** flag `&0x20` → draw melee (armed attacks, the Ready idles, fishing) — only
///    while not ranged-drawn;
/// 5. a remote unit with no force pulls back to the server's descriptor byte (`0x5fe16e`; the
///    local player's committed state is never server-reconciled).
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

/// Whether a **looping base arm** keeps the deterministic HEAD variation instead of rolling a
/// fresh one — the client's re-zero gate on the arm helper (`0x5fdba0`): every base (re-)arm
/// carries `variationIdx = −1` (a weighted roll) **unless** the unit has an auto-attack target, is
/// holding a cast/channel, or the *outgoing* clip was a combat/cast/ready id — then it is forced to
/// `0` (the head). This is why a relaxed unit "looks around" on each Stand re-arm while a fighting
/// one holds one steady idle. The outgoing-id families here approximate the client's four
/// classifier calls (`0x5fcc10`/`0x5fcbb0`/`0x5fde40`/`0x5fde60`, exact id sets unpinned): the
/// melee swings (16–19), the ready stances (25–29, the 0111-cited set), and the spell ready/cast
/// poses (51–54) — engagement and the cast hold carry the main weight regardless (decision 0123).
pub(super) fn arm_forces_head(engaged: bool, casting: bool, outgoing: u16) -> bool {
    engaged || casting || matches!(outgoing, 16..=19 | 25..=29 | 51..=54)
}

/// The victim wound-flinch id (`0x60ea70`, decision 0111 §5.3 — byte-verified): decided **solely**
/// by `(severity, engaged)`. Crit (`HitInfo & 0x80`) → CombatCritical; the victim engaged in
/// melee (its auto-attack-target GUID set, `[unit+0xc48]`) → CombatWound; else StandWound.
pub(super) fn wound_anim(hit_info: u32, engaged: bool) -> u16 {
    if hit_info & 0x80 != 0 {
        10 // CombatCritical
    } else if engaged {
        9 // CombatWound
    } else {
        8 // StandWound
    }
}

/// Whether the wound overlay covers the **full body** (the client forces op4's key-bone to `-1` =
/// bone 0) or stays **masked** to the upper-body subtree — the two byte-decoded mechanisms
/// (decision 0111 §5.2, `0x60eae8` / `0x60eb9a`):
///
/// - **(A), all ids:** the victim's current bone-0 pose is a combat-ready stance {25–29} — a
///   weapon-drawn victim standing between its own swings flinches full-body. `base_anim` is the
///   **resolved** id actually driving the base track (the client reads the armed record,
///   `[block0+0xf8]`), so mid-swing the base is the swing — not ready — and the flinch masks.
/// - **(B), StandWound(8) only:** genuinely stationary — the client's `[+0x118]+0x40 & 0x20200f`
///   (move/jump/swim; note the keyboard-turn bits `0x30` are **not** in the mask) — **and not
///   mounted** (the secondary-blend note's `[unit+0xdc]==0` companion clause; a mounted rider's
///   flinch never replaces the seat pose, decision 0441). The transport-substate companion is
///   still a state benilla doesn't model.
///
/// Everything else is masked: the legs keep the base animation untouched.
pub(super) fn wound_full_body(id: u16, base_anim: u16, flags: u32, mounted: bool) -> bool {
    matches!(base_anim, 25..=29)
        || (id == 8
            && !mounted
            && flags & (move_flags::ANY_MOVE | move_flags::FALLING | move_flags::SWIMMING) == 0)
}

/// The wound overlay's decay amplitude λ₀ — the client seeds `+0x108 = 0.75f` on the op4
/// `linkFlag=0` path (`0x712682`, decision 0111): the flinch peaks at 75% wound, never fully
/// replacing the pose underneath. (A normal clip transition's outgoing-pose fade uses 1.0.)
pub(super) const WOUND_AMPLITUDE: f32 = 0.75;

/// The wound overlay's Bevy graph weight this frame (decision 0111): the client's kernel computes
/// `λ = smoothstep(t) · λ₀` with `smoothstep(t) = (3 − 2t)·t²` and `t` the remaining fraction of
/// the decay window (`0x714880`: t runs 1 → 0 over the clip's span, so λ **decays** λ₀ → 0 — then
/// the slot self-releases), and blends `out = primary + (secondary − primary)·λ`. Bevy's
/// normalized weighted blend gives `out = others·base/(others+w) + w·wound/(others+w)`, so the
/// weight that lands the blend exactly on λ is `w = others · λ/(1−λ)` — `others` being the total
/// weight of the other animations driving the same subtree. Bounded: λ ≤ 0.75 ⇒ `w ≤ 3·others`.
pub(super) fn wound_weight(remaining_frac: f32, others: f32) -> f32 {
    let t = remaining_frac.clamp(0.0, 1.0);
    let lambda = (3.0 - 2.0 * t) * t * t * WOUND_AMPLITUDE;
    others * lambda / (1.0 - lambda)
}

/// The client's per-bone blend weight λ at amplitude 1.0 (`0x714880`–`0x714921`): `smoothstep(t)`
/// with `smoothstep(t) = (3 − 2t)·t²` and `t` the fraction of the blend window **still to run**,
/// so λ **decays 1 → 0** across it. Shared by the key-bone slot's two cross-fades (decision 0878):
/// the fade-to-rest that releases a finished one-shot over a fixed 150 ms (op4 `param_3 = −1`,
/// `0x7123af`: `+0x104 = 1/150`, amplitude `+0x108 = 1.0`) and the blended re-arm that cross-fades
/// a new masked clip in over the *incoming* sequence's own blendTime (`0x7125f2`). The wound's
/// twin ([`wound_weight`]) is the same curve at amplitude 0.75.
pub(crate) fn blend_lambda(remaining_frac: f32) -> f32 {
    let t = remaining_frac.clamp(0.0, 1.0);
    (3.0 - 2.0 * t) * t * t
}

/// The engaged standing idle (decision 0073 — the `0x5fd360` arm's weapon-class Ready pick,
/// `0x5fcdc0`). Note the buckets differ from the swing table: fist **and** dagger ready as 1H.
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

/// The Special state a unit is in this frame, if any. Airborne splits three ways: FALLINGFAR
/// latched → **Fall** (the 40 loop, `0x602c40`); else a **jump** arc (launched upward, `jump_arc`)
/// → Jump (the 37/38 bracket); else a **step-off fall** → `None` — the base selector's keep-current
/// freeze (`0x5fd8e8`): the gait keeps playing off the takeoff-frozen flags/speed until FALLINGFAR
/// latches or the unit lands. Standing with a transition-able stand-state (sit/sleep/kneel) → Pose;
/// movement suppresses a pose.
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

/// Where a requested one-shot (a swing or an emote) is routed this play (decision 0087; the route
/// flag `esi`, `0x5fe6c8..0x5fe74d`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum OneShotRoute {
    /// Onto the SpineLow-subtree **masked overlay** ([`AnimClip::upper_node`]): the torso plays the
    /// clip while the legs keep whatever the base is doing (run / sit / jump-arc). `Mode` is untouched.
    Masked,
    /// **Full-body on the base track** (bone 0): the clip replaces the base, legs included. Standing
    /// idle only — the clip's authored leg keys then read as-is (a swing lunges, a `/cheer` is
    /// full-body, a `/clap`/`/bow` reads waist-up because the *asset* barely keys the legs).
    FullBody,
}

/// The **CLASS_A** membership (`0x5fed90`) — the maskable-eligible set that gates the whole
/// state-route block; a non-CLASS_A id is always full-body. The load-bearing memberships were
/// byte-decoded (17/66/68/80 ∈; the 37–45 jump/swim/locomotion band excluded); the patchy interior of
/// the wide ranges is inferred and still open, but never load-bearing here — the only ids that
/// reach [`route_oneshot`] are swings, emotes, and the spell-kit cast anims (32/33/51–54 — CLASS_A
/// covers swings, emotes and casts), all squarely inside these ranges.
fn is_class_a(id: u16) -> bool {
    matches!(id,
        2 | 8..=10 | 14..=36 | 46..=49 | 51..=90 | 105..=113 | 117..=118 | 122..=138 | 185..=186 | 195)
}

/// The **COMBAT** membership (`0x5fcc10`) — the set whose airborne test can route to the mask
/// (a mid-jump swing). Byte-decoded memberships: **17 ∈**, and **66/68/80 ∉** (the emotes never mask
/// on airborne alone). Every swing id (16–19/85/87/88/117) is in it; no emote id is — and no CAST id
/// (32/33/51–54) either, which is why a jump-in-place cast routes FULL-BODY and replaces the hang
/// (decision 0864), while the same cast over a moving jump masks via the frozen-in move bits.
fn is_combat(id: u16) -> bool {
    matches!(id, 10 | 16..=24 | 30 | 36 | 57..=59 | 85..=88 | 95 | 117 | 118)
}

/// The **forced-full-body** carve-outs: Death-class `{1,6,131,132}` (`0x5fda90`) and the
/// sit-transition ids `{57,58,118}` (`0x5fec60`) route to bone 0 regardless of state. Kept faithful
/// though benilla never feeds these through the one-shot path.
fn is_forced_full_body(id: u16) -> bool {
    matches!(id, 1 | 6 | 131 | 132 | 57 | 58 | 118)
}

/// Route a requested one-shot (swing/emote id) by the unit's **live state**, per play — the client's
/// `esi` decision `0x5fe6c8..0x5fe74d` (decision 0087): masked onto the SpineLow overlay
/// when the lower body is committed — **moving/turning/swimming** (`[9e8] & 0x20003f`), a **non-Stand
/// stand-state** (seated/sleep/kneel/chair, `standState ≠ 0`), or a **combat** id while **airborne**
/// (`activeCMovement+0x40 & 0x2000`); **full-body** on bone 0 when standing idle (none of those). The
/// id gates only *which* tests apply (CLASS_A gates the block; COMBAT gates the airborne test); it
/// never decides maskability by itself — the same Attack1H is full-body standing and masked running.
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

/// `AnimationData` ids whose playback rate the client scales by movement speed (`0x5fee80`):
/// the locomotion gaits. An id outside this set plays at rate 1×. The **same** table is the
/// LOCOMOTION membership the transplant predicates key on ([`is_locomotion`]).
const RATE_SCALED: &[u16] = &[4, 5, 11, 12, 13, 37, 38, 39, 42, 43, 44, 45, 135, 143, 187];

/// The client's **LOCOMOTION** membership (`0x5fee80` — the very table the rate scaler uses):
/// what the transplant predicates test as "the clip being *requested* is a base locomotion clip"
/// (`0x5feae0`/`0x5fe912`). Note Fall(40) and SwimIdle(41) are
/// **not** members — a FALLINGFAR latch mid-cast replaces the cast on bone 0 rather than
/// transplanting it, exactly as the bytes order it.
pub(super) fn is_locomotion(id: u16) -> bool {
    RATE_SCALED.contains(&id)
}

/// Whether the gait this movement state resolves to is a **locomotion** id — i.e. whether a
/// movement-flag re-arm is the transplant's `0x5fee80` request or an ordinary bone-0 overwrite.
///
/// The re-arm at a flag change has no id in hand yet (the gait is recomputed the following frame),
/// so this asks [`gait_candidates`] what it *will* pick. `ready`/`ranged_load` are passed `None`
/// deliberately and it changes no answer: both are **standing** idles that locomotion outranks in
/// the cascade, so a moving unit picks its locomotion id whether or not they are set, and a still
/// one picks something that is not locomotion either way.
///
/// This is the gate Ice Block turns on. A stun's root wipes the direction bits, the flag change
/// re-arms, and the pick is **Stand(0)** — *not* locomotion — so the cast one-shot on bone 0 is
/// **overwritten** rather than transplanted onto the torso: the character goes fully neutral before
/// the freeze catches it (decision 0894, the director's reading of the reference client).
pub(super) fn gait_is_locomotion(state: &MovementState, walk_speed: f32) -> bool {
    gait_candidates(state, walk_speed, None, None)
        .first()
        .is_some_and(|id| is_locomotion(*id))
}

/// The playback rate for a clip given the unit's live speed and its rendered model scale — the
/// client's `0x5fe2f0` divide (`0x5fe4be..0x5fe550`):
///
/// ```text
/// 5fe508  call 0x711a20   ; DIVISOR = ‖row0(M+0xbc)‖ · M2Sequence[reqId].moveSpeed
/// 5fe515  fcomp 0.0 ; jne ; GUARD A: divisor > 0     (else stay 1×)
/// 5fe528  call 0x5fee80   ; GUARD B: the locomotion id whitelist (else stay 1×)
/// 5fe53c  call 0x7c4c90   ; NUMERATOR = the flag-scalar current speed
/// 5fe541  fdiv            ; rate = speed / (moveSpeed · scale)
///
/// M      = CGUnit+0xdc (the mount) if nonzero, else CGUnit+0xd8 (the body)
/// ‖row0‖ = (mounted ? CGUnit+0x9c : 1.0) · CGUnit+0x94 · CGUnit+0x90
/// ```
///
/// **`model_scale` is the half we were missing** (decision 0903). A sequence's `moveSpeed` is the
/// ground speed its authored cycle covers *at the model's authored size*; render that model at `s×`
/// and one cycle covers `s×` the ground, so the legs must cycle `s×` slower to hold the same speed.
/// Without the divisor every off-1.0-scale creature ran its gait exactly `s×` too fast — a Gordok
/// Ogre-Mage (`CreatureDisplayInfo.CreatureModelScale` 2.2) scurried its walk at 2.2× rate, and a
/// 1.5× riding sabre its run at 1.5×.
///
/// The scale to pass is the **rendered world scale of the model actually playing the clip** — which
/// is what the two terms above are: `CGUnit+0x90` is `OBJECT_FIELD_SCALE_X` (the server has already
/// folded the DBC scale in — `entities::attach`), and `+0x9c` is a mount's own
/// `CreatureDisplayInfo` column, composed under the rider's. Our unit transform and mount-child
/// transform carry exactly those, so `transform.scale.x × host` reproduces the product (decision
/// 0910). Two exactness notes:
///
/// - The client takes the **L2 length of row 0** of the model's world matrix, not an `abs()`. For a
///   *uniform* scale — which every unit of ours is (`Vec3::splat`) — the two are identical at any
///   rotation, since row 0 is `s · (a unit row)`. `abs()` is the cheaper spelling of the same
///   number, and is only a distinction if unit scale ever stops being uniform.
/// - `CGUnit+0x94`, a spell-visual scale multiplier clamped to `[0.75, 2.0]` (1.0 when idle), is a
///   **third** factor we do not carry — because we have no producer for it: the aura CharProc
///   dispatch (`crate::aura_visual::node_for`) implements Alpha/Tint/AnimRate and nothing that
///   scales a unit. Whoever builds that layer must multiply it in here, or a scaling spell visual
///   will re-time the gait wrongly for its duration. (`CGUnit+0x98`, the fourth slot, is a literal
///   constant 1.0 in the binary — its constructor is its only writer.)
pub(super) fn playback_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> f32 {
    scaled_rate(clip, speed, model_scale).unwrap_or(1.0)
}

/// [`playback_rate`] as the scaler's own question — **`Some` only where the scaler applies at
/// all**: both of `0x5fe2f0`'s guards passed (a positive divisor, a locomotion id).
///
/// The distinction is what the per-frame rate write
/// ([`sync_base_rate`](super::driver::play::sync_base_rate)) needs, and it is not pedantry: the
/// scaler is one rate producer among several. The combat fast-path doubles an already-playing
/// swing to 2× (decision 0406, op6 `2.0f`), the whiff slows a missed one to 0.5×, decision 0503
/// freezes an airborne snapshot to 0×. Each owns the clip it wrote, and a per-frame write that
/// returned a blanket `1.0` for "the guards failed" would silently stomp all three (it did — the
/// suite caught it, decision 0906).
/// The `abs()` is on the **scale only** — deliberately, and the asymmetry is the client's (decision
/// 0912). `‖row0‖` is a matrix-row length, so it is never negative; `moveSpeed` is *signed*, and a
/// backwards gait is authored negative (`RidingKodo.m2` WalkBackwards, `-2.5`). Guard A's strict
/// `> 0` therefore leaves an authored backwards clip at a flat 1× while a model that falls back to
/// forward Walk (`+2.5`) gets speed-scaled. Do not "tidy" this into `move_speed.abs()`.
pub(super) fn scaled_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> Option<f32> {
    let divisor = clip.move_speed * model_scale.abs();
    (divisor > 0.0 && RATE_SCALED.contains(&clip.anim_id)).then(|| speed / divisor)
}

/// Build the unified movement view, in precedence order: a populated [`MovementState`] (our avatar,
/// driven by the controller) wins; else a remote player's live `moveFlags` + extrapolation speed
/// ([`RemoteMotion`], from the relayed `MSG_MOVE_*` stream); else a creature's server spline (it moves
/// forward at the path speed); else stationary. The remote flags are raw CMovement bits, which already
/// match [`move_flags`], so the same selector animates a remote player's walk/run/strafe/backpedal/turn.
///
/// `creature_swimming` is the creature legs' swim state ([`crate::net::CreatureSwimming`] — the wire
/// never carries `SWIMMING` for a creature, so it's derived from the water over its feet): it folds
/// the flag into the spline/stationary views, so a pathing water creature plays Swim 42 and an idle
/// one treads water (SwimIdle 41) instead of running/standing on the lakebed. Ignored for the
/// self/remote legs — their flag arrives properly.
///
/// `modes` is the unit's **server-granted** movement modes ([`crate::net::UnitMoveModes`], decision
/// 1780) — the `SMSG_SPLINE_MOVE_*` family. They are OR'd into the flags word on the remote and
/// creature legs, because in the reference they are bits of the *same* `CMovement+0x40` word the
/// selector already tests; a creature the server has put in walk mode or rooted has those bits set
/// exactly like a player who arrived carrying them in a pose. The self leg deliberately skips them:
/// our own mover's modes are the ack'd family's, held on [`crate::player::state::MoveModes`] and
/// already folded into the controller's own `MovementState`.
pub(super) fn unify(
    movement: Option<&MovementState>,
    remote: Option<&RemoteMotion>,
    spline: Option<&Spline>,
    creature_swimming: bool,
    modes: Option<&crate::net::UnitMoveModes>,
) -> MovementState {
    // The flying-spline fact is read through to the live `Spline` on EVERY leg — the client's
    // selector reads the active CMovement's spline flags at select time (`0x5fd19c`),
    // so a self ride (MovementState leg) and a remote taxi (spline leg) both fly.
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
