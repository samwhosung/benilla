//! The effect lane: the render-world half of the shared effect stream, drawn as [`Transparent3d`]
//! items in bevy_ui_render's shape. Queue sorts each draw at its anchor's view z plus its rung,
//! the material path's metric; prepare, after `PhaseSort`, rebases and uploads the vertices and
//! merges sort-adjacent items sharing a `RunKey`: the phase renderer advances by
//! `batch_range.len()`, so an opener spanning its followers absorbs them. Bevy's mesh batcher
//! leaves the items alone while their main entities own no mesh.

use std::ops::Range;

use bevy::asset::{AssetEvent, AssetId};
use bevy::core_pipeline::core_3d::{Transparent3d, CORE_3D_DEPTH_FORMAT};
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::ecs::system::SystemParamItem;
use bevy::image::BevyDefault as _;
use bevy::mesh::VertexBufferLayout;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
};
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferId, BufferUsages, ColorTargetState,
    ColorWrites, CompareFunction, DepthBiasState, DepthStencilState, DynamicUniformBuffer,
    FragmentState, IndexFormat, MultisampleState, PipelineCache, PrimitiveState, RawBufferVec,
    RenderPipelineDescriptor, SamplerBindingType, ShaderStages, SpecializedRenderPipeline,
    SpecializedRenderPipelines, StencilState, TextureFormat, TextureSampleType, VertexFormat,
    VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::view::{
    ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use crate::lighting::SharedLightBuffer;

use super::buffer::{EffectBlend, EffectFog, EffectQuads, EffectTopology, EffectVertex};

/// One extracted draw: the render-world copy of a [`super::buffer::EffectDraw`].
struct ExtractedDraw {
    cam: Entity,
    main_entity: Entity,
    texture: AssetId<Image>,
    blend: EffectBlend,
    topology: EffectTopology,
    fog: EffectFog,
    lit: bool,
    anchor: Vec3,
    bias: f32,
    raster_bias: i32,
    raster_slope: f32,
    cam_relative: bool,
    no_depth_test: bool,
    range: Range<u32>,
    /// A booth's light buffer; `None` is the world's.
    light: Option<Buffer>,
    clip: Option<Vec4>,
}

/// One GPU draw after the merge walk: an index range and the bind-group identity its items share.
struct MergedDraw {
    index_range: Range<u32>,
    texture: AssetId<Image>,
    light: Option<Buffer>,
    /// This draw's fog and clip row in [`EffectMeta::params`].
    params_offset: u32,
}

/// The lane's per-frame GPU state: vertices, indices, draws, merged draws and params rows.
#[derive(Resource)]
pub struct EffectMeta {
    vertices: RawBufferVec<EffectVertex>,
    indices: RawBufferVec<u32>,
    draws: Vec<ExtractedDraw>,
    merged: Vec<MergedDraw>,
    view_bind_group: Option<BindGroup>,
    params: DynamicUniformBuffer<EffectParams>,
    /// The six canonical fog rows' dynamic offsets, in [`EffectFog::slot`] order.
    params_offsets: [u32; 6],
    /// The params buffer last written; a new one invalidates the bind-group cache.
    params_buffer: Option<BufferId>,
}

/// One per-draw uniform row: the fog policy and the target-pixel clip (`clip.z <= clip.x`: none).
#[derive(Clone, Copy, bevy::render::render_resource::ShaderType)]
struct EffectParams {
    fog: Vec4,
    clip: Vec4,
}

impl Default for EffectMeta {
    fn default() -> Self {
        Self {
            vertices: RawBufferVec::new(BufferUsages::VERTEX),
            indices: RawBufferVec::new(BufferUsages::INDEX),
            draws: Vec::new(),
            merged: Vec::new(),
            view_bind_group: None,
            params: DynamicUniformBuffer::default(),
            params_offsets: [0; 6],
            params_buffer: None,
        }
    }
}

/// Clipped params rows per frame; a pane past it draws unclipped (a pet bar and two pings is ten).
const MAX_CLIP_ROWS: usize = 64;

impl EffectMeta {
    /// Write this frame's uniform rows and return the clipped rows' offsets by (fog slot, clip
    /// bits). Rows 0..6 are the fog policies in [`EffectFog::slot`] order: `fog.x` the colour
    /// policy (`0x70baf0`), `fog.y` rain's forced fog with `zw` its start and end.
    fn build_params(
        &mut self,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) -> HashMap<(u32, [u32; 4]), u32> {
        const NO_CLIP: Vec4 = Vec4::ZERO;
        let fog_rows = [
            Vec4::new(0.0, 0.0, 0.0, 0.0),
            Vec4::new(1.0, 0.0, 0.0, 0.0),
            Vec4::new(2.0, 0.0, 0.0, 0.0),
            Vec4::new(3.0, 0.0, 0.0, 0.0),
            Vec4::new(4.0, 0.0, 0.0, 0.0),
            Vec4::new(
                0.0,
                1.0,
                crate::weather::RAIN_FOG_START,
                crate::weather::RAIN_FOG_END,
            ),
        ];
        self.params.clear();
        let mut offsets = [0u32; 6];
        for (i, fog) in fog_rows.iter().enumerate() {
            offsets[i] = self.params.push(&EffectParams {
                fog: *fog,
                clip: NO_CLIP,
            });
        }
        self.params_offsets = offsets;
        let mut clip_rows: HashMap<(u32, [u32; 4]), u32> = HashMap::default();
        for i in 0..self.draws.len() {
            let (Some(clip), slot) = (self.draws[i].clip, self.draws[i].fog.slot()) else {
                continue;
            };
            let key = (slot, clip.to_array().map(f32::to_bits));
            if clip_rows.contains_key(&key) || clip_rows.len() >= MAX_CLIP_ROWS {
                continue;
            }
            let off = self.params.push(&EffectParams {
                fog: fog_rows[slot as usize],
                clip,
            });
            clip_rows.insert(key, off);
        }
        self.params.write_buffer(device, queue);
        self.params_buffer = self.params.buffer().map(|b| b.id());
        clip_rows
    }
}

/// Bind groups per (texture, light buffer), cached until the image changes; the light key is
/// `None` for the world's buffer, which is never re-created.
#[derive(Resource, Default)]
pub struct EffectBindGroups {
    images: HashMap<(AssetId<Image>, Option<BufferId>), BindGroup>,
    /// The params buffer the cached groups bind.
    params_buffer: Option<BufferId>,
}

/// The lane's pipeline: layouts and shader, specialized per (blend, raster bias, msaa, hdr).
#[derive(Resource)]
pub struct EffectPipeline {
    view_layout: BindGroupLayoutDescriptor,
    image_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

pub fn init_effect_pipeline(mut commands: Commands, asset_server: Res<AssetServer>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "effect_view_layout",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX_FRAGMENT,
            uniform_buffer::<ViewUniform>(true),
        ),
    );
    let image_layout = BindGroupLayoutDescriptor::new(
        "effect_image_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                // The shared light and fog blob, sized at bind time; the WGSL struct pins it.
                storage_buffer_read_only_sized(false, None),
                // The per-draw params row, by dynamic offset.
                uniform_buffer::<EffectParams>(true),
            ),
        ),
    );
    commands.insert_resource(EffectPipeline {
        view_layout,
        image_layout,
        shader: asset_server.load("embedded://benilla_world/shaders/wow_effect.wgsl"),
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectPipelineKey {
    samples: u32,
    hdr: bool,
    blend: EffectBlend,
    /// The rasterizer depth-bias constant, 0 for free-floating geometry; nonzero also selects
    /// `DECAL_WORLD_CLIP`, absolute verts through the mesh path's `clip_from_world`.
    raster_bias: i32,
    /// The depth-bias slope scale as bits: `f32` is not `Hash`.
    raster_slope_bits: u32,
    /// The fragment multiplies by the scene's light (`EFFECT_LIT`).
    lit: bool,
    /// Depth compare `Always`; see [`super::buffer::EffectDrawSpec::no_depth_test`].
    no_depth_test: bool,
}

impl SpecializedRenderPipeline for EffectPipeline {
    type Key = EffectPipelineKey;

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let vertex_layout = VertexBufferLayout::from_vertex_formats(
            VertexStepMode::Vertex,
            vec![
                // position: camera-relative, absolute for decals
                VertexFormat::Float32x3,
                // uv
                VertexFormat::Float32x2,
                // color: raw authored gamma RGBA
                VertexFormat::Float32x4,
            ],
        );
        let (blend, depth_write, blend_def) = match key.blend {
            EffectBlend::Add => (
                Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                false,
                "BLEND_ADD",
            ),
            EffectBlend::Alpha => (Some(BlendState::ALPHA_BLENDING), false, "BLEND_ALPHA"),
            EffectBlend::Opaque => (None, true, "BLEND_OPAQUE"),
            // EGxBlend 1: Opaque's state, its alpha test in the shader (wgpu has no `glAlphaFunc`).
            EffectBlend::AlphaKey => (None, true, "BLEND_ALPHAKEY"),
            EffectBlend::Multiply => (
                Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Dst,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent::OVER,
                }),
                false,
                "BLEND_MULTIPLY",
            ),
            EffectBlend::Mod2x => (
                Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Dst,
                        dst_factor: BlendFactor::Src,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                }),
                false,
                "BLEND_MOD2X",
            ),
        };
        let mut shader_defs = vec![blend_def.into()];
        // Coplanar draws: absolute verts through the world meshes' own `clip_from_world`, so the
        // bias settles against the same rounding; `prepare_effects` skips their rebase to match.
        if key.raster_bias != 0 {
            shader_defs.push("DECAL_WORLD_CLIP".into());
        }
        if key.lit {
            shader_defs.push("EFFECT_LIT".into());
        }
        // `$WOW_PARTICLE_FLAT`: solid magenta, no fragment inputs.
        if std::env::var_os("WOW_PARTICLE_FLAT").is_some() {
            shader_defs.push("WOW_PARTICLE_FLAT".into());
        }
        // `$WOW_PARTICLE_NODEPTH`: depth compare `Always`, to tell occluded from never drawn.
        let depth_compare =
            if key.no_depth_test || std::env::var_os("WOW_PARTICLE_NODEPTH").is_some() {
                CompareFunction::Always
            } else {
                CompareFunction::GreaterEqual
            };
        RenderPipelineDescriptor {
            label: Some("wow_effect_pipeline".into()),
            layout: vec![self.view_layout.clone(), self.image_layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                buffers: vec![vertex_layout],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState {
                    format: if key.hdr {
                        ViewTarget::TEXTURE_FORMAT_HDR
                    } else {
                        TextureFormat::bevy_default()
                    },
                    blend,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                cull_mode: None, // billboards, trails, decals: never backface-cull
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: depth_write,
                depth_compare,
                stencil: StencilState::default(),
                bias: DepthBiasState {
                    constant: key.raster_bias,
                    slope_scale: f32::from_bits(key.raster_slope_bits),
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..default()
            },
            ..default()
        }
    }
}

/// Copy the frame's stream in, and drop the cached bind groups of images that changed.
fn extract_effects(
    mut meta: ResMut<EffectMeta>,
    mut bind_groups: ResMut<EffectBindGroups>,
    quads: Extract<Res<EffectQuads>>,
    mut image_events: Extract<MessageReader<AssetEvent<Image>>>,
) {
    for event in image_events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id } => {
                bind_groups.images.retain(|(image, _), _| image != id);
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
    meta.vertices.clear();
    meta.vertices.extend(quads.verts.iter().copied());
    meta.draws.clear();
    meta.draws.extend(quads.draws.iter().map(|d| ExtractedDraw {
        cam: d.cam,
        main_entity: d.main_entity,
        texture: d.texture,
        blend: d.blend,
        topology: d.topology,
        fog: d.fog,
        lit: d.lit,
        anchor: d.anchor,
        bias: d.bias,
        raster_bias: d.raster_bias,
        raster_slope: d.raster_slope,
        cam_relative: d.cam_relative,
        no_depth_test: d.no_depth_test,
        range: d.range.clone(),
        light: d.light.clone(),
        clip: d.clip,
    }));
}

/// `$WOW_EFFECT_TRACE`'s name for a surface-decal draw, keyed on its sort rung, the one field
/// unique per decal lane; `None` for everything else.
fn decal_lane(bias: f32) -> Option<&'static str> {
    use crate::sky_order::Rung;
    match bias {
        b if b == Rung::RING => Some("ring"),
        b if b == Rung::SHADOW_SORT => Some("shadow"),
        b if b == Rung::FOOTPRINT => Some("footprint"),
        b if b == Rung::RETICLE => Some("reticle"),
        b if b == Rung::GROUND_FX => Some("ground-fx"),
        b if b == Rung::DRIFT_CLOUD => Some("drift-cloud"),
        _ => None,
    }
}

/// Add one [`Transparent3d`] item per draw record to the matching camera's phase.
fn queue_effects(
    effect_pipeline: Res<EffectPipeline>,
    mut pipelines: ResMut<SpecializedRenderPipelines<EffectPipeline>>,
    pipeline_cache: Res<PipelineCache>,
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    meta: Res<EffectMeta>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<(&ExtractedView, &Msaa)>,
) {
    if meta.draws.is_empty() {
        return;
    }
    let draw_function = draw_functions.read().id::<DrawEffects>();
    for (view, msaa) in &views {
        // Views without a transparent phase (shadow, prepass) fall out here.
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let rangefinder = view.rangefinder3d();
        for (i, draw) in meta.draws.iter().enumerate() {
            if view.retained_view_entity.main_entity != MainEntity::from(draw.cam) {
                continue;
            }
            let pipeline = pipelines.specialize(
                &pipeline_cache,
                &effect_pipeline,
                EffectPipelineKey {
                    samples: msaa.samples(),
                    hdr: view.hdr,
                    blend: draw.blend,
                    raster_bias: draw.raster_bias,
                    raster_slope_bits: draw.raster_slope.to_bits(),
                    lit: draw.lit,
                    no_depth_test: draw.no_depth_test,
                },
            );
            phase.add(Transparent3d {
                // The material path's metric: the sort point's view z plus the rung.
                distance: rangefinder.distance(&draw.anchor) + draw.bias,
                pipeline,
                entity: (Entity::PLACEHOLDER, MainEntity::from(draw.main_entity)),
                draw_function,
                // The draw index, until the prepare walk rewrites it to a merged draw and span.
                batch_range: (i as u32)..(i as u32 + 1),
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

/// What adjacent items must share to merge: pipeline, texture, light buffer and params row.
type RunKey = (
    bevy::render::render_resource::CachedRenderPipelineId,
    AssetId<Image>,
    Option<BufferId>,
    u32,
);

/// The last frame's `[effect items, merged draws]`, which `FPS_PROBE` prints as `fx=`.
pub static EFFECT_DRAW_STATS: [std::sync::atomic::AtomicU32; 2] = [
    std::sync::atomic::AtomicU32::new(0),
    std::sync::atomic::AtomicU32::new(0),
];

/// After `PhaseSort`: rebase and upload the vertices, then build the index stream in sorted
/// order, merging adjacent compatible items.
fn prepare_effects(
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    draw_functions: Res<DrawFunctions<Transparent3d>>,
    mut meta: ResMut<EffectMeta>,
    mut phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    views: Query<&ExtractedView>,
) {
    let meta = &mut *meta;
    // Subtract the exact value `ViewUniform.world_position` is built from, so the shader's
    // reconstruction matches bitwise; only the upload copy moves.
    let mut cams: HashMap<MainEntity, Vec3> = HashMap::default();
    for view in &views {
        cams.insert(
            view.retained_view_entity.main_entity,
            view.world_from_view.translation(),
        );
    }
    for draw in &meta.draws {
        // Coplanar draws stay absolute: `DECAL_WORLD_CLIP` keys on the same `raster_bias != 0`.
        if draw.raster_bias != 0 {
            continue;
        }
        // So does a draw its producer already wrote camera-relative.
        if draw.cam_relative {
            continue;
        }
        let Some(cam) = cams.get(&MainEntity::from(draw.cam)) else {
            continue;
        };
        let offset = cam.to_array();
        for v in &mut meta.vertices.values_mut()[draw.range.start as usize..draw.range.end as usize]
        {
            v.pos[0] -= offset[0];
            v.pos[1] -= offset[1];
            v.pos[2] -= offset[2];
        }
    }
    meta.vertices.write_buffer(&device, &queue);

    // The merge walk, over each live view's sorted items, never the raw phase map (an unswept
    // entry carries a dead frame's indices): a run of items sharing a [`RunKey`] becomes one
    // merged draw, and its opener's `batch_range` is rewritten to `merged .. merged + run_len`.
    let effect_fn = draw_functions.read().id::<DrawEffects>();
    // `$WOW_EFFECT_TRACE`: each surface-decal item's phase index, sort distance and index
    // range, then the frame's totals.
    let trace = effect_trace();
    let mut trace_lines: Vec<String> = Vec::new();
    // Rebuilt every frame: the clip set is per-frame, and the runs key on the row offsets.
    let clip_rows = meta.build_params(&device, &queue);
    meta.indices.clear();
    meta.merged.clear();
    let mut n_items = 0u32;
    let mut walked: Vec<bevy::render::view::RetainedViewEntity> = Vec::new();
    for view in &views {
        // Prepass/shadow views share a camera; only the first walk of a phase counts.
        if walked.contains(&view.retained_view_entity) {
            continue;
        }
        walked.push(view.retained_view_entity);
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let mut open: Option<(usize, u32, RunKey)> = None;
        let close = |items: &mut Vec<Transparent3d>,
                     open: &mut Option<(usize, u32, RunKey)>,
                     merged_len: usize| {
            if let Some((item_idx, run_len, _)) = open.take() {
                let m = merged_len as u32 - 1;
                items[item_idx].batch_range = m..m + run_len;
            }
        };
        for i in 0..phase.items.len() {
            let item = &phase.items[i];
            if item.draw_function != effect_fn {
                close(&mut phase.items, &mut open, meta.merged.len());
                continue;
            }
            let (pipeline, draw_idx) = (item.pipeline, item.batch_range.start as usize);
            // `batch_range` is a draw index only while bevy's batcher leaves the item alone, which
            // it does while the main entity owns no mesh; otherwise skip the item, never panic.
            let Some(draw) = meta.draws.get(draw_idx) else {
                close(&mut phase.items, &mut open, meta.merged.len());
                phase.items[i].batch_range = 0..0;
                continue;
            };
            n_items += 1;
            let index_start = meta.indices.len() as u32;
            match draw.topology {
                EffectTopology::Quads => {
                    let mut b = draw.range.start;
                    while b < draw.range.end {
                        for k in [b, b + 1, b + 2, b, b + 2, b + 3] {
                            meta.indices.push(k);
                        }
                        b += 4;
                    }
                }
                EffectTopology::Tris => {
                    for k in draw.range.clone() {
                        meta.indices.push(k);
                    }
                }
            }
            let index_end = meta.indices.len() as u32;
            if let (true, Some(lane)) = (trace, decal_lane(draw.bias)) {
                trace_lines.push(format!(
                    "  item {i}: {lane} anchor {:.1?}, dist {:.1}, verts {}, indices \
                     {index_start}..{index_end}, tex {:?}",
                    draw.anchor,
                    item.distance,
                    draw.range.len(),
                    draw.texture,
                ));
            }
            let params_offset = match draw.clip {
                None => meta.params_offsets[draw.fog.slot() as usize],
                // A clipped draw past the row budget degrades to its unclipped row.
                Some(clip) => *clip_rows
                    .get(&(draw.fog.slot(), clip.to_array().map(f32::to_bits)))
                    .unwrap_or(&meta.params_offsets[draw.fog.slot() as usize]),
            };
            let key: RunKey = (
                pipeline,
                draw.texture,
                draw.light.as_ref().map(|b| b.id()),
                params_offset,
            );
            match &mut open {
                Some((_, run_len, open_key)) if *open_key == key => {
                    meta.merged
                        .last_mut()
                        .expect("open run has a merged record")
                        .index_range
                        .end = index_end;
                    *run_len += 1;
                }
                _ => {
                    close(&mut phase.items, &mut open, meta.merged.len());
                    meta.merged.push(MergedDraw {
                        index_range: index_start..index_end,
                        texture: draw.texture,
                        light: draw.light.clone(),
                        params_offset,
                    });
                    open = Some((i, 1, key));
                }
            }
        }
        close(&mut phase.items, &mut open, meta.merged.len());
    }
    EFFECT_DRAW_STATS[0].store(n_items, std::sync::atomic::Ordering::Relaxed);
    EFFECT_DRAW_STATS[1].store(
        meta.merged.len() as u32,
        std::sync::atomic::Ordering::Relaxed,
    );
    if trace {
        info!(
            "effect trace: {} draws, {} merged, {} indices; shadow items:\n{}",
            meta.draws.len(),
            meta.merged.len(),
            meta.indices.len(),
            trace_lines.join("\n")
        );
    }
    if !meta.indices.is_empty() {
        meta.indices.write_buffer(&device, &queue);
    }
}

/// Build the view bind group and any missing per-texture groups for this frame's draws.
fn prepare_effect_bind_groups(
    device: Res<RenderDevice>,
    pipeline_cache: Res<PipelineCache>,
    pipeline: Res<EffectPipeline>,
    view_uniforms: Res<ViewUniforms>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    light: Option<Res<SharedLightBuffer>>,
    mut meta: ResMut<EffectMeta>,
    mut bind_groups: ResMut<EffectBindGroups>,
) {
    let Some(view_binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    meta.view_bind_group = Some(device.create_bind_group(
        "effect_view_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.view_layout),
        &BindGroupEntries::single(view_binding),
    ));
    let Some(light) = light else { return };
    let Some(params_binding) = meta.params.binding() else {
        return;
    };
    // The cached groups bind into the params buffer: a re-created one invalidates them all.
    if bind_groups.params_buffer != meta.params_buffer {
        bind_groups.images.clear();
        bind_groups.params_buffer = meta.params_buffer;
    }
    for draw in &meta.draws {
        let key = (draw.texture, draw.light.as_ref().map(|b| b.id()));
        if bind_groups.images.contains_key(&key) {
            continue;
        }
        // Not on the GPU yet: the draw is skipped this frame.
        let Some(image) = gpu_images.get(draw.texture) else {
            continue;
        };
        if effect_trace() {
            info!(
                "effect bind: new group for tex {:?} ({}x{})",
                draw.texture, image.size.width, image.size.height
            );
        }
        let light_buf = draw.light.as_ref().unwrap_or(&light.0);
        bind_groups.images.insert(
            key,
            device.create_bind_group(
                "effect_image_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipeline.image_layout),
                &BindGroupEntries::sequential((
                    &image.texture_view,
                    &image.sampler,
                    light_buf.as_entire_binding(),
                    params_binding.clone(),
                )),
            ),
        );
    }
}

pub type DrawEffects = (SetItemPipeline, SetEffectViewBindGroup<0>, DrawEffectBatch);

pub struct SetEffectViewBindGroup<const I: usize>;
impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetEffectViewBindGroup<I> {
    type Param = SRes<EffectMeta>;
    type ViewQuery = Read<ViewUniformOffset>;
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        view_uniform: &'w ViewUniformOffset,
        _entity: Option<()>,
        meta: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(view_bind_group) = meta.into_inner().view_bind_group.as_ref() else {
            return RenderCommandResult::Failure("effect view bind group not available");
        };
        pass.set_bind_group(I, view_bind_group, &[view_uniform.offset]);
        RenderCommandResult::Success
    }
}

pub struct DrawEffectBatch;
impl<P: PhaseItem> RenderCommand<P> for DrawEffectBatch {
    type Param = (SRes<EffectMeta>, SRes<EffectBindGroups>);
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        (meta, bind_groups): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let meta = meta.into_inner();
        // `batch_range.start` is the merged-draw index; its length is the renderer's advance.
        let Some(draw) = meta.merged.get(item.batch_range().start as usize) else {
            return RenderCommandResult::Skip;
        };
        // Image not on the GPU yet: withheld.
        let key = (draw.texture, draw.light.as_ref().map(|b| b.id()));
        let Some(image_bind_group) = bind_groups.into_inner().images.get(&key) else {
            return RenderCommandResult::Skip;
        };
        let (Some(vertices), Some(indices)) = (meta.vertices.buffer(), meta.indices.buffer())
        else {
            return RenderCommandResult::Failure("effect lane buffers not available");
        };
        pass.set_bind_group(1, image_bind_group, &[draw.params_offset]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);
        // `$WOW_EFFECT_TRACE`: what `draw_indexed` actually ran.
        if effect_trace() {
            info!(
                "effect draw: merged {} indices {:?}",
                item.batch_range().start,
                draw.index_range
            );
        }
        pass.draw_indexed(draw.index_range.clone(), 0, 0..1);
        RenderCommandResult::Success
    }
}

/// Registers the render-world half; [`EffectQuads`] and its writers belong to the family plugins.
pub struct EffectLanePlugin;

impl Plugin for EffectLanePlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<EffectMeta>()
            .init_resource::<EffectBindGroups>()
            .init_resource::<SpecializedRenderPipelines<EffectPipeline>>()
            .add_render_command::<Transparent3d, DrawEffects>()
            .add_systems(RenderStartup, init_effect_pipeline)
            .add_systems(ExtractSchedule, extract_effects)
            .add_systems(
                Render,
                (
                    queue_effects.in_set(RenderSystems::Queue),
                    // After `PhaseSort`: the merge walk needs the final item order.
                    prepare_effects.in_set(RenderSystems::PrepareResources),
                    prepare_effect_bind_groups.in_set(RenderSystems::PrepareBindGroups),
                ),
            );
    }
}

/// `$WOW_EFFECT_TRACE`: the lane's per-item, per-bind and per-draw trace, read once.
fn effect_trace() -> bool {
    static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("WOW_EFFECT_TRACE").is_some())
}

#[cfg(test)]
mod tests {
    use bevy::math::{Mat3, Mat4, Vec3};

    /// One constant-bias unit at a `Depth32Float` depth: `2^(e−23)`, the step at its exponent.
    fn bias_unit(depth: f32) -> f32 {
        f32::from_bits(depth.to_bits() + 1) - depth
    }

    /// The world meshes' and `DECAL_WORLD_CLIP`'s route: `clip_from_world × p`.
    fn depth_world(clip_from_world: &Mat4, p: Vec3) -> f32 {
        let c = *clip_from_world * p.extend(1.0);
        c.z / c.w
    }

    /// The cam-relative route: prepare's rebase, then rotation + `clip_from_view`.
    fn depth_cam_relative(
        view_from_world: &Mat4,
        clip_from_view: &Mat4,
        p: Vec3,
        cam: Vec3,
    ) -> f32 {
        let view_pos = Mat3::from_mat4(*view_from_world) * (p - cam);
        let c = *clip_from_view * view_pos.extend(1.0);
        c.z / c.w
    }

    /// At WoW-scale coordinates the cam-relative route misses the world-mesh depth by the order of
    /// a 4096-unit bias up close, while the same matrix matches bitwise: a decal rides its
    /// receiver's matrix.
    #[test]
    fn decal_depth_ties_only_through_the_mesh_matrix() {
        // A floor vertex at Kalimdor-scale coordinates (bevy = (−wow.y, wow.z, −wow.x)).
        let ground = Vec3::new(3807.13, 7.42, 7093.87);
        let clip_from_view =
            Mat4::perspective_infinite_reverse_rh(std::f32::consts::FRAC_PI_4, 16.0 / 9.0, 0.1);
        let dir = Vec3::new(0.35, 0.55, 0.76).normalize();
        let mut worst_route_over_bias = 0.0_f32;
        for step in 0..80 {
            // The zoom sweep: camera 1.5..41 yd out along a fixed off-axis orbit offset.
            let d = 1.5 + step as f32 * 0.5;
            let cam = ground + dir * d;
            // Bevy's own construction (`ExtractedView`), the rounding the mesh shaders see.
            let world_from_view = Mat4::look_at_rh(cam, ground, Vec3::Y).inverse();
            let view_from_world = world_from_view.inverse();
            let clip_from_world = clip_from_view * view_from_world;
            let mesh = depth_world(&clip_from_world, ground);
            let bias = 4096.0 * bias_unit(mesh);
            // Same matrix, same input: bitwise the same depth.
            assert_eq!(depth_world(&clip_from_world, ground), mesh);
            let route =
                (depth_cam_relative(&view_from_world, &clip_from_view, ground, cam) - mesh).abs();
            worst_route_over_bias = worst_route_over_bias.max(route / bias);
        }
        // 0.93× at d = 1.5 on this sweep; the bound leaves headroom for platform rounding.
        assert!(
            worst_route_over_bias > 0.5,
            "cam-relative route divergence stayed far inside the bias \
             (worst {worst_route_over_bias:.2}× across the sweep) — 0781's premise would be \
             unfounded"
        );
    }

    /// CPU-baked decal verts and GPU-transformed receiver verts differ by a few ulps of the world
    /// coordinate, which a sloped receiver takes onto its normal as depth. Sized at 3 ulps on the
    /// Stormwind gate ramp across zoom and slope: above an 8192-unit bias, under half of
    /// `Rung::DECAL_RASTER`. One bake path, so one number serves every ground decal.
    #[test]
    fn raised_bias_dominates_the_bake_residual() {
        use crate::sky_order::Rung;
        // The Stormwind gate ramp, wow (−8843.41, 642.68, 95.92) in bevy axes.
        let ground = Vec3::new(-642.68, 95.92, 8843.41);
        let clip_from_view =
            Mat4::perspective_infinite_reverse_rh(std::f32::consts::FRAC_PI_4, 16.0 / 9.0, 0.1);
        let ulp = |v: f32| f32::from_bits(v.to_bits() + 1) - v;
        // 3 ulps per world axis, signs free: the worst normal offset is the absolute sum.
        let worst_normal_offset = |n: Vec3| {
            3.0 * (n.x.abs() * ulp(ground.x)
                + n.y.abs() * ulp(ground.y)
                + n.z.abs() * ulp(ground.z))
        };
        let cam_dir = Vec3::new(0.35, 0.55, 0.76).normalize();
        let mut worst_over_old = 0.0_f32;
        let mut worst_over_retired_ring = 0.0_f32;
        let mut worst_over_new = 0.0_f32;
        // Receiver grades from level street to a steep ramp, tilted along 8 azimuths.
        for slope in [0.0_f32, 0.08, 0.2, 0.35] {
            for az in 0..8 {
                let a = az as f32 * std::f32::consts::FRAC_PI_4;
                let tilt = Vec3::new(a.cos(), 0.0, a.sin());
                let normal = (Vec3::Y - tilt * slope).normalize();
                let delta_n = worst_normal_offset(normal);
                for step in 0..80 {
                    let d = 1.0 + step as f32 * 0.5;
                    let cam = ground + cam_dir * d;
                    let world_from_view = Mat4::look_at_rh(cam, ground, Vec3::Y).inverse();
                    let view_from_world = world_from_view.inverse();
                    let clip_from_world = clip_from_view * view_from_world;
                    let depth = depth_world(&clip_from_world, ground);
                    // The offset's window-depth cost, differenced in f64 to keep f32 noise out.
                    let m = clip_from_world.as_dmat4();
                    let z_at = |p: bevy::math::DVec3| {
                        let c = m * p.extend(1.0);
                        c.z / c.w
                    };
                    let p = ground.as_dvec3();
                    let eps = 0.05;
                    let grad = (z_at(p + normal.as_dvec3() * eps) - z_at(p)).abs() / eps;
                    let conflict = delta_n as f64 * grad;
                    let unit = bias_unit(depth) as f64;
                    worst_over_old = worst_over_old.max((conflict / (4096.0 * unit)) as f32);
                    worst_over_retired_ring =
                        worst_over_retired_ring.max((conflict / (8192.0 * unit)) as f32);
                    worst_over_new =
                        worst_over_new.max((conflict / (Rung::DECAL_RASTER as f64 * unit)) as f32);
                }
            }
        }
        eprintln!(
            "bake residual worst: {worst_over_old:.3}x the 0781-era 4096 margin, \
             {worst_over_retired_ring:.3}x the 8192 the ring rode until 1817, \
             {worst_over_new:.3}x Rung::DECAL_RASTER"
        );
        assert!(
            worst_over_old > 0.5,
            "the 3-ulp residual stayed far inside the 0781-era 4096 margin \
             (worst {worst_over_old:.2}×) — the resize's premise would be unfounded"
        );
        assert!(
            worst_over_retired_ring > 1.0,
            "the 3-ulp residual fits inside the retired +8192 (worst {worst_over_retired_ring:.2}×) \
             — then the ring and reticle were NOT under-biased and 1817's raise wants re-arguing"
        );
        assert!(
            worst_over_new < 0.5,
            "Rung::DECAL_RASTER leaves under 2× headroom against the 3-ulp residual \
             (worst {worst_over_new:.2}×) — raise it"
        );
    }
}
