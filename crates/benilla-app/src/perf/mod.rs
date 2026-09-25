//! Performance instrumentation and the standing dev HUD ([`PerfPlugin`]), plus the one
//! instrument that ships to players, the FPS journal ([`FpsJournalPlugin`]).
//!
//! The HUD is the always-on cost pill (top-center), toggled by the dev chord + `P`. While synced,
//! wall frame time measures the display's present grant, not our cost, so the pill's headline is
//! CPU ms and fps is drawn dim.
//!
//! - [`clock`]: the CPU clocks (process, main thread, machine) every number is denominated in,
//! - [`stats`]: the rolling windows the pill reads,
//! - [`hud`]: the cost pill,
//! - [`trace`] / [`journal`]: the CSV instruments (`WOW_STREAM_TRACE`, `WOW_FPS_JOURNAL`); the
//!   journal also answers the `fpsJournal` CVar, in every build,
//! - [`stall`]: the stuck-main-thread self-sampler (macOS),
//! - [`census`]: the env-gated premise counters.
//!
//! `clock` and `journal` compile in the player build; everything else is
//! `#[cfg(feature = "dev")]`, and the journal depends on nothing on the dev side.
//!
//! Apple GPUs lack `TIMESTAMP_QUERY_INSIDE_PASSES`, so bevy's render diagnostics carry CPU spans
//! only there; on Vulkan and DX12 their GPU spans are the journal's `gpu_*` columns. Whole-frame
//! GPU ms on Apple comes from pass-boundary timestamps, [`gpu`] (`WOW_GPU_MS=1`).

#[cfg(feature = "dev")]
mod blend_check;
#[cfg(feature = "dev")]
mod census;
mod clock;
#[cfg(feature = "dev")]
mod crash_inject;
#[cfg(feature = "dev")]
mod gpu;
#[cfg(feature = "dev")]
mod hud;
mod journal;
#[cfg(feature = "dev")]
mod main_split;
#[cfg(feature = "dev")]
mod phases;
#[cfg(all(feature = "dev", target_os = "macos"))]
mod stall;
#[cfg(feature = "dev")]
mod stats;
#[cfg(feature = "dev")]
mod trace;

#[cfg(feature = "dev")]
use bevy::prelude::*;

#[cfg(feature = "dev")]
pub(crate) use blend_check::BlendMismatchShared;
#[cfg(feature = "dev")]
pub(crate) use clock::{process_cpu_secs, process_faults, system_cpu_ticks, thread_cpu_table};
#[cfg(feature = "dev")]
pub(crate) use gpu::{GpuMsShared, WgpuCensusShared};
#[cfg(feature = "dev")]
pub(crate) use hud::PerfHud;
pub(crate) use journal::FpsJournalPlugin;
#[cfg(test)]
pub(crate) use journal::{on_cvar, FpsJournalSetting};
#[cfg(feature = "dev")]
pub(crate) use main_split::MainThreadSplit;

/// The frame budget: a 60 fps floor. No frame should exceed this.
pub const FRAME_BUDGET_MS: f32 = 1000.0 / 60.0;

/// The dev-side instruments as one plugin: everything in this module but the journal.
#[cfg(feature = "dev")]
pub struct PerfPlugin;

#[cfg(feature = "dev")]
impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        // Bevy's render-pass diagnostics are registered by `FpsJournalPlugin`, in every build.
        main_split::plugin(app);
        app.init_resource::<stats::FrameStats>()
            .init_resource::<PerfHud>()
            .insert_resource(trace::StreamTrace {
                path: std::env::var("WOW_STREAM_TRACE").unwrap_or_default(),
                frame: 0,
                log_until: 0,
                prev_cpu_secs: None,
                prev_pipes_created: 0,
            })
            .add_systems(
                Update,
                // `toggle_hud` needs no ordering against the UI keyboard feed: its dev chord
                // cannot be typed text.
                (
                    hud::toggle_hud,
                    hud::refresh_hud_snapshot,
                    stats::sample_frame_time,
                ),
            )
            .add_systems(Update, hud::pill_quads.in_set(crate::ui_pass::UiQuadAppend))
            // `Last`, so a row carries all of this frame's Stream chain; it runs untraced too, to
            // reset the per-frame counters.
            .add_systems(Last, trace::trace_stream);
        if std::env::var_os("WOW_MESH_EVENTS").is_some() {
            app.add_systems(Update, census::count_mesh_events);
        }
        if std::env::var_os("WOW_PART_CHURN").is_some() {
            app.add_systems(Update, census::count_part_churn);
        }
        if std::env::var_os("WOW_MESH_HOLDERS").is_some() {
            app.add_systems(Last, census::mesh_holders);
        }
        if std::env::var_os("WOW_CAM_CHANGED").is_some() {
            app.add_systems(bevy::app::PostUpdate, census::count_camera_changes);
        }
        if let Some(at) = std::env::var("WOW_ARCH_CENSUS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            app.insert_resource(census::ArchCensusAt(at));
            app.add_systems(Last, census::arch_census);
        }
        if std::env::var("WOW_ROW_BLOAT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .is_some_and(|n| n > 0)
        {
            app.add_systems(Update, census::row_bloat);
        }
        if let Some(at) = std::env::var("WOW_MESH_TOUCH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            app.insert_resource(census::MeshTouchAt(at));
            app.add_systems(Update, census::mesh_touch);
        }
        #[cfg(target_os = "macos")]
        if let Some(c) = census::cpu_census::CpuCensus::from_env() {
            app.insert_resource(c);
            app.add_systems(Update, census::cpu_census::cpu_census);
        }
        if let Some(c) = census::res_census::ResCensus::from_env() {
            app.insert_resource(c);
            app.add_systems(Last, census::res_census::res_census);
        }
        // `WOW_CRASH_INJECT=<at>`: armed here, on every platform, not in the macOS-only sampler.
        crash_inject::arm(app);
        #[cfg(target_os = "macos")]
        stall::plugin(app);
        phases::plugin(app);
        gpu::plugin(app);
        blend_check::plugin(app);
    }
}
