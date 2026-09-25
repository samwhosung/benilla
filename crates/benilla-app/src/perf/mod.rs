//! `perf` — performance instrumentation + the standing dev HUD ([`PerfPlugin`]), and the one
//! instrument that ships to players: the FPS journal ([`FpsJournalPlugin`]).
//!
//! Owns the **frame-cost measurement layer** that every future subsystem is measured against (the
//! standard), and draws the always-on cost pill (top-center) — the whole HUD since 1454; anything
//! deeper is an instrument's job (the journal, the probes, Tracy). The **dev chord + `P`** toggles
//! it (`Ctrl+Shift+P`).
//!
//! The concerns, one file each:
//! - [`clock`] — the three CPU clocks (process, main thread, machine) every number is denominated in,
//! - [`stats`] — the rolling windows the pill reads,
//! - [`hud`] — the standing cost pill,
//! - [`trace`] / [`journal`] — the two CSV instruments (`WOW_STREAM_TRACE`, `WOW_FPS_JOURNAL`);
//!   the journal also answers the `fpsJournal` CVar, in every build (2008),
//! - [`stall`] — the stuck-main-thread self-sampler (macOS),
//! - [`census`] — the env-gated premise counters.
//!
//! **The dev seam runs through this module.** 1173 named `perf` dev wholesale; 2008 carved the
//! journal, and the clocks it reads, out to the player side: `clock` and `journal` compile in the
//! player build, everything else — the HUD, the meters, the census counters, [`PerfPlugin`]
//! itself — is `#[cfg(feature = "dev")]`. The rule is unchanged (`dev.rs`): dev may see anything;
//! nothing may depend on dev. The journal depends on nothing on the dev side.
//!
//! **The law the whole surface obeys: while synced, wall frame time measures the display's
//! present grant, not our cost.** Only the CPU series measure work. That is why the pill's headline
//! is `cpu ms` and not fps — on a 120 Hz-adaptive panel, cost can double with the granted interval
//! unchanged and framerate unmoved, and the pill's old red threshold (fps < 58) sat ~5.7× above a
//! healthy frame. fps stays on the pill — dim and small, the familiar anchor, never the thing
//! being watched (1448).
//!
//! The budget is a **60 fps floor = 16.7 ms; no frame should exceed it** — but note that the
//! *missed-interval* threshold is derived from the interval we actually observe, not from this
//! constant (see [`stats::DROPPED_FACTOR`]). Deep per-system attribution is via Tracy
//! (`cargo run --features tracy`) — the `info_span!` markers on the hot systems feed it.
//!
//! ⚠️ **macOS/Metal — GPU timing.** We have `Features::TIMESTAMP_QUERY` on this machine, but
//! **`TIMESTAMP_QUERY_INSIDE_PASSES` is `false`**: Apple GPUs sample counters only at stage
//! boundaries (`MTLCounterSamplingPoint::AtStageBoundary`). Bevy's `RenderDiagnosticsPlugin`
//! (registered by the journal, so present in every build) writes its timestamps *inside* passes,
//! so on Apple Silicon every span falls through to the CPU-only branch and the store carries
//! **zero** `elapsed_gpu` paths — verified on a live run: 14 diagnostic paths, all `elapsed_cpu`.
//! (bevy_render's own "Vulkan and DX12 only" comment is stale as a statement about the
//! *platform* — an Intel Mac would report `INSIDE_PASSES` and emit GPU spans.) Where the feature
//! IS present — Vulkan and DX12, so the Linux and Windows builds — those spans are the journal's
//! `gpu_*` columns (2008), which is how a Steam Deck's frame gets read without a Deck here.
//! Pass-boundary `timestamp_writes` **does** work on Apple and is the route to whole-frame
//! GPU-ms — built as [`gpu`] (`WOW_GPU_MS=1`): two sentinel render-graph nodes at ~0.03 ms/frame,
//! the query set resolved in a *different* command buffer than the timed pass or Metal returns
//! zeros (1389). Per-pass on Apple remains unbuilt: a GPU-bound frame here is identified rather
//! than timed — the one that runs long while the CPU meters stay flat, read side by side in the
//! journal or a probe line.

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

/// The dev-side instruments, as one plugin — everything in this module but the journal (2008).
#[cfg(feature = "dev")]
pub struct PerfPlugin;

#[cfg(feature = "dev")]
impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        // (bevy's render-pass diagnostics — the CPU spans here, the GPU spans on Vulkan/DX12, and
        // the hook Tracy's GPU zones ride — are registered by `FpsJournalPlugin`, in every build.)
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
                // `toggle_hud` needs no ordering against the UI keyboard feed any more: its dev
                // chord can't be typed text, so there's no `UiKeyboardCapture` to read.
                (
                    hud::toggle_hud,
                    hud::refresh_hud_snapshot,
                    stats::sample_frame_time,
                ),
            )
            // The pill rides the player-UI quad pass (1453): appended with the other overlay
            // producers, so the HUD costs a Vec clone and never touches the egui lane (1454).
            .add_systems(Update, hud::pill_quads.in_set(crate::ui_pass::UiQuadAppend))
            // `Last`, so a row carries everything this frame's Stream chain did — and the reset runs
            // whether or not anything is tracing, or the counters would accumulate forever.
            .add_systems(Last, trace::trace_stream);
        // `WOW_MESH_EVENTS=1` — who churns Mesh assets per frame? The premise counter behind
        // the Stormwind trace's `allocate_and_free_meshes` row (0.86 ms/frame at a PARKED pin):
        // the allocator answers every Modified with a free+realloc, so a steady scene should
        // show ~zero here. Printed once a second with the top mutated ids' first sighting.
        if std::env::var_os("WOW_MESH_EVENTS").is_some() {
            app.add_systems(Update, census::count_mesh_events);
        }
        // `WOW_PART_CHURN=1` — the moving-regime premise counters (1461): how many frames per
        // second despawn an M2 part (= `classify_water_side` full-walk promotions), and how
        // often model materials fire Modified (= the `AssetChanged` scan wake-ups).
        if std::env::var_os("WOW_PART_CHURN").is_some() {
            app.add_systems(Update, census::count_part_churn);
        }
        // `WOW_MESH_HOLDERS=1` — who OWNS the meshes `WOW_MESH_EVENTS` counts? Prints each
        // holder's archetype signature once a second (1461's writer hunt).
        if std::env::var_os("WOW_MESH_HOLDERS").is_some() {
            app.add_systems(Last, census::mesh_holders);
        }
        // `WOW_CAM_CHANGED=1` — is the world camera's transform bit-stable at a parked pin?
        // The premise counter for gating the per-submesh visibility sweep on frame-stable
        // camera inputs: if the controller rewrites an equal transform every frame, change
        // detection fires anyway and the gate can never hold — the controller's no-op write
        // is then the first fix, not the sweep's.
        if std::env::var_os("WOW_CAM_CHANGED").is_some() {
            app.add_systems(bevy::app::PostUpdate, census::count_camera_changes);
        }
        // `WOW_ARCH_CENSUS=<secs>` — one archetype census at t=secs: every non-empty
        // archetype's entity count beside its component set, largest first. The exact by-lane
        // entity picture (1354's anchor census generalized to every lane at once), for sizing
        // which populations are worth collapsing before designing any collapse.
        if let Some(at) = std::env::var("WOW_ARCH_CENSUS")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            app.insert_resource(census::ArchCensusAt(at));
            app.add_systems(Last, census::arch_census);
        }
        // `WOW_ROW_BLOAT=<n>` — spawn n inert clones of a live static row (see the system doc):
        // the d(cpu_ms)/d(rows) instrument the consolidation option divides by.
        if std::env::var("WOW_ROW_BLOAT")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .is_some_and(|n| n > 0)
        {
            app.add_systems(Update, census::row_bloat);
        }
        // `WOW_MESH_TOUCH=<secs>` — the `AssetChanged` tax meter (see the system's doc): from
        // `secs` onward, one scratch `Assets<Mesh>` modification per frame and nothing else, so
        // the arming's price is read as a within-run paired delta with the UI's own work removed.
        if let Some(at) = std::env::var("WOW_MESH_TOUCH")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            app.insert_resource(census::MeshTouchAt(at));
            app.add_systems(Update, census::mesh_touch);
        }
        // `WOW_CPU_CENSUS=<at>:<secs>` — the per-thread CPU census (see its module doc): where
        // the pill's cpu-ms goes, thread by thread, summing exactly to the process total.
        #[cfg(target_os = "macos")]
        if let Some(c) = census::cpu_census::CpuCensus::from_env() {
            app.insert_resource(c);
            app.add_systems(Update, census::cpu_census::cpu_census);
        }
        // `WOW_RES_CENSUS=<at>:<secs>` — the resource change census (see its module doc): on how
        // many of the window's frames each resource read as changed, noisiest first — the
        // finder for the dead-gate class 1982's `noisy=` counted five of by hand.
        if let Some(c) = census::res_census::ResCensus::from_env() {
            app.insert_resource(c);
            app.add_systems(Last, census::res_census::res_census);
        }
        // `WOW_CRASH_INJECT=<at>` — the crash reporter's standing injector (its module doc says
        // why it lives here): armed on every platform, and ahead of the stall sampler's own
        // off-switch for the same reason the sampler arms its injectors first.
        crash_inject::arm(app);
        #[cfg(target_os = "macos")]
        stall::plugin(app);
        // `WOW_FRAME_PHASES=<ms>` — which PHASE of a slow frame spent it (see the module doc).
        phases::plugin(app);
        // `WOW_GPU_MS=1` — the whole-frame GPU meter (its module doc owns the design and the
        // 1389 resolve-on-a-later-submission trap). Registers nothing when off, so campaign
        // anchors never carry its ~0.03 ms sentinel cost uninvited.
        gpu::plugin(app);
        blend_check::plugin(app);
    }
}
