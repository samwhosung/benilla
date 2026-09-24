//! The precip **instruments** — the read-only field censuses the build loop logs at 1 Hz.
//! Split from [`super::pool`], which owns the sim: nothing here mutates a pool or is read by
//! one, and the two answer different questions — [`census`] reads the field **horizontally**
//! (is there weather in front of me?), [`column`]/[`profile`] read it **vertically** (where
//! does it stop overhead, and how abruptly?).

use bevy::math::Vec3;

use super::pool::Drop;

/// The eye band a census splits on: ±3 yd of the camera's height — the slice a player actually
/// looks *through*, as opposed to the column of sky above them.
const CENSUS_BAND: f32 = 3.0;
/// The near-field radius a census counts: the sphere whose flakes dominate the look (the point
/// sprite is 14 px at the eye and under 5 px past 30 yd).
const CENSUS_NEAR: f32 = 15.0;

/// A one-line **spatial** census of a live drop field, relative to the camera — the instrument
/// for *"can you outrun the weather?"*, which the pool COUNTS alone cannot answer.
///
/// `axis` is a horizontal unit vector: the player's motion heading while they move, their view
/// heading while they stand.
///
/// **`fwd`/`bwd` is the metric — read it first.** It splits the eye band about the plane through
/// the camera ⊥ `axis`, reads ~50/50 on a centred field whatever the heading, and is what actually
/// answers "is there weather in front of me". Measured across the 1159 fix: 3200/3350 standing,
/// collapsing to 0 while running before the spawn-slab tilt and holding ~2115 after it.
///
/// **`centroid` is a weak second, and is easy to misread — it was, once.** Two biases sit on it:
/// the drift heading is a fixed WORLD azimuth, so even a standing field's centroid sits
/// `drift · mean_age` off along it (−5.5 yd at wire grade 0.6, −14 at 1.0, depending on which way
/// the player faces); and it is *count-weighted*, so long-lived particles born high and to the rear
/// dominate it. Under the slab tilt those two effects nearly cancel the forward shift at low grade:
/// a Monte-Carlo of [`super::pool::spawn_particle`] predicts the running centroid improving only 9% at grade 0.6
/// against 49% at 1.0, and the live client measured 11% and 61% — agreeing with the law while
/// looking, on the centroid alone, like the fix had barely worked. Never conclude from it alone.
///
/// `frames` is the count since the previous census, and it is **not decoration**: `run_kind`'s
/// budget is `rate · min(dt, 1/60)` with the remainder dropped, so emission per second scales with
/// frame rate and a sub-60 fps leg measures a genuinely thinner field than a 60 fps one on
/// identical code. Two census lines are only comparable at equal `frames` — measured, the field
/// tracks `fps/60` and nothing else: 1542/2115 = 72.9% and 1598/2134 = 74.9% against frame-rate
/// ratios of 71.8% and 73.0%. (An earlier revision of this line claimed 48%, which was an
/// arithmetic slip on my part; it read as an unexplained anomaly and cost a wasted RE question.)
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

/// The vertical profile's step, in yards — fine enough to resolve the alpha fade-in, which spans
/// only the first `|v_z|` yards of fall (≈3.9 yd at wire grade 0.6).
const CENSUS_TOP_STEP: f32 = 2.0;
/// How far below the ceiling the profile reaches: 10 steps = 20 yd, which brackets the whole
/// approach to the plateau at every grade (the fastest blizzard flake falls 6.5 yd in its fade).
const CENSUS_TOP_BANDS: usize = 10;

/// The **vertical** profile of a live field, top-down from its own ceiling — the instrument for
/// *"where does the snow stop, and how abruptly?"*, which neither the pool counts nor [`census`]'s
/// horizontal split can answer.
///
/// This exists because the director reported the top edge of benilla's snowfall reading as a hard
/// horizontal line where the reference's fades in. That is a claim about `flakes(height)`, so it
/// is measured as `flakes(height)`: the ceiling, the alpha-weighted flakes per yard in 2 yd steps
/// below it, and the plateau the profile is climbing toward.
///
/// **Alpha-weighted, deliberately.** The geometric ceiling is a razor plane by construction —
/// every flake is born at local `z = +30` ([`super::pool::spawn_particle`]) — so a raw count *always* reports a
/// hard edge and can never tell the two clients apart. What the eye sees is softened by the 1 s
/// linear fade-in (`alpha = clamp01(t − f1)`, `snowpoint.bls`), and that fade is
/// the only thing standing between a flat spawn plane and a visible cut. So the weight is the alpha
/// the renderer actually emits, and the profile reads the way the look does: a soft edge climbs to
/// the plateau over several steps, a hard one reaches it in the first.
///
/// `fade_in` is the kind's fade-in duration in seconds; pass `0.0` for a kind that has none (rain's
/// streaks carry no vertex alpha), which weights every particle 1.
///
/// All densities are per **yard of height**, so the steps and the plateau are directly comparable.
pub(super) struct Column {
    /// The highest particle's height above the eye — the field's geometric ceiling.
    pub(super) top: f32,
    /// Alpha-weighted density per yard, in [`CENSUS_TOP_STEP`]-yd steps down from [`Column::top`].
    pub(super) steps: [f32; CENSUS_TOP_BANDS],
    /// Alpha-weighted density per yard through the eye band — what the steps climb toward.
    pub(super) plateau: f32,
}

/// The measurement behind [`profile`], separated so tests read numbers instead of parsing prose.
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

    /// A steady snow column: `n` flakes spread uniformly over the 40 yd from a ceiling at eye + 30
    /// down to ground 10 below, each aged by how far it has already fallen at `vz`. This is the
    /// shape a constant emission rate onto a flat spawn plane actually produces.
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

    /// What the column profile has to be able to do is **tell a faded top edge from a cut one** —
    /// it is the instrument for the director's "hard line" report, and an instrument that reads the
    /// same either way would have closed that report on nothing.
    ///
    /// The geometric ceiling is a razor plane in both cases (every flake is born at local `z = +30`),
    /// so the *only* thing that can soften it is the 1 s fade-in. Weighted by that fade, the top
    /// step reads a quarter of the plateau and the profile climbs into it over the ≈3.9 yd a
    /// grade-0.6 flake falls in its first second; with the fade switched off the identical field
    /// reports its true cut — the first step already at the plateau.
    #[test]
    fn the_column_profile_separates_a_faded_edge_from_a_cut_one() {
        // The director's wire grade 0.6 through the published knee (`0x67bcc8`,
        // `max(0, (g − 0.25)·4/3)` = 0.4667). This is the BASE fall only, without
        // `spawn_particle`'s `+w·rand()` spread — 3.63 yd/s against the live field's ~3.9 mean.
        // One speed for every flake is what keeps the alpha profile analytic here.
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
        // Mean alpha over the top 2 yd is `1/vz` — a quarter of full at this grade.
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
