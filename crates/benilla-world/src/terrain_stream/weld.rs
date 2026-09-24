//! Doodad-hull welding: world-static hulls batch into a few dozen welded trimeshes instead of
//! thousands of colliders, with the same triangles, layers and [`PickOccluder`] clamp (the clamp
//! reads a distance, and hover picks cast against pick meshes, never these hulls).
//!
//! - WMO props weld per placement (uniqueId) and despawn with it, as the single hulls did.
//! - ADT doodads weld per the placement's first-registering tile and despawn with that tile. A
//!   straddler another tile keeps alive loses its hull when the owner unloads, but only past the
//!   unload line, far beyond any reachable or visible doodad.

use std::collections::HashMap;

use bevy::prelude::*;

use super::collider::{build_collider_task, doodad_hulls_bare, PendingCollider};
use crate::collision::PickOccluder;
use crate::interact::WorldObject;
use crate::model_render::ModelKind;

/// Hull cap per weld: keeps a weld's broadphase AABB a neighbourhood, not a zone.
const WELD_MAX_HULLS: u32 = 128;

/// Triangle cap per weld: a full batch attaches in ~0.3 ms, well under `ATTACH_BUDGET`. A single
/// oversized hull exceeds it alone and closes its batch at once.
const WELD_MAX_TRIS: usize = 16_384;

/// Quiet frames (a quarter second) that close a batch in settled play; under the loading cover one
/// quiet frame does, since the settle release waits on the collider backlog.
const WELD_IDLE_FRAMES: u32 = 15;

/// One accumulating weld: a world-space triangle soup, indices rebased as hulls append.
struct WeldAcc {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    hulls: u32,
    /// [`HullWelds::frame`] at the last append: the idle clock.
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

/// The in-flight weld accumulators, drained by [`flush_hull_welds`]. Cleared with the world
/// (`drop_streamed_world`): tile keys repeat across maps, so a stale one would weld into the next.
#[derive(Resource, Default)]
pub struct HullWelds {
    /// Flush-system tick, the idle clock; wrapping, read only as a difference.
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

    /// Accumulators not yet flushed, counted into `colliders_pending` so the settle release waits
    /// for them; an overcount only delays.
    pub(super) fn unflushed(&self) -> usize {
        self.tiles.len() + self.props.len()
    }

    pub(super) fn clear(&mut self) {
        self.tiles.clear();
        self.props.clear();
    }
}

/// `WOW_NO_HULL_WELD=1` spawns one hull entity per placement (a measurement lever).
pub(super) fn hull_weld_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_HULL_WELD").is_some())
}

/// Closes ready batches into [`PendingCollider`] entities owned by their tile (`TileState::welds`)
/// or placement (`Placement::entities`); a dead owner's batch is discarded. Runs right after
/// `spawn_loaded_placements` in the Stream chain.
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

/// One closed batch as one off-thread trimesh build with a single hull's components: default
/// layers, the pick clamp and a static body unless `doodad_hulls_bare`. Not pickable.
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
