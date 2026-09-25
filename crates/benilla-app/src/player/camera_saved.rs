//! The **camera pose** the client remembers — orbit distance and pitch, per
//! character. The fourth resident of 0954's folder to be character-scoped, and the smallest: two
//! floats.
//!
//! The reference keeps exactly these two, in `WTF/Account/<ACC>/<REALM>/<CHAR>/camera-settings.txt`
//! — read out of a real 26-character tree (decision 1128's persistence-surface map):
//!
//! ```text
//! cameraDistance 16.068569
//! cameraPitch 13.449968
//! ```
//!
//! Two keys, LF, six decimals, trailing newline — VERIFIED at the writer (`0x50c4d0`) and reader
//! (`0x50c5a0`). **No yaw**: the live heading is never persisted anywhere. (The
//! `SaveView(2..5)` custom views *are* persisted, but as archived `config.wtf` CVars —
//! `cameraYaw`/`cameraYawA..D` and their Distance/Pitch — not in this file. 1131 §3 said otherwise;
//! 1138 corrects it. benilla has no `SaveView`, so nothing is owed here yet.)
//!
//! benilla writes the same two keys in the same order to
//! `benilla-config/camera/<realm>-<character>.txt` ([`crate::local_state::camera_character_path`]) — a file
//! that stays readable beside its ancestor.
//!
//! **Character-scoped, deliberately.** It is where the reference puts it, and it is what the setting
//! means: a gnome and a tauren want different zooms, and a tank and a healer want different pitches.
//! (The minimap's zoom, the other half of 1131, is install-scoped for the same reason the reference
//! makes it a CVar: it is about the map widget, not the character in front of it.)
//!
//! **Written at the session edges** — `OnExit(InWorld)` (a `/logout`, a disconnect) and `AppExit`
//! (quit) — the same two edges the saved variables use ([`crate::ui_saved`]), and the same posture
//! the reference has for its caches: no autosave, no dirty bit. A camera pose is a thing you settle
//! into once and leave; there is nothing an intermediate write would preserve. The reference's write
//! set is the UI-shutdown root set at `0x490bd0`, which is *wider* than ours — it also fires on
//! `/reload`, `/console reloadUI` and a UI-scale change, each of which then reads the file straight
//! back (1138). benilla has no UI reload, so there is no edge here to miss; when one lands it joins
//! this list.
//!
//! **Read once per session**, when the roster names the character we are entering the world as —
//! the macro/binding load's own seam ([`crate::ui_macro::identity`]). Absent file = the shipped
//! defaults, which is the normal first run: the rig is re-seated at them first, so a fresh alt does
//! not open at the zoom of whoever played last. The pitch reaches the camera through the login
//! seize ([`super::Player::login_pitch`]), which is the one place the opening pitch is seated.

use std::path::PathBuf;

use bevy::prelude::*;

use crate::char_select::{ClientState, InWorldGated};

use super::camera::{
    CameraControl, FlyCam, CAM_DIST_DEFAULT, CAM_DIST_MAX, CAM_DIST_MIN, CAM_PITCH_LIMIT,
};
use super::Player;
use benilla_world::view::WorldCamera;

/// The persisted pose's file keys — the reference's own spellings, in the reference's own order.
const KEY_DISTANCE: &str = "cameraDistance";
const KEY_PITCH: &str = "cameraPitch";

/// Which character's file we are on, and where it lives. `identity` doubles as the once-per-
/// character latch (the macro/binding load's pattern), and it is what the *save* keys off — by the
/// time `OnExit(InWorld)` fires, the roster may already have moved on.
#[derive(Resource, Default)]
pub(super) struct CameraPoseFile {
    identity: Option<(String, String)>,
    path: Option<PathBuf>,
}

/// The persisted `cameraPitch` → the live [`FlyCam::pitch`].
///
/// **Both halves VERIFIED** at the bytes (1131 §2.1 had the right conversion for the
/// wrong reason). The file is **degrees**, and the client's own reader is a pure unit conversion
/// with **no sign flip**: `deg × 0.01745329238474369` at `0x50c6f9` on load, `× 57.295780181884766`
/// at `0x50c54f` on save. The client's internal pitch is itself **positive = looking down** — its
/// forward is `(cos y·cos p, sin y·cos p, −sin p)` with `eye = pivot − dist·forward`, so `p > 0`
/// puts the eye above the pivot.
///
/// The negation below is therefore ours, not the file's: **[`FlyCam::pitch`] is positive = looking
/// UP.** Our forward is `Quat::from_euler(YXZ, yaw, pitch, 0)` applied to `−Z`, whose Y component is
/// `+sin(pitch)`, and `camera::control` seats the camera at `pivot − forward·distance`. Two opposite
/// conventions meeting, which is exactly what these two functions exist to bridge.
///
/// **Two carriers, one encoding.** The saved camera views ([`super::camera_view`]) store their
/// pitch in the same units through a different door — the archived `cameraPitch{,A..D}` CVars,
/// whose writer `0x50f990` scales the *same* internal field by `180/pi` before its `"%f"`
/// (`0x50f9dc`) and whose loader `0x50fb80` multiplies back by `pi/180` (`0x50fba3`). So these two
/// functions are the app's single bridge for both, and "file" in their names is the first carrier,
/// not the only one.
pub(super) fn pitch_from_file(degrees: f32) -> f32 {
    (-degrees.to_radians()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT)
}

/// The live [`FlyCam::pitch`] → the persisted `cameraPitch` (see [`pitch_from_file`]).
pub(super) fn pitch_to_file(radians: f32) -> f32 {
    -radians.to_degrees()
}

/// Render the pose exactly as the reference's writer does: two keys, one per line, `%f`'s six
/// decimals, LF, trailing newline.
fn render(distance: f32, pitch_radians: f32) -> String {
    format!(
        "{KEY_DISTANCE} {distance:.6}\n{KEY_PITCH} {:.6}\n",
        pitch_to_file(pitch_radians)
    )
}

/// Parse the two keys out of the file, each independently optional — a file with only one line
/// restores only that half, and an unknown key is skipped rather than failing the parse (a later
/// build's third key must not cost this build its zoom). Returns `(distance, pitch_radians)`.
///
/// Permissive in the same three ways the reference's reader is (`0x50c5a0`, VERIFIED 1138): keys
/// match **case-insensitively** (`SStrCmpI 0x64a4c0`), either line ending is fine (its tokenizer
/// splits on `"\r\n"`; `str::lines` strips the `\r`), and an unrecognised key is silently skipped.
///
/// One deliberate divergence: **we clamp, the client does not.** Its load path writes the parsed
/// float straight into the camera with no bound, so a hand-edited `cameraPitch 400` survives there
/// until the next mouse-look re-clamps it. Ours lands somewhere legal instead — a file is not a
/// gesture, and there is nothing to be faithful *to* in an unreachable pose.
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

/// Restore the pose once the roster names the character — the same identity the macro and binding
/// loads key off, so all three land on the same character on the same frame it becomes knowable.
///
/// Sets the rig's three distances together (`distance`, `target_distance`, `collision_distance`):
/// seeding only the target would make every login open with a visible glide out from the default 15,
/// which is precisely the "we forgot" the file exists to end.
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
        return; // the camera entity is not up yet — try again next frame, identity unlatched
    };
    file.path = crate::local_state::camera_character_path(&id.0, &id.1);
    file.identity = Some(id);
    // This character's pose starts from the shipped defaults, not from the last one's.
    rig.distance = CAM_DIST_DEFAULT;
    rig.target_distance = CAM_DIST_DEFAULT;
    rig.collision_distance = CAM_DIST_DEFAULT;
    player.login_pitch = None;

    let Some(path) = file.path.clone() else {
        return; // hermetic capture, or no install — session-only, defaults stand
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // Absent is the normal first-run case; anything else is worth a line.
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

/// Write the pose from the live rig. Keyed off the path the *load* resolved, not off the roster:
/// on `OnExit(InWorld)` the roster is already unwinding, and the character whose pose this is is the
/// one we logged in as.
fn save(file: &CameraPoseFile, rig: &CameraControl, pitch: f32) {
    let Some(path) = &file.path else {
        return; // never loaded (glue-only run, hermetic capture) — nothing to write back
    };
    // `target_distance`, never `distance`: the live one is the collision-pulled arm, so quitting
    // with your back to a wall would otherwise save the wall's zoom instead of your own.
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
    // The next login reads the file again, whoever it is — a reconnect or a relog of this same
    // character included, whose seize would otherwise open at the shipped pitch. The path goes
    // too: a quit from the character screen after this has no character's pose to write, and
    // would otherwise save the glue camera's pitch over this one's.
    file.identity = None;
    file.path = None;
}

/// `AppExit`: quitting the client. Reads the message rather than a state edge because a quit from
/// in-world never leaves `InWorld` ([`crate::ui_saved`]'s same reason).
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
            // Before the controller reads the rig, so the restored pose is what the first in-world
            // frame renders rather than something the player watches glide into place. Not gated on
            // capture mode: a capture resolves no state path at all (0954's hermetic rule), so this
            // is already inert there.
            load_camera_pose.before(super::control).in_set(InWorldGated),
        )
        .add_systems(OnExit(ClientState::InWorld), save_on_session_end);
    // The quit root goes on the exit edge, never `Update`: the close button's
    // `AppExit` is not written until `PostUpdate`, so an `Update` reader watched the pose die with
    // the process — a `/logout` saved it and quitting from in-world did not.
    crate::shutdown::on_app_exit(app, save_on_exit.into_configs());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file we write is byte-for-byte the reference's shape: its two keys, its order, its six
    /// decimals, LF, trailing newline — checked against a real `camera-settings.txt` the reference
    /// client wrote (`16.068569` / `13.449968`).
    #[test]
    fn the_rendered_file_matches_the_references_shape() {
        let text = render(16.068_57, pitch_from_file(13.449_968));
        assert_eq!(text, "cameraDistance 16.068569\ncameraPitch 13.449968\n");
    }

    /// Round trip: what we write parses back to what we had, through the sign flip and both clamps.
    ///
    /// The first assertion is the one 1138 settled: the file's positive pitch is the client's
    /// *looking down*, and OUR pitch is positive-up, so the stored sign must invert on the way in.
    #[test]
    fn the_pose_round_trips() {
        let pitch = pitch_from_file(24.2);
        assert!(pitch < 0.0, "a positive saved pitch looks DOWN for us");
        let (d, p) = parse(&render(17.509_666, pitch));
        assert_eq!(d, Some(17.509_666));
        assert!((p.unwrap() - pitch).abs() < 1e-4, "{p:?} vs {pitch}");
        // The reference's one negative sample survives too (camera slightly below, looking up).
        let (_, up) = parse("cameraPitch -4.749999\n");
        assert!(up.unwrap() > 0.0);
    }

    /// A hand-edited or truncated file never lands an illegal pose: each key is independent, the
    /// distance clamps to the zoom range, the pitch to ±89°, and junk is skipped rather than fatal.
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

    /// The reference's reader is case-insensitive on the key (`SStrCmpI`) and takes either line
    /// ending (1138). Only a hand edit can produce either — our own writer is exact — but matching
    /// it costs nothing and a file we refuse to read is a pose silently lost.
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

    /// **The restored pitch reaches the login seize, and an alt starts from the defaults.** The
    /// seize seated its own constant over the pitch this load had just restored, so only the
    /// distance half of the remembered pose ever survived a login; and a character with no file
    /// kept whatever zoom the previous one had left in the rig.
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
