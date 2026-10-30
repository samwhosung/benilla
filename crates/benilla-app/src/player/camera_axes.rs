//! The camera's six keyboard axes, which the `MoveView*` pairs start and stop (`0x5112d0`): zoom
//! in and out, turn right and left, look up and down. An active axis moves its target by a CVar
//! speed times the elapsed time, and the live value with it.

use benilla_ui::script::ViewAxis;

use super::camera::{CameraControl, FlyCam, LookConfig, CAM_PITCH_LIMIT};
use super::camera_zoom::CAM_DIST_MIN;

/// `0x5112d0`'s pitch rate is `cameraPitchMoveSpeed` × 0.00025 per millisecond (`0x808ac8` at
/// `0x511550`), a quarter of the yaw's 0.001 (`0x801360` at `0x51149b`): degrees a second × 0.25.
const PITCH_RATE_SCALE: f32 = 0.25;

/// One axis: `[cam+0x128]` bit `2i` (active) and `2i+1` (stopping), whether its start time is
/// this frame's, so the first tick moves nothing (`now − [cam+0x12c + 4i]`), and the seconds left
/// to its queued end (`[cam+0x15c + 4i]`), which a zoom sets.
#[derive(Clone, Copy, Default, Debug)]
struct Axis {
    active: bool,
    stopping: bool,
    fresh: bool,
    queued: Option<f32>,
}

/// The six axes, indexed as [`ViewAxis`].
#[derive(Default, Debug)]
pub(super) struct ViewAxes([Axis; 6]);

impl ViewAxes {
    /// `0x50fd40(axis, now, 0)`: set the active bit and the start time; a stopping bit already
    /// set stays, so a Stop then a Start before the tick still ends the axis.
    fn start(&mut self, axis: ViewAxis) {
        let a = &mut self.0[axis as usize];
        a.active = true;
        a.fresh = true;
        a.queued = None;
    }

    /// `CameraZoomIn`/`Out` (`0x50fc60`, `0x50fcd0`) on the distance axes: `yards` at `speed`, in
    /// whole milliseconds (`0x50fc6c`-`0x50fc79`), the opposite axis stopped first (`0x50fc83`),
    /// then added to the axis's queued end while it runs toward one (`0x50fca9`), else the axis
    /// started with that end (`0x50fcbe` → `0x50fd40`), none for a zero span (`0x50fd80`).
    pub(super) fn zoom(&mut self, out: bool, yards: f32, speed: f32) {
        let (axis, other) = if out {
            (ViewAxis::Out, ViewAxis::In)
        } else {
            (ViewAxis::In, ViewAxis::Out)
        };
        let secs = (yards / speed * 1000.0).trunc() / 1000.0;
        self.stop(other);
        let a = &mut self.0[axis as usize];
        match a.queued {
            Some(left) if a.active => a.queued = Some(left + secs),
            _ => {
                self.start(axis);
                self.0[axis as usize].queued = (secs != 0.0).then_some(secs);
            }
        }
    }

    /// `0x50fd90(axis, now)`: on an active axis, set the stopping bit and the end time.
    fn stop(&mut self, axis: ViewAxis) {
        let a = &mut self.0[axis as usize];
        if a.active {
            a.stopping = true;
        }
    }

    /// Apply this frame's `MoveView*` calls in call order.
    pub(super) fn apply(&mut self, calls: &[(ViewAxis, bool)]) {
        for &(axis, start) in calls {
            if start {
                self.start(axis);
            } else {
                self.stop(axis);
            }
        }
    }

    /// Whether `axis` is active, its bit `2i` of `[cam+0x128]`.
    #[cfg(test)]
    pub(super) fn active(&self, axis: ViewAxis) -> bool {
        self.0[axis as usize].active
    }

    /// Whether `axis` is stopping, its bit `2i + 1`.
    #[cfg(test)]
    pub(super) fn stopping(&self, axis: ViewAxis) -> bool {
        self.0[axis as usize].stopping
    }

    /// Whether any axis is active, the reference's `[cam+0x128] != 0` (`0x50ea48`).
    fn any(&self) -> bool {
        self.0.iter().any(|a| a.active)
    }

    /// Each active axis's seconds this tick, clearing the ones that stop: a continuing axis runs
    /// the frame, one started this frame runs nothing yet, and a stopping one runs to its end
    /// time, then both bits clear (`0x511372`-`0x5113a0`, `max(end − start, 0)`). A queued end
    /// reached stops its axis at the tick (`0x511341`-`0x511359`), which then runs whole.
    fn advance(&mut self, dt: f32) -> [f32; 6] {
        let mut secs = [0.0; 6];
        for (a, s) in self.0.iter_mut().zip(&mut secs) {
            if !a.active {
                continue;
            }
            if let Some(left) = a.queued.as_mut() {
                if !a.fresh {
                    *left -= dt;
                }
                if *left <= 0.0 {
                    a.stopping = true;
                }
            }
            *s = if a.fresh { 0.0 } else { dt };
            a.fresh = false;
            if a.stopping {
                *a = Axis::default();
            }
        }
        secs
    }
}

/// The axes' tick (`0x5112d0`), from the camera update while any is active. Distance moves
/// `cameraDistanceMoveSpeed` yards a second, held to `[0, max]` (`0x511402`, `0x51145e`); the yaw
/// `cameraYawMoveSpeed` degrees a second; the pitch a quarter of `cameraPitchMoveSpeed`, held to
/// ±89° (`0x8089d8`). The live value follows the target (`0x511422`, `0x51152f`, `0x5115d6`).
/// Here Right raises the yaw as the reference's `+0x210` rises (`0x5114a7`), both turning left as
/// they grow, so the camera swings to the character's right; Up raises the reference's pitch,
/// which looks down as it grows, so ours, up-positive, falls.
pub(super) fn tick(
    rig: &mut CameraControl,
    cam: &mut FlyCam,
    dt: f32,
    look: LookConfig,
    distance_speed: f32,
    max_distance: f32,
) {
    if !rig.view_axes.any() {
        return;
    }
    let secs = rig.view_axes.advance(dt);
    let at = |axis: ViewAxis| secs[axis as usize];
    let zoom = distance_speed * (at(ViewAxis::Out) - at(ViewAxis::In));
    if zoom != 0.0 {
        // `max`/`min`, never `clamp`: the limit is CVar-fed, as in the wheel's glide.
        let target = (rig.target_distance + zoom)
            .max(CAM_DIST_MIN)
            .min(max_distance);
        rig.distance = (rig.distance + (target - rig.target_distance))
            .max(CAM_DIST_MIN)
            .min(max_distance);
        rig.target_distance = target;
    }
    let yaw = look.yaw_speed.to_radians() * (at(ViewAxis::Right) - at(ViewAxis::Left));
    cam.yaw += yaw;
    let pitch = (look.pitch_speed * PITCH_RATE_SCALE).to_radians()
        * (at(ViewAxis::Down) - at(ViewAxis::Up));
    if pitch != 0.0 {
        cam.pitch = (cam.pitch + pitch).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> FlyCam {
        FlyCam {
            yaw: 0.0,
            pitch: 0.0,
            speed: 0.0,
        }
    }

    fn rig(distance: f32) -> CameraControl {
        CameraControl {
            distance,
            target_distance: distance,
            ..Default::default()
        }
    }

    /// Run `frames` ticks of `dt` with `calls` made before the first.
    fn run(
        rig: &mut CameraControl,
        cam: &mut FlyCam,
        calls: &[(ViewAxis, bool)],
        frames: usize,
        dt: f32,
    ) {
        rig.view_axes.apply(calls);
        for _ in 0..frames {
            tick(rig, cam, dt, LookConfig::default(), 8.33, 15.0);
        }
    }

    /// `0x5112d0` at the defaults: yaw 180°/s (`0x51149b`), pitch 90 × 0.25 = 22.5°/s
    /// (`0x511550`), distance 8.33 yd/s; the start frame runs nothing, each later one its `dt`.
    #[test]
    fn each_axis_moves_at_its_cvar_speed() {
        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Right, true)], 11, 0.1);
        assert!((c.yaw - 180f32.to_radians()).abs() < 1e-4, "{}", c.yaw);

        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Down, true)], 11, 0.1);
        assert!((c.pitch - 22.5f32.to_radians()).abs() < 1e-4, "{}", c.pitch);

        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::In, true)], 3, 0.1);
        assert!((r.target_distance - (10.0 - 2.0 * 0.833)).abs() < 1e-4);
        assert_eq!(
            r.distance, r.target_distance,
            "the live distance moves with it"
        );
    }

    /// Right raises the yaw and Left lowers it (`0x5114a7`, `0x5114f0`); Up lowers our pitch, the
    /// reference's rising (`0x51155c`), and Down raises it.
    #[test]
    fn right_and_up_move_the_camera_their_way() {
        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Left, true)], 2, 0.1);
        assert!(c.yaw < 0.0);
        run(&mut r, &mut c, &[(ViewAxis::Left, false)], 1, 0.1);
        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Up, true)], 2, 0.1);
        assert!(c.pitch < 0.0, "the camera climbs and looks down");
    }

    /// The stop runs its last stretch and clears the axis; later ticks move nothing.
    #[test]
    fn a_stop_ends_the_axis_after_its_last_stretch() {
        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Right, true)], 2, 0.1);
        run(&mut r, &mut c, &[(ViewAxis::Right, false)], 1, 0.1);
        let yaw = c.yaw;
        assert!((yaw - 2.0 * 18f32.to_radians()).abs() < 1e-4, "{yaw}");
        run(&mut r, &mut c, &[], 5, 0.1);
        assert_eq!(c.yaw, yaw);
        // A Stop on an idle axis sets nothing (`0x50fda3`).
        run(&mut r, &mut c, &[(ViewAxis::Left, false)], 1, 0.1);
        assert!(!r.view_axes.any());
    }

    /// A Stop then a Start before the tick keeps the stopping bit (`0x50fd40` only ors the
    /// active bit in), so the axis ends at once.
    #[test]
    fn a_stop_then_a_start_in_one_frame_ends_the_axis() {
        let (mut r, mut c) = (rig(10.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::Right, true)], 2, 0.1);
        run(
            &mut r,
            &mut c,
            &[(ViewAxis::Right, false), (ViewAxis::Right, true)],
            1,
            0.1,
        );
        assert!(!r.view_axes.any());
    }

    /// A zoom queues its span on its distance axis (`0x50fc60`): 1 yd at 8.33 yd/s is 120 ms,
    /// truncated (`0x50fc79`), and the axis stops at the first tick past it, which runs whole
    /// (`0x511359`). A zoom the opposite way stops it first (`0x50fc83`): of `In` then `Out` in
    /// one frame, the camera moves out alone. A second zoom the same way adds to the span
    /// (`0x50fca9`).
    #[test]
    fn a_zoom_runs_its_axis_for_its_span_and_stops_the_opposite_one() {
        let speed = 8.33;
        let tick = |r: &mut CameraControl, c: &mut FlyCam| {
            for _ in 0..10 {
                super::tick(r, c, 0.05, LookConfig::default(), speed, 15.0);
            }
        };
        let (mut r, mut c) = (rig(10.0), cam());
        r.view_axes.zoom(false, 1.0, speed);
        r.view_axes.zoom(true, 1.0, speed);
        tick(&mut r, &mut c);
        // The start tick runs nothing; the 0.12 s queued ends at the third tick after it.
        assert!((r.target_distance - (10.0 + 3.0 * 0.05 * speed)).abs() < 1e-4);
        assert!(!r.view_axes.any(), "both axes ended");

        let (mut r, mut c) = (rig(10.0), cam());
        r.view_axes.zoom(true, 1.0, speed);
        r.view_axes.zoom(true, 1.0, speed);
        tick(&mut r, &mut c);
        assert!((r.target_distance - (10.0 + 5.0 * 0.05 * speed)).abs() < 1e-4);
    }

    /// The Lua pairs reach the axes through the bindings' frame state, in call order.
    #[test]
    fn the_move_view_calls_reach_the_axes() {
        use benilla_ui::script::BindingInput;
        let binds = crate::bindings::BindingsState::after_calls(
            vec![BindingInput::ViewStart(ViewAxis::Right)],
            vec![
                BindingInput::ViewStop(ViewAxis::Right),
                BindingInput::ViewStart(ViewAxis::Up),
            ],
        );
        assert_eq!(
            binds.view_calls(),
            [(ViewAxis::Right, false), (ViewAxis::Up, true)],
            "the earlier frame's calls are spent"
        );
    }

    /// Distance holds to `[0, max]`, the pitch to ±89°, and opposite axes cancel.
    #[test]
    fn the_axes_hold_to_their_bounds() {
        let (mut r, mut c) = (rig(1.0), cam());
        run(&mut r, &mut c, &[(ViewAxis::In, true)], 10, 0.1);
        assert_eq!((r.distance, r.target_distance), (0.0, 0.0));
        run(
            &mut r,
            &mut c,
            &[(ViewAxis::In, false), (ViewAxis::Out, true)],
            40,
            0.1,
        );
        assert_eq!(r.target_distance, 15.0);
        run(&mut r, &mut c, &[(ViewAxis::Down, true)], 100, 0.1);
        assert_eq!(c.pitch, CAM_PITCH_LIMIT);
        let (mut r, mut c) = (rig(10.0), cam());
        run(
            &mut r,
            &mut c,
            &[(ViewAxis::Right, true), (ViewAxis::Left, true)],
            5,
            0.1,
        );
        assert_eq!(c.yaw, 0.0);
    }
}
