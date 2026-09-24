//! Paced per-cell tile furnishing — the fix for B181's *first-contact* spike.
//!
//! A landed tile's ~256 MCNK cell meshes used to arrive as loader-built sub-assets: the whole
//! set (and, decodes bunching, usually the whole 5-tile row's ~1300) hit the render world's
//! extract/prepare/upload in ONE frame — 80–105 ms of process CPU at the Emerald Dream pin,
//! 120–240 ms at Undercity, on every first crossing of a tile line. No budget downstream of the
//! loader could spread work the loader had already packaged, so the build moved here: each cell's
//! mesh is made from the decoded [`AdtTile::chunks`] + [`AdtTile::shading`]
//! ([`benilla_assets::chunk_to_mesh`]), a bounded number per frame while live, nearest tile
//! first. Mesh-asset creation is the pacing point; everything downstream follows its rate.
//!
//! While the body is **not** live (entry, teleport, world-stale) the cap is off: the loading
//! screen exists to absorb exactly that burst, and the residency the settle release reads counts
//! *furnished* tiles, so the cover never lifts onto bare ground.

use std::time::Instant;

use benilla_assets::{chunks_to_mesh, AdtTile};
use bevy::camera::primitives::MeshAabb;
use bevy::prelude::*;

use super::TerrainStreamer;

/// Cells built per frame while live. Measured on the B181 carve: a landed cell mesh costs ~70 µs
/// end-to-end (asset add + extract + prepare + upload, dev profile, Metal) — 64 ≈ 4.5 ms of
/// render-side work per frame, and a whole fresh row (5 × 256 cells, two tiles out, in fog)
/// furnishes in ~20 frames. The placement cap (`SPAWN_COUNT_CAP`) is this same idea one lane over.
const CELL_SPAWN_CAP: usize = 64;

/// A tile's 16×16 chunks furnish as 4×4 CELLS of 4×4 chunks each — one mesh, one entity, one
/// draw per cell. A chunk was one draw each, and a city horizon is over a
/// thousand of them a frame; every per-chunk fact is baked per vertex and the material is the
/// tile's, so the cell mesh is the chunks' concatenation. `static_gx` and the merge lanes cut
/// the world into the same 133⅓-yd cell for the same locality reason.
const CELL_SPAN: usize = 4;
const CELLS_PER_TILE: usize = (16 / CELL_SPAN) * (16 / CELL_SPAN);
const CHUNKS_PER_CELL: usize = CELL_SPAN * CELL_SPAN;

/// Build + spawn the cell entities of spawned-but-unfurnished tiles, [`CELL_SPAWN_CAP`] per frame
/// while live, nearest tile to the stream focus first. Chained directly after `stream_terrain`
/// (roots spawned there furnish from the same frame on) and before `spawn_loaded_placements`.
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
    // Ablation switch (`WOW_NO_TILE_CELLS=1`): tiles stream — asset residency, collider,
    // placements, liquid, clutter — without their per-chunk cell entities, isolating the
    // render-entity lane from the asset lane when measuring a streaming cost. The B181 carve ran
    // on it (a crossing's spike survived zero cells, exonerating the entity lane); the
    // `WOW_NO_LIQUID`/`WOW_NO_PARTICLES` family's pattern. Tiles still MARK furnished, so the
    // loading-screen residency (which now waits on furnishing) converges.
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
        // The handle keeps the asset resident for the tile's whole life, so a spawned tile's
        // `AdtTile` is always here; a missing one (asset store torn down mid-shutdown) just
        // leaves the tile unfurnished.
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
            // A hole-emptied cell yields no mesh (and costs nothing — it doesn't count).
            let Some(mesh) = chunks_to_mesh(&parts) else {
                continue;
            };
            // The cell's Aabb, computed HERE and inserted explicitly: the mesh is
            // `RENDER_WORLD`-only, so Bevy's own `calculate_bounds` may find its data already
            // extracted — and the exterior cull FAILS OPEN on a missing Aabb (an unbounded cell
            // would draw forever, silently undoing 0780's cull unit).
            let aabb = mesh.compute_aabb();
            let handle = meshes.add(mesh);
            commands.entity(root).with_children(|cells| {
                let mut cell = cells.spawn((
                    Mesh3d(handle),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY, // cell meshes are in absolute world coords
                    // ADT terrain is exterior scene: from inside a WMO a cell draws only through
                    // a portal window (`0x683bf0`, fed solely by the per-window walk `0x682fa0` —
                    // see `crate::exterior_cull`).
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

/// `WOW_NO_TILE_CELLS=1` — furnish no per-chunk cell entities (the ablation switch described at
/// its use site in [`furnish_tile_cells`]). Dev-only.
fn tile_cells_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_TILE_CELLS").is_some())
}

/// The one thing the furnisher promises the rest of the streamer: a tile is `furnished` only
/// when its cursor walked every chunk (or cells are ablated off) — never merely "root exists".
#[cfg(test)]
mod tests {
    /// The cap's arithmetic, kept honest in one place: a full fresh row must furnish in well
    /// under a second of frames at 60 Hz.
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
