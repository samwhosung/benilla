//! The particle census ([`ParticleCensusPlugin`]) — one `PARTICLE_CENSUS_EMITTER` line per live
//! emitter plus the `PARTICLE_CENSUS` summary, including the draw-distance accounting that is
//! B39's numeric form.

use bevy::prelude::*;

use super::ProbeClock;

/// The particle census (`WOW_PARTICLE_CENSUS=<secs>`): once, `t` seconds in, print one line per
/// live emitter (blend, file flags, sampled rate keys, texture, live count) plus a machine-
/// readable total — the like-for-like number to put beside a reference-trace quad count (the
/// login whirlpool investigation: the real client draws 793 particle quads across 23 draws in
/// one `UI_MainMenu` frame). Works at any state — the glue screens included, unlike the
/// in-world-gated FPS probe.
///
/// It also measures **draw distance**: each emitter's planar depth along
/// camera-forward — the coordinate the far-clip wall uses — and the draw-set gate's verdict, with
/// `drawn_beyond_wall` on the summary line. That is the numeric form of "effects render at
/// unlimited distance" (bug B39): emitters still ticking and drawing past the wall that has already
/// discarded the terrain beneath them. **It must read 0**; a non-zero value is the bug, live.
///
/// **`WOW_PARTICLE_CENSUS=+<secs>` fires that long after the world is first SHOWN** (the loading
/// screen dropping) instead of after app start. Everything that rides the appear ramp — decision
/// 0827/0833's `alpha` column above all — lives in a 2-second window whose start moves by *seconds*
/// between a warm and a cold load, and a wall-clock timer either lands in it or does not: three
/// runs in a row missed it while the question was "does a weapon glow ramp with its wearer?". A
/// probe should not be a dice roll, and the ramp's own trigger is the thing to time from.
pub(crate) struct ParticleCensusPlugin;

impl Plugin for ParticleCensusPlugin {
    fn build(&self, app: &mut App) {
        let spec = std::env::var("WOW_PARTICLE_CENSUS").unwrap_or_default();
        let after_shown = spec.starts_with('+');
        let at = spec
            .trim_start_matches('+')
            .parse()
            .unwrap_or(if after_shown { 1.0 } else { 10.0 });
        app.insert_resource(ParticleCensus {
            at,
            after_shown,
            fired: false,
        })
        .add_systems(Update, fire_particle_census);
    }
}

/// [`ParticleCensusPlugin`] state: the fire time and the once-latch.
#[derive(Resource)]
struct ParticleCensus {
    at: f32,
    /// `at` is measured from the world being shown, not from app start — and is rewritten into an
    /// absolute time the first frame the loading screen stops covering.
    after_shown: bool,
    fired: bool,
}

fn fire_particle_census(
    mut probe: ResMut<ParticleCensus>,
    screen: Res<crate::loading_screen::LoadingScreen>,
    time: ProbeClock,
    view: Res<benilla_world::view::ViewDistance>,
    cam: Query<&GlobalTransform, With<benilla_world::view::WorldCamera>>,
    emitters: Query<(
        &benilla_world::particles::ParticleEmitter,
        Option<&benilla_world::particles::EmitterFade>,
        Option<&bevy::camera::visibility::RenderLayers>,
    )>,
) {
    // Latch the shown-relative deadline the first frame the screen drops (and only then: while it
    // still covers there is no ramp to be relative to).
    if probe.after_shown {
        if screen.covering() {
            return;
        }
        probe.at += time.elapsed_secs();
        probe.after_shown = false;
    }
    if probe.fired || time.elapsed_secs() < probe.at {
        return;
    }
    probe.fired = true;
    let mut total = 0usize;
    let mut n = 0usize;
    // The B39 columns. Per emitter: its planar depth along camera-forward (the
    // coordinate the far-clip wall is measured in) and the draw-set gate's live verdict.
    //
    // **`drawn_beyond_wall` is the number that names the bug.** It counts emitters the gate is
    // still ticking and drawing at a depth where the detailed world — the terrain under them
    // included — has already been discarded by the wall. Before 0678 it was routinely non-zero,
    // because `doodad_fade_alpha` admits any owner over `NEVER_FADE_RADIUS` at *every* distance
    // and nothing else bounded depth; that is precisely "all effects render at unlimited
    // distance", out past the point where the terrain itself stops drawing.
    // It must now be **0**: past the wall the gate hides the emitter and freezes its pool.
    //
    // `beyond_wall` (verdict ignored) stays as the denominator — emitters *exist* out there and
    // should, they are simply frozen. A fix that despawned them would be the wrong fix.
    //
    // **Booth-layered emitters are excluded from the distance accounting** — the same layer filter
    // `simulate_particles` uses to pick a booth's camera. The portrait/glue scenes are parked
    // thousands of yards from the world and drawn by their OWN camera, so the world camera's wall
    // says nothing about them; counting them read as 28 phantom "effects past the wall" (all of
    // them Karazahn braziers and night-elf glows at ~7080 yd) on a build where the world was
    // already clean. Measuring the right subject is the instrument's job, not the reader's.
    let cam_tf = cam.iter().next();
    let mut beyond_wall = 0usize;
    let mut drawn_beyond_wall = 0usize;
    let mut drawn_beyond_wall_live = 0usize;
    let mut booth = 0usize;
    let mut max_drawn_depth = f32::NEG_INFINITY;
    for (e, fade, layers) in &emitters {
        let world_layer =
            layers.is_none_or(|l| l.intersects(&bevy::camera::visibility::RenderLayers::default()));
        // Depth to the OWNER's fade sphere where there is one (the gate's own subject), else the
        // emitter's live anchor — so the number always names what the gate actually tests.
        let depth = cam_tf.map(|t| {
            let center = fade.map_or_else(|| e.anchor_world(), |f| f.center);
            let radius = fade.map_or(0.0, |f| f.radius);
            (center - t.translation()).dot(Vec3::from(t.forward())) - radius
        });
        let drawn = e.drawn();
        if !world_layer {
            booth += 1;
        }
        if let Some(d) = depth.filter(|_| world_layer) {
            if drawn {
                max_drawn_depth = max_drawn_depth.max(d);
            }
            if d > view.farclip {
                beyond_wall += 1;
                if drawn {
                    drawn_beyond_wall += 1;
                    drawn_beyond_wall_live += e.live();
                }
            }
        }
        let dist = depth
            .map(|d| {
                let lane = if world_layer { "world" } else { "booth" };
                let c = fade.map_or_else(|| e.anchor_world(), |f| f.center);
                format!(
                    // `has_fade`, not `gated`: this column prints whether the emitter carries an
                    // `EmitterFade`, and calling it `gated` made it read as the draw-set verdict —
                    // which is `drawn` (its inverse), right beside it. A tile-emitter reading was
                    // taken backwards off the old label before it was renamed.
                    " depth={d:.1} drawn={drawn} lane={lane} has_fade={} at=({:.0},{:.0},{:.0})",
                    fade.is_some(),
                    c.x,
                    c.y,
                    c.z
                )
            })
            .unwrap_or_default();
        let d = e.def();
        // The rate summary: the constant (the common shape), else each slot's key count — the
        // full per-sequence choreography lives in `benilla-extract m2anim`, not a census line.
        let rate_keys: Vec<String> = match d.timing.constant_rate() {
            Some(r) => vec![format!("{r:.1}")],
            None => d
                .timing
                .slot_views()
                .iter()
                .enumerate()
                .map(|(s, (_, r, _))| format!("s{s}:{}k", r.map_or(0, <[(f32, f32)]>::len)))
                .collect(),
        };
        // The orientation fingerprint (world plane normal + thickness/radius) is the numeric
        // "which way does this cloud face" — the flat-vs-standing question a screenshot can
        // only suggest (the InstancePortal swirl-plane investigation).
        let plane = e
            .cloud_fingerprint()
            .map(|(c, nrm, thick, radius)| {
                format!(
                    " ctr=({:.1},{:.1},{:.1}) normal=({:+.2},{:+.2},{:+.2}) thick={thick:.2} radius={radius:.2}",
                    c.x, c.y, c.z, nrm.x, nrm.y, nrm.z
                )
            })
            .unwrap_or_default();
        println!(
            "PARTICLE_CENSUS_EMITTER blend={:?} flags={:#06x} rate=[{}] life={:.2} tex={} live={} alpha={:.2}{dist}{plane}",
            d.blend,
            d.flags,
            rate_keys.join(","),
            d.params.sample(None, 0.0, 0.0).lifespan,
            d.texture.as_deref().unwrap_or("-"),
            e.live(),
            // The frame's composed MODEL alpha — the number that answers
            // "this cloud is drawing, why can't I see it / why is it full strength?". An effect on
            // a unit that has not appeared yet reads ~0; one with no model above it reads 1.
            e.render_alpha(),
        );
        total += e.live();
        n += 1;
    }
    let max_drawn_depth = if max_drawn_depth.is_finite() {
        max_drawn_depth
    } else {
        0.0
    };
    // The camera pose goes on the line so a census is self-describing: every distance number here
    // is measured from it, and a probe whose `.go` silently failed otherwise reports crisp numbers
    // about the wrong place.
    let where_ = cam_tf
        .map(|t| {
            let p = t.translation();
            format!(" cam=({:.1},{:.1},{:.1})", p.x, p.y, p.z)
        })
        .unwrap_or_else(|| " cam=none".into());
    println!(
        "PARTICLE_CENSUS emitters={n} booth={booth} live_total={total} farclip={:.0} \
         beyond_wall={beyond_wall} drawn_beyond_wall={drawn_beyond_wall} \
         drawn_beyond_wall_live={drawn_beyond_wall_live} max_drawn_depth={max_drawn_depth:.1}{where_}",
        view.farclip,
    );
}
