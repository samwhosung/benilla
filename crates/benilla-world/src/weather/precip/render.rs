//! Streak, patter and flake geometry pushed each frame from the live pools onto the shared
//! effect stream, in world space except the flakes, which are camera-relative.

// Explicit, not `bevy::prelude::*`: the parent's prelude glob arrives through `use super::*`, and
// two globs of the same names draw an unused-import warning on non-macOS builds.
use bevy::math::{Quat, Vec3};

use crate::particles::buffer::EffectVertex;

use super::pool::{Drop, Patter};
use super::*;

/// Falling-drop streaks: one fixed-size triangle per drop, tilted at the apex only.
pub(super) fn push_streaks(out: &mut Vec<EffectVertex>, drops: &[Drop], tilt: Quat, cam: Vec3) {
    let white = [1.0, 1.0, 1.0, 1.0];
    for d in drops.iter().take(POOL) {
        let anti_vel = -d.vel.normalize_or(Vec3::NEG_Y);
        let to_cam = (cam - d.pos).normalize_or(Vec3::X);
        let right = to_cam.cross(anti_vel).normalize_or(Vec3::X) * STREAK_HALF_W;
        let apex = d.pos + tilt * (anti_vel * STREAK_TAIL);
        for (pos, uv) in [
            (d.pos - right, [0.0, 1.0]),
            (d.pos + right, [1.0, 1.0]),
            (apex, [0.5, 0.0]),
        ] {
            out.push(EffectVertex {
                pos: pos.to_array(),
                uv,
                color: white,
            });
        }
    }
}

/// Ground patters: one camera-facing triangle per splash, stepping across its atlas row.
pub(super) fn push_patters(
    out: &mut Vec<EffectVertex>,
    patters: &[Patter],
    cam_right: Vec3,
    cam_up: Vec3,
) {
    let right = cam_right * PATTER_RIGHT;
    let up = cam_up * PATTER_UP;
    for p in patters.iter().take(GROUND_CAP) {
        let t = (p.age / PATTER_LIFE).clamp(0.0, 1.0);
        let frame = ((t * 4.0) as u32).min(3) as f32;
        let (u0, v0) = (frame * 0.25, f32::from(p.variant) * 0.25);
        // No vertex alpha: the atlas's 4 frames are the animation, its grey-128 ground neutral.
        // The texcoord law (`0x675ac0`): base-left, apex, base-right.
        for (pos, uv) in [
            (p.pos - right, [u0, v0 + 0.25]),
            (p.pos + up, [u0 + 0.125, v0 + 0.043]),
            (p.pos + right, [u0 + 0.25, v0 + 0.25]),
        ] {
            out.push(EffectVertex {
                pos: pos.to_array(),
                uv,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
    }
}

/// The camera terms [`push_flakes`] needs to turn a pixel point size into a world-space quad.
pub(super) struct FlakeView {
    pub(super) eye: Vec3,
    /// For a flake's view depth, which the pixel-to-world conversion keys on; the size law keys
    /// on the radial `|flake − eye|`, which differs off-axis.
    pub(super) forward: Vec3,
    pub(super) right: Vec3,
    pub(super) up: Vec3,
    /// `tan(fovY/2) / SNOW_PX_REF_HEIGHT`, world units per era pixel per unit of view depth: a
    /// `px` sprite at depth `z` wants half-extent `px·z·this`. Never the live viewport height.
    pub(super) world_per_px: f32,
}

/// Snow flakes: the point-sprite leg `0x678610` as screen-aligned quads, since wgpu has no point
/// size. Per `snowpoint.bls`: white, alpha `clamp01(t − f1)` while falling and
/// `clamp01(1 − 4·(t − f2))` settled, the whole texture per flake (`GL_COORD_REPLACE`). There is
/// no per-flake size: the 32-byte record has no spare field and none of the 5 spawn draws is one.
pub(super) fn push_flakes(
    out: &mut Vec<EffectVertex>,
    drops: &[Drop],
    settled: &[Patter],
    view: &FlakeView,
) {
    let mut sprite = |center: Vec3, alpha: f32| {
        // Camera-relative (the draw sets `EffectDrawSpec::cam_relative`): a near flake's ~1.6 mm
        // half-extent is 3 f32 ULPs at 5600 yd, so absolute coordinates lose it and flicker.
        let to_flake = center - view.eye;
        let z = to_flake.dot(view.forward);
        if z <= 0.0 {
            return; // behind the eye, where the pixel-to-world map is undefined
        }
        // The size law off the radial distance, then pixels to world at this view depth.
        let px = (SNOW_PX_AT_EYE * (1.0 - SNOW_PX_FALLOFF * to_flake.length()).clamp(0.0, 1.0))
            .max(SNOW_PX_MIN);
        let half = px * z * view.world_per_px;
        let r = view.right * half;
        let u = view.up * half;
        // Perimeter order (bl, br, tr, tl) for the stream's quad-index pattern.
        for (pos, uv) in [
            (to_flake - r - u, [0.0, 1.0]),
            (to_flake + r - u, [1.0, 1.0]),
            (to_flake + r + u, [1.0, 0.0]),
            (to_flake - r + u, [0.0, 0.0]),
        ] {
            out.push(EffectVertex {
                pos: pos.to_array(),
                uv,
                color: [1.0, 1.0, 1.0, alpha],
            });
        }
    };
    for d in drops.iter().take(POOL) {
        sprite(d.pos, (d.age / SNOW_FADE_IN).clamp(0.0, 1.0));
    }
    for s in settled.iter().take(GROUND_CAP) {
        sprite(s.pos, 1.0 - (s.age / SNOW_SETTLE_LIFE).clamp(0.0, 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOVY: f32 = std::f32::consts::FRAC_PI_4;

    /// A camera at `eye` looking down −Z, 45° vertical fov, with the production `world_per_px`.
    fn view_at(eye: Vec3) -> FlakeView {
        FlakeView {
            eye,
            forward: Vec3::NEG_Z,
            right: Vec3::X,
            up: Vec3::Y,
            world_per_px: (FOVY * 0.5).tan() / SNOW_PX_REF_HEIGHT,
        }
    }

    fn view() -> FlakeView {
        view_at(Vec3::ZERO)
    }

    /// `snowpoint.bls`'s law, restated independently of the implementation.
    fn reference_px(d: f32) -> f32 {
        (14.0 * (1.0 - 0.02 * d).clamp(0.0, 1.0)).max(1.0)
    }

    fn flake(pos: Vec3, age: f32) -> Drop {
        Drop {
            pos,
            vel: Vec3::NEG_Y,
            land_y: -1000.0,
            cell: (0, 0),
            age,
        }
    }

    /// A flake's footprint in pixels at a render height, measured by projecting the pushed quad.
    fn footprint_px(d: f32, render_h: f32) -> f32 {
        // The RH projection Bevy builds: clip.w = −z_view, so a flake at z_view = −d has w = d.
        let proj = Mat4::perspective_rh(FOVY, 16.0 / 9.0, 0.1, 1000.0);
        let window_y = |p: Vec3| {
            let clip = proj * p.extend(1.0);
            (clip.y / clip.w + 1.0) * 0.5 * render_h
        };
        let mut out = Vec::new();
        push_flakes(
            &mut out,
            &[flake(Vec3::new(0.0, 0.0, -d), 5.0)],
            &[],
            &view(),
        );
        assert_eq!(out.len(), 4, "one flake = one quad");
        // Perimeter order (bl, br, tr, tl): corner 0 is the bottom edge, corner 3 the top.
        window_y(Vec3::from(out[3].pos)) - window_y(Vec3::from(out[0].pos))
    }

    /// At the era's screen height the footprint is the reference's pixel count exactly.
    #[test]
    fn flake_covers_the_reference_pixel_footprint() {
        for d in [1.0f32, 5.0, 7.14, 20.0, 46.43, 60.0, 100.0] {
            let px = footprint_px(d, SNOW_PX_REF_HEIGHT);
            let want = reference_px(d);
            assert!(
                (px - want).abs() < 0.01,
                "at {d} yd the flake covers {px:.3} px, the reference gives {want:.3}"
            );
        }
    }

    /// On a taller screen a flake keeps the same share of the screen, not the same pixels.
    #[test]
    fn the_flake_holds_its_angle_across_resolutions() {
        for d in [1.0f32, 12.0, 30.0, 60.0] {
            let era = footprint_px(d, SNOW_PX_REF_HEIGHT) / SNOW_PX_REF_HEIGHT;
            for h in [480.0f32, 800.0, 1080.0, 1440.0, 2144.0, 4320.0] {
                let share = footprint_px(d, h) / h;
                assert!(
                    (share - era).abs() < 1e-6,
                    "at {d} yd a flake covers {:.4}% of a {h}-px screen but {:.4}% of the era's \
                     — the size law has gone back to being resolution-dependent",
                    share * 100.0,
                    era * 100.0,
                );
            }
            // The absolute pixel count scales with the height.
            let tall = footprint_px(d, 2144.0);
            let want = reference_px(d) * 2144.0 / SNOW_PX_REF_HEIGHT;
            assert!(
                (tall - want).abs() < 0.01,
                "at {d} yd: {tall:.2} vs {want:.2}"
            );
        }
    }

    /// 14 px at the eye, the 1 px floor from 46.43 yd, linear between: not a `1/d` size.
    #[test]
    fn point_size_is_linear_in_distance_not_inverse() {
        assert!((reference_px(0.0) - 14.0).abs() < 1e-6);
        assert!((reference_px(7.142_857) - 12.0).abs() < 1e-4);
        assert!((reference_px(46.428_57) - 1.0).abs() < 1e-4);
        assert!(
            (reference_px(200.0) - 1.0).abs() < 1e-6,
            "flat past the floor"
        );
        // 10 and 40 yd give a ratio of 4 under either law; 25 yd is where the two disagree.
        let (near, far) = (reference_px(10.0), reference_px(40.0));
        assert!((near - 11.2).abs() < 1e-4 && (far - 2.8).abs() < 1e-4);
        assert!(
            (reference_px(25.0) - 7.0).abs() < 1e-4,
            "linear midpoint; a 1/d law would give {}",
            near * 10.0 / 25.0
        );
    }

    /// Alpha `clamp01(t − f1)` while falling, then `clamp01(1 − 4·(t − f2))` settled.
    #[test]
    fn flake_alpha_fades_in_then_out() {
        let view = view();
        let at = Vec3::new(0.0, 0.0, -10.0);
        let alpha_of = |drops: &[Drop], settled: &[Patter]| {
            let mut out = Vec::new();
            push_flakes(&mut out, drops, settled, &view);
            out[0].color[3]
        };
        assert!(
            (alpha_of(&[flake(at, 0.0)], &[]) - 0.0).abs() < 1e-6,
            "born invisible"
        );
        assert!((alpha_of(&[flake(at, 0.5)], &[]) - 0.5).abs() < 1e-6);
        assert!((alpha_of(&[flake(at, 1.0)], &[]) - 1.0).abs() < 1e-6);
        assert!(
            (alpha_of(&[flake(at, 4.0)], &[]) - 1.0).abs() < 1e-6,
            "held, not overshooting"
        );
        let settled = |age| {
            vec![Patter {
                pos: at,
                age,
                variant: 0,
            }]
        };
        assert!((alpha_of(&[], &settled(0.0)) - 1.0).abs() < 1e-6);
        assert!((alpha_of(&[], &settled(0.125)) - 0.5).abs() < 1e-6);
        assert!((alpha_of(&[], &settled(0.25)) - 0.0).abs() < 1e-6);
    }

    /// Camera-relative verts keep a near flake's ~1.6 mm half-extent exact far from the origin.
    #[test]
    fn the_footprint_survives_being_far_from_the_world_origin() {
        let proj = Mat4::perspective_rh(FOVY, 16.0 / 9.0, 0.1, 1000.0);
        for eye in [
            Vec3::new(529.48, 399.67, 5595.89), // Kharanos, in Bevy space
            Vec3::splat(17066.0),               // the far corner of a map
        ] {
            let view = view_at(eye);
            for d in [0.3f32, 0.5, 1.0, 5.0, 30.0] {
                let mut out = Vec::new();
                push_flakes(
                    &mut out,
                    &[flake(eye + Vec3::new(0.0, 0.0, -d), 5.0)],
                    &[],
                    &view,
                );
                // Camera-relative verts project as from a camera at the origin; at the era height
                // the footprint is the reference's own pixel count.
                let window_y = |p: Vec3| {
                    let clip = proj * p.extend(1.0);
                    (clip.y / clip.w + 1.0) * 0.5 * SNOW_PX_REF_HEIGHT
                };
                let px = window_y(Vec3::from(out[3].pos)) - window_y(Vec3::from(out[0].pos));
                let want = reference_px(d);
                assert!(
                    (px - want).abs() < 0.01,
                    "eye {eye:?}, {d} yd: {px:.3} px vs the reference's {want:.3}"
                );
            }
        }
    }

    /// A flake behind the eye would be mirrored through the camera.
    #[test]
    fn flakes_behind_the_eye_are_dropped() {
        let mut out = Vec::new();
        push_flakes(
            &mut out,
            &[
                flake(Vec3::new(0.0, 0.0, 5.0), 2.0),  // behind
                flake(Vec3::new(0.0, 0.0, -5.0), 2.0), // in front
            ],
            &[],
            &view(),
        );
        assert_eq!(out.len(), 4, "only the flake in front is drawn");
    }
}
