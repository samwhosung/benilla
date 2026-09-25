//! The FPS journal (`/console fpsJournal 1` in any build, or `WOW_FPS_JOURNAL=<csv path>`): once
//! a second, one row of where the player is, what the frame cost on the wall, the CPU and the
//! GPU, and what is resident ([`JOURNAL_HEADER`]). It ships in the player build so a player on any
//! hardware can record a run; the file is `benilla-config/Diagnostics/fps-journal.csv`, opened
//! with a `#` line naming the adapter, the backend and whether the device can time passes.
//!
//! The GPU columns read bevy's per-pass render diagnostics, which exist only where the device
//! has `TIMESTAMP_QUERY_INSIDE_PASSES` (Vulkan and DX12, never an Apple GPU); elsewhere the cells
//! stay empty, not zero. `gpu_ms` is the sum of every pass, and an unnamed pass lands in
//! `gpu_other`.

use std::path::PathBuf;
use std::time::Instant;

use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice};
use bevy::time::Real;

use super::clock::{main_thread_cpu_secs, process_cpu_secs};

pub(crate) struct FpsJournalPlugin;

/// The `fpsJournal` CVar: on, the journal appends from the next second; off, it stops and the
/// file keeps what it has.
#[derive(Resource, Default)]
pub(crate) struct FpsJournalSetting(pub(crate) bool);

/// The column order, written once into a fresh file after the `#` adapter line. Columns only
/// ever grow at the end, so an older journal still parses against its own header.
const JOURNAL_HEADER: &str = "t,x,y,z,mean_ms,p95_ms,streamed,entities,cpu_ms,mats,meshes,images,\
                              m2,uv,tint,pmat,emat,skin,cmat,tex,cgeo,evicted,fx,fy,fz,main_ms,\
                              gpu_ms,gpu_opaque,gpu_static,gpu_transp,gpu_glow,gpu_post,gpu_ui,\
                              gpu_other\n";

/// The `fpsJournal` change callback: a flag, int-parsed and `!= 0`.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut journal: ResMut<FpsJournalSetting>) {
    if ev.is("fpsJournal") {
        journal.0 = ev.flag();
    }
}

impl Plugin for FpsJournalPlugin {
    fn build(&self, app: &mut App) {
        // Bevy's per-pass render diagnostics, the source of the GPU columns and of Tracy's GPU
        // zones, registered in every build.
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
    /// Where rows go while the journal is on; `None` when off, or on a hermetic run with no
    /// state folder.
    path: Option<PathBuf>,
    window: Vec<f32>,
    last_flush: f32,
    /// Process CPU seconds at the previous flush, for the row's per-frame `cpu_ms`.
    cpu_at_flush: Option<f64>,
    /// Main-thread CPU seconds at the previous flush, for the row's `main_ms`: `cpu_at_flush`'s
    /// measurement narrowed to the serialized part.
    main_at_flush: Option<f64>,
    /// This second's GPU spans, folded per frame from the diagnostics store.
    gpu: GpuAccum,
}

/// The GPU columns after `gpu_ms`, in header order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GpuBucket {
    /// Bevy's `main_opaque_pass_3d`: terrain, opaque model parts, the sky shells.
    Opaque = 0,
    /// Our retained static pass (`static_gx`): the WMOs and the doodads.
    Static,
    /// Bevy's transparent and transmissive 3D passes: water, glow cards, particles.
    Transparent,
    /// The `ffx_glow` chain: the quarter-res downsample, the two Gauss taps and a bake's combine.
    /// The world's combine draws inside `main_transparent_pass_2d`'s span, so it lands in
    /// [`Self::Ui`].
    Glow,
    /// The full-screen tail on every camera: tonemapping, upscaling, the MSAA writeback.
    Post,
    /// The 2D camera's passes, bevy UI, and our `ui_gamma_decode`.
    Ui,
    /// Every span this file does not name, counted, never dropped.
    Other,
}

const GPU_BUCKETS: usize = 7;

/// Which column a diagnostics path lands in; `None` for anything but a top-level GPU span (a
/// nested span's parent already carries it).
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

/// One second's GPU spans, summed per bucket and divided by the frames whose readback landed:
/// bevy hands the store at most one frame per sync and drops the rest when readbacks bunch up.
#[derive(Default)]
struct GpuAccum {
    sum: [f64; GPU_BUCKETS],
    frames: u32,
    /// The newest measurement time consumed; all measurements of one sync share one `Instant`,
    /// which is what makes a frame countable.
    seen: Option<Instant>,
    /// `WOW_GPU_PASSES=1`: the same sums per pass, printed beside each row as a `GPU_PASSES`
    /// line; empty unless armed.
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

    /// The row's GPU cells, `,gpu_ms,<one per bucket>`, and the reset; all empty when no frame
    /// was read this second.
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
                // Costliest first, ms per read frame.
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

/// The `#` line a fresh file opens with: the adapter, and whether the GPU columns can ever fill.
fn preamble(adapter: Option<&RenderAdapterInfo>, device: Option<&RenderDevice>) -> String {
    let (gpu, backend, driver) = adapter.map_or_else(
        || ("?".to_string(), "?".to_string(), "?".to_string()),
        |a| {
            let driver = format!("{} {}", a.driver, a.driver_info).trim().to_string();
            (
                a.name.clone(),
                format!("{:?}", a.backend),
                // Metal reports no driver string.
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

/// The journal's residency columns, grouped for Bevy's system-param arity limit. The
/// `Assets<T>` counts are totals; the [`ArtCensus`] half breaks the same population down by the
/// cache that holds it, and `evicted` is the running total dropped by distance.
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
    /// The view focus, where art is asked for; it leaves the avatar's `x,y,z` in a detached
    /// free-fly.
    scope: Res<'w, benilla_world::art_scope::ArtScopeState>,
}

/// The diagnostics store and the preamble's adapter and device; all absent without a renderer.
#[derive(bevy::ecs::system::SystemParam)]
struct JournalGpu<'w> {
    store: Option<Res<'w, DiagnosticsStore>>,
    adapter: Option<Res<'w, RenderAdapterInfo>>,
    device: Option<Res<'w, RenderDevice>>,
}

/// Pinned to the main thread: [`main_thread_cpu_secs`] reads the calling thread's clock.
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
    // Read every frame: turning on opens the file and restarts every baseline; turning off
    // drops the partial second.
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
                return; // hermetic: no state folder
            };
            // The header goes in once, at creation; rows append across runs.
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
    // `ArtSlot::ALL` order is the header's column order.
    for slot in benilla_world::art_scope::ArtSlot::ALL {
        line.push_str(&format!(",{}", residency.art.live(slot)));
    }
    line.push_str(&format!(",{}", residency.art.dropped_total()));
    match residency.scope.focus() {
        Some(f) => line.push_str(&format!(",{:.1},{:.1},{:.1}", f[0], f[1], f[2])),
        None => line.push_str(",,,"),
    }
    // New columns only ever append at the end of the row.
    let main_now = main_thread_cpu_secs();
    match (journal.main_at_flush, main_now) {
        (Some(t0), Some(t1)) => {
            line.push_str(&format!(",{:.2}", (t1 - t0) * 1000.0 / v.len() as f64))
        }
        _ => line.push(','),
    }
    journal.main_at_flush = main_now;
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
