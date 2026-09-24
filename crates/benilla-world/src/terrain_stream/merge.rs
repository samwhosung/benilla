//! **The production static-world consolidation, doodad lanes — ON by default** (1426;
//! `WOW_STATIC_MERGE=0` opts out. Design 1417, lane order re-ranked by 1418's density
//! verdict; the measured premise is 1413/1416's 44 ns/row tax).
//!
//! **Since 1434 this is the SECOND consolidator in line**: the retained pass
//! (`crate::static_gx`, also default-on) takes the static populations first at the same
//! assemble gate, and what reaches these accumulators under the defaults is its declined
//! families (env-mapped, depth-flagged batches) plus everything under `WOW_STATIC_GX=0`,
//! where this module is again the whole consolidation. The default flip deliberately
//! removed no code here — this lane IS the A/B arm every gx comparison diffs against.
//!
//! The assembler diverts every ADT-doodad batch that is fully static (the bracket's anim
//! exclusions), **order-free** (`Opaque`/`AlphaTest`, not additive: 0858's law that authored
//! draw order exists only on transparent-pass batches) and not an interior-slot prop, into
//! this buffer; the flush bakes each `(owner tile, 133⅓-yd cell, material)` group into ONE
//! mesh entity with placement transforms baked into the vertices. **The fader lane (lane 2)
//! is dev-opt-in only (`WOW_MERGE_FADERS=1`)**: each vertex carries its
//! placement's fade sphere ([`benilla_assets::ATTRIBUTE_WOW_FADE_SPHERE`]) and
//! `wow_model.wgsl`'s `WOW_MERGED_FADE` lane computes the faithful fade curve per vertex —
//! alpha in-shader, `Hidden` as a clip-space collapse at zero — on the BLEND TWIN permanently
//! (1420), the reference's own fading render state. The lane is correct per PIXEL but wrong
//! per PHASE: one transparent draw with one sort key spanning a cell of depths, depth-write
//! on, depth-kills per-entity faders behind its translucent pixels whenever the cell's sort
//! centre lands beyond them (the director's popping lamppost — 1422 shrank the class,
//! 1423 pulled the lane; the sort-near re-entry design is 1423's follow-up).
//!
//! The cell key preserves the frustum-cull locality the bracket's round 1 proved load-bearing
//! (+1.38 without it, −0.93 with); the owner tile buys the weld's whole lifetime story — the
//! blob lands in `TileState::merged` and despawns with its tile.
//!
//! **WMO group geometry never diverts (1418's verdict):** `batch_order` is a `MatKey` axis, so
//! every WMO batch already owns a unique material handle — under the correct
//! `(uid, group, material)` key the measured merge is EXACTLY 1:1, zero rows saved. The WMO
//! share of the frame belongs to option B's cross-material retained draw, not to any
//! entity-level lane. [`MergeSite::Wmo`] survives only to feed the census predictor.
//!
//! The close rule is the weld's (1369), not the bracket's wall clock: vertex cap + idle-frame
//! tail — [`MERGE_IDLE_FRAMES`] (a quarter second) in settled play, ONE quiet frame under the
//! arrival cover, where the settle release waits on the backlog (`merge_pending` is a
//! `presentable()` term) and a fixed tail would push every world entry longer for nothing.
//! Play-streaming then RE-CONSOLIDATES: distance admission trickles a cell's doodads in over
//! the whole approach, gapped in seconds, so in-play closes shred a cell into fragments
//! (Goldshire: 214/445 singleton blobs — 1421's census; its "1-frame tail" attribution was a
//! misread, the in-play tail was already ¼ s and no sane tail bridges an approach). Fragments
//! spawn fast — appearance latency is the tail's job — and once a cell's key has been quiet
//! for [`RECONSOLIDATE_IDLE_FRAMES`] its fragments re-bake into one blob per vertex cap and
//! the originals retire after an upload cushion ([`RETIRE_FRAMES`]).
//! Dead-owner accumulators are discarded, and the whole buffer clears on map drop for the same
//! tile-keys-repeat-across-maps reason the weld's does.
//!
//! The straddler gap (1417's lifecycle §) is CLOSED: when an owner tile unloads out from under
//! a placement a loaded neighbour still references, `handoff_straddlers` (terrain_stream.rs)
//! re-owns the placement to that referrer and queues a respawn, so its batches re-divert here
//! under the new owner — and its hulls re-weld, closing 1369's matching collider gap whenever
//! the merge is on.

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

/// Vertex cap per blob: bounds any one bake + upload, and keeps a blob's cull bound a
/// neighbourhood rather than a zone (the weld's `WELD_MAX_TRIS` argument, in render units). A
/// single oversized batch closes its blob immediately, same as an oversized hull.
const MERGE_MAX_VERTS: usize = 65_536;

/// Quiet frames that close a live accumulator in settled play — a quarter second, because a
/// budget-paced burst's gaps are frames, not seconds. Under the arrival cover ONE quiet frame
/// closes it instead: the settle release waits on the merge backlog, and a fixed tail would
/// push every world entry longer for nothing (the weld's exact rule — its comment owns the
/// reasoning).
const MERGE_IDLE_FRAMES: u32 = 15;

/// Quiet frames before a settled cell's fragments re-bake into one blob (~3 s at 60 Hz): long
/// enough that the approach's distance-admission trickle has moved past the cell, short enough
/// that the fragment population stays transient. This is why the in-play tail can stay a
/// quarter second: fast appearance first, consolidation once the cell goes quiet.
const RECONSOLIDATE_IDLE_FRAMES: u32 = 180;

/// Frames a replaced fragment keeps drawing after its consolidated successor spawns — the
/// successor's mesh-upload cushion. The merged lanes are order-free (`Opaque`/`AlphaTest`), so
/// the overlap double-draw is invisible; a gap instead (a despawn before the upload lands)
/// would flicker the cell off, the exact defect class the 1421–1423 hunt closed.
const RETIRE_FRAMES: u32 = 10;

/// The doodad spatial cell, ¼ of an ADT tile — the retired `WOW_MEGA_STATIC` bracket's measured
/// 133⅓-yd locality key. Its round 1 grouped by material alone and LOST (+1.38 cpu_ms at SW,
/// drawn 400 → ~830): one blob per material spans the whole streamed scene, so its Aabb defeats
/// the frustum cull and every blob's full vertex load encodes every frame. The cell restores
/// locality; the distinct-material count (~5.6k at SW) remains the blob-count floor either way —
/// the census finding that per-material merging alone cannot reach the few-hundred-row regime.
const CELL: f32 = 533.333_3 / 4.0;

/// One accumulating blob: shared geometry + placement transforms + per-placement fade
/// spheres (index-parallel with `parts`), baked at flush.
struct MergeAcc {
    parts: Vec<(Arc<RenderSubmesh>, Transform)>,
    /// Each part's placement identity (index-parallel with `parts`) — what the pick names when
    /// the cursor lands on this blob. Shared per placement, so a merged cell of
    /// 300 trees holds 300 refcount bumps, not 300 strings.
    objects: Vec<Arc<crate::interact::WorldObject>>,
    spheres: Vec<Vec4>,
    /// Interior-prop accs only (index-parallel with `parts`): each part's SH-probe slot, baked
    /// per vertex at flush. Empty on every other lane — homogeneous per key by construction,
    /// because the interior flag is a material axis and the material is in the key.
    slots: Vec<u32>,
    verts: usize,
    blend: ModelBlend,
    kind: ModelKind,
    /// [`StaticMerge::frame`] at the last append — the idle clock.
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

/// One blob bake's input — an accumulator's view at close, or a re-consolidated cell's
/// concatenation. Index-parallel slices; `slots` is empty on every non-interior lane.
struct BlobSource<'a> {
    parts: &'a [(Arc<RenderSubmesh>, Transform)],
    objects: &'a [Arc<crate::interact::WorldObject>],
    spheres: &'a [Vec4],
    slots: &'a [u32],
    blend: ModelBlend,
    kind: ModelKind,
}

/// Where a diverted batch belongs — built once per placement by the spawn driver, consumed per
/// batch by the assembler's divert.
pub enum MergeSite<'a> {
    /// An ADT map doodad: owned by its first-registering tile (the weld's ownership).
    Doodad { owner: (i32, i32) },
    /// WMO group geometry: owned by its placement; `groups` is the asset's per-submesh group
    /// index table (index-parallel with the submeshes the assembler iterates).
    Wmo {
        uid: u32,
        groups: &'a [u16],
        portal_gated: bool,
    },
    /// A WMO doodad prop (1418 lane 3): owned by its placement, keyed by the referrer-set of
    /// rooms that name it (`groups` — the blob takes the same set-valued `WmoGroupVis` its
    /// members carried) and, for an interior prop, carrying the per-prop SH-probe slot the
    /// bake writes per vertex.
    Prop {
        uid: u32,
        groups: &'a Arc<[u16]>,
        slot: Option<u16>,
    },
}

impl MergeSite<'_> {
    /// The would-be merge key of one batch under this site, hashed — the census's blob-count
    /// predictor (each lane's expected blob count = distinct keys in its class). Every site's
    /// key here mirrors its real divert key exactly.
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

/// (owner tile, 133⅓-yd cell, material) — a doodad blob's identity.
type DoodadKey = ((i32, i32), (i32, i32), Handle<WowModelMaterial>);

/// One spawned doodad blob retained with its bake input, so a shredded cell can re-bake.
struct SettledBlob {
    entity: Entity,
    parts: Vec<(Arc<RenderSubmesh>, Transform)>,
    objects: Vec<Arc<crate::interact::WorldObject>>,
    spheres: Vec<Vec4>,
    verts: usize,
}

/// A key's spawned blobs — the re-consolidation ledger's entry. `dirty` arms one evaluation
/// after the next quiet window; a declined evaluation (nothing to win under the cap) does not
/// re-run until a new fragment lands.
struct SettledCell {
    blobs: Vec<SettledBlob>,
    blend: ModelBlend,
    kind: ModelKind,
    /// [`StaticMerge::frame`] at the last fragment spawn — the quiet clock.
    last_add: u32,
    dirty: bool,
}

/// A replaced fragment drawing out its successor's upload cushion (see [`RETIRE_FRAMES`]).
struct Retiring {
    entity: Entity,
    owner: (i32, i32),
    /// [`StaticMerge::frame`] when the successor spawned.
    since: u32,
}
/// (placement uid, referrer-set, material) — a prop blob's identity. The `Arc<[u16]>` hashes
/// by CONTENT, so two props named by the same rooms share a blob and distinct sets never can.
type PropKey = (u32, Arc<[u16]>, Handle<WowModelMaterial>);

/// The in-flight merge accumulators. Same lifecycle discipline as [`super::weld::HullWelds`]:
/// fed by the spawn chain, drained one chain-step later, cleared with the world it describes.
#[derive(Resource, Default)]
pub struct StaticMerge {
    /// Flush-system tick, the idle clock. Wrapping u32 — only ever read as a difference.
    frame: u32,
    doodads: HashMap<DoodadKey, MergeAcc>,
    props: HashMap<PropKey, MergeAcc>,
    /// Spawned doodad blobs retained per key with their bake inputs — the re-consolidation
    /// ledger (see the module header: play-streaming's distance admission shreds a cell).
    settled: HashMap<DoodadKey, SettledCell>,
    /// Replaced fragments still drawing out their successor's upload cushion.
    retiring: Vec<Retiring>,
    /// Running totals since the last drain report (1417's VRAM honesty line): blobs spawned,
    /// batches baked into them, vertices BAKED (every placement a copy) vs the vertices the
    /// members' SHARED assets hold (each distinct geometry once — Arc identity), so the log
    /// states the duplication factor the desk estimate guessed at ~3×.
    blobs: u64,
    batches: u64,
    baked_verts: u64,
    shared_verts: u64,
    seen_geometry: std::collections::HashSet<usize>,
    reported: bool,
}

/// Is the consolidation armed? Read once; the assembler divert and the flush both key on it.
/// **ON by default** (1426: both recorded blockers closed — 1424 fragmentation, 1425 straddler
/// handoff — and the director's eye passed lanes 1+3 at both pins). `WOW_STATIC_MERGE=0` is
/// the opt-out lever; `=1` still reads as an explicit on for anything that predates the flip.
pub fn merge_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_STATIC_MERGE").as_deref() != Ok("0"))
}

impl StaticMerge {
    /// Take one mergeable batch into its accumulator. `fade_sphere` = the placement's world
    /// fade center + radius, baked per vertex at flush (a never-fader carries its true radius
    /// and the shader's `> 7` arm pins it opaque). `false` = this site never merges (WMO group
    /// geometry and props — 1418's verdict / the referrer-set key) — the caller spawns the
    /// batch individually, the fail-open arm.
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
                // The material's interior axis makes a key all-interior or all-exterior; a
                // ragged slot list would misindex the per-vertex bake, so it is a hard error.
                debug_assert!(acc.slots.is_empty() || acc.slots.len() == acc.parts.len() + 1);
                acc
            }
            // WMO group geometry never merges: measured 1:1 under its correct key (1418 —
            // batch_order rides MatKey). The site exists for the census predictor.
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

    /// Accumulators not yet baked — the reveal gate's term (`WorldLoadProgress::merge_pending`;
    /// the weld's `unflushed` argument, on the render side). An overcount only delays a
    /// release, never wrongs one.
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

/// Close ready accumulators into blob entities and hand each to its owner tile
/// (`TileState::merged` — despawned with the tile, like the welds). A dead owner discards its
/// accumulator: spawning a blob nothing owns is a leak (the weld's rule, and its reachability
/// argument — the owner died past the unload line).
///
/// Runs in the Stream chain right after `flush_hull_welds`, for the weld's own reason: the
/// frame's appends see the flush at a deterministic point and the owner lookups race nothing.
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
            // Retain the bake input: a later admission wave under this key makes the cell a
            // re-consolidation candidate once it goes quiet.
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
        // The blob takes exactly the vis/tagging its members had (spawn/mod.rs's prop site):
        // the set-valued `WmoGroupVis` + `ExteriorScene` when the building has an instance and
        // rooms name the prop; untagged otherwise (no key ⇒ no exemption possible).
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
    // Re-consolidate settled cells (the module header owns the why): once a key has been quiet
    // for the window and holds two or more fragments, re-bake them into one blob per vertex cap
    // and retire the originals after the upload cushion. First-fit over whole fragments; only a
    // strictly smaller shape is worth the churn.
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
    // Retire replaced fragments once the successor's upload cushion has passed. A dead owner
    // already despawned the entity with its tile — drop the record without touching it.
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
    // Publish the backlog for the reveal gate (this system sits in the Stream chain, so the
    // consumers read this frame's depth — the weld's publish discipline).
    if let Some(progress) = progress.as_mut() {
        progress.merge_pending = merge.unflushed();
    }
    // The drain report (1417's VRAM honesty line), once per settled wave: what the merge took
    // and what the transform-baking duplication actually costs against the shared assets.
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

/// `WOW_MERGE_FADERS=1` — the fader lane (lane 2) is OPT-IN dev-only since decision 1423:
/// a fader blob is one transparent draw with ONE sort key covering a 133⅓-yd cell of content
/// at DIFFERENT depths, drawn with the twin's faithful depth-write — and whenever the cell's
/// sort centre lands beyond a per-entity fader, the blob draws first and its translucent
/// pixels depth-kill the entity behind them (the director's popping lamppost; 1422's
/// centre-sort only shrank the class, it cannot eliminate it). Under the default merge
/// faders spawn per-entity — the configuration the director's eye passed. 1423's recorded
/// re-entry (sort-near on a blob twin) was REFUTED before build (1428): the blob's
/// opaque-intent pixels ride the same near-sorted transparent draw and erase any
/// non-depth-writing card in front of them, O(1) and permanent. Lane 2 stays parked; the
/// question folds into option B's retained draw, which bins opaque-intent content properly
/// by construction.
pub(crate) fn merge_faders_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_MERGE_FADERS").is_some())
}

/// `WOW_BLOB_VIS=1` — dump every merged blob's live visibility verdict every 2 s, plus any
/// entity whose `WorldObject::id` is listed in `WOW_BLOB_VIS_UID` (comma-separated). The
/// instrument for a "this popped in/out" report: run parked at the report's two points and
/// diff which row flips (`vis`/`inh`/`view`), which separates a positional cull flip from a
/// temporal one (a live pipeline compile, a late bake).
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

/// One blob bake (a closed accumulator, or a re-consolidated cell's chunk) → one blob entity
/// carrying exactly what its members carried minus
/// the per-placement machinery the shader lane now owns (1418): no `DoodadFade` (the baked
/// fade spheres drive `WOW_MERGED_FADE`; `MeshTag` stays at opaque), `ExteriorScene` (every
/// member had it), the union `Aabb` (authored: `NoAutoAabb`).
///
/// Its pick declaration is a [`PickBlob`], not a `PickMesh`: the baked mesh is the union of
/// every member, so casting against it would answer "static-merge", which is what this blob's
/// `WorldObject` said and why 1418's "nameable, not pickable" was a real loss of the inspector
/// over merged content. The members carry the placement identities, and the
/// entity keeps the broad phase and the drawn test it always had.
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
    // `center` is the blob's world position and therefore its TRANSPARENT-PHASE SORT KEY (the
    // mesh is baked blob-local around it). On `Transform::IDENTITY` a fader
    // blob sorted at the world origin: drawn first among all transparent content, its
    // depth-write killing every transparent entity behind its translucent pixels.
    let (mesh, mn, mx, center) =
        merged_static_mesh_faded(parts, spheres, (!slots.is_empty()).then_some(slots));
    // An interior blob's tag keeps the members' INTERIOR_FOG staging bit (the slot half of the
    // payload is dead under WOW_MERGED_SLOT — the vertices carry it).
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
        // The blob's own identity is the DRAW's, not a placement's — the members answer the
        // pick, and this is only what a query for the blob entity itself reads.
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

    /// A test placement identity — every divert carries one.
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

    /// Two doodads in the same cell on the same material accumulate into ONE blob; a third in
    /// a different cell opens a second accumulator (the locality key round 1 proved out).
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

    /// WMO group geometry never diverts (1418's 1:1 verdict) — the site exists for the census
    /// predictor only.
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

    /// An interior-prop blob (1418 lane 3) lands in its placement's entity list carrying the
    /// set-valued room key, the per-vertex probe slots, and the INTERIOR_FOG-staged tag.
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
        // The tag keeps the interior-fog staging bit with the slot half dead (probe 0).
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

    /// An EXTERIOR prop blob (no slots) bakes no slot attribute, and a prop no room names
    /// takes no vis key and no exterior tag (the untagged-not-gated-blind rule).
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

    /// The idle tail closes a quiet doodad accumulator into a blob owned by its tile, with the
    /// authored bound, the per-vertex fade spheres, and no per-entity fade enrollment.
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
        // The baked mesh carries one fade sphere per vertex — the WOW_MERGED_FADE contract.
        let mesh3d = app.world().get::<Mesh3d>(blob).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let mesh = meshes.get(&mesh3d).unwrap();
        let spheres = mesh.attribute(ATTRIBUTE_WOW_FADE_SPHERE).unwrap();
        assert_eq!(spheres.len(), 3);
    }

    /// A dead owner discards the accumulator — no blob, no leak (the weld's rule).
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

    /// Play-streaming's admission trickle closes a cell in fragments; once the key goes quiet
    /// they re-bake into ONE blob, the originals drawing out the upload cushion before retiring.
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
        // The consolidated mesh carries both placements — per-vertex fade spheres for 3+3 verts.
        let mesh3d = app.world().get::<Mesh3d>(after[0]).unwrap().0.clone();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let spheres = meshes
            .get(&mesh3d)
            .unwrap()
            .attribute(ATTRIBUTE_WOW_FADE_SPHERE)
            .unwrap();
        assert_eq!(spheres.len(), 6);
    }

    /// Two fragments already at the vertex cap re-bake into the same count — no win, so the
    /// evaluation declines, keeps the originals, and does not run again until a new fragment.
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
            // The cap closes each immediately — two fragments under one key.
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

    /// A dead owner drops its settled ledger without touching entities — the tile's own unload
    /// despawned them, and a second despawn would be a bug.
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

    /// The vertex cap closes an accumulator without waiting for the idle tail.
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
