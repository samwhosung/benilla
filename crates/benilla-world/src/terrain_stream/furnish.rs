//! Paced tile furnishing: cell meshes are built from a landed tile's decoded chunks
//! ([`chunks_to_mesh`]) a bounded number per frame while live, nearest tile first, so crossing a
//! tile line never uploads a whole row in one frame. While the body is not live (entry, teleport)
//! the cap is off: the loading screen absorbs the burst and waits on furnished tiles.

use std::time::Instant;

use benilla_assets::{chunks_to_mesh, AdtTile};
use bevy::camera::primitives::MeshAabb;
use bevy::prelude::*;

use super::TerrainStreamer;

/// Chunks furnished per frame while live (four cells): a chunk's mesh costs ~70 µs from asset add
/// to upload, so ~4.5 ms a frame.
const CELL_SPAWN_CAP: usize = 64;

/// A tile's 16×16 chunks furnish as 4×4 cells of 4×4 chunks, one mesh and one draw each: every
/// per-chunk fact is baked per vertex and the material is the tile's. The 133⅓-yd cell matches
/// `static_gx` and the merge lanes.
const CELL_SPAN: usize = 4;
const CELLS_PER_TILE: usize = (16 / CELL_SPAN) * (16 / CELL_SPAN);
const CHUNKS_PER_CELL: usize = CELL_SPAN * CELL_SPAN;

/// Spawns the cell entities of unfurnished tiles, nearest the focus first, paced while live.
/// Chained after `stream_terrain` and before `spawn_loaded_placements`.
pub(super) fn furnish_tile_cells(
    mut commands: Commands,
    mut state: ResMut<TerrainStreamer>,
    tiles: Res<Assets<AdtTile>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut activity: ResMut<crate::terrain_stream::StreamActivity>,
    focus: Res<crate::terrain_stream::ViewFocus>,
) {
    let t0 = Instant::now();
    let live = focus.paced;
    // The cap was measured in chunks; a cell is sixteen of them.
    let cap = if live {
        CELL_SPAWN_CAP / CHUNKS_PER_CELL
    } else {
        usize::MAX
    };
    // `WOW_NO_TILE_CELLS=1`: tiles stream without cell entities, but still mark furnished so the
    // loading screen converges.
    let ablated = tile_cells_disabled();

    let focus = state.focus;
    let mut pending: Vec<(i32, (i32, i32))> = state
        .tiles
        .iter()
        .filter(|(_, t)| t.entity.is_some() && !t.furnished)
        .map(|(&(tx, ty), _)| ((tx - focus.0).abs().max((ty - focus.1).abs()), (tx, ty)))
        .collect();
    if pending.is_empty() {
        return;
    }
    pending.sort_unstable();

    let mut built = 0usize;
    'tiles: for (_, key) in pending {
        let Some(tile) = state.tiles.get_mut(&key) else {
            continue;
        };
        let (Some(root), Some(material)) = (tile.entity, tile.material.clone()) else {
            continue; // unreachable given the filter above, but never worth a panic
        };
        // Resident for the tile's life; missing only mid-shutdown, which leaves it unfurnished.
        let Some(adt) = tiles.get(&tile.handle) else {
            continue;
        };
        while tile.next_cell < CELLS_PER_TILE {
            if built >= cap {
                break 'tiles;
            }
            let cell = tile.next_cell;
            tile.next_cell += 1;
            if ablated {
                continue;
            }
            // The cell's chunks: MCIN order is row-major, index = row·16 + column.
            let (cy, cx) = (cell / (16 / CELL_SPAN), cell % (16 / CELL_SPAN));
            let parts: Vec<_> = (0..CELL_SPAN)
                .flat_map(|dy| {
                    (0..CELL_SPAN).map(move |dx| (cy * CELL_SPAN + dy) * 16 + cx * CELL_SPAN + dx)
                })
                .filter_map(|i| Some((adt.chunks.get(i)?, adt.shading.get(i)?)))
                .collect();
            // A hole-emptied cell yields no mesh and does not count.
            let Some(mesh) = chunks_to_mesh(&parts) else {
                continue;
            };
            // The Aabb is computed here: the mesh is `RENDER_WORLD`-only, so `calculate_bounds`
            // may find it already extracted, and the exterior cull fails open on a missing Aabb.
            let aabb = mesh.compute_aabb();
            let handle = meshes.add(mesh);
            commands.entity(root).with_children(|cells| {
                let mut cell = cells.spawn((
                    Mesh3d(handle),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY, // cell meshes are in absolute world coords
                    // Exterior scene: from inside a WMO a cell draws only through a portal window
                    // (`0x683bf0`, fed only by the per-window walk `0x682fa0`).
                    crate::exterior_cull::ExteriorScene,
                ));
                if let Some(aabb) = aabb {
                    cell.insert(aabb);
                }
            });
            built += 1;
        }
        tile.furnished = true;
    }
    activity.cells_spawned += built as u32;
    activity.furnish_ms += t0.elapsed().as_secs_f32() * 1000.0;
}

/// `WOW_NO_TILE_CELLS=1` furnishes no cell entities (a dev measurement switch).
fn tile_cells_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_TILE_CELLS").is_some())
}

/// A tile is `furnished` only once its cursor has walked every cell, never when its root exists.
#[cfg(test)]
mod tests {
    /// A fresh 5-tile row (5 × 256 chunks) must furnish within half a second at 60 Hz.
    #[test]
    fn a_fresh_row_furnishes_in_under_half_a_second() {
        let row_cells: usize = 5 * 256;
        let frames = row_cells.div_ceil(super::CELL_SPAWN_CAP);
        assert!(
            frames <= 30,
            "a 5-tile row must land within ~half a second at 60 Hz, got {frames} frames"
        );
    }
}
