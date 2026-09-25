//! `WOW_GPU_MS=1`: the whole-frame GPU meter. Two sentinel render passes bracket the camera
//! driver and write a timestamp pair at their pass boundaries, which Apple GPUs can sample where
//! bevy's in-pass diagnostics cannot.
//!
//! Resolving the query set in the same command buffer as the timed work reads zeros on Metal, so
//! the query set holds two pairs that alternate by frame ([`pairs`]): the `begin` node resolves
//! the pair the previous frame wrote into a ring of `MAP_READ` buffers on the graph's own encoder,
//! then writes this frame's. `Cleanup` only starts the map, which lands in the next frame's
//! `queue.submit` (every submit maintains the device), so the meter adds no submission or poll.
//! The freshest delta is published in nanoseconds through one shared `AtomicU64`.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use bevy::core_pipeline::core_2d::{Opaque2d, Transparent2d};
use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::pbr::Shadow;
use bevy::prelude::*;
use bevy::render::graph::CameraDriverLabel;
use bevy::render::render_graph::{
    Node, NodeRunError, RenderGraph, RenderGraphContext, RenderLabel,
};
use bevy::render::render_phase::{ViewBinnedRenderPhases, ViewSortedRenderPhases};
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages, MapMode};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui_render::TransparentUi;
use wgpu::{QuerySet, QuerySetDescriptor, QueryType};

/// Whether the meter is armed, read once.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_GPU_MS").as_deref() == Ok("1"))
}

/// The freshest whole-frame GPU duration in nanoseconds, written by the render app's readback;
/// 0 until the first reading.
#[derive(Resource, Clone)]
pub(crate) struct GpuMsShared(pub Arc<AtomicU64>);

/// The wgpu resource census: live buffers, textures and bind groups from wgpu-core's registry
/// report, refreshed once a frame. Every pass wgpu encodes sizes its usage trackers to these
/// counts, so pass-encode CPU scales with residency as well as with what is drawn.
#[derive(Resource, Clone)]
pub(crate) struct WgpuCensusShared(pub Arc<WgpuCensus>);

#[derive(Default)]
pub(crate) struct WgpuCensus {
    pub buffers: AtomicU64,
    pub textures: AtomicU64,
    pub bind_groups: AtomicU64,
    /// Draw calls the render phases issue this frame, per family ([`count_draws`]).
    pub draws_opaque: AtomicU64,
    pub draws_transparent: AtomicU64,
    pub draws_shadow: AtomicU64,
    pub draws_ui: AtomicU64,
}

/// Render world, `Cleanup`: refresh the census from wgpu-core's report.
fn census(instance: Res<bevy::render::renderer::RenderInstance>, shared: Res<WgpuCensusShared>) {
    let Some(report) = instance.generate_report() else {
        return;
    };
    let hub = &report.hub;
    let c = &shared.0;
    c.buffers
        .store(hub.buffers.num_allocated as u64, Ordering::Relaxed);
    c.textures
        .store(hub.textures.num_allocated as u64, Ordering::Relaxed);
    c.bind_groups
        .store(hub.bind_groups.num_allocated as u64, Ordering::Relaxed);
}

/// Render world, `Cleanup`: the frame's draw calls per phase family. A binned phase draws once
/// per bin (Metal has no multi-draw indirect) plus once per unbatchable entity and non-mesh item;
/// a sorted phase draws once per item whose batch range survived batching.
#[allow(clippy::type_complexity)]
fn count_draws(
    shared: Res<WgpuCensusShared>,
    opaque: Option<Res<ViewBinnedRenderPhases<Opaque3d>>>,
    mask: Option<Res<ViewBinnedRenderPhases<AlphaMask3d>>>,
    transparent: Option<Res<ViewSortedRenderPhases<Transparent3d>>>,
    shadow: Option<Res<ViewBinnedRenderPhases<Shadow>>>,
    ui: Option<Res<ViewSortedRenderPhases<TransparentUi>>>,
    two_d: Option<Res<ViewSortedRenderPhases<Transparent2d>>>,
    opaque_2d: Option<Res<ViewBinnedRenderPhases<Opaque2d>>>,
) {
    fn binned<BPI: bevy::render::render_phase::BinnedPhaseItem>(
        phases: Option<Res<ViewBinnedRenderPhases<BPI>>>,
    ) -> u64 {
        phases
            .map(|p| {
                p.0.values()
                    .map(|phase| {
                        let multi: usize =
                            phase.multidrawable_meshes.values().map(|b| b.len()).sum();
                        let unbatchable: usize = phase
                            .unbatchable_meshes
                            .values()
                            .map(|u| u.entities.len())
                            .sum();
                        let non_mesh: usize = phase
                            .non_mesh_items
                            .values()
                            .map(|n| n.entities.len())
                            .sum();
                        multi + phase.batchable_meshes.len() + unbatchable + non_mesh
                    })
                    .sum::<usize>() as u64
            })
            .unwrap_or(0)
    }
    fn sorted<SPI: bevy::render::render_phase::SortedPhaseItem>(
        phases: Option<Res<ViewSortedRenderPhases<SPI>>>,
    ) -> u64 {
        phases
            .map(|p| {
                p.0.values()
                    .map(|phase| {
                        phase
                            .items
                            .iter()
                            .filter(|i| !i.batch_range().is_empty())
                            .count()
                    })
                    .sum::<usize>() as u64
            })
            .unwrap_or(0)
    }
    let c = &shared.0;
    c.draws_opaque
        .store(binned(opaque) + binned(mask), Ordering::Relaxed);
    c.draws_transparent
        .store(sorted(transparent), Ordering::Relaxed);
    c.draws_shadow.store(binned(shadow), Ordering::Relaxed);
    c.draws_ui.store(
        sorted(ui) + sorted(two_d) + binned(opaque_2d),
        Ordering::Relaxed,
    );
}

const RING: usize = 4;

/// Ring-slot states: a copy may only target a `FREE` slot, since submitting into a mapped or
/// map-pending buffer is a wgpu validation error.
const FREE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;

/// `(write, resolve)` pairs for a [`GpuStamp::frame`]: a run resolves the pair the run before
/// it wrote, a later submission whose queries were reset by being written. Pair `p` is queries
/// `2p` (begin) and `2p + 1` (end).
fn pairs(frame: usize) -> (u32, u32) {
    #[allow(clippy::cast_possible_truncation)]
    let write = (frame % 2) as u32;
    (write, 1 - write)
}

/// Render-world state: the query set (two alternating stamp pairs), the resolve buffer, and the
/// mapping ring.
#[derive(Resource)]
struct GpuStamp {
    query_set: QuerySet,
    /// A 1×1 target the sentinel passes clear: an empty pass never reaches the Metal encoder and
    /// samples nothing.
    sentinel_view: wgpu::TextureView,
    resolve: Buffer,
    ring: Vec<(Buffer, Arc<AtomicU8>)>,
    /// `Cleanup`s seen, counting the creating one; the graph run after each writes pair
    /// `frame % 2` and resolves the other ([`pairs`]). The first run is armed with no
    /// [`Self::slot`]: no command has reset its query pool yet, and resolving one is forbidden
    /// on Vulkan (`VUID-vkCmdCopyQueryPoolResults-None-09402`), a GPU hang on NVIDIA.
    frame: usize,
    /// The `FREE` ring slot the next graph run copies the previous run's stamps into; `None`
    /// before any pair is written or while the ring is full, and that sample is skipped.
    slot: Option<usize>,
    period: f32,
    shared: Arc<AtomicU64>,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct GpuStampBeginLabel;
#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct GpuStampEndLabel;

struct GpuStampNode {
    index: u32,
}

impl Node for GpuStampNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        if let Some(stamp) = world.get_resource::<GpuStamp>() {
            let (write, resolve) = pairs(stamp.frame);
            if self.index == 0 {
                if let Some(slot) = stamp.slot {
                    // A later submission than the stamps it reads, on the graph's own encoder.
                    let encoder = render_context.command_encoder();
                    encoder.resolve_query_set(
                        &stamp.query_set,
                        resolve * 2..resolve * 2 + 2,
                        &stamp.resolve,
                        0,
                    );
                    encoder.copy_buffer_to_buffer(&stamp.resolve, 0, &stamp.ring[slot].0, 0, 16);
                }
            }
            // A sentinel pass, not a bare `write_timestamp`: Apple GPUs sample counters only at
            // stage boundaries, and an encoder-level stamp resolves to zero.
            let (begin, end) = if self.index == 0 {
                (Some(write * 2), None)
            } else {
                (None, Some(write * 2 + 1))
            };
            render_context
                .command_encoder()
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("gpu-ms sentinel"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &stamp.sentinel_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                        query_set: &stamp.query_set,
                        beginning_of_pass_write_index: begin,
                        end_of_pass_write_index: end,
                    }),
                    occlusion_query_set: None,
                });
        }
        Ok(())
    }
}

/// Build [`GpuStamp`] on the first render frame: `RenderPlugin` creates the device in `finish`,
/// after plugin build.
fn init_stamp(
    mut commands: Commands,
    stamp: Option<Res<GpuStamp>>,
    device: Option<Res<RenderDevice>>,
    queue: Option<Res<RenderQueue>>,
    shared: Res<GpuMsShared>,
) {
    if stamp.is_some() {
        return;
    }
    let (Some(device), Some(queue)) = (device, queue) else {
        return;
    };
    let query_set = device.wgpu_device().create_query_set(&QuerySetDescriptor {
        label: Some("gpu-ms stamps"),
        ty: QueryType::Timestamp,
        count: 4,
    });
    let resolve = device.create_buffer(&BufferDescriptor {
        label: Some("gpu-ms resolve"),
        size: 16,
        usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let ring = (0..RING)
        .map(|_| {
            (
                device.create_buffer(&BufferDescriptor {
                    label: Some("gpu-ms read"),
                    size: 16,
                    usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                Arc::new(AtomicU8::new(FREE)),
            )
        })
        .collect();
    let sentinel = device
        .wgpu_device()
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("gpu-ms sentinel"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
    commands.insert_resource(GpuStamp {
        query_set,
        sentinel_view: sentinel.create_view(&wgpu::TextureViewDescriptor::default()),
        resolve,
        ring,
        frame: 0,
        slot: None,
        period: queue.get_timestamp_period(),
        shared: shared.0.clone(),
    });
}

/// `Cleanup`, after the graph's submission: start the map on the slot the graph just filled,
/// publish every landed mapping, and arm a slot for the next run.
fn readback(stamp: Option<ResMut<GpuStamp>>) {
    let Some(mut stamp) = stamp else { return };
    // The slot stays out of the arming below until its mapping is read and released.
    if let Some(i) = stamp.slot.take() {
        let flag = stamp.ring[i].1.clone();
        flag.store(PENDING, Ordering::Relaxed);
        stamp.ring[i]
            .0
            .slice(..)
            .map_async(MapMode::Read, move |r| {
                flag.store(if r.is_ok() { MAPPED } else { FREE }, Ordering::Relaxed);
            });
    }
    // Publish every landed mapping; slots return to FREE.
    for i in 0..RING {
        if stamp.ring[i].1.load(Ordering::Relaxed) == MAPPED {
            let (t0, t1) = {
                let data = stamp.ring[i].0.slice(..).get_mapped_range();
                let words: &[u64] = bytemuck::cast_slice(&data);
                (words[0], words[1])
            };
            stamp.ring[i].0.unmap();
            stamp.ring[i].1.store(FREE, Ordering::Relaxed);
            if t1 > t0 {
                let ns = (t1 - t0) as f64 * f64::from(stamp.period);
                stamp.shared.store(ns as u64, Ordering::Relaxed);
            }
        }
    }
    // Not on the creation `Cleanup`, before any pair is written; no `FREE` slot, no sample.
    if stamp.frame >= 1 {
        stamp.slot = (0..RING).find(|&i| stamp.ring[i].1.load(Ordering::Relaxed) == FREE);
    }
    stamp.frame += 1;
}

pub(crate) fn plugin(app: &mut App) {
    if !enabled() {
        return;
    }
    let shared = Arc::new(AtomicU64::new(0));
    let counts = Arc::new(WgpuCensus::default());
    app.insert_resource(GpuMsShared(shared.clone()));
    app.insert_resource(WgpuCensusShared(counts.clone()));
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.insert_resource(GpuMsShared(shared));
    render_app.insert_resource(WgpuCensusShared(counts));
    render_app.add_systems(
        Render,
        (init_stamp, readback, census, count_draws)
            .chain()
            .in_set(RenderSystems::Cleanup),
    );
    let mut graph = render_app.world_mut().resource_mut::<RenderGraph>();
    graph.add_node(GpuStampBeginLabel, GpuStampNode { index: 0 });
    graph.add_node(GpuStampEndLabel, GpuStampNode { index: 1 });
    graph.add_node_edge(GpuStampBeginLabel, CameraDriverLabel);
    graph.add_node_edge(CameraDriverLabel, GpuStampEndLabel);
}

#[cfg(test)]
mod tests {
    use super::pairs;

    #[test]
    fn a_run_resolves_the_pair_the_run_before_it_wrote() {
        for frame in 1..12 {
            let (write, resolve) = pairs(frame);
            assert_ne!(write, resolve);
            assert!(write < 2 && resolve < 2);
            assert_eq!(resolve, pairs(frame - 1).0);
        }
    }
}
