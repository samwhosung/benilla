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

/// The authored aspect of every 1.12 glue composition, 4/3: the Lua screen is `768a × 768`
/// (`GetScreenWidth 0x48b480`, `GetScreenHeight 0x48b4d0`), so the design space is 1024×768.
pub(super) use benilla_formats::{ArtExtent, GLUE_AUTHORED_ASPECT};

/// The glue scene's vertical opening angle.
///
/// The reference's glue `<ModelFFX>` fills the screen, so `0x5c3cc0`'s aspect is the display's
/// (`0x76d42d`) and the diagonal angle is the invariant: `0.600·fov` at 4:3, `0.492·fov` at 16:9,
/// `0.386·fov` at 21:9, where the character's head and feet leave the frame.
///
/// Deviation: the authored 4:3 box is held instead, because the reference's diagonal law crops
/// the character on wide panels and the GlueXML chrome holds its height at every width. From 4/3
/// up the vertical opening holds and the width grows up to [`GLUE_BOX_ASPECT`], then the width
/// holds and the vertical closes down to [`glue_zoom_floor`], the reference's own 16:9 opening;
/// past that the scene is pillarboxed ([`glue_box_aspect`]). No scene's art is drawn wider than
/// about 3:2. Below 4/3 the authored horizontal extent holds and the view opens upward, up to the
/// art's `half_h` but never inside the authored box. Continuous at 4/3, where both legs give
/// `0.6·fov`. A `<PlayerModel>` pane is not the window and keeps its verbatim crop.
pub(super) fn glue_scene_framing(fov: f32, window_aspect: f32, art: Option<ArtExtent>) -> f32 {
    let authored = diag_to_vert(fov, GLUE_AUTHORED_ASPECT);
    if window_aspect <= 0.0 {
        return authored; // a degenerate (mid-resize) window: the authored opening, finite
    }
    let t0 = (authored * 0.5).tan(); // the authored vertical half-extent
    let h0 = t0 * GLUE_AUTHORED_ASPECT; // …and horizontal
    if window_aspect >= GLUE_AUTHORED_ASPECT {
        // Hor+ up to the box's width, then the vertical closes to the 16:9 floor; wider still is
        // boxed by `glue_box_aspect`.
        let floor = glue_zoom_floor(fov);
        let half_w = (t0 * window_aspect).min(GLUE_BOX_ASPECT * floor);
        2.0 * (half_w / window_aspect).max(floor).atan()
    } else {
        // Hold the authored half-width and open upward to the art's edge; past it the sides crop,
        // so no floor and no bars.
        let ceiling = art.map_or(f32::INFINITY, |a| a.half_h.max(t0));
        2.0 * (h0 / window_aspect).min(ceiling).atan()
    }
}

/// The pillarbox aspect for a window: `Some(GLUE_BOX_ASPECT)` when the window is wider, else
/// `None`. It depends on the window alone, so the booth viewport and the chrome's canvas
/// ([`crate::glue::GlueCanvas`]) hold still when the selected race changes the stage.
pub(crate) fn glue_box_aspect(window_aspect: f32) -> Option<f32> {
    (window_aspect > GLUE_BOX_ASPECT).then_some(GLUE_BOX_ASPECT)
}

/// The one glue frame, 1.672:1: the max over the seven shipped scenes of authored 4:3 half-width
/// over the [`glue_zoom_floor`], so no scene's authored composition is cropped (`UI_MainMenu`, 86°,
/// sits exactly on it). Pinned by [`tests::the_glue_frame_is_the_narrowest_box_no_scene_pays_for`].
pub(super) const GLUE_BOX_ASPECT: f32 = 1.672_042_7;

/// 16:9, the panel whose reference framing is the glue zoom floor.
pub(super) const REFERENCE_PANEL: f32 = 16.0 / 9.0;

/// The vertical half-extent (tan units) a glue scene is never framed tighter than: the
/// reference's opening at [`REFERENCE_PANEL`].
pub(super) fn glue_zoom_floor(fov: f32) -> f32 {
    (diag_to_vert(fov, REFERENCE_PANEL) * 0.5).tan()
}

/// The pillarbox in physical pixels, `(x, width)` of the centred box. The booth viewport
/// (`super::glue_booth::pillarbox_glue_scene`) and the chrome's canvas both read it, so they
/// agree to the pixel.
pub(crate) fn glue_box_physical(
    full_w: u32,
    full_h: u32,
    viewport_aspect: Option<f32>,
) -> Option<(u32, u32)> {
    let aspect = viewport_aspect?;
    let (full_w, full_h) = (full_w.max(1), full_h.max(1));
    // A NaN or negative aspect saturates to 0 through `as u32` and clamps to 1, never a panic.
    let box_w = ((full_h as f32 * aspect).round() as u32).clamp(1, full_w);
    Some(((full_w - box_w) / 2, box_w))
}

/// The pillarbox's left and right bars in logical px, the glue chrome canvas's inset. Two numbers,
/// not one halved: an odd leftover puts the box a pixel off centre. Measured in physical pixels
/// and divided by the scale factor, so they cannot round differently from the camera viewport.
pub(crate) fn glue_canvas_bars(
    window: Option<&Window>,
    viewport_aspect: Option<f32>,
) -> (f32, f32) {
    let Some(w) = window else {
        return (0.0, 0.0);
    };
    let full_w = w.physical_width().max(1);
    let Some((x, box_w)) = glue_box_physical(full_w, w.physical_height(), viewport_aspect) else {
        return (0.0, 0.0);
    };
    let sf = if w.scale_factor() > 0.0 {
        w.scale_factor()
    } else {
        1.0
    };
    (x as f32 / sf, full_w.saturating_sub(x + box_w) as f32 / sf)
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

    /// A 3440×1440 window gets the 16:9 opening, `0.490·fov`, where the reference's law gives
    /// `0.386·fov`.
    #[test]
    fn the_glue_scene_is_never_framed_tighter_than_the_reference_at_16_9() {
        let close = |a: f32, b: f32| (a - b).abs() < 5e-4;
        let authored = diag_to_vert(1.0, GLUE_AUTHORED_ASPECT);
        assert!(
            close(authored, 0.6),
            "the authored 4:3 opening is the 0.6 legend"
        );
        // 4:3 itself: the authored opening, unboxed.
        assert!(close(
            glue_scene_framing(1.0, GLUE_AUTHORED_ASPECT, None),
            authored
        ));
        assert!(glue_box_aspect(GLUE_AUTHORED_ASPECT).is_none());
        // Between 4:3 and the frame the opening closes, monotonically, and the window still fills.
        let mut last = authored;
        for a in [1.4, 16.0 / 10.0, GLUE_BOX_ASPECT] {
            let vert = glue_scene_framing(1.0, a, None);
            assert!(vert <= last + 1e-6, "a{a}: {vert} > {last}");
            assert!(glue_box_aspect(a).is_none(), "a{a} fills the window");
            last = vert;
        }
        // At and past the frame: the reference's 16:9 opening, exactly.
        let reference_16_9 = diag_to_vert(1.0, REFERENCE_PANEL);
        assert!(close(reference_16_9, 0.490_26));
        for wide in [REFERENCE_PANEL, 3440.0 / 1440.0, 32.0 / 9.0] {
            let vert = glue_scene_framing(1.0, wide, None);
            assert!(close(vert, reference_16_9), "a{wide}: {vert}");
            assert_eq!(glue_box_aspect(wide), Some(GLUE_BOX_ASPECT));
        }
        // The reference's own opening at 3440×1440.
        assert!(close(diag_to_vert(1.0, 3440.0 / 1440.0), 0.386_14));
    }

    #[test]
    fn a_narrow_glue_window_holds_its_authored_width_instead() {
        let half_width = |vert: f32, a: f32| (vert * 0.5).tan() * a;
        let authored = half_width(
            diag_to_vert(1.0, GLUE_AUTHORED_ASPECT),
            GLUE_AUTHORED_ASPECT,
        );
        for narrow in [5.0 / 4.0, 1.0, 3.0 / 4.0] {
            let got = half_width(glue_scene_framing(1.0, narrow, None), narrow);
            assert!(
                (got - authored).abs() < 5e-4,
                "a{narrow}: authored half-width {authored} vs {got}"
            );
        }
        // A tall window sees more, never less.
        assert!(
            glue_scene_framing(1.0, 1.0, None)
                > glue_scene_framing(1.0, GLUE_AUTHORED_ASPECT, None)
        );
        // A degenerate mid-resize window must not produce a NaN fov.
        assert!(glue_scene_framing(1.0, 0.0, None).is_finite());
    }

    #[test]
    fn every_glue_scene_is_framed_in_the_same_box() {
        let close = |a: f32, b: f32| (a - b).abs() < 5e-4;
        let half_width = |vert: f32, a: f32| (vert * 0.5).tan() * a;
        // The shipped fovs span 60° to 86°; plus the unit fov.
        let deg = std::f32::consts::PI / 180.0;
        for fov in [1.0_f32, 60.0 * deg, 65.0 * deg, 80.0 * deg, 86.0 * deg] {
            let authored = diag_to_vert(fov, GLUE_AUTHORED_ASPECT);
            let t0 = (authored * 0.5).tan();
            let floor = glue_zoom_floor(fov);
            assert!(close(
                floor,
                (diag_to_vert(fov, REFERENCE_PANEL) * 0.5).tan()
            ));
            // 4:3: the authored opening, filling the window.
            assert!(close(
                glue_scene_framing(fov, GLUE_AUTHORED_ASPECT, None),
                authored
            ));
            // Between 4:3 and the frame: the width is the box's and the vertical closes.
            for a in [1.5, 1.6, GLUE_BOX_ASPECT] {
                let vert = glue_scene_framing(fov, a, None);
                assert!(
                    close(half_width(vert, a), GLUE_BOX_ASPECT * floor),
                    "fov {fov} a{a}: half-width {}",
                    half_width(vert, a)
                );
                assert!(vert < authored && vert >= 2.0 * floor.atan() - 1e-6);
            }
            // Past it: the floor holds the opening and the box is the frame.
            for a in [REFERENCE_PANEL, 3440.0 / 1440.0, 32.0 / 9.0] {
                let vert = glue_scene_framing(fov, a, None);
                assert!(
                    close((vert * 0.5).tan(), floor),
                    "fov {fov} a{a} off the floor"
                );
                assert_eq!(glue_box_aspect(a), Some(GLUE_BOX_ASPECT));
                assert!(
                    close(half_width(vert, GLUE_BOX_ASPECT), GLUE_BOX_ASPECT * floor),
                    "fov {fov} a{a}: the box ends where the width does"
                );
            }
            // The wide leg ignores the art; only the narrow leg reads `half_h`.
            for art in [
                Some(ArtExtent {
                    half_w: t0 * 1.31,
                    half_h: 10.0,
                }),
                Some(ArtExtent {
                    half_w: t0 * 4.0,
                    half_h: 10.0,
                }),
            ] {
                for a in [1.5, REFERENCE_PANEL, 3440.0 / 1440.0] {
                    assert_eq!(
                        glue_scene_framing(fov, a, art),
                        glue_scene_framing(fov, a, None),
                        "fov {fov} a{a}: the wide leg read the art"
                    );
                }
            }
        }
    }

    /// No scene's art runs out inside the frame, except the night elves' sky card, which ends just
    /// inside its own authored box.
    #[test]
    fn the_glue_frame_is_the_narrowest_box_no_scene_pays_for() {
        use benilla_formats::{authored_half_height, SHIPPED_GLUE_SCENES};
        let mut widest = 0.0_f32;
        for scene in SHIPPED_GLUE_SCENES {
            let t0 = authored_half_height(scene.fov);
            let h0 = t0 * GLUE_AUTHORED_ASPECT;
            let floor = glue_zoom_floor(scene.fov);
            // No scene gives up its authored width: the frame reaches at least h0.
            assert!(
                GLUE_BOX_ASPECT * floor >= h0 - 1e-4,
                "UI_{}: the frame stops at {} inside the authored {h0}",
                scene.token,
                GLUE_BOX_ASPECT * floor
            );
            // The night elf frame runs about 1.3% past its sky card, into black.
            if scene.token == "NightElf" {
                assert!(scene.art.half_w < h0, "the night elf exception closed?");
                assert!(
                    GLUE_BOX_ASPECT * floor <= h0 * 1.015,
                    "UI_NightElf: the frame reaches {} past the authored {h0}",
                    GLUE_BOX_ASPECT * floor
                );
            } else {
                assert!(
                    GLUE_BOX_ASPECT * floor <= scene.art.half_w,
                    "UI_{}: the frame reaches {} past the art's {}",
                    scene.token,
                    GLUE_BOX_ASPECT * floor,
                    scene.art.half_w
                );
            }
            widest = widest.max(h0 / floor);
        }
        // The narrowest such box: the widest-fov scene sits exactly on it.
        assert!(
            (GLUE_BOX_ASPECT - widest).abs() < 5e-4,
            "the frame is {GLUE_BOX_ASPECT}, the narrowest box no scene pays for is {widest}"
        );
    }

    /// The 1.93 aspect gives an odd leftover at 3440×1440 (`330 | 2779 | 331`), the case the two
    /// bars exist for.
    #[test]
    fn the_chrome_canvas_is_the_cameras_own_box() {
        use bevy::window::WindowResolution;

        // The real frame at 3440×1440.
        assert_eq!(
            glue_box_physical(3440, 1440, glue_box_aspect(3440.0 / 1440.0)),
            Some((516, 2408))
        );

        let (x, box_w) = glue_box_physical(3440, 1440, Some(1.93)).expect("21:9 boxes the gate");
        assert_eq!((x, box_w), (330, 2779));
        assert_eq!(
            3440 - (x + box_w),
            331,
            "an odd leftover leaves the box one pixel off centre — the canvas follows it"
        );

        let win = |sf: Option<f32>| Window {
            resolution: match sf {
                Some(sf) => WindowResolution::new(3440, 1440).with_scale_factor_override(sf),
                None => WindowResolution::new(3440, 1440),
            },
            ..default()
        };
        assert_eq!(
            glue_canvas_bars(Some(&win(None)), Some(1.93)),
            (330.0, 331.0),
            "at scale 1 the logical bars are the physical ones"
        );
        assert_eq!(
            glue_canvas_bars(Some(&win(Some(2.0))), Some(1.93)),
            (165.0, 165.5),
            "on a 2x display Val::Px is logical: half the physical bar"
        );
        // Unboxed, and no window at all: no inset.
        assert_eq!(glue_canvas_bars(Some(&win(None)), None), (0.0, 0.0));
        assert_eq!(glue_canvas_bars(None, Some(1.93)), (0.0, 0.0));
        // A degenerate aspect boxes to a 1 px scene rather than panicking or wrapping.
        assert_eq!(
            glue_box_physical(3440, 1440, Some(f32::NAN)),
            Some((1719, 1))
        );
        assert_eq!(glue_box_physical(0, 0, Some(1.93)), Some((0, 1)));
    }

    #[test]
    fn the_box_the_camera_renders_is_the_box_the_chrome_lays_out_in() {
        for (w, h) in [(3440, 1440), (2560, 1080), (5120, 1440), (3439, 1440)] {
            let boxed =
                glue_box_aspect(w as f32 / h as f32).expect("every window past the frame is boxed");
            let (x, box_w) = glue_box_physical(w, h, Some(boxed)).expect("boxed");
            let (left, right) = glue_canvas_bars(
                Some(&Window {
                    resolution: bevy::window::WindowResolution::new(w, h),
                    ..default()
                }),
                Some(boxed),
            );
            assert_eq!((left as u32, right as u32), (x, w - x - box_w), "{w}x{h}");
            assert!(
                left + right > 0.0 && (box_w as f32) < w as f32,
                "{w}x{h}: the bars are real"
            );
            assert_eq!(
                x + box_w + (right as u32),
                w,
                "{w}x{h}: no pixel unaccounted"
            );
        }
    }

    #[test]
    fn art_inside_the_authored_box_never_zooms_past_the_composition() {
        let close = |a: f32, b: f32| (a - b).abs() < 5e-4;
        let authored = diag_to_vert(1.0, GLUE_AUTHORED_ASPECT);
        let none = Some(ArtExtent {
            half_w: 0.0,
            half_h: 0.0,
        });
        assert!(close(
            glue_scene_framing(1.0, GLUE_AUTHORED_ASPECT, none),
            authored
        ));
        assert!(close(glue_scene_framing(1.0, 1.0, none), authored));
        // At the frame's aspect the vertical is on the floor and the width still covers 4:3.
        let t0 = (authored * 0.5).tan();
        let floor = glue_zoom_floor(1.0);
        let v = glue_scene_framing(1.0, GLUE_BOX_ASPECT, none);
        assert!(close((v * 0.5).tan(), floor));
        assert!(
            (v * 0.5).tan() * GLUE_BOX_ASPECT >= t0 * GLUE_AUTHORED_ASPECT - 1e-4,
            "the frame stopped inside the authored width"
        );
        // 16:10 still fills the window: it is narrower than the frame.
        assert!(glue_box_aspect(1.6).is_none());
        // A narrow window with bounded art height holds that height, cropping the sides.
        let short = Some(ArtExtent {
            half_w: 10.0,
            half_h: t0 * 1.1,
        });
        let v = glue_scene_framing(1.0, 0.75, short);
        assert!(
            close((v * 0.5).tan(), t0 * 1.1),
            "height held at the art's edge"
        );
        assert!(
            glue_scene_framing(1.0, 0.75, None) > v,
            "…below the unbounded law's"
        );
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
