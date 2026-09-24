//! The render-world half of the retained pass: extraction of the published set, per-region GPU
//! assembly against the shared texture pool, the pipelines, and the draw node before the main
//! opaque pass. Texture dims and formats exist only here (BLP images are `RENDER_WORLD`-only), so
//! a region draws once every member's `GpuImage` is resident.

use bevy::camera::primitives::Aabb;
use bevy::ecs::query::QueryItem;
use bevy::image::Image;
use bevy::mesh::VertexBufferLayout;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::diagnostic::RecordDiagnostics;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::mesh::allocator::MeshAllocator;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_2d_array, uniform_buffer, uniform_buffer_sized,
};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::view::{
    ExtractedView, Msaa, ViewDepthTexture, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Render, RenderSystems};
use bevy::shader::ShaderDefVal;
use std::ops::Range;

use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT;

/// One baked item's draw facts, in bake order: the vertex word's low bits index its record.
#[derive(Clone)]
pub(crate) struct GxItemDraw {
    pub index_range: Range<u32>,
    pub texture: Option<AssetId<Image>>,
    pub cutout: bool,
    pub two_sided: bool,
    #[allow(dead_code)] // bake-side bookkeeping; the node draws by index range alone
    pub vertex_range: Range<u32>,
    /// The selection key (a WMO group or a prop referrer set); `None` on cell items.
    pub group: Option<u16>,
    /// The authored batch order (the coplanar-MOBA clip-z nudge; 0 on cell items).
    pub order: u16,
    /// The MOMT SIDN night-glow colour (gamma bytes; zero on cell items).
    pub sidn: [u8; 3],
    /// The interior prop's SH-probe slot, record column w bits 1..=13; 0 elsewhere.
    pub slot: u16,
}

/// One baked cell or region, published by the main-world flush.
#[derive(Clone)]
pub(crate) struct GxCellDraw {
    pub mesh: Handle<Mesh>,
    /// The recentring origin: shader world = vertex + origin.
    pub origin: Vec3,
    /// Mesh-local bound (recentred); world bound = origin + this.
    pub aabb: Aabb,
    pub draws: Vec<GxItemDraw>,
    /// The exile kill bitmap, bit i dropping item i; all-zero on regions.
    pub killed: Vec<u64>,
    /// Bumped on every bitmap change; the render side re-syncs the kill column on a new one.
    pub killed_rev: u32,
    /// Mesh-local bounds per selection key (group or referrer set) for the cull; empty on cells.
    pub groups: Vec<(u16, Aabb)>,
    /// A prop region's distinct referrer sets, indexed like `groups`.
    pub sets: Vec<std::sync::Arc<[u16]>>,
}

/// Marks the world camera, the one view the pass draws into, so no portrait-booth view gets
/// world cells.
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
pub(crate) struct StaticGxView;

/// Mark the world camera, again whenever it respawns.
fn mark_world_camera(
    mut commands: Commands,
    cam: Query<Entity, (With<crate::view::WorldCamera>, Without<StaticGxView>)>,
) {
    for e in &cam {
        commands.entity(e).insert(StaticGxView);
    }
}

/// One entry of the doodad-phase draw list: ADT-doodad cells and WMO-prop regions are one phase
/// in the 1.12 order (the M2 scene, after the WMOs), so they sort near-first together.
#[derive(Clone, PartialEq)]
pub(crate) enum GxDoodadVis {
    Cell((i32, i32)),
    Prop(Entity, GxSel),
}

/// One region's verdicts this frame, indexed by group (WMO) or referrer set (props).
#[derive(Clone, Default, PartialEq)]
pub(crate) struct GxSel {
    /// Drawn this frame: PVS ∧ frustum ∧ farclip ∧ the exterior window gate.
    pub drawn: Vec<bool>,
    /// On the interior fog lane: the client's per-group `[0xca7f00]`, from the portal flood
    /// ([`crate::wmo_portal::GroupPvs::interior_fog`]).
    pub fog: Vec<bool>,
}

/// The published half the render world clones each frame. Regions sit behind `Arc`, so the clones
/// are refcount bumps and the kill scan, the one writer, copies a region on write.
#[derive(Clone, Default, Resource, ExtractResource)]
pub(crate) struct GxWorld {
    pub cells: HashMap<(i32, i32), std::sync::Arc<GxCellDraw>>,
    /// This frame's doodad-phase draw list, near-first across cells and prop regions.
    pub visible: Vec<GxDoodadVis>,
    pub wmos: HashMap<Entity, std::sync::Arc<GxCellDraw>>,
    /// The prop regions, keyed like `wmos` but apart, so a prop arrival never re-bakes a building.
    pub props: HashMap<Entity, std::sync::Arc<GxCellDraw>>,
    /// This frame's per-group admission per WMO region; the node draws only admitted runs.
    pub visible_wmos: Vec<(Entity, GxSel)>,
}

use super::pool::GxTexturePool;

/// Record column w, bit 14: the item's interior fog lane, read by `static_gx.wgsl`. Bit 0 is the
/// kill bit and bits 1..=13 the probe slot.
const RECORD_FOG_BIT: u32 = 1 << 14;

/// One coalesced draw run of adjacent live items sharing bind-group slot, pipeline bucket and
/// selection key; a killed item is never in one.
struct GxRun {
    /// Index into the region's `bind_groups`, not a pool class.
    slot: usize,
    cutout: bool,
    two_sided: bool,
    index_range: Range<u32>,
    /// The run's selection key; `None` for a cell run, which always draws.
    group: Option<u16>,
}

/// A region's assembled GPU state, cached until its bake (mesh handle) changes.
struct GxCellGpu {
    mesh: AssetId<Mesh>,
    /// One bind group per pool class the region's items touch: (pool class, bind group).
    bind_groups: Vec<(u16, BindGroup)>,
    record_table: Buffer,
    #[allow(dead_code)] // held alive for the bind groups that reference it
    cell_uniform: Buffer,
    /// Per item, its index into `bind_groups`, kept for kill-driven run rebuilds.
    item_slot: Vec<u16>,
    runs: Vec<GxRun>,
    /// CPU copy of the record table, which the kill and fog syncs rewrite and re-upload.
    records: Vec<[u32; 4]>,
    killed_applied: u32,
    /// The [`GxSel::fog`] the records were last written from; empty until the first sync.
    fog_applied: Vec<bool>,
}

#[derive(Resource, Default)]
struct GxGpuCache {
    cells: HashMap<(i32, i32), GxCellGpu>,
    wmos: HashMap<Entity, GxCellGpu>,
    props: HashMap<Entity, GxCellGpu>,
}

#[derive(Resource)]
struct GxPipelines {
    view_layout: BindGroupLayoutDescriptor,
    cell_layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    sampler_clamp: Sampler,
    /// Keyed `(cutout, two_sided)`, re-specialized if the world view's (samples, format) changes.
    pipelines: HashMap<(bool, bool), CachedRenderPipelineId>,
    specialized_for: Option<(u32, TextureFormat)>,
}

fn init_pipelines(mut commands: Commands, render_device: Res<RenderDevice>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "static_gx_view_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                uniform_buffer::<ViewUniform>(true),
                storage_buffer_read_only_sized(false, None),
            ),
        ),
    );
    let cell_layout = BindGroupLayoutDescriptor::new(
        "static_gx_cell_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX_FRAGMENT,
            (
                // origin (xyz) + pad
                uniform_buffer_sized(false, Some(std::num::NonZero::new(16).unwrap())),
                // the per-item record table
                storage_buffer_read_only_sized(false, None),
                texture_2d_array(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    // Repeat and clamp samplers, picked by the vertex word's wrap bits since a shared array has no
    // per-layer address mode. Mip filter and anisotropy come from `benilla_assets::tex_filter`,
    // the policy the reference applies to every texture from `[0x835250]`/`[0x835254]`. A
    // mixed-wrap batch takes the repeat sampler and the shader clamps its clamped axis a half
    // texel in; the reference clamps that axis in the sampler.
    let filter = benilla_assets::tex_filter();
    let make = |label: &'static str, mode: AddressMode| {
        render_device.create_sampler(&SamplerDescriptor {
            label: Some(label),
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            mipmap_filter: filter.gpu_mipmap_filter(),
            address_mode_u: mode,
            address_mode_v: mode,
            anisotropy_clamp: filter.anisotropy_clamp(),
            ..Default::default()
        })
    };
    commands.insert_resource(GxPipelines {
        view_layout,
        cell_layout,
        sampler: make("static_gx_repeat", AddressMode::Repeat),
        sampler_clamp: make("static_gx_clamp", AddressMode::ClampToEdge),
        pipelines: HashMap::default(),
        specialized_for: None,
    });
}

/// The world view's pipeline-key inputs.
type GxViewKey = (
    &'static ExtractedView,
    &'static Msaa,
    &'static ViewTarget,
    &'static StaticGxView,
);

/// The bake's interleaved vertex layout, in the attribute-id order Bevy interleaves by: position,
/// normal, uv, colour, then the word and anchor (988_101, 988_102). Keep in sync with `bake_cell`
/// and `static_gx.wgsl`.
fn vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: 64,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            VertexAttribute {
                format: VertexFormat::Float32x2,
                offset: 24,
                shader_location: 2,
            },
            VertexAttribute {
                format: VertexFormat::Float32x4,
                offset: 32,
                shader_location: 5,
            },
            VertexAttribute {
                format: VertexFormat::Uint32,
                offset: 48,
                shader_location: 3,
            },
            VertexAttribute {
                format: VertexFormat::Float32x3,
                offset: 52,
                shader_location: 4,
            },
        ],
    }
}

/// Specialize the four pipelines for the world view, and assemble visible regions' GPU state.
fn prepare_static_gx(
    gx: Res<GxWorld>,
    mut cache: ResMut<GxGpuCache>,
    mut pool: ResMut<GxTexturePool>,
    mut pipes: ResMut<GxPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    asset_server: Res<AssetServer>,
    images: Res<RenderAssets<GpuImage>>,
    views: Query<GxViewKey>,
) {
    let _t = super::gx_perf_guard(3);
    let Some((view, msaa, _, _)) = views.iter().next() else {
        return;
    };
    let format = if view.hdr {
        ViewTarget::TEXTURE_FORMAT_HDR
    } else {
        TextureFormat::bevy_default()
    };
    let key = (msaa.samples(), format);
    if pipes.specialized_for != Some(key) {
        let shader: Handle<Shader> =
            asset_server.load("embedded://benilla_world/shaders/static_gx.wgsl");
        pipes.pipelines.clear();
        for cutout in [false, true] {
            for two_sided in [false, true] {
                let mut defs = vec![];
                if cutout {
                    defs.push(ShaderDefVal::from("GX_CUTOUT"));
                }
                let id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some(
                        format!("static_gx c{} t{}", u8::from(cutout), u8::from(two_sided)).into(),
                    ),
                    layout: vec![pipes.view_layout.clone(), pipes.cell_layout.clone()],
                    vertex: VertexState {
                        shader: shader.clone(),
                        shader_defs: defs.clone(),
                        entry_point: Some("vertex".into()),
                        buffers: vec![vertex_layout()],
                    },
                    fragment: Some(FragmentState {
                        shader: shader.clone(),
                        shader_defs: defs,
                        entry_point: Some("fragment".into()),
                        targets: vec![Some(ColorTargetState {
                            format,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                    }),
                    primitive: PrimitiveState {
                        cull_mode: (!two_sided).then_some(Face::Back),
                        ..Default::default()
                    },
                    depth_stencil: Some(DepthStencilState {
                        format: CORE_3D_DEPTH_FORMAT,
                        depth_write_enabled: true,
                        depth_compare: CompareFunction::GreaterEqual,
                        stencil: StencilState::default(),
                        bias: DepthBiasState::default(),
                    }),
                    multisample: MultisampleState {
                        count: msaa.samples(),
                        ..Default::default()
                    },
                    ..default()
                });
                pipes.pipelines.insert((cutout, two_sided), id);
            }
        }
        pipes.specialized_for = Some(key);
    }

    // Every published map empty is a map change (`StaticGx::clear`): reset the pool and cache.
    if gx.cells.is_empty() && gx.wmos.is_empty() && gx.props.is_empty() {
        if !pool.is_empty() {
            *pool = GxTexturePool::default();
        }
        cache.cells.clear();
        cache.wmos.clear();
        cache.props.clear();
        return;
    }

    // Drop cache entries whose region vanished or re-baked.
    cache
        .cells
        .retain(|c, gpu| gx.cells.get(c).is_some_and(|d| d.mesh.id() == gpu.mesh));
    cache
        .wmos
        .retain(|e, gpu| gx.wmos.get(e).is_some_and(|d| d.mesh.id() == gpu.mesh));
    cache
        .props
        .retain(|e, gpu| gx.props.get(e).is_some_and(|d| d.mesh.id() == gpu.mesh));

    for vis in &gx.visible {
        match vis {
            GxDoodadVis::Cell(cell) => {
                if cache.cells.contains_key(cell) {
                    continue;
                }
                let Some(draw) = gx.cells.get(cell) else {
                    continue;
                };
                if let Some(gpu) = assemble_region(
                    draw,
                    &mut pool,
                    &pipes,
                    &pipeline_cache,
                    &render_device,
                    &render_queue,
                    &images,
                ) {
                    cache.cells.insert(*cell, gpu);
                }
            }
            GxDoodadVis::Prop(entity, _) => {
                if cache.props.contains_key(entity) {
                    continue;
                }
                let Some(draw) = gx.props.get(entity) else {
                    continue;
                };
                if let Some(gpu) = assemble_region(
                    draw,
                    &mut pool,
                    &pipes,
                    &pipeline_cache,
                    &render_device,
                    &render_queue,
                    &images,
                ) {
                    cache.props.insert(*entity, gpu);
                }
            }
        }
    }
    for (entity, _) in &gx.visible_wmos {
        if cache.wmos.contains_key(entity) {
            continue;
        }
        let Some(draw) = gx.wmos.get(entity) else {
            continue;
        };
        if let Some(gpu) = assemble_region(
            draw,
            &mut pool,
            &pipes,
            &pipeline_cache,
            &render_device,
            &render_queue,
            &images,
        ) {
            cache.wmos.insert(*entity, gpu);
        }
    }
    pool.drain_pending(&render_device, &render_queue);

    // The kill-bit sync: on a new bitmap revision, rewrite the kill column, re-upload and rebuild
    // the runs. A cell that changed out of view syncs on re-entry.
    for vis in &gx.visible {
        let GxDoodadVis::Cell(cell) = vis else {
            continue; // prop regions carry no faders
        };
        let (Some(gpu), Some(draw)) = (cache.cells.get_mut(cell), gx.cells.get(cell)) else {
            continue;
        };
        if gpu.killed_applied == draw.killed_rev {
            continue;
        }
        for (i, rec) in gpu.records.iter_mut().enumerate() {
            // Column w also carries the probe slot and fog bit: rewrite bit 0 alone.
            rec[3] = (rec[3] & !1) | kill_bit(&draw.killed, i);
        }
        render_queue.write_buffer(&gpu.record_table, 0, bytemuck::cast_slice(&gpu.records));
        gpu.runs = build_runs(&draw.draws, &gpu.item_slot, &draw.killed);
        gpu.killed_applied = draw.killed_rev;
    }

    // The interior-fog sync: the client's per-group `[0xca7f00]` picks a WMO group's and its
    // props' fog triple per frame; rewritten only when the verdict moves, runs untouched.
    let sync_fog = |gpu: &mut GxCellGpu, draw: &GxCellDraw, sel: &GxSel| {
        if gpu.fog_applied == sel.fog {
            return;
        }
        for (rec, item) in gpu.records.iter_mut().zip(&draw.draws) {
            let on = item
                .group
                .is_some_and(|g| sel.fog.get(usize::from(g)).copied().unwrap_or(false));
            rec[3] = (rec[3] & !RECORD_FOG_BIT) | (u32::from(on) * RECORD_FOG_BIT);
        }
        render_queue.write_buffer(&gpu.record_table, 0, bytemuck::cast_slice(&gpu.records));
        gpu.fog_applied.clone_from(&sel.fog);
    };
    for (entity, sel) in &gx.visible_wmos {
        if let (Some(gpu), Some(draw)) = (cache.wmos.get_mut(entity), gx.wmos.get(entity)) {
            sync_fog(gpu, draw, sel);
        }
    }
    for vis in &gx.visible {
        let GxDoodadVis::Prop(entity, sel) = vis else {
            continue; // cell items are never on the interior lane
        };
        if let (Some(gpu), Some(draw)) = (cache.props.get_mut(entity), gx.props.get(entity)) {
            sync_fog(gpu, draw, sel);
        }
    }
}

/// Assemble one region's GPU state against the shared pool; `None`, and undrawn this frame, while
/// any member texture is not resident (slots already assigned stay assigned).
fn assemble_region(
    draw: &GxCellDraw,
    pool: &mut GxTexturePool,
    pipes: &GxPipelines,
    pipeline_cache: &PipelineCache,
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    images: &RenderAssets<GpuImage>,
) -> Option<GxCellGpu> {
    // Each item's pool slot, deduped by texture id; untextured items take the white class.
    let mut white: Option<u16> = None;
    let mut item_class_layer: Vec<(u16, u16)> = Vec::with_capacity(draw.draws.len());
    for item in &draw.draws {
        item_class_layer.push(match item.texture {
            Some(tex) => pool.assign(tex, images.get(tex)?, render_device),
            None => {
                let w = *white.get_or_insert_with(|| pool.white(render_device, render_queue));
                (w, 0)
            }
        });
    }
    // The per-item record table, indexed by the word's low bits: [layer, batch-order nudge, SIDN,
    // flags]. Flags: bit 0 the kill bit, re-synced by revision (hence COPY_DST); bits 1..=13 the
    // probe slot, 13 bits being `MAX_PROP_PROBES` exactly.
    let records: Vec<[u32; 4]> = draw
        .draws
        .iter()
        .enumerate()
        .map(|(i, item)| {
            [
                u32::from(item_class_layer[i].1),
                u32::from(item.order),
                u32::from(item.sidn[0])
                    | (u32::from(item.sidn[1]) << 8)
                    | (u32::from(item.sidn[2]) << 16),
                kill_bit(&draw.killed, i) | (u32::from(item.slot) << 1),
            ]
        })
        .collect();
    let record_table = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("static_gx_records"),
        contents: bytemuck::cast_slice(&records),
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
    });
    let cell_uniform = render_device.create_buffer_with_data(&BufferInitDescriptor {
        label: Some("static_gx_cell"),
        contents: bytemuck::cast_slice(&[draw.origin.x, draw.origin.y, draw.origin.z, 0.0f32]),
        usage: BufferUsages::UNIFORM,
    });
    // One bind group per distinct pool class the region touches.
    let cell_layout = pipeline_cache.get_bind_group_layout(&pipes.cell_layout);
    let mut bind_groups: Vec<(u16, BindGroup)> = Vec::new();
    let mut item_slot: Vec<u16> = Vec::with_capacity(draw.draws.len());
    for &(class, _) in &item_class_layer {
        let slot = match bind_groups.iter().position(|(c, _)| *c == class) {
            Some(s) => s,
            None => {
                let bg = render_device.create_bind_group(
                    "static_gx_cell",
                    &cell_layout,
                    &BindGroupEntries::sequential((
                        cell_uniform.as_entire_binding(),
                        record_table.as_entire_binding(),
                        pool.view(class),
                        &pipes.sampler,
                        &pipes.sampler_clamp,
                    )),
                );
                bind_groups.push((class, bg));
                bind_groups.len() - 1
            }
        };
        item_slot.push(u16::try_from(slot).expect("gx region under u16 slots"));
    }
    let runs = build_runs(&draw.draws, &item_slot, &draw.killed);
    Some(GxCellGpu {
        mesh: draw.mesh.id(),
        bind_groups,
        record_table,
        cell_uniform,
        item_slot,
        runs,
        records,
        killed_applied: draw.killed_rev,
        // Empty, so the first fog sync always writes.
        fog_applied: Vec::new(),
    })
}

/// Coalesce adjacent live items sharing (slot, bucket, group) into draw runs; a killed item is
/// skipped, so a fully gone cell submits nothing.
fn build_runs(draws: &[GxItemDraw], item_slot: &[u16], killed: &[u64]) -> Vec<GxRun> {
    let mut runs: Vec<GxRun> = Vec::new();
    for (i, item) in draws.iter().enumerate() {
        if kill_bit(killed, i) != 0 {
            continue;
        }
        let slot = usize::from(item_slot[i]);
        match runs.last_mut() {
            Some(r)
                if r.slot == slot
                    && r.cutout == item.cutout
                    && r.two_sided == item.two_sided
                    && r.group == item.group
                    && r.index_range.end == item.index_range.start =>
            {
                r.index_range.end = item.index_range.end;
            }
            _ => runs.push(GxRun {
                slot,
                cutout: item.cutout,
                two_sided: item.two_sided,
                index_range: item.index_range.clone(),
                group: item.group,
            }),
        }
    }
    runs
}

/// Record-table column 3: item `i`'s exile kill bit from the published bitmap.
fn kill_bit(killed: &[u64], i: usize) -> u32 {
    u32::from(
        killed
            .get(i / 64)
            .is_some_and(|w| w & (1u64 << (i % 64)) != 0),
    )
}

/// Group 0: bevy's view uniform and the `wow_shared_light` storage every material binds.
#[derive(Resource)]
struct GxViewBind(BindGroup);

fn prepare_view_bind(
    mut commands: Commands,
    pipes: Res<GxPipelines>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    light: Option<Res<crate::lighting::SharedLightBuffer>>,
) {
    let (Some(view_binding), Some(light)) = (view_uniforms.uniforms.binding(), light) else {
        return;
    };
    let layout = pipeline_cache.get_bind_group_layout(&pipes.view_layout);
    commands.insert_resource(GxViewBind(render_device.create_bind_group(
        "static_gx_view",
        &layout,
        &BindGroupEntries::sequential((view_binding, light.0.as_entire_binding())),
    )));
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct StaticGxLabel;

#[derive(Default)]
struct StaticGxNode;

impl ViewNode for StaticGxNode {
    type ViewQuery = (
        &'static ViewTarget,
        &'static ViewDepthTexture,
        &'static ViewUniformOffset,
        &'static StaticGxView,
    );

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext<'w>,
        (target, depth, view_offset, _marker): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let _t = super::gx_perf_guard(4);
        let gx = world.resource::<GxWorld>();
        if gx.visible.is_empty() && gx.visible_wmos.is_empty() {
            return Ok(());
        }
        let cache = world.resource::<GxGpuCache>();
        let pipes = world.resource::<GxPipelines>();
        let pipeline_cache = world.resource::<PipelineCache>();
        let Some(view_bind) = world.get_resource::<GxViewBind>() else {
            return Ok(());
        };
        let meshes = world.resource::<RenderAssets<RenderMesh>>();
        let allocator = world.resource::<MeshAllocator>();
        // WMO regions first, then the doodad phase near-first, the 1.12 client's WMO-then-doodad
        // drain order. Cells draw whole; a region draws only its admitted runs.
        let mut resolved: Vec<(&GxCellGpu, &GxCellDraw, Option<&GxSel>)> = Vec::new();
        for (entity, sel) in &gx.visible_wmos {
            if let (Some(gpu), Some(draw)) = (cache.wmos.get(entity), gx.wmos.get(entity)) {
                resolved.push((gpu, draw, Some(sel)));
            }
        }
        for vis in &gx.visible {
            match vis {
                GxDoodadVis::Cell(cell) => {
                    if let (Some(gpu), Some(draw)) = (cache.cells.get(cell), gx.cells.get(cell)) {
                        resolved.push((gpu, draw, None));
                    }
                }
                GxDoodadVis::Prop(entity, sel) => {
                    if let (Some(gpu), Some(draw)) = (cache.props.get(entity), gx.props.get(entity))
                    {
                        resolved.push((gpu, draw, Some(sel)));
                    }
                }
            }
        }
        if resolved.is_empty() {
            return Ok(());
        }
        // All four pipelines or none: a region drawing only its opaque half would flash its
        // cutout content off for a frame.
        let mut ready: HashMap<(bool, bool), &RenderPipeline> = HashMap::default();
        for (k, id) in &pipes.pipelines {
            match pipeline_cache.get_render_pipeline(*id) {
                Some(p) => {
                    ready.insert(*k, p);
                }
                None => return Ok(()),
            }
        }
        let depth_attachment = depth.get_attachment(StoreOp::Store);
        let color_attachment = target.get_color_attachment();
        // `render/static_gx/elapsed_gpu` on Vulkan and DX12: the perf journal's `gpu_static`.
        let diagnostics = render_context.diagnostic_recorder();
        let mut pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("static_gx"),
            color_attachments: &[Some(color_attachment)],
            depth_stencil_attachment: Some(depth_attachment),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let span = diagnostics.pass_span(&mut pass, "static_gx");
        pass.set_bind_group(0, &view_bind.0, &[view_offset.offset]);
        for (gpu, draw, sel) in &resolved {
            let Some(mesh) = meshes.get(draw.mesh.id()) else {
                continue;
            };
            let (Some(vslice), Some(islice)) = (
                allocator.mesh_vertex_slice(&draw.mesh.id()),
                allocator.mesh_index_slice(&draw.mesh.id()),
            ) else {
                continue;
            };
            let index_format = match &mesh.buffer_info {
                bevy::render::mesh::RenderMeshBufferInfo::Indexed { index_format, .. } => {
                    *index_format
                }
                bevy::render::mesh::RenderMeshBufferInfo::NonIndexed => continue,
            };
            pass.set_vertex_buffer(0, vslice.buffer.slice(..));
            pass.set_index_buffer(islice.buffer.slice(..), index_format);
            for run in &gpu.runs {
                // A region's run draws only if its selection key is admitted; a cell run always.
                if let (Some(sel), Some(group)) = (sel, run.group) {
                    if !sel.drawn.get(usize::from(group)).copied().unwrap_or(false) {
                        continue;
                    }
                }
                pass.set_render_pipeline(ready[&(run.cutout, run.two_sided)]);
                pass.set_bind_group(1, &gpu.bind_groups[run.slot].1, &[]);
                pass.draw_indexed(
                    (islice.range.start + run.index_range.start)
                        ..(islice.range.start + run.index_range.end),
                    i32::try_from(vslice.range.start).unwrap_or(0),
                    0..1,
                );
            }
        }
        span.end(&mut pass);
        Ok(())
    }
}

/// Wire the render half (called by the plugin only when armed).
pub(super) fn build(app: &mut App) {
    // The shader registers in `crate::shaders`: `embedded_asset!` prefixes by the calling file.
    // `publish_gx_world` mirrors `StaticGx`'s published half into this resource for extraction.
    app.add_plugins((
        ExtractResourcePlugin::<GxWorld>::default(),
        ExtractComponentPlugin::<StaticGxView>::default(),
    ));
    app.init_resource::<GxWorld>();
    app.add_systems(Update, mark_world_camera);
    let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) else {
        return;
    };
    render_app
        .init_resource::<GxGpuCache>()
        .init_resource::<GxTexturePool>()
        .add_systems(bevy::render::RenderStartup, init_pipelines)
        .add_systems(
            Render,
            (
                prepare_static_gx.in_set(RenderSystems::PrepareResources),
                prepare_view_bind.in_set(RenderSystems::PrepareBindGroups),
            ),
        )
        .add_render_graph_node::<ViewNodeRunner<StaticGxNode>>(Core3d, StaticGxLabel)
        // Before bevy's opaque pass, as early-z occluders for terrain. This pass takes the depth
        // and colour clear (attachments clear on first use); with nothing visible it returns
        // untouched and the opaque pass clears.
        .add_render_graph_edges(
            Core3d,
            (Node3d::StartMainPass, StaticGxLabel, Node3d::MainOpaquePass),
        );
}

/// Copy the collector's published half into the extractable resource.
pub(super) fn publish_gx_world(gx: Res<super::StaticGx>, mut out: ResMut<GxWorld>) {
    let _t = super::gx_perf_guard(2);
    // Write only what changed: a write through `ResMut` marks the resource changed, which costs a
    // full clone at extract. The maps hold `Arc`s, so identity is pointer identity.
    fn same_arcs<K: std::hash::Hash + Eq>(
        a: &HashMap<K, std::sync::Arc<GxCellDraw>>,
        b: &HashMap<K, std::sync::Arc<GxCellDraw>>,
    ) -> bool {
        a.len() == b.len()
            && a.iter()
                .all(|(k, v)| b.get(k).is_some_and(|w| std::sync::Arc::ptr_eq(v, w)))
    }
    let w = &gx.world;
    if !same_arcs(&out.cells, &w.cells) {
        out.cells.clone_from(&w.cells);
    }
    if out.visible != w.visible {
        out.visible.clone_from(&w.visible);
    }
    if !same_arcs(&out.wmos, &w.wmos) {
        out.wmos.clone_from(&w.wmos);
    }
    if !same_arcs(&out.props, &w.props) {
        out.props.clone_from(&w.props);
    }
    if out.visible_wmos != w.visible_wmos {
        out.visible_wmos.clone_from(&w.visible_wmos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(index_range: Range<u32>, cutout: bool) -> GxItemDraw {
        GxItemDraw {
            index_range,
            texture: None,
            cutout,
            two_sided: false,
            vertex_range: 0..0,
            group: None,
            order: 0,
            sidn: [0; 3],
            slot: 0,
        }
    }

    #[test]
    fn runs_fuse_live_items_and_split_at_kills() {
        let draws = vec![
            item(0..3, false),
            item(3..6, false),
            item(6..9, false),
            item(9..12, true), // bucket change
            item(12..15, true),
        ];
        let slots = vec![0, 0, 0, 0, 1]; // the last item binds another pool class
        let runs = build_runs(&draws, &slots, &[]);
        assert_eq!(runs.len(), 3, "opaque span fused; cutout split by slot");
        assert_eq!(runs[0].index_range, 0..9);
        assert_eq!(runs[1].index_range, 9..12);
        assert!(runs[1].cutout);
        assert_eq!(runs[2].slot, 1);
        // Kill the middle opaque item: the fused run splits around it.
        let runs = build_runs(&draws, &slots, &[0b010u64]);
        assert_eq!(runs.len(), 4);
        assert_eq!(runs[0].index_range, 0..3);
        assert_eq!(runs[1].index_range, 6..9);
        // Kill everything: nothing is submitted.
        assert!(build_runs(&draws, &slots, &[0b11111u64]).is_empty());
    }
}
