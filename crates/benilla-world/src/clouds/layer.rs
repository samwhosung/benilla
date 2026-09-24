//! The visible cloud layer — the reference's celestial-pass cloud dome (`0x6d0530` mesh,
//! `0x6cfb00` coloring, `0x58ac70` per-regen texture upload).
//!
//! The reference draws a 12-ring hemisphere strip (pole → the 45° rim, ring co-latitudes bunched
//! toward the rim, per-ring vertex alpha fading the rim out) textured by an image generated from
//! the coverage byte tile every regen, as the **last draw of its sky pass** — one squashed depth
//! slice `[0.975, 0.98]` shared by stars/discs/gradient/clouds, depth-write off, painter's order —
//! so terrain occludes the clouds
//! and the clouds blend over a setting sun. We reproduce the layering with real depth: every
//! vertex is pushed to **uniform radius** along its recentred direction (the reference's squashed
//! cap relies on the depth-range remap; at real depth its apex would sit at `0.29·r` — inside the
//! scene, clouds drawing over cliffs) and the dome is camera-centred at `far·0.87` — inside the
//! opaque sky dome (`far·0.9`), behind all terrain — with its slot in the reference's fixed order
//! held by [`crate::sky_order::CLOUDS_BIAS`]: after the discs, before the rain, before the glare
//! (the frame's last render, per the same pin).
//!
//! The strip topology is converted to a triangle list (11 bands × 32 triangles — the reference's
//! 11 separate 34-index strips; same pixels, one mesh). The coverage image is R8; the color math
//! runs per-fragment in `cloud.wgsl` from the same bytes the occlusion sampler reads.

use bevy::asset::RenderAssetUsages;
use bevy::camera::Projection;
use bevy::image::Image;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;

use crate::dev_state::DebugState;
use crate::sky_order::{sky_pipeline_state, SKY_VERTEX_SHADER};
use crate::view::WorldCamera;

use super::kernel::COLS;

/// The cloud dome material: unlit premultiplied-gamma blend over the sky, coverage from the R8
/// tile texture, palette/glow uniforms packed like `SkyExt`.
pub type CloudMaterial = ExtendedMaterial<StandardMaterial, CloudExt>;

/// The colored cloud texture (see `cloud.wgsl`). All color math already happened CPU-side in the
/// kernel's `0x6cfb00` port (gradient + glow + weather dim, in gamma bytes exactly like the
/// reference) — the material only carries the resulting RGBA image, whose texels are **raw gamma
/// values** (a non-sRGB texture, never linearised).
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct CloudExt {
    /// The live colored tile (RGBA8, 128², alpha = coverage), re-uploaded on regen — the
    /// reference's `0x58ac70` zero-copy bind of the `0x6cfb00` color buffer.
    #[texture(100)]
    #[sampler(101)]
    pub(crate) texels: Handle<Image>,
}

impl MaterialExtension for CloudExt {
    /// The shared sky vertex stage — the far-depth pin ([`crate::sky_order`], "The depth law").
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_world/shaders/cloud.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}

/// Marker for the cloud dome entity.
#[derive(Component)]
pub(super) struct CloudDome;

/// The tick system's upload target — the colored tile image the dome samples, plus the material
/// to touch on every upload (a modified `Image` gets a fresh GPU texture, and the material's
/// cached bind group must be rebuilt to see it — without the touch the dome keeps sampling the
/// first upload forever).
#[derive(Resource)]
pub(super) struct CloudLayer {
    pub(crate) image: Handle<Image>,
    pub(crate) material: Handle<CloudMaterial>,
}

/// Ring co-latitude fractions ×π (`0x811570` table, `0x6d0530`): pole → the 45° rim, bunched
/// toward the rim.
const RING_COLAT: [f32; 12] = [
    0.0, 0.025, 0.05, 0.075, 0.10, 0.125, 0.15, 0.175, 0.205, 0.23, 0.245, 0.25,
];
/// Per-ring vertex alpha (`0x8115a0`): opaque inner 9 rings, half ring 10, transparent rim.
#[rustfmt::skip]
const RING_ALPHA: [f32; 12] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 128.0 / 255.0, 0.0, 0.0,
];
/// Azimuth steps per ring (`0x6d0530`: 12 rings × 16 = 192 verts).
const AZ_STEPS: usize = 16;

/// The reference dome (`0x6d0530`, radius 1): positions recentred `−cos(π/4)` so the rim sits at
/// eye level, the polar UV `(sin·V + 0.5, cos·V + 0.5)` with `V = ring/24` — the same square
/// mapping the coverage sampler uses, so the drawn cloud and the glare occlusion co-locate.
/// Normals = the unit sky direction (feeds the sun-glow alignment).
fn cloud_dome_mesh() -> Mesh {
    let shift = std::f32::consts::FRAC_PI_4.cos();
    let mut positions = Vec::with_capacity(12 * AZ_STEPS);
    let mut normals = Vec::with_capacity(12 * AZ_STEPS);
    let mut uvs = Vec::with_capacity(12 * AZ_STEPS);
    let mut colors = Vec::with_capacity(12 * AZ_STEPS);
    for (ring, (&colat, &alpha)) in RING_COLAT.iter().zip(RING_ALPHA.iter()).enumerate() {
        let phi = colat * std::f32::consts::PI;
        let v_r = ring as f32 / 24.0; // ring·(1/12)·0.5 — the polar UV radius
        for j in 0..AZ_STEPS {
            let az = j as f32 / AZ_STEPS as f32 * std::f32::consts::TAU;
            let (sa, ca) = az.sin_cos();
            // The recentred sky direction (`cos φ − cos45°` height: rim at eye level), pushed to
            // unit radius so the whole cap sits at one distance (see the module docs).
            let dir = Vec3::new(phi.sin() * sa, phi.cos() - shift, phi.sin() * ca).normalize();
            positions.push([dir.x, dir.y, dir.z]);
            normals.push([dir.x, dir.y, dir.z]);
            // u tracks world x, v tracks world z — matching the sampler's (col ← x, row ← z).
            uvs.push([sa * v_r + 0.5, ca * v_r + 0.5]);
            colors.push([1.0, 1.0, 1.0, alpha]);
        }
    }
    // The reference's 11 band strips (34 indices each), as a triangle list.
    let mut indices = Vec::with_capacity(11 * AZ_STEPS * 6);
    for ring in 0..11u32 {
        for j in 0..AZ_STEPS as u32 {
            let jn = (j + 1) % AZ_STEPS as u32;
            let a = ring * AZ_STEPS as u32 + j;
            let b = (ring + 1) * AZ_STEPS as u32 + j;
            let c = ring * AZ_STEPS as u32 + jn;
            let d = (ring + 1) * AZ_STEPS as u32 + jn;
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub(super) fn setup_cloud_layer(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<CloudMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let image = images.add(Image::new(
        Extent3d {
            width: COLS as u32,
            height: COLS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0; COLS * COLS * 4], // fully transparent until the field primes
        // NON-sRGB: the texels are the kernel's gamma bytes; sampling must return them raw.
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    ));
    let material = materials.add(CloudMaterial {
        base: StandardMaterial {
            unlit: true,
            cull_mode: None, // viewed from inside
            alpha_mode: AlphaMode::Premultiplied,
            // The sky pass's last draw — over the discs, under the rain and the glare (the
            // reference's fixed order; see the ladder in `sky_order`).
            depth_bias: crate::sky_order::CLOUDS_BIAS,
            ..default()
        },
        extension: CloudExt {
            texels: image.clone(),
        },
    });
    commands.spawn((
        Mesh3d(meshes.add(cloud_dome_mesh())),
        MeshMaterial3d(material.clone()),
        Transform::default(),
        CloudDome,
    ));
    commands.insert_resource(CloudLayer { image, material });
}

/// The sky-dome visibility toggle (the clouds hide with the rest of the sky). All color state
/// lives in the texture — the kernel's `0x6cfb00` port owns it.
///
/// A WMO skybox ([`crate::skybox`]) hides the clouds too: `CSky::Render`'s one shared boolean skips
/// **all six** sky elements together (`0x6d4a3b test edi,edi; je`), so the painted art is the whole
/// sky rather than a backdrop the procedural clouds keep drawing over. The art carries its own cloud
/// banks; layering ours on top lifted the painted zenith out of its near-black. The gate is the
/// slot's **weight** past 0.99 ([`crate::skybox::SkyboxWeight`]): through the body of the 4-second
/// crossfade the clouds keep drawing and the painted sky alpha-blends over them.
///
/// A **submerged eye** hides them the same way: the scene driver's `0x6812a4` submerged test skips
/// the whole `CSky::Render` call — from under the surface there is no sky, only the murk.
pub(super) fn apply_cloud_visibility(
    debug: Res<DebugState>,
    skybox: Res<crate::skybox::SkyboxWeight>,
    underwater: Res<crate::liquid::Underwater>,
    mut dome: Query<&mut Visibility, With<CloudDome>>,
) {
    if let Ok(mut vis) = dome.single_mut() {
        *vis =
            if debug.lighting.disable_sky_dome || skybox.replaces_celestial() || underwater.0.any()
            {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
    }
}

/// Pin the dome to the camera at `far·0.87` — inside the opaque sky dome (`far·0.9`), sorted after
/// the disc shells so the transparent pass draws the clouds over a setting sun, the reference's
/// depth-band layering. The radius sets the dome's *screen* geometry only: occlusion against the
/// world is the far depth `sky_vertex.wgsl` pins (`sky_order`, "The depth law"), not this shell —
/// the WDL horizon reaches past it (0588).
#[allow(clippy::type_complexity)]
pub(super) fn follow_cloud_dome(
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut dome: Query<
        (&mut Transform, &mut GlobalTransform),
        (With<CloudDome>, Without<WorldCamera>),
    >,
) {
    let Some((cam_gt, proj)) = cam.iter().next() else {
        return;
    };
    let Ok((mut tf, mut gt)) = dome.single_mut() else {
        return;
    };
    let far = match proj {
        Projection::Perspective(p) => p.far,
        _ => 3000.0,
    };
    tf.translation = cam_gt.translation();
    tf.rotation = Quat::IDENTITY;
    tf.scale = Vec3::splat(far * 0.87);
    *gt = GlobalTransform::from(*tf);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dome is the reference's `0x6d0530` build: 192 verts (12 rings × 16), the 11 band
    /// strips as 352 triangles, rim at eye level (y = 0 after the −cos45° recentre), UV radius
    /// growing ring-linear to 11/24.
    #[test]
    fn dome_matches_the_reference_build() {
        let mesh = cloud_dome_mesh();
        assert_eq!(mesh.count_vertices(), 192);
        let Some(Indices::U32(idx)) = mesh.indices() else {
            panic!("u32 indices")
        };
        assert_eq!(idx.len(), 11 * AZ_STEPS * 6); // 11 strips × 32 tris
        let pos = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .unwrap()
            .as_float3()
            .unwrap();
        // Every vertex sits at unit radius (the uniform-distance push); the pole points straight
        // up and the rim (colat 0.25π, recentred to eye level) stays at y = 0.
        for p in pos {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 1.0).abs() < 1e-5, "radius {r}");
        }
        assert!((pos[0][1] - 1.0).abs() < 1e-6);
        let rim = pos[11 * AZ_STEPS][1];
        assert!(rim.abs() < 1e-6, "rim y {rim}");
    }
}
