//! The avatar's shared state + the movement constants — [`Player`] (the controller's resource),
//! its transport attachment ([`PlayerRide`]), the capsule/speed resources, and every binary-derived
//! movement constant the controller family reads. Pure state, no systems: [`super::control`] and
//! the concern modules beside it (`mover`, `swim`, `arc`, `movement_net`, …) all borrow from here.

use avian3d::prelude::*;
use benilla_protocol::{JumpInfo, MoveMode};
use bevy::prelude::*;

/// Backpedal speed as a fraction of run: vanilla `MOVE_RUN_BACK` 4.5 / `MOVE_RUN` 7.0. Moving backward
/// (the `0x2` backward move-flag) selects the backward speed over run, **dominating strafe** —
/// the speed getter `FUN_007c4c90` tests only bit `0x2` on the forward/back axis, never the
/// forward/strafe bits — and the backward arm is a **`min(runBack, run)`**, not a plain select
/// (`0x7c4d1d`; identical in template to the swim pair's `min(swimBack, swim)`). runBack is
/// server-seeded (the ctor zeroes it;
/// standard vanilla 4.5), so we keep it as a ratio of the configured run speed → a `$WOW_MOVE_SPEED`
/// override / Ctrl sprint scales backpedal too. A backward *jump* inherits this automatically: takeoff
/// freezes the current horizontal speed (`FUN_007c61f0` never rewrites it), so a backward jump lands
/// ~36% shorter — no separate constant.
pub(super) const RUN_BACK_RATIO: f32 = 4.5 / 7.0;

/// Walk speed as a fraction of run: vanilla `MOVE_WALK` 2.5 / `MOVE_RUN` 7.0 — the sibling of
/// [`RUN_BACK_RATIO`], and used the same way and only for the same reason: `$WOW_MOVE_SPEED`
/// replaces the server's speed set outright, and the walk gait has to stay a walk under it
/// rather than becoming a second run. The live number is always the server's (`MoveSpeeds::walk`,
/// seeded by the create block and moved by `SMSG_FORCE_WALK_SPEED_CHANGE`); this ratio never
/// reaches a real session.
pub(super) const WALK_RATIO: f32 = 2.5 / 7.0;

/// Turn rate (rad/s) **fallback** — how fast A/D rotate the facing when not mouse-looking, used
/// only until the mover's own sixth movement speed arrives.
///
/// The live rate is per-unit and server-authoritative: `CMovement+0x9c`, last of six consecutive
/// speed floats on the **mover's own** `CMovement`, set by `0x7c6ff0` (which acks with
/// `CMSG_FORCE_TURN_RATE_CHANGE_ACK`, `0x2df`) and read for held turns by `GetYawRate 0x7c5c50`.
/// The base ctor `0x7c4850` *zeroes* all six, so the client keeps no default of its own — π is the
/// vanilla server's value, and this constant stands in only for frames before a create block lands.
/// (`0x7c4f30` heading integrate; the speed block).
///
/// Because it lives on the mover, a possessed creature turns at **its** rate, not ours. Reduced to
/// [`TURN_RATE_MOVING`] while translating **or falling** (`flags & 0x200f`), and the same cell is
/// the held-*pitch* rate — that integrator's keys are default-unbound, so we never bind it.
/// Decision 0050.
pub(super) const TURN_RATE: f32 = std::f32::consts::PI;

/// The mouselook swim-pitch clamp (radians) — ±89.0° = 1.5533431 (`0x8089d8` =
/// `0x3fc6d3f2`, the camera→SetPitch path's clamp; decision
/// 0492). NOT ±π/2 — that clamp belongs to the separate, rate-limited pitch-KEY integrator
/// (`0x7c4f80`), whose keys are default-unbound and which we don't bind.
pub(super) const MOUSELOOK_PITCH_CLAMP: f32 = 1.553_343;

/// The **shallowest mover pitch that still lets water walking see the water** (radians) —
/// **VERIFIED** −37.0° = −0.6457718 (`[0x80dfe8] = 0xbf25514d`, read from the image), the third
/// gate of the trace-mask arm `0x6315f0`:
///
/// ```text
/// 63161e d9 46 20        fld   dword ptr [esi + 0x20]     ; the mover pitch
/// 631621 d8 1d e8 df 80  fcomp dword ptr [0x80dfe8]       ; -37.0 deg
/// 631627 df e0 / f6 c4 41  fnstsw ax ; test ah, 0x41
/// 63162c 75 06           jne   0x631634                   ; ZF form -> skip the arm
/// 63162e 81 cf 00 00 03  or    edi, 0x30000               ; liquid layers 0-1 into the mask
/// ```
///
/// The emitted `jne` reads **ZF**, so the liquid layers are added only when `ah & 0x41 == 0`, i.e.
/// **pitch strictly greater** than −37°; at exactly −37° the arm is skipped. Aim down past it and
/// the water stops being geometry, which is how a water-walker gets back *into* the water.
/// (`SetPitch 0x7c6f70`'s own complement at `0x7c6fb3` is the standstill half of the same move —
/// see the note in [`super::mover::step`] on why we need no second kick.).
pub(super) const WATER_WALK_PITCH_FLOOR: f32 = -0.645_771_8;
/// Turn-rate scale while also translating (moving/strafing) — the verified `×0.75` (`flags & 0x200f`).
pub(super) const TURN_RATE_MOVING: f32 = 0.75;

/// The stationary body catch-up: once steering input stops, the rendered body closes on the aim at
/// `turnRate × 8` rad/s, gap-clamped (the client's chase, `0x607ed0` tail — its clock is stamped
/// every non-steering frame, so elapsed ≈ one frame). While steering, the catch-up is FROZEN and
/// only the 90° ceiling moves the body — the head-leads-then-body-follows turn-in-place
/// (`0x60818a`–`0x6081bf`).
pub(super) const STATIONARY_CHASE_RATE: f32 = 8.0;

// ── Character-controller feel knobs ──────────────────────────────────────────────
// These are binary-derived values kept because they give the WoW feel cheaply — *tunables*, not
// fidelity targets. The mechanism is a thin kinematic controller over avian's `MoveAndSlide`; further
// refinements (accel/decel curves, partial air control beyond the one-shot nudge) dial up from here.

/// Player capsule radius (yd) — the vanilla `CMovement` ctor's ±1/3 half-width, and a *tunable* per
/// the block header above, not a fidelity target.
///
/// Known since to be the ctor **placeholder**, not the real mover's radius: `0x6174b0` overwrites
/// `+0xb0`/`+0xb4` from `CreatureModelData` on every model build (`0x5fb880` → `0x5fb9dd` passes
/// `force = 1`, which skips the only refusal path), so the live extents are per-model — human male
/// radius **0.30555**, human female **0.20835**. Adopting them is the same
/// movement-fidelity question [`CAPSULE_HEIGHT`] describes below, and this radius is its other
/// half.
pub(crate) const CAPSULE_RADIUS: f32 = 1.0 / 3.0;
/// Player capsule total height (yd) — the **movement** capsule avian sweeps, deliberately a
/// constant. Numerically the vanilla ctor-default collision height it was derived from
/// ([`DEFAULT_COLLISION_HEIGHT`]), but it is not the same quantity and no longer stands in for one:
/// a unit's real collision height is per-model and lives on [`crate::entities::CollisionHeight`].
/// This one is a *feel knob* per the block header above — it feeds the swept box,
/// the step-vs-fall election's reach and the head/feet offsets, where going per-race would change
/// where every short race can walk, step and fit. That is a movement-fidelity question of its own
/// (our kinematic capsule vs the reference's k-DOP), not the depth-line question 0645 settled, so
/// the two are kept apart on purpose rather than by oversight.
pub(crate) const CAPSULE_HEIGHT: f32 = 2.027_777_7;

/// The client's own **empty-world collision height** (yd) — the `CMovement` ctor's immediate
/// `0x4001c71c` at `0x616fd8`, which the
/// per-unit param setter `0x6174b0` overwrites from the unit's model. This is the fallback every
/// depth line takes for a unit whose display id doesn't resolve to a `CreatureModelData` row — the
/// same role it plays in the reference, and what vmangos falls back to as well (its `2.f`).
///
/// It is **not** a stand-in for a real unit's height: see [`crate::entities::CollisionHeight`].
pub(crate) const DEFAULT_COLLISION_HEIGHT: f32 = 2.027_777_7;
/// Downward gravity (yd/s²) — binary-VERIFIED vanilla value (set on avian's `Gravity` too; matches
/// vmangos `Movement::gravity` exactly). Shared with the remote dead-reckoner ([`crate::net`]), which
/// integrates a relayed jump's arc under the same gravity so an observer's view matches the mover's.
pub(crate) const GRAVITY: f32 = 19.291_105;
/// Jump take-off speed (yd/s) — binary-VERIFIED vanilla value.
pub(super) const JUMP_SPEED: f32 = 7.955_547;
/// Terminal fall speed (yd/s) — binary-VERIFIED vanilla value (matches vmangos `terminalVelocity`).
/// Shared with [`crate::net`]'s ballistic integration (caps a long fall's vertical speed).
pub(crate) const TERMINAL_VELOCITY: f32 = 60.148_003;
/// **Terminal fall speed under feather fall** (yd/s) — the *whole* of what Slow Fall does. The
/// reference's fall-velocity query `0x7c5d20` picks its clamp from one flag test
/// (`0x7c5d23 test [ecx+0x40], 0x20000000`): the ordinary cap `[0x87d894]` = 60.148, or this one
/// `[0x87d898]` when `MOVEFLAG_SAFE_FALL` is set. `0x7c5f50` makes the same select as `0x7c5d20`.
///
/// The same 7.0 shows up on the server: vmangos raises its anticheat's expected jump speed to
/// `max(current, 7.0f)` on receiving a feather-fall ack (`HandleMovementFlagChangeToggleAck:576`) —
/// it is matching this clamp.
pub(crate) const FEATHER_TERMINAL_VELOCITY: f32 = 7.0;
/// **How far above the ground a hovering body rests** (yd) — the whole of what Hover does. The
/// reference extends the walk resolver's down-probe by `[0x7ff9d8]` = 1.0 (`0x636dd2`) and then
/// subtracts it back out of the snap, landing the body exactly a yard clear: a **direct write to
/// `CMovement.pos.z`** (`0x636e81`–`0x636ea9`) rather than a probe widening.
pub(crate) const HOVER_HEIGHT: f32 = 1.0;
/// **How fast a hovering body rises to that clearance** (yd/s). The snap itself can only *lower* the
/// body — `pos.z −= max(L − 1.0, 0)` (`0x636e52`–`0x636ea9`) skips its write entirely when the floor
/// is already within a yard — so the rise is a second, rate-limited pass at `0x636fa1`–`0x6370f1`
/// whose rate comes from `0x7c61b0` returning `[0x87d898]`: the very same 7.0 the feather-fall clamp
/// reads. Without it a granted hover reads as an instant pop instead of a float.
pub(crate) const HOVER_CLIMB_RATE: f32 = 7.0;
/// Standability gate: a surface is walkable iff its normal is within ~50° of straight up (cos 50° —
/// the vanilla threshold). Steeper than this you can't climb and you slide back down.
pub(super) const GROUND_COS: f32 = 0.642_788;
/// Downward probe distance (yd) to decide whether we're standing on ground.
pub(super) const GROUND_PROBE: f32 = 0.2;
/// The post-move downward snap's **slope ratio** — the client's step-vs-fall election
/// (`0x6367b0`, constant `[0x80c740]` = 1.8493990): the snap
/// probe reaches `d_h · ratio + slack + collision height` below the post-move position, where
/// `d_h` is the frame's achieved horizontal travel. Scaling by the travel makes the absorbed
/// *slope* the constant (atan 1.8494 ≈ 61.6°, comfortably above the 50° walkable limit),
/// frame-rate independent; the collision-height term (our [`CAPSULE_HEIGHT`]) is
/// what absorbs a discrete ledge: a fence-height drop is a silent straight-down step, only a deeper
/// floor becomes a fall. That term is a **known deviation** on two counts — the reference's `H` is
/// 1.0, not 2.028, and its extension is `0x4000000`-gated rather than always-on.
///
/// The ratio itself is confirmed to be one constant with two jobs, exactly as we use it: `0x80c740`
/// has four references image-wide, and they are the foot cone's waist ring (`0x631c0b`) and this
/// step-down term (`0x636dda`, inside `0x6367b0`) among them — the same 61.6° serving the climb and
/// the descent.
pub(super) const STEP_SLOPE_RATIO: f32 = 1.849_399;
/// The election's fixed slack (yd) added to the travel-scaled snap reach — `[0x7ff9d0]` = 1/36 yd.
pub(super) const STEP_SNAP_SLACK: f32 = 0.027_777_8;
/// The step-up **rise ceiling** (yd): how tall an obstacle the atomic step-up can walk you onto —
/// the reference's own `H`, VERIFIED.
///
/// 0209 set this to 0.7 as a deliberately modest tunable, on the understanding that the reference's
/// budget was a ~2 yd body height we did not want. That understanding was wrong on its central fact:
/// `0x617430` returns `CMovement+0xb8`, which is **not a height** but the dimensionless ratio
/// `max(SCALE_X / CreatureModelScale, 1)`, and a complete writer census puts it at **1.0** for a
/// clean local player. The reference's rise budget was never 2 — it was 1.0, and the
/// gap we were preserving was 0.7 vs 1.0, not 0.7 vs 2.
///
/// 0209's invariant is untouched by closing it: a fence's collision top is 1.8–2.3 yd, so fences,
/// trunks and walls still slide at exactly the height they always did. What changes is the class
/// 0209's note invited us to fix — "one number to nudge if a real spot feels too restrictive" —
/// which the director's captures found twice: a 0.91 yd Stormwind step (`14282v1`, a 66° face onto a
/// flat top) and 1121's deferred 1.04 yd ledge.
pub(crate) const STEP_UP_HEIGHT: f32 = 1.0;
/// The rise budget of a body the reference does **not** treat as player-controlled — a creature, a
/// pet, a charmed/possessed/feared/confused/rooted player: `0x617430` takes
/// `0x5fa550`'s FALSE leg and returns the constant
/// `2.0` at `[0x801628]`, where a player's is its own `[CMovement+0xb8]` = 1.0. So a creature steps
/// twice as high as we do, and its certify advance (`H·tan50°`) reaches twice as far; the creature
/// clamp's walker step ([`crate::net::motion::spline::ground_clamp_creatures`]) is the consumer.
pub(crate) const CREATURE_STEP_UP_HEIGHT: f32 = 2.0;
/// The step-up **certify advance** (yd): how far ahead the maneuver looks for the tread it would
/// stand on. A property of the BODY, never of the frame.
///
/// 0209 advanced by this frame's own travel, which made every kerb in the game a frame-rate
/// lottery: at 60 fps a run is 0.117 yd of travel, at 144 fps it is 0.049, and walking is 0.041 —
/// while the capsule's own radius is [`CAPSULE_RADIUS`]. A settle probe that far forward is still
/// over the riser, so it lands on the riser's face and the walkable gate rejects it. Measured at
/// Stormwind's Trade District kerbs: the live 0.117 rung fails, 0.20 and beyond
/// commit onto the tread, at every one of the seven captured contacts. Sidewalks 0.28 yd tall
/// refused to be stepped onto because the probe never reached them.
///
/// A body radius was the principled *guess* — exactly the distance at which the capsule's footprint
/// has cleared the lip it is standing against, and the same number whatever the frame rate or the
/// gait. It is now superseded by the measured one: the reference steps
/// `max(H·tan50°, radius + 1/720)` square into the certified face, which with the verified `H` of 1.0
/// is **1.1918 yd** — three and a half times our radius. 1121 reached for a body-scaled number in the
/// absence of this one and got the *shape* of the answer right; the magnitude was never ours to
/// invent. The frame's travel still wins when it is longer, so this remains a floor, not a fixed
/// reach.
///
/// It does not widen what may be climbed, and that argument is what makes the larger reach safe: the
/// rise ceiling is still [`STEP_UP_HEIGHT`], the settle must still find a *walkable* floor higher
/// than the feet, and — the load-bearing part — **the elevated forward sweep is clipped by anything
/// in the way**, so the advance only ever reaches as far as there is clear air at the raised height.
/// A fence, a trunk and a two-trunk pinch still block at the same body height, with a clipped advance
/// and a net-zero settle. What the longer reach buys is the case the director's capture found: a step
/// whose *face* is unwalkable for most of a yard before the flat top begins, where a probe that stops
/// at one radius is still over the face and reads it as a steep floor.
pub(super) const STEP_UP_ADVANCE: f32 = 1.191_753_6;
/// The certify reach **per yard of rise budget** — `tan 50°`, since the reference's reach is
/// `max(H·tan50°, r + 1/720)` with `H` the mover's own budget (`0x636147`–`0x636190`).
/// [`STEP_UP_ADVANCE`] is this at the player's `H` of 1.0; a creature's 2.0
/// (`CREATURE_STEP_UP_HEIGHT`) reaches 2.38 yd through the same law.
pub(super) const STEP_UP_ADVANCE_PER_YARD: f32 = STEP_UP_ADVANCE / STEP_UP_HEIGHT;
/// The **foot cone's height** (yd): how far above the feet the reference's movement solid is still
/// narrower than its full radius — and therefore the band within which a blocking edge is *ridden
/// up* rather than stepped onto.
///
/// The real client's movement solid is not a capsule. Its lower half is a **cone**: the k-DOP build
/// at `0x631440` emits 9 planes — 4 vertical box sides at `center.xy ± radius`, and 4 bevels running
/// from a point at the foot out to `(pos.xy ± radius, pos.z + r')` with `r' = radius · 1.8493990`
/// (`[0x80c740]` — our [`STEP_SLOPE_RATIO`]), above which it is a plain vertical box. There is no
/// −Z plane: the solid is open at the bottom.
///
/// So a *low* edge never meets a wall — it meets the slanted skirt, and the resolver's ordinary
/// slide carries the body up it at the cone's own surface slope, atan 1.8494 ≈ 61.6°, gaining
/// `1.8494 · cosθ · len` per step (`0x635c00`'s `T`). Only an edge **above** this height meets
/// the vertical box square, and that is the case the instant step-up ([`STEP_UP_HEIGHT`]) exists
/// for. One solid, two behaviours, selected by the edge's height above the feet.
///
/// Not a new tunable: it is our own two constants multiplied, so it is *our* mover's cone — the band
/// is a property of the body's radius, and ours is [`CAPSULE_RADIUS`].
///
/// It is **not** the real client's band, and the gap is recorded rather than papered over (decision
/// 1125, correcting 1123's claim that it matched exactly). Because [`CAPSULE_RADIUS`] is the ctor
/// placeholder, ours comes out at 0.616 against a live human male's **0.565** and a human female's
/// **0.385** — ~9% and ~60% generous. It only decides ride-vs-pop for obstacles *between* the two
/// bands, so the cost is a narrow slice of over-smooth rides on tall-ish steps; closing it properly
/// means per-model mover extents, which moves far more than this constant.
pub(super) const FOOT_CONE_HEIGHT: f32 = CAPSULE_RADIUS * STEP_SLOPE_RATIO;
/// The landing probe (yd): while airborne, walk mode resumes only this close to the floor, so
/// the arc ends where the slide actually contacts (skin scale) instead of [`GROUND_PROBE`]
/// early — which cut the last ~0.2 yd of every fall into a same-frame snap, the visible pop at
/// every silent landing.
pub(super) const LAND_PROBE: f32 = 0.05;
/// Wedge-rest detection: a "fall" that is no longer falling. A capsule can
/// come to rest held between two steep faces — the flaring trunk bases at the Northshire trees
/// form exactly this funnel (contact normals ~0.2 up) — where gravity feeds the slide, the
/// opposing contacts cancel it, and with mid-air control locked (vanilla momentum rules) the
/// falling pose is permanent. This many consecutive *stalled* frames (see
/// [`WEDGE_STALL_RATIO`]) is unambiguously a rest — land there: the fall ends, walking control
/// returns, and stepping off the support resumes a normal fall. Nothing becomes walkable or
/// climbable by this.
pub(super) const WEDGE_STILL_FRAMES: u8 = 3;
/// A frame counts as stalled when the achieved descent is under this fraction of the descent
/// gravity intended (`vel_y·dt`), already falling faster than [`WEDGE_MIN_FALL`]. Free fall
/// achieves ~100% and a steep-slope slide ≥75% (the steeper the face, the *freer* the
/// vertical), so only opposing contacts hold an arc under this — and because the intent keeps
/// growing while the funnel eats the motion, the pinch-in registers the frame it starts
/// instead of after a visible millimeter-creep tail in the falling pose (0211's absolute
/// stillness test).
pub(super) const WEDGE_STALL_RATIO: f32 = 0.15;
/// Fall speed (yd/s) the arc must exceed before stalled frames count: a jump apex hovers near 0
/// and never qualifies; a wedge accumulates gravity while frozen and passes within a few frames.
pub(super) const WEDGE_MIN_FALL: f32 = 1.0;
/// One-shot air-control nudge (yd/s): a jump from a standstill can be steered this much in the pressed
/// direction; a jump taken with momentum keeps it locked (vanilla feel). Less than a walking jump.
///
/// **VERIFIED as behaviour, but this constant is data, not a binary constant**: the
/// reference's nudge speed is `min(MOVE_WALK, MOVE_RUN)`, the walk override inside `0x7c4c90(1)`
/// (`0x7c4d19`/`0x7c4d1b`), read from the unit's live speeds. `2.5` is the *default* `MOVE_WALK`, so
/// this agrees with the reference right up until a walk aura, a Slow or a daze moves either speed.
pub(super) const AIR_NUDGE_SPEED: f32 = 2.5;
/// The FALLINGFAR **distance leg** (yd): a *jump* arc (launch vz ≠ 0) latches MOVEFLAG_FALLINGFAR
/// once it descends this far below its launch height — the fall resolver's `0x633240`, constant
/// `[0x80dff8]` = 1/9 yd. Latched, the arc is a **far fall**:
/// the anim layer swaps to Fall(40) mid-air. A flat jump never descends below its takeoff, so it
/// never latches — its hang stays Jump(38). The legs are exclusive on the launch vz: step-off
/// falls take [`FALL_FAR_TIME`] instead.
pub(super) const FALL_FAR_DROP: f32 = 0.111_11;
/// The FALLINGFAR **timer leg** (s): a *step-off fall* (launch vz = 0 — the walk election's
/// `StartFalling(0)`) latches once airborne this long — `0x633240`'s accumulator test,
/// `0x1f4` = 500 ms. Free-falling from rest that is ≈ 2.41 yd of descent. Since the election
/// absorbs anything up to ~collision height as a step, elected step-off falls
/// start just under this: a wagon-height drop crosses it a frame or two before the floor.
pub(super) const FALL_FAR_TIME: f32 = 0.5;
/// Skin width (yd) kept between the capsule and geometry on casts.
pub(super) const SKIN_WIDTH: f32 = 0.02;

/// Seconds of **stalled** streaming before the post-snap hold gives up (see [`Player::settling`]).
/// A *stall* budget, not a load budget: the deadline is pushed forward while the resident world is
/// still the departed map's (0710's fail-closed law) **and while the destination is visibly still
/// arriving** (any load-progress counter moved). It can therefore only fire
/// against a stream that has made no progress at all for this long — missing data, dead IO — never
/// against a slow machine. As a fixed load budget it was measured 0.01 s from firing on a *fast*
/// machine (a Stormwind arrival consumed 5.99 s of the 6.00 s), which is B263: on any slower
/// machine it released gravity mid-stream and the body fell through the not-yet-collided city.
pub(crate) const SETTLE_TIMEOUT: f32 = 6.0;

/// The player body's collision capsule, built once at startup and swept by avian's `MoveAndSlide`
/// each frame. Its origin is the capsule centre; the player's `pos` is its feet (centre −
/// half-height·Y).
///
/// **Every player body, not only ours.** A remote mover's dead-reckon sweeps this same capsule
/// against the same colliders ([`super::mover::grounded_step`]) — the reference drives non-local
/// units through the *same* movement controller as the local player (decision 0059's byte trail:
/// `0x616620` integrates any mover; the local-player GUID compare gates only a timing budget). The
/// reference reads each unit's own collision height (`[unit+0xb8]`); we use the one player capsule
/// for every player body, which the 1.12 races are close enough to share.
#[derive(Resource)]
pub(crate) struct PlayerCapsule(pub(crate) Collider);

/// The avatar run-speed fallback + dev override. `value` is `$WOW_MOVE_SPEED` when set
/// (`env_override` — the absolute dev knob, backpedal scaled by [`RUN_BACK_RATIO`] under it), else
/// the vanilla 7.0 used only until the server's own speeds stream in: the self create's `LIVING`
/// block seeds [`crate::net::UnitSpeeds`], and every `SMSG_FORCE_*_SPEED_CHANGE` updates it live —
/// the controller reads those as the authoritative run/runback/swim speeds.
#[derive(Resource)]
pub(super) struct MoveSpeed {
    pub(super) value: f32,
    pub(super) env_override: bool,
}

/// **The granted mover modes — one system, five bits**.
///
/// A *mode* is a `MOVEMENTFLAGS` bit the **server grants** that changes how our mover behaves rather
/// than where it is heading. They live here as typed fields, not as bits in
/// [`Player::move_flags`], for the reason decision 0726 first hit: `move_flags` is last-streamed
/// wire bookkeeping, rebuilt from state every frame, so a mode parked there alone would be gone
/// before the mover ever read it. The flag word is *rebuilt from these fields* each frame instead —
/// which is also what keeps the server's copy of our mode alive (drop the bit from our stream and
/// the next server-authored move echoes a mode-less word back and clears it under us).
///
/// **Two arrival routes, and the difference matters.** Four of the five come through the *ack'd*
/// family ([`benilla_protocol::MoveMode`]) — an addressed opcode we must answer with the echoed
/// counter or the grant never lands. [`Self::levitating`] does not: it arrives merged out of a
/// server-authored `MSG_MOVE_*` (the `SERVER_AUTHORED` mask), unhandshaked.
///
/// **Each one's effect is the reference's, byte-verified** — the addresses are in
/// [`benilla_protocol::MoveMode`]'s table; here is where each is consumed:
///
/// - [`rooted`](Self::rooted) — translation and jumps die; **turning stays live** ([`super::control`]).
/// - [`feather_fall`](Self::feather_fall) — terminal fall speed becomes [`FEATHER_TERMINAL_VELOCITY`]
///   ([`super::mover::step`]).
/// - [`hover`](Self::hover) — ground contact rises by [`HOVER_HEIGHT`] ([`super::mover::step`]).
/// - [`water_walking`](Self::water_walking) — the liquid surface becomes ordinary ground, in the
///   classify and in the resolve alike ([`super::mover::step`]).
/// - [`levitating`](Self::levitating) — the swim/depth decision is suppressed entirely
///   ([`super::swim::update_swimming`]).
///
/// **Levitate (spell 1706) grants three at once** — feather fall + hover + water walk — which is why
/// they are one struct rather than four unrelated booleans.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct MoveModes {
    /// The server rooted our mover (`SMSG_FORCE_MOVE_ROOT` — death until release, and every root or
    /// stun). Translation input and jumps are dead — the faithful "can't move between
    /// death and release" — but **turning stays live, and by design rather than by omission**
    /// (corrected by decision 0872): the reference's input tick consults an allow-list
    /// (`0x615c71` → the byte table at `0x618054`) that blocks the translation command ids and
    /// **explicitly permits** turn, pitch, run/walk and SetFacing. `SetRoot 0x7c7340` additionally
    /// clears the direction bits once, at apply. A character who cannot even pivot is *stunned* —
    /// a separate `UNIT_FIELD_FLAGS` gate ([`crate::player::UNIT_FLAG_STUNNED`]).
    pub(crate) rooted: bool,
    /// Water-walking (`SMSG_MOVE_WATER_WALK`, `SPELL_AURA_WATER_WALK` — Water Walking, Levitate, and
    /// the ghost form): the liquid surface counts as ordinary walkable ground, so we stand on it
    /// instead of sinking ([`super::mover::step`] — the classify and the clamp).
    ///
    /// **It does NOT stop the swim latch arming**, and the claim that it did stood here until
    /// decision 1611 read the bytes: `0x6030c0`'s only mode test is `test ah,4` — LEVITATING
    /// (`0x400`) — and it never looks at `0x10000000` at all. The exclusion runs the other way and
    /// lives one layer down, in the trace-mask arm at `0x631617`: *swimming* turns water-walking
    /// off, not the reverse. So a swimmer who gains *this* bit keeps swimming, by construction —
    /// but a swimmer who casts **Levitate** does not, because Levitate also grants
    /// [`hover`](Self::hover), whose handler jumps the body out first.
    pub(crate) water_walking: bool,
    /// Feather fall (`SMSG_MOVE_FEATHER_FALL`, `SPELL_AURA_FEATHER_FALL` — Slow Fall, Levitate): the
    /// fall integrator's terminal velocity drops to [`FEATHER_TERMINAL_VELOCITY`]. Nothing else
    /// changes — the arc, the flags and the landing report are the ordinary ones.
    pub(crate) feather_fall: bool,
    /// Hover (`SMSG_MOVE_SET_HOVER`, `SPELL_AURA_HOVER` — Levitate): ground contact sits
    /// [`HOVER_HEIGHT`] above the surface, so the body floats and walks along a yard up. It also
    /// refuses the jump outright — the first test in `CMovement::Jump 0x7c6230`, live for the
    /// keyboard press ([`super::mover::step`], and the swim breach in [`super::control`]).
    ///
    /// **And the grant itself jumps you** — the one granted mode that moves the body, and the whole
    /// of why Levitate looks like a launch ([`Player::hover_launch`]).
    ///
    /// **Composed with [`water_walking`](Self::water_walking), those two gates leave a swimmer
    /// with no way to the surface** (B322's first symptom): water-walking is off
    /// while SWIMMING, and SWIMMING only clears on the depth compare or on a breach that HOVER
    /// refuses. Both gates are individually VERIFIED; the *composition* is inferred, and it is the
    /// one part of the Levitate lane still open.
    pub(crate) hover: bool,
    /// **Free flight** (`MOVEFLAG_LEVITATING`, GM `.cheat fly`). The one mode that
    /// arrives *unhandshaked*, merged out of a server-authored move ([`super::wire_in`]).
    ///
    /// It does exactly one thing, and it does it by suppression: while set, the water/depth swim
    /// decision does not run at all ([`super::swim::update_swimming`] — the reference's
    /// `0x6030d2 test ah,4` bail). So a [`Player::swimming`] the server switched on stays on with no
    /// water under it — which *is* GM flight — and, symmetrically, real water can no longer put us
    /// into swim while it is set.
    pub(crate) levitating: bool,
}

impl MoveModes {
    /// Grant or revoke one ack'd mode. [`Self::levitating`] is deliberately unreachable here — it
    /// has no opcode of its own and arrives through the server-authored flag merge instead.
    pub(crate) fn set(&mut self, mode: MoveMode, apply: bool) {
        match mode {
            MoveMode::Root => self.rooted = apply,
            MoveMode::WaterWalk => self.water_walking = apply,
            MoveMode::FeatherFall => self.feather_fall = apply,
            MoveMode::Hover => self.hover = apply,
        }
    }

    /// The granted modes as `MOVEMENTFLAGS` bits — **what every outbound packet must carry**.
    ///
    /// This is the half that is easy to skip and expensive to skip: the reference has one
    /// `[cmov+0x40]` that is both the live state and the wire word, so a real client echoes back
    /// whatever the server granted for free. Ours rebuilds the word from state each frame, so
    /// dropping a mode here makes the server's copy forget it — and the next server-authored move
    /// echoes a mode-less word back, our merge clears the mode, and the aura silently stops working
    /// (decision 0726 hit exactly this with GM flight).
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

    /// Lift the granted modes **out of a server-authored flag word** into typed state (decision
    /// 0726's step, now the whole family's). All five bits sit inside the reference's
    /// `SERVER_AUTHORED` merge mask (`0x75a0_7dff`), so a bare `MSG_MOVE_*` the server writes for
    /// our mover can grant or revoke them with no handshake — `.cheat fly` is the case that proved
    /// it, and it is why LEVITATING has no opcode of its own.
    ///
    /// **Root is a deliberate divergence, and not because the mask excludes it** — bit 12 is inside
    /// the mask, so the reference *does* merge root from a server-authored pose. We don't: in this
    /// client the ack'd opcode is root's only grant path, and letting a bare pose clear it would put
    /// a locally-unrooted body against a server that still has us rooted — which streams moving bits
    /// and trips `CHEAT_TYPE_ROOT_MOVE`. The reference gets away with it because vmangos re-adds
    /// `MOVEFLAG_ROOT` to anything it writes for a rooted mover (`MovementHandler.cpp:1064`), so its
    /// merge is a no-op in practice. Revisit if a case ever needs the wire to unroot.
    pub(crate) fn merge_from_wire(&mut self, wire: u32) {
        use crate::creature_anim::move_flags as f;
        self.water_walking = wire & f::WATER_WALKING != 0;
        self.feather_fall = wire & f::SAFE_FALL != 0;
        self.hover = wire & f::HOVER != 0;
        self.levitating = wire & f::LEVITATING != 0;
    }
}

/// **The precondition both movement-input predicates evaluate first** — the reference's
/// `0x5144e0`, called at the head of `0x514560` (at `0x514568`) and of `0x5145b0` (at `0x5145b8`).
/// It answers *"is there a mover, and is it in a state where input means anything at all"*.
/// Its five conjuncts are:
///
/// 1. the mover object resolves (`0x5144e7`);
/// 2. **`[[mover+0x110]+0x40] > 0` — UNIT_FIELD_HEALTH**, signed i32, strictly greater, the `jg` at
///    `0x5144fd`. (The field identity is established three independent ways — the
///    descriptor-base arithmetic, the `UnitHealth` registrar pair `.data 0x850510 → 0x5174d0`, and
///    the low-health warning's `fild [eax+0x40]; fidiv [eax+0x58]` at `0x5ecafd`.) — the
///    [`MoverInput::dead`] field;
/// 3. the CMovement-aux gate `[[mover+0x118]+0xa4]` null-or-`&4` (`0x514516`);
/// 4. `!0x60f5b0(obj)` — the **KNOCKDOWN animation lockout**, and *not* an on-taxi predicate: it
///    returns `AnimationData.dbc` column 3 bit `0x80`, which the shipped table sets on exactly one
///    of 208 rows, id 121 `Knockdown`. It contributes nothing while dead;
/// 5. `!(IsActivePlayer(mover) && [mover+0x1c70] & 1)` — the **far-sight ENGAGED latch**, the
///    [`MoverInput::view_is_out`] field.
///
/// **Conjuncts 1, 3 and 4 have no field here.** Conjunct 1 is structural:
/// [`super::controller::control`] returns before the axes on `control_lost` and on `reseat`, the
/// frames where the mover would not resolve. Conjunct 3 has no benilla analog yet. Conjunct 4 is
/// named, verified and unbuilt — its own behaviour with its own retest — and this struct is where
/// it lands when it is built.
///
/// Carried as a **value** rather than as loose booleans threaded through each predicate because
/// that is the reference's own shape (one precondition, evaluated once for the mover, handed to
/// both), and because two same-typed terms passed positionally would transpose silently.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MoverInput {
    /// **Conjunct 2, the health term** — health `== 0` off the **mover's** descriptor
    /// ([`benilla_protocol::messages::update_object::ObjectFields::unit_is_dead`]). The mover's and
    /// not ours, because `0x5144e0` reads `[esi+0x110]` where `esi` is whatever we are driving
    /// (decision 1277's possessed creature).
    ///
    /// **This is the term the struct exists for.** benilla modelled the server's root on death
    /// and nothing else, and a root deliberately leaves turning live, so a corpse on
    /// the ground could still be spun with the turn keys or a right-drag.
    ///
    /// **A ghost is not dead by this test:** the server puts a released player's health at 1
    /// (0308 §the release), so `0x5144fd`'s `jg` is taken and the ghost gets every input back.
    /// Nothing in the input path, the emitters or the send gates reads the ghost bit at all — a
    /// band census finds **zero** `PLAYER`-block reads across the whole InputControl and CMovement
    /// ranges (`0x513000`–`0x516000`, `0x7c4000`–`0x7c8000`). The ghost bit *is* read in the
    /// collision layer, where it adds trace mask bit `0x8000` (`0x631658`) — a separate finding,
    /// and not this one.
    pub(crate) dead: bool,
    /// **Conjunct 5, the far-sight term** — `!(IsActivePlayer(mover) && [mover+0x1c70] & 1)`, held
    /// here in the affirmative: *my view is out on a far-sight object, and the body I would be
    /// driving is my own*. While that holds the reference refuses **both** predicates — you may
    /// neither walk nor turn your own body while you are looking through Mind Vision.
    ///
    /// **Both halves are load-bearing, and `IsActivePlayer` is the one that would be easy to
    /// drop.** `0x514537 call 0x5fa6d0` tests the *mover* against the active player and returns the
    /// conjunct satisfied the moment they differ (`0x51453e je 0x514550`), so the latch is only
    /// ever read for our own body. Mind Control rides the same `PLAYER_FARSIGHT` field the camera
    /// does ([`super::view_subject`]), so during a possession the latch is engaged — and without
    /// this half the victim would freeze solid, which is the one body the whole spell exists to
    /// walk around. It is also why the latch is never read off a creature's descriptor, where
    /// `+0x1c70` is not a field at all.
    ///
    /// **Engaged means the subject RESOLVED, not merely that the field arrived.** The far-sight
    /// machine `0x5ee290` sets the latch only on its ENGAGE leg (`0x5ee3f6 or ecx,1`), reached
    /// after `0x5ee2ef` resolves the guid; the *field non-zero but object unresolved* leg falls
    /// through to `0x5ee32d` — camera home, pending cast cancelled, `return 0` — and sets nothing.
    /// So the benilla analog
    /// is the **resolved** [`super::view_subject::ViewSubject`] subject, not the raw
    /// `PLAYER_FARSIGHT` field: through the ~400 ms vmangos defers the set by, the body is still
    /// ours to drive.
    pub(crate) view_is_out: bool,
}

/// **Conjunct 5's two halves, composed** — `IsActivePlayer(mover) && [mover+0x1c70] & 1`, the
/// value [`MoverInput::view_is_out`] carries and the field's doc explains in full.
///
/// A named function and not an inline `&&` at the one call site, because the half that is easy to
/// lose is `driving_own_body`: drop it and Mind Control freezes its victim, which is a bug nothing
/// about far sight would lead you to look for.
///
/// - `driving_own_body` — the mover is our own character, i.e. no possession is in force
///   (`0x514537 call 0x5fa6d0`, the active-player test on the *mover*).
/// - `far_sight_engaged` — our view is out on a far-sight object we have actually **resolved**
///   (`[mover+0x1c70] & 1`, which the machine `0x5ee290` sets only on its post-resolve ENGAGE leg).
pub(crate) fn view_is_out(driving_own_body: bool, far_sight_engaged: bool) -> bool {
    driving_own_body && far_sight_engaged
}

impl MoverInput {
    /// `0x5144e0` itself — every conjunct we model, and the answer both predicates start from.
    fn ready(self) -> bool {
        !self.dead && !self.view_is_out
    }

    /// **`0x514560` — "may this unit translate?"**, the ROOT predicate. Consumed by the input tick
    /// at `0x5146c1` (`test bl,bl`), the sole gate on the forward/back emitter `0x514da0` and the
    /// strafe emitter `0x514e80`. Its own terms past the shared precondition are `MOVEMENTFLAGS &
    /// 0x1200` (`0x51458c test dh,0x12` — our `rooted`) and stand state `!= 7` (`0x514591`), which
    /// vmangos never writes and which is therefore inert here.
    ///
    /// Note the health term is tested **twice** — once inside `0x5144e0` and again at `0x51457c` on
    /// the predicate's own path. Death is not a corner of this gate; it is its first question.
    pub(crate) fn may_translate(self, rooted: bool) -> bool {
        self.ready() && !rooted
    }

    /// **`0x5145b0` — "is this unit not stunned?"**, the STUN predicate. Consumed at `0x514755`
    /// (`je 0x51479c`), which skips the turn emitter `0x514f50` and the pitch emitter `0x515010`
    /// outright *and* force-stops either already in flight. Each emitter has exactly one caller,
    /// immediately behind that gate, and no data or vtable reference anywhere in the image — so
    /// while this predicate is false **no keyboard or mouse turn can reach `CMovement::StartTurn
    /// 0x7c6d90` by any route**.
    ///
    /// Its own term past the shared precondition is `UNIT_FIELD_FLAGS & 0x40000` — a descriptor
    /// read, not an aura and not a movement flag. **And because the precondition is
    /// shared, a dead body is "stunned" as far as this predicate is concerned**: that single fact is
    /// why the reference refuses to turn a corpse.
    ///
    /// **The MOUSE turn reaches the same answer down a different path** (this corrects
    /// what 1753 first wrote). The right-drag handler is `0x514400`, called from the mouse-MOVE
    /// handler `0x492c00`; its body hand-off `0x51447b call 0x5103e0` is gated at `0x514474` by a
    /// *third* predicate, **`0x5145e0`**, whose first act is to call this one — so the health term
    /// arrives through `0x5145b0` → `0x5144e0` and short-circuits before `0x5145e0` reaches its own
    /// remaining conjuncts (`[input+4] & 1`, and `GetStandState() == 0` via the CGPlayer vtable slot
    /// `0x5ed570`). The refusal is then **triple-redundant**: the two downstream commit gates
    /// `0x5151b0` (yaw, at `0x5151e6`) and `0x515250` (pitch, at `0x515283`) each carry their own
    /// health test. A closed census of all nine call sites of the body-facing setters
    /// `0x60de30`/`0x60de70` — neither of which has any dword reference image-wide — finds every one
    /// health-gated.
    ///
    /// So one term in one place really does fix the keys and the mouse together, but not because
    /// they share `0x514755`: they share `0x5144e0`.
    pub(crate) fn may_turn(self, stunned: bool) -> bool {
        self.ready() && !stunned
    }

    /// **`0x5145e0` — "may the MOUSE hand the camera's facing to the body?"**. The
    /// third predicate, consumed at `0x514474` (`je 0x514480`) in the mouse-MOVE handler `0x514400`
    /// to gate the camera→body hand-off `0x51447b call 0x5103e0`, and again at `0x51495a` for the
    /// both-button site. Its conjuncts, in the binary's order: the stun predicate above
    /// (`0x5145eb call 0x5145b0`, which carries the shared precondition), the mouse-look input bit
    /// (`0x5145fa test byte [esi+4],1` — the session's own right-button condition, which is the
    /// caller's here), and **`GetStandState() == 0`** (`0x51460c call [eax+0xa4]` → the CGPlayer
    /// override `0x5ed570`, the client-predicted cache `[player+0x1d68]`; `0x514612 neg; sbb; inc`).
    /// The commit is refused a second, independent time downstream: `0x5151b0` re-tests the raw
    /// descriptor byte at `0x51520a`.
    ///
    /// So a right-drag while **seated** — sitting, in a chair (2, and the server-driven 4/5/6),
    /// asleep — turns the camera and leaves the body exactly where it is, and puts nothing on the
    /// wire. It does not stand the body up either: the turn emitter `0x514f50` skips its stand arm
    /// while the RMB bit is held (`0x514f6d`), and `MSG_MOVE_SET_FACING` is exempt from the
    /// emitter's stand table. Standing up is the keyboard's (`turned`), the X key's, or a flick's
    /// (1766); the moment the predicted stand state reads 0 again, the next motion sample hands
    /// the camera's yaw to the body like any other right-drag.
    ///
    /// `stand_state` is the **predicted** value — our `stand_pending` overlaid on the descriptor
    /// byte — because that is what `0x5ed570` returns for the active player.
    pub(crate) fn mouse_may_turn_body(self, stunned: bool, stand_state: u8) -> bool {
        self.may_turn(stunned) && stand_state == 0
    }

    /// **The input tick's teardown leg** — `0x5146d6 call 0x60fb60(0, 1)`, which cancels
    /// click-to-move and `/follow` and fires `AUTOFOLLOW_END` (event `0x170`).
    ///
    /// It is reached only when **BOTH** predicates are down: `0x5146c3 jne` not taken (`bl == 0`)
    /// *and* `0x5146ce jne` not taken (`[ebp+0xf] == 0`). The health test alone is necessary, but
    /// not enough to name the leg. **A pure ROOT does not cancel a follow** (translate down, turn
    /// still up), and neither does a pure stun; **death does**, because it takes both. Ice Block,
    /// which grants root and stun together, does too. So does the far-sight term, for the same
    /// reason death does — it is the other conjunct of the shared precondition.
    ///
    /// benilla had the root alone in this position and so cancelled on a Frost Nova, which the
    /// reference does not.
    pub(crate) fn torn_down(self, rooted: bool, stunned: bool) -> bool {
        !self.may_translate(rooted) && !self.may_turn(stunned)
    }
}

/// **The two incapacitate suppressions, applied to a freshly built move-flag word**
/// — the last step of [`super::control`]'s per-frame rebuild, before the word drives the animation
/// and goes on the wire.
///
/// The reference never needs this: its `[cmov+0x40]` is written *only* by emitters that already
/// refuse, so the bits are absent by construction. Ours rebuilds the word from raw input every
/// frame, so a suppression not re-applied here streams motion the body is not performing (decision
/// 0056's law) and feeds the same lie to the anim layer.
///
/// - **Rooted → no direction bits.** The input tick's allow-list (`0x615c71` → the byte table at
///   `0x618054`) DROPS command ids 0–5 — StartMove/StopMove/StartStrafe — while `0x1000` is set, so
///   a key pressed under a root never reaches `0x7c6ae0`/`0x7c6c50` and never sets a bit; `SetRoot
///   0x7c7340`'s own `and 0xffe07f00` wipes whatever was set, at apply. With the low nibble clear
///   the movement-anim resolver `0x5fd100` takes its not-moving exit at `5fd10c test al,0xf` before
///   it can reach any locomotion id — **that** is why a rooted character shows no walk/run
///   animation, and why ours was still showing one: it kept building FORWARD out of the raw axis
///   while the mover ignored it.
/// - **Stunned → no turn bits.** `0x514755` skips the turn emitter `0x514f50` outright and
///   force-stops one already in flight, so `0x7c6d90 StartTurn` never runs. Decision 0872 stopped
///   the *rotation* (`control` restores the aim) but left the bits streaming, which still told every
///   observer to play the turn-in-place shuffle.
///
/// They are keyed on different state on purpose, exactly as the reference has them: a pure root
/// (Frost Nova, Entangling Roots) still pivots — turn is on the allow-list *deliberately* — and only
/// `UNIT_FLAG_STUNNED` takes the pivot away. Nothing else
/// is touched: the granted modes ride on (drop one and the server forgets it), SWIMMING survives a
/// root exactly as `0xffe07f00` preserves `0x200000`, and FALLING is already gone by the time we get
/// here — the root ended the arc ([`super::mover`]'s anchor).
///
/// The two arguments are therefore not "rooted" and "stunned" but **the two predicates being down**
/// ([`may_translate`], [`may_turn`]) — a wider set by exactly one member: **death drops both**,
/// through the precondition they share ([`MoverInput`]). A corpse streams
/// neither a direction bit nor a turn bit.
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

/// Our controllable avatar. Until `active`, the camera free-flies; once the server reports our
/// position we take control (third-person) and drive movement. Toggle free-fly with the dev
/// chord + `F`.
/// `active`/`pos`/`detached` are `pub(crate)` so terrain streaming can center the loaded block on the
/// avatar in third-person and on the free-flying camera while detached.
/// `PartialEq` is here for one assertion, and it is the assertion that keeps this type honest:
/// the session boundary returns the whole resource to `Player::default()`
/// ([`super::wire_in::release_on_session_end`]), and its test says exactly that
/// rather than re-listing the fields a hand-picked reset would have to remember.
#[derive(Resource, Default, PartialEq)]
pub(crate) struct Player {
    pub(crate) active: bool,
    /// **The granted mover modes** — what the server has switched on for our mover, as typed state
    /// rather than raw bits. See [`MoveModes`].
    pub(super) modes: MoveModes,
    /// **Autorun** latched on — the reference's input bit `0x1000` in the local mover's input word
    /// `[MOVE+4]`, flipped by `ToggleAutoRun 0x513de0` (a read+invert: the command family's only
    /// *toggle*, where every directional command is a set/clear pair).
    ///
    /// It is not a movement of its own. The axis emitter `0x514da0` folds the bit into the
    /// **forward axis** (`test ah,0x10`), so autorun *is* held-forward: it nets against a held S
    /// and diagonals with a strafe, exactly like the both-button run. Ours folds in at the same
    /// places the other forward sources do — the direction vector, the wire flags, the swim
    /// amounts, and the turn-rate's "am I translating" test.
    pub(super) autorun: bool,
    /// **Walk mode latched on** — the reference's `MOVEMENTFLAGS` bit `0x100`
    /// (`CMovement+0x40`), flipped by `ToggleRun 0x513d50` → `0x60e080`, which reads the CURRENT
    /// bit out of the cached word (`0x60e083 mov eax,[ecx+0x9e8]; 0x60e08c and eax,0x100`) and
    /// hands it down to be inverted. The other true *toggle* in the movement family, beside
    /// [`autorun`](Self::autorun) — and unlike it, this one is a real wire bit rather than an
    /// input-word bit, so it rides every outbound packet ([`super::flags`]) and observers read our
    /// gait straight off it.
    ///
    /// **Typed state, not the flag word.** [`Player::move_flags`] is last-streamed wire
    /// bookkeeping, rebuilt from state every frame, so a latch parked only there is gone by the
    /// next frame — the same trap [`MoveModes`] exists to avoid. It is *also* inside the
    /// `SERVER_AUTHORED` merge mask, so a server-authored move for our mover can flip it; that
    /// merge lands here, in [`super::wire_in`], not in the word.
    ///
    /// It is **not** a [`MoveModes`] entry: that family is the four ack'd server grants, each with
    /// its own SMSG/ack pair. This one owes no ack, is granted by nobody, and is ours to set.
    pub(super) walking: bool,
    /// **`/follow` is holding the forward key this frame**. Not a mode of its own:
    /// the reference's follow owns no translation and simply pushes the same move-forward bit the W
    /// key does (`0x60e790`), so this folds into [`forward_axis`] beside `autorun` and the
    /// both-button run rather than driving the mover itself. Rewritten every frame by
    /// [`super::follow::steer_follow`], which runs just before the controller reads it.
    pub(super) follow_forward: bool,
    /// Free-fly (`F`): the camera moves on its own and the avatar/server position is frozen.
    pub(crate) detached: bool,
    /// **The body in our hands may not be moved** — `SMSG_CLIENT_CONTROL_UPDATE` with
    /// `allowMove = 0` about whatever we are currently driving.
    ///
    /// Two cases, one meaning. The packet names **us** while somebody is
    /// mind-controlling us; it names **the creature we are possessing** when that creature
    /// becomes feared or confused, which vmangos sends from the fear/flee movement generators
    /// without ending the possession at all. Both say the same thing — the reins are still where
    /// they were, and nothing may move — and the reference collapses them the same way, by zeroing
    /// the mover global for whichever unit the packet names when it is the one already being
    /// driven.
    ///
    /// So this is **not** "someone is controlling me"; reading it that way is what made the second
    /// case clear [`Player::foreign_mover`] and walk our own body around under a creature's mover.
    ///
    /// This is the *whole* of what stops a mind-controlled player walking away, and that is a
    /// verified property of the server rather than an assumption: vmangos never roots the victim,
    /// sends it no speed change, its `StopMoving()` is a documented no-op for a possessed player,
    /// and `HandleMovementOpcodes` carries no charm check — so it will accept and apply any
    /// movement we choose to send. Nothing else is coming; if the client does not stop itself,
    /// nothing stops.
    ///
    /// Deliberately **not** a [`MoveModes`] entry. That family is the four ack'd server modes,
    /// each with its own SMSG/ack pair and its own movement-flag bit; this owes no
    /// ack, carries no flag, and suppresses turning too — which root explicitly does not.
    pub(crate) control_lost: bool,
    /// **We hold somebody else's reins** — a unit the server handed us and we claimed as our mover
    /// (mind-controlling a creature, Eye of Kilrogg). `None` whenever the mover is our own body.
    ///
    /// While this holds, the controller must not drive our own body onto the wire, and the reason
    /// is sharper than tidiness: outbound `MSG_MOVE_*` carry **no guid** — the server attributes
    /// them to whatever we last claimed — so streaming our body's pose under a claimed creature's
    /// mover would teleport that creature onto us. Suppressing our body is also simply what
    /// possession looks like: your own character stands still while you drive the other thing.
    ///
    /// While this holds, [`crate::net::Embodied`] sits on that unit's entity — or on nothing at
    /// all until it streams — and the controller drives *it*. Our own body then simulates nothing,
    /// animates from nothing, and sends nothing, which is both the fix and what possession looks
    /// like from the outside.
    pub(crate) foreign_mover: Option<u64>,
    /// **The reins are between hands** — the body we drive has changed, and we drive *nothing*
    /// until we have adopted its pose.
    ///
    /// Set when the mover guid changes (the control handshake in [`super::wire_in`], and
    /// [`super::embody::maintain_embodiment`] when the marker follows it); cleared at the same
    /// take-control edge that seizes our own body on login, once a streamed pose is actually there
    /// to seize. Both halves matter:
    ///
    /// - **Adopt before driving** — the resource's `pos`/facing still describe the body we were
    ///   driving a moment ago, so the first streamed frame would otherwise report the creature
    ///   standing wherever we left our character.
    /// - **Drive nothing while pending** — the handshake runs *inside* the controller, a frame
    ///   ahead of the marker that follows it, and the claimed unit may not have streamed in at all.
    ///   Either way there is a window where what we intend to drive and what carries the marker
    ///   disagree, and driving during it means writing one body's pose onto another.
    ///
    /// A flag rather than a direct write because the pose lives on the entity and the seize needs
    /// the camera too — both of which the controller already holds and the marker's owner does not.
    pub(crate) reseat: bool,
    /// Feet position in **Bevy** coords (converted to raw WoW only when sending to the server).
    pub(crate) pos: Vec3,
    /// Vertical velocity (yd/s, Bevy +Y up) for gravity/jump/fall. Integrated each frame; zeroed while
    /// grounded. Fed into avian's `MoveAndSlide`.
    pub(super) vel_y: f32,
    /// Current horizontal velocity (yd/s). Live (from input) while grounded; while airborne it's the
    /// take-off momentum (a moving jump keeps its trajectory — the WoW feel), except a jump from a
    /// standstill gets one [`AIR_NUDGE_SPEED`] steer in the pressed direction. Zero when standing still.
    pub(super) horiz_vel: Vec3,
    /// The CMovement `moveFlags` we last streamed to the server (directional + turn bits, see
    /// [`crate::net::move_flags`]). Diffed against this frame's flags to emit a `MSG_MOVE_*` per
    /// movement-axis transition — the way the real client announces its movement.
    pub(super) move_flags: u32,
    /// The facing (WoW orientation) as of **last frame** — the reference's facing-change detector
    /// (`0x617170`, exact equality against the unit's live facing cell). Any change off the turn axis
    /// streams a `MSG_MOVE_SET_FACING` that frame, moving or standing, so observers see us aim.
    /// Updated every frame whether or not a packet went out.
    pub(super) last_facing: f32,
    /// The **position we last told the server** (WoW coords, exactly the floats that went on the
    /// wire). Diffed against this frame's live position to catch a drift we would otherwise never
    /// report: our resolver settles a resting body a fraction of a millimetre after the packet that
    /// reported the rest (a landing, a login, a teleport onto a server-authored pose), and while
    /// standing still nothing else goes out. vmangos compares an incoming position to its stored one
    /// with EXACT float equality and interrupts a movement-interrupt cast on any difference, so a
    /// stale copy turns the next packet — in practice the first `MSG_MOVE_SET_FACING` of a
    /// right-drag — into a mid-cast "you moved". Decision 0907; the reconcile lives in
    /// [`super::movement_net::stream_self_movement`].
    pub(super) last_pos: [f32; 3],
    /// The stand state we last committed locally — volunteered (`CMSG_STANDSTATECHANGE`) or the
    /// server's own (`SMSG_STANDSTATE_UPDATE`, 2339) — whose echo into our `UNIT_FIELD_BYTES_1`
    /// hasn't landed yet: the reference's predicted cache `[player+0x1d68]`, written by the
    /// setter's local half `0x6127b0`, which sends nothing itself (the volunteer path `0x5ed430`
    /// sends first, then calls it; decisions 0080c, 2339). `None` = at the echoed value.
    pub(super) stand_pending: Option<u8>,
    /// **Settling after a teleport/summon/login**: the streamed world (terrain *and* its WMO
    /// buildings + colliders) arrives over several frames, so the collision under the destination
    /// isn't there the instant we snap to it. While settling the movers hold the avatar in place
    /// with gravity **off** — otherwise it falls through the not-yet-loaded city/building floor —
    /// and the loading screen waits for the release. **Released by the terrain streamer** (decision
    /// 0737) once the destination is resident — scene spawned and collider queue quiet
    /// ([`crate::loading_screen::WorldLoadProgress`]) — or at [`SETTLE_TIMEOUT`]; **never by ground
    /// contact**, which a flyer, a swimmer, or a genuinely airborne teleport never produces (the
    /// old probe release ran only in the walk mover, and each other mover mode needed — and got —
    /// its own leak patch before 0737 deleted the class).
    pub(crate) settling: bool,
    /// `Time::elapsed_secs` deadline to give up settling and release (see [`Player::settling`]).
    /// Pushed forward by the streamer while [`Player::world_stale`] (0710's fail-closed law) and
    /// while any load-progress counter is still moving — so it measures
    /// *stall*, and a slow arrival can never exhaust it.
    pub(crate) settle_deadline: f32,
    /// `Time::elapsed_secs` when the current settle hold began (the snap) — what the `sett` trace
    /// reports the wait against. The deadline above stopped measuring this the moment it became a
    /// stall budget: it is re-pushed all through a live stream, so "deadline minus timeout" names
    /// the last push, not the arrival.
    pub(crate) settle_since: f32,
    /// **The colliders under our feet may still belong to the map we just left.** Set at every
    /// snap, cleared by the terrain streamer once the destination's own world is resident.
    ///
    /// The snap runs a whole `WorldStage` *before* the streamer swaps maps — so on the arrival
    /// frame the physics world is still entirely the old map's, and its despawn is a deferred
    /// command on top of that. Any settle judgement made in that window would be about the wrong
    /// world — and an **instance entrance sits within a yard of its own outdoor portal**
    /// (Zul'Gurub's pad is z 92.53 against the Stranglethorn ground you left at 92.30), which is
    /// how the pre-0710 ground probe declared the floor found on frame 0 and dropped the body
    /// 15 yd through the ZG city WMO. While this flag is set the streamer makes no release
    /// judgement and pushes [`Player::settle_deadline`] forward, so the timeout budget measures
    /// time waiting for the *destination's* world.
    pub(crate) world_stale: bool,
    /// The pitch the login seize seats the camera at — the saved pose's `cameraPitch` when
    /// [`super::camera_saved`] restored one for this session, else the shipped opening pitch. Set
    /// by the pose load, read once by the seize: the seize used to seat its own constant over the
    /// restored value, so the pitch half of the remembered pose never survived a login.
    pub(super) login_pitch: Option<f32>,
    /// A same-map teleport landed: the server relocated the mover, so any in-progress self
    /// server-ride (charge/taxi) is **void** — vmangos teleports at ITS flight end (its own spline
    /// finishes ~latency before ours) and its spline-done handler ignores acks while the teleport
    /// is pending, so the relocation IS the hand-back. `drive_self_ride` takes this flag first:
    /// it drops the ride + spline without mirroring the stale flight pose over the snap (the
    /// 4-yd-hover + full-6s-settle landing bug) and owes no `CMSG_MOVE_SPLINE_DONE`.
    pub(super) ride_abort: bool,
    /// **A `MSG_MOVE_WORLDPORT_ACK` is owed, payable at the settle release**.
    /// The real client sends the worldport ack as the LAST act of its blocking destination load
    /// (`0x401bc0` sends `0xDC` at `0x401cae` only after `0x66fbe0`'s load returns) — our
    /// async re-expression of "after the load" is the release. Safe to defer: vmangos has no load
    /// timeout, drops every packet we'd send meanwhile (the player is out-of-world for the whole
    /// window), and force-acks at logout. Set by the non-riding worldport snap; a riding crossing
    /// never settles and acks immediately, as before.
    pub(crate) owes_worldport_ack: bool,
    /// `Time::elapsed_secs` when we last sent a heartbeat.
    pub(super) last_heartbeat: f32,
    /// **Milliseconds of movement simulation this client advanced through without integrating** —
    /// the quantity `CMSG_MOVE_TIME_SKIPPED` reports. Accumulated while
    /// [`Self::settling`] holds the mover (the world under us has not streamed in, so no step
    /// runs), drained and sent by [`super::movement_net::stream_self_movement`] on the release
    /// edge. Fractional because it accumulates a frame `dt` at a time; the wire takes whole ms.
    pub(super) skipped_ms: f32,
    /// `Time::elapsed_secs` when the current airborne phase (jump or step-off) began, else `None` on the
    /// ground. Drives the wire `fall_time` (ms airborne) and detects the take-off / landing transitions
    /// that emit `MSG_MOVE_JUMP` / `MSG_MOVE_FALL_LAND`.
    pub(super) airborne_since: Option<f32>,
    /// At rest wedged between steep faces ([`WEDGE_STILL_FRAMES`] stalled airborne frames):
    /// treated as standing — the fall is over, walking control is live — while a close down-probe
    /// still finds support. Cleared by real ground, by jumping, or by walking off the support into
    /// open air (a fresh fall).
    pub(super) wedged: bool,
    /// Consecutive stalled airborne frames (see [`WEDGE_STALL_RATIO`]).
    pub(super) wedge_still: u8,
    /// **Supported by a certified steep contact, not a walkable floor** — the reference's
    /// `0x4000000`, whose meaning decision 1125 settled from the bytes. Two ways to earn it, one
    /// meaning: climbing the slanted skirt of a low edge already certified clearable (the foot-cone
    /// ride, 1123), or resting on the surface being followed down off a ledge (1127).
    ///
    /// Counts as standing. The frame-start ground probe looks straight down and sees only the steep
    /// face, so without this the frame reads as airborne and gravity takes the body off the surface:
    /// the climb undone into the mid-face dwelling 0209 exists to prevent, and on the way down the
    /// dive past the edge the director reported. Re-earned every frame from the certification going
    /// up and from the cone's descent bound coming down — neither can hold a body that has not just
    /// proved it is on a surface — and cleared the moment a walkable floor takes over.
    pub(super) steep_support: bool,
    /// **A `SetHover(true)` owes a jump** — the reference's hover wire handler `0x61a620` does not
    /// merely set the flag: its enable arm is `61a62e push 0; 61a630 call 0x7c6230`, i.e.
    /// `CMovement::Jump(force = 0)`, and only *then* `61a646 call 0x7c7310` sets `0x40000000`.
    /// The disable arm is the mirror — `61a637 call 0x7c61c0` (`StartFalling`). Neither of the
    /// other two granted modes does anything of the kind: `SetWaterWalk`'s handler `0x61a3d0` is
    /// four instructions around `call 0x7c7280`, and `SetFeatherFall`'s `0x61a4e0` only refreshes
    /// the fall clamp. **Hover is the one mode that moves the body**, and that is the whole of why
    /// Levitate (spell 1706, which grants all three) visibly launches you.
    ///
    /// `force == 0` is what makes it a *different* jump from the player's: it skips
    /// `0x7c623a`'s `test [ecx+0x40], 0x40000000` hover refusal — the wire path exists precisely to
    /// be able to jump a unit that is already hovering. It still
    /// takes the ROOT|FALLING refusal at `0x7c625c`, and it still reads SWIMMING at `0x7c6261` to
    /// pick the take-off seed. So this is a latch and not a direct write: the reference commits the
    /// jump inside the handler because `MOVEFLAG_FALLING` then keeps its resolver off the body,
    /// while our mover re-derives contact from probes every frame and would zero the velocity again
    /// on the next step. Consumed at the two take-off sites in [`super::control`], where the
    /// keyboard jump is consumed, so the seed is picked by the same code that picks it for Space.
    pub(super) hover_launch: bool,
    /// **The knockback waiting to be flown** — one [`PendingKnockback`], armed by
    /// [`super::wire_in`] the frame `SMSG_MOVE_KNOCK_BACK` lands and consumed by the take-off site in
    /// [`super::control`] (the same place the keyboard jump and the hover launch are consumed, so
    /// all three enter the mover through one door). `None` the rest of the time.
    pub(super) knockback: Option<PendingKnockback>,
    /// The take-off vertical speed (yd/s, WoW +Z up) snapshotted when the airborne phase began — the
    /// client's `StartFalling` argument (`+0xa0`, constant per arc) and the `zspeed` we send in the
    /// jump tail: `JUMP_SPEED` for a jump, **exactly 0** for a step-off (the walk election calls
    /// `StartFalling(0)`). Observers replay the parabola from it, and the FALLINGFAR latch splits
    /// its distance/timer legs on it; held constant while `fall_time` advances.
    pub(super) jump_zspeed: f32,
    /// **The launch vertical speed, recorded the instant a take-off is decided** — before this
    /// frame's gravity touches it. [`super::arc`] snapshots [`Self::jump_zspeed`] from *this*, not
    /// from `vel_y`: the mover now integrates gravity on the take-off frame too, so
    /// by the time the arc bookkeeping runs `vel_y` is already one step down the parabola and would
    /// seat a launch speed `g·dt` short. The reference has no such ambiguity — `+0xa0` is written
    /// once by `StartFalling` and never touched again for the arc.
    pub(super) launch_vz: f32,
    /// **The arc's direction nibble — the reference's `[CMovement+0x40] & 0xf`**.
    /// Air control opens exactly while this is CLEAR: `0x7c5a20`/`0x7c5c20` bail on
    /// `FALLING && arg == 0`, and the only two openers that pass `arg = 1` sit behind
    /// `0x7c6afc test al,0xf`. So a jump from a standstill can be steered once, a jump taken with a
    /// direction already held cannot, and a **knockback never can** — its launch plants FORWARD
    /// itself (`0x6179c0`'s `0x617a18 or edx,0x8001`), which is the real freeze mechanism and has
    /// nothing to do with the arc's speed. Seeded at take-off, set by the one nudge that fires,
    /// cleared when the arc ends.
    ///
    /// This replaced a `horiz_vel.length_squared() < 0.01` proxy that agreed with the reference for
    /// every ordinary jump and disagreed for one input: a knockback with `xy_speed ≈ 0`, which the
    /// proxy read as "standing still, may steer".
    pub(super) arc_dirs_set: bool,
    /// This airborne arc was launched by a **knockback**, so its planted FORWARD bit rides the wire
    /// for the arc's whole length — the reference's `0x617a18` sets FORWARD and
    /// clears BACKWARD, and the send mask `0x618909 and edx,0x75a07dff` keeps bit 0. Distinct from
    /// [`super::mover::Outcome::knocked`], which is true on the launch frame only.
    pub(super) knock_arc: bool,
    /// **Was the body airborne at the end of the previous mover step** — the mover's own record,
    /// so the arc-start seeding does not have to read [`Self::airborne_since`].
    /// That field belongs to the *wire* lifecycle in [`super::arc`] and is written by
    /// [`super::flags`], which runs AFTER the mover: reading it here made a physics decision
    /// depend on a system further down the frame, so a step-off's launch state was seeded a frame
    /// late whenever the mover ran on its own. Separate owners, separate fields.
    pub(super) airborne_prev: bool,
    /// The translation-direction move-flag bits ([`crate::creature_anim::move_flags::ANY_MOVE`]) the
    /// current airborne arc launched with. Mid-air these are the *actual* motion — momentum is frozen at
    /// takeoff, so held keys move nothing — and the live flags (animation, pose, wire) read them instead
    /// of the keys (the flags mirror the avatar's motion, never raw key state). Re-seeded
    /// by the standstill-jump air nudge, the one input that really moves us mid-air. Stale while grounded.
    pub(super) airborne_dirs: u32,
    /// Launch height (Bevy Y) snapshotted when the airborne arc began — the client's StartFalling
    /// `+0x7c = +0x18` Z snapshot; the FALLINGFAR distance leg measures descent below it.
    pub(super) fall_start_y: f32,
    /// MOVEFLAG_FALLINGFAR latched for this arc: a jump descended [`FALL_FAR_DROP`] below its
    /// launch, or a step-off fall lasted [`FALL_FAR_TIME`] (the legs are exclusive on the launch
    /// vz). Latched once per arc (only landing clears it, like the client's
    /// StopFalling); sets [`crate::creature_anim::move_flags::FALLING_FAR`] on the live flags — the
    /// mid-air Fall(40) pose, the landing-anim gate, and the wire.
    pub(super) fall_far: bool,
    /// The character's facing (Bevy yaw, radians). Right-drag and movement keep this in sync with the
    /// camera; left-drag (camera-only orbit) leaves it alone, so it can diverge from the camera yaw —
    /// and that offset now persists (no auto-follow back behind while moving). Sent to the server as
    /// orientation, and the basis WASD move in. This is the *aim*/facing — distinct from the rendered
    /// body heading.
    pub(super) face_yaw: f32,
    /// In **swim mode** ([`super::swim`]): the water over the feet crossed the swim-enter depth, so
    /// the avatar floats and swims in 3D instead of walking, sets `MOVEFLAG_SWIMMING` (lighting the
    /// swim gait and streaming it), and pitches its body to the swim heading. Hysteresis-latched
    /// (see [`super::swim::update_swimming`]) so wading the boundary doesn't flicker.
    pub(crate) swimming: bool,
    /// **Our own collision height** — the avatar's copy of [`crate::entities::CollisionHeight`],
    /// mirrored onto this resource because the swim arm runs off it and never touches the ECS
    /// entity. Every swim depth line is a fraction of it, which is why a gnome floats with her head
    /// out and a night elf sits 0.3 yd deeper. Kept as the component's own type
    /// rather than a bare `f32` precisely so `Player::default()` cannot seed it to **zero** — at
    /// zero every depth line collapses to 0 and the avatar swims on dry land. It defaults to
    /// [`DEFAULT_COLLISION_HEIGHT`] and is replaced once our body's display id resolves.
    pub(crate) collision_height: crate::entities::CollisionHeight,
    /// **The liquid surface over our feet, as the last movement tick left it** — Bevy-Y, `None`
    /// when there is no liquid there.
    ///
    /// Cached rather than re-queried, because that is what the reference reads. `0x511ad0`'s depth
    /// comes through `0x670630`, which is a **field accessor** and not a query: `0x670637 test
    /// byte ptr [ecx+0x90],0x20` gates it and `0x670640 mov eax,[ecx+0x98]` returns whatever the
    /// movement tick last stored. The camera's water corridor
    /// ([`super::camera_water::classify`]) therefore reads *this*, one frame behind, which is the
    /// same lag the reference's own camera runs at — and it is load-bearing, not incidental: a
    /// depth that is piecewise constant is what keeps `d - target` from grazing a band edge while
    /// swimming (2173).
    pub(crate) liquid_surface: Option<f32>,
    /// The **mover pitch** (radians, +up) — the client's persistent per-unit pitch
    /// (`CMovement+0x20`): **held** when unsteered (an idle floater keeps its
    /// pitch — never auto-leveled; the only zeroing writer `0x7c6e80` fires from
    /// stop-swim/teleport, not mouse release). ActiveMover by mouselook as a **DIRECT set** of the
    /// camera aim pitch, clamped [`MOUSELOOK_PITCH_CLAMP`] (±89°): the ref's
    /// mouse-move event chain lands in `SetPitch 0x7c6f70`, an unconditional
    /// store with no integrator and no rate limit, and the basis rebuild re-aims travel in-call —
    /// hence zero lag. The `0x7c4f80` 0.75·turnRate integrator (clamp ±π/2) belongs to the
    /// PitchUp/Down keys, default-unbound in 1.12, which we don't bind. A left-drag camera
    /// orbit steers nothing (it doesn't turn the character, so it must not bend the swim);
    /// Space never touches the pitch (it is the Jump command).
    ///
    /// **It is not the *swim* pitch, and the name it used to carry was the bug** (
    /// B322): `+0x20` is one field, live in every mode. The mouse path that writes it —
    /// `0x514400 → 0x5103e0 → 0x515330 → 0x60de70 → 0x6198a0` — carries **no swim test** at any
    /// link (the unit gate `0x5145e0` is controllability only), and
    /// `SetPitch`'s own store at `0x7c6f91` precedes its `test [esi+0x40],0x200000`. Swimming gates
    /// only what *reads* it: the travel basis, the body pose, the wire tail. On land it has two
    /// other readers, and both are water walking's — the trace-mask arm's third gate
    /// (`0x63161e`, pitch > −37°) and `SetPitch`'s own dive-through (`0x7c6fb3`). Steering it only
    /// inside the swim branch left both unbuildable, which is why 1611 could only name them.
    /// Streamed on the wire's swim tail; the body renders pitched by it while swimming fwd/back
    /// (the `Ry` law of `0x60a110`, see the render block in [`super::control`]).
    pub(super) mover_pitch: f32,
    /// The camera pitch the **last pitch event** carried — the aim value we most recently pushed
    /// into [`Player::mover_pitch`]. Not state of the avatar's; state of the *input*, and it is
    /// here because the reference's pitch push is **event-driven, not per-frame**: `SetPitch` is
    /// called from the mouse-MOTION handler `0x514400` (input event `0x400500cb`, gated on a held
    /// drag button), so a mouse that does not move enqueues nothing at all. Our control system has
    /// only the already-accumulated camera angle, so "the mouse moved" is spelled here as "the
    /// camera aim differs from the one the last push carried".
    ///
    /// Which is load-bearing rather than pedantic: the other writers of the pitch —
    /// the drunk wobble, and StopSwim's zeroing at `0x7c6e80` — are only real if a still mouse
    /// leaves their value alone. Re-asserting the camera angle every frame would overwrite the
    /// swim-exit levelling on the very next one, and a water-walker who left the water nose-down
    /// would fall straight back through the surface.
    pub(super) aim_pitch_seen: f32,
    /// This frame's **flag-scalar swim travel speed** (yd/s) — the directional swim/swimBack
    /// speed when any swim translation input is live, else 0. The swim stroke's playback-rate
    /// numerator: `0x5fe2f0` divides `GetCurrentSpeed`
    /// (flags + static speed fields only, never a velocity/pitch projection) by the clip's
    /// moveSpeed, so a vertically pitched stroke plays at full rate. Written by the controller's
    /// swim arm; read at the `MovementState` fill; stale while not swimming.
    pub(super) swim_stroke_speed: f32,
    /// The rendered **body** heading (Bevy yaw, radians) — the client's display-facing pose: while
    /// strafing it eases toward `face_yaw ± 90°/45°` ([`crate::creature_anim::strafe_body_offset`])
    /// with the
    /// SpineLow/Head counter-twist walking the upper body back onto the aim
    /// ([`crate::creature_anim::BodyTwist`]); moving without a strafe it snaps to `face_yaw`;
    /// standing it chases at [`STATIONARY_CHASE_RATE`] × turn rate.
    pub(super) model_yaw: f32,
    /// A server-authored spline owns the avatar this frame (Charge, and later knockback/taxi/fear):
    /// the server sent an `SMSG_MONSTER_MOVE` for our own guid, so `sample_splines` drives the
    /// transform and [`super::server_ride::drive_self_ride`] mirrors it into `pos`/facing while input,
    /// physics, and the outbound movement stream all yield. Set the frame the ride's [`Spline`]
    /// appears, cleared the frame it ends — where we send `CMSG_MOVE_SPLINE_DONE` and resume.
    pub(super) server_riding: bool,
    /// The `splineId` of the ride in progress (echoed in `CMSG_MOVE_SPLINE_DONE` when it ends).
    pub(super) ride_spline_id: u32,
    /// Was the ride in progress a **ground** path? — the `FLYING` bit of the [`crate::net::Spline`]
    /// that drove it, kept because the frame the ride *ends* has already lost the spline
    /// (`sample_splines` drops a finished path) and the endpoint still has to be grounded by the
    /// same law as every frame before it. A taxi's endpoint keeps its own altitude.
    pub(super) ride_grounded: bool,
    /// Standing on a transport (boat/zepp): the mover lives in that platform's frame (decision
    /// 0438 phase 2). Attached when the ground support is a [`crate::transport::Transport`]
    /// collider; kept through jumps above the deck (deck-frame ballistics — a jump on a moving
    /// boat lands where it took off); detached on world-ground support, on entering the water,
    /// or when the boat despawns. See the carry/attach blocks in [`super::control`].
    pub(super) ride: Option<PlayerRide>,
}

/// The player's attachment to a transport's platform frame — see [`Player::ride`].
#[derive(PartialEq)]
pub(super) struct PlayerRide {
    /// The transport's ECS entity (its collider is the ground support that attached us).
    pub(super) entity: Entity,
    /// The transport's guid — the wire tail names the boat by guid, not entity.
    pub(super) guid: u64,
    /// Feet position in the transport's local frame (Bevy axes), snapshotted at frame end;
    /// next frame's carry recomposes `world = boat_transform × local` before input integrates.
    pub(super) local_pos: Vec3,
    /// The boat yaw (Bevy, radians) at the snapshot — the carry applies the per-frame delta to
    /// `face_yaw` (the deck turns the standing player with it), and the wire's local orientation
    /// is `face_yaw − boat_yaw`.
    pub(super) boat_yaw: f32,
}

/// **A knockback the server has aimed at our mover, latched until the mover flies it**
/// (`SMSG_MOVE_KNOCK_BACK`). A latch and not a direct write for [`Player::hover_launch`]'s
/// reason: the reference commits the launch inside its handler and `MOVEFLAG_FALLING` then keeps the
/// walk resolver off the body, while our mover re-derives ground contact from probes every frame and
/// would zero the velocity again on the next step. So the take-off is taken where every other
/// take-off is taken — inside [`super::mover::step`], by the same code that seeds a jump.
///
/// It carries the handshake with it because the two are inseparable: the ack is owed *only* if the
/// launch actually happened, and it must echo `launch` back verbatim as the `MovementInfo` jump tail
/// (the server matches all four floats within `0.01` before it will relay the knockback to anyone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PendingKnockback {
    /// Our own mover guid, echoed in the ack (a **full** u64 there, packed on the way in).
    pub(super) guid: u64,
    /// The server's movement counter for this change — echoed, or the ack is a logged cheat.
    pub(super) counter: u32,
    /// The launch quad: world-XY direction + horizontal speed + the take-off vertical speed in the
    /// jump tail's **down-positive** convention (negative is upward).
    pub(super) launch: JumpInfo,
}

impl Player {
    /// **Take the jump a `SetHover(true)` owes**, consuming the latch — the reference's
    /// `CMovement::Jump(force = 0)` at `0x61a630`, minus the seed select and the commit, which are
    /// the caller's because they are shared with Space.
    ///
    /// The two refusals are `0x7c625c test ah, 0x30` — **ROOT** (`0x1000`) and **FALLING**
    /// (`0x2000`), our [`Self::airborne_since`]. The one it does *not* take is the hover refusal at
    /// `0x7c623a`, which `force == 0` skips at `0x7c6236`; that is the entire reason this leg
    /// exists, since the body it is about to launch was granted hover in the same breath.
    ///
    /// It is a take-and-clear rather than a level read because the reference's handler fires once
    /// per opcode, not once per frame: a `SetHover` that lands on a rooted or already-falling body
    /// is *dropped*, not deferred until the refusal lifts (`0x7c6288 xor eax,eax; ret` — the
    /// refusal returns failure and nothing retries it).
    pub(super) fn take_wire_jump(&mut self) -> bool {
        let fire = self.hover_launch && !self.modes.rooted && self.airborne_since.is_none();
        self.hover_launch = false;
        fire
    }

    /// **Take the knockback the server aimed at us**, consuming the latch.
    ///
    /// Unconditional here, unlike [`Self::take_wire_jump`]: the refusals that matter are the mover's
    /// own — the settle hold and the root anchor, both of which freeze *every* axis and are already
    /// the reason a rooted body cannot be launched by anything. Taking it here and letting the mover
    /// decline keeps one refusal in one place, and keeps the ack honest: the caller sends
    /// `CMSG_MOVE_KNOCK_BACK_ACK` only if the launch actually happened, because that ack's whole
    /// content is a claim about the arc we are now flying.
    ///
    /// Take-and-clear, not a level read, for the same reason the hover launch is: the reference's
    /// handler fires once per opcode. A knockback that arrives on a frozen body is *dropped*, never
    /// deferred until the freeze lifts.
    pub(super) fn take_knockback(&mut self) -> Option<PendingKnockback> {
        self.knockback.take()
    }

    /// Turn the avatar's **aim** by `radians` — the scripted mouse-turn's one lever
    /// (`capture::probe_look`). Writing `face_yaw` is deliberately the whole of it:
    /// from here on this is the identical path a real mouse-turn takes, and it is the same value
    /// `stream_self_movement` diffs to decide on a `MSG_MOVE_SET_FACING`. A named method rather
    /// than a `pub(crate)` field for decision 1174's reason — an instrument may reach into
    /// gameplay, but not by opening gameplay's internals to the whole crate.
    pub(crate) fn turn_aim(&mut self, radians: f32) {
        self.face_yaw += radians;
    }

    /// Aim the avatar's **mover pitch** — the scripted dive's one lever (`capture::probe_pitch`),
    /// the twin of [`Self::turn_aim`] and for the same reason.
    ///
    /// The only writer of this field in gameplay is the mouse-look push in
    /// `controller`, which needs a real held mouse button (`mouselook`) and a moving OS cursor
    /// inside the viewport. Neither is available to an unfocused probe window, so before this
    /// there was **no way to make a benilla client swim nose-up or nose-down without a human on
    /// the mouse** — which is why the observed-swimmer tilt could ship, and
    /// sit for weeks, with nothing but the director's eye able to say whether it worked.
    ///
    /// Writes the same field `SetPitch 0x7c6f70` writes, under the same ±89°
    /// [`MOUSELOOK_PITCH_CLAMP`], so from here on this is the identical path a real mouse-aimed
    /// dive takes: the swim frame's travel basis, the body pose, and the wire tail all read this
    /// one value.
    ///
    /// Not a *byte* copy of `SetPitch`, and deliberately: that store is epsilon-gated — it commits
    /// iff `|new − old| >= [0x8026bc] = 2^-20`, the `==` case included (a parity test).
    /// Reproducing a 1e-6 rad deadband in
    /// an instrument would buy nothing and would make a slow scripted sweep stutter; the gate is
    /// gameplay's to model on the mouse-look push if it ever matters, which at that magnitude it
    /// cannot.
    /// Returns what was actually stored, which is the clamped value — so an instrument reports the
    /// aim the mover *has* rather than the one it asked for. A sweep that runs past ±89° otherwise
    /// logs a number the body never held.
    pub(crate) fn aim_pitch(&mut self, radians: f32) -> f32 {
        self.mover_pitch = radians.clamp(-MOUSELOOK_PITCH_CLAMP, MOUSELOOK_PITCH_CLAMP);
        // The mouse-look push is edge-triggered on the camera pitch it last saw; parking that at
        // the value we just wrote keeps a *real* mouse-look from re-pushing an unchanged camera
        // over the top of the script on the next frame.
        self.aim_pitch_seen = self.mover_pitch;
        self.mover_pitch
    }

    /// The character's *facing* (Bevy yaw, radians) — the aim, kept in sync with the camera by
    /// right-drag/movement. This is the unit's orientation as sent to the server, distinct from the
    /// rendered body heading (`model_yaw`, which a strafe rotates). The 3D-audio listener panning
    /// tracks this (the listener forward is the character facing, not the camera: `0x483430`).
    pub(crate) fn facing(&self) -> f32 {
        self.face_yaw
    }

    /// End the post-snap settle hold — called by the terrain streamer, the only
    /// system that knows the destination's residency. `resident` = the world arrived (scene +
    /// colliders); `false` = the [`SETTLE_TIMEOUT`] backstop fired without it. Which end it was is
    /// the whole diagnosis of a fall-through report, so it goes through the `sett` trace either way.
    pub(crate) fn end_settle(&mut self, resident: bool, now: f32) {
        self.settling = false;
        let waited = now - self.settle_since;
        super::move_trace::settle(resident, waited, self.pos);
    }

    /// The avatar's current CMovement move-flags as last streamed (directional + turn bits — see
    /// [`crate::creature_anim::move_flags`]). The water-foam selector reads the same two bit-tests as
    /// the reference (`& 0xf` translating, `& 0x30` turning; the CWater0Ripple driver `0x5fa760`).
    pub(crate) fn move_flags(&self) -> u32 {
        self.move_flags
    }

    /// **What is holding this body still** — every state on the resource that kills WASD, named,
    /// or `"none"`. An *instrument*, not a gate: the controller keeps its own
    /// tests, because each of these suppresses a different amount (`control_lost`/`server_riding`
    /// take the whole controller at [`super::control`]'s early-out; `rooted` zeroes the direction
    /// vector and the swim amounts but deliberately leaves turning live). What this exists for is
    /// the question a *log line* has to answer after a session boundary — "can the character that
    /// just entered the world be driven?" — which B306 proved nothing was asking: `scripts/smoke.sh`
    /// has crossed `/logout` → re-enter on every run since 2277 while counting UI rebuilds, errors
    /// and shutdown writes, none of which a frozen character disturbs.
    ///
    /// It does **not** make the smoke a B306 regression: that run logs in as a GM probe, so vmangos
    /// takes the instant-logout branch and never roots us at all (`smoke.sh` reports that gap on
    /// every run rather than banking a vacuous pass). What this covers live is the rest of the
    /// family — a possession or a lost session that leaves the reins where they were.
    ///
    /// It reads only this resource. A stun (`UNIT_FIELD_FLAGS`, [`crate::player::UNIT_FLAG_STUNNED`])
    /// lives on the unit's descriptor block and is not visible from here.
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

    /// The **commanded planar speed** in yd/s — the reference's `[[player+0x118]+0x84]`, the
    /// CMovement speed scalar its producer `0x7c4c90` returns:
    ///
    /// ```text
    /// mov  edx,[ecx+0x40]        ; movement flags
    /// test dl,0xf                ; the four DIRECTION bits
    /// jne  …                     ; moving → the speed
    /// fld  dword ptr [0x7ffd74]  ; = 0.0
    /// ```
    ///
    /// So it is **exactly zero with no direction key held** — not a measured velocity that decays,
    /// and not the weather wind's 149 ms positional average (`0x67c150`): it is live, and tracks a
    /// speed change on the same frame. The precipitation spawn slab's tilt keys on it through
    /// `mgr+0x7c` (`0x67bf8b`), which is why the distinction earns an
    /// accessor: an averaged stand-in would leave the slab leaning for 150 ms after a stop, and a
    /// raw `horiz_vel` would keep it leaning through an entire fall on take-off momentum.
    pub(crate) fn planar_speed(&self) -> f32 {
        if self.move_flags & 0xf == 0 {
            return 0.0;
        }
        self.horiz_vel.with_y(0.0).length()
    }

    /// The transport we're standing on (its guid), if any — the platform-frame attachment
    /// (decision 0438 phase 2). For instruments (the crossing probe watches the ride survive
    /// the map seam).
    pub(crate) fn riding(&self) -> Option<u64> {
        self.ride.as_ref().map(|r| r.guid)
    }

    /// The transport we're standing on, as its ECS entity — what the **ride frame** is read from
    /// (`benilla_world::ride_frame`): a rider's world-space effects are stored in
    /// the deck's frame, and the deck's live pose is the transport entity's transform.
    pub(crate) fn ride_entity(&self) -> Option<Entity> {
        self.ride.as_ref().map(|r| r.entity)
    }

    /// A server-authored spline currently owns the avatar (Charge/knockback/taxi — the
    /// [`super::server_ride`] state). For instruments (the taxi probe watches the flight run).
    /// **Not** `UnitOnTaxi` — a fear or a charge is a server ride too; that verb reads
    /// [`crate::player::UNIT_FLAG_TAXI_FLIGHT`] off the descriptor (`0x517a86`).
    pub(crate) fn server_riding(&self) -> bool {
        self.server_riding
    }
}

/// The **forward/back axis** — a net accumulation, at the reference's emitter `0x514da0`:
/// `autorun(+1) + forward(+1) + both-buttons(+1) − backward(−1)`, then one START in `sign(axis)`,
/// or a genuine STOP at zero. A pure function so the state table it encodes can be pinned by test —
/// the controller reads it for the direction vector, the backpedal speed, the swim amounts, and the
/// streamed flags, so all four can never disagree.
pub(super) fn forward_axis(
    forward: bool,
    backward: bool,
    both_buttons: bool,
    autorun: bool,
) -> i32 {
    i32::from(forward) + i32::from(both_buttons) + i32::from(autorun) - i32::from(backward)
}

/// Does this frame's input **destroy** autorun? The cancel set (six writers clear the reference's
/// `0x1000`; these are the four with a benilla analog).
///
/// `fwd_down`/`back_down` are **key-DOWN edges, not held state**: the clear lives in the shared SET
/// helper (`0x514a5a`, gated `test cl,0x30`), and the release path restores nothing. `both_engaged`
/// is the transition *into* both-buttons-held (`0x514a73`). `lost_mover` is death / root / stun / a
/// taxi hand-off, where the emitter's gate drops and `0x514748` clears the bit — a level, not an
/// edge.
///
/// A jump, a chat EditBox taking focus, and a zone change are each VERIFIED *survivors* and are
/// deliberately absent. Mounting is unsettled in the reference and is treated as a survivor here.
pub(super) fn autorun_cancelled(
    fwd_down: bool,
    back_down: bool,
    both_engaged: bool,
    lost_mover: bool,
) -> bool {
    fwd_down || back_down || both_engaged || lost_mover
}

/// Does the reference **refuse** this stand-state change? The sit-down gate inside the client's one
/// `SetStandState` (`0x5ed430`) — bug B155, *"you can sit fully underwater"*.
///
/// The client will not seat a body the movement layer is already driving. Sitting down reads the
/// live `CMovement` flags word `[[this+0x118]+0x40]` — the same word we stream and the cast gates
/// read — against a per-target-state mask, and **returns before the packet is
/// built** when any masked bit is set: no `CMSG_STANDSTATECHANGE`, no local apply, no message.
/// Standing up is the asymmetry: `newState == 0` jumps straight to the send and never consults the
/// word at all, so movement can always stand you.
///
/// Byte-for-byte (`0x5ed4d8`–`0x5ed501`):
///
/// ```c
/// if (newState != 0) {
///     if (newState == 3 && (mov->flags & 0x30))  return;   // 0x5ed4e6, `test byte [ecx+0x40],0x30`
///     if (mov->flags & 0x20000f)                 return;   // 0x5ed4f8, `test dword [edx+0x40],0x20000f`
/// }
/// ```
///
/// **SLEEP takes BOTH tests, not a different one.** `0x5ed4ec eb 04` is an unconditional `jmp`
/// *into* the second test, not around it — an either/or would have jumped to the send at
/// `0x5ed501`. 1581 shipped it as an alternative. The `newState == 3` test is a plain
/// equality: `2` (SIT_CHAIR) and `8` (KNEEL) take the shared leg exactly like `1` (SIT).
///
/// | target state | effective mask | refused while |
/// |---|---|---|
/// | 0 STAND | — (word never read) | never |
/// | 1 SIT · 2 SIT_CHAIR · 8 KNEEL · … | `0x20000f` | translating **or swimming** |
/// | 3 SLEEP | `0x20003f` | that, **plus** turning |
///
/// Neither mask is invented here; both are byte-verified twice over, at unrelated sites, as this
/// client's own movement tests — `0x20000f` is the stationary-cast pin's `[9e8] & 0x20000f` at
/// `0x5fde80` ([`crate::creature_anim::move_flags::CAST_PIN_MOVE`]) and `0x20003f` is the one-shot
/// route's `[9e8] & 0x20003f` at `0x5fe6dc` ([`ROUTE_COMMITTED_MOVE`](crate::creature_anim::move_flags::ROUTE_COMMITTED_MOVE)).
/// The swim bit in them is the whole of B155: a swimmer is *always* carrying it, so the press is
/// refused for as long as they are in the water, whether or not they are stroking. Note what is
/// **absent** from both — pitch, walk-mode, ROOT, and `FALLING` (`0x2000`): a standing jump does
/// not block a sit, while a running one does, through `FORWARD` rather than through the jump.
///
/// Because the refusal lives in the **one setter**, it covers every caller at once. Its caller
/// census is exhaustive (six `e8` callers, no tail-jumps, no dword reference
/// anywhere in the image, in no vtable) — the `X` keybind (`SitOrStand`, Lua `0x48b920`), the
/// posture emotes through `DoEmote`'s `EmoteSpecProc == 1` leg, `StartAttack`, the movement-input
/// wrapper `0x60be30`, and the five-minute AFK auto-sit in `WorldFrame::Render`.
///
/// The emote layer does **not** double up on the water half: the shipped `Emotes.dbc` gives
/// STATE_SIT (13), STATE_SLEEP (12), STATE_KNEEL (68) and STATE_STAND (26) flags `0x6202`, with the
/// swim-suppress bit `0x0080` clear — read off the 5875 data by
/// `ui_chat::tests::the_posture_emotes_carry_no_swim_suppression_flag`. (It has a *separate*,
/// louder gate that fires on movement rather than water — `0x4000` → `ERR_NOEMOTEWHILERUNNING` —
/// which benilla does not model; see [`crate::ui_chat::input::emote_send_eligible`].)
pub(super) fn stand_state_refused(reads_dead: bool, move_flags: u32, new_state: u8) -> bool {
    use crate::creature_anim::move_flags as f;
    // **A body that reads dead is refused outright, in EITHER direction** — the same setter's first
    // two guards, ahead of the stand-up asymmetry below and of the movement word: `0x5ed4a9 cmp
    // [eax+0x40],ebx` / `0x5ed4ac jle 0x5ed566` (UNIT_FIELD_HEALTH ≤ 0), then `0x5ed4b2`–`0x5ed4bd`
    // on `UNIT_DYNAMIC_FLAGS & 0x20` — so a **feigner** is refused too, at unchanged health.
    // A corpse cannot sit, and it cannot stand up
    // either, which is why this sits above the `new_state == 0` exit rather than beside it.
    if reads_dead {
        return true;
    }
    // The stand-up asymmetry: never gated (`0x5ed4f0 je 0x5ed501`).
    if new_state == 0 {
        return false;
    }
    // SLEEP's extra test, which falls THROUGH into the shared one below rather than replacing it.
    if new_state == 3 && move_flags & (f::TURN_LEFT | f::TURN_RIGHT) != 0 {
        return true;
    }
    move_flags & (f::ANY_MOVE | f::SWIMMING) != 0
}

#[cfg(test)]
mod stand_state_tests {
    use super::stand_state_refused;
    use crate::creature_anim::move_flags as f;

    /// **A body that reads dead is refused in EITHER direction** — `SetStandState`'s first two
    /// guards, ahead of the stand-up asymmetry and of the movement word: health ≤ 0 at `0x5ed4ac`,
    /// and `UNIT_DYNAMIC_FLAGS & 0x20` at `0x5ed4b2`–`0x5ed4bd`, which catches a **feigner** whose
    /// health never moved.
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

    /// B155's exact press: swimming, X pressed, the client refuses — and the same swimmer standing
    /// up is not refused, which is the asymmetry that makes the water escapable.
    #[test]
    fn a_swimmer_cannot_sit_but_can_always_stand() {
        // Floating still, no stroke: SWIMMING alone is enough.
        assert!(
            stand_state_refused(false, f::SWIMMING, 1),
            "sit refused mid-swim"
        );
        assert!(
            stand_state_refused(false, f::SWIMMING | f::FORWARD, 1),
            "swimming forward too"
        );
        // …and every seat shape the posture emotes can ask for.
        for state in [1u8, 2, 8] {
            assert!(
                stand_state_refused(false, f::SWIMMING, state),
                "state {state} refused mid-swim"
            );
        }
        // Standing up is never gated — `newState == 0` skips the word entirely.
        assert!(!stand_state_refused(false, f::SWIMMING | f::FORWARD, 0));
        assert!(!stand_state_refused(false, f::ANY_MOVE | f::SWIMMING, 0));
    }

    /// On dry land the gate is the *translation* test, not a blanket "any flag": sitting while
    /// walking is refused, sitting while turning in place is not — the turn bits are outside
    /// `0x20000f`. (The control that says this is a real mask rather than a swim special-case.)
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
        // Mode bits are not movement: a rooted or water-walking body may still sit.
        for bit in [f::ROOT, f::WATER_WALKING, f::FALLING, f::WALK_MODE] {
            assert!(
                !stand_state_refused(false, bit, 1),
                "mode bit {bit:#x} is not a move"
            );
        }
    }

    /// SLEEP (3) takes the shared mask **and one more test**, not a different one — the `0x30` turn
    /// pair falls THROUGH into `0x20000f` (`0x5ed4ec eb 04`, a jmp *into* the second test). 1581
    /// shipped it as an alternative and this is the assertion that pins the correction: a swimmer
    /// cannot `/sleep` any more than they can `/sit`, and turning blocks only the sleep.
    #[test]
    fn sleep_takes_both_tests_so_it_is_the_strictest_posture() {
        // The extra test, SLEEP's alone.
        assert!(
            stand_state_refused(false, f::TURN_LEFT, 3),
            "turning: /sleep refused"
        );
        assert!(stand_state_refused(false, f::TURN_RIGHT, 3));
        assert!(
            !stand_state_refused(false, f::TURN_LEFT, 1),
            "turning blocks ONLY the sleep — a sit is granted"
        );
        // …and the shared one it falls into, which 1581's either/or wrongly skipped.
        assert!(
            stand_state_refused(false, f::SWIMMING, 3),
            "swimming: /sleep refused"
        );
        assert!(
            stand_state_refused(false, f::FORWARD, 3),
            "walking: /sleep refused"
        );
        // So SLEEP's effective mask is exactly `0x20003f` — the one-shot route's own constant.
        assert_eq!(
            f::TURN_LEFT | f::TURN_RIGHT | f::ANY_MOVE | f::SWIMMING,
            f::ROUTE_COMMITTED_MOVE,
            "`0x20003f`, byte-verified at 0x5fe6dc as well as 0x5ed4e6+0x5ed4f8"
        );
        // Standing up is still ungated from SLEEP, which is what makes it escapable.
        assert!(!stand_state_refused(false, f::ROUTE_COMMITTED_MOVE, 0));
        // FALLING is in neither mask: a standing jump does not block a posture.
        assert!(
            !stand_state_refused(false, f::FALLING, 3),
            "falling: /sleep granted"
        );
    }
}

#[cfg(test)]
mod autorun_tests {
    use super::{autorun_cancelled, forward_axis};

    /// The four states of the reference's axis table (`0x514da0`), in its own terms.
    #[test]
    fn the_axis_reproduces_the_verified_state_table() {
        // autorun, nothing held → +1, runs forward.
        assert_eq!(forward_axis(false, false, false, true), 1);
        // "autorun + forward held" is the state AFTER W's key-down destroyed the bit, which is why
        // the table reads +1 and why pressing W is wire-silent: the axis sees forward alone, the
        // value it already had. The axis never sees the raw combination in this order.
        assert_eq!(forward_axis(true, false, false, false), 1);
        // Reaching it the other way (hold W, then toggle) DOES sum to 2 — `0x514da5`'s `mov ebx,1`
        // for autorun then `inc ebx` for forward. Only the sign is ever consumed, so it behaves
        // identically; asserted so the byte-shape is recorded rather than accidentally "fixed".
        assert_eq!(forward_axis(true, false, false, true), 2);
        // autorun ON, then S pressed → the key-down destroyed the bit first, so the axis sees
        // backward alone: −1, a clean reversal.
        assert_eq!(forward_axis(false, true, false, false), -1);
        // S held, THEN autorun toggled on → the toggle misses the clear helper, both live: 0.
        // The state no "autorun = held forward" reading can produce.
        assert_eq!(forward_axis(false, true, false, true), 0);
    }

    /// The order-dependence is the whole shape of the feature: the same two inputs, applied in the
    /// two orders, end in different places — and only one of them can resume.
    #[test]
    fn the_two_orders_differ_and_only_one_resumes() {
        // Order A — autorun first, then press S.
        let mut autorun = true;
        if autorun_cancelled(false, true, false, false) {
            autorun = false;
        }
        assert_eq!(
            forward_axis(false, true, false, autorun),
            -1,
            "walks backward"
        );
        // Releasing S leaves nothing behind: the release restores no bit.
        assert_eq!(
            forward_axis(false, false, false, autorun),
            0,
            "stops, does not resume"
        );

        // Order B — S held first, then toggle autorun on. The toggle is not a directional SET, so
        // the cancel set never fires.
        let autorun = true;
        assert!(!autorun_cancelled(false, false, false, false));
        assert_eq!(
            forward_axis(false, true, false, autorun),
            0,
            "STOP with S still held"
        );
        // Releasing S now DOES resume — the bit survived.
        assert_eq!(
            forward_axis(false, false, false, autorun),
            1,
            "resumes forward"
        );
    }

    /// Both-button run and autorun stack on the same axis, and a held S nets either to a standstill.
    #[test]
    fn both_button_run_shares_the_axis() {
        assert_eq!(forward_axis(false, false, true, false), 1);
        assert_eq!(
            forward_axis(false, true, true, false),
            0,
            "S nets the both-button run to a stop"
        );
        // Engaging the both-button run destroys autorun rather than stacking with it.
        assert!(autorun_cancelled(false, false, true, false));
    }

    /// The verified survivors: only the four listed inputs clear the bit.
    #[test]
    fn a_jump_or_a_chat_line_is_not_in_the_cancel_set() {
        assert!(!autorun_cancelled(false, false, false, false));
        // Losing the mover (death / root / taxi) is, and it is a level rather than an edge.
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

    /// Every ack'd mode round-trips: granted through [`MoveModes::set`], out as its own
    /// `MOVEMENTFLAGS` bit, and revoked again. The bit values are the ones vmangos reads back out of
    /// our acks, so a mismatch is silent on our side and wrong on the server's.
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

    /// **Levitate grants three modes at once** (spell 1706 = feather fall + hover + water walk), so
    /// the word has to carry all three together — the case that proves this is one system and not
    /// four independent booleans.
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
        // …and dropping one leaves the other two standing.
        modes.set(MoveMode::Hover, false);
        assert_eq!(modes.wire_flags(), f::SAFE_FALL | f::WATER_WALKING);
    }

    /// The server-authored merge owns the four unhandshaked bits and **must not touch root**. Root
    /// is inside the reference's `0x75a0_7dff` mask, so this is our deliberate divergence, not the
    /// mask's doing — and the failure it prevents is concrete: a bare pose clearing root locally
    /// while the server still holds us rooted streams moving bits into `CHEAT_TYPE_ROOT_MOVE`.
    #[test]
    fn the_wire_merge_never_unroots_us() {
        let mut modes = MoveModes {
            rooted: true,
            ..Default::default()
        };
        modes.merge_from_wire(0); // a pose claiming no modes at all
        assert!(modes.rooted, "only the ack'd opcode may unroot");
        assert!(!modes.hover && !modes.feather_fall && !modes.water_walking && !modes.levitating);

        // The other four do follow the wire, in both directions — this is how `.cheat fly` arrives.
        modes.merge_from_wire(f::SAFE_FALL | f::HOVER | f::WATER_WALKING | f::LEVITATING);
        assert!(modes.hover && modes.feather_fall && modes.water_walking && modes.levitating);
        assert!(modes.rooted, "still ours alone");
    }

    /// **Feather fall is a terminal-velocity substitution and nothing more** (`0x7c5d20`): gravity
    /// is untouched, so the drop *accelerates normally* and only then rides the lower cap. Pinning
    /// the shape, not just the constant — an implementation that scaled gravity instead would give
    /// a fall that starts wrong, and would pass a constants-only test.
    #[test]
    fn feather_fall_caps_the_descent_without_softening_gravity() {
        let dt = 1.0 / 60.0;
        let step = |v: f32, terminal: f32| (v - GRAVITY * dt).max(-terminal);

        // One frame from rest is identical with and without the aura — gravity is the same.
        assert_eq!(
            step(0.0, FEATHER_TERMINAL_VELOCITY),
            step(0.0, TERMINAL_VELOCITY),
            "the first frame of a slow fall is an ordinary fall"
        );

        // Held long enough, the two settle at their own caps.
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

        // And the cap is reached in well under half a second, which is why Slow Fall reads as a
        // drift rather than a fall: v = g·t ⇒ t = 7.0/19.291105 ≈ 0.363 s.
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

    /// The two mover constants are the reference's, not round numbers we liked. A drift here is
    /// invisible in play and wrong everywhere.
    #[test]
    fn the_mode_constants_are_the_verified_ones() {
        assert_eq!(FEATHER_TERMINAL_VELOCITY, 7.0); // [0x87d898]
        assert_eq!(HOVER_HEIGHT, 1.0); // [0x7ff9d8]
        assert_eq!(TERMINAL_VELOCITY, 60.148_003); // [0x87d894]
    }

    /// Feather fall must be the SLOWER cap — the whole point of the aura. A *compile-time* assert,
    /// because both sides are constants: swapped, they would otherwise only show up in play.
    const _: () = assert!(FEATHER_TERMINAL_VELOCITY < TERMINAL_VELOCITY);

    /// **A root and a stun are different states, and the difference is the pivot**.
    /// This is the distinction the first pass got wrong — B179 was filed under the movement-mode
    /// family, and it is not a member of it: root is a `MOVEMENTFLAGS` bit that leaves turning live
    /// by the input tick's *authored* allow-list, while the freeze is `UNIT_FIELD_FLAGS` bit
    /// `0x40000` gating the turn emitter. vmangos's `HandleModStun` grants both at once, which is
    /// the only reason they look like one thing.
    ///
    /// The bit values are what the gate reads, so a transcription slip here silently un-freezes
    /// every stun; `UNIT_FLAG_STUNNED` is checked against vmangos `UnitDefines.h` and against the
    /// reference's own `shr eax, 0x12` (bit 18 → `1 << 18` = `0x40000`).
    #[test]
    fn a_stun_is_not_a_root() {
        assert_eq!(crate::player::UNIT_FLAG_STUNNED, 1 << 18);
        assert_eq!(crate::player::UNIT_FLAG_STUNNED, 0x0004_0000);
        // It is a UNIT_FIELD_FLAGS bit, not a MOVEMENTFLAGS one — they are different words, and the
        // whole confusion was reading the freeze as a movement flag. Nothing in the mode family may
        // collide with it.
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
        // …and a root, alone, is not a stun: granting it touches no unit flag at all.
        let mut modes = MoveModes::default();
        modes.set(MoveMode::Root, true);
        assert_eq!(modes.wire_flags(), crate::creature_anim::move_flags::ROOT);
    }

    /// The hover clearance is reached by a **rate-limited climb**, not a snap — the reference's
    /// second writer. Pins the rate and, more importantly, that the rise takes real time: a body a
    /// full yard low needs ~0.143 s, which is the difference between a float and a pop.
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

    /// **A hover grant owes exactly one jump, and two states eat it** — the
    /// `Jump(force = 0)` at `0x61a630`, refused by `0x7c625c test ah, 0x30` for ROOT and FALLING and
    /// by nothing else. In particular NOT by hover itself: `0x7c6236`'s `force == 0` skip is the
    /// whole point of the leg, and the body being launched is hovering by construction.
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

    /// **Death is both a root and a stun, and it is the only state that is both**
    /// — the truth table of the reference's two movement-input predicates, whose shared
    /// precondition `0x5144e0` is what makes a corpse unturnable.
    ///
    /// The last two assertions are the bug this test exists for: benilla only ever modelled the
    /// server's root on death, and a pure root leaves the pivot live *on purpose*, so a dead body
    /// could be spun with the turn keys or a right-drag. The predicate that stops it must not need
    /// the root, the stun, or the server's cooperation.
    /// **A seated body is not re-faced by the mouse**: `0x5145e0`'s last conjunct
    /// is `GetStandState() == 0`, so sitting, a chair (client 2 and the server's 4/5/6) and sleep
    /// all refuse the camera→body hand-off — while the keyboard turn (`may_turn`) is untouched,
    /// because that path stands you up instead of being refused.
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

    /// **The teardown leg needs BOTH predicates down** (`0x5146d6 call 0x60fb60`). benilla had the
    /// root in this position and so ended a follow on a Frost Nova, which the reference does not.
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

    /// **Far sight freezes your own body — and never a possessed one** (conjunct 5 of
    /// `0x5144e0`). While your view is out on a far-sight object you may neither walk nor turn:
    /// the term sits in the *shared* precondition, so it takes both predicates exactly the way
    /// death does.
    ///
    /// The second half is the one worth a test of its own. Mind Control rides the same
    /// `PLAYER_FARSIGHT` field the camera does, so during a possession the latch is engaged — and
    /// the reference reads it only after `0x514537`'s active-player test on the **mover** has
    /// already passed. Model the latch without that test and the victim freezes solid, which is the
    /// one body the whole spell exists to walk around.
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

    /// **The two incapacitate suppressions take exactly their own bits** — the
    /// difference between a pure root and a stun, which is the whole distinction B179 was drawing,
    /// expressed on the one word that drives both the animation and the wire.
    #[test]
    fn the_incapacitate_suppressions_take_exactly_their_own_bits() {
        // Every reported motion a body can hold at once, plus every mode a grant can have riding.
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
        // A stun is both at once (vmangos `HandleModStun` = SetFlag(STUNNED) + SetRooted(true)).
        let blocked = incapacitated_flags(keys | modes, true, true);
        assert_eq!(
            blocked, modes,
            "Ice Block leaves NO reported motion — which is what starves the anim resolver"
        );
        // …and the modes must survive it, or the next server-authored echo clears them under us
        // (decision 0726's failure mode). ROOT above all: dropping it unroots us locally against a
        // server that still has us rooted.
        assert_eq!(blocked & f::ROOT, f::ROOT, "the root bit itself rides on");
        assert_eq!(
            blocked & f::SWIMMING,
            f::SWIMMING,
            "a rooted swimmer is still swimming — `0xffe07f00` preserves 0x200000"
        );
    }
}
