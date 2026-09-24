//! Read-only censuses of the live precip field, logged at 1 Hz: [`census`] reads it horizontally
//! (is there weather ahead?), [`column`] and [`profile`] vertically (where does it stop overhead,
//! and how abruptly?).

use bevy::math::Vec3;

use super::pool::Drop;

/// The eye band a census counts: ±3 yd of the camera's height.
const CENSUS_BAND: f32 = 3.0;
/// The near-field radius a census counts: the flakes that dominate the look.
const CENSUS_NEAR: f32 = 15.0;

/// A one-line spatial census of a live drop field relative to the camera; `axis` is the motion
/// heading while moving, the view heading while standing. Read `fwd`/`bwd` first (~50/50 on a
/// centred field): the centroid is biased by the world-fixed drift and long-lived particles.
/// Below 60 fps emission scales with frame rate, so two lines compare only at equal `frames`.
pub(super) fn census(
    drops: &[Drop],
    cam: Vec3,
    axis: Vec3,
    speed: f32,
    frames: u32,
) -> Option<String> {
    if drops.is_empty() {
        return None;
    }
    let (mut sum, mut near, mut fwd, mut bwd) = (Vec3::ZERO, 0usize, 0usize, 0usize);
    for d in drops {
        let rel = d.pos - cam;
        sum += rel;
        if rel.length_squared() <= CENSUS_NEAR * CENSUS_NEAR {
            near += 1;
        }
        if rel.y.abs() <= CENSUS_BAND {
            if rel.dot(axis) > 0.0 {
                fwd += 1;
            } else {
                bwd += 1;
            }
        }
    }
    let along = (sum / drops.len() as f32).dot(axis);
    Some(format!(
        "{} live, eye-band fwd {fwd} / bwd {bwd}, near({CENSUS_NEAR:.0} yd) {near}, \
         centroid {along:+.1} yd along heading (moving {speed:.1} yd/s, {frames} fps)",
        drops.len(),
    ))
}

/// The profile's step in yards: fine enough to resolve the 1 s fade-in (~4 yd of fall).
const CENSUS_TOP_STEP: f32 = 2.0;
/// 10 steps reach 20 yd below the ceiling, past the fastest flake's 6.5 yd fade.
const CENSUS_TOP_BANDS: usize = 10;

/// The vertical profile of a live field, top-down from its own ceiling, in alpha-weighted flakes
/// per yard of height: it tells a faded top edge from a cut one. Alpha-weighted because every
/// flake is born on the slab's plane, so a raw count always reads a cut; the eye sees the 1 s
/// fade-in (`snowpoint.bls`). `fade_in` is 0 for a kind with no vertex alpha (rain).
pub(super) struct Column {
    /// The highest particle's height above the eye.
    pub(super) top: f32,
    /// Density per yard in [`CENSUS_TOP_STEP`]-yd steps down from [`Column::top`].
    pub(super) steps: [f32; CENSUS_TOP_BANDS],
    /// Density per yard through the eye band, which the steps climb toward.
    pub(super) plateau: f32,
}

/// The measurement behind [`profile`].
pub(super) fn column(drops: &[Drop], cam: Vec3, fade_in: f32) -> Option<Column> {
    let top = drops
        .iter()
        .map(|d| d.pos.y - cam.y)
        .fold(f32::NEG_INFINITY, f32::max);
    if !top.is_finite() {
        return None;
    }
    let mut steps = [0.0f32; CENSUS_TOP_BANDS];
    let mut plateau = 0.0f32;
    for d in drops {
        let rel = d.pos.y - cam.y;
        let alpha = if fade_in > 0.0 {
            (d.age / fade_in).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // `top` is the maximum of the same values, so the index is never negative.
        if let Some(b) = steps.get_mut(((top - rel) / CENSUS_TOP_STEP) as usize) {
            *b += alpha;
        }
        if rel.abs() <= CENSUS_BAND {
            plateau += alpha;
        }
    }
    for s in &mut steps {
        *s /= CENSUS_TOP_STEP;
    }
    Some(Column {
        top,
        steps,
        plateau: plateau / (2.0 * CENSUS_BAND),
    })
}

pub(super) fn profile(drops: &[Drop], cam: Vec3, fade_in: f32) -> Option<String> {
    let c = column(drops, cam, fade_in)?;
    let steps = c
        .steps
        .iter()
        .map(|s| format!("{s:.0}"))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!(
        "ceiling {:+.1} yd; α-weighted flakes/yd in {CENSUS_TOP_STEP:.0}-yd steps below it: \
         {steps}; eye-band plateau {:.0}/yd",
        c.top, c.plateau,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather::precip::{SNOW_FADE_IN, SNOW_VZ_BASE, SNOW_VZ_W};

    /// `n` flakes spread evenly from a ceiling at eye + 30 to ground 10 below, each aged by its
    /// fall at `vz`: what a constant emission onto a flat spawn plane produces.
    fn steady_column(n: usize, vz: f32) -> Vec<Drop> {
        (0..n)
            .map(|i| {
                let fallen = 40.0 * i as f32 / n as f32;
                Drop {
                    pos: Vec3::new(0.0, 30.0 - fallen, 0.0),
                    vel: Vec3::NEG_Y * vz,
                    land_y: -10.0,
                    cell: (0, 0),
                    age: fallen / vz,
                }
            })
            .collect()
    }

    /// The ceiling is a flat plane either way; only the 1 s fade-in can soften it.
    #[test]
    fn the_column_profile_separates_a_faded_edge_from_a_cut_one() {
        // Wire grade 0.6 through the knee `max(0, (g − 0.25)·4/3)` (`0x67bcc8`), at the base
        // fall speed only: one speed for every flake keeps the alpha profile analytic.
        let vz = SNOW_VZ_BASE + SNOW_VZ_W * ((0.6 - 0.25) * (4.0 / 3.0));
        let drops = steady_column(8000, vz);

        let cut = column(&drops, Vec3::ZERO, 0.0).expect("a populated field");
        assert!((cut.top - 30.0).abs() < 0.02, "ceiling {}", cut.top);
        assert!(
            (cut.steps[0] / cut.plateau - 1.0).abs() < 0.02,
            "an unfaded field is a cut: top step {} vs plateau {}",
            cut.steps[0],
            cut.plateau
        );

        let faded = column(&drops, Vec3::ZERO, SNOW_FADE_IN).expect("a populated field");
        // Mean alpha over the top 2 yd is `1/vz`, a quarter of full at this grade.
        let edge = faded.steps[0] / faded.plateau;
        assert!(
            (edge - 1.0 / vz).abs() < 0.05,
            "a faded edge reads ~{:.2} of the plateau, got {edge:.2}",
            1.0 / vz
        );
        // …and climbs monotonically into the plateau within the fade's own reach.
        for w in faded.steps.windows(2) {
            assert!(w[1] >= w[0] - 1e-3, "profile dips: {:?}", faded.steps);
        }
        assert!(
            faded.steps[2] > 0.98 * faded.plateau,
            "past the fade the column is at plateau: {:?} vs {}",
            faded.steps,
            faded.plateau
        );
    }
}
