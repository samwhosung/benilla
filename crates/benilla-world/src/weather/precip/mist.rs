//! The companion mist — every precip type owns one in the reference (its own object: ctor
//! `0x67a5b0`, spawn `0x67a990`, render `0x67ae20`; rain/snow texture `SnowMist01.blp`, sand
//! `WeatherMistGrainy01.blp`).
//!
//! The mechanism: up to 128 puffs, spawned at `2·max(density−0.5, 0)·K·Q` nodes/s (zero below
//! the density knee), each born from draws in the **wind-heading frame** — a polar motion
//! basis `dir` (per-type radius, azimuth −1.57±0.175, a gentle rise) and a box scatter
//! (±22 × ±22 × ±12.5, ctor args stored verbatim) — placed `1.5·dir` upstream of the camera
//! with `z = max(ground, seeded z) + 6`. Nodes live **2.7 ± 0.15 s** (the 1024 Hz weather
//! clock), stream at `pos += dt·dir` (≈5 yd/s rain, ≈9 snow) plus a small isotropic
//! `0.5·dt²·tail` rise-assist, and fade over **0.4 s** ramps to **full opacity** — a churning
//! VOLUME of fog-coloured 12×12 quads through the camera's height band. (The *constant near
//! haze* of reference rain is NOT these puffs — they are invisible within 6 yd — it is the
//! weather→scene-fog coupling: the zone's clear↔storm LightParams fog lerped at
//! `bcc = min(1, density·4)`, which `lighting::update_time_lighting` already applies.)

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

use crate::weather::{WeatherKind, WeatherState};

use super::pool::{HeightCache, CELL};
use super::{rand01, wow_azimuth_to_bevy, WeatherWind};

/// Mist spawn-rate gain `Q` — per **kind** and per leg. The rate is `2·max(density − 0.5, 0)·K·Q`
/// nodes/s (each density param-set's step 5 → effect+0x3c = mist+0x20 accumulator).
///
/// **Snow carries its own pair and it is not rain's** (the snow param set `0x6776c0` reads
/// `0x80732c` = 24 / `0x80ffd8` = 48, against
/// the rain param set `0x6749e0`'s `0x80ff9c` = 18 / `0x80ffa0` = 38). Applying rain's 38 to
/// snow ran the snow mist at **0.79×** for as long as this constant was shared — the same class of
/// mistake as reading a shared byte offset and assuming a shared table. Every scalar on these two
/// paths is a *pair*; check the pair before reusing a number across kinds.
const MIST_Q_RAIN: f32 = if super::SHADER_LEG { 38.0 } else { 18.0 };
const MIST_Q_SNOW: f32 = if super::SHADER_LEG { 48.0 } else { 24.0 };
/// Node capacity (ctor arg; up to 128 camera-anchored puffs).
pub(super) const MIST_CAP: usize = 128;
/// Puff quad size: 12×12 world units (ctor arg) → half-extent 6.
const MIST_HALF: f32 = 6.0;
/// Puff floor bias: `z = max(grid_ground, seeded z) + 6.0` (`0x67a990`: the
/// `size·0.5` term over the fog-grid sample). The **max** is what makes the mist a volume
/// rather than a sheet.
const MIST_FLOOR: f32 = 6.0;
/// Placement box extents — ctor args {44, 44, 25} stored **verbatim** at mist+0x04/08/0c
/// (VERIFIED: plain `mov`, no doubling); the spawn scatters `((m−1)−0.5)·ext` per axis in the
/// wind-heading frame → **±22 × ±22 × ±12.5** around the cluster centre.
const MIST_EXT_XY: f32 = 44.0;
const MIST_EXT_Z: f32 = 25.0;
/// The polar motion basis each node carries (`0x67a990` draws 1–3, stored at node+0x114):
/// azimuth `−1.57 ± 0.5·0.349` in the wind-heading frame (+0x24/+0x28), a per-type radius
/// (below), rise `0.333 ± 0.5·0.0333` (`0x807a40`/`0x807334`). The node is born `1.5·dir`
/// upstream of the camera (`0x80308c` = 1.5) and streams along `dir`.
const MIST_AZ_BASE: f32 = -1.57;
const MIST_AZ_SCALE: f32 = 0.349;
/// Per-type radius base/span (mist+0x34/+0x38, ctor-verified): rain 5.0/1.2, snow 9.0/3.0.
/// (Sand is 15.0/4.5 and lands with the sand slice.)
const MIST_R_RAIN: (f32, f32) = (5.0, 1.2);
const MIST_R_SNOW: (f32, f32) = (9.0, 3.0);
const MIST_DIR_Z_BASE: f32 = 0.333_333_34;
const MIST_DIR_Z_SCALE: f32 = 0.033_333_335;
const MIST_PLACE_SCALE: f32 = 1.5;
/// Alpha (VERIFIED — there is **no 0.4 cap**): per corner,
/// `round(255 · linearstep(6, 18, corner→cam) · trapezoid)`. The distance term rises with
/// distance (puffs are invisible within 6 yd of the eye — the near haze is the scene fog, not
/// these); a mid-life puff at ≥ 18 yd is **fully opaque** fog colour.
const MIST_ALPHA_NEAR: f32 = 6.0;
const MIST_ALPHA_FAR: f32 = 18.0;
/// Node life (VERIFIED): draw 8 stamps `round((2.7 ± 0.15)·1024)` ticks of the **1024 Hz**
/// weather clock (`0xc62970`) — life uniform in [2.55, 2.85) s.
const MIST_LIFE_BASE: f32 = 2.7;
const MIST_LIFE_SCALE: f32 = 0.3;
/// The fade trapezoid (VERIFIED): `clamp(age/0.4) · clamp((life−age)/0.4)` — mist+0x30 = 0.4
/// is each ramp's DURATION in seconds (not an alpha cap); the plateau sits at 1.0.
const MIST_FADE_S: f32 = 0.4;
/// The node's tail draw (`0x67a990` draw 7): `((m−1)−0.5)·3.333` ∈ ±1.667 — the isotropic
/// `0.5·dt²·tail` term of the per-frame advance.
const MIST_TAIL_SCALE: f32 = 3.333_333_3;
/// While a node sits below its terrain-follow target, `tail += 5/3` per FRAME (`0x80655c`,
/// `0x67b421`) — a frame-rate-dependent rise-assist in the reference, transcribed as-is.
const MIST_TAIL_RISE: f32 = 5.0 / 3.0;
/// The spawn **path lookahead** (round 7, `0x67a7a0`): up to 64 heights sampled one grid cell
/// (`1.0417 = 0x80ff98`) apart along the node's horizontal heading; count =
/// `min(64, round(planarSpeed · 0.96 / 1.0417))` (≈5 rain, ≈8 snow — a ~1 s lookahead).
const MIST_LOOKAHEAD_SPEED_S: f32 = 0.96;
const MIST_LOOKAHEAD_MAX: usize = 64;
/// The slope-validity deltas over the baked samples (`0x67a8e1–0x67a933`): group `m` is invalid
/// when `s[m+1]−s[m] > 0.5` or `s[m+2]−s[m] > 0.75` or `s[m+3]−s[m] > 1.0` — a WMO wall's
/// roof-height jump trips it, so **puffs die at the wall and never enter the room**: the first
/// invalid group truncates life to `life·(valid+1)/count`; zero valid ⇒ STILLBORN (`0x67a96c`).
const MIST_SLOPE_1: f32 = 0.5;
const MIST_SLOPE_2: f32 = 0.75;
const MIST_SLOPE_3: f32 = 1.0;
/// The follow's upward-only per-frame z-step cap (`0x67b433–5a`; the exact scalar order is
/// unconfirmed).
const MIST_Z_STEP_CAP: f32 = 3.0;

/// One mist puff: position, its stored motion basis, and the lifecycle.
struct MistNode {
    pos: Vec3,
    /// The wind-yawed polar direction (`0x67a990` node+0x114): advanced `pos += dt·dir` per
    /// frame (VERIFIED — the drift scalar is the frame dt, ≈|dir| yd/s, framerate-independent).
    dir: Vec3,
    /// node+0x128: the isotropic `0.5·dt²·tail` term; grows 5/3 per frame while below the
    /// ground-follow target (the rise-assist).
    tail: f32,
    age: f32,
    /// Per-node life (2.7 ± 0.15 s on the 1024 Hz clock).
    life: f32,
    /// The spawn-baked height profile along the node's own path — one sample per grid cell
    /// (round 7, `0x67a7a0`; the render never queries the grid live — `0x67ae20` has ZERO grid
    /// reads, refuting an interim render-side hide). The follow target lerps into this array;
    /// the slope-validity walk at spawn already truncated life at the first jump, so a puff
    /// FADES OUT before reaching a wall instead of drifting into the room.
    path: Vec<f32>,
    /// Horizontal speed (yd/s) — the baked-array index advances `speed·age/1.0417`.
    planar_speed: f32,
}

/// The mist companion pool — every precip type carries one (the reference builds it inside
/// each effect ctor); it spawns only above the density 0.5 knee.
#[derive(Default)]
pub(super) struct Mist {
    nodes: Vec<MistNode>,
    budget: f32,
}

impl Mist {
    /// The stop-flag retire (`0x67b234`, round 7): a wire TYPE change also retires the
    /// SCHEDULED-but-unborn nodes — the spawn budget zeroes with the cut. Live nodes finish
    /// their (possibly truncated) lives normally.
    pub(super) fn cut(&mut self) {
        self.budget = 0.0;
    }
}

/// The slope-validity walk over the baked path (`0x67a8e1–0x67a933`): `Some(m)` = the first
/// invalid group's index (`m` = how many groups were valid before it), `None` = all valid.
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

/// The mist's spawn accumulator — `acc := min(acc + dt·rate, MIST_CAP)` (`0x67b141`–`0x67b1aa`).
///
/// **This is the one precip layer whose density survives a frame-rate drop**, and the reason is
/// this function. The drop kernels budget `rate · min(dt, 1/60)` and *discard* the remainder
/// (verified: the constant at `0x80ffcc` has exactly six sites, one `fcomp`/`fld` pair per effect,
/// and the count lands in a stack local that never persists) — so streaks and flakes thin in
/// proportion to `fps/60`. The mist instead multiplies the **raw, uncapped** dt (`0xc62510` at
/// `0x67b141`, absent from that six-site census), adds the carry, and stores it back
/// (`0x67b172`), spending exactly one whole node per free slot (`0x67b261`: `fsub 1.0`) and
/// keeping the fraction. A hitch therefore shifts the storm's **composition**, not just its
/// density — the mist holds while the flakes thin.
///
/// The ceiling is `[mist+0x48]`, the **size field of the node container** at `mist+0x44`, written
/// once from the ctor's 8th argument (`0x67a724`) which is `push 0x80` = **128 at all three call
/// sites** (`0x674645` rain, `0x677448` snow, `0x679149` sand). It is a spiral-of-death guard and
/// never a budget cap: the drain can only spend into a free slot and there are [`MIST_CAP`] of
/// them, so 128 is the *smallest* ceiling that discards nothing the array could have absorbed.
///
/// benilla carried a bare, uncited `8.0` here. At the shader leg's 38 nodes/s that saturates after
/// a **0.21 s** hitch (0.32 s at the CVar's default gain), against the binary's 3.4 s — quietly
/// re-introducing the very frame-rate coupling the accumulator exists to prevent, and *only* on
/// hitches. Decision 1159's follow-up.
fn accrue(budget: f32, rate: f32, dt: f32) -> f32 {
    (budget + rate * dt).min(MIST_CAP as f32)
}

/// The mist companion's frame: spawn-rate accumulator `budget += dt·2·max(density−0.5, 0)·K·Q`
/// (the arg is the RAMPED effect density — the same `effect+0xd0` value the drop kinematics
/// read, so the knee sits on the density, not the raw wire grade), one node per unit, cap 128;
/// then every node streams along its motion basis and retires at end-of-life.
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
    // The ground samples cast from the SAME spawn plane as the active kind's drops — the
    // reference has ONE fog grid per effect, and the grid must see ROOFS (drops die on them,
    // round 3). Casting from the camera's own height put the ray UNDER the ceiling with the
    // camera indoors: every sample saw the interior floor and the mist volume filled the room
    // at floor+6 (director-caught, 2026-07-13 — the forge's ceiling quads). Sharing the plane
    // also stops the height cache thrashing between the drop and mist passes (its clear
    // triggers on a >8 yd plane move).
    let cast_plane = cam_pos.y
        + match weather.effect_kind {
            WeatherKind::Snow => super::SNOW_Z_OFF,
            _ => super::RAIN_Z_OFF,
        };
    let (q, (r_base, r_span)) = match weather.effect_kind {
        WeatherKind::Snow => (MIST_Q_SNOW, MIST_R_SNOW),
        _ => (MIST_Q_RAIN, MIST_R_RAIN),
    };
    // Any active precip type drives its mist; sand's law differs (no 0.5 knee, no ×2) and
    // lands with the sand slice.
    let rate = if weather.effect_kind != WeatherKind::Fine {
        2.0 * (weather.effect_density - 0.5).max(0.0) * weather.density_gain() * q
    } else {
        0.0
    };
    mist.budget = accrue(mist.budget, rate, dt);
    let yaw = wind.mist_yaw();
    while mist.budget >= 1.0 && mist.nodes.len() < MIST_CAP {
        mist.budget -= 1.0;
        // The difftested spawn (`0x67a990`): a polar motion basis + a box scatter,
        // both in the wind-heading frame; the node is born 1.5·|dir| upstream of the camera.
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
        // `z = max(ground, seeded z) + 6` — a VOLUME from ground+6 up through the camera's
        // height band, not a ground sheet. Over a building "ground" is the ROOF (the grid is
        // a max-from-above), so indoor volumes never fill.
        let ground = heights.ground_y(pos.x, pos.z, cast_plane, spatial, filter);
        pos.y = MIST_FLOOR + pos.y.max(ground);
        // The spawn path lookahead (round 7): bake the height profile one cell at a time along
        // the horizontal heading, walk the slope-validity groups, truncate life at the first
        // jump — a wall's roof step kills the puff before it can enter the room.
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
        // The verified per-frame advance: `pos += dt·dir + 0.5·dt²·tail` (isotropic tail
        // term). The follow target is a LERP into the SPAWN-BAKED path profile (+6) — never a
        // live grid query (round 7, `0x67ae20` reads no grid); while below it the tail grows
        // 5/3 per frame and z steps up toward it, capped per frame.
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

/// Mist puffs: 12×12 camera-facing quads, RGB = the CURRENT fog colour (one flat tint), alpha
/// evaluated **per corner** — `linearstep(6, 18, corner→cam) · trapezoid(age)` at full range
/// (the reference computes the distance ramp per billboard corner at `0x67add0`, packed into the
/// vertex colour at `0x67ae20`; no peak cap) — so a quad hanging beside the camera still shows its
/// far corners. Alpha-blended, fog off (the tint already IS the fog colour). Pushed onto the
/// shared effect stream (0733), perimeter corner order for the quad-index pattern.
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
        // trapezoid = clamp(age/0.4) · clamp((life−age)/0.4) — 0.4 s ramps, plateau 1.0. The
        // spawn lookahead already truncated `life` at the first slope jump, so a wall-bound
        // puff rides this SAME ramp to zero before the wall — no separate hide.
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

    /// The mist accumulator carries a hitch's whole backlog, where the drop kernels would have
    /// thrown it away — the one place precip density is frame-rate-independent.
    ///
    /// The old uncited `8.0` ceiling clipped that at a fifth of a second of stall. Pinned in the
    /// units that matter: the shader leg's 38 nodes/s against the byte ceiling of [`MIST_CAP`].
    #[test]
    fn a_hitch_keeps_its_mist_backlog_up_to_the_node_capacity() {
        let rate = 2.0 * (1.0 - 0.5) * 1.0 * MIST_Q_RAIN; // full density, weatherDensity 3 → 38/s
                                                          // A 1 s stall: every node owed is still owed afterwards. The retired `8.0` kept 8 of 38.
        let after = accrue(0.0, rate, 1.0);
        assert!(
            (after - rate).abs() < 1e-3,
            "a 1 s stall owes {rate} nodes, accumulator kept {after}"
        );
        // The ceiling is the node capacity, and it is a spiral guard, not a budget cap: nothing
        // above `MIST_CAP` could ever be spent, because the drain needs a free slot per node.
        assert_eq!(accrue(0.0, rate, 60.0), MIST_CAP as f32);
        assert!(
            accrue(0.0, rate, 3.0) < MIST_CAP as f32,
            "a 3 s stall must still be carried whole — the binary's ceiling is 3.4 s at this rate"
        );
        // The fraction is kept across frames rather than truncated (`0x67b172` stores it back).
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

    /// The mist is the one precip rate the reference does **not** couple to the frame rate: its
    /// accumulator carries the sub-unit remainder across frames (`0x67b172`) where the drop
    /// kernels throw theirs away. So a second of mist is a second of mist at any frame rate —
    /// which is why 1165's `REF_FPS_GAIN` is applied to the flake rate alone. Halving this too,
    /// for symmetry with the flakes, would be a fresh bug wearing consistency's clothes.
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

    /// Rain and snow read **different** `Q` constants, and the pair is per leg. Pinned because
    /// benilla shared rain's for as long as the constant was a single value, which ran snow's
    /// mist at 38/48 = 0.79× — a defect no test could see while the number had nowhere to differ.
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
        // The knee and the doubling are shared; only Q splits, so the rates split in the same
        // ratio at every density above the 0.5 knee.
        let at = |q: f32, d: f32| 2.0 * (d - 0.5f32).max(0.0) * q;
        assert!((at(MIST_Q_SNOW, 1.0) / at(MIST_Q_RAIN, 1.0) - 48.0 / 38.0).abs() < 1e-6);
        assert_eq!(at(MIST_Q_SNOW, 0.5), 0.0, "the 0.5 knee is shared");
    }

    /// The slope-validity walk (`0x67a8e1–0x67a933`): flat terrain passes whole; a roof-height
    /// jump within 3 cells of spawn stillbirths (the k=3 delta trips at m=0); farther jumps
    /// truncate at the first group that sees them.
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
        // Gentle rises inside the deltas pass — the cumulative limits (0.5/0.75/1.0 over
        // 1/2/3 cells) mean sustained slopes must stay under ~0.33 yd per cell.
        assert_eq!(first_invalid_group(&[10.0, 10.3, 10.5, 10.7, 10.9]), None);
    }
}
