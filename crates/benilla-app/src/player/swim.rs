//! Swim mode: deep enough, the avatar leaves the floor and swims in 3D along its pitched facing.
//! The reference's decision `0x6030c0` latches `MOVEFLAG_SWIMMING` (`0x00200000`) on the depth
//! over the feet against 0.75 × the unit's collision height, with a 1/36-yd hysteresis; there is
//! no wade flag (`0x40000000` is HOVER).
//!
//! The floating resolver bypasses gravity: an idle swimmer's depth is frozen, the vertical comes
//! only from the pitched stroke, and the one constraint is a top cap with the feet at
//! `surface - 0.75·h` (`0x632ba0`), satisfied again when the surface falls ([`settle_to_rest`]) and
//! never pulling a deeper swimmer up. A rise into the cap turns level at full speed
//! ([`cap_redirect`]). The way out through the top is the swim jump ([`breach_step`], `0x7c6230`);
//! the way to rise is aiming up and swimming forward (`0x7c6f70`).

use avian3d::prelude::*;
use bevy::prelude::*;

use benilla_world::world_point::{Subject, WorldPoint};

use super::mover::Outcome;
use super::{Player, CAPSULE_HEIGHT, GRAVITY, GROUND_COS, GROUND_PROBE, SKIN_WIDTH};
use benilla_assets::coords::bevy_to_wow;

/// Swim speed (yd/s), the stock `MOVE_SWIM` (vmangos `Unit.cpp:71`): the fallback until the
/// server's speeds stream in, and under the `$WOW_MOVE_SPEED` run override.
pub(super) const SWIM_SPEED: f32 = 4.722_222;

/// Backward swim speed (yd/s), the stock `MOVE_SWIM_BACK` (vmangos `Unit.cpp:72`), with the same
/// fallback role as [`SWIM_SPEED`].
pub(super) const SWIM_BACK_SPEED: f32 = 2.5;

/// Swim-jump take-off speed (yd/s): `0x7c6230`'s swim seed `0xc1118c48` (-9.096748, down-positive),
/// about 14% over the land jump's 7.955547.
const SWIM_JUMP_SPEED: f32 = 9.096_748;

/// The share of the unit's collision height the water must cover to swim (`0x8012cc`, compared
/// at `0x6030c0`).
const SWIM_DEPTH_FRAC: f32 = 0.75;
/// The enter/leave band (yd, `0x7ff9d0`): enter above `0.75·h`, leave below `0.75·h - 1/36`,
/// hold between (`0x603100`, `0x6031c0`).
const SWIM_HYSTERESIS: f32 = 1.0 / 36.0;

/// The `UNIT_FIELD_FLAGS` bits that let a unit swim: `PLAYER_CONTROLLED` (0x8, set on every
/// player, vmangos `Player.cpp:462`), `PET_IN_COMBAT` (0x800) and `USE_SWIM_ANIMATION` (0x8000,
/// from the creature's `CAN_SWIM`). `0x6030c0` tests them on both legs: with none set a unit
/// cannot enter and is stopped at any depth (`0x6031b4`, then `0x60dff0`). The client's only reads
/// of 0x8000 are this predicate (`0x5fb922`, `0x603111`, `0x6031d0`). Every body a player can be
/// [`crate::net::Embodied`] in has 0x8, so only the creature marker
/// (`crate::net::motion::spline::mark_swimming_creatures`) needs this.
const SWIM_UNIT_FLAGS: u32 = 0x8 | 0x800 | 0x8000;

/// Whether a unit may swim at all ([`SWIM_UNIT_FLAGS`]): false blocks entry and forces an exit.
pub(crate) fn may_swim(unit_flags: u32) -> bool {
    unit_flags & SWIM_UNIT_FLAGS != 0
}

/// The depth over the feet (yd) past which a unit swims: 0.75 × its own collision height
/// ([`crate::entities::CollisionHeight`]), about 1.52 yd for a human male and 0.86 for a gnome
/// female. Also the wade ceiling: wading is being in liquid without swimming.
pub(crate) fn swim_enter_depth(h: f32) -> f32 {
    SWIM_DEPTH_FRAC * h
}
/// The depth (yd) below which swimming stops, `0.75·h - 1/36`: the band is absolute, never scaled
/// by `h` (`0x6030f2` subtracts `[0x7ff9d0]`).
pub(super) fn swim_exit_depth(h: f32) -> f32 {
    swim_enter_depth(h) - SWIM_HYSTERESIS
}

/// The depth (yd) of the top cap a rising swimmer's feet stop at, the floating resolver's top
/// plane (`0x632ba0`). It equals the enter depth, so a capped swimmer never drops out of the latch.
fn rest_cap(h: f32) -> f32 {
    swim_enter_depth(h)
}

/// The Bevy-Y surface of any liquid over the feet: magma and slime are swum in like water. The
/// subject is the player, so its own interior claim picks which room's liquid answers.
pub(super) fn surface_over_feet(world: &WorldPoint, feet: Vec3) -> Option<f32> {
    let wow = bevy_to_wow(feet);
    world
        .liquid_at(Subject::Player, wow)
        .map(|hit| feet.y + (hit.surface_z - wow[2]))
}

/// Latches [`Player::swimming`] as `0x6030c0` does and returns it: enter on a strict
/// `depth > 0.75·h`, leave on `depth < 0.75·h - 1/36` or no liquid at all. Entry also waits out a
/// rising launch until its upward speed halves (`0x7c5de0`, about 0.236 s for a swim jump), so the
/// hop re-latches still rising and its residual velocity is dropped (`0x7c6e50` → `0x7c6290`).
/// `now` is `Time::elapsed_secs`. Never touches [`Player::settling`], the terrain streamer's.
pub(super) fn update_swimming(player: &mut Player, surface_y: Option<f32>, now: f32) -> bool {
    // LEVITATING skips the whole decision (`0x6030d2` test ah,4, to `0x6031fa`): it is GM flight,
    // whose SWIMMING the dry ground would otherwise clear the next frame.
    if player.modes.levitating {
        return player.swimming;
    }
    let was = player.swimming;
    let Some(surface) = surface_y else {
        player.swimming = false; // not in liquid
        stop_pitch(player, was);
        return false;
    };
    let depth = surface - player.pos.y;
    let h = player.collision_height.0;
    player.swimming = if player.swimming {
        depth >= swim_exit_depth(h)
    } else {
        let hop_blocked = player.vel_y > 0.0
            && player
                .airborne_since
                .is_some_and(|t0| now - t0 < player.jump_zspeed / (2.0 * GRAVITY));
        depth > swim_enter_depth(h) && !hop_blocked
    };
    stop_pitch(player, was);
    player.swimming
}

/// A depth-driven stop zeroes the mover pitch, which water walking reads on land: StopSwim
/// `0x7c6e80` (via `0x6031eb` → `0x60dff0` → `0x61a070`) is the pitch's only zeroing writer. A
/// breach keeps its aim, as `CMovement::Jump` (`0x7c6230`) clears SWIMMING through `0x7c61f0`.
fn stop_pitch(player: &mut Player, was: bool) {
    if was && !player.swimming {
        player.mover_pitch = 0.0;
    }
}

/// The take-off frame of a jump while swimming (`0x7c6230`), at any depth: [`SWIM_JUMP_SPEED`],
/// then FALLING. The last swim frame's travel carries the leap, frozen as in every jump
/// (`0x7c61f0` never rewrites it), and there is no gravity tick this frame, so the jump tail
/// carries the exact seed. The caller has already cleared [`Player::swimming`].
pub(super) fn breach_step(
    player: &mut Player,
    time: &Time,
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
) -> Outcome {
    player.vel_y = SWIM_JUMP_SPEED;
    // For the arc bookkeeping, or the jump tail sends `zspeed = 0` and observers see a step-off.
    player.launch_vz = SWIM_JUMP_SPEED;
    // The fresh arc's direction nibble seeds from the swim keys, as a ledge step-off's does.
    player.arc_dirs_set = player.horiz_vel.length_squared() > 0.0;
    player.knock_arc = false;
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let out = world.slide_body(
        capsule,
        player.pos + half_h,
        player.horiz_vel + Vec3::Y * player.vel_y,
        time.delta(),
        &MoveAndSlideConfig::default(),
        |_hit| MoveAndSlideHitResponse::Accept,
    );
    player.pos = out.position - half_h;
    Outcome {
        held: false,
        grounded: false,
        jumped: true,
        knocked: false,
        air_nudged: false,
        ground: None,
    }
}

/// One swim step's result, for the controller's flags and animation.
pub(super) struct SwimOutcome {
    /// Walkable floor right under the feet: a shallow bottom.
    pub grounded: bool,
    /// The travel pitch after a surface redirect, for the pose and the wire; `None` when the cap
    /// did not bite. The raw aim stays in [`Player::mover_pitch`].
    pub surface_pitch: Option<f32>,
}

/// Clamps a rising stroke's upward speed to `cap`, the rise that reaches the rest line this
/// frame, and turns the rest level at the same speed; returns the velocity and, when the cap bit,
/// the travel pitch. The 1.12 client is seen swimming level at full speed along the surface; its
/// own-input resolver (`0x634640`) as read slides a steep aim along the cap at `cos(pitch)·speed`,
/// near zero, so the mechanism behind the seen behaviour is untraced.
fn cap_redirect(input_vel: Vec3, cap: f32) -> (Vec3, Option<f32>) {
    if input_vel.y <= 0.0 || input_vel.y <= cap {
        return (input_vel, None);
    }
    let speed = input_vel.length();
    let level_dir = Vec3::new(input_vel.x, 0.0, input_vel.z).normalize_or_zero();
    let level_speed = (speed * speed - cap * cap).max(0.0).sqrt();
    (
        level_dir * level_speed + Vec3::Y * cap,
        Some(cap.atan2(level_speed)),
    )
}

/// How far the feet sit above the rest line: the sink that satisfies it, which [`swim_step`]
/// sweeps so a lakebed can still hold the feet higher.
fn settle_to_rest(feet_y: f32, surface_y: f32, h: f32) -> f32 {
    (feet_y - (surface_y - rest_cap(h))).max(0.0)
}

/// The surface [`swim_step`] caps the swimmer under, or `None` in GM flight: `LEVITATING` lifts
/// the water's authority (`0x6030d2`, as in [`update_swimming`]), so an ascent is free. A
/// momentary sample miss in real water is `Some(own feet Y)`, a hold for the frame. Both arms of
/// the constraint, the rise cap and the settle, must read this one source.
pub(super) fn rest_line(player: &Player, surface_y: Option<f32>) -> Option<f32> {
    (!player.modes.levitating).then(|| surface_y.unwrap_or(player.pos.y))
}

/// One swim frame through the floating resolver (`0x634640`): no gravity, the vertical only from
/// `input_vel`, a slide against the lakebed and banks, the rise capped at the rest line
/// ([`cap_redirect`]) and a stroke left above it settled back ([`settle_to_rest`]). `surface_y`
/// is the waterline where the frame starts, which caps its climb; `surface_at` resamples it where
/// the stroke lands, since on a steep river a frame's descent can be the whole hysteresis band.
pub(super) fn swim_step(
    player: &mut Player,
    time: &Time,
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    input_vel: Vec3,
    surface_y: Option<f32>,
    surface_at: impl Fn(Vec3) -> Option<f32>,
) -> SwimOutcome {
    let dt = time.delta_secs();
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let center = player.pos + half_h;

    // Cap the upward velocity, not the position after the slide: a position clamp overrides the
    // lakebed's collision, so in the shallows the depth never drops below the leave threshold and
    // the swimmer cannot walk out. `None` is GM flight, uncapped.
    let cap = match surface_y {
        Some(surface) if dt > 0.0 => {
            let rest_feet_y = surface - rest_cap(player.collision_height.0);
            ((rest_feet_y - player.pos.y) / dt).max(0.0)
        }
        _ => f32::INFINITY,
    };
    let (vel, surface_pitch) = cap_redirect(input_vel, cap);

    let out = world.slide_body(
        capsule,
        center,
        vel,
        time.delta(),
        &MoveAndSlideConfig::default(),
        |_hit| MoveAndSlideHitResponse::Accept,
    );
    let mut c = out.position;
    // Satisfy the rest line, not only guard it: a swimmer mostly ends up above it by the surface
    // falling to meet them, and a river falling a tenth of a yard per yard sheds the 1/36-yd band
    // within a few frames, flapping the latch between swim and fall. The drop is swept, so a bank
    // still holds the feet up. Only under a stroke, as the resolver's outer gate skips an idle
    // mover (`0x634100` test [esi+0x40],0x200f), and only under the rest line the cap reads, or GM
    // flight over a lake would drop onto the waterline.
    if surface_y.is_some() && input_vel != Vec3::ZERO {
        if let Some(surface_now) = surface_at(c - half_h) {
            let excess = settle_to_rest(c.y - half_h.y, surface_now, player.collision_height.0);
            if excess > 0.0 {
                let drop = world
                    .cast_body(capsule, c, Vec3::NEG_Y * excess, SKIN_WIDTH)
                    .map_or(excess, |h| h.distance.min(excess));
                c.y -= drop;
            }
        }
    }
    player.pos = c - half_h;
    // Swim owns its vertical: a clean zero, so a fall out of the water starts from rest.
    player.vel_y = 0.0;
    player.horiz_vel = Vec3::new(vel.x, 0.0, vel.z);

    let probe = world.cast_body(capsule, c, Vec3::NEG_Y * GROUND_PROBE, SKIN_WIDTH);
    let grounded = probe.is_some_and(|h| h.normal1.y >= GROUND_COS);
    SwimOutcome {
        grounded,
        surface_pitch,
    }
}

/// One swim frame's mover outcome and its presented pitch, for the pose and the wire: the raw aim,
/// or the level travel pitch after a surface redirect.
pub(super) struct SwimFrame {
    pub outcome: Outcome,
    pub pitch: f32,
}

/// One swimming frame: the drunk pitch wobble, the pitched travel basis, the directional speed,
/// the stroke rate and [`swim_step`]. `basis` is the level `(forward, right)` pair, `amounts` the
/// netted `(fwd, side)` translation ([`translate_amounts`]), and `wobble` the drunk angle, zero
/// when it must not apply.
pub(super) fn drive_step(
    player: &mut Player,
    time: &Time,
    collide: &benilla_world::collision::WorldCollision<'_, '_>,
    capsule: &Collider,
    world: &WorldPoint,
    surface_y: Option<f32>,
    basis: (Vec3, Vec3),
    amounts: (f32, f32),
    speeds: Option<benilla_protocol::MoveSpeeds>,
    move_speed: &super::MoveSpeed,
    wobble: f32,
) -> SwimFrame {
    let (swim_fwd, swim_side) = amounts;
    // Drunk and swimming, the pitch wobbles too, by the facing veer's angle × 4.0 each frame
    // (`0x60aabc`-`0x60ab0a`, `[0x80306c]`, committed through `0x60de70`), clamped to ±π/2
    // (`0x60aba0`, `0x808acc`/`0x80c5e4`), not the mouselook's ±89°.
    if wobble != 0.0 {
        player.mover_pitch = (player.mover_pitch + wobble * super::drunk::SWIM_PITCH_WOBBLE_SCALE)
            .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
    }
    let mut pitch = player.mover_pitch;
    // The travel basis (`0x7c5880`): forward is the facing pitched by the swim pitch, strafe stays
    // level, and there is no vertical input; Space's one swim role is the jump.
    let (sp, cp) = player.mover_pitch.sin_cos();
    let fwd_axis = basis.0 * cp + Vec3::Y * sp;
    let v = fwd_axis * swim_fwd + basis.1 * swim_side;
    let dir3 = v.normalize_or_zero();
    let rest_line = rest_line(player, surface_y);
    // The directional speed (`0x7c4c90`'s swim arm): backward takes `min(swimBack, swim)`,
    // forward and strafe the swim speed.
    let (swim_speed, swim_back_speed) = match speeds {
        Some(s) if !move_speed.env_override => (s.swim, s.swim_back),
        _ => (SWIM_SPEED, SWIM_BACK_SPEED),
    };
    let dir_speed = if swim_fwd < 0.0 {
        swim_back_speed.min(swim_speed)
    } else {
        swim_speed
    };
    // The stroke rate's numerator is the flag speed whatever the pitch: `0x5fe2f0` divides
    // `GetCurrentSpeed` by the clip's speed, for local and observed units alike.
    player.swim_stroke_speed = if dir3 == Vec3::ZERO { 0.0 } else { dir_speed };
    let out = swim_step(
        player,
        time,
        collide,
        capsule,
        dir3 * dir_speed,
        rest_line,
        |feet| surface_over_feet(world, feet),
    );
    // After a surface redirect the pose and the wire show the travel pitch, while the raw aim
    // stays in `mover_pitch` so a nose-down dives at once.
    if let Some(p) = out.surface_pitch {
        pitch = p;
    }
    let outcome = Outcome {
        held: false,
        grounded: out.grounded,
        jumped: false,
        // A knockback clears SWIMMING at take-off (`StartFalling`, `0x7c61f0`), so the land
        // mover flies it.
        knocked: false,
        air_nudged: false,
        ground: None, // swimming detaches from any platform frame below
    };
    SwimFrame { outcome, pitch }
}

/// The swim translation `(fwd, side)`, read by [`drive_step`] and the flag build alike so the two
/// agree; under mouselook A/D strafe.
pub(super) fn translate_amounts(
    axes: &super::input::MoveAxes,
    translate_gated: bool,
) -> (f32, f32) {
    // The translate predicate `0x514560` (a root, or death) cuts swim translation as it cuts the
    // walk; the water reads its own axes, so it needs its own cut.
    if translate_gated {
        return (0.0, 0.0);
    }
    let swim_fwd = axes.fwd.signum() as f32;
    let mut swim_side = 0.0_f32; // +right
    if axes.strafe_right {
        swim_side += 1.0;
    }
    if axes.strafe_left {
        swim_side -= 1.0;
    }
    if axes.mouselook {
        if axes.turn_right {
            swim_side += 1.0;
        }
        if axes.turn_left {
            swim_side -= 1.0;
        }
    }
    (swim_fwd, swim_side)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CreatureModelData.collisionHeight × displayScale` for three shipped bodies, as
    /// `benilla_formats`' `collision_height_is_the_m2_collision_box` pins them.
    const HUMAN_MALE: f32 = 2.031;
    const GNOME_FEMALE: f32 = 1.150; // 1.000 column × 1.15 display scale
    const NIGHT_ELF_MALE: f32 = 2.438;

    fn player_at(y: f32) -> Player {
        player_of_height(y, HUMAN_MALE)
    }

    fn player_of_height(y: f32, h: f32) -> Player {
        Player {
            pos: Vec3::new(0.0, y, 0.0),
            collision_height: crate::entities::CollisionHeight(h),
            ..Default::default()
        }
    }

    #[test]
    fn leaving_the_water_by_depth_levels_the_pitch_and_a_breach_does_not() {
        let deep = swim_enter_depth(HUMAN_MALE) + 1.0;
        let mut player = player_at(0.0);
        player.mover_pitch = -0.9;
        assert!(update_swimming(&mut player, Some(deep), 0.0), "must swim");
        assert_eq!(player.mover_pitch, -0.9, "swimming holds the aim");
        let band = swim_exit_depth(HUMAN_MALE) + 0.001;
        assert!(update_swimming(&mut player, Some(band), 0.1));
        assert_eq!(player.mover_pitch, -0.9);
        // Out through the bottom of the band.
        assert!(!update_swimming(&mut player, Some(0.5), 0.2));
        assert_eq!(
            player.mover_pitch, 0.0,
            "`0x7c6e80` zeroes +0x20 on StopSwim"
        );
        // No liquid at all is the same stop.
        player.mover_pitch = -0.9;
        assert!(update_swimming(&mut player, Some(deep), 1.0));
        assert!(!update_swimming(&mut player, None, 1.1));
        assert_eq!(player.mover_pitch, 0.0);
        // A breach clears the latch itself (`0x7c61f0`), so the next tick sees no stop.
        player.mover_pitch = -0.9;
        assert!(update_swimming(&mut player, Some(deep), 2.0));
        player.swimming = false; // as the controller's breach arm does
        breach_pitch_survives(&mut player, deep);
        assert_eq!(player.mover_pitch, -0.9, "a dolphin-hop keeps its aim");
    }

    /// One latch tick after a breach has cleared it.
    fn breach_pitch_survives(player: &mut Player, depth: f32) {
        player.vel_y = 0.0;
        update_swimming(player, Some(depth), 2.1);
    }

    /// `PET_IN_COMBAT` is no typo: `0x6030c0` tests bit 11 on entry (`0x60310b`) and on the stop
    /// (`0x6031ca`).
    #[test]
    fn the_swim_gate_is_those_three_unit_flags_and_no_others() {
        const PLAYER_CONTROLLED: u32 = 0x8;
        const PET_IN_COMBAT: u32 = 0x800;
        const USE_SWIM_ANIMATION: u32 = 0x8000;
        assert_eq!(
            SWIM_UNIT_FLAGS,
            PLAYER_CONTROLLED | PET_IN_COMBAT | USE_SWIM_ANIMATION
        );
        for bit in [PLAYER_CONTROLLED, PET_IN_COMBAT, USE_SWIM_ANIMATION] {
            assert!(may_swim(bit), "any ONE of the three admits ({bit:#x})");
        }
        assert!(!may_swim(0), "no flags at all → walks the lakebed");
        assert!(!may_swim(!SWIM_UNIT_FLAGS));
    }

    #[test]
    fn swim_entry_and_exit_hysteresis() {
        for h in [HUMAN_MALE, GNOME_FEMALE, NIGHT_ELF_MALE] {
            assert!((swim_enter_depth(h) - swim_exit_depth(h) - 1.0 / 36.0).abs() < 1e-6);
            assert!((swim_enter_depth(h) - 0.75 * h).abs() < 1e-6);
        }

        let (enter, exit) = (swim_enter_depth(HUMAN_MALE), swim_exit_depth(HUMAN_MALE));
        let mut p = player_at(0.0); // feet at 0, so the surface height is the depth
        let mid_band = (enter + exit) * 0.5;
        assert!(
            !update_swimming(&mut p, Some(enter), 0.0),
            "exactly at the enter depth does not yet swim (strict >)"
        );
        assert!(
            !update_swimming(&mut p, Some(mid_band), 0.0),
            "a depth in the band, entered from walking, stays walking"
        );
        assert!(
            update_swimming(&mut p, Some(enter + 0.01), 0.0),
            "past the enter depth starts swimming"
        );
        assert!(
            update_swimming(&mut p, Some(mid_band), 0.0),
            "the same in-band depth, now entered from swimming, keeps swimming (hysteresis)"
        );
        assert!(
            !update_swimming(&mut p, Some(exit - 0.01), 0.0),
            "dropping below the leave threshold returns to walking"
        );
        assert!(update_swimming(&mut p, Some(enter + 0.5), 0.0));
        assert!(
            !update_swimming(&mut p, None, 0.0),
            "no liquid over the feet stops swimming (the binary's inLiquid == 0 → STOP)"
        );
    }

    #[test]
    fn levitating_bails_the_water_decision_in_both_directions() {
        // `.cheat fly on` sets SWIMMING and LEVITATING (`Player.cpp:4595`); on dry ground.
        let mut flying = player_at(0.0);
        flying.swimming = true;
        flying.modes.levitating = true;
        assert!(
            update_swimming(&mut flying, None, 0.0),
            "no liquid at all must NOT stop a levitating swimmer — this is the whole of GM flight"
        );
        assert!(
            update_swimming(&mut flying, Some(-50.0), 0.0),
            "nor may a surface far below the feet, which is what flying over a lake looks like"
        );

        let mut dry = player_at(0.0);
        dry.modes.levitating = true;
        assert!(
            !update_swimming(&mut dry, Some(swim_enter_depth(HUMAN_MALE) + 1.0), 0.0),
            "the ENTER arm is skipped too — the bail is before the branch, not inside it"
        );

        // `.cheat fly off` sends flags 0.
        flying.modes.levitating = false;
        assert!(
            !update_swimming(&mut flying, None, 0.0),
            "with the mode gone, no liquid stops swimming again"
        );
    }

    #[test]
    fn the_latch_leaves_the_settle_hold_alone() {
        for (surface, name) in [
            (Some(swim_enter_depth(HUMAN_MALE) + 1.0), "swim depth"),
            (Some(swim_exit_depth(HUMAN_MALE) - 0.5), "wading depth"),
            (None, "dry land"),
        ] {
            let mut p = player_at(0.0);
            p.settling = true;
            update_swimming(&mut p, surface, 0.0);
            assert!(p.settling, "{name} must leave the hold to the streamer");
        }
    }

    #[test]
    fn the_hop_relatches_at_half_launch_velocity() {
        let half_decay = SWIM_JUMP_SPEED / (2.0 * GRAVITY);
        let deep = Some(swim_enter_depth(HUMAN_MALE) + 1.0);
        let mut p = player_at(0.0);
        p.airborne_since = Some(0.0);
        p.jump_zspeed = SWIM_JUMP_SPEED;
        p.vel_y = SWIM_JUMP_SPEED;
        assert!(
            !update_swimming(&mut p, deep, half_decay * 0.5),
            "young launch, still fast — the hop keeps rising"
        );
        p.vel_y = SWIM_JUMP_SPEED * 0.49;
        assert!(
            update_swimming(&mut p, deep, half_decay + 1e-3),
            "velocity decayed to half — swim re-latches while STILL rising (the ~1.6 yd hop top)"
        );
        let mut q = player_at(0.0);
        q.airborne_since = Some(0.0);
        q.jump_zspeed = SWIM_JUMP_SPEED;
        q.vel_y = -0.1;
        assert!(
            update_swimming(&mut q, deep, 0.01),
            "descending into depth enters swim — the gate only guards a rising launch"
        );
    }

    #[test]
    fn the_rest_line_is_satisfied_from_above_and_never_pulls_up() {
        for h in [HUMAN_MALE, GNOME_FEMALE, NIGHT_ELF_MALE] {
            let cap = rest_cap(h);
            assert_eq!(settle_to_rest(10.0 - cap, 10.0, h), 0.0);
            assert_eq!(settle_to_rest(10.0 - cap - 0.01, 10.0, h), 0.0);
            assert_eq!(settle_to_rest(10.0 - cap - 30.0, 10.0, h), 0.0);
            // Above it, as when a river's surface comes down to the feet: sink exactly the excess.
            let excess = 0.25;
            assert!((settle_to_rest(10.0 - cap + excess, 10.0, h) - excess).abs() < 1e-6);
        }
    }

    #[test]
    fn gm_flight_hands_the_water_no_authority_over_the_vertical() {
        let lake_surface = 40.0;

        // 300 yd above a lake in GM flight: the liquid query still answers `Some` under us.
        let mut flying = player_at(lake_surface + 300.0);
        flying.swimming = true;
        flying.modes.levitating = true;
        assert_eq!(
            rest_line(&flying, Some(lake_surface)),
            None,
            "no rest line while levitating — this is the gate BOTH arms take, not just the cap"
        );
        assert!(
            settle_to_rest(flying.pos.y, lake_surface, flying.collision_height.0) > 299.0,
            "the ungated settle's drop is the whole altitude — that was the yank"
        );

        flying.modes.levitating = false;
        assert_eq!(rest_line(&flying, Some(lake_surface)), Some(lake_surface));

        let swimmer = player_at(lake_surface - 2.0);
        assert_eq!(
            rest_line(&swimmer, None),
            Some(swimmer.pos.y),
            "a chunk-seam miss must stay a Some, or it is indistinguishable from GM flight"
        );
    }

    #[test]
    fn every_race_floats_with_its_head_out_of_the_water() {
        for (label, h) in [
            ("human male", HUMAN_MALE),
            ("gnome female", GNOME_FEMALE),
            ("night elf male", NIGHT_ELF_MALE),
        ] {
            let submerged = rest_cap(h);
            assert!(
                submerged < h,
                "{label}: rest line {submerged} is at or below the top of a {h}-yd body"
            );
            assert!(((h - submerged) / h - 0.25).abs() < 1e-6, "{label}");
        }

        // One shared height would put a gnome's rest line above her head.
        let pre_0645 = rest_cap(crate::player::DEFAULT_COLLISION_HEIGHT);
        assert!(
            pre_0645 > GNOME_FEMALE,
            "the old constant rest line ({pre_0645}) must sit above a {GNOME_FEMALE}-yd gnome — \
             that was the bug, and if this ever fails the test below is no longer testing it"
        );
        assert!(
            pre_0645 - GNOME_FEMALE > 0.3,
            "…by a third of a yard of water over her head, not a rounding error"
        );
    }

    /// `SLOPE` is Felwood's Felfire Hill channel, as benilla-world's
    /// `liquid::real_data::the_felfire_channel_falls_about_a_tenth_of_a_yard_per_yard` measures it.
    #[test]
    fn a_descending_surface_does_not_flap_the_swim_latch() {
        const SLOPE: f32 = 0.099;
        const DT: f32 = 1.0 / 60.0;
        const H: f32 = HUMAN_MALE;
        let cap = rest_cap(H);
        let surface_after = |secs: f32| 100.0 - SLOPE * SWIM_SPEED * secs;

        // Frozen feet: the water leaves them behind.
        let mut frozen = player_of_height(surface_after(0.0) - cap, H);
        frozen.swimming = true;
        let mut left_at = None;
        for i in 0..600 {
            let t = i as f32 * DT;
            if !update_swimming(&mut frozen, Some(surface_after(t)), t) {
                left_at = Some(t);
                break;
            }
        }
        let left_at = left_at.expect("frozen feet cannot hold the latch on a slope");
        assert!(
            left_at < 0.1,
            "the whole 1/36-yd band is spent in {left_at:.3}s of downhill swimming"
        );

        // Settled after each stroke, on the surface where the stroke landed, as `swim_step` does.
        let mut held = player_of_height(surface_after(0.0) - cap, H);
        held.swimming = true;
        for i in 0..600 {
            let t = i as f32 * DT;
            let surface = surface_after(t);
            held.pos.y -= settle_to_rest(held.pos.y, surface, H);
            assert!(
                update_swimming(&mut held, Some(surface), t),
                "left swim at frame {i} ({t:.2}s)"
            );
            assert!(
                (surface - held.pos.y - cap).abs() < 1e-4,
                "frame {i}: depth {} is off the rest line",
                surface - held.pos.y
            );
        }
    }

    #[test]
    fn the_rest_line_redirects_the_stroke_level_at_full_speed() {
        let free = Vec3::new(0.1, 3.0, 0.2);
        assert_eq!(cap_redirect(free, 100.0), (free, None));
        assert_eq!(cap_redirect(free, f32::INFINITY), (free, None));
        let dive = Vec3::new(1.0, -2.0, 0.0);
        assert_eq!(cap_redirect(dive, 0.0), (dive, None));

        let steep = Vec3::new(0.08, 4.72, 0.0); // ~89° aim, forward at swim speed
        let (vel, pitch) = cap_redirect(steep, 0.0);
        assert!((vel.length() - steep.length()).abs() < 1e-4, "speed kept");
        assert_eq!(vel.y, 0.0, "level at the line");
        assert!(vel.x > 4.7, "the whole speed went forward");
        assert_eq!(pitch, Some(0.0), "the presented pitch levels out");

        // A partial cap, approaching the line.
        let (vel2, pitch2) = cap_redirect(steep, 2.0);
        assert!((vel2.length() - steep.length()).abs() < 1e-4);
        assert_eq!(vel2.y, 2.0);
        let aim = steep.y.atan2(steep.x);
        let eased = pitch2.expect("the cap bit");
        assert!(eased > 0.0 && eased < aim, "between level and the raw aim");
    }
}
