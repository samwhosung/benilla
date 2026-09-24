//! Doodad-hull **welding**. The 1367 premise check measured ~0.8 cpu_ms at
//! Stormwind riding the doodad-hull *population* — 12.2k collider entities' tree proxies, AABBs
//! and per-collider row visits in avian's per-frame passes — and refuted the row-weight redesign
//! (a body-less collider costs the same). So the fix is the terrain collider's own pattern
//! (`terrain_collider_data` welds 256 chunks into one trimesh for exactly this reason): batch the
//! world-static hulls into a few dozen welded trimeshes instead of thousands of individual ones.
//! Same triangles, same layers, same [`PickOccluder`] clamp — collision and the mouse-pick
//! answer are unchanged; only the entity granularity collapses. Identity was verified free to
//! give up before this was built: the pick clamp consumes a *distance* (`update_pick_occlusion`),
//! and the inspector/GO hover casts against resident [`PickMesh`] geometry, never these hulls.
//!
//! **Ownership** is the design's load-bearing choice (1367 named it the question to answer
//! first):
//!
//! - **WMO props** weld per *placement* (keyed by uniqueId, batch-split by the caps below). Every
//!   prop of a building despawns with the building (`Placement::entities`), so a weld pushed
//!   there has exactly the lifetime the individual hulls had. No approximation — and on a
//!   WMO-only map (an instance), the entire prop population welds under the one global placement.
//! - **ADT map doodads** weld per the placement's **first-registering tile** (recorded by
//!   `register_doodad`, when that tile is by construction loaded), the weld entity owned by that
//!   tile (`TileState::welds`) like its impassable-wall collider. This is the one deliberate
//!   deviation from per-placement lifetime: a straddler kept alive by a *second* tile keeps its
//!   model when the owner unloads, but its hull goes with the owner's weld. The mismatch can only
//!   open past the unload line — `radius + 1` tiles (over 500 yd of hysteresis alone) from the
//!   focus, far past doodad draw fade — so no reachable or visible doodad ever misses its hull;
//!   near a mover, every referencing tile is loaded, owner included. The converse (a phantom hull
//!   outliving every model) cannot happen at all: the owner tile references the placement, so the
//!   models live at least as long as the weld.

use std::collections::HashMap;

use bevy::prelude::*;

use super::collider::{build_collider_task, doodad_hulls_bare, PendingCollider};
use crate::collision::PickOccluder;
use crate::interact::WorldObject;
use crate::model_render::ModelKind;

/// Hull-count cap per weld. 12.2k hulls at the Stormwind pin batch into ~100 colliders at this
/// size — the population collapse that IS the win — while keeping any one weld's broadphase AABB
/// a neighbourhood, not a zone.
const WELD_MAX_HULLS: u32 = 128;

/// Triangle cap per weld: bounds the off-thread QBVH build and the single attach. At 0610's
/// measured attach cost (~0.004 ms + 1.8e-5 ms/triangle) a full batch attaches in ~0.3 ms — well
/// under `ATTACH_BUDGET`, so one weld can never eat a frame's attach queue. A single oversized
/// hull (a bridge span) may exceed the cap alone; it then closes its batch immediately.
const WELD_MAX_TRIS: usize = 16_384;

/// Quiet frames that close a live batch's tail. The spawner lands a tile's placements over many
/// budget-paced frames; a quarter second of silence means the burst is done. Under the loading
/// cover the tail closes after ONE quiet frame instead: the settle release waits on the collider
/// backlog (via `finish_colliders`' publish), and a fixed quarter-second tail would push
/// every world entry longer for nothing.
const WELD_IDLE_FRAMES: u32 = 15;

/// One accumulating weld: a world-space triangle soup, index-rebased as hulls append — the same
/// concatenation `terrain_collider_data` does for its 256 chunks.
struct WeldAcc {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    hulls: u32,
    /// [`HullWelds::frame`] at the last append — the idle clock the tail flush reads.
    last_add: u32,
}

impl WeldAcc {
    fn append(&mut self, verts: Vec<Vec3>, tris: Vec<[u32; 3]>, frame: u32) {
        let base = self.verts.len() as u32;
        self.verts.extend(verts);
        self.tris
            .extend(tris.into_iter().map(|t| t.map(|i| base + i)));
        self.hulls += 1;
        self.last_add = frame;
    }

    fn ready(&self, frame: u32, idle_frames: u32) -> bool {
        self.hulls >= WELD_MAX_HULLS
            || self.tris.len() >= WELD_MAX_TRIS
            || frame.wrapping_sub(self.last_add) >= idle_frames
    }
}

/// The in-flight weld accumulators, fed by `spawn_loaded_placements` and drained by
/// [`flush_hull_welds`] one chain-step later. Cleared with the world it describes
/// (`drop_streamed_world`) — a stale accumulator would otherwise weld the previous map's geometry
/// into a new map's same-numbered tile, since tile keys are re-inserted at request time on the
/// very frame the old world drops.
#[derive(Resource, Default)]
pub struct HullWelds {
    /// Flush-system tick, the idle clock. Wrapping u32 — only ever read as a difference.
    frame: u32,
    /// ADT map-doodad hulls by owner tile.
    tiles: HashMap<(i32, i32), WeldAcc>,
    /// WMO prop hulls by placement uniqueId.
    props: HashMap<u32, WeldAcc>,
}

impl HullWelds {
    pub(super) fn add_tile(&mut self, tile: (i32, i32), verts: Vec<Vec3>, tris: Vec<[u32; 3]>) {
        let frame = self.frame;
        self.tiles
            .entry(tile)
            .or_insert_with(|| WeldAcc {
                verts: Vec::new(),
                tris: Vec::new(),
                hulls: 0,
                last_add: frame,
            })
            .append(verts, tris, frame);
    }

    pub(super) fn add_prop(&mut self, uid: u32, verts: Vec<Vec3>, tris: Vec<[u32; 3]>) {
        let frame = self.frame;
        self.props
            .entry(uid)
            .or_insert_with(|| WeldAcc {
                verts: Vec::new(),
                tris: Vec::new(),
                hulls: 0,
                last_add: frame,
            })
            .append(verts, tris, frame);
    }

    /// Accumulators not yet flushed into a [`PendingCollider`] — counted into
    /// `WorldLoadProgress::colliders_pending` by `finish_colliders`, so the settle release never
    /// lets a body go while doodad hulls still sit in a batch (each accumulator becomes at least
    /// one pending collider). Like the queue depth itself, an overcount here (an accumulator a
    /// later flush discards) can only delay a release, never wrong one.
    pub(super) fn unflushed(&self) -> usize {
        self.tiles.len() + self.props.len()
    }

    pub(super) fn clear(&mut self) {
        self.tiles.clear();
        self.props.clear();
    }
}

/// `WOW_NO_HULL_WELD=1` — spawn doodad/prop hulls one entity per placement, the pre-1369 shape:
/// the welding A/B lever (the `WOW_NO_*` pattern), never a setting.
pub(super) fn hull_weld_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_HULL_WELD").is_some())
}

/// Close ready batches into [`PendingCollider`] entities and hand each to its owner — tile welds
/// onto `TileState::welds` (despawned with the tile, like the wall), prop welds into
/// `Placement::entities` (despawned with the placement, like the hulls they replace). An
/// accumulator whose owner is already gone is discarded: its hulls were only ever reachable past
/// the unload line (module doc), and the alternative — spawning a weld nothing owns — is a leak.
///
/// Runs in the Stream chain right after `spawn_loaded_placements`, so the frame's appends see the
/// flush at a deterministic point and the owner lookups race nothing.
pub(super) fn flush_hull_welds(
    mut commands: Commands,
    welds: ResMut<HullWelds>,
    mut streamer: ResMut<super::TerrainStreamer>,
    mut placements: ResMut<super::Placements>,
    focus: Res<super::ViewFocus>,
) {
    let welds = welds.into_inner();
    welds.frame = welds.frame.wrapping_add(1);
    let frame = welds.frame;
    let idle = if focus.paced { WELD_IDLE_FRAMES } else { 1 };
    welds.tiles.retain(|&key, acc| {
        let Some(tile) = streamer.tiles.get_mut(&key) else {
            return false;
        };
        if !acc.ready(frame, idle) {
            return true;
        }
        tile.welds.push(spawn_weld(&mut commands, acc));
        false
    });
    welds.props.retain(|&uid, acc| {
        let Some(p) = placements.by_id.get_mut(&uid) else {
            return false;
        };
        if !acc.ready(frame, idle) {
            return true;
        }
        p.entities.push(spawn_weld(&mut commands, acc));
        false
    });
}

/// One closed batch → one off-thread trimesh build, carrying exactly what an individual hull
/// carried: default layers (both audiences), the pick clamp, and — unless the 1367 lever bares
/// it — a static body. Tagged for the census/inspector so the row is nameable, but with no
/// `PickMesh`/`PickBox` it is not pickable (pick geometry is declared, never inferred).
fn spawn_weld(commands: &mut Commands, acc: &mut WeldAcc) -> Entity {
    let verts = std::mem::take(&mut acc.verts);
    let tris = std::mem::take(&mut acc.tris);
    commands
        .spawn((
            PendingCollider::new(build_collider_task(verts, tris), None, !doodad_hulls_bare()),
            PickOccluder,
            WorldObject {
                kind: ModelKind::Doodad,
                label: "hull-weld".into(),
                id: 0,
                detail: format!("{} hulls welded", acc.hulls),
            },
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::super::{ModelHandle, Placement, Placements, TerrainStreamer, TileState, ViewFocus};
    use super::*;
    use bevy::app::TaskPoolPlugin;
    use bevy::ecs::system::RunSystemOnce;

    fn hull() -> (Vec<Vec3>, Vec<[u32; 3]>) {
        (vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![[0, 1, 2]])
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.init_resource::<HullWelds>();
        app.init_resource::<TerrainStreamer>();
        app.init_resource::<Placements>();
        app.init_resource::<ViewFocus>();
        app
    }

    fn blank_tile() -> TileState {
        TileState {
            handle: Default::default(),
            entity: None,
            material: None,
            next_cell: 0,
            furnished: false,
            placements: Vec::new(),
            liquid: Vec::new(),
            wall: None,
            clutter: Vec::new(),
            welds: Vec::new(),
            merged: Vec::new(),
        }
    }

    fn blank_placement() -> Placement {
        Placement {
            model: ModelHandle::M2(Default::default()),
            transform: Transform::IDENTITY,
            entities: Vec::new(),
            spawned: true,
            doodad_set: 0,
            name_set: 0,
            doodads: Vec::new(),
            portal_instance: None,
            refs: 1,
            owner: (0, 0),
        }
    }

    /// Appending rebases indices onto the running vertex count — two 3-vert hulls weld into one
    /// 6-vert soup whose second triangle indexes past the first.
    #[test]
    fn append_rebases_indices() {
        let mut welds = HullWelds::default();
        let (v, t) = hull();
        welds.add_tile((0, 0), v.clone(), t.clone());
        welds.add_tile((0, 0), v, t);
        let acc = welds.tiles.get(&(0, 0)).unwrap();
        assert_eq!(acc.hulls, 2);
        assert_eq!(acc.verts.len(), 6);
        assert_eq!(acc.tris, vec![[0, 1, 2], [3, 4, 5]]);
        assert_eq!(welds.unflushed(), 1);
    }

    /// A batch past the hull cap flushes on the next pass, and the weld entity lands in its
    /// owner tile's `welds` (the despawn list).
    #[test]
    fn cap_flush_ties_weld_to_tile() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((3, 4), blank_tile());
        {
            let mut welds = app.world_mut().resource_mut::<HullWelds>();
            for _ in 0..WELD_MAX_HULLS {
                let (v, t) = hull();
                welds.add_tile((3, 4), v, t);
            }
        }
        app.world_mut().run_system_once(flush_hull_welds).unwrap();
        assert_eq!(app.world().resource::<HullWelds>().unflushed(), 0);
        let streamer = app.world().resource::<TerrainStreamer>();
        let owned = streamer.tiles.get(&(3, 4)).unwrap().welds.clone();
        assert_eq!(owned.len(), 1);
        assert!(app.world().get::<PendingCollider>(owned[0]).is_some());
        assert!(app.world().get::<PickOccluder>(owned[0]).is_some());
    }

    /// An under-cap batch holds while appends keep landing, then closes after the idle tail.
    #[test]
    fn idle_tail_closes_a_quiet_batch() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((0, 0), blank_tile());
        {
            let mut welds = app.world_mut().resource_mut::<HullWelds>();
            let (v, t) = hull();
            welds.add_tile((0, 0), v, t);
        }
        for _ in 0..(WELD_IDLE_FRAMES - 1) {
            app.world_mut().run_system_once(flush_hull_welds).unwrap();
        }
        assert_eq!(app.world().resource::<HullWelds>().unflushed(), 1);
        app.world_mut().run_system_once(flush_hull_welds).unwrap();
        assert_eq!(app.world().resource::<HullWelds>().unflushed(), 0);
        let streamer = app.world().resource::<TerrainStreamer>();
        assert_eq!(streamer.tiles.get(&(0, 0)).unwrap().welds.len(), 1);
    }

    /// An accumulator whose owner tile is gone is discarded — no weld entity, no leak.
    #[test]
    fn dead_owner_discards_the_batch() {
        let mut app = test_app();
        {
            let mut welds = app.world_mut().resource_mut::<HullWelds>();
            for _ in 0..WELD_MAX_HULLS {
                let (v, t) = hull();
                welds.add_tile((9, 9), v, t);
            }
        }
        let before = app.world().entities().len();
        app.world_mut().run_system_once(flush_hull_welds).unwrap();
        assert_eq!(app.world().resource::<HullWelds>().unflushed(), 0);
        assert_eq!(app.world().entities().len(), before);
    }

    /// A prop batch flushes into its placement's entity list — the lifetime the individual
    /// hulls had.
    #[test]
    fn prop_weld_lands_in_placement_entities() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<Placements>()
            .by_id
            .insert(7, blank_placement());
        {
            let mut welds = app.world_mut().resource_mut::<HullWelds>();
            for _ in 0..WELD_MAX_HULLS {
                let (v, t) = hull();
                welds.add_prop(7, v, t);
            }
        }
        app.world_mut().run_system_once(flush_hull_welds).unwrap();
        let placements = app.world().resource::<Placements>();
        let owned = &placements.by_id.get(&7).unwrap().entities;
        assert_eq!(owned.len(), 1);
        assert!(app.world().get::<PendingCollider>(owned[0]).is_some());
    }
}
