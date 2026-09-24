//! Static-world consolidation of doodad batches, on unless `WOW_STATIC_MERGE=0`. It runs behind
//! the retained pass (`crate::static_gx`), which takes the static populations first: what reaches
//! it is that pass's declined families, or everything under `WOW_STATIC_GX=0`.
//!
//! The assembler diverts every fully static, order-free (`Opaque`/`AlphaTest`) batch that does not
//! fade, and the flush bakes each ADT-doodad (owner tile, 133⅓-yd cell, material) or WMO-prop
//! (placement, room set, material) group into one mesh with the placement transforms in its
//! vertices. A blob closes at the vertex cap or after an idle tail; a doodad blob despawns with
//! its tile (`TileState::merged`). Distance admission closes a cell in fragments during play, so a
//! cell quiet for [`RECONSOLIDATE_IDLE_FRAMES`] re-bakes into one blob per vertex cap and the
//! fragments retire after [`RETIRE_FRAMES`]. A straddler whose owner tile unloads is re-owned by
//! a loaded referrer (`handoff_straddlers`) and re-diverts under it.
//!
//! WMO group geometry never diverts: `batch_order` is a `MatKey` axis, so each WMO batch owns its
//! material and a merge saves nothing ([`MergeSite::Wmo`] feeds only the census predictor).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::merged_static_mesh_faded;
use benilla_formats::{ModelBlend, RenderSubmesh};

use crate::interact::WorldObject;
use crate::mesh_tag::alpha_bits;
use crate::model_render::{ModelKind, ModelPart};
use crate::wmo_portal::WmoGroupVis;

/// Vertex cap per blob: bounds one bake and upload and keeps the cull bound a neighbourhood.
const MERGE_MAX_VERTS: usize = 65_536;

/// Quiet frames (a quarter second) that close an accumulator in settled play; under the arrival
/// cover one quiet frame does, since the settle release waits on the backlog.
const MERGE_IDLE_FRAMES: u32 = 15;

/// Quiet frames (~3 s at 60 Hz) before a settled cell's fragments re-bake into one blob.
const RECONSOLIDATE_IDLE_FRAMES: u32 = 180;

/// Frames a replaced fragment keeps drawing while its successor's mesh uploads: the overlap is
/// invisible on order-free batches, where a gap would flicker the cell off.
const RETIRE_FRAMES: u32 = 10;

/// The doodad spatial cell, ¼ of an ADT tile (133⅓ yd), so a blob's bound stays local enough for
/// the frustum cull.
const CELL: f32 = 533.333_3 / 4.0;

/// One accumulating blob; every list is index-parallel with `parts`.
struct MergeAcc {
    parts: Vec<(Arc<RenderSubmesh>, Transform)>,
    /// Each part's placement identity, which the pick names.
    objects: Vec<Arc<crate::interact::WorldObject>>,
    spheres: Vec<Vec4>,
    /// Each part's SH-probe slot, interior props only.
    slots: Vec<u32>,
    verts: usize,
    blend: ModelBlend,
    kind: ModelKind,
    /// [`StaticMerge::frame`] at the last append: the idle clock.
    last_add: u32,
}

impl MergeAcc {
    fn ready(&self, frame: u32, idle_frames: u32) -> bool {
        self.verts >= MERGE_MAX_VERTS || frame.wrapping_sub(self.last_add) >= idle_frames
    }

    fn source(&self) -> BlobSource<'_> {
        BlobSource {
            parts: &self.parts,
            objects: &self.objects,
            spheres: &self.spheres,
            slots: &self.slots,
            blend: self.blend,
            kind: self.kind,
        }
    }
}

/// One blob bake's input as index-parallel slices: a closed accumulator or a re-consolidated cell.
struct BlobSource<'a> {
    parts: &'a [(Arc<RenderSubmesh>, Transform)],
    objects: &'a [Arc<crate::interact::WorldObject>],
    spheres: &'a [Vec4],
    slots: &'a [u32],
    blend: ModelBlend,
    kind: ModelKind,
}

/// Where a diverted batch belongs, built once per placement by the spawn driver.
pub enum MergeSite<'a> {
    /// An ADT map doodad: owned by its first-registering tile (the weld's ownership).
    Doodad { owner: (i32, i32) },
    /// WMO group geometry: `groups` is the asset's per-submesh group index table.
    Wmo {
        uid: u32,
        groups: &'a [u16],
        portal_gated: bool,
    },
    /// A WMO doodad prop: owned by its placement, keyed by the set of rooms that name it
    /// (`groups`, the blob's `WmoGroupVis`) and, indoors, carrying its SH-probe slot.
    Prop {
        uid: u32,
        groups: &'a Arc<[u16]>,
        slot: Option<u16>,
    },
}

impl MergeSite<'_> {
    /// One batch's would-be merge key, hashed, for the census's blob-count predictor; it must
    /// mirror the real divert key.
    pub fn census_key(
        &self,
        batch_idx: usize,
        mat: &Handle<WowModelMaterial>,
        transform: &Transform,
    ) -> Option<u64> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            MergeSite::Doodad { owner } => {
                let cell = (
                    (transform.translation.x / CELL).floor() as i32,
                    (transform.translation.z / CELL).floor() as i32,
                );
                (0u8, owner, cell, mat.id()).hash(&mut h);
            }
            MergeSite::Wmo { uid, groups, .. } => {
                (1u8, uid, groups.get(batch_idx)?, mat.id()).hash(&mut h);
            }
            MergeSite::Prop { uid, groups, .. } => {
                (2u8, uid, groups, mat.id()).hash(&mut h);
            }
        }
        Some(h.finish())
    }
}

/// A doodad blob's identity: (owner tile, 133⅓-yd cell, material).
type DoodadKey = ((i32, i32), (i32, i32), Handle<WowModelMaterial>);

/// One spawned doodad blob retained with its bake input, so a shredded cell can re-bake.
struct SettledBlob {
    entity: Entity,
    parts: Vec<(Arc<RenderSubmesh>, Transform)>,
    objects: Vec<Arc<crate::interact::WorldObject>>,
    spheres: Vec<Vec4>,
    verts: usize,
}

/// A key's spawned blobs. `dirty` arms one re-consolidation check after the next quiet window; a
/// declined one waits for a new fragment.
struct SettledCell {
    blobs: Vec<SettledBlob>,
    blend: ModelBlend,
    kind: ModelKind,
    /// [`StaticMerge::frame`] at the last fragment spawn: the quiet clock.
    last_add: u32,
    dirty: bool,
}

/// A replaced fragment drawing out its successor's upload cushion ([`RETIRE_FRAMES`]).
struct Retiring {
    entity: Entity,
    owner: (i32, i32),
    /// [`StaticMerge::frame`] when the successor spawned.
    since: u32,
}
/// A prop blob's identity: (placement uid, room set, material); the `Arc<[u16]>` hashes by content.
type PropKey = (u32, Arc<[u16]>, Handle<WowModelMaterial>);

/// The in-flight merge accumulators, fed by the spawn chain and cleared with the world.
#[derive(Resource, Default)]
pub struct StaticMerge {
    /// Flush-system tick, the idle clock; wrapping, read only as a difference.
    frame: u32,
    doodads: HashMap<DoodadKey, MergeAcc>,
    props: HashMap<PropKey, MergeAcc>,
    /// Spawned doodad blobs per key with their bake inputs, for re-consolidation.
    settled: HashMap<DoodadKey, SettledCell>,
    /// Replaced fragments still drawing out their successor's upload cushion.
    retiring: Vec<Retiring>,
    /// Totals since the last drain report: blobs, batches, and baked vertices (a copy per
    /// placement) against the shared assets' vertices (each geometry once).
    blobs: u64,
    batches: u64,
    baked_verts: u64,
    shared_verts: u64,
    seen_geometry: std::collections::HashSet<usize>,
    reported: bool,
}

/// Whether the consolidation is armed, read once: on unless `WOW_STATIC_MERGE=0`.
pub fn merge_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_STATIC_MERGE").as_deref() != Ok("0"))
}

impl StaticMerge {
    /// Takes one mergeable batch into its accumulator. `fade_sphere` is the placement's world fade
    /// centre and radius, baked per vertex (a never-fader's true radius hits the shader's `> 7`
    /// opaque arm). `false` for WMO group geometry, which the caller spawns individually.
    pub(super) fn divert(
        &mut self,
        site: &MergeSite<'_>,
        _batch_idx: usize,
        mat: &Handle<WowModelMaterial>,
        geometry: &Arc<RenderSubmesh>,
        transform: Transform,
        fade_sphere: Vec4,
        blend: ModelBlend,
        kind: ModelKind,
        object: &Arc<crate::interact::WorldObject>,
    ) -> bool {
        let frame = self.frame;
        let acc = match site {
            MergeSite::Doodad { owner } => {
                let cell = (
                    (transform.translation.x / CELL).floor() as i32,
                    (transform.translation.z / CELL).floor() as i32,
                );
                self.doodads
                    .entry((*owner, cell, mat.clone()))
                    .or_insert_with(|| MergeAcc {
                        parts: Vec::new(),
                        objects: Vec::new(),
                        spheres: Vec::new(),
                        slots: Vec::new(),
                        verts: 0,
                        blend,
                        kind,
                        last_add: frame,
                    })
            }
            MergeSite::Prop { uid, groups, slot } => {
                let acc = self
                    .props
                    .entry((*uid, Arc::clone(groups), mat.clone()))
                    .or_insert_with(|| MergeAcc {
                        parts: Vec::new(),
                        objects: Vec::new(),
                        spheres: Vec::new(),
                        slots: Vec::new(),
                        verts: 0,
                        blend,
                        kind,
                        last_add: frame,
                    });
                if let Some(slot) = slot {
                    acc.slots.push(u32::from(*slot));
                }
                // A key is all interior or all exterior; a ragged slot list misindexes the bake.
                debug_assert!(acc.slots.is_empty() || acc.slots.len() == acc.parts.len() + 1);
                acc
            }
            // WMO group geometry never merges: each batch owns its material. Census only.
            MergeSite::Wmo { .. } => return false,
        };
        acc.spheres.push(fade_sphere);
        let verts = geometry.positions.len();
        if self
            .seen_geometry
            .insert(Arc::as_ptr(geometry) as *const () as usize)
        {
            self.shared_verts += verts as u64;
        }
        self.baked_verts += verts as u64;
        self.batches += 1;
        acc.parts.push((geometry.clone(), transform));
        acc.objects.push(object.clone());
        acc.verts += verts;
        acc.last_add = frame;
        true
    }

    /// Accumulators not yet baked, the reveal gate's `merge_pending`; an overcount only delays.
    pub(super) fn unflushed(&self) -> usize {
        self.doodads.len() + self.props.len()
    }

    pub(super) fn clear(&mut self) {
        self.doodads.clear();
        self.props.clear();
        self.settled.clear();
        self.retiring.clear();
        self.blobs = 0;
        self.batches = 0;
        self.baked_verts = 0;
        self.shared_verts = 0;
        self.seen_geometry.clear();
        self.reported = true;
    }
}

/// Closes ready accumulators into blobs owned by their tile (`TileState::merged`) or placement,
/// re-consolidates quiet cells and retires replaced fragments; a dead owner's accumulator is
/// discarded. Runs in the Stream chain right after `flush_hull_welds`.
pub(super) fn flush_static_merge(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    merge: ResMut<StaticMerge>,
    mut streamer: ResMut<super::TerrainStreamer>,
    mut placements: ResMut<super::Placements>,
    focus: Res<super::ViewFocus>,
    mut progress: Option<ResMut<super::WorldLoadProgress>>,
) {
    let merge = merge.into_inner();
    merge.frame = merge.frame.wrapping_add(1);
    let frame = merge.frame;
    let idle = if focus.paced { MERGE_IDLE_FRAMES } else { 1 };
    let mut blobs = 0u64;
    {
        let StaticMerge {
            doodads, settled, ..
        } = &mut *merge;
        doodads.retain(|key, acc| {
            let Some(tile) = streamer.tiles.get_mut(&key.0) else {
                return false;
            };
            if !acc.ready(frame, idle) {
                return true;
            }
            blobs += 1;
            let entity = spawn_blob(&mut commands, &mut meshes, &key.2, acc.source(), None, true);
            tile.merged.push(entity);
            // Keep the bake input for a later re-consolidation.
            let cell = settled.entry(key.clone()).or_insert_with(|| SettledCell {
                blobs: Vec::new(),
                blend: acc.blend,
                kind: acc.kind,
                last_add: frame,
                dirty: true,
            });
            cell.blobs.push(SettledBlob {
                entity,
                parts: std::mem::take(&mut acc.parts),
                objects: std::mem::take(&mut acc.objects),
                spheres: std::mem::take(&mut acc.spheres),
                verts: acc.verts,
            });
            cell.last_add = frame;
            cell.dirty = true;
            false
        });
    }
    merge.props.retain(|key, acc| {
        let Some(p) = placements.by_id.get_mut(&key.0) else {
            return false;
        };
        if !acc.ready(frame, idle) {
            return true;
        }
        blobs += 1;
        // The members' tagging: `WmoGroupVis` and `ExteriorScene` when the building has an
        // instance and rooms name the prop, untagged otherwise.
        let vis = (!key.1.is_empty())
            .then_some(p.portal_instance)
            .flatten()
            .map(|instance| WmoGroupVis {
                instance,
                groups: Arc::clone(&key.1),
            });
        let exterior = vis.is_some();
        p.entities.push(spawn_blob(
            &mut commands,
            &mut meshes,
            &key.2,
            acc.source(),
            vis,
            exterior,
        ));
        false
    });
    // Re-consolidate quiet cells: first-fit whole fragments under the vertex cap, only when that
    // makes strictly fewer blobs.
    let mut recon = (0u64, 0usize, 0usize); // (cells, fragments before, blobs after)
    {
        let StaticMerge {
            settled, retiring, ..
        } = &mut *merge;
        settled.retain(|key, cell| {
            let Some(tile) = streamer.tiles.get_mut(&key.0) else {
                // The owner died and took the entities with it; the ledger follows.
                return false;
            };
            if !cell.dirty || frame.wrapping_sub(cell.last_add) < RECONSOLIDATE_IDLE_FRAMES {
                return true;
            }
            cell.dirty = false;
            let mut splits = vec![0usize];
            let mut open = 0usize;
            for (i, b) in cell.blobs.iter().enumerate() {
                if open > 0 && open + b.verts > MERGE_MAX_VERTS {
                    splits.push(i);
                    open = 0;
                }
                open += b.verts;
            }
            if splits.len() >= cell.blobs.len() {
                return true;
            }
            let old = std::mem::take(&mut cell.blobs);
            recon.0 += 1;
            recon.1 += old.len();
            recon.2 += splits.len();
            splits.push(old.len());
            for w in splits.windows(2) {
                let members = &old[w[0]..w[1]];
                let parts: Vec<_> = members
                    .iter()
                    .flat_map(|b| b.parts.iter().cloned())
                    .collect();
                let objects: Vec<_> = members
                    .iter()
                    .flat_map(|b| b.objects.iter().cloned())
                    .collect();
                let spheres: Vec<_> = members
                    .iter()
                    .flat_map(|b| b.spheres.iter().copied())
                    .collect();
                let entity = spawn_blob(
                    &mut commands,
                    &mut meshes,
                    &key.2,
                    BlobSource {
                        parts: &parts,
                        objects: &objects,
                        spheres: &spheres,
                        slots: &[],
                        blend: cell.blend,
                        kind: cell.kind,
                    },
                    None,
                    true,
                );
                tile.merged.push(entity);
                cell.blobs.push(SettledBlob {
                    entity,
                    verts: members.iter().map(|b| b.verts).sum(),
                    parts,
                    objects,
                    spheres,
                });
            }
            for b in &old {
                retiring.push(Retiring {
                    entity: b.entity,
                    owner: key.0,
                    since: frame,
                });
            }
            true
        });
    }
    if recon.0 > 0 {
        debug!(
            "static-merge: reconsolidated {} cell(s): {} fragments → {} blob(s)",
            recon.0, recon.1, recon.2
        );
    }
    // Retire fragments past the cushion; a dead owner already despawned them with its tile.
    merge.retiring.retain(|r| {
        if frame.wrapping_sub(r.since) < RETIRE_FRAMES {
            return true;
        }
        if let Some(tile) = streamer.tiles.get_mut(&r.owner) {
            tile.merged.retain(|e| *e != r.entity);
            commands.entity(r.entity).despawn();
        }
        false
    });
    merge.blobs += blobs;
    if blobs > 0 {
        merge.reported = false;
    }
    if let Some(progress) = progress.as_mut() {
        progress.merge_pending = merge.unflushed();
    }
    // The drain report, once per settled wave: what baking duplicates against the shared assets.
    if !merge.reported && merge.doodads.is_empty() && merge.props.is_empty() {
        merge.reported = true;
        debug!(
            "static-merge: {} blobs from {} batches; baked {}kv vs {}kv shared ({:.2}x duplication)",
            merge.blobs,
            merge.batches,
            merge.baked_verts / 1000,
            merge.shared_verts / 1000,
            merge.baked_verts as f64 / merge.shared_verts.max(1) as f64
        );
    }
}

/// `WOW_MERGE_FADERS=1` merges fading doodads too (dev only). A fader blob is one transparent draw
/// with one sort key over a cell of depths, and its depth write hides any per-entity fader it
/// sorts ahead of, so by default faders spawn per entity.
pub(crate) fn merge_faders_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_MERGE_FADERS").is_some())
}

/// `WOW_BLOB_VIS=1` prints every merged blob's visibility verdicts every 2 s, plus any entity
/// whose `WorldObject::id` is in `WOW_BLOB_VIS_UID` (comma-separated).
pub(crate) fn blob_vis_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_BLOB_VIS").is_some())
}

#[allow(clippy::type_complexity)]
pub(crate) fn log_blob_vis(
    time: Res<Time>,
    mut last: Local<f32>,
    q: Query<(
        Entity,
        &WorldObject,
        &GlobalTransform,
        Option<&Aabb>,
        &Visibility,
        &InheritedVisibility,
        &ViewVisibility,
    )>,
) {
    static UIDS: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    let uids = UIDS.get_or_init(|| {
        std::env::var("WOW_BLOB_VIS_UID")
            .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
            .unwrap_or_default()
    });
    let now = time.elapsed_secs();
    if now - *last < 2.0 {
        return;
    }
    *last = now;
    for (e, obj, xf, aabb, vis, inh, view) in &q {
        if obj.label != "static-merge" && !uids.contains(&obj.id) {
            continue;
        }
        let c = aabb.map_or(xf.translation(), |a| {
            xf.transform_point(Vec3::from(a.center))
        });
        let h = aabb.map_or(Vec3::ZERO, |a| Vec3::from(a.half_extents));
        eprintln!(
            "[blob-vis] t={now:.1} {e} {} #{} [{}] c=({:.0},{:.0},{:.0}) h=({:.0},{:.0},{:.0}) \
             vis={vis:?} inh={} view={}",
            obj.label,
            obj.id,
            obj.detail,
            c.x,
            c.y,
            c.z,
            h.x,
            h.y,
            h.z,
            inh.get(),
            view.get(),
        );
    }
}

/// Spawns one blob entity: no `DoodadFade` (the baked fade spheres drive `WOW_MERGED_FADE` and
/// `MeshTag` stays opaque), the union `Aabb` under `NoAutoAabb`, and a `PickBlob` so a pick names
/// the member placement it hits rather than the blob.
fn spawn_blob(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mat: &Handle<WowModelMaterial>,
    src: BlobSource<'_>,
    vis: Option<WmoGroupVis>,
    exterior: bool,
) -> Entity {
    let BlobSource {
        parts,
        objects,
        spheres,
        slots,
        blend,
        kind,
    } = src;
    let n = parts.len();
    // `center` is the blob's transparent sort key (the mesh is baked around it); at the world
    // origin a fader blob would sort first and hide what is behind it.
    let (mesh, mn, mx, center) =
        merged_static_mesh_faded(parts, spheres, (!slots.is_empty()).then_some(slots));
    // An interior blob's tag keeps the INTERIOR_FOG bit; the vertices carry the slot.
    let tag = if slots.is_empty() {
        alpha_bits(1.0)
    } else {
        crate::mesh_tag::probe_bits(0)
    };
    let mut blob = commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(mat.clone()),
        Transform::from_translation(center),
        ModelPart { kind, blend },
        MeshTag(tag),
        Aabb::from_min_max(mn, mx),
        NoAutoAabb,
        // The draw's own identity; the members answer the pick.
        WorldObject {
            kind,
            label: "static-merge".into(),
            id: 0,
            detail: format!("{n} batches merged"),
        },
        crate::interact::PickBlob(
            objects
                .iter()
                .zip(parts)
                .map(
                    |(object, (geometry, transform))| crate::interact::PickMember {
                        object: object.clone(),
                        geometry: geometry.clone(),
                        transform: *transform,
                    },
                )
                .collect(),
        ),
    ));
    if exterior {
        blob.insert(crate::exterior_cull::ExteriorScene);
    }
    if let Some(vis) = vis {
        blob.insert(vis);
    }
    blob.id()
}

#[cfg(test)]
mod tests {
    use super::super::{ModelHandle, Placement, Placements, TerrainStreamer, TileState, ViewFocus};
    use super::*;
    use benilla_assets::{ATTRIBUTE_WOW_FADE_SPHERE, ATTRIBUTE_WOW_MERGED_SLOT};
    use bevy::app::TaskPoolPlugin;
    use bevy::ecs::system::RunSystemOnce;

    fn geometry(verts: usize) -> Arc<RenderSubmesh> {
        Arc::new(RenderSubmesh {
            positions: vec![[0.0, 0.0, 0.0]; verts],
            ..Default::default()
        })
    }

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(TaskPoolPlugin::default());
        app.init_resource::<Assets<Mesh>>();
        app.init_resource::<StaticMerge>();
        app.init_resource::<TerrainStreamer>();
        app.init_resource::<super::super::Placements>();
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

    /// A test placement identity; every divert carries one.
    fn object() -> Arc<crate::interact::WorldObject> {
        Arc::new(crate::interact::WorldObject {
            kind: ModelKind::Doodad,
            label: "World\\test\\tree.m2".into(),
            id: 1,
            detail: String::new(),
        })
    }

    fn divert_doodad(merge: &mut StaticMerge, owner: (i32, i32), at: Vec3, verts: usize) {
        let mat = Handle::<WowModelMaterial>::default();
        assert!(merge.divert(
            &MergeSite::Doodad { owner },
            0,
            &mat,
            &geometry(verts),
            Transform::from_translation(at),
            Vec4::new(at.x, at.y, at.z, 1.5),
            ModelBlend::Opaque,
            ModelKind::Doodad,
            &object(),
        ));
    }

    #[test]
    fn cell_key_partitions_doodad_accumulators() {
        let mut merge = StaticMerge::default();
        divert_doodad(&mut merge, (0, 0), Vec3::new(1.0, 0.0, 1.0), 3);
        divert_doodad(&mut merge, (0, 0), Vec3::new(2.0, 0.0, 2.0), 3);
        divert_doodad(&mut merge, (0, 0), Vec3::new(CELL + 1.0, 0.0, 1.0), 3);
        assert_eq!(merge.doodads.len(), 2);
        let joint = merge
            .doodads
            .values()
            .find(|a| a.parts.len() == 2)
            .expect("same-cell parts share an accumulator");
        assert_eq!(joint.verts, 6);
        assert_eq!(joint.spheres.len(), 2);
    }

    #[test]
    fn wmo_site_refuses_the_divert() {
        let mut merge = StaticMerge::default();
        let mat = Handle::<WowModelMaterial>::default();
        assert!(!merge.divert(
            &MergeSite::Wmo {
                uid: 7,
                groups: &[4, 9],
                portal_gated: true,
            },
            0,
            &mat,
            &geometry(3),
            Transform::IDENTITY,
            Vec4::new(0.0, 0.0, 0.0, f32::INFINITY),
            ModelBlend::Opaque,
            ModelKind::Wmo,
            &object(),
        ));
        assert!(merge.doodads.is_empty() && merge.props.is_empty());
    }

    fn blank_placement(portal_instance: Option<Entity>) -> Placement {
        Placement {
            model: ModelHandle::M2(Default::default()),
            transform: Transform::IDENTITY,
            entities: Vec::new(),
            spawned: true,
            doodad_set: 0,
            name_set: 0,
            doodads: Vec::new(),
            portal_instance,
            refs: 1,
            owner: (0, 0),
        }
    }

    #[test]
    fn interior_prop_blob_carries_rooms_and_baked_slots() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = false;
        let instance = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<Placements>()
            .by_id
            .insert(7, blank_placement(Some(instance)));
        let rooms: Arc<[u16]> = Arc::from([3u16, 5].as_slice());
        {
            let mut merge = app.world_mut().resource_mut::<StaticMerge>();
            let mat = Handle::<WowModelMaterial>::default();
            for slot in [11u16, 12] {
                assert!(merge.divert(
                    &MergeSite::Prop {
                        uid: 7,
                        groups: &rooms,
                        slot: Some(slot),
                    },
                    0,
                    &mat,
                    &geometry(3),
                    Transform::IDENTITY,
                    Vec4::new(0.0, 0.0, 0.0, f32::INFINITY),
                    ModelBlend::Opaque,
                    ModelKind::Doodad,
                    &object(),
                ));
            }
        }
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        let placements = app.world().resource::<Placements>();
        let owned = placements.by_id.get(&7).unwrap().entities.clone();
        assert_eq!(owned.len(), 1);
        let blob = owned[0];
        let vis = app.world().get::<WmoGroupVis>(blob).unwrap();
        assert_eq!(vis.instance, instance);
        assert_eq!(&*vis.groups, &[3, 5]);
        // The tag keeps the interior-fog bit; its slot half is dead (probe 0).
        let tag = app.world().get::<MeshTag>(blob).unwrap();
        assert_eq!(tag.0, crate::mesh_tag::probe_bits(0));
        let mesh3d = app.world().get::<Mesh3d>(blob).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh3d).unwrap();
        // 3 verts per part, slots 11 then 12 replicated per vertex.
        match mesh.attribute(ATTRIBUTE_WOW_MERGED_SLOT).unwrap() {
            bevy::mesh::VertexAttributeValues::Uint32(v) => {
                assert_eq!(v, &[11, 11, 11, 12, 12, 12]);
            }
            other => panic!("slot attribute has the wrong format: {other:?}"),
        }
    }

    /// An exterior prop blob bakes no slot attribute; one no room names takes no vis key or tag.
    #[test]
    fn exterior_and_unnamed_prop_blobs_stay_plain() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = false;
        app.world_mut()
            .resource_mut::<Placements>()
            .by_id
            .insert(9, blank_placement(None));
        let rooms: Arc<[u16]> = Arc::from([].as_slice());
        {
            let mut merge = app.world_mut().resource_mut::<StaticMerge>();
            let mat = Handle::<WowModelMaterial>::default();
            assert!(merge.divert(
                &MergeSite::Prop {
                    uid: 9,
                    groups: &rooms,
                    slot: None,
                },
                0,
                &mat,
                &geometry(3),
                Transform::IDENTITY,
                Vec4::new(0.0, 0.0, 0.0, 4.0),
                ModelBlend::Opaque,
                ModelKind::Doodad,
                &object(),
            ));
        }
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        let placements = app.world().resource::<Placements>();
        let owned = placements.by_id.get(&9).unwrap().entities.clone();
        assert_eq!(owned.len(), 1);
        let blob = owned[0];
        assert!(app.world().get::<WmoGroupVis>(blob).is_none());
        assert!(app
            .world()
            .get::<crate::exterior_cull::ExteriorScene>(blob)
            .is_none());
        let mesh3d = app.world().get::<Mesh3d>(blob).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh3d).unwrap();
        assert!(mesh.attribute(ATTRIBUTE_WOW_MERGED_SLOT).is_none());
        assert!(mesh.attribute(ATTRIBUTE_WOW_FADE_SPHERE).is_some());
    }

    #[test]
    fn idle_tail_closes_a_doodad_blob_onto_its_tile() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((3, 4), blank_tile());
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (3, 4),
            Vec3::new(5.0, 0.0, 5.0),
            3,
        );
        for _ in 0..(MERGE_IDLE_FRAMES - 1) {
            app.world_mut().run_system_once(flush_static_merge).unwrap();
        }
        assert_eq!(app.world().resource::<StaticMerge>().doodads.len(), 1);
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        assert!(app.world().resource::<StaticMerge>().doodads.is_empty());
        let streamer = app.world().resource::<TerrainStreamer>();
        let owned = streamer.tiles.get(&(3, 4)).unwrap().merged.clone();
        assert_eq!(owned.len(), 1);
        let blob = owned[0];
        assert!(app.world().get::<NoAutoAabb>(blob).is_some());
        assert!(app
            .world()
            .get::<crate::model_fade::DoodadFade>(blob)
            .is_none());
        // One fade sphere per vertex: the `WOW_MERGED_FADE` contract.
        let mesh3d = app.world().get::<Mesh3d>(blob).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh3d).unwrap();
        let spheres = mesh.attribute(ATTRIBUTE_WOW_FADE_SPHERE).unwrap();
        assert_eq!(spheres.len(), 3);
    }

    #[test]
    fn dead_owner_discards_the_accumulator() {
        let mut app = test_app();
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (9, 9),
            Vec3::ZERO,
            3,
        );
        let before = app.world().entities().len();
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        assert!(app.world().resource::<StaticMerge>().doodads.is_empty());
        assert_eq!(app.world().entities().len(), before);
    }

    fn flush_n(app: &mut App, n: u32) {
        for _ in 0..n {
            app.world_mut().run_system_once(flush_static_merge).unwrap();
        }
    }

    fn tile_merged(app: &App, tile: (i32, i32)) -> Vec<Entity> {
        app.world()
            .resource::<TerrainStreamer>()
            .tiles
            .get(&tile)
            .unwrap()
            .merged
            .clone()
    }

    #[test]
    fn a_settled_cell_reconsolidates_its_fragments() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((3, 4), blank_tile());
        // Two admission waves, each closing on the idle tail → two fragment blobs.
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (3, 4),
            Vec3::new(1.0, 0.0, 1.0),
            3,
        );
        flush_n(&mut app, MERGE_IDLE_FRAMES);
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (3, 4),
            Vec3::new(2.0, 0.0, 2.0),
            3,
        );
        flush_n(&mut app, MERGE_IDLE_FRAMES);
        let fragments = tile_merged(&app, (3, 4));
        assert_eq!(fragments.len(), 2, "two waves must close two fragments");
        // The quiet window passes: the successor spawns while the fragments keep drawing.
        flush_n(&mut app, RECONSOLIDATE_IDLE_FRAMES);
        assert_eq!(
            tile_merged(&app, (3, 4)).len(),
            3,
            "the fragments must cover the successor's upload cushion"
        );
        // The cushion passes: the fragments retire, the consolidated blob remains.
        flush_n(&mut app, RETIRE_FRAMES);
        let after = tile_merged(&app, (3, 4));
        assert_eq!(after.len(), 1);
        assert!(!fragments.contains(&after[0]));
        for e in fragments {
            assert!(
                app.world().get_entity(e).is_err(),
                "a retired fragment must despawn"
            );
        }
        // Both placements' fade spheres, 3 + 3 vertices.
        let mesh3d = app.world().get::<Mesh3d>(after[0]).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let spheres = meshes
            .get(&mesh3d)
            .unwrap()
            .attribute(ATTRIBUTE_WOW_FADE_SPHERE)
            .unwrap();
        assert_eq!(spheres.len(), 6);
    }

    /// Two fragments at the vertex cap cannot shrink, so the originals stay.
    #[test]
    fn reconsolidation_declines_when_the_cap_leaves_no_win() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((0, 0), blank_tile());
        for _ in 0..2 {
            divert_doodad(
                &mut app.world_mut().resource_mut::<StaticMerge>(),
                (0, 0),
                Vec3::ZERO,
                MERGE_MAX_VERTS,
            );
            // The cap closes each at once: two fragments under one key.
            flush_n(&mut app, 1);
        }
        let fragments = tile_merged(&app, (0, 0));
        assert_eq!(fragments.len(), 2);
        flush_n(&mut app, RECONSOLIDATE_IDLE_FRAMES + RETIRE_FRAMES);
        assert_eq!(
            tile_merged(&app, (0, 0)),
            fragments,
            "capped fragments must stay exactly as they were"
        );
    }

    /// The tile's own unload despawned the entities, so the ledger drops without touching them.
    #[test]
    fn a_dead_owner_drops_the_settled_ledger() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((5, 5), blank_tile());
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (5, 5),
            Vec3::ZERO,
            3,
        );
        flush_n(&mut app, MERGE_IDLE_FRAMES);
        assert_eq!(app.world().resource::<StaticMerge>().settled.len(), 1);
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .remove(&(5, 5));
        flush_n(&mut app, 1);
        assert!(app.world().resource::<StaticMerge>().settled.is_empty());
    }

    #[test]
    fn vert_cap_closes_immediately() {
        let mut app = test_app();
        app.world_mut().resource_mut::<ViewFocus>().paced = true;
        app.world_mut()
            .resource_mut::<TerrainStreamer>()
            .tiles
            .insert((0, 0), blank_tile());
        divert_doodad(
            &mut app.world_mut().resource_mut::<StaticMerge>(),
            (0, 0),
            Vec3::ZERO,
            MERGE_MAX_VERTS,
        );
        app.world_mut().run_system_once(flush_static_merge).unwrap();
        let streamer = app.world().resource::<TerrainStreamer>();
        assert_eq!(streamer.tiles.get(&(0, 0)).unwrap().merged.len(), 1);
    }
}
