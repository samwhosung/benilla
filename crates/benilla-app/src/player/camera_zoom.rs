//! The zoom: the orbit distance's range, from `cameraDistanceMax` × `cameraDistanceMaxFactor`
//! under the reference's 0 and 50 yd bounds, and the wheel's glide to the chosen distance.

use bevy::prelude::*;

use super::camera::CameraControl;

/// The zoom floor, yd (`ds:0x8089a8` = 0.0): at 0 the eye sits at the framing pivot, inside the
/// head, and the avatar fades out. Deviation: the starting zoom, [`CAM_DIST_DEFAULT`], is 15 yd
/// where the reference's `cameraDistance` is 5.55 (`0x84f488`), for a wider view.
pub(super) const CAM_DIST_MIN: f32 = 0.0;
/// The zoom ceiling, yd (`ds:0x8089a4` = 50.0): the cap on [`ZoomLimit`] (`0x511309`) and the top
/// of the distance validator `0x50b310`; also the clamp for a distance read back off disk.
pub(super) const CAM_DIST_MAX: f32 = 50.0;
pub(super) const CAM_DIST_DEFAULT: f32 = 15.0;
/// `cameraDistanceMax`'s registered default, "15.0" (`0x84fbd0`).
const CAM_DISTANCE_MAX_DEFAULT: f32 = 15.0;
/// `cameraDistanceMaxFactor`'s registered default, "1.0" (`0x82e92c`). The CVar has no validator:
/// the options slider's 1 to 2 (`UIOptionsFrame.lua:90`) bounds the slider, not the value.
const CAM_DISTANCE_MAX_FACTOR_DEFAULT: f32 = 1.0;
/// `cameraDistanceMax`'s validator (`0x50b310` → `0x50b330(v, 0.0, 50.0)`) accepts `0 ≤ v ≤ 50`;
/// anything else, NaN included (`fcom` unordered sets C0), is refused and the old value stands.
pub(crate) const CAM_DISTANCE_MAX_RANGE: std::ops::RangeInclusive<f32> =
    CAM_DIST_MIN..=CAM_DIST_MAX;
/// Yards per wheel notch, the stock bindings' `CameraZoomIn(1.0)` (`Bindings.xml:707`).
const CAM_ZOOM_STEP: f32 = 1.0;
/// Zoom speed in yd/s, `cameraDistanceMoveSpeed`'s default: the reference glides the distance to
/// the wheel target at this constant speed (`0x5112d0`), not an ease.
const CAM_MOVE_SPEED: f32 = 8.33;

/// The max orbit distance, yd, from `cameraDistanceMax` and `cameraDistanceMaxFactor`: 15 at the
/// reference's defaults. Lowering it pulls the live target in on the next frame.
#[derive(Resource)]
pub(crate) struct ZoomLimit {
    /// The live limit, [`orbit_limit`] of the two below: in `[0, 50]`, never NaN.
    pub(crate) max: f32,
    distance_max: f32,
    factor: f32,
}

impl Default for ZoomLimit {
    fn default() -> Self {
        let (distance_max, factor) = (CAM_DISTANCE_MAX_DEFAULT, CAM_DISTANCE_MAX_FACTOR_DEFAULT);
        Self {
            max: orbit_limit(distance_max, factor),
            distance_max,
            factor,
        }
    }
}

impl ZoomLimit {
    /// `cameraDistanceMax`, through its validator's range ([`CAM_DISTANCE_MAX_RANGE`]); `false`
    /// when refused, the old value standing.
    pub(crate) fn set_distance_max(&mut self, v: f32) -> bool {
        if !CAM_DISTANCE_MAX_RANGE.contains(&v) {
            return false;
        }
        self.distance_max = v;
        self.max = orbit_limit(self.distance_max, self.factor);
        true
    }

    /// `cameraDistanceMaxFactor`, taken as it comes: the reference registers it with no validator.
    pub(crate) fn set_factor(&mut self, factor: f32) {
        self.factor = factor;
        self.max = orbit_limit(self.distance_max, self.factor);
    }

    #[cfg(test)]
    pub(crate) fn distance_max(&self) -> f32 {
        self.distance_max
    }

    #[cfg(test)]
    pub(crate) fn factor(&self) -> f32 {
        self.factor
    }
}

/// The reference's orbit limit (`0x5112d6`–`0x51131e`): `cameraDistanceMax × cameraDistanceMaxFactor`
/// through two ordered x87 compares. Below 0.0 (`fcom ds:0x8089a8`, C0 alone) it is 0.0; else
/// below 50.0 (`fcom ds:0x8089a4`, C0 alone) it stands; else it is 50.0. A NaN product compares
/// unordered (C0, C2 and C3 all set), which passes the first test and fails the second: 50.0.
fn orbit_limit(distance_max: f32, factor: f32) -> f32 {
    let product = distance_max * factor;
    if product < CAM_DIST_MIN {
        CAM_DIST_MIN
    } else if product < CAM_DIST_MAX {
        product
    } else {
        CAM_DIST_MAX
    }
}

/// Wheel zoom, run every frame: `CAMERAZOOMIN`/`OUT` move the target, and the distance glides to it
/// at a constant `cameraDistanceMoveSpeed`, as the reference's does. `scroll` is this frame's net
/// zoom-in in notches (positive is closer). `max` is [`ZoomLimit::max`], CVar-fed, so it bounds by
/// `max`/`min`, never `f32::clamp`, which panics on a NaN or inverted bound.
pub(super) fn apply_zoom_scroll(scroll: f32, dt: f32, rig: &mut CameraControl, max: f32) {
    if scroll != 0.0 {
        rig.target_distance = (rig.target_distance - scroll * CAM_ZOOM_STEP)
            .max(CAM_DIST_MIN)
            .min(max);
    }
    // Re-clamp every frame, so lowering the max-distance slider pulls the camera in.
    rig.target_distance = rig.target_distance.min(max);
    let max_step = CAM_MOVE_SPEED * dt;
    rig.distance += (rig.target_distance - rig.distance).clamp(-max_step, max_step);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(distance_max: f32, factor: f32) -> f32 {
        let mut z = ZoomLimit::default();
        assert!(
            z.set_distance_max(distance_max),
            "{distance_max} is in range"
        );
        z.set_factor(factor);
        z.max
    }

    #[test]
    fn the_limit_is_the_product_of_both_cvars() {
        assert_eq!(ZoomLimit::default().max, 15.0, "the reference's defaults");
        // The factor has no validator: 3 is not held to the slider's 2.
        assert_eq!(limit(15.0, 3.0), 45.0);
        assert_eq!(limit(15.0, 0.5), 7.5, "nor raised to the slider's 1");
        assert_eq!(limit(25.0, 1.0), 25.0);
    }

    #[test]
    fn the_limit_holds_to_zero_and_fifty() {
        assert_eq!(limit(15.0, 4.0), 50.0);
        assert_eq!(limit(50.0, 1.0), 50.0);
        assert_eq!(limit(15.0, f32::INFINITY), 50.0);
        assert_eq!(limit(15.0, -2.0), 0.0);
        assert_eq!(limit(15.0, f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn a_nan_product_fails_both_compares_and_is_fifty() {
        assert_eq!(limit(15.0, f32::NAN), 50.0);
        // 0 × ∞ is NaN too, with both values accepted.
        assert_eq!(limit(0.0, f32::INFINITY), 50.0);
    }

    #[test]
    fn cameradistancemax_is_refused_outside_its_validator_range() {
        let mut z = ZoomLimit::default();
        for bad in [-0.5, 50.5, f32::NAN, f32::INFINITY] {
            assert!(!z.set_distance_max(bad), "{bad} is refused");
            assert_eq!(
                (z.distance_max(), z.max),
                (15.0, 15.0),
                "the old value stands"
            );
        }
        assert!(
            z.set_distance_max(0.0) && z.max == 0.0,
            "both ends are inclusive"
        );
        assert!(z.set_distance_max(50.0) && z.max == 50.0);
    }

    #[test]
    fn a_nan_factor_zooms_without_panicking() {
        let mut z = ZoomLimit::default();
        z.set_factor(f32::NAN);
        let mut rig = CameraControl {
            target_distance: 10.0,
            distance: 10.0,
            ..default()
        };
        apply_zoom_scroll(-1.0, 0.1, &mut rig, z.max);
        assert_eq!(rig.target_distance, 11.0);
        // Even a NaN bound handed straight in bounds nothing and panics nowhere.
        apply_zoom_scroll(-1.0, 0.1, &mut rig, f32::NAN);
        assert_eq!(rig.target_distance, 12.0);
    }

    #[test]
    fn the_wheel_stops_at_the_limit() {
        let mut rig = CameraControl {
            target_distance: 44.5,
            ..default()
        };
        apply_zoom_scroll(-1.0, 0.1, &mut rig, limit(15.0, 3.0));
        assert_eq!(rig.target_distance, 45.0);
        apply_zoom_scroll(1000.0, 0.1, &mut rig, 45.0);
        assert_eq!(rig.target_distance, CAM_DIST_MIN);
    }
}
