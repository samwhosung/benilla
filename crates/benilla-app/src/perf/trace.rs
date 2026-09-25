//! The STREAM TRACE (`WOW_STREAM_TRACE=<csv path>`) — B181's instrument.

use bevy::prelude::*;
use bevy::time::Real;

use super::clock::process_cpu_secs;

/// A frame past this is logged even with no streamer activity — catches a spike whose cost lands
/// outside the Stream chain (command application, render extraction, asset frees frames later).
const TRACE_FRAME_MS: f32 = 25.0;
/// Frames still logged after the last streamer event: the despawn commands apply after the system,
/// the asset frees land when the last handle drops, and the render world reacts a frame later —
/// the tail is where the cost shows, so the window must outlive the event.
const TRACE_TAIL_FRAMES: u64 = 10;

/// One row per frame in which the terrain streamer did anything (plus a [`TRACE_TAIL_FRAMES`]
/// tail, plus any frame over [`TRACE_FRAME_MS`]): what was dropped/requested/spawned, the Stream
/// chain's own self-times, and the frame's wall delta beside it. **The off-by-one to remember
/// reading it: `delta_ms` on a row is the interval that *ended* at this frame's start, so frame N's
/// cost appears in row N+1.**
#[derive(Resource)]
pub(super) struct StreamTrace {
    pub(super) path: String,
    pub(super) frame: u64,
    pub(super) log_until: u64,
    /// [`process_cpu_secs`] at the previous frame — the row's `cpu_ms` is this frame's process-CPU
    /// delta (user+system, all threads): the load-robust cost meter beside the wall delta, because
    /// on this machine wall frame time moves with whoever else is compiling (a parallel
    /// build polluted this instrument's first A/B).
    pub(super) prev_cpu_secs: Option<f64>,
    /// `PipeWatch::created` at the previous frame, so `pipes_new` is per-frame like every other
    /// counter (the render world bumps the shared atomic; ±1 frame skew is inherent and fine).
    pub(super) prev_pipes_created: usize,
}

const STREAM_TRACE_HEADER: &str = "frame,t,delta_ms,cpu_ms,ents,stream_ms,furnish_ms,mfurnish_ms,\
                                   spawn_ms,collider_ms,tiles_dropped,pl_dropped,pl_ents_dropped,\
                                   tiles_req,tiles_spawned,cells_spawned,mmeshes_built,pl_spawned,\
                                   attached,adt_freed,meshes_freed,images_freed,adt_added,\
                                   meshes_added,images_added,pipes_new,pipes_pending\n";

pub(super) fn trace_stream(
    mut trace: ResMut<StreamTrace>,
    mut activity: ResMut<benilla_world::terrain_stream::StreamActivity>,
    time: Res<Time<Real>>,
    entities: Query<()>,
    mut adt_events: MessageReader<bevy::asset::AssetEvent<benilla_assets::AdtTile>>,
    mut mesh_events: MessageReader<bevy::asset::AssetEvent<Mesh>>,
    mut image_events: MessageReader<bevy::asset::AssetEvent<bevy::image::Image>>,
    pipes: Res<crate::pipe_warm::PipeWatch>,
) {
    // Taken every frame — the counters are per-frame by contract, tracing or not.
    let a = std::mem::take(&mut *activity);
    let pipes_created = pipes.0.created.load(std::sync::atomic::Ordering::Relaxed);
    let pipes_settled = pipes.0.settled.load(std::sync::atomic::Ordering::Relaxed);
    let pipes_new = pipes_created.saturating_sub(trace.prev_pipes_created);
    let pipes_pending = pipes_created.saturating_sub(pipes_settled);
    trace.prev_pipes_created = pipes_created;
    trace.frame += 1;
    if trace.path.is_empty() {
        return;
    }
    let cpu_now = process_cpu_secs();
    let cpu_ms = match (trace.prev_cpu_secs, cpu_now) {
        (Some(t0), Some(t1)) => format!("{:.2}", (t1 - t0) * 1000.0),
        _ => String::new(),
    };
    trace.prev_cpu_secs = cpu_now;
    // One pass per reader: `Added` beside `Unused` — B181's recurring spike was the free wave's
    // family, but the first-contact head frame is the ADD wave (a landed tile's cell meshes +
    // texture arrays hitting the render world's prepare at once), invisible until counted.
    //
    // The freed columns count `Unused` (last strong handle dropped), not `Removed`: a
    // `RENDER_WORLD`-only asset (chunk-cell meshes, the tile arrays) leaves the
    // main store at *extract* via the untracked path, so `Removed` never fires for it; `Unused`
    // is the release signal both usage kinds emit exactly once, and it is what actually frees
    // the GPU copy.
    fn count_events<A: bevy::asset::Asset>(
        events: &mut MessageReader<bevy::asset::AssetEvent<A>>,
    ) -> (u32, u32) {
        let (mut added, mut unused) = (0u32, 0u32);
        for e in events.read() {
            match e {
                bevy::asset::AssetEvent::Added { .. } => added += 1,
                bevy::asset::AssetEvent::Unused { .. } => unused += 1,
                _ => {}
            }
        }
        (added, unused)
    }
    let (added_adt, removed_adt) = count_events(&mut adt_events);
    let (added_mesh, removed_mesh) = count_events(&mut mesh_events);
    let (added_img, removed_img) = count_events(&mut image_events);
    let delta_ms = time.delta_secs() * 1000.0;
    if a.any_event() || removed_adt > 0 || added_adt > 0 || pipes_new > 0 {
        trace.log_until = trace.frame + TRACE_TAIL_FRAMES;
    }
    if !(a.any_event()
        || removed_adt > 0
        || added_adt > 0
        || pipes_new > 0
        || delta_ms > TRACE_FRAME_MS
        || trace.frame <= trace.log_until)
    {
        return;
    }
    if trace.frame == 1 || !std::path::Path::new(&trace.path).exists() {
        let _ = std::fs::write(&trace.path, STREAM_TRACE_HEADER);
    }
    let line = format!(
        "{},{:.2},{delta_ms:.2},{cpu_ms},{},{:.2},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{},{},{removed_adt},{removed_mesh},{removed_img},{added_adt},{added_mesh},{added_img},{pipes_new},{pipes_pending}\n",
        trace.frame,
        time.elapsed_secs(),
        entities.iter().len(),
        a.stream_ms,
        a.furnish_ms,
        a.mfurnish_ms,
        a.spawn_ms,
        a.collider_ms,
        a.tiles_dropped,
        a.placements_dropped,
        a.placement_entities_dropped,
        a.tiles_requested,
        a.tiles_spawned,
        a.cells_spawned,
        a.model_meshes_built,
        a.placements_spawned,
        a.colliders_attached,
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace.path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}
