//! The visible cloud layer, the reference's cloud dome (`0x6d0530` mesh, `0x6cfb00` coloring,
//! `0x58ac70` upload): a 12-ring cap from the pole to the 45° rim, faded at the rim, drawn last in
//! the sky pass so clouds blend over a setting sun. [`crate::sky_order::CLOUDS_BIAS`] holds that
//! place; the shared far-depth pin puts it behind all terrain.

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

/// The cloud dome material: the kernel's colored texels, blended premultiplied in gamma.
pub type CloudMaterial = ExtendedMaterial<StandardMaterial, CloudExt>;

/// The colored cloud texture. The color math is CPU-side as in the reference, so the texels are
/// raw gamma bytes in a non-sRGB texture.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct CloudExt {
    /// The colored tile, alpha the coverage, re-uploaded each regen (`0x58ac70`).
    #[texture(100)]
    #[sampler(101)]
    pub(crate) texels: Handle<Image>,
}

impl MaterialExtension for CloudExt {
    /// The shared sky vertex stage, which pins every sky vertex to the far depth.
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

#[derive(Component)]
pub(super) struct CloudDome;

/// The tick's upload target: the image the dome samples, and the material to touch on each upload
/// so its bind group picks up the new texture.
#[derive(Resource)]
pub(super) struct CloudLayer {
    pub(crate) image: Handle<Image>,
    pub(crate) material: Handle<CloudMaterial>,
}

/// Ring co-latitudes in units of π (`0x811570`), from the pole to the 45° rim.
const RING_COLAT: [f32; 12] = [
    0.0, 0.025, 0.05, 0.075, 0.10, 0.125, 0.15, 0.175, 0.205, 0.23, 0.245, 0.25,
];
/// Per-ring vertex alpha (`0x8115a0`): opaque inner 9 rings, half ring 10, transparent rim.
#[rustfmt::skip]
const RING_ALPHA: [f32; 12] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 128.0 / 255.0, 0.0, 0.0,
];
/// Azimuth steps per ring (`0x6d0530`).
const AZ_STEPS: usize = 16;

/// The reference dome (`0x6d0530`): recentred by `−cos(π/4)` so the rim sits at eye level, with
/// the coverage sampler's polar UV so the drawn cloud and the glare's occlusion line up.
fn cloud_dome_mesh() -> Mesh {
    let shift = std::f32::consts::FRAC_PI_4.cos();
    let mut positions = Vec::with_capacity(12 * AZ_STEPS);
    let mut normals = Vec::with_capacity(12 * AZ_STEPS);
    let mut uvs = Vec::with_capacity(12 * AZ_STEPS);
    let mut colors = Vec::with_capacity(12 * AZ_STEPS);
    for (ring, (&colat, &alpha)) in RING_COLAT.iter().zip(RING_ALPHA.iter()).enumerate() {
        let phi = colat * std::f32::consts::PI;
        let v_r = ring as f32 / 24.0; // the polar UV radius
        for j in 0..AZ_STEPS {
            let az = j as f32 / AZ_STEPS as f32 * std::f32::consts::TAU;
            let (sa, ca) = az.sin_cos();
            // Recentred so the rim sits at eye level, then pushed to unit radius.
            let dir = Vec3::new(phi.sin() * sa, phi.cos() - shift, phi.sin() * ca).normalize();
            positions.push([dir.x, dir.y, dir.z]);
            normals.push([dir.x, dir.y, dir.z]);
            // u follows world x and v world z, as the sampler's column and row do.
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
        // Not sRGB: the texels are gamma bytes and must sample raw.
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    ));
    let material = materials.add(CloudMaterial {
        base: StandardMaterial {
            unlit: true,
            cull_mode: None, // viewed from inside
            alpha_mode: AlphaMode::Premultiplied,
            // The sky pass's last draw: over the discs, under the rain and the glare.
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

/// Hides the dome with the rest of the sky: under a WMO skybox past weight 0.99, as `CSky::Render`
/// skips all six sky elements on one flag (`0x6d4a3b`), and while submerged, as the scene skips
/// `CSky::Render` (`0x6812a4`).
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

/// Centres the dome on the camera at `far·0.87`, inside the sky dome. The radius only sizes it on
/// screen; occlusion comes from the far-depth pin in `sky_vertex.wgsl`.
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
        // Unit radius everywhere; the pole points straight up and the rim sits at y = 0.
        for p in pos {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 1.0).abs() < 1e-5, "radius {r}");
        }
        assert!((pos[0][1] - 1.0).abs() < 1e-6);
        let rim = pos[11 * AZ_STEPS][1];
        assert!(rim.abs() < 1e-6, "rim y {rim}");
    }
}
