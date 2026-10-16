//! Booth camera placement. Both families use the model's own authored camera and fit nothing:
//!
//! - [`frame`]: a round portrait, `cameraLookup[0]` verbatim (`0x713540`); a camera-less model
//!   gets a heuristic head closeup at [`head_anchor`].
//! - [`body_frame`]: a `<PlayerModel>` pane, raw `cameras[1]` verbatim, else the client's fixed
//!   rig (`0x505890`).

use bevy::camera::{CameraProjection, PerspectiveProjection, Projection, SubCameraView};
use bevy::math::Vec3A;
use bevy::prelude::*;

/// Vertical FOV (radians, about 29°) of the heuristic camera for a camera-less model.
pub(super) const PORTRAIT_FOV: f32 = 0.5;

/// The aspect the client's portrait bake feeds `0x5c3cc0`: exactly 1.0 on every screen, so
/// `fovy = fov/√2`. `0x524f60` builds it as `(G44/G48)·(H/W)`, where `[0x832a44]/[0x832a48]` is the
/// live screen aspect, so the two cancel; the bake viewport is a square 64×64 box (`0x41ade0`).
/// The "fov × 0.6" figure is the 4/3 value, which the bake never uses.
pub(super) const PORTRAIT_ASPECT: f32 = 1.0;

/// A camera record's diagonal fov to the vertical opening at `aspect`: `fov/√(a²+1)`, the
/// client's `0x5c3cc0` relation. `aspect` is the same one that sets the squeeze, never a second
/// number: [`PORTRAIT_ASPECT`] for the round bake, the pane's own rect for a `<PlayerModel>`.
pub(super) fn diag_to_vert(fov: f32, aspect: f32) -> f32 {
    fov / (aspect * aspect + 1.0).sqrt()
}

/// A stock glue `ModelFFX` pane's view of its scene, in the booth's window-sized target.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct PaneView {
    /// The viewport, the pane's rect in physical pixels (`0x76d5d7`).
    pub(super) position: UVec2,
    pub(super) size: UVec2,
    /// The projection's aspect, the viewport's `w/h` (`0x7ada40`), which also turns the camera
    /// record's diagonal fov into the vertical opening (`0x5c3cc0`, [`diag_to_vert`]).
    pub(super) aspect: f32,
}

/// The view of a stock pane whose rect is `pane`, `[left, right, top, bottom]` as fractions of a
/// `full_w`×`full_h` window from its top-left: the scene fills the pane at the pane's own aspect,
/// with no box of its own. `None` for an empty rect.
pub(super) fn stock_pane_view(full_w: u32, full_h: u32, pane: [f32; 4]) -> Option<PaneView> {
    let [l, r, t, b] = pane;
    let px = |f: f32, full: u32| (f.clamp(0.0, 1.0) * full as f32).round() as u32;
    let (x0, x1) = (px(l, full_w), px(r, full_w));
    let (y0, y1) = (px(t, full_h), px(b, full_h));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let size = UVec2::new(x1 - x0, y1 - y0);
    Some(PaneView {
        position: UVec2::new(x0, y0),
        size,
        aspect: size.x as f32 / size.y as f32,
    })
}

/// The client's portrait and model projection, `0x5c3cc0`: a diagonal-FOV perspective with
/// half-angle `θ = (fov/2)/√(aspect²+1)`, `m11 = 1/tan θ`, `m00 = m11/aspect`.
///
/// One `aspect` sets both the squeeze and the crop, as in `0x5c3cc0`: 1.0 for the round portrait
/// ([`PORTRAIT_ASPECT`]), the pane's width÷height for a model pane (318×224 gives 0.576·fov). A
/// custom [`CameraProjection`] because Bevy re-derives `aspect_ratio` from the square render
/// target on every write; `update` is a no-op, as the client's bake emits the same matrix at any
/// window size.
#[derive(Debug, Clone)]
pub(crate) struct WowPortraitProjection {
    /// The M2 record's fov (radians), a diagonal angle, not fovy.
    pub(super) fov: f32,
    pub(super) near: f32,
    pub(super) far: f32,
    /// `0x5c3cc0`'s one aspect: the squeeze (`m00 = m11/aspect`) and the crop
    /// (`fovy = fov/√(aspect²+1)`). It must be the destination's aspect, because the booth's square
    /// target is stretched by the UI onto that rect.
    pub(super) aspect: f32,
}

impl WowPortraitProjection {
    /// Full vertical opening angle, `2θ`: `0x5c3cc0` has `tan(fovy/2) = tan θ` exactly.
    fn fovy(&self) -> f32 {
        diag_to_vert(self.fov, self.aspect)
    }
}

impl CameraProjection for WowPortraitProjection {
    fn get_clip_from_view(&self) -> Mat4 {
        // The client's matrix with Bevy's reverse-z infinite depth; the record's far (27.8 on
        // HumanMale) never clips a model-local portrait.
        Mat4::perspective_infinite_reverse_rh(self.fovy(), self.aspect, self.near)
    }

    fn get_clip_from_view_for_sub(&self, _sub_view: &SubCameraView) -> Mat4 {
        self.get_clip_from_view() // the booth never renders sub-camera views
    }

    fn update(&mut self, _width: f32, _height: f32) {} // target-size independent, as in the client

    fn far(&self) -> f32 {
        self.far
    }

    fn get_frustum_corners(&self, z_near: f32, z_far: f32) -> [Vec3A; 8] {
        // PerspectiveProjection's corner layout at our fovy and aspect; corners that disagree with
        // the matrix would cull models that render.
        let tan_half_fovy = (self.fovy() * 0.5).tan();
        let a = z_near.abs() * tan_half_fovy;
        let b = z_far.abs() * tan_half_fovy;
        let (ax, bx) = (a * self.aspect, b * self.aspect);
        [
            Vec3A::new(ax, -a, z_near),  // bottom right
            Vec3A::new(ax, a, z_near),   // top right
            Vec3A::new(-ax, a, z_near),  // top left
            Vec3A::new(-ax, -a, z_near), // bottom left
            Vec3A::new(bx, -b, z_far),   // bottom right
            Vec3A::new(bx, b, z_far),    // top right
            Vec3A::new(-bx, b, z_far),   // top left
            Vec3A::new(-bx, -b, z_far),  // bottom left
        ]
    }
}

/// A display id's framing inputs ([`Creatures::display_anchors`]), model-local at scale 1: the
/// reference bakes with the root scale reset (`0x47a230`).
#[derive(Clone)]
pub(crate) struct PortraitAnchors {
    /// The round portrait's authored camera, `cameraLookup[0]`; `None` uses the heuristic fields.
    pub(crate) camera: Option<benilla_assets::PortraitCamera>,
    /// The body pane's authored camera, raw index 1; `None` uses [`body_frame`]'s fixed rig.
    pub(crate) pane_camera: Option<benilla_assets::PortraitCamera>,
    /// The MD20 header bbox centre, the fixed pane camera's look-at target.
    pub(crate) bbox_center: Vec3,
    /// The bind-pose head anchor ([`head_anchor`]) the heuristic framing aims at.
    pub(crate) head: Option<Vec3>,
    /// Neck height (the follow-camera pivot), the heuristic's size and head-less fallback.
    pub(crate) pivot_height: f32,
    /// Footprint radius, a size hint for the heuristic standoff.
    pub(crate) ground_radius: f32,
}

/// A bone's bind-pose position in Bevy space: the rest skeleton is pure translations, so summing
/// the parent chain gives the pivot. `None` for a malformed or cyclic skeleton.
fn bind_bone_global(skeleton: &benilla_assets::ModelSkeleton, bone: u16) -> Option<Vec3> {
    let mut pos = Vec3::ZERO;
    let mut idx = usize::from(bone);
    for _ in 0..=skeleton.joints.len() {
        let j = skeleton.joints.get(idx)?;
        pos += j.local_translation;
        match usize::try_from(j.parent) {
            Ok(p) => idx = p,
            Err(_) => return Some(pos), // -1 = root reached
        }
    }
    None
}

/// A model attachment's bind-pose position in Bevy model space (bone pivot plus offset); a glue
/// scene's attachment 0 is the stage spot on camera 0's axis where the character stands.
pub(crate) fn attachment_point(
    skeleton: &benilla_assets::ModelSkeleton,
    attachments: &[benilla_assets::ModelAttachment],
    id: u16,
) -> Option<Vec3> {
    let a = attachments.iter().find(|a| a.id == id)?;
    Some(bind_bone_global(skeleton, a.bone)? + a.offset)
}

/// A model's bind-pose head anchor in Bevy space: the head key bone's pivot (KeyBoneID 6), else
/// the helm attachment (id 11).
pub(crate) fn head_anchor(
    skeleton: &benilla_assets::ModelSkeleton,
    attachments: &[benilla_assets::ModelAttachment],
) -> Option<Vec3> {
    if let Some(head) = skeleton.head_bone {
        return bind_bone_global(skeleton, head);
    }
    attachments
        .iter()
        .find(|a| a.id == 11)
        .and_then(|a| Some(bind_bone_global(skeleton, a.bone)? + a.offset))
}

/// Heuristic camera yaw off the model's front (radians, about +Y), a slight three-quarter view.
const PORTRAIT_YAW: f32 = 0.42;
/// Heuristic framed height as a fraction of the neck-pivot height.
const WINDOW_OF_PIVOT: f32 = 0.34;
/// Heuristic framed-height clamp (model-local yards), so a whelp fills the circle and a
/// devilsaur's head fits.
const WINDOW_MIN: f32 = 0.55;
const WINDOW_MAX: f32 = 1.1;

/// The round portrait's camera rig: the model's authored camera verbatim (`lookAt` with up rolled
/// about the view axis) through [`WowPortraitProjection`] at [`PORTRAIT_ASPECT`]. A camera-less
/// model gets a three-quarter head closeup sized by its height, with a footprint floor so a
/// long-bodied quadruped does not crop to its nose.
pub(super) fn frame(a: &PortraitAnchors) -> (Transform, Projection) {
    if let Some(cam) = a.camera {
        let fwd = (cam.target - cam.eye).normalize_or_zero();
        let up = Quat::from_axis_angle(fwd, cam.roll) * Vec3::Y;
        return (
            Transform::from_translation(cam.eye).looking_at(cam.target, up),
            Projection::custom(WowPortraitProjection {
                fov: cam.fov,
                near: cam.near,
                far: cam.far,
                // The client's bake aspect, and our sampling region is square too.
                aspect: PORTRAIT_ASPECT,
            }),
        );
    }
    // No bounds: a generic humanoid neck height.
    let neck = if a.pivot_height > 0.01 {
        a.pivot_height
    } else {
        1.8
    };
    let target = a.head.unwrap_or(Vec3::new(0.0, 1.05 * neck, 0.0));
    let window = (WINDOW_OF_PIVOT * neck)
        .max(0.9 * a.ground_radius)
        .clamp(WINDOW_MIN, WINDOW_MAX);
    let dist = (window * 0.5) / (PORTRAIT_FOV * 0.5).tan();
    // Orbit the front axis by the three-quarter yaw, eye level with the head.
    let offset = Quat::from_rotation_y(PORTRAIT_YAW) * Vec3::new(0.0, 0.0, -dist);
    (
        Transform::from_translation(target + offset).looking_at(target, Vec3::Y),
        Projection::from(PerspectiveProjection {
            fov: PORTRAIT_FOV,
            near: 0.02,
            far: 100.0,
            ..default()
        }),
    )
}

/// The client's renormalize-to-4:3 model-root factor, `G48·(5/3)` with `G48 = [0x832a48] =
/// 1/√(a²+1)` (`0x41ad10`) and `5/3` the literal at `[0x80655c]`, which equals `√((4/3)²+1)`:
/// 1.0 at 4:3, 0.8171 at 16:9. `a` is `gxResolution`'s width/height, or 4/3 when the `widescreen`
/// CVar (default 1, `0x63a74f`) is 0 (`0x46ab40`).
///
/// It only shows on the fixed-camera path: `0x71439b` publishes an authored camera through the
/// same scaled root (`0x718960`), so there it cancels, while the fixed rig's `CCamera`
/// (`0x7ac930`) is outside the model and sees the geometry shrink and sit low by `(1−s)·centre.y`.
pub(super) fn pane_model_scale(display_aspect: f32) -> f32 {
    const REF: f32 = 4.0 / 3.0;
    ((REF * REF + 1.0) / (display_aspect * display_aspect + 1.0)).sqrt()
}

/// The eye (WoW model space) of the client's fixed pane camera for a model with fewer than two
/// cameras (`0x505890`): the literals 5.5555558 and 2.4166667, aimed at the bbox centre with no
/// normalization, so a small model renders small.
const PANE_FIXED_EYE_WOW: [f32; 3] = [200.0 / 36.0, 0.0, 87.0 / 36.0];
/// The fixed camera's diagonal fov (radians); also the fov the pipeline-warm pass compiles with.
pub(super) const PANE_FIXED_FOV: f32 = 0.5;
/// The fixed camera's clip planes, verbatim.
const PANE_FIXED_NEAR: f32 = 1.0 / 36.0;
const PANE_FIXED_FAR: f32 = 5000.0;

/// A `<PlayerModel>` pane's camera rig, the widget's own (`0x505890`): the model's raw camera
/// index 1 (the "characterinfo" camera), else the fixed rig; no fit, no bone, no normalization.
/// Panes look normalized only because each model's standoff is authored (eye x: GnomeFemale 2.16,
/// HumanMale 3.66, TaurenMale 4.43, Boar 4.86). Yaw is the caller's, on the model root, as
/// `Model:SetRotation` sets the facing; the root scale is [`pane_root_scale`]. `aspect` is the
/// pane's width÷height.
pub(super) fn body_frame(a: &PortraitAnchors, aspect: f32) -> (Transform, Projection) {
    let cam = pane_camera(a);
    // `lookAt` with up rolled about the view axis; every 1.12 camera's roll track is one zero key.
    let fwd = (cam.target - cam.eye).normalize_or_zero();
    let up = Quat::from_axis_angle(fwd, cam.roll) * Vec3::Y;
    (
        Transform::from_translation(cam.eye).looking_at(cam.target, up),
        Projection::custom(pane_projection(&cam, aspect)),
    )
}

/// The body pane's model-root scale, separate from [`body_frame`] because a window resize
/// re-latches it without a re-bake. 1.0 with an authored camera, where [`pane_model_scale`]
/// cancels.
pub(super) fn pane_root_scale(a: &PortraitAnchors, display_aspect: f32) -> f32 {
    if a.pane_camera.is_some() {
        1.0
    } else {
        pane_model_scale(display_aspect)
    }
}

/// The camera a body pane renders through: authored `cameras[1]`, else the fixed rig.
pub(super) fn pane_camera(a: &PortraitAnchors) -> benilla_assets::PortraitCamera {
    a.pane_camera.unwrap_or(benilla_assets::PortraitCamera {
        eye: benilla_assets::coords::wow_to_bevy(PANE_FIXED_EYE_WOW),
        target: a.bbox_center,
        roll: 0.0,
        fov: PANE_FIXED_FOV,
        near: PANE_FIXED_NEAR,
        far: PANE_FIXED_FAR,
    })
}

/// A pane camera's projection: the record's scalars at the pane's own aspect, not the bake's 1.0.
/// Both paths build through `0x7ada40`/`0x7ac640`, but the widget (`0x76d42d`) passes the frame's
/// cached layout rect (`0x768320`), so `aspect = (W/H)·(a/a_screen)`, which is `W/H` in every
/// state the client reaches; the `0x41ade0` unscales (`0x76d45b`/`0x76d46e`) serve the viewport
/// only.
pub(crate) fn pane_projection(
    cam: &benilla_assets::PortraitCamera,
    aspect: f32,
) -> WowPortraitProjection {
    WowPortraitProjection {
        fov: cam.fov,
        near: cam.near,
        far: cam.far,
        aspect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Perspective-divided clip-space XY of a model-local point through a rig.
    fn ndc(transform: &Transform, proj: &WowPortraitProjection, p: Vec3) -> Vec2 {
        let clip = proj.get_clip_from_view() * transform.to_matrix().inverse() * p.extend(1.0);
        Vec2::new(clip.x / clip.w, clip.y / clip.w)
    }

    fn ndc_y(transform: &Transform, proj: &WowPortraitProjection, p: Vec3) -> f32 {
        ndc(transform, proj, p).y
    }

    /// An M2 camera record from WoW-space eye and target.
    fn cam(eye_wow: [f32; 3], target_wow: [f32; 3], fov: f32) -> benilla_assets::PortraitCamera {
        benilla_assets::PortraitCamera {
            eye: benilla_assets::coords::wow_to_bevy(eye_wow),
            target: benilla_assets::coords::wow_to_bevy(target_wow),
            roll: 0.0,
            fov,
            near: 0.222_222,
            far: 27.777_8,
        }
    }

    fn anchors_with(
        pane_camera: Option<benilla_assets::PortraitCamera>,
        bbox_center: Vec3,
    ) -> PortraitAnchors {
        PortraitAnchors {
            // The heuristic fields are populated so a pane rig that reads them fails these tests.
            camera: Some(cam(
                [0.6335, -0.3879, 1.8867],
                [0.0627, 0.0343, 1.8636],
                0.785,
            )),
            pane_camera,
            bbox_center,
            head: Some(Vec3::new(0.0, 1.75, 0.0)),
            pivot_height: 1.90,
            ground_radius: 0.35,
        }
    }

    #[test]
    fn a_pane_takes_the_models_own_camera_untouched() {
        // HumanMale's `cameras[1]`, measured off the shipped file.
        let human = cam([3.6585, 0.0338, 0.9227], [-0.3644, 0.0291, 0.9873], 0.97991);
        for &aspect in &[1.0_f32, 318.0 / 224.0, 233.0 / 224.0, 316.0 / 351.0] {
            let (transform, projection) =
                body_frame(&anchors_with(Some(human), Vec3::ZERO), aspect);
            assert!(
                transform.translation.distance(human.eye) < 1e-5,
                "eye moved: {:?} vs {:?}",
                transform.translation,
                human.eye
            );
            let want = (human.target - human.eye).normalize();
            assert!(
                transform.forward().dot(want) > 0.9999,
                "not aimed at the record's target (dot {})",
                transform.forward().dot(want)
            );
            // Compared as the clip matrix, which is what a wrong field would move.
            let want = pane_projection(&human, aspect);
            assert_eq!(
                (want.fov, want.near, want.far),
                (human.fov, human.near, human.far)
            );
            assert_eq!(want.aspect, aspect);
            assert_eq!(
                projection.get_clip_from_view(),
                want.get_clip_from_view(),
                "the rig's projection is not the record's"
            );
        }
    }

    /// Measured as the on-screen span of one model yard at the subject.
    #[test]
    fn a_pane_is_not_normalized_across_models() {
        let aspect = 318.0 / 224.0; // the pet pane
        let bodies = [
            // (name, cameras[1] eye, target, fov, the model's own standing height)
            ("GnomeFemale", [2.1591_f32, 0.0, 0.6136], 0.6136, 0.85_f32),
            ("HumanMale", [3.6585, 0.0338, 0.9227], 0.9227, 1.85),
            ("TaurenMale", [4.4317, -0.0213, 1.0861], 1.0861, 2.60),
        ];
        let mut apparent = Vec::new();
        for (name, eye, target_z, _height) in bodies {
            let c = cam(eye, [-0.2, 0.0, target_z], 0.9);
            let (transform, _) = body_frame(&anchors_with(Some(c), Vec3::ZERO), aspect);
            let proj = WowPortraitProjection {
                fov: c.fov,
                near: c.near,
                far: c.far,
                aspect,
            };
            // One yard of height at the subject's own standing point.
            let base = Vec3::new(0.0, c.target.y, 0.0);
            let span =
                (ndc_y(&transform, &proj, base + Vec3::Y) - ndc_y(&transform, &proj, base)).abs();
            apparent.push((name, span));
        }
        // A yard subtends less of the frame the further out the authored camera stands.
        for w in apparent.windows(2) {
            assert!(
                w[0].1 > w[1].1,
                "{} should show a yard larger than {} ({} vs {})",
                w[0].0,
                w[1].0,
                w[0].1,
                w[1].1
            );
        }
    }

    /// Only the look-at target moves with the model; the eye stays put.
    #[test]
    fn a_camera_less_model_gets_the_clients_fixed_rig() {
        let want_eye = Vec3::new(0.0, 87.0 / 36.0, -200.0 / 36.0);
        let mut eyes = Vec::new();
        for centre in [
            Vec3::ZERO,
            Vec3::new(0.0, 0.44, 0.0),
            Vec3::new(0.1, 1.3, 0.0),
        ] {
            let (transform, projection) = body_frame(&anchors_with(None, centre), 318.0 / 224.0);
            assert!(
                transform.translation.distance(want_eye) < 1e-4,
                "fixed eye drifted: {:?}",
                transform.translation
            );
            let want = (centre - want_eye).normalize();
            assert!(
                transform.forward().dot(want) > 0.9999,
                "the fixed camera must look at the bbox centre"
            );
            let fixed = pane_camera(&anchors_with(None, centre));
            assert_eq!(fixed.fov, PANE_FIXED_FOV);
            assert!((fixed.near - 1.0 / 36.0).abs() < 1e-6 && fixed.far == 5000.0);
            assert_eq!(
                projection.get_clip_from_view(),
                pane_projection(&fixed, 318.0 / 224.0).get_clip_from_view()
            );
            eyes.push(transform.translation);
        }
        assert!(
            eyes.windows(2).all(|w| w[0] == w[1]),
            "the fallback camera is FIXED — the model's size must not move it"
        );
    }

    #[test]
    fn the_model_root_renormalizes_to_four_thirds() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(
            close(pane_model_scale(4.0 / 3.0), 1.0),
            "the reference aspect"
        );
        assert!(close(pane_model_scale(16.0 / 10.0), 0.883_332));
        assert!(close(pane_model_scale(16.0 / 9.0), 0.817_102));
        assert!(close(pane_model_scale(5.0 / 4.0), 1.041_158));
    }

    #[test]
    fn the_root_factor_applies_only_where_it_does_not_cancel() {
        let human = cam([3.6585, 0.0338, 0.9227], [-0.3644, 0.0291, 0.9873], 0.97991);
        for &a in &[4.0 / 3.0, 16.0 / 9.0, 16.0 / 10.0, 5.0 / 4.0] {
            assert_eq!(
                pane_root_scale(&anchors_with(Some(human), Vec3::ZERO), a),
                1.0,
                "an authored camera cancels the factor at aspect {a}"
            );
            assert_eq!(
                pane_root_scale(&anchors_with(None, Vec3::new(0.0, 0.62, 0.0)), a),
                pane_model_scale(a),
                "the fixed fallback camera does not"
            );
        }
    }

    /// The pet pane (318×224), the character pane (233×224) and the round portrait (1:1).
    #[test]
    fn the_diagonal_crop_follows_the_aspect() {
        let close = |a: f32, b: f32| (a - b).abs() < 5e-4;
        assert!(close(diag_to_vert(1.0, 4.0 / 3.0), 0.6), "the 4/3 legend");
        assert!(close(diag_to_vert(1.0, 318.0 / 224.0), 0.575_86));
        assert!(close(diag_to_vert(1.0, 233.0 / 224.0), 0.693_05));
        assert!(close(
            diag_to_vert(1.0, PORTRAIT_ASPECT),
            std::f32::consts::FRAC_1_SQRT_2
        ));
    }

    /// The 1.12 client's GL stream uploads the same bake matrix for HumanMale's `cameraLookup[0]`
    /// (`fov = π/4`) at 1152×648 and at 1280×800: `m00 = 3.508226`, `m11 = 3.508225`, so
    /// `1/m11 = tan((fov/2)/√2)`.
    #[test]
    fn the_portrait_bake_matches_the_clients_own_matrix() {
        const OBSERVED_M11: f32 = 3.508_226;
        let proj = WowPortraitProjection {
            fov: std::f32::consts::FRAC_PI_4,
            near: 0.111_57,
            far: 27.778,
            aspect: PORTRAIT_ASPECT,
        };
        let m = proj.get_clip_from_view();
        assert!(
            (m.x_axis.x - OBSERVED_M11).abs() < 1e-4,
            "m00 {} is not the client's {OBSERVED_M11}",
            m.x_axis.x
        );
        assert!(
            (m.y_axis.y - OBSERVED_M11).abs() < 1e-4,
            "m11 {} is not the client's {OBSERVED_M11}",
            m.y_axis.y
        );
        // A 4/3 crop frames about 18.7% tighter than the client.
        let tight = WowPortraitProjection {
            aspect: 4.0 / 3.0,
            ..proj
        };
        assert!(
            tight.get_clip_from_view().y_axis.y > OBSERVED_M11 * 1.15,
            "the 4/3 crop should be visibly tighter than the client's"
        );
    }

    /// The square booth target is stretched onto the pane, so the projection must run at the
    /// pane's aspect; measured as screen pixels per model yard, sideways against upward.
    #[test]
    fn a_pane_shows_the_figure_unstretched_at_any_aspect() {
        for &(w, h) in &[(316.0_f32, 351.0_f32), (233.0, 224.0), (512.0, 512.0)] {
            let aspect = w / h;
            let c = cam([3.6585, 0.0338, 0.9227], [-0.3644, 0.0291, 0.9873], 0.97991);
            let (transform, _) = body_frame(&anchors_with(Some(c), Vec3::ZERO), aspect);
            let proj = WowPortraitProjection {
                fov: c.fov,
                near: c.near,
                far: c.far,
                aspect,
            };
            // A small cross at the framing centre, each arm measured in pane pixels.
            let mid = Vec3::new(0.0, transform.translation.y, 0.0);
            let step = 0.05_f32;
            let o = ndc(&transform, &proj, mid);
            let px_x = (ndc(&transform, &proj, mid + Vec3::X * step).x - o.x).abs() * 0.5 * w;
            let px_y = (ndc(&transform, &proj, mid + Vec3::Y * step).y - o.y).abs() * 0.5 * h;
            let stretch = px_y / px_x;
            assert!(
                (stretch - 1.0).abs() < 0.005,
                "a {w}×{h} pane stretches the figure by {stretch} (1.0 = round)"
            );
        }
    }
}
