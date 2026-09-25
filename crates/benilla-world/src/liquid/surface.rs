//! The animated liquid surfaces: one shared material per (kind, [`LiquidPath`], scroll) over a
//! `texture_2d_array` of the kind's frames, the two spawn paths (an ADT chunk's MCLQ, a WMO
//! group's MLIQ) and the flat mesh. The 24 fps frame flip and the lava scroll run in the shader
//! off the wall clock, so no CPU mutates a material; a deterministic run bakes the clock off
//! (frame 0, scroll 0). Day and night follow server time.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;

use super::query::{wet_footprint, FoamPatch, LiquidSource, WmoPool};
use crate::collision::liquid_layers;
use crate::lighting::WATER_SHININESS;
use avian3d::prelude::{Collider, RigidBody};
use benilla_assets::coords::wow_to_bevy;
use benilla_assets::materials::{LiquidExt, LiquidMaterial};
use benilla_assets::LockRecover;
use benilla_assets::{liquid_frame_array, RenderConfig, WorldAssets};
use benilla_formats::{read_texture_mip_chain, BlpMipChain, LiquidKind, LiquidMesh};

/// The shared liquid materials by [`LiquidKey`]; absent without client data.
#[derive(Resource, Default)]
pub(crate) struct LiquidAssets {
    materials: HashMap<LiquidKey, LiquidEntry>,
}

/// Which of the reference's three liquid renderers draws a surface: `0x6b62e0` sends the type
/// nibble to a category, and category 0 (nibbles 0/4/8) splits on the group's `MOGP.flags & 0x48`.
/// Magma and slime share `0x6b68f0` (WMO) / `0x68dca0` (ADT) whatever the flags, so for them the
/// path only picks the fog block. Fog is per pass: the WMO liquid pass re-submits the smoothed
/// interior fog block (`0x6b6323`–`0x6b6342`) under the same `[0xca7f00]` gate as the WMO geometry,
/// so a pool fogs like its walls, while ADT liquid submits none and draws under the scene block.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum LiquidPath {
    /// An ADT chunk's MCLQ (`0x6851b0` river, `0x685010` ocean): two stages, the depth swatch at
    /// `tc0 = (0.5, LUT[depthByte])` and the sheet at `tc1 = (col·¼, row·¼)`; no vertex colour.
    Adt,
    /// A WMO group's MLIQ, exterior or exterior-lit (`MOGP.flags & 0x48 != 0`), `0x6b6630`: one
    /// stage, a 9-float vertex with an up normal and a colour dword, `MapObjExtWater0.bls` bound at
    /// `0x6b6654`.
    WmoExterior,
    /// A WMO group's MLIQ, a true interior (`MOGP.flags & 0x48 == 0`), `0x6b6420`: one stage, a
    /// 6-float vertex with no normal, lighting off, no pixel program.
    WmoInterior,
}

impl LiquidPath {
    /// A WMO group's own class, as the reference's `[owner+0x10] & 0x48` test reads it.
    pub(crate) fn wmo(interior: bool) -> Self {
        if interior {
            Self::WmoInterior
        } else {
            Self::WmoExterior
        }
    }

    fn interior_fog(self) -> bool {
        matches!(self, Self::WmoInterior)
    }

    /// The renderer selector `liquid.wgsl` branches on (`LiquidParams.path.x`).
    fn shader_id(self) -> f32 {
        match self {
            Self::Adt => 0.0,
            Self::WmoExterior => 1.0,
            Self::WmoInterior => 2.0,
        }
    }

    /// Does this arm's pool take its body colour from its own MOMT `diffColor`? Only the interior
    /// one: the exterior reads a live `Light.dbc` band, and ADT liquid has no MOMT.
    fn takes_material_body_color(self) -> bool {
        matches!(self, Self::WmoInterior)
    }
}

/// Which shared material a surface takes. The scroll is per nibble and per path, which
/// [`LiquidKind`] collapses (2 and 6 are both `Magma`), so it keys the material too.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct LiquidKey {
    kind: LiquidKind,
    path: LiquidPath,
    /// This surface takes the animated stage-0 texture matrix ([`scrolls`]).
    scroll: bool,
}

/// Does a WMO MLIQ surface of this type nibble take the reference's animated stage-0 texture
/// matrix, the lava/slime scroll? Only 6 and 7: the WMO magma/slime kernel `0x6b68f0` gates on
/// `and esi,0xc; cmp esi,4` over the `0x6ba970` nibble and builds an identity but for element 13,
/// the v-translate. Nibbles 2 and 3 reach the same kernel and stay still (the Great Forge is 2,
/// Blackrock 6). ADT liquid never scrolls, though open-world lava is nibble 6: its kernel
/// `0x68dca0` zeroes element 13 at `0x68dda1`, so the caller gates the path. The phase runs off
/// the reference's uptime clock; only its rate and period are reproducible.
fn scrolls(nibble: u8) -> bool {
    matches!(nibble, 6 | 7)
}

struct LiquidEntry {
    material: Handle<LiquidMaterial>,
}

impl LiquidAssets {
    /// The shared material for a kind, renderer and scroll, if one was built and its frames loaded.
    pub(crate) fn material(
        &self,
        kind: LiquidKind,
        path: LiquidPath,
        scroll: bool,
    ) -> Option<Handle<LiquidMaterial>> {
        self.materials
            .get(&LiquidKey { kind, path, scroll })
            .map(|e| e.material.clone())
    }
}

/// Marks a spawned liquid surface: one per MCNK liquid layer or WMO group pool.
#[derive(Component)]
pub(crate) struct LiquidSurface;

/// `WOW_NO_LIQUID`: hide every liquid surface and only the surface, since the swim grid, foam and
/// sound ride sibling components. An override run after both per-frame `Visibility` owners (the
/// exterior cull for ADT surfaces, `apply_model_visibility` for WMO pools), so it wins the frame.
pub(super) fn hide_liquid_surfaces(mut surfaces: Query<&mut Visibility, With<LiquidSurface>>) {
    for mut vis in &mut surfaces {
        if *vis != Visibility::Hidden {
            *vis = Visibility::Hidden;
        }
    }
}

/// The ambient loop's sound-class nibble, resolved through `SoundWaterType.dbc` (`0x54e0a0`), on
/// every liquid surface; the driver reads the geometry off the same entity's `WaterChunkInfo`.
#[derive(Component)]
pub(crate) struct LiquidSoundSource {
    /// The surface's sound-class nibble (`class = n & 3`, `FluidSpeed = n & 0xc`).
    pub(crate) nibble: u8,
}

/// Spawn an ADT tile's liquid surfaces into `entities`, to despawn with the tile.
pub(crate) fn spawn_liquids<'a>(
    commands: &mut Commands,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
    liquid_assets: Option<&LiquidAssets>,
    meshes: &mut Assets<Mesh>,
    entities: &mut Vec<Entity>,
) {
    let Some(liquid) = liquid_assets else {
        return;
    };
    for lq in liquids {
        // The scene fog (the ADT passes submit none and draw under the scene submit `0x66ff20`),
        // and never a scroll, though ADT magma is nibble 6: its kernel zeroes the v-translate.
        let Some(material) = liquid.material(lq.kind, LiquidPath::Adt, false) else {
            continue; // this kind's frames failed to load (warned at setup)
        };
        let info = wet_footprint(lq, &Transform::IDENTITY, LiquidSource::AdtChunk);
        let foam = !lq.kind.is_fullbright(); // white surf is a water thing
        entities.push(
            commands
                .spawn((
                    Mesh3d(meshes.add(liquid_bevy_mesh(lq, None))),
                    MeshMaterial3d(material),
                    Transform::IDENTITY,
                    LiquidSurface,
                    // Exterior scene: the ADT liquid producer `0x683ab0` is called only from the
                    // per-window populate `0x682fa0`, like ADT terrain (`0x683bf0`) and doodads
                    // (`0x683700`), one entity per MCNK layer. The exterior cull is its only
                    // `Visibility` writer.
                    crate::exterior_cull::ExteriorScene,
                    info,
                    LiquidSoundSource {
                        nibble: lq.sound_nibble,
                    },
                ))
                .id(),
        );
        if foam {
            commands
                .entity(*entities.last().expect("just pushed"))
                .insert(FoamPatch);
        }
        // The waterline for the camera sweep under `cameraWaterCollision`; nothing else queries it.
        if let Some(collider) = liquid_collider(lq) {
            commands
                .entity(*entities.last().expect("just pushed"))
                .insert((collider, RigidBody::Static, liquid_layers()));
        }
    }
}

/// The collision trimesh for one [`LiquidMesh`]: the render mesh's own wet-cell triangles, `None`
/// when every cell is dry. Built inline, as a layer is at most 128 triangles.
fn liquid_collider(lq: &LiquidMesh) -> Option<Collider> {
    let tris: Vec<[u32; 3]> = lq
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    if tris.is_empty() {
        return None;
    }
    let verts: Vec<Vec3> = lq.positions.iter().map(|p| wow_to_bevy(*p)).collect();
    Some(Collider::trimesh(verts, tris))
}

/// The render mesh for one [`LiquidMesh`] in its own space (absolute for MCLQ, model-local for WMO
/// liquid): a flat up normal, the tiling UVs, and the per-vertex depth `V` in UV1.x.
fn liquid_bevy_mesh(lq: &LiquidMesh, body_color: Option<[f32; 3]>) -> Mesh {
    let positions: Vec<[f32; 3]> = lq
        .positions
        .iter()
        .map(|p| wow_to_bevy(*p).to_array())
        .collect();
    let n = positions.len();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    // WoW up (0, 0, 1) is Bevy up (0, 1, 0).
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, lq.uvs.clone());
    let uv1: Vec<[f32; 2]> = lq.depths.iter().map(|&d| [d, 0.0]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1);
    // An interior pool's `MOMT.diffColor` rides the vertex colour, where the reference's interior
    // vertex carries it, keeping one material per lane; other lanes take the shader's white.
    if let Some([red, green, blue]) = body_color {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[red, green, blue, 1.0]; n]);
    }
    mesh.insert_indices(Indices::U32(lq.indices.clone()));
    mesh
}

/// Spawn a WMO group's liquid surfaces at the building's placement `transform`, pushed onto
/// `entities` to despawn with the placement. `interior` is the group's `MOGI & 0x48 == 0` class,
/// which picks the renderer and the fog block; `pool` scopes each surface to its room and floor.
pub(crate) fn spawn_wmo_liquids<'a>(
    commands: &mut Commands,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
    liquid_assets: Option<&LiquidAssets>,
    meshes: &mut Assets<Mesh>,
    transform: Transform,
    interior: bool,
    pool: WmoPool,
    // The owning root's MOMT `diffColor` table: MOMT lives in the root, not the group file.
    material_diff_color: &[[f32; 3]],
    entities: &mut Vec<Entity>,
) {
    let Some(liquid) = liquid_assets else {
        return;
    };
    let path = LiquidPath::wmo(interior);
    for lq in liquids {
        // The one path that can scroll, decided by the nibble, not the kind (`scrolls`).
        let scroll = scrolls(lq.sound_nibble);
        // The interior arm's body colour via the pool's MLIQ `materialId`; none when fullbright.
        let body_color = (path.takes_material_body_color() && !lq.kind.is_fullbright())
            .then(|| {
                lq.material_id
                    .and_then(|id| material_diff_color.get(usize::from(id)))
                    .copied()
            })
            .flatten();
        let Some(material) = liquid.material(lq.kind, path, scroll) else {
            continue; // this kind's frames failed to load (warned at setup)
        };
        if scroll {
            debug!(
                "liquid: {:?} nibble {} takes the scroll lane",
                lq.kind, lq.sound_nibble
            );
        }
        let surface = commands
            .spawn((
                Mesh3d(meshes.add(liquid_bevy_mesh(lq, body_color))),
                MeshMaterial3d(material),
                transform,
                LiquidSurface,
                // Bit 30 is the interior-fog lane (`liquid.wgsl`'s `room_fog`), written each frame
                // by the `Visibility` authority off the pool's room; spawned clear.
                bevy::mesh::MeshTag(0),
                // Every kind, the lava and slime hum too.
                LiquidSoundSource {
                    nibble: lq.sound_nibble,
                },
            ))
            .id();
        // Every kind carries the swim grid, so lava and slime swim; their damage is not modelled.
        commands.entity(surface).insert(wet_footprint(
            lq,
            &transform,
            LiquidSource::WmoGroup(pool),
        ));
        if !lq.kind.is_fullbright() {
            commands.entity(surface).insert(FoamPatch);
        }
        // The camera's waterline, model-local under the entity's placement `transform`.
        if let Some(collider) = liquid_collider(lq) {
            commands
                .entity(surface)
                .insert((collider, RigidBody::Static, liquid_layers()));
        }
        entities.push(surface);
    }
}

/// Each kind's frames, `XTextures\<dir>\<stem>.<1..=count>.blp`, as `(kind, dir, stem, count)`.
const FRAME_SETS: &[(LiquidKind, &str, &str, u32)] = &[
    (LiquidKind::Still, "river", "lake_a", 30),
    (LiquidKind::Rapids, "river", "fast_a", 16),
    (LiquidKind::Ocean, "ocean", "ocean_h", 30),
    // Fullbright: the sheet is the opaque, unlit, fogged body (`0x6b68f0`). Magma comes from both
    // the WMO pools and the ADT magma queue; the reference has no ADT queue for slime.
    (LiquidKind::Magma, "lava", "lava", 30),
    (LiquidKind::Slime, "slime", "slime", 30),
];

pub(super) fn setup_liquid(
    mut commands: Commands,
    config: Option<Res<RenderConfig>>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<LiquidMaterial>>,
) {
    let (Some(_config), Some(mut world_assets)) = (config, world_assets) else {
        return; // no client data → no terrain, so no water either
    };
    // Light, fog and the water swatches come off the shared global-light buffer, as for terrain.
    let mut assets = LiquidAssets::default();
    for &(kind, dir, stem, count) in FRAME_SETS {
        let Some((frames, frame_count)) =
            load_frame_array(&mut world_assets, &mut images, kind, dir, stem, count)
        else {
            warn!("liquid: no frames for {stem} — {kind:?} water will not render");
            continue;
        };
        // Blend per kind, over the device defaults (`0x593bf0`) every reference setter pushes and
        // pops against. Water and ocean: EGxBlend 2 (`SRC_ALPHA / INV_SRC_ALPHA`), depth write off
        // (both under the fancy-water CVar `[0xc9a324]`, which is not modelled). Magma and slime:
        // blend stays 0 (disabled) with depth test and write on (ids `0x10`/`0x12`), as the ADT
        // lava pass sets (`0x6855e2`) and the WMO arm leaves it, so they are opaque. All four
        // liquid passes turn culling off at entry (`0x59d7d8`); `glFrontFace` is never imported.
        let alpha_mode = if kind.is_fullbright() {
            AlphaMode::Opaque
        } else {
            AlphaMode::Blend
        };
        // One material per renderer and scroll the kind can take, all on one frame array; only
        // magma and slime (nibbles 6 and 7) can scroll.
        let scroll_lanes: &[bool] = if kind.is_fullbright() {
            &[false, true]
        } else {
            &[false]
        };
        for (path, &scroll) in [
            LiquidPath::Adt,
            LiquidPath::WmoExterior,
            LiquidPath::WmoInterior,
        ]
        .into_iter()
        .flat_map(|p| scroll_lanes.iter().map(move |s| (p, s)))
        {
            let material = materials.add(ExtendedMaterial {
                base: StandardMaterial {
                    // The shader does the WoW lighting.
                    unlit: true,
                    alpha_mode,
                    cull_mode: None,
                    double_sided: true,
                    // Water takes the water-pass slot of the reference's `0x483460` interleave
                    // (`sky_order::WATER_BIAS`); the opaque kinds take no rung.
                    depth_bias: if kind.is_fullbright() {
                        0.0
                    } else {
                        crate::sky_order::WATER_BIAS
                    },
                    ..default()
                },
                extension: LiquidExt {
                    frames: frames.clone(),
                    // x = fullbright (the sheet as body, still fogged); y = the ocean swatch;
                    // z = the interior fog block; w = water's sun-sheen exponent.
                    kind: Vec4::new(
                        if kind.is_fullbright() { 1.0 } else { 0.0 },
                        if kind == LiquidKind::Ocean { 1.0 } else { 0.0 },
                        if path.interior_fog() { 1.0 } else { 0.0 },
                        WATER_SHININESS,
                    ),
                    // Which of the reference's three liquid renderers `liquid.wgsl` runs.
                    path: Vec4::new(path.shader_id(), 0.0, 0.0, 0.0),
                    // x reserved; y = frame count; z = scroll; w = clock enable (0 on a
                    // deterministic run: frame 0, scroll 0).
                    anim: Vec4::new(
                        0.0,
                        frame_count as f32,
                        if scroll { 1.0 } else { 0.0 },
                        if crate::dev_state::deterministic_run() {
                            0.0
                        } else {
                            1.0
                        },
                    ),
                    light_buf: world_assets.shared_light.clone(),
                },
            });
            assets
                .materials
                .insert(LiquidKey { kind, path, scroll }, LiquidEntry { material });
        }
    }
    // Frame sets, not materials.
    info!(
        "liquid: loaded {} water frame set(s)",
        FRAME_SETS
            .iter()
            .filter(|(k, ..)| assets.material(*k, LiquidPath::Adt, false).is_some())
            .count()
    );
    commands.insert_resource(assets);
}

/// Decode frames `1..=count` with their authored mips into one repeating, anisotropic array,
/// stopping at the first missing, non-square or mis-sized frame; returns it and the count loaded.
fn load_frame_array(
    world_assets: &mut WorldAssets,
    images: &mut Assets<Image>,
    kind: LiquidKind,
    dir: &str,
    stem: &str,
    count: u32,
) -> Option<(Handle<Image>, u32)> {
    let mut frames: Vec<BlpMipChain> = Vec::new();
    let mut size = 0u32;
    for i in 1..=count {
        let path = format!("XTextures\\{dir}\\{stem}.{i}.blp");
        let Ok(chain) = read_texture_mip_chain(&mut world_assets.chain.lock_recover(), &path)
        else {
            break;
        };
        if chain.width != chain.height {
            break; // water frames are square; bail rather than build a ragged array
        }
        if size == 0 {
            size = chain.width;
        } else if chain.width != size {
            break; // a frame at a different resolution can't share the array
        }
        frames.push(chain);
    }
    if frames.is_empty() {
        return None;
    }
    let loaded = frames.len() as u32;
    // Deviation: water frames have their per-frame mean flattened (`flatten_frame_dc`), because
    // mipping turns the shipped frames' drifting means into a flicker on distant water; magma and
    // slime keep theirs, the body's intended pulse.
    let normalize_dc = !kind.is_fullbright();
    Some((images.add(liquid_frame_array(frames, normalize_dc)), loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_nibbles_six_and_seven_scroll() {
        // Blackrock's lava and Stratholme's slime.
        assert!(scrolls(6), "magma variant 6 takes the animated matrix");
        assert!(scrolls(7), "slime variant 7 takes the animated matrix");

        // Same kernel, same sheet, still static: the Great Forge (2) and Undercity's slime (3).
        assert!(!scrolls(2), "the Great Forge's lava is static");
        assert!(!scrolls(3), "nibble-3 slime is static");

        // 4 passes a bare `nibble & 0xc == 4` test but is a river variant, never the kernel's.
        assert!(!scrolls(4), "nibble 4 is lake_a, not a hazard liquid");

        // Everything else in the table, including the hole nibble.
        for n in [0u8, 1, 5, 8, 0xf] {
            assert!(!scrolls(n), "nibble {n} must not scroll");
        }
    }

    /// Only the fullbright kinds have nibbles that scroll, so only they get a scroll lane.
    #[test]
    fn only_fullbright_kinds_can_take_a_scroll_lane() {
        for &(kind, ..) in FRAME_SETS {
            let wants_lane = kind.is_fullbright();
            assert_eq!(
                wants_lane,
                [2u8, 3, 6, 7]
                    .iter()
                    .any(|&n| scrolls(n) && LiquidKind::from_nibble(n) == Some(kind)),
                "{kind:?}: scroll lane and the nibbles that can ask for one must agree"
            );
        }
    }

    /// The smallest MCLQ sheet `spawn_liquids` builds: 2×2 vertices at `at`.
    fn one_sheet(at: [f32; 3]) -> LiquidMesh {
        let [x, y, z] = at;
        LiquidMesh {
            grid: [2, 2],
            wet: vec![true],
            shared: vec![false],
            positions: vec![
                [x, y, z],
                [x + 8.0, y, z],
                [x, y + 8.0, z],
                [x + 8.0, y + 8.0, z],
            ],
            uvs: vec![[0.0, 0.0]; 4],
            depths: vec![1.0; 4],
            indices: vec![0, 1, 2, 1, 3, 2],
            sound_nibble: 0,
            material_id: None,
            kind: LiquidKind::Still,
        }
    }

    /// Only the ADT still-water material, without which `spawn_liquids` spawns nothing.
    fn adt_assets() -> LiquidAssets {
        LiquidAssets {
            materials: HashMap::from([(
                LiquidKey {
                    kind: LiquidKind::Still,
                    path: LiquidPath::Adt,
                    scroll: false,
                },
                LiquidEntry {
                    material: Handle::default(),
                },
            )]),
        }
    }

    /// An ADT surface carries `ExteriorScene` and none of the components `UnownedSceneFilter`
    /// excludes, so the exterior cull reaches it.
    #[test]
    fn an_adt_liquid_surface_is_tagged_exterior_scene() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<LiquidMaterial>();
        let sheets = [one_sheet([100.0, 100.0, 5.0])];
        let assets = adt_assets();
        let mut spawned = Vec::new();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>| {
                    let mut ents = Vec::new();
                    spawn_liquids(
                        &mut commands,
                        sheets.iter(),
                        Some(&assets),
                        &mut meshes,
                        &mut ents,
                    );
                    ents
                },
            )
            .map(|ents| spawned = ents)
            .expect("the spawn system ran");
        assert_eq!(spawned.len(), 1, "one sheet in, one surface out");
        let e = app.world().entity(spawned[0]);
        assert!(
            e.contains::<crate::exterior_cull::ExteriorScene>(),
            "an open-world liquid surface is exterior scene — untagged, the window cull never \
             queries it and the lake draws through a sealed ceiling"
        );
        assert!(
            e.contains::<LiquidSurface>(),
            "…and is still a liquid surface"
        );
        // The three exclusions in `UnownedSceneFilter`.
        assert!(!e.contains::<crate::model_render::ModelPart>());
        assert!(!e.contains::<crate::wmo_portal::WmoGroupVis>());
        assert!(!e.contains::<crate::world_unit::WorldUnit>());
    }
}

/// The scroll and fog-block rules against the real client files.
#[cfg(test)]
mod real_data {
    use super::LiquidKind;
    use benilla_formats::{parse_wmo_root, wmo_group_liquid_mesh};
    use std::collections::HashMap;

    /// The scroll gate over the shipped pools, where both behaviours share buildings: Ironforge has
    /// 9 static nibble-2 and 2 scrolling nibble-6 magma groups, Blackrock is nibble 6 throughout,
    /// Undercity's 38 slime canals are nibble 3 and still, and Stratholme's are nibble 7 and flow.
    #[test]
    fn only_the_nibble_six_and_seven_pools_scroll_in_the_shipped_data() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        // Every liquid-bearing group of a WMO, as `(nibble, kind)` → count.
        let mut census = |root_path: &str| -> HashMap<(u8, LiquidKind), usize> {
            let bytes = chain.read_file(root_path).expect("root readable");
            let root = parse_wmo_root(&bytes).expect("parse root");
            let stem = root_path
                .strip_suffix(".wmo")
                .unwrap_or(root_path)
                .to_string();
            let mut tally = HashMap::new();
            for gi in 0..root.group_count() as usize {
                let Ok(gb) = chain.read_file(&format!("{stem}_{gi:03}.wmo")) else {
                    continue;
                };
                if let Some(m) = wmo_group_liquid_mesh(&gb) {
                    *tally.entry((m.sound_nibble, m.kind)).or_default() += 1;
                }
            }
            tally
        };

        let ironforge = census("world\\wmo\\khazmodan\\cities\\ironforge\\ironforge.wmo");
        assert_eq!(
            ironforge,
            HashMap::from([
                ((2, LiquidKind::Magma), 9),
                ((4, LiquidKind::Still), 2),
                ((6, LiquidKind::Magma), 2),
            ]),
            "Ironforge's liquid census moved",
        );

        let blackrock = census("world\\wmo\\dungeon\\az_blackrock\\blackrock.wmo");
        assert_eq!(
            blackrock,
            HashMap::from([((6, LiquidKind::Magma), 2)]),
            "Blackrock Mountain's lava — the `.go xyz -7531.21 -1123.64 172.58` pin — is all nibble 6",
        );

        let undercity = census("world\\wmo\\lorderon\\undercity\\undercity.wmo");
        assert_eq!(
            undercity,
            HashMap::from([((3, LiquidKind::Slime), 38)]),
            "Undercity's slime is nibble 3 throughout",
        );

        let stratholme = census("world\\wmo\\dungeon\\ld_stratholme\\stratholme.wmo");
        assert_eq!(
            stratholme,
            HashMap::from([((7, LiquidKind::Slime), 3)]),
            "Stratholme's slime is nibble 7 throughout",
        );

        // The gate's verdict over all of it.
        assert_eq!(
            ironforge
                .iter()
                .filter(|((n, _), _)| super::scrolls(*n))
                .map(|(_, c)| c)
                .sum::<usize>(),
            2,
            "exactly 2 of Ironforge's 11 magma pools scroll",
        );
        for (label, tally) in [("blackrock", &blackrock), ("stratholme", &stratholme)] {
            assert!(
                tally.keys().all(|(n, _)| super::scrolls(*n)),
                "{label}: every pool scrolls",
            );
        }
        assert!(
            undercity.keys().all(|(n, _)| !super::scrolls(*n)),
            "Undercity's slime stands still",
        );
    }

    /// A WMO pool's fog block follows its group's `MOGI & 0x48 == 0` class: Undercity's 38 liquid
    /// groups are interior but for group 7, and Stormwind's 22 are all exterior.
    #[test]
    fn a_wmo_pools_fog_block_follows_its_groups_interior_class() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        // Which groups of a WMO carry liquid, and whether each is an interior group.
        let liquid_groups = |chain: &mut benilla_formats::Chain, root_path: &str| {
            let bytes = chain.read_file(root_path).expect("root readable");
            let root = parse_wmo_root(&bytes).expect("parse root");
            let stem = root_path
                .strip_suffix(".wmo")
                .unwrap_or(root_path)
                .to_string();
            (0..root.group_count() as usize)
                .filter_map(|gi| {
                    let gb = chain.read_file(&format!("{stem}_{gi:03}.wmo")).ok()?;
                    wmo_group_liquid_mesh(&gb)?;
                    Some((gi, root.group_infos().get(gi).is_some_and(|g| g.interior)))
                })
                .collect::<Vec<_>>()
        };

        let uc = liquid_groups(&mut chain, "world\\wmo\\lorderon\\undercity\\undercity.wmo");
        let exterior: Vec<usize> = uc.iter().filter(|(_, i)| !i).map(|(g, _)| *g).collect();
        assert_eq!(uc.len(), 38, "Undercity's liquid group count moved: {uc:?}");
        assert_eq!(
            exterior,
            vec![7],
            "exactly one Undercity liquid group is exterior (the flag is per group, not per building)",
        );

        let sw = liquid_groups(
            &mut chain,
            "world\\wmo\\azeroth\\buildings\\stormwind\\stormwind.wmo",
        );
        assert_eq!(sw.len(), 22, "Stormwind's liquid group count moved: {sw:?}");
        assert!(
            sw.iter().all(|(_, interior)| !interior),
            "Stormwind's canals and fountains are open to the sky — none takes the interior fog: {sw:?}",
        );
    }
}
