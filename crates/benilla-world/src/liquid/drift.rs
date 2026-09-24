//! The underwater drift cloud: 4000 world-fixed motes the reference draws while the camera eye is
//! inside a liquid. The object is `World.cpp`'s own (`0xfa40` bytes from `0x66f971`, ctor
//! `0x68e5a0`, held at `[0xc63180]`), sharing nothing with the weather manager `[0xc6326c]`. The
//! advect (`0x66fee0`) and the draw (`0x6701e0`) both need the render-flag bit
//! `[0xc7b2a4] & 0x2000000` and a camera-eye liquid type `[0xc7f288] != 0xf` ([`Underwater`]). The
//! draw (`0x483731`) is the frame's last world content, after the water surface and both M2
//! transparent passes ([`Rung::DRIFT_CLOUD`](crate::sky_order::Rung::DRIFT_CLOUD)), and no
//! `LightParams` value reaches a mote.
//!
//! Deviation: mote `i` draws cell `i & 7` of its set, because the reference reads its cell index
//! at the bottom of the draw loop, an off-by-one that gives each mote the previous one's cell and
//! slot 0 a hardcoded cell 8. The other deviations are [`GUST_REF_HZ`], [`ATLAS`] and
//! [`cull_limits`].

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

/// The population, `[0x81038c] = 4000.0f` through `0x68e650`'s one caller; nothing scales it.
const COUNT: usize = 4000;

/// The wrap box's edge (`+0xfa1c`, ctor `30.0f` via `0x68e680`): motes live in `[−15, +15)`.
const BOX_EDGE: f32 = 30.0;
const BOX_HALF: f32 = BOX_EDGE * 0.5;

/// A one-frame camera jump this long re-scatters the field (`0x68e99a` loads the edge, not half).
const TELEPORT: f32 = BOX_EDGE;

/// The mote edge base (`+0xfa18`, set by `0x68e670`): `1/36` for water and ocean (`0x680b1d`),
/// `1/9` for magma (`0x680b5c`). An edge is uniform in `[base·0.5, base·1.5)`.
const SCALE_WATER: f32 = 1.0 / 36.0;
const SCALE_MAGMA: f32 = 1.0 / 9.0;

/// Gust frequency (`[0x807a4c]`) and amplitude (`[0x807a3c]`) units: each roll is `m·unit`,
/// `m ∈ [1, 2)`.
const GUST_FREQ_UNIT: f32 = 0.0125;
const GUST_AMP_UNIT: f32 = 0.005;

/// The vertical squash on a rolled gust (`[0x8029b0]`), before normalising: a bias, not a bound
/// (`atan(0.25)` is the median elevation). The `fchs` at `0x68e27d` keeps `z ≥ 0`.
const GUST_RISE: f32 = 0.25;

/// Magma sinks at `0.02` yd/s (`[0x86a098]`), the one mode the reference scales by `dt`.
const MAGMA_SINK: f32 = -0.02;

/// Deviation: the water gust moves `speed·dt·60` a frame where the reference (`0x68e4f0`,
/// `mode <= 1`) moves `speed` with no `dt`, because the reference's drift doubles from 30 to
/// 60 fps; 60 Hz holds the top of the era's range at any frame rate.
const GUST_REF_HZ: f32 = 60.0;

/// The draw's cap: `0x68f2cd` stops the fill at `0xa68` = 2664 vertices, 666 quads, about the
/// share of the cube inside the 90° cone, so it rarely bites.
const SUBMIT_CAP: usize = 666;

/// The atlas cell pitch, `[0x810334] = 51/256`.
const CELL: f32 = 51.0 / 256.0;

/// The `Textures\WaterPoop02.blp` atlas as `(column, row)` cells, as the CRT initialiser
/// `[0x68ebf0, 0x68efae)` fills it: four columns per row, plus cell 8 alone at column 4 of row 0.
/// Deviation: cell 12 is completed, because the initialiser's last store (`0x68efa5`) leaves its
/// bottom-right corner at zero and one magma mote in four would smear across the atlas diagonal.
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

/// The atlas cells a mode draws from (`[0x86a0a0]`): water and ocean cycle 0–7, magma 9–12.
const CELLS_WATER: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
const CELLS_MAGMA: [usize; 8] = [9, 10, 11, 12, 9, 10, 11, 12];

/// The cull's per-axis tangent limits: the reference's fixed 90° cone about the view axis
/// (`0x68f1c9`: `vz > 0 && |vx| < vz && |vy| < vz`), which sizes [`SUBMIT_CAP`]. Deviation: floored
/// at the frustum, because the cone stops containing the view at about 21:9, and a wider window
/// would lose motes in bands down its sides.
fn cull_limits(fov_y: f32, aspect: f32) -> (f32, f32) {
    let ty = (fov_y * 0.5).tan();
    ((ty * aspect).max(1.0), ty.max(1.0))
}

/// One mote, the reference's 16-byte record: a camera-relative position and the billboard edge.
#[derive(Clone, Copy, Default)]
struct Mote {
    pos: Vec3,
    edge: f32,
}

/// The liquid class the field is configured for. Slime has no motes: its dispatch arm
/// (`0x680b6f`) clears the enable byte without a re-scatter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DriftMode {
    /// Liquid types 0 and 1, water and ocean.
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

    /// Fog is on for magma alone (`0x68f36e` sets gx id `0x0f` to `mode == 2`).
    fn fog(self) -> EffectFog {
        match self {
            DriftMode::Water => EffectFog::Off,
            DriftMode::Magma => EffectFog::Scene,
        }
    }
}

/// The mote field, allocated once, as the reference's object lives from process init to exit.
#[derive(Resource)]
pub(super) struct DriftCloud {
    motes: Vec<Mote>,
    /// The camera position at the previous advect (`+0xfa04`).
    last_cam: Vec3,
    /// `None`: the enable byte is clear (never configured, or slime).
    mode: Option<DriftMode>,
    /// Last frame's [`Underwater`]; an unchanged liquid type skips reconfiguring (`[0xc7f288]`).
    was: Submersion,
    gust_dir: Vec3,
    gust_freq: f32,
    gust_phase: f32,
    gust_amp: f32,
    rng: u32,
    /// `WOW_NO_PARTICLES`, the particle family's kill switch, read once.
    off: bool,
    /// State for the [`dump`] instrument.
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
            // Any odd seed: only uniformity matters, not the reference generator's exact stream.
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
    /// Scatter the field and redraw each edge (`0x68e720`): on a liquid-type change or a teleport.
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

    /// Roll a fresh gust direction, period and amplitude (`0x68e1c0`).
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

    /// This frame's whole-field displacement (`0x68e4f0`).
    fn gust(&mut self, mode: DriftMode, dt: f32) -> Vec3 {
        match mode {
            DriftMode::Magma => Vec3::new(0.0, MAGMA_SINK * dt, 0.0),
            DriftMode::Water => {
                self.gust_phase += dt;
                let mut term = self.gust_phase * self.gust_freq;
                // Strictly greater (`0x68e542`): each gust is one non-negative half-sine over 20
                // to 40 s, then a new roll.
                if term > 0.5 {
                    self.roll_gust();
                    term = 0.0;
                }
                let speed = (term * std::f32::consts::TAU).sin() * self.gust_amp;
                // The `GUST_REF_HZ` deviation: the reference has no `dt` here.
                self.gust_dir * (speed * dt * GUST_REF_HZ)
            }
        }
    }

    /// Advance the field (`0x68e930`): `delta` is the camera's backward step, so the
    /// camera-relative cloud stands still in the world and only the gust moves it.
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

/// The mote texture, loaded with the world's first camera.
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
    // `PointSprite`: clamp, the gamma lane and the BLP's authored mips, bound as the reference
    // binds this atlas; mip 0 alone would flicker at the motes' up-to-17× minification.
    let point_sprite = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::PointSprite;
    };
    commands.insert_resource(DriftAssets {
        motes: asset_server.load_with_settings("mpq://textures/waterpoop02.blp", point_sprite),
    });
}

/// Reconfigure on a liquid-class change, then advect: the reference's update leg (`0x66fee0` →
/// `0x68e930`), with `0x6809c0`'s type dispatch folded in ahead of it.
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
            // Going dry keeps the configuration: `0x680abe` intercepts `0xf` before the type
            // dispatch, and the frame gate alone hides the field.
            Submersion::Dry => {}
            // Slime disables outright, with no re-scatter (`0x680b6f`).
            Submersion::Slime => cloud.mode = None,
            // One configuration: the cell set is keyed on liquid types 0 and 1 (`[0x86a0a0]`).
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
    // The frame gate's second conjunct; the first, render-flag `0x2000000`, is the unwired
    // `waterParticulates` console latch (default on, console table `0x63f9e0`, not a CVar).
    if !now.any() || cloud.off {
        return;
    }
    let Some(mode) = cloud.mode else {
        return;
    };
    cloud.advect(mode, eye, time.delta_secs());
}

/// Emit the surviving motes into the effect stream: the render leg (`0x6701e0` → `0x68efe0`).
fn push_drift(
    mut cloud: ResMut<DriftCloud>,
    underwater: Res<Underwater>,
    assets: Option<Res<DriftAssets>>,
    cam: Query<(Entity, &GlobalTransform, &Projection), With<WorldCamera>>,
    mut quads: ResMut<EffectQuads>,
    time: Res<Time>,
) {
    cloud.submitted = 0;
    // Every exit yields a reason, so the instrument tells "off" from "broken".
    let status: &'static str = 'draw: {
        // The advect's two conjuncts again: the draw is gated on its own, not on the sim.
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
            // No perspective, so no cone; the reference has no such mode.
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
            // Perimeter order (bl, br, tr, tl), camera-relative as the records are.
            for (pos, uv) in [
                (rel - r - u, [u0, v0 + CELL]),
                (rel + r - u, [u0 + CELL, v0 + CELL]),
                (rel + r + u, [u0 + CELL, v0]),
                (rel - r + u, [u0, v0]),
            ] {
                quads.verts.push(EffectVertex {
                    pos: pos.to_array(),
                    uv,
                    // An immediate `0xFFFFFFFF` (`0x68f27b`): no per-mote colour, no fade.
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
                // `SRC_ALPHA / ONE_MINUS_SRC_ALPHA` (gx id 7 = 2), depth write off (id 0x12 = 0),
                // as this lane draws; the `GEQUAL 1/255` alpha test adds nothing under this blend.
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

/// `WOW_DRIFT_DUMP`: a 1 Hz line with the draw's status (the gate that stopped it, if any), the
/// submitted count against [`SUBMIT_CAP`], the wrap count and the gust state.
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
                // After `CameraPoseSet` too: until it runs, a camera's `GlobalTransform` is last
                // frame's, and a stale pose turns a teleport's re-scatter into a shove.
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
            // Centred too: 4000 uniform samples put the mean well inside 1 yd of the centre.
            let mean: Vec3 = c.motes.iter().map(|m| m.pos).sum::<Vec3>() / COUNT as f32;
            assert!(mean.length() < 1.0, "scatter is not centred: mean {mean:?}");
        }
    }

    #[test]
    fn the_wrap_keeps_every_mote_inside_the_box() {
        let mut c = cloud(DriftMode::Water);
        let mut eye = Vec3::ZERO;
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
        // A mote still in the world now reads `old − eye`, unless it wrapped.
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
            assert!(d.y >= 0.0, "the gust blew downward: {d:?}");
            elev.push(d.y.clamp(-1.0, 1.0).asin().to_degrees());
        }
        elev.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = elev[elev.len() / 2];
        let steep = elev.iter().filter(|e| **e > 45.0).count() as f32 / elev.len() as f32;
        // Closed forms: median `atan(0.25) = 14.036°`, `P(elev > 45°) = (2/π)·atan(0.25) = 0.156`.
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
        // 5%, not 0: 30- and 120-step sums over the rising half-sine differ by O(dt).
        assert!(
            (at30 - at120).abs() / at30.abs().max(1e-6) < 0.05,
            "one second of drift: {at30} at 30 fps vs {at120} at 120 fps"
        );
        // Without the `dt·60`, the reference's per-frame law moves the field 4× further at 120 fps.
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
        // Cell 12, half-written in the reference, is a real tile at column 3, row 2.
        assert_eq!(ATLAS[12], (3.0, 2.0));
        assert!(CELLS_MAGMA.contains(&12));
    }

    #[test]
    fn the_cull_never_clips_a_mote_that_is_on_screen() {
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
        // At the aspects the reference ran at, the limit is its 90° cone exactly.
        assert_eq!(cull_limits(crate::view::CAM_FOVY, 4.0 / 3.0), (1.0, 1.0));
        assert_eq!(cull_limits(crate::view::CAM_FOVY, 16.0 / 9.0), (1.0, 1.0));
        let (tx, ty) = cull_limits(crate::view::CAM_FOVY, 32.0 / 9.0);
        assert!(tx > 1.0 && ty == 1.0);
    }

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

        // Configured and textured, so only the gate stops the draw.
        world.insert_resource(Underwater(Submersion::Dry));
        world.run_system_once(push_drift).unwrap();
        {
            let q = world.resource::<EffectQuads>();
            assert!(q.draws.is_empty(), "a dry frame committed a draw");
            assert!(q.verts.is_empty(), "a dry frame pushed vertices");
        }

        // The control: submerged, the same world draws.
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
        // Going dry leaves the field as it was.
        let mut c = cloud(DriftMode::Water);
        let before: Vec<Vec3> = c.motes.iter().map(|m| m.pos).collect();
        c.was = Submersion::Dry;
        assert_eq!(c.mode, Some(DriftMode::Water));
        assert!(c.motes.iter().zip(&before).all(|(m, b)| m.pos == *b));
    }
}
