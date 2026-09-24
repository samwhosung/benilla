//! The WMO-interior minimap's **offscreen composite** — the client's own compositing pipeline
//! (decision 1466).
//!
//! Indoors the reference does not draw the group tiles onto the screen. It draws them into a fixed
//! **256 × 256** render target created once at `0x4eda42`/`0x4eda48`, under an ortho half-extent of
//! **1.5 × the view radius** (`0x4ec130`, the `× 1.5` at `0x80308c`), cleared **colour-only** to
//! opaque black (the packed `0xAARRGGBB` unpacked at `0x59b910`), with **blending disabled** and an
//! **alpha test** at `GEQUAL 224/255` — then blits the **middle two-thirds** of that target, which
//! is what nets the `1.0 × radius` on screen.
//!
//! **Both halves of that are load-bearing, and they only work together.** The alpha test is what
//! stops two group tiles that meet along a shared wall from leaving the clear colour between them
//! (blended, each contributes a filtered partial edge and up to 25% of the black shows through —
//! B141's "odd black lines", at *every* joint). But the target's resolution is what stops the
//! **genuine** gaps in the bake from reading: `3 · radius / 256` model-yd per texel is 0.703 yd at
//! the default indoor zoom against the tiles' authored 0.5 — a ~1.4× **minification before the
//! test**, in which a one-texel bake gap is simply never sampled. Drawing the tiles alpha-tested
//! straight to the screen, where we rasterise ~1.85× *finer* than the client ever did, turns those
//! same gaps from a soft grey line into a hard black one — measured, and the reason this module
//! exists rather than just the render state (1466's "why part 1 alone makes it worse").
//!
//! Mechanically: [`super::emit_minimap`]'s interior branch fills [`MinimapComposite`] with this
//! frame's tiles in **target space** instead of pushing them at the screen; [`drive_composite`]
//! materialises them as pooled quads on the composite camera's own render layer; and the interior
//! branch draws ONE screen quad sampling the target, masked to the minimap circle as usual.
//!
//! The tiles ride a **shared unit-quad mesh and a per-texture material**, moving on their
//! `Transform` alone (1463's law): the composite re-aims every frame the player walks, and
//! rebuilding ~50 meshes a frame to express a pan is exactly the churn that pass avoids.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ClearColorConfig, RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::portrait::MINIMAP_COMPOSITE_LAYER;
use crate::ui_pass::UiQuadMaterial;

/// The composite target's edge, in texels — the client's `mov edx,0x100` at `0x4eda42`. It is a
/// **fidelity constant, not a quality knob**: raising it re-opens the hairlines this module exists
/// to close, because the bake's own one-texel gaps stop being sub-texel (see the module docs).
pub(super) const RT_SIZE: u32 = 256;

/// The composite's ortho half-extent as a multiple of the on-screen view radius (`0x80308c`). The
/// target holds 1.5 × what is shown; the blit takes the middle two-thirds back out.
pub(super) const RT_HALF_EXTENT_SCALE: f32 = 1.5;

/// The fraction of the target's edge the blit shows — `1 / RT_HALF_EXTENT_SCALE`, i.e. the middle
/// two-thirds, which is what nets `1.0 × radius` on screen.
pub(super) const RT_BLIT_FRACTION: f32 = 1.0 / RT_HALF_EXTENT_SCALE;

/// One tile to composite, already in **target space**: the camera's y-up units, origin at the
/// target's centre (= the player), `RT_SIZE / 2` units to an edge.
pub(super) struct CompositeTile {
    pub(super) texture: Handle<Image>,
    /// Centre, in target units.
    pub(super) center: Vec2,
    /// Size, in target units.
    pub(super) size: Vec2,
    /// The placement yaw, as a **clockwise-on-screen** angle (what the screen path uses); negated
    /// on the way into the camera's y-up frame.
    pub(super) rotation: f32,
    /// Draw order within the composite — ascending draws later (on top), matching the group sort.
    pub(super) order: usize,
}

/// This frame's interior composite: the tiles to draw and whether the composite is live at all.
/// Written by [`super::emit_minimap`], consumed by [`drive_composite`].
#[derive(Resource, Default)]
pub(super) struct MinimapComposite {
    /// `false` = outdoors (or no minimap widget): the camera is switched off and the pool retires.
    pub(super) active: bool,
    pub(super) tiles: Vec<CompositeTile>,
}

/// The composite's durable pieces: the target image, its camera, the shared quad mesh, the
/// per-texture material cache and the entity pool.
#[derive(Resource)]
pub(super) struct CompositeRig {
    /// The render target — sampled by the blit quad on the screen lane.
    pub(super) image: Handle<Image>,
    camera: Entity,
    quad: Handle<Mesh>,
    /// Materials keyed by tile texture. A WMO's tile set is small and stable while you are inside
    /// it, so this fills once per building rather than per frame.
    materials: bevy::platform::collections::HashMap<AssetId<Image>, Handle<UiQuadMaterial>>,
    pool: Vec<Entity>,
}

/// Build the target, its camera and the shared quad once, at startup.
///
/// The target is **un-encoded** and float, exactly like the portrait booths' (decisions 0254/0804):
/// the UI arc composites in gamma bytes and does its one sRGB encode at the end, so a target that
/// pre-encoded would land a second encode downstream — and quantising un-encoded values to 8 bits
/// is B126's banding collapse, which a map of large flat colour fields would show as plainly as the
/// glue screens did. The tile draw therefore writes the sampled texel **un-encoded** (the
/// [`UiQuad::alpha_test`](crate::ui_pass::UiQuad::alpha_test) arm in `ui_quad.wgsl`), and the blit
/// quad encodes it on the way to the screen like any other UI texture.
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

    // A 1×1 quad centred on the origin: every tile is this mesh under its own Transform, so a
    // panning composite never rewrites a vertex buffer (1463). Its layout is a pipeline key axis
    // and pipe_warm warms the same mesh, so the builder has one author (2262).
    let quad = meshes.add(crate::ui_pass::tile_quad_mesh());

    let camera = commands
        .spawn((
            Name::new("minimap composite camera"),
            Camera2d,
            // **No MSAA** — the client's target is a plain RGBA8 surface with none, and here it is
            // load-bearing rather than cosmetic: multisampling an alpha-TESTED edge hands back
            // partial coverage, and partial coverage over an opaque black clear is precisely the
            // grey seam this whole module exists to remove. The test's whole point is that a
            // fragment is all or nothing.
            bevy::render::view::Msaa::Off,
            RenderLayers::layer(MINIMAP_COMPOSITE_LAYER),
            RenderTarget::Image(image.clone().into()),
            Camera {
                // The client's clear: colour only, opaque black (`0x4ec8ef`, mask 1). Everything
                // the bake does not cover reads as this — including, faithfully, the exterior
                // group's whole footprint, which has no authored tile at all.
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                // Off until the player is indoors; `drive_composite` owns the switch.
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

/// Materialise [`MinimapComposite`] onto the composite camera: one pooled entity per tile, each the
/// shared quad under a Transform, each wearing its texture's cached material.
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

    // Ascending order draws later. The composite camera's own ortho spans ±0.5·RT_SIZE in z by
    // default, so keep the span well inside it however many tiles a zoom level asks for.
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
