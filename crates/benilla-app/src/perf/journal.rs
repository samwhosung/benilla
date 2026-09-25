//! The FPS JOURNAL — `/console fpsJournal 1` in any build, or `WOW_FPS_JOURNAL=<csv path>` on a
//! harness run: once a second, append one row of where the player is, what the frame cost on
//! the wall, on the CPU and on the GPU, and **what is resident** (see [`JOURNAL_HEADER`] for the
//! column order, written into every fresh file) — the "where does it dip" instrument for a
//! director-driven run, and since decision 2008 the instrument a PLAYER on hardware we do not
//! own can run for us. They play normally; the journal turns "it drops in the Dwarven District"
//! into coordinates a headless probe can tele straight back to, and its GPU columns turn a Steam
//! Deck's "GPU 94 % busy" into which pass is eating it. Negligible cost: one line of IO a second,
//! samples reused from the frame meters.
//!
//! **Player-facing, so it lives outside the `dev` seam** (2008, on 1495's precedent): the people
//! whose frames we need to read run the player build, which compiles every other instrument out
//! (1173). The CVar's knob is [`FpsJournalSetting`]; the file is
//! `benilla-config/Diagnostics/fps-journal.csv` ([`crate::local_state::fps_journal_path`]), the
//! thing a reporter attaches. `WOW_FPS_JOURNAL` names another path and turns the journal on for
//! the run regardless of the CVar — the harness lever it always was. A fresh file opens with one
//! `#` line naming the adapter, the backend and whether the device can time passes, so a journal
//! from a machine we have never seen says what it was read on.
//!
//! **The GPU columns read bevy's render diagnostics** (`RenderDiagnosticsPlugin`, registered here
//! so it is present in every build): one timestamp pair per render pass, resolved a few frames
//! later into `render/<pass>/elapsed_gpu` measurements. Those exist where the device offers
//! `TIMESTAMP_QUERY_INSIDE_PASSES` — Vulkan and DX12, so the Linux and Windows builds — and never
//! on an Apple GPU, which samples counters only at stage boundaries (the `perf` module header);
//! there the columns stay EMPTY rather than zero, so "no reading" cannot be mistaken for "free".
//! Our own passes (`static_gx`, the `ffx_glow` chain, `ui_gamma_decode`) open spans of their own,
//! because a city's biggest draw must not land in `gpu_other`. The buckets are [`gpu_bucket`]; a
//! pass this file does not name is still counted, under `gpu_other`, and `gpu_ms` is the sum of
//! every pass — the GPU's busy time inside passes, which is what "GPU-bound" reads against the
//! wall `mean_ms` beside it.
//!
//! The residency columns make it the **leak curve** instrument too: `FPS_PROBE`'s residency
//! meter samples once per run, which can only compare two runs at one point each — it cannot tell
//! "grows with distance streamed" from "grows with time elapsed", and cannot show *where* on a
//! route the cost arrives. A per-second row of `cpu_ms` beside `mats/images/uv/tint` plots the
//! per-frame cost directly against residency along one continuous leg, on the same time axis as
//! the position — so a same-map traverse (no `MapChange`, so no map-scoped eviction) shows its
//! accumulation as a slope instead of a before/after pair.

use std::path::PathBuf;
use std::time::Instant;

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice};
use bevy::time::Real;

use super::clock::{main_thread_cpu_secs, process_cpu_secs};

pub(crate) struct FpsJournalPlugin;

/// The `fpsJournal` CVar's knob (2008): on, the journal appends to the player's
/// `Diagnostics/fps-journal.csv` from the next second on; off, it stops mid-run and the file
/// keeps what it has. `/console fpsJournal 1` is the whole recipe a reporter needs.
#[derive(Resource, Default)]
pub(crate) struct FpsJournalSetting(pub(crate) bool);

/// The journal's column order, written as the first line of a fresh file (after the `#` adapter
/// line). Appended-to files keep whatever header they were created with — the columns only ever
/// grow at the end, so an older journal still parses against its own header.
const JOURNAL_HEADER: &str = "t,x,y,z,mean_ms,p95_ms,streamed,entities,cpu_ms,mats,meshes,images,\
                              m2,uv,tint,pmat,emat,skin,cmat,tex,cgeo,evicted,fx,fy,fz,main_ms,\
                              gpu_ms,gpu_opaque,gpu_static,gpu_transp,gpu_glow,gpu_post,gpu_ui,\
                              gpu_other\n";

/// The FPS journal switch's change callback (2008, 2303): a flag, the client's int-parse +
/// `!= 0`. The journal system reads the knob every frame, so the file opens on the next second
/// and closes the second it is turned off.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut journal: ResMut<FpsJournalSetting>) {
    if ev.is("fpsJournal") {
        journal.0 = ev.flag();
    }
}

impl Plugin for FpsJournalPlugin {
    fn build(&self, app: &mut App) {
        // bevy's per-pass render diagnostics — the source of the GPU columns, and (under the
        // `tracy` feature) the hook Tracy's GPU zones ride. Present in every build: its per-frame
        // cost is one query resolve and one buffer map on the render thread, and a player's
        // journal is exactly the build that has to carry it (2008).
        app.add_plugins(RenderDiagnosticsPlugin)
            .init_resource::<FpsJournalSetting>()
            .add_observer(on_cvar)
            .insert_resource(FpsJournal {
                env_path: std::env::var("WOW_FPS_JOURNAL")
                    .ok()
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from),
                path: None,
                window: Vec::new(),
                last_flush: 0.0,
                cpu_at_flush: None,
                main_at_flush: None,
                gpu: GpuAccum::default(),
            })
            .add_systems(Update, journal_fps);
    }
}

#[derive(Resource)]
struct FpsJournal {
    /// `WOW_FPS_JOURNAL`: a fixed path, on for the whole run whatever the CVar says.
    env_path: Option<PathBuf>,
    /// Where rows go while the journal is on; `None` = off, or nowhere to write (a hermetic
    /// run has no state folder, and the CVar has no other place to point).
    path: Option<PathBuf>,
    window: Vec<f32>,
    last_flush: f32,
    /// Process CPU seconds at the previous flush — the row's `cpu_ms` is this second's CPU cost
    /// per frame, the load-robust half of the measurement.
    cpu_at_flush: Option<f64>,
    /// Main-thread CPU seconds at the previous flush, for the row's `main_ms`. Exactly parallel to
    /// `cpu_at_flush`, so the two columns are the same measurement at two scopes: `cpu_ms` is every
    /// thread's work, `main_ms` the serialized part of it. A leg where they diverge is a leg whose
    /// cost moved off (or onto) the critical path — which the all-threads column alone cannot say.
    main_at_flush: Option<f64>,
    /// This second's GPU spans, folded per frame from the diagnostics store.
    gpu: GpuAccum,
}

/// The GPU columns after `gpu_ms`, in header order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GpuBucket {
    /// bevy's `main_opaque_pass_3d` — terrain, the model lane's opaque parts, the sky shells.
    Opaque = 0,
    /// Our retained static pass (`static_gx`): the WMOs and the doodads, the city's own draw.
    Static,
    /// bevy's transparent (and transmissive) 3D passes — water, glow cards, particles.
    Transparent,
    /// The `ffx_glow` chain: the quarter-res downsample, the two Gauss taps — and a bake's
    /// combine. The world's combine is the first draw of the UI camera's main pass since 2234,
    /// inside `main_transparent_pass_2d`'s own span (it has none of its own — 2258), so it lands
    /// in [`Self::Ui`] with that pass.
    Glow,
    /// The full-screen tail on every camera: tonemapping, upscaling, the MSAA writeback.
    Post,
    /// The 2D camera's passes, bevy UI, and our `ui_gamma_decode`.
    Ui,
    /// Every span this file does not name — counted, never dropped.
    Other,
}

const GPU_BUCKETS: usize = 7;

/// Which column a diagnostics path lands in. `None` = not a top-level GPU span: a CPU span, a
/// non-render diagnostic, or a span nested under another (its parent already carries it).
fn gpu_bucket(path: &str) -> Option<GpuBucket> {
    let pass = path.strip_prefix("render/")?.strip_suffix("/elapsed_gpu")?;
    if pass.contains('/') {
        return None;
    }
    Some(match pass {
        "main_opaque_pass_3d" => GpuBucket::Opaque,
        "static_gx" => GpuBucket::Static,
        "main_transparent_pass_3d" | "main_transmissive_pass_3d" => GpuBucket::Transparent,
        p if p.starts_with("ffx_glow") => GpuBucket::Glow,
        "tonemapping" | "upscaling" | "msaa_writeback" | "postprocessing" => GpuBucket::Post,
        "main_opaque_pass_2d" | "main_transparent_pass_2d" | "ui" | "ui_gamma_decode" => {
            GpuBucket::Ui
        }
        _ => GpuBucket::Other,
    })
}

/// One second's GPU spans: summed per bucket, divided at the flush by the number of frames
/// whose readback landed — not by the wall window's frame count, because bevy's diagnostics
/// mutex hands the store at most one frame per sync and drops the rest when readbacks bunch up,
/// so the honest divisor is the frames actually read.
#[derive(Default)]
struct GpuAccum {
    sum: [f64; GPU_BUCKETS],
    frames: u32,
    /// The newest measurement time consumed, so each fold reads only what arrived since. All
    /// measurements of one sync share one `Instant`, which is what makes "a frame" countable.
    seen: Option<Instant>,
    /// `WOW_GPU_PASSES=1` — the same sums per PASS, printed beside each row as a `GPU_PASSES`
    /// line: the journal's buckets fold both 2D passes, the UI pass and the gamma decode into
    /// one `gpu_ui`, and a pass-level question (an empty pass encoded every frame; one filter
    /// pass of a chain) needs the raw split. Empty and unread unless armed.
    passes: std::collections::BTreeMap<String, f64>,
}

/// `WOW_GPU_PASSES=1`, read once.
fn passes_armed() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_GPU_PASSES").is_some())
}

impl GpuAccum {
    /// Fold in every GPU measurement newer than the last fold.
    fn fold(&mut self, store: &DiagnosticsStore) {
        let mut newest = self.seen;
        let mut frame_times: Vec<Instant> = Vec::new();
        for diagnostic in store.iter() {
            let path = diagnostic.path().as_str();
            let Some(bucket) = gpu_bucket(path) else {
                continue;
            };
            let pass = passes_armed()
                .then(|| path.strip_prefix("render/")?.strip_suffix("/elapsed_gpu"))
                .flatten();
            for m in diagnostic
                .measurements()
                .filter(|m| self.seen.is_none_or(|s| m.time > s))
            {
                self.sum[bucket as usize] += m.value;
                if let Some(pass) = pass {
                    *self.passes.entry(pass.to_string()).or_default() += m.value;
                }
                if !frame_times.contains(&m.time) {
                    frame_times.push(m.time);
                }
                if newest.is_none_or(|n| m.time > n) {
                    newest = Some(m.time);
                }
            }
        }
        self.frames += frame_times.len() as u32;
        self.seen = newest;
    }

    /// The row's GPU cells — `,gpu_ms,<one per bucket>` — and the reset. All empty when no
    /// frame was read this second: the platform has no in-pass timestamps, and an empty cell
    /// says so where a zero would lie.
    fn columns(&mut self) -> String {
        let mut s = String::new();
        if self.frames == 0 {
            s.push_str(&",".repeat(GPU_BUCKETS + 1));
        } else {
            let n = f64::from(self.frames);
            let total: f64 = self.sum.iter().sum();
            s.push_str(&format!(",{:.2}", total / n));
            for bucket in self.sum {
                s.push_str(&format!(",{:.2}", bucket / n));
            }
            if passes_armed() {
                // Costliest first, ms per read frame — the raw split the buckets fold.
                let mut rows: Vec<(&String, &f64)> = self.passes.iter().collect();
                rows.sort_by(|a, b| b.1.total_cmp(a.1));
                let line: Vec<String> = rows
                    .iter()
                    .map(|(k, v)| format!("{k}={:.3}", *v / n))
                    .collect();
                eprintln!("GPU_PASSES frames={} {}", self.frames, line.join(" "));
            }
        }
        self.sum = [0.0; GPU_BUCKETS];
        self.frames = 0;
        self.passes.clear();
        s
    }
}

/// The `#` line a fresh file opens with: what the rows were read on, and whether the GPU
/// columns can ever fill (the device's in-pass timestamp feature, the one bevy's pass spans
/// need). Everything a reader needs to know before comparing two journals.
fn preamble(adapter: Option<&RenderAdapterInfo>, device: Option<&RenderDevice>) -> String {
    let (gpu, backend, driver) = adapter.map_or_else(
        || ("?".to_string(), "?".to_string(), "?".to_string()),
        |a| {
            let driver = format!("{} {}", a.driver, a.driver_info).trim().to_string();
            (
                a.name.clone(),
                format!("{:?}", a.backend),
                // Metal reports no driver string at all; a `?` reads better than a blank.
                if driver.is_empty() {
                    "?".to_string()
                } else {
                    driver
                },
            )
        },
    );
    let spans = device.map_or("?", |d| {
        let f = d.features();
        if f.contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
        ) {
            "yes"
        } else {
            "no"
        }
    });
    format!("# benilla fps journal | gpu {gpu} | backend {backend} | driver {driver} | gpu_spans {spans}\n")
}

/// The journal's residency columns, grouped because `journal_fps` is near Bevy's system-param
/// arity limit.
///
/// The `Assets<T>` counts are the totals — what the process holds. The [`ArtCensus`] half is the
/// same population **broken down by the cache that holds it**, which is what turns
/// "materials are growing" into a named holder in one row instead of a run-length probe. `evicted`
/// is the running total dropped by distance: on a same-map traverse it was structurally zero before
/// 0793, because nothing but a `MapChange` evicted anything.
///
/// [`ArtCensus`]: benilla_world::art_scope::ArtCensus
#[derive(bevy::ecs::system::SystemParam)]
struct JournalResidency<'w> {
    mats: Res<'w, Assets<benilla_assets::materials::WowModelMaterial>>,
    meshes: Res<'w, Assets<Mesh>>,
    images: Res<'w, Assets<bevy::image::Image>>,
    m2: Res<'w, Assets<benilla_assets::M2Model>>,
    uv_reg: Res<'w, benilla_world::doodad_anim::UvAnimMaterials>,
    tint_reg: Res<'w, benilla_world::doodad_anim::TintAnimMaterials>,
    art: Res<'w, benilla_world::art_scope::ArtCensus>,
    /// The **view focus** — where art is actually being asked for. Distinct from the row's `x,y,z`,
    /// which is the avatar: through a detached free-fly the body stands still while the camera covers
    /// kilometres, so on that leg the position columns describe nothing that is happening. The
    /// director's first run was exactly that leg, and reading it needed this column.
    scope: Res<'w, benilla_world::art_scope::ArtScopeState>,
}

/// The GPU side: the diagnostics store the render spans sync into, and the two facts the
/// preamble names. All optional — a headless app without a renderer has none of them, and the
/// journal then writes its CPU columns and leaves the GPU cells empty.
#[derive(bevy::ecs::system::SystemParam)]
struct JournalGpu<'w> {
    store: Option<Res<'w, DiagnosticsStore>>,
    adapter: Option<Res<'w, RenderAdapterInfo>>,
    device: Option<Res<'w, RenderDevice>>,
}

/// `NonSendMarker` pins this to the main thread, which the `main_ms` column requires:
/// [`main_thread_cpu_secs`] reports *the calling thread*, so on a worker it would silently log
/// whichever pool thread ran the flush.
fn journal_fps(
    _pin_to_main_thread: bevy::ecs::system::NonSendMarker,
    mut journal: ResMut<FpsJournal>,
    setting: Res<FpsJournalSetting>,
    time: Res<Time<Real>>,
    player: Option<Res<crate::player::Player>>,
    streamed: Query<(), With<crate::net::NetEntity>>,
    entities: Query<()>,
    residency: JournalResidency,
    gpu: JournalGpu,
) {
    let now = time.elapsed_secs();
    // The switch, read every frame: the env lever or the CVar. Turning on opens (or creates) the
    // file and restarts every per-second baseline; turning off drops the half-second in hand.
    let wanted = journal.env_path.is_some() || setting.0;
    match (wanted, journal.path.is_some()) {
        (false, false) => return,
        (false, true) => {
            info!("fps journal: off");
            journal.path = None;
            journal.window.clear();
            journal.gpu = GpuAccum::default();
            return;
        }
        (true, false) => {
            let Some(path) = journal
                .env_path
                .clone()
                .or_else(crate::local_state::fps_journal_path)
            else {
                return; // hermetic: no state folder, and nothing else to write into
            };
            // The header goes in exactly once, at creation: the rows are appended for the life
            // of the run (and across runs, deliberately — a journal accumulates legs).
            if !path.exists() {
                let head = format!(
                    "{}{JOURNAL_HEADER}",
                    preamble(gpu.adapter.as_deref(), gpu.device.as_deref())
                );
                if let Err(e) = crate::local_state::write_atomic(&path, &head) {
                    warn!("fps journal: cannot create {}: {e}", path.display());
                    return;
                }
            }
            info!("fps journal: writing {}", path.display());
            journal.last_flush = now;
            journal.cpu_at_flush = process_cpu_secs();
            journal.main_at_flush = main_thread_cpu_secs();
            // Only spans that land from here on count: the store may hold a history.
            journal.gpu = GpuAccum {
                seen: Some(Instant::now()),
                ..GpuAccum::default()
            };
            journal.path = Some(path);
        }
        (true, true) => {}
    }
    journal.window.push(time.delta_secs() * 1000.0);
    if let Some(store) = gpu.store.as_deref() {
        journal.gpu.fold(store);
    }
    if now - journal.last_flush < 1.0 {
        return;
    }
    journal.last_flush = now;
    let mut v = std::mem::take(&mut journal.window);
    if v.is_empty() {
        return;
    }
    v.sort_by(f32::total_cmp);
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let p95 = v[((v.len() - 1) as f32 * 0.95).round() as usize];
    // Raw WoW coords, so the line pastes straight into a `.go xyz` probe.
    let pos = player
        .filter(|p| p.active)
        .map(|p| benilla_assets::coords::bevy_to_wow(p.pos))
        .unwrap_or([0.0; 3]);
    // CPU per frame over this second — the number the reporter's "CPU %" compares against, and
    // the one that does not move with whatever else is compiling on this machine.
    let cpu_now = process_cpu_secs();
    let cpu_ms = match (journal.cpu_at_flush, cpu_now) {
        (Some(t0), Some(t1)) => format!("{:.2}", (t1 - t0) * 1000.0 / v.len() as f64),
        _ => String::new(),
    };
    journal.cpu_at_flush = cpu_now;
    let mut line = format!(
        "{now:.1},{:.1},{:.1},{:.1},{mean:.2},{p95:.2},{},{},{cpu_ms},{},{},{},{},{},{}",
        pos[0],
        pos[1],
        pos[2],
        streamed.iter().len(),
        entities.iter().len(),
        residency.mats.len(),
        residency.meshes.len(),
        residency.images.len(),
        residency.m2.len(),
        residency.uv_reg.0.len(),
        residency.tint_reg.0.len(),
    );
    // The per-cache breakdown, in `ArtSlot::ALL` order — which IS the header's column order.
    for slot in benilla_world::art_scope::ArtSlot::ALL {
        line.push_str(&format!(",{}", residency.art.live(slot)));
    }
    line.push_str(&format!(",{}", residency.art.dropped_total()));
    match residency.scope.focus() {
        Some(f) => line.push_str(&format!(",{:.1},{:.1},{:.1}", f[0], f[1], f[2])),
        None => line.push_str(",,,"),
    }
    // Appended at the end, per this file's own rule: the columns only ever grow there, so an
    // existing journal keeps parsing against the header it was created with.
    let main_now = main_thread_cpu_secs();
    match (journal.main_at_flush, main_now) {
        (Some(t0), Some(t1)) => {
            line.push_str(&format!(",{:.2}", (t1 - t0) * 1000.0 / v.len() as f64))
        }
        _ => line.push(','),
    }
    journal.main_at_flush = main_now;
    // The GPU cells (2008), last of all.
    let gpu_cells = journal.gpu.columns();
    line.push_str(&gpu_cells);
    line.push('\n');
    use std::io::Write;
    let Some(path) = journal.path.as_ref() else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::diagnostic::{Diagnostic, DiagnosticMeasurement, DiagnosticPath};
    use std::time::Duration;

    #[test]
    fn buckets_name_every_pass_we_draw_and_skip_what_is_not_a_gpu_span() {
        use GpuBucket::*;
        let b = |p: &str| gpu_bucket(p);
        assert_eq!(b("render/main_opaque_pass_3d/elapsed_gpu"), Some(Opaque));
        assert_eq!(b("render/static_gx/elapsed_gpu"), Some(Static));
        assert_eq!(
            b("render/main_transparent_pass_3d/elapsed_gpu"),
            Some(Transparent)
        );
        assert_eq!(b("render/ffx_glow_gauss_h/elapsed_gpu"), Some(Glow));
        assert_eq!(b("render/ffx_glow_combine/elapsed_gpu"), Some(Glow));
        assert_eq!(b("render/tonemapping/elapsed_gpu"), Some(Post));
        assert_eq!(b("render/upscaling/elapsed_gpu"), Some(Post));
        assert_eq!(b("render/ui/elapsed_gpu"), Some(Ui));
        assert_eq!(b("render/ui_gamma_decode/elapsed_gpu"), Some(Ui));
        assert_eq!(b("render/main_transparent_pass_2d/elapsed_gpu"), Some(Ui));
        // Unnamed passes are counted, not dropped.
        assert_eq!(
            b("render/early_mesh_preprocessing/elapsed_gpu"),
            Some(Other)
        );
        // CPU spans, non-render diagnostics and nested spans are not GPU cells.
        assert_eq!(b("render/main_opaque_pass_3d/elapsed_cpu"), None);
        assert_eq!(b("fps"), None);
        assert_eq!(b("render/outer/inner/elapsed_gpu"), None);
    }

    fn push(store: &mut DiagnosticsStore, path: &str, time: Instant, value: f64) {
        let p = DiagnosticPath::new(path.to_string());
        if store.get(&p).is_none() {
            store.add(Diagnostic::new(p.clone()));
        }
        store
            .get_mut(&p)
            .unwrap()
            .add_measurement(DiagnosticMeasurement { time, value });
    }

    #[test]
    fn a_flush_averages_per_frame_read_and_a_fold_takes_only_what_arrived() {
        let mut store = DiagnosticsStore::default();
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(10);
        let t2 = t0 + Duration::from_millis(20);
        // Frame 1: opaque 2, static 3, and the full-screen tail on two cameras (0.5 + 0.3).
        push(
            &mut store,
            "render/main_opaque_pass_3d/elapsed_gpu",
            t0,
            2.0,
        );
        push(&mut store, "render/static_gx/elapsed_gpu", t0, 3.0);
        push(&mut store, "render/tonemapping/elapsed_gpu", t0, 0.5);
        push(&mut store, "render/tonemapping/elapsed_gpu", t0, 0.3);
        // A CPU span beside them, never counted.
        push(
            &mut store,
            "render/main_opaque_pass_3d/elapsed_cpu",
            t0,
            99.0,
        );
        let mut acc = GpuAccum::default();
        acc.fold(&store);
        assert_eq!(acc.frames, 1);
        // Frame 2 lands; the fold reads only it (frame 1's values would double otherwise).
        push(
            &mut store,
            "render/main_opaque_pass_3d/elapsed_gpu",
            t1,
            4.0,
        );
        acc.fold(&store);
        acc.fold(&store); // nothing new: a no-op, not a double count
        assert_eq!(acc.frames, 2);
        let cols = acc.columns();
        // gpu_ms = (2 + 3 + 0.8 + 4) / 2; opaque (2 + 4) / 2; static 3 / 2; post 0.8 / 2.
        assert_eq!(cols, ",4.90,3.00,1.50,0.00,0.00,0.40,0.00,0.00");
        // The reset: a second with nothing read writes empty cells, one per column.
        assert_eq!(acc.columns(), ",,,,,,,,");
        // And a later frame is still read after the reset.
        push(
            &mut store,
            "render/main_opaque_pass_3d/elapsed_gpu",
            t2,
            1.0,
        );
        acc.fold(&store);
        assert_eq!(acc.columns(), ",1.00,1.00,0.00,0.00,0.00,0.00,0.00,0.00");
    }

    #[test]
    fn the_header_ends_with_the_gpu_cells_in_bucket_order() {
        let cols: Vec<&str> = JOURNAL_HEADER.trim_end().split(',').collect();
        let gpu: Vec<&str> = cols[cols.len() - (GPU_BUCKETS + 1)..].to_vec();
        assert_eq!(
            gpu,
            [
                "gpu_ms",
                "gpu_opaque",
                "gpu_static",
                "gpu_transp",
                "gpu_glow",
                "gpu_post",
                "gpu_ui",
                "gpu_other"
            ]
        );
        // An empty second writes exactly one cell per GPU column.
        assert_eq!(
            GpuAccum::default().columns().matches(',').count(),
            gpu.len()
        );
    }

    #[test]
    fn the_preamble_names_the_adapter_or_says_it_cannot() {
        assert_eq!(
            preamble(None, None),
            "# benilla fps journal | gpu ? | backend ? | driver ? | gpu_spans ?\n"
        );
    }
}
