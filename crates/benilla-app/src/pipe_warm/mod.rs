//! The pipeline warm pass + its instrument — decision 0837 (the B181 city-approach stall).
//!
//! **Why this exists:** on macOS, Bevy compiles every GPU pipeline **synchronously on the render
//! thread** — `bevy_render`'s `create_pipeline_task` has a `target_os = "macos"` carve-out (0.18
//! and 0.19 both) that `block_on`s the build regardless of `synchronous_pipeline_compilation`,
//! and the Metal half of that build runs out-of-process in `MTLCompilerService` (near-zero
//! process CPU while the frame is blocked). So any pipeline variant first drawn *live* is a
//! frame-long stall the app cannot pace; the only fix is compiling everything where a stall is
//! invisible — behind the loading cover, where 0540 put the warm-up. (The worst offender — the
//! per-batch-index depth bias that made every WMO batch its own pipeline, ~3000 variants at
//! Stormwind — left the pipeline key in this same decision: the nudge now rides `sun_scale.y`
//! into `wow_model.wgsl`'s vertex stage as uniform data.)
//!
//! The pieces:
//!
//! **The burst is paced**. Compiling behind the cover fixed *where* the stall
//! lands, not its shape: all ~1480 variants became drawable in one frame, so one frame blocked
//! 1.0–2.3 s — the cover frozen solid, and CoreAudio missing a device cycle inside it (the
//! crackle of 1114/1115). Rigs now spawn hidden and are revealed [`WARM_REVEAL_PER_FRAME`] at a
//! time, each hidden again the frame after (its pipeline is compiled by then), so every frame's
//! synchronous batch is bounded and the machine gets gaps. Same pipelines, same order of
//! magnitude of work — verified byte-identical inventories via `WOW_PIPE_TRACE`.
//!
//! - [`WarmPass`] + `spawn_menagerie` — the warm pass: one tiny rig per reachable pipeline
//!   variant — the model lane with its shard-rung and far-side twins, and the sky/water lanes
//!   (celestial, stars, clouds, gradient dome, WMO skybox, liquid; decision 0945 widened 0837's
//!   model-only scope) — parented to the world camera, spawned a few frames AFTER the entry
//!   cover rises (so the cover is on the glass before the burst, not racing it); the
//!   loading screen's clear condition holds on [`WarmPass::satisfied`] until the pipeline cache
//!   drains (10 s backstop, 0737's rule), then the menagerie despawns (roots only — recursion
//!   takes the twin booth's children). Captures skip it.
//!   Booth twins ride a real booth's layer (samples=1) AND the pass's own twin booth
//!   ([`crate::portrait::spawn_warm_booth`] — the custom-projection view key real bakes install;
//!   decision 0958), and [`warm_effect_lane`] pushes the `wow_effect` lane's whole key cross
//!   through the production stream each warm frame (the ring's first-target stall).
//! - [`PipeWatch`] — an `Arc` shared by the main and render worlds: how many pipelines the cache
//!   has ever queued, how many have settled (Ok/Err), and whether a cover currently hides the
//!   frame (loading screen up, or not in world — the glue scene is its own cover).
//! - [`watch_pipelines`] (render world, after the cache's own process step): maintains the
//!   counters and — the permanent tripwire — logs a `warn!` for every pipeline compiled while
//!   **uncovered**. That line firing in a session log IS the regression signal: it means the
//!   menagerie has a coverage hole (extend its loops, don't guess).
//! - `WOW_PIPE_TRACE=<path>` — the inventory dump: one line per pipeline creation (covered or
//!   not) with the full variant identity (shaders, defs, depth bias, blend, write mask, vertex
//!   buffers, cull), the ground truth the menagerie was built from.
//! - Two stream-trace columns (`pipes_new`, `pipes_pending` — see `perf::trace_stream`) so a
//!   compile burst is attributable on the same row as the frame that paid for it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::char_select::ClientState;
use crate::loading_screen::LoadingScreen;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_render::MaterialCache;
use benilla_world::particles::buffer::{
    begin_effect_frame, EffectBlend, EffectDrawSpec, EffectFog, EffectQuads, EffectVertex,
};

mod menagerie;
use menagerie::{spawn_menagerie, BoothCamQuery, WarmLanes};

/// The cross-world channel: cloned into the render app at plugin build. Frame alignment between
/// the two worlds is ±1 frame under pipelined rendering — fine for counters and a tripwire.
#[derive(Resource, Clone)]
pub(crate) struct PipeWatch(pub(crate) Arc<PipeShared>);

pub(crate) struct PipeShared {
    /// Pipelines the cache has ever queued (its vec only grows; ids are indices).
    pub(crate) created: AtomicUsize,
    /// Of those, how many have settled — `Ok` or a non-retryable `Err`. A retryable error
    /// (shader not loaded yet) flips back to `Queued` and correctly reads as pending.
    pub(crate) settled: AtomicUsize,
    /// Main-world truth: an opaque cover hides the frame (loading screen, or not `InWorld`).
    pub(crate) covered: AtomicBool,
}

impl PipeWatch {
    /// **Is a pipeline still being built?** — i.e. is there a variant the render world would
    /// silently DROP the draw for right now.
    ///
    /// Off macOS, `create_pipeline_task` spawns the build on the async pool
    /// (`bevy_render` 0.18.1 `pipeline_cache.rs:855`; the `block_on` arm is `cfg`'d to
    /// wasm/macOS/single-threaded), and until it settles `PipelineCache::get_render_pipeline`
    /// returns `None` — at which point `SetItemPipeline` returns
    /// [`RenderCommandResult::Skip`](bevy::render::render_phase::RenderCommandResult::Skip)
    /// (`render_phase/mod.rs:1745`) and that batch simply **does not draw this frame**. A live
    /// view redraws it the moment the pipeline lands; a one-shot bake that has already gone to
    /// sleep keeps the hole forever (report B331 — see [`crate::portrait`]'s pipeline settle).
    ///
    /// Reading `settled` first is deliberate: the pair is published unsynchronised, so the only
    /// skew this ordering can produce is a stale-low `settled` against a fresh `created` — a
    /// spurious `true`, which costs one extra rendered frame. The other order could produce a
    /// spurious `false`, which is a wrong still.
    pub(crate) fn compiling(&self) -> bool {
        let settled = self.0.settled.load(Ordering::Relaxed);
        self.0.created.load(Ordering::Relaxed) > settled
    }
}

pub(crate) fn plugin(app: &mut App) {
    let shared = Arc::new(PipeShared {
        created: AtomicUsize::new(0),
        settled: AtomicUsize::new(0),
        covered: AtomicBool::new(true),
    });
    app.insert_resource(PipeWatch(shared.clone()));
    app.init_resource::<WarmPass>();
    app.add_systems(
        Last,
        (
            publish_cover,
            publish_compile_burst,
            record_warmed_views,
            census_view_classes,
        ),
    );
    // Before the Present stage so the loading screen reads this frame's gate, not last frame's.
    app.add_systems(
        Update,
        run_warm_pass.before(benilla_world::schedule::WorldStage::Present),
    );
    // The effect-lane warm writer rides the production stream, which is cleared at the top of
    // PostUpdate's effect set — so it writes after the clear, like every family writer. The
    // HUD-quad warm rides the UI append lane the same way (cleared at the top of its own set).
    app.add_systems(PostUpdate, warm_effect_lane.after(begin_effect_frame));
    app.add_systems(
        Update,
        warm_ui_quad_lane.in_set(crate::ui_pass::UiQuadAppend),
    );
    let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
        return;
    };
    render_app.insert_resource(PipeWatch(shared));
    render_app.add_systems(Render, watch_pipelines.in_set(RenderSystems::Cleanup));
}

/// Main world → render thread: the render thread is about to spend this window blocked inside
/// Metal pipeline compilation with nothing but a still cover on screen, so it drops out of the
/// frame-critical QoS band for the duration (the band itself is `thread_qos`).
fn publish_compile_burst(warm: Res<WarmPass>) {
    let bursting = warm.spawned_at.is_some() && !warm.done;
    benilla_world::thread_qos::COMPILE_BURST.store(bursting, Ordering::Relaxed);
}

/// Main world → render world: is the frame covered right now?
fn publish_cover(
    watch: Res<PipeWatch>,
    loading: Res<LoadingScreen>,
    state: Res<State<ClientState>>,
) {
    let covered = loading.covering() || *state.get() != ClientState::InWorld;
    watch.0.covered.store(covered, Ordering::Relaxed);
}

/// Render world, after `PipelineCache::process_pipeline_queue_system` has merged this frame's new
/// pipelines and started (= on macOS: finished) their builds. `seen` is how many cache entries the
/// previous frame had — everything past it is new this frame.
fn watch_pipelines(
    cache: Res<PipelineCache>,
    watch: Res<PipeWatch>,
    mut seen: Local<usize>,
    mut settled_seen: Local<usize>,
) {
    let covered = watch.0.covered.load(Ordering::Relaxed);
    // O(1) early-out for the steady state: nothing new since last look AND everything had already
    // settled then, so the walk below could only re-derive last frame's counts. `size_hint().0`
    // because the opaque `impl Iterator` hides the backing slice's `.len()`; for a slice iterator
    // the lower bound is exact. The settled conjunct is load-bearing: Queued/Creating pipelines
    // settle on later frames without `total` moving.
    let total = cache.pipelines().size_hint().0;
    if total == *seen && *settled_seen == total {
        return;
    }
    let mut total = 0usize;
    let mut settled = 0usize;
    for (id, pipe) in cache.pipelines().enumerate() {
        total += 1;
        if matches!(
            pipe.state,
            CachedPipelineState::Ok(_) | CachedPipelineState::Err(_)
        ) {
            settled += 1;
        }
        if id >= *seen {
            let line = describe(&pipe.descriptor);
            if covered {
                debug!("pipeline compiled (covered) [{id}] {line}");
            } else {
                // THE TRIPWIRE: after 0837, a live compile is a stall the director can feel —
                // this line in a session log means the warm pass has a coverage hole.
                warn!("pipeline compiled LIVE [{id}] {line}");
            }
            if let Ok(path) = std::env::var("WOW_PIPE_TRACE") {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let cov = if covered { "covered" } else { "LIVE" };
                    let _ = writeln!(f, "[{id}] {cov} {line}");
                }
            }
        }
    }
    *seen = total;
    *settled_seen = settled;
    watch.0.created.store(total, Ordering::Relaxed);
    watch.0.settled.store(settled, Ordering::Relaxed);
}

/// One line of variant identity: everything that distinguishes this pipeline from its neighbours
/// (label + shaders + defs + the raster/depth/blend states), compact enough to grep and diff.
fn describe(desc: &PipelineDescriptor) -> String {
    fn defs(d: &[bevy::shader::ShaderDefVal]) -> String {
        let mut v: Vec<String> = d
            .iter()
            .map(|d| match d {
                bevy::shader::ShaderDefVal::Bool(k, true) => k.clone(),
                bevy::shader::ShaderDefVal::Bool(k, false) => format!("!{k}"),
                bevy::shader::ShaderDefVal::Int(k, i) => format!("{k}={i}"),
                bevy::shader::ShaderDefVal::UInt(k, u) => format!("{k}={u}"),
            })
            .collect();
        v.sort();
        v.join("+")
    }
    match desc {
        PipelineDescriptor::RenderPipelineDescriptor(d) => {
            let label = d.label.as_deref().unwrap_or("?");
            let vs = d
                .vertex
                .shader
                .path()
                .map_or_else(|| format!("{:?}", d.vertex.shader.id()), |p| p.to_string());
            let vbufs: Vec<String> = d
                .vertex
                .buffers
                .iter()
                .map(|b| {
                    let locs: Vec<String> = b
                        .attributes
                        .iter()
                        .map(|a| a.shader_location.to_string())
                        .collect();
                    format!("stride{}@[{}]", b.array_stride, locs.join(","))
                })
                .collect();
            let (bias, dw, cmp) = d.depth_stencil.as_ref().map_or_else(
                || (0, false, String::from("none")),
                |ds| {
                    (
                        ds.bias.constant,
                        ds.depth_write_enabled,
                        format!("{:?}", ds.depth_compare),
                    )
                },
            );
            let frag = d.fragment.as_ref().map_or_else(
                || String::from("frag=none"),
                |f| {
                    let fs = f
                        .shader
                        .path()
                        .map_or_else(|| format!("{:?}", f.shader.id()), |p| p.to_string());
                    let tgt = f.targets.iter().flatten().next().map_or_else(
                        || String::from("none"),
                        |t| format!("blend={:?} mask={:?}", t.blend, t.write_mask),
                    );
                    format!("fs={fs} fs_defs=[{}] {tgt}", defs(&f.shader_defs))
                },
            );
            format!(
                "label={label} vs={vs} vs_defs=[{}] bufs=[{}] cull={:?} bias={bias} depth_write={dw} cmp={cmp} {frag} samples={}",
                defs(&d.vertex.shader_defs),
                vbufs.join(";"),
                d.primitive.cull_mode,
                d.multisample.count,
            )
        }
        PipelineDescriptor::ComputePipelineDescriptor(d) => {
            let label = d.label.as_deref().unwrap_or("?");
            let cs = d
                .shader
                .path()
                .map_or_else(|| format!("{:?}", d.shader.id()), |p| p.to_string());
            format!("label={label} compute={cs} defs=[{}]", defs(&d.shader_defs))
        }
    }
}

/// Marker on every menagerie entity.
#[derive(Component)]
struct WarmRig;

/// Marker on the menagerie's twin booth camera ([`crate::portrait::spawn_warm_booth`] — the
/// custom-projection view key space), so [`warm_effect_lane`] can address its
/// view. It also carries [`WarmRig`], which despawns it with the rest of the pass.
#[derive(Component)]
struct WarmBoothCam;

/// Main-world warm-pass state. The loading screen folds [`Self::satisfied`] into its clear
/// condition, so the cover holds while menagerie pipelines are still compiling.
#[derive(Resource, Default)]
pub(crate) struct WarmPass {
    /// `Time<Real>::elapsed_secs` when the menagerie spawned under the current cover; `None` =
    /// idle (no cover, or the pass already finished for this cover). **Real, not virtual**: the
    /// pass's whole subject is a burst that stalls frames, and `Time<Virtual>` clamps its delta
    /// at 250 ms — so on the virtual clock this pass measured its own 1.3–2.3 s burst as
    /// "0.27 s" and its 10 s backstop was 10 *virtual* seconds.
    spawned_at: Option<f32>,
    /// This cover's warm work is done (drained, timed out, or not applicable).
    done: bool,
    /// The 1×1 stand-in texture [`warm_effect_lane`]'s draws bind while the pass runs (a strong
    /// handle so the asset lives exactly as long as the pass; `None` = the lane isn't warming).
    effect_tex: Option<Handle<Image>>,
    /// **The menagerie has drained cleanly once in this process.** The warm set is a fixed
    /// variant cross built from `Startup`-populated stores (see `menagerie`) — not the resident
    /// map's art — and `PipelineCache` is process-global and never evicts, so a second pass
    /// compiles nothing. Re-running it under every later cover was pure cost, and *visible*
    /// cost: it holds the cover (`satisfied`) for the whole paced reveal, which measured 3.8 s
    /// on a mid-session teleport whose world was resident in 0.7 s. Later covers therefore skip
    /// the pass. If that is ever wrong the tripwire says so out loud — `PipeWatch`'s "compiled
    /// LIVE" warn is exactly the instrument for it — which is why this can be a latch and not a
    /// guess. A timeout does NOT latch it: something was still pending.
    warmed_once: bool,
    /// **The cameras the menagerie parents rigs to** — the world camera, one real portrait booth,
    /// the twin booth and the orthographic twin. Recorded as entities at spawn so
    /// [`record_warmed_views`] can read their LIVE view key each frame instead of restating what
    /// the spawn code meant (2264).
    anchors: Vec<Entity>,
    /// **The view keys those anchors actually carried while the pass ran** — the census's only
    /// notion of "warm". Never a rule, never an inference: if a rig did not render through it,
    /// it is not in here.
    warmed_views: Vec<ViewClass>,
    /// Pacing state (1116): rigs warmed so far, when the last slice went out, how many frames
    /// the reveal spanned, and the slice currently on screen (hidden again next frame).
    revealed: usize,
    last_reveal: f32,
    reveal_frames: u32,
    showing: Vec<Entity>,
}

impl WarmPass {
    /// Cover-lift gate: false while the menagerie still has pipelines in flight.
    pub(crate) fn satisfied(&self) -> bool {
        self.done
    }
}

/// The last revealed slice must have been extracted + drawn + its pipelines queued before
/// `pending == 0` means anything (the counters cross worlds ±1 frame) — anchored to the last
/// reveal, not to the spawn, now that the reveal is paced.
const WARM_SETTLE_SECS: f32 = 0.25;
/// Rigs revealed per frame — the pacing slice.
///
/// On macOS Bevy compiles every pipeline with `block_on` **inline on the render thread**, and
/// `PipelineCache::process_queue` drains the whole backlog in one frame with no budget
/// (bevy_render 0.18.1 `pipeline_cache.rs:869` and `:697`; `synchronous_pipeline_compilation`
/// is `cfg`'d out of existence here). So the burst's shape is set entirely by how many variants
/// we make *drawable* per frame. Revealing all ~1480 at once bought one unbroken 1.3 s (warm
/// shader cache) to 2.3 s (cold) frame — the loading screen frozen solid, and CoreAudio's IO
/// cycle missing its hardware deadline inside it (1114/1115). A slice this size keeps each
/// frame's compile batch inside one 43 ms device cycle, so the audio HAL always gets its turn
/// and the cover animates instead of freezing.
///
/// Override with `$WOW_WARM_SLICE` for A/B work; **0 means unpaced** (the pre-1116 behaviour,
/// kept as the baseline arm — the honest way to re-measure the burst this const exists to
/// break up).
const WARM_REVEAL_PER_FRAME: usize = 24;

/// The pacing slice actually in force, `$WOW_WARM_SLICE` applied once.
fn reveal_slice() -> usize {
    static SLICE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *SLICE.get_or_init(|| {
        let n = std::env::var("WOW_WARM_SLICE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map_or(
                WARM_REVEAL_PER_FRAME,
                |n| if n == 0 { usize::MAX } else { n },
            );
        if n != WARM_REVEAL_PER_FRAME {
            info!("pipeline warm: slice overridden to {n} rigs/frame (WOW_WARM_SLICE)");
        }
        n
    })
}

/// Marker: this rig has had its one visible frame, so its pipeline is compiled and it is hidden
/// again. Without it a slice stays drawable for the rest of the pass and every later frame
/// redraws the whole warmed set, so the pass's per-frame cost climbs as it runs. (Measured: it
/// does *not* move the pass's total wall time — the per-frame floor is elsewhere — it keeps the
/// per-frame cost flat, which is what pacing is trying to buy.)
#[derive(Component)]
struct Warmed;

/// The pacing query: every rig but the twin booth CAMERA, which is a view, not a variant —
/// hiding it would take the booth-layer rigs' camera away mid-pass.
type WarmRigVis<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Visibility, Has<Warmed>),
    (With<WarmRig>, Without<WarmBoothCam>),
>;

/// 0737's rule: never hold a cover unbounded. A timeout fires the tripwire-adjacent warn and
/// releases; the remaining compiles land live (the pre-0837 world, once, with a named cause).
const WARM_TIMEOUT_SECS: f32 = 10.0;
fn run_warm_pass(
    mut commands: Commands,
    mut warm: ResMut<WarmPass>,
    watch: Res<PipeWatch>,
    cover: Res<crate::loading_screen::EntryCover>,
    time: Res<Time<Real>>,
    camera: Query<Entity, With<benilla_world::view::WorldCamera>>,
    rigs: Query<(Entity, Option<&ChildOf>), With<WarmRig>>,
    mut rig_vis: WarmRigVis,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    mut lanes: WarmLanes,
    booth: BoothCamQuery,
    mut gizmos: Gizmos,
    mut cache: Local<MaterialCache>,
    shared_light: Option<Res<benilla_world::lighting::SharedLightBuffer>>,
) {
    // `EntryCover` already IS "a loading cover is up and we are in world", counted once for the
    // whole client (see its doc); this used to spell the pair out for itself.
    if !cover.covering() {
        // No world cover → nothing to hold; a leftover menagerie (timeout, teleport race)
        // despawns. `done` stays true so the gate never blocks an uncovered frame.
        warm.done = true;
        warm.spawned_at = None;
        warm.effect_tex = None;
        warm.showing.clear();
        despawn_rigs(&mut commands, &rigs);
        return;
    }
    // Captures boot straight in-world, deterministic by construction — no menagerie in a shot.
    if benilla_world::dev_state::deterministic_run() {
        warm.done = true;
        return;
    }
    let now = time.elapsed_secs();
    // Already warmed in this process: nothing to compile, so nothing to hold the cover for.
    if warm.warmed_once {
        warm.done = true;
        return;
    }
    let Some(spawned) = warm.spawned_at else {
        // The cover just rose (or the world just became live under one): raise the gate and
        // spawn the menagerie once the camera + shared light exist (both are entry-frame-early;
        // until they do, the gate holds the cover, which is exactly right) — and once the cover
        // has actually reached the glass ([`EntryCover`]), so the burst is hidden rather than
        // holding the frozen character screen.
        warm.done = false;
        let Ok(cam) = camera.single() else { return };
        let Some(light) = shared_light.as_ref() else {
            return;
        };
        if !cover.presented() {
            return;
        }
        warm.spawned_at = Some(now);
        warm.last_reveal = now;
        warm.revealed = 0;
        warm.reveal_frames = 0;
        // The twin booth: the custom-projection view key space real bakes use — the real
        // booths warm the placeholder-Perspective class, this camera the NONSTANDARD one. It is
        // a WarmRig, so every despawn path below cleans it up with the rigs.
        let warm_booth = crate::portrait::spawn_warm_booth(&mut commands, &mut lanes.images);
        commands
            .entity(warm_booth.0)
            .insert((WarmRig, WarmBoothCam));
        // The orthographic twin (2262): the THIRD projection class, the one the UI model tile
        // atlas draws through. Same deal — a WarmRig, so the despawn paths take it too.
        let warm_ortho = crate::ui_models::spawn_warm_tile_cam(&mut commands, &mut lanes.images);
        commands.entity(warm_ortho.0).insert(WarmRig);
        // The effect lane's stand-in texture — held for the life of the pass.
        warm.effect_tex = Some(lanes.images.add(Image::default()));
        // The anchors the census measures "warm" from (2264): every camera the menagerie actually
        // hangs rigs on. Recorded as entities, not as remembered shapes.
        warm.anchors = vec![cam, warm_booth.0, warm_ortho.0];
        warm.anchors.extend(booth.iter().next().map(|(e, _)| e));
        warm.warmed_views.clear();
        let count = spawn_menagerie(
            &mut commands,
            cam,
            booth.iter().next(),
            &warm_booth,
            &warm_ortho,
            &mut meshes,
            &mut materials,
            &mut lanes,
            &mut cache,
            &light.0,
        );
        info!("pipeline warm: menagerie up ({count} variants, {WARM_REVEAL_PER_FRAME}/frame)");
        return;
    };
    if warm.done {
        return;
    }
    // The gizmo-line lane: gizmos are immediate-mode, so the warm draw happens per frame
    // while the pass runs — one tiny line through the DEFAULT config group, exactly the config
    // the bowstring draws with, compiles the `LineGizmo` pipeline that otherwise waits for the
    // first bow-wielder in view.
    gizmos.line(
        Vec3::new(0.0, 0.0, -0.5),
        Vec3::new(0.001, 0.0, -0.5),
        Color::WHITE,
    );
    // The pacing slice (1116). Two moves per frame, in this order:
    //
    // 1. Hide the slice revealed LAST frame. Its rigs were extracted and drawn by that frame's
    //    render, which is where the (synchronous) compile happened — they have nothing left to
    //    contribute, and leaving them visible makes every later frame redraw the whole warmed
    //    set. Hiding is safe precisely because extraction for the frame that revealed them has
    //    already run: this system is in `Update`, an extract behind.
    // 2. Reveal the next slice, bounding this frame's compile batch.
    //
    // A frame that reveals nothing means the menagerie is fully warmed, and only then can
    // `pending == 0` mean "drained".
    for e in std::mem::take(&mut warm.showing) {
        if let Ok((_, mut vis, _)) = rig_vis.get_mut(e) {
            *vis = Visibility::Hidden;
        }
    }
    let slice = reveal_slice();
    for (e, mut vis, warmed) in &mut rig_vis {
        if warmed {
            continue;
        }
        if warm.showing.len() >= slice {
            break;
        }
        *vis = Visibility::Visible;
        commands.entity(e).insert(Warmed);
        warm.showing.push(e);
    }
    let revealed_now = warm.showing.len();
    if revealed_now > 0 {
        warm.revealed += revealed_now;
        warm.last_reveal = now;
        warm.reveal_frames += 1;
    }
    let all_revealed = revealed_now == 0;
    let last_reveal = warm.last_reveal;
    let pending = watch
        .0
        .created
        .load(Ordering::Relaxed)
        .saturating_sub(watch.0.settled.load(Ordering::Relaxed));
    if all_revealed && pending == 0 && now - last_reveal >= WARM_SETTLE_SECS {
        warm.done = true;
        warm.warmed_once = true;
        warm.effect_tex = None;
        despawn_rigs(&mut commands, &rigs);
        info!(
            "pipeline warm: {} rigs drained in {:.2}s over {} paced frames",
            warm.revealed,
            now - spawned,
            warm.reveal_frames,
        );
    } else if now - spawned >= WARM_TIMEOUT_SECS {
        warm.done = true;
        warm.effect_tex = None;
        despawn_rigs(&mut commands, &rigs);
        warn!("pipeline warm: TIMED OUT with {pending} pipelines pending — cover released");
    }
}

/// **The view-class census** (2262, rebuilt by 2264) — 0958's claim, turned from a sentence into
/// an instrument that can actually fire.
///
/// 0958 verified that "the whole 3-D view space is `(samples, projection class)`, and both classes
/// of both sample counts are now warm". True the day it was written; false a month later, when
/// 2013 gave the UI model tile atlas an orthographic camera — a third class, warmed by nothing,
/// whose first tile compiled its whole batch set live. Neither lane-coverage gate test can see a
/// *camera*, so nothing caught it.
///
/// **2262's first attempt at this census could not fire.** It decided what was warm from a rule it
/// held in its own head — "samples=1 is warm in all three classes, and the world camera's own pair
/// is warm" — and then skipped every camera matching it. But `gxMultisample` registers `"1"`
/// (`cvars.rs`), `MsaaSetting::default()` is 1, and **only the world camera ever reads it**
/// (`player/setup.rs:99`, `:118`); every other 3-D camera hard-codes `Msaa::Off` through
/// `booth_view_shape()`. So `samples == 1` covered the entire population and the loop body was
/// unreachable — including for the very camera 2013 added. An instrument that restates what the
/// author believed is not an instrument.
///
/// So it no longer believes anything. [`WarmPass::warmed_views`] records the view key of each
/// camera the menagerie **actually parented rigs to**, read off those cameras live while the pass
/// runs; the census compares every `Camera3d` against that recorded set and nothing else. It also
/// runs for the whole session rather than once at drain, warning once per distinct unwarmed key,
/// because the class a booth is warm in is one it installs *at runtime* on its first bake — which
/// a single read at drain is too early to see.
fn census_view_classes(
    warm: Res<WarmPass>,
    mut reported: Local<Vec<ViewClass>>,
    cams: ViewCamQuery,
) {
    if !warm.warmed_once {
        return;
    }
    for (name, projection, msaa, hdr) in &cams {
        let class = view_class(projection, msaa, hdr);
        if warm.warmed_views.contains(&class) || reported.contains(&class) {
            continue;
        }
        reported.push(class);
        warn!(
            "pipeline warm: camera {} draws through {} — a view key the menagerie never rendered \
             a rig through, so its whole model-pipeline space compiles LIVE on first sight. Give \
             it a warm arm (decisions 0958/2262/2264). Warm keys this session: {}.",
            name.map_or("<unnamed>", Name::as_str),
            class.describe(),
            warm.warmed_views
                .iter()
                .map(ViewClass::describe)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
}

/// Every 3-D camera in the world, for [`census_view_classes`]: the name it reports itself by, and
/// the three components that decide its mesh-pipeline view key.
type ViewCamQuery<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static Name>,
        Option<&'static Projection>,
        Option<&'static Msaa>,
        Has<bevy::render::view::Hdr>,
    ),
    With<Camera3d>,
>;

/// The key a 3-D view contributes to `MeshPipelineKey`, as [`census_view_classes`] compares them.
///
/// `hdr` is here because it is a key bit in its own right AND the gate on two more: bevy admits
/// `TONEMAP_IN_SHADER` and `DEBAND_DITHER` into the key only when the view is **not** HDR
/// (`bevy_pbr-0.18.1` `render/mesh.rs:418`), and `Camera3d`'s required components hand a camera
/// both by default. 0958's "every 3-D camera is HDR so tonemap/dither are dead axes" is true of
/// every camera we spawn today and is exactly the kind of sentence this census exists to stop
/// trusting: a new 3-D camera that forgets `Hdr` flips three key bits at once, and 2262's version
/// did not look at it.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) struct ViewClass {
    projection: &'static str,
    samples: u32,
    hdr: bool,
}

impl ViewClass {
    fn describe(&self) -> String {
        format!(
            "{}/samples={}/{}",
            self.projection,
            self.samples,
            if self.hdr { "hdr" } else { "no-hdr" }
        )
    }
}

fn view_class(projection: Option<&Projection>, msaa: Option<&Msaa>, hdr: bool) -> ViewClass {
    ViewClass {
        // bevy_pbr folds exactly these three into `MeshPipelineKey` (`bevy_pbr-0.18.1`
        // `render/mesh.rs:397`); the match is exhaustive so a fourth variant upstream stops the
        // build here rather than opening a silent hole. A view with no `Projection` at all sets
        // no bits, which is the NONSTANDARD pattern — the same class as Custom.
        projection: match projection {
            Some(Projection::Perspective(_)) => "Perspective",
            Some(Projection::Orthographic(_)) => "Orthographic",
            Some(Projection::Custom(_)) | None => "Custom",
        },
        samples: msaa.copied().unwrap_or_default().samples(),
        hdr,
    }
}

/// The menagerie's anchor cameras, read for their LIVE view key by [`record_warmed_views`] —
/// entity first, so the recorder can match against [`WarmPass::anchors`].
type AnchorViewQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static Projection>,
        Option<&'static Msaa>,
        Has<bevy::render::view::Hdr>,
    ),
    With<Camera3d>,
>;

/// Record the view key of every camera the menagerie actually parents rigs to, while the pass is
/// running. Read off the live cameras rather than restated from what the spawn code intended —
/// that restatement is what made 2262's census unable to fire.
fn record_warmed_views(mut warm: ResMut<WarmPass>, cams: AnchorViewQuery) {
    if warm.spawned_at.is_none() || warm.done {
        return;
    }
    let anchors = warm.anchors.clone();
    for (e, projection, msaa, hdr) in &cams {
        if !anchors.contains(&e) {
            continue;
        }
        let class = view_class(projection, msaa, hdr);
        if !warm.warmed_views.contains(&class) {
            warm.warmed_views.push(class);
        }
    }
}

/// Tear the pass down by despawning only its ROOT entities. `despawn` is recursive, and the twin
/// booth's rigs are *children* of the twin booth camera — itself a `WarmRig` — so despawning
/// every query row queues the children twice (once explicitly, once via the parent's recursion):
/// a warn per child on the teardown frame. Children of a live camera (the world camera,
/// a real booth) still get their explicit despawn.
fn despawn_rigs(commands: &mut Commands, rigs: &Query<(Entity, Option<&ChildOf>), With<WarmRig>>) {
    for (e, child_of) in rigs {
        if child_of.is_some_and(|c| rigs.contains(c.parent())) {
            continue;
        }
        commands.entity(e).despawn();
    }
}

/// The **effect-lane** warm writer (the 07:45 log's [831], the selection ring's
/// first-target stall). `wow_effect` is a custom `SpecializedRenderPipeline` lane, not a
/// `MaterialPlugin` one, so no menagerie *entity* can reach it: its pipelines exist only when a
/// draw record sits in the shared stream at queue time. So while the pass runs, this pushes one
/// degenerate draw per reachable [`EffectPipelineKey`] — the full blend × raster-bias cross
/// ({Add, Alpha, Opaque, AlphaKey, Multiply, Mod2x} × {0, ground-decal, blob-shadow}; the key's own doc
/// pins the closed bias set) — through the PRODUCTION stream (`EffectQuads` → extract → queue →
/// specialize), once per view class: the world camera (samples=N) and the twin booth (samples=1).
/// The queue path specializes per matching view regardless of coverage, so a 4-vertex sliver at
/// the origin compiles the whole space behind the cover; the stand-in texture keeps the prepare
/// half exercised too. Per-frame like the gizmo line: the stream clears every frame.
///
/// [`EffectPipelineKey`]: benilla_world::particles::render::EffectPipelineKey
/// The HUD-substrate warm (0958's sweep, residual): on a normal entry the HUD's first quad batch
/// lands under the cover — but nothing structural holds that timing (a slow Interface load would
/// land it after the lift). One invisible overlay quad per warm frame pins the compile inside the
/// cover window; the append lane clears itself every frame, so nothing lingers.
///
/// This warms the HUD's **batch mesh** layout (POSITION + UV_0 + COLOR) only. 0958 read
/// `UiQuadMaterial` as "exactly ONE pipeline"; it is two, because a `Material2d` pipeline is
/// keyed on the mesh layout as well as the view, and the minimap interior composite draws the
/// same material on a `Rectangle` (POSITION + NORMAL + UV_0). That second one is warmed as a rig
/// in [`menagerie`], not here — this lane's stream only ever builds the batch layout (2262).
fn warm_ui_quad_lane(warm: Res<WarmPass>, mut quads: ResMut<crate::ui_pass::UiQuads>) {
    if warm.spawned_at.is_none() || warm.done {
        return;
    }
    quads.overlays.push(crate::ui_pass::UiQuad {
        rect: Rect::new(0.0, 0.0, 1.0, 1.0),
        color: [0.0, 0.0, 0.0, 0.0],
        ..default()
    });
}

fn warm_effect_lane(
    warm: Res<WarmPass>,
    mut quads: ResMut<EffectQuads>,
    world_cam: Query<Entity, With<benilla_world::view::WorldCamera>>,
    warm_booth: Query<Entity, With<WarmBoothCam>>,
) {
    if warm.spawned_at.is_none() || warm.done {
        return;
    }
    let Some(tex) = warm.effect_tex.as_ref() else {
        return;
    };
    for cam in world_cam.iter().chain(warm_booth.iter()) {
        for blend in [
            EffectBlend::Add,
            EffectBlend::Alpha,
            EffectBlend::Opaque,
            EffectBlend::AlphaKey,
            EffectBlend::Multiply,
            EffectBlend::Mod2x,
        ] {
            // The rasterizer settle is a PAIR — constant and slope-scale — and both halves are
            // pipeline-key axes, so they are warmed as the pairs that actually ship, not as a
            // cross product: every ground decal shares `Rung::DECAL_RASTER` (1817) and the foam
            // takes its own, much smaller one. A hole here is a live compile the first time anyone
            // wades or drops a shadow.
            for (raster_bias, raster_slope) in [
                (0, 0.0),
                (benilla_world::sky_order::Rung::DECAL_RASTER, 0.0),
                (
                    benilla_world::sky_order::Rung::FOAM_RASTER,
                    benilla_world::sky_order::Rung::FOAM_RASTER_SLOPE,
                ),
            ] {
                // `lit` is a pipeline-key axis (a shader def), so BOTH arms are warmed — a hole
                // here is a first-lit-emitter compile mid-play, which is the whole failure this
                // module exists to prevent (0937's holes, 0958's blind lanes). The lit arm is
                // rare content (400 of 7792 emitters) which is exactly why it would otherwise
                // never be warm when it finally shows up.
                for lighting in [
                    benilla_world::particles::buffer::EffectLighting::None,
                    benilla_world::particles::buffer::EffectLighting::Scene,
                ] {
                    let start = quads.begin();
                    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                        quads.verts.push(EffectVertex {
                            pos: [u * 0.01, v * 0.01, 0.0],
                            uv: [u, v],
                            color: [1.0, 1.0, 1.0, 1.0],
                        });
                    }
                    quads.commit_quads(
                        start,
                        EffectDrawSpec {
                            cam,
                            texture: tex.id(),
                            blend,
                            fog: EffectFog::Off,
                            lighting,
                            anchor: Vec3::ZERO,
                            bias: 0.0,
                            raster_bias,
                            raster_slope,
                            cam_relative: false,
                            no_depth_test: false,
                            main_entity: cam,
                            light: None,
                            clip: None,
                        },
                    );
                }
            }
        }
        // The depth-test-off arm (2076), warmed as the ONE combination that ships rather than as
        // another factor of the cross product above — the weapon swing trail is its only producer
        // and it always draws alpha-blended, unlit, with no rasterizer settle. A hole here is a
        // live compile on the first Heroic Strike anyone lands, which is the whole failure this
        // module exists to prevent; a doubled cross product would be 36 more warm draws per camera
        // for pipelines nothing will ever ask for.
        let start = quads.begin();
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            quads.verts.push(EffectVertex {
                pos: [u * 0.01, v * 0.01, 0.0],
                uv: [u, v],
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam,
                texture: tex.id(),
                blend: EffectBlend::Alpha,
                fog: EffectFog::Off,
                lighting: benilla_world::particles::buffer::EffectLighting::None,
                anchor: Vec3::ZERO,
                bias: 0.0,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: true,
                main_entity: cam,
                light: None,
                clip: None,
            },
        );
    }
}

#[cfg(test)]
mod census_tests {
    use super::{view_class, ViewClass};
    use bevy::camera::{OrthographicProjection, PerspectiveProjection, Projection};
    use bevy::render::view::Msaa;

    fn perspective() -> Projection {
        Projection::Perspective(PerspectiveProjection::default())
    }

    fn orthographic() -> Projection {
        Projection::Orthographic(OrthographicProjection::default_3d())
    }

    /// The warm set as it stood the day 2013 landed: the world camera's Perspective and the two
    /// `Msaa::Off` booth classes. No orthographic arm — because there was no orthographic camera
    /// when 0958 wrote the census down.
    fn warm_set_before_2262() -> Vec<ViewClass> {
        vec![
            view_class(Some(&perspective()), Some(&Msaa::Off), true),
            view_class(Some(&perspective()), Some(&Msaa::Sample4), true),
            view_class(None, Some(&Msaa::Off), true),
        ]
    }

    /// **The census must fire for decision 2013's tile camera.** This is the case 2262's first
    /// version could not report: that camera is `Msaa::Off` like every other booth, and 2262
    /// skipped `samples == 1` outright, so the loop body was unreachable for it — and for every
    /// other camera the client spawns, since `gxMultisample` defaults to 1 and only the world
    /// camera ever reads it. The census now compares against the keys rigs actually rendered
    /// through, so a class nothing warmed is a class it names.
    #[test]
    fn the_census_fires_for_an_unwarmed_orthographic_camera() {
        let warm = warm_set_before_2262();
        let tile_cam = view_class(Some(&orthographic()), Some(&Msaa::Off), true);
        assert!(
            !warm.contains(&tile_cam),
            "the ui_models tile camera's view key must read as unwarmed against a warm set that \
             has no orthographic arm — this is the report 2262 was written to produce"
        );
        // And the rule 2262 actually shipped would have swallowed it.
        assert_eq!(
            tile_cam.samples, 1,
            "the tile camera is Msaa::Off, like every booth"
        );
    }

    /// The other half: a class the menagerie DID render through is silent, so the census cannot
    /// cry wolf on the cameras it is meant to bless.
    #[test]
    fn the_census_is_silent_for_every_warmed_class() {
        let warm = warm_set_before_2262();
        for class in &warm {
            assert!(warm.contains(class));
        }
        let booth_after_first_bake = view_class(None, Some(&Msaa::Off), true);
        assert!(
            warm.contains(&booth_after_first_bake),
            "a booth's runtime-installed custom projection is the NONSTANDARD class, warmed by \
             the twin booth (0958)"
        );
    }

    /// `Hdr` is a key bit and the gate on two more (tonemap, deband — `bevy_pbr` `mesh.rs:418`).
    /// 2262's census did not read it, so a 3-D camera that forgot `Hdr` was invisible to it.
    #[test]
    fn dropping_hdr_is_a_different_view_key() {
        let with = view_class(Some(&perspective()), Some(&Msaa::Off), true);
        let without = view_class(Some(&perspective()), Some(&Msaa::Off), false);
        assert_ne!(with, without);
        assert!(!warm_set_before_2262().contains(&without));
    }
}
