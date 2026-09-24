//! Sky dome — the gradient backdrop. The real 1.12.1 client draws the sky FIRST each frame as an
//! unlit, vertex-coloured dome with depth-write OFF (apitrace WoW.5: prog 140, GL_TRIANGLE_STRIP 300
//! idx, no texture, additive); the gradient colours are the five `Light.dbc` `SkyColor` stops
//! (LightIntBand rows 2–6) plus the fog colour (row 7), decoded into `WowLighting.{sky,fog_color}`.
//!
//! **Geometry + mapping are verified** (binary `WoW.exe`: builder `0x6d0d10`, colour gen
//! `0x6d0f50`; cross-checked against the actual memcpy'd dome vertices in apitrace WoW.5 — both
//! agree). WoW's dome is a camera-centred cap of 1 apex + 5 rings × 24 segments, recentred by
//! `−cos(45°)` so the rim sits at eye level (horizon). The five stops sit one-per-ring at GEOMETRIC
//! elevations (above the horizon): **SkyColor0 @ 90° (zenith), SkyColor1 @ 16.8°, SkyColor2 @ 9.8°,
//! SkyColor3 @ 3.7°, SkyColor4 @ 1.8°**, and the **rim/horizon (0°) and below = the FOG colour**
//! (row 7) — the sky converges into the distance fog at the horizon. GL Gouraud-interpolates between
//! rings → piecewise-linear in elevation. So SkyColor0 dominates the whole upper sky and the gradient
//! is crushed into the bottom ~17°.
//!
//! We reproduce the same *result* with a camera-centred sphere scaled to just inside the far plane
//! and an unlit material that interpolates the stops **per-fragment** by the view-direction
//! elevation (tessellation-independent → no banding). Like the reference, the dome does NOT write
//! depth (the [`SkyExt::specialize`] hook) — the z-buffer stays clean over sky pixels, which is what
//! lets the far-forced glare quads (`celestial.wgsl`) be occluded per-pixel by world geometry alone.
//! The dome's own depth is likewise the far plane, pinned in the vertex stage every sky material
//! shares (`sky_vertex.wgsl`; the law is in [`crate::sky_order`]), so the world always paints over
//! the backdrop no matter how the radius compares to a given piece of geometry — the WDL horizon
//! ring reaches past this shell. The dome tracks the camera
//! *position* but stays world-aligned (identity rotation) so the gradient is fixed to the world
//! horizon regardless of look direction.
//!
//! Deferred (not in this basic dome): the day/night sky-intensity scalar that pre-scales the stops,
//! and the additive sun-azimuth glow (sun-side brightening) — both per `FUN_006d0f50`. Sun/moon discs
//! and clouds are separate later layers.

use bevy::asset::RenderAssetUsages;
use bevy::camera::Projection;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use crate::dev_state::DebugState;
use crate::lighting::WowLighting;
use crate::view::WorldCamera;
use benilla_assets::AssetSet;

/// Unlit gradient sky over a `StandardMaterial` shell — the shell only supplies `unlit` +
/// `cull_mode: None` (the dome is viewed from the inside); the extension's fragment shader ignores PBR
/// and outputs the elevation gradient.
pub type SkyMaterial = ExtendedMaterial<StandardMaterial, SkyExt>;

/// The five sky-gradient stops (zenith→horizon) + the fog/horizon colour, as shader uniforms packed
/// onto one binding (100) — same packing trick as `WowModelExt` (`terrain.rs`). WGSL struct order
/// must match this field order (each `Vec4` is 16 B, no implicit padding).
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct SkyExt {
    /// `SkyColor0` — zenith (90°).
    #[uniform(100)]
    pub(crate) sky0: Vec4,
    /// `SkyColor1` — 16.8°.
    #[uniform(100)]
    pub(crate) sky1: Vec4,
    /// `SkyColor2` — 9.8°.
    #[uniform(100)]
    pub(crate) sky2: Vec4,
    /// `SkyColor3` — 3.7°.
    #[uniform(100)]
    pub(crate) sky3: Vec4,
    /// `SkyColor4` — 1.8° (last sky ring).
    #[uniform(100)]
    pub(crate) sky4: Vec4,
    /// Fog colour (LightIntBand row 7) — the horizon (0°) and below converge into this.
    #[uniform(100)]
    pub(crate) fog: Vec4,
    /// Dawn/dusk azimuthal warp inputs (`FUN_006d0f50`): `x` = strength `S` (`WowLighting.sky_warp`,
    /// 0 = no warp / midday), `y` = sun azimuth `atan2(sun.z, sun.x)` rad (Bevy world), `zw` reserved.
    #[uniform(100)]
    pub(crate) warp: Vec4,
}

impl MaterialExtension for SkyExt {
    /// The shared sky vertex stage — the far-depth pin ([`crate::sky_order`], "The depth law").
    fn vertex_shader() -> ShaderRef {
        crate::sky_order::SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_world/shaders/sky.wgsl".into()
    }

    /// Depth-write OFF — the reference draws its whole sky without writing depth (the apitrace
    /// note in the module header, byte-confirmed: every sky element inherits `CSky::Render`'s
    /// (`0x6d4940`) depth-write-off state). Depth-TEST stays on, so terrain
    /// still occludes the dome; what this buys is a clean z-buffer over sky pixels — the glare
    /// quads (pinned to the far depth like the dome itself) are occluded per-pixel by *world*
    /// geometry only, never by the backdrop dome they must shine over. Plus the state every sky
    /// pipeline takes ([`crate::sky_order::sky_pipeline_state`]).
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = false;
        }
        crate::sky_order::sky_pipeline_state(descriptor);
        Ok(())
    }
}

/// Marker for the sky-dome entity.
#[derive(Component)]
struct Sky;

/// Sky-dome subsystem: registers the material, spawns the dome at startup, and per frame pins it to
/// the camera + pushes the time-of-day `Light.dbc` sky/fog colours into its material.
pub(crate) struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SkyMaterial>::default())
            .add_systems(Startup, setup_sky.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // The dome's stops are the resolved atmosphere, so the push belongs on the
                    // READ side of the resolve (`lighting::LightingConsumeSet`) — unordered it
                    // runs at the top of `Update` and paints last frame's palette, which on a
                    // surfacing frame is the underwater one.
                    update_sky_colors.in_set(crate::lighting::LightingConsumeSet),
                    // The dome stands down for a WMO skybox, so its gate must read the SETTLED
                    // resolve, not whichever side of it the executor picked (`crate::skybox`).
                    apply_sky_visibility
                        .after(crate::skybox::SkyboxResolve)
                        .after(crate::liquid::SubmersionVerdict),
                ),
            )
            // Camera-anchored placement runs post-propagation (the BillboardPlace slot) off the
            // camera's SAME-frame pose — the Update wiring read the last-frame camera (decision
            // 0504; sub-visible at the dome's far radius, but it's the same latent bug the near
            // glare quads made visible, so the whole camera-anchored class moved together).
            .add_systems(
                PostUpdate,
                follow_camera.in_set(crate::billboard::BillboardPlace),
            );
    }
}

fn col(c: [f32; 3]) -> Vec4 {
    Vec4::new(c[0], c[1], c[2], 1.0)
}

/// A unit UV sphere (Y-up). `cull_mode: None` makes both faces visible, so winding doesn't matter.
/// The gradient is per-fragment from the view direction, so a modest tessellation is plenty; the lower
/// hemisphere resolves to the fog colour and is covered by terrain anyway.
fn dome_mesh() -> Mesh {
    const STACKS: usize = 24;
    const SECTORS: usize = 48;
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for i in 0..=STACKS {
        let v = i as f32 / STACKS as f32;
        let phi = v * std::f32::consts::PI; // 0 (zenith) .. π (nadir)
        let (sp, cp) = phi.sin_cos();
        for j in 0..=SECTORS {
            let u = j as f32 / SECTORS as f32;
            let theta = u * std::f32::consts::TAU;
            let (st, ct) = theta.sin_cos();
            let p = [sp * ct, cp, sp * st]; // y = cos(phi): +1 zenith, −1 nadir
            positions.push(p);
            normals.push([-p[0], -p[1], -p[2]]); // inward (unused — unlit)
            uvs.push([u, v]);
        }
    }
    let mut indices: Vec<u32> = Vec::new();
    let stride = SECTORS + 1;
    for i in 0..STACKS {
        for j in 0..SECTORS {
            let a = (i * stride + j) as u32;
            let b = a + 1;
            let c = a + stride as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

fn setup_sky(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    let mesh = meshes.add(dome_mesh());
    // Seeded with the neutral-day fallback; `update_sky_colors` overwrites from Light.dbc each frame.
    let material = materials.add(SkyMaterial {
        base: StandardMaterial {
            unlit: true,
            cull_mode: None, // viewed from inside the dome
            ..default()
        },
        extension: SkyExt {
            sky0: col([0.30, 0.50, 0.85]),
            sky1: col([0.35, 0.58, 0.88]),
            sky2: col([0.55, 0.72, 0.92]),
            sky3: col([0.68, 0.80, 0.93]),
            sky4: col([0.78, 0.86, 0.95]),
            fog: col([0.55, 0.72, 0.92]),
            warp: Vec4::ZERO, // update_sky_colors fills S + sun azimuth each frame
        },
    });
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(material),
        Transform::default(),
        Sky,
    ));
}

/// Pin the dome to the camera and scale it to just inside the far plane (the radius only has to keep
/// the dome unclipped — occlusion is `sky_vertex.wgsl`'s far-depth pin). World-aligned (identity rotation)
/// → the gradient stays fixed to the world horizon regardless of camera look direction.
#[allow(clippy::type_complexity)]
fn follow_camera(
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    mut dome: Query<(&mut Transform, &mut GlobalTransform), (With<Sky>, Without<WorldCamera>)>,
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
    tf.scale = Vec3::splat(far * 0.9);
    // Propagation already ran this frame — the direct global write is what renders.
    *gt = GlobalTransform::from(*tf);
}

/// Hide/show the dome from the debug panel's "disable sky dome" toggle — and stand it down entirely
/// while a **WMO skybox** owns the backdrop ([`crate::skybox`]), or while the eye is **submerged**:
/// a building whose group asks for its MOSB model replaces this gradient, it does not layer over it,
/// and underwater the reference skips the whole sky pass (the scene driver's `0x6812a4` submerged
/// test gates `CSky::Render 0x6d4940` — stars, discs, gradient band and cloud dome together).
/// With the dome hidden the `ClearColor`
/// backdrop (the row-7 fog colour — submerged, the murk — or black via "black backdrop") shows through.
///
/// The skybox gate is its **weight**, not the resolve: below the 0.99 threshold the dome still
/// draws and the painted sky alpha-blends OVER it — the visible half of the 4-second crossfade
/// ([`crate::skybox::SkyboxWeight`]).
fn apply_sky_visibility(
    debug: Res<DebugState>,
    skybox: Res<crate::skybox::SkyboxWeight>,
    underwater: Res<crate::liquid::Underwater>,
    mut dome: Query<&mut Visibility, With<Sky>>,
) {
    let Ok(mut vis) = dome.single_mut() else {
        return;
    };
    let want =
        if debug.lighting.disable_sky_dome || skybox.replaces_celestial() || underwater.0.any() {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
    if *vis != want {
        *vis = want;
    }
}

/// Push the time-of-day sky stops + fog colour (`WowLighting`, sampled from `Light.dbc`) into the dome
/// material.
fn update_sky_colors(
    light: Res<WowLighting>,
    dome: Query<&MeshMaterial3d<SkyMaterial>, With<Sky>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    let Ok(handle) = dome.single() else {
        return;
    };
    // Byte-quantized (`quant255` — Light.dbc stops are byte colors) and write-gated: the stops
    // drift continuously with time-of-day, but an `Assets::get_mut` alone re-uploads the material
    // every frame; gated, the dome only pays when a display-visible band actually moves.
    let sky: Vec<Vec4> = light
        .sky
        .iter()
        .map(|c| col(benilla_assets::quant255(*c)))
        .collect();
    let f = benilla_assets::quant255(light.fog_color);
    // `fog.w` unused by the shader (the old raw-vs-linearised A/B is gone — GAMMA LANE).
    let fog = Vec4::new(f[0], f[1], f[2], 0.0);
    // Dawn/dusk warp: strength S + the sun's compass azimuth (Bevy world `atan2(z, x)` of the
    // camera→sun direction). The warp's glow table is symmetric about this bearing, so the sun-facing
    // dome quarter warms and the opposite side desaturates. S=0 (all midday / highlightSky=0 zones) →
    // the shader leaves the gradient untouched. Azimuth/strength gate at 1/4096 — sub-visible
    // steps of the warp's smooth glow table.
    let s = light.celestial_dir;
    let sun_az = s.z.atan2(s.x);
    let warp = Vec4::new(
        benilla_assets::quantize(light.sky_warp, 4096.0),
        benilla_assets::quantize(sun_az, 4096.0),
        0.0,
        0.0,
    );
    benilla_assets::write_gated(
        &mut materials,
        &handle.0,
        |m| {
            m.extension.sky0 != sky[0]
                || m.extension.sky1 != sky[1]
                || m.extension.sky2 != sky[2]
                || m.extension.sky3 != sky[3]
                || m.extension.sky4 != sky[4]
                || m.extension.fog != fog
                || m.extension.warp != warp
        },
        |m| {
            m.extension.sky0 = sky[0];
            m.extension.sky1 = sky[1];
            m.extension.sky2 = sky[2];
            m.extension.sky3 = sky[3];
            m.extension.sky4 = sky[4];
            m.extension.fog = fog;
            m.extension.warp = warp;
        },
    );
}
