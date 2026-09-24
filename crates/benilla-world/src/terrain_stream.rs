//! The terrain streamer, the world's one terrain owner: it loads every ADT tile in range through
//! the `AssetServer`, spawns its cells, collider, placements, MCLQ water and ground clutter, and
//! publishes the residency the loading screen reads.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use benilla_assets::coords::{bevy_to_wow, placement_rotation, wow_to_bevy};
use benilla_assets::{AdtTile, M2Model, WdtIndex, WmoModel};
use benilla_formats::{Doodad, WmoInstance};
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::render::render_resource::Face;

use crate::clutter::{scatter_tile_clutter, ClutterConfig, GroundClutter};
use crate::collision::{walk_layers, GroundDecalSurface, PickOccluder};
use crate::interior::WmoResidency;
use crate::lighting::SharedLightBuffer;
use crate::liquid::{spawn_liquids, LiquidAssets};
use crate::model_render::MaterialCache;
use crate::schedule::WorldStage;
use crate::view::WorldCamera;
use crate::vis_chain::VisChainOnly;
use crate::world_map::CurrentMap;
use benilla_assets::materials::{TerrainExtension, TerrainMaterial};
use benilla_assets::MapCatalogRes;
use benilla_assets::RenderConfig;
use benilla_assets::{m2_url, wmo_url};

mod collider;
mod furnish;
pub mod merge;
mod queries;
mod spawn;
mod weld;
pub(crate) mod window;

use collider::{finish_colliders, impassable_wall_data, terrain_collider_data};
use furnish::furnish_tile_cells;
use merge::{flush_static_merge, StaticMerge};
use spawn::prop_light::WmoDoodadInst;
use spawn::spawn_loaded_placements;
use weld::{flush_hull_welds, HullWelds};
pub use window::StreamWindow;
// The WMO prop-light items, for the interior classifier and the app's WMO props.
pub use spawn::prop_light::{fold_interior_probe, hex_word, interior_light_up, PropLobeLight};
// The placed-model assembler and off-thread collider build, shared with WMO gameobject props.
pub use collider::{build_collider_task, placement_collider_data, PendingCollider};
pub use spawn::{m2_anim_bound, m2_fade, point_light, spawn_model_entities, SpawnedModel};
// The position queries and the area authority.
use queries::update_current_area;
pub use queries::{
    area_id_under, doodad_ground_shade, ground_effect_under, terrain_height_under,
    terrain_height_under_cached, AreaAuthoritySet, CurrentArea, ShadeResolve,
};

/// Wall-clock per frame for spawning streamed tiles and placements before the rest waits a frame;
/// uncapped, a cold start spawns the whole ring at once and blocks the main thread for seconds.
const SPAWN_BUDGET: Duration = Duration::from_millis(4);

/// The streamer's residency state: the loaded tiles and the entity each spawned as.
#[derive(Resource, Default)]
pub struct TerrainStreamer {
    /// Loaded tiles by `(tile_x, tile_y)`; each handle keeps its asset resident.
    tiles: HashMap<(i32, i32), TileState>,
    /// The map directory the tiles are for; a change drops every tile.
    map_dir: Option<String>,
    /// The focus tile last streamed around, the furnisher's nearest-first key.
    focus: (i32, i32),
    /// The window's `(inner, outer)` half-widths last frame, logged once per change.
    reach: Option<(i32, i32)>,
    /// The map's WDT tile index: ADT requests wait for it and consult its `MAIN` grid.
    wdt: Option<Handle<WdtIndex>>,
    /// The WDT failed to load: stream every tile ungated rather than show no world.
    wdt_ungated: bool,
    /// This WMO-only map's one building is registered as a placement.
    global_wmo: bool,
    /// [`ViewFocus::paced`] last frame: the residency latch re-arms on the edge into a load, since
    /// a viewer with no avatar is unpaced forever and the level would re-arm it every frame.
    was_paced: bool,
    /// Last frame had nothing to request, drop or spawn and every tile stood furnished, so an
    /// unchanged frame skips the walk.
    settled: bool,
    /// The off-grid focus tile last warned about, so the warning fires once per tile.
    off_grid_reported: Option<(i32, i32)>,
}

/// The map-global WMO's placement id: a WMO-only map has no ADT uniqueIds to collide with, and
/// this is the file's own dead `uniqueId` (the reference overwrites it from a counter, `0xc9a320`).
const GLOBAL_WMO_UID: u32 = u32::MAX;

impl TerrainStreamer {
    /// `(spawned, requested)` tiles for the debug panel: furnished, and all in the window.
    pub fn residency(&self) -> (usize, usize) {
        let spawned = self.tiles.values().filter(|t| t.furnished).count();
        (spawned, self.tiles.len())
    }
}

/// One loaded tile and everything it owns.
struct TileState {
    handle: Handle<AdtTile>,
    /// The terrain root, spawned once the `AdtTile` loads, when its placements register too.
    entity: Option<Entity>,
    /// The tile's one terrain material, shared by every cell.
    material: Option<Handle<TerrainMaterial>>,
    /// The index of the next cell (4×4 chunks) to furnish; cells land a few per frame while live.
    next_cell: usize,
    /// Every cell is up: the residency bit the loading screen counts.
    furnished: bool,
    /// The uniqueIds of the placements this tile references, released when it unloads.
    placements: Vec<u32>,
    liquid: Vec<Entity>,
    /// The impassable-chunk wall collider, a body of its own since its audience differs.
    wall: Option<Entity>,
    /// The per-chunk `ClutterChunk`s, whose meshes are children and despawn with them.
    clutter: Vec<Entity>,
    /// The welded doodad hull colliders of the placements that registered here first.
    welds: Vec<Entity>,
    /// Those placements' merged static-render blobs, per cell and material (`WOW_STATIC_MERGE`).
    merged: Vec<Entity>,
}

/// Doodad and WMO placements, spawned once and refcounted across the tiles that list them.
#[derive(Resource, Default)]
pub(crate) struct Placements {
    /// By MDDF/MODF uniqueId.
    by_id: HashMap<u32, Placement>,
    /// The material dedup, so submeshes sharing a look share one handle and batch.
    materials: MaterialCache,
    /// Placements awaiting their model plus WMO props awaiting their M2s; at zero the spawner skips
    /// its [`Self::by_id`] walk. Every register, handoff, release and spawn site keeps it.
    pending_spawns: usize,
}

impl Placements {
    /// Every entity a registered placement owns; a placed part outside it is a doubled prop.
    pub(crate) fn owned(&self) -> std::collections::HashSet<Entity> {
        self.by_id
            .values()
            .flat_map(|p| p.entities.iter().copied())
            .collect()
    }
}

/// A shared placement: its model, transform and spawned entities.
struct Placement {
    model: ModelHandle,
    transform: Transform,
    /// Spawned submesh entities, WMO prop submeshes included so they despawn together.
    entities: Vec<Entity>,
    /// Spawned, or found to have nothing to spawn.
    spawned: bool,
    /// The MODF doodad set shown beside set 0 (WMOs only).
    doodad_set: u16,
    /// The MODF name set (WMOs only): the placement's `WMOAreaTable.NameSetID`.
    name_set: u16,
    /// WMO doodad props, resolved when the root loads and spawned as each M2 arrives.
    doodads: Vec<WmoDoodadInst>,
    /// The building's portal instance, held so later props cull with the group that owns them.
    portal_instance: Option<Entity>,
    /// How many loaded tiles reference this placement; despawned when it hits zero.
    refs: u32,
    /// The tile owning an M2 doodad's hull weld and merged blobs, first the registering tile;
    /// WMOs never read it.
    owner: (i32, i32),
}

/// How much of the world wanted around the view focus is there, read by the loading bar and by
/// the post-snap physics hold, which keys on this and never on ground contact.
#[derive(Resource, Default)]
pub struct WorldLoadProgress {
    /// Units done of [`Self::total`]: desired tiles furnished, focus-neighbourhood placements up.
    pub ready: usize,
    pub total: usize,
    /// The tile under the view focus is furnished; false at startup and whenever a teleport lands
    /// on unloaded ground, same map or not, which is the loading screen's backstop trigger.
    pub focus_resident: bool,
    /// The focus tile and its 8 neighbours are up with every placement they reference; consumers
    /// read [`Self::presentable`], which adds the backlogs.
    pub scene_ready: bool,
    /// Outstanding collider attaches, from `finish_colliders`.
    pub colliders_pending: usize,
    /// Unflushed static-merge accumulators (`WOW_STATIC_MERGE`), holes the reveal must not show.
    pub merge_pending: usize,
    /// Focus-neighbourhood placements not yet spawned.
    pub placements_pending: usize,
    /// Retained-pass regions spawned but not yet drawable (`StaticGx::undrawn_regions`).
    pub gx_pending: usize,
    /// The focus tile these facts are about. The settle release refuses on a mismatch with the
    /// avatar's own tile, so residency for one tile never unfreezes a body on another.
    pub focus_tile: Option<(i32, i32)>,
    /// Latched once the load reads fully ready, freezing the counts until a new load re-arms it;
    /// [`Self::focus_tile`] and [`Self::focus_resident`] stay live every frame regardless.
    pub complete: bool,
}

impl WorldLoadProgress {
    /// The bar's 0..1 fraction, 0 while nothing is wanted yet so the bar starts empty.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.ready as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    /// Every wanted unit is resident; `total == 0` is a frame before the window, never done.
    pub fn is_ready(&self) -> bool {
        self.total > 0 && self.ready >= self.total
    }

    /// The scene term with no collider, merge or retained-pass backlog: what the loading cover and
    /// the settle release key on.
    pub fn presentable(&self) -> bool {
        self.scene_ready
            && self.colliders_pending == 0
            && self.merge_pending == 0
            && self.gx_pending == 0
    }
}

/// Where the world streams from, written by whatever owns a viewer and read by every streaming
/// lane. The ladder: a live attached avatar, else the picked character's entry row, else the
/// camera, else [`SPAWN_XY`]. A detached eye streams from the camera but keeps the body, which the
/// zone authority follows.
#[derive(Resource, Default, Clone, Copy)]
pub struct ViewFocus {
    /// The avatar's position in wow coords when one is live, even while detached.
    body: Option<[f32; 3]>,
    /// The eye follows the body; false in free-fly.
    attached: bool,
    /// The picked character's map and position, before an avatar exists.
    entry: Option<(u32, [f32; 3])>,
    /// The focus is settled, so spawning is paced; false through entry, a teleport and a world
    /// swap, while the loading cover absorbs the burst.
    pub(crate) paced: bool,
}

impl ViewFocus {
    /// No viewer: follow the camera, or the spawn point without one.
    pub fn camera() -> Self {
        Self::default()
    }

    /// A live avatar at `wow` with the eye on it; `paced` is the settled bit.
    pub fn body(wow: [f32; 3], paced: bool) -> Self {
        Self {
            body: Some(wow),
            attached: true,
            entry: None,
            paced,
        }
    }

    /// The stream follows the body's own ground, so residency is a fact about the player; a
    /// detached eye streams wherever the camera went.
    pub fn follows_body(&self) -> bool {
        self.attached && self.body.is_some()
    }

    /// A live avatar with the eye detached: the stream follows the camera, the zone the body.
    pub fn detached(wow: [f32; 3], paced: bool) -> Self {
        Self {
            attached: false,
            ..Self::body(wow, paced)
        }
    }

    /// No avatar yet: stream the picked character's map and position, not the parked camera's.
    pub fn entry(map: u32, wow: [f32; 3]) -> Self {
        Self {
            entry: Some((map, wow)),
            ..Self::default()
        }
    }

    /// The point to stream around, wow coords.
    pub(crate) fn resolve(&self, camera: Option<Vec3>) -> [f32; 3] {
        if let (Some(pos), true) = (self.body, self.attached) {
            return pos;
        }
        if self.body.is_none() {
            if let Some((_, pos)) = self.entry {
                return pos;
            }
        }
        match camera {
            Some(t) => bevy_to_wow(t),
            None => [SPAWN_XY.0, SPAWN_XY.1, 0.0],
        }
    }

    /// The map to stream: the picked character's before an avatar exists, else `current`. It must
    /// agree with [`Self::resolve`], or a position names tiles on another map's grid.
    pub(crate) fn map(&self, current: Option<u32>) -> u32 {
        if self.body.is_none() {
            if let Some((map, _)) = self.entry {
                return map;
            }
        }
        current.unwrap_or(0)
    }

    /// The body's position, which the area authority measures from, never the camera's.
    pub(crate) fn body_pos(&self) -> Option<[f32; 3]> {
        self.body
    }

    /// A body stands in a settled world: the area authority's gate. Through entry, a teleport and
    /// a world swap the area under the body is a stale tile's, and publishing it would announce,
    /// play and join the channels of zones the player was never in.
    pub(crate) fn body_settled(&self) -> bool {
        self.body.is_some() && self.paced
    }
}

/// The stream focus when nothing else can say: the Human start in Northshire.
pub const SPAWN_XY: (f32, f32) = (-8949.95, -132.49);

/// A placement's model handle, which keeps the asset resident.
enum ModelHandle {
    M2(Handle<M2Model>),
    Wmo(Handle<WmoModel>),
}

/// What the streamer did this frame, bumped by the `WorldStage::Stream` chain and zeroed by its
/// reader, so a frame spike can be read against what was dropped, requested and spawned.
#[derive(Resource, Default)]
pub struct StreamActivity {
    /// Tiles whose owned entities were despawned.
    pub tiles_dropped: u32,
    /// Shared placements whose refcount hit zero.
    pub placements_dropped: u32,
    /// Entities despawned with those placements.
    pub placement_entities_dropped: u32,
    pub tiles_requested: u32,
    pub tiles_spawned: u32,
    /// Terrain cells (4×4 MCNK chunks each) spawned by the furnisher.
    pub cells_spawned: u32,
    /// Model submesh forms built by the model furnisher.
    pub model_meshes_built: u32,
    /// Placements (or WMO props) whose model landed and spawned.
    pub placements_spawned: u32,
    pub colliders_attached: u32,
    /// Self-times in ms, in order: `stream_terrain`, `furnish_tile_cells`, `furnish_model_forms`,
    /// `spawn_loaded_placements`, `finish_colliders`.
    pub stream_ms: f32,
    pub furnish_ms: f32,
    pub mfurnish_ms: f32,
    pub spawn_ms: f32,
    pub collider_ms: f32,
}

impl StreamActivity {
    /// The streamer did something this frame; the self-timers, which run every frame, do not count.
    pub fn any_event(&self) -> bool {
        self.tiles_dropped
            + self.placements_dropped
            + self.tiles_requested
            + self.tiles_spawned
            + self.cells_spawned
            + self.model_meshes_built
            + self.placements_spawned
            + self.colliders_attached
            > 0
    }
}

pub(crate) struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        // Defaulted here, so a program with no game boots and streams around the camera.
        app.init_resource::<ViewFocus>()
            .init_resource::<WorldLoadProgress>()
            .init_resource::<TerrainStreamer>()
            .init_resource::<StreamActivity>()
            .init_resource::<Placements>()
            .init_resource::<HullWelds>()
            .init_resource::<StaticMerge>()
            .init_resource::<crate::model_forms::ModelForms>()
            .init_resource::<CurrentArea>()
            // In `WorldStage::Stream`, between the teleport snap (Input) and the loading cover
            // (Present), so a swap never renders uncovered. `finish_colliders` heads the chain so
            // the collider queue read downstream is this frame's.
            .add_systems(
                Update,
                (
                    finish_colliders,
                    stream_terrain,
                    furnish_tile_cells,
                    spawn_loaded_placements,
                    flush_hull_welds,
                    flush_static_merge,
                    sync_interior_volumes,
                )
                    .chain()
                    .in_set(WorldStage::Stream)
                    // No world, and no chain, until a character is in one.
                    .run_if(crate::schedule::world_is_live),
            )
            // `WOW_BLOB_VIS=1`: the blob-visibility dump.
            .add_systems(
                Update,
                merge::log_blob_vis
                    .run_if(merge::blob_vis_enabled)
                    .run_if(crate::schedule::world_is_live),
            )
            // Ungated: the glue screens' character preview requests forms before any world. Before
            // the spawner, so a form finished this frame lands this frame.
            .add_systems(
                Update,
                crate::model_forms::furnish_model_forms
                    .in_set(WorldStage::Stream)
                    .before(spawn_loaded_placements),
            )
            // Leaving the world releases it; the gate above only stops it being maintained.
            .add_systems(
                Update,
                release_world
                    .run_if(crate::schedule::world_left)
                    // On the frame the world went away: the session publishes its edge before
                    // `Net`, and the release lands before the stream stage.
                    .after(crate::schedule::WorldStage::Net)
                    .before(crate::schedule::WorldStage::Stream),
            )
            // Ungated, so the last world's art drains at character select too.
            .add_systems(Update, scope_placement_art)
            // Ungated, so the density tracker never misses a boot-time config apply.
            .add_systems(Update, rescatter_clutter)
            .add_systems(
                Update,
                // After the interior claim: the reference resolves leaf, indoor and names from one
                // node state in one pass (`0x67e510`).
                update_current_area
                    .after(crate::wmo_portal::WmoPvsSet)
                    .in_set(AreaAuthoritySet),
            );
    }
}

/// Stream `AdtTile`s around the view focus: drop tiles out of range, request the tiles in range
/// that the WDT `MAIN` grid authors, spawn each as it loads, and publish residency.
#[allow(clippy::type_complexity)] // the bundled asset_stores tuple
fn stream_terrain(
    mut commands: Commands,
    mut state: ResMut<TerrainStreamer>,
    placements: ResMut<Placements>,
    asset_server: Res<AssetServer>,
    tiles: Res<Assets<AdtTile>>,
    // One tuple, under Bevy's 16-param system limit.
    asset_stores: (
        ResMut<Assets<TerrainMaterial>>,
        ResMut<Assets<Mesh>>,
        Res<Assets<WdtIndex>>,
        Res<Time>,
        ResMut<StreamActivity>,
    ),
    liquid_assets: Option<Res<LiquidAssets>>,
    clutter: Option<Res<GroundClutter>>,
    clutter_cfg: Option<Res<ClutterConfig>>,
    focus: Res<ViewFocus>,
    camera: Query<&Transform, With<WorldCamera>>,
    shared_light: Option<Res<SharedLightBuffer>>,
    cfg: Option<Res<RenderConfig>>,
    // The server's map, the catalog naming its directory, and the view distance the window uses.
    location: (
        Option<Res<CurrentMap>>,
        Option<Res<MapCatalogRes>>,
        Res<crate::view::ViewDistance>,
    ),
    mut load_progress: Option<ResMut<WorldLoadProgress>>,
    // The stream-batch accumulators the map-change drop must clear.
    batchers: (
        ResMut<HullWelds>,
        ResMut<StaticMerge>,
        // `None` only under `WOW_STATIC_GX=0`.
        Option<ResMut<crate::static_gx::StaticGx>>,
    ),
) {
    let (mut welds, mut static_merge, mut staticgx) = batchers;
    let (mut materials, mut meshes, wdts, _time, mut activity) = asset_stores;
    let (current_map, map_catalog, view) = location;
    // Idle until other plugins' startup has made the light buffer and the map catalog.
    let (Some(shared_light), Some(map_catalog)) = (shared_light, map_catalog) else {
        return;
    };
    let t0 = Instant::now();
    let placements = placements.into_inner();
    let map_id = focus.map(current_map.map(|m| m.0));
    let Some(dir) = map_catalog.0.directory(map_id).map(str::to_string) else {
        return;
    };

    // A new map directory: drop every tile and request the new map's WDT.
    if state.map_dir.as_deref() != Some(dir.as_str()) {
        info!(
            "terrain: streaming map {dir} (was {:?}, {} tiles released)",
            state.map_dir,
            state.tiles.len()
        );
        drop_streamed_world(
            &mut commands,
            &mut state,
            placements,
            &mut welds,
            &mut static_merge,
            staticgx.as_deref_mut(),
            &mut activity,
        );
        state.map_dir = Some(dir.clone());
        state.wdt = Some(asset_server.load(format!("mpq://World/Maps/{dir}/{dir}.wdt")));
        state.wdt_ungated = false;
    }
    // A failed WDT falls back to ungated probing, warned once per map, never to no world.
    let wdt_index = state.wdt.as_ref().and_then(|h| wdts.get(h));
    if wdt_index.is_none() && !state.wdt_ungated {
        if let Some(h) = &state.wdt {
            if matches!(
                asset_server.load_state(h),
                bevy::asset::LoadState::Failed(_)
            ) {
                warn!("terrain: no WDT for map {dir} — streaming ungated (every tile probed)");
                state.wdt_ungated = true;
            }
        }
    }

    // A WMO-only map is one building, registered like any ADT WMO, as the reference's WDT branch
    // hands its global MODF entry to the MCRF walk's consumer (`0x695650`). Registered once per
    // map, released on map change.
    if let Some(g) = wdt_index.and_then(|w| w.global_wmo()) {
        if !state.global_wmo {
            info!(
                "terrain: map {dir} has no tiles — its world is one WMO ({})",
                g.model
            );
            register_wmo(
                placements,
                &asset_server,
                &WmoInstance {
                    model: g.model.clone(),
                    position: g.position,
                    rotation: g.rotation,
                    unique_id: GLOBAL_WMO_UID,
                    doodad_set: g.doodad_set,
                    name_set: g.name_set,
                },
            );
            state.global_wmo = true;
        }
    }

    // The residency window is the reference's, from the live `farclip` in chunk units.
    let center = focus.resolve(camera.single().ok().map(|c| c.translation));
    let window = StreamWindow::at(view.farclip, center[0], center[1]);
    let (cx, cy) = window.focus_tile();
    let same_window = state.focus == (cx, cy) && state.reach == Some((window.inner, window.outer));
    // Skip an unchanged frame only in a paced, complete, presentable world, since the tail it
    // bypasses releases a load; even then the undrawn count stays fresh, so a rebake un-skips it.
    if same_window
        && state.settled
        && focus.paced
        && state.was_paced == focus.paced
        && (wdt_index.is_some() || state.wdt_ungated)
        && load_progress
            .as_ref()
            .is_none_or(|p| p.complete && p.presentable())
    {
        if let (Some(p), Some(gx)) = (load_progress.as_mut(), staticgx.as_deref()) {
            p.gx_pending = crate::static_gx::StaticGx::undrawn_regions(gx);
        }
        return;
    }
    state.focus = (cx, cy);
    if state.reach != Some((window.inner, window.outer)) {
        state.reach = Some((window.inner, window.outer));
        info!(
            "terrain: residency window ±{} chunks ({:.0} yd), release past ±{} — farclip {:.0}",
            window.outer,
            window.outer as f32 * benilla_formats::CHUNK_SIZE,
            window.outer + window::KEEP_BAND_CHUNKS,
            view.farclip
        );
    }

    // Every tile the outer window touches on the 64×64 grid, less those the WDT's `MAIN` bit 0
    // says do not exist, as the reference tests (`0x69863c`).
    let mut desired = window.wanted_tiles();
    if let Some(w) = wdt_index {
        desired.retain(|&(tx, ty)| w.has_tile(tx as u32, ty as u32));
    }

    // A focus off the map's tile grid, usually one written in another map's coordinates, wants no
    // tile, so `total == 0` and the loading cover can never clear. Warned once per focus tile;
    // WMO-only maps author no tiles and are exempt.
    if wdt_index.is_some() && !state.global_wmo && desired.is_empty() {
        let focus_tile = window.focus_tile();
        if state.off_grid_reported != Some(focus_tile) {
            state.off_grid_reported = Some(focus_tile);
            let map_name = state.map_dir.clone().unwrap_or_default();
            warn!(
                "terrain: view focus [{:.1}, {:.1}] is OFF map {map_name}'s tile grid (tile \
                 {focus_tile:?}) — the window wants no tile the WDT authors, so nothing streams \
                 and a loading cover cannot clear. A focus written in another map's coordinates \
                 is the usual cause.",
                center[0], center[1],
            );
        }
    } else if !desired.is_empty() {
        state.off_grid_reported = None;
    }

    // Unload stale tiles, at most `unload_budget` per frame and farthest first, since a whole
    // row's free wave is a frame spike; a cross-map swap is not budgeted, as the cover hides it.
    // Deviation: a tile is released only one chunk past the outer window (`StreamWindow::keeps`),
    // where the reference releases by window membership (`0x6984f0`), because our reload is a
    // re-decode and re-spawn, not a cheap async read.
    let unload_budget = cfg.as_ref().map(|c| c.unload_budget).unwrap_or(1);
    let mut stale: Vec<(i32, i32)> = if tile_drop_disabled() {
        Vec::new()
    } else {
        state
            .tiles
            .keys()
            .copied()
            .filter(|&tile| !window.keeps(tile))
            .collect()
    };
    let stale_empty = stale.is_empty();
    if unload_budget > 0 && stale.len() > unload_budget {
        stale.sort_by_key(|&(tx, ty)| std::cmp::Reverse((tx - cx).abs().max((ty - cy).abs())));
        stale.truncate(unload_budget);
    }
    for c in stale {
        if let Some(t) = state.tiles.remove(&c) {
            despawn_tile_owned(&mut commands, &t);
            activity.tiles_dropped += 1;
            for &uid in &t.placements {
                release_placement(&mut commands, placements, uid, &mut activity);
            }
            handoff_straddlers(
                &mut commands,
                &state.tiles,
                placements,
                c,
                &t.placements,
                merge::merge_enabled(),
            );
            // A retained-pass batch has no entity or blob, so its owner's items leave here or
            // never; the cells re-bake without them.
            if let Some(gx) = staticgx.as_deref_mut() {
                gx.release_owner(c);
            }
        }
    }

    // Request new tiles once the WDT has answered. While paced, one per frame and nearest first,
    // so a fresh row's landings stagger; entry and teleports request all at once under the cover.
    let mut fresh_empty = true;
    if wdt_index.is_some() || state.wdt_ungated {
        let live = focus.paced;
        let mut fresh: Vec<(i32, i32)> = desired
            .iter()
            .copied()
            .filter(|c| !state.tiles.contains_key(c))
            .collect();
        fresh_empty = fresh.is_empty();
        if live && fresh.len() > 1 {
            fresh.sort_by_key(|&(tx, ty)| (tx - cx).abs().max((ty - cy).abs()));
            fresh.truncate(1);
        }
        for (tx, ty) in fresh {
            activity.tiles_requested += 1;
            state.tiles.insert(
                (tx, ty),
                TileState {
                    handle: asset_server
                        .load(format!("mpq://World/Maps/{dir}/{dir}_{tx}_{ty}.adt")),
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
                },
            );
        }
    }

    // Spawn loaded tiles within the frame's budget. The material binds the shared light buffer,
    // so a new tile is lit and fogged on its first frame.
    let tile_deadline = Instant::now() + SPAWN_BUDGET;
    for (&(tx, ty), tile) in state.tiles.iter_mut() {
        if tile.entity.is_some() {
            continue;
        }
        let Some(adt) = tiles.get(&tile.handle) else {
            continue; // not loaded yet, or missing
        };
        let material = materials.add(ExtendedMaterial {
            base: terrain_base_material(),
            extension: TerrainExtension {
                layer_array: adt.layer_array.clone(),
                alpha_array: adt.alpha_array.clone(),
                shadow_array: adt.shadow_array.clone(),
                params: Vec4::new(benilla_formats::TERRAIN_LAYER_TILES, 0.0, 0.0, 0.0),
                light_buf: shared_light.0.clone(),
            },
        });
        // One static trimesh per tile from the drawn chunks, built off-thread, riding the root.
        let collider_data = terrain_collider_data(&adt.chunks);
        // The impassable-chunk fences are a second collider, on the walk layer only: the reference
        // emits them into the movement box gather alone, never its segment and ray path.
        let wall_data = impassable_wall_data(&adt.chunks);
        // The root draws nothing: it carries the collider and surface roles, and its cells of
        // 4×4 chunks are children, a unit small enough for the exterior and frustum culls.
        // `furnish_tile_cells` spawns them a few per frame; the root keeps the visibility chain.
        let mut tile_ent = commands.spawn((Transform::IDENTITY, Visibility::default()));
        tile_ent.vis_chain_only();
        if let Some((verts, tris)) = collider_data {
            // Terrain takes the selection ring and clamps the pick (the reference's world trace).
            tile_ent.insert((
                PendingCollider::new(build_collider_task(verts, tris), None, true),
                GroundDecalSurface,
                PickOccluder,
            ));
        }
        tile.entity = Some(tile_ent.id());
        tile.wall = wall_data.map(|(verts, tris)| {
            commands
                .spawn(PendingCollider::new(
                    build_collider_task(verts, tris),
                    Some(walk_layers()),
                    true,
                ))
                .id()
        });
        tile.material = Some(material);
        activity.tiles_spawned += 1;

        // Placements register by uniqueId; their ground shade resolves at spawn by a global lookup,
        // since a doodad's origin may lie on another tile.
        for d in &adt.doodads {
            register_doodad(placements, &asset_server, d, (tx, ty));
            tile.placements.push(d.unique_id);
        }
        for w in &adt.wmos {
            register_wmo(placements, &asset_server, w);
            tile.placements.push(w.unique_id);
        }

        let mut liquid_ents = Vec::new();
        spawn_liquids(
            &mut commands,
            adt.chunks.iter().flat_map(|c| c.liquids.iter()),
            liquid_assets.as_deref(),
            &mut meshes,
            &mut liquid_ents,
        );
        tile.liquid = liquid_ents;

        // Ground clutter per chunk, meshed lazily within ~70 yd by `stream_chunk_clutter`.
        if let (Some(clutter), Some(clutter_cfg)) = (clutter.as_ref(), clutter_cfg.as_ref()) {
            let mut clutter_ents = Vec::new();
            scatter_tile_clutter(
                &mut commands,
                &adt.chunks,
                tx as u32,
                ty as u32,
                &clutter.catalog,
                clutter_cfg.density,
                &mut clutter_ents,
            );
            tile.clutter = clutter_ents;
        }

        // Past the budget the rest waits a frame; the `entity` guard above makes this re-entrant.
        if Instant::now() >= tile_deadline {
            break;
        }
    }

    // Publish residency; the pacing edge is read and advanced once per frame.
    state.settled = stale_empty
        && fresh_empty
        && state
            .tiles
            .values()
            .all(|t| t.entity.is_some() && t.furnished);
    let was_paced = std::mem::replace(&mut state.was_paced, focus.paced);
    if let Some(p) = load_progress.as_mut() {
        p.focus_tile = Some((cx, cy));
        // Judged from the desired set, so a desired tile not yet requested is not there; ground
        // the WDT never authors counts as resident, or the wait would never end.
        p.focus_resident = if desired.contains(&(cx, cy)) {
            state.tiles.get(&(cx, cy)).is_some_and(|t| t.furnished)
        } else {
            true
        };
        // Nothing is resident before the WDT answers, or a WMO-only map would show the void.
        if wdt_index.is_none() && !state.wdt_ungated {
            p.focus_resident = false;
        }
        // A WMO-only map's focus residency is its one building's spawn.
        if state.global_wmo {
            p.focus_resident &= placements
                .by_id
                .get(&GLOBAL_WMO_UID)
                .is_some_and(|p| p.spawned);
        }
        // The counting below latches off once fully ready (`complete`); a focus on unfurnished
        // ground is a new load and re-arms it.
        if !p.focus_resident {
            p.complete = false;
        }
        // So does a snap onto furnished ground, whose buildings may still be arriving; on the edge
        // into the load, not the level.
        if was_paced && !focus.paced {
            p.complete = false;
        }
        if !p.complete {
            // Spawned means furnished, root and every cell, so a reveal never shows bare ground;
            // it lags the root by a frame, as the furnisher runs after this system.
            let spawned = |c: &(i32, i32)| state.tiles.get(c).is_some_and(|t| t.furnished);
            p.total = desired.len();
            p.ready = desired.iter().filter(|c| spawned(c)).count();
            // The focus neighbourhood's placements count too, each up once its model and its WMO
            // props have spawned; a tile not yet up holds `scene_ready` down itself.
            let mut near_pending = 0usize;
            let mut near_tile_missing = false;
            for dx in -1..=1i32 {
                for dy in -1..=1i32 {
                    let c = (cx + dx, cy + dy);
                    if !desired.contains(&c) {
                        continue; // no tile to wait for
                    }
                    match state.tiles.get(&c) {
                        Some(t) if t.furnished => {
                            for uid in &t.placements {
                                if let Some(pl) = placements.by_id.get(uid) {
                                    let up = pl.spawned && pl.doodads.iter().all(|d| d.spawned);
                                    p.total += 1;
                                    p.ready += usize::from(up);
                                    near_pending += usize::from(!up);
                                }
                            }
                        }
                        _ => near_tile_missing = true,
                    }
                }
            }
            p.placements_pending = near_pending;
            // `focus_resident` already folds in the WDT gate and the global WMO's spawn.
            p.scene_ready = p.focus_resident && !near_tile_missing && near_pending == 0;
            // A WMO-only map counts its one placement, props included, which keeps `total > 0`.
            if state.global_wmo {
                let full = placements
                    .by_id
                    .get(&GLOBAL_WMO_UID)
                    .is_some_and(|p| p.spawned && p.doodads.iter().all(|d| d.spawned));
                p.total += 1;
                p.ready += usize::from(full);
                p.scene_ready &= full;
            }
            p.complete = p.scene_ready && p.is_ready();
        }
        // Spawned geometry that cannot draw yet is a hole in the reveal; published every frame.
        p.gx_pending = staticgx
            .as_deref()
            .map_or(0, crate::static_gx::StaticGx::undrawn_regions);
        // During a load, once nothing else is outstanding, flush the pass rather than wait out
        // its quiet window.
        if !focus.paced
            && p.gx_pending > 0
            && p.scene_ready
            && p.colliders_pending == 0
            && p.merge_pending == 0
        {
            if let Some(gx) = staticgx.as_deref_mut() {
                gx.flush_now();
            }
        }
    }
    activity.stream_ms += t0.elapsed().as_secs_f32() * 1000.0;
}

/// `WOW_NO_TILE_DROP=1`: never release stale tiles, so a crossing only adds (dev-only).
fn tile_drop_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_TILE_DROP").is_some())
}

/// Register one M2 doodad placement, or bump its refcount if already known.
fn register_doodad(
    placements: &mut Placements,
    asset_server: &AssetServer,
    d: &Doodad,
    tile: (i32, i32),
) {
    if let Some(p) = placements.by_id.get_mut(&d.unique_id) {
        p.refs += 1;
        return;
    }
    let handle: Handle<M2Model> = asset_server.load(m2_url(&d.model));
    placements.by_id.insert(
        d.unique_id,
        Placement {
            model: ModelHandle::M2(handle),
            transform: Transform {
                translation: wow_to_bevy(d.position),
                rotation: placement_rotation(d.rotation),
                scale: Vec3::splat(d.scale),
            },
            entities: Vec::new(),
            spawned: false,
            doodad_set: 0, // M2 doodads carry no doodad set
            name_set: 0,
            doodads: Vec::new(),
            portal_instance: None,
            refs: 1,
            owner: tile,
        },
    );
    placements.pending_spawns += 1;
}

/// Register one WMO placement, or bump its refcount; a 1.12 WMO placement has no scale.
fn register_wmo(placements: &mut Placements, asset_server: &AssetServer, w: &WmoInstance) {
    if let Some(p) = placements.by_id.get_mut(&w.unique_id) {
        p.refs += 1;
        return;
    }
    let handle: Handle<WmoModel> = asset_server.load(wmo_url(&w.model));
    placements.by_id.insert(
        w.unique_id,
        Placement {
            model: ModelHandle::Wmo(handle),
            transform: Transform {
                translation: wow_to_bevy(w.position),
                rotation: placement_rotation(w.rotation),
                scale: Vec3::ONE,
            },
            entities: Vec::new(),
            spawned: false,
            doodad_set: w.doodad_set,
            name_set: w.name_set,
            doodads: Vec::new(),
            portal_instance: None,
            refs: 1,
            owner: (0, 0), // unread for WMOs: their prop hulls weld per placement
        },
    );
    placements.pending_spawns += 1;
}

/// Drop every streamed tile and the placements they reference, on a cross-map teleport or on
/// leaving the world; the material dedup goes too, or its handles would keep the old map's art.
fn drop_streamed_world(
    commands: &mut Commands,
    state: &mut TerrainStreamer,
    placements: &mut Placements,
    welds: &mut HullWelds,
    merge: &mut StaticMerge,
    staticgx: Option<&mut crate::static_gx::StaticGx>,
    activity: &mut StreamActivity,
) {
    for ((_tx, _ty), t) in state.tiles.drain() {
        despawn_tile_owned(commands, &t);
        activity.tiles_dropped += 1;
        for uid in t.placements {
            release_placement(commands, placements, uid, activity);
        }
    }
    if std::mem::take(&mut state.global_wmo) {
        release_placement(commands, placements, GLOBAL_WMO_UID, activity);
    }
    placements.materials.clear();
    // Cleared here, not by the flush's dead-key discard: tile keys repeat across maps, and the
    // new map's tiles are inserted this same frame. The same holds for the merge accumulators,
    // the census tallies and the retained cells.
    welds.clear();
    merge.clear();
    if let Some(gx) = staticgx {
        gx.clear();
    }
    crate::static_merge::reset();
}

/// Expire the placement material dedup by distance, the within-map half of
/// [`drop_streamed_world`]'s clear.
fn scope_placement_art(mut scope: crate::art_scope::ArtScope, mut placements: ResMut<Placements>) {
    scope.apply(
        &mut placements.materials,
        crate::art_scope::ArtSlot::PlaceMats,
    );
}

/// Release the world on leaving it; `map_dir` and `wdt` clear too, so re-entering the same map
/// streams afresh.
fn release_world(
    mut commands: Commands,
    mut state: ResMut<TerrainStreamer>,
    mut placements: ResMut<Placements>,
    mut forms: ResMut<crate::model_forms::ModelForms>,
    mut progress: Option<ResMut<WorldLoadProgress>>,
    mut activity: ResMut<StreamActivity>,
    // The stream-batch accumulators the drop must clear, bundled under clippy's argument count.
    batchers: (
        ResMut<HullWelds>,
        ResMut<StaticMerge>,
        Option<ResMut<crate::static_gx::StaticGx>>,
    ),
) {
    let (mut welds, mut static_merge, mut staticgx) = batchers;
    if state.tiles.is_empty() && state.map_dir.is_none() {
        return;
    }
    info!(
        "terrain: leaving the world — releasing {} tiles",
        state.tiles.len()
    );
    // Drop the whole model-form cache, or an entry another holder keeps would pin its meshes.
    forms.clear();
    drop_streamed_world(
        &mut commands,
        &mut state,
        &mut placements,
        &mut welds,
        &mut static_merge,
        staticgx.as_deref_mut(),
        &mut activity,
    );
    state.map_dir = None;
    state.wdt = None;
    state.wdt_ungated = false;
    // Reset residency, or the next entry could clear its cover on this world's counts.
    if let Some(p) = progress.as_mut() {
        **p = WorldLoadProgress::default();
    }
}

fn despawn_tile_owned(commands: &mut Commands, t: &TileState) {
    for e in t.entity.into_iter().chain(t.wall) {
        commands.entity(e).try_despawn();
    }
    for &e in t
        .liquid
        .iter()
        .chain(&t.clutter)
        .chain(&t.welds)
        .chain(&t.merged)
    {
        commands.entity(e).try_despawn();
    }
}

/// Re-scatter the loaded tiles when `WorldDetail` changes, as the reference's setter tail-calls
/// the chunk rebuild (`0x6725a0` → `0x6b1d20`). It watches the density value, since
/// `ClutterConfig` also holds the detail-doodad cutout; the first sight only arms.
fn rescatter_clutter(
    mut commands: Commands,
    mut streamer: ResMut<TerrainStreamer>,
    adts: Res<Assets<AdtTile>>,
    clutter: Option<Res<GroundClutter>>,
    cfg: Res<ClutterConfig>,
    mut last: Local<Option<f32>>,
) {
    let prev = last.replace(cfg.density);
    let (Some(prev), Some(clutter)) = (prev, clutter) else {
        return;
    };
    if prev == cfg.density {
        return;
    }
    let mut n = 0;
    for (&(tx, ty), tile) in streamer.tiles.iter_mut() {
        if tile.entity.is_none() {
            continue;
        }
        let Some(adt) = adts.get(&tile.handle) else {
            continue;
        };
        for e in tile.clutter.drain(..) {
            commands.entity(e).try_despawn();
        }
        scatter_tile_clutter(
            &mut commands,
            &adt.chunks,
            tx as u32,
            ty as u32,
            &clutter.catalog,
            cfg.density,
            &mut tile.clutter,
        );
        n += 1;
    }
    if n > 0 {
        info!(
            "clutter: world detail x{} — re-scattered {n} tiles",
            cfg.density
        );
    }
}

/// Mirror the spawned WMO placements into [`WmoResidency`] each frame, so the interior classifier
/// re-tests when a building streams in or out; rebuilt whole, as few WMOs are resident.
fn sync_interior_volumes(placements: Res<Placements>, mut vols: ResMut<WmoResidency>) {
    vols.update(
        placements
            .by_id
            .values()
            .filter_map(|p| match (&p.model, p.spawned) {
                (ModelHandle::Wmo(h), true) => Some(h.id()),
                _ => None,
            }),
    );
}

/// Hand the dead tile's surviving M2 placements to a still-loaded referrer and queue a respawn:
/// their merged batches and welded hulls live with the owner tile and died with it. Only with the
/// merge on; a WMO's prop blobs ride its own entity list.
fn handoff_straddlers(
    commands: &mut Commands,
    tiles: &HashMap<(i32, i32), TileState>,
    placements: &mut Placements,
    dead: (i32, i32),
    uids: &[u32],
    merge_on: bool,
) {
    if !merge_on {
        return;
    }
    // The dead tile's M2 placements with refs left, then one pass over each neighbour's list.
    let mut straddlers: HashMap<u32, Option<(i32, i32)>> = uids
        .iter()
        .copied()
        .filter(|uid| {
            placements
                .by_id
                .get(uid)
                .is_some_and(|p| p.owner == dead && matches!(p.model, ModelHandle::M2(_)))
        })
        .map(|uid| (uid, None))
        .collect();
    if straddlers.is_empty() {
        return; // every one released outright
    }
    // Another referrer shares the MDDF row across a tile seam, so it is an 8-neighbour; the full
    // scan is the fallback.
    let neighbours = [-1i32, 0, 1]
        .iter()
        .flat_map(|dx| [-1i32, 0, 1].map(|dy| (dead.0 + dx, dead.1 + dy)))
        .filter(|c| *c != dead);
    for c in neighbours {
        let Some(t) = tiles.get(&c) else {
            continue;
        };
        for uid in &t.placements {
            if let Some(slot) = straddlers.get_mut(uid) {
                if slot.is_none() {
                    *slot = Some(c);
                }
            }
        }
    }
    if straddlers.values().any(Option::is_none) {
        for (c, t) in tiles {
            if *c == dead {
                continue;
            }
            for uid in &t.placements {
                if let Some(slot) = straddlers.get_mut(uid) {
                    if slot.is_none() {
                        *slot = Some(*c);
                    }
                }
            }
        }
    }
    let mut handed = 0u32;
    for (uid, new_owner) in straddlers {
        let Some(new_owner) = new_owner else {
            warn!("straddler handoff: uid {uid} holds refs but no loaded tile references it");
            continue;
        };
        let Some(p) = placements.by_id.get_mut(&uid) else {
            continue;
        };
        p.owner = new_owner;
        if p.spawned {
            for e in p.entities.drain(..) {
                commands.entity(e).try_despawn();
            }
            p.spawned = false;
            placements.pending_spawns += 1;
        }
        handed += 1;
    }
    if handed > 0 {
        debug!(
            "straddler handoff: {handed} placement(s) re-owned off dead tile ({},{})",
            dead.0, dead.1
        );
    }
}

/// Drop one ref of a placement, despawning its entities, collider included, at zero.
fn release_placement(
    commands: &mut Commands,
    placements: &mut Placements,
    uid: u32,
    activity: &mut StreamActivity,
) {
    let drop_it = match placements.by_id.get_mut(&uid) {
        Some(p) => {
            p.refs -= 1;
            p.refs == 0
        }
        None => false,
    };
    if drop_it {
        if let Some(p) = placements.by_id.remove(&uid) {
            // Its unspawned model and props leave the pending count with it.
            placements.pending_spawns -=
                usize::from(!p.spawned) + p.doodads.iter().filter(|d| !d.spawned).count();
            activity.placements_dropped += 1;
            activity.placement_entities_dropped += p.entities.len() as u32;
            for e in p.entities {
                commands.entity(e).try_despawn();
            }
        }
    }
}

/// The terrain base material, backface-culled so the ground is see-through from below, as in the
/// reference: its terrain pass (`0x684510`/`0x6beb50`) never sets `EGxRs 0x14` and keeps the
/// device baseline cull on (`0x593bf0`); only the liquid passes and the WDL mesh clear it
/// (`0x6bd780`, at `0x6bd79d`). Front faces point up only while the winding tests
/// `terrain_fans_wind_ccw_seen_from_above` and `transform_is_a_proper_rotation` hold.
fn terrain_base_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        double_sided: false,
        cull_mode: Some(Face::Back),
        ..default()
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    /// Stratholme's map and position.
    const PICK: (u32, [f32; 3]) = (329, [3398.9, -3381.8, 142.7]);
    fn cam_at_northshire() -> Option<Vec3> {
        Some(wow_to_bevy([SPAWN_XY.0, SPAWN_XY.1, 100.0]))
    }

    #[test]
    fn the_pick_outranks_the_camera_until_the_avatar_exists() {
        let idle = ViewFocus::entry(PICK.0, PICK.1);
        assert_eq!(
            idle.map(Some(0)),
            329,
            "the map must be the picked character's, not the boot default"
        );
        assert_eq!(idle.resolve(cam_at_northshire()), PICK.1);
    }

    #[test]
    fn the_avatar_outranks_the_pick_once_it_is_in_the_world() {
        // The entry row survives world entry, so it must never pull the focus off an avatar.
        let walking = ViewFocus::body([100.0, 200.0, 30.0], true);
        assert_eq!(walking.map(Some(1)), 1);
        let f = walking.resolve(cam_at_northshire());
        assert!(
            (f[0] - 100.0).abs() < 0.01 && (f[1] - 200.0).abs() < 0.01,
            "{f:?}"
        );
    }

    #[test]
    fn free_flight_follows_the_camera_and_a_server_less_run_still_has_one() {
        let flying = ViewFocus::detached([100.0, 200.0, 30.0], true);
        let f = flying.resolve(cam_at_northshire());
        assert!((f[0] - SPAWN_XY.0).abs() < 0.01, "{f:?}");
        // The body is still published: the zone authority follows the character.
        assert_eq!(flying.body_pos(), Some([100.0, 200.0, 30.0]));
        // No avatar: the camera, else the spawn point, never the origin.
        assert!((ViewFocus::camera().resolve(cam_at_northshire())[0] - SPAWN_XY.0).abs() < 0.01);
        assert_eq!(
            ViewFocus::camera().resolve(None),
            [SPAWN_XY.0, SPAWN_XY.1, 0.0]
        );
    }
}

#[cfg(test)]
mod straddler_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn tile_listing(uids: &[u32]) -> TileState {
        TileState {
            handle: Default::default(),
            entity: None,
            material: None,
            next_cell: 0,
            furnished: false,
            placements: uids.to_vec(),
            liquid: Vec::new(),
            wall: None,
            clutter: Vec::new(),
            welds: Vec::new(),
            merged: Vec::new(),
        }
    }

    fn straddler(owner: (i32, i32), entities: Vec<Entity>) -> Placement {
        Placement {
            model: ModelHandle::M2(Default::default()),
            transform: Transform::IDENTITY,
            entities,
            spawned: true,
            doodad_set: 0,
            name_set: 0,
            doodads: Vec::new(),
            portal_instance: None,
            refs: 1,
            owner,
        }
    }

    fn run_handoff(world: &mut World, dead: (i32, i32), uids: Vec<u32>, merge_on: bool) {
        world
            .run_system_once(
                move |mut commands: Commands,
                      streamer: Res<TerrainStreamer>,
                      mut placements: ResMut<Placements>| {
                    handoff_straddlers(
                        &mut commands,
                        &streamer.tiles,
                        &mut placements,
                        dead,
                        &uids,
                        merge_on,
                    );
                },
            )
            .unwrap();
    }

    #[test]
    fn a_dead_owner_hands_its_straddler_to_a_loaded_referrer() {
        let mut world = World::new();
        let doodad_ent = world.spawn_empty().id();
        let mut streamer = TerrainStreamer::default();
        streamer.tiles.insert((3, 5), tile_listing(&[7]));
        let mut placements = Placements::default();
        placements
            .by_id
            .insert(7, straddler((3, 4), vec![doodad_ent]));
        world.insert_resource(streamer);
        world.insert_resource(placements);
        run_handoff(&mut world, (3, 4), vec![7], true);
        let placements = world.resource::<Placements>();
        let p = placements.by_id.get(&7).unwrap();
        assert_eq!(
            p.owner,
            (3, 5),
            "ownership must move to the loaded referrer"
        );
        assert!(!p.spawned, "the placement must queue for respawn");
        assert!(p.entities.is_empty());
        assert!(
            world.get_entity(doodad_ent).is_err(),
            "the stale entities must despawn ahead of the respawn"
        );
    }

    #[test]
    fn non_owned_and_merge_off_placements_stay_untouched() {
        let mut world = World::new();
        let e1 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();
        let mut streamer = TerrainStreamer::default();
        streamer.tiles.insert((3, 5), tile_listing(&[7, 8]));
        let mut placements = Placements::default();
        placements.by_id.insert(7, straddler((3, 5), vec![e1])); // owned by the survivor
        placements.by_id.insert(8, straddler((3, 4), vec![e2])); // owned by the dead tile
        world.insert_resource(streamer);
        world.insert_resource(placements);
        // Merge off: nothing moves, even for the dead tile's own straddler.
        run_handoff(&mut world, (3, 4), vec![7, 8], false);
        {
            let placements = world.resource::<Placements>();
            assert_eq!(placements.by_id.get(&8).unwrap().owner, (3, 4));
            assert!(placements.by_id.get(&8).unwrap().spawned);
        }
        // Merge on: uid 8 hands off, uid 7 (not owned by the dead tile) is untouched.
        run_handoff(&mut world, (3, 4), vec![7, 8], true);
        let placements = world.resource::<Placements>();
        let p7 = placements.by_id.get(&7).unwrap();
        assert_eq!(p7.owner, (3, 5));
        assert!(
            p7.spawned,
            "a placement the dead tile did not own keeps its entities"
        );
        let p8 = placements.by_id.get(&8).unwrap();
        assert_eq!(p8.owner, (3, 5));
        assert!(!p8.spawned);
        assert!(world.get_entity(e1).is_ok());
        assert!(world.get_entity(e2).is_err());
    }
}
