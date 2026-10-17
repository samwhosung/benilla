//! The world query gate ahead of the player's resolvers. Each substep the reference grows a swept
//! box (`0x633840`) and gathers the world inside it; when that query fails, as it does when a
//! terrain chunk under the box is not loaded (`0x6aa8b0`), the substep loop reports the time
//! skipped and resolves nothing (`0x634154 je 0x6342a3`). Where the WDT authors no tile, the
//! world's edge, that holds the body like a wall, and a body already falling hangs with FALLING
//! kept. Only the terrain leg is built ([`benilla_world::terrain_stream::TerrainResidency`]).

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use super::{CAPSULE_RADIUS, STEP_UP_HEIGHT};

/// tan 50° (`0x3f988b62`), the box's slope allowance.
const TAN_50: f32 = 1.191_753_6;
/// The skin every face of the box moves out by (`0x3ab60b61`, 1/720, applied at `0x633b14`).
const SKIN: f32 = 1.0 / 720.0;
/// Below this travel the direction is zero (`0x35800000`, 2^-20: `0x6340af fcomp [0x8026bc]`, `0x6340ba jnp`).
const MIN_TRAVEL: f32 = 1.0 / 1_048_576.0;

/// Which growth rule the swept box takes, the first matching mode of `0x633840`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Sweep {
    /// FALLING (0x2000), with the arc's further drop this step (yd, down positive).
    Fall { drop: f32 },
    /// SWIMMING (0x200000): the travel keeps its vertical.
    Swim,
    /// Anything else: the walking resolver's box.
    Walk,
}

/// The swept box's x and y extent in WoW space for a body with feet at `feet` (Bevy) moving by
/// `travel` (Bevy, this step's displacement), as `0x633840` grows it ("Swept box growth"). Only x
/// and y are built: the terrain gate reads no z (`0x6aa930`–`0x6aa964`).
pub(super) fn swept_xy(feet: Vec3, travel: Vec3, sweep: Sweep) -> ([f32; 2], [f32; 2]) {
    let p = bevy_to_wow(feet);
    let d = bevy_to_wow(travel);
    let r = CAPSULE_RADIUS;
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let u = if dist < MIN_TRAVEL {
        [0.0; 3]
    } else {
        [d[0] / dist, d[1] / dist, d[2] / dist]
    };
    let mut lo = [p[0] - r, p[1] - r];
    let mut hi = [p[0] + r, p[1] + r];
    // The union with the box moved by `m`.
    let sweep_by = |lo: &mut [f32; 2], hi: &mut [f32; 2], m: [f32; 2]| {
        for i in 0..2 {
            lo[i] = lo[i].min(lo[i] + m[i]);
            hi[i] = hi[i].max(hi[i] + m[i]);
        }
    };
    // The union with the square of half-width `w` around `c`.
    let cover = |lo: &mut [f32; 2], hi: &mut [f32; 2], c: [f32; 2], w: f32| {
        for i in 0..2 {
            lo[i] = lo[i].min(c[i] - w);
            hi[i] = hi[i].max(c[i] + w);
        }
    };
    match sweep {
        Sweep::Fall { drop } => {
            // `m = u·dist`, `m.z −= f`; a descent widens x and y by `−tan 50° · m.z` (`0x633ac4`).
            sweep_by(&mut lo, &mut hi, [u[0] * dist, u[1] * dist]);
            let t = -TAN_50 * (u[2] * dist - drop);
            if t > 0.0 {
                for i in 0..2 {
                    lo[i] -= t;
                    hi[i] += t;
                }
            }
        }
        Sweep::Swim => {
            // The cube of half-width `dist/2` around the half-way point, widened by `r·√2`.
            let s = dist * 0.5;
            let c = [p[0] + u[0] * s, p[1] + u[1] * s];
            cover(&mut lo, &mut hi, c, s + r * std::f32::consts::SQRT_2);
        }
        Sweep::Walk => {
            // The box moved ahead by the step allowance and the travel (`0x633c97`), then the
            // `r·√2 + dist/2` square around the half-way point (`0x633dab`).
            let g = (STEP_UP_HEIGHT * TAN_50).max(r + SKIN);
            sweep_by(&mut lo, &mut hi, [u[0] * (g + dist), u[1] * (g + dist)]);
            let half = dist * 0.5;
            let c = [p[0] + u[0] * half, p[1] + u[1] * half];
            cover(&mut lo, &mut hi, c, r * std::f32::consts::SQRT_2 + half);
        }
    }
    for i in 0..2 {
        lo[i] -= SKIN;
        hi[i] += SKIN;
    }
    (lo, hi)
}

/// Whether the step may resolve: the world query over its swept box would complete.
pub(super) fn may_resolve(
    world: &benilla_world::collision::WorldCollision<'_, '_>,
    feet: Vec3,
    travel: Vec3,
    sweep: Sweep,
) -> bool {
    let (lo, hi) = swept_xy(feet, travel, sweep);
    world.terrain_resident(lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::coords::wow_to_bevy;

    /// Feet at the WoW origin, travel `d` in WoW axes.
    fn wow_box(d: [f32; 3], sweep: Sweep) -> ([f32; 2], [f32; 2]) {
        swept_xy(Vec3::ZERO, wow_to_bevy(d), sweep)
    }

    #[test]
    fn a_walk_box_reaches_the_step_allowance_ahead_and_r_root_two_aside() {
        // North (+x) at 7 yd/s for a 60 fps frame.
        let dist = 7.0 / 60.0;
        let (lo, hi) = wow_box([dist, 0.0, 0.0], Sweep::Walk);
        let ahead = CAPSULE_RADIUS + STEP_UP_HEIGHT * TAN_50 + dist + SKIN;
        assert!((hi[0] - ahead).abs() < 1e-5, "ahead {} vs {ahead}", hi[0]);
        // The square around the half-way point reaches `r·√2` behind the feet.
        let behind = CAPSULE_RADIUS * std::f32::consts::SQRT_2 + SKIN;
        assert!((lo[0] + behind).abs() < 1e-5, "behind {}", lo[0]);
        let aside = CAPSULE_RADIUS * std::f32::consts::SQRT_2 + dist * 0.5 + SKIN;
        assert!((hi[1] - aside).abs() < 1e-5 && (lo[1] + aside).abs() < 1e-5);
    }

    #[test]
    fn a_descent_widens_the_fall_box_and_a_rise_does_not() {
        let (lo, hi) = wow_box([0.0; 3], Sweep::Fall { drop: 1.0 });
        let edge = CAPSULE_RADIUS + TAN_50 + SKIN;
        assert!((hi[0] - edge).abs() < 1e-5 && (lo[1] + edge).abs() < 1e-5);
        let (lo, hi) = wow_box([0.0; 3], Sweep::Fall { drop: -1.0 });
        assert!((hi[0] - CAPSULE_RADIUS - SKIN).abs() < 1e-5);
        assert!((lo[1] + CAPSULE_RADIUS + SKIN).abs() < 1e-5);
    }

    #[test]
    fn a_swim_box_covers_the_stroke_widened_by_r_root_two() {
        let (lo, hi) = wow_box([0.0, 2.0, 0.0], Sweep::Swim);
        // s = 1 around (0, 1): y in [1 − 1 − q, 1 + 1 + q].
        let q = CAPSULE_RADIUS * std::f32::consts::SQRT_2;
        assert!((hi[1] - (2.0 + q + SKIN)).abs() < 1e-5);
        assert!(
            (lo[1] + q + SKIN).abs() < 1e-5,
            "the stroke's cube reaches back: {}",
            lo[1]
        );
        assert!((hi[0] - (1.0 + q + SKIN)).abs() < 1e-5);
    }
}
