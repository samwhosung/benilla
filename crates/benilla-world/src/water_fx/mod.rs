//! Water foam: the reference's `CWater0Ripple` wade wake, standing ring and step-in splash, as its
//! pool records. A record holds a feet-anchored centre on the liquid surface, a heading, a start
//! size, a growth rate, a lifetime and a peak alpha.
//!
//! A record's geometry is built once, at emission, from the wet liquid cells under its final box,
//! which clips foam at banks. Growth is texgen only: each frame the stencil maps `centre ± size(t)`
//! to UV [0, 1] at the heading, clamped to its transparent border.
//!
//! The driver (`0x5fa760`) emits a wake when translating, a ring when turning or standing, and a
//! full ring on crossing the wade depth (`0x6030c0`).
//!
//! The patch is the liquid mesh's own triangles through the same `clip_from_world`, so the depth
//! test settles its tie with the water; a lift would overshoot onto dry sand on a beach. The
//! reference also lays foam at the MCLQ height, with only a depth bias (`0x68fd0f`):
//! `0.125 × [0x810390]`, D3D `DEPTHBIAS` −1/8192.

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

/// The ring and wake stencils, loaded raw: the alpha is the shape and the near-black RGB the
/// intensity, as the reference modulates both, so foam recoloured white is about 20× too bright.
const RING_TEXTURE: &str = "xtextures/splash/splash.blp";
const WAKE_TEXTURE: &str = "xtextures/splash/wake.blp";

/// The record pool (`0x68f8b0`, `0x68f9f0`): the active mover allocates from `[0, 32)` and
/// everyone else from `[32, 128)`, so others never evict its records.
const POOL_SIZE: usize = 128;
const SELF_SLOTS: usize = 32;

/// The step-in and step-out one-shot depth as a fraction of the unit's own collision height
/// (`0x6030c0`, CMovement+0xb4), latched either way: about 0.81 yd for a human.
const ONESHOT_DEPTH_FRAC: f32 = 0.4;

/// The emission depth gate, `max(2 × collision height, 1.0)`: about 4.06 yd for a human, so a
/// surface swimmer at its 1.52-yd rest depth still foams. The field, `[unit+0x297]`, is
/// CMovement+0xb4, the collision height, not `UNIT_FIELD_BOUNDINGRADIUS`.
const GATE_DEPTH_FRAC: f32 = 2.0;

/// Speed (yd/s) past which a streamed unit counts as translating: its `MOVEMENTFLAGS & 0xf` proxy.
const MOVE_EPSILON: f32 = 0.5;
/// Yaw rate (rad/s) past which a still streamed unit counts as turning: its `& 0x30` proxy.
const TURN_EPSILON: f32 = 0.35;

/// A `CWater0Ripple` pool record: static geometry, with size and alpha derived from `born`.
struct FoamRecord {
    /// Feet position at emission, WoW XY.
    center: [f32; 2],
    /// Texgen heading (WoW radians): the movement direction (wake) or random (ring).
    heading: f32,
    size0: f32,
    /// yd/s.
    growth: f32,
    /// Seconds; the record dies at `born + lifetime`.
    lifetime: f32,
    peak: f32,
    born: f32,
    /// Render category: ring (`splash.blp`) vs wake (`wake.blp`).
    ring: bool,
    /// Static triangles in Bevy space, on the surface, from the wet cells under the final box.
    verts: Vec<Vec3>,
    /// The liquid chunk that hosted the emission; records draw grouped per chunk.
    chunk: Entity,
}

/// Per-unit emitter state: the cooldown cell both kinds share (`unit+0xc78`), so a ring delays the
/// next wake and vice versa; the step-in latch; the motion history for the proxies.
struct UnitFoam {
    last_pos: Option<Vec3>,
    last_yaw: Option<f32>,
    /// Absolute time the next emission is allowed.
    ready: f32,
    /// Step-in latch: currently deeper than the one-shot threshold.
    wading: bool,
    rng: u32,
    /// Fed this frame; an unfed unit's state retires.
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

/// The record pool, its cursors and the per-unit emitters; the avatar keys `Entity::PLACEHOLDER`.
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

/// The next slot in a partition, its oldest record (counters `0xc7f3b0` self, `0xc81d48` others).
fn alloc_slot(cursor: &mut usize, base: usize, len: usize) -> usize {
    let i = base + *cursor;
    *cursor = (*cursor + 1) % len;
    i
}

/// The two foam stencils. Their draws carry the reference's foam render state (`0x68fae0`):
/// additive, fog off, depth-tested `LEQUAL` without depth write, and a depth bias (`0x68fd0f`).
#[derive(Resource)]
struct FoamAssets {
    ring: Handle<Image>,
    wake: Handle<Image>,
}

/// A rung over the whole water band, not an epsilon over one surface: the reference draws foam
/// once per frame, after all liquid (`0x6816d0` tail-jumps to `0x68fae0` on both branches).
use crate::sky_order::FOAM_BIAS;

/// Loads the stencils raw, clamped and without mips, the reference's sampler state.
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

/// Decodes a foam stencil with its authored RGBA verbatim, as raw gamma bytes.
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

/// A record's static patch: every wet cell overlapping the final box, cut into the liquid surface's
/// own two triangles in Bevy space, and the chunk that contains the centre.
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

/// The driver (`0x5fa760`) for one unit this frame: classify, gate and emit, paced by its cooldown.
fn drive_unit(
    foam_state: &mut UnitFoam,
    alloc: &mut dyn FnMut(FoamRecord),
    pos: Vec3,
    state: WadeState,
    scale: f32,
    // The unit's collision height (yd), which both depth lines scale.
    h: f32,
    water: &Query<(Entity, &WaterChunkInfo, &FoamPatch)>,
    index: &WaterIndex,
    now: f32,
) {
    let gate = (GATE_DEPTH_FRAC * h).max(1.0);
    foam_state.active = true;
    let wow = bevy_to_wow(pos);
    // The surface height from the wet cell underfoot: a chunk's box or top vertex puts a sloped
    // river's wade depth 2 yd out. Through the index, so a dry unit costs one hash miss; a dead
    // entry fails `water.get`, and overlapping surfaces take the first match.
    let Some(surface) = index.over(wow[0], wow[1]).iter().find_map(|&e| {
        let (_, info, _) = water.get(e).ok()?;
        info.surface_z_at(wow[0], wow[1])
    }) else {
        foam_state.wading = false;
        return;
    };
    let depth = surface - wow[2];

    // The one-shot fires on crossing the wade depth either way (`0x6030c0`, latch `[+0x269]`);
    // the driver's mode-0xC9 call also clears the cooldown.
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
    // Patch candidates resolve only here, past the gate and the cooldown, over the final box.
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
    // The shared cooldown: the one-shot resets it, a ring re-arms in 400 + U[0, 50) ms, and a
    // wake after about 0.625 yd of travel.
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

/// Runs the driver for the avatar on its movement flags, and for streamed units on the proxies.
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
        if let Some(body) = viewer.at {
            let uf = foam
                .units
                .entry(Entity::PLACEHOLDER)
                .or_insert_with(|| UnitFoam::new(0x5EED_F0A5));
            let prev = uf.last_pos.replace(body);
            let vel = prev.map_or(Vec3::ZERO, |p| (body - p) / dt);
            let w = bevy_to_wow(vel);
            let speed = (w[0] * w[0] + w[1] * w[1]).sqrt();
            // Selection off our own movement flags, as `0x5fa760` does; the masks live on `Viewer`.
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

        // Streamed units; `Without<ViewerUnit>` excludes the avatar's own wire ghost.
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

/// Ages the pool and pushes one additive draw per (chunk, category) with live records: static
/// positions, the texgen at `size(t)` and white times the alpha ramp.
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

    // One contiguous draw per (chunk, category).
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
                // The reference's foam render sets fog off (`0x68fcd0`, `0x68fcd2`, `0x68fcd7`).
                fog: EffectFog::Off,
                // The foam's own additive path (`0x68fae0`) sets no GL_LIGHTING.
                lighting: crate::particles::buffer::EffectLighting::None,
                anchor: centroid / n as f32,
                bias: FOAM_BIAS,
                // A few ULPs and no slope, as the patch already ties the water. The nonzero
                // constant selects `DECAL_WORLD_CLIP`, the world meshes' matrix, so depths agree.
                raster_bias: crate::sky_order::Rung::FOAM_RASTER,
                raster_slope: crate::sky_order::Rung::FOAM_RASTER_SLOPE,
                cam_relative: false,
                // Never the chunk entity: an entity that owns a registered mesh lets bevy's
                // sorted-phase batcher claim the item and rewrite its `batch_range`, a crash.
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
                light: None,
                clip: None,
            },
        );
    }
}

/// The foam emitter's set: the app's `waterfx` capture fixture moves its wading dummy before it.
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
            // The push runs after the frame's clear; emission ran in Update.
            .add_systems(PostUpdate, push_water_foam.after(begin_effect_frame));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty or stale index means no foam anywhere, which no helper-level test catches.
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
        // Feet 1.5 yd under, past the 0.4·h step-in line: the one-shot fires on the first frame.
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

    #[test]
    fn patch_clips_to_wet_cells() {
        // Four 5-yd cells over `[0,10]²`; only (0,0) is wet, so the box's far half is dry ground.
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
        // On the plane: a lift would overshoot onto dry ground at every shoreline.
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
