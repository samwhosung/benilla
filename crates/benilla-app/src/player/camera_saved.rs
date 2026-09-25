//! The per-character camera pose, as the reference keeps it in
//! `WTF/Account/<ACC>/<REALM>/<CHAR>/camera-settings.txt`:
//!
//! ```text
//! cameraDistance 16.068569
//! cameraPitch 13.449968
//! ```
//!
//! Two keys, LF, six decimals, a trailing newline (writer `0x50c4d0`, reader `0x50c5a0`), no yaw.
//! Ours is `benilla-config/camera/<realm>-<character>.txt`, written at logout, disconnect and
//! quit; the reference's UI teardown (`0x490bd0`) also writes it on `ReloadUI` and a UI-scale
//! change and reads it straight back, which ours does not. Read once per session, when the roster
//! names the character; the pitch reaches the camera through the login seize.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::char_select::{ClientState, InWorldGated};

use super::camera::{
    CameraControl, FlyCam, CAM_DIST_DEFAULT, CAM_DIST_MAX, CAM_DIST_MIN, CAM_PITCH_LIMIT,
};
use super::Player;
use benilla_world::view::WorldCamera;

/// The file's keys, the reference's spellings in its order.
const KEY_DISTANCE: &str = "cameraDistance";
const KEY_PITCH: &str = "cameraPitch";

/// The loaded character and its file. `identity` is the once-per-character latch; the save uses
/// `path`, since the roster may have moved on by `OnExit(InWorld)`.
#[derive(Resource, Default)]
pub(super) struct CameraPoseFile {
    identity: Option<(String, String)>,
    path: Option<PathBuf>,
}

/// A saved `cameraPitch` (degrees, positive looking down) as [`FlyCam::pitch`] (radians, positive
/// looking up). The reference converts without a sign flip (`× π/180` at `0x50c6f9`, `× 180/π` at
/// `0x50c54f` on save), its own pitch being positive-down; the flip is ours. The saved-view CVars
/// carry pitch in the same units (writer `0x50f990`, loader `0x50fb80`), so
/// [`super::camera_view`] converts through here too.
pub(super) fn pitch_from_file(degrees: f32) -> f32 {
    (-degrees.to_radians()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT)
}

/// The inverse of [`pitch_from_file`].
pub(super) fn pitch_to_file(radians: f32) -> f32 {
    -radians.to_degrees()
}

/// The pose as the reference writes it: `%f`'s six decimals, LF, a trailing newline.
fn render(distance: f32, pitch_radians: f32) -> String {
    format!(
        "{KEY_DISTANCE} {distance:.6}\n{KEY_PITCH} {:.6}\n",
        pitch_to_file(pitch_radians)
    )
}

/// `(distance, pitch_radians)`, each optional. As in the reference's reader (`0x50c5a0`), keys
/// match case-insensitively (`SStrCmpI 0x64a4c0`), either line ending reads, and an unknown key
/// is skipped.
///
/// Deviation: both values clamp to the rig's range, because a hand-edited file should not land a
/// pose play cannot reach; the reference stores them unbounded, the pitch until a mouse-look
/// re-clamps it.
fn parse(text: &str) -> (Option<f32>, Option<f32>) {
    let (mut distance, mut pitch) = (None, None);
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(value)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(v) = value.parse::<f32>() else {
            warn!("camera pose: unparseable {key} '{value}' ignored");
            continue;
        };
        if !v.is_finite() {
            continue;
        }
        if key.eq_ignore_ascii_case(KEY_DISTANCE) {
            distance = Some(v.clamp(CAM_DIST_MIN, CAM_DIST_MAX));
        } else if key.eq_ignore_ascii_case(KEY_PITCH) {
            pitch = Some(pitch_from_file(v));
        }
    }
    (distance, pitch)
}

/// Restores the pose once the roster names the character, the identity the macro and binding
/// loads use. All three rig distances are set, so the login opens at the zoom, not gliding to it.
fn load_camera_pose(
    roster: Res<crate::char_select::Roster>,
    mut file: ResMut<CameraPoseFile>,
    mut rig: ResMut<CameraControl>,
    mut cam: Query<&mut FlyCam, With<WorldCamera>>,
    mut player: ResMut<Player>,
) {
    let Some(id) = crate::ui_macro::identity(&roster) else {
        return;
    };
    if file.identity.as_ref() == Some(&id) {
        return; // already restored for this character
    }
    let Ok(mut cam) = cam.single_mut() else {
        return; // no camera yet: retry next frame, the identity still unlatched
    };
    file.path = crate::local_state::camera_character_path(&id.0, &id.1);
    file.identity = Some(id);
    // This character's pose starts from the shipped defaults, not from the last one's.
    rig.distance = CAM_DIST_DEFAULT;
    rig.target_distance = CAM_DIST_DEFAULT;
    rig.collision_distance = CAM_DIST_DEFAULT;
    player.login_pitch = None;

    let Some(path) = file.path.clone() else {
        return; // a hermetic capture or no install: the defaults stand
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!("camera pose: cannot read {}: {e}", path.display());
            return;
        }
    };
    let (distance, pitch) = parse(&text);
    if let Some(d) = distance {
        rig.distance = d;
        rig.target_distance = d;
        rig.collision_distance = d;
    }
    if let Some(p) = pitch {
        cam.pitch = p;
        player.login_pitch = Some(p);
    }
    info!("camera pose: restored from {}", path.display());
}

/// Writes the pose to the path the load resolved: by `OnExit(InWorld)` the roster is unwinding.
fn save(file: &CameraPoseFile, rig: &CameraControl, pitch: f32) {
    let Some(path) = &file.path else {
        return; // never loaded: a glue-only run or a hermetic capture
    };
    // `target_distance`: the live `distance` is the arm a wall has pulled in.
    let body = render(rig.target_distance, pitch);
    if let Err(e) = crate::local_state::write_atomic(path, &body) {
        warn!("camera pose: cannot write {}: {e}", path.display());
    }
}

/// `OnExit(InWorld)`: `/logout` back to the glue, or a disconnect.
fn save_on_session_end(
    mut file: ResMut<CameraPoseFile>,
    rig: Res<CameraControl>,
    cam: Query<&FlyCam, With<WorldCamera>>,
) {
    if let Ok(cam) = cam.single() {
        save(&file, &rig, cam.pitch);
    }
    // The next login re-reads, a relog of this character included; without a path, a quit from
    // the character screen cannot save the glue camera's pitch over this one's.
    file.identity = None;
    file.path = None;
}

/// `AppExit`, read as a message because a quit from in-world never leaves `InWorld`.
fn save_on_exit(
    file: Res<CameraPoseFile>,
    rig: Res<CameraControl>,
    cam: Query<&FlyCam, With<WorldCamera>>,
    mut exits: MessageReader<AppExit>,
) {
    if exits.read().next().is_none() {
        return;
    }
    if let Ok(cam) = cam.single() {
        save(&file, &rig, cam.pitch);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CameraPoseFile>()
        .add_systems(
            Update,
            // Before the controller, so the first in-world frame renders the restored pose. A
            // capture resolves no state path, so this is inert there.
            load_camera_pose.before(super::control).in_set(InWorldGated),
        )
        .add_systems(OnExit(ClientState::InWorld), save_on_session_end);
    // On the exit edge, never `Update`: the close button's `AppExit` is written in `PostUpdate`.
    crate::shutdown::on_app_exit(app, save_on_exit.into_configs());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The values come from a `camera-settings.txt` the reference wrote.
    #[test]
    fn the_rendered_file_matches_the_references_shape() {
        let text = render(16.068_57, pitch_from_file(13.449_968));
        assert_eq!(text, "cameraDistance 16.068569\ncameraPitch 13.449968\n");
    }

    /// The file's positive pitch looks down and ours looks up, so the sign inverts on the way in.
    #[test]
    fn the_pose_round_trips() {
        let pitch = pitch_from_file(24.2);
        assert!(pitch < 0.0, "a positive saved pitch looks DOWN for us");
        let (d, p) = parse(&render(17.509_666, pitch));
        assert_eq!(d, Some(17.509_666));
        assert!((p.unwrap() - pitch).abs() < 1e-4, "{p:?} vs {pitch}");
        // The reference's one negative sample, the camera slightly below and looking up.
        let (_, up) = parse("cameraPitch -4.749999\n");
        assert!(up.unwrap() > 0.0);
    }

    #[test]
    fn a_hand_edited_file_cannot_land_an_illegal_pose() {
        let (d, p) = parse("cameraDistance 999\ncameraPitch 400\n");
        assert_eq!(d, Some(CAM_DIST_MAX));
        assert_eq!(p, Some(-CAM_PITCH_LIMIT));
        let (d, p) = parse("cameraDistance -3\n");
        assert_eq!(
            (d, p),
            (Some(CAM_DIST_MIN), None),
            "one key restores one half"
        );
        let (d, p) = parse("cameraDistance banana\nfutureKey 1\n\ncameraPitch nan\n");
        assert_eq!((d, p), (None, None), "junk and unknown keys are skipped");
    }

    /// The reference's reader matches keys case-insensitively (`SStrCmpI`) and takes CRLF.
    #[test]
    fn a_hand_written_file_may_shout_its_keys_and_use_crlf() {
        let (d, p) = parse("CAMERADISTANCE 12.5\r\ncamerapitch 10.0\r\n");
        assert_eq!(d, Some(12.5));
        assert!((p.unwrap() - pitch_from_file(10.0)).abs() < 1e-6);
    }
}

#[cfg(test)]
mod login_tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::char_select::Roster;
    use crate::local_state::test_env::{EnvGuard, ENV_LOCK};

    fn roster(name: &str, guid: u64) -> Roster {
        Roster::with_pending_pick(
            vec![benilla_protocol::Character {
                guid,
                name: name.into(),
                race: 1,
                class: 1,
                gender: 0,
                skin: 0,
                face: 0,
                hair_style: 0,
                hair_color: 0,
                facial_hair: 0,
                level: 1,
                zone: 0,
                map: 0,
                position: benilla_protocol::wire::Vector3d {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                flags: 0,
                equipment: [benilla_protocol::CharEnumItem::default(); 19],
                pet_display_id: 0,
                pet_level: 0,
                pet_family: 0,
            }],
            guid,
        )
    }

    #[test]
    fn the_saved_pitch_is_the_login_pitch_and_a_fresh_alt_gets_the_defaults() {
        let _l = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = std::env::temp_dir().join(format!("benilla-cam-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let _c = EnvGuard::unset("WOW_CAPTURE");
        let _h = EnvGuard::set("BENILLA_HOME", tmp.to_str().unwrap());
        let path = crate::local_state::camera_character_path("Realm", "Tank").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, render(8.0, -0.3)).unwrap();

        let mut app = App::new();
        app.init_resource::<CameraPoseFile>()
            .init_resource::<CameraControl>()
            .init_resource::<Player>()
            .insert_resource(roster("Tank", 7));
        app.world_mut().spawn((
            FlyCam {
                yaw: 0.0,
                pitch: 0.0,
                speed: 1.0,
            },
            WorldCamera,
        ));
        app.world_mut().run_system_once(load_camera_pose).unwrap();
        let restored = app.world().resource::<Player>().login_pitch;
        assert!(
            restored.is_some_and(|p| (p + 0.3).abs() < 1e-3),
            "the file's pitch is what the seize will seat: {restored:?}"
        );
        assert_eq!(app.world().resource::<CameraControl>().target_distance, 8.0);

        // The session ends; an alt with no file logs in.
        app.world_mut()
            .run_system_once(save_on_session_end)
            .unwrap();
        *app.world_mut().resource_mut::<Player>() = Player::default();
        app.insert_resource(roster("Healer", 8));
        app.world_mut().run_system_once(load_camera_pose).unwrap();
        assert_eq!(app.world().resource::<Player>().login_pitch, None);
        assert_eq!(
            app.world().resource::<CameraControl>().target_distance,
            CAM_DIST_DEFAULT,
            "a fresh alt opens at the shipped zoom, not the tank's"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
