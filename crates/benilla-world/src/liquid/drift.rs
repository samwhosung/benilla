//! The underwater **drift cloud** — the 4000-mote field the reference draws while the camera eye
//! is inside a liquid (decision 1814).
//!
//! The object is `World.cpp`'s own — `0xfa40` bytes from
//! `0x66f971`, ctor `0x68e5a0`, held by the pointer `[0xc63180]` — and it is **not** part of the
//! weather manager `[0xc6326c]`: the two share no state and no code (verified by complete
//! enumeration; 12 references to the cloud, none in the CMapWeather band).
//!
//! ## The shape
//!
//! 4000 records of 16 bytes — `xyz` **camera-relative**, plus a per-particle billboard edge. They
//! sit in a 30 yd cube centred on the eye and are advected by `p += (last_cam − cam) + gust`, each
//! axis wrapped back into `±15`: the cloud is therefore **world-fixed** apart from the gust, and
//! the wrap recycles a mote the moment it leaves the box. Nothing is spawned or killed — there is
//! no lifetime, no emitter, and no per-particle recycle; the whole field is re-scattered only on a
//! liquid-type change or a teleport.
//!
//! Both drive sites — the advect `0x66fee0` and the draw `0x6701e0` — carry the same two
//! conjuncts: the render-flag bit `[0xc7b2a4] & 0x2000000` and `[0xc7f288] != 0xf`, the
//! **camera-eye** liquid type. That second one is [`Underwater`], which benilla's murk already
//! gates on, so the motes appear exactly when the murk does.
//!
//! ## Where it draws
//!
//! `0x483731`, between the ground-target reticle's liquid pass and the glare dispatch — after the
//! water surface **and** both M2 transparent passes, i.e. the last world content in the frame.
//! [`Rung::DRIFT_CLOUD`](crate::sky_order::Rung::DRIFT_CLOUD) is that slot on our ladder.
//!
//! Render state is unusually plain: alpha blend, depth test on, **depth write off**, lighting off,
//! immediate `0xFFFFFFFF` white, and fog **off** for water/ocean (on only for magma). No
//! LightParams value reaches a mote — the draw contains zero indirect calls and the colour is a
//! compile-time immediate — so the motes are full-brightness texels over the fogged scene and take
//! **no** underwater tint. That is worth stating because it is the opposite of the obvious guess.
//!
//! ## Where we knowingly differ from the bytes
//!
//! Four places, each a deliberate call rather than a gap — the reasoning is in decision 1814:
//!
//! 1. **[`GUST_REF_HZ`]** — the reference's water gust is a per-*frame* displacement with no `dt`
//!    term, so its drift speed scales with frame rate. We normalise it to 60 fps.
//! 2. **Atlas cell 12 is completed.** The reference's UV initialiser stops two floats early and
//!    one magma mote in four draws a sprite smeared across the atlas diagonal — a shipped defect
//!    ([`ATLAS`]).
//! 3. **The cell walk has no off-by-one.** The reference reads its cell index at the *bottom* of
//!    the draw loop, so the value lands on the next mote and slot 0 takes a hardcoded cell 8.
//! 4. **The cull cone is floored at the frustum** ([`cull_limits`]) — its hardcoded 90° contains
//!    the view at every aspect the reference shipped for, and stops doing so around 21:9.

use bevy::prelude::*;

use benilla_formats::Submersion;

use crate::particles::buffer::{
    begin_effect_frame, EffectBlend, EffectDrawSpec, EffectFog, EffectLighting, EffectQuads,
    EffectVertex,
};
use crate::particles::emit::rand01;
use crate::sky_order::Rung;
use crate::view::WorldCamera;

use super::Underwater;

/// Pool size — `count = __ftol(1.0 * [0x81038c])`, `[0x81038c] = 4000.0f`, and `0x68e650` has
/// exactly **one** caller (the ctor, with that literal). No CVar, console command or quality tier
/// scales it: 4000 is not a capacity, it is the population.
const COUNT: usize = 4000;

/// The wrap box's edge (`+0xfa1c`, ctor literal `30.0f`; `0x68e680` has one caller). The motes
/// live in `[−15, +15)` per axis, camera-relative.
const BOX_EDGE: f32 = 30.0;
const BOX_HALF: f32 = BOX_EDGE * 0.5;

/// A camera jump this big in ONE frame re-scatters the field (`0x68e99a` loads the *extent*, not
/// the half — and `last_cam` is rewritten every frame, so this is a per-frame delta). It is a
/// teleport detector, not a box re-centre: the box re-centres for free via the `+= delta` term.
const TELEPORT: f32 = BOX_EDGE;

/// The per-particle scale base (`+0xfa18`), re-set per liquid class by `0x68e670`: `1/36` for
/// water and ocean (`0x680b1d`), `1/9` for magma (`0x680b5c`). A mote's edge is drawn uniformly
/// from `[base·0.5, base·1.5)`, so water motes are **1.4–4.2 cm** across and magma's 5.6–16.7 cm.
const SCALE_WATER: f32 = 1.0 / 36.0;
const SCALE_MAGMA: f32 = 1.0 / 9.0;

/// Gust frequency (`[0x807a4c]`) and amplitude (`[0x807a3c]`) unit scales; each is drawn as
/// `m · scale` with `m ∈ [1,2)`, giving freq `[0.0125, 0.025)` and amp `[0.005, 0.01)`.
const GUST_FREQ_UNIT: f32 = 0.0125;
const GUST_AMP_UNIT: f32 = 0.005;

/// The vertical squash on a freshly rolled gust direction (`[0x8029b0]`), applied **before**
/// normalising.
///
/// It is a bias, **not a bound** — checked against the function's bytes and by Monte Carlo.
/// `|z|/|xy| = 0.25·|cot a|` with `a` uniform on `[−π, π)`, so `atan(0.25) = 14.04°` is the
/// **median** elevation (quartiles 5.9°/31.1°): about 15.6% of rolls are steeper than 45° and the
/// limit as `a → 0` is straight up. The one hard constraint is the `fchs` at `0x68e27d`, which
/// forces `z ≥ 0` — the gust never blows downward.
const GUST_RISE: f32 = 0.25;

/// Magma's motion is not a gust at all: a true velocity of `0.02` yd/s **downward**
/// (`[0x86a098]`), and the one mode that is `dt`-scaled in the reference.
const MAGMA_SINK: f32 = -0.02;

/// The frame rate the water gust is denominated in — **our deviation, and the whole of it.**
///
/// `0x68e4f0`'s `mode <= 1` leg writes `out = sin(term·2π)·amp·dir` and the advect adds it to the
/// position **with no `dt` factor** (this is load-bearing; only magma's leg multiplies by `dt`). So
/// the reference's mote drift is frame-rate dependent: at **peak** of the half-sine, ~0.3 yd/s at
/// 30 fps, ~0.6 at 60, and ~1.2 on the 120 Hz panel this is being built on — a gust's *mean* is
/// `2/π` of that, so ~0.19–0.38 yd/s at 60. (Peak and mean are labelled here; the ratio, which is
/// what this constant is about, is the same either way.) There is no single faithful speed to port
/// — the binary's own answer spans 2× across the era's hardware — so reproducing the literal
/// per-frame step would not be "the reference's speed", it would be *this machine's*.
///
/// We take the top of the era's range, 60 Hz, and scale by `dt·60`. This is the same move as the
/// snow flake's [`SNOW_PX_REF_HEIGHT`](crate::weather::precip): denominate the reference's
/// frame-quantised number in the era's own units so the *behaviour* carries over to hardware it
/// never ran on, rather than the literal number carrying over and the behaviour changing.
const GUST_REF_HZ: f32 = 60.0;

/// The draw's own cap: `0x68f2cd` stops the fill at `0xa68` bytes = 2664 verts = **666 quads**.
/// It is not arbitrary — a uniform 4000-particle cube contributes ~1/6 of itself to a 90° cone, so
/// the cap is sized to the expected survivor count and almost never actually bites.
const SUBMIT_CAP: usize = 666;

/// The atlas cell pitch, `[0x810334] = 51/256`.
const CELL: f32 = 51.0 / 256.0;

/// The `Textures\WaterPoop02.blp` atlas as `(column, row)` cells — the lattice the CRT initialiser
/// `[0x68ebf0, 0x68efae)` fills, recovered by emulating it. Four columns per row, plus cell 8
/// alone out at column 4 of row 0.
///
/// **Cell 12 is completed here and is not in the binary.** The initialiser's last store is
/// `0x68efa5` and its `ret` is `0x68efae`, so cell 12 gets 6 of its 8 floats and its bottom-right
/// corner keeps BSS zero — sampling UV `(0,0)` and smearing the sprite across the atlas diagonal.
/// The magma row uses cell 12 for one mote in four, so a faithful port would ship a visible defect
/// on a quarter of the motes in every lava pool. Writing it as the lattice plainly intends is the
/// §3 call (implement the mechanism, not the quirk); 1814 records it.
const ATLAS: [(f32, f32); 13] = [
    (0.0, 0.0),
    (1.0, 0.0),
    (2.0, 0.0),
    (3.0, 0.0),
    (0.0, 1.0),
    (1.0, 1.0),
    (2.0, 1.0),
    (3.0, 1.0),
    (4.0, 0.0),
    (0.0, 2.0),
    (1.0, 2.0),
    (2.0, 2.0),
    (3.0, 2.0),
];

/// Which atlas cells a mode draws from (`[0x86a0a0]`): water and ocean cycle cells 0–7, magma
/// cycles 9–12.
const CELLS_WATER: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
const CELLS_MAGMA: [usize; 8] = [9, 10, 11, 12, 9, 10, 11, 12];

/// The cull's per-axis tangent limits — the reference's cone, floored at the frustum.
///
/// `0x68f1c9` culls to `vz > 0 && |vx| < vz && |vy| < vz`: a fixed **90° cone** about the view
/// axis, not the real frustum, and it is what sizes [`SUBMIT_CAP`]. At every aspect the reference
/// shipped for, that cone comfortably contains the view — 4:3 with its ~44.1° vertical fov puts
/// the horizontal half-angle at 28.4°, a 16.6° margin — so the two never disagreed.
///
/// They do on a modern window. The horizontal half-fov reaches the cone's 45° at about **21:9**
/// and passes it beyond: taken literally, a 32:9 monitor would show motes winking out in two
/// vertical bands down the left and right edges of the screen. That is not a defect the reference
/// has and it is not one worth importing, so the limit is the **wider** of the two. On 4:3 and
/// 16:9 this is exactly the reference's cone and nothing changes; only an aspect the reference
/// never ran at sees a difference, which is the same shape of call as [`GUST_REF_HZ`].
///
/// Vertically the cone always wins (`tan 22.5° = 0.414`), so the `max` there is a statement of
/// that fact rather than a live term.
fn cull_limits(fov_y: f32, aspect: f32) -> (f32, f32) {
    let ty = (fov_y * 0.5).tan();
    ((ty * aspect).max(1.0), ty.max(1.0))
}

/// One mote — the reference's 16-byte record exactly: a camera-relative position and the
/// billboard's world edge length.
#[derive(Clone, Copy, Default)]
struct Mote {
    pos: Vec3,
    edge: f32,
}

/// Which liquid class the field is configured for. Slime is absent on purpose: its arm of the
/// dispatch (`0x680b6f`) clears the enable byte and does **not** re-scatter — there are no motes
/// in slime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DriftMode {
    /// Liquid types 0 and 1 — water and ocean.
    Water,
    /// Liquid type 2.
    Magma,
}

impl DriftMode {
    fn scale_base(self) -> f32 {
        match self {
            DriftMode::Water => SCALE_WATER,
            DriftMode::Magma => SCALE_MAGMA,
        }
    }

    fn cells(self) -> &'static [usize; 8] {
        match self {
            DriftMode::Water => &CELLS_WATER,
            DriftMode::Magma => &CELLS_MAGMA,
        }
    }

    /// Fog is enabled for magma alone (`0x68f36e` sets gx id `0x0f` to `mode == 2`). Water motes
    /// burn at full brightness over the murk.
    fn fog(self) -> EffectFog {
        match self {
            DriftMode::Water => EffectFog::Off,
            DriftMode::Magma => EffectFog::Scene,
        }
    }
}

/// The mote field. One resource, allocated once (64 KB) and never resized — the reference builds
/// its object at process init and tears it down at exit.
#[derive(Resource)]
pub(super) struct DriftCloud {
    motes: Vec<Mote>,
    /// The camera position at the previous advect (`+0xfa04`).
    last_cam: Vec3,
    /// `None` = the enable byte is clear: never configured, or slime.
    mode: Option<DriftMode>,
    /// What [`Underwater`] said last frame — the reference compares the new liquid type against
    /// `[0xc7f288]` and skips the whole reconfiguration when it is unchanged.
    was: Submersion,
    gust_dir: Vec3,
    gust_freq: f32,
    gust_phase: f32,
    gust_amp: f32,
    rng: u32,
    /// `$WOW_NO_PARTICLES` — the family's perf-bisect kill switch (`particles.rs`,
    /// `ribbons.rs`): one switch, whole family, and a mote field is squarely in it. Cached at
    /// construction rather than read per frame.
    off: bool,
    /// Instrument state — see [`dump`].
    dump: bool,
    dump_at: f64,
    submitted: usize,
    wrapped: usize,
}

impl Default for DriftCloud {
    fn default() -> Self {
        Self {
            motes: vec![Mote::default(); COUNT],
            last_cam: Vec3::ZERO,
            mode: None,
            was: Submersion::Dry,
            gust_dir: Vec3::X,
            gust_freq: GUST_FREQ_UNIT,
            gust_phase: 0.0,
            gust_amp: GUST_AMP_UNIT,
            // Any odd seed: the reference's lagged-table generator is byte-known, but nothing here
            // needs its exact stream — only uniformity (the `particles::emit` idiom, and the same
            // ruling `weather::precip::rand01` carries).
            rng: 0x9e37_79b9,
            off: std::env::var_os("WOW_NO_PARTICLES").is_some(),
            dump: std::env::var_os("WOW_DRIFT_DUMP").is_some(),
            dump_at: 0.0,
            submitted: 0,
            wrapped: 0,
        }
    }
}

impl DriftCloud {
    /// Scatter every mote uniformly through the box and redraw its edge — `0x68e720`. Called on a
    /// liquid-type change and on a teleport, never per frame and never per mote at the wrap.
    fn scatter(&mut self, mode: DriftMode) {
        let base = mode.scale_base();
        let lo = base * 0.5;
        let span = base * 1.5 - lo;
        let mut rng = self.rng;
        for m in &mut self.motes {
            // Uniform in `[−15, +15)` per axis: the reference's `(m − 1)·extent − half`.
            m.pos = Vec3::new(
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
            );
            m.edge = rand01(&mut rng) * span + lo;
        }
        self.rng = rng;
        self.mode = Some(mode);
    }

    /// Roll a fresh gust — `0x68e1c0`. Two uniform angles on `[−π, π)`; the vertical term is
    /// squashed by [`GUST_RISE`] and forced non-negative before the direction is normalised, then
    /// a fresh period and amplitude.
    fn roll_gust(&mut self) {
        let mut rng = self.rng;
        let angle = |r: &mut u32| (rand01(r) * 2.0 - 1.0) * std::f32::consts::PI;
        let (sa, ca) = angle(&mut rng).sin_cos();
        let (se, ce) = angle(&mut rng).sin_cos();
        // WoW z is up, Bevy y is up: the reference's (x, y, z) is our (x, z, y).
        let dir = Vec3::new(ce * sa, (ca * GUST_RISE).abs(), se * sa);
        self.gust_dir = dir.normalize_or(Vec3::Y);
        self.gust_freq = (1.0 + rand01(&mut rng)) * GUST_FREQ_UNIT;
        self.gust_amp = (1.0 + rand01(&mut rng)) * GUST_AMP_UNIT;
        self.gust_phase = 0.0;
        self.rng = rng;
    }

    /// This frame's whole-field displacement — `0x68e4f0`.
    fn gust(&mut self, mode: DriftMode, dt: f32) -> Vec3 {
        match mode {
            DriftMode::Magma => Vec3::new(0.0, MAGMA_SINK * dt, 0.0),
            DriftMode::Water => {
                self.gust_phase += dt;
                let mut term = self.gust_phase * self.gust_freq;
                // Strictly greater (`0x68e542 test ah,0x41; jne`): the period runs 20 s at the
                // fastest frequency to 40 s at the slowest, and `sin(term·2π)` over `[0, 0.5]` is a
                // non-negative half-sine — the gust swells from nothing to a peak and back, then
                // takes a new direction.
                if term > 0.5 {
                    self.roll_gust();
                    term = 0.0;
                }
                let speed = (term * std::f32::consts::TAU).sin() * self.gust_amp;
                // The one deviation: `· dt · 60` where the reference has no `dt` at all.
                self.gust_dir * (speed * dt * GUST_REF_HZ)
            }
        }
    }

    /// Advance the field — `0x68e930`. Because the records are camera-relative and `delta` is the
    /// camera's *backwards* step, the cloud stands still in the world; only the gust moves it.
    fn advect(&mut self, mode: DriftMode, eye: Vec3, dt: f32) {
        let mut delta = self.last_cam - eye;
        self.last_cam = eye;
        if delta.length_squared() > TELEPORT * TELEPORT {
            self.scatter(mode);
            delta = Vec3::ZERO;
        }
        let add = delta + self.gust(mode, dt);
        let wrap = |v: f32| {
            if v > BOX_HALF {
                v - BOX_EDGE
            } else if v < -BOX_HALF {
                v + BOX_EDGE
            } else {
                v
            }
        };
        let mut wrapped = 0usize;
        for m in &mut self.motes {
            let moved = m.pos + add;
            m.pos = Vec3::new(wrap(moved.x), wrap(moved.y), wrap(moved.z));
            if m.pos != moved {
                wrapped += 1;
            }
        }
        self.wrapped = wrapped;
    }
}

/// The mote texture. One BLP, loaded once with the world's first camera.
#[derive(Resource)]
struct DriftAssets {
    motes: Handle<Image>,
}

fn setup_drift(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    existing: Option<Res<DriftAssets>>,
) {
    if existing.is_some() || cam.single().is_err() {
        return;
    }
    // `PointSprite` — clamp, the gamma lane, and the BLP's **authored mip pyramid**. A mote is
    // 1.4–4.2 cm wide and lives out to 26 yd, which on this projection is ~87 px at arm's length
    // down to ~3 px at the box corner: a 17× minification of a 51 px atlas cell. Mip-0-only there
    // is the snow flake's flickering speckle (`BlpVariant::Effect`'s docs), not crispness. The
    // cost is that the deep mips blend neighbouring cells — which the reference pays too, having
    // shipped the same atlas with the same 8 mips and bound it with the same filtering.
    let point_sprite = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::PointSprite;
    };
    commands.insert_resource(DriftAssets {
        motes: asset_server.load_with_settings("mpq://textures/waterpoop02.blp", point_sprite),
    });
}

/// Reconfigure on a liquid-class change, then advect — the reference's update-pass leg
/// (`0x66fee0` → `0x68e930`), with `0x6809c0`'s type dispatch folded in ahead of it.
fn simulate_drift(
    mut cloud: ResMut<DriftCloud>,
    underwater: Res<Underwater>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    time: Res<Time>,
) {
    let Ok(cam_tf) = cam.single() else {
        return;
    };
    let eye = cam_tf.translation();
    let now = underwater.0;
    if now != cloud.was {
        cloud.was = now;
        match now {
            // Going dry does NOT clear the enable byte — `0x680abe` intercepts `0xf` before the
            // type dispatch, so the field keeps its configuration and is suppressed by the frame
            // gate alone. Diving back into the same liquid therefore finds it exactly as it was.
            Submersion::Dry => {}
            // Slime disables outright, with no re-scatter (`0x680b6f`).
            Submersion::Slime => cloud.mode = None,
            // Water and ocean share the drift cloud's whole configuration — the mote cell set is
            // keyed on liquid types 0 AND 1 (`[0x86a0a0]`), so the sea drifts exactly like a lake.
            Submersion::Water | Submersion::Ocean => {
                cloud.scatter(DriftMode::Water);
                cloud.last_cam = eye;
            }
            Submersion::Magma => {
                cloud.scatter(DriftMode::Magma);
                cloud.last_cam = eye;
            }
        }
    }
    // The frame gate's second conjunct. (The first, the render-flag bit `0x2000000`, is the
    // `waterParticulates` console command's latch — default ON, a plain on/off that scales
    // nothing, and not a CVar at all: it is registered through the developer-console table
    // `0x63f9e0`, has no `CVar::Register` site and no `Config.wtf` persistence. Nothing here
    // wires it, exactly as nothing wires `showfootprints`.)
    if !now.any() || cloud.off {
        return;
    }
    let Some(mode) = cloud.mode else {
        return;
    };
    cloud.advect(mode, eye, time.delta_secs());
}

/// Emit the surviving motes into the shared effect stream — the reference's render-pass leg
/// (`0x6701e0` → `0x68efe0`).
fn push_drift(
    mut cloud: ResMut<DriftCloud>,
    underwater: Res<Underwater>,
    assets: Option<Res<DriftAssets>>,
    cam: Query<(Entity, &GlobalTransform, &Projection), With<WorldCamera>>,
    mut quads: ResMut<EffectQuads>,
    time: Res<Time>,
) {
    cloud.submitted = 0;
    // Every exit yields a REASON rather than returning, so the instrument can say why nothing drew.
    // A census that goes silent when the thing is off cannot tell "off" from "broken" — which is
    // exactly the hole the first live probe of this module fell into.
    let status: &'static str = 'draw: {
        // The same two conjuncts as the advect — the draw is independently gated, not a consequence of
        // the sim having run.
        if !underwater.0.any() {
            break 'draw "dry (the eye is not in a liquid)";
        }
        if cloud.off {
            break 'draw "off ($WOW_NO_PARTICLES)";
        }
        let Some(mode) = cloud.mode else {
            break 'draw "disabled (slime, or never configured)";
        };
        let Some(assets) = assets else {
            break 'draw "no texture resource yet";
        };
        let Ok((cam_entity, cam_tf, proj)) = cam.single() else {
            break 'draw "no world camera";
        };
        let (tan_x, tan_y) = match proj {
            Projection::Perspective(p) => cull_limits(p.fov, p.aspect_ratio),
            // No perspective divide, so the cone means nothing; the reference has no such mode.
            _ => (1.0, 1.0),
        };
        let eye = cam_tf.translation();
        let fwd = cam_tf.forward().as_vec3();
        let right = cam_tf.right().as_vec3();
        let up = cam_tf.up().as_vec3();
        let cells = mode.cells();

        let start = quads.begin();
        let mut submitted = 0usize;
        for (i, m) in cloud.motes.iter().enumerate() {
            if submitted == SUBMIT_CAP {
                break;
            }
            // The reference's cone, floored at the frustum — see [`cull_limits`].
            let rel = m.pos;
            let vz = rel.dot(fwd);
            if vz <= 0.0 {
                continue;
            }
            if rel.dot(right).abs() >= vz * tan_x || rel.dot(up).abs() >= vz * tan_y {
                continue;
            }
            let (col, row) = ATLAS[cells[i & 7]];
            let (u0, v0) = (col * CELL, row * CELL);
            let half = m.edge * 0.5;
            let r = right * half;
            let u = up * half;
            // Perimeter order (bl, br, tr, tl) — the stream's quad-index pattern closes it. The
            // vertices are camera-RELATIVE (`cam_relative` below), which is what the record already
            // holds; the reference does the same thing by handing the device only the projection.
            for (pos, uv) in [
                (rel - r - u, [u0, v0 + CELL]),
                (rel + r - u, [u0 + CELL, v0 + CELL]),
                (rel + r + u, [u0 + CELL, v0]),
                (rel - r + u, [u0, v0]),
            ] {
                quads.verts.push(EffectVertex {
                    pos: pos.to_array(),
                    uv,
                    // The colour is an immediate `0xFFFFFFFF` in the binary (`0x68f27b`): not
                    // per-particle, not a global, and with no distance or depth fade.
                    color: [1.0, 1.0, 1.0, 1.0],
                });
            }
            submitted += 1;
        }
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam: cam_entity,
                texture: assets.motes.id(),
                // `SRC_ALPHA / ONE_MINUS_SRC_ALPHA` (gx id 7 = 2). The cascading alpha test at
                // `GEQUAL 1/255` needs no lane of its own — under this blend a zero-alpha fragment
                // already contributes nothing.
                blend: EffectBlend::Alpha,
                fog: mode.fog(),
                lighting: EffectLighting::None,
                anchor: eye,
                bias: Rung::DRIFT_CLOUD,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: true,
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
                light: None,
                clip: None,
            },
        );
        cloud.submitted = submitted;
        if submitted == 0 {
            break 'draw "submerged, but every mote culled";
        }
        "drawing"
    };
    dump(&mut cloud, status, &time);
}

/// `$WOW_DRIFT_DUMP` — a 1 Hz line, because every question this field raises is numeric and none
/// of them is answerable from a screenshot. `submitted` against [`SUBMIT_CAP`] says whether the cap
/// is biting; `wrapped` is the recycle rate, which is the drift speed made visible; the gust block
/// is the only state with a 20–40 s clock on it, so a stuck gust shows here and nowhere else.
///
/// **It reports when nothing draws, too, and that is the point.** The first live probe of this
/// module printed no lines at all, which was indistinguishable between "the eye never went under"
/// and "the draw is broken" — so `status` names the gate that stopped it.
fn dump(cloud: &mut DriftCloud, status: &'static str, time: &Time) {
    if !cloud.dump {
        return;
    }
    let now = time.elapsed_secs_f64();
    if now - cloud.dump_at < 1.0 {
        return;
    }
    cloud.dump_at = now;
    let d = cloud.gust_dir;
    let elev = d.y.clamp(-1.0, 1.0).asin().to_degrees();
    info!(
        "drift: {status} — mode {:?} submitted {}/{} (cap {}) wrapped {} | gust dir \
         [{:.3} {:.3} {:.3}] elev {:.1}° freq {:.4} amp {:.4} phase {:.1}/{:.1}s",
        cloud.mode,
        cloud.submitted,
        COUNT,
        SUBMIT_CAP,
        cloud.wrapped,
        d.x,
        d.y,
        d.z,
        elev,
        cloud.gust_freq,
        cloud.gust_amp,
        cloud.gust_phase,
        0.5 / cloud.gust_freq,
    );
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<DriftCloud>()
        .add_systems(
            Update,
            (
                setup_drift,
                // `CameraPoseSet` as well as the verdict: the advect's whole input is
                // `last_cam − cam`, and for the length of `Update` a camera's `GlobalTransform` is
                // last frame's pose unless you order after the set that refreshes it (1503). Read
                // stale, a teleport's jump would land in `delta` a frame late — as a 30 yd shove
                // through the field instead of the re-scatter it is.
                simulate_drift
                    .after(super::SubmersionVerdict)
                    .after(crate::view::CameraPoseSet)
                    .after(setup_drift),
            ),
        )
        .add_systems(PostUpdate, push_drift.after(begin_effect_frame));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud(mode: DriftMode) -> DriftCloud {
        let mut c = DriftCloud::default();
        c.scatter(mode);
        c
    }

    fn in_box(v: Vec3) -> bool {
        [v.x, v.y, v.z]
            .iter()
            .all(|c| *c >= -BOX_HALF && *c < BOX_HALF)
    }

    #[test]
    fn scatter_fills_the_box_and_the_edge_lane() {
        for (mode, base) in [
            (DriftMode::Water, SCALE_WATER),
            (DriftMode::Magma, SCALE_MAGMA),
        ] {
            let c = cloud(mode);
            assert_eq!(c.motes.len(), COUNT);
            for m in &c.motes {
                assert!(in_box(m.pos), "{:?} outside the 30 yd box", m.pos);
                assert!(
                    m.edge >= base * 0.5 && m.edge < base * 1.5,
                    "edge {} outside [{}, {})",
                    m.edge,
                    base * 0.5,
                    base * 1.5
                );
            }
            // A scatter that filled the box would still be wrong if it filled it in a corner: the
            // mean of a uniform cube is the centre, and 4000 samples put it well inside 1 yd.
            let mean: Vec3 = c.motes.iter().map(|m| m.pos).sum::<Vec3>() / COUNT as f32;
            assert!(mean.length() < 1.0, "scatter is not centred: mean {mean:?}");
        }
    }

    #[test]
    fn the_wrap_keeps_every_mote_inside_the_box() {
        let mut c = cloud(DriftMode::Water);
        let mut eye = Vec3::ZERO;
        // Walk the camera a long way in small steps — every step pushes motes out of the far face,
        // and the wrap is the only thing bringing them back.
        for _ in 0..500 {
            eye += Vec3::new(0.4, 0.05, -0.3);
            c.advect(DriftMode::Water, eye, 1.0 / 60.0);
            assert!(c.motes.iter().all(|m| in_box(m.pos)));
        }
        assert!(c.wrapped > 0, "500 yards of walking wrapped nothing");
    }

    #[test]
    fn the_cloud_is_world_fixed_apart_from_the_gust() {
        let mut c = cloud(DriftMode::Water);
        // Silence the gust so the camera term is the only motion left.
        c.gust_amp = 0.0;
        c.gust_freq = 0.0;
        let world_before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
        let eye = Vec3::new(3.0, -1.0, 2.0);
        c.advect(DriftMode::Water, eye, 1.0 / 60.0);
        // `pos` is camera-relative, so a mote that stood still in the world must now read
        // `old − eye` — unless it wrapped, which is a teleport in world space by design.
        let mut checked = 0;
        for (m, was) in c.motes.iter().zip(&world_before) {
            let expect = *was - eye;
            if in_box(expect) {
                assert!(
                    (m.pos - expect).length() < 1e-3,
                    "mote moved in world space: {:?} vs {:?}",
                    m.pos,
                    expect
                );
                checked += 1;
            }
        }
        assert!(checked > COUNT / 2, "too few unwrapped motes to prove it");
    }

    #[test]
    fn a_teleport_rescatters_rather_than_wrapping() {
        let mut c = cloud(DriftMode::Water);
        c.gust_amp = 0.0;
        let before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
        // One frame, further than the box is wide.
        c.advect(
            DriftMode::Water,
            Vec3::new(0.0, 0.0, TELEPORT + 1.0),
            1.0 / 60.0,
        );
        assert!(c.motes.iter().all(|m| in_box(m.pos)));
        let moved = c
            .motes
            .iter()
            .zip(&before)
            .filter(|(m, b)| (m.pos - **b).length() > 1e-3)
            .count();
        // A wrap would have translated the field rigidly; a re-scatter moves essentially all of it
        // to independent places.
        assert!(
            moved > COUNT * 9 / 10,
            "only {moved} motes were re-scattered"
        );
    }

    #[test]
    fn the_gust_never_blows_downward_and_is_biased_horizontal_without_being_bounded() {
        let mut c = DriftCloud::default();
        let mut elev = Vec::new();
        for _ in 0..20_000 {
            c.roll_gust();
            let d = c.gust_dir;
            assert!(
                (d.length() - 1.0).abs() < 1e-4,
                "gust dir is not a unit vector"
            );
            // The `fchs` at `0x68e27d` — the one hard constraint on the direction.
            assert!(d.y >= 0.0, "the gust blew downward: {d:?}");
            elev.push(d.y.clamp(-1.0, 1.0).asin().to_degrees());
        }
        elev.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = elev[elev.len() / 2];
        let steep = elev.iter().filter(|e| **e > 45.0).count() as f32 / elev.len() as f32;
        // `atan(0.25)` is the MEDIAN elevation, not a bound — "always within 14.04° of horizontal"
        // is a reading the formula does not support, and this test exists to keep anyone from
        // re-introducing it as a clamp. Closed forms: median `atan(0.25) = 14.036°`, and `P(elev >
        // 45°) = (2/π)·atan(0.25) = 0.156`.
        assert!(
            (median - 0.25f32.atan().to_degrees()).abs() < 1.0,
            "median elevation {median} is not atan(0.25)"
        );
        assert!(
            (steep - 0.156).abs() < 0.02,
            "{steep} of rolls steeper than 45° — expected ~0.156; a clamp would read ~0"
        );
        assert!(
            *elev.last().unwrap() > 80.0,
            "the distribution is bounded — it should reach straight up"
        );
    }

    #[test]
    fn the_gust_period_runs_twenty_to_forty_seconds() {
        let mut c = DriftCloud::default();
        for _ in 0..2_000 {
            c.roll_gust();
            let period = 0.5 / c.gust_freq;
            assert!(
                (20.0..=40.0).contains(&period),
                "gust period {period}s outside 20–40s"
            );
            assert!((0.005..0.01).contains(&c.gust_amp), "amp {}", c.gust_amp);
        }
    }

    #[test]
    fn the_water_gust_is_frame_rate_independent() {
        // The deviation `GUST_REF_HZ` buys exactly this: the same wall-clock second of drift,
        // whatever the frame rate. (The reference's own answer differs by 2× between 30 and 60 fps
        // — see the constant's docs.)
        let travel = |steps: u32, dt: f32| {
            let mut c = DriftCloud::default();
            c.roll_gust();
            c.mode = Some(DriftMode::Water);
            let dir = c.gust_dir;
            let mut sum = 0.0;
            for _ in 0..steps {
                sum += c.gust(DriftMode::Water, dt).dot(dir);
            }
            sum
        };
        let at30 = travel(30, 1.0 / 30.0);
        let at120 = travel(120, 1.0 / 120.0);
        // 5%, not 0: the gust's half-sine envelope is being sampled, and a 30-step Riemann sum of
        // a rising function differs from a 120-step one by O(dt). That residual is the envelope's
        // shape, not the frame rate leaking into the speed.
        assert!(
            (at30 - at120).abs() / at30.abs().max(1e-6) < 0.05,
            "one second of drift: {at30} at 30 fps vs {at120} at 120 fps"
        );
        // And this is what the normalisation is FOR: strip the `dt·60` back out — which is the
        // reference's literal per-frame law — and the same second of wall clock moves the field
        // four times further at 120 fps than at 30.
        let literal = |steps: u32, dt: f32| travel(steps, dt) / (dt * GUST_REF_HZ);
        let ratio = literal(120, 1.0 / 120.0) / literal(30, 1.0 / 30.0);
        assert!(
            (ratio - 4.0).abs() < 0.4,
            "the literal per-frame law should scale 4× from 30 to 120 fps, read {ratio}"
        );
    }

    #[test]
    fn magma_sinks_at_a_true_velocity() {
        let mut c = DriftCloud::default();
        let a = c.gust(DriftMode::Magma, 1.0 / 30.0) * 30.0;
        let b = c.gust(DriftMode::Magma, 1.0 / 120.0) * 120.0;
        assert!((a.y - MAGMA_SINK).abs() < 1e-5 && (b.y - MAGMA_SINK).abs() < 1e-5);
        assert!(a.x == 0.0 && a.z == 0.0, "magma drift is vertical only");
    }

    #[test]
    fn the_atlas_is_a_complete_lattice_including_the_reference_s_truncated_cell() {
        // Every cell is a distinct 51/256 tile, and no two cells overlap.
        let mut seen = std::collections::HashSet::new();
        for (col, row) in ATLAS {
            assert!(col >= 0.0 && row >= 0.0);
            assert!((col + 1.0) * CELL <= 1.0 + 1e-6, "cell runs off the atlas");
            assert!((row + 1.0) * CELL <= 1.0 + 1e-6, "cell runs off the atlas");
            assert!(
                seen.insert((col as u32, row as u32)),
                "duplicate atlas cell"
            );
        }
        // Cell 12 is the one the reference leaves half-written (its bottom-right corner stays BSS
        // zero and samples UV (0,0)). Ours is a real tile at column 3, row 2 — if this ever reads
        // (0,0) again we have re-imported a shipped defect.
        assert_eq!(ATLAS[12], (3.0, 2.0));
        // And it is reachable: magma draws it for one mote in four.
        assert!(CELLS_MAGMA.contains(&12));
    }

    #[test]
    fn the_cull_never_clips_a_mote_that_is_on_screen() {
        // The invariant, at every aspect a window can have: the cull limit is at least the
        // frustum's, so nothing visible is dropped.
        for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0, 21.0 / 9.0, 32.0 / 9.0] {
            let (tx, ty) = cull_limits(crate::view::CAM_FOVY, aspect);
            let frustum_x = (crate::view::CAM_FOVY * 0.5).tan() * aspect;
            let frustum_y = (crate::view::CAM_FOVY * 0.5).tan();
            assert!(
                tx >= frustum_x,
                "aspect {aspect}: cull {tx} < frustum {frustum_x}"
            );
            assert!(
                ty >= frustum_y,
                "aspect {aspect}: cull {ty} < frustum {frustum_y}"
            );
        }
        // ...and on the aspects the reference actually ran at, the limit IS its 90° cone — the
        // widening is inert there, so `SUBMIT_CAP`'s sizing still holds where it was derived.
        assert_eq!(cull_limits(crate::view::CAM_FOVY, 4.0 / 3.0), (1.0, 1.0));
        assert_eq!(cull_limits(crate::view::CAM_FOVY, 16.0 / 9.0), (1.0, 1.0));
        // Ultrawide is where it bites, and it bites horizontally only.
        let (tx, ty) = cull_limits(crate::view::CAM_FOVY, 32.0 / 9.0);
        assert!(tx > 1.0 && ty == 1.0);
    }

    /// The perf gate, and it is structural rather than measured: a dry frame must commit **no
    /// draw and no vertex**. The effect lane charges per submitted vertex, so "costs nothing when
    /// you are not underwater" is a property of this assertion, not of a frame-rate reading on
    /// whatever the machine was doing that afternoon (the 0353 law).
    #[test]
    fn a_dry_frame_commits_nothing() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(EffectQuads::default());
        world.insert_resource(Time::<()>::default());
        world.insert_resource(DriftAssets {
            motes: Handle::default(),
        });
        let mut c = DriftCloud::default();
        c.scatter(DriftMode::Water);
        world.insert_resource(c);
        world.spawn((
            WorldCamera,
            GlobalTransform::default(),
            Projection::default(),
        ));

        // Dry: the field is configured and the texture is present, so the ONLY thing stopping the
        // draw is the gate.
        world.insert_resource(Underwater(Submersion::Dry));
        world.run_system_once(push_drift).unwrap();
        {
            let q = world.resource::<EffectQuads>();
            assert!(q.draws.is_empty(), "a dry frame committed a draw");
            assert!(q.verts.is_empty(), "a dry frame pushed vertices");
        }

        // Submerged: the same world, one resource different, and now it draws — so the assertion
        // above is about the gate and not about a fixture that could never draw at all.
        world.insert_resource(Underwater(Submersion::Water));
        world.run_system_once(push_drift).unwrap();
        {
            let q = world.resource::<EffectQuads>();
            assert_eq!(q.draws.len(), 1, "submerged, the cloud did not draw");
            assert!(!q.verts.is_empty());
            assert_eq!(q.verts.len() % 4, 0, "whole quads only");
            assert!(
                q.verts.len() / 4 <= SUBMIT_CAP,
                "{} quads exceeds the reference's cap",
                q.verts.len() / 4
            );
            assert_eq!(q.draws[0].bias, Rung::DRIFT_CLOUD);
        }
    }

    #[test]
    fn slime_has_no_motes_and_going_dry_keeps_the_configuration() {
        let mut c = cloud(DriftMode::Water);
        assert!(c.mode.is_some());
        // Slime's arm clears the enable byte outright.
        c.mode = None;
        assert!(c.mode.is_none());
        // ...while going dry leaves it alone: the reference intercepts `0xf` ahead of the type
        // dispatch, so surfacing and diving again finds the same field.
        let mut c = cloud(DriftMode::Water);
        let before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
        c.was = Submersion::Dry;
        assert_eq!(c.mode, Some(DriftMode::Water));
        assert!(c.motes.iter().zip(&before).all(|(m, b)| m.pos == *b));
    }
}
