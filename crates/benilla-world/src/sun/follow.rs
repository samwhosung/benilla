//! Per-frame placement of the celestial layer: discs, glares and the star dome face the camera at
//! their world directions, tinted by the celestial diffuse. The glares sit on the reference's near
//! sphere (`cam + 12·dir`), clipped per pixel by depth (its `[0.995, 1.0]` slice, `0x7e5a0c`) and
//! dimmed by the [`FlareGate`] envelope. These run after propagation and write `GlobalTransform`,
//! so they place from this frame's camera: a frame of lag is ~1% of the glare's distance.

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

/// Every disc and glare is a unit quad at radius 12 (`[0x80c4e8]`), 4.77° per world unit, so a
/// billboard at distance `d` scales by `0.0833 × size × d`.
const SUN_SIZE: f32 = 0.0833;

/// The glares sit on the reference's near sphere, `cam + 12·dir` (`[0x80c4e8]`), in world units,
/// at the sky's far depth pin (`sky_vertex.wgsl`), drawn last ([`crate::sky_order::GLARE_BIAS`]).
const GLARE_DIST: f32 = 12.0;

/// The occlusion march: 48 quadratic samples to just inside the far plane, across the ADT ring and
/// the WDL beyond, at most ~117 units apart at the far end.
const FLARE_RAY_SAMPLES: u32 = 48;
const FLARE_RAY_RANGE: f32 = 2800.0;

/// The envelope's linear slew rates per second (`[glare+0x28]`/`[+0x2c]`, applied by `0x6cf5ea`):
/// the sun rises at 4.0, the moon at 100/33, and both fall at 50/33.
const SUN_FLARE_RISE: f32 = 4.0; // [0xce97d0]
const MOON_FLARE_RISE: f32 = f32::from_bits(0x4041_f07c); // [0xce9720] ≈ 3.0303
const FLARE_FALL: f32 = f32::from_bits(0x3fc1_f07c); // [0xce97d4]/[0xce9724] ≈ 1.5152

/// A body's sprite rows; every disc and glare rides [`CelestialMaterial`].
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

/// The lens flare's view lerp (`0x6cf490`), growing as the view swings onto the body.
fn view_lerp(cam_forward: Vec3, to_body: Vec3) -> f32 {
    ((cam_forward.dot(to_body) - 0.7) / 0.3).clamp(0.0, 1.0)
}

/// A smoothstep to 0 at the horizon for the glares, which skip the disc clip: our stand-in for the
/// reference's occlusion query seeing a set body sink, a factor of the slew target (`0x6cf490`).
fn horizon_gate(to_body: Vec3) -> f32 {
    let t = (to_body.y / 0.035).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One step of the `[glare+0x30]` slew toward `target` (`0x6cf59b`-`0x6cf5ea`).
fn flare_slew(current: f32, target: f32, rise: f32, fall: f32, dt: f32) -> f32 {
    current + (target - current).clamp(-fall * dt, rise * dt)
}

/// The disc quad's bottom and top edges in sin-elevation, `elevation ∓ atan(size / 2)`, for the
/// shader's per-vertex fade; `size` is the quad's scale over its distance.
pub(super) fn disc_span(dir_y: f32, size: f32) -> Vec4 {
    let elev = dir_y.clamp(-1.0, 1.0).asin();
    let half = (0.5 * size).atan();
    Vec4::new((elev - half).sin(), (elev + half).sin(), 0.0, 0.0)
}

/// A [`disc_span`] in 1/4096 sin-elevation steps, so its drift does not defeat the write gate.
fn quant_span(v: Vec4) -> Vec4 {
    Vec4::new(
        benilla_assets::quantize(v.x, 4096.0),
        benilla_assets::quantize(v.y, 4096.0),
        v.z,
        v.w,
    )
}

/// No terrain column blocks the ray within [`FLARE_RAY_RANGE`]; the samples densify near the
/// camera, and Bevy `y` is WoW `z`, so the ray's height is `p.y`.
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

/// The occlusion grid's side: 4×4 rays, whose 1/16 steps the envelope slew smooths.
const FLARE_FRACTION_GRID: u32 = 4;

const FLARE_CELLS: u32 = FLARE_FRACTION_GRID * FLARE_FRACTION_GRID;

/// Cells re-marched per frame: the grid refreshes in 8 frames, inside both slew time constants.
const FLARE_RAYS_PER_FRAME: u32 = 2;

/// The visible fraction of the body's quad against terrain, a ray grid over its extent; the
/// reference takes `visiblePixels / projectedArea` of the disc's quad from a GPU occlusion query
/// (`0x7e5220`), WMOs and doodads included. The tests' oracle for the live drip.
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

/// Whether one cell's centre ray over the quad's `[−half, +half]²` footprint is clear.
fn flare_cell_clear(
    height_under: impl Fn(Vec3) -> Option<f32>,
    cam_pos: Vec3,
    dir: Vec3,
    half: f32,
    cell: u32,
) -> bool {
    // Any orthonormal frame across the quad works: the probe is symmetric.
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let right = if right == Vec3::ZERO { Vec3::X } else { right };
    let up = right.cross(dir);
    let n = FLARE_FRACTION_GRID;
    let u = ((cell / n) as f32 + 0.5) / n as f32 - 0.5;
    let v = ((cell % n) as f32 + 0.5) / n as f32 - 0.5;
    let d = (dir + right * (u * 2.0 * half) + up * (v * 2.0 * half)).normalize();
    flare_ray_clear(&height_under, cam_pos, d)
}

/// Re-march `cells` cells from `cursor`, then read the whole mask (bit = clear) as the fraction.
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

/// The flare envelope, the reference's `[glare+0x30]`, seeded at 0 like its `.bss` and slewed
/// toward dnCurve × horizon × cloud cover `occ1` (`0x6cf7b0`, `0x6cf7d0`) × the quad's visible
/// fraction, 0 in a WMO interior; a 0 target skips the probe (`0x6cf58c`). Not modelled: the
/// reference's `occ2` and lens-flare-slot gate `1−V`, both 1.0 outdoors without scene-light flares.
#[derive(SystemParam)]
pub(super) struct FlareGate<'w, 's> {
    time: Res<'w, Time>,
    streamer: Res<'w, TerrainStreamer>,
    adt_tiles: Res<'w, Assets<AdtTile>>,
    /// The whole-map WDL heightfield, the march's far leg beyond the ADT ring.
    wdl: Option<Res<'w, WdlStreamer>>,
    camera_interior: Res<'w, CameraInteriorClaim>,
    /// For the glare's own submersion fade: its pass (`0x6cf490` ← `0x6d48c0` ← `0x483740`) is
    /// outside the sky pass (`0x6d4940` in `0x681070`) that `0x6812a4` skips underwater.
    submerged: Res<'w, crate::liquid::SubmergedEye>,
    /// The cloud coverage `occ1` samples; as in the reference, the visible clouds draw from it too.
    clouds: Res<'w, CloudCoverage>,
    env: Local<'s, f32>,
    /// Cached cell verdicts (bit = clear); `primed` falls when the probe is skipped.
    mask: Local<'s, u16>,
    cursor: Local<'s, u32>,
    primed: Local<'s, bool>,
}

/// The glares' submersion fade, a 10-yard ramp on `liquidSurfaceHeight − probeZ`, reached through
/// vtable slot 4 (`0x6cf4fe call [edx+0x10]` → `0x6cf800`, from `0x6d48cf`/`0x6d48ea`); the discs
/// have none, as their whole sky pass is skipped.
fn submersion_glare_fade(depth: f32) -> f32 {
    1.0 - (depth * 0.1).clamp(0.0, 1.0)
}

/// The weather alpha seed (`0x6d2c74`): with `bcc > 0` the recompute writes `floor(255·(1−bcc))`
/// over all five body alpha bytes, moon02's included; clear weather leaves them alone.
fn celestial_alpha_seed(bcc: f32) -> Option<f32> {
    (bcc > 0.0).then(|| (255.0 * (1.0 - bcc.min(1.0))).floor() / 255.0)
}

impl FlareGate<'_, '_> {
    /// The envelope along `dir`, `half` the quad's angular half-size. The submersion fade is inside
    /// the slew, so surfacing ramps the glare back in: the target `[glare+0x34]` (reset at
    /// `0x6cf499`) takes it at `0x6cf501`/`0x6cf504`, before the limiter reads it at `0x6cf59b`.
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
            // Resident ADT terrain first, the whole-map WDL surface elsewhere.
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
            // Unprimed, march every cell; primed, `FLARE_RAYS_PER_FRAME` of them.
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

/// Place the sun's disc and glare, tinted by the `0x6d2260` broadcast. The disc is the unit quad ×
/// the day curve (`0xce8cac`: 2× at the horizon, 1× at midday); the glare is `0x6cf490`'s flare,
/// `lerp(3, 20, f)` units at `lerp(0.5, 1, f)` × the envelope, a day flare (07:30 to 19:30).
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
    // The visible sun, which rises and sets, not the near-fixed lighting sun.
    let to_light = light.celestial_dir.normalize_or_zero();
    if to_light == Vec3::ZERO {
        return;
    }
    let far = match proj {
        Projection::Perspective(p) => p.far,
        _ => 3000.0,
    };
    let dist = far * 0.85; // inside the far plane; occlusion is the far-depth pin
    let cam_pos = cam_gt.translation();
    // Billboard: the quad's +Z normal faces back toward the camera (= −to_light).
    let rot = Quat::from_rotation_arc(Vec3::Z, -to_light);
    let hidden = debug.lighting.disable_sky_dome; // part of the sky, hidden with the dome
    let f = view_lerp(*cam_gt.forward(), to_light);
    // Byte-quantized like the reference's colour lanes, so the write gates fire on visible change.
    let tint = benilla_assets::quant255(light.celestial_tint);
    let seed = celestial_alpha_seed(light.storm_bcc);
    // The occlusion probe (`0x7e5220`) covers the disc's own quad: its angular half-size.
    let sun_half = (0.5 * SUN_SIZE * light.sun_disc_scale).atan();
    // occ1: the cloud cover at the glare point, `cam + 12·dir` (`0x6cf7b0`), dims the flare.
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
                // Above-band opacity: the 0xFF broadcast, or the weather seed.
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
                // `lerp(3, 20, f)` world units (`[0xce9838]`/`[0xce983c]`).
                let units = 3.0 + 17.0 * f;
                tf.scale = Vec3::splat(if hidden { 0.0 } else { units });
                // Alpha is `lerp(0.5, 1, f)` (`[glare+0x9c]`) × the envelope × the weather seed,
                // `0x6cf490`'s `[+0x1b] = floor(255·lerp·[+0x30])` on the seed byte.
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
        // Propagation already ran, so the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

/// Place the white moon (azimuth 45°) and its glare, tinted by the same broadcast, and moon02. The
/// disc is the unit quad × 1.75 × the size curve (`0xce8c8c`: 1.5× at the horizon, 1× overhead);
/// the glare is 2.0 × that curve at `lerp(0.1, 1, f)` × the envelope, dark until 22:45.
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
    let tint = benilla_assets::quant255(light.celestial_tint);
    let seed = celestial_alpha_seed(light.storm_bcc);
    // One envelope, along the white moon's ray, the only glare; its dnCurve is 0 until 22:45.
    let to_white = light.moon_dir_white.normalize_or_zero();
    let env30 = if to_white == Vec3::ZERO {
        0.0
    } else {
        // The occlusion probe (`0x7e5220`) covers the white moon's own disc quad.
        let moon_half = (0.5 * SUN_SIZE * 1.75 * light.moon_disc_scale).atan();
        // occ1: the tent `1−|2(R−0.5)|` (`0x6cf7d0`), 0 in clear sky and under full cover,
        // peaking on a wisp.
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
        // moon02 rides its own phase-precessed bearing; the disc and glare share the white moon's.
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
                // Base ×1.0 on its own phase clock. Its colour (`0xce98a4`) is never written, black
                // at alpha 0, but the weather seed lands on its alpha byte, a faint dark disc
                // (`0x6d2c74`); the span still drives its wedge in the horizon band.
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
                // `2.0 × the size curve` (`[0xce9718]`) at both lerp ends, whatever `f`.
                tf.translation = cam_pos + to_moon * GLARE_DIST;
                let size = if hidden {
                    0.0
                } else {
                    2.0 * light.moon_disc_scale
                };
                tf.scale = Vec3::splat(size);
                // `lerp(0.1, 1, f)` (`[0xce9794]`) × the envelope, `0x6cf490`'s byte shape.
                let f = view_lerp(*cam_gt.forward(), to_moon);
                // × the weather seed on the glare's alpha byte (`0x6d2c74`).
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
        // Propagation already ran, so the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
    }
}

/// Anchor the star dome and fade it by the model-global byte `trunc(curve·254 + 1)`, the draw
/// skipped below 2 (`0x6d1b50`/`0x7e6120`), times each patch's [`StarDome::weight`].
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
    let byte = (light.star_alpha * 254.0 + 1.0).trunc();
    let global = if debug.lighting.disable_sky_dome || byte < 2.0 {
        0.0
    } else {
        byte / 255.0
    };
    for (mut tf, mut gt, mat, dome) in &mut stars {
        tf.translation = cam_gt.translation();
        tf.scale = Vec3::splat(far * 0.88);
        // Propagation already ran, so the direct global write is what renders.
        *gt = GlobalTransform::from(*tf);
        // The fade × the patch weight, in 1/255 steps, as the reference's vertex alpha is a byte.
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

    /// Dry must be the exact identity, or the fade would dim every glare in the game.
    #[test]
    fn the_glare_fades_linearly_over_ten_yards() {
        assert_eq!(submersion_glare_fade(0.0), 1.0, "dry is the exact identity");
        assert_eq!(submersion_glare_fade(5.0), 0.5);
        assert_eq!(submersion_glare_fade(10.0), 0.0);
        assert_eq!(submersion_glare_fade(200.0), 0.0, "clamped, never negative");
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

    /// The glare is placed from the moving camera's same-frame pose, under `SunPlugin`'s wiring.
    #[test]
    fn glare_rides_the_same_frame_camera_while_moving() {
        let dir = Vec3::new(0.0, 0.5, -1.0).normalize();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::transform::TransformPlugin));
        app.init_resource::<TerrainStreamer>();
        app.init_resource::<CameraInteriorClaim>();
        app.init_resource::<CloudCoverage>();
        app.init_resource::<DebugState>();
        // Dry (depth 0), where the submersion fade is the exact identity.
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
        let high = Vec3::new(1.0, 1.0, 0.0).normalize(); // 45°, well over it
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
        assert_eq!(flare_visible_fraction(ridge, cam, dir(12.0), half), 1.0);
        assert_eq!(flare_visible_fraction(ridge, cam, dir(1.0), half), 0.0);
        // Straddling the crest, partly visible (`0x7e5220`: half hidden, about half the flare).
        let frac = flare_visible_fraction(ridge, cam, dir(5.4), half);
        assert!(
            (0.25..=0.75).contains(&frac),
            "straddling the crest: expected a partial fraction, got {frac}"
        );
    }

    /// Priming reads the full march's fraction, and a whole drip cycle over an unchanged scene
    /// lands back on it.
    #[test]
    fn flare_mask_drip_converges_to_the_full_march() {
        let cam = Vec3::new(0.0, 2.0, 0.0);
        let e = 5.4_f32.to_radians();
        let dir = Vec3::new(e.cos(), e.sin(), 0.0); // straddles the ridge crest: a partial mask
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
        // Elevated: above the ~1.9° fade band (sin ≈ 1/30), so both edges take the colour alpha.
        let up = disc_span(30_f32.to_radians().sin(), 0.0833);
        assert!(up.x > 1.0 / 30.0 && up.y > up.x, "elevated: above the band");
        // A 2× setting sun 2° up: bottom edge below the horizon, top edge above the band.
        let setting = disc_span(2_f32.to_radians().sin(), 0.1667);
        assert!(setting.x < 0.0, "setting: bottom edge under the horizon");
        assert!(setting.y > 1.0 / 30.0, "setting: top edge above the band");
        // The span brackets the body symmetrically in elevation.
        let mid = disc_span(0.0, 0.1);
        assert!((mid.x + mid.y).abs() < 1e-6);
    }

    #[test]
    fn weather_seed_is_gated_and_byte_quantized() {
        assert_eq!(celestial_alpha_seed(0.0), None);
        // Full storm: the seed zeroes every body alpha.
        assert_eq!(celestial_alpha_seed(1.0), Some(0.0));
        // Half density: floor(255·0.5) = 127, the byte floor, not a smooth 0.5.
        let half = celestial_alpha_seed(0.5).unwrap();
        assert!((half - 127.0 / 255.0).abs() < 1e-6);
    }
}
