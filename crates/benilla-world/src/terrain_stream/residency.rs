//! Which terrain the mover may resolve against. The reference's world triangle query `0x6aa8b0`
//! fails, and the mover's substep is skipped, on any of three legs: a map object not ready
//! (`0x6aa90c` → `0x6aaab0`, which `0x6aa913` tests before the one-object branch), a terrain
//! chunk under the swept box not loaded (`0x6aadc0`), or a doodad in a gathered chunk not ready
//! (`0x6ab4a1` → `0x6abc40`, 0 at `0x6abcda`). [`publish_terrain_residency`] writes the tiles that
//! pass the terrain leg each frame, after the attaches, and [`TerrainResidency::box_resident`]
//! answers for a box.
//!
//! Deviation: benilla gates on terrain alone, which is more permissive; the settle hold covers a
//! building or doodad arriving after a teleport. And an authored tile whose ADT failed to load
//! counts as resident, where the reference would have loaded it: a permanent wall over a bad file
//! is worse for a player than walking on without its ground.

use bevy::prelude::*;

use super::collider::PendingCollider;
use super::TerrainStreamer;

/// The tile grid's half extent, `0x7ffab4`: a world coordinate maps to `17066.666 − c`.
const GRID_ORIGIN: f32 = 17066.666;
/// The tile grid's extent, `0x7ffab0`: a mapped coordinate must lie below it.
const GRID_SPAN: f32 = 34133.332;
/// Cells per yard, `0x810ae4`: eight cells to a chunk, sixteen chunks to a tile.
const CELLS_PER_YARD: f32 = 0.24;

/// The tiles whose terrain the mover's world query may gather, as of the last stream stage.
#[derive(Resource, Clone, PartialEq, Eq, Debug)]
pub struct TerrainResidency {
    /// Bit `tile_x` of row `tile_y` (the [`benilla_formats::world_to_tile`] key): the tile is
    /// spawned and its terrain and fence colliders are attached, or its ADT failed to load.
    rows: [u64; 64],
    /// The map is one object (WDT `MPHD` bit 0, `[0xc9e384]`): after the map-object leg its query
    /// returns 1 without testing terrain (`0x6aa919`), so terrain gates nothing there.
    one_object: bool,
}

impl Default for TerrainResidency {
    /// Nothing resident: before the first stream stage, and after leaving the world.
    fn default() -> Self {
        Self {
            rows: [0; 64],
            one_object: false,
        }
    }
}

impl TerrainResidency {
    /// A residency with exactly `tiles` resident, for tests and tools.
    pub fn with_tiles(tiles: impl IntoIterator<Item = (i32, i32)>) -> Self {
        let mut r = Self::default();
        for (tx, ty) in tiles {
            r.insert(tx, ty);
        }
        r
    }

    /// A one-object map's residency, whose terrain gates nothing.
    pub fn one_object() -> Self {
        Self {
            one_object: true,
            ..Self::default()
        }
    }

    fn insert(&mut self, tx: i32, ty: i32) {
        if (0..64).contains(&tx) && (0..64).contains(&ty) {
            self.rows[ty as usize] |= 1 << tx;
        }
    }

    /// Whether the tile `(tile_x, tile_y)` is resident.
    pub fn tile(&self, tx: i32, ty: i32) -> bool {
        (0..64).contains(&tx) && (0..64).contains(&ty) && self.rows[ty as usize] & (1 << tx) != 0
    }

    /// The terrain leg of `0x6aa8b0` for the WoW-space box `min..max` (x, y; z is not read): every
    /// chunk the box's cells touch must be loaded (`0x6aadc0` returns 0 for a null tile at
    /// `0x6aadfa` and a null chunk at `0x6aae1f`), and the box must lie on the grid (`0x6aa967`,
    /// `0x6aa98c`). benilla loads whole tiles, so a chunk is loaded when its tile is resident.
    pub fn box_resident(&self, min: [f32; 2], max: [f32; 2]) -> bool {
        if self.one_object {
            return true;
        }
        // `fsub 17066.666; fchs` per corner, so the max corner gives the low cell.
        let lo = [-(max[0] - GRID_ORIGIN), -(max[1] - GRID_ORIGIN)];
        let hi = [-(min[0] - GRID_ORIGIN), -(min[1] - GRID_ORIGIN)];
        // `0x6aa967`: off the grid's north or west edge. The reference passes a NaN on; no
        // finite position makes one, and it is refused here.
        if !(lo[0] >= 0.0 && lo[1] >= 0.0) {
            return false;
        }
        // `0x6aa98c`: off the south or east edge.
        if !(hi[0] < GRID_SPAN && hi[1] < GRID_SPAN) {
            return false;
        }
        // `0x6aa9b4`: the product stored as a float, less 0.5 on the x87 stack and `fistp`'d to
        // nearest-even, then `>> 3` to the chunk (`0x6aaa23`) and `>> 4` to the tile (`0x6aaddc`).
        let cell = |v: f32| (f64::from(v * CELLS_PER_YARD) - 0.5).round_ties_even() as i32;
        let tile = |v: f32| (cell(v) >> 3) >> 4;
        // Rows run along x and columns along y; the tile key is (column, row).
        let (row0, row1) = (tile(lo[0]), tile(hi[0]));
        let (col0, col1) = (tile(lo[1]), tile(hi[1]));
        (row0..=row1).all(|row| (col0..=col1).all(|col| self.tile(col, row)))
    }
}

/// Publishes [`TerrainResidency`] at the end of the stream stage, so the next frame's mover reads
/// the tiles whose colliders are attached by then: a tile counts once its root is spawned and
/// neither its terrain nor its fence collider is still building, or once its ADT's load failed
/// (the module's `Deviation:`). A tile the WDT does not author is never requested, so never
/// resident.
pub(super) fn publish_terrain_residency(
    state: Res<TerrainStreamer>,
    pending: Query<(), With<PendingCollider>>,
    asset_server: Res<AssetServer>,
    mut residency: ResMut<TerrainResidency>,
) {
    let mut next = TerrainResidency {
        one_object: state.one_object,
        ..TerrainResidency::default()
    };
    for (&(tx, ty), t) in &state.tiles {
        let Some(root) = t.entity else {
            if matches!(
                asset_server.load_state(&t.handle),
                bevy::asset::LoadState::Failed(_)
            ) {
                next.insert(tx, ty);
            }
            continue;
        };
        if pending.contains(root) || t.wall.is_some_and(|w| pending.contains(w)) {
            continue;
        }
        next.insert(tx, ty);
    }
    if *residency != next {
        *residency = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::{tile_to_world, world_to_tile, TILE_SIZE};

    /// The WoW-space box of half-width `half` around `(x, y)`.
    fn around(x: f32, y: f32, half: f32) -> ([f32; 2], [f32; 2]) {
        ([x - half, y - half], [x + half, y + half])
    }

    #[test]
    fn a_box_inside_one_resident_tile_is_resident_and_its_neighbour_is_not() {
        let r = TerrainResidency::with_tiles([(32, 48)]);
        let (x, y) = tile_to_world(32, 48);
        // `tile_to_world` gives the tile's north-west corner; step a quarter tile into it.
        let (cx, cy) = (x - TILE_SIZE * 0.25, y - TILE_SIZE * 0.25);
        assert_eq!(world_to_tile(cx, cy), (32, 48));
        let (lo, hi) = around(cx, cy, 2.0);
        assert!(r.box_resident(lo, hi));
        // One tile south (−x is the next row) is not.
        let (lo, hi) = around(cx - TILE_SIZE, cy, 2.0);
        assert!(!r.box_resident(lo, hi));
        // A box straddling the row line needs both tiles.
        let (lo, hi) = around(x - TILE_SIZE, cy, 2.0);
        assert!(!r.box_resident(lo, hi));
        let both = TerrainResidency::with_tiles([(32, 48), (32, 49)]);
        assert!(both.box_resident(lo, hi));
    }

    #[test]
    fn a_box_off_the_grid_is_never_resident() {
        let every =
            TerrainResidency::with_tiles((0..64).flat_map(|tx| (0..64).map(move |ty| (tx, ty))));
        assert!(every.box_resident([-10.0, -10.0], [10.0, 10.0]));
        assert!(
            !every.box_resident([17060.0, 0.0], [17070.0, 1.0]),
            "north edge"
        );
        assert!(
            !every.box_resident([-17070.0, 0.0], [-17060.0, 1.0]),
            "south edge"
        );
        assert!(
            !every.box_resident([0.0, 17060.0], [1.0, 17070.0]),
            "west edge"
        );
        assert!(
            !every.box_resident([0.0, -17070.0], [1.0, -17060.0]),
            "east edge"
        );
    }

    /// A spawned tile counts once neither its terrain nor its fence collider is still building;
    /// a tile still being read, or never authored, never does.
    #[test]
    fn a_tile_counts_once_its_colliders_are_attached() {
        use super::super::TileState;
        let mut app = test_app();
        let pending =
            || PendingCollider::ready(avian3d::prelude::Collider::sphere(1.0), None, true);
        let root = app.world_mut().spawn(pending()).id();
        let wall = app.world_mut().spawn(pending()).id();
        let mut state = TerrainStreamer::default();
        let mut spawned = TileState::requested(Handle::default());
        spawned.entity = Some(root);
        spawned.wall = Some(wall);
        state.tiles.insert((32, 48), spawned);
        state
            .tiles
            .insert((33, 48), TileState::requested(Handle::default()));
        app.insert_resource(state);

        app.update();
        assert!(
            !app.world().resource::<TerrainResidency>().tile(32, 48),
            "both still build"
        );
        app.world_mut().entity_mut(root).remove::<PendingCollider>();
        app.update();
        assert!(
            !app.world().resource::<TerrainResidency>().tile(32, 48),
            "the fence builds"
        );
        app.world_mut().entity_mut(wall).remove::<PendingCollider>();
        app.update();
        let r = app.world().resource::<TerrainResidency>();
        assert!(r.tile(32, 48), "spawned with both colliders attached");
        assert!(!r.tile(33, 48), "a tile still being read is not resident");
        assert!(!r.tile(34, 48), "nor is one never requested");
    }

    /// The publish under a headless asset server, so a tile's load state is real.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<benilla_assets::AdtTile>()
            .init_resource::<TerrainResidency>()
            .add_systems(Update, publish_terrain_residency);
        app
    }

    /// An authored tile whose ADT fails to load is walked on, not walled off.
    #[test]
    fn a_tile_whose_adt_failed_to_load_counts_as_resident() {
        use super::super::TileState;
        let mut app = test_app();
        let handle = app
            .world()
            .resource::<AssetServer>()
            .load::<benilla_assets::AdtTile>("no_such_dir/no_such_tile.adt");
        let mut state = TerrainStreamer::default();
        state
            .tiles
            .insert((32, 48), TileState::requested(handle.clone()));
        app.insert_resource(state);
        let failed = |app: &App| {
            matches!(
                app.world().resource::<AssetServer>().load_state(&handle),
                bevy::asset::LoadState::Failed(_)
            )
        };
        for _ in 0..400 {
            if failed(&app) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.update();
        }
        assert!(failed(&app), "the read never failed");
        app.update();
        assert!(
            app.world().resource::<TerrainResidency>().tile(32, 48),
            "a failed read keeps the old walk-on behaviour"
        );
    }

    #[test]
    fn a_one_object_map_gates_nothing() {
        let r = TerrainResidency::one_object();
        assert!(r.box_resident([-10.0, -10.0], [10.0, 10.0]));
        assert!(r.box_resident([17060.0, 0.0], [17070.0, 1.0]));
        assert!(!TerrainResidency::default().box_resident([-10.0, -10.0], [10.0, 10.0]));
    }
}
