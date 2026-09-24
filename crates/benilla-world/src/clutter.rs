//! Ground clutter: grass/flowers/pebbles scattered per MCNK cell (authored `GroundEffect*.dbc`
//! placement). At tile-load each chunk's tufts are **scattered** (MCSH/normal-baked) into a per-chunk
//! [`ClutterChunk`]; [`stream_chunk_clutter`] then **builds** its merged meshes lazily only while the
//! chunk is within the ~70 yd detail-doodad horizon and tears them down past it — the reference's
//! per-chunk `CDetailDoodadInst` lifecycle (build/unlink at the 70 yd `[0x867958]`), which bounds
//! live grass to a bubble and spreads the build across frames. [`ClutterPlugin`] owns the
//! catalog + the lazy build lifecycle
//! independently of the terrain streamer; whichever streamer is active does the per-tile *scatter* (it
//! has the tile's chunks) into the `ClutterChunk`s this builds — so the streamer can be swapped.

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

/// Ground clutter as its own subsystem: loads the `GroundEffect*` catalog + config at startup, and runs
/// the lazy per-chunk build/teardown every frame. The per-tile *scatter* is driven by whichever terrain
/// streamer is active (it has the tile's chunks); this plugin owns the catalog and the build lifecycle
/// so they don't depend on the terrain streamer's own setup (which lets the streamer be swapped).
pub(crate) struct ClutterPlugin;

impl Plugin for ClutterPlugin {
    fn build(&self, app: &mut App) {
        // The config exists from plugin build, not `setup_clutter`: the CVar loader
        // (`crate::cvars::load_config`, also Startup) applies a saved `WorldDetail` onto it, and
        // an ordering flip must not silently drop the file's value (0992).
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

/// Drop every built clutter mesh when the detail-doodad **cutout** moves, so the lazy builder
/// re-meshes the bubble against the new reference — the alpha-test ref is baked into the material at
/// build time (it is part of `model_material`'s dedup key), so nothing already on screen would
/// otherwise notice. `detailDoodadAlpha` is a live console command in the reference
/// (`0x6739a0`, registrar `0x63f9e0` — a command, not a CVar, so it never persists), where the
/// global is read at draw time and needs no rebuild; ours costs a re-mesh of the ~30 chunks in the
/// bubble, which the per-frame cap spreads over a few frames.
///
/// Watches the **value**, not `is_changed()`, because the predicate is "the cutout moved" and
/// not "the resource moved": `ClutterConfig` also carries the density, whose own writer would
/// otherwise cost a re-mesh of the bubble on every detail-slider notch. First sight only arms.
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

/// Drop the decoded clutter-geometry cache on a cross-map transition (`world_map::MapChange` —
/// see its doc): it holds CPU submesh copies of every detail M2 ever decoded, and clutter models
/// are map-flavored (a new map re-decodes its own handful on first build).
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

/// Expire the decoded clutter geometry by **distance** (decision 0793) — the within-map half of
/// [`evict_clutter_geometry`]. Clutter models are map-*flavoured*: the handful a zone uses is decoded
/// on its first chunk build and then held for the process, so a continent tour accumulates every
/// zone's set. Re-approaching re-decodes one M2 off the chain, inside the same lazy per-chunk build
/// that would rebuild the meshes anyway.
fn scope_clutter_geometry(
    mut scope: crate::art_scope::ArtScope,
    geometry: Option<ResMut<ClutterGeometry>>,
) {
    if let Some(mut g) = geometry {
        scope.apply(&mut g.0, crate::art_scope::ArtSlot::ClutterGeo);
    }
}

/// Startup: load the ground-effect (clutter) catalog off the shared chain + insert the per-chunk
/// geometry cache. (Moved out of the terrain streamer's `setup_terrain`.)
fn setup_clutter(mut commands: Commands, world_assets: Option<ResMut<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    // Per-chunk clutter geometry cache, read by the lazy `stream_chunk_clutter` build.
    commands.insert_resource(ClutterGeometry::default());
    let mut chain = world_assets.chain.lock_recover();
    // key_by_internal_id = true: the faithful GroundEffectDoodad keying (see load_ground_effect_catalog).
    match load_ground_effect_catalog(&mut chain, true) {
        Ok(catalog) => {
            info!("ground-effect catalog: {} effects", catalog.len());
            commands.insert_resource(GroundClutter { catalog });
        }
        Err(e) => warn!("ground-effect catalog unavailable, no ground clutter: {e:#}"),
    }
}

/// Ground-clutter rendering: the `effectId → (models, density)` catalog (GroundEffect* DBCs).
/// Optional — if the DBCs fail to load, the ground simply has no detail doodads. Clutter is scattered
/// per chunk into [`ClutterChunk`] units at tile-load (tracked with the tile so it streams out with the
/// terrain) and built lazily by [`stream_chunk_clutter`]. The raw model geometry is cached in
/// [`ClutterGeometry`].
#[derive(Resource)]
pub(crate) struct GroundClutter {
    pub(crate) catalog: GroundEffectCatalog,
}

/// The client's detail-doodad alpha-test reference: `detailDoodadAlpha`, default **128/255 ≈ 0.5**
/// (RE'd in `WoW.exe`). Foliage is an alpha-tested cutout at
/// this ref. We default clutter to it instead of the general (higher) vanilla model key, which was
/// over-cutting the thin grass blades in the shared grass/flower atlas (they vanished while the solid
/// flowers survived).
pub(crate) const DETAIL_DOODAD_ALPHA_REF: f32 = 128.0 / 255.0;

/// The client's detail-doodad draw distance — a **hardcoded 70 yd** (RE'd `_DAT_00867958`); clutter
/// fades out over the last quarter (full to 52.5 yd = 0.75×, gone by 70 yd). This is why the real
/// client's clutter "appears as you walk into it" rather than being drawn to the horizon.
pub(crate) const DETAIL_DOODAD_FADE_FAR: f32 = 70.0;

/// Ground-clutter tunables (read at tile scatter; a `density` change re-scatters LOADED tiles too —
/// `terrain_stream::rescatter_clutter`, the 1.12 setter's own chunk-rebuild law, 0992):
/// `density` multiplies the per-chunk cell-visit count (the client's `frillDensity`, faithful=16 at ×1)
/// — player-settable through **either** registered CVar over this one field, [`ClutterConfig::frill_density`]
/// being the conversion: `WorldDetail` (the panel's stop, 0/1/2 → ×1/×2/×3) or `frillDensity` (the
/// reference's own cells-per-chunk, 1..256), both arms in `benilla-app`'s `cvars`;
/// `alpha_ref` is the alpha-test cutout threshold
/// ([`DETAIL_DOODAD_ALPHA_REF`]); `fade_far` is the clutter draw-distance horizon (yd,
/// [`DETAIL_DOODAD_FADE_FAR`]; fade starts at 0.75×). Initial values from `$WOW_CLUTTER_DENSITY` /
/// `$WOW_CLUTTER_ALPHA` / `$WOW_CLUTTER_FADE`; `density 0` disables clutter.
#[derive(Resource, Clone, Copy)]
pub struct ClutterConfig {
    pub density: f32,
    pub alpha_ref: f32,
    pub fade_far: f32,
}

impl ClutterConfig {
    /// This session's ground cover in the **reference's own unit** — `frillDensity`, the number of
    /// cells the scatter visits per chunk ([`benilla_formats::FRILL_DENSITY`] at ×1).
    ///
    /// The multiplier above is benilla's spelling of a knob 1.12 keeps in cells: its slider stops
    /// are `SetWorldDetail 0x488dd0`'s 16/32/48, which are this constant times 1/2/3. So the two
    /// registered CVars over this field — `WorldDetail` (the stop) and `frillDensity` (the cells) —
    /// are one knob read two ways, and this pair is where that conversion lives so neither the CVar
    /// host nor the scatter carries a bare 16.
    pub fn frill_density(&self) -> f32 {
        self.density * benilla_formats::FRILL_DENSITY as f32
    }

    /// Set the ground cover from a `frillDensity`, under the reference's own clamp.
    ///
    /// `[1, 256]` is what the real client's change callback `0x688de0` pins a written value to —
    /// **not** `[16, 48]`: the slider's three stops are one writer of this CVar, and a console
    /// `frillDensity 200` is another. It is also why the `WorldDetail` arm's clamp is the tighter
    /// one: there the stop is the value, here the cells are.
    ///
    /// Zero is deliberately NOT reachable here, matching that callback — clutter-off stays the
    /// `$WOW_CLUTTER_DENSITY=0` lever's, which is an instrument rather than a setting.
    pub fn set_frill_density(&mut self, frill: f32) {
        self.density = frill.clamp(1.0, benilla_formats::FRILL_DENSITY_MAX as f32)
            / benilla_formats::FRILL_DENSITY as f32;
    }
}

impl Default for ClutterConfig {
    fn default() -> Self {
        let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<f32>().ok());
        Self {
            // Default ×2 = frillDensity 32 = the panel's Medium (1649). It was ×3/High, matched to
            // the director's OWN reference install, which runs High — a setting on their machine,
            // not a default of the game's: `frillDensity` registers at 16, and a first launch has
            // `hwDetect` overwrite it from `VideoHardware.dbc` to 24 on any D3D9-class part and 8
            // on the weakest. Both are below Medium, and 24 is not on a panel stop at all, so every
            // stop is a divergence; High was simply the dearest one, and this is alpha-tested
            // overdraw on the ground — the thing a bandwidth-bound part has least of. Medium is the
            // nearest stop no sparser than a fresh install, and the row is one drag from High.
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

/// Decoded clutter-model geometry cache (model path → submeshes), keyed by normalized path so each
/// tiny detail M2 is loaded once and then *merged* per chunk. Lives as a resource (not on the streamer)
/// so the lazy per-chunk build system can reach it independently of `WorldAssets`. Replaces the
/// streamer's old per-tile `clutter_cache`.
#[derive(Resource, Default)]
pub(crate) struct ClutterGeometry(
    pub(crate) benilla_assets::SpatialCache<String, Vec<RenderSubmesh>>,
);

/// One MCNK chunk's ground clutter as a **lazily-built** unit — the faithful per-chunk `CDetailDoodadInst`
/// lifecycle (build/unlink at the 70 yd `[0x867958]`): scattered + MCSH/normal-baked at
/// tile-load, but its meshes are
/// built only while the chunk is within the detail-doodad horizon and torn down past it. This bounds live
/// grass to a ~70 yd bubble (vs every loaded tile) and spreads the mesh-build over frames instead of one
/// per-tile hitch. Owned by its tile (despawned on unload, which cascades to `built`).
#[derive(Component)]
pub(crate) struct ClutterChunk {
    /// The chunk's clutter AABB in Bevy space (its terrain box, grown by a tuft's height).
    /// Build/teardown gate on the camera's distance to the BOX — its nearest point — which is what
    /// "any of this chunk is still within reach" actually means. A centre distance has to carry a
    /// chunk-radius margin to approximate it, and that margin is a horizontal figure: on relief a
    /// 33 yd chunk's vertical spread pushes its centre past the margin while its near corner is
    /// still deep inside the visible band, and the whole chunk's grass pops in on approach (2004).
    bounds: (Vec3, Vec3),
    /// Scattered placements grouped by model path; each becomes one merged mesh per submesh when built.
    models: Vec<(String, Vec<ShadedPlacement>)>,
    /// The spawned merged meshes while built (children of this entity); empty ⇒ not currently built.
    built: Vec<Entity>,
}

/// Slack (yd) past the reach computed below, so a chunk is built a moment before any of its grass
/// could be visible and the per-frame build cap has room to spend. Past the reach the ramp is
/// already alpha 0, so building/tearing down here is invisible — the same reason the reference ties
/// its build distance to the fade end.
const CLUTTER_BUILD_MARGIN: f32 = 8.0;

/// Hysteresis (yd) between the build distance and the teardown distance, so a chunk hovering at the
/// boundary doesn't thrash build↔despawn every frame.
const CLUTTER_TEARDOWN_HYSTERESIS: f32 = 6.0;

/// How far above the terrain surface a tuft reaches (yd) — the chunk's clutter AABB is its terrain
/// box grown by this, so the gate measures the geometry that actually draws.
const CLUTTER_TUFT_HEIGHT: f32 = 3.0;

/// Build at most this many chunks' clutter per frame, so entering a dense area (or a teleport) spreads
/// the mesh-build + GPU upload over a few frames instead of one stutter.
const CLUTTER_BUILDS_PER_FRAME: usize = 8;

/// **Step 8 — per-doodad MCSH shadow tint.** Vanilla draws detail doodads UNLIT with a hardcoded
/// DIFFUSE constant: `0xFFFFFFFF ≈ 1.0` (lit) or `0xFFC0C0C0 ≈ 0.753` (MCSH-shadowed). Picked
/// per-vertex from the chunk's static MCSH map at the doodad's placement coord. q12 RE:
/// no day/night, no ambient, no N·L — the only shading
/// input is this grey. Baked into vertex `ATTRIBUTE_COLOR` at scatter time so the model shader
/// can `texture × vertex_color` in gamma space (the faithful MODULATE 1×).
const MCSH_LIT: f32 = 1.0;
const MCSH_SHADOWED: f32 = 192.0 / 255.0; // 0xC0 / 0xFF

/// Sample the chunk's MCSH map at a world-space placement coord. Returns the per-doodad shadow
/// tint (1.0 lit, ≈0.753 shadowed). Falls back to lit when the chunk has no MCSH (most chunks
/// don't) or when the position somehow lands outside the chunk's footprint. Coord math: the
/// chunk's NW corner is `chunk.positions[0]`; +X is north (south distance = `nw_x − wx`), +Y is
/// west (east distance = `nw_y − wy`); MCSH is row-major over the chunk's 0..1 UV with row=south,
/// col=east, 64×64 texels.
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

/// Sample the chunk's MCNR terrain normal (WoW coords) nearest a world-space position — the **ground
/// normal under the tuft**. Lighting clutter by this (instead of a uniform world-up) makes a tuft
/// follow the terrain beneath it: darker on a slope facing away from the sun, like the dirt under it
/// — the per-position sun/shade variation the reference shows. Nearest outer-grid vertex (≈2 yd
/// spacing) is plenty; falls back to WoW-up (`+Z`) when the chunk has no MCNR.
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

/// A scattered placement plus its baked MCSH tint and ground normal — what the merge iterates over.
struct ShadedPlacement {
    placement: GroundDoodadPlacement,
    tint: f32,
    /// Terrain (ground) normal under the tuft, **Bevy space** — baked onto every vertex so clutter is
    /// lit by the ground it stands on (`wow_model.wgsl` ground-normal N·L). Same value for all of a
    /// tuft's vertices ⇒ no per-face view-dependence. VERIFIED faithful: WoW writes the terrain
    /// quadrant-plane normal onto the clutter vertex normal channel.
    ground_normal: [f32; 3],
}

/// **Scatter** a tile's ground clutter into per-chunk [`ClutterChunk`] units — WITHOUT building any
/// meshes. Each chunk's placements are MCSH/normal-baked here (while the parent chunk is in scope; the
/// static per-vertex shading matches the real client, baked at map-compile time) and stored on a
/// `ClutterChunk` entity owned by the tile. [`stream_chunk_clutter`] builds the meshes lazily as chunks
/// come within the detail-doodad horizon. Empty chunks are skipped (no entity). Replaces the old
/// build-everything-at-tile-load path.
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
        // The chunk's terrain AABB in Bevy space — what the LOD system measures against.
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in &chunk.positions {
            let v = wow_to_bevy(*p);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        // Tufts stand ON the surface, so the drawn geometry reaches a little above the terrain box.
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

/// **Lazily build** one chunk's clutter: merge every instance of each model (within this chunk) into one
/// mesh per submesh — baking each tuft's yaw/pos/scale + MCSH tint + ground normal into the vertices —
/// and spawn them as children of `chunk_entity` (so a tile unload cascades to them). Returns the spawned
/// entities (tracked on the `ClutterChunk` for distance teardown). Same merge as the old per-tile path,
/// now per-chunk so only the ~70 yd bubble is ever built/drawn.
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
        // One merged mesh per submesh (each submesh has its own texture/blend → material).
        for sub in subs.iter() {
            let vcount = sub.positions.len() * placements.len();
            let mut positions = Vec::with_capacity(vcount);
            let mut uvs = Vec::with_capacity(vcount);
            // Clutter normals = the GROUND normal under each tuft (the terrain quadrant normal WoW
            // bakes onto the clutter vertex channel — NOT the grass-blade M2 normal). Same value on
            // every vertex of a tuft ⇒ no per-face view-dependence; the shader lights it like the
            // terrain beneath.
            let mut normals = Vec::with_capacity(vcount);
            let mut colors = Vec::with_capacity(vcount);
            let mut indices = Vec::with_capacity(sub.indices.len() * placements.len());
            for sp in placements {
                let d = &sp.placement;
                let base = positions.len() as u32;
                let origin = wow_to_bevy(d.position);
                let rot = Quat::from_rotation_y(d.yaw);
                // Per-instance scale (the client's random [0.9,1.1]).
                let inst_scale = d.scale;
                // MCSH tint per placement — same value on every vertex of THIS instance, so all
                // tris of one grass clump share the lit/shadowed colour. The shader multiplies
                // `texture × vertex_color` in gamma space (the faithful MODULATE 1×).
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
            // Step 8: per-vertex MCSH tint (1.0 lit / ≈0.753 shadowed). Triggers Bevy's
            // `VERTEX_COLORS` shader def → `pbr_input.material.base_color` becomes
            // `vertex_color × texture`. Read by the clutter path in `wow_model.wgsl`.
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(Indices::U32(indices));
            let mesh = meshes.add(mesh);
            // **The detail-doodad pass owns its render state** — it is not the batch's (2004).
            // The reference sets one state for every tuft in the frame (`0x6b2b80`: blend mode 2
            // = SRC_ALPHA/INV_SRC_ALPHA, ALPHAREF = `detailDoodadAlpha` 128, cull OFF, depth-write
            // ON) and its per-slot draw `0x6b2d60` binds nothing but the texture — the M2 batch's
            // own blend/cull is never consulted for a detail doodad. Passing `sub.blend` through
            // gave the two Opaque-authored detail models no cutout at all (their transparent
            // margin drew as solid card) and left five single-sided ones showing only one face.
            let material = assets.model_material(
                sub.texture.as_deref(),
                ModelBlend::AlphaTest, // the pass's cutout, not the batch's authored blend
                true,                  // cull off for every tuft, like the reference's pass
                Some(alpha_ref), // detail-doodad alpha-test ref (≈0.5), not the higher general key
                Some(fade_far),  // detail-doodad distance fade (≈70 yd horizon — opacity, dc30e1f)
                false,           // clutter is not WMO (keeps its ground-normal N·L path)
                false, // not a doodad fade twin (clutter's own clutter_fade path handles it)
                (sub.wrap_x, sub.wrap_y), // the M2 texture record's own address mode (0763)
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
        // Parent the freshly-built clutter to its chunk — but tolerate the chunk having been despawned
        // before this command applies. A cross-map teleport (or any tile unload) despawns the tile,
        // which cascades to its `ClutterChunk` children; a plain `commands.entity(..).add_children`
        // then PANICS on the dead parent (the Ashenvale-teleport crash). Check at apply time, and if the
        // parent is gone, despawn the orphan clutter we just spawned so it doesn't leak as parentless
        // (world-baked) geometry.
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

/// Squared distance from a point to an axis-aligned box — zero inside it. The clutter LOD's measure:
/// "is any of this chunk within reach", which a centre distance can only approximate.
fn box_distance_squared(p: Vec3, (lo, hi): (Vec3, Vec3)) -> f32 {
    (lo - p).max(p - hi).max(Vec3::ZERO).length_squared()
}

/// How far (as a multiple of the fade horizon) a chunk can sit from the eye and still have grass the
/// shader will draw.
///
/// The fade is keyed on **view-space depth** (`wow_model.wgsl`, the reference's camera-space texgen),
/// so the horizon is a plane across the view, not a sphere around the eye: a fragment out at the
/// frustum's corner is `1/cos θ` further in straight-line distance than one dead ahead at the same
/// depth. The LOD gate measures straight-line distance — it must not depend on where the camera
/// happens to be pointing, or a turn would drop and rebuild chunks that are still being drawn — so it
/// takes the worst case, the corner ray, and that is exactly this factor. Derived from the live
/// projection rather than pinned, because it moves with the aspect ratio: ~1.31 at 16:9 and ~1.45 at
/// 21:9, and a constant tuned on one monitor silently clips the screen edges on a wider one.
fn frustum_corner_reach(fov_y: f32, aspect: f32) -> f32 {
    let tan_v = (fov_y * 0.5).tan();
    let tan_h = tan_v * aspect;
    (1.0 + tan_v * tan_v + tan_h * tan_h).sqrt()
}

/// The per-chunk clutter LOD: build a chunk's clutter when its BOX comes within reach of the
/// detail-doodad horizon and tear it down when it leaves — the reference's `CDetailDoodad` lifecycle
/// (per-chunk build/unlink at the 70 yd `[0x867958]`, which it measures as the nearest view-space
/// depth of the chunk's bounding sphere and then never frees). Bounds live grass to a bubble around
/// the player instead of every loaded tile and, with the per-frame build cap, spreads the cost so a
/// tile-load no longer builds 256 chunks at once. Build/teardown happen where the fade ramp is
/// already alpha 0, so they are invisible.
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
    // Both come from `setup_clutter`, which inserts neither when there is no client data — and a
    // hard `ResMut` here is a *validation* failure, not a `None` the body can handle: the system
    // never runs, Bevy's default error handler panics, and a client that found no install died on
    // its first frame instead of sitting at the login screen (decision 1451).
    let (Some(mut geometry), Some(mut assets)) = (geometry, assets) else {
        return;
    };
    let Some((cam_tf, projection)) = cam.iter().next() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    // The straight-line reach the view-depth fade implies at the frustum's corner (see the helper).
    // A missing/non-perspective projection falls back to Bevy's own perspective default, the one
    // `view.rs` leaves in place (≈ the reference's 44.1° vertical) at 16:9.
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
    // The LATE-BUILD tripwire (2012). A chunk built while its nearest corner is ALREADY inside the
    // fade horizon had grass the player could see before the mesh existed — the "it popped in"
    // class, and the thing to rule out first whenever clutter is reported appearing abruptly.
    // Straight-line distance under the horizon implies view depth under it too, so this catches
    // every genuinely-visible case (and some invisible ones, which is the safe direction). Two
    // causes, and the line says which: the gate let it through late, or the per-frame cap deferred
    // it — the second is expected in the burst right after a login or a teleport, when the whole
    // bubble is built at once, and is why this reports the backlog rather than just the count.
    let visible_d2 = cfg.fade_far.powi(2);

    // Pass 1: measure every chunk, tear down what has left, and collect what wants building.
    // **Nearest first** (2012). The build budget is spent in whatever order the query hands the
    // chunks over, which is archetype order — so a chunk 90 yd away, where the ramp is already
    // alpha 0 and nobody can see it, would take a build slot ahead of one at 48 yd that is inside
    // the visible band. On a cold fill that is a pop with a free fix: sort the candidates by
    // distance and the cap always buys the most visible grass first.
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
    // Two cases, and only one is a defect — the message always said so, but BOTH were warnings, so
    // the benign one cried wolf. A login/teleport burst has a BACKLOG: the per-frame cap is why the
    // near chunks are late, and the whole burst runs behind the loading cover. The director's
    // 2026-09-15 log has three of these at t+1.7 s under a cover that did not lift until t+4.3 s —
    // including one announcing grass at 4.4 yd that nobody could possibly have seen. With NO
    // backlog the cap is not the cause: the distance gate let a near chunk through late on an
    // ordinary frame, the player can see that one, and that one still warns.
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

    /// The ramp `wow_model.wgsl` evaluates, mirrored here so the reference's four landmarks are
    /// **checkable** rather than a comment. Not a second implementation to keep in sync — a pin: if
    /// anyone retunes `0.75`, the `254`/`256` texel-centre constants or the `252/255` cap without
    /// re-deriving them, these numbers move and the test says so.
    fn ramp(z: f32, far: f32) -> f32 {
        let near = far * 0.75;
        let u = (z - near) / (far - near);
        ((254.0 - 256.0 * u) / 255.0).clamp(0.0, 252.0 / 255.0)
    }

    /// The 64-texel CLAMP/LINEAR ramp (`0x6b235b` fills it, `[0x867958] = 70.0` anchors it): its
    /// texel centres give `alpha = (254 − 256u)/255`, capped at texel 0's `252/255`. So the plateau
    /// runs to 52.63672 yd (not 52.5), the ramp hits zero at 69.86328 yd (not 70), the slope is
    /// −0.0573670 per yard, and the 128/255 detail-doodad cutout erases a fully-opaque texel at
    /// 61.11328 yd — the horizon grass actually vanishes at, well short of the 70 yd draw distance.
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
        // The cutout erases a fully-opaque texel where the ramp crosses the 128/255 reference.
        assert!(ramp(61.11328, 70.0) < DETAIL_DOODAD_ALPHA_REF + 1e-5);
        assert!(ramp(61.11, 70.0) > DETAIL_DOODAD_ALPHA_REF);
    }

    /// The gate measures the chunk's box, not its centre: zero inside, and the nearest point outside.
    #[test]
    fn box_distance_measures_the_nearest_point() {
        let b = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(box_distance_squared(Vec3::ZERO, b), 0.0);
        assert_eq!(box_distance_squared(Vec3::new(4.0, 0.0, 0.0), b), 9.0);
        // A tall thin chunk: a point level with the top face is 3 yd away, though its CENTRE — the
        // old measure — is 5 yd away. That gap is the relief case that popped whole chunks.
        let tall = (Vec3::new(-1.0, -4.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
        assert_eq!(box_distance_squared(Vec3::new(4.0, 4.0, 0.0), tall), 9.0);
    }

    /// The fade is keyed on view depth, so the LOD's straight-line reach is the corner ray's — and it
    /// widens with the aspect ratio, which is why it is derived and not pinned to one monitor.
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
