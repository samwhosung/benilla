//! The render-world half of the B1 retained pass (see `mod.rs`): extraction of
//! the published cell set, per-cell GPU assembly (texture-array classes + the item→layer
//! table), the pipeline family, and the draw node between the main opaque and transparent
//! passes.
//!
//! Assembly happens where each fact lives: the MAIN world bakes geometry (it owns the
//! submeshes) but cannot know texture dims/format (BLP images are `RENDER_WORLD`-only), so
//! classing into `texture_2d_array`s happens HERE, once each member's `GpuImage` is resident.
//! A cell whose textures aren't all loaded yet simply isn't drawn that frame (the entity path
//! streams batches in piecewise; cell-granular appearance is the same arrival class, mostly
//! under the load cover).
//!
//! **The arrays are ONE SHARED POOL, not per-cell (B3)** — `pool.rs` owns the
//! design note (the two driver taxes 1431's `sample` caught, and how dedup + drain-once +
//! sibling growth remove them structurally). Here, a re-bake costs a record table and a few
//! bind groups, never a texture.

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

/// One baked item's draw facts (index-parallel with the bake order; the vertex word's low bits
/// carry this item's index, which the record table resolves to an array layer + the WMO
/// per-item record).
#[derive(Clone)]
pub(crate) struct GxItemDraw {
    pub index_range: Range<u32>,
    pub texture: Option<AssetId<Image>>,
    pub cutout: bool,
    pub two_sided: bool,
    #[allow(dead_code)] // bake-side bookkeeping; the node draws by index range alone
    pub vertex_range: Range<u32>,
    /// The range-selection key (`None` on cell items — always drawn): a WMO item's GROUP,
    /// or a prop item's referrer-SET index (B4). A run never crosses a selection boundary,
    /// so the per-frame verdict selects whole runs.
    pub group: Option<u16>,
    /// The authored batch order (the coplanar-MOBA clip-z nudge; 0 on cell items).
    pub order: u16,
    /// The MOMT SIDN night-glow colour (gamma bytes; zero on cell items).
    pub sidn: [u8; 3],
    /// The interior prop's SH-probe slot (B4; 0 elsewhere — read only under the word's
    /// INTERIOR-without-WMO lane). Rides the record table's w column, bits 1..14.
    pub slot: u16,
}

/// One baked cell (or WMO region), published by the main-world flush.
#[derive(Clone)]
pub(crate) struct GxCellDraw {
    pub mesh: Handle<Mesh>,
    /// The recentring origin (0974's precision split): shader world = vertex + origin.
    pub origin: Vec3,
    /// Mesh-local bound (recentred); world bound = origin + this.
    pub aabb: Aabb,
    pub draws: Vec<GxItemDraw>,
    /// The exile kill bitmap (B2, 1431): bit *i* set ⇒ item *i* is punched out of the
    /// retained draw (its placement is feathering as ordinary entities, or fully faded).
    /// Rebuilt in place by the main-world scan; all-zero on WMO regions.
    pub killed: Vec<u64>,
    /// Bumped by the scan on every bitmap change — the render side syncs the record table's
    /// kill column when it sees a revision it hasn't applied.
    pub killed_rev: u32,
    /// Per-selection-grain mesh-local bounds (empty for cells): a WMO region's per-GROUP
    /// bounds, or a prop region's per-referrer-SET bounds (B4) — what the cull's admission
    /// walk tests.
    pub groups: Vec<(u16, Aabb)>,
    /// A prop region's distinct referrer sets (B4), indexed by the same u16 as `groups` /
    /// item selection: the rooms the PVS admission ORs over (empty set = unnamed — admitted
    /// bare, never exterior-gated). Empty on cells and WMO regions.
    pub sets: Vec<std::sync::Arc<[u16]>>,
}

/// Marks the ONE view the retained pass draws into — the world camera. Without this the node
/// would run for EVERY Core3d view, including the portrait-booth bakes, and paint world cells
/// into a portrait with the booth's view matrices (the cull list is the world camera's).
#[derive(Component, Clone, Copy, Default, ExtractComponent)]
pub(crate) struct StaticGxView;

/// Insert the marker on the world camera (idempotent — the camera can respawn).
fn mark_world_camera(
    mut commands: Commands,
    cam: Query<Entity, (With<crate::view::WorldCamera>, Without<StaticGxView>)>,
) {
    for e in &cam {
        commands.entity(e).insert(StaticGxView);
    }
}

/// One admitted entry of the doodad-phase draw list (B4): ADT-doodad cells and WMO-prop
/// regions are the SAME drain phase in the 1.12 order (both are the M2 scene, after the WMO
/// phase), so the cull sorts them near-first TOGETHER — a far cell must not shade before a
/// near building's furniture.
#[derive(Clone, PartialEq)]
pub(crate) enum GxDoodadVis {
    Cell((i32, i32)),
    /// A prop region + this frame's per-referrer-SET verdicts.
    Prop(Entity, GxSel),
}

/// One region's per-selection-grain verdicts for this frame — a WMO region's grain is the GROUP,
/// a prop region's the referrer-SET index, and both index these vectors the same way.
#[derive(Clone, Default, PartialEq)]
pub(crate) struct GxSel {
    /// Drawn this frame: PVS ∧ frustum ∧ farclip ∧ the exterior window gate.
    pub drawn: Vec<bool>,
    /// On the interior fog lane — the client's per-group `[0xca7f00]`, resolved by the portal
    /// flood ([`crate::wmo_portal::GroupPvs::interior_fog`]). Rides beside `drawn` because it is
    /// the same walk's answer at the same grain, and because the node syncs it into the record
    /// table exactly where it already syncs the kill column.
    pub fog: Vec<bool>,
}

/// The published half the render world clones each frame. The baked regions sit behind `Arc`:
/// the 1435 band map priced the publish + extract clone pair at 0.39 ms/f —
/// tens of thousands of `GxItemDraw`s memcpy'd twice a frame — so the per-frame clones are
/// refcount bumps now, and the ONE writer that mutates a published region (the kill scan's
/// bitmap rebuild) pays a copy-on-write of that region alone, only on a real fade transition.
#[derive(Clone, Default, Resource, ExtractResource)]
pub(crate) struct GxWorld {
    pub cells: HashMap<(i32, i32), std::sync::Arc<GxCellDraw>>,
    /// This frame's doodad-phase draw list, near-first across cells AND prop regions (B4):
    /// frustum + farclip + exterior gate at cell/set granularity, PVS per set.
    pub visible: Vec<GxDoodadVis>,
    /// The WMO regions (slice 2), keyed by placement instance entity.
    pub wmos: HashMap<Entity, std::sync::Arc<GxCellDraw>>,
    /// The prop regions (B4), keyed by the same instance entity as `wmos` (their lifecycle),
    /// held apart so prop arrivals never re-bake building geometry.
    pub props: HashMap<Entity, std::sync::Arc<GxCellDraw>>,
    /// This frame's per-group admission per region (indexed by absolute group index): the
    /// portal flood's verdict collapsed to CPU range selection — the node draws exactly the
    /// runs whose group bit is set.
    pub visible_wmos: Vec<(Entity, GxSel)>,
}

use super::pool::GxTexturePool;

/// Record column `w`, bit 14 — the per-item **interior fog** lane, written per
/// frame by the fog sync and read by `static_gx.wgsl`'s fog select. Bit 0 is the exile kill bit
/// and bits 1..=13 the interior-prop probe slot, so 14 is the first free bit.
const RECORD_FOG_BIT: u32 = 1 << 14;

/// One coalesced draw run: adjacent live bake items sharing (bind-group slot, pipeline
/// bucket, group). Killed items are SKIPPED at build time (B3): a run never carries an exiled
/// or gone item, so a far cell of fully-faded faders submits no vertex work at all (the WGSL
/// kill-bit collapse stays as the belt for the same frame's record table).
struct GxRun {
    /// Index into the cell's `bind_groups` (NOT a pool class index).
    slot: usize,
    cutout: bool,
    two_sided: bool,
    index_range: Range<u32>,
    /// The WMO group every item in this run belongs to (`None` = a cell run, always drawn) —
    /// the bake sorts group inside (bucket, texture), so runs are group-homogeneous by
    /// construction and the flood's per-group verdict selects whole runs.
    group: Option<u16>,
}

/// A cell's assembled GPU state, cached across frames; rebuilt when the bake (mesh handle)
/// changes — which, with the shared pool, costs a record table and a few bind groups, never
/// a texture.
struct GxCellGpu {
    mesh: AssetId<Mesh>,
    /// One bind group per pool class this cell's items touch: (pool class index, group).
    bind_groups: Vec<(u16, BindGroup)>,
    record_table: Buffer,
    #[allow(dead_code)] // held alive for the bind groups that reference it
    cell_uniform: Buffer,
    /// Per item: its index into `bind_groups` — the run key, kept for kill-driven run
    /// rebuilds.
    item_slot: Vec<u16>,
    runs: Vec<GxRun>,
    /// CPU copy of the record table — the kill-bit sync rewrites column 3 and re-uploads.
    records: Vec<[u32; 4]>,
    /// The `killed_rev` this table last uploaded.
    killed_applied: u32,
    /// The per-grain interior-fog verdict this table's records were last written from
    /// ([`GxSel::fog`]) — empty until the first sync. Compared rather than revisioned: it is
    /// tens of bools, it changes only when the camera changes rooms, and unlike the kill bitmap
    /// it is produced by the per-frame cull rather than owned by the region.
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
    /// Keyed `(cutout, two_sided)`; specialized for the world view's (samples, format) pair —
    /// re-specialized if that pair ever changes (a window move across displays).
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
                // item → texture-array layer
                storage_buffer_read_only_sized(false, None),
                texture_2d_array(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    // TWO samplers per array — repeat and clamp, both matching the BLP loader's model albedo
    // sampler exactly. Parity with `blp.rs` was always the rule here; what changed is that
    // `blp.rs` no longer picks the numbers. The mip filter and the anisotropy are the process
    // filter policy's (`benilla_assets::tex_filter`), which the reference forces onto every
    // texture it creates from `[0x835250]`/`[0x835254]` — so this lane stays in step by asking
    // the same question rather than by copying the same literals. The shader selects by the
    // vertex word's wrap bits: a shared array cannot carry per-layer address modes. The rare
    // MIXED-wrap batch (repeat one axis, clamp the other) keeps the repeat sampler plus the
    // shader's half-texel inset clamp on its clamped axis — an approximation confined to that
    // class (decision 0763's silhouette concern, honoured per axis).
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

/// The pipeline-key query: the world view's (samples, format) inputs (the marker keeps booth
/// views out of it).
type GxViewKey = (
    &'static ExtractedView,
    &'static Msaa,
    &'static ViewTarget,
    &'static StaticGxView,
);

/// The fixed interleaved vertex layout the bake authors — **attribute-ID order**, which is
/// how Bevy interleaves a mesh's buffer: position (0), normal (1), uv (2), COLOR (5 — MOCV /
/// the baked constant tint, white default), then the custom word + anchor (988_101/988_102).
/// Kept in sync with `bake_cell` and `static_gx.wgsl`.
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

/// (Re-)specialize the four pipelines for the world view's (samples, format), and assemble
/// visible cells' GPU state: classes, arrays, layer table, bind groups, runs.
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
    // The world view's pipeline key (the marker keeps booth views out of it).
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

    // The map cleared (the main world published an empty set): the pool's assignments point
    // at content the world no longer holds — reset it with the cache. Never fires on a mere
    // area change; only `StaticGx::clear` empties ALL published maps.
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
    // Encode this frame's queued layer copies, exactly once (B3 — see the module doc; B2's
    // per-cell pending list was never drained and re-encoded every frame).
    pool.drain_pending(&render_device, &render_queue);

    // The exile kill-bit sync (B2, 1431): when the scan's bitmap revision moved, rewrite the
    // record table's kill column, re-upload, and REBUILD THE RUNS (B3) so killed items stop
    // being submitted at all. One whole-table write + one CPU coalesce per changed cell per
    // change frame — band crossings are rare and a table is tens of KB; a cell that changed
    // while out of view syncs on re-entry (the revision mismatch persists until applied).
    for vis in &gx.visible {
        let GxDoodadVis::Cell(cell) = vis else {
            continue; // prop regions carry no faders — their bitmap never revs
        };
        let (Some(gpu), Some(draw)) = (cache.cells.get_mut(cell), gx.cells.get(cell)) else {
            continue;
        };
        if gpu.killed_applied == draw.killed_rev {
            continue;
        }
        for (i, rec) in gpu.records.iter_mut().enumerate() {
            // Column w carries the probe slot in the high bits (B4) — rewrite only bit 0.
            rec[3] = (rec[3] & !1) | kill_bit(&draw.killed, i);
        }
        render_queue.write_buffer(&gpu.record_table, 0, bytemuck::cast_slice(&gpu.records));
        gpu.runs = build_runs(&draw.draws, &gpu.item_slot, &draw.killed);
        gpu.killed_applied = draw.killed_rev;
    }

    // The interior-fog sync: the client's per-group `[0xca7f00]` decides which
    // fog triple a WMO group's surfaces — and its doodad props — are pushed with, so it is a
    // per-frame property of the SELECTION, not of the bake. It rides the record table's w column
    // (bit `RECORD_FOG_BIT`) beside the kill bit, written per item from its own selection grain.
    // Rewritten only when the verdict actually moves — which is when the camera changes rooms —
    // and it never touches run membership, so no coalesce is owed.
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

/// Assemble one region's GPU state against the shared pool: pool slots for its textures, the
/// per-item record table, one bind group per touched pool class, coalesced runs. `None` while
/// any member texture is not yet resident — the region simply isn't drawn that frame (the
/// entity path streams batches in piecewise; this is the same arrival class; slots already
/// assigned stay assigned, so the retry finishes cheaper).
fn assemble_region(
    draw: &GxCellDraw,
    pool: &mut GxTexturePool,
    pipes: &GxPipelines,
    pipeline_cache: &PipelineCache,
    render_device: &RenderDevice,
    render_queue: &RenderQueue,
    images: &RenderAssets<GpuImage>,
) -> Option<GxCellGpu> {
    // Resolve every item to a pool (class, layer) — deduped globally by texture id (many
    // items share one texture: Stormwind's region carries 3,042 items over a few hundred
    // distinct BLPs, and neighbouring cells repeat most of them; per-item layers blew the
    // D2-array limit the moment a city root baked, and per-CELL arrays paid the driver churn
    // 1431 measured). Untextured items ride the white class (never sampled — TEXTURED clear).
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
    // The per-item record table: [layer, batch-order nudge, packed SIDN, kill bit + probe
    // slot] per item — the vertex word's low bits index it. Column 3's bit 0 is the exile
    // kill bit (B2), folded from the published bitmap here and kept in sync by
    // `prepare_static_gx`'s revision check (hence COPY_DST); bits 1..14 carry the interior
    // prop's SH-probe slot (B4 — 13 bits fits `MAX_PROP_PROBES` exactly).
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
    // One bind group per DISTINCT pool class this region touches; items collapse to slots.
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
        // Empty ⇒ the first fog sync always fires (the baked records carry no lane bit).
        fog_applied: Vec::new(),
    })
}

/// Coalesce adjacent LIVE items sharing (slot, bucket, group) into draw runs (the bake sorted
/// by (bucket, texture[, group]), so repeated textures and same-bucket spans fuse; a WMO run
/// never crosses a group boundary — the selection grain). Killed items are skipped whole
/// (B3): their vertices are never submitted, and the kill-bit sync rebuilds the runs on every
/// bitmap revision — a fully-gone cell coalesces to NOTHING.
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

/// The per-frame view bind group (group 0): bevy's view uniform + the shared light buffer —
/// the SAME `wow_shared_light` storage every material binds (1429: identical lighting by
/// construction).
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
        // The world camera only — a booth bake must never receive world cells (see the marker).
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
        // Cells draw whole; a WMO region draws only the runs whose group the flood admitted
        // this frame, a prop region only the runs whose referrer SET the walk admitted (the
        // selection rides beside the gpu state — `None` = draw everything). WMO regions
        // FIRST, then the doodad phase — cells and prop regions in one near-first order
        // (B3/B4): the real client's own drain order (1429's byte-true anchor: terrain →
        // WMO → … → doodad), and the buildings are the frame's best early-z occluders for
        // the doodads behind them.
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
        // (Layer copies are encoded + submitted by `prepare_static_gx`'s pool drain, exactly
        // once per texture — B3; the node encodes nothing outside its pass anymore.)
        // The four bucket pipelines must all be compiled before the first draw (all-or-none:
        // a cell drawing only its opaque half would flash cutout content off for a frame).
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
        // The pass's diagnostic span — `render/static_gx/elapsed_gpu` where the device times
        // passes (Vulkan/DX12), the CPU span everywhere: the journal's `gpu_static` column (2008).
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
                // The PVS range selection (1429's collapse): a WMO run draws iff its group's
                // admission bit is set this frame; a cell run always draws.
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
    // (The shader registers in `crate::shaders` with the other engine WGSL — `embedded_asset!`
    // derives its path from the CALLING file, so registering here would mis-prefix it.)
    // The main-world half lives inside `StaticGx`; `publish_gx_world` (registered by the
    // plugin, chained after the scene walk) mirrors it into this standalone resource for
    // `ExtractResourcePlugin` to clone.
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
        // BEFORE bevy's opaque pass: the retained statics — every building and
        // every steady doodad — are the frame's best early-Z occluders, and terrain's fragment is
        // the frame's dearest (four splat layers, the alpha map, the baked shadow: six samples a
        // pixel). Drawn first, the walls and trunks fill the depth buffer with early writes and
        // the terrain behind them is rejected before it samples anything; drawn after (the order
        // 1429 inherited from "bevy's pass, then ours"), every terrain fragment under a building
        // was shaded in full and then overwritten. The same holds against the entity lane's
        // creatures and animated doodads, which stand in front of nothing static as a rule. The
        // pass takes the depth and colour CLEAR with it (bevy's attachments clear on first use,
        // `DepthAttachment::get_attachment`); when nothing is visible it returns before touching
        // either and the opaque pass clears as before. An immediate-mode GPU (the Steam Deck,
        // every Windows part) is where this counts; a tile-based one (Apple) resolves opaque
        // order in hardware and reads the same frame either way — which is why no measurement
        // of this exists on the rig, and the player journal's `gpu_opaque`/`gpu_static` columns
        // (2008) are where the number lands.
        .add_render_graph_edges(
            Core3d,
            (Node3d::StartMainPass, StaticGxLabel, Node3d::MainOpaquePass),
        );
}

/// Copy the collector's published half into the extractable resource.
pub(super) fn publish_gx_world(gx: Res<super::StaticGx>, mut out: ResMut<GxWorld>) {
    let _t = super::gx_perf_guard(2);
    // Compare before writing: a parked frame's collector walk produces the same five
    // structures it produced last frame, and an unconditional `clone_from` through `ResMut`
    // both re-cloned them here and marked the resource changed, so the render world cloned
    // the whole set again at extract — 1435's two 0.2 ms rows, paid on every frame that
    // changed nothing. The maps hold `Arc`s, so identity is pointer identity.
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

    /// Runs fuse adjacent live items of one (slot, bucket, group); a killed item is dropped
    /// whole and SPLITS the run around it (B3: no vertex work is submitted for killed rows);
    /// a slot or bucket change breaks the run; an all-killed region coalesces to nothing.
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
        // Kill everything: nothing is submitted at all.
        assert!(build_runs(&draws, &slots, &[0b11111u64]).is_empty());
    }
}
