//! `WOW_FX_CENSUS=1`: which camera each particle draw is addressed to, and whether that camera
//! is active. A booth emitter takes the first booth camera sharing its `RenderLayers`, so two
//! booths on one layer can route draws to an inactive view. Prints every 2 s: frames, vertex
//! count, distinct vertex-buffer states (a frozen sim repeats one) and the per-camera histogram.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::time::Real;

use crate::portrait::BoothCam;
use benilla_world::particles::buffer::EffectQuads;

/// Adds no system unless `WOW_FX_CENSUS=1`.
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
    // Fingerprints only the head of the vertex buffer: enough to tell frames apart.
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
    // Draws on the scene-lit pipeline arm: only ~5% of emitters clear the unlit bit.
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
