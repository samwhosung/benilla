//! **The retained static-world pass — ON by default** (decisions 1429–1434;
//! `WOW_STATIC_GX=0` opts out): the static world leaves bevy_pbr entirely and draws from
//! retained buffers in one custom render node, cross-material — per-vertex texture-array
//! layers + flag bits where the entity path had one material handle per batch. Grown in
//! four slices: B1 never-fade doodads + WMO group geometry, B2 faders via the exile
//! protocol, B3 the churn payments (shared pool, re-bake cadence, draw order, kill-aware
//! runs), B4 the WMO-prop absorption; flipped default-on at 1434 after the director's eye
//! passed at the pins.
//!
//! The correctness bar is pixel-identity against the entity path
//! (`WOW_STATIC_MERGE=0 WOW_STATIC_GX=0` vs the armed default) at the capture fixtures —
//! the on-demand A/B harness, not the director's eye (captures are a regression tool).
//!
//! What it deliberately does NOT build (1429 stages them): horizon occlusion, class-aware
//! index baking (items are sorted by `(bucket, texture)` main-world and coalesced into runs
//! render-side; dims and format are only knowable there — BLP images are
//! `RENDER_WORLD`-only), and any default flip.
//!
//! The visibility terms at this lane's granularity (1429's table): frustum + farclip + the
//! exterior window gate are CPU per-CELL ([`cull_cells`] — the real client's own shape, a CPU
//! scene walk feeding retained draws); fade is the EXILE PROTOCOL (B2, decision 1431 — see
//! below); portal PVS and the billboard-owner term never apply to ADT doodads; the dev
//! kind/blend toggles are honoured cell-wholesale via [`crate::dev_state::DebugState`]'s
//! doodad toggle (a prototype coarsening — the entity path toggles per blend class).
//!
//! **B2 (decision 1431): faders ride the retained pass via the exile protocol.** The pass
//! draws only fade = 1 content: a fader placement diverts with an exile SEED (uid + handle
//! clones of the production bundle), and [`cull::cull_cells`]'s per-frame scan classifies it
//! on `doodad_fade_alpha` — the same function the entity authority evaluates. Steady = drawn
//! retained, no entity rows; Exiled (0 < α < 1) = every item takes the record-table KILL BIT
//! (the vertex stage collapses it to a zero-area point) and the placement respawns as
//! ordinary entities, so the feather is the entity path's own look by construction; Gone
//! (α = 0) = killed, nothing spawned anywhere. Exits carry 1 yd hysteresis; entry none.
//! The frame discipline is the OVERLAP PROTOCOL (1421's pop class, prevented structurally):
//! an exile's entities flush at `PostUpdate`'s end and first draw NEXT frame, and the kill
//! bit waits for them (`FaderState::Exiled::armed`) — the retained item dies in the same
//! rendered frame the entity appears; a re-admit clears the bit and despawns in one frame
//! (the despawn lands before extract, the cleared bit rides the same publish). Never order
//! this chain's commands to flush mid-`PostUpdate` — the plugin's `chain_ignore_deferred`
//! note records the bevy_pbr panic that forbids it. `WOW_STATIC_GX_FADE=0` is the fader
//! lane-isolation lever; `WOW_GX_FADE_TRACE=1` logs transitions; the census carries
//! `faders=`/`fade_events=`.
//!
//! Exclusions the collector refuses (each falls through to the ordinary merge/entity path, and
//! the flush logs a one-line census — refusals beside the ACCEPTED count, once a streaming burst
//! settles — so a declined population is never silent, and never mistakable for a dead pass in
//! the other direction either): env-mapped
//! batches (`texture_unit_lookup > 2` — view-generated UVs), the depth-flag oddities
//! (`no_depth_write`/`no_depth_test` — pipeline-state per batch), and the `ShadeSel::Matte`/
//! `Rig` families (exterior WMO MODD props ride the prop site, not this lane).
//!
//! **Slice-1 verdict (the first capture A/B, 2026-08-18):** against a selfcheck floor of
//! exactly 0, the armed sweep reads MAE 0.000 on all four scenarios with 13–65 pixels per
//! 5.8-Mpx frame differing by exactly 1/255 (float-order residue of the baked-transform
//! arithmetic; zero pixels beyond 8) — pixel-identical to the quantization limit, ~1,500
//! retained cell bakes participating. Two desk-found parity terms live in render.rs/the WGSL:
//! the aniso-8 samplers, and the two-sided normal law (the entity shader's `select` UNDOES a
//! bevy negation this pipeline never had — copying it flipped every back-facing canopy card).
//!
//! **Known B1 gaps** (B2+ owns them): dev-instrument identity (inspector/WOW_PICK name nothing
//! — the weld/merge trade, one side table if missed) is **CLOSED by 1534**: the side table is the
//! items' own [`GxItem::object`]/[`GxItem::local_aabb`], and [`pick`] is the lane answering the ray
//! from this frame's published selection. The other one B1 left open — no
//! `presentable()` coupling, so an unbaked region shows a hole for a beat on a cold arrival
//! where the merge publishes `merge_pending` into the reveal cover (1419) — is **CLOSED by
//! 1498**: [`StaticGx::undrawn_regions`] is that coupling, and [`StaticGx::flush_now`] is why
//! it costs the load no extra frames. It was not a beat: it was the director's report of
//! teleporting into a Stormwind with no Stormwind in it.
//!
//! **B3 (decision 1432): the churn 1431 priced is paid.** Texture arrays are ONE SHARED POOL
//! across every cell and region (dedup by asset id; `pool.rs` owns the design note), layer
//! copies encode exactly once, re-bakes ride the long quiet window ([`REBAKE_FRAMES`] —
//! 1424's consolidate-on-quiet at bake grain), admitted cells and regions draw NEAR-FIRST
//! (the 1.12 band walk's effect at cell grain), and killed items are dropped from the draw
//! runs entirely rather than collapsed in the vertex stage (the WGSL collapse stays as belt).
//!
//! **Slice 2 (1429's second half): WMO group geometry.** An order-free WMO group batch diverts
//! into a region keyed by its placement's [`crate::wmo_portal::WmoPortalInstance`] ENTITY —
//! the PVS identity — with draws bucketed per (pipeline bucket × texture × GROUP), so the
//! portal flood's per-frame verdict becomes CPU range selection over retained buffers (the
//! design's load-bearing collapse: the client's own per-group draw records). Per-group
//! admission each frame = the PVS bit (fail-open, honouring the `portal_cull` A/B switch) ∧
//! frustum ∧ farclip ∧ the exterior window gate with the own-building exemption — the same
//! seven-term collapse as cells, at group granularity. Region lifecycle follows the INSTANCE
//! entity's death, never the owner tile: a straddler handoff keeps the placement (and its
//! instance) alive under a new owner, and tile-keyed release would have dropped the retained
//! geometry out from under it. The WMO lighting lanes (MOCV inside the clamp, INT/TRANS batch
//! classes, SIDN night glow, WINDOW midpoint light, the interior fog triple, ZERO point
//! lights, the authored batch-order clip-z nudge) mirror `wow_model.wgsl` in `static_gx.wgsl`.
//!
//! **B4 (decision 1433): the prop site joins the pass — A's blobs absorbed.** WMO doodad
//! props (1418's lane 3, the population A merged) divert into PROP REGIONS keyed by the same
//! instance entity as the WMO regions but held apart (props trickle in as their M2s load;
//! a shared region would re-bake the building's geometry per arrival — B3's churn class).
//! Admission is per REFERRER SET (the distinct room-sets naming this region's props): the
//! PVS bit ORs over the set exactly like `WmoGroupVis::drawn_by`, ∧ frustum ∧ farclip ∧ the
//! exterior gate with the own-building exemption — an empty set (an unnamed prop) admits
//! bare and is never exterior-gated (0784's untagged rule). Interior props light from their
//! folded SH probe: the slot rides the record table (w bits 1..14) and the shader reads the
//! probe region of the SAME shared light buffer every material binds; live point lights are
//! zero (folded at spawn), fog is the interior triple. Exterior props take the Matte
//! fixed-1.0 sun family ([`WORD_MATTE`]). Declined and censused, not absorbed: exterior
//! FADER props (the exile protocol has no prop shape — they keep the default path's own
//! per-entity fade) and props of instance-less placements (no PVS identity). With B4, the
//! armed lane leaves A's merge only the divert's declined families; the default flip is a
//! later record's question. `WOW_STATIC_GX_PROP=0` is the lane-isolation lever.
//!
//! **Slice-2 verdict (2026-08-18):** floors exactly 0 on all 11 scenarios; the armed A/B
//! reads MAE ≤ 0.003 with 99.9 % of differing pixels at exactly 1/255 and 2–35 px/frame
//! beyond 8 (isolated silhouette-edge pixels) — the arithmetic floor of a baked
//! re-expression, not a lane error; the tram (near-origin placements) matches at exactly 0.
//! LBRS agreement: entity-lane WMO drops 33 → 0 drawn while the retained pass selects 2
//! groups / 52 items, stable over the 300-frame window — the flood as range selection, no
//! leak. Three desk-found laws this slice: texture-array layers DEDUPE by texture (per-item
//! layers blew the 2,048 D2-array limit the moment Stormwind baked); the bake's position leg
//! runs in f64 (`bake.rs` owns the note — the f32 world intermediate was 0974's defect
//! reintroduced, half a pixel indoors); and a colour-less batch multiplies by CONSTANT 1.0,
//! never the interpolated white (GPU interpolation of a constant is 1.0±ε and re-rounded
//! ~half of all textured pixels one byte down). `WOW_STATIC_GX_WMO=0` is the lane-isolation
//! lever that attributed the residue; `WOW_GX_CENSUS=1` is the agreement instrument; the
//! `house-north-midnight` fixture is the one capture where SIDN is live pixels.

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

/// The doodad spatial cell — ¼ ADT tile, the same 133⅓-yd locality key the merge lanes use
/// (`terrain_stream::merge::CELL`; 1413 round 2 proved the locality load-bearing).
const CELL: f32 = 533.333_3 / 4.0;

/// Quiet frames before a dirty cell's FIRST bake (~¼ s at 60 Hz) — the merge's own close
/// rationale: fast appearance, consolidation once the admission burst has moved on.
const IDLE_FRAMES: u32 = 15;
/// Quiet frames before an already-published cell RE-bakes (~2 s) — 1424's spawn-fast/
/// consolidate-on-quiet shape at bake granularity (B3, decision 1432): the admission trickle
/// arrives in bursts spaced wider than [`IDLE_FRAMES`], so the short window re-baked cells
/// once per arrival for minutes (1431 measured the churn); the long window batches a whole
/// trickle span per re-bake. The cost is arrival latency for content joining an ALREADY-baked
/// cell (distance-trickle admissions, under fog at the cell's range) — the entity path's own
/// arrival class, slower by construction.
const REBAKE_FRAMES: u32 = 120;
/// A dirty cell this old re-bakes even if never quiet (~10 s) — the trickle must not be able
/// to starve a re-bake forever.
const MAX_DIRTY_FRAMES: u32 = 600;

/// Per-vertex packed word: texture-array layer in bits 0..16, flag bits above —
/// see `static_gx.wgsl` (kept in sync by the flush's packing below).
pub const ATTRIBUTE_GX_WORD: MeshVertexAttribute =
    MeshVertexAttribute::new("Gx_Word", 988_101, VertexFormat::Uint32);
/// Per-vertex point-light anchor: the PLACEMENT origin (world space) — the entity path anchors
/// its ≤3-nearest selection at the instance origin (`wow_model.wgsl`), and baking it per vertex
/// keeps exact parity where the merged-blob path coarsened to the blob origin.
pub const ATTRIBUTE_GX_ANCHOR: MeshVertexAttribute =
    MeshVertexAttribute::new("Gx_Anchor", 988_102, VertexFormat::Float32x3);

const WORD_WRAP_X: u32 = 1 << 16;
const WORD_WRAP_Y: u32 = 1 << 17;
const WORD_UNLIT: u32 = 1 << 18;
const WORD_FOG_OFF: u32 = 1 << 19;
const WORD_SHADE_LIT: u32 = 1 << 20;
const WORD_TEXTURED: u32 = 1 << 21;
// The WMO lane (slice 2) — mirrors of the entity path's per-material facts:
// model_flags.x (the WMO surface laws: MOCV inside the clamp, zero point lights)…
const WORD_WMO: u32 = 1 << 22;
// …model_flags.z (interior group: the INT/TRANS/EXT batch-class lanes + the interior fog
// triple)…
const WORD_INTERIOR: u32 = 1 << 23;
// …tint.w == 1 / == 2 (the batch's MOBA class lane; EXT is both bits clear)…
const WORD_CLASS_INT: u32 = 1 << 24;
const WORD_CLASS_TRANS: u32 = 1 << 25;
// …sidn.w (the MOMT WINDOW midpoint light)…
const WORD_WINDOW: u32 = 1 << 26;
// …and "this batch AUTHORED vertex colours": the entity shader's VERTEX_COLORS shader-def.
// The gx layout always carries COLOR (white default), and white is bit-identical through
// every lane EXCEPT the INT ×(1+4·MOCV.a) self-illumination override — the entity path's
// colourless INT batch takes the no-COLORS combine (plain tex × lit), so the override must
// key on authored colours, not on the lane.
const WORD_HAS_VC: u32 = 1 << 27;
// The prop lane (B4, decision 1433) — an exterior WMO MODD prop's Matte sun family: intensity
// FIXED 1.0 (`ShadeSel::Matte`, the mid-band selector — the ADT 2.5 site (`0x69e4ad`) is one a
// MODD prop never reaches). A distinct bit rather than an alias of SHADE_LIT: under the recorded
// `min(I,1)` cap the two read identically today, but the cap is the lane's one unfaithful
// term (0803 §3) and lifting it must not silently split this lane's parity.
const WORD_MATTE: u32 = 1 << 28;
// An INTERIOR M2 prop (B4) is WORD_INTERIOR with WORD_WMO clear — exactly the entity shader's
// `interior_prop = flags.z && !flags.x`: the per-item SH-probe lane (record-table slot),
// interior fog, zero live point lights (the group-MOLR lobes are folded into the probe).

/// Armed? **ON by default** (decision 1434: the B chapter's population story is complete —
/// B1–B4 — with pixel parity at the quantization floor, legs negative at all four pins, and
/// the director's eye passed at the pins). `WOW_STATIC_GX=0` is the opt-out lever — the A/B
/// arm for every comparison this chapter still owes; `=1` still reads as an explicit on for
/// anything that predates the flip. Read once; the plugin registers nothing when off.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_STATIC_GX").as_deref() != Ok("0"))
}

/// `WOW_WMO_BIAS=0` — B38's A/B diagnostic, honoured exactly where `model_material` honours
/// it: the authored batch-order nudge bakes as zero.
fn wmo_bias_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_WMO_BIAS").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_WMO=0` — the slice-2 lane-isolation lever: the WMO half of the divert
/// refuses wholesale (batches fall to the entity path), cells unaffected. Built for the
/// parity sweep's attribution question — "is this dust the WMO film or a doodad regression?"
/// — and kept: any future A/B on one lane needs exactly this.
fn wmo_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_WMO").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_FADE=0` — the B2 lane-isolation lever (the WMO lever's twin, decision
/// 1431): fader batches fall back to the entity path wholesale; never-fade cells and WMO
/// regions unaffected. The attribution instrument for any fader-lane A/B.
pub(crate) fn fade_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_FADE").as_deref(), Ok("0")))
}

/// `WOW_STATIC_GX_PROP=0` — the B4 lane-isolation lever (the family's third): WMO-prop
/// batches refuse wholesale and fall back to the merge/entity path, cells and WMO regions
/// unaffected. The attribution instrument for any prop-lane A/B.
fn prop_lane_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| matches!(std::env::var("WOW_STATIC_GX_PROP").as_deref(), Ok("0")))
}

/// `WOW_GX_PERF=1` — the lane's own cost meter (1431's regression hunt): each armed system
/// accumulates its wall time into [`GX_PERF`], and the cull prints + zeroes the set at
/// ~1 Hz — `GX_PERF flush/cull/publish/prepare/node ms-per-frame`. Main-world and
/// render-world sites share the table (atomics; the two threads never contend on cadence).
pub(crate) fn gx_perf_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_GX_PERF").is_some())
}

/// Lifetime bytes of texture-array VRAM created by `assemble_region` (never decremented —
/// a high-water ledger of what the per-cell arrays cost; the cache drop path frees the
/// textures but the meter's question is how much this design ALLOCATES).
pub(crate) static GX_VRAM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Nanosecond accumulators: flush, cull, publish, prepare, node.
pub(crate) static GX_PERF: [std::sync::atomic::AtomicU64; 5] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// Scope guard timing one armed site into [`GX_PERF`] slot `i` — one line at the top of
/// each measured fn; a no-op branch when the meter is off.
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

/// One diverted batch, held until its cell bakes (and retained after, so an owner-tile unload
/// or a later admission can re-bake the cell without the entity path's help).
struct GxItem {
    geometry: Arc<RenderSubmesh>,
    transform: Transform,
    /// This item's placement identity — what the inspector/`WOW_PICK` name when the cursor lands
    /// on it (decision 1534). Shared per placement, so a 40-batch building costs one allocation.
    object: Arc<crate::interact::WorldObject>,
    /// The batch's model-local bound (the render form's build-time `Aabb`), kept for the pick's
    /// broad phase — the drawn side recentres and unions its bounds per cell, which cannot
    /// answer "which batch is under the cursor". `None` ⇒ narrow-tested unconditionally.
    local_aabb: Option<Aabb>,
    /// The owner TILE — read only by [`StaticGx::release_owner`], i.e. only for cell items.
    /// A WMO region's lifecycle keys on its instance entity instead (see `cull::cull_cells`).
    owner: (i32, i32),
    texture: Option<AssetId<bevy::image::Image>>,
    /// Kept as a live handle so the render world's `GpuImage` can never be dropped from under
    /// the baked cell (the id alone holds nothing).
    _texture_handle: Option<Handle<bevy::image::Image>>,
    cutout: bool,
    two_sided: bool,
    unlit: bool,
    fog_off: bool,
    shade_lit: bool,
    wrap_x: bool,
    wrap_y: bool,
    /// The exterior-prop Matte sun family (B4): intensity fixed 1.0. False everywhere else.
    matte: bool,
    /// `Some` on a WMO group-geometry item (slice 2) — the per-batch WMO facts the shader's
    /// WMO lanes read.
    wmo: Option<GxItemWmo>,
    /// `Some` on a WMO-prop item (B4, decision 1433) — the referrer-set index + probe slot.
    prop: Option<GxItemProp>,
    /// `Some(uid)` on a FADER item (B2, decision 1431): the placement this item exiles with.
    /// The flush maps post-sort item indices back to the cell's [`GxFader`] through it.
    fader: Option<u32>,
}

/// A WMO-prop item's baked facts (B4): which of its region's referrer SETS admits it (the
/// index into [`GxCell::sets`] — the PVS admission ORs over the set's rooms, the entity
/// path's `WmoGroupVis::drawn_by` at range-selection grain), and the interior prop's folded
/// SH-probe slot (`None` = exterior light: an exterior MODD prop, or a probe-table overflow —
/// the entity path's own fallback).
struct GxItemProp {
    set: u16,
    slot: Option<u16>,
}

/// One fader placement's exile seed + live state (B2, decision 1431): everything needed to
/// respawn the placement's diverted batches as ordinary entities the frame the camera enters
/// its feather band, and to punch its retained items out (the per-item kill bit) while any
/// state but Steady holds. Lives in its cell, keyed by placement uniqueId; released with the
/// owner tile exactly like the items.
struct GxFader {
    owner: (i32, i32),
    uid: u32,
    /// The placement's identity — carried onto the exiled entities' `WorldObject` so a feathering
    /// doodad names the same thing whichever lane is drawing it that frame.
    object: Arc<crate::interact::WorldObject>,
    transform: Transform,
    radius: f32,
    /// Model-local fade-sphere centre; `transform` maps it to [`Self::center`].
    local_center: Vec3,
    /// World-space fade-sphere centre — the scan measures horizontal distance to THIS, the
    /// same point `apply_model_visibility` measures to (`FUN_006952a0`'s transformed centre).
    center: Vec3,
    /// Band edges from [`crate::model_fade::fade_band`] — the cell ring test's terms. The
    /// state machine itself classifies on `doodad_fade_alpha` directly (one source).
    near: f32,
    far: f32,
    /// One seed per diverted batch, in divert order.
    batches: Vec<GxFaderBatch>,
    /// This placement's item indices in the CURRENT bake (post-sort; refreshed by every
    /// flush). Empty until the cell first bakes after the placement diverts.
    items: Vec<u16>,
    state: FaderState,
}

/// One diverted batch's respawn payload — handle clones of exactly what the production
/// assembler would have spawned (`assemble.rs`'s ordinary-part bundle), so the feather-band
/// look is the entity path's own by construction.
struct GxFaderBatch {
    stat_mesh: Handle<Mesh>,
    aabb: Option<bevy::camera::primitives::Aabb>,
    cutout: Handle<benilla_assets::materials::WowModelMaterial>,
    blend: Handle<benilla_assets::materials::WowModelMaterial>,
    blend_mode: ModelBlend,
    geometry: Arc<RenderSubmesh>,
}

/// The exile state machine (B2). Steady = retained draws it (fade 1, no entity); Exiled =
/// the placement lives as ordinary entities (the feather is the entity path's own look),
/// kill bits set once `armed`; Gone = kill bits set, nothing drawn (fully faded — the
/// entity path culls at alpha 0, this lane never spawns the rows at all).
///
/// `armed` is the overlap protocol's one-frame delay: a fresh exile's entities flush at the
/// end of `PostUpdate` (see the plugin's `chain_ignore_deferred` note) and first draw NEXT
/// frame — the scan arms the kill bit on that same next frame, so the retained item dies in
/// the rendered frame the entity appears. Killing on the spawn frame would open a one-frame
/// hole: 1421's pop class.
enum FaderState {
    Steady,
    Exiled { ents: Vec<Entity>, armed: bool },
    Gone,
}

/// A WMO item's baked facts (see [`GxWmoBatch`] for provenance).
struct GxItemWmo {
    group: u16,
    interior: bool,
    /// The batch-class lane exactly as `model_render` packs `tint.w`: 0 = EXT law, 1 = INT,
    /// 2 = TRANS — non-zero only on an interior group's batches.
    class_lane: u8,
    sidn: [u8; 3],
    window: bool,
    /// The authored batch order for the clip-z nudge (`sun_scale.y`'s mirror); zeroed when
    /// `WOW_WMO_BIAS=0` exactly like `model_material`.
    order: u16,
}

/// A cell's collected items + bake state.
#[derive(Default)]
struct GxCell {
    items: Vec<GxItem>,
    dirty: bool,
    last_change: u32,
    /// The frame `dirty` last went false→true — [`MAX_DIRTY_FRAMES`]'s clock.
    dirty_since: u32,
    /// The cell's fader placements (B2), keyed by placement uniqueId. Empty on WMO regions.
    faders: bevy::platform::collections::HashMap<u32, GxFader>,
    /// The published kill bitmap no longer matches the fader states (a re-bake reassigned
    /// item indices, or a state transitioned) — the scan rebuilds it whole and bumps the
    /// draw's revision.
    bits_stale: bool,
    /// XZ bounds over the faders' sphere centres (the scan's cheap ring test); rebuilt on
    /// insert/release.
    fader_bounds: Option<(Vec2, Vec2)>,
    /// `(min near, max far)` over the cell's faders — the ring test's band union. Only read
    /// while `faders` is non-empty.
    ring: (f32, f32),
    /// The scan's last wholesale verdict for this cell (`None` = mixed/unknown → walk every
    /// frame). A repeated all-steady / all-gone verdict skips the per-placement walk.
    settled: Option<bool>,
    /// The DISTINCT referrer sets of this region's prop items (B4; prop regions only —
    /// empty on cells and WMO regions). An item's [`GxItemProp::set`] indexes here; content
    /// dedup means props named by the same rooms share one selection bit and one run key.
    sets: Vec<Arc<[u16]>>,
}

/// The main-world collector + the published draw set. One resource carries both halves so the
/// divert (spawn), the flush, the unload hook and the cull all see one consistent state.
#[derive(Resource, Default)]
pub struct StaticGx {
    cells: bevy::platform::collections::HashMap<(i32, i32), GxCell>,
    /// The WMO regions (slice 2), keyed by placement instance entity — the same identity the
    /// portal PVS is computed on, so range selection needs no translation table.
    wmos: bevy::platform::collections::HashMap<Entity, GxCell>,
    /// The PROP regions (B4, decision 1433), keyed by the SAME instance entity but held
    /// apart from [`Self::wmos`] deliberately: props trickle in over seconds as their M2s
    /// load, and sharing the building's region would convert every arrival into a re-bake
    /// of the whole building's geometry — the exact churn class B3 paid down.
    props: bevy::platform::collections::HashMap<Entity, GxCell>,
    /// The published, extracted half — what the render node draws.
    pub(crate) world: render::GxWorld,
    frame: u32,
    /// Declined-batch census (env-map / depth-flag / shade-family / prop-fader /
    /// prop-without-instance refusals), logged once a streaming burst settles so a
    /// silently-thinner population can't masquerade as covered (1429's no-silent-caps note).
    declined: [u32; 5],
    declined_logged: [u32; 5],
    /// The census's last-seen counts, the frame they last MOVED, and the frame it last PRINTED —
    /// the settle timer that turns a login's per-frame line storm into one line per burst. Before
    /// it, the census fired on ANY count change from a `PostUpdate` system that runs every frame,
    /// so streaming in a zone emitted a near-identical INFO line per frame (ten of them in one
    /// Goldshire login). The two frame stamps are distinct on purpose: the settle window measures
    /// from the last change, the ceiling from the last line, or a count that moves every frame
    /// resets its own deadline forever and never reports.
    declined_seen: [u32; 5],
    declined_changed: u32,
    declined_printed: u32,
    /// Batches the divert ACCEPTED, against which the refusals above are read. Without it the
    /// census line cannot distinguish "1,630 declined out of ~25,000" from "the pass is dead" —
    /// which is exactly the reading it forced when a log was triaged on 2026-09-06.
    accepted: u32,
    /// Exiled entities whose seed died out from under them (owner release, map clear) —
    /// drained and despawned by the scan, which owns the exile lifecycle end to end.
    pending_despawn: Vec<Entity>,
    /// Lifetime exile-event counters (spawns, re-admits, gone-despawns) — the census line's
    /// churn instrument.
    fade_events: [u32; 3],
    /// [`StaticGx::flush_now`]'s request: the next flush ignores every quiet window. Consumed
    /// (cleared) by that flush.
    flush_now: bool,
}

/// The divert hook's per-call facts (everything is in scope at `assemble.rs`'s gate).
pub struct GxBatch<'a> {
    pub geometry: &'a Arc<RenderSubmesh>,
    pub transform: Transform,
    /// The placement's shared identity — the pick's answer for geometry no entity owns (1534).
    pub object: &'a Arc<crate::interact::WorldObject>,
    /// The batch's model-local build-time bound (the render form's `Aabb`) — the pick's broad
    /// phase.
    pub aabb: Option<Aabb>,
    /// The owner tile (cell items' release key; unread for WMO items — their region follows
    /// the instance entity's lifetime).
    pub owner: (i32, i32),
    pub texture: Option<Handle<bevy::image::Image>>,
    pub blend: ModelBlend,
    pub two_sided: bool,
    pub unlit: bool,
    pub fog_policy: benilla_formats::FogPolicy,
    pub env_map: bool,
    pub no_depth_write: bool,
    pub no_depth_test: bool,
    /// Unread on the WMO lane (the entity path passes `ShadeSel::Matte` for every WMO batch
    /// and its material never reads the selector) — the shade refusal applies to cells only.
    pub shade: ShadeSel,
    /// `Some` = a WMO group-geometry batch (slice 2).
    pub wmo: Option<GxWmoBatch>,
    /// `Some` = a WMO-prop batch (B4, decision 1433).
    pub prop: Option<GxPropBatch>,
    /// `Some` = a FADER batch (B2, decision 1431): the exile seed. `None` on never-fade
    /// batches, the WMO lane, and the prop lane (an exterior fader prop never diverts —
    /// see the assemble gate).
    pub fade: Option<GxFadeSeed>,
}

/// The prop half of [`GxBatch`] (B4): the building's instance entity (the region key, the
/// PVS source, and the lifecycle — the instance dies with the placement, `refs` and all,
/// so a prop region needs no owner-tile bookkeeping), the referrer set of rooms naming the
/// prop (empty = unnamed: always admitted, never exterior-gated — 0784's untagged rule),
/// and the interior prop's SH-probe slot.
pub struct GxPropBatch {
    pub instance: Entity,
    pub groups: Arc<[u16]>,
    pub slot: Option<u16>,
}

/// The fader half of [`GxBatch`] (B2): the placement identity + the respawn payload the
/// exile protocol spawns the feather-band entity from. Everything is a handle clone of what
/// the production assembler holds at the divert point, so the exiled entity IS the entity
/// path's bundle.
pub struct GxFadeSeed {
    /// Bounding-sphere radius (yd, placement-scaled) — selects the fade band.
    pub radius: f32,
    /// Model-local bbox centre; the fade distance is measured to its world image.
    pub local_center: Vec3,
    pub stat_mesh: Handle<Mesh>,
    pub aabb: Option<bevy::camera::primitives::Aabb>,
    pub cutout: Handle<benilla_assets::materials::WowModelMaterial>,
    pub blend: Handle<benilla_assets::materials::WowModelMaterial>,
}

/// Where the divert hook is standing (`spawn/mod.rs` builds it, `assemble.rs` threads it):
/// the site decides which population an eligible batch joins and which lifecycle key it
/// takes — cells release by owner tile, WMO regions by instance death.
pub enum GxSite<'a> {
    /// A world-static ADT-doodad placement — cell items, released by owner tile. The exile unit
    /// (B2) is the placement, identified by the shared [`GxBatch::object`] every site now carries.
    Doodad { owner: (i32, i32) },
    /// A WMO placement's group geometry: the pre-spawned `WmoPortalInstance` entity + the
    /// model's per-batch group map (`WmoModel::submesh_group`, index-parallel with batches).
    Wmo { instance: Entity, groups: &'a [u16] },
    /// A WMO doodad prop (B4, decision 1433 — 1418's lane 3, absorbed): the building's
    /// instance entity, the referrer set of rooms that name the prop, and the interior
    /// prop's folded SH-probe slot. Only a placement WITH an instance qualifies (no
    /// instance ⇒ no PVS identity ⇒ the merge/entity path, tallied).
    Prop {
        instance: Entity,
        groups: &'a Arc<[u16]>,
        slot: Option<u16>,
    },
}

/// The WMO half of [`GxBatch`], gathered at the spawn site (`spawn/mod.rs` owns the instance
/// entity; `assemble.rs` zips the per-batch group index out of `submesh_group`).
pub struct GxWmoBatch {
    /// The placement's `WmoPortalInstance` entity — the region key AND the PVS source.
    pub instance: Entity,
    /// Absolute group index within the building (the PVS bit this batch's draws select on).
    pub group: u16,
    /// This batch's group is a true interior (`MOGI & 0x48 == 0`) — `RenderSubmesh::interior`.
    pub interior: bool,
    /// The MOBA batch class (INT/TRANS/EXT) — the lighting-lane selector on interior groups.
    pub class: Option<WmoBatchClass>,
    /// The MOMT SIDN night-glow colour.
    pub sidn: Option<[u8; 3]>,
    /// The MOMT WINDOW midpoint-light flag.
    pub window: bool,
    /// The authored batch order (batch index + 1) — the coplanar-MOBA clip-z nudge.
    pub batch_order: u16,
}

impl StaticGx {
    /// **Collected geometry that draws nothing yet** — regions holding items whose bake has not
    /// published (a first bake still inside its quiet window, [`IDLE_FRAMES`]).
    ///
    /// A retained region is a *hole in the frame* between the divert and its first publish: the
    /// entity path no longer draws that geometry (slice 2 moved WMO group geometry here
    /// wholesale), so a spawned building whose region has not baked is a building that is not on
    /// screen. The reveal gate ([`crate::terrain_stream::WorldLoadProgress`]) reads this, which
    /// is why it is published as a fact rather than left an internal of the bake.
    ///
    /// A *re-bake* of an already-published region is deliberately NOT counted: that region keeps
    /// drawing its previous bake meanwhile, so it is content on screen, not a hole.
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

    /// The WMO lane's headline numbers for an instrument: regions **collected**, regions
    /// **published** (baked, so drawable at all), and regions the cull **selected** this frame.
    /// Collected-but-unpublished is the reveal hole [`Self::undrawn_regions`] counts; published
    /// minus selected is ordinary culling.
    pub fn wmo_census(&self) -> (usize, usize, usize) {
        (
            self.wmos.len(),
            self.world.wmos.len(),
            self.world.visible_wmos.len(),
        )
    }

    /// What this frame's scene walk actually **selected to draw** — doodad/prop draw entries,
    /// WMO regions selected, and the admitted group bits summed across them. The companion to
    /// [`Self::wmo_census`]: that one says what geometry EXISTS and is drawable, this one says
    /// how much of it the cull let through. A frame where everything is published and almost
    /// nothing is selected is a visibility fault, not a residency one — the distinction the
    /// reveal audit could not make.
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

    /// **Bake every dirty region on the next flush, quiet window or not** — what the reveal gate
    /// asks for once nothing more is arriving.
    ///
    /// The quiet windows ([`IDLE_FRAMES`]/[`REBAKE_FRAMES`]) exist to batch an *arrival trickle*
    /// during play. The end of a load is not a trickle: the streamer knows the burst is over
    /// (every wanted tile spawned, every focus placement up, the collider queue quiet), and
    /// waiting another 15 frames there is 15 frames of a city that has spawned and cannot draw.
    /// So the end of the burst says "publish what you have" instead of arming another timer.
    pub(crate) fn flush_now(&mut self) {
        self.flush_now = true;
    }

    /// Take one eligible batch instead of spawning it (or merging it). Returns `false` — and
    /// tallies why — when the batch's facts fall outside the prototype's lane; the caller then
    /// falls through to the ordinary path, exactly like a refused merge divert.
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
            return false; // the lane-isolation lever: WMO batches back to the entity path
        }
        if b.prop.is_some() && prop_lane_disabled() {
            return false; // the B4 lever: prop batches back to the merge/entity path
        }
        // The shade family gates CELLS only: the WMO lane never reads the selector (the
        // entity path passes `Matte` for every WMO batch and lights on the FFP N·L).
        //
        // **`Matte` is now BOTH doodad classes' family, not just the prop lane's** (2050): an
        // ADT map doodad is fixed-1.0 exactly like an exterior MODD prop, because they are one
        // C++ class (`CMapDoodadDef`) and the 2.5 belongs to the WENTITY node, which neither
        // has. So the arm no longer requires `prop.is_some()`. `Shaded` still lands on neither
        // bit and still reads 0.5, because the shader's `shade_t` defaults to 1.0 without
        // `WORD_SHADE_LIT`.
        //
        // That leaves `ShadeSel::Lit` with no producer in this lane at all — its three
        // populations are ADT doodads, WMO group geometry and WMO props, and entities (the only
        // holders of a light node) never divert here. The arm is kept as a backstop rather than
        // deleted: retiring `WORD_SHADE_LIT` and its shader branch is a word-layout change and
        // wants its own round, named in 2050 rather than folded in here.
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
        // A prop item resolves its referrer set to the region's dedup list (B4): same rooms
        // ⇒ same selection bit ⇒ same run key, and the cull tests each distinct set once.
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
        // A fader batch (B2) registers its exile seed on the cell's placement entry. Fresh
        // placements start Steady; the scan classifies them against the live camera the same
        // frame their cell bakes (states run before the bitmap rebuild), so a placement that
        // streams in beyond its band never draws retained for even one frame.
        let fader_uid = b.fade.as_ref().map(|_| b.object.id);
        if let Some(seed) = b.fade {
            // A never-fade radius cannot reach here (the assemble gate builds a seed only
            // for `!class.never_fade`), but a MAX band is the honest fallback: it reads
            // "always steady", which IS never-fade semantics.
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
                // A later batch of the same placement shares its sphere and band — the
                // ring/bounds caches move only when a placement joins.
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

    /// Tally a prop-site refusal the divert never sees (B4): an exterior FADER prop (the
    /// exile protocol has no prop shape — it keeps today's per-entity fade, the default
    /// path's own look), or a whole prop on a placement WITHOUT an instance entity (no PVS
    /// identity to key a region on). Censused so the declined population is never silent.
    pub fn tally_prop_declined(&mut self, no_instance: bool) {
        self.declined[if no_instance { 4 } else { 3 }] += 1;
    }

    /// Drop a dead owner tile's items and mark their cells for re-bake — the unload hook
    /// (`terrain_stream`'s release loop calls this beside the straddler handoff; a diverted
    /// batch has no entity, so nothing else could ever release it).
    ///
    /// **Cells only, deliberately.** A WMO region's items follow the placement's INSTANCE
    /// entity (reaped in `cull::cull_cells` when it despawns): a straddler handoff keeps the
    /// placement alive under a NEW owner tile, and dropping its retained geometry on the old
    /// tile's death would blank a living building.
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
            // The fader seeds ride the same owner key. An exiled placement's entities are
            // this lane's own spawns (nothing else knows them), so its death queues them;
            // the scan drains the queue with `Commands` next pass.
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

    /// Map drop: everything goes (the streamer's `drop_streamed_world` — same staleness law as
    /// the weld/merge accumulators beside this). Exiled entities queue for despawn — the map
    /// transition despawns the placement lane's entities through the placement release, but
    /// the exiles are this lane's own.
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
        // The census counts THIS world's population. `static_merge::reset` exists for exactly
        // this reason — a cumulative counter carried across a map change reports the sum of two
        // worlds and reads as a leak.
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
            // Chained: bake, then this frame's scene walk (after the camera settles), then
            // publish the extractable snapshot.
            //
            // `.after(CheckVisibility)` is LOAD-BEARING (B2, 1431): the scan SPAWNS exile
            // entities via `Commands`, and a `PostUpdate` spawner that runs before the
            // visibility check can have its queued commands flushed by ANY other system
            // pair's auto sync point sitting before `CheckVisibility` (this app already has
            // two: `exterior_cull` and `billboard` both order `.before(CheckVisibility)`
            // with deferred params). A spawn surfacing there is VISIBLE this frame while
            // bevy_pbr's per-entity specialization tick — recorded by
            // `check_entities_needing_specialization`, which may already have run — arrives
            // only next frame, and `specialize_material_meshes` UNWRAPS the tick of every
            // visible entity: the render app panics (`bevy_pbr-0.18.1/material.rs:1061`,
            // reproduced and pinned with a vendored diagnostic — the gap entities were
            // exactly this scan's exiles, one frame each). Running after `CheckVisibility`
            // makes the hazard structural nonsense: however early some sync point flushes
            // our spawns, they cannot be seen by a visibility check that already ran — the
            // entity is invisible its first frame, ticked and visible together the next,
            // which is precisely the overlap protocol's arm delay (see `cull.rs`).
            // `chain_ignore_deferred` keeps this chain from minting sync points of its own;
            // `CheckVisibility` is itself after `UpdateFrusta`, so the frustum read is this
            // frame's camera (B1 left that ambiguous — a latent fixed by the same pin).
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

    /// A test placement identity — `uid` is the id the fader lane keys its exile unit on.
    pub fn object(uid: u32) -> Arc<crate::interact::WorldObject> {
        Arc::new(crate::interact::WorldObject {
            kind: crate::model_render::ModelKind::Doodad,
            label: "World\\test\\fence.m2".into(),
            id: uid,
            detail: String::new(),
        })
    }

    /// The one identity [`batch`] borrows — a test that needs distinct placements uses
    /// [`batch_of`] with its own [`object`].
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

    /// A diverted batch is collected geometry that draws NOTHING until its region bakes — the
    /// reveal hole [`StaticGx::undrawn_regions`] exists to publish, and the fact the reveal gate
    /// keys on (decision 1498). Publishing the region clears it; a re-bake of a published region
    /// is not a hole, because the previous bake is still on screen.
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
        // What the bake does when it publishes a region: clears `dirty`.
        let cell = *gx.cells.keys().next().unwrap();
        gx.cells.get_mut(&cell).unwrap().dirty = false;
        assert_eq!(gx.undrawn_regions(), 0, "baked — on screen");
    }

    /// The collector refuses exactly the recorded exclusion families — and says so in the
    /// census — while an eligible batch diverts (1429's no-silent-caps note).
    ///
    /// **The shade-family exclusion is `Rig` now, not `Matte`** (2050). A cell arriving `Matte`
    /// used to be refused, but only because nothing could produce one: ADT doodads were on
    /// `Lit`, props carried `prop: Some`, group geometry `wmo: Some`. Since ADT doodads are the
    /// fixed-1.0 family — the same C++ class as an exterior MODD prop — a `Matte` cell is the
    /// ordinary case and MUST divert, which is what the added assertion below pins. What is left
    /// genuinely excluded is the authored-rig booth material, which has no business in the
    /// world's retained lane at all.
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
        // The ADT doodad's own family: a cell on fixed-1.0, no prop record.
        let mut b = batch(&g, Vec3::ZERO, None, ModelBlend::Opaque);
        b.shade = ShadeSel::Matte;
        assert!(
            gx.divert(b),
            "an ADT map doodad is Matte and still belongs here"
        );
        assert!(gx.divert(batch(&g, Vec3::ZERO, None, ModelBlend::Opaque)));
        assert_eq!(gx.cells.len(), 1);
    }

    /// A fader batch (B2) registers its exile seed on the cell — placement-keyed, band from
    /// the fade law, items tagged with the uid — while a never-fade batch stays bare. The
    /// same placement's second batch joins the SAME seed (one exile unit).
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
        // The exile unit is keyed by the PLACEMENT identity every lane shares (1534) — one
        // object, two batches, one seed.
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
        // Band from the fade law's own table: radius 0.4 → 40.4..50.4.
        assert!((f.near - 40.4).abs() < 1e-4 && (f.far - 50.4).abs() < 1e-4);
        assert!(matches!(f.state, FaderState::Steady));
        assert_eq!(cell.ring, (f.near, f.far));
        assert_eq!(
            cell.items.iter().filter(|i| i.fader == Some(77)).count(),
            2,
            "both fader items carry the uid; the never-fade item stays bare"
        );
    }

    /// A prop batch (B4) diverts into a region keyed by its INSTANCE entity; props naming
    /// the same rooms share one referrer-set index and a distinct set opens a new one; the
    /// interior slot and the Matte family ride the item (Matte admits on this lane — still
    /// refused on cells).
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

    /// A dead owner drops its fader seeds with its items, and an exiled placement's entities
    /// land on the despawn queue — nothing else knows those entities exist.
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

    /// A dead owner's items leave their cells and the re-bake drops to the survivors — the
    /// unload hook's contract (a diverted batch has no entity; nothing else can release it).
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
