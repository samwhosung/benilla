//! Per-frame positioning + tinting of the celestial layer — pin the sun and the white moon (each a
//! disc with an additive lens-flare glare) and the star dome to their world directions, camera-facing;
//! rewrite every disc/glare tint from the celestial diffuse band; drive the `0x6cf490` view-lerped
//! glare scale/intensity and the star-curve global alpha. The DISCS sit just inside the far plane
//! (terrain occludes them per-pixel, like the reference's far depth slice) and their horizon clip+fade
//! lives in the shader ([`super::materials`]); the GLARE quads sit on the reference's **near sphere**
//! (`cam + 12·dir`) with their fragment depth forced far (`celestial.wgsl`) — like the reference,
//! the flare is dimmed by BOTH the z-buffer (per-pixel: a ridge, a wall, a leaf clips it — its
//! `[0.995, 1.0]` back-slice depth test, `0x7e5a0c`) and the envelope —
//! the slewed [`FlareGate`]: the per-body **dnCurve** day/night gate × the below-horizon
//! smoothstep × the terrain/interior visibility (the occ3 fractional-probe stand-in),
//! rate-limited like the reference's `[glare+0x30]` (decision 0508).
//!
//! **These systems run in `PostUpdate` after transform propagation** ([`crate::billboard::BillboardPlace`],
//! the camera-anchored placement slot), reading the camera's SAME-frame propagated pose and writing
//! `GlobalTransform` directly — the placement that renders. In plain `Update` they read the camera's
//! *last*-frame `GlobalTransform` (Bevy syncs it only in `PostUpdate`): invisible at the far discs
//! (error ≈ v·dt over ~2.5k units), but the near-sphere glare quads are ~200× more sensitive — one
//! frame of camera motion is ~1% of their 12-unit distance, so the moon's halo visibly swam while
//! strafing and pumped bigger/brighter running toward it, smaller/dimmer running away (the director's
//! Westfall moonrise report; measured exactly one frame in the wiring test below — decision 0504,
//! the billboard/nameplate lag's celestial sibling).

use bevy::camera::Projection;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::clouds::{occ1_moon, occ1_sun, CloudCoverage};
use crate::dev_state::DebugState;
use crate::lighting::WowLighting;
use crate::terrain_stream::TerrainStreamer;
use crate::view::WorldCamera;
use crate::wdl::WdlStreamer;
use crate::wmo_portal::CameraInteriorClaim;
use benilla_assets::AdtTile;

use super::materials::{CelestialMaterial, StarMaterial};
use super::{MoonPart, MoonSprite, StarDome, SunPart, SunSprite};

/// Faithful body base angular size: every disc/glare is a **unit quad at radius 12** (`[0x80c4e8]`,
/// decision 0485) — 1 world unit of quad scale subtends `2·atan(0.5/12) = 4.77°`, so a world-placed
/// billboard scales by `0.0833 × (its size in the builder's world units) × its distance`.
const SUN_SIZE: f32 = 0.0833;

/// The lens-flare quads live on the reference's **near sphere**: `cam + 12·dir` (`[0x80c4e8]`), quad
/// scale = the builder's world units directly — the same unit-quad-at-12 convention [`SUN_SIZE`]
/// encodes for the far-placed discs, so the angular sizes are identical by construction. Far
/// placement was our first "equivalent" and it broke at the byte-law flare size: a 20-unit quad at
/// `far·0.85` pierced the sky-dome sphere (`far·0.9`) and the depth test cut it along the
/// plane∩sphere circle — a giant faceted halo edge around the sun (decision 0500). At 12 units the
/// quad can touch neither the dome nor the far plane; its DEPTH is forced far per-fragment instead
/// (`celestial.wgsl` — the reference's own split: glare geometry on the near sphere, depth squashed
/// to the `[0.995, 1.0]` back slice, drawn LAST in the frame; the sort slot is
/// [`crate::sky_order::GLARE_BIAS`]).
const GLARE_DIST: f32 = 12.0;

/// Flare occlusion-march tuning: sample count and reach. The reach spans the whole DRAWN horizon —
/// the detailed ADT ring (the farclip-derived window, ≈ ±867 units at 777) and the coarse WDL beyond it, out to just
/// inside the far plane — so a distant WDL mountain that visibly hides the body also kills its flare
/// (the moon shone through the Westfall ridge when the march stopped at 1000). 48 quadratic samples
/// ⇒ ≤ ~117-unit spacing at the far end, well under a horizon mountain's footprint.
const FLARE_RAY_SAMPLES: u32 = 48;
const FLARE_RAY_RANGE: f32 = 2800.0;

/// Flare-envelope slew rates (per second) — `[glare+0x28]`/`[+0x2c]` (decision 0508): the
/// reference smooths the whole occlusion×dnCurve product with an **asymmetric LINEAR rate
/// limiter** (`[glare+0x30]`, sole writer `0x6cf5ea`) —
/// not an exponential ease. The sun's flare rises at 4.0/s, the moon's at 100/33 ≈ 3.03/s, and both
/// fall at 50/33 ≈ 1.52/s (a killed flare takes ~0.66 s to go dark). Bit-exact `.rdata` constants.
const SUN_FLARE_RISE: f32 = 4.0; // [0xce97d0]
const MOON_FLARE_RISE: f32 = f32::from_bits(0x4041_f07c); // [0xce9720] ≈ 3.0303
const FLARE_FALL: f32 = f32::from_bits(0x3fc1_f07c); // [0xce97d4]/[0xce9724] ≈ 1.5152

/// One celestial body's sprite row: transform + the body tag + its material — every disc AND glare
/// rides [`CelestialMaterial`] (disc mode alpha-blends with the horizon clip; glare mode gamma-ADDS,
/// the reference's SRC_ALPHA, ONE lens-flare blend — decision 0502).
type BodySprites<'w, 's, T> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut GlobalTransform,
        &'static T,
        &'static MeshMaterial3d<CelestialMaterial>,
    ),
    // Statically disjoint from the camera query in the same system (both touch GlobalTransform).
    Without<WorldCamera>,
>;

/// The `0x6cf490` lens-flare **view lerp**: `f = saturate((cosθ − 0.7) / 0.3)`, `cosθ` between the
/// camera's facing and the body direction — the glare grows and brightens as the view swings onto it.
fn view_lerp(cam_forward: Vec3, to_body: Vec3) -> f32 {
    ((cam_forward.dot(to_body) - 0.7) / 0.3).clamp(0.0, 1.0)
}

/// Below-horizon gate for the additive glares (they can't route the disc clip): a smoothstep on the
/// body's `sin(elev)` → 0 at/below the horizon. Stands in for the reference's occlusion queries
/// seeing a set body sink (a factor of the slew target, like them, `0x6cf490`; the day/night term
/// they multiply is now modeled for real as the dnCurve).
fn horizon_gate(to_body: Vec3) -> f32 {
    let t = (to_body.y / 0.035).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One step of the reference's `[glare+0x30]` **asymmetric linear slew** toward `target`
/// (`0x6cf59b`-`0x6cf5ea`): rising is capped at `rise·dt`, falling at `fall·dt`, and the value
/// never overshoots the target. Pure, for the unit test; the envelope maths live here.
fn flare_slew(current: f32, target: f32, rise: f32, fall: f32, dt: f32) -> f32 {
    current + (target - current).clamp(-fall * dt, rise * dt)
}

/// The disc quad's vertical span in **sin-elevation** — `(bottom edge, top edge, 0, 0)`, the
/// per-frame input to the shader's per-vertex fade reconstruction ([`CelestialExt::span`],
/// decision 0529). `dir_y` = sin(body elevation) (the Bevy camera→body direction's `y`); `size` =
/// the quad's angular scale factor (`tf.scale / dist` — the ±0.5 quad subtends `2·atan(size/2)`).
/// The camera-facing quad's vertical axis maps onto elevation, so the edges sit at
/// `elevation ∓ atan(size/2)`.
pub(super) fn disc_span(dir_y: f32, size: f32) -> Vec4 {
    let elev = dir_y.clamp(-1.0, 1.0).asin();
    let half = (0.5 * size).atan();
    Vec4::new((elev - half).sin(), (elev + half).sin(), 0.0, 0.0)
}

/// [`benilla_assets::quantize`] a [`disc_span`] to 1/4096 sin-elevation steps — the write gate's
/// floor for the one geometry lane the celestial writes carry. A step is ≪ one pixel of fade edge
/// on any disc, while un-quantized the span drifts every frame with time-of-day and would defeat
/// the gate.
fn quant_span(v: Vec4) -> Vec4 {
    Vec4::new(
        benilla_assets::quantize(v.x, 4096.0),
        benilla_assets::quantize(v.y, 4096.0),
        v.z,
        v.w,
    )
}

/// True when no resident-terrain column blocks the camera→body ray within [`FLARE_RAY_RANGE`].
/// Pure over a height oracle (`Some(ground WoW z)` under a Bevy point, `None` = no terrain in that
/// column — never an occluder), so the march is unit-testable without a streamed world. Quadratic
/// sample spacing densifies near the camera, where a hill subtends the most sky. Bevy `y` IS WoW
/// `z` (`coords::bevy_to_wow`), so the ray's own height is `p.y`.
fn flare_ray_clear(height_under: impl Fn(Vec3) -> Option<f32>, cam_pos: Vec3, dir: Vec3) -> bool {
    for i in 1..=FLARE_RAY_SAMPLES {
        let s = i as f32 / FLARE_RAY_SAMPLES as f32;
        let p = cam_pos + dir * (FLARE_RAY_RANGE * s * s);
        if height_under(p).is_some_and(|ground| ground > p.y) {
            return false;
        }
    }
    true
}

/// Ray-grid side count for the occ3 fraction — 4×4 = 16 rays; the envelope slew smooths the 1/16
/// quantization between frames.
const FLARE_FRACTION_GRID: u32 = 4;

/// The grid's cell count — the fraction's denominator and the cached mask's width.
const FLARE_CELLS: u32 = FLARE_FRACTION_GRID * FLARE_FRACTION_GRID;

/// How many grid cells the probe re-marches per frame (decision 1436): the full 16×48-sample march
/// every frame priced 0.45 ms/f — the biggest single system in the 1435 parked band map — for a
/// value the slew rate-limits anyway. Two cells/frame refreshes the whole grid in 8 frames
/// (0.13 s at 60 Hz), inside BOTH slew time constants (rise 0.25 s, fall 0.66 s), so the drip is
/// invisible under the smoothing that already covers the grid's own quantization. The faithful
/// end-state stays the reference's GPU occlusion query (`0x7e5220`, recorded below).
const FLARE_RAYS_PER_FRAME: u32 = 2;

/// The **visible fraction** of the body's quad footprint against resident terrain ∈ [0, 1] — our
/// CPU stand-in for the reference's fractional occlusion probe (`0x7e5220`): the real
/// client draws the disc's OWN quad (same size/position) depth-tested inside a GPU occlusion query
/// and uses `visiblePixels / projectedArea`, so a half-hidden sun carries ~half the flare. Here: a
/// [`FLARE_FRACTION_GRID`]² ray grid over the quad's angular extent (`half` = the quad's angular
/// half-size), each ray the same quadratic terrain march — fraction = clear rays / total rays.
/// Terrain-only, like the march it generalizes; the reference's z-buffer probe also catches
/// WMOs/doodads — the GPU-occlusion-query instrument remains the faithful end-state (recorded).
/// Since decision 1436 the live path drips cells through [`flare_mask_update`] instead; the full
/// one-call march stays as the convergence oracle for the tests.
#[cfg(test)]
fn flare_visible_fraction(
    height_under: impl Fn(Vec3) -> Option<f32>,
    cam_pos: Vec3,
    dir: Vec3,
    half: f32,
) -> f32 {
    let clear = (0..FLARE_CELLS)
        .filter(|&c| flare_cell_clear(&height_under, cam_pos, dir, half, c))
        .count();
    clear as f32 / FLARE_CELLS as f32
}

/// March ONE grid cell of the quad's [−half, +half]² angular footprint — the unit the round-robin
/// re-prices per frame (decision 1436). Cell `c` = row `c / 4`, column `c % 4`, centre-sampled.
fn flare_cell_clear(
    height_under: impl Fn(Vec3) -> Option<f32>,
    cam_pos: Vec3,
    dir: Vec3,
    half: f32,
    cell: u32,
) -> bool {
    // Any orthonormal frame across the camera-facing quad works — the probe is symmetric.
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let right = if right == Vec3::ZERO { Vec3::X } else { right };
    let up = right.cross(dir);
    let n = FLARE_FRACTION_GRID;
    let u = ((cell / n) as f32 + 0.5) / n as f32 - 0.5;
    let v = ((cell % n) as f32 + 0.5) / n as f32 - 0.5;
    let d = (dir + right * (u * 2.0 * half) + up * (v * 2.0 * half)).normalize();
    flare_ray_clear(&height_under, cam_pos, d)
}

/// One frame's round-robin step over the cached cell mask (bit set = that cell's ray is clear):
/// re-march `cells` cells from `cursor`, then read the WHOLE mask as the fraction. Pure, for the
/// convergence test; [`FlareGate::envelope`] owns the state.
fn flare_mask_update(
    mask: &mut u16,
    cursor: &mut u32,
    cells: u32,
    clear_of: impl Fn(u32) -> bool,
) -> f32 {
    for _ in 0..cells {
        let c = *cursor % FLARE_CELLS;
        let bit = 1u16 << c;
        if clear_of(c) {
            *mask |= bit;
        } else {
            *mask &= !bit;
        }
        *cursor = (c + 1) % FLARE_CELLS;
    }
    f32::from(mask.count_ones() as u16) / FLARE_CELLS as f32
}

/// The flare **envelope** — our build of the reference's `[glare+0x30]` smoothed intensity
/// (Addenda #5+#8, decisions 0508+0532): each frame a slew target is assembled from the terms we
/// model — the per-body **dnCurve** (`WowLighting.{sun,moon}_flare_dn`, the real day/night gate),
/// the below-horizon smoothstep, and the **fractional** terrain visibility of the disc's quad
/// ([`flare_visible_fraction`] — the reference's occ3 occlusion probe: a half-hidden sun carries
/// ~half the flare; a camera in a WMO interior kills it, the roof) — and the envelope slews toward
/// it linearly (rise per body, fall shared, never overshooting). Like the byte law, a zero
/// dnCurve×horizon target skips the probe entirely (`0x6cf58c`). **occ1** — the cloud coverage
/// over the body's sky point — is a caller-supplied factor now that the coverage field exists
/// (`clouds`): sun `1−R`, moon the thin-cloud tent `1−|2(R−0.5)|`, sampled at the glare's
/// 12-unit position exactly like `0x6cf7b0`/`0x6cf7d0`. The remaining unmodeled factors —
/// **occ2** (scene async-occlusion, 1.0 in normal outdoor play) and the scene lens-flare-slot
/// gate **(1−V)** (1.0 without scene-light flares) — stand at
/// 1.0 (recorded residual, 0532). One instance per system → one slewed scalar per body
/// (`follow_sun` the sun, `follow_moons` the white moon); seeds at 0 like the reference's `.bss`,
/// so a flare always rises into view.
#[derive(SystemParam)]
pub(super) struct FlareGate<'w, 's> {
    time: Res<'w, Time>,
    streamer: Res<'w, TerrainStreamer>,
    adt_tiles: Res<'w, Assets<AdtTile>>,
    /// The coarse whole-map WDL heightfield — the far leg of the march, beyond the resident ADT
    /// ring (the same surface that draws the distant horizon mountains). Absent in assetless dev.
    wdl: Option<Res<'w, WdlStreamer>>,
    camera_interior: Res<'w, CameraInteriorClaim>,
    /// The submerged-eye depth — the glare's own submersion fade (decision 1829). The glare is
    /// NOT part of the sky pass the client skips underwater: it draws in its own top-level pass
    /// (`0x6cf490` ← `0x6d48c0` ← `0x483740`, unconditional), while the sky is `0x6d4940` *inside*
    /// `0x681070`, which is what `0x6812a4` skips. Both facts are true at once, which is why the
    /// fade is live code and not a ramp on a pass that never runs.
    submerged: Res<'w, crate::liquid::SubmergedEye>,
    /// The live cloud coverage field — the occ1 sample source (one field serves the glare and
    /// the visible layer, like the reference).
    clouds: Res<'w, CloudCoverage>,
    env: Local<'s, f32>,
    /// The occ3 grid's cached per-cell verdicts (bit = clear) and the round-robin cursor
    /// (decision 1436): [`FLARE_RAYS_PER_FRAME`] cells re-march per active frame. `primed`
    /// falls when the probe is skipped (night, interior, below horizon) so the first active
    /// frame after a gap marches the whole grid instead of trusting a stale mask.
    mask: Local<'s, u16>,
    cursor: Local<'s, u32>,
    primed: Local<'s, bool>,
}

/// The glare's **submersion fade** — `1 − clamp(depth × 0.1, 0, 1)`, a 10-yard linear ramp on the
/// glare alpha (decision 1829). `depth` is `liquidSurfaceHeight − probeZ` in world-Z yards,
/// positive when submerged, so a dry camera reads `0` and the fade is the exact identity.
///
/// **The glare's submersion term is easy to miss.** The read is one indirection out
/// (`0x6cf4fe call [edx+0x10]` → the glare vtable's slot 4 → `0x6cf800`), which a direct-call
/// census cannot see. It is worth implementing rather than filing as trivia: the glare is
/// additive, so a black underwater celestial band would make it invisible anyway — and **288 of
/// 374 `Light.dbc` rows (77%) carry a non-zero one**, so on most of the map there is a glare down
/// there to fade.
///
/// Applies to the **sun and moon glares alike** (`0x6d48cf` / `0x6d48ea`; slot 4 of both glare
/// vtables is `0x6cf800`, and they differ only at slot 3) and to **neither disc** — the discs have
/// no submerged path at all, so suppressing them outright with the skipped sky pass is right. Both
/// confirmed by census rather than assumed.
fn submersion_glare_fade(depth: f32) -> f32 {
    1.0 - (depth * 0.1).clamp(0.0, 1.0)
}

/// The weather **celestial-alpha seed** (`0x6d2c74`): under active weather the recompute writes
/// `floor(255·(1−bcc))` over the five body colour alpha bytes (sun disc, sun glare, white-moon
/// disc, moon glare, moon02) — gated `bcc > 0`, so clear weather leaves the per-frame broadcast
/// `0xFF` (discs) and moon02's unwritten `0` untouched. Returns the byte-quantized seed, or
/// `None` in clear weather.
fn celestial_alpha_seed(bcc: f32) -> Option<f32> {
    (bcc > 0.0).then(|| (255.0 * (1.0 - bcc.min(1.0))).floor() / 255.0)
}

impl FlareGate<'_, '_> {
    /// The slewed `[0, 1]` flare envelope along `dir` from the camera: slew target = `dn` (the
    /// body's dnCurve sample) × the horizon smoothstep × `occ1` (the body's cloud occlusion) ×
    /// the visible fraction of the body's quad (`half` = its angular half-size; the interior
    /// claim zeroes it) and the **submersion fade** ([`submersion_glare_fade`]), all approached at
    /// `rise`/[`FLARE_FALL`] per second.
    ///
    /// The fade is INSIDE the slew — one of the factors composed into the target, not a multiply on
    /// the drawn alpha. `[glare+0x34]` is the per-frame target accumulator (reset to 1.0 at
    /// `0x6cf499`); `0x6cf501`/`0x6cf504` multiply the fade into it, and the limiter first reads it
    /// 158 bytes later at `0x6cf59b`, writing the envelope `[glare+0x30]` at `0x6cf5ea`. So
    /// surfacing ramps the glare back in over the rise time rather than snapping.
    ///
    /// We shipped it the other way first, on the reasoning that a rate limiter should smooth
    /// *visibility* and depth is not visibility. That reasoning was fine and the fact was against
    /// it: the fade is not a multiply on the drawn alpha byte `[glare+0x1b]`. Corrected at the
    /// bytes.
    ///
    /// It falls out that a deep camera (`fade == 0`) drives `base` to zero, which skips the terrain
    /// ray march entirely — the reference's own `0x6cf58c` shortcut, for free.
    fn envelope(
        &mut self,
        cam_pos: Vec3,
        dir: Vec3,
        dn: f32,
        occ1: f32,
        half: f32,
        rise: f32,
    ) -> f32 {
        let base = dn * horizon_gate(dir) * occ1 * submersion_glare_fade(self.submerged.depth);
        let target = if base > 0.0 && self.camera_interior.0.is_none() {
            // Resident detailed terrain first; the coarse WDL surface everywhere else (it
            // covers the whole map, so it also plugs the ADT-ring-to-farclip gap).
            let (streamer, adt_tiles, wdl) = (&self.streamer, &self.adt_tiles, &self.wdl);
            let tile = std::cell::RefCell::new(None);
            let oracle = |p| {
                crate::terrain_stream::terrain_height_under_cached(
                    streamer,
                    adt_tiles,
                    p,
                    &mut tile.borrow_mut(),
                )
                .or_else(|| wdl.as_ref().and_then(|w| w.height_under(p)))
            };
            // The round-robin drip (decision 1436): an unprimed mask marches every cell once,
            // a primed one re-prices FLARE_RAYS_PER_FRAME — the slew smooths the ≤8-frame
            // staleness exactly as it smooths the grid's own 1/16 quantization.
            let cells = if *self.primed {
                FLARE_RAYS_PER_FRAME
            } else {
                FLARE_CELLS
            };
            *self.primed = true;
            let fraction = flare_mask_update(&mut self.mask, &mut self.cursor, cells, |c| {
                flare_cell_clear(oracle, cam_pos, dir, half, c)
            });
            base * fraction
        } else {
            *self.primed = false;
            0.0
        };
        *self.env = flare_slew(*self.env, target, rise, FLARE_FALL, self.time.delta_secs());
        *self.env
    }
}

/// Pin the sun's disc + glare to the **visible celestial sun** direction, camera-facing, and rewrite
/// the tints from the per-frame celestial diffuse band (`WowLighting.celestial_tint`, the `0x6d2260`
/// broadcast). The DISC sits just inside the far plane (terrain occludes it) and sets edge-first
/// behind the horizon via the celestial.wgsl clip; its size is the unit quad × the day curve
/// (`0xce8cac`: 2× at the dawn/dusk horizon → 1× midday). The GLARE is the `0x6cf490` lens flare on
/// the near sphere ([`GLARE_DIST`]): quad scale `lerp(3, 20, f)` world units and intensity
/// `lerp(0.5, 1, f)` on the view lerp, × the slewed [`FlareGate`] envelope (the sun's dnCurve makes
/// it a DAY flare — full 07:30→19:30, gone by 21:00; decision 0508) — near-screen-filling star rays
/// when you look into the sun. Disabled together with the sky dome.
pub(super) fn follow_sun(
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    light: Res<WowLighting>,
    debug: Res<DebugState>,
    mut gate: FlareGate,
    mut mats: ResMut<Assets<CelestialMaterial>>,
    mut sprites: BodySprites<SunSprite>,
) {
    let Some((cam_gt, proj)) = cam.iter().next() else {
        return;
    };
    // Track the **visible celestial sun** (which rises and sets in elevation), NOT the near-fixed
    // lighting sun — `WowLighting.celestial_dir` is already the camera→sun (to-sun) direction.
    let to_light = light.celestial_dir.normalize_or_zero();
    if to_light == Vec3::ZERO {
        return;
    }
    let far = match proj {
        Projection::Perspective(p) => p.far,
        _ => 3000.0,
    };
    let dist = far * 0.85; // inside the sky dome (far*0.9), so the sun draws over the gradient
    let cam_pos = cam_gt.translation();
    // Billboard: the quad's +Z normal faces back toward the camera (= −to_light).
    let rot = Quat::from_rotation_arc(Vec3::Z, -to_light);
    let hidden = debug.lighting.disable_sky_dome; // the sun is part of the sky — hide it with the dome
    let f = view_lerp(*cam_gt.forward(), to_light);
    // Byte-quantized at the source ([`benilla_assets::quant255`]) so the write gates below only
    // fire on a display-visible change — the reference's own color lanes are bytes.
    let tint = benilla_assets::quant255(light.celestial_tint);
    // The weather celestial-alpha seed — dims the disc + glare under active weather.
    let seed = celestial_alpha_seed(light.storm_bcc);
    // The occlusion probe (`0x7e5220`) covers the disc's own quad: its angular half-size.
    let sun_half = (0.5 * SUN_SIZE * light.sun_disc_scale).atan();
    // occ1: cloud coverage over the sun's glare point (`0x6cf7b0` samples at glarePos = camera +
    // 12·dir) — a cloud drifting over the sun dims the flare linearly.
    let occ1 = occ1_sun(gate.clouds.coverage(to_light * GLARE_DIST));
    let env30 = gate.envelope(
        cam_pos,
        to_light,
        light.sun_flare_dn,
        occ1,
        sun_half,
        SUN_FLARE_RISE,
    );
    for (mut tf, mut gt, sprite, mat) in &mut sprites {
        tf.rotation = rot;
        match sprite.part {
            SunPart::Disc => {
                tf.translation = cam_pos + to_light * dist;
                let size = if hidden {
                    0.0
                } else {
                    SUN_SIZE * light.sun_disc_scale
                };
                tf.scale = Vec3::splat(size * dist);
                let color = Color::srgb(tint[0], tint[1], tint[2]);
                let span = quant_span(disc_span(to_light.y, size));
                // Above-band disc opacity: the 0xFF broadcast, overwritten by the weather
                // seed under active weather ([`celestial_alpha_seed`]).
                let fade_w = benilla_assets::quantize(seed.unwrap_or(1.0), 255.0);
                benilla_assets::write_gated(
                    &mut mats,
                    &mat.0,
                    |m| {
                        m.base.base_color != color
                            || m.extension.span != span
                            || m.extension.fade.w != fade_w
                    },
                    |m| {
                        m.base.base_color = color;
                        m.extension.span = span;
                        m.extension.fade.w = fade_w;
                    },
                );
            }
            SunPart::Glare => {
                tf.translation = cam_pos + to_light * GLARE_DIST;
                // `lerp(3, 20, f)` world units (sun endpoints `[0xce9838]`/`[0xce983c]`, base ×1.0)
                // — on the near sphere the builder's units are the quad scale directly.
                let units = 3.0 + 17.0 * f;
                tf.scale = Vec3::splat(if hidden { 0.0 } else { units });
                // Intensity: the view lerp's `lerp(0.5, 1, f)` (`[glare+0x9c]=0.5 → 1.0`, instant)
                // × the slewed envelope (dnCurve × horizon × visibility — `0x6cf490`'s
                // `[+0x1b] = floor(255·lerp·[+0x30])` shape). Rides base_color alpha — the shader's
                // glare mode ADDS `gamma(tint × texel) × a` onto the scene, the reference's
                // SRC_ALPHA byte weighting (decision 0502).
                // × the weather seed — the glare's alpha byte is the seed the per-frame pack
                // modulates (`oldByte` into `0x6cf490`), so storms dim the flare.
                let env =
                    benilla_assets::quantize((0.5 + 0.5 * f) * env30 * seed.unwrap_or(1.0), 255.0);
                let color = Color::srgba(tint[0], tint[1], tint[2], env);
                benilla_assets::write_gated(
                    &mut mats,
                    &mat.0,
                    |m| m.base.base_color != color,
                    |m| m.base.base_color = color,
                );
            }
        }
        // Propagation already ran this frame — the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

/// Pin the white moon's disc + glare to the moon's world direction (azimuth 45°, the sun's bearing),
/// camera-facing, at the same far distance as the sun, and rewrite both tints from the per-frame
/// celestial diffuse band (the same `0x6d2260` broadcast the sun reads — the glare is WARM; the teal
/// rim the director sees is the dome's night bands through the disc's feathered edge, decision 0485).
/// Disc size = the unit quad × the white-moon base ×1.75 × the shared size curve (`0xce8c8c`, 1.5×
/// at moonrise/set → 1× overhead); it rises/sets edge-first via the shader clip+fade. The GLARE quad
/// = `2.0 ×` the same curve (`0x6cf490` overwrites both lerp endpoints with it; ≈1.14× the disc),
/// intensity `lerp(0.1, 1, f)` on the view lerp × the slewed [`FlareGate`] envelope — whose moon
/// dnCurve makes the halo a DEEP-NIGHT thing: nothing until 22:45, full only near midnight
/// (decision 0508). Disabled with the sky dome.
pub(super) fn follow_moons(
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    light: Res<WowLighting>,
    debug: Res<DebugState>,
    mut gate: FlareGate,
    mut mats: ResMut<Assets<CelestialMaterial>>,
    mut sprites: BodySprites<MoonSprite>,
) {
    let Some((cam_gt, proj)) = cam.iter().next() else {
        return;
    };
    let far = match proj {
        Projection::Perspective(p) => p.far,
        _ => 3000.0,
    };
    let dist = far * 0.85;
    let cam_pos = cam_gt.translation();
    let hidden = debug.lighting.disable_sky_dome;
    // Byte-quantized at the source, like [`follow_sun`] — the write gates fire on display-visible
    // change only.
    let tint = benilla_assets::quant255(light.celestial_tint);
    // The weather celestial-alpha seed — dims both moons' discs + the glare under active weather.
    let seed = celestial_alpha_seed(light.storm_bcc);
    // One slewed envelope per frame, along the white moon's ray (only its glare exists). The moon's
    // dnCurve inside the target is the load-bearing night gate: flat 0 until 22:45, full near
    // midnight (decision 0508) — a 22:30 moonrise carries NO halo, exactly like the reference.
    let to_white = light.moon_dir_white.normalize_or_zero();
    let env30 = if to_white == Vec3::ZERO {
        0.0
    } else {
        // The occlusion probe (`0x7e5220`) covers the white moon's own disc quad.
        let moon_half = (0.5 * SUN_SIZE * 1.75 * light.moon_disc_scale).atan();
        // occ1: the moon's halo is a THIN-CLOUD effect — the tent `1−|2(R−0.5)|` (`0x6cf7d0`) is
        // zero in a perfectly clear patch of sky AND under full cover, peaking on a wisp.
        let occ1 = occ1_moon(gate.clouds.coverage(to_white * GLARE_DIST));
        gate.envelope(
            cam_pos,
            to_white,
            light.moon_flare_dn,
            occ1,
            moon_half,
            MOON_FLARE_RISE,
        )
    };
    for (mut tf, mut gt, moon, mat) in &mut sprites {
        // moon02 rides its own phase-precessed bearing; the disc + glare share the white moon's.
        let to_moon = match moon.part {
            MoonPart::Moon02 => light.moon_dir_02.normalize_or_zero(),
            _ => to_white,
        };
        if to_moon == Vec3::ZERO {
            continue;
        }
        tf.rotation = Quat::from_rotation_arc(Vec3::Z, -to_moon);
        match moon.part {
            MoonPart::Disc => {
                tf.translation = cam_pos + to_moon * dist;
                let size = if hidden {
                    0.0
                } else {
                    SUN_SIZE * 1.75 * light.moon_disc_scale
                };
                tf.scale = Vec3::splat(size * dist);
                let color = Color::srgb(tint[0], tint[1], tint[2]);
                let span = quant_span(disc_span(to_moon.y, size));
                let fade_w = benilla_assets::quantize(seed.unwrap_or(1.0), 255.0);
                benilla_assets::write_gated(
                    &mut mats,
                    &mat.0,
                    |m| {
                        m.base.base_color != color
                            || m.extension.span != span
                            || m.extension.fade.w != fade_w
                    },
                    |m| {
                        m.base.base_color = color;
                        m.extension.span = span;
                        m.extension.fade.w = fade_w;
                    },
                );
            }
            MoonPart::Moon02 => {
                // Base ×1.0, the shared size curve on moon02's own phase clock. The binary never
                // writes its colour dword — black RGB, zero alpha, so the quad is invisible in
                // clear weather (`0xce98a4`) — but the weather seed lands on its alpha
                // byte like the other four: under active weather moon02 surfaces as the
                // reference's faint dark disc (`0x6d2c74`). The span drives the per-vertex
                // fade lane (0529) so a horizon crossing keeps the faithful sub-band wedge.
                tf.translation = cam_pos + to_moon * dist;
                let size = if hidden {
                    0.0
                } else {
                    SUN_SIZE * light.moon02_disc_scale
                };
                tf.scale = Vec3::splat(size * dist);
                let span = quant_span(disc_span(to_moon.y, size));
                let fade_w = benilla_assets::quantize(seed.unwrap_or(0.0), 255.0);
                benilla_assets::write_gated(
                    &mut mats,
                    &mat.0,
                    |m| m.extension.span != span || m.extension.fade.w != fade_w,
                    |m| {
                        m.extension.span = span;
                        m.extension.fade.w = fade_w;
                    },
                );
            }
            MoonPart::Glare => {
                // `2.0 × the moon size curve` world units (`[0xce9718]=2.0`; both view-lerp
                // endpoints get the curve, so the size is f-independent) — near-sphere placement,
                // so the builder's units are the quad scale directly (same angular size as before:
                // `2.0/12` rad ≈ 1.14× the disc).
                tf.translation = cam_pos + to_moon * GLARE_DIST;
                let size = if hidden {
                    0.0
                } else {
                    2.0 * light.moon_disc_scale
                };
                tf.scale = Vec3::splat(size);
                // The view lerp's `lerp(0.1, 1, f)` (`[0xce9794]=0.1`, instant) × the slewed
                // envelope (dnCurve × horizon × visibility) — `0x6cf490`'s byte shape.
                let f = view_lerp(*cam_gt.forward(), to_moon);
                // × the weather seed (the glare alpha byte the pack modulates, `0x6d2c74`).
                let env =
                    benilla_assets::quantize((0.1 + 0.9 * f) * env30 * seed.unwrap_or(1.0), 255.0);
                let color = Color::srgba(tint[0], tint[1], tint[2], env);
                benilla_assets::write_gated(
                    &mut mats,
                    &mat.0,
                    |m| m.base.base_color != color,
                    |m| m.base.base_color = color,
                );
            }
        }
        // Propagation already ran this frame — the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

/// Camera-anchor the star dome (just inside the sky gradient dome, at `far*0.88` — over the gradient at
/// `far*0.9`, behind the sun/moon at `far*0.85`; the reference draws stars FIRST among the bodies —
/// though the sky-internal order is the `sky_order` bias ladder and world occlusion is the far depth
/// `sky_vertex.wgsl` pins, so this radius decides neither, only the dots' screen scale) and
/// drive its **global alpha** from the verified star curve. The reference quantises the curve to the
/// model-global byte `trunc(curve·254 + 1)` and SKIPS the draw below 2 (`0x6d1b50`/`0x7e6120`, decision
/// 0485); each patch then multiplies its own authored transparency weight ([`StarDome::weight`]) under
/// the global fade. Hidden (alpha 0) with the sky dome.
pub(super) fn follow_stars(
    cam: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    light: Res<WowLighting>,
    debug: Res<DebugState>,
    mut star_mats: ResMut<Assets<StarMaterial>>,
    mut stars: Query<
        (
            &mut Transform,
            &mut GlobalTransform,
            &MeshMaterial3d<StarMaterial>,
            &StarDome,
        ),
        Without<WorldCamera>,
    >,
) {
    let Some((cam_gt, proj)) = cam.iter().next() else {
        return;
    };
    let far = match proj {
        Projection::Perspective(p) => p.far,
        _ => 3000.0,
    };
    // The byte-exact global fade: byte = trunc(curve·254 + 1), skipped when < 2, else alpha = byte/255.
    let byte = (light.star_alpha * 254.0 + 1.0).trunc();
    let global = if debug.lighting.disable_sky_dome || byte < 2.0 {
        0.0
    } else {
        byte / 255.0
    };
    for (mut tf, mut gt, mat, dome) in &mut stars {
        tf.translation = cam_gt.translation();
        tf.scale = Vec3::splat(far * 0.88);
        // Propagation already ran this frame — the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
        // White dots; global fade × the patch's authored weight, multiplied onto the texture's
        // per-dot alpha, then blended gamma-correctly by star.wgsl. The vertex alpha is a byte
        // in the reference, so the product gates at 1/255 like the rest of the family.
        let color = Color::srgba(
            1.0,
            1.0,
            1.0,
            benilla_assets::quantize(global * dome.weight, 255.0),
        );
        benilla_assets::write_gated(
            &mut star_mats,
            &mat.0,
            |m| m.base.base_color != color,
            |m| m.base.base_color = color,
        );
    }
}

#[cfg(test)]
mod glare_fade_tests {
    use super::submersion_glare_fade;

    /// The 10-yard linear ramp, and — the half that matters most — the exact identity when dry.
    /// A dry camera reads `depth = 0`, so this multiplies every glare in the game by 1.0; if that
    /// were ever not exact, the feature would be a global dimmer rather than an underwater one.
    #[test]
    fn the_glare_fades_linearly_over_ten_yards() {
        assert_eq!(submersion_glare_fade(0.0), 1.0, "dry is the exact identity");
        assert_eq!(submersion_glare_fade(5.0), 0.5);
        assert_eq!(submersion_glare_fade(10.0), 0.0);
        assert_eq!(submersion_glare_fade(200.0), 0.0, "clamped, never negative");
        // Linear in between, and monotone all the way down.
        let mut prev = 1.0;
        for i in 1..=10 {
            let f = submersion_glare_fade(i as f32);
            assert!(f <= prev && f >= 0.0);
            assert!((f - (1.0 - i as f32 * 0.1)).abs() < 1e-6);
            prev = f;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev_state::DebugState;
    use crate::wmo_portal::CameraInteriorClaim;

    /// The moonrise-halo lag regression (decision 0504): with the camera moving every frame, the
    /// glare quad's rendered pose (`GlobalTransform`) must be placed from the SAME frame's camera —
    /// zero error. The old plain-`Update` wiring read the camera's `GlobalTransform` (synced only in
    /// `PostUpdate`), placing the quad from the *previous* frame's camera: this exact harness
    /// measured the error at exactly one frame of camera motion (1.0 unit/frame here; `v·dt` live —
    /// ~1% of the glare's 12-unit distance per 60 fps frame at run speed), which on screen was the
    /// director's halo swim + toward/away size pump. Mirrors `SunPlugin`'s registration: PostUpdate,
    /// the post-propagation [`crate::billboard::BillboardPlace`] slot, direct `GlobalTransform` write.
    #[test]
    fn glare_rides_the_same_frame_camera_while_moving() {
        let dir = Vec3::new(0.0, 0.5, -1.0).normalize();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin));
        app.init_resource::<TerrainStreamer>();
        app.init_resource::<CameraInteriorClaim>();
        app.init_resource::<CloudCoverage>();
        app.init_resource::<DebugState>();
        // Dry (depth 0) — this fixture is about the glare tracking the camera within one frame,
        // and the submersion fade is the exact identity there, so it cannot perturb the assertion.
        app.init_resource::<crate::liquid::SubmergedEye>();
        app.insert_resource(WowLighting {
            moon_dir_white: dir,
            moon_disc_scale: 1.0,
            celestial_tint: [1.0, 1.0, 1.0],
            ..default()
        });
        app.insert_resource(Assets::<AdtTile>::default());
        let mut mats = Assets::<CelestialMaterial>::default();
        let mat = mats.add(CelestialMaterial {
            base: StandardMaterial::default(),
            extension: super::super::materials::CelestialExt::default(),
        });
        app.insert_resource(mats);
        app.world_mut().spawn((
            crate::view::WorldCamera,
            Transform::default(),
            GlobalTransform::default(),
            Projection::Perspective(PerspectiveProjection::default()),
        ));
        let glare = app
            .world_mut()
            .spawn((
                MoonSprite {
                    part: MoonPart::Glare,
                },
                Transform::default(),
                GlobalTransform::default(),
                MeshMaterial3d(mat),
            ))
            .id();
        // The camera mover (stands in for seat_camera): +1 unit along the moon ray per Update.
        app.add_systems(
            Update,
            move |mut q: Query<&mut Transform, With<crate::view::WorldCamera>>| {
                q.single_mut().unwrap().translation += dir * 1.0;
            },
        );
        // The plugin's wiring: post-propagation placement in the BillboardPlace slot.
        app.configure_sets(
            PostUpdate,
            crate::billboard::BillboardPlace.after(bevy::transform::TransformSystems::Propagate),
        );
        app.add_systems(
            PostUpdate,
            follow_moons.in_set(crate::billboard::BillboardPlace),
        );
        app.update();
        app.update();
        app.update();
        let cam_now = app
            .world_mut()
            .query_filtered::<&GlobalTransform, With<crate::view::WorldCamera>>()
            .single(app.world())
            .unwrap()
            .translation();
        let glare_pos = app
            .world()
            .entity(glare)
            .get::<GlobalTransform>()
            .unwrap()
            .translation();
        let want = cam_now + dir * GLARE_DIST;
        let err = (glare_pos - want).length();
        assert!(
            err < 1e-4,
            "glare must be placed from the render-frame camera: cam_now {cam_now:?} glare \
             {glare_pos:?} want {want:?} error {err} (the old Update wiring measured exactly 1.0 \
             frame of camera motion here)"
        );
    }

    /// Height oracle: a flat plain at WoW z = 0, with a 30-unit ridge crest everywhere more than
    /// 300 horizontal units out.
    fn ridge(p: Vec3) -> Option<f32> {
        Some(if Vec2::new(p.x, p.z).length() > 300.0 {
            30.0
        } else {
            0.0
        })
    }

    /// The `[glare+0x30]` slew (`0x6cf59b`–`0x6cf5ea`, 0508): asymmetric LINEAR rates — rise capped
    /// per body, fall shared and slower — and it clamps to the target instead of overshooting.
    #[test]
    fn flare_slew_is_asymmetric_linear_and_never_overshoots() {
        // Rising at the sun's 4.0/s: 0.1 s covers exactly 0.4.
        assert!((flare_slew(0.0, 1.0, SUN_FLARE_RISE, FLARE_FALL, 0.1) - 0.4).abs() < 1e-6);
        // Falling is the shared slower 1.5152/s regardless of the rise rate passed.
        let fell = flare_slew(1.0, 0.0, SUN_FLARE_RISE, FLARE_FALL, 0.1);
        assert!((fell - (1.0 - FLARE_FALL * 0.1)).abs() < 1e-6);
        // Never overshoots: a big step lands exactly on the target.
        assert_eq!(flare_slew(0.9, 1.0, SUN_FLARE_RISE, FLARE_FALL, 1.0), 1.0);
        assert_eq!(flare_slew(0.1, 0.0, SUN_FLARE_RISE, FLARE_FALL, 1.0), 0.0);
        // The moon rises slower than the sun (100/33 ≈ 3.03/s vs 4.0/s).
        assert!(MOON_FLARE_RISE < SUN_FLARE_RISE && (MOON_FLARE_RISE - 100.0 / 33.0).abs() < 1e-4);
    }

    #[test]
    fn flare_ray_blocked_by_a_ridge_but_not_by_open_sky() {
        let cam = Vec3::new(0.0, 2.0, 0.0); // eye 2 units above the plain
        let low = Vec3::new(1.0, 0.01, 0.0).normalize(); // grazes under the 30-unit crest
        let high = Vec3::new(1.0, 1.0, 0.0).normalize(); // 45° — well over it
        assert!(!flare_ray_clear(ridge, cam, low));
        assert!(flare_ray_clear(ridge, cam, high));
        // No terrain in any column (open ocean / unstreamed) is never an occluder.
        assert!(flare_ray_clear(|_| None, cam, low));
    }

    #[test]
    fn flare_fraction_is_partial_on_a_half_hidden_disc() {
        // The ridge crest (30 units at 300+ out, eye at 2) blocks rays under ≈5.4° of elevation.
        let cam = Vec3::new(0.0, 2.0, 0.0);
        let dir = |deg: f32| {
            let e = deg.to_radians();
            Vec3::new(e.cos(), e.sin(), 0.0)
        };
        let half = 2.0_f32.to_radians();
        // Fully clear well above the crest line; fully blocked well under it.
        assert_eq!(flare_visible_fraction(ridge, cam, dir(12.0), half), 1.0);
        assert_eq!(flare_visible_fraction(ridge, cam, dir(1.0), half), 0.0);
        // A disc straddling the crest line is PARTIALLY visible — the fractional occ3 law
        // (`0x7e5220`: half-hidden sun → ~half flare), not the old all-or-nothing gate.
        let frac = flare_visible_fraction(ridge, cam, dir(5.4), half);
        assert!(
            (0.25..=0.75).contains(&frac),
            "straddling the crest: expected a partial fraction, got {frac}"
        );
    }

    /// The 1436 round-robin drip is the full march, just spread over frames: priming reads the
    /// exact full-march fraction, and a whole 2-cells-per-step cycle over an unchanged scene
    /// lands back on it (the mask never reads a partial window — every step reads all 16 bits).
    #[test]
    fn flare_mask_drip_converges_to_the_full_march() {
        let cam = Vec3::new(0.0, 2.0, 0.0);
        let e = 5.4_f32.to_radians();
        let dir = Vec3::new(e.cos(), e.sin(), 0.0); // straddles the ridge crest — a partial mask
        let half = 2.0_f32.to_radians();
        let full = flare_visible_fraction(ridge, cam, dir, half);
        assert!((0.0..1.0).contains(&full), "oracle must be partial: {full}");
        let (mut mask, mut cursor) = (0u16, 0u32);
        let march = |c| flare_cell_clear(ridge, cam, dir, half, c);
        let primed = flare_mask_update(&mut mask, &mut cursor, FLARE_CELLS, march);
        assert_eq!(primed, full, "priming = the full march in one call");
        let mut last = primed;
        for _ in 0..(FLARE_CELLS / FLARE_RAYS_PER_FRAME) {
            last = flare_mask_update(&mut mask, &mut cursor, FLARE_RAYS_PER_FRAME, march);
        }
        assert_eq!(last, full, "a full drip cycle re-derives the same fraction");
        assert_eq!(cursor, 0, "the cursor wrapped exactly once");
    }

    #[test]
    fn disc_span_feeds_the_per_vertex_fade_regimes() {
        // Elevated (every fixture's regime): the whole span sits far above the ~1.9° fade band
        // (sin ≈ 1/30), so the shader's vertex rule returns the colour alpha at both edges — the
        // disc renders exactly `a × a_disc`, unchanged from before 0529.
        let up = disc_span(30_f32.to_radians().sin(), 0.0833);
        assert!(up.x > 1.0 / 30.0 && up.y > up.x, "elevated: above the band");
        // A 2×-sized setting sun with its centre 2° up straddles the horizon: bottom edge below
        // 0 (clipped at the cut, alpha 0 there), top edge above the band (alpha = colour) — the
        // whole-disc melt gradient.
        let setting = disc_span(2_f32.to_radians().sin(), 0.1667);
        assert!(setting.x < 0.0, "setting: bottom edge under the horizon");
        assert!(setting.y > 1.0 / 30.0, "setting: top edge above the band");
        // The span brackets the body symmetrically in elevation.
        let mid = disc_span(0.0, 0.1);
        assert!((mid.x + mid.y).abs() < 1e-6);
    }

    /// The weather celestial-alpha seed (`0x6d2c74`): gated `bcc > 0` — clear weather leaves the
    /// broadcast alphas alone — and byte-quantized `floor(255·(1−bcc))/255` under weather.
    #[test]
    fn weather_seed_is_gated_and_byte_quantized() {
        assert_eq!(celestial_alpha_seed(0.0), None);
        // Full storm: the seed zeroes every body alpha.
        assert_eq!(celestial_alpha_seed(1.0), Some(0.0));
        // Half density: floor(255·0.5) = 127 — the byte floor, not a smooth 0.5.
        let half = celestial_alpha_seed(0.5).unwrap();
        assert!((half - 127.0 / 255.0).abs() < 1e-6);
    }
}
