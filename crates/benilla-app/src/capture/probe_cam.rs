//! `WOW_PROBE_CAM`: parks the third-person camera at an absolute yaw, pitch and zoom so an
//! unattended probe can frame a subject; [`super::probe_look`] turns the avatar, never the pitch.
//!
//! Format: `WOW_PROBE_CAM="<yaw_deg>,<pitch_deg>[,<dist_yd>]@<start_s>[:<pan_deg_per_s>][;...]"`,
//! e.g. `"180,12,25@25"`: from 25 s on, yaw 180, 12 degrees up, 25 yd back. Yaw and pitch are
//! absolute (`+pitch` is up); an omitted distance is left alone. The pose is re-applied every
//! frame, so a teleport or reset cannot knock it off, and the latest armed entry wins. A pan turns
//! the yaw at a constant rate from the pose's start, for defects that only show with the camera
//! moving (z-fighting).
//!
//! It writes the rig fields directly, as a mouse-drag would, since faking mouse motion needs the
//! OS cursor inside an unfocused window.

use bevy::prelude::*;

use crate::player::camera::{CameraControl, FlyCam};
use benilla_world::schedule::WorldStage;

use super::ProbeClock;

/// One parked pose: yaw and pitch (radians), orbit distance (yards), start (s), yaw pan (rad/s).
struct Pose {
    yaw: f32,
    pitch: f32,
    distance: Option<f32>,
    at: f32,
    pan: f32,
}

#[derive(Resource)]
pub(crate) struct ProbeCam {
    poses: Vec<Pose>,
}

/// Parses `WOW_PROBE_CAM`; unparseable entries are dropped, and no pose leaves the probe inert.
pub(crate) fn from_env() -> Option<ProbeCam> {
    let spec = std::env::var("WOW_PROBE_CAM").ok()?;
    let poses: Vec<Pose> = spec
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let (pose, at) = s.split_once('@')?;
            let mut parts = pose.split(',').map(str::trim);
            let yaw = parts.next()?.parse::<f32>().ok()?;
            let pitch = parts.next()?.parse::<f32>().ok()?;
            // A third field is the orbit distance; a fourth is refused.
            let distance = match (parts.next(), parts.next()) {
                (None, _) => Some(None),
                (Some(d), None) => d.parse::<f32>().ok().map(Some),
                (Some(_), Some(_)) => None,
            }?;
            // `@<start>` or `@<start>:<pan>`, the `WOW_PROBE_LOOK` shape.
            let (at, pan) = match at.trim().split_once(':') {
                Some((at, pan)) => (at.trim(), pan.trim().parse::<f32>().ok()?),
                None => (at.trim(), 0.0),
            };
            Some(Pose {
                yaw: yaw.to_radians(),
                pitch: pitch.to_radians(),
                distance,
                at: at.parse::<f32>().ok()?,
                pan: pan.to_radians(),
            })
        })
        .collect();
    if poses.is_empty() {
        warn!("probe-cam: no usable pose in {spec:?} (want e.g. 180,12,25@25) — inert");
    }
    (!poses.is_empty()).then_some(ProbeCam { poses })
}

/// Holds the camera at the latest armed pose, before `control` so the same frame renders it.
/// Times and pan rates are wall-clock ([`ProbeClock`]), not `Res<Time>`, which clamps and freezes.
pub(crate) fn drive_probe_cam(
    probe: Res<ProbeCam>,
    time: ProbeClock,
    mut rig: ResMut<CameraControl>,
    mut cam: Query<&mut FlyCam, With<benilla_world::view::WorldCamera>>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
) {
    if self_player.is_empty() {
        return; // in-world only, like the other probes
    }
    let now = time.elapsed_secs();
    // Latest armed wins, so a `;` list reads as a timeline.
    let Some(pose) = probe.poses.iter().rfind(|p| now >= p.at) else {
        return;
    };
    let Ok(mut cam) = cam.single_mut() else {
        return;
    };
    // Absolute from the pose's start, so the sweep does not depend on frame times.
    cam.park(pose.yaw + pose.pan * (now - pose.at), pose.pitch);
    if let Some(d) = pose.distance {
        // `park_distance`, not the wheel target: the glide would ease back toward the old target.
        rig.park_distance(d);
    }
}

/// Registers `WOW_PROBE_CAM` in the same slot as [`super::probe_look::ProbeLookPlugin`]; inert
/// without the variable.
pub(crate) struct ProbeCamPlugin;

impl Plugin for ProbeCamPlugin {
    fn build(&self, app: &mut App) {
        let Some(cam) = from_env() else {
            return;
        };
        app.insert_resource(cam).add_systems(
            Update,
            drive_probe_cam
                .in_set(WorldStage::Input)
                .before(crate::player::PlayerControlSet)
                .in_set(crate::char_select::InWorldGated),
        );
    }
}
