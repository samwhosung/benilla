//! Ground clutter (`GroundEffect*.dbc`): the terrain streamer scatters each chunk's tufts at tile
//! load, and a chunk's meshes exist only inside the 70 yd detail-doodad horizon (`[0x867958]`),
//! the reference's per-chunk `CDetailDoodadInst` build and unlink.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::view::WorldCamera;
use benilla_assets::coords::wow_to_bevy;
use benilla_assets::materials::WowModelMaterial;
use benilla_assets::LockRecover;
use benilla_assets::{AssetSet, WorldAssets};
use benilla_formats::{
    load_ground_effect_catalog, load_m2_mesh, scatter_ground_doodads, ChunkMesh,
    GroundDoodadPlacement, GroundEffectCatalog, ModelBlend, RenderSubmesh, SHADOW_MAP_SIZE,
};

/// Loads the ground-effect catalog at startup and runs the per-chunk build and teardown.
pub(crate) struct ClutterPlugin;

impl Plugin for ClutterPlugin {
    fn build(&self, app: &mut App) {
        // Inserted at build, not in `setup_clutter`: `benilla-app`'s Startup CVar loader applies a
        // saved `WorldDetail` onto it.
        app.init_resource::<ClutterConfig>()
            .add_systems(Startup, setup_clutter.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    remesh_on_cutout_change,
                    stream_chunk_clutter,
                    evict_clutter_geometry,
                    scope_clutter_geometry,
                )
                    .chain(),
            );
    }
}

/// Drop the built meshes when the cutout value moves (a density write must not re-mesh): the ref is
/// baked into the material. The reference's `detailDoodadAlpha` is a console command (`0x6739a0`,
/// registered at `0x63f9e0`), not a CVar, so it never persists.
fn remesh_on_cutout_change(
    mut commands: Commands,
    cfg: Res<ClutterConfig>,
    mut chunks: Query<&mut ClutterChunk>,
    mut last: Local<Option<f32>>,
) {
    let Some(prev) = last.replace(cfg.alpha_ref) else {
        return;
    };
    if prev == cfg.alpha_ref {
        return;
    }
    let mut n = 0;
    for mut cc in &mut chunks {
        for e in cc.built.drain(..) {
            commands.entity(e).try_despawn();
            n += 1;
        }
    }
    info!(
        "clutter: detailDoodadAlpha {} — dropped {n} built mesh(es) to re-cut",
        (cfg.alpha_ref * 255.0).round() as u32
    );
}

/// Drop the decoded clutter geometry on a map change; the new map decodes its own models.
fn evict_clutter_geometry(
    mut changes: MessageReader<crate::world_map::MapChange>,
    geometry: Option<ResMut<ClutterGeometry>>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    if let Some(mut g) = geometry {
        g.0.clear();
    }
}

/// Expire the decoded clutter geometry by distance within a map.
fn scope_clutter_geometry(
    mut scope: crate::art_scope::ArtScope,
    geometry: Option<ResMut<ClutterGeometry>>,
) {
    if let Some(mut g) = geometry {
        scope.apply(&mut g.0, crate::art_scope::ArtSlot::ClutterGeo);
    }
}

/// Startup: load the ground-effect catalog and insert the geometry cache.
fn setup_clutter(mut commands: Commands, world_assets: Option<ResMut<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    commands.insert_resource(ClutterGeometry::default());
    let mut chain = world_assets.chain.lock_recover();
    // `true`: a `doodadId` names a `GroundEffectDoodad.internalId`, as the reference keys it.
    match load_ground_effect_catalog(&mut chain, true) {
        Ok(catalog) => {
            info!("ground-effect catalog: {} effects", catalog.len());
            commands.insert_resource(GroundClutter { catalog });
        }
        Err(e) => warn!("ground-effect catalog unavailable, no ground clutter: {e:#}"),
    }
}

/// The `effectId → (models, density)` catalog; absent, and no clutter, when the DBCs fail to load.
#[derive(Resource)]
pub(crate) struct GroundClutter {
    pub(crate) catalog: GroundEffectCatalog,
}

/// The detail-doodad alpha-test ref, `detailDoodadAlpha` = 128/255, lower than the model key.
pub(crate) const DETAIL_DOODAD_ALPHA_REF: f32 = 128.0 / 255.0;

/// The reference's hardcoded detail-doodad draw distance, 70 yd (`[0x867958]`); clutter fades out
/// over its last quarter.
pub(crate) const DETAIL_DOODAD_FADE_FAR: f32 = 70.0;

/// Ground-clutter tunables. `density` scales the reference's 16 cells per chunk (`frillDensity`),
/// and a change re-scatters loaded tiles, as the 1.12 setter rebuilds its chunks. Seeded from
/// `$WOW_CLUTTER_DENSITY`, `$WOW_CLUTTER_ALPHA` and `$WOW_CLUTTER_FADE`; density 0 is no clutter.
#[derive(Resource, Clone, Copy)]
pub struct ClutterConfig {
    pub density: f32,
    pub alpha_ref: f32,
    pub fade_far: f32,
}

impl ClutterConfig {
    /// The ground cover as the reference's `frillDensity`, cells visited per chunk; the
    /// `WorldDetail` slider stops are 16/32/48 (`SetWorldDetail`, `0x488dd0`).
    pub fn frill_density(&self) -> f32 {
        self.density * benilla_formats::FRILL_DENSITY as f32
    }

    /// Set the ground cover from a `frillDensity`, clamped to `[1, 256]` as the reference's change
    /// callback `0x688de0` does, so zero is unreachable here.
    pub fn set_frill_density(&mut self, frill: f32) {
        self.density = frill.clamp(1.0, benilla_formats::FRILL_DENSITY_MAX as f32)
            / benilla_formats::FRILL_DENSITY as f32;
    }
}

impl Default for ClutterConfig {
    fn default() -> Self {
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<f32>().ok());
        Self {
            // Deviation: ×2, `frillDensity` 32 (the panel's Medium), because a fresh install's
            // `hwDetect` value (24 from `VideoHardware.dbc`, 8 on the weakest parts) is on no panel
            // stop, and Medium is the nearest stop no sparser.
            density: env("WOW_CLUTTER_DENSITY").unwrap_or(2.0).max(0.0),
            alpha_ref: env("WOW_CLUTTER_ALPHA")
                .unwrap_or(DETAIL_DOODAD_ALPHA_REF)
                .clamp(0.0, 1.0),
            fade_far: env("WOW_CLUTTER_FADE")
                .unwrap_or(DETAIL_DOODAD_FADE_FAR)
                .max(1.0),
        }
    }
}

/// Decoded clutter-model submeshes by path: each detail M2 is decoded once, then merged per chunk.
#[derive(Resource, Default)]
pub(crate) struct ClutterGeometry(
    pub(crate) benilla_assets::SpatialCache<String, Vec<RenderSubmesh>>,
);

/// One MCNK chunk's scattered clutter, meshed only while inside the detail-doodad horizon.
#[derive(Component)]
pub(crate) struct ClutterChunk {
    /// The chunk's terrain box grown by a tuft's height, in Bevy space. The gate measures to its
    /// nearest point, not its centre, so a chunk on relief is not built late.
    bounds: (Vec3, Vec3),
    /// Placements by model path; each model merges into one mesh per submesh.
    models: Vec<(String, Vec<ShadedPlacement>)>,
    /// The built meshes, children of this entity; empty when not built.
    built: Vec<Entity>,
}

/// Slack (yd) past the fade reach, so a chunk is built before any of its grass can show.
const CLUTTER_BUILD_MARGIN: f32 = 8.0;

/// Hysteresis (yd) between build and teardown, so a chunk at the boundary does not thrash.
const CLUTTER_TEARDOWN_HYSTERESIS: f32 = 6.0;

/// How far a tuft reaches above the terrain (yd); the chunk's box is grown by it.
const CLUTTER_TUFT_HEIGHT: f32 = 3.0;

/// Most chunks built per frame, so an arrival in a dense area spreads over a few frames.
const CLUTTER_BUILDS_PER_FRAME: usize = 8;

/// The reference's per-tuft detail-doodad diffuse, picked from the chunk's MCSH map at the
/// placement: `0xFFFFFFFF` lit, `0xFFC0C0C0` shadowed. Baked into the vertex colour.
const MCSH_LIT: f32 = 1.0;
const MCSH_SHADOWED: f32 = 192.0 / 255.0; // 0xC0 / 0xFF

/// The MCSH tint at a world position; lit when the chunk has no MCSH or the point falls outside.
/// MCSH is 64×64, row-major, rows running south and columns east from the chunk's NW corner.
fn mcsh_tint_at(chunk: &ChunkMesh, world: [f32; 3]) -> f32 {
    let Some(shadow) = chunk.shadow.as_ref() else {
        return MCSH_LIT;
    };
    let nw_x = chunk.positions[0][0];
    let nw_y = chunk.positions[0][1];
    let south = (nw_x - world[0]) / benilla_formats::TILE_SIZE * 16.0; // 0..1 across the chunk
    let east = (nw_y - world[1]) / benilla_formats::TILE_SIZE * 16.0;
    if !(0.0..=1.0).contains(&south) || !(0.0..=1.0).contains(&east) {
        return MCSH_LIT;
    }
    let n = SHADOW_MAP_SIZE as usize;
    let row = ((south * n as f32) as usize).min(n - 1);
    let col = ((east * n as f32) as usize).min(n - 1);
    if shadow[row * n + col] >= 128 {
        MCSH_SHADOWED
    } else {
        MCSH_LIT
    }
}

/// The MCNR normal (WoW space) at the outer-grid vertex nearest a position, the ground under a
/// tuft; `+Z` when the chunk has no MCNR.
fn terrain_normal_at(chunk: &ChunkMesh, world: [f32; 3]) -> [f32; 3] {
    if chunk.normals.len() < 145 || chunk.positions.is_empty() {
        return [0.0, 0.0, 1.0];
    }
    let nw_x = chunk.positions[0][0];
    let nw_y = chunk.positions[0][1];
    let south = ((nw_x - world[0]) / benilla_formats::TILE_SIZE * 16.0).clamp(0.0, 1.0);
    let east = ((nw_y - world[1]) / benilla_formats::TILE_SIZE * 16.0).clamp(0.0, 1.0);
    let row = (south * 8.0).round() as usize; // outer 9×9 grid in the stride-17 MCVT layout
    let col = (east * 8.0).round() as usize;
    chunk.normals[row * 17 + col]
}

/// A scattered placement with its baked MCSH tint and ground normal.
struct ShadedPlacement {
    placement: GroundDoodadPlacement,
    tint: f32,
    /// The ground normal under the tuft, in Bevy space, on every vertex: here the MCNR normal of
    /// the nearest outer-grid vertex ([`terrain_normal_at`]). The reference uses the plane of the
    /// cell's centre-fan triangle the tuft lands in (planes built at `0x6bfec1`–`0x6bfefe`, the
    /// triangle picked at `0x6c0148`–`0x6c0193`, the normal stored at `0x6b2a87`–`0x6b2a9c`).
    ground_normal: [f32; 3],
}

/// Scatter a tile's clutter into [`ClutterChunk`]s, tint and normal baked, building no meshes.
pub(crate) fn scatter_tile_clutter(
    commands: &mut Commands,
    chunks: &[ChunkMesh],
    tile_x: u32,
    tile_y: u32,
    catalog: &GroundEffectCatalog,
    density: f32,
    entities: &mut Vec<Entity>,
) {
    for chunk in chunks {
        let mut by_model: HashMap<String, Vec<ShadedPlacement>> = HashMap::new();
        for placement in scatter_ground_doodads(chunk, catalog, tile_x, tile_y, density) {
            let tint = mcsh_tint_at(chunk, placement.position);
            let ground_normal =
                wow_to_bevy(terrain_normal_at(chunk, placement.position)).to_array();
            by_model
                .entry(placement.model.clone())
                .or_default()
                .push(ShadedPlacement {
                    placement,
                    tint,
                    ground_normal,
                });
        }
        if by_model.is_empty() {
            continue;
        }
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in &chunk.positions {
            let v = wow_to_bevy(*p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi.y += CLUTTER_TUFT_HEIGHT;
        entities.push(
            commands
                .spawn(ClutterChunk {
                    bounds: (lo, hi),
                    models: by_model.into_iter().collect(),
                    built: Vec::new(),
                })
                .id(),
        );
    }
}

/// Build one chunk's clutter: each model's instances merged into one mesh per submesh, every tuft's
/// transform, tint and ground normal baked in, spawned as children of `chunk_entity`.
fn build_chunk_clutter(
    chunk_entity: Entity,
    models: &[(String, Vec<ShadedPlacement>)],
    alpha_ref: f32,
    fade_far: f32,
    geometry: &mut ClutterGeometry,
    assets: &mut WorldAssets,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<WowModelMaterial>,
    commands: &mut Commands,
) -> Vec<Entity> {
    let mut out = Vec::new();
    for (model_path, placements) in models {
        let subs = geometry.0.or_insert_with(model_path.clone(), || {
            load_m2_mesh(&mut assets.chain.lock_recover(), model_path).unwrap_or_default()
        });
        if subs.is_empty() {
            continue;
        }
        for sub in subs.iter() {
            let vcount = sub.positions.len() * placements.len();
            let mut positions = Vec::with_capacity(vcount);
            let mut uvs = Vec::with_capacity(vcount);
            // The ground normal under each tuft, never the blade's own M2 normal.
            let mut normals = Vec::with_capacity(vcount);
            let mut colors = Vec::with_capacity(vcount);
            let mut indices = Vec::with_capacity(sub.indices.len() * placements.len());
            for sp in placements {
                let d = &sp.placement;
                let base = positions.len() as u32;
                let origin = wow_to_bevy(d.position);
                let rot = Quat::from_rotation_y(d.yaw);
                // The reference's random per-tuft scale, in [0.9, 1.1].
                let inst_scale = d.scale;
                // One tint per tuft. The reference makes the grey the lit material (colour material
                // on ambient and diffuse, `0x59cfec`), `clamp(grey × light) × texture`; the shader
                // multiplies it outside the clamp, `texture × grey × clamp(light)`, so under light
                // above 1.0 a shadowed tuft comes out darker than the reference's.
                let tint = [sp.tint, sp.tint, sp.tint, 1.0];
                for (i, p) in sub.positions.iter().enumerate() {
                    positions.push((rot * (wow_to_bevy(*p) * inst_scale) + origin).to_array());
                    uvs.push(sub.uvs[i]);
                    normals.push(sp.ground_normal);
                    colors.push(tint);
                }
                indices.extend(sub.indices.iter().map(|idx| base + idx));
            }
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            // The MCSH tint, under the `VERTEX_COLORS` def the shader's clutter path reads.
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(Indices::U32(indices));
            let mesh = meshes.add(mesh);
            // The detail-doodad pass owns the render state, never the batch: the reference sets one
            // for every tuft (`0x6b2b80`: alpha blend, ALPHAREF `detailDoodadAlpha`, cull off,
            // depth write on), and its per-slot draw `0x6b2d60` binds only the texture.
            let material = assets.model_material(
                sub.texture.as_deref(),
                ModelBlend::AlphaTest, // the pass's cutout, not the batch's authored blend
                true,                  // cull off for every tuft, like the reference's pass
                Some(alpha_ref),       // the detail-doodad alpha-test ref, not the general key
                Some(fade_far),        // the detail-doodad distance fade
                false,                 // not WMO: keeps the ground-normal N·L path
                false,                 // not a doodad fade twin: clutter fades on its own path
                (sub.wrap_x, sub.wrap_y), // the M2 texture record's own address mode
                images,
                materials,
            );
            out.push(
                commands
                    .spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::IDENTITY))
                    .id(),
            );
        }
    }
    if !out.is_empty() {
        // Parent at apply time: a tile unload in between despawns the chunk, and `add_children`
        // on a dead parent panics, so the orphans are despawned instead.
        let children = out.clone();
        commands.queue(
            move |world: &mut World| match world.get_entity_mut(chunk_entity) {
                Ok(mut parent) => {
                    parent.add_children(&children);
                }
                Err(_) => {
                    for e in children {
                        if let Ok(child) = world.get_entity_mut(e) {
                            child.despawn();
                        }
                    }
                }
            },
        );
    }
    out
}

/// Squared distance from a point to an axis-aligned box, zero inside.
fn box_distance_squared(p: Vec3, (lo, hi): (Vec3, Vec3)) -> f32 {
    (lo - p).max(p - hi).max(Vec3::ZERO).length_squared()
}

/// The fade horizon's reach along the frustum's corner ray, as a multiple of it: the fade is on
/// view depth (the reference's camera-space texgen), the gate on a distance that ignores heading.
fn frustum_corner_reach(fov_y: f32, aspect: f32) -> f32 {
    let tan_v = (fov_y * 0.5).tan();
    let tan_h = tan_v * aspect;
    (1.0 + tan_v * tan_v + tan_h * tan_h).sqrt()
}

/// Build a chunk's clutter when its box comes within the fade reach, and tear it down past it,
/// where the fade is already zero. The reference builds and unlinks per chunk at 70 yd
/// (`[0x867958]`), by the nearest view depth of the chunk's bounding sphere.
pub(crate) fn stream_chunk_clutter(
    mut commands: Commands,
    cam: Query<(&GlobalTransform, Option<&Projection>), With<WorldCamera>>,
    mut chunks: Query<(Entity, &mut ClutterChunk)>,
    cfg: Res<ClutterConfig>,
    geometry: Option<ResMut<ClutterGeometry>>,
    assets: Option<ResMut<WorldAssets>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
) {
    // Both are absent without an install, and a hard `ResMut` would fail validation and panic.
    let (Some(mut geometry), Some(mut assets)) = (geometry, assets) else {
        return;
    };
    let Some((cam_tf, projection)) = cam.iter().next() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    // Without a perspective projection, Bevy's default stands in: the one `view.rs` keeps, about
    // the reference's 44.1° vertical, at 16:9.
    let reach = cfg.fade_far
        * match projection {
            Some(Projection::Perspective(p)) => frustum_corner_reach(p.fov, p.aspect_ratio),
            _ => {
                let d = PerspectiveProjection::default();
                frustum_corner_reach(d.fov, d.aspect_ratio)
            }
        };
    let build_d2 = (reach + CLUTTER_BUILD_MARGIN).powi(2);
    let drop_d2 = (reach + CLUTTER_BUILD_MARGIN + CLUTTER_TEARDOWN_HYSTERESIS).powi(2);
    // The late-build tripwire: a chunk built already inside the horizon pops in where it is seen.
    let visible_d2 = cfg.fade_far.powi(2);

    // Pass 1: tear down what has left; collect the rest nearest first, for the per-frame cap.
    let mut wanted: Vec<(f32, Entity)> = Vec::new();
    for (ent, mut cc) in &mut chunks {
        let d2 = box_distance_squared(cam_pos, cc.bounds);
        if cc.built.is_empty() {
            if d2 <= build_d2 {
                wanted.push((d2, ent));
            }
        } else if d2 > drop_d2 {
            for e in cc.built.drain(..) {
                commands.entity(e).try_despawn();
            }
        }
    }
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let backlog = wanted.len().saturating_sub(CLUTTER_BUILDS_PER_FRAME);

    // Pass 2: spend the frame's budget on the nearest of them.
    let (mut late, mut late_nearest) = (0usize, f32::MAX);
    for &(d2, ent) in wanted.iter().take(CLUTTER_BUILDS_PER_FRAME) {
        let Ok((_, mut cc)) = chunks.get_mut(ent) else {
            continue;
        };
        if d2 < visible_d2 {
            late += 1;
            late_nearest = late_nearest.min(d2.sqrt());
        }
        let built = build_chunk_clutter(
            ent,
            &cc.models,
            cfg.alpha_ref,
            cfg.fade_far,
            &mut geometry,
            &mut assets,
            &mut meshes,
            &mut images,
            &mut materials,
            &mut commands,
        );
        cc.built = built;
    }
    // With a backlog the cap made it late, the expected burst behind a login or teleport's loading
    // cover; with none the distance gate let a visible chunk through late, a defect.
    if late > 0 {
        if backlog > 0 {
            debug!(
                "clutter: {late} chunk(s) built inside the {:.0} yd horizon (nearest \
                 {late_nearest:.1} yd) — {backlog} more queued behind the \
                 {CLUTTER_BUILDS_PER_FRAME}/frame cap (the expected login/teleport burst)",
                cfg.fade_far,
            );
        } else {
            warn!(
                "clutter: {late} chunk(s) built INSIDE the {:.0} yd horizon (nearest \
                 {late_nearest:.1} yd) — grass appeared where it could already be seen, and with \
                 no build backlog, so the DISTANCE GATE let it through late",
                cfg.fade_far,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fade ramp `wow_model.wgsl` evaluates, mirrored as a pin on its constants.
    fn ramp(z: f32, far: f32) -> f32 {
        let near = far * 0.75;
        let u = (z - near) / (far - near);
        ((254.0 - 256.0 * u) / 255.0).clamp(0.0, 252.0 / 255.0)
    }

    /// The reference's 64-texel clamp/linear ramp (`0x6b235b` fills it, `[0x867958]` = 70 anchors
    /// it): `alpha = (254 − 256u)/255` capped at `252/255`, so the plateau ends at 52.63672 yd,
    /// zero falls at 69.86328 yd, and the 128/255 cutout erases opaque grass at 61.11328 yd.
    #[test]
    fn the_ramp_hits_the_reference_landmarks() {
        assert!(
            (ramp(0.0, 70.0) - 252.0 / 255.0).abs() < 1e-6,
            "near plateau"
        );
        assert!(
            (ramp(52.63672, 70.0) - 252.0 / 255.0).abs() < 1e-5,
            "plateau ends at 52.63672, not 52.5"
        );
        assert!(
            ramp(52.7, 70.0) < 252.0 / 255.0,
            "past the plateau it falls"
        );
        assert!(ramp(69.86328, 70.0) < 1e-5, "zero at 69.86328, not 70");
        assert_eq!(ramp(70.0, 70.0), 0.0);
        let slope = ramp(60.0, 70.0) - ramp(61.0, 70.0);
        assert!((slope - 0.0573670).abs() < 1e-6, "slope per yard");
        assert!(ramp(61.11328, 70.0) < DETAIL_DOODAD_ALPHA_REF + 1e-5);
        assert!(ramp(61.11, 70.0) > DETAIL_DOODAD_ALPHA_REF);
    }

    #[test]
    fn box_distance_measures_the_nearest_point() {
        let b = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(box_distance_squared(Vec3::ZERO, b), 0.0);
        assert_eq!(box_distance_squared(Vec3::new(4.0, 0.0, 0.0), b), 9.0);
        // A tall chunk: the point is 3 yd from its top face but 5 yd from its centre.
        let tall = (Vec3::new(-1.0, -4.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
        assert_eq!(box_distance_squared(Vec3::new(4.0, 4.0, 0.0), tall), 9.0);
    }

    #[test]
    fn the_corner_reach_widens_with_the_aspect() {
        let fov = PerspectiveProjection::default().fov;
        let wide = frustum_corner_reach(fov, 16.0 / 9.0);
        let ultrawide = frustum_corner_reach(fov, 21.0 / 9.0);
        assert!((wide - 1.309).abs() < 0.01, "16:9 reach was {wide}");
        assert!(
            (ultrawide - 1.451).abs() < 0.01,
            "21:9 reach was {ultrawide}"
        );
        assert!(ultrawide > wide);
        // Dead ahead is the degenerate case: no width, no extra reach.
        assert!((frustum_corner_reach(0.0, 0.0) - 1.0).abs() < 1e-6);
    }
}
