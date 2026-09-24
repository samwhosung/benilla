//! Distant low-detail terrain (WDL): the map's coarse horizon heightmap streamed around the view
//! beyond the ADT ring, drawn unlit under a fog pair of its own that saturates it to the flat
//! scene-fog colour. The parse and mesh are `benilla_formats::wdl`; the reference's backdrop law
//! that partitions it from the detailed world is `wdl.wgsl`'s header.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;

use crate::view::WorldCamera;
use crate::world_map::CurrentMap;
use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::materials::{WdlExt, WdlMaterial};
use benilla_assets::LockRecover;
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, RenderConfig, WorldAssets};
use benilla_formats::WdlFile;

/// Chebyshev tile radius of the WDL ring. The shader keeps it a backdrop (near plane at
/// `farclip − 33`, depth behind the detailed world), so it only fills what that leaves empty.
/// Deviation: 5, not the reference's ±3-tile far walk, because hills out to its `horizonfarclip`
/// (2112 yd, about 4 tiles) still rise above the horizon as silhouettes.
const WDL_RADIUS: u32 = 5;

/// New WDL tiles built per frame (545 vertices each), so a whole ring does not hitch a zone load.
const WDL_LOADS_PER_FRAME: usize = 8;

/// Loads the map's `.wdl` and a shared material at startup, then streams the ring around the view.
pub(crate) struct WdlPlugin;

impl Plugin for WdlPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<WdlMaterial>::default())
            .add_systems(Startup, setup_wdl.after(AssetSet::Open))
            // The ring follows the world's lifecycle: it spawns only while a world is live.
            .add_systems(Update, stream_wdl.run_if(crate::schedule::world_is_live))
            .add_systems(
                Update,
                release_wdl_ring
                    .run_if(crate::schedule::world_left)
                    .after(crate::schedule::WorldStage::Net)
                    .before(crate::schedule::WorldStage::Stream),
            );
    }
}

/// The parsed WDL, the shared ring material (it reads the global light buffer) and the spawned
/// ring tiles.
#[derive(Resource)]
pub(crate) struct WdlStreamer {
    wdl: WdlFile,
    /// The `mapId` of the loaded `wdl`; a cross-map teleport reloads it.
    map_id: u32,
    material: Handle<WdlMaterial>,
    loaded: HashMap<(u32, u32), Entity>,
}

impl WdlStreamer {
    /// The drawn WDL height (raw WoW `z`) under a Bevy-space position, exact to the mesh: the far
    /// leg of the flare occlusion march (`sun::follow::FlareGate`). `None` off the map or over
    /// unauthored ocean.
    pub(crate) fn height_under(&self, bevy_pos: Vec3) -> Option<f32> {
        let wow = bevy_to_wow(bevy_pos);
        self.wdl.height_at(wow[0], wow[1])
    }
}

/// Marks a spawned WDL tile.
#[derive(Component)]
struct WdlTile;

fn setup_wdl(
    mut commands: Commands,
    config: Option<Res<RenderConfig>>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut materials: ResMut<Assets<WdlMaterial>>,
) {
    let (Some(_config), Some(world_assets)) = (config, world_assets) else {
        return; // no client data → no terrain at all, so no WDL
    };
    // Azeroth (Eastern Kingdoms), matching `world_map`'s `DEFAULT_MAP_ID`.
    let wdl = match WdlFile::load(&mut world_assets.chain.lock_recover(), "Azeroth") {
        Ok(w) => {
            info!(
                "WDL: Azeroth.wdl loaded ({} distant tiles)",
                w.present_count()
            );
            w
        }
        Err(e) => {
            warn!("WDL unavailable, no distant terrain: {e:#}");
            return;
        }
    };
    // Reads the shared global light, whose fog `build_light_data` packs before the first draw.
    // Opaque: depth-LEQUAL with depth write and no blend, as the reference draws it.
    let material = materials.add(ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            cull_mode: None,
            ..default()
        },
        extension: WdlExt {
            light_buf: world_assets.shared_light.clone(),
        },
    });
    commands.insert_resource(WdlStreamer {
        wdl,
        map_id: 0,
        material,
        loaded: HashMap::new(),
    });
}

fn stream_wdl(
    mut commands: Commands,
    streamer: Option<ResMut<WdlStreamer>>,
    assets: Option<ResMut<WorldAssets>>,
    focus: Res<crate::terrain_stream::ViewFocus>,
    camera: Query<&Transform, With<WorldCamera>>,
    current_map: Option<Res<CurrentMap>>,
    map_catalog: Option<Res<MapCatalogRes>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // All four are absent without client data. `Option`, not `Res`: a missing `Res` fails
    // validation, which panics the client, and a build run from the wrong folder gets here.
    let (Some(mut streamer), Some(assets), Some(current_map), Some(map_catalog)) =
        (streamer, assets, current_map, map_catalog)
    else {
        return;
    };
    // The focus's map: the picked character's during world entry, so the ring is never built for
    // the map being left.
    let map_id = focus.map(Some(current_map.0));

    // Cross-map teleport: reload the new map's `.wdl` and drop the old ring.
    if map_id != streamer.map_id {
        if let Some(dir) = map_catalog.0.directory(map_id) {
            match WdlFile::load(&mut assets.chain.lock_recover(), dir) {
                Ok(wdl) => {
                    for (_, e) in streamer.loaded.drain() {
                        commands.entity(e).despawn();
                    }
                    streamer.wdl = wdl;
                    streamer.map_id = map_id;
                }
                // No `.wdl` for this map (instances often lack one): clear the ring.
                Err(_) => {
                    for (_, e) in streamer.loaded.drain() {
                        commands.entity(e).despawn();
                    }
                    streamer.map_id = map_id;
                }
            }
        }
    }

    // The detailed streamer's own view focus.
    let center = focus.resolve(camera.single().ok().map(|c| c.translation));
    // The full window, the camera's own tile included (at a low view distance it is the near
    // horizon); the shader's near plane bounds the band, never the streamed set.
    let desired = streamer.wdl.tiles_in_ring(center[0], center[1], WDL_RADIUS);

    let stale: Vec<(u32, u32)> = streamer
        .loaded
        .keys()
        .filter(|c| !desired.contains(c))
        .copied()
        .collect();
    for coords in stale {
        if let Some(e) = streamer.loaded.remove(&coords) {
            commands.entity(e).despawn();
        }
    }

    let missing: Vec<(u32, u32)> = desired
        .into_iter()
        .filter(|c| !streamer.loaded.contains_key(c))
        .take(WDL_LOADS_PER_FRAME)
        .collect();
    for coords in missing {
        let Some(tile) = streamer.wdl.tile_mesh(coords.0, coords.1) else {
            continue;
        };
        let positions: Vec<[f32; 3]> = tile
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        // A constant normal and zero UV satisfy the `StandardMaterial` pipeline; the shader reads
        // neither.
        let n = positions.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n]);
        mesh.insert_indices(Indices::U32(tile.indices));
        let e = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(streamer.material.clone()),
                Transform::IDENTITY,
                WdlTile,
                // Exterior scene: the reference's far walk `0x683040` shares the per-window
                // populate `0x682fa0`, so a sealed room hides the horizon (`crate::exterior_cull`).
                // One ring tile is one object, the reference's far-tier granularity.
                crate::exterior_cull::ExteriorScene,
            ))
            .id();
        streamer.loaded.insert(coords, e);
    }
}

/// Leaving the world drops the ring; the parsed `.wdl` stays for `height_under`.
fn release_wdl_ring(mut commands: Commands, streamer: Option<ResMut<WdlStreamer>>) {
    let Some(mut streamer) = streamer else { return };
    for (_, e) in streamer.loaded.drain() {
        commands.entity(e).despawn();
    }
}

/// The backdrop law lives in the shader, so it is checked there: a shared clip plane or a dropped
/// frag-depth write reopens a seam visible only at a ridge crest on a fogged horizon.
#[test]
fn the_far_band_stays_a_depth_pushed_backdrop() {
    let src = benilla_assets::materials::WDL_WGSL;
    assert!(
        src.contains("const WDL_OVERLAP: f32 = 33.0;"),
        "wdl.wgsl: the far band's 33 yd overlap into the wall is gone — the coarse-vs-fine seam \
         reopens as a hole at the horizon (the reference's far-band near plane, [0x8101b0])"
    );
    assert!(
        src.contains("out.depth = depth;") && src.contains("view_z_to_depth_ndc(-farclip)"),
        "wdl.wgsl: the far band no longer clamps its depth behind the far-clip wall — its overlap \
         now pokes THROUGH the detailed terrain (the reference's compressed far-band depth range)"
    );
}

/// The reference's far-band emitter submits its own fog pair, start 0 and end 1.0 (`0x6bd7ae` to
/// `0x6bd7c8` in `0x6bd780`), so the hull is the flat fog colour at every distance. Under the scene
/// fog the 33 yd overlap shows as a pale band at low view distances.
#[test]
fn the_far_band_is_a_fog_hull_not_a_fogged_surface() {
    let src = benilla_assets::materials::WDL_WGSL;
    assert!(
        src.contains("rgb = w.fog_color.xyz;"),
        "wdl.wgsl: the hull no longer paints the flat fog colour (the reference's own start-0 / \
         end-1.0 fog pair, saturated beyond one yard)"
    );
    assert!(
        !src.contains("w.fog_params.x") && !src.contains("w.fog_params.y"),
        "wdl.wgsl: the hull reads the SCENE fog distances again — the 33 yd overlap goes partly \
         white at low view distances (the band decision 1521 closed)"
    );
}
