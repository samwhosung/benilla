//! `WOW_FX_CENSUS=1` — where this frame's particle draws are actually addressed, and whether the
//! view they name is switched on.
//!
//! Built to answer "the login screen's braziers are simulating but nothing is on screen." A draw
//! record names the **main-world camera entity** whose view it belongs to
//! ([`benilla_world::particles::buffer::EffectDraw::cam`]), and `particles::sim` resolves that for a booth-layered
//! emitter by finding the first booth camera whose `RenderLayers` intersect the emitter's. If two
//! booths share a layer, the wrong camera can win — and if that camera is inactive, the emitter
//! simulates forever and never draws. Nothing about that is visible in a frame time, a trace, or
//! the emitter's own state: the particles are all *correct*, just addressed to a view nobody is
//! rendering. The census makes it a line of output.
//!
//! Prints every 2 s: frames, live vertex count, how many DISTINCT vertex-buffer states were seen
//! (the liveness check — a frozen sim repeats one state), and the per-camera draw histogram with
//! each camera's booth token and active flag.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::time::Real;

use crate::portrait::BoothCam;
use benilla_world::particles::buffer::EffectQuads;

/// Off unless `WOW_FX_CENSUS=1` — the plugin adds no system at all otherwise.
pub(crate) fn plugin(app: &mut App) {
    if std::env::var("WOW_FX_CENSUS").as_deref() != Ok("1") {
        return;
    }
    app.add_systems(Update, census);
}

fn census(
    cams: Query<(Entity, &Camera, Option<&BoothCam>)>,
    quads: Option<Res<EffectQuads>>,
    time: Res<Time<Real>>,
    mut last: Local<f32>,
    mut states: Local<HashSet<u64>>,
    mut frames: Local<u32>,
) {
    use std::hash::{Hash, Hasher};
    let Some(quads) = quads else { return };
    *frames += 1;
    // A cheap fingerprint of the head of the vertex buffer: enough to tell "this frame differs
    // from the last" without hashing megabytes.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    quads.verts.len().hash(&mut h);
    for v in quads.verts.iter().take(600) {
        bytemuck::bytes_of(v).hash(&mut h);
    }
    states.insert(h.finish());

    if time.elapsed_secs() - *last < 2.0 {
        return;
    }
    *last = time.elapsed_secs();

    let mut per_cam: HashMap<Entity, usize> = HashMap::new();
    // How many of this frame's draws take the SCENE-LIT pipeline arm. Only ~5% of the emitter
    // corpus clears the unlit file bit, so "the lit variant compiles" and "the lit variant ever
    // runs" are entirely different claims — and a scene with no lit emitter in it cannot tell
    // them apart. This counter is what makes the second one checkable.
    let mut lit_draws = 0usize;
    for d in &quads.draws {
        *per_cam.entry(d.cam).or_default() += 1;
        lit_draws += usize::from(d.lit);
    }
    let mut rows: Vec<String> = per_cam
        .iter()
        .map(|(e, n)| {
            let (token, active) =
                cams.get(*e)
                    .map_or(("<despawned>".into(), false), |(_, c, b)| {
                        (
                            b.map_or_else(|| "world".to_string(), |b| format!("booth:{}", b.0)),
                            c.is_active,
                        )
                    });
            format!(
                "{n} → {token}{}",
                if active {
                    " [ACTIVE]"
                } else {
                    " [OFF — these draws go nowhere]"
                }
            )
        })
        .collect();
    rows.sort();
    info!(
        "[fx-census] {} frames · {} verts · {} distinct buffer states · {} lit · draws: {}",
        *frames,
        quads.verts.len(),
        states.len(),
        lit_draws,
        rows.join(", ")
    );
    *frames = 0;
    states.clear();
}
