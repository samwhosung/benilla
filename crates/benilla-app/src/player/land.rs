//! **Land here** — free-fly's missing other half: put the avatar where the camera is.
//!
//! `F` detaches the camera and the avatar freezes ([`super::camera::fly_free`]); the terrain
//! streamer follows the *camera* while detached, so you can fly anywhere and the world loads
//! around you — but there was no way to bring the body along. This is that: fly to a spot, ask to
//! land, and the avatar arrives there. Unreal's eject → possess, in our shape.
//!
//! **The teleport is the server's, not ours.** The ask goes out as the GM command `.go xyz x y z
//! <map>` (vmangos `HandleGoXYZCommand` → `HandleGoHelper`, which keeps the exact Z when one is
//! given and saves the recall position on the way), and the answer comes back as the ordinary
//! `MSG_MOVE_TELEPORT_ACK` this client already handles ([`super::wire_in`]) — so the destination
//! is one the server agrees with, zone/area/grid bookkeeping included, and nothing here invents a
//! pose on the wire. It also means the affordance needs the account's GM rights, which is the
//! default for this project's characters; a refused command shows up as the
//! timeout warning below rather than as silence.
//!
//! Two details worth stating, because both are choices:
//! - **The feet land at the camera point**, not the eye — the camera translation IS the
//!   destination, so you arrive standing where you were floating (a ~2 yd eye rise on arrival).
//! - **Re-attach waits for the teleport**, rather than happening at the ask. Staying detached
//!   through the round trip means the camera is already at the destination while the body flies
//!   to it, so the tiles are resident before the landing instead of streaming in under it.

use benilla_assets::coords::bevy_to_wow;
use bevy::prelude::*;

use crate::net::{ChatKind, ClientCommand, NetCommands, TeleportMessage, WorldportMessage};
use benilla_world::world_map::CurrentMap;

use super::state::Player;
use benilla_world::view::WorldCamera;

/// Ask to land the avatar at the free-flying camera. Written by the debug panel's **land here**
/// button; the dev chord's `G` is read directly by [`land_here`].
#[derive(Message)]
pub(crate) struct LandHere;

/// How long to wait for the server's teleport before giving up on the ask and saying so. Generous
/// next to a local round trip: the point is to name a *refused* command (no GM rights, coordinates
/// off the map), not to race the network.
const LAND_TIMEOUT: f32 = 5.0;

/// The land-here ask, and the re-attach that closes it. Runs before [`super::control`], so the
/// frame that applies the teleport is the frame that takes third-person control again.
// One system phase's input set, like the controller's own params.
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
    // The pending ask's give-up deadline. A `Local` rather than a resource because this system is
    // the only writer AND the only reader — nothing else in the app has an opinion about it.
    mut pending: Local<Option<f32>>,
) {
    // Drain both teleport readers every frame (own cursors — `wire_in` reads the same messages
    // through its own): any teleport while an ask is out is our landing.
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
