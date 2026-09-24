//! Precipitation — the pooled rain/snow simulation, its ground layers, and the mist companion.
//! Spawn kinematics and render laws are byte-exact transcriptions of the reference's own.
//! Implemented idiomatically (f32 math, Bevy space) — the *laws* (constants, rates, gates, blend
//! states) are the reference's, the x87 quirks are not.
//!
//! The verified structure:
//! - **Placement** is one law for both kinds ([`pool::spawn_particle`], `0x677965`–`0x677a86`):
//!   `pos = R·(O − T·V) + 1.75·W + C`. The scatter `O` lies on the
//!   plane the particle *arrives* on; `−T·V` back-projects it up its own velocity; **`R` leans the
//!   whole slab into the direction of travel** by `65°·sat(speed/18)`; then the wind lead and the
//!   live camera eye. `R` was missing until decision 1159 and its absence IS B233's second half —
//!   without it a particle needs its entire fall to reach eye height, which a running player
//!   outpaces (snow at grade 0.6: 7.8 s of fall against 54 yd of running, versus a 45 yd box).
//! - **Rain**: drops scatter across 130×130 yd, `T = −37.5/Vz` (`0x674df6–e53`) — the old "spawns
//!   AT eye height" read killed only the box's z-RANDOM, not the constant lift, and made density
//!   camera-angle-dependent. Each drop dies at the **terrain** under its spawn column and leaves
//!   a 0.25 s *patter* splash **1:1** (calm wind only). Streaks draw as fixed-size comet-tail
//!   TRIANGLES (0.1 wide × 2.0 long) with
//!   **no vertex colour** — the look is `RainDrop01.blp` (authored grey-128-neutral) under
//!   **Mod2x** (`2·src·dst`) and a **forced grey fog** over 70..75 yd that IS the distance fade.
//! - **Snow**: same rails, 90×90 box, slab lift +30, slow wandering kinematics — but a
//!   completely different DRAW. The leg that runs is the ARB **point sprite** `0x678610`
//!   (`glDrawArrays(GL_POINTS)`, `snowpoint.bls`), so a flake is sized in **pixels**,
//!   `max(1, 14·clamp01(1 − 0.02·d))` — denominated in the ERA's screen height so the angle, not
//!   the pixel count, carries to a modern display ([`SNOW_PX_REF_HEIGHT`]) — with
//!   alpha `clamp01(t − f1)` falling (a 1 s fade-in) and
//!   `clamp01(1 − 4·(t − f2))` settled. Blend mode 2 (SrcAlpha, 1−SrcAlpha), fog OFF,
//!   depth-write off, RGB white. The `1/12` world-space triangle of `0x678960` is the
//!   fixed-function FALLBACK and never runs on real hardware — benilla drew it (as a *quad*,
//!   times an invented size jitter) while taking the shader leg's population, which is B233's
//!   "flakes seem too big".
//! - **Mist**: every precip type carries a companion mist — its own object in the reference
//!   (ctor `0x67a5b0`, spawn `0x67a990`, render `0x67ae20`); the law lives in [`mist`].
//! - Ground heights come from a **lazy height cache** of the reference's weather ground
//!   oracle (`0x67c760 → mgr+0x34 → 0x6b7070`): **WMO/doodad-AWARE**, not terrain-only — after
//!   the terrain sample it probes the chunk's static
//!   object refs and MAXes the hit (`CMapObj::IntersectSegment 0x6a37b0`, `0x6b7237–4a`), so
//!   drops **land on roofs**: splashes on the inn roof, never inside, from any camera.
//!   Independently, "no weather indoors" is the global weather-visible flag (`[0xca80c4]`) —
//!   it kills the DRAW alone (`0x677380`/`0x6790b0`/`0x67a520`; simulation always runs); force-set
//!   with the camera outdoors (`0x6811d4`), and set from inside whenever an
//!   exterior group survives the view-clipped portal-window pass (`0x6b42d9`) — rain shows
//!   through a doorway the camera can SEE. See [`gate_weather_indoors`].
//! - **Drift heading is WORLD-FIXED**: both spawn kernels centre the per-drop horizontal drift
//!   on the constant azimuth −1.57 rad (`0x80ffbc`, R3/R3a) with a grade-scaled spread. The
//!   **wind** — the local PLAYER's trailing-149-ms average velocity (`0x67c150`, a true yd/s
//!   vector; the camera orbiting does not stir it) — enters ONLY through the spawn-box lead
//!   (×1.75) and the streak APEX tilt (`lerp(0°, 45°, sat(|wind|/30))`, verified DOWNWIND) —
//!   never the drift heading. (R10's spawn-plane tilt keys on the zone-ambient wind, which
//!   benilla doesn't model yet; the patter gate keys on ridden-transport velocity — no gate on
//!   foot.)
//!
//! - **The packet pipeline** (rf-weather-emission-timeline, rounds 2–3): drops are RECORDED
//!   into an open packet, which renders NOTHING until it seals (shader legs draw only the
//!   close-baked buffer) and its `baseTime = open + 6144/rate` stamp passes — a delay line
//!   that hides an upswing's sparse early drizzle (first visible rain stochastic, mostly
//!   ~8.5–14 s, AFTER the fog and mist) and lets committed rain persist through a same-type
//!   clear-down. A wire TYPE change instead cuts: emission stops at once and the unreplayed
//!   pipeline is discarded (`pool::Pool::cut`). A packet is also **retired outright** once its
//!   open-time camera anchor falls further than [`RETIRE_DIST`] from the live eye
//!   (`pool::Pool::retire_far`) — the teleport guard, which is why a `.go` no longer strands the
//!   field at the position you left. See `pool` and [`SHADER_LEG`].
//!
//! Geometry is pushed per frame from the live pools onto the shared effect stream:
//! rain's Mod2x blend + forced fog are the lane's `EffectBlend::Mod2x` + `EffectFog::Rain`
//! variants; snow and mist ride the Alpha/fog-off rows. Idle pools push nothing — the
//! structural replacement for the old fixed-capacity meshes' write gate (the 0353 fps hunt).

use bevy::prelude::*;

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};

use crate::lighting::WowLighting;
use crate::particles::buffer::{
    begin_effect_frame, EffectBlend, EffectDrawSpec, EffectFog, EffectQuads,
};
use crate::view::WorldCamera;
use crate::wmo_portal::{CameraInteriorClaim, WmoPvsSet};

use super::{WeatherKind, WeatherState, WeatherTick};

mod census;
mod mist;
mod pool;
mod render;
mod wind;
use census::{census, profile};
use mist::{push_mist, run_mist, Mist};
use pool::{run_kind, HeightCache, Pool};
use render::{push_flakes, push_patters, push_streaks, FlakeView};
pub(crate) use wind::{wow_azimuth_to_bevy, WeatherWind};

// ===== The reference's constants (byte-cited) =====

/// Which weather **leg** we run — the reference picks per the `useWeatherShaders` CVar
/// (default "1", registered `0x67b81d`) AND a rain.bls/patter.bls ARB validation (`0x58b360`);
/// the verdict lands in `effect+0`. **SHADER — settled in round 3** (2272, retracting 0332's
/// fixed-func inference): the shader leg's ~10 s onset is real — close-only packet visibility
/// (`0x6752b0` bakes at flush alone), the packet-1 forecast orphan, and the RNE parity stick
/// (`0x6754cc`) stack into a stochastic onset (~75% in the 8.5–14 s band, ~25% at ~5 s), and
/// only the shader populations (rain 35000, patters CAP-saturated) reproduce the director's
/// observed density. LIVE-CAPTURE flag: repeated reference upswings should OCCASIONALLY show
/// a ~5 s onset; an always-10 s reference means an unfound parity bias.
const SHADER_LEG: bool = true;
/// Per-packet record capacity — `0x1800` = 6144 (`0x80ffc8` as float; the open stamp divides it
/// by the live rate). NOT a live cap: packets form a growing list (`effect+0x8c`, TSList at
/// `effect+0x70`), so the steady population is demand-bounded (`rate · fall_time`).
const PACKET_CAP: usize = 0x1800;
/// **The anchor cull** — retirement condition 3 of the active-list walk (`0x677ff0`, tested
/// `0x6780d3`–`0x678110` against `[0x810008]` = 200.0): a packet whose OPEN-time camera anchor is
/// now further than this from the live eye is **discarded outright** — it leaves the render list
/// the same frame and is not allowed to finish replaying.
///
/// The anchor (`pkt+0x3001c`) is written once, in the allocation block (`0x678598`), from that
/// update's camera snapshot `0xc7cf20`, and its only readers image-wide are the three `fsub`s of
/// this test; the comparison is **3-D**, not planar (`0x4549f0` sums all three squares,
/// `0x678103 fsqrt`).
///
/// **This is a teleport / zone-change guard, not a motion mechanism**, and the arithmetic says so:
/// a packet lives at most `close_age + fall_time` (snow ≈ 2.3 + 7.5 ≈ 10 s), so even an epic mount
/// at ~14 yd/s carries the eye only ~140 yd from a live anchor. Nothing but a discontinuity trips
/// it. See [`RETIRE_DROP_SLACK`] for how it reaches benilla's already-activated drops.
const RETIRE_DIST: f32 = 200.0;
/// Slack added to [`RETIRE_DIST`] for the **active-drop** half of the cull.
///
/// benilla splits what the reference keeps together: a flake lives in its packet's static buffer
/// for the packet's whole life, so retiring the packet un-draws it, whereas here a record that
/// replays becomes a free-standing [`pool::Drop`] with no packet identity left to test. The
/// available proxy is the drop's own distance, and the honest way to use it is to make it
/// **strictly weaker** than the reference's rule, so it can never retire a particle the reference
/// would still be drawing:
///
/// > a flake is born inside the spawn box, i.e. at most `√(2·half_xy² + z_off²)` from the
/// > emission origin (99.3 yd rain / 70.4 yd snow — a rotation preserves length, so the slab tilt
/// > does not widen it), and that origin is the eye plus the wind lead `1.75·W` (`W` is the
/// > player's own averaged velocity, ≤ ~25 yd for an epic mount). So the reference never draws a
/// > flake past `200 + 99.3 + 25 ≈ 324` yd.
///
/// 50 yd covers the lead and the lateral drift over a fall with room to spare, and the *cases the
/// rule exists for* clear any such threshold by three orders of magnitude — a `.go` moves the eye
/// thousands of yards.
const RETIRE_DROP_SLACK: f32 = 50.0;
/// Benilla's live+pending bound per effect (the reference has none — allocator-bounded): the
/// leg's steady demand `rate · fall_time` plus one packet of pipeline lag, with headroom.
///
/// **Per kind, because the two demands differ by 1.7×** and one shared number silently clipped
/// snow. Rain falls at ~28 yd/s from +37.5, so `35000 · 1.34 s ≈ 47k`; snow *sinks* at 5.5–6.5
/// from +30, so `14000 · ~5.5 s ≈ 76k` — against the old shared `0xC000` = 49152 the snow field
/// saturated, emission stalled at the bound, and the sky held ~64% of the reference's flakes.
/// The vertex cost is the reason to keep these tight: the reference draws one *point* per flake
/// (`glDrawArrays(GL_POINTS)`, 32 B), while benilla's shared effect stream draws a quad, so every
/// flake costs 4 vertices here.
///
/// **`fall_time` is TERRAIN, not a constant — which is what the `0x18000` sizing missed.** A drop
/// lives until the ground under it, so the demand scales with how far the ground falls away below
/// the spawn plane, and that is a property of where the player is standing. Measured live (the
/// 1 Hz field census, grade 1.0): Kharanos' bowl gives a mean fall of **29.6 yd** and a steady
/// ~69k, while open ground 210 yd west gives **44.5 yd** and an unclipped demand of
/// `14000 · 44.5/6.0 ≈ 104k` live plus a packet — past `0x18000` = 98304, so emission throttled
/// and the field ran ~11% thin. Sized here off that measured worst case with headroom.
///
/// **Those demand figures predate 1165's halving** and are kept because they are what was
/// measured. Re-measured after it (1166's probe, two Dun Morogh pins at effect density 0.60):
/// **32.4k and 38.3k live** plus one full packet. Demand is *sub*-linear in density — a denser
/// flake also falls faster ([`SNOW_VZ_W`]), so it lives less long — which puts grade 1.0 at
/// ~48k + 6k, i.e. the snow bound now sits ~2.4× above its worst case. Deliberately not retuned:
/// this is a **ceiling, not a reservation** (`pool_bound` is a spawn cap and [`POOL`] a `take(…)`
/// guard; nothing here preallocates, the pools grow to live demand), so slack costs nothing, and
/// it is right again the moment [`REF_MAXFPS`] is turned back to the byte-faithful 60.
///
/// **The tell that this bound is biting is `pending_len()` falling below [`PACKET_CAP`]**: the
/// pipeline holds exactly one full packet whenever emission is unthrottled (measured 6110–6150 at
/// both grades and both sites), so a census line reporting a *suppressed* pipe count is the clip,
/// and the visible field is thinner than the law asks for. Read it that way before concluding
/// anything about density from a `live` count.
const fn pool_bound(kind: WeatherKind) -> usize {
    match kind {
        WeatherKind::Snow => {
            if SHADER_LEG {
                0x20000
            } else {
                0x4000
            }
        }
        _ => {
            if SHADER_LEG {
                0xC000
            } else {
                0x4000
            }
        }
    }
}
/// The largest pool bound — the `take(…)` guard on a push, where the kind is already implied by
/// which pool is being drawn.
const POOL: usize = pool_bound(WeatherKind::Snow);
// Density gain `K` (`rate = K·P·grade` per second) is the live `weatherDensity` setting's table
// entry — `WeatherState::density_gain()` (`0x67b870`: 0.1/0.33/0.66/1.0 for setting 0–3). The
// transcriptions carried quality 2's 0.66 as a baked constant; the reference runs at 3 (K=1.0).
/// Rain population `P` — fixed-func `0x80ffa8` = 6500, shader `0x80ffac` = 35000. (The old
/// `_DAY/_NIGHT` labels were a misread; `effect+0` is the leg flag — see [`SHADER_LEG`].)
const RAIN_P: f32 = if SHADER_LEG { 35000.0 } else { 6500.0 };
/// Snow population `P` — fixed-func `0x80ffdc` = 1300, shader `0x80ffe0` = 14000.
const SNOW_P: f32 = if SHADER_LEG { 14000.0 } else { 1300.0 };
/// The frame cap the **reference install** runs at — `SET maxfps "30"` in its `Config.wtf`.
///
/// **This is a LOOK constant, not a byte-cited one** (the same species as [`SNOW_PX_REF_HEIGHT`],
/// and it is here for the same reason). Do not "correct" it to 60 for fidelity: 60 *is* the
/// byte-faithful value and it is what shipped before 1165.
///
/// The reference's per-frame budget is `min(dt, 1/60)·rate` with the remainder **discarded**
/// (`0x67846f`–`0x678480`), so emission per second is `rate · min(1, fps/60)` — a client below
/// 60 fps gets proportionally less precipitation. The trace measured the reference emitting
/// exactly 233 flakes per frame, pinning `rate` to 14000/s nominal; at its own 30 fps cap that is
/// **6990/s on screen, half of nominal**. benilla at 60+ fps runs the full 14000/s and reads twice
/// as dense against the director's A/B with every constant identical on both sides.
///
/// So the number that needed denominating was the **frame rate the look was observed at**, exactly
/// as 1162's `14` needed the screen height it was authored against. A quantity whose unit is
/// "per frame" is denominated in a frame rate, and porting it forward means porting the
/// denominator (1162 §2).
const REF_MAXFPS: f32 = 30.0;
/// [`REF_MAXFPS`] as a gain on the spawn rate: `min(1, REF_MAXFPS/60)`.
///
/// Applied to the **drop/flake** rate only, never to the mist. The drop kernels throw their
/// sub-unit remainder away every frame, which is what couples them to the frame rate; the mist
/// accumulator carries its leftover across frames (`0x67b172`) and is the one
/// precip rate in the reference that is *already* frame-rate independent. Halving it too would be
/// a second, unrelated error dressed as consistency.
const REF_FPS_GAIN: f32 = if REF_MAXFPS < 60.0 {
    REF_MAXFPS / 60.0
} else {
    1.0
};
/// A patter lives 0.25 s (`0x675280`: `lifetime = now + 0.25`).
const PATTER_LIFE: f32 = 0.25;
/// Per-frame spawn budget uses `min(dt, 1/60)` (`0x80ffcc`).
const DT_CAP: f32 = 1.0 / 60.0;
/// The spawn box leads the camera by `wind_motion · 1.75` (the spawn kernels' R11, `0x8680f8`).
const WIND_LEAD: f32 = 1.75;
/// The drift heading CENTRE — the bit-cited constant −1.57 rad (`0x80ffbc`, rain R3 / snow R3a):
/// a **fixed WORLD azimuth** in the WoW frame, shared by rain and snow. The 0311-era code steered
/// the drift by the live wind heading (falling back to 0 when calm) — the director's "rain
/// direction seems different": calm-camera rain leaned toward an arbitrary Bevy axis instead of
/// the reference's world-anchored one.
const DRIFT_AZ_CENTER: f32 = -1.57;
/// Rain fall speed: `vz = −28 − 4w − 2w·r` (`0x80306c`/`0x80ffb4`, bit-cited).
const RAIN_VZ_BASE: f32 = 28.0;
const RAIN_VZ_W: f32 = 4.0;
const RAIN_VZ_RNG: f32 = 2.0;
/// Rain horizontal drift: `((2r−1) + 9.49)·w + 0.01` (R4), heading spread `w·0.209 + 0.052` rad
/// (R3) about the wind heading.
const RAIN_DRIFT_BASE: f32 = 9.49;
const RAIN_DRIFT_EPS: f32 = 0.01;
const RAIN_SPREAD_W: f32 = f32::from_bits(0x3e56_7750); // 0x80ffc4 ≈ 0.20944 (12°)
const RAIN_SPREAD_BIAS: f32 = f32::from_bits(0x3d56_7750); // 0x80ffc0 ≈ 0.05236 (3°)
/// The streak triangle: base verts `head ∓ 0.05·RIGHT` (`0x80ff78`),
/// apex `head + tilt·(2.0·antiVel̂)` (`0x80ff74`) — FIXED world-space sizes, not |vel|-scaled.
/// No vertex colour/alpha; the look is the texture under Mod2x + the forced fog.
const STREAK_HALF_W: f32 = 0.05;
const STREAK_TAIL: f32 = 2.0;
/// Rain's forced fog window (render-state 0x0a/0x0b, CPU leg): start 70, end 75 — under Mod2x
/// the grey-0.5 fog colour is NEUTRAL, so this IS the streak distance fade. `pub(crate)`: the
/// effect lane's canonical `EffectFog::Rain` params row is written from these.
pub(crate) const RAIN_FOG_START: f32 = 70.0;
pub(crate) const RAIN_FOG_END: f32 = 75.0;
/// Patter triangle half-edges: `view_right/12` × `view_up/6` (`0x80e004`/`0x803568`, CONFIRMED).
const PATTER_RIGHT: f32 = 1.0 / 12.0;
const PATTER_UP: f32 = 1.0 / 6.0;
/// Spawn-box horizontal half-extents (driver `0x67be40` ctor args): rain 130×130 (`0x43020000`),
/// snow 90×90 (`0x42b40000`).
const RAIN_HALF_XY: f32 = 65.0;
const SNOW_HALF_XY: f32 = 45.0;
/// The slab's lift in **slab-local** space — HALF the ctor vertical extent (rain 75 → +37.5,
/// snow 60 → +30). The box's z-RANDOM is dead (`·0.0`) but the constant lift is not —
/// `0x674df6–e53` builds `T = −37.5/Vz` and seeds `local.z = −T·Vz = +37.5`, back-projecting the
/// drop so its trajectory passes the arrival-plane scatter point at t = T. The earlier "spawns AT
/// eye height" read was the bug behind camera-angle-dependent density, the precip-free view above
/// the horizon, and uphill terrain getting no rain (director-caught, 2026-07-12).
///
/// **It is not a world height.** The slab tilt rotates the local offset, so the realised spawn
/// heights fan out to `z_off·cos α ∓ half_xy·sin α` — ~8..46 yd for snow at a 7 yd/s run, and the
/// leading edge dips *below* the eye entirely past ~13 yd/s. Reading this as "the plane the
/// particles are born on" is what made the untilted model look self-consistent.
const RAIN_Z_OFF: f32 = 37.5;
const SNOW_Z_OFF: f32 = 30.0;
/// Snow fall speed: `vz = −2 − 3.5m − m·r` (`0x6778bc`–`0x6778ec`, bit-cited) — calm
/// flakes sink at 2 yd/s, a full blizzard at up to 6.5.
const SNOW_VZ_BASE: f32 = 2.0;
const SNOW_VZ_W: f32 = 3.5;
/// Snow horizontal drift: `((r − 0.5) + 5.985)·m + 0.015` (R3b: 5.985/0.015 bit-cited).
const SNOW_DRIFT_OFF: f32 = 5.985;
const SNOW_DRIFT_EPS: f32 = 0.015;
/// Snow heading spread: `2π − 5.934·m` (R3a, `0x80fff4`) — calm snow wanders in ANY direction,
/// a blizzard's flakes align to ±10° of the wind.
const SNOW_SPREAD_W: f32 = f32::from_bits(0x40bd_e44f); // ≈ 5.9341197
/// Snow flake size — **`snowpoint.bls`'s point-size law, in WINDOW PIXELS** (read at the shipped
/// shader's bytes):
///
/// ```text
/// pointsize(d) = max(1.0, 14.0 · clamp01(1 − 0.02·d))   pixels, d = |flake − eye| in yards
/// ```
///
/// The snow leg that RUNS is `0x678610`, the ARB **point-sprite** pass
/// (`glDrawArrays(GL_POINTS)`, one vertex per flake, `GL_COORD_REPLACE` texcoords,
/// `GL_VERTEX_PROGRAM_POINT_SIZE_ARB`); `0x678960`'s 1/12 world-space triangle is the
/// fixed-function fallback, dead on any GPU since ~2002 (ctor `0x677420` clears the leg flag only
/// when `snowpoint.bls` fails to compile, `GL_ARB_point_sprite` is missing, or
/// `useWeatherShaders` is 0). benilla drew the *fallback's* geometry while taking the *shader*
/// leg's population — the two mutually exclusive sides of `0x6790c1` — and inflated it further
/// with an invented per-flake size jitter; that pairing is what made the flakes read as far too
/// big.
///
/// **This is not a world size, and it is not `∝ 1/d`.** A flake 1 yd away is 14 px and one 30 yd
/// away is 5.6 px — near flakes far smaller, and distant flakes far larger, than any fixed
/// world-space quad. wgpu has no point-sprite size (WebGPU pins `PointList` at 1 px), so
/// [`render::push_flakes`] reproduces the law by inverting the projection: a quad of world
/// half-extent `px · z_view · tan(fovY/2) / viewport_height_px` covers exactly `px` pixels.
const SNOW_PX_AT_EYE: f32 = 14.0;
/// The screen height those pixels were authored against — **the resolution the size law is
/// implicitly denominated in**, and the one term that has to be ours rather than the reference's.
///
/// `snowpoint.bls` sizes a flake in *framebuffer pixels*, so its **angular** size is
/// `14 / framebuffer_height`. That was never a choice about looks — it is simply how
/// `GL_VERTEX_PROGRAM_POINT_SIZE_ARB` works — and on 2004 hardware the framebuffer *was* the
/// screen. Obey the number literally on a modern display and the flakes shrink in proportion to
/// how good the monitor is: at the reference install's own `gxResolution 1280x800` a near flake
/// spans `14/800` = **1.75%** of frame height, while benilla on a scale-factor-2 4K panel
/// (physical ≈ 2144 tall) spans `14/2144` = **0.65%** — **2.7× smaller**, from resolution alone.
/// That is B233's "the size of the snow is way too small" (director A/B), and it is
/// the same failure mode `lib.rs` already records for FFXGlow's texel-pinned blur geometry.
///
/// So the law is evaluated in **era pixels** and converted to an angle, which makes the sprite
/// resolution-independent: the live viewport height cancels out of
/// `px · z · tan(fovY/2) / height` entirely. At an 800-px-tall window benilla draws exactly the
/// reference's 14 physical pixels; at 4K it draws 37 physical pixels covering the same angle.
///
/// **This is the number to turn if the flakes still read wrong.** It is a look constant, not a
/// byte-cited one — 800 is this install's `Config.wtf`, i.e. the resolution the director's own
/// side-by-side was captured at. A different era resolution is a different (equally faithful)
/// look: 640×480 would make them 1.67× bigger again.
const SNOW_PX_REF_HEIGHT: f32 = 800.0;
/// The falloff slope: `1 − 0.02·d` reaches the 1 px floor at `d = 13/0.28 ≈ 46.4` yd.
const SNOW_PX_FALLOFF: f32 = 0.02;
/// `max(1.0, …)` — the floor the ARB program applies last.
const SNOW_PX_MIN: f32 = 1.0;
/// A settled flake fades over 0.25 s (`0x678dfa`, constant `0x8029b0`: `max(t,0) + 0.25`).
const SNOW_SETTLE_LIFE: f32 = 0.25;
/// A FALLING flake fades **in** over its first second — the vertex program's
/// `alpha = clamp01(t − f1)`. benilla drew every falling flake at
/// alpha 1.0, so the top of the column was a hard-edged sheet instead of a soft one.
const SNOW_FADE_IN: f32 = 1.0;

/// The precipitation state: per-kind pools + the mist companion + the shared xorshift RNG.
#[derive(Resource)]
pub(super) struct Precip {
    rain: Pool,
    snow: Pool,
    mist: Mist,
    rng: u32,
}

/// Is weather killed by the camera's room? The client's weather-visible flag `[0xca80c4]`
/// inverted: cleared per frame (`0x6b38c1`), force-set with the camera outdoors (`0x6811d4`),
/// and it gates the ENTIRE weather update+render dispatch (`0x677380`/`0x6790b0`/`0x67a520`:
/// load, `je → ret`) — drops, patters, and mist together, with the wire state untouched. The
/// flag's SECOND leg (recorded at `0x6b42d9`, folded 2026-07-13): with the camera in an
/// interior it re-sets if an EXTERIOR-flagged group (`&0x148`) is portal-reachable — rain
/// stays visible (and falling) through a doorway's frame; the reference through the inn's
/// open door shows the storm running (director ref-shot). Only a room with no reachable
/// outside kills the effect — there it FREEZES (drops hang, unrendered) and resumes on
/// stepping out. Exact writer semantics are not fully enumerated; this is the recorded
/// shape.
#[derive(Resource, Default)]
pub(super) struct WeatherIndoors(bool);

/// Resolve [`WeatherIndoors`] from the camera's PVS-flood interior claim
/// ([`CameraInteriorClaim`], written by the portal pass this frame). The flag gates both the
/// sim ([`simulate_precip`] — the freeze) and the stream push ([`push_precip`] — frozen drops
/// hang, unrendered, exactly the reference's `[0xca80c4]` draw kill).
fn gate_weather_indoors(claim: Res<CameraInteriorClaim>, mut indoors: ResMut<WeatherIndoors>) {
    indoors.0 = claim.0.is_some_and(|c| !c.exterior_visible);
}

/// xorshift32 → [0,1) — the reference's lagged-table generator is byte-known but its
/// distribution is uniform; the mechanism needs uniformity, not that exact stream.
fn rand01(rng: &mut u32) -> f32 {
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Ground-layer capacity — the reference's patter pool is 0x1800 slots (`Packet<Patter,Rain>`),
/// same as the drop pool; at grade 1 the 1:1 landing spawns keep ~rate·0.25 s ≈ 3–6k alive.
const GROUND_CAP: usize = 0x1800;

/// The four precip textures (loaded once; the stream's render-side residency gate withholds
/// draws until they arrive).
#[derive(Resource)]
struct PrecipAssets {
    streak: Handle<Image>,
    splash: Handle<Image>,
    flake: Handle<Image>,
    mist: Handle<Image>,
}

fn setup_precip(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    existing: Option<Res<Precip>>,
) {
    // Lazy spawn on the first frame with a live camera (the same lifecycle as every other
    // particle entity — M2 emitters spawn when terrain streams).
    if existing.is_some() || cam.single().is_err() {
        return;
    }
    // Streaks keep the authored mips (a solid bar survives minification); the splash + mist
    // cut-outs load mip-0-only (`BlpVariant::Effect`) — they draw as world-space quads that are
    // magnified far more often than minified, and their thin arms collapse under the mip chain.
    // The FLAKE is the exception (`BlpVariant::PointSprite`): the reference's point sprite is
    // 14 px at the eye and 1 px past 46 yd — a 4.5×–64× minification of a 64×64 texture, which
    // is why the asset ships 7 mips. Mip-0-only there is flickering speckle, not crispness.
    let effect = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::Effect;
    };
    let point_sprite = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::PointSprite;
    };
    commands.insert_resource(PrecipAssets {
        streak: asset_server.load("mpq://textures/weather/raindrop01.blp"),
        splash: asset_server
            .load_with_settings("mpq://textures/weather/raindropsplash01.blp", effect),
        flake: asset_server
            .load_with_settings("mpq://textures/weather/snowflake01.blp", point_sprite),
        mist: asset_server.load_with_settings("mpq://textures/weather/snowmist01.blp", effect),
    });
    commands.insert_resource(Precip {
        rain: Pool::default(),
        snow: Pool::default(),
        mist: Mist::default(),
        rng: 0x9E37_79B9,
    });
}

/// Per frame: wind, spawn budgets, integrate, land. (The geometry push is [`push_precip`]'s,
/// in `PostUpdate` after the stream clear.)
fn simulate_precip(
    time: Res<Time>,
    weather: Res<WeatherState>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    player: Query<&Transform, (With<crate::world_unit::ViewerUnit>, Without<WorldCamera>)>,
    // The commanded planar speed the spawn slab's tilt keys on (`mgr+0x7c`) — live, not the
    // wind's 149 ms average.
    viewer: Res<crate::view::Viewer>,
    spatial: SpatialQuery,
    indoors: Res<WeatherIndoors>,
    mut wind: ResMut<WeatherWind>,
    mut heights: ResMut<HeightCache>,
    mut precip: Option<ResMut<Precip>>,
    mut last_cut: Local<u32>,
    // Frames since the previous census — the spawn budget scales with frame rate, so two census
    // lines are only comparable at equal frame counts (see [`pool::census`]).
    mut census_frames: Local<u32>,
) {
    let Some(precip) = precip.as_deref_mut() else {
        return;
    };
    // Indoors the whole effect freezes — the reference gates the update+render dispatch on
    // `[0xca80c4]` (`je → ret`), so nothing spawns, integrates, ages, or rebuilds; the meshes
    // are already hidden by [`gate_weather_indoors`].
    if indoors.0 {
        return;
    }
    let Ok(cam_tf) = cam.single() else { return };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let now = time.elapsed_secs();
    let cam_pos = cam_tf.translation();
    // The wind tracks the local PLAYER's motion (object 0x8fe) — orbiting the camera does not
    // stir it. The camera stands in only when no player entity exists (captures, data-less), and
    // then its own forward is the heading's sub-1-yd/s source.
    let (wind_pos, facing) = player
        .single()
        .map_or((cam_pos, cam_tf.forward().as_vec3()), |t| {
            (t.translation, t.forward().as_vec3())
        });
    wind.update(wind_pos, facing, viewer.planar_speed, dt);

    let filter = crate::collision::WorldCollision::body_filter();
    let Precip {
        rain,
        snow,
        mist,
        rng,
        ..
    } = precip;

    // The type-change cut (`0x67585d`): a wire TYPE change (fine included) stops emission at once
    // and discards the not-yet-replaying pipeline; playing cohorts and falling drops finish. The
    // mist's scheduled-but-unborn nodes retire with it (`0x67b234`).
    if weather.cut_seq != *last_cut {
        *last_cut = weather.cut_seq;
        rain.cut(now);
        snow.cut(now);
        mist.cut();
    }

    run_kind(
        rain,
        WeatherKind::Rain,
        &weather,
        &wind,
        &mut heights,
        &spatial,
        &filter,
        rng,
        now,
        dt,
        cam_pos,
    );
    run_kind(
        snow,
        WeatherKind::Snow,
        &weather,
        &wind,
        &mut heights,
        &spatial,
        &filter,
        rng,
        now,
        dt,
        cam_pos,
    );

    // Throttled 1 Hz state log — the capture/live diagnosis instrument (debug builds only
    // chatter when something is falling or queued in the packet pipeline). Second-resolution
    // deliberately: the pipeline's visible-onset instant is what look-checks time against.
    *census_frames += 1;
    if (weather.effect_density > 0.0
        || !rain.drops.is_empty()
        || !snow.drops.is_empty()
        || rain.pending_len() > 0
        || snow.pending_len() > 0)
        && time.elapsed_secs().fract() < dt
    {
        debug!(
            "weather pools: rain {} (+{} pipe, +{} ground) snow {} (+{} pipe, +{} ground), density {:.2}",
            rain.drops.len(),
            rain.pending_len(),
            rain.patters.len(),
            snow.drops.len(),
            snow.pending_len(),
            snow.patters.len(),
            weather.effect_density,
        );
        // …and WHERE those drops are relative to the player, which the counts cannot say. The
        // split axis is the player's heading while they move and their view heading while they
        // stand, so the standing reading is the ~50/50 baseline the moving one is read against.
        let motion = wind.vel.with_y(0.0);
        let axis = motion
            .try_normalize()
            .or_else(|| cam_tf.forward().as_vec3().with_y(0.0).try_normalize())
            .unwrap_or(Vec3::Z);
        // …and, for the vertical question the horizontal census cannot answer, WHERE the field
        // stops overhead and how sharply it gets there (the director's "hard line" report).
        // Rain passes fade-in 0: its streaks carry no vertex alpha, so every drop weighs 1.
        for (name, pool, fade_in) in [("rain", &*rain, 0.0), ("snow", &*snow, SNOW_FADE_IN)] {
            if let Some(c) = census(&pool.drops, cam_pos, axis, motion.length(), *census_frames) {
                debug!("weather field ({name}): {c}");
            }
            if let Some(p) = profile(&pool.drops, cam_pos, fade_in) {
                debug!("weather column ({name}): {p}");
            }
        }
        *census_frames = 0;
    }

    // ===== the mist companion =====
    {
        let Precip { mist, rng, .. } = precip;
        run_mist(
            mist,
            &weather,
            &wind,
            &mut heights,
            &spatial,
            &filter,
            rng,
            dt,
            cam_pos,
        );
    }
}

/// Push the frame's precip geometry onto the shared effect stream (PostUpdate, after the
/// stream clear): rain streaks + patters as Mod2x tri-lists under the forced grey fog (their
/// textures are authored grey-128-neutral — checked: both backgrounds mean exactly RGB 128);
/// snow and mist as alpha-blended quads with fog off (`0x678610`, `0x67ae20`). All four draws
/// anchor at the camera — view-z ≈ 0 sorts them after every world transparent, before the
/// biased glare/nameplate rungs, exactly where the old camera-anchored layer entities landed.
/// Indoors nothing is pushed — the frozen drops hang, unrendered (`[0xca80c4]`'s draw kill).
fn push_precip(
    precip: Option<Res<Precip>>,
    assets: Option<Res<PrecipAssets>>,
    indoors: Res<WeatherIndoors>,
    lighting: Res<WowLighting>,
    wind: Res<WeatherWind>,
    cam: Query<(Entity, &GlobalTransform, &Camera, &Projection), With<WorldCamera>>,
    mut quads: ResMut<EffectQuads>,
) {
    let (Some(precip), Some(assets)) = (precip, assets) else {
        return;
    };
    if indoors.0 {
        return;
    }
    let Ok((cam_entity, cam_tf, camera, proj)) = cam.single() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    let cam_right = cam_tf.right().as_vec3();
    let cam_up = cam_tf.up().as_vec3();
    // Snow's size law is in ERA PIXELS (`snowpoint.bls` sizes in framebuffer pixels; benilla
    // denominates them in the era's screen height so the *angle* carries over — see
    // [`SNOW_PX_REF_HEIGHT`]). The live viewport is still required, but only as the "is this camera
    // actually drawing?" gate: a camera with no sized target draws nothing this frame anyway.
    let flake_view = camera
        .physical_viewport_size()
        .filter(|vp| vp.y > 0)
        .map(|_| FlakeView {
            eye: cam_pos,
            forward: cam_tf.forward().as_vec3(),
            right: cam_right,
            up: cam_up,
            world_per_px: match proj {
                Projection::Perspective(p) => (p.fov * 0.5).tan(),
                // An orthographic world camera has no perspective divide; the reference has no
                // such mode, so there is nothing to be faithful to — keep the sprites sane.
                _ => (crate::view::CAM_FOVY * 0.5).tan(),
            } / SNOW_PX_REF_HEIGHT,
        });
    let spec = |texture: &Handle<Image>, blend: EffectBlend, fog: EffectFog| EffectDrawSpec {
        cam: cam_entity,
        texture: texture.id(),
        blend,
        fog,
        // Rain/snow ride the reference's own weather render state (its verified Mod2x /
        // forced-grey-fog trio), not the M2 batch state producer — no GL_LIGHTING on them.
        lighting: crate::particles::buffer::EffectLighting::None,
        anchor: cam_pos,
        bias: 0.0,
        raster_bias: 0,
        raster_slope: 0.0,
        // Streaks, patters and mist are all centimetre-scale or bigger, so absolute world verts
        // cost them nothing; the flake draw below overrides this (its quads are millimetres).
        cam_relative: false,
        no_depth_test: false,
        main_entity: Entity::PLACEHOLDER,
        light: None,
        clip: None,
    };
    let start = quads.begin();
    push_streaks(&mut quads.verts, &precip.rain.drops, wind.tilt, cam_pos);
    quads.commit_tris(
        start,
        spec(&assets.streak, EffectBlend::Mod2x, EffectFog::Rain),
    );
    let start = quads.begin();
    push_patters(&mut quads.verts, &precip.rain.patters, cam_right, cam_up);
    quads.commit_tris(
        start,
        spec(&assets.splash, EffectBlend::Mod2x, EffectFog::Rain),
    );
    if let Some(view) = &flake_view {
        let start = quads.begin();
        push_flakes(
            &mut quads.verts,
            &precip.snow.drops,
            &precip.snow.patters,
            view,
        );
        quads.commit_quads(
            start,
            EffectDrawSpec {
                // [`push_flakes`] wrote camera-relative offsets — the only family in the lane
                // whose geometry is small enough for absolute-world f32 to lose it.
                cam_relative: true,
                ..spec(&assets.flake, EffectBlend::Alpha, EffectFog::Off)
            },
        );
    }
    let start = quads.begin();
    push_mist(
        &mut quads.verts,
        &precip.mist,
        cam_right,
        cam_up,
        cam_pos,
        lighting.fog_color,
    );
    quads.commit_quads(
        start,
        spec(&assets.mist, EffectBlend::Alpha, EffectFog::Off),
    );
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<WeatherWind>()
        .init_resource::<HeightCache>()
        .init_resource::<WeatherIndoors>()
        .add_systems(
            Update,
            (
                setup_precip,
                gate_weather_indoors.after(setup_precip).after(WmoPvsSet),
                simulate_precip
                    .after(WeatherTick)
                    .after(gate_weather_indoors),
            ),
        )
        // The stream push: PostUpdate after the frame's clear (the sim ran in Update).
        .add_systems(PostUpdate, push_precip.after(begin_effect_frame));
}
