//! The particle census ([`ParticleCensusPlugin`]): a `PARTICLE_CENSUS_EMITTER` line per live
//! emitter and a `PARTICLE_CENSUS` summary with draw-distance accounting.

use bevy::prelude::*;

use super::ProbeClock;

/// `WOW_PARTICLE_CENSUS=<secs>`: once, prints one line per live emitter and a total, at any state
/// including glue screens (the reference draws 793 quads in 23 draws on one `UI_MainMenu` frame).
/// `drawn_beyond_wall` counts emitters drawn past the far-clip wall and must read 0.
/// `+<secs>` times from the loading screen dropping, so the appear ramp is caught on any load.
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

/// [`ParticleCensusPlugin`] state.
#[derive(Resource)]
struct ParticleCensus {
    at: f32,
    /// `at` counts from the world being shown; made absolute when the loading screen drops.
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
    // Latch the shown-relative deadline the first frame the screen drops.
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
    // Per emitter: planar depth along camera-forward (the far-clip wall's coordinate) and the
    // draw-set gate's verdict. Past the wall the gate hides the emitter and freezes its pool, so
    // `beyond_wall` counts frozen emitters and `drawn_beyond_wall` must be 0. Booth-layered
    // emitters are drawn by their own camera, far from the world, and are left out.
    let cam_tf = cam.iter().next();
    let mut beyond_wall = 0usize;
    let mut drawn_beyond_wall = 0usize;
    let mut drawn_beyond_wall_live = 0usize;
    let mut booth = 0usize;
    let mut max_drawn_depth = f32::NEG_INFINITY;
    for (e, fade, layers) in &emitters {
        let world_layer =
            layers.is_none_or(|l| l.intersects(&bevy::camera::visibility::RenderLayers::default()));
        // Depth to the owner's fade sphere if any, else the anchor: what the gate tests.
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
                    // `has_fade` is whether it carries an `EmitterFade`; the verdict is `drawn`.
                    " depth={d:.1} drawn={drawn} lane={lane} has_fade={} at=({:.0},{:.0},{:.0})",
                    fade.is_some(),
                    c.x,
                    c.y,
                    c.z
                )
            })
            .unwrap_or_default();
        let d = e.def();
        // The constant rate, else each slot's key count (`benilla-extract m2anim` has the rest).
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
        // Which way the cloud faces: world plane normal, thickness and radius.
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
            // The composed model alpha: ~0 on a unit not yet appeared, 1 with no model above.
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
    // Every distance is from this camera; it also shows a failed `.go`.
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
