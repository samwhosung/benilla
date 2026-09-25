//! Land here: moves the avatar frozen by free-fly ([`super::camera::fly_free`]) to the camera
//! through the GM command `.go xyz x y z <map>` (vmangos `HandleGoXYZCommand`,
//! `TeleportCommands.cpp:854`, which keeps the given Z and saves the recall position), answered by
//! the ordinary teleport or worldport. Control re-attaches only then, so the destination's tiles
//! are resident before the landing.

use benilla_assets::coords::bevy_to_wow;
use bevy::prelude::*;

use crate::net::{ChatKind, ClientCommand, NetCommands, TeleportMessage, WorldportMessage};
use benilla_world::world_map::CurrentMap;

use super::state::Player;
use benilla_world::view::WorldCamera;

/// Asks to land the avatar at the free-flying camera, from the debug panel's button; the dev
/// chord's `G` is read directly by [`land_here`].
#[derive(Message)]
pub(crate) struct LandHere;

/// Seconds to wait for the teleport before reporting the command refused (no GM rights,
/// coordinates off the map).
const LAND_TIMEOUT: f32 = 5.0;

/// Sends the land-here ask and re-attaches when it lands. Runs before [`super::control`], so the
/// frame that applies the teleport takes third-person control again.
pub(crate) fn land_here(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut asks: MessageReader<LandHere>,
    mut player: ResMut<Player>,
    camera: Query<&Transform, With<WorldCamera>>,
    map: Option<Res<CurrentMap>>,
    net: Res<NetCommands>,
    mut teleports: MessageReader<TeleportMessage>,
    mut worldports: MessageReader<WorldportMessage>,
    // The pending ask's give-up deadline.
    mut pending: Local<Option<f32>>,
) {
    // Both readers drain every frame on their own cursors; a teleport while an ask is out is the
    // landing.
    let arrived = teleports.read().count() > 0 || worldports.read().count() > 0;
    let asked = asks.read().count() > 0 || crate::run_mode::dev_chord(&keys, KeyCode::KeyG);

    if let Some(deadline) = *pending {
        if arrived {
            *pending = None;
            player.detached = false;
            info!("land: arrived — third-person control is back");
        } else if time.elapsed_secs() > deadline {
            *pending = None;
            warn!(
                "land: no teleport came back within {LAND_TIMEOUT:.0}s — the `.go xyz` was refused \
                 (GM rights? coordinates off the map?). Still free-flying."
            );
        }
    }

    if !asked {
        return;
    }
    if !player.active {
        info!("land: not in the world yet — nothing to land");
        return;
    }
    if !player.detached {
        info!(
            "land: not free-flying — press {chord}+F, fly somewhere, then land",
            chord = benilla_world::modkeys::DEV_CHORD
        );
        return;
    }
    let Ok(cam) = camera.single() else {
        return;
    };
    let [x, y, z] = bevy_to_wow(cam.translation);
    let text = match map.as_ref() {
        Some(m) => format!(".go xyz {x:.2} {y:.2} {z:.2} {}", m.0),
        None => format!(".go xyz {x:.2} {y:.2} {z:.2}"),
    };
    info!("land: {text}");
    let _ = net.0.send(ClientCommand::Chat {
        kind: ChatKind::Say,
        target: None,
        text,
    });
    *pending = Some(time.elapsed_secs() + LAND_TIMEOUT);
}
