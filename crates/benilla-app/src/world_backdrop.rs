//! The world as the UI's backdrop: the world's final pass, the FFXGlow combine ([`FfxBackdrop`]),
//! is the first draw of the UI camera's main pass, straight into the UI's byte target.
//!
//! The reference's fixed-function device blends in gamma bytes. The world lane emits gamma bytes,
//! and the UI lane blends in an `Rgba8UnormSrgb` target, so every UI blend is arithmetic on the
//! gamma value (`alphaMode="ADD"` is `dst + texel·α`, clamped, as EGxBlend 3). Drawing the world
//! inside that target keeps the UI-over-world blend in bytes too; composited through the sRGB
//! swapchain it would run in linear and lighten every translucent UI pixel over the world (a docked
//! chat tab, black at α 102/255, must leave the scene at 60 %; a linear blend leaves 77.5 %).
//!
//! The world camera's own target is a size-carrier: a one-byte image at the world's render size
//! that nothing writes, kept because Bevy sizes a view's main texture from its target and the
//! camera's logical viewport rides it ([`retarget_world_camera`]).
//!
//! [`RenderScale`] renders the world at `window × scale` while the UI stays native. At exactly 2.0
//! the combine's bilinear read is a 2×2 box average, so supersampling needs no filter of its own.
//! The camera's logical viewport must not move with it ([`render_target_for`]).

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::camera::MipBias;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::window::PrimaryWindow;

use crate::ui_pass::PlayerUiCamera;
use benilla_world::ffx_glow::FfxBackdrop;
use benilla_world::view::WorldCamera;

/// The render scale, the `renderScale` CVar, not a 1.12 CVar: it scales the 3D and keeps the
/// interface native. At the default 1.0 [`render_target_for`] returns the window's own numbers.
#[derive(Resource, Clone, Copy, PartialEq, Debug)]
pub(crate) struct RenderScale(pub(crate) f32);

/// The settable range of [`RenderScale`], shared by the CVar and `$WOW_RENDER_SCALE`. Past 2 on
/// purpose: supersampling prices a pixel on a machine whose present is railed at vsync.
pub(crate) const RENDER_SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

/// The per-axis ceiling in physical px, `wgpu::Limits::default()`'s `max_texture_dimension_2d`,
/// applied to the ratio ([`render_target_for`]) so the picture shrinks rather than reshapes.
const MAX_RENDER_AXIS: u32 = 8192;

impl Default for RenderScale {
    /// `$WOW_RENDER_SCALE` overrides the default for the session only, never into `config.toml`.
    fn default() -> Self {
        let scale = std::env::var("WOW_RENDER_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map_or(1.0, |v| {
                v.clamp(*RENDER_SCALE_RANGE.start(), *RENDER_SCALE_RANGE.end())
            });
        Self(scale)
    }
}

/// The backdrop's size and the world camera's target scale factor, for a window of `px` physical
/// pixels at `window_factor`, rendered at `scale`.
///
/// 1. The ratio, never one axis, clamps to [`MAX_RENDER_AXIS`], so the image keeps the window's
///    aspect.
/// 2. `size.x / factor`, the camera's logical viewport width, is the window's logical width at any
///    `scale`: the factor derives from the size built, after the `round()`, since every pick ray is
///    denominated in it.
///
/// At `scale == 1.0` both are exact in IEEE, so the window's own numbers come back bit for bit.
fn render_target_for(px: UVec2, window_factor: f32, scale: f32) -> (UVec2, f32) {
    let px = px.max(UVec2::ONE);
    let ceiling = |axis: u32| MAX_RENDER_AXIS as f32 / axis as f32;
    let scale = scale
        .clamp(*RENDER_SCALE_RANGE.start(), *RENDER_SCALE_RANGE.end())
        .min(ceiling(px.x))
        .min(ceiling(px.y));
    let size = (px.as_vec2() * scale)
        .round()
        .as_uvec2()
        .clamp(UVec2::ONE, UVec2::splat(MAX_RENDER_AXIS));
    (size, window_factor * size.x as f32 / px.x as f32)
}

/// The texture LOD bias a render at `effective` scale owes its mipmapped textures: `log2(scale)`,
/// clamped at 0 so supersampling keeps its sharper mips.
///
/// WebGPU samplers carry no LOD bias, so it rides [`MipBias`] into the view uniform, which the PBR
/// lane applies and our own shaders must too (`terrain.wgsl`, `static_gx.wgsl`, `liquid.wgsl`, the
/// coverage re-sample in `wow_model.wgsl`).
fn mip_bias(effective: f32) -> f32 {
    effective.log2().min(0.0)
}

/// The world camera's target, the size-carrier, in physical px × [`RenderScale`]; at 1.0 the
/// combine's read of the world is an identity resample.
#[derive(Resource)]
pub(crate) struct WorldBackdrop {
    /// The image the world camera targets. Nothing writes or samples it (the camera's final pass
    /// is the UI camera's, [`FfxBackdrop`]); it gives the view its size and logical viewport.
    pub(crate) image: Handle<Image>,
    /// The size the image was last built at, in physical px: the resize gate.
    size: UVec2,
    /// The image and scale factor the camera was last stamped with. Both gate the re-stamp: a
    /// rebuilt backdrop is a new asset ([`track_render_size`]). `None` until the first stamp.
    stamped: Option<(AssetId<Image>, f32)>,
}

impl WorldBackdrop {
    /// The world's render size in physical px, after the rounding and the axis ceiling.
    pub(crate) fn render_size(&self) -> UVec2 {
        self.size
    }
}

/// A fresh size-carrier at `size` physical px: one byte a pixel, a render attachment only, its CPU
/// copy dropped once the render world has it.
fn new_world_target(size: UVec2) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0],
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage = TextureUsages::RENDER_ATTACHMENT;
    image
}

fn window_physical_size(window: &Window) -> UVec2 {
    UVec2::new(
        window.physical_width().max(1),
        window.physical_height().max(1),
    )
}

fn setup_backdrop(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    scale: Res<RenderScale>,
) {
    let (size, _) = windows.single().map_or((UVec2::new(1280, 720), 1.0), |w| {
        render_target_for(
            window_physical_size(w),
            w.resolution.scale_factor(),
            scale.0,
        )
    });
    let image = images.add(new_world_target(size));
    commands.insert_resource(WorldBackdrop {
        image,
        size,
        // Nothing stamped yet, so `retarget_world_camera`'s first run always fires.
        stamped: None,
    });
}

/// Keep the target at `window × `[`RenderScale`]; runs before the stamp, whose factor must pair
/// with this size.
///
/// A rebuild publishes a new asset and removes the old one, never writing through the old handle:
/// `AssetId<Image>` names a GPU texture, and anything keyed on it would keep the retired texture.
fn track_render_size(
    mut backdrop: ResMut<WorldBackdrop>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    scale: Res<RenderScale>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let (size, _) = render_target_for(
        window_physical_size(window),
        window.resolution.scale_factor(),
        scale.0,
    );
    if size == backdrop.size {
        return;
    }
    let retired = std::mem::replace(&mut backdrop.image, images.add(new_world_target(size)));
    images.remove(&retired);
    backdrop.size = size;
    // Logged on every change: the rendered pixel count is the one term a probe cannot infer.
    info!(
        "render scale {:.3}: world renders at {}x{} into a {}x{} window",
        scale.0,
        size.x,
        size.y,
        window.physical_width(),
        window.physical_height()
    );
}

/// Point the world camera at the backdrop instead of the swapchain, carrying the window's scale
/// factor. A system, as the camera has two spawn sites; `benilla-worldview` keeps the swapchain.
///
/// `From<Handle<Image>>` stamps `scale_factor: 1.0`, which on a physical-px image makes the logical
/// viewport the physical size and throws every pick ray and `world_to_viewport` off by the display
/// scale. With the window's factor the logical viewport stays the window's, and render scale moves
/// size and factor together, so projection and picking never see it. Re-stamped when the image or
/// the factor changes, with the [`MipBias`] ([`mip_bias`]) inserted on every stamp.
fn retarget_world_camera(
    mut commands: Commands,
    mut backdrop: ResMut<WorldBackdrop>,
    windows: Query<&Window, With<PrimaryWindow>>,
    added: Query<(), Added<WorldCamera>>,
    mut cameras: Query<(Entity, &mut RenderTarget), With<WorldCamera>>,
) {
    let (px, window_factor) = windows.single().map_or((UVec2::ONE, 1.0), |w| {
        (window_physical_size(w), w.resolution.scale_factor())
    });
    // The built size, not the requested one: `track_render_size` may have clamped.
    let scale_factor = window_factor * backdrop.size.x as f32 / px.x.max(1) as f32;
    // The image as well as the factor: a pure window resize rebuilds without moving the factor.
    let want = (backdrop.image.id(), scale_factor);
    let moved = backdrop.stamped != Some(want);
    if !moved && added.is_empty() {
        return;
    }
    backdrop.stamped = Some(want);
    let bias = MipBias(mip_bias(scale_factor / window_factor));
    for (entity, mut current) in &mut cameras {
        *current = RenderTarget::Image(bevy::camera::ImageRenderTarget {
            handle: backdrop.image.clone(),
            scale_factor,
        });
        commands.entity(entity).insert(bias.clone());
    }
}

/// Point the player-UI camera's ground pass ([`FfxBackdrop`]) at the active world camera; with
/// none it is `None`, and the UI camera clears to transparent rather than show a stale world frame.
fn claim_backdrop(
    cameras: Query<(Entity, &Camera), With<WorldCamera>>,
    mut ui: Query<&mut FfxBackdrop, With<PlayerUiCamera>>,
) {
    let source = cameras
        .iter()
        .find_map(|(entity, camera)| camera.is_active.then_some(entity));
    for mut backdrop in &mut ui {
        // Compare first: a write would trip change detection for nothing.
        if backdrop.source != source {
            backdrop.source = source;
        }
    }
}

/// Owns the world camera's target (the size-carrier) and the UI camera's claim on the world.
pub(crate) struct WorldBackdropPlugin;

/// Render scale's change callback, clamped to [`RENDER_SCALE_RANGE`]; the backdrop and the
/// camera's factor follow together on the next frame.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut scale: ResMut<RenderScale>) {
    if ev.is("renderScale") {
        scale.0 = ev
            .num()
            .clamp(*RENDER_SCALE_RANGE.start(), *RENDER_SCALE_RANGE.end());
    }
}

impl Plugin for WorldBackdropPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<RenderScale>()
            .add_systems(Startup, setup_backdrop)
            .add_systems(
                Update,
                (track_render_size, retarget_world_camera, claim_backdrop).chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::TextureFormatPixelInfo as _;

    /// One byte a pixel, a render attachment, never sampled.
    #[test]
    fn the_world_target_is_a_size_carrier_only() {
        let image = new_world_target(UVec2::new(320, 200));
        assert_eq!(image.texture_descriptor.format, TextureFormat::R8Unorm);
        let usage = image.texture_descriptor.usage;
        assert!(usage.contains(TextureUsages::RENDER_ATTACHMENT));
        assert!(
            !usage.contains(TextureUsages::TEXTURE_BINDING),
            "nothing samples the world's target: the UI camera's ground pass reads the view's \
             main texture, not this image"
        );
        assert_eq!(
            image.asset_usage,
            RenderAssetUsages::RENDER_WORLD,
            "the CPU copy of a texture nothing reads is dropped once the render world has it"
        );
    }

    #[test]
    fn the_ui_camera_claims_the_drawing_world_camera() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_systems(Update, claim_backdrop);
        let ui = app
            .world_mut()
            .spawn((PlayerUiCamera, FfxBackdrop::default()))
            .id();
        let world_cam = app
            .world_mut()
            .spawn((
                WorldCamera,
                Camera {
                    is_active: true,
                    ..default()
                },
            ))
            .id();
        let source = |app: &mut App| app.world().get::<FfxBackdrop>(ui).unwrap().source;
        app.update();
        assert_eq!(source(&mut app), Some(world_cam), "active: claimed");
        app.world_mut()
            .get_mut::<Camera>(world_cam)
            .unwrap()
            .is_active = false;
        app.update();
        assert_eq!(source(&mut app), None, "gated: the claim goes with it");
    }

    /// The window's factor, not `1.0`, re-stamped when the window moves to another display.
    #[test]
    fn the_world_cameras_target_carries_the_windows_scale_factor() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let window = |sf: f32| {
            let mut w = Window::default();
            w.resolution.set_scale_factor(sf);
            w
        };
        app.world_mut().spawn((window(2.0), PrimaryWindow));
        app.init_resource::<RenderScale>()
            .add_systems(Startup, setup_backdrop)
            // Chained as the plugin chains them: the stamp reads the size the resize pass settled.
            .add_systems(Update, (track_render_size, retarget_world_camera).chain());
        let cam = app
            .world_mut()
            .spawn((WorldCamera, RenderTarget::default()))
            .id();
        app.update();

        let stamped = |app: &App, e: Entity| match app.world().entity(e).get::<RenderTarget>() {
            Some(RenderTarget::Image(t)) => t.scale_factor,
            _ => panic!("the world camera must target the backdrop image"),
        };
        assert_eq!(
            stamped(&app, cam),
            2.0,
            "a 2× display: the target must report the window's scale factor, or every logical \
             cursor position is read as a physical one and the pick ray misses by 2×"
        );
        // …and it follows the window across displays.
        let mut w = app.world_mut().query::<&mut Window>();
        w.single_mut(app.world_mut())
            .unwrap()
            .resolution
            .set_scale_factor(1.0);
        app.update();
        assert_eq!(
            stamped(&app, cam),
            1.0,
            "moving to a 1× display re-stamps: a stale factor is the same defect, inverted"
        );
    }

    /// A zero-size window (minimised on some platforms) still builds a legal texture.
    #[test]
    fn a_degenerate_size_still_builds_a_legal_texture() {
        let image = new_world_target(UVec2::ZERO);
        assert_eq!(image.texture_descriptor.size.width, 1);
        assert_eq!(image.texture_descriptor.size.height, 1);
        assert_eq!(
            image.data.as_ref().map(Vec::len),
            TextureFormat::R8Unorm.pixel_size().ok()
        );
    }

    /// Scale 1.0 gives the window's own numbers exactly, which every visual golden rests on.
    #[test]
    fn scale_one_reproduces_the_windows_own_numbers_exactly() {
        for px in [
            UVec2::new(1280, 720),
            UVec2::new(3200, 1800),
            UVec2::new(1601, 901), // odd on both axes, the case a `/ 2` would round
        ] {
            for sf in [1.0, 1.5, 2.0] {
                assert_eq!(render_target_for(px, sf, 1.0), (px, sf), "{px} at {sf}×");
            }
        }
    }

    /// `logical = image_size / scale_factor` (`bevy_camera`'s `to_logical`) stays the window's at
    /// every scale: x exact, y within one `round()`.
    #[test]
    fn every_scale_keeps_the_logical_viewport_the_windows_own() {
        for px in [UVec2::new(1280, 720), UVec2::new(3200, 1800)] {
            for sf in [1.0, 2.0] {
                let want = px.as_vec2() / sf;
                for scale in [0.25, 0.5, 0.6667, 0.75, 1.0, 1.25, 2.0, 4.0] {
                    let (size, factor) = render_target_for(px, sf, scale);
                    let logical = size.as_vec2() / factor;
                    assert!(
                        (logical.x - want.x).abs() < 0.001,
                        "{px} at {sf}× scaled {scale}: logical width {logical:?}, want {want:?}"
                    );
                    assert!(
                        (logical.y - want.y).abs() < 1.0,
                        "{px} at {sf}× scaled {scale}: logical height {logical:?}, want {want:?}"
                    );
                }
            }
        }
    }

    /// The ceiling clamps the ratio, keeping the aspect, and binds on a huge window at 1.0 too.
    #[test]
    fn the_axis_ceiling_shrinks_the_picture_it_does_not_reshape_it() {
        let px = UVec2::new(3840, 2160);
        let (size, _) = render_target_for(px, 1.0, 4.0);
        assert_eq!(size.x, MAX_RENDER_AXIS);
        let aspect = |v: UVec2| v.x as f32 / v.y as f32;
        assert!(
            (aspect(size) - aspect(px)).abs() < 0.001,
            "aspect kept: {size}"
        );
        // …and the clamp is not render scale's alone.
        let huge = UVec2::new(10240, 4320);
        let (size, _) = render_target_for(huge, 1.0, 1.0);
        assert!(
            size.x <= MAX_RENDER_AXIS && size.y <= MAX_RENDER_AXIS,
            "{size}"
        );
        assert!(
            (aspect(size) - aspect(huge)).abs() < 0.001,
            "aspect kept: {size}"
        );
    }

    #[test]
    fn the_mip_bias_is_log2_below_one_and_nothing_above_it() {
        assert!((mip_bias(0.5) - -1.0).abs() < 1e-6);
        assert!((mip_bias(0.25) - -2.0).abs() < 1e-6);
        assert_eq!(
            mip_bias(1.0),
            0.0,
            "off must be exactly off — 0.0, not -0.0's cousin"
        );
        assert_eq!(mip_bias(2.0), 0.0);
        assert_eq!(mip_bias(4.0), 0.0);
    }

    /// The id moves, the old asset is removed, and the camera is re-stamped onto the new image.
    #[test]
    fn a_rebuilt_backdrop_is_a_new_asset_and_the_camera_follows_it() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let mut w = Window::default();
        w.resolution.set_scale_factor(2.0);
        app.world_mut().spawn((w, PrimaryWindow));
        app.insert_resource(RenderScale(1.0))
            .add_systems(Startup, setup_backdrop)
            .add_systems(Update, (track_render_size, retarget_world_camera).chain());
        let cam = app
            .world_mut()
            .spawn((WorldCamera, RenderTarget::default()))
            .id();
        app.update();
        let first = app.world().resource::<WorldBackdrop>().image.id();

        // Move the render scale while the world is up.
        app.insert_resource(RenderScale(0.5));
        app.update();

        let second = app.world().resource::<WorldBackdrop>().image.id();
        assert_ne!(
            first, second,
            "a rebuilt backdrop is a NEW GPU texture, so it must be a new AssetId — \
             the same id hands `ui_pass` back a bind group pointing at the retired texture"
        );
        assert!(
            !app.world().resource::<Assets<Image>>().contains(first),
            "the retired image must be removed, not merely dropped: a cached material still \
             holds a strong handle to it"
        );
        match app.world().entity(cam).get::<RenderTarget>() {
            Some(RenderTarget::Image(t)) => assert_eq!(
                t.handle.id(),
                second,
                "the camera must be re-stamped onto the new image"
            ),
            _ => panic!("the world camera must target the backdrop image"),
        }
    }

    /// A pure window resize moves the image without moving the scale factor.
    #[test]
    fn a_window_resize_restamps_the_camera_even_though_the_factor_does_not_move() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let mut w = Window::default();
        w.resolution.set_scale_factor(2.0);
        let win = app.world_mut().spawn((w, PrimaryWindow)).id();
        app.insert_resource(RenderScale(1.0))
            .add_systems(Startup, setup_backdrop)
            .add_systems(Update, (track_render_size, retarget_world_camera).chain());
        let cam = app
            .world_mut()
            .spawn((WorldCamera, RenderTarget::default()))
            .id();
        app.update();
        let before = match app.world().entity(cam).get::<RenderTarget>() {
            Some(RenderTarget::Image(t)) => (t.handle.id(), t.scale_factor),
            _ => panic!("the world camera must target the backdrop image"),
        };

        app.world_mut()
            .entity_mut(win)
            .get_mut::<Window>()
            .expect("the primary window")
            .resolution
            .set(640.0, 400.0);
        app.update();

        let after = match app.world().entity(cam).get::<RenderTarget>() {
            Some(RenderTarget::Image(t)) => (t.handle.id(), t.scale_factor),
            _ => panic!("the world camera must target the backdrop image"),
        };
        assert_ne!(before.0, after.0, "the resize must re-point the camera");
        assert!(
            (before.1 - after.1).abs() < 1e-6,
            "at scale 1.0 the factor is the window's own and does not move on a resize — \
             which is exactly why the re-stamp cannot be gated on it alone: {} vs {}",
            before.1,
            after.1
        );
        assert!(
            app.world().resource::<Assets<Image>>().contains(after.0),
            "the camera must point at an image that still exists"
        );
    }

    /// A half-scale world on a 2× display: a half-size image, a factor of 1.0, `MipBias(-1)`.
    #[test]
    fn a_half_scale_world_halves_the_image_the_factor_and_the_mip() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let mut w = Window::default();
        w.resolution.set_scale_factor(2.0);
        let px = window_physical_size(&w);
        app.world_mut().spawn((w, PrimaryWindow));
        app.insert_resource(RenderScale(0.5))
            .add_systems(Startup, setup_backdrop)
            .add_systems(Update, (track_render_size, retarget_world_camera).chain());
        let cam = app
            .world_mut()
            .spawn((WorldCamera, RenderTarget::default()))
            .id();
        app.update();

        let backdrop = app.world().resource::<WorldBackdrop>();
        assert_eq!(backdrop.size, UVec2::new(px.x / 2, px.y / 2));
        match app.world().entity(cam).get::<RenderTarget>() {
            Some(RenderTarget::Image(t)) => assert!(
                (t.scale_factor - 1.0).abs() < 1e-6,
                "half the pixels at half the factor is the same logical viewport: {}",
                t.scale_factor
            ),
            _ => panic!("the world camera must target the backdrop image"),
        }
        let bias = app.world().entity(cam).get::<MipBias>().expect("a bias");
        assert!(
            (bias.0 - -1.0).abs() < 1e-6,
            "half scale owes one mip level: {}",
            bias.0
        );
    }
}
