//! The liquid **spatial index** — which surfaces are over an XY, without walking them all.
//!
//! [`super::surfaces_at`] answers "what liquid is at this XY" by testing every loaded
//! [`WaterChunkInfo`] in turn. That is fine for the consumers that ask once or twice a frame
//! (the camera waterline, the submersion verdict, a footstep) — and it detonates the moment a
//! consumer asks per *draw*: the water-plane interleave's mesh lane classified ~13k
//! transparent batches against ~2.2k loaded surfaces every frame — 29 M box tests, 54 ms, the
//! 2026-08-03 "60 → 12 fps" regression, measured at the Stormwind pin by the live FPS probe.
//!
//! The index is a plain XY grid hash: one bucket per [`CELL`]-sized cell, each holding every
//! surface whose wet-footprint box overlaps that cell. A point query is one hash lookup, and the
//! hot path re-uses the exact same predicates ([`WaterChunkInfo::contains`] et al.) on the
//! handful of candidates it returns — the verdict set is identical to the full walk by
//! construction, because a surface is registered in every cell its box overlaps.
//!
//! Membership only changes when surfaces stream in or out with their tiles, so the rebuild is
//! edge-triggered (`Added` / `RemovedComponents`) and costs nothing on a steady frame. A stale
//! entry between despawn and rebuild self-filters at the consumer (`Query::get` on a dead entity
//! misses), and a surface spawned this frame is indexed before the next frame's consumers run.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use super::query::WaterChunkInfo;

/// Cell pitch, yards: one MCNK (100/3). An ADT surface's wet footprint fits one-to-few cells and
/// a WMO pool's larger box lands in a handful (Stormwind's biggest MLIQ box, 95 × 80 yd, spans
/// ~12) — buckets stay short, and a dry-land query misses the map entirely.
const CELL: f32 = 100.0 / 3.0;

/// The XY grid hash over every loaded liquid surface (module docs). Read via [`Self::over`];
/// maintained by [`maintain_water_index`].
#[derive(Resource, Default)]
pub(crate) struct WaterIndex {
    cells: HashMap<[i32; 2], Vec<Entity>>,
}

impl WaterIndex {
    /// The cell containing a WoW-space XY.
    fn cell_of(x: f32, y: f32) -> [i32; 2] {
        [(x / CELL).floor() as i32, (y / CELL).floor() as i32]
    }

    /// Every surface whose wet-footprint box overlaps the cell containing this WoW-space XY — the
    /// candidate set for [`super::surfaces_at`], a superset of the surfaces actually containing
    /// the point (the caller's own `contains`/`answers` tests still decide).
    pub(crate) fn over(&self, x: f32, y: f32) -> &[Entity] {
        self.cells
            .get(&Self::cell_of(x, y))
            .map_or(&[], Vec::as_slice)
    }

    /// Every surface registered in any cell the WoW-space box `[lo, hi]` touches, deduplicated —
    /// the candidate set for a small box query (the foam patch builder), with [`Self::over`]'s
    /// same superset guarantee: a surface overlapping the box overlaps one of its cells, and a
    /// surface is registered in every cell its footprint overlaps. Boxes here are a few yards
    /// (1–4 cells), buckets are short — the linear dedup is the cheap shape.
    pub(crate) fn over_box(&self, lo: [f32; 2], hi: [f32; 2]) -> Vec<Entity> {
        let [x0, y0] = Self::cell_of(lo[0], lo[1]);
        let [x1, y1] = Self::cell_of(hi[0], hi[1]);
        let mut out = Vec::new();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                for &e in self.cells.get(&[cx, cy]).map_or(&[][..], Vec::as_slice) {
                    if !out.contains(&e) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }
}

/// Rebuild [`WaterIndex`] when the surface population changed — tile stream edges only. A full
/// rebuild over ~2k surfaces is microseconds, so incremental bookkeeping would be complexity
/// with nothing to buy.
pub(crate) fn maintain_water_index(
    mut index: ResMut<WaterIndex>,
    added: Query<(), Added<WaterChunkInfo>>,
    mut removed: RemovedComponents<WaterChunkInfo>,
    chunks: Query<(Entity, &WaterChunkInfo)>,
) {
    if removed.read().next().is_none() && added.is_empty() {
        return;
    }
    index.cells.clear();
    for (entity, chunk) in &chunks {
        let Some([lo, hi]) = chunk.xy_bounds() else {
            continue; // an empty grid claims no area
        };
        let [x0, y0] = WaterIndex::cell_of(lo[0], lo[1]);
        let [x1, y1] = WaterIndex::cell_of(hi[0], hi[1]);
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                index.cells.entry([cx, cy]).or_default().push(entity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::query::{LiquidClaim, LiquidSource};
    use super::*;
    use crate::liquid::surfaces_at;
    use benilla_formats::LiquidKind;

    /// A flat all-wet `cols × rows` grid at height `z` with vertex `(0,0)` at `(x0, y0)`,
    /// 10 yd pitch.
    fn chunk(x0: f32, y0: f32, z: f32, cols: usize, rows: usize) -> WaterChunkInfo {
        let positions = (0..rows)
            .flat_map(|j| (0..cols).map(move |i| [x0 + 10.0 * i as f32, y0 + 10.0 * j as f32, z]))
            .collect();
        WaterChunkInfo::new(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            [cols, rows],
            positions,
            vec![true; (cols - 1) * (rows - 1)],
        )
    }

    /// An app with just the maintainer, so `Added`/`RemovedComponents` drive it as they do live.
    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<WaterIndex>()
            .add_systems(Update, maintain_water_index);
        app
    }

    /// The index answers exactly like the full walk — same surfaces, same heights — from inside
    /// a footprint, at its edge, and from dry land; and a surface spanning many cells is found
    /// from every one of them.
    #[test]
    fn indexed_candidates_match_the_full_walk() {
        let mut app = app();
        // One small pool and one large one overlapping it, both crossing CELL boundaries.
        app.world_mut().spawn(chunk(-20.0, -20.0, 5.0, 3, 3));
        app.world_mut().spawn(chunk(-50.0, -50.0, 8.0, 12, 12));
        app.update();
        let world = app.world_mut();
        let index = world.remove_resource::<WaterIndex>().unwrap();
        let mut chunks = world.query::<&WaterChunkInfo>();
        for x in (-60..=70).step_by(7) {
            for y in (-60..=70).step_by(7) {
                let wow = [x as f32, y as f32, 0.0];
                let mut walk: Vec<f32> =
                    surfaces_at(chunks.iter(world), wow, LiquidClaim::Outdoors).collect();
                let candidates: Vec<&WaterChunkInfo> = index
                    .over(wow[0], wow[1])
                    .iter()
                    .filter_map(|&e| chunks.get(world, e).ok())
                    .collect();
                let mut indexed: Vec<f32> =
                    surfaces_at(candidates.into_iter(), wow, LiquidClaim::Outdoors).collect();
                walk.sort_by(f32::total_cmp);
                indexed.sort_by(f32::total_cmp);
                assert_eq!(walk, indexed, "divergence at ({x}, {y})");
            }
        }
    }

    /// Despawning the last surface empties the index on the next pass — the stream-out edge.
    #[test]
    fn a_despawned_surface_leaves_the_index() {
        let mut app = app();
        let e = app.world_mut().spawn(chunk(0.0, 0.0, 5.0, 3, 3)).id();
        app.update();
        assert!(!app
            .world()
            .resource::<WaterIndex>()
            .over(5.0, 5.0)
            .is_empty());
        app.world_mut().entity_mut(e).despawn();
        app.update();
        assert!(app
            .world()
            .resource::<WaterIndex>()
            .over(5.0, 5.0)
            .is_empty());
    }
}
