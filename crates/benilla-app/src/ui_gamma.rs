//! The player-UI **gamma composite lane**'s single decode (decision 0254) — the UI-arc twin of
//! [`benilla_world::ffx_glow`], which owns the world lane's one decode (0161).
//!
//! [`ui_quad.wgsl`](shaders/ui_quad.wgsl) composites the UI in gamma bytes, the way the
//! reference's fixed-function device composites into its 8-bit backbuffer: every tint is a gamma
//! multiply, every blend is gamma arithmetic clamped at each write, and `alphaMode="ADD"` is the
//! byte add `dst + texel·α` (EGxBlend 3 = `glBlendFunc(GL_SRC_ALPHA, GL_ONE)`;
//! factor tables `0x85c1f8`/`0x85c224`). This node converts that finished gamma
//! image to linear ONCE, rendering straight into the swapchain — the camera's output mode is
//! `Skip`, so there is no output blit (decision 2206, [`benilla_world::final_pass`]) — whose sRGB
//! write re-encodes it to the exact client byte.
//!
//! The node is **mandatory** on the player-UI camera: without it the whole UI presents ~2.2× bright
//! (the same failure mode `$WOW_NO_FFX` produces for the world). It is gated on [`UiGammaLane`], so
//! it runs on that camera and no other `Camera2d` sharing the `Core2d` graph (the egui dev overlay).
//!
//! The module also carries the lane's **Bevy-UI half** (decision 0541): the glue + loading screens
//! are Bevy UI trees rather than quads, so they need the same gamma conversion in their own shaders
//! before this decode is correct for them — see [`use_gamma_ui_shaders`]. That is why
//! [`crate::ui_pass`] marks the player-UI camera `IsDefaultUiCamera`: Bevy UI has to land on the one
//! camera this node runs on.

use bevy::core_pipeline::core_2d::graph::{Core2d, Node2d};
use bevy::core_pipeline::FullscreenShader;
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};
use bevy::ui_render::graph::NodeUi;
use bevy::ui_render::ui_texture_slice_pipeline::{
    init_ui_texture_slice_pipeline, UiTextureSlicePipeline,
};
use bevy::ui_render::{init_ui_pipeline, UiPipeline};

use benilla_world::final_pass::FinalPassTarget;

/// Marks the camera whose target holds a gamma-composited image awaiting this pass's one decode,
/// and carries the [`DisplayGamma`] exponent that decode applies on the way through (2182).
#[derive(Component, Clone, Copy, ExtractComponent)]
pub(crate) struct UiGammaLane {
    /// The `gamma` CVar's value, ready for the shader. `1.0` is the identity ramp.
    pub(crate) gamma: f32,
}

impl Default for UiGammaLane {
    fn default() -> Self {
        Self {
            gamma: DEFAULT_GAMMA,
        }
    }
}

/// **The display-brightness correction** (decision 2182) — the `gamma` CVar, as the reference
/// registers it (`0x402d70`, name `0x82e924`, default string `0x82e92c` = `"1.0"`, flags 0).
///
/// The reference applies it as an OS **hardware gamma ramp**: its change callback `0x4034d0`
/// builds `ramp[i] = __ftol(pow(i · 1/255, gamma) · 65535)` at `0x591680` and hands the 3×256 words
/// to `GDI32!SetDeviceGammaRamp` (there is no other whole-frame grade in the client, and
/// `Brightness`/`Contrast` CVars do not exist in the binary at all). That upload is **skipped
/// windowed** — `byte[dev+0x20b]` is `CGxFormat +0x07`, which is `gxWindow` — and windowed is every
/// mode benilla has (1627), so copying the mechanism byte for byte would ship a slider that never
/// moves a pixel.
///
/// So the ramp goes where a ramp goes when you own the compositor: **the same curve, applied to the
/// same values, one stage later.** `i/255` is the framebuffer byte the RAMDAC would have read;
/// [`UiGammaNode`] samples exactly that value (the UI lane composites in gamma bytes — the module
/// doc above) and raising it to `gamma` before the one decode is the continuous form of the
/// reference's 256-entry LUT.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub(crate) struct DisplayGamma(pub(crate) f32);

/// The reference's registered `"1.0"` — the identity ramp, and the value the pass is a **no-op**
/// at, byte for byte (see [`UiGammaNode`]'s uniform branch).
pub(crate) const DEFAULT_GAMMA: f32 = 1.0;

/// **The clamp is ours, and the reference has none** — `SetGamma(5)` writes `gamma = "-4.000000"`
/// there with no arm anywhere to catch it (`SetGamma 0x4891f0`, with
/// `baseMip`'s validating callback `0x689090` as the positive control). The reference can afford
/// that because its ramp is a fullscreen-only OS call a player can escape by alt-tabbing; ours is
/// the image itself, and `pow(x, 12)` is a black screen with the panel that undoes it somewhere
/// inside it. This range is the widest that keeps the client legible enough to reach that panel —
/// it spans the stock slider's own `[0.5, 1.5]` four times over, so nothing a player can do from
/// the UI ever meets it, and the CVar still keeps whatever truth was written to it (0959's posture:
/// the consumer clamps at its own edge, the store does not lie).
pub(crate) const GAMMA_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

impl Default for DisplayGamma {
    fn default() -> Self {
        Self(DEFAULT_GAMMA)
    }
}

/// Carry the knob onto the camera the pass runs on. Change-detected on the resource, so a settled
/// session writes nothing.
fn stamp_lane_gamma(gamma: Res<DisplayGamma>, mut lanes: Query<&mut UiGammaLane>) {
    if !gamma.is_changed() {
        return;
    }
    for mut lane in &mut lanes {
        lane.gamma = gamma.0.clamp(*GAMMA_RANGE.start(), *GAMMA_RANGE.end());
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct UiGammaLabel;

/// The decode's shared halves. The pipeline itself is specialised per view on the format it
/// renders in ([`ViewUiGammaPipeline`]).
#[derive(Resource)]
struct UiGammaPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
    /// The ramp exponent's 16-byte uniform, rewritten each frame with a queue write (which lands
    /// before the graph's submit executes) — the same shape `ffx_glow`'s combine uses. One buffer,
    /// not one per view: [`UiGammaLane`] is on exactly one camera.
    ramp: Buffer,
}

impl SpecializedRenderPipeline for UiGammaPipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("ui_gamma_decode".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs: vec![],
                entry_point: Some("fs_decode".into()),
                // An 8-bit sRGB target either way: the swapchain's view for the player-UI camera
                // (`Skip`, 2206), or — for a `Write` camera — its own main texture, which is not
                // `Hdr` and so carries Bevy's default 8-bit sRGB format, the one that makes the
                // lane clamp at every blend like the reference.
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            ..default()
        }
    }
}

/// The decode pipeline for one view — specialised on where the decode lands (2206,
/// [`benilla_world::final_pass`]): the swapchain, for the player-UI camera, whose output mode is
/// `Skip`. Stamped every frame, the way bevy stamps its own `ViewUpscalingPipeline`.
#[derive(Component)]
struct ViewUiGammaPipeline(CachedRenderPipelineId);

fn prepare_view_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Res<UiGammaPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<UiGammaPipeline>>,
    views: Query<(Entity, &ExtractedCamera, &ViewTarget), With<UiGammaLane>>,
) {
    for (entity, camera, target) in &views {
        let format = FinalPassTarget::format(&camera.output_mode, target);
        let id = pipelines.specialize(&pipeline_cache, &pipeline, format);
        commands.entity(entity).insert(ViewUiGammaPipeline(id));
    }
}

fn init_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "ui_gamma_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // The ramp exponent, as a `vec4<f32>` — one scalar, but a uniform struct is
                // 16-byte aligned and a vec4 says so without a padding field to keep in step.
                uniform_buffer_sized(false, Some(std::num::NonZero::new(16).unwrap())),
            ),
        ),
    );
    // Point sampling: this is a 1:1 full-screen resolve, never a resample.
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Nearest,
        mag_filter: FilterMode::Nearest,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..Default::default()
    });
    let ramp = render_device.create_buffer(&BufferDescriptor {
        label: Some("ui_gamma_ramp"),
        size: 16,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    commands.insert_resource(UiGammaPipeline {
        layout,
        sampler,
        shader: asset_server.load("embedded://benilla_app/shaders/ui_gamma.wgsl"),
        fullscreen: fullscreen_shader.clone(),
        ramp,
    });
}

#[derive(Default)]
struct UiGammaNode;

impl ViewNode for UiGammaNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static UiGammaLane,
        &'static ExtractedCamera,
        &'static ViewUiGammaPipeline,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (view_target, lane, camera, pipeline): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let pipelines = world.resource::<UiGammaPipeline>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let Some(decode) = pipeline_cache.get_render_pipeline(pipeline.0) else {
            // Still compiling. Skipping leaves the UI undecoded (over-bright) for a frame or two,
            // which beats dropping the UI entirely.
            return Ok(());
        };
        // The ramp exponent for this frame (2182). `[g, 0, 0, 0]` — the shader reads `.x`.
        world.resource::<RenderQueue>().write_buffer(
            &pipelines.ramp,
            0,
            bytemuck::cast_slice(&[lane.gamma, 0.0, 0.0, 0.0]),
        );
        // Where the decode lands (2206, `final_pass`): the swapchain itself for the player-UI
        // camera (`Skip`), the ping-pong for a `Write` camera.
        let out = FinalPassTarget::resolve(camera, view_target);
        let layout = pipeline_cache.get_bind_group_layout(&pipelines.layout);
        let bind = render_context.render_device().create_bind_group(
            "ui_gamma_decode",
            &layout,
            &BindGroupEntries::sequential((
                out.source,
                &pipelines.sampler,
                pipelines.ramp.as_entire_binding(),
            )),
        );
        // Its diagnostic span: the journal's `gpu_ui` column counts this decode (2008).
        let diagnostics = render_context.diagnostic_recorder();
        let scissor = out.scissor_rect();
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("ui_gamma_decode"),
                color_attachments: &[Some(out.destination)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        if let Some((x, y, w, h)) = scissor {
            pass.set_scissor_rect(x, y, w, h);
        }
        let span = diagnostics.pass_span(&mut pass, "ui_gamma_decode");
        pass.set_pipeline(decode);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        span.end(&mut pass);
        Ok(())
    }
}

/// Put **Bevy UI** on the gamma lane too (decision 0541): swap the two stock pipelines' shaders for
/// our gamma-emitting copies.
///
/// The glue screens (login / character select / character create) and the loading screen are Bevy UI
/// trees, not [`crate::ui_pass`] quads, so they never went through `ui_quad.wgsl`'s conversion — they
/// composited in LINEAR while the rest of the client composited in the reference's gamma bytes. Over
/// the login scene's bright sky that washed the edit-box fills from byte ~77 to ~138 (measured).
///
/// Bevy UI exposes no colour-space hook, so the conversion goes where `ui_quad.wgsl` puts it: the
/// fragment shader. [`UiPipeline`] and [`UiTextureSlicePipeline`] each hold a public `shader` handle
/// they clone into every specialisation, so replacing it here — once, at `RenderStartup`, after the
/// upstream init systems have built the resources — redirects every UI draw with no other change:
/// art keeps loading as sRGB, colours keep being authored as `Color::srgb`, and the booth's linear
/// scene image is converted by the same `linear_to_srgb` as everything else.
///
/// The shaders are vendored copies (see their headers) — the maintenance cost of Bevy not exposing
/// the seam. The long-term exit is decision 0068's engine: the reference's glue screens are
/// themselves FrameXML, so when they migrate onto our own quad pass this whole module's Bevy-UI half
/// is deleted rather than maintained.
fn use_gamma_ui_shaders(
    asset_server: Res<AssetServer>,
    mut node: ResMut<UiPipeline>,
    mut slice: ResMut<UiTextureSlicePipeline>,
) {
    node.shader = asset_server.load("embedded://benilla_app/shaders/ui_node_gamma.wgsl");
    slice.shader = asset_server.load("embedded://benilla_app/shaders/ui_slice_gamma.wgsl");
}

pub(crate) struct UiGammaPlugin;

/// Brightness's change callback (2182, 2303). The clamp is OURS and the reference has none —
/// the reason it costs one is on [`GAMMA_RANGE`], and nothing a player can reach from the panel
/// meets it.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut gamma: ResMut<DisplayGamma>) {
    if ev.is(benilla_ui::script::CVAR_GAMMA) {
        gamma.0 = ev.num().clamp(*GAMMA_RANGE.start(), *GAMMA_RANGE.end());
    }
}

impl Plugin for UiGammaPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<DisplayGamma>()
            .add_plugins(ExtractComponentPlugin::<UiGammaLane>::default())
            .add_systems(Update, stamp_lane_gamma);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<UiGammaPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(
                Render,
                prepare_view_pipelines.in_set(RenderSystems::Prepare),
            )
            .add_systems(
                RenderStartup,
                use_gamma_ui_shaders
                    .after(init_ui_pipeline)
                    .after(init_ui_texture_slice_pipeline),
            )
            .add_render_graph_node::<ViewNodeRunner<UiGammaNode>>(Core2d, UiGammaLabel)
            // After the quads are composited, in the slot before the output blit's — which a
            // `Skip` camera leaves empty: the decode IS the output write.
            .add_render_graph_edges(
                Core2d,
                (Node2d::EndMainPass, UiGammaLabel, Node2d::Upscaling),
            )
            // ...and after the Bevy-UI subgraph, whose nodes now write gamma into the same target.
            // Upstream orders `UiPass` only against `EndMainPass`/`Upscaling`, which leaves it and
            // the decode mutually unordered — the glue screens would decode or not depending on how
            // the graph happened to sort. This edge pins it.
            .add_render_graph_edge(Core2d, NodeUi::UiPass, UiGammaLabel);
    }
}
