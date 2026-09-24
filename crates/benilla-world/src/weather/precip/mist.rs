//! The mist companion every precip type owns in the reference (ctor `0x67a5b0`, spawn
//! `0x67a990`, render `0x67ae20`): up to 128 fog-coloured 12×12 puffs streaming through the
//! camera's height band. The near haze of reference rain is not these puffs, invisible within
//! 6 yd, but the scene fog's storm lerp at `min(1, density·4)` (`update_time_lighting`).

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

use crate::weather::{WeatherKind, WeatherState};

use super::pool::{HeightCache, CELL};
use super::{rand01, wow_azimuth_to_bevy, WeatherWind};

/// Mist spawn-rate gain `Q` per kind and leg, in `2·max(density − 0.5, 0)·K·Q` nodes/s: rain
/// `0x80ff9c` = 18 / `0x80ffa0` = 38 (`0x6749e0`), snow `0x80732c` = 24 / `0x80ffd8` = 48
/// (`0x6776c0`). Every scalar on these paths is a per-kind pair.
const MIST_Q_RAIN: f32 = if super::SHADER_LEG { 38.0 } else { 18.0 };
const MIST_Q_SNOW: f32 = if super::SHADER_LEG { 48.0 } else { 24.0 };
/// Node capacity, a ctor argument.
pub(super) const MIST_CAP: usize = 128;
/// Half the 12×12 puff quad (a ctor argument).
const MIST_HALF: f32 = 6.0;
/// Puff floor bias, `z = max(ground, seeded z) + 6` (`0x67a990`): the max makes a volume.
const MIST_FLOOR: f32 = 6.0;
/// Placement box: ctor args {44, 44, 25} stored as-is, so the scatter is ±22 × ±22 × ±12.5.
const MIST_EXT_XY: f32 = 44.0;
const MIST_EXT_Z: f32 = 25.0;
/// Each node's polar motion basis in the wind-heading frame (`0x67a990` draws 1–3; rise
/// `0x807a40`, `0x807334`); it is born `1.5·dir` upstream of the camera (`0x80308c`).
const MIST_AZ_BASE: f32 = -1.57;
const MIST_AZ_SCALE: f32 = 0.349;
/// Per-type radius base and span (ctor arguments).
const MIST_R_RAIN: (f32, f32) = (5.0, 1.2);
const MIST_R_SNOW: (f32, f32) = (9.0, 3.0);
const MIST_DIR_Z_BASE: f32 = 0.333_333_34;
const MIST_DIR_Z_SCALE: f32 = 0.033_333_335;
const MIST_PLACE_SCALE: f32 = 1.5;
/// Per-corner alpha `linearstep(6, 18, corner→cam)·trapezoid`, uncapped: a mid-life puff past
/// 18 yd is fully opaque.
const MIST_ALPHA_NEAR: f32 = 6.0;
const MIST_ALPHA_FAR: f32 = 18.0;
/// Node life, draw 8: `round((2.7 ± 0.15)·1024)` ticks of the 1024 Hz weather clock (`0xc62970`).
const MIST_LIFE_BASE: f32 = 2.7;
const MIST_LIFE_SCALE: f32 = 0.3;
/// The fade trapezoid's ramp length in seconds, not an alpha cap.
const MIST_FADE_S: f32 = 0.4;
/// The tail draw (`0x67a990` draw 7), ±1.667: the isotropic `0.5·dt²·tail` term of the advance.
const MIST_TAIL_SCALE: f32 = 3.333_333_3;
/// Below its follow target a node's `tail` grows 5/3 per frame (`0x80655c`, `0x67b421`), a
/// frame-rate-dependent rise-assist in the reference too.
const MIST_TAIL_RISE: f32 = 5.0 / 3.0;
/// The spawn path lookahead (`0x67a7a0`): `min(64, round(planarSpeed·0.96/CELL))` heights, one
/// grid cell apart along the node's heading, about 1 s of travel (≈5 rain, ≈8 snow).
const MIST_LOOKAHEAD_SPEED_S: f32 = 0.96;
const MIST_LOOKAHEAD_MAX: usize = 64;
/// Slope validity (`0x67a8e1`–`0x67a933`): group `m` fails when `s[m+k] − s[m]` exceeds 0.5,
/// 0.75, 1.0 for k = 1, 2, 3; life is cut to `(valid + 1)/count` of itself, so puffs die at walls.
const MIST_SLOPE_1: f32 = 0.5;
const MIST_SLOPE_2: f32 = 0.75;
const MIST_SLOPE_3: f32 = 1.0;
/// The follow's upward per-frame z step cap (`0x67b433`); its exact scalar order is untraced.
const MIST_Z_STEP_CAP: f32 = 3.0;

/// One mist puff.
struct MistNode {
    pos: Vec3,
    /// The wind-yawed motion basis, `pos += dt·dir` per frame: `|dir|` yd/s at any frame rate.
    dir: Vec3,
    /// The isotropic `0.5·dt²·tail` term of the advance.
    tail: f32,
    age: f32,
    /// Seconds, cut short at spawn by the slope walk.
    life: f32,
    /// The ground baked per cell at spawn (`0x67a7a0`); the render reads no grid (`0x67ae20`).
    path: Vec<f32>,
    /// Horizontal speed in yd/s; the path index advances `speed·age/CELL`.
    planar_speed: f32,
}

/// The mist pool; it spawns only above the 0.5 density knee.
#[derive(Default)]
pub(super) struct Mist {
    nodes: Vec<MistNode>,
    budget: f32,
}

impl Mist {
    /// A type change retires the unborn nodes (`0x67b234`); live ones finish their lives.
    pub(super) fn cut(&mut self) {
        self.budget = 0.0;
    }
}

/// The first invalid group's index, which is how many groups before it were valid.
fn first_invalid_group(path: &[f32]) -> Option<usize> {
    let n = path.len();
    for m in 0..n {
        let jump = |k: usize, lim: f32| m + k < n && path[m + k] - path[m] > lim;
        if jump(1, MIST_SLOPE_1) || jump(2, MIST_SLOPE_2) || jump(3, MIST_SLOPE_3) {
            return Some(m);
        }
    }
    None
}

/// The spawn accumulator, `min(acc + dt·rate, MIST_CAP)` (`0x67b141`–`0x67b1aa`): the raw dt
/// (`0xc62510`) with the fraction kept (`0x67b172`, `0x67b261`), so the mist holds through a
/// hitch while the flakes thin. The ceiling, 128 at all three ctor call sites (`0x67a724`:
/// `0x674645`, `0x677448`, `0x679149`), is a spiral guard, never a budget cap.
fn accrue(budget: f32, rate: f32, dt: f32) -> f32 {
    (budget + rate * dt).min(MIST_CAP as f32)
}

/// The mist's frame: accrue, spawn one node per whole unit, stream, retire. The 0.5 knee sits on
/// the ramped effect density the drop kinematics read (`effect+0xd0`), not the raw wire grade.
pub(super) fn run_mist(
    mist: &mut Mist,
    weather: &WeatherState,
    wind: &WeatherWind,
    heights: &mut HeightCache,
    spatial: &SpatialQuery,
    filter: &SpatialQueryFilter,
    rng: &mut u32,
    dt: f32,
    cam_pos: Vec3,
) {
    // One grid per effect: cast from the drops' spawn plane, since from the camera an indoor ray
    // starts under the ceiling and fills the room, and two planes would thrash the height cache.
    let cast_plane = cam_pos.y
        + match weather.effect_kind {
            WeatherKind::Snow => super::SNOW_Z_OFF,
            _ => super::RAIN_Z_OFF,
        };
    let (q, (r_base, r_span)) = match weather.effect_kind {
        WeatherKind::Snow => (MIST_Q_SNOW, MIST_R_SNOW),
        _ => (MIST_Q_RAIN, MIST_R_RAIN),
    };
    // Sand takes the rain law here; the reference's sand mist has no 0.5 knee, no ×2 and a
    // 15/4.5 radius.
    let rate = if weather.effect_kind != WeatherKind::Fine {
        2.0 * (weather.effect_density - 0.5).max(0.0) * weather.density_gain() * q
    } else {
        0.0
    };
    mist.budget = accrue(mist.budget, rate, dt);
    let yaw = wind.mist_yaw();
    while mist.budget >= 1.0 && mist.nodes.len() < MIST_CAP {
        mist.budget -= 1.0;
        // A polar motion basis and a box scatter, both in the wind-heading frame (`0x67a990`).
        let az = MIST_AZ_BASE + (rand01(rng) - 0.5) * MIST_AZ_SCALE;
        let radius = r_base + (rand01(rng) - 0.5) * r_span;
        let rise = MIST_DIR_Z_BASE + (rand01(rng) - 0.5) * MIST_DIR_Z_SCALE;
        let dir = yaw * (wow_azimuth_to_bevy(az) * radius + Vec3::Y * rise);
        let place = yaw
            * Vec3::new(
                (rand01(rng) - 0.5) * MIST_EXT_XY,
                (rand01(rng) - 0.5) * MIST_EXT_Z,
                (rand01(rng) - 0.5) * MIST_EXT_XY,
            );
        let mut pos = cam_pos - dir * MIST_PLACE_SCALE + place;
        // Over a building the ground is the roof (the grid is a max from above): rooms never fill.
        let ground = heights.ground_y(pos.x, pos.z, cast_plane, spatial, filter);
        pos.y = MIST_FLOOR + pos.y.max(ground);
        // The lookahead: bake the path's ground, then cut life at the first slope jump (a wall).
        let planar = Vec3::new(dir.x, 0.0, dir.z);
        let planar_speed = planar.length();
        let n = ((planar_speed * MIST_LOOKAHEAD_SPEED_S / CELL).round() as usize)
            .min(MIST_LOOKAHEAD_MAX);
        let step = if n > 0 {
            planar / planar_speed * CELL
        } else {
            Vec3::ZERO
        };
        let mut path = Vec::with_capacity(n.max(1));
        path.push(ground);
        for m in 1..n {
            let at = pos + step * m as f32;
            path.push(heights.ground_y(at.x, at.z, cast_plane, spatial, filter));
        }
        let mut life = MIST_LIFE_BASE + (rand01(rng) - 0.5) * MIST_LIFE_SCALE;
        if let Some(valid) = first_invalid_group(&path) {
            if valid == 0 {
                continue; // stillborn (`0x67a96c`)
            }
            life *= (valid + 1) as f32 / path.len() as f32;
        }
        mist.nodes.push(MistNode {
            pos,
            dir,
            tail: (rand01(rng) - 0.5) * MIST_TAIL_SCALE,
            age: 0.0,
            life,
            path,
            planar_speed,
        });
    }
    for n in &mut mist.nodes {
        n.age += dt;
        // `pos += dt·dir + 0.5·dt²·tail`; the follow target lerps the baked path (+6), and below
        // it the tail grows and z steps up, capped per frame.
        n.pos += n.dir * dt + Vec3::splat(0.5 * dt * dt * n.tail);
        let idx = (n.planar_speed * n.age / CELL).max(0.0);
        let (lo, hi) = (idx.floor() as usize, idx.ceil() as usize);
        let last = n.path.len() - 1;
        let (a, b) = (n.path[lo.min(last)], n.path[hi.min(last)]);
        let target = a + (b - a) * idx.fract() + MIST_FLOOR;
        if n.pos.y < target {
            n.tail += MIST_TAIL_RISE;
            n.pos.y = (n.pos.y + MIST_Z_STEP_CAP).min(target);
        }
    }
    mist.nodes.retain(|n| n.age < n.life);
}

/// Mist puffs: 12×12 camera-facing quads in the current fog colour (so drawn fog-off), alpha per
/// corner (`0x67add0`, packed at `0x67ae20`) so a quad beside the camera shows its far corners.
pub(super) fn push_mist(
    out: &mut Vec<crate::particles::buffer::EffectVertex>,
    mist: &Mist,
    cam_right: Vec3,
    cam_up: Vec3,
    cam_pos: Vec3,
    fog_color: [f32; 3],
) {
    let r = cam_right * MIST_HALF;
    let u = cam_up * MIST_HALF;
    for node in mist.nodes.iter().take(MIST_CAP) {
        // A wall-bound puff's life was cut at spawn, so this same ramp fades it before the wall.
        let trapezoid = (node.age / MIST_FADE_S).clamp(0.0, 1.0)
            * ((node.life - node.age) / MIST_FADE_S).clamp(0.0, 1.0);
        let c = node.pos;
        // Perimeter order (bl, br, tr, tl).
        for (corner, uv) in [
            (c - r - u, [0.0, 1.0]),
            (c + r - u, [1.0, 1.0]),
            (c + r + u, [1.0, 0.0]),
            (c - r + u, [0.0, 0.0]),
        ] {
            let dist_a = ((corner.distance(cam_pos) - MIST_ALPHA_NEAR)
                / (MIST_ALPHA_FAR - MIST_ALPHA_NEAR))
                .clamp(0.0, 1.0);
            out.push(crate::particles::buffer::EffectVertex {
                pos: corner.to_array(),
                uv,
                color: [fog_color[0], fog_color[1], fog_color[2], dist_a * trapezoid],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader leg's 38 nodes/s against the ceiling of [`MIST_CAP`].
    #[test]
    fn a_hitch_keeps_its_mist_backlog_up_to_the_node_capacity() {
        let rate = 2.0 * (1.0 - 0.5) * 1.0 * MIST_Q_RAIN; // full density, weatherDensity 3 → 38/s
        let after = accrue(0.0, rate, 1.0);
        assert!(
            (after - rate).abs() < 1e-3,
            "a 1 s stall owes {rate} nodes, accumulator kept {after}"
        );
        assert_eq!(accrue(0.0, rate, 60.0), MIST_CAP as f32);
        assert!(
            accrue(0.0, rate, 3.0) < MIST_CAP as f32,
            "a 3 s stall must still be carried whole — the binary's ceiling is 3.4 s at this rate"
        );
        // The fraction carries across frames (`0x67b172`).
        let dt = 1.0 / 60.0;
        let (mut budget, mut spawned) = (0.0f32, 0u32);
        for _ in 0..60 {
            budget = accrue(budget, rate, dt);
            while budget >= 1.0 {
                budget -= 1.0;
                spawned += 1;
            }
        }
        assert_eq!(
            spawned, 38,
            "a second at 38 nodes/s must spawn 38, not 30-odd"
        );
    }

    /// The reason `REF_FPS_GAIN` scales the drop rate alone.
    #[test]
    fn mist_throughput_does_not_move_with_the_frame_rate() {
        let rate = 2.0 * (1.0 - 0.5) * 1.0 * MIST_Q_SNOW;
        let over_a_second = |fps: u32| {
            let (mut budget, mut spawned) = (0.0f32, 0u32);
            for _ in 0..fps {
                budget = accrue(budget, rate, 1.0 / fps as f32);
                while budget >= 1.0 {
                    budget -= 1.0;
                    spawned += 1;
                }
            }
            spawned
        };
        assert_eq!(over_a_second(60), 48);
        assert_eq!(over_a_second(30), 48, "half the frames, the same mist");
        assert_eq!(over_a_second(144), 48);
    }

    #[test]
    fn snow_and_rain_mist_gains_are_separate_constants() {
        assert!(
            (MIST_Q_RAIN - 38.0).abs() < 1e-6,
            "rain shader Q = 0x80ffa0"
        );
        assert!(
            (MIST_Q_SNOW - 48.0).abs() < 1e-6,
            "snow shader Q = 0x80ffd8"
        );
        // Only `Q` splits: the knee and the doubling are shared.
        let at = |q: f32, d: f32| 2.0 * (d - 0.5f32).max(0.0) * q;
        assert!((at(MIST_Q_SNOW, 1.0) / at(MIST_Q_RAIN, 1.0) - 48.0 / 38.0).abs() < 1e-6);
        assert_eq!(at(MIST_Q_SNOW, 0.5), 0.0, "the 0.5 knee is shared");
    }

    #[test]
    fn slope_walk_flags_the_wall() {
        assert_eq!(first_invalid_group(&[10.0, 10.2, 10.4, 10.6, 10.8]), None);
        // A wall's roof at sample 2 (+5 yd): m=0 sees s[2]−s[0] = 5 > 0.75 → stillborn.
        assert_eq!(
            first_invalid_group(&[10.0, 10.1, 15.0, 15.0, 15.0]),
            Some(0)
        );
        // The jump at sample 4 of 5: m=1 first sees it via k=3 (s[4]−s[1] > 1.0).
        assert_eq!(
            first_invalid_group(&[10.0, 10.1, 10.2, 10.3, 15.0]),
            Some(1)
        );
        // Gentle rises pass: a sustained slope must stay under ~0.33 yd per cell.
        assert_eq!(first_invalid_group(&[10.0, 10.3, 10.5, 10.7, 10.9]), None);
    }
}
