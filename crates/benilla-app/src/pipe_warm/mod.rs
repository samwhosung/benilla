//! The pipeline warm pass and its instrument. On macOS Bevy compiles every pipeline synchronously
//! on the render thread, so a variant first drawn live is a frame-long stall; the pass compiles
//! every reachable variant behind the loading cover instead.
//!
//! - [`WarmPass`] and `spawn_menagerie`: one tiny rig per reachable variant, spawned once the
//!   entry cover is on screen and revealed [`WARM_REVEAL_PER_FRAME`] at a time so each frame's
//!   compile batch is bounded; the cover holds on [`WarmPass::satisfied`] until the cache drains.
//! - [`PipeWatch`]: counters shared by the main and render worlds, and whether a cover hides the
//!   frame.
//! - [`watch_pipelines`]: the tripwire, a `warn!` for every pipeline compiled uncovered, which
//!   means the menagerie has a coverage hole.
//! - `WOW_PIPE_TRACE=<path>`: one line per pipeline created, with its full variant identity.

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

/// The channel between the main and render worlds, aligned to within one frame.
#[derive(Resource, Clone)]
pub(crate) struct PipeWatch(pub(crate) Arc<PipeShared>);

pub(crate) struct PipeShared {
    /// Pipelines the cache has ever queued (its vec only grows; ids are indices).
    pub(crate) created: AtomicUsize,
    /// Of those, how many are `Ok` or a non-retryable `Err`; a retryable one reads as pending.
    pub(crate) settled: AtomicUsize,
    /// An opaque cover hides the frame: the loading screen, or not `InWorld`.
    pub(crate) covered: AtomicBool,
}

impl PipeWatch {
    /// Whether a pipeline is still building. Off macOS the build is async, and until it settles
    /// `SetItemPipeline` skips the batch, so a one-shot bake taken now would miss it.
    ///
    /// `settled` is read first: the pair is unsynchronised, and this order can only err towards a
    /// spurious `true` (one extra frame), never a spurious `false` (a wrong still).
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
    // Before Present, so the loading screen reads this frame's gate.
    app.add_systems(
        Update,
        run_warm_pass.before(benilla_world::schedule::WorldStage::Present),
    );
    // Both warm writers push into streams cleared at the top of their sets, so they run after.
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

/// Tell the render thread a compile burst is on, so it leaves the frame-critical QoS band.
fn publish_compile_burst(warm: Res<WarmPass>) {
    let bursting = warm.spawned_at.is_some() && !warm.done;
    benilla_world::thread_qos::COMPILE_BURST.store(bursting, Ordering::Relaxed);
}

/// Publish whether the frame is covered to the render world.
fn publish_cover(
    watch: Res<PipeWatch>,
    loading: Res<LoadingScreen>,
    state: Res<State<ClientState>>,
) {
    let covered = loading.covering() || *state.get() != ClientState::InWorld;
    watch.0.covered.store(covered, Ordering::Relaxed);
}

/// Count the cache's pipelines after it queues this frame's builds, and warn on any new one
/// compiled uncovered. `seen` is the previous frame's count.
fn watch_pipelines(
    cache: Res<PipelineCache>,
    watch: Res<PipeWatch>,
    mut seen: Local<usize>,
    mut settled_seen: Local<usize>,
) {
    let covered = watch.0.covered.load(Ordering::Relaxed);
    // Early-out when nothing is new and everything had settled; queued pipelines settle later
    // without `total` moving. `size_hint().0` is exact for the slice iterator behind it.
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
                // The tripwire: a live compile is a visible stall, and a warm pass coverage hole.
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

/// One greppable line of a pipeline's variant identity.
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

/// Marker on the menagerie's twin booth camera, for [`warm_effect_lane`]; it is also a
/// [`WarmRig`].
#[derive(Component)]
struct WarmBoothCam;

/// Warm-pass state; the loading screen holds its cover on [`Self::satisfied`].
#[derive(Resource, Default)]
pub(crate) struct WarmPass {
    /// `Time<Real>` when the menagerie spawned under this cover, `None` when idle. Real, because
    /// `Time<Virtual>` clamps its delta at 250 ms and would shrink the stalls being timed.
    spawned_at: Option<f32>,
    /// This cover's warm work is done (drained, timed out, or not applicable).
    done: bool,
    /// The 1x1 stand-in texture [`warm_effect_lane`] binds, held strong for the pass's life.
    effect_tex: Option<Handle<Image>>,
    /// The menagerie drained cleanly once. The warm set does not depend on the map and the
    /// pipeline cache never evicts, so later covers skip the pass; a timeout does not latch this.
    warmed_once: bool,
    /// The cameras the menagerie parents rigs to, whose live view keys [`record_warmed_views`]
    /// reads.
    anchors: Vec<Entity>,
    /// The view keys those anchors carried while the pass ran: the census's only notion of warm.
    warmed_views: Vec<ViewClass>,
    /// Pacing state: rigs warmed so far, when the last slice went out, frames the reveal spanned,
    /// and the slice on screen now (hidden again next frame).
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

/// How long after the last reveal `pending == 0` means drained: the counters cross worlds a frame
/// apart.
const WARM_SETTLE_SECS: f32 = 0.25;
/// Rigs revealed per frame. On macOS the pipeline cache drains its whole backlog in one frame,
/// inline, so this bounds each frame's compile batch to within one 43 ms audio device cycle.
/// `$WOW_WARM_SLICE` overrides it; 0 means unpaced.
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

/// Marker: this rig has had its one visible frame and is hidden again, which keeps the pass's
/// per-frame cost flat.
#[derive(Component)]
struct Warmed;

/// The pacing query: every rig but the twin booth camera, whose rigs need it visible.
type WarmRigVis<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Visibility, Has<Warmed>),
    (With<WarmRig>, Without<WarmBoothCam>),
>;

/// Never hold a cover unbounded: on timeout the pass warns and releases, and what remains
/// compiles live.
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
    if !cover.covering() {
        // No cover: a leftover menagerie despawns, and `done` keeps the gate open.
        warm.done = true;
        warm.spawned_at = None;
        warm.effect_tex = None;
        warm.showing.clear();
        despawn_rigs(&mut commands, &rigs);
        return;
    }
    // No menagerie in a capture.
    if benilla_world::dev_state::deterministic_run() {
        warm.done = true;
        return;
    }
    let now = time.elapsed_secs();
    // Already warmed in this process: nothing to compile.
    if warm.warmed_once {
        warm.done = true;
        return;
    }
    let Some(spawned) = warm.spawned_at else {
        // A new cover: close the gate, and spawn once the camera and shared light exist and the
        // cover is on screen ([`EntryCover`]), so the burst is hidden behind it.
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
        // The twin booth warms the custom-projection view key real bakes install; the real
        // booths warm the Perspective one.
        let warm_booth = crate::portrait::spawn_warm_booth(&mut commands, &mut lanes.images);
        commands
            .entity(warm_booth.0)
            .insert((WarmRig, WarmBoothCam));
        // The orthographic twin, the projection class the UI model tile atlas draws through.
        let warm_ortho = crate::ui_models::spawn_warm_tile_cam(&mut commands, &mut lanes.images);
        commands.entity(warm_ortho.0).insert(WarmRig);
        // The effect lane's stand-in texture, held for the life of the pass.
        warm.effect_tex = Some(lanes.images.add(Image::default()));
        // Every camera the menagerie hangs rigs on, for the census.
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
    // Gizmos are immediate-mode, so each frame draws one tiny line in the default config group,
    // the bowstring's, to compile the `LineGizmo` pipeline.
    gizmos.line(
        Vec3::new(0.0, 0.0, -0.5),
        Vec3::new(0.001, 0.0, -0.5),
        Color::WHITE,
    );
    // The pacing slice, in this order: hide last frame's slice (already extracted and drawn,
    // as this runs in `Update`, an extract behind), then reveal the next. A frame that reveals
    // nothing means every rig is warmed, and only then does `pending == 0` mean drained.
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

/// The view-class census: warn once per `Camera3d` view key that no rig rendered through, since
/// its whole model-pipeline space would compile live. It compares only against the recorded
/// [`WarmPass::warmed_views`], and runs all session because a booth installs its projection on
/// its first bake.
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
             it a warm arm. Warm keys this session: {}.",
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

/// Every 3-D camera, with its name and the three components that decide its view key.
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

/// The key a 3-D view contributes to `MeshPipelineKey`. `hdr` is a key bit and gates two more:
/// bevy keys `TONEMAP_IN_SHADER` and `DEBAND_DITHER` only on a non-HDR view
/// (`bevy_pbr-0.18.1` `render/mesh.rs:418`).
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
        // bevy_pbr keys these three (`bevy_pbr-0.18.1` `render/mesh.rs:397`); exhaustive so a
        // new variant stops the build. No `Projection` sets no bits, the same class as Custom.
        projection: match projection {
            Some(Projection::Perspective(_)) => "Perspective",
            Some(Projection::Orthographic(_)) => "Orthographic",
            Some(Projection::Custom(_)) | None => "Custom",
        },
        samples: msaa.copied().unwrap_or_default().samples(),
        hdr,
    }
}

/// The 3-D cameras by entity, for [`record_warmed_views`] to match against the anchors.
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

/// Record the live view key of every anchor camera while the pass runs.
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

/// Despawn the pass's root rigs only: `despawn` is recursive, and the twin booth camera is itself
/// a rig, so its children would be despawned twice.
fn despawn_rigs(commands: &mut Commands, rigs: &Query<(Entity, Option<&ChildOf>), With<WarmRig>>) {
    for (e, child_of) in rigs {
        if child_of.is_some_and(|c| rigs.contains(c.parent())) {
            continue;
        }
        commands.entity(e).despawn();
    }
}

/// One invisible HUD quad per warm frame, so its batch-mesh pipeline (POSITION + UV_0 + COLOR)
/// compiles under the cover. The minimap composite's `Rectangle` layout is a rig in [`menagerie`].
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

/// Push one degenerate draw per reachable [`EffectPipelineKey`] through the production stream,
/// per warm view: `wow_effect` is a specialized lane whose pipelines exist only when a draw is
/// queued, so no menagerie entity reaches it. Per frame, as the stream clears every frame.
///
/// [`EffectPipelineKey`]: benilla_world::particles::render::EffectPipelineKey
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
            // The rasterizer settle's constant and slope are both key axes, warmed as the pairs
            // that ship: every ground decal's, and the foam's.
            for (raster_bias, raster_slope) in [
                (0, 0.0),
                (benilla_world::sky_order::Rung::DECAL_RASTER, 0.0),
                (
                    benilla_world::sky_order::Rung::FOAM_RASTER,
                    benilla_world::sky_order::Rung::FOAM_RASTER_SLOPE,
                ),
            ] {
                // Lighting is a key axis (a shader def), so both arms are warmed; lit emitters
                // are rare.
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
        // Depth-test off, warmed as the one combination that ships: the weapon swing trail,
        // alpha-blended, unlit, no rasterizer settle.
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

    /// A warm set with no orthographic arm: the world camera's Perspective and the two
    /// `Msaa::Off` booth classes.
    fn warm_set_without_an_orthographic_arm() -> Vec<ViewClass> {
        vec![
            view_class(Some(&perspective()), Some(&Msaa::Off), true),
            view_class(Some(&perspective()), Some(&Msaa::Sample4), true),
            view_class(None, Some(&Msaa::Off), true),
        ]
    }

    /// The tile camera is `Msaa::Off` like every booth, so a census keyed on sample count alone
    /// would miss it.
    #[test]
    fn the_census_fires_for_an_unwarmed_orthographic_camera() {
        let warm = warm_set_without_an_orthographic_arm();
        let tile_cam = view_class(Some(&orthographic()), Some(&Msaa::Off), true);
        assert!(
            !warm.contains(&tile_cam),
            "the ui_models tile camera's view key must read as unwarmed against a warm set that \
             has no orthographic arm — the census must report it"
        );
        assert_eq!(
            tile_cam.samples, 1,
            "the tile camera is Msaa::Off, like every booth"
        );
    }

    #[test]
    fn the_census_is_silent_for_every_warmed_class() {
        let warm = warm_set_without_an_orthographic_arm();
        for class in &warm {
            assert!(warm.contains(class));
        }
        let booth_after_first_bake = view_class(None, Some(&Msaa::Off), true);
        assert!(
            warm.contains(&booth_after_first_bake),
            "a booth's runtime-installed custom projection is the NONSTANDARD class, warmed by \
             the twin booth"
        );
    }

    /// `Hdr` is a key bit and gates two more (`bevy_pbr` `mesh.rs:418`).
    #[test]
    fn dropping_hdr_is_a_different_view_key() {
        let with = view_class(Some(&perspective()), Some(&Msaa::Off), true);
        let without = view_class(Some(&perspective()), Some(&Msaa::Off), false);
        assert_ne!(with, without);
        assert!(!warm_set_without_an_orthographic_arm().contains(&without));
    }
}
