//! The retained static-world pass, on unless `WOW_STATIC_GX=0`: ADT doodads, WMO group geometry
//! and WMO doodad props draw from retained, recentred buffers in one render node instead of
//! bevy_pbr, at pixel parity with the entity path (`WOW_STATIC_MERGE=0 WOW_STATIC_GX=0`).
//! Visibility is a CPU scene walk feeding retained draws, the reference's own shape. Only fade-1
//! content draws here: a placement inside its fade band is exiled to ordinary entities.
//!
//! Declined batches fall through to the merge or entity path, counted in a census: env-mapped
//! (`texture_unit_lookup > 2`, view-generated UVs), depth-flagged, the `ShadeSel::Rig` family,
//! exterior fader props, and props of a placement without a portal instance.

use bevy::camera::primitives::Aabb;
use bevy::mesh::MeshVertexAttribute;
use bevy::prelude::*;
use bevy::render::render_resource::VertexFormat;
use std::sync::Arc;

use crate::model_render::ShadeSel;
use benilla_formats::{ModelBlend, RenderSubmesh, WmoBatchClass};

mod bake;
mod cull;
mod pick;
mod pool;
mod render;

/// The doodad spatial cell, a quarter ADT tile (133⅓ yd), as `terrain_stream::merge::CELL`.
const CELL: f32 = 533.333_3 / 4.0;

/// Quiet frames before a dirty cell's first bake (~¼ s at 60 Hz).
const IDLE_FRAMES: u32 = 15;
/// Quiet frames before a published cell re-bakes (~2 s), wider than the admission trickle's gaps.
const REBAKE_FRAMES: u32 = 120;
/// A dirty cell this old (~10 s) bakes even if never quiet, so a trickle cannot starve it.
const MAX_DIRTY_FRAMES: u32 = 600;

/// Per-vertex word: the item index in bits 0..16, flag bits above, as `static_gx.wgsl` reads it.
pub const ATTRIBUTE_GX_WORD: MeshVertexAttribute =
    MeshVertexAttribute::new("Gx_Word", 988_101, VertexFormat::Uint32);
/// Per-vertex point-light anchor: the placement's world origin, as in `wow_model.wgsl`.
pub const ATTRIBUTE_GX_ANCHOR: MeshVertexAttribute =
    MeshVertexAttribute::new("Gx_Anchor", 988_102, VertexFormat::Float32x3);

const WORD_WRAP_X: u32 = 1 << 16;
const WORD_WRAP_Y: u32 = 1 << 17;
const WORD_UNLIT: u32 = 1 << 18;
const WORD_FOG_OFF: u32 = 1 << 19;
const WORD_SHADE_LIT: u32 = 1 << 20;
const WORD_TEXTURED: u32 = 1 << 21;
// The WMO lane, the entity path's material facts as bits. model_flags.x: a WMO surface.
const WORD_WMO: u32 = 1 << 22;
// model_flags.z: an interior group.
const WORD_INTERIOR: u32 = 1 << 23;
// tint.w == 1 and == 2: the MOBA INT and TRANS classes; EXT has both clear.
const WORD_CLASS_INT: u32 = 1 << 24;
const WORD_CLASS_TRANS: u32 = 1 << 25;
// sidn.w: the MOMT WINDOW midpoint light.
const WORD_WINDOW: u32 = 1 << 26;
// The batch authors vertex colours. White passes every lane unchanged except INT's
// ×(1 + 4·MOCV.a) self-illumination, which a colourless entity batch skips, so it keys on this.
const WORD_HAS_VC: u32 = 1 << 27;
// `ShadeSel::Matte`, fixed 1.0: a map doodad never reaches the 2.5 site (`0x69e4ad`). Not
// `WORD_SHADE_LIT`, so lifting the shader's `min(I, 1)` cap leaves this at 1.0.
const WORD_MATTE: u32 = 1 << 28;
// An interior M2 prop is WORD_INTERIOR with WORD_WMO clear, the entity shader's `interior_prop =
// flags.z && !flags.x`: SH-probe light from its record slot, interior fog, no live point lights.

/// On unless `WOW_STATIC_GX=0`; read once, and the plugin registers nothing when off.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_STATIC_GX").as_deref() != Ok("0"))
}

/// `WOW_WMO_BIAS=0`: the authored batch-order nudge bakes as zero, as in `model_material`.
fn wmo_bias_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_WMO_BIAS").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_WMO=0`: WMO group batches fall back to the entity path; cells are unaffected.
fn wmo_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_WMO").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_FADE=0`: fader batches fall back to the entity path; the rest is unaffected.
pub(crate) fn fade_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_FADE").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_PROP=0`: WMO-prop batches fall back to the merge or entity path.
fn prop_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_PROP").as_deref(), Ok("0")))
}

/// `WOW_GX_PERF=1`: system wall times into [`GX_PERF`], printed and zeroed every 64 frames.
pub(crate) fn gx_perf_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_GX_PERF").is_some())
}

/// Lifetime bytes of texture-array VRAM the pool has allocated, never decremented.
pub(crate) static GX_VRAM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Nanosecond accumulators: flush, cull, publish, prepare, node.
pub(crate) static GX_PERF: [std::sync::atomic::AtomicU64; 5] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// Scope guard adding one site's wall time to [`GX_PERF`] slot `i`; a no-op when the meter is off.
pub(crate) struct GxPerfGuard(Option<(usize, std::time::Instant)>);
impl Drop for GxPerfGuard {
    fn drop(&mut self) {
        if let Some((i, t0)) = self.0 {
            GX_PERF[i].fetch_add(
                u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
    }
}
pub(crate) fn gx_perf_guard(i: usize) -> GxPerfGuard {
    GxPerfGuard(gx_perf_enabled().then(|| (i, std::time::Instant::now())))
}

/// One diverted batch, kept after its cell bakes so an unload or a later admission can re-bake.
struct GxItem {
    geometry: Arc<RenderSubmesh>,
    transform: Transform,
    /// The placement identity the pick names, shared by the placement's batches.
    object: Arc<crate::interact::WorldObject>,
    /// The batch's model-local bound for the pick's broad phase; `None` is always narrow-tested.
    local_aabb: Option<Aabb>,
    /// The owner tile, read only by [`StaticGx::release_owner`]; regions follow their instance.
    owner: (i32, i32),
    texture: Option<AssetId<bevy::image::Image>>,
    /// Keeps the render world's `GpuImage` alive under the baked cell; the id alone holds nothing.
    _texture_handle: Option<Handle<bevy::image::Image>>,
    cutout: bool,
    two_sided: bool,
    unlit: bool,
    fog_off: bool,
    shade_lit: bool,
    wrap_x: bool,
    wrap_y: bool,
    /// The `ShadeSel::Matte` sun family, intensity fixed at 1.0.
    matte: bool,
    wmo: Option<GxItemWmo>,
    prop: Option<GxItemProp>,
    /// `Some(uid)` on a fader item: the placement it exiles with.
    fader: Option<u32>,
}

/// A WMO-prop item's referrer set (an index into [`GxCell::sets`]) and folded SH-probe slot;
/// `None` takes exterior light, as on the entity path.
struct GxItemProp {
    set: u16,
    slot: Option<u16>,
}

/// One fader placement's exile seed and state, keyed by uniqueId and released by owner tile.
struct GxFader {
    owner: (i32, i32),
    uid: u32,
    /// Carried onto the exiled entities, so the pick names the same placement in either lane.
    object: Arc<crate::interact::WorldObject>,
    transform: Transform,
    radius: f32,
    local_center: Vec3,
    /// World fade-sphere centre, which the scan measures horizontal distance to: the reference's
    /// transformed centre (`0x6952a0`).
    center: Vec3,
    /// Band edges from [`crate::model_fade::fade_band`], for the cell ring test only.
    near: f32,
    far: f32,
    batches: Vec<GxFaderBatch>,
    /// This placement's item indices in the current bake, refreshed by every flush.
    items: Vec<u16>,
    state: FaderState,
}

/// One diverted batch's respawn payload: handle clones of the bundle `assemble.rs` would spawn.
struct GxFaderBatch {
    stat_mesh: Handle<Mesh>,
    aabb: Option<bevy::camera::primitives::Aabb>,
    cutout: Handle<benilla_assets::materials::WowModelMaterial>,
    blend: Handle<benilla_assets::materials::WowModelMaterial>,
    blend_mode: ModelBlend,
    geometry: Arc<RenderSubmesh>,
}

/// A fader's exile state. Steady: drawn retained at fade 1. Exiled: drawn as ordinary entities,
/// its retained items killed once `armed`. Gone: fully faded, killed, nothing spawned.
///
/// `armed` waits a frame: a fresh exile's entities first draw the next frame, so killing its
/// retained items on the spawn frame would leave a one-frame hole.
enum FaderState {
    Steady,
    Exiled { ents: Vec<Entity>, armed: bool },
    Gone,
}

/// A WMO item's baked facts (see [`GxWmoBatch`]).
struct GxItemWmo {
    group: u16,
    interior: bool,
    /// The batch-class lane as `model_render` packs `tint.w`: 0 EXT, 1 INT, 2 TRANS.
    class_lane: u8,
    sidn: [u8; 3],
    window: bool,
    /// The authored batch order for the clip-z nudge; 0 under `WOW_WMO_BIAS=0`.
    order: u16,
}

/// A cell's or region's collected items and bake state.
#[derive(Default)]
struct GxCell {
    items: Vec<GxItem>,
    dirty: bool,
    last_change: u32,
    /// The frame `dirty` last went from false to true: [`MAX_DIRTY_FRAMES`]'s clock.
    dirty_since: u32,
    /// The cell's fader placements by uniqueId; empty on regions.
    faders: bevy::platform::collections::HashMap<u32, GxFader>,
    /// The published kill bitmap no longer matches the fader states; the scan rebuilds it.
    bits_stale: bool,
    /// XZ bounds of the faders' centres, for the scan's ring test.
    fader_bounds: Option<(Vec2, Vec2)>,
    /// `(min near, max far)` over the cell's faders: the ring test's band union.
    ring: (f32, f32),
    /// The scan's last wholesale verdict (all steady or all gone); `None` walks every frame.
    settled: Option<bool>,
    /// A prop region's distinct referrer sets, indexed by [`GxItemProp::set`].
    sets: Vec<Arc<[u16]>>,
}

/// The main-world collector and its published draw set.
#[derive(Resource, Default)]
pub struct StaticGx {
    cells: bevy::platform::collections::HashMap<(i32, i32), GxCell>,
    /// The WMO regions, keyed by placement instance entity, the portal PVS's own key.
    wmos: bevy::platform::collections::HashMap<Entity, GxCell>,
    /// The prop regions, keyed by the same instance entity but apart from [`Self::wmos`]: props
    /// trickle in as their M2s load, and sharing would re-bake the building per arrival.
    props: bevy::platform::collections::HashMap<Entity, GxCell>,
    /// The published half the render node draws.
    pub(crate) world: render::GxWorld,
    frame: u32,
    /// Declined-batch census: env-map, depth-flag, shade-family, prop-fader, prop-no-instance.
    declined: [u32; 5],
    declined_logged: [u32; 5],
    /// The census's last-seen counts, the frame they last moved and the frame it last printed:
    /// with one stamp, a count that moves every frame would never print.
    declined_seen: [u32; 5],
    declined_changed: u32,
    declined_printed: u32,
    /// Printed beside the refusals, so a thin population never reads as a dead pass.
    accepted: u32,
    /// Exiled entities whose seed died (owner release, map clear), despawned by the next scan.
    pending_despawn: Vec<Entity>,
    /// Lifetime exile events for the census: spawns, re-admits, gone-despawns.
    fade_events: [u32; 3],
    /// [`StaticGx::flush_now`]'s request, cleared by the next flush.
    flush_now: bool,
}

/// The divert hook's per-call facts, gathered at `assemble.rs`'s gate.
pub struct GxBatch<'a> {
    pub geometry: &'a Arc<RenderSubmesh>,
    pub transform: Transform,
    /// The placement's shared identity, the pick's answer for geometry no entity owns.
    pub object: &'a Arc<crate::interact::WorldObject>,
    /// The batch's model-local build-time bound, for the pick's broad phase.
    pub aabb: Option<Aabb>,
    /// The owner tile: a cell item's release key, unread for region items.
    pub owner: (i32, i32),
    pub texture: Option<Handle<bevy::image::Image>>,
    pub blend: ModelBlend,
    pub two_sided: bool,
    pub unlit: bool,
    pub fog_policy: benilla_formats::FogPolicy,
    pub env_map: bool,
    pub no_depth_write: bool,
    pub no_depth_test: bool,
    /// Unread for WMO group batches, whose entity material never reads it.
    pub shade: ShadeSel,
    /// `Some` for a WMO group-geometry batch.
    pub wmo: Option<GxWmoBatch>,
    /// `Some` for a WMO-prop batch.
    pub prop: Option<GxPropBatch>,
    /// `Some` for a fader batch: its exile seed. Never set on WMO or prop batches.
    pub fade: Option<GxFadeSeed>,
}

/// The prop half of [`GxBatch`]: the building's instance entity (region key, PVS source and
/// lifetime), the rooms naming the prop (empty: always admitted) and its SH-probe slot.
pub struct GxPropBatch {
    pub instance: Entity,
    pub groups: Arc<[u16]>,
    pub slot: Option<u16>,
}

/// The fader half of [`GxBatch`]: the fade sphere and the bundle the exile respawns.
pub struct GxFadeSeed {
    /// Bounding-sphere radius in yards, placement-scaled; selects the fade band.
    pub radius: f32,
    /// Model-local bbox centre; the fade distance is measured to its world image.
    pub local_center: Vec3,
    pub stat_mesh: Handle<Mesh>,
    pub aabb: Option<bevy::camera::primitives::Aabb>,
    pub cutout: Handle<benilla_assets::materials::WowModelMaterial>,
    pub blend: Handle<benilla_assets::materials::WowModelMaterial>,
}

/// The population a diverted batch joins: cells release by owner tile, regions by instance.
pub enum GxSite<'a> {
    /// A world-static ADT doodad placement: cell items.
    Doodad { owner: (i32, i32) },
    /// A WMO placement's group geometry, with the per-batch group map (`WmoModel::submesh_group`).
    Wmo { instance: Entity, groups: &'a [u16] },
    /// A WMO doodad prop: the building's instance entity, the rooms that name the prop and its
    /// SH-probe slot. A placement without an instance has no PVS key and is declined.
    Prop {
        instance: Entity,
        groups: &'a Arc<[u16]>,
        slot: Option<u16>,
    },
}

/// The WMO half of [`GxBatch`].
pub struct GxWmoBatch {
    /// The placement's `WmoPortalInstance` entity: the region key and the PVS source.
    pub instance: Entity,
    /// Absolute group index in the building: the PVS bit this batch selects on.
    pub group: u16,
    /// The group is a true interior (`MOGI & 0x48 == 0`).
    pub interior: bool,
    /// The MOBA batch class (INT, TRANS, EXT), the lighting lane on interior groups.
    pub class: Option<WmoBatchClass>,
    /// The MOMT SIDN night-glow colour.
    pub sidn: Option<[u8; 3]>,
    /// The MOMT WINDOW midpoint-light flag.
    pub window: bool,
    /// The authored batch order (batch index + 1), for the coplanar-MOBA clip-z nudge.
    pub batch_order: u16,
}

impl StaticGx {
    /// Regions whose first bake has not published: diverted geometry that nothing draws yet, which
    /// the reveal gate ([`crate::terrain_stream::WorldLoadProgress`]) waits on. A published region
    /// re-baking still draws its previous bake and does not count.
    pub(crate) fn undrawn_regions(&self) -> usize {
        let world = &self.world;
        let cells = self
            .cells
            .iter()
            .filter(|(k, s)| s.dirty && !s.items.is_empty() && !world.cells.contains_key(*k))
            .count();
        let wmos = self
            .wmos
            .iter()
            .filter(|(k, s)| s.dirty && !s.items.is_empty() && !world.wmos.contains_key(*k))
            .count();
        let props = self
            .props
            .iter()
            .filter(|(k, s)| s.dirty && !s.items.is_empty() && !world.props.contains_key(*k))
            .count();
        cells + wmos + props
    }

    /// WMO regions collected, published (baked, so drawable) and selected by this frame's cull.
    pub fn wmo_census(&self) -> (usize, usize, usize) {
        (
            self.wmos.len(),
            self.world.wmos.len(),
            self.world.visible_wmos.len(),
        )
    }

    /// What this frame's walk selected: doodad-phase entries, WMO regions, and admitted WMO groups.
    pub fn draw_census(&self) -> (usize, usize, usize) {
        let groups = self
            .world
            .visible_wmos
            .iter()
            .map(|(_, bits)| bits.drawn.iter().filter(|b| **b).count())
            .sum();
        (
            self.world.visible.len(),
            self.world.visible_wmos.len(),
            groups,
        )
    }

    /// Bake every dirty region on the next flush, quiet or not: the reveal gate's end of a load.
    pub(crate) fn flush_now(&mut self) {
        self.flush_now = true;
    }

    /// Take one batch instead of spawning or merging it; `false` sends it down the ordinary path.
    pub fn divert(&mut self, b: GxBatch<'_>) -> bool {
        if b.env_map {
            self.declined[0] += 1;
            return false;
        }
        if b.no_depth_write || b.no_depth_test {
            self.declined[1] += 1;
            return false;
        }
        if b.wmo.is_some() && wmo_lane_disabled() {
            return false;
        }
        if b.prop.is_some() && prop_lane_disabled() {
            return false;
        }
        // WMO group batches light on the FFP N·L and skip this. ADT doodads and MODD props are one
        // class (`CMapDoodadDef`), `Matte` at 1.0: the 2.5 is the entity light node's, which
        // neither has. `Shaded` reads 0.5; `Lit` has no producer here and is a backstop.
        let (shade_lit, matte) = match (&b.wmo, &b.prop, b.shade) {
            (Some(_), _, _) => (false, false),
            (None, _, ShadeSel::Matte) => (false, true),
            (None, _, ShadeSel::Lit) => (true, false),
            (None, _, ShadeSel::Shaded) => (false, false),
            _ => {
                self.declined[2] += 1;
                return false;
            }
        };
        let wmo_key = b.wmo.as_ref().map(|w| w.instance);
        let wmo = b.wmo.map(|w| GxItemWmo {
            group: w.group,
            interior: w.interior,
            class_lane: match (w.interior, w.class) {
                (true, Some(WmoBatchClass::Int)) => 1,
                (true, Some(WmoBatchClass::Trans)) => 2,
                _ => 0,
            },
            sidn: w.sidn.unwrap_or([0; 3]),
            window: w.window,
            order: if wmo_bias_disabled() {
                0
            } else {
                w.batch_order
            },
        });
        let entry = match (wmo_key, &b.prop) {
            (Some(instance), _) => self.wmos.entry(instance).or_default(),
            (None, Some(p)) => self.props.entry(p.instance).or_default(),
            (None, None) => {
                let cell = (
                    (b.transform.translation.x / CELL).floor() as i32,
                    (b.transform.translation.z / CELL).floor() as i32,
                );
                self.cells.entry(cell).or_default()
            }
        };
        // Props named by the same rooms share one set index, so the cull tests each set once.
        let prop = b.prop.map(|p| {
            let set = match entry
                .sets
                .iter()
                .position(|s| s.as_ref() == p.groups.as_ref())
            {
                Some(i) => i,
                None => {
                    entry.sets.push(p.groups);
                    entry.sets.len() - 1
                }
            };
            GxItemProp {
                set: u16::try_from(set).expect("gx region under u16 referrer sets"),
                slot: p.slot,
            }
        });
        // A new fader is Steady until the scan classifies it the frame its cell bakes, before the
        // bitmap rebuild, so one streaming in past its band never draws retained.
        let fader_uid = b.fade.as_ref().map(|_| b.object.id);
        if let Some(seed) = b.fade {
            // Only faders carry a seed; a band-less radius reads as always steady.
            let (near, far) =
                crate::model_fade::fade_band(seed.radius).unwrap_or((f32::MAX, f32::MAX));
            let uid = b.object.id;
            let is_new = !entry.faders.contains_key(&uid);
            let fader = entry.faders.entry(uid).or_insert_with(|| GxFader {
                owner: b.owner,
                uid,
                object: b.object.clone(),
                transform: b.transform,
                radius: seed.radius,
                local_center: seed.local_center,
                center: b.transform.transform_point(seed.local_center),
                near,
                far,
                batches: Vec::new(),
                items: Vec::new(),
                state: FaderState::Steady,
            });
            fader.batches.push(GxFaderBatch {
                stat_mesh: seed.stat_mesh,
                aabb: seed.aabb,
                cutout: seed.cutout,
                blend: seed.blend,
                blend_mode: b.blend,
                geometry: b.geometry.clone(),
            });
            if is_new {
                // A placement's later batches share its sphere: the caches move only on a new one.
                let c = Vec2::new(fader.center.x, fader.center.z);
                entry.fader_bounds = Some(match entry.fader_bounds {
                    Some((mn, mx)) => (mn.min(c), mx.max(c)),
                    None => (c, c),
                });
                entry.ring = if entry.faders.len() == 1 {
                    (near, far)
                } else {
                    (entry.ring.0.min(near), entry.ring.1.max(far))
                };
                entry.settled = None;
            }
        }
        entry.items.push(GxItem {
            geometry: b.geometry.clone(),
            transform: b.transform,
            object: b.object.clone(),
            local_aabb: b.aabb,
            owner: b.owner,
            texture: b.texture.as_ref().map(Handle::id),
            _texture_handle: b.texture,
            cutout: b.blend == ModelBlend::AlphaTest && !crate::model_render::alphatest_disabled(),
            two_sided: b.two_sided,
            unlit: b.unlit,
            fog_off: matches!(b.fog_policy, benilla_formats::FogPolicy::Off),
            shade_lit,
            matte,
            wrap_x: b.geometry.wrap_x,
            wrap_y: b.geometry.wrap_y,
            wmo,
            prop,
            fader: fader_uid,
        });
        if !entry.dirty {
            entry.dirty_since = self.frame;
        }
        entry.dirty = true;
        entry.last_change = self.frame;
        self.accepted += 1;
        true
    }

    /// Count a prop the divert never sees: an exterior fader prop (the exile protocol has no prop
    /// shape), or a prop of a placement without an instance entity.
    pub fn tally_prop_declined(&mut self, no_instance: bool) {
        self.declined[if no_instance { 4 } else { 3 }] += 1;
    }

    /// Drop a dead owner tile's cell items and mark their cells for re-bake; a diverted batch has
    /// no entity, so nothing else releases it. Regions follow their instance entity instead: a
    /// straddler handoff keeps a placement alive under a new owner tile.
    pub fn release_owner(&mut self, owner: (i32, i32)) {
        let frame = self.frame;
        let Self {
            cells,
            pending_despawn,
            ..
        } = self;
        for cell in cells.values_mut() {
            let before = cell.items.len();
            cell.items.retain(|i| i.owner != owner);
            if cell.items.len() != before {
                if !cell.dirty {
                    cell.dirty_since = frame;
                }
                cell.dirty = true;
                cell.last_change = frame;
            }
            // A dead exile's entities are this lane's own: they queue for the scan to despawn.
            let faders_before = cell.faders.len();
            cell.faders.retain(|_, f| {
                if f.owner != owner {
                    return true;
                }
                if let FaderState::Exiled { ents, .. } = &mut f.state {
                    pending_despawn.append(ents);
                }
                false
            });
            if cell.faders.len() != faders_before {
                cell.fader_bounds = cell
                    .faders
                    .values()
                    .map(|f| Vec2::new(f.center.x, f.center.z))
                    .fold(None, |acc: Option<(Vec2, Vec2)>, c| {
                        Some(acc.map_or((c, c), |(mn, mx)| (mn.min(c), mx.max(c))))
                    });
                cell.ring = cell
                    .faders
                    .values()
                    .fold((f32::MAX, 0.0), |(n, x), f| (n.min(f.near), x.max(f.far)));
                cell.settled = None;
                cell.bits_stale = true;
            }
        }
    }

    /// Map drop: everything goes, and the exiles, known only to this lane, queue for despawn.
    pub fn clear(&mut self) {
        let Self {
            cells,
            pending_despawn,
            ..
        } = self;
        for cell in cells.values_mut() {
            for f in cell.faders.values_mut() {
                if let FaderState::Exiled { ents, .. } = &mut f.state {
                    pending_despawn.append(ents);
                }
            }
        }
        self.cells.clear();
        self.wmos.clear();
        self.props.clear();
        self.world.cells.clear();
        self.world.wmos.clear();
        self.world.props.clear();
        self.world.visible.clear();
        self.world.visible_wmos.clear();
        // The census counts this world only; carried across a map change it reads as a leak.
        self.declined = [0; 5];
        self.declined_logged = [0; 5];
        self.declined_seen = [0; 5];
        self.declined_changed = 0;
        self.declined_printed = 0;
        self.accepted = 0;
    }
}

pub struct StaticGxPlugin;

impl Plugin for StaticGxPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        info!("static-gx: ARMED (default since 1434; WOW_STATIC_GX=0 opts out) — the retained static-world pass (1429–1434)");
        app.init_resource::<StaticGx>().add_systems(
            PostUpdate,
            // Bake, scene walk, publish; after `CheckVisibility`, which is load-bearing. A spawned
            // exile flushed by an earlier sync point would be visible before bevy_pbr ticked its
            // specialization, a render panic (`bevy_pbr-0.18.1/material.rs:1061`); after it, the
            // exile shows the next frame, ticked. `chain_ignore_deferred` adds no sync points.
            (
                bake::flush_static_gx,
                cull::cull_cells,
                render::publish_gx_world,
            )
                .chain_ignore_deferred()
                .after(bevy::transform::TransformSystems::Propagate)
                .after(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
        render::build(app);
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    //! Shared test fixtures for the collector/bake test modules.
    use super::*;

    /// A test placement identity; `uid` keys the fader lane's exile unit.
    pub fn object(uid: u32) -> Arc<crate::interact::WorldObject> {
        Arc::new(crate::interact::WorldObject {
            kind: crate::model_render::ModelKind::Doodad,
            label: "World\\test\\fence.m2".into(),
            id: uid,
            detail: String::new(),
        })
    }

    /// The one identity [`batch`] borrows; distinct placements use [`batch_of`].
    fn shared_object() -> &'static Arc<crate::interact::WorldObject> {
        static O: std::sync::OnceLock<Arc<crate::interact::WorldObject>> =
            std::sync::OnceLock::new();
        O.get_or_init(|| object(1))
    }

    pub fn batch(
        geometry: &Arc<RenderSubmesh>,
        at: Vec3,
        texture: Option<Handle<bevy::image::Image>>,
        blend: ModelBlend,
    ) -> GxBatch<'_> {
        batch_of(shared_object(), geometry, at, texture, blend)
    }

    pub fn batch_of<'a>(
        object: &'a Arc<crate::interact::WorldObject>,
        geometry: &'a Arc<RenderSubmesh>,
        at: Vec3,
        texture: Option<Handle<bevy::image::Image>>,
        blend: ModelBlend,
    ) -> GxBatch<'a> {
        GxBatch {
            geometry,
            transform: Transform::from_translation(at),
            object,
            aabb: None,
            owner: (0, 0),
            texture,
            blend,
            two_sided: false,
            unlit: false,
            fog_policy: benilla_formats::FogPolicy::Scene,
            env_map: false,
            no_depth_write: false,
            no_depth_test: false,
            shade: ShadeSel::Lit,
            wmo: None,
            prop: None,
            fade: None,
        }
    }

    pub fn tri(at: [f32; 3]) -> Arc<RenderSubmesh> {
        Arc::new(RenderSubmesh {
            positions: vec![at, [at[0] + 1.0, at[1], at[2]], [at[0], at[1] + 1.0, at[2]]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0]; 3],
            indices: vec![0, 1, 2],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{batch, batch_of, object, tri};
    use super::*;

    #[test]
    fn a_collected_region_is_undrawn_until_it_publishes() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        assert_eq!(
            gx.undrawn_regions(),
            0,
            "nothing collected, nothing pending"
        );
        assert!(gx.divert(batch(&g, Vec3::ZERO, None, ModelBlend::Opaque)));
        assert_eq!(gx.undrawn_regions(), 1, "diverted, unbaked — a hole");
        // Publishing a region clears `dirty`.
        let cell = *gx.cells.keys().next().unwrap();
        gx.cells.get_mut(&cell).unwrap().dirty = false;
        assert_eq!(gx.undrawn_regions(), 0, "baked — on screen");
    }

    #[test]
    fn the_divert_declines_the_excluded_families() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.env_map = true;
        assert!(!gx.divert(b));
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.no_depth_write = true;
        assert!(!gx.divert(b));
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Rig;
        assert!(!gx.divert(b), "a glue-booth rig material never diverts");
        assert_eq!(gx.declined, [1, 1, 1, 0, 0]);
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        assert!(
            gx.divert(b),
            "an ADT map doodad is Matte and still belongs here"
        );
        assert!(gx.divert(batch(&g, Vec3::ZERO, None, ModelBlend::Opaque)));
        assert_eq!(gx.cells.len(), 1);
    }

    #[test]
    fn a_fader_divert_registers_its_placement_seed() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let mk_seed = || GxFadeSeed {
            radius: 0.4,
            local_center: Vec3::ZERO,
            stat_mesh: Handle::default(),
            aabb: None,
            cutout: Handle::default(),
            blend: Handle::default(),
        };
        // One placement, two batches, one seed; a third, never-fade batch stays bare.
        let fence = object(77);
        let mut a = batch_of(
            &fence,
            &g,
            Vec3::new(5.0, 0.0, 5.0),
            None,
            ModelBlend::Opaque,
        );
        a.fade = Some(mk_seed());
        assert!(gx.divert(a));
        let mut b = batch_of(
            &fence,
            &g,
            Vec3::new(5.0, 0.0, 5.0),
            None,
            ModelBlend::AlphaTest,
        );
        b.fade = Some(mk_seed());
        assert!(gx.divert(b));
        assert!(gx.divert(batch(
            &g,
            Vec3::new(6.0, 0.0, 6.0),
            None,
            ModelBlend::Opaque
        )));
        let cell = &gx.cells[&(0, 0)];
        assert_eq!(cell.faders.len(), 1, "one placement, one exile unit");
        let f = &cell.faders[&77];
        assert_eq!(f.batches.len(), 2);
        // Radius 0.4 takes the fade table's 40.4..50.4 band.
        assert!((f.near - 40.4).abs() < 1e-4 && (f.far - 50.4).abs() < 1e-4);
        assert!(matches!(f.state, FaderState::Steady));
        assert_eq!(cell.ring, (f.near, f.far));
        assert_eq!(
            cell.items.iter().filter(|i| i.fader == Some(77)).count(),
            2,
            "both fader items carry the uid; the never-fade item stays bare"
        );
    }

    /// Props land in their instance's region, not a cell, with slot and Matte bit on the item.
    #[test]
    fn a_prop_divert_dedups_referrer_sets() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let instance = Entity::PLACEHOLDER;
        let rooms_a: Arc<[u16]> = Arc::from([3u16, 5].as_slice());
        let rooms_b: Arc<[u16]> = Arc::from([9u16].as_slice());
        let mk = |groups: &Arc<[u16]>, slot| GxPropBatch {
            instance,
            groups: Arc::clone(groups),
            slot,
        };
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.prop = Some(mk(&rooms_a, Some(11)));
        assert!(gx.divert(b));
        let mut b = batch(&g, Vec3::ONE, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        b.prop = Some(mk(&rooms_a, Some(12)));
        assert!(gx.divert(b));
        let mut b = batch(&g, Vec3::ONE, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Shaded;
        b.prop = Some(mk(&rooms_b, None));
        assert!(gx.divert(b));
        assert!(gx.cells.is_empty() && gx.wmos.is_empty());
        let region = &gx.props[&instance];
        assert_eq!(region.sets.len(), 2, "same rooms share a set");
        assert_eq!(region.items.len(), 3);
        let sets: Vec<u16> = region
            .items
            .iter()
            .map(|i| i.prop.as_ref().unwrap().set)
            .collect();
        assert_eq!(sets, vec![0, 0, 1]);
        assert_eq!(region.items[0].prop.as_ref().unwrap().slot, Some(11));
        assert!(region.items[0].matte, "Matte is the prop lane's own bit");
        assert!(!region.items[2].matte, "Shaded stays the 0.5 family");
        gx.clear();
        assert!(gx.props.is_empty() && gx.world.props.is_empty());
    }

    #[test]
    fn a_dead_owner_queues_its_exiles() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let doomed = object(9);
        let mut a = batch_of(&doomed, &g, Vec3::ZERO, None, ModelBlend::Opaque);
        a.owner = (1, 1);
        a.fade = Some(GxFadeSeed {
            radius: 0.4,
            local_center: Vec3::ZERO,
            stat_mesh: Handle::default(),
            aabb: None,
            cutout: Handle::default(),
            blend: Handle::default(),
        });
        assert!(gx.divert(a));
        let ghost = Entity::PLACEHOLDER;
        gx.cells
            .get_mut(&(0, 0))
            .unwrap()
            .faders
            .get_mut(&9)
            .unwrap()
            .state = FaderState::Exiled {
            ents: vec![ghost],
            armed: true,
        };
        gx.release_owner((1, 1));
        let cell = &gx.cells[&(0, 0)];
        assert!(cell.faders.is_empty());
        assert_eq!(gx.pending_despawn, vec![ghost]);
    }

    #[test]
    fn a_dead_owner_leaves_the_cells() {
        let mut gx = StaticGx::default();
        let g = tri([0.0; 3]);
        let mut a = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        a.owner = (1, 1);
        assert!(gx.divert(a));
        let mut b = batch(&g, Vec3::ONE, None, ModelBlend::Opaque);
        b.owner = (2, 2);
        assert!(gx.divert(b));
        gx.frame += IDLE_FRAMES; // silence any later-change guard
        gx.release_owner((1, 1));
        let state = &gx.cells[&(0, 0)];
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.items[0].owner, (2, 2));
        assert!(state.dirty, "the survivor cell re-bakes");
        gx.clear();
        assert!(gx.cells.is_empty() && gx.world.cells.is_empty());
    }
}
