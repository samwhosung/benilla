//! The liquid spatial index for per-draw consumers: an XY grid hash, one bucket per [`CELL`]
//! holding every surface whose wet-footprint box overlaps it, so a point query returns a superset
//! the caller's own tests narrow to exactly the full walk's answer. Rebuilt only when surfaces
//! stream in or out; a stale entry self-filters at the consumer's `Query::get`.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use super::query::WaterChunkInfo;

/// Cell pitch in yards: one MCNK (100/3).
const CELL: f32 = 100.0 / 3.0;

/// The XY grid hash over every loaded liquid surface.
#[derive(Resource, Default)]
pub(crate) struct WaterIndex {
    cells: HashMap<[i32; 2], Vec<Entity>>,
}

impl WaterIndex {
    /// The cell containing a WoW-space XY.
    fn cell_of(x: f32, y: f32) -> [i32; 2] {
        [(x / CELL).floor() as i32, (y / CELL).floor() as i32]
    }

    /// The surfaces whose box overlaps this WoW XY's cell: a superset the caller's tests narrow.
    pub(crate) fn over(&self, x: f32, y: f32) -> &[Entity] {
        self.cells
            .get(&Self::cell_of(x, y))
            .map_or(&[], Vec::as_slice)
    }

    /// The surfaces in any cell the WoW box `[lo, hi]` touches, deduplicated: a superset, as
    /// [`Self::over`].
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

/// Rebuild [`WaterIndex`] in full when a surface streams in or out.
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

    /// A flat all-wet `cols × rows` grid at `z`, vertex `(0, 0)` at `(x0, y0)`, 10 yd pitch.
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

    /// The index answers as the full walk does: inside a footprint, at its edge, on dry land.
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
