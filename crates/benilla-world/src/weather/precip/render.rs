//! The precip **stream pushers** — streak/patter/flake geometry emitted each frame from the
//! live pools straight into the shared effect stream (0733; world-space, and the lane rebases
//! camera-relative render-side — except the FLAKE draw, which writes camera-relative itself for
//! f32 precision, see [`push_flakes`]). Split from `precip`'s root; geometry only, no sim state.
//! Empty pools push nothing, so an idle sky costs zero here — the structural replacement for
//! the old fixed-capacity meshes' `WriteGate` (the 0353 fps hunt).

// Explicit, not `bevy::prelude::*`: the parent module's own prelude glob reaches this file
// through `use super::*` below, and two globs supplying the same names had rustc crediting the
// use to one or the other by platform — the "unused import" every non-macOS build warned on
// (2205's next move 4).
use bevy::math::{Quat, Vec3};

use crate::particles::buffer::EffectVertex;

use super::pool::{Drop, Patter};
use super::*;

/// Falling-drop streaks — the verified triangle law (`0x80ff78`, `0x80ff74`): per drop, base
/// verts `head ∓ 0.05·RIGHT` (RIGHT = normalize(cross(toCam, antiVel)), camera-facing width
/// axis), apex `head + M·(2.0·antiVel̂)` with M the wind-tilt applied to the APEX ONLY. UVs
/// (0,1)/(1,1)/(0.5,0). No vertex colour/alpha (white; the look is the texture under Mod2x).
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

/// Ground patters: one camera-facing **triangle** per splash (the byte geometry: corners
/// `center − right`, `center + up`, `center + right` with `right = view_right/12`,
/// `up = view_up/6`), animated left→right across its atlas row over the 0.25 s life.
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
        // No vertex alpha (Mod2x has none): the atlas's 4 growth frames are the animation, and
        // the texture's grey-128 background is neutral.
        // The byte texcoord law (`0x675ac0`): base-left, apex, base-right.
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

/// The camera terms [`push_flakes`] needs to turn the reference's **pixel** point size into a
/// world-space quad. `snowpoint.bls` sizes a flake in window coordinates, so the size a flake
/// gets is a property of the projection, not of the world.
pub(super) struct FlakeView {
    pub(super) eye: Vec3,
    /// The camera's forward axis — a flake's *view depth* (`z`, the perspective divisor), which
    /// is what the pixel↔world conversion keys on. Distinct from `|flake − eye|`, which is what
    /// the size LAW keys on; off-axis flakes differ in the two.
    pub(super) forward: Vec3,
    pub(super) right: Vec3,
    pub(super) up: Vec3,
    /// `tan(fovY/2) / SNOW_PX_REF_HEIGHT` — world units per **era pixel**, per unit of view depth.
    /// A world length `L` at view depth `z` covers `L / (z · this)` era pixels, so a `px`-era-pixel
    /// sprite wants half-extent `px · z · this`.
    ///
    /// The denominator is deliberately **not** the live viewport height — see
    /// [`super::SNOW_PX_REF_HEIGHT`]. Dividing by the live height reproduces the reference's pixel
    /// *count* and destroys its apparent size on any screen taller than the era's.
    pub(super) world_per_px: f32,
}

/// Snow flakes — the ARB point-sprite leg `0x678610`, reproduced as screen-aligned quads because
/// wgpu has no point size (WebGPU pins `PointList` at 1 px).
///
/// Per flake (the shipped `snowpoint.bls` read verbatim):
/// - size `max(1, 14·clamp01(1 − 0.02·d))` **era pixels**, `d = |flake − eye|` in yards — inverted
///   through the projection into a world half-extent, so the on-screen footprint matches the
///   reference's *angular* size at any resolution or fov (see [`super::SNOW_PX_REF_HEIGHT`] for
///   why the era's screen height, and not the live one, is the denominator);
/// - RGB white, alpha `clamp01(t − f1)` while falling (a 1 s linear fade-IN from spawn) and
///   `clamp01(1 − 4·(t − f2))` once settled (the 0.25 s fade-out);
/// - the whole texture per flake (`GL_COORD_REPLACE`), which the 0..1 UVs below reproduce.
///
/// There is **no per-flake size**: the 32-byte flake record has no spare field, the spawn draws
/// exactly 5 RNG values (none a size), and both legs' size terms are per-draw constants.
///
/// `drops` = falling flakes; `settled` = landed ones fading out over the `+0.25 s` window.
pub(super) fn push_flakes(
    out: &mut Vec<EffectVertex>,
    drops: &[Drop],
    settled: &[Patter],
    view: &FlakeView,
) {
    let mut sprite = |center: Vec3, alpha: f32| {
        // CAMERA-RELATIVE from here down (the draw sets `EffectDrawSpec::cam_relative`, so the
        // lane's rebase skips it). This is not a convenience: a near flake's half-extent is
        // ~1.6 mm, which is 3 f32 ULPs at Kharanos's ~5600-yd coordinates and under 1 at a map
        // corner — written absolutely, a 14 px sprite loses 2–7 px of width and flickers. Every
        // term below is small, so the arithmetic is exact wherever in the world we are.
        let to_flake = center - view.eye;
        let z = to_flake.dot(view.forward);
        if z <= 0.0 {
            return; // behind the eye — clipped anyway, and the pixel↔world map is undefined
        }
        // The size law, in pixels, off the RADIAL distance; then pixels → world at this flake's
        // view depth.
        let px = (SNOW_PX_AT_EYE * (1.0 - SNOW_PX_FALLOFF * to_flake.length()).clamp(0.0, 1.0))
            .max(SNOW_PX_MIN);
        let half = px * z * view.world_per_px;
        let r = view.right * half;
        let u = view.up * half;
        // Perimeter order (bl, br, tr, tl) — the stream's quad-index pattern closes it.
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

    /// A camera at `eye` looking down −Z (Bevy's convention), 45° vertical fov — carrying the
    /// production `world_per_px`, which denominates in the ERA height and not the render height.
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

    /// A flake's footprint, in real pixels, at a given render height — measured by pushing the
    /// quad and projecting its corners through a real perspective matrix, never by re-deriving
    /// the half-extent formula.
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

    /// **At the era's own screen height benilla draws the reference's pixels exactly** —
    /// `max(1, 14·clamp01(1 − 0.02·d))`. This is the fidelity anchor; the test below is what
    /// keeps it from being read as "14 px, always".
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

    /// …and on a taller screen it grows to hold the same **angle**, rather than staying at 14 px
    /// and shrinking away.
    ///
    /// `snowpoint.bls` sizes in framebuffer pixels because that is what
    /// `GL_VERTEX_PROGRAM_POINT_SIZE_ARB` does, not as a statement about apparent size; on 2004
    /// hardware the framebuffer *was* the screen. Obeyed literally, the flake's angular size falls
    /// as `1/height` — the director's A/B put benilla's flakes 2.7× under the reference's purely
    /// because a 4K scale-2 framebuffer is 2.7× taller than the reference install's 800
    /// (decision 1162). Pin the invariant that fixed it: the fraction of the screen a flake covers
    /// does not depend on the resolution.
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
            // And the absolute pixel count really does scale, which is the whole change.
            let tall = footprint_px(d, 2144.0);
            let want = reference_px(d) * 2144.0 / SNOW_PX_REF_HEIGHT;
            assert!(
                (tall - want).abs() < 0.01,
                "at {d} yd: {tall:.2} vs {want:.2}"
            );
        }
    }

    /// The two ends of the law, stated as absolutes so a regression is unmistakable: 14 px at the
    /// eye, and the 1 px floor from 46.43 yd out — **not** a `1/d` world-space size, which would
    /// blow up at the eye and vanish in the distance.
    #[test]
    fn point_size_is_linear_in_distance_not_inverse() {
        assert!((reference_px(0.0) - 14.0).abs() < 1e-6);
        assert!((reference_px(7.142_857) - 12.0).abs() < 1e-4);
        assert!((reference_px(46.428_57) - 1.0).abs() < 1e-4);
        assert!(
            (reference_px(200.0) - 1.0).abs() < 1e-6,
            "flat past the floor"
        );
        // A fixed world-size quad's pixel span is ∝ 1/d; this one FALLS OFF LINEARLY, so the
        // ratio between 10 yd and 40 yd is 11.2/2.8 = 4, not 4 by coincidence of 1/d (which would
        // also be 4) — pin the shape at a third point where the two laws disagree.
        let (near, far) = (reference_px(10.0), reference_px(40.0));
        assert!((near - 11.2).abs() < 1e-4 && (far - 2.8).abs() < 1e-4);
        assert!(
            (reference_px(25.0) - 7.0).abs() < 1e-4,
            "linear midpoint; a 1/d law would give {}",
            near * 10.0 / 25.0
        );
    }

    /// Alpha: `clamp01(t − f1)` falling — a 1 s linear fade-IN from spawn — then
    /// `clamp01(1 − 4·(t − f2))` settled. benilla drew every falling flake at 1.0.
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

    /// **The precision pin.** The same footprint, with the camera where the game actually puts it:
    /// Kharanos (~5600 yd out) and the far corner of a 17066-yd map. `EffectVertex::pos` is f32,
    /// and a near flake's half-extent is ~1.6 mm — 3 ULPs at 5600, under 1 at 17066. Written in
    /// ABSOLUTE world coordinates (the lane's default, which subtracts the camera only later, on
    /// the upload copy) a 14 px sprite loses 2–7 px of width and flickers; written camera-relative
    /// it is exact anywhere. This test fails on the absolute form and passes on the relative one,
    /// which is the only reason to have it.
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
                // The emitted verts are camera-relative, so they project through the same matrix
                // a camera at the origin would use — that IS the rebase the lane would have done.
                // Rendered at the era height, so the expected footprint is the reference's own
                // pixel count (this test is about precision, not about resolution).
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

    /// A flake behind the eye emits nothing — the pixel↔world map has no meaning at or behind the
    /// projection plane, and the quad would be mirrored through the camera.
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
