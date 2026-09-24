//! Water foam — the client's **`CWater0Ripple`** wade wake / standing ring / step-in splash,
//! rebuilt as the reference's actual **record model** (decision 0264; supersedes the ribbon of 0240
//! and the stamps of 0234).
//!
//! Ground truth, twice over: byte-exact reverse engineering **and** two live GL traces
//! of the reference reconstructed frame-by-frame (Northshire wade 2026-07-08 + standing-ring
//! capture 2026-07-10; per-record texgen fits residual ≈ 0). The verified model:
//!
//! - **A decal is a pool record, not a mesh idiom.** Per wading unit the client emits records into
//!   a 128-slot pool (32 reserved for the active mover, 96 for everyone else — eviction only).
//!   Each record: feet-anchored center (Z = the **liquid surface** height), heading, `size0`,
//!   growth rate, lifetime, peak alpha, birth time.
//! - **Geometry is built ONCE, at emission**, from the wet liquid-lattice cells overlapping the
//!   record's *final* box (`size0 + growth·lifetime`) — which clips foam at banks. **Growth is
//!   pure texgen**: each frame the texture maps the box `[center ± size(t)]` to UV `[0,1]`,
//!   rotated to the heading; outside the box the stencil clamps to its transparent border (both
//!   foam textures have alpha-0 edges; sampler CLAMP — measured; the 0240 "GL_REPEAT tiling" was
//!   a misread). The shape *stretches* over a static patch as `size(t)` grows.
//! - **Alpha** rises to the peak over `0.4·L`, decays to 0 over `0.6·L`; the record dies at `L`.
//!   Vertex colour is flat white × that alpha; the **stencil's own near-black RGB is the
//!   intensity** (the reference fragment is a full MODULATE — measured in its FFP GLSL:
//!   `tex × diffuse` on RGB *and* A). Recolouring the stencil to white — 0234/0240's move — made
//!   the foam ~20× too bright; that was the blown-out white wedge.
//! - **Selection** (driver `0x5fa760`): translating (`MOVEMENTFLAGS & 0xf` — the avatar's real
//!   flags; a velocity proxy for streamed units) → WAKE, V-apex **along** the movement vector;
//!   turning in place (`& 0x30`) → full-size RING; standing → reduced RING (size ×0.6, growth
//!   ×0.25, alpha ×0.8); plus a one-shot full RING on the wade-depth crossing (`0x6030c0`).
//!   Emission gates on `depth < max(2·collisionHeight, 1.0)` — ≈4.06 yd for a human (decision
//!   0489: surface SWIMMING sits well inside the gate at its ~1.52-yd rest depth, so a floating
//!   or stroking swimmer keeps its ring/wake, matching the reference; only a real dive past
//!   ~2 body heights silences it) — with a linear attenuation past half. One shared per-unit
//!   cooldown cell paces both
//!   kinds: rings every 400–450 ms, wakes one per ~0.625 yd of travel (the byte cadence laws —
//!   the 0264 INTERIM constants are resolved, decision 0265).
//!
//! - **The patch sits ON the plane; its coplanar ties are settled in DEPTH, never by a lift**
//!   (B348, 1807/1808). A decal's verts are the plane's own, so it ties with the water surface
//!   everywhere and with the *terrain* along the shoreline, where the two cross. Lifting the patch
//!   in world space wins that tie and also moves the geometry — and on a beach a vertical lift is a
//!   horizontal overshoot: a decal `l` yards up keeps painting for `l / slope` yards past the
//!   waterline, on dry sand (0.10 yd at the 29 % Stranglethorn beach the defect was reported on,
//!   proportionally more on every flatter shore — measured with
//!   `benilla-formats --example water_here`). **The reference agrees**: `0x68fd0f` arms a
//!   depth-buffer bias and the foam vertices sit at the queried MCLQ height exactly, so there is no
//!   geometric lift in the binary to be faithful to (the "−2048 GL units" this file cited for a
//!   year is refuted; the real term is `footstepBias(0.125) × [0x810390]` = D3D `DEPTHBIAS −1/8192`).
//!   Ours needs no settle at all, and that is worth stating rather than assuming (1811): the patch
//!   is not a decal *over* the surface, it **is** the liquid mesh's own triangles — the same wet
//!   cells, the same winding, through the same `clip_from_world` (`DECAL_WORLD_CLIP`, 0781) as a
//!   mesh whose `Transform` is `IDENTITY` — so the depths agree exactly and `GreaterEqual` passes
//!   the tie unaided. [`Rung::FOAM_RASTER`] stays as a few ULPs of insurance against a driver
//!   rounding two pipelines differently; the **slope half is disarmed**, because its pull grows as
//!   z² and the only thing it can reach is the skirt of wet-cell geometry lying over dry sand.
//!
//! The formulas + lifecycle math live in [`params`]; this half is the ECS: the emitter over the
//! avatar + streamed units, the record pool, and one additive effect-stream draw per
//! (chunk, category) with live records (fog OFF — the reference's verified foam render state).

mod params;

use bevy::asset::RenderAssetUsages;
use bevy::ecs::entity::EntityHashMap;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};

use crate::liquid::{FoamPatch, WaterChunkInfo, WaterIndex};
use crate::particles::buffer::{
    begin_effect_frame, EffectBlend, EffectDrawSpec, EffectFog, EffectQuads, EffectVertex,
};
use crate::schedule::WorldStage;
use crate::view::WorldCamera;
use crate::world_unit::{ViewerUnit, WorldUnit};
use benilla_assets::{AssetSet, WorldAssets};

use params::{foam_params, foam_uv, rand01, record_alpha, record_size, WadeState};
use params::{wake_cooldown, RING_INTERVAL};

/// The two foam stencils (render categories: 0 = ring, 1 = wake). Loaded RAW — the alpha carries
/// the shape, the near-black RGB carries the intensity (never recolour; see module doc).
const RING_TEXTURE: &str = "xtextures/splash/splash.blp";
const WAKE_TEXTURE: &str = "xtextures/splash/wake.blp";

/// Pool geometry (VERIFIED `0x68f8b0`/`0x68f9f0`): 128 records; the active mover allocates from
/// `[0, 32)`, everyone else from `[32, 128)` — an anti-eviction partition, nothing more.
const POOL_SIZE: usize = 128;
const SELF_SLOTS: usize = 32;

/// Step-in/out one-shot depth threshold, as a fraction of the unit's collision height (VERIFIED
/// `0x6030c0`: feet-Z vs `0.4 × collision height` from CMovement+0xb4, crossing latched either
/// direction). A human's 2.031 yd fires it ~0.81 yd deep, a gnome's 1.15 at ~0.46 — the unit's own
/// `h` since decision 0645, where it used to be a human's for everybody.
const ONESHOT_DEPTH_FRAC: f32 = 0.4;

/// The emission depth gate: **2 × the unit's collision height** (≈4.06 yd for a human, ≈2.3 for a
/// gnome), floored at 1.0 like the reference's `max(…, 1.0)`. The gate field
/// `[unit+0x297]` is the dword-indexed `+0xa5c` = CMovement+0xb4 = **collision height**, not
/// `UNIT_FIELD_BOUNDINGRADIUS`: reading it as the latter (`2 × UNIT_FIELD_BOUNDINGRADIUS` ≈ 0.78,
/// clamped to a 1-yd gate) is what killed all foam the moment swim latched: the ~1.52-yd swim
/// rest depth sat past the misread gate (decision 0489 —
/// the director's ref-check shows surface swimmers foaming, which the true 4-yd gate allows).
const GATE_DEPTH_FRAC: f32 = 2.0;

/// Horizontal speed (yd/s) above which a streamed unit counts as translating — the velocity proxy
/// for the reference's `MOVEMENTFLAGS & 0xf` bit-test (the avatar uses its real flags). A small
/// guard over extrapolation jitter.
const MOVE_EPSILON: f32 = 0.5;
/// Yaw rate (rad/s) above which a non-translating streamed unit counts as turning in place
/// (the `& 0x30` proxy).
const TURN_EPSILON: f32 = 0.35;

/// One live foam decal — a `CWater0Ripple` pool record. Geometry is static (built at emission);
/// per-frame size/alpha are derived from `born` + the params (`params.rs`).
struct FoamRecord {
    /// Feet position at emission, WoW XY.
    center: [f32; 2],
    /// Texgen heading (WoW radians): the movement direction (wake) or random (ring).
    heading: f32,
    size0: f32,
    /// yd/s.
    growth: f32,
    /// s; death at `born + lifetime`.
    lifetime: f32,
    /// Peak vertex alpha.
    peak: f32,
    born: f32,
    /// Render category: ring (`splash.blp`) vs wake (`wake.blp`).
    ring: bool,
    /// Static patch triangles (Bevy space, surface-lifted), from the wet liquid cells overlapping
    /// the final box. Empty patches never allocate a record.
    verts: Vec<Vec3>,
    /// The liquid chunk that hosted the emission — foam meshes group per chunk so the transparent
    /// pass sorts them with their water (then `depth_bias` wins the coplanar tie).
    chunk: Entity,
}

/// Per-unit emitter state: the shared cooldown cell (`unit+0xc78` — ONE cell for both kinds: a
/// ring pulse delays a following wake and vice versa, faithful), the step-in latch, and motion
/// history for the streamed-unit velocity/yaw proxies.
struct UnitFoam {
    last_pos: Option<Vec3>,
    last_yaw: Option<f32>,
    /// Absolute time the next emission is allowed (the shared tick cooldown).
    ready: f32,
    /// Step-in latch: currently deeper than the one-shot threshold.
    wading: bool,
    rng: u32,
    /// Fed this frame (drives retiring state for despawned units).
    active: bool,
}

impl UnitFoam {
    fn new(seed: u32) -> Self {
        UnitFoam {
            last_pos: None,
            last_yaw: None,
            ready: 0.0,
            wading: false,
            rng: seed | 1,
            active: false,
        }
    }
}

/// All foam state: the 128-slot record pool with its two allocation cursors, and the per-unit
/// emitter states (the avatar keys [`Entity::PLACEHOLDER`] — it has no streamed entity of its own).
#[derive(Resource)]
struct WaterFoam {
    pool: Vec<Option<FoamRecord>>,
    self_cursor: usize,
    other_cursor: usize,
    units: EntityHashMap<UnitFoam>,
}

impl Default for WaterFoam {
    fn default() -> Self {
        WaterFoam {
            pool: (0..POOL_SIZE).map(|_| None).collect(),
            self_cursor: 0,
            other_cursor: 0,
            units: EntityHashMap::default(),
        }
    }
}

/// Advance a partition cursor, returning the absolute pool slot to (over)write — the reference's
/// oldest-in-partition eviction (VERIFIED: counters `0xc7f3b0` self / `0xc81d48` others).
fn alloc_slot(cursor: &mut usize, base: usize, len: usize) -> usize {
    let i = base + *cursor;
    *cursor = (*cursor + 1) % len;
    i
}

/// The two foam stencils (ring/wake), decoded raw at startup. The draws they feed carry the
/// reference's **whole** foam render state (`0x68fae0`, folded back as 1808): additive; **fog
/// off** (`0x68fcd0`/`0x68fcd2`/`0x68fcd7` — *not*
/// `0x68fcc1`, which is the blend value's `mov edx,3` and was this file's citation until then);
/// **depth-tested `LEQUAL`, depth-write off**; and a depth-buffer bias armed at `0x68fd0f`.
/// Sorted on [`crate::sky_order::FOAM_BIAS`], a rung over the whole water band, because
/// `0x68fae0` is `0x6816d0`'s tail-jump on both arms of the submersion branch — one call per
/// frame, after **all** liquid has drained, with no sort key in it anywhere.
#[derive(Resource)]
struct FoamAssets {
    ring: Handle<Image>,
    wake: Handle<Image>,
}

/// The foam draw's sort rung — [`crate::sky_order::FOAM_BIAS`], whose doc carries the arithmetic.
/// It is a **rung above the whole water band**, not an epsilon over one surface: the reference
/// drains every liquid queue before it draws foam, so no water surface in the frame can come after
/// it. (It rode `WATER_BIAS + 1.0` until B348, where the water chunk one lattice nearer the eye
/// outsorted the foam patch by ~33 and painted over it.)
use crate::sky_order::FOAM_BIAS;

/// Load the stencils raw (CLAMP, no mips — the reference's measured sampler state).
fn setup_water_fx(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.init_resource::<WaterFoam>();
    let Some(mut world_assets) = world_assets else {
        return;
    };
    let Some(ring) = foam_image(&mut world_assets, RING_TEXTURE, &mut images) else {
        return;
    };
    let Some(wake) = foam_image(&mut world_assets, WAKE_TEXTURE, &mut images) else {
        return;
    };
    commands.insert_resource(FoamAssets { ring, wake });
}

/// Decode a foam stencil, keeping its authored RGBA verbatim (raw gamma bytes, the house
/// invariant — the near-black RGB is the intensity). `None` (with a warning) if missing.
fn foam_image(
    world_assets: &mut WorldAssets,
    path: &str,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let Some((w, h, rgba)) = world_assets.decode_rgba(path) else {
        warn!("water foam: {path} unavailable — no water foam");
        return None;
    };
    let mut image = Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    Some(images.add(image))
}

/// Build a record's static patch: every wet liquid cell whose XY bounds overlap the final box, cut
/// into the same two triangles the liquid surface draws, surface-lifted, in Bevy space; plus the
/// hosting chunk (the one containing the center). `None` when the center has no hosting water or no
/// wet cell overlaps (off the edge).
fn build_patch(
    center: [f32; 2],
    final_size: f32,
    chunks: &[(Entity, &WaterChunkInfo, &FoamPatch)],
) -> Option<(Vec<Vec3>, Entity)> {
    let (lo_x, hi_x) = (center[0] - final_size, center[0] + final_size);
    let (lo_y, hi_y) = (center[1] - final_size, center[1] + final_size);
    let mut verts = Vec::new();
    let mut host: Option<Entity> = None;
    for (entity, info, _foam) in chunks {
        if !info.overlaps(lo_x, hi_x, lo_y, hi_y) {
            continue;
        }
        if host.is_none() && info.contains(center[0], center[1]) {
            host = Some(*entity);
        }
        info.for_each_wet_cell(|[tl, tr, bl, br]| {
            let x = [tl[0], tr[0], bl[0], br[0]];
            let y = [tl[1], tr[1], bl[1], br[1]];
            let (cx0, cx1) = (
                x.iter().copied().fold(f32::MAX, f32::min),
                x.iter().copied().fold(f32::MIN, f32::max),
            );
            let (cy0, cy1) = (
                y.iter().copied().fold(f32::MAX, f32::min),
                y.iter().copied().fold(f32::MIN, f32::max),
            );
            if cx1 < lo_x || cx0 > hi_x || cy1 < lo_y || cy0 > hi_y {
                return;
            }
            // The liquid mesh's own winding: [tl, bl, br] then [tl, br, tr].
            for v in [tl, bl, br, tl, br, tr] {
                verts.push(wow_to_bevy(v));
            }
        });
    }
    let host = host?;
    if verts.is_empty() {
        None
    } else {
        Some((verts, host))
    }
}

/// Classify + gate + emit for one unit this frame (the driver `0x5fa760`, once per unit per
/// frame; emission paced by the unit's shared cooldown cell).
fn drive_unit(
    foam_state: &mut UnitFoam,
    alloc: &mut dyn FnMut(FoamRecord),
    pos: Vec3,
    state: WadeState,
    scale: f32,
    // The unit's collision height (yd) — both depth lines below are fractions of it (0645).
    h: f32,
    water: &Query<(Entity, &WaterChunkInfo, &FoamPatch)>,
    index: &WaterIndex,
    now: f32,
) {
    let gate = (GATE_DEPTH_FRAC * h).max(1.0);
    foam_state.active = true;
    let wow = bevy_to_wow(pos);
    // The surface height under the unit, from the wet cell it stands on — not the chunk's box and
    // not the chunk's highest vertex, which on a sloped river put the wade depth ~2 yd out
    // (decision 0642). Through [`WaterIndex`], not the full chunk walk: this line used to be a
    // linear scan of every loaded surface (~2.2k at a city pin) PER UNIT PER FRAME — exactly the
    // per-consumer shape `liquid/spatial.rs`'s module doc warns detonates; a dry-land unit now
    // costs one hash miss. A dead index entry self-filters at `water.get` (the index contract),
    // and overlapping surfaces were already first-match-in-arbitrary-order before this.
    let Some(surface) = index.over(wow[0], wow[1]).iter().find_map(|&e| {
        let (_, info, _) = water.get(e).ok()?;
        info.surface_z_at(wow[0], wow[1])
    }) else {
        foam_state.wading = false;
        return;
    };
    let depth = surface - wow[2];

    // Step-in/out one-shot: fires once on crossing the wade depth in EITHER direction
    // (`0x6030c0`, latched at `[+0x269]` — VERIFIED; the driver's mode-0xC9 call also clears the
    // shared cooldown).
    let wading_now = depth > ONESHOT_DEPTH_FRAC * h;
    let oneshot = wading_now != foam_state.wading;
    foam_state.wading = wading_now;

    if !oneshot && now < foam_state.ready {
        return;
    }
    let Some(p) = foam_params(state, oneshot, scale, gate, depth, &mut foam_state.rng) else {
        return;
    };
    let heading = match (p.ring, state) {
        (false, WadeState::Translating { heading, .. }) => heading,
        _ => rand01(&mut foam_state.rng) * std::f32::consts::TAU,
    };
    let final_size = p.size0 + p.growth * p.lifetime;
    let center = [wow[0], wow[1]];
    // Candidates for the patch box only — resolved here, on the EMISSION path (past the depth
    // gate and the cooldown), never on the every-frame classify above. `over_box` unions the
    // 1–4 cells a final-size box touches, so the set `build_patch` filters with `overlaps` is
    // the same one the full walk offered it.
    let candidates: Vec<_> = index
        .over_box(
            [center[0] - final_size, center[1] - final_size],
            [center[0] + final_size, center[1] + final_size],
        )
        .into_iter()
        .filter_map(|e| water.get(e).ok())
        .collect();
    if let Some((verts, chunk)) = build_patch(center, final_size, &candidates) {
        alloc(FoamRecord {
            center,
            heading,
            size0: p.size0,
            growth: p.growth,
            lifetime: p.lifetime,
            peak: p.peak,
            born: now,
            ring: p.ring,
            verts,
            chunk,
        });
    }
    // The shared cooldown cell: the one-shot resets it; a ring pulses on the 400+U[0,50) ms law;
    // a wake re-arms on the distance law (one decal per ~0.625 yd of travel).
    let mut uni = |a: f32, b: f32| a + (b - a) * rand01(&mut foam_state.rng);
    foam_state.ready = if oneshot {
        now
    } else if p.ring {
        now + uni(RING_INTERVAL.0, RING_INTERVAL.1)
    } else {
        let speed = match state {
            WadeState::Translating { speed, .. } => speed,
            _ => 0.0,
        };
        now + wake_cooldown(speed, &mut foam_state.rng)
    };
}

/// Per frame: run the driver for the avatar (its real movement flags — the reference's own
/// selection bits) and for every streamed unit (the velocity/yaw proxy), emitting pool records.
fn emit_water_foam(
    time: Res<Time>,
    materials: Option<Res<FoamAssets>>,
    mut foam: ResMut<WaterFoam>,
    viewer: Res<crate::view::Viewer>,
    self_unit: Query<&WorldUnit, With<ViewerUnit>>,
    units: Query<(Entity, &Transform, &WorldUnit), Without<ViewerUnit>>,
    water: Query<(Entity, &WaterChunkInfo, &FoamPatch)>,
    index: Res<WaterIndex>,
) {
    if materials.is_none() {
        return;
    }
    let now = time.elapsed_secs();
    let dt = time.delta_secs().max(1.0e-4);

    for uf in foam.units.values_mut() {
        uf.active = false;
    }

    if !water.is_empty() {
        // The avatar: byte-faithful selection off our own streamed movement flags.
        if let Some(body) = viewer.at {
            let uf = foam
                .units
                .entry(Entity::PLACEHOLDER)
                .or_insert_with(|| UnitFoam::new(0x5EED_F0A5));
            let prev = uf.last_pos.replace(body);
            let vel = prev.map_or(Vec3::ZERO, |p| (body - p) / dt);
            let w = bevy_to_wow(vel);
            let speed = (w[0] * w[0] + w[1] * w[1]).sqrt();
            // Byte-faithful selection off our own streamed movement flags (the CWater0Ripple
            // driver `0x5fa760`); the masks live on `Viewer`, next to the field they read.
            let state = if viewer.translating() {
                WadeState::Translating {
                    speed,
                    heading: w[1].atan2(w[0]),
                }
            } else if viewer.turning() {
                WadeState::Turning
            } else {
                WadeState::Standing
            };
            let scale = self_unit.single().map(|u| u.scale).unwrap_or(1.0);
            let h = viewer.height;
            let WaterFoam {
                pool,
                self_cursor,
                units,
                ..
            } = &mut *foam;
            let uf = units.get_mut(&Entity::PLACEHOLDER).expect("inserted above");
            let mut alloc = |rec: FoamRecord| {
                pool[alloc_slot(self_cursor, 0, SELF_SLOTS)] = Some(rec);
            };
            drive_unit(uf, &mut alloc, body, state, scale, h, &water, &index, now);
        }

        // Streamed units (the avatar's own wire ghost is excluded via `Without<SelfPlayer>`).
        for (entity, transform, unit) in &units {
            if !unit.wades {
                continue;
            }
            let pos = transform.translation;
            let seed = (entity.to_bits() as u32) ^ 0xA11C_E5ED;
            let uf = foam
                .units
                .entry(entity)
                .or_insert_with(|| UnitFoam::new(seed));
            let prev_pos = uf.last_pos.replace(pos);
            let yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
            let prev_yaw = uf.last_yaw.replace(yaw);
            let vel = prev_pos.map_or(Vec3::ZERO, |p| (pos - p) / dt);
            let w = bevy_to_wow(vel);
            let speed = (w[0] * w[0] + w[1] * w[1]).sqrt();
            let yaw_rate = prev_yaw.map_or(0.0, |p| {
                let mut d = yaw - p;
                while d > std::f32::consts::PI {
                    d -= std::f32::consts::TAU;
                }
                while d < -std::f32::consts::PI {
                    d += std::f32::consts::TAU;
                }
                (d / dt).abs()
            });
            let state = if speed > MOVE_EPSILON {
                WadeState::Translating {
                    speed,
                    heading: w[1].atan2(w[0]),
                }
            } else if yaw_rate > TURN_EPSILON {
                WadeState::Turning
            } else {
                WadeState::Standing
            };
            let h = unit.height;
            let WaterFoam {
                pool,
                other_cursor,
                units,
                ..
            } = &mut *foam;
            let uf = units.get_mut(&entity).expect("inserted above");
            let mut alloc = |rec: FoamRecord| {
                pool[alloc_slot(other_cursor, SELF_SLOTS, POOL_SIZE - SELF_SLOTS)] = Some(rec);
            };
            drive_unit(
                uf, &mut alloc, pos, state, unit.scale, h, &water, &index, now,
            );
        }
    }

    // Retire emitter state not fed this frame (unit despawned); its records age out on their own.
    foam.units.retain(|_, uf| uf.active);
}

/// Per frame (PostUpdate, after the stream clear): age the pool, drop dead records, and push
/// one effect-stream draw per (chunk, category) with live records — positions static, UV = the
/// texgen at `size(t)`, colour = white × the alpha ramp (the stencil's near-black RGB carries
/// the intensity through the texture product). Sorted at the group's vert centroid + the water
/// tie-break bias; a chunk that streamed out lets its records age out silently.
fn push_water_foam(
    time: Res<Time>,
    assets: Option<Res<FoamAssets>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut foam: ResMut<WaterFoam>,
    mut quads: ResMut<EffectQuads>,
    chunks_alive: Query<(), With<WaterChunkInfo>>,
) {
    let Some(assets) = assets else { return };
    let Ok(cam) = cam.single() else { return };
    let now = time.elapsed_secs();

    for slot in &mut foam.pool {
        if slot.as_ref().is_some_and(|r| now - r.born >= r.lifetime) {
            *slot = None;
        }
    }

    // Group record indices per (chunk, category) — each group is one contiguous draw.
    let mut groups: HashMap<(Entity, bool), Vec<usize>> = HashMap::default();
    for (i, rec) in foam.pool.iter().enumerate() {
        let Some(rec) = rec else { continue };
        if chunks_alive.get(rec.chunk).is_err() {
            continue; // chunk streamed out; the record ages out silently
        }
        groups.entry((rec.chunk, rec.ring)).or_default().push(i);
    }
    for ((_chunk, ring), records) in groups {
        let start = quads.begin();
        let mut centroid = Vec3::ZERO;
        let mut n = 0u32;
        for i in records {
            let rec = foam.pool[i].as_ref().expect("grouped above");
            let size = record_size(rec.size0, rec.growth, rec.born, now);
            let alpha = record_alpha(rec.peak, rec.lifetime, rec.born, now);
            for v in &rec.verts {
                let wow = bevy_to_wow(*v);
                quads.verts.push(EffectVertex {
                    pos: v.to_array(),
                    uv: foam_uv(rec.center, rec.heading, size, [wow[0], wow[1]]),
                    color: [1.0, 1.0, 1.0, alpha],
                });
                centroid += *v;
                n += 1;
            }
        }
        if n == 0 {
            continue;
        }
        quads.commit_tris(
            start,
            EffectDrawSpec {
                cam,
                texture: if ring {
                    assets.ring.id()
                } else {
                    assets.wake.id()
                },
                blend: EffectBlend::Add,
                // The reference's foam render sets FOG off (VERIFIED `0x68fcd0`/`0x68fcd2`/
                // `0x68fcd7`; the long-standing `0x68fcc1` citation was the blend setup — 1808).
                fog: EffectFog::Off,
                // The reference's foam render is its own additive path (`0x68fae0`), not the
                // M2 batch state producer — no GL_LIGHTING on it.
                lighting: crate::particles::buffer::EffectLighting::None,
                anchor: centroid / n as f32,
                bias: FOAM_BIAS,
                // A few ULPs of constant and **no slope term** (`Rung::FOAM_RASTER*`, whose docs
                // carry the bytes and the reasoning, 1811): the patch is the liquid mesh's own
                // triangles, so `GreaterEqual` already passes the coplanar tie and the only thing
                // a pull toward the eye can reach is the wet-lattice skirt over dry sand. The
                // nonzero constant also selects `DECAL_WORLD_CLIP`, so these absolute verts skip
                // the cam-relative rebase and go through the world meshes' own matrix (0781) —
                // which is what makes the two depths agree in the first place.
                raster_bias: crate::sky_order::Rung::FOAM_RASTER,
                raster_slope: crate::sky_order::Rung::FOAM_RASTER_SLOPE,
                cam_relative: false,
                // NEVER the chunk entity: a draw's probe identity must not own a registered
                // mesh, or bevy's sorted-phase batcher claims the item and rewrites its
                // `batch_range` (gpu_preprocessing.rs keys purely on `item.main_entity()`) —
                // the Goldshire-teleport crash. The chunk still keys the record grouping above.
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
                light: None,
                clip: None,
            },
        );
    }
}

/// Ordering handle for the foam emitter — the `waterfx` capture fixture
/// (`crate::capture::waterfx`) drives its synthetic wading dummy **before** this runs, so the
/// emitter reads the motion through its normal velocity/yaw proxies on the same frame. The
/// fixture registers itself against this set rather than being chained in here: an instrument
/// that the engine has to name is not an instrument, it is a dependency.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaterFoamSet;

/// The water foam plugin.
pub(crate) struct WaterFxPlugin;

impl Plugin for WaterFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_water_fx.after(AssetSet::Open))
            .add_systems(
                Update,
                emit_water_foam
                    .in_set(WaterFoamSet)
                    .in_set(WorldStage::Present),
            )
            // The stream push: PostUpdate after the frame's clear (emission ran in Update).
            .add_systems(PostUpdate, push_water_foam.after(begin_effect_frame));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitter reaches its water through [`WaterIndex`], end to end: a wading unit standing
    /// in an indexed pool emits a record on its first frame (the step-in one-shot), and a unit
    /// on dry land emits nothing. This is the regression fence for the index rewire — an empty
    /// or stale index is a world with NO foam, which no helper-level test would catch.
    #[test]
    fn the_emitter_finds_its_water_through_the_index() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<WaterIndex>()
            .init_resource::<WaterFoam>()
            .init_resource::<crate::view::Viewer>()
            .insert_resource(FoamAssets {
                ring: Handle::default(),
                wake: Handle::default(),
            })
            .add_systems(
                Update,
                (crate::liquid::maintain_water_index, emit_water_foam).chain(),
            );

        // A 3×3-vertex all-wet grid over [0,10]², surface z = 5.
        let mut positions = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                positions.push([i as f32 * 5.0, j as f32 * 5.0, 5.0]);
            }
        }
        let info = WaterChunkInfo::new(
            crate::liquid::LiquidSource::AdtChunk,
            benilla_formats::LiquidKind::Still,
            [3, 3],
            positions,
            vec![true; 4],
        );
        app.world_mut().spawn((info, FoamPatch));

        let unit = || WorldUnit {
            wades: true,
            scale: 1.0,
            height: 2.0,
            bound: None,
        };
        // Feet 1.5 yd under the surface — past the 0.4·h step-in line, so the one-shot ring
        // fires on the first classified frame.
        app.world_mut().spawn((
            Transform::from_translation(wow_to_bevy([2.0, 2.0, 3.5])),
            unit(),
        ));
        // The dry control, far outside every indexed cell.
        app.world_mut().spawn((
            Transform::from_translation(wow_to_bevy([500.0, 500.0, 3.5])),
            unit(),
        ));

        app.update();
        let foam = app.world().resource::<WaterFoam>();
        let live: Vec<usize> = foam
            .pool
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|_| i))
            .collect();
        assert_eq!(live.len(), 1, "one record: the wader's, not the dry unit's");
        assert!(
            live[0] >= SELF_SLOTS,
            "a streamed unit allocates from the other partition"
        );
    }

    /// Pool partitioning: the self cursor wraps within [0, 32), others within [32, 128) —
    /// eviction replaces the oldest of the same partition, never crosses it.
    #[test]
    fn pool_partitions_and_evicts() {
        let mut foam = WaterFoam::default();
        for _ in 0..SELF_SLOTS + 3 {
            let i = alloc_slot(&mut foam.self_cursor, 0, SELF_SLOTS);
            assert!(i < SELF_SLOTS);
        }
        assert_eq!(foam.self_cursor, 3);
        for _ in 0..(POOL_SIZE - SELF_SLOTS) + 5 {
            let i = alloc_slot(&mut foam.other_cursor, SELF_SLOTS, POOL_SIZE - SELF_SLOTS);
            assert!((SELF_SLOTS..POOL_SIZE).contains(&i));
        }
        assert_eq!(foam.other_cursor, 5);
    }

    /// The patch builder: wet cells overlapping the final box make it in (surface-lifted), ones
    /// outside stay out, and a dry position (no hosting chunk) yields no record.
    #[test]
    fn patch_clips_to_wet_cells() {
        // A 3×3-vertex grid over `[0,10]²` — 4 cells of 5 yd. Only the near (0,0) cell is wet, so
        // the far half of the box is dry ground the patch must not cover. The grid rides the info
        // now (the swim query needs the same cells); `FoamPatch` is just the marker.
        let mut positions = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                positions.push([i as f32 * 5.0, j as f32 * 5.0, 5.0]);
            }
        }
        let info = WaterChunkInfo::new(
            crate::liquid::LiquidSource::AdtChunk,
            benilla_formats::LiquidKind::Still,
            [3, 3],
            positions,
            vec![true, false, false, false],
        );
        let patch = FoamPatch;
        let chunks = vec![(Entity::PLACEHOLDER, &info, &patch)];
        let (verts, _) = build_patch([2.0, 2.0], 1.5, &chunks).unwrap();
        assert_eq!(verts.len(), 6, "only the one wet cell overlaps");
        let wow = bevy_to_wow(verts[0]);
        // ON the plane, to the bit — the coplanar tie is the rasterizer's, and a geometric lift
        // here is a horizontal overshoot onto dry ground at every shoreline (B348).
        assert!(
            (wow[2] - 5.0).abs() < 1e-4,
            "the patch sits exactly on the liquid surface, unlifted"
        );
        assert!(
            build_patch([50.0, 50.0], 1.5, &chunks).is_none(),
            "dry ⇒ none"
        );
    }
}
