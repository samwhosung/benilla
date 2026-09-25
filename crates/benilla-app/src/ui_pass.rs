//! The player-UI quad pass: a sorted-quad renderer for the WoW UI, not `bevy_ui`. Producers
//! flatten a frame's `(stratum, level, layer, sublayer, decl)` order into [`UiQuad::z_key`].
//!
//! [`PlayerUiCamera`] (order 1) sits over the offscreen world camera (0) and under the egui dev
//! overlay (2). Its main pass draws the world first ([`crate::world_backdrop`]), then the quads,
//! and [`crate::ui_gamma`]'s decode writes the swapchain with no blit.
//!
//! The reference draws its UI through the fixed-function device into an 8-bit backbuffer, so every
//! UI multiply and blend is gamma-byte arithmetic, clamped at each write. So is this pass, decoded
//! to linear once at the end; composited in linear, an `ADD` quad lands at about a quarter of the
//! reference's lift.
//!
//! Quads are stable-sorted by `z_key`, CPU-clipped and split into contiguous runs of one texture
//! and material state, one `Mesh2d` draw each. A run never spans a quad of another state, so the
//! total order holds and interleaved textures pay one draw per switch.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::CameraOutputMode;
use bevy::image::Image;
use bevy::math::Rect;
use bevy::mesh::{Indices, Mesh, MeshTag, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Extent3d,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::sprite_render::{Material2d, Material2dKey, Material2dPlugin};
use bevy::window::PrimaryWindow;

/// One `(u, v)` per screen corner, in [`Run::push_quad`]'s winding (top-left, top-right,
/// bottom-right, bottom-left), not a `(min, max)` rect: a `<TexCoords>` with `left > right` (the
/// PlayerFrame ring) must stay mirrored, and a backdrop top or bottom edge maps atlas u to screen
/// Y, which only per-corner UVs express.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct UvRect {
    pub corners: [[f32; 2]; 4],
}

impl UvRect {
    /// The full texture, no crop.
    pub const FULL: Self = Self {
        corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
    };

    /// From a `<TexCoords>` `[left, right, top, bottom]` tuple, unnormalized, so a mirror survives.
    pub fn from_tex_coords([left, right, top, bottom]: [f32; 4]) -> Self {
        Self {
            corners: [[left, top], [right, top], [right, bottom], [left, bottom]],
        }
    }

    /// From explicit corners in the `push_quad` winding: the backdrop's rotated edge slices.
    pub fn from_corners(corners: [[f32; 2]; 4]) -> Self {
        Self { corners }
    }

    /// From an already-normalized [`Rect`], such as a glyph atlas cell: `min` to TL, `max` to BR.
    pub fn from_rect(r: Rect) -> Self {
        Self::from_tex_coords([r.min.x, r.max.x, r.min.y, r.max.y])
    }
}

/// One resolved screen-space quad, the pass's whole input. `texture`, the flags from `additive`
/// to `uv_clamp`, and `mask` are material state: a quad differing in any of them starts a new run.
#[derive(Clone, PartialEq)]
pub(crate) struct UiQuad {
    /// Screen pixels, y-down, anchor-resolved.
    pub rect: Rect,
    /// Paint order, stable-sorted ascending: higher draws on top.
    pub z_key: u64,
    /// `None` samples the shared 1×1 white.
    pub texture: Option<Handle<Image>>,
    pub uv: UvRect,
    /// Straight-alpha tint in client-space sRGB, the raw FrameXML/Lua value, never converted.
    pub color: [f32; 4],
    /// WoW `ADD` blend (EGxBlend 3, `glBlendFunc(GL_SRC_ALPHA, GL_ONE)`) instead of straight alpha.
    pub additive: bool,
    /// Mask to the inscribed circle: the live unit portrait.
    pub circular: bool,
    /// Draw the texel's luminance (`Texture:SetDesaturated(1)`), folded in gamma bytes before the
    /// tint, so `SetItemButtonDesaturated(button, 1, 0.65, 0.65, 0.65)`
    /// (`Blizzard_TalentUI.lua:215`) lands grey and dim.
    pub desaturated: bool,
    /// The texture is already premultiplied: a booth render target ([`crate::portrait`]), where
    /// an additive particle adds colour with no coverage and a second alpha weight would erase it.
    pub premultiplied: bool,
    /// The texture holds undecoded gamma bytes (`BlpVariant::MapTile`: the reference's tile
    /// sampler sets `GL_TEXTURE_SRGB_DECODE_EXT = GL_SKIP_DECODE_EXT`), so the tint multiplies the
    /// authored byte and the ordinary arm's `linear_to_srgb` must not encode it again.
    pub gamma_texel: bool,
    /// Alpha test against this value on `texel.a × tint.a` instead of blending: a pass writes
    /// opaque, a fail discards. `Some(224.0 / 255.0)` is the WMO-interior minimap tile, drawn by
    /// the reference with EGxBlend 1 (blending off) and `glAlphaFunc(GL_GEQUAL, 0.87843144)`
    /// (`0x85ad20[1] = 224`); blended, two tiles' seam keeps `(1−a)(1−b)` of the black clear.
    pub alpha_test: Option<f32>,
    /// The half-texel-inset window `[u_min, v_min, u_max, v_max]` sampled, so an atlas cell clamps
    /// at its own edge instead of filtering in its neighbour (`ClampToEdge` stops at the image's).
    /// Per axis: UVs past `[0, 1]` are the reference's tiling idiom and never clamp.
    pub uv_clamp: Option<[f32; 4]>,
    /// CPU clip standing in for a scissor rect.
    pub clip: Option<Rect>,
    /// Radians clockwise on screen about the rect centre (the minimap player arrow), the UVs
    /// riding their corners; `clip` applies first, unrotated.
    pub rotation: f32,
    /// A screen-anchored alpha mask (the minimap's `MinimapMask.blp` circle), so a tile panning
    /// under the fixed circle masks correctly; outside [`UiQuadMask::rect`] the fragment drops.
    pub mask: Option<UiQuadMask>,
    /// Explicit screen corners (y-down px) drawn as the fan `(0,1,2)(0,2,3)` (the cooldown pie's
    /// wedge), bypassing `rotation`, `clip` and `rect`, which should still bound them.
    pub corners: Option<[Vec2; 4]>,
}

/// A screen-anchored alpha mask over a [`UiQuad`].
#[derive(Clone, PartialEq)]
pub(crate) struct UiQuadMask {
    /// The mask art, whose alpha is the coverage (`MinimapMask.blp`: DXT3, the circle in alpha).
    pub texture: Handle<Image>,
    /// The span it covers, in [`UiQuad::rect`]'s y-down logical px; it samples 0..1 across it.
    pub rect: Rect,
}

impl Default for UiQuad {
    fn default() -> Self {
        Self {
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            z_key: 0,
            texture: None,
            uv: UvRect::FULL,
            color: [1.0, 1.0, 1.0, 1.0],
            additive: false,
            circular: false,
            desaturated: false,
            premultiplied: false,
            gamma_texel: false,
            alpha_test: None,
            uv_clamp: None,
            clip: None,
            rotation: 0.0,
            mask: None,
            corners: None,
        }
    }
}

/// The pass's input in two lanes. [`Self::quads`], the base lane, is replaced whole by the script
/// extract, which sets [`Self::dirty`] only when it changed. [`Self::overlays`], the append lane
/// ([`UiQuadAppend`]), is cleared and re-emitted every frame and diffed by [`rebuild_ui_mesh`]
/// against the last frame's; its appenders never touch `dirty`.
#[derive(Resource, Default)]
pub(crate) struct UiQuads {
    pub quads: Vec<UiQuad>,
    pub overlays: Vec<UiQuad>,
    /// The append lane as of the last rebuild.
    last_overlays: Vec<UiQuad>,
    pub dirty: bool,
}

/// The append lane's z bands, far below the scripted UI's keys (a [`benilla_ui::order::ZKey`]
/// region sets `1 << 20`), so the whole lane paints under the player UI.
pub(crate) mod overlay_z {
    /// Floating combat, XP and honor numbers, drawn by the client in the world scene under all UI.
    pub(crate) const WORLD_TEXT: u64 = 0;
    /// The chat bubbles: one frame level per live bubble, farthest from the camera first.
    pub(crate) const BUBBLE: u64 = 1 << 8;
    /// Keys per bubble level: its bg, edge, tail and text.
    pub(crate) const BUBBLE_STRIDE: u64 = 4;
    /// The V-plates, over every bubble; a unit never shows both, so this orders only across units.
    pub(crate) const VPLATE: u64 = 1 << 16;
    /// The highest bubble level below [`VPLATE`], a guard rail far past any real count (a bubble
    /// needs a speaker within 20 yd).
    pub(crate) const BUBBLE_MAX_LEVEL: u64 = (VPLATE - BUBBLE) / BUBBLE_STRIDE - 1;
}

/// Clear the append lane at the top of the [`UiQuadAppend`] window, before every appender.
fn clear_ui_overlays(mut quads: ResMut<UiQuads>) {
    quads.overlays.clear();
}

/// The render layer this pass's camera and batches share, disjoint from the world camera's 0.
const UI_RENDER_LAYER: usize = 1;

pub(crate) fn ui_render_layers() -> RenderLayers {
    RenderLayers::layer(UI_RENDER_LAYER)
}

/// Above the world camera (0), below the egui dev overlay (2, `debug_panel::spawn_egui_camera`).
const UI_CAMERA_ORDER: isize = 1;

/// Marker on the player-UI camera, whose backdrop [`crate::world_backdrop`] points at the world
/// camera drawing this frame.
#[derive(Component)]
pub(crate) struct PlayerUiCamera;

/// Marker on each pooled batch entity, one `Mesh2d` draw per run; the FPS probe's `ui_batches=`
/// counts them.
#[derive(Component)]
pub(crate) struct UiQuadBatch;

/// The shared 1×1 opaque white that texture-less quads sample.
#[derive(Resource)]
struct UiWhiteTexture(Handle<Image>);

/// The quad material: a texture and the [`UiQuad`] flags as uniforms, drawn through one
/// premultiplied-alpha pipeline (`ui_quad.wgsl`). It holds no colour, so it is its identity key
/// alone and a steady frame re-prepares none.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct UiQuadMaterial {
    #[uniform(0)]
    additive: u32,
    #[texture(1)]
    #[sampler(2)]
    texture: Option<Handle<Image>>,
    #[uniform(3)]
    circular: u32,
    #[uniform(7)]
    desaturate: u32,
    #[uniform(8)]
    premultiplied: u32,
    /// `<= 0` disables the alpha test.
    #[uniform(9)]
    alpha_ref: f32,
    #[uniform(10)]
    gamma_texel: u32,
    /// The mask span in physical px (the shader compares `@builtin(position)`); `z <= x` disables.
    #[uniform(4)]
    mask_rect: Vec4,
    #[texture(5)]
    #[sampler(6)]
    mask: Option<Handle<Image>>,
    /// `(u_min, v_min, u_max, v_max)`; `min > max` on an axis disables that axis.
    #[uniform(11)]
    uv_clamp: Vec4,
}

/// A run's one colour for the batch entity's [`MeshTag`], unpacked by `ui_quad.wgsl`'s vertex
/// stage, so a colour pulse is a component write, never a mesh or material write. One byte per
/// channel, `×255 + 0.5`, as the reference packs a vertex colour (`CImVector`; `SetVertexColor`
/// at `0x79abd0`, the frame-alpha fold at `0x77fac0`). Complemented, so an entity with no tag
/// (the minimap's interior tiles) unpacks to opaque white.
fn tint_tag(color: [f32; 4]) -> u32 {
    let byte = |v: f32| {
        // `0x40a2b0`: `×255.0 + 0.5`, truncated, after the binding's `[0, 1]` clamp; no overflow.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        b
    };
    !(byte(color[0]) | byte(color[1]) << 8 | byte(color[2]) << 16 | byte(color[3]) << 24)
}

impl UiQuadMaterial {
    /// The WMO-interior minimap tile material: the texture and an alpha test, nothing else, for the
    /// minimap's own composite target ([`crate::minimap`]). A passing fragment emits `a = 1`, so
    /// the forced blend of [`Self::specialize`] is a plain replace: the reference's blending off.
    pub(crate) fn interior_tile(texture: Handle<Image>, alpha_ref: f32) -> Self {
        Self {
            additive: 0,
            texture: Some(texture),
            circular: 0,
            desaturate: 0,
            premultiplied: 0,
            alpha_ref,
            // The alpha-test arm decodes explicitly, so the tile's SKIP_DECODE holds without this.
            gamma_texel: 0,
            mask_rect: Vec4::new(0.0, 0.0, -1.0, -1.0),
            mask: None,
            uv_clamp: UV_CLAMP_OFF,
        }
    }
}

/// The mesh every `UiQuadMaterial` quad outside the batches draws with: a 1×1 rectangle placed by
/// its Transform. Its layout (POSITION + NORMAL + UV_0) keys a second pipeline beside the batch
/// mesh's (POSITION + UV_0 + COLOR), which `pipe_warm` warms by drawing this same mesh.
pub(crate) fn tile_quad_mesh() -> Mesh {
    Rectangle::new(1.0, 1.0).into()
}

impl Material2d for UiQuadMaterial {
    /// Our own vertex stage, which unpacks the run's tint from the instance tag ([`tint_tag`]).
    fn vertex_shader() -> ShaderRef {
        "embedded://benilla_app/shaders/ui_quad.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_app/shaders/ui_quad.wgsl".into()
    }

    fn alpha_mode(&self) -> bevy::sprite_render::AlphaMode2d {
        // The transparent phase, sorted by mesh z (the run order); `specialize` sets the blend.
        bevy::sprite_render::AlphaMode2d::Blend
    }

    fn specialize(
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        if let Some(fragment) = descriptor.fragment.as_mut() {
            if let Some(target) = fragment.targets.first_mut().and_then(|t| t.as_mut()) {
                // `(One, OneMinusSrcAlpha)` over gamma values: the shader emits `(rgb·a, a)` for
                // BLEND and `(rgb·a, 0)` for ADD, so this one state is EGxBlend 2 and EGxBlend 3,
                // clamped at each write like the reference's 8-bit backbuffer.
                let premultiplied = BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::OneMinusSrcAlpha,
                    operation: BlendOperation::Add,
                };
                target.blend = Some(BlendState {
                    color: premultiplied,
                    alpha: premultiplied,
                });
            }
        }
        Ok(())
    }
}

/// Producers that append to [`UiQuads`] after the script extract. In Update after
/// [`benilla_world::schedule::WorldStage::Input`], the world-anchored ones (combat numbers,
/// nameplates) project through this frame's camera `Transform` (`GlobalTransform::from`; the
/// camera is a root entity), not the propagated one a frame behind. Never PostUpdate:
/// [`rebuild_ui_mesh`] spawns `Mesh2d` batches, and bevy_sprite_render cannot specialize one
/// spawned there (the whole UI vanishes).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct UiQuadAppend;

/// Project a world point for a world-anchored overlay, with the reference projector's verdict:
/// `None` does not draw. `0x483ee0` rejects a point behind the near plane (`0x483f7a`) or outside
/// the WorldFrame's region (`[0xb4b2bc]`; `0x484075`, `0x484086`, `0x484096`, `0x4840a6`), which
/// `0x483970` mirrors ÷G44/÷G48 so the aspect cancels: exactly the viewport, inclusive. A viewport
/// rejection has already written a plausible out-param (`0x484065`, `0x484067`), so the verdict
/// must be honoured; plate and world-text callers destroy what they were placing. The chat bubble
/// does not call this: its seat ignores the verdict, and a bubble slides off with its speaker.
pub(crate) fn project_overlay(
    cam: &Camera,
    cam_tf: &GlobalTransform,
    world: Vec3,
    viewport: Vec2,
) -> Option<Vec2> {
    // Bevy's `Err` is the near-plane half (`0x483f7a`); `accepts` is the viewport half.
    let p = cam.world_to_viewport(cam_tf, world).ok()?;
    accepts(p, viewport).then_some(p)
}

/// `WOW_PROBE_UI_ONE_TEX=1`, for pricing only (it draws wrong): runs split on state flags alone,
/// each drawn with its first quad's texture, the draw count a UI texture atlas would reach.
fn one_texture_probe() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_PROBE_UI_ONE_TEX").is_some())
}

fn accepts(p: Vec2, viewport: Vec2) -> bool {
    (0.0..=viewport.x).contains(&p.x) && (0.0..=viewport.y).contains(&p.y)
}

/// Owns the player-UI camera and the per-frame quad-to-mesh rebuild.
pub(crate) struct PlayerUiPlugin;

impl Plugin for PlayerUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiQuads>()
            .init_resource::<UiMeshCost>()
            .init_resource::<crate::ui_script::UiCostWanted>()
            .add_plugins((
                Material2dPlugin::<UiQuadMaterial>::default(),
                // The lane's decode is not optional, so this plugin owns it.
                crate::ui_gamma::UiGammaPlugin,
                // Skips the 2D opaque pass when empty, which on this camera is always.
                crate::opaque2d::SkipEmptyOpaque2dPlugin,
            ))
            .add_systems(Startup, (spawn_ui_camera, init_white_texture))
            .configure_sets(
                Update,
                UiQuadAppend.after(benilla_world::schedule::WorldStage::Input),
            )
            .add_systems(Update, clear_ui_overlays.before(UiQuadAppend))
            .add_systems(Update, rebuild_ui_mesh.after(UiQuadAppend))
            .add_systems(Last, count_material_events);
    }
}

/// The full-window player-UI camera: the world, then the quads, then the gamma decode.
fn spawn_ui_camera(mut commands: Commands) {
    commands.spawn((
        PlayerUiCamera,
        Name::new("player-UI camera"),
        Camera2d,
        // No MSAA, named because silence means 4×: `Camera` requires `Msaa`, whose default is
        // `Sample4`. The world arrives resolved, and the quads are axis-aligned.
        bevy::render::view::Msaa::Off,
        // The lane's mandatory decode: the quad pass leaves gamma bytes in the target.
        crate::ui_gamma::UiGammaLane::default(),
        // The world as this pass's first draw; `source` is `None` until a world camera draws.
        benilla_world::ffx_glow::FfxBackdrop::default(),
        // Bevy UI (the glue and loading screens) renders here, in the gamma lane; unmarked, it
        // would take the highest-order camera, the egui overlay.
        bevy::ui::IsDefaultUiCamera,
        ui_render_layers(),
        Camera {
            order: UI_CAMERA_ORDER,
            // No output blit ([`benilla_world::final_pass`]): the decode writes the swapchain.
            output_mode: CameraOutputMode::Skip,
            // Clear to transparent, writeback off: `MsaaWriteback::Auto` would re-emit an earlier
            // camera's output through this one, a re-encode round trip that tints the world.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            msaa_writeback: bevy::camera::MsaaWriteback::Off,
            ..default()
        },
    ));
}

/// Build the shared 1×1 white texture that texture-less quads sample.
fn init_white_texture(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[255, 255, 255, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.insert_resource(UiWhiteTexture(images.add(image)));
}

/// Clip `rect` to `clip`, reprojecting the UVs to the clipped edges; `None` when nothing is left.
fn clip_quad(rect: Rect, uv: UvRect, clip: Rect) -> Option<(Rect, UvRect)> {
    let clipped = rect.intersect(clip);
    if clipped.is_empty() {
        return None;
    }
    let (w, h) = (rect.width(), rect.height());
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    // A bilinear sample of the four corner UVs at the clipped edges' fractions: an axis-aligned
    // crop stays a separable lerp (a mirror survives), and a rotated backdrop edge keeps its turn.
    let t_min_x = (clipped.min.x - rect.min.x) / w;
    let t_max_x = (clipped.max.x - rect.min.x) / w;
    let t_min_y = (clipped.min.y - rect.min.y) / h;
    let t_max_y = (clipped.max.y - rect.min.y) / h;
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let [tl, tr, br, bl] = uv.corners;
    let bilerp = |fx: f32, fy: f32| -> [f32; 2] {
        let top = [lerp(tl[0], tr[0], fx), lerp(tl[1], tr[1], fx)];
        let bot = [lerp(bl[0], br[0], fx), lerp(bl[1], br[1], fx)];
        [lerp(top[0], bot[0], fy), lerp(top[1], bot[1], fy)]
    };
    let new_uv = UvRect::from_corners([
        bilerp(t_min_x, t_min_y),
        bilerp(t_max_x, t_min_y),
        bilerp(t_max_x, t_max_y),
        bilerp(t_min_x, t_max_y),
    ]);
    Some((clipped, new_uv))
}

/// One run: contiguous quads, in `z_key` order, sharing a texture and every material flag.
struct Run {
    texture: Handle<Image>,
    additive: bool,
    circular: bool,
    desaturated: bool,
    premultiplied: bool,
    gamma_texel: bool,
    alpha_test: Option<f32>,
    mask: Option<UiQuadMask>,
    uv_clamp: Option<[f32; 4]>,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Run {
    /// A fresh run with `q`'s material flags under the resolved `texture` (its own, or the white).
    fn new(texture: Handle<Image>, q: &UiQuad) -> Self {
        Self {
            texture,
            additive: q.additive,
            circular: q.circular,
            desaturated: q.desaturated,
            premultiplied: q.premultiplied,
            gamma_texel: q.gamma_texel,
            alpha_test: q.alpha_test,
            mask: q.mask.clone(),
            uv_clamp: q.uv_clamp,
            positions: Vec::new(),
            uvs: Vec::new(),
            colors: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Append one clipped quad as two triangles in the TL, TR, BR, BL winding; append order is
    /// paint order within the run. `rotation` spins the corners clockwise about the rect centre,
    /// UVs riding along; `to_world` maps y-down screen px into the camera's y-up, centred space.
    fn push_quad(
        &mut self,
        rect: Rect,
        uv: UvRect,
        color: [f32; 4],
        rotation: f32,
        to_world: impl Fn(Vec2) -> Vec2,
    ) {
        let [tl, tr, br, bl] = uv.corners;
        let mut corners = [
            (Vec2::new(rect.min.x, rect.min.y), Vec2::from(tl)),
            (Vec2::new(rect.max.x, rect.min.y), Vec2::from(tr)),
            (Vec2::new(rect.max.x, rect.max.y), Vec2::from(br)),
            (Vec2::new(rect.min.x, rect.max.y), Vec2::from(bl)),
        ];
        if rotation != 0.0 {
            // Clockwise on a y-down screen is the standard CCW matrix in y-down coordinates.
            let (sin, cos) = rotation.sin_cos();
            let center = (rect.min + rect.max) * 0.5;
            for (p, _) in &mut corners {
                let d = *p - center;
                *p = center + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
            }
        }
        self.push_corners(corners, color, to_world);
    }

    /// Append four screen corners with their UVs as the fan `(0,1,2)(0,2,3)`.
    fn push_corners(
        &mut self,
        corners: [(Vec2, Vec2); 4],
        color: [f32; 4],
        to_world: impl Fn(Vec2) -> Vec2,
    ) {
        let base = self.positions.len() as u32;
        for (p, t) in corners {
            let w = to_world(p);
            self.positions.push([w.x, w.y, 0.0]);
            self.uvs.push([t.x, t.y]);
            self.colors.push(color);
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// `WOW_UI_COST=1` also logs the frame's [`UiQuadMaterial`] asset events: bevy re-prepares a
/// material (a bind group and a uniform buffer, on the render thread) for exactly each `Added` or
/// `Modified`. It reads zero on a settled frame, pulsing glow or not; a line there is a material
/// rebuilt for something other than a new texture, a regression.
fn count_material_events(
    mut events: MessageReader<AssetEvent<UiQuadMaterial>>,
    materials: Res<Assets<UiQuadMaterial>>,
    server: Res<AssetServer>,
    mut named: Local<u32>,
) {
    if !crate::ui_script::extract::ui_cost_enabled() {
        events.clear();
        return;
    }
    let (mut added, mut modified, mut unused) = (0usize, 0usize, 0usize);
    for event in events.read() {
        match event {
            AssetEvent::Added { .. } => added += 1,
            AssetEvent::Modified { id } => {
                modified += 1;
                // The first hundred name themselves: the texture the rewritten material binds.
                if *named < 100 {
                    *named += 1;
                    if let Some(m) = materials.get(*id) {
                        let path = m
                            .texture
                            .as_ref()
                            .and_then(|t| server.get_path(t.id()))
                            .map(|p| p.to_string());
                        info!("[ui-mat] modified {id:?} texture={path:?}");
                    }
                }
            }
            AssetEvent::Removed { .. } | AssetEvent::Unused { .. } => unused += 1,
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
    if added + modified + unused > 0 {
        info!("[ui-mat] added={added} modified={modified} unused={unused}");
    }
}

/// The rebuild's state across frames, so a rebuild pays only for what changed: batch entities and
/// meshes pooled by run index and rewritten in place, materials cached by their identity key.
#[derive(Default)]
struct BatchPools {
    entities: Vec<Entity>,
    meshes: Vec<Handle<Mesh>>,
    /// Each slot's last-written mesh (base positions): an unchanged run is not rewritten.
    stored: Vec<StoredRun>,
    /// Each slot's pan from its base, on the entity's `Transform`: a run that only moved (the
    /// minimap tiles) costs no `Assets<Mesh>` write, whose `AssetChanged` probe sweeps every
    /// `Mesh3d` in the scene.
    offsets: Vec<Vec2>,
    materials: std::collections::HashMap<MatKey, Handle<UiQuadMaterial>>,
    /// Per slot: the material the batch entity carries.
    bound: Vec<Option<AssetId<UiQuadMaterial>>>,
    /// Per slot: the batch entity's [`MeshTag`], the run's one colour ([`tint_tag`]).
    tags: Vec<Option<u32>>,
}

/// One pooled slot's full identity: the mesh bytes and what the entity was last written with
/// (material key, z). Compared in full, never hashed: a collision would draw a stale batch.
#[derive(PartialEq)]
struct StoredRun {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
    z_bits: u32,
    key: MatKey,
}

impl StoredRun {
    /// Why [`Self::translation_from`] missed, for `WOW_UI_DIFF=1`. That gate is `Some(d)` when
    /// every vertex moved by one XY delta and all else is bit-equal (`Some(ZERO)`: unchanged); its
    /// exact float compare can only cost a rewrite, never correctness.
    fn translation_miss_reason(&self, new: &StoredRun) -> &'static str {
        if self.key != new.key {
            "key"
        } else if self.z_bits != new.z_bits {
            "z"
        } else if self.positions.len() != new.positions.len() {
            "len"
        } else if self.indices != new.indices {
            "indices"
        } else if self.uvs != new.uvs {
            "uvs"
        } else if self.colors != new.colors {
            "colors"
        } else {
            "delta-nonuniform"
        }
    }

    fn translation_from(&self, new: &StoredRun) -> Option<Vec2> {
        if self.key != new.key
            || self.z_bits != new.z_bits
            || self.positions.len() != new.positions.len()
            || self.positions.is_empty()
            || self.indices != new.indices
            || self.uvs != new.uvs
            || self.colors != new.colors
        {
            return None;
        }
        let d = Vec2::new(
            new.positions[0][0] - self.positions[0][0],
            new.positions[0][1] - self.positions[0][1],
        );
        self.positions
            .iter()
            .zip(&new.positions)
            .all(|(b, n)| n[0] == b[0] + d.x && n[1] == b[1] + d.y && n[2] == b[2])
            .then_some(d)
    }
}

/// A material's full identity, floats by their bits so byte-equal materials share one entry.
type MatKey = (
    AssetId<Image>,
    bool,
    bool,
    bool,
    bool,
    bool,
    u32,
    Option<AssetId<Image>>,
    [u32; 4],
    [u32; 4],
);

/// The [`UiQuadMaterial::uv_clamp`] that clamps neither axis: `min > max` turns an axis off.
const UV_CLAMP_OFF: Vec4 = Vec4::new(1.0, 1.0, 0.0, 0.0);

/// Despawn every pooled batch entity; mesh handles and the material cache stay, free to keep.
fn retire_batches(pools: &mut BatchPools, commands: &mut Commands) {
    for entity in pools.entities.drain(..) {
        commands.entity(entity).despawn();
    }
    // The skip and pan gates forget with them: a retired slot's entity is gone, so unchanged
    // content must not skip the respawn when the UI returns.
    pools.stored.clear();
    pools.offsets.clear();
}

/// The two asset stores [`rebuild_ui_mesh`] writes and the image-removal stream its material
/// cache hears, bundled under clippy's argument ceiling.
type RebuildStores<'w, 's> = (
    ResMut<'w, Assets<Mesh>>,
    ResMut<'w, Assets<UiQuadMaterial>>,
    MessageReader<'w, 's, AssetEvent<Image>>,
);

fn ui_mesh_frozen() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_FREEZE_UI_MESH").is_some())
}

static UI_DIFF: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var_os("WOW_UI_DIFF").is_some());

static UI_PROBE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WOW_UI_PROBE").as_deref() == Ok("1"));

/// A rebuild's render-side cost, phases in µs: the sort, clip, split and mesh writes that the
/// script-side `[ui-cost]` phases do not see. Its own resource, not a `UiFrameCost` field: the two
/// systems are unordered in `Update`, and whichever ran second would wipe a shared field.
#[derive(Resource, Default, Clone)]
pub(crate) struct UiMeshCost {
    pub(crate) rebuilt: bool,
    pub(crate) total: u128,
    pub(crate) sort: u128,
    pub(crate) split: u128,
    pub(crate) write: u128,
    pub(crate) quads: usize,
    pub(crate) runs: usize,
    /// Pooled meshes rewritten rather than left or panned, each re-extracted in
    /// `RenderExtractApp`; `rewrites == runs` every frame means the skip gate is defeated.
    pub(crate) rewrites: usize,
}

fn rebuild_ui_mesh(
    mut quads: ResMut<UiQuads>,
    mut commands: Commands,
    mut stores: RebuildStores,
    mut pools: Local<BatchPools>,
    white: Option<Res<UiWhiteTexture>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    // The hide binding and the cost meter's pair, bundled under clippy's argument ceiling.
    mut hide_and_meter: (
        Res<crate::ui_hide::UiHidden>,
        ResMut<UiMeshCost>,
        Res<crate::ui_script::UiCostWanted>,
    ),
) {
    // Forget the materials of removed images first, before any early return, since a missed read
    // leaks: a cached material holds a strong image handle and, prepared, its GPU texture.
    // `Removed`, not `Unused`: this cache is itself a strong handle, so `Unused` never fires for a
    // cached image, and a producer retiring an image removes the asset explicitly.
    let retired: Vec<AssetId<Image>> = stores
        .2
        .read()
        .filter_map(|e| match e {
            AssetEvent::Removed { id } => Some(*id),
            _ => None,
        })
        .collect();
    if !retired.is_empty() {
        pools.materials.retain(|key, _| !retired.contains(&key.0));
    }
    let (hidden, mesh_cost, cost_wanted) =
        (&hide_and_meter.0, &mut hide_and_meter.1, &hide_and_meter.2);
    // The meter is off unless asked (the hover recorder, `WOW_UI_COST=1`): one bool test.
    let cost_on = cost_wanted.0 || crate::ui_script::extract::ui_cost_enabled();
    **mesh_cost = UiMeshCost::default();
    let t_rebuild = cost_on.then(std::time::Instant::now);
    let mut t_mark = t_rebuild;
    let mut lap = move || -> u128 {
        if !cost_on {
            return 0;
        }
        t_mark
            .replace(std::time::Instant::now())
            .map_or(0, |t| t.elapsed().as_micros())
    };
    let (meshes, materials) = (&mut stores.0, &mut stores.1);
    let q = quads.as_mut();
    // TOGGLEUI hides at the draw, not the producers: both lanes keep filling, so the UI returns as
    // it was ([`crate::ui_hide::UiHidden`]). The world is in neither lane (it is this camera's
    // first draw), so retiring every batch leaves the world. While dark, the change flag is still
    // swallowed and the append-lane mirror kept current, so neither lane reports a stale
    // "unchanged" on return. The edge is `UiHidden`'s own change tick.
    if hidden.is_changed() {
        q.dirty = true;
    }
    let lanes_hidden = hidden.0;
    if lanes_hidden {
        q.last_overlays.clone_from(&q.overlays);
        // Nothing moves a pixel while dark, so only the toggle edge rebuilds.
        if !q.dirty {
            return;
        }
    } else if !q.dirty && q.overlays == q.last_overlays {
        return;
    }
    // `WOW_UI_DIFF=1`: names what re-triggered the rebuild, the base lane's dirty flag or the
    // first differing overlay quad.
    if *UI_DIFF {
        if q.dirty {
            eprintln!("[ui-diff] BASE lane dirty");
        } else {
            let i = q
                .overlays
                .iter()
                .zip(&q.last_overlays)
                .position(|(a, b)| a != b);
            match i {
                Some(i) => {
                    let (a, b) = (&q.overlays[i], &q.last_overlays[i]);
                    eprintln!(
                        "[ui-diff] overlay {i}/{} differs: tex={:?} rect {:?} -> {:?} uv_changed={} color_changed={}",
                        q.overlays.len(),
                        a.texture.as_ref().and_then(|t| t.path()),
                        b.rect,
                        a.rect,
                        a.uv != b.uv,
                        a.color != b.color,
                    );
                }
                None => eprintln!(
                    "[ui-diff] overlay COUNT changed: {} -> {}",
                    q.last_overlays.len(),
                    q.overlays.len()
                ),
            }
        }
    }
    q.dirty = false;
    q.last_overlays.clone_from(&q.overlays);

    let (Ok(window), Some(white)) = (windows.single(), white) else {
        // No window, or the white texture not built yet: nothing to draw, and the flag stays
        // cleared rather than spinning.
        retire_batches(&mut pools, &mut commands);
        return;
    };
    let lanes_empty = lanes_hidden || (q.quads.is_empty() && q.overlays.is_empty());
    if lanes_empty {
        retire_batches(&mut pools, &mut commands);
        q.dirty = false;
        q.last_overlays.clone_from(&q.overlays);
        return;
    }

    // Screen px (y-down, top-left origin) to the camera's world space (y-up, centred): the default
    // `OrthographicProjection`, one unit per logical px.
    let (half_w, half_h) = (window.width() * 0.5, window.height() * 0.5);
    let to_world = move |p: Vec2| Vec2::new(p.x - half_w, half_h - p.y);

    // Stable, so equal keys keep producer order, the base lane before the append lane.
    let mut sorted: Vec<&UiQuad> = if lanes_hidden {
        Vec::new()
    } else {
        q.quads.iter().chain(q.overlays.iter()).collect()
    };
    sorted.sort_by_key(|q| q.z_key);
    let n_sorted = sorted.len();
    let us_sort = lap();

    // `WOW_UI_PROBE=1`: every quad's screen rect on each rebuild; the last block is current.
    if *UI_PROBE {
        {
            info!(
                "ui probe: window {}x{} logical",
                window.width(),
                window.height()
            );
            for q in &sorted {
                info!(
                    "ui probe: [{:.0},{:.0} {:.0}x{:.0}] tex={} z={:x}",
                    q.rect.min.x,
                    q.rect.min.y,
                    q.rect.width(),
                    q.rect.height(),
                    q.texture.as_ref().map_or_else(
                        || "-".into(),
                        |h| {
                            h.path()
                                .map_or_else(|| format!("{:?}", h.id()), |p| p.to_string())
                        }
                    ),
                    q.z_key
                );
            }
        }
    }

    // Split into contiguous runs, clipping each quad on the way in; a quad clipped to nothing is
    // never pushed and does not break a run.
    let mut runs: Vec<Run> = Vec::new();
    for q in sorted {
        // Explicit corners (the cooldown pie's wedge) are exact: no clip, rotation or reprojection.
        let plain = if q.corners.is_none() {
            match q.clip {
                Some(clip) => match clip_quad(q.rect, q.uv, clip) {
                    Some(pair) => Some(pair),
                    None => continue,
                },
                None => Some((q.rect, q.uv)),
            }
        } else {
            None
        };
        let texture = q.texture.clone().unwrap_or_else(|| white.0.clone());
        let same_run = runs.last().is_some_and(|r| {
            (r.texture == texture || one_texture_probe())
                && r.additive == q.additive
                && r.circular == q.circular
                && r.desaturated == q.desaturated
                && r.premultiplied == q.premultiplied
                && r.gamma_texel == q.gamma_texel
                && r.alpha_test == q.alpha_test
                && r.mask == q.mask
                && r.uv_clamp == q.uv_clamp
        });
        if !same_run {
            runs.push(Run::new(texture, q));
        }
        let run = runs.last_mut().unwrap();
        match (q.corners, plain) {
            (Some(c), _) => {
                let [tl, tr, br, bl] = q.uv.corners;
                run.push_corners(
                    [
                        (c[0], Vec2::from(tl)),
                        (c[1], Vec2::from(tr)),
                        (c[2], Vec2::from(br)),
                        (c[3], Vec2::from(bl)),
                    ],
                    q.color,
                    to_world,
                );
            }
            (None, Some((rect, uv))) => run.push_quad(rect, uv, q.color, q.rotation, to_world),
            (None, None) => unreachable!("plain is Some when corners is None"),
        }
    }

    // Across runs, ascending mesh z (`bevy_sprite_render`'s `Transparent2d` sort), spread over
    // ±450, inside the camera's ±1000 near/far whatever the run count.
    let us_split = lap();
    let run_count = runs.len().max(1) as f32;
    // The key set is unbounded (a resize moves every mask rect), so it resets; live batches keep
    // their materials through their own handles, which re-enter on the next miss.
    if pools.materials.len() > 256 {
        pools.materials.clear();
    }
    let mut used = 0usize;
    let mut n_rewrites = 0usize;
    for (i, run) in runs.into_iter().enumerate() {
        if run.indices.is_empty() {
            continue;
        }
        let z = -450.0 + (i as f32 / run_count) * 900.0;
        let scale = window.scale_factor();
        let (mask_rect, mask) = match &run.mask {
            Some(m) => (
                Vec4::new(
                    m.rect.min.x * scale,
                    m.rect.min.y * scale,
                    m.rect.max.x * scale,
                    m.rect.max.y * scale,
                ),
                Some(m.texture.clone()),
            ),
            None => (Vec4::new(0.0, 0.0, -1.0, -1.0), None),
        };
        if *UI_PROBE && run.mask.is_some() {
            info!(
                "ui probe: masked run {i}: mask_rect={mask_rect:?} scale={scale} tex={:?}",
                run.mask.as_ref().map(|m| m.texture.id())
            );
        }
        let alpha_ref = run.alpha_test.unwrap_or(0.0);
        let uv_clamp = run.uv_clamp.map_or(UV_CLAMP_OFF, Vec4::from_array);
        let key: MatKey = (
            run.texture.id(),
            run.additive,
            run.circular,
            run.desaturated,
            run.premultiplied,
            run.gamma_texel,
            alpha_ref.to_bits(),
            mask.as_ref().map(bevy::asset::Handle::id),
            mask_rect.to_array().map(f32::to_bits),
            uv_clamp.to_array().map(f32::to_bits),
        );
        // The per-slot skip gate: a rebuild covers the whole stream, but a slot whose run matches
        // its pooled mesh in full is not rewritten. A one-colour run stores white vertices and
        // puts the colour on the batch's tag ([`tint_tag`]), so a pulse or a fade skips too.
        let one_colour = run
            .colors
            .first()
            .filter(|&&c| run.colors.iter().all(|&o| o == c))
            .copied();
        let tag = tint_tag(one_colour.unwrap_or([1.0; 4]));
        let colors = match one_colour {
            Some(_) => vec![[1.0; 4]; run.colors.len()],
            None => run.colors,
        };
        let stored = StoredRun {
            positions: run.positions,
            uvs: run.uvs,
            colors,
            indices: run.indices,
            z_bits: z.to_bits(),
            key,
        };
        while pools.bound.len() <= used {
            pools.bound.push(None);
            pools.tags.push(None);
        }
        let material_handle = pools
            .materials
            .entry(key)
            .or_insert_with(|| {
                materials.add(UiQuadMaterial {
                    additive: u32::from(run.additive),
                    texture: Some(run.texture.clone()),
                    circular: u32::from(run.circular),
                    desaturate: u32::from(run.desaturated),
                    premultiplied: u32::from(run.premultiplied),
                    alpha_ref,
                    gamma_texel: u32::from(run.gamma_texel),
                    mask_rect,
                    mask: mask.clone(),
                    uv_clamp,
                })
            })
            .clone();
        // The pan gate: a run matching its slot's base up to one XY delta moves on the entity's
        // `Transform`, with no mesh write; the unchanged case, `Some(ZERO)`, writes it only to
        // undo an earlier pan. A changed material or tag is one component write.
        let pan = pools
            .stored
            .get(used)
            .and_then(|prev| prev.translation_from(&stored).map(|d| (d, prev.z_bits)));
        if let Some((d, z_bits)) = pan {
            if pools.bound[used] != Some(material_handle.id()) {
                pools.bound[used] = Some(material_handle.id());
                if let Some(&entity) = pools.entities.get(used) {
                    commands
                        .entity(entity)
                        .insert(MeshMaterial2d(material_handle.clone()));
                }
            }
            if pools.tags[used] != Some(tag) {
                pools.tags[used] = Some(tag);
                if let Some(&entity) = pools.entities.get(used) {
                    commands.entity(entity).insert(MeshTag(tag));
                }
            }
            if pools.offsets[used] != d {
                pools.offsets[used] = d;
                if let Some(&entity) = pools.entities.get(used) {
                    commands.entity(entity).insert(Transform::from_xyz(
                        d.x,
                        d.y,
                        f32::from_bits(z_bits),
                    ));
                }
            }
            used += 1;
            continue;
        }
        n_rewrites += 1;
        // `WOW_UI_DIFF=1`, at most 3 lines a second: the check that sent this slot to a rewrite.
        if *UI_DIFF {
            use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
            static SHOWN: AtomicU32 = AtomicU32::new(0);
            static LAST_SEC: AtomicU64 = AtomicU64::new(0);
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let sec = START
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_secs();
            if LAST_SEC.swap(sec, Ordering::Relaxed) != sec {
                SHOWN.store(0, Ordering::Relaxed);
            }
            if SHOWN.fetch_add(1, Ordering::Relaxed) < 3 {
                let why = pools
                    .stored
                    .get(used)
                    .map_or("no-prev", |p| p.translation_miss_reason(&stored));
                eprintln!(
                    "[ui-pan] slot {used} rewrite: {why} (quads={}, key.tex={:?})",
                    stored.positions.len() / 4,
                    stored.key.0
                );
            }
        }
        // Main world too, not render world only: the pool rewrites this asset in place.
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, stored.positions.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, stored.uvs.clone());
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, stored.colors.clone());
        mesh.insert_indices(Indices::U32(stored.indices.clone()));
        if pools.stored.len() > used {
            pools.stored[used] = stored;
        } else {
            pools.stored.push(stored);
        }
        // A rebaked mesh holds absolute positions, so its pan resets (the entity gets `(0, 0, z)`).
        if pools.offsets.len() > used {
            pools.offsets[used] = Vec2::ZERO;
        } else {
            pools.offsets.push(Vec2::ZERO);
        }
        let mesh_handle = match pools.meshes.get(used) {
            Some(handle) => {
                // `WOW_FREEZE_UI_MESH=1` freezes the UI at its first build, pricing the hash probe
                // one modified mesh arms over every `Mesh3d` row (bevy 0.18's `AssetChanged`).
                if !ui_mesh_frozen() {
                    let _ = meshes.insert(handle.id(), mesh);
                }
                handle.clone()
            }
            None => {
                let handle = meshes.add(mesh);
                pools.meshes.push(handle.clone());
                handle
            }
        };
        pools.bound[used] = Some(material_handle.id());
        pools.tags[used] = Some(tag);
        match pools.entities.get(used) {
            Some(&entity) => {
                commands.entity(entity).insert((
                    Mesh2d(mesh_handle),
                    MeshMaterial2d(material_handle),
                    MeshTag(tag),
                    Transform::from_xyz(0.0, 0.0, z),
                ));
            }
            None => {
                let entity = commands
                    .spawn((
                        UiQuadBatch,
                        Mesh2d(mesh_handle),
                        MeshMaterial2d(material_handle),
                        MeshTag(tag),
                        Transform::from_xyz(0.0, 0.0, z),
                        ui_render_layers(),
                    ))
                    .id();
                pools.entities.push(entity);
            }
        }
        used += 1;
    }
    // Entities past this frame's run count despawn; their meshes drop with the truncation.
    for entity in pools.entities.drain(used..) {
        commands.entity(entity).despawn();
    }
    pools.meshes.truncate(used);
    pools.stored.truncate(used);
    pools.offsets.truncate(used);
    if cost_on {
        let us_write = lap();
        **mesh_cost = UiMeshCost {
            rebuilt: true,
            total: t_rebuild.map_or(0, |t| t.elapsed().as_micros()),
            sort: us_sort,
            split: us_split,
            write: us_write,
            quads: n_sorted,
            runs: used,
            rewrites: n_rewrites,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rebuild in isolation: real resources, no renderer.
    fn rebuild_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<UiQuadMaterial>()
            // The pass binds images and hears their removal, so the harness needs the Image asset.
            .init_asset::<Image>()
            .init_resource::<UiQuads>()
            .init_resource::<UiMeshCost>()
            .init_resource::<crate::ui_script::UiCostWanted>()
            .init_resource::<crate::ui_hide::UiHidden>()
            .insert_resource(UiWhiteTexture(Handle::default()))
            .add_systems(Update, rebuild_ui_mesh);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let mut quads = app.world_mut().resource_mut::<UiQuads>();
        quads.quads.push(UiQuad {
            rect: Rect::new(0.0, 0.0, 64.0, 64.0),
            ..default()
        });
        quads.dirty = true;
        app
    }

    /// The WorldFrame's region mirrored ÷G44/÷G48 (`0x483970`) is the viewport, inclusive. The
    /// last row is a measured unit two thirds of a screen below the bottom edge.
    #[test]
    fn the_accept_region_is_the_viewport_inclusive() {
        let vp = Vec2::new(1440.0, 810.0);
        assert!(accepts(Vec2::new(720.0, 405.0), vp));
        assert!(accepts(Vec2::ZERO, vp), "the corner is in view");
        assert!(accepts(vp, vp), "so is the far corner");
        for out in [
            Vec2::new(-0.5, 405.0),
            Vec2::new(1440.5, 405.0),
            Vec2::new(720.0, -0.5),
            Vec2::new(720.0, 810.5),
            Vec2::new(1409.0, 1332.0),
        ] {
            assert!(!accepts(out, vp), "{out:?} is not on screen");
        }
    }

    fn batches(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), With<UiQuadBatch>>()
            .iter(app.world())
            .count()
    }

    fn set_hidden(app: &mut App, hidden: bool) {
        app.world_mut().resource_mut::<crate::ui_hide::UiHidden>().0 = hidden;
    }

    /// Un-hiding brings back the same content, though neither lane changed while dark and the
    /// rebuild would otherwise take its early-out.
    #[test]
    fn toggleui_retires_the_batches_and_restores_them() {
        let mut app = rebuild_app();
        app.update();
        assert_eq!(batches(&mut app), 1, "one batch drawn to begin with");

        set_hidden(&mut app, true);
        app.update();
        assert_eq!(batches(&mut app), 0, "hidden ⇒ nothing drawn");
        // A flagged frame while dark still takes the dark path.
        app.world_mut().resource_mut::<UiQuads>().dirty = true;
        app.update();
        assert_eq!(batches(&mut app), 0, "hidden stays hidden");

        set_hidden(&mut app, false);
        app.update();
        assert_eq!(
            batches(&mut app),
            1,
            "the UI comes back on the same content"
        );
    }

    /// The cache's strong handle would otherwise pin the image and its GPU texture for good.
    #[test]
    fn a_removed_image_does_not_keep_its_material_alive() {
        let mut app = rebuild_app();
        let art = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::new_fill(
                Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                &[255; 4],
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::default(),
            ));
        let art_id = art.id();
        {
            let mut q = app.world_mut().resource_mut::<UiQuads>();
            q.quads.push(UiQuad {
                rect: Rect::new(0.0, 0.0, 32.0, 32.0),
                texture: Some(art.clone()),
                ..default()
            });
            q.dirty = true;
        }
        app.update();
        assert!(
            names_image(&app, art_id),
            "the textured quad must have produced a material naming that image"
        );

        // Drop every quad that names it, then remove the asset: only the cache still holds it.
        drop(art);
        {
            let mut q = app.world_mut().resource_mut::<UiQuads>();
            q.quads.retain(|quad| quad.texture.is_none());
            q.dirty = true;
        }
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .remove(art_id);
        // Three one-frame lags: `Removed` is written in `PostUpdate` and read in `Update`, the
        // retired batch's handle drops at that frame's command flush, and `track_assets` reclaims
        // it the next `PreUpdate`.
        for _ in 0..4 {
            app.update();
        }

        assert!(
            !names_image(&app, art_id),
            "a material for a removed image is dead weight: it pins the image AND the GPU \
             texture behind its prepared bind group for as long as the cache holds the key"
        );
    }

    /// Does any live `UiQuadMaterial` still name this image?
    fn names_image(app: &App, id: AssetId<Image>) -> bool {
        app.world()
            .resource::<Assets<UiQuadMaterial>>()
            .iter()
            .any(|(_, m)| m.texture.as_ref().is_some_and(|t| t.id() == id))
    }

    /// The world is the UI camera's first draw (`benilla_world::ffx_glow::FfxBackdrop`), not a
    /// batch, so dark leaves the world and nothing else; a batch alive while dark is a bug.
    #[test]
    fn toggleui_retires_every_batch_and_the_world_is_none_of_them() {
        let mut app = rebuild_app();
        app.update();
        let lit = batches(&mut app);
        assert!(lit >= 1, "content drawn to begin with");

        set_hidden(&mut app, true);
        app.update();
        assert_eq!(
            batches(&mut app),
            0,
            "dark ⇒ nothing drawn: the world is this camera's own pass, not a batch"
        );

        set_hidden(&mut app, false);
        app.update();
        assert_eq!(
            batches(&mut app),
            lit,
            "the UI comes back over the same world"
        );
    }

    /// The greyscale is a per-material uniform: merged, the first quad would decide both (a talent
    /// tree greying in blocks).
    #[test]
    fn desaturation_splits_the_run_and_lands_on_the_material() {
        let mut app = rebuild_app();
        // Same texture (the shared white), same blend, adjacent in z: only `desaturated` differs.
        let mut quads = app.world_mut().resource_mut::<UiQuads>();
        quads.quads.push(UiQuad {
            rect: Rect::new(64.0, 0.0, 128.0, 64.0),
            z_key: 1,
            desaturated: true,
            ..default()
        });
        quads.dirty = true;
        app.update();
        assert_eq!(
            batches(&mut app),
            2,
            "a desaturated quad cannot share a material with a full-colour one"
        );

        let mut flags: Vec<u32> = app
            .world_mut()
            .resource::<Assets<UiQuadMaterial>>()
            .iter()
            .map(|(_, m)| m.desaturate)
            .collect();
        flags.sort_unstable();
        assert_eq!(
            flags,
            vec![0, 1],
            "one material greys and one does not — the uniform the shader branches on"
        );
    }

    /// `SetVertexColor` quantises `×255 + 0.5` into one byte per channel (`0x79abd0`).
    #[test]
    fn tint_tag_is_the_reference_byte_colour_complemented() {
        assert_eq!(tint_tag([1.0; 4]), 0, "white is the untagged default");
        assert_eq!(
            tint_tag([0.0; 4]),
            u32::MAX,
            "transparent black is a real colour, not a missing tag"
        );
        // Gold at half alpha: 255, 224 (224.9 truncated), 64 (64.25), 128 (128.0), r in the low
        // byte as `unpack4x8unorm` reads it.
        assert_eq!(
            tint_tag([1.0, 0.88, 0.25, 0.5]),
            !(255 | 224 << 8 | 64 << 16 | 128 << 24)
        );
        assert_eq!(
            tint_tag([2.0, -1.0, 0.5, 1.0]),
            !(255 | 128 << 16 | 255 << 24),
            "clamped to [0, 1] first, as the Lua binding clamps"
        );
    }

    /// A colour pulse (the stock PlayerFrame status glow, every frame at rest) must leave the
    /// `WOW_UI_COST=1` material-event count at zero: no mesh write, no material write.
    #[test]
    fn a_colour_pulse_writes_the_tag_and_neither_asset() {
        let mut app = rebuild_app();
        // The meter is off unless asked, and this test reads `rewrites`.
        app.world_mut()
            .resource_mut::<crate::ui_script::UiCostWanted>()
            .0 = true;
        app.update();
        assert_eq!(batches(&mut app), 1);
        // A cursor, not the current buffer: `TimePlugin` swaps message buffers only after a fixed
        // step (`signal_message_update_system`), and this harness ticks faster, so frame 1's
        // events are still current on frame 2.
        let mut cursor = bevy::ecs::message::MessageCursor::<AssetEvent<UiQuadMaterial>>::default();
        let mut material_events = |app: &App| {
            let (mut added, mut modified) = (0, 0);
            for e in cursor.read(
                app.world()
                    .resource::<Messages<AssetEvent<UiQuadMaterial>>>(),
            ) {
                match e {
                    AssetEvent::Added { .. } => added += 1,
                    AssetEvent::Modified { .. } => modified += 1,
                    _ => {}
                }
            }
            (added, modified)
        };
        assert_eq!(
            material_events(&app),
            (1, 0),
            "the first frame builds the run's one material (and this harness hears it)"
        );
        let (batch, tag0) = app
            .world_mut()
            .query_filtered::<(Entity, &MeshTag), With<UiQuadBatch>>()
            .single(app.world())
            .expect("one batch");
        assert_eq!(
            **tag0,
            tint_tag([1.0; 4]),
            "a white quad carries the white tag"
        );

        let gold = [1.0, 0.88, 0.25, 0.5];
        {
            let mut q = app.world_mut().resource_mut::<UiQuads>();
            q.quads[0].color = gold;
            q.dirty = true;
        }
        app.update();

        let cost = app.world().resource::<UiMeshCost>();
        assert!(cost.rebuilt, "the dirty flag ran the rebuild");
        assert_eq!(
            cost.rewrites, 0,
            "no pooled mesh was rewritten for a colour"
        );
        assert_eq!(
            material_events(&app),
            (0, 0),
            "no material was built or rebuilt for a colour — a colour is a tag, not a material"
        );
        assert_eq!(
            app.world().resource::<Assets<UiQuadMaterial>>().len(),
            1,
            "still the one material: colour is not part of its identity"
        );
        let tag = app
            .world()
            .get::<MeshTag>(batch)
            .expect("the batch keeps its tag");
        assert_eq!(**tag, tint_tag(gold), "the colour went to the instance tag");
        let mesh_handle = app.world().get::<Mesh2d>(batch).expect("the batch's mesh");
        let mesh = app
            .world()
            .resource::<Assets<Mesh>>()
            .get(&mesh_handle.0)
            .expect("the pooled mesh");
        let Some(bevy::mesh::VertexAttributeValues::Float32x4(colors)) =
            mesh.attribute(Mesh::ATTRIBUTE_COLOR)
        else {
            panic!("the run's vertex colours are Float32x4");
        };
        assert!(
            colors.iter().all(|c| *c == [1.0; 4]),
            "the vertices stay white: the colour is the tag's, not the mesh's"
        );
    }

    // PlayerFrameTexture's `<TexCoords left="1.0" right="0.09375">` (`PlayerFrame.xml:56`) must
    // reach the vertex buffer mirrored; a normalizing `Rect` would un-mirror the ring.
    #[test]
    fn mirrored_tex_coords_emit_unnormalized_corners() {
        // `[left, right, top, bottom]` as the extract hands them over.
        let uv = UvRect::from_tex_coords([1.0, 0.09375, 0.0, 0.78125]);
        let [tl, tr, br, bl] = uv.corners;
        assert!(
            tl[0] > tr[0],
            "mirror preserved: TL.u {} > TR.u {}",
            tl[0],
            tr[0]
        );
        assert_eq!(tl, [1.0, 0.0]);
        assert_eq!(tr, [0.09375, 0.0]);
        assert_eq!(br, [0.09375, 0.78125]);
        assert_eq!(bl, [1.0, 0.78125]);
    }

    #[test]
    fn clip_preserves_mirrored_uv() {
        let uv = UvRect::from_tex_coords([1.0, 0.09375, 0.0, 0.78125]);
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let clip = Rect::new(0.0, 0.0, 50.0, 100.0);
        let (crect, cuv) =
            clip_quad(rect, uv, clip).expect("left half is a non-empty intersection");
        assert_eq!(crect, Rect::new(0.0, 0.0, 50.0, 100.0));
        let [tl, tr, ..] = cuv.corners;
        assert!(
            tl[0] > tr[0],
            "mirror survives clip: TL.u {} > TR.u {}",
            tl[0],
            tr[0]
        );
        assert_eq!(tl[0], 1.0);
        assert!((tr[0] - 0.546_875).abs() < 1e-6); // lerp(1.0, 0.09375, 0.5)
        assert_eq!(tl[1], 0.0);
        assert_eq!(cuv.corners[3][1], 0.78125); // BL.v
    }

    // A backdrop top edge maps atlas u to screen Y and v to screen X reversed (`0x77f0c0`), so
    // clipping the left half reprojects v and leaves u.
    #[test]
    fn clip_reprojects_rotated_uv() {
        // Top edge with widthRun = 4.
        let uv = UvRect::from_corners([[0.25, 4.0], [0.25, 0.0], [0.375, 0.0], [0.375, 4.0]]);
        let rect = Rect::new(0.0, 0.0, 100.0, 16.0);
        let clip = Rect::new(0.0, 0.0, 50.0, 16.0);
        let (crect, cuv) = clip_quad(rect, uv, clip).expect("left half intersects");
        assert_eq!(crect, Rect::new(0.0, 0.0, 50.0, 16.0));
        let [tl, tr, br, bl] = cuv.corners;
        assert!((tl[0] - 0.25).abs() < 1e-6 && (tr[0] - 0.25).abs() < 1e-6);
        assert!((bl[0] - 0.375).abs() < 1e-6 && (br[0] - 0.375).abs() < 1e-6);
        assert!((tl[1] - 4.0).abs() < 1e-6, "left v kept: {}", tl[1]);
        assert!((tr[1] - 2.0).abs() < 1e-6, "right v halved: {}", tr[1]);
    }
}
