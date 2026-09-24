//! The terrain streamer — the world's sole terrain owner, built on `benilla-assets`.
//!
//! It **streams** the ground around the player through the standard `AssetServer` (the `mpq://` source +
//! `AdtLoader`): it loads a `Handle<AdtTile>` for every tile in range and drops it when the tile leaves
//! range — so residency, async loading, the decode cache, and refcounted unloading are all the engine's
//! job now, not a bespoke worker/finalize pipeline. Per loaded tile it spawns: the merged terrain mesh +
//! splat [`TerrainMaterial`]; an avian static terrain collider (a trimesh from the same merged mesh,
//! riding the tile entity's lifecycle); the doodad/WMO placements (from `Handle<M2Model>`/`Handle<WmoModel>`,
//! deduped by the `AssetServer`, each with its own static collider entity, with the same
//! [`WowModelMaterial`] + [`DoodadFade`] components the legacy path used — so the existing
//! visibility/fade/lighting systems govern them unchanged); the MCLQ water surfaces; and the ground
//! clutter (scattered per chunk into `ClutterChunk`s the shared `stream_chunk_clutter` builds lazily).
//! It also publishes loading-screen residency. Current-map directory + id come from
//! [`CurrentMap`]/[`MapCatalogRes`] ([`crate::world_map`]); the clutter catalog + build lifecycle from
//! [`crate::clutter::ClutterPlugin`].

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
// The WMO prop-light machinery lives spawn-side (0830's named carve, executed in 0832); the two
// outside consumers — `crate::interior` and `crate::entities`' `wmo_props` — keep their
// `terrain_stream::X` paths, same as the `queries` items below.
pub use spawn::prop_light::{fold_interior_probe, hex_word, interior_light_up, PropLobeLight};
// The shared placed-model assembler + the off-thread collider build — also the WMO-gameobject
// doodad-prop path's spawner (`crate::entities`' `wmo_props`: the ship's sails ride the streamed
// gameobject entity, and its cargo hulls ride the boat's kinematic body).
pub use collider::{build_collider_task, placement_collider_data, PendingCollider};
pub use spawn::{m2_anim_bound, m2_fade, point_light, spawn_model_entities, SpawnedModel};
// The position queries + area authority (their home is `queries`; paths stay `terrain_stream::X`).
use queries::update_current_area;
pub use queries::{
    area_id_under, doodad_ground_shade, ground_effect_under, terrain_height_under,
    terrain_height_under_cached, AreaAuthoritySet, CurrentArea, ShadeResolve,
};

/// Wall-clock spent per frame spawning streamed-in geometry (terrain tiles in [`stream_terrain`],
/// doodad/WMO placements in [`spawn_loaded_placements`]) before deferring the rest to the next frame.
/// Building a static collider is a synchronous parry trimesh/QBVH build; without this cap a cold-start
/// load spawns the whole ring at once and blocks the main thread (window beachball) for seconds. ~4 ms
/// keeps each frame responsive while still streaming the world in quickly.
const SPAWN_BUDGET: Duration = Duration::from_millis(4);

/// The new streamer's residency state: which tiles are loaded and the entity each was spawned as.
/// `pub(crate)` (fields private): the sound subsystem resolves ground lookups through
/// [`ground_effect_under`] against the resident tiles.
#[derive(Resource, Default)]
pub struct TerrainStreamer {
    /// Loaded tiles by `(tile_x, tile_y)`. The [`Handle<AdtTile>`] keeps the asset resident; dropping
    /// it (on unload / map swap) lets the `AssetServer` release the tile and its sub-assets.
    tiles: HashMap<(i32, i32), TileState>,
    /// The map directory the loaded tiles are for (e.g. `"Azeroth"`); a change means a cross-map
    /// teleport, so every tile is dropped and re-streamed for the new map.
    map_dir: Option<String>,
    /// The focus tile [`stream_terrain`] last streamed around — the furnisher's nearest-first
    /// ordering key, published here so it never recomputes the focus ladder a second time.
    focus: (i32, i32),
    /// The window's `(inner, outer)` half-widths as of last frame, so the reach is logged once
    /// per change (the Terrain Distance slider) rather than per frame.
    reach: Option<(i32, i32)>,
    /// The current map's WDT tile index, requested with the map: ADT requests wait
    /// for it and consult its `MAIN` grid — open ocean authors no tiles to ask for.
    wdt: Option<Handle<WdtIndex>>,
    /// The WDT failed to load (unheard of for a shipped map): stream ungated like pre-0476 rather
    /// than showing no world. Reset on map change.
    wdt_ungated: bool,
    /// `true` once this map's global WMO has been registered as a placement — on a
    /// WMO-only map that one building is the whole world, so there is nothing else to stream.
    /// Cleared on map change, which also releases the placement.
    global_wmo: bool,
    /// [`ViewFocus::paced`] as of last frame — so the residency latch can re-arm on the EDGE into
    /// a load rather than on the level. A viewer with no avatar publishes `paced: false` forever
    /// (`ViewFocus::camera`), and re-arming on the level would leave that viewer walking its focus
    /// neighbourhood's every placement every frame for the rest of the run — the settled-work scan
    /// the latch exists to stop.
    was_paced: bool,
    /// The last frame found nothing to request, drop or spawn and every resident tile stood
    /// furnished — so a frame with the same window, the same pacing and a complete load has
    /// nothing to do and skips the window walk (three window-sized allocations and a scan of
    /// every resident tile, on every still frame — decision 1979's floor).
    settled: bool,
    /// The focus tile the off-grid tripwire below last reported, so a focus parked outside the
    /// map's WDT grid says so **once** rather than every frame. `None` while the focus is on the
    /// grid (so re-entering the state re-reports).
    off_grid_reported: Option<(i32, i32)>,
}

/// The placement id the map-global WMO is registered under. A WMO-only map authors **no** ADT tiles
/// at all, so it contributes no MODF/MDDF uniqueIds this could collide with — and the value is the
/// one the file itself carries in the dead `uniqueId` slot (the reference overwrites it from its own
/// counter at `0xc9a320`, having no more use for it than we do).
const GLOBAL_WMO_UID: u32 = u32::MAX;

impl TerrainStreamer {
    /// Residency counts for the debug panel's World readout: `(spawned, requested)` — tiles fully
    /// up (root + cells furnished) vs. every tile in the stream window (up + loading/furnishing).
    pub fn residency(&self) -> (usize, usize) {
        let spawned = self.tiles.values().filter(|t| t.furnished).count();
        (spawned, self.tiles.len())
    }
}

/// One loaded tile: the resident asset handle, the terrain entity, and the placement ids it references.
struct TileState {
    handle: Handle<AdtTile>,
    /// `None` until the `AdtTile` finishes loading and the terrain root entity is spawned (which is also
    /// when this tile's placements are registered).
    entity: Option<Entity>,
    /// The tile's terrain material, made at root spawn and shared by every cell the furnisher hangs
    /// off the root (one material per tile — the loader's arrays make that possible).
    material: Option<Handle<TerrainMaterial>>,
    /// The furnishing cursor: the next `AdtTile::chunks` index whose cell [`furnish::furnish_tile_cells`]
    /// should build. Cells land a few per frame while live — the mesh-asset creation
    /// IS the pacing point; everything downstream (extract, prepare, upload) follows its rate.
    next_cell: usize,
    /// `true` once every cell is up (or cells are ablated off): the "this tile is really resident"
    /// bit the loading-screen accounting reads, so a reveal never shows a root with bare ground.
    furnished: bool,
    /// uniqueIds of the doodad/WMO placements this tile references — registered once the tile's `AdtTile`
    /// loads, refcount-released when the tile unloads.
    placements: Vec<u32>,
    /// The tile's water-surface entities (tile-exclusive, like the terrain mesh — despawned on unload).
    liquid: Vec<Entity>,
    /// The tile's impassable-chunk wall collider, if it authors any. Its own static
    /// body rather than a child of the root, matching the per-WMO walk/camera bakes: two colliders
    /// with different audiences are two bodies, not a nested one.
    wall: Option<Entity>,
    /// The tile's per-chunk `ClutterChunk` entities (tile-exclusive; their lazily-built clutter meshes
    /// are children, so despawning these cascades to them).
    clutter: Vec<Entity>,
    /// The welded map-doodad hull colliders this tile owns: batches of the hulls
    /// whose placements registered here first, despawned with the tile like [`Self::wall`].
    welds: Vec<Entity>,
    /// The merged static-render blobs this tile owns (`WOW_STATIC_MERGE`):
    /// per-(cell × material) bakes of the doodads whose placements registered here first,
    /// despawned with the tile exactly like [`Self::welds`].
    merged: Vec<Entity>,
}

/// Doodad/WMO placements spawned **once** and shared across the tiles that reference them — the client's
/// own cross-tile dedup (a building straddling N tiles is spawned once and refcounted). The model assets
/// are loaded by `Handle`, so the `AssetServer` dedups the *decode*; this dedups the *instance*.
#[derive(Resource, Default)]
pub(crate) struct Placements {
    /// By MDDF/MODF uniqueId: the placement + how many loaded tiles reference it.
    by_id: HashMap<u32, Placement>,
    /// Material dedup, so submeshes sharing a (texture, blend, sidedness, kind, fade-variant) share one
    /// `WowModelMaterial` handle — what lets Bevy batch them (mirrors the old `model_material` cache).
    materials: MaterialCache,
    /// Registered-but-unspawned work — placements awaiting their model plus WMO props awaiting
    /// their own M2s. Zero means the spawner's full [`Self::by_id`] walk has nothing to find, so
    /// it returns without paying it (a resident city is thousands of placements, all spawned, in
    /// steady state). Kept by the register/handoff/release sites here; the spawner itself settles
    /// it as models land (and adds a WMO's props the moment they resolve).
    pending_spawns: usize,
}

impl Placements {
    /// Every entity some registered placement owns — the set a despawn walks. The duplicate
    /// census (`world_census`) subtracts it from the live `WorldObject` population: a placed
    /// part alive outside this set outlived its registration, which is what a doubled prop is.
    pub(crate) fn owned(&self) -> std::collections::HashSet<Entity> {
        self.by_id
            .values()
            .flat_map(|p| p.entities.iter().copied())
            .collect()
    }
}

/// A shared placement: its resident model handle, world transform, and spawned submesh entities. Doodad
/// vs WMO is read off the [`ModelHandle`] variant, so it isn't stored separately.
struct Placement {
    model: ModelHandle,
    transform: Transform,
    /// Spawned submesh entities — empty until the model asset finishes loading (`spawned` then `true`).
    /// WMO doodad-prop submeshes ([`Placement::doodads`]) are appended here too, so they despawn together.
    entities: Vec<Entity>,
    /// `true` once we've spawned (or determined there's nothing to spawn) — so we don't retry.
    spawned: bool,
    /// MODF doodad-set index (WMOs only; 0 for M2 doodads) — which extra prop set to show beyond set 0.
    doodad_set: u16,
    /// MODF name-set index (WMOs only) — the placement's `WMOAreaTable.NameSetID` audio variant.
    name_set: u16,
    /// WMO doodad props (candle stands, banners) resolved once the WMO root loads — each its own M2,
    /// spawned across frames as its asset arrives. Empty for M2-doodad placements.
    doodads: Vec<WmoDoodadInst>,
    /// The placement's [`crate::wmo_portal::WmoPortalInstance`] entity, spawned with the building's
    /// groups. Held so the props — which spawn later, as their own M2 assets land — can be tagged
    /// with the same instance and cull alongside the group that owns them.
    portal_instance: Option<Entity>,
    /// How many loaded tiles reference this placement; despawned when it hits zero.
    refs: u32,
    /// The first tile to register this placement — loaded at that moment by construction. For an
    /// M2 doodad this is its hull weld's owner tile (the lifetime argument is in
    /// `weld`'s module doc). WMO placements never read it: their prop hulls weld per placement.
    owner: (i32, i32),
}

/// How much of the world the streamer wants around the view focus is actually there. Written each
/// frame by [`stream_terrain`] (tiles + placements) and [`finish_colliders`] (the attach queue).
///
/// It lived in `loading_screen` — the engine publishing its own residency fact to the wrong side
/// of 1160's line (1163's finding). Two readers, and only one of them is a screen: the loading
/// bar reads it to draw itself, and the **player's post-snap hold** reads it to decide when to let
/// go (the physics hold keys on this signal, never on ground contact). Both are
/// the game asking the world what it has; neither is the world asking the game anything.
#[derive(Resource, Default)]
pub struct WorldLoadProgress {
    /// Units done, of [`Self::total`]: desired tiles spawned + focus-neighbourhood placements up.
    pub ready: usize,
    pub total: usize,
    /// Whether the tile under the view focus (the nearest desired tile) is spawned. `false` until the
    /// streamer first runs, and again whenever a teleport/worldport drops the focus onto unloaded
    /// ground — the backstop trigger (covers startup, cross-map worldport, AND far same-map
    /// teleports like a cross-continent `.tele`, which don't change `CurrentMap`). During normal
    /// streaming the focus tile is always resident, so it never fires spuriously.
    pub focus_resident: bool,
    /// The scene term: the focus tile **and its 8 neighbours** are spawned with every
    /// placement they reference (WMOs, doodads, WMO props) — "the buildings and trees around you
    /// exist", which is what makes a reveal read as a world rather than bare terrain. Collider
    /// quiet is deliberately *not* folded in here (it is published a stage apart); consumers use
    /// [`Self::presentable`].
    pub scene_ready: bool,
    /// Outstanding collider attaches (decision 0610's queue) — published by `finish_colliders`.
    pub colliders_pending: usize,
    /// Unflushed static-merge accumulators (1418, `WOW_STATIC_MERGE`) — published by
    /// `flush_static_merge`. Merged world geometry that has not yet baked is a hole the reveal
    /// must not show; same conservative semantics as the weld backlog inside
    /// [`Self::colliders_pending`]. Always `0` with the lever off.
    pub merge_pending: usize,
    /// Focus-neighbourhood placements not yet spawned (the wait instrument's term).
    pub placements_pending: usize,
    /// Retained-pass regions collected but not yet drawable (`StaticGx::undrawn_regions`) —
    /// geometry that has *spawned* and still draws nothing.
    ///
    /// Since slice 2 (1429/1430) a WMO's group geometry leaves the entity path for the retained
    /// pass, so "the placement spawned" stopped meaning "the building is on screen": between the
    /// divert and its first bake the region is a hole. The reveal used to be gated on the spawn
    /// alone and therefore lifted up to `IDLE_FRAMES` before the buildings could draw — a city of
    /// bare ground for a quarter second. Same conservative semantics as the collider and merge
    /// backlogs beside it.
    pub gx_pending: usize,
    /// **Which tile these facts are about** — the focus tile the streamer computed this frame.
    /// `None` until the streamer first runs (and again after `release_world`). The settle release
    /// compares it against the tile under the avatar's own feet and refuses the resident release on
    /// a mismatch: residency published for one tile must never unfreeze a body
    /// standing on another — the fail-closed guard that makes any future focus/snap ordering
    /// regression cost a logged 6 s timeout instead of a silent fall through the world.
    pub focus_tile: Option<(i32, i32)>,
    /// Latched by the publisher once its own terms first read fully ready ([`Self::scene_ready`]
    /// and [`Self::is_ready`]): the focus neighbourhood's per-placement walk is skipped from
    /// then on — the counts stay at their converged values — until the focus lands on
    /// unfurnished ground again (a worldport's map swap, a far teleport), which re-arms the full
    /// accounting. [`Self::focus_tile`] and [`Self::focus_resident`] stay live every frame
    /// regardless: the loading screen's backstop and the settle release key on them.
    pub complete: bool,
}

impl WorldLoadProgress {
    /// 0..1 residency fraction for the bar — **0.0 when nothing is wanted yet** (the cold-start /
    /// pre-`desired` frame), so the bar starts empty rather than flashing full. (Distinct from the
    /// clear test [`Self::is_ready`], which treats `total == 0` as not-ready.)
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.ready as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    /// True once every wanted unit is resident. Requires `total > 0` so the cold-start / post-swap
    /// frame (before `desired` is computed) doesn't read as "done".
    pub fn is_ready(&self) -> bool {
        self.total > 0 && self.ready >= self.total
    }

    /// The world at the focus is presentable — the scene term plus a quiet collider queue. What
    /// both consumers (the screen clear here, the settle release in the streamer) key on.
    pub fn presentable(&self) -> bool {
        self.scene_ready
            && self.colliders_pending == 0
            && self.merge_pending == 0
            && self.gx_pending == 0
    }
}

/// **Where the world should stream from** — the engine's one required input besides
/// [`crate::world_map::CurrentMap`], written by whatever owns a viewer and read by every
/// streaming lane (terrain, the WDL ring, the area authority, the form furnisher).
///
/// The streamer used to read `player::Player` directly, which is 1160's wire (a): the engine
/// asking the game where the avatar is. Inverted, the question becomes the game's to answer, and
/// a second program with no avatar at all (`benilla-worldview`) answers it from its free-fly
/// camera without stubbing a player.
///
/// The ladder it encodes, unchanged (`0x695650`-era behaviour): a live attached
/// avatar wins; with no avatar, the picked character's entry row; else the camera; else the spawn
/// point. A **detached** eye (free-fly) skips straight to the camera — but keeps publishing the
/// body's position, because the zone authority follows the character and not the camera.
#[derive(Resource, Default, Clone, Copy)]
pub struct ViewFocus {
    /// The avatar's world position (wow coords) when one is live — present even while detached.
    body: Option<[f32; 3]>,
    /// Is the eye following the body? `false` in free-fly, where the stream follows the camera.
    attached: bool,
    /// The picked character's map + position, for the entry window before an avatar exists.
    entry: Option<(u32, [f32; 3])>,
    /// Is the focus **settled** — steady enough to pace spawning behind the per-frame caps?
    /// `false` through entry, a teleport and a world swap, when the loading cover is absorbing the
    /// burst and a cap would only lengthen the reveal.
    pub(crate) paced: bool,
}

impl ViewFocus {
    /// No viewer at all — follow the camera, or the spawn point if there is not even one.
    pub fn camera() -> Self {
        Self::default()
    }

    /// A live avatar at `wow`, with the eye on it. `paced` is the settled bit.
    pub fn body(wow: [f32; 3], paced: bool) -> Self {
        Self {
            body: Some(wow),
            attached: true,
            entry: None,
            paced,
        }
    }

    /// **Is the stream following the body's own ground?** True only for an attached live avatar.
    ///
    /// The question anyone asking "is the world under the *player* loaded?" actually means. While
    /// the eye is detached — free-fly, a cinematic fly-by — residency describes wherever the
    /// camera went, so a consumer that reads it as a fact about the body is reading someone else's
    /// tile. `release_post_snap_hold` already learned that the hard way and tests
    /// the tile itself; this is the cheap form for a consumer that only needs the yes/no.
    pub fn follows_body(&self) -> bool {
        self.attached && self.body.is_some()
    }

    /// A live avatar whose eye has been **detached** (free-fly): the stream follows the camera,
    /// the zone authority still follows the body.
    pub fn detached(wow: [f32; 3], paced: bool) -> Self {
        Self {
            attached: false,
            ..Self::body(wow, paced)
        }
    }

    /// No avatar yet — the picked character's own map and position, so world entry streams the
    /// destination rather than wherever the camera was parked.
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

    /// Which map the streamers should be loading — `current` once the server has said, and the
    /// picked character's own map for the entry window before that. It **must** agree with
    /// [`Self::resolve`]: a position from one map against the tile grid of another names tiles
    /// that exist but are somewhere else entirely.
    pub(crate) fn map(&self, current: Option<u32>) -> u32 {
        if self.body.is_none() {
            if let Some((map, _)) = self.entry {
                return map;
            }
        }
        current.unwrap_or(0)
    }

    /// The **body's** own position when there is one — what the zone/area authority measures from,
    /// which is the character and never the free-fly camera.
    pub(crate) fn body_pos(&self) -> Option<[f32; 3]> {
        self.body
    }

    /// Is there a body **and** is the world under it the one it is standing in?
    ///
    /// The area authority's gate (1287) — the same bit [`ViewFocus::paced`] carries, read through
    /// its own name because it answers a different question here. Through entry, a teleport and a
    /// world swap the body has been snapped but its destination is still streaming, so the leaf
    /// area under it is whatever tile happens to be resident: at first login that is the pre-snap
    /// position's — the map centre, Eastern Plaguelands on map 0 and The Barrens on map 1 — and a
    /// beat later the outdoor parent of an interior whose WMO claim has not landed yet. Publishing
    /// those makes the client announce, play the music of, and join the channels of zones the
    /// player was never in.
    pub(crate) fn body_settled(&self) -> bool {
        self.body.is_some() && self.paced
    }
}

/// **Where the world streams from when nothing else can say** — the Human start (Northshire),
/// where `one`/`One` logs in, so a viewer sits in the middle of the loaded block rather than at
/// Stormwind's edge.
///
/// The last rung of [`ViewFocus`]'s ladder, and the reason it lives here rather than at the crate
/// root: it is the *streamer's* fallback, and an engine that cannot answer "where do I stream
/// from" with no game attached is not an engine. It was a `lib.rs` const, which made it invisible
/// to both walls — neither could see a crate-root item at all until the scan learned to.
pub const SPAWN_XY: (f32, f32) = (-8949.95, -132.49);

/// A placement's model asset handle — an M2 doodad or a WMO building. Keeps the asset resident.
enum ModelHandle {
    M2(Handle<M2Model>),
    Wmo(Handle<WmoModel>),
}

/// The terrain streamer plugin (added by `main` as the world's terrain owner).
/// **Per-frame streamer activity** — what this streamer did this frame, bumped by the
/// `WorldStage::Stream` chain as it works (a handful of integer adds + five self-timers,
/// maintained unconditionally and zeroed by whoever reads them).
///
/// Exists because B181's symptom — a frame spike on every ADT tile-boundary crossing — is keyed to
/// *what the streamer did that frame*, which no 1 Hz journal row can attribute: the spike frame
/// needs its own row saying what was dropped, requested and spawned beside what the frame cost.
/// `perf`'s stream trace is that reader, and it used to own this resource too, which meant the
/// engine's own streaming account only existed if an instrument was installed (
/// the same mis-filing as `terrain_stream` writing its residency into `loading_screen`). The
/// writer owns it now; the instrument reads it.
#[derive(Resource, Default)]
pub struct StreamActivity {
    /// Tiles whose owned entities (terrain root + cells, liquid, clutter) were despawned.
    pub tiles_dropped: u32,
    /// Shared placements whose refcount hit zero (their entities despawned with them).
    pub placements_dropped: u32,
    /// Entity handles despawned with those placements (submeshes, colliders, lights, props).
    pub placement_entities_dropped: u32,
    /// New ADT asset requests fired.
    pub tiles_requested: u32,
    /// Loaded tiles spawned (root + collider kicked off + placements registered + liquid + clutter).
    pub tiles_spawned: u32,
    /// MCNK cell meshes built + spawned by the paced furnisher.
    pub cells_spawned: u32,
    /// Model submesh render forms built by the paced model furnisher.
    pub model_meshes_built: u32,
    /// Placements (or WMO props) whose model landed and spawned.
    pub placements_spawned: u32,
    /// Off-thread colliders attached.
    pub colliders_attached: u32,
    /// Self-time of `stream_terrain` / `furnish_tile_cells` / `furnish_model_forms` /
    /// `spawn_loaded_placements` / `finish_colliders` (ms).
    pub stream_ms: f32,
    pub furnish_ms: f32,
    pub mfurnish_ms: f32,
    pub spawn_ms: f32,
    pub collider_ms: f32,
}

impl StreamActivity {
    /// Did the streamer *do* anything this frame? (The self-timers don't count: the desired-set
    /// scan runs every frame and would make every row an "event".)
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
        // The engine's own input resources, defaulted HERE so a second program that
        // registers no game at all still boots: with no writer, `ViewFocus::camera()` is the
        // free-fly answer and the streamer follows the camera (this is what `benilla-worldview`
        // does, and why the spike needed a stub `Player` before this inversion).
        app.init_resource::<ViewFocus>()
            .init_resource::<WorldLoadProgress>()
            .init_resource::<TerrainStreamer>()
            .init_resource::<StreamActivity>()
            .init_resource::<Placements>()
            .init_resource::<HullWelds>()
            .init_resource::<StaticMerge>()
            .init_resource::<crate::model_forms::ModelForms>()
            .init_resource::<CurrentArea>()
            // **The streaming chain lives in `WorldStage::Stream`** — the load-bearing half of the
            // frame ordering contract (schedule.rs): the teleport snap (Input) → the streamer
            // recomputes focus + publishes residency HERE → the loading screen covers (Present),
            // all in one frame, so a swap never renders uncovered. The legacy streamer carried
            // this membership and the cutover to this one silently dropped it — the executor was
            // then free to run the streamer *before* the snap, and the cover landed a frame late
            // on exactly the burst frame (the `.tele` destination flash).
            // `finish_colliders` heads the chain so the collider-queue depth the settle release
            // and the Present-stage clear read is this frame's, not last frame's.
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
                    // **No world until a character is in one**. The whole chain,
                    // not just the request side: with nothing streamed there is nothing to spawn,
                    // no collider to finish and no WMO volume to mirror, and gating the head alone
                    // would leave three systems walking empty queries to prove it every frame.
                    .run_if(crate::schedule::world_is_live),
            )
            // The blob-visibility dump (`WOW_BLOB_VIS=1`) — a dev instrument, registered only
            // when asked for; see `merge::log_blob_vis`.
            .add_systems(
                Update,
                merge::log_blob_vis
                    .run_if(merge::blob_vis_enabled)
                    .run_if(crate::schedule::world_is_live),
            )
            // The model-forms furnisher runs UNGATED, unlike the chain above: the
            // glue screens' character preview requests forms pre-world (`update_display_models`
            // serves the glue stage too), and a world-live gate would starve it forever. Ordered
            // before the placement spawner so a form completed this frame can land this frame;
            // everything it does is demand-driven, so with no requests it is a no-op.
            .add_systems(
                Update,
                crate::model_forms::furnish_model_forms
                    .in_set(WorldStage::Stream)
                    .before(spawn_loaded_placements),
            )
            // Leaving the world releases it. Without this, a `/logout` back to character select
            // would leave the whole streamed world resident and unowned — the gate above stops it
            // being *maintained*, which is precisely what makes an abandoned world immortal.
            .add_systems(
                Update,
                release_world
                    .run_if(crate::schedule::world_left)
                    // Between the wire drain and the stream stage, so the release happens on the
                    // frame the world went away — the promptness `OnExit` used to give it. The
                    // session's publisher runs before `Net`, so this reads THIS frame's edge.
                    .after(crate::schedule::WorldStage::Net)
                    .before(crate::schedule::WorldStage::Stream),
            )
            // Within-map art residency: the placement material dedup expires by
            // distance, so a long flight on one continent stops ratcheting. Ungated by
            // `world_is_live` on purpose — a cache still holding the last world's art is exactly
            // what should be draining while you sit at character select.
            .add_systems(Update, scope_placement_art)
            // The WorldDetail re-scatter runs UNGATED: out of world the tile map is empty (a
            // no-op walk) and the density tracker must never miss a boot-time config apply.
            .add_systems(Update, rescatter_clutter)
            .add_systems(
                Update,
                // After the interior claim so the leaf override reads THIS frame's claim —
                // the client resolves leaf + indoor + names from ONE node state in one pass
                // (`0x67e510`); a stale-leaf/fresh-claim frame let the abbey login big-splash
                // (the A ≠ subzone gate saw "Northshire Valley" against the abbey's name).
                // `AreaAuthoritySet` lets the zone-text feed order after this in turn.
                update_current_area
                    .after(crate::wmo_portal::WmoPvsSet)
                    .in_set(AreaAuthoritySet),
            );
    }
}

/// Stream `AdtTile`s around the view focus: drop tiles that left range (releasing their placements),
/// request newly-in-range tiles, and — as each finishes loading — spawn its terrain mesh + material and
/// register its doodad/WMO placements. The desired square is gated on the map's WDT `MAIN` grid:
/// a tile the map doesn't author is never requested — no NotFound error spam on
/// open-ocean crossings, and the loading screen's ready/total counts only tiles that can exist.
#[allow(clippy::type_complexity)] // the bundled asset_stores tuple
fn stream_terrain(
    mut commands: Commands,
    mut state: ResMut<TerrainStreamer>,
    placements: ResMut<Placements>,
    asset_server: Res<AssetServer>,
    tiles: Res<Assets<AdtTile>>,
    // Bundled into one tuple param to stay within Bevy's 16-element system-param limit; destructured
    // to the same bindings below.
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
    // "Where are we, and how far do we look" — the server's map, the catalog that names its
    // directory, and the live view distance the residency window derives from (1513). Bundled
    // for the same param-limit reason as `asset_stores`.
    location: (
        Option<Res<CurrentMap>>,
        Option<Res<MapCatalogRes>>,
        Res<crate::view::ViewDistance>,
    ),
    mut load_progress: Option<ResMut<WorldLoadProgress>>,
    // Bundled for the same 16-param reason as `asset_stores`: the two stream-batch accumulators
    // the map-change drop must clear.
    batchers: (
        ResMut<HullWelds>,
        ResMut<StaticMerge>,
        // `None` only under `WOW_STATIC_GX=0` (the collector exists whenever the retained
        // pass is armed — the default since 1434).
        Option<ResMut<crate::static_gx::StaticGx>>,
    ),
) {
    let (mut welds, mut static_merge, mut staticgx) = batchers;
    let (mut materials, mut meshes, wdts, _time, mut activity) = asset_stores;
    let (current_map, map_catalog, view) = location;
    // The shared light buffer + map catalog are set up by other plugins' startup; until they exist
    // there's nothing to stream against, so idle.
    let (Some(shared_light), Some(map_catalog)) = (shared_light, map_catalog) else {
        return;
    };
    let t0 = Instant::now();
    let placements = placements.into_inner();
    let map_id = focus.map(current_map.map(|m| m.0));
    let Some(dir) = map_catalog.0.directory(map_id).map(str::to_string) else {
        return;
    };

    // Cross-map teleport: the map directory changed → drop every loaded tile (despawn + release the
    // handles + placements) so the new map streams in fresh, and request the new map's WDT (the
    // tile-existence index every ADT request below consults).
    if state.map_dir.as_deref() != Some(dir.as_str()) {
        // Say which map's tiles we are about to spend the IO budget on, and how many we are
        // throwing away to do it. Silent before, which is exactly why nobody noticed that every
        // world entry began by streaming Northshire regardless of where the character was.
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
    // The WDT gate. A missing/failed WDT (unheard of for a shipped map) falls back to the
    // old ungated probing, warned once per map — a broken index must never mean "no world".
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

    // A WMO-only map: the WDT says this map authors no terrain and its entire world
    // is one building. Register it exactly like any ADT-placed WMO — same `Placement`, same
    // spawn/collider/doodad/portal path — because that is what the reference does: its WDT branch
    // hands the global MODF entry to the SAME consumer (`0x695650`) an ADT's MCRF walk does. It is
    // registered once for the map and released on the map change above; nothing streams it in or
    // out, since there is no "range" for a building that is the map.
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

    // Stream around the *view focus* — see [`ViewFocus`] for the ladder — as far as the view
    // distance reaches: the residency window is the reference's own, derived from the live
    // `farclip` in chunk units (`window`). The slider that moves the far-clip
    // wall moves this with it.
    let center = focus.resolve(camera.single().ok().map(|c| c.translation));
    let window = StreamWindow::at(view.farclip, center[0], center[1]);
    let (cx, cy) = window.focus_tile();
    let same_window = state.focus == (cx, cy) && state.reach == Some((window.inner, window.outer));
    // Never while a load is in flight: `paced` is false from a snap until the body settles, and
    // the tail this skip bypasses is where the load is RELEASED — the retained pass's undrawn
    // count that `presentable()` reads, the burst-over `flush_now`, and the settle hold. A
    // skip that engaged the frame `complete` flipped, with the pass still baking the new
    // cells, froze that count above zero and held the loading screen up for good — the
    // .tele hang of 2026-09-04, the day after this skip landed (1982). So the skip needs a
    // paced, presentable, complete world, and even then keeps the undrawn count fresh so a
    // later rebake un-skips it.
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
    // One line per change of reach — the slider moved, or the first frame — so a run log says
    // what the residency window was when a tile count or a frame cost was read off it.
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

    // Desired set: every tile the outer window touches, clamped to the 64×64 tile grid — and
    // filtered to tiles the WDT says exist (open ocean authors none; the reference's own
    // `MAIN` bit-0 test at `0x69863c`). The filter also fixes the loading screen's accounting:
    // `ready == total` is reachable on a coast, and any fallback-era entry for a nonexistent
    // tile turns stale and unloads below.
    let mut desired = window.wanted_tiles();
    if let Some(w) = wdt_index {
        desired.retain(|&(tx, ty)| w.has_tile(tx as u32, ty as u32));
    }

    // **The focus is off this map's tile grid** — the window touches no tile the WDT authors, so
    // there is nothing to stream and nothing to wait for. On an ADT map that is never a place a
    // body can legitimately stand: it means the focus was written in some *other* map's
    // coordinates (the 2026-09-05 report — a rider composed through a cross-map transport whose
    // own pose was still the source continent's) or has left the world entirely.
    //
    // It earns a tripwire because of what it does DOWNSTREAM and silently: `desired` empty makes
    // `total == 0`, which [`WorldLoadProgress::is_ready`] reads as *not ready* — so the loading
    // screen can never clear — while the vacuous arm below reports `focus_resident = true`, so the
    // backstop that would have re-raised a cover cannot fire either. The player is left staring at
    // a loading screen whose wait line reads `0/0 resident` with every other term nominal. Naming
    // the map and the tile turns that into one greppable line. Once per focus tile, never a frame
    // loop; `global_wmo` maps legitimately author no tiles and are excluded.
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

    // Unload tiles no longer desired: despawn the terrain entity, release the placements, drop the
    // handle. **Budgeted per frame**: dropping the whole trailing row on the crossing frame
    // freed ~1300 mesh assets — each tile's 256 chunk cells, CPU copies included — plus the tiles'
    // decoded chunks and their placements' models, all in one frame. Measured (`WOW_STREAM_TRACE`,
    // Emerald Dream flat-map pin): that free wave is the 2–3-dropped-interval spike on every ADT
    // boundary crossing, while this chain's own systems — request, spawn, collider attach, each
    // already budgeted — stayed under 3 ms. So the drop lane gets the same treatment: at most
    // `unload_budget` tiles release per frame, farthest from the focus first. The un-drained
    // remainder just stays resident a few frames longer — behind the direction of travel, and a
    // re-cross inside the drain window finds its tile still loaded (no re-decode, no re-spawn).
    // A cross-map swap is NOT budgeted (`drop_streamed_world` above): the loading screen covers
    // it, and holding a dead map's tiles through a fresh map's IO burst helps nothing.
    // **The keep-band**: a tile is wanted inside the outer window but released only past
    // it plus one chunk (`StreamWindow::keeps`) — hysteresis. Without it, a body pacing across
    // the one chunk line that toggles a far tile cycles that tile per step: the reporter's own
    // minimal repro (`.go` half a yard across, and back) re-decoded and re-spawned five tiles
    // each way under the old tile-centred window, and the row's landing — not the streamer's
    // own budgeted systems — is the frame spike (a fresh Undercity row lands ~3900 placements
    // and ~1300 mesh assets). A band tile stays fully resident, so a re-cross finds it and
    // streams NOTHING; the band's dedup entries need no art-sweep accommodation (0793's
    // `radius_floor` is about art the streamer will re-fetch — a band tile is already spawned
    // and never re-consults the caches). The reference releases by window membership alone
    // (`0x6984f0`) — its reload is a cheap async read; ours is the spawn wave above.
    //
    // Ablation switch (`WOW_NO_TILE_DROP=1`): never release stale tiles — a crossing becomes a
    // pure asset-ADD event and a re-cross a pure no-op, separating the load lane from the free
    // lane when measuring a streaming cost. Residency grows for the life of the run; dev-only.
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
            // The B1 retained cells (1429): a diverted batch has no entity and no blob, so the
            // dead owner's items leave here or never — the cells re-bake without them.
            if let Some(gx) = staticgx.as_deref_mut() {
                gx.release_owner(c);
            }
        }
    }

    // Request newly-desired tiles (the `AssetServer` dedups, so re-requesting a loaded one is
    // free) — only once the WDT has answered (or failed into the ungated fallback): a request
    // fired before the index lands could probe a tile that doesn't exist.
    //
    // **Staggered while live**: a whole fresh row requested in one frame decodes in
    // parallel and LANDS near-together — ~1300 mesh assets plus the texture arrays hitting the
    // render world's prepare in one or two frames, a wave no app-side budget downstream of the
    // request can spread. One fresh request per frame, nearest first, staggers the landings for
    // the only cost of the window edge (~2 tiles out, in fog) filling over a few frames. World
    // entry and teleports stay unstaggered: the loading screen exists to absorb that burst, and
    // the settle release waits on exactly these tiles.
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

    // Spawn any loaded-but-unspawned tile with the production terrain material, and register its
    // placements. The material references the ONE shared global-light buffer (updated in place each
    // frame), so a freshly-streamed tile is correctly lit + fogged on its first frame. Budgeted per
    // frame (a tile's terrain collider is a big trimesh/QBVH build): on cold start the whole ring
    // finishes loading near-together, and spawning every tile in one frame stalls the main thread.
    let tile_deadline = Instant::now() + SPAWN_BUDGET;
    for (&(tx, ty), tile) in state.tiles.iter_mut() {
        if tile.entity.is_some() {
            continue;
        }
        let Some(adt) = tiles.get(&tile.handle) else {
            continue; // not loaded yet (or missing) — try again next frame
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
        // Terrain collider: ONE static trimesh per tile, welded from the same decoded
        // chunks the drawn cells are built from (so you stand on the visible ground), built off-thread
        // (attached by `finish_colliders`) so a tile streaming in never hitches the frame. It rides the
        // tile root's lifecycle — gone when the tile despawns, no separate bookkeeping.
        let collider_data = terrain_collider_data(&adt.chunks);
        // …and, separately, the tile's impassable-chunk fences (report B129). A
        // SECOND collider because the audience is the point: the reference's fence is emitted only
        // into the movement box gather, and its segment/ray path never reads the flag at all. So —
        // the walk layer, where the body sees it and the camera boom does not, and unmarked, so it
        // neither takes the selection ring nor clamps the mouse pick. An authored wall is not scenery.
        let wall_data = impassable_wall_data(&adt.chunks);
        // The tile ROOT draws nothing: it carries the collider and the surface roles, and its MCNK
        // cells hang off it as children — one drawn object per 33.333 yd chunk. That is the unit the
        // exterior-scene cull needs (a 533 yd slab the camera stands on intersects
        // every portal window, so it could never be hidden), and it also gives Bevy's own frustum cull
        // something smaller than half a kilometre to reject. Despawning the root takes the cells.
        //
        // The cells themselves do NOT spawn here: a whole row of tiles finishes
        // decoding in one frame, and spawning ~1300 loader-built cell meshes at once handed the
        // render world their entire extract/prepare/upload as one 80–105 ms (process-CPU) frame —
        // the B181 first-contact spike. `furnish_tile_cells` (chained right after this system)
        // builds them from `AdtTile::chunks` + `shading` a few per frame while live.
        // Chain-only visibility (`crate::vis_chain`): the tile root renders nothing itself —
        // its cell meshes are the drawn children, and they need the inheritance chain intact.
        let mut tile_ent = commands.spawn((Transform::IDENTITY, Visibility::default()));
        tile_ent.vis_chain_only();
        if let Some((verts, tris)) = collider_data {
            // `GroundDecalSurface`: terrain receives the selection ring (see `crate::collision`).
            // `PickOccluder`: terrain clamps the mouse pick (the reference's world trace).
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

        // Register this tile's doodad/WMO placements (deduped + refcounted by uniqueId across tiles).
        // The MCSH ground-shade is NOT sampled here: a doodad straddles several tiles and this one may
        // not contain its origin, so the shade is resolved at spawn via a global lookup (see
        // `doodad_ground_shade` / `spawn_loaded_placements`) — the reference's own per-frame model.
        for d in &adt.doodads {
            register_doodad(placements, &asset_server, d, (tx, ty));
            tile.placements.push(d.unique_id);
        }
        for w in &adt.wmos {
            register_wmo(placements, &asset_server, w);
            tile.placements.push(w.unique_id);
        }

        // Spawn this tile's water surfaces (tile-exclusive — despawned with the tile, like its mesh).
        let mut liquid_ents = Vec::new();
        spawn_liquids(
            &mut commands,
            adt.chunks.iter().flat_map(|c| c.liquids.iter()),
            liquid_assets.as_deref(),
            &mut meshes,
            &mut liquid_ents,
        );
        tile.liquid = liquid_ents;

        // Scatter this tile's ground clutter into per-chunk `ClutterChunk` units (tile-owned; the
        // shared `stream_chunk_clutter` builds + tears down their meshes lazily within the ~70 yd
        // detail-doodad horizon, same as for the old streamer's chunks). Needs the app-side
        // ground-effect catalog + density; absent before they're set up, so skipped until then.
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

        // One tile spawned; if that used up the frame's budget, leave the rest for next frame (the
        // `tile.entity.is_some()` guard above makes this re-entrant). The loading bar reflects the
        // partial residency below, so it animates rather than jumping to full after a stall.
        if Instant::now() >= tile_deadline {
            break;
        }
    }

    // Publish residency for the loading screen: how many desired tiles are actually spawned,
    // whether the tile under the view focus is up (the backstop trigger), and the scene term
    // — the focus tile + its 8 neighbours with every placement they reference, so
    // "ready to reveal" means the buildings and trees around you exist, not just bare terrain. A
    // focus tile that doesn't exist (map edge) counts as resident so we never get stuck waiting
    // for ground that isn't there.
    // The pacing edge, read and advanced once per frame (see [`TerrainStreamer::was_paced`]).
    state.settled = stale_empty
        && fresh_empty
        && state
            .tiles
            .values()
            .all(|t| t.entity.is_some() && t.furnished);
    let was_paced = std::mem::replace(&mut state.was_paced, focus.paced);
    if let Some(p) = load_progress.as_mut() {
        p.focus_tile = Some((cx, cy));
        // Resident = the focus tile is furnished — judged from the DESIRED set, not from the
        // presence of a `state.tiles` entry: a desired tile whose request hasn't landed yet is
        // absent from the map, and "no entry" must read as *not there*, never as "nothing to wait
        // for". The vacuous arm is only for ground the WDT never authors (map edge, open ocean,
        // a WMO-only map) — waiting there would hang forever on terrain that cannot exist.
        p.focus_resident = if desired.contains(&(cx, cy)) {
            state.tiles.get(&(cx, cy)).is_some_and(|t| t.furnished)
        } else {
            true
        };
        // Until the WDT answers we don't yet know what this map is made of, so nothing under the
        // focus can be called resident. Without this the post-worldport frames before the index
        // lands read as "ground is up" — harmless on an ADT map (the tile entries below close the
        // gap a frame later) but on a WMO-only map, where `desired` is empty forever, it is the
        // difference between a loading screen and a glimpse of the void.
        if wdt_index.is_none() && !state.wdt_ungated {
            p.focus_resident = false;
        }
        // A WMO-only map's focus residency IS its one building's spawn — there is no
        // tile to gate on. The bar/scene accounting for it lives in the gated block below.
        if state.global_wmo {
            p.focus_resident &= placements
                .by_id
                .get(&GLOBAL_WMO_UID)
                .is_some_and(|p| p.spawned);
        }
        // `focus_tile`/`focus_resident` above stay live every frame — the loading screen's
        // backstop and the settle release key on them. The counting below is only consumed
        // while a load is underway, so once it has reported fully ready it latches off
        // (`complete`) instead of walking the focus neighbourhood's every placement per frame
        // forever. A focus on unfurnished ground IS a new load beginning (a worldport's map
        // swap, a far teleport), so it re-arms the latch; `release_world`'s reset covers the
        // logout → re-entry path.
        if !p.focus_resident {
            p.complete = false;
        }
        // A **snap** re-arms it too, even onto ground that is already resident. `paced` is false
        // exactly through entry, a teleport and a world swap (the game writes it from the settle
        // hold), and a near teleport — `.tele` across a city, a summon — lands on furnished ground
        // whose *buildings* may still be arriving. Without this the latch answers a fresh
        // destination with the departure's converged numbers: `scene_ready` reads true because it
        // was true where we left, and both consumers (the cover, the settle release) are told a
        // world is presentable that nobody has looked at yet.
        //
        // On the EDGE into the load, not on the level — see [`TerrainStreamer::was_paced`].
        if was_paced && !focus.paced {
            p.complete = false;
        }
        if !p.complete {
            // "Spawned" means FURNISHED (root + every cell): the furnisher is uncapped while the
            // cover is up, so this costs entry nothing — but it keeps a reveal from ever showing a
            // root whose ground is still landing (`furnished` lags root spawn by one frame here,
            // since the furnisher runs after this system in the same chain).
            let spawned = |c: &(i32, i32)| state.tiles.get(c).is_some_and(|t| t.furnished);
            p.total = desired.len();
            p.ready = desired.iter().filter(|c| spawned(c)).count();
            // The focus neighbourhood's placements join the accounting (bar + scene term). A
            // placement is *up* once its own model spawned AND its WMO props (each an M2 arriving
            // on its own schedule) have — the furniture the reveal would otherwise pop in.
            // Placements of tiles not yet spawned aren't known yet; the missing tile itself holds
            // `scene_ready` down.
            let mut near_pending = 0usize;
            let mut near_tile_missing = false;
            for dx in -1..=1i32 {
                for dy in -1..=1i32 {
                    let c = (cx + dx, cy + dy);
                    if !desired.contains(&c) {
                        continue; // off-grid or WDT says no tile — nothing to wait for
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
            // `focus_resident` already folds the WDT gate and the global WMO's spawn, so both
            // ride into the scene term through it.
            p.scene_ready = p.focus_resident && !near_tile_missing && near_pending == 0;
            // A WMO-only map's bar and clear-condition ride the placement. Counting it
            // keeps `total > 0`, which is what `is_ready` requires before it will clear the
            // screen at all; the scene term also waits for its props (the Stockade's 740
            // candles are the reveal).
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
        // The retained pass's own residency (1429's lane): geometry that has spawned and cannot
        // draw yet is a hole in the reveal exactly like an unattached collider is a hole in the
        // floor. Published every frame — one map walk, no per-item work — because both consumers
        // read `presentable()` and neither can see the bake.
        p.gx_pending = staticgx
            .as_deref()
            .map_or(0, crate::static_gx::StaticGx::undrawn_regions);
        // …and once nothing else is outstanding, the burst is over by definition: ask the pass to
        // publish what it holds instead of letting the quiet window add its own 15 frames to the
        // end of every load. Gated on the load itself (`!paced` — entry, a teleport, a world
        // swap), so steady-state play keeps the batching the windows exist for.
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
        // Residency is *published* here and read by the mover's post-snap hold one stage later
        // (`player::release_post_snap_hold`). The streamer is still the only authority on which
        // map the colliders under the avatar belong to — it just states the fact now instead of
        // reaching across 1160's line to act on it (decision 0737's release, inverted).
    }
    activity.stream_ms += t0.elapsed().as_secs_f32() * 1000.0;
}

/// `WOW_NO_TILE_DROP=1` — never release stale tiles (the ablation switch described at its use
/// site in [`stream_terrain`]): a crossing becomes a pure asset-ADD event. Dev-only.
fn tile_drop_disabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_NO_TILE_DROP").is_some())
}

/// Register one M2 doodad placement: bump the refcount if it's already known, else load its model and
/// record it (spawned later by [`spawn_loaded_placements`] once the asset is ready).
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

/// Register one WMO building placement (vanilla scale is 1, so no per-placement scale).
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
            owner: (0, 0), // unread for WMOs — their prop hulls weld per placement (1369)
        },
    );
    placements.pending_spawns += 1;
}

/// Despawn a tile's exclusively-owned entities — the terrain mesh, water surfaces, and per-chunk
/// `ClutterChunk`s (whose built meshes are children, so the despawn cascades). Shared doodad/WMO
/// placements are NOT touched here; they're refcount-released separately.
/// Drop every streamed tile and the placements they reference — the world, released.
///
/// One body, two triggers, because they are the same event seen from different sides: a cross-map
/// teleport ends the world you were in, and so does leaving for character select. The material
/// dedup goes with the placements it deduped for — its strong handles are what kept every
/// previous map's materials (and their textures) resident forever (the teleport leak), so it
/// is cleared *here*, sharing the exact trigger of the teardown it belongs to, rather than hanging
/// off `world_map::MapChange`.
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
    // The weld accumulators describe the world just dropped. Cleared HERE and not left to the
    // flush system's dead-key discard, because tile keys are not unique across maps: the new
    // map's tiles are requested (and their `TileState`s inserted) on this very frame, so a stale
    // accumulator could weld the previous map's geometry into a same-numbered fresh tile.
    welds.clear();
    // Same staleness argument for the merge accumulators (1417) and the census tallies: both
    // describe the world just dropped, and tile keys repeat across maps.
    merge.clear();
    // …and for the B1 retained cells (1429): cell keys repeat across maps too.
    if let Some(gx) = staticgx {
        gx.clear();
    }
    crate::static_merge::reset();
}

/// Expire the placement material dedup by **distance** — the within-map half of
/// [`drop_streamed_world`]'s clear. This is the big one: `mats` 2603 → 26786 over ten minutes on a
/// same-map tour is overwhelmingly submesh materials of doodads and buildings whose
/// tiles unloaded thousands of yards ago. The placements themselves are already refcount-released;
/// only the dedup kept their materials — and through them their textures — alive.
fn scope_placement_art(mut scope: crate::art_scope::ArtScope, mut placements: ResMut<Placements>) {
    scope.apply(
        &mut placements.materials,
        crate::art_scope::ArtSlot::PlaceMats,
    );
}

/// Leaving `InWorld` (a `/logout` back to character select): release the world.
///
/// `map_dir`/`wdt` are cleared too, so the next entry reads as a fresh map rather than a resumed
/// one — otherwise a re-entry on the SAME map would take the "nothing changed" path and stream
/// against a `TerrainStreamer` whose tiles have all been despawned.
fn release_world(
    mut commands: Commands,
    mut state: ResMut<TerrainStreamer>,
    mut placements: ResMut<Placements>,
    mut forms: ResMut<crate::model_forms::ModelForms>,
    mut progress: Option<ResMut<WorldLoadProgress>>,
    mut activity: ResMut<StreamActivity>,
    // Bundled for the same reason as `stream_terrain`'s pair: the stream-batch
    // accumulators the world drop must clear (and clippy's argument-count line).
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
    // The model-form cache frees per asset on `Unused` events; leaving the world drops the whole
    // cache in one deterministic sweep instead — nothing world-scoped may survive a logout, and
    // an entry whose asset another holder keeps resident would otherwise pin its meshes forever.
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
    // The residency the loading screen reads is about a world that no longer exists; leaving it
    // standing would let the next entry clear its cover against the previous world's counts.
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

/// The `WorldDetail` re-scatter: 1.12's own setter law — writing the density CVar
/// tail-calls the chunk-rebuild walk (`0x6725a0` → `0x6b1d20`), so a change
/// re-scatters the LOADED tiles too, not just future streams. The fresh `ClutterChunk`s spawn
/// unbuilt and the lazy builder re-meshes the ~70 yd bubble over the next frames. Watches the
/// VALUE, not `is_changed()`, because the predicate is "the density moved" and not "the resource
/// moved": `ClutterConfig` also carries the detail-doodad cutout, whose console command would
/// otherwise re-scatter every loaded tile. First sight only arms.
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

/// Mirror the live, spawned WMO placement SET into [`WmoResidency`] each frame, so the interior
/// classifier ([`crate::interior`]) re-tests standing entities when a building streams in/out under
/// them (the down-ray itself reads the live portal instances). Rebuilt wholesale: few WMOs are ever
/// resident, so it's cheap.
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

/// **1417's owner-handoff re-emit.** A straddler's merged batches and welded hulls live with
/// its OWNER tile (`Placement::owner`), so an owner unloading out from under a placement that
/// a loaded neighbour still references left a permanent hole: the blob and hull died with the
/// owner's tile, and a later re-cross of the owner only bumps refs (`register_doodad`) — the
/// re-cross defect. Hand the placement to a still-loaded referrer instead and queue a respawn:
/// the spawner re-runs the whole assembler under the new owner — the merge divert and the hull
/// weld both read `p.owner` at spawn — so 1369's matching weld gap heals in the same stroke.
/// The per-entity churn is invisible (the unload line sits past every fade band), and the
/// respawned batches open as cell singletons that fold in on the next quiet window (1424's
/// recon). M2 map doodads only: a WMO's prop blobs ride the placement's own entity list, and
/// without the merge a surviving placement never lost anything (its entities are its own).
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
    // The straddlers first — the dead tile's M2 placements that survived the release with refs
    // left (a handful to ~90 of its ~1k uids) — then ONE pass over each 8-neighbour's list. The
    // old shape asked `Vec::contains` of every neighbour's whole list per straddler: 8 × ~1k
    // compares × every straddler, on every tile drop (1697 item 2).
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
        return; // refs hit zero — released outright, nothing survives to hand off
    }
    // A straddler's other referrer shares its MDDF row across a tile seam, so it is an
    // 8-neighbour of the dead owner; the full scan is a fallback for data that defies that.
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

/// A tile referencing this placement unloaded: drop one ref, despawning its entities (render submeshes
/// + the avian collider entity) at zero — the collider rides the placement's entity lifecycle.
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
            // The removed record may still owe spawns (a model that never landed, a building's
            // props still arriving) — its share leaves the pending count with it.
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

/// The terrain base material (the `TerrainExtension` does the real work).
///
/// **Single-sided, backface-culled** — the ground is see-through from underneath, exactly as the
/// real client renders it (testers see through 1.12.1's terrain from below, the
/// way a WMO exterior reads from inside a cave; we drew it solid). The reference never sets
/// `EGxRs 0x14` (`GL_CULL_FACE`) in its terrain-chunk pass `0x684510`/`0x6beb50`, so terrain inherits
/// the device baseline `0x14 = 1` written by `0x593bf0` — culling ON, and a census of all 39
/// `0x14` setters proves none leaks in unbracketed ahead of the terrain drain. Only the passes that
/// *want* two sides clear it inside their own `Push`/`PopRenderState` bracket: the four liquid
/// passes, and the WDL mesh `0x6bd780` (cull 0 at `0x6bd79d`) — which is why `wdl.rs` stays
/// `cull_mode: None` while this one culls. Measured, not only derived: in the reference's own
/// apitrace all 103 MCNK-format draws run cull-enabled and all 20 WDL draws cull-disabled.
/// `double_sided: true` here was a Bevy default that no decision ever stood behind.
///
/// Rests on the winding: `terrain_fans_wind_ccw_seen_from_above` (benilla-formats) pins every MCNK
/// fan CCW-from-above in WoW space, and `transform_is_a_proper_rotation` pins the WoW→Bevy map at
/// det +1 — so the ground's front face points up under wgpu's `FrontFace::Ccw`. Flip either and
/// terrain disappears from *above*.
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

    /// Stratholme's coordinates, and a camera parked where `player::setup` leaves it at boot.
    const PICK: (u32, [f32; 3]) = (329, [3398.9, -3381.8, 142.7]);
    fn cam_at_northshire() -> Option<Vec3> {
        Some(wow_to_bevy([SPAWN_XY.0, SPAWN_XY.1, 100.0]))
    }

    #[test]
    fn the_pick_outranks_the_camera_until_the_avatar_exists() {
        // The regression this guards is invisible and expensive: with the camera winning, world
        // entry spends its scarcest IO on Northshire tiles for a character in Stratholme, then
        // throws them away when the snap lands.
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
        // `pending_pick` deliberately survives world entry (0065's seamless reconnect re-answers
        // with it), so a stale entry row must never pull the focus off a walking avatar.
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
        // …and the body is still published, because the zone authority follows the character.
        assert_eq!(flying.body_pos(), Some([100.0, 200.0, 30.0]));
        // A capture run: no roster, no avatar — the camera is the only answer, and the last-ditch
        // fallback must still be the anchor rather than the origin.
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

    /// The owner dies while a neighbour still lists the placement: ownership moves to the
    /// neighbour and the placement is queued for respawn (entities down, `spawned` cleared) so
    /// the assembler re-emits its merged batches and hulls under the new owner.
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

    /// A placement the dead tile merely referenced (owner elsewhere) is untouched, and the
    /// whole pass is a no-op without the merge — a surviving placement never lost anything.
    #[test]
    fn non_owned_and_merge_off_placements_stay_untouched() {
        let mut world = World::new();
        let e1 = world.spawn_empty().id();
        let e2 = world.spawn_empty().id();
        let mut streamer = TerrainStreamer::default();
        streamer.tiles.insert((3, 5), tile_listing(&[7, 8]));
        let mut placements = Placements::default();
        placements.by_id.insert(7, straddler((3, 5), vec![e1])); // owned by the SURVIVOR
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
