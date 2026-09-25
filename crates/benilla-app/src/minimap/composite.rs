//! The WMO-interior minimap's offscreen composite, the reference's own pipeline: tiles drawn into
//! a 256 × 256 target (`0x4eda42`/`0x4eda48`) under an ortho half-extent of 1.5 × the view radius
//! (`0x4ec130`, the factor at `0x80308c`), cleared colour-only to opaque black (unpacked at
//! `0x59b910`), blending off and alpha-tested at `GEQUAL 224/255`; the blit shows the middle
//! two-thirds, netting 1.0 × radius on screen.
//!
//! The alpha test and the target's resolution only work together: the test keeps the clear from
//! showing where two tiles meet at a shared wall, and the coarse target (0.703 yd per texel at the
//! default indoor zoom, against the bake's 0.5) minifies before the test, so a one-texel gap in
//! the bake is never sampled. Alpha-tested straight to the screen, those gaps read as black lines.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::portrait::MINIMAP_COMPOSITE_LAYER;
use crate::ui_pass::UiQuadMaterial;

/// The target's edge in texels (`0x4eda42`). A fidelity constant, not a quality knob: a larger
/// target samples the bake's one-texel gaps and draws them as hairlines.
pub(super) const RT_SIZE: u32 = 256;

/// The target's ortho half-extent as a multiple of the on-screen view radius (`0x80308c`).
pub(super) const RT_HALF_EXTENT_SCALE: f32 = 1.5;

/// The fraction of the target's edge the blit shows, netting 1.0 × radius on screen.
pub(super) const RT_BLIT_FRACTION: f32 = 1.0 / RT_HALF_EXTENT_SCALE;

/// One tile in target space: y-up, origin at the target's centre (the player), `RT_SIZE / 2`
/// units to an edge.
pub(super) struct CompositeTile {
    pub(super) texture: Handle<Image>,
    /// Centre, in target units.
    pub(super) center: Vec2,
    /// Size, in target units.
    pub(super) size: Vec2,
    /// The placement yaw, clockwise on screen; negated into the camera's y-up frame.
    pub(super) rotation: f32,
    /// Ascending draws later (on top), matching the group sort.
    pub(super) order: usize,
}

/// This frame's interior composite, filled by [`super::emit_minimap`] for [`drive_composite`].
#[derive(Resource, Default)]
pub(super) struct MinimapComposite {
    /// `false` outdoors or with no minimap widget: the camera is off and the pool empties.
    pub(super) active: bool,
    pub(super) tiles: Vec<CompositeTile>,
}

/// The composite's durable pieces: target, camera, shared quad, material cache and entity pool.
#[derive(Resource)]
pub(super) struct CompositeRig {
    /// The render target, sampled by the blit quad.
    pub(super) image: Handle<Image>,
    camera: Entity,
    quad: Handle<Mesh>,
    /// Materials by tile texture; a building's tile set is small, so this fills once per building.
    materials: bevy::platform::collections::HashMap<AssetId<Image>, Handle<UiQuadMaterial>>,
    pool: Vec<Entity>,
}

/// Build the target, its camera and the shared quad once, at startup.
///
/// The target is float and un-encoded, like the portrait booths': the UI pass does its one sRGB
/// encode at the end, and un-encoded 8-bit values band. The tile draw writes the texel un-encoded
/// (the [`UiQuad::alpha_test`](crate::ui_pass::UiQuad::alpha_test) arm in `ui_quad.wgsl`) and the
/// blit quad encodes it like any other UI texture.
pub(super) fn setup_composite(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let mut image = Image::new_fill(
        Extent3d {
            width: RT_SIZE,
            height: RT_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0; 8],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    let image = images.add(image);

    // A 1×1 quad at the origin, every tile a Transform of it. `pipe_warm` warms this same mesh (its
    // layout is a pipeline key axis), so both take it from one builder.
    let quad = meshes.add(crate::ui_pass::tile_quad_mesh());

    let camera = commands
        .spawn((
            Name::new("minimap composite camera"),
            Camera2d,
            // No MSAA, like the reference's plain RGBA8 target: multisampling an alpha-tested edge
            // gives partial coverage over the black clear, the grey seam this target removes.
            bevy::render::view::Msaa::Off,
            RenderLayers::layer(MINIMAP_COMPOSITE_LAYER),
            RenderTarget::Image(image.clone().into()),
            Camera {
                // The reference's clear: colour only, opaque black (`0x4ec8ef`, mask 1). Anything
                // the bake leaves uncovered reads black, the exterior group's footprint included.
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                // Off until indoors; `drive_composite` owns the switch.
                is_active: false,
                ..default()
            },
            // One target unit = one target texel, origin at the centre.
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::Fixed {
                    #[allow(clippy::cast_precision_loss)] // 256 is exact in f32
                    width: RT_SIZE as f32,
                    #[allow(clippy::cast_precision_loss)]
                    height: RT_SIZE as f32,
                },
                ..OrthographicProjection::default_2d()
            }),
        ))
        .id();

    commands.insert_resource(CompositeRig {
        image,
        camera,
        quad,
        materials: bevy::platform::collections::HashMap::default(),
        pool: Vec::new(),
    });
}

/// Draw [`MinimapComposite`] on the composite camera: one pooled entity per tile, the shared quad
/// under a Transform with its texture's cached material.
pub(super) fn drive_composite(
    mut composite: ResMut<MinimapComposite>,
    rig: Option<ResMut<CompositeRig>>,
    mut commands: Commands,
    mut materials: ResMut<Assets<UiQuadMaterial>>,
    mut cameras: Query<&mut Camera>,
) {
    let Some(mut rig) = rig else { return };
    if let Ok(mut cam) = cameras.get_mut(rig.camera) {
        let want = composite.active;
        if cam.is_active != want {
            cam.is_active = want;
        }
    }
    if !composite.active {
        for e in rig.pool.drain(..) {
            commands.entity(e).despawn();
        }
        composite.tiles.clear();
        return;
    }

    // Ascending order draws later; z stays within -50..50 whatever the tile count.
    let count = composite.tiles.len().max(1);
    for (i, tile) in composite.tiles.iter().enumerate() {
        let material = rig
            .materials
            .entry(tile.texture.id())
            .or_insert_with(|| {
                materials.add(UiQuadMaterial::interior_tile(
                    tile.texture.clone(),
                    super::INTERIOR_TILE_ALPHA_REF,
                ))
            })
            .clone();
        #[allow(clippy::cast_precision_loss)] // tile counts are in the tens
        let z = -50.0 + (tile.order as f32 / count as f32) * 100.0;
        let transform = Transform {
            translation: tile.center.extend(z),
            // The screen path's angle is clockwise-on-screen; the camera's frame is y-up.
            rotation: Quat::from_rotation_z(-tile.rotation),
            scale: tile.size.extend(1.0),
        };
        match rig.pool.get(i) {
            Some(&e) => {
                commands.entity(e).insert((
                    Mesh2d(rig.quad.clone()),
                    MeshMaterial2d(material),
                    transform,
                ));
            }
            None => {
                let e = commands
                    .spawn((
                        Mesh2d(rig.quad.clone()),
                        MeshMaterial2d(material),
                        transform,
                        RenderLayers::layer(MINIMAP_COMPOSITE_LAYER),
                    ))
                    .id();
                rig.pool.push(e);
            }
        }
    }
    let used = composite.tiles.len();
    for e in rig.pool.drain(used..) {
        commands.entity(e).despawn();
    }
    composite.tiles.clear();
}
