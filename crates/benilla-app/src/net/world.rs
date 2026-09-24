//! The bridge's own world feed (in the net handler table since 2325, moved out of the drain's
//! world arm file) — what the server pushes about the *world* rather than about an entity or a
//! window, forwarded by the bridge because what receives it is not an app subsystem: the weather
//! change goes to the world crate's weather as a [`WeatherMessage`], and the world-state table the
//! UI's `$<n>w` tokens read is a cache the bridge initialises ([`WorldStates`]). Registered from
//! [`super::NetPlugin`].

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_world::weather::WeatherMessage;
use bevy::prelude::*;

use super::NetHandlerApp;
use crate::world_state::WorldStates;

/// Register the two handlers — called from [`super::NetPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::Weather, on_weather)
        .net_handler(K::WorldStates, on_world_states);
}

fn on_weather(In(ev): In<SessionEvent>, mut out: MessageWriter<WeatherMessage>) {
    if let SessionEvent::Weather {
        weather_type,
        grade,
        sound_id,
        instant,
    } = ev
    {
        weather(weather_type, grade, sound_id, instant, &mut out);
    }
}

fn on_world_states(In(ev): In<SessionEvent>, mut states: ResMut<WorldStates>) {
    if let SessionEvent::WorldStates {
        scope,
        states: pairs,
    } = ev
    {
        world_states(scope, pairs, &mut states);
    }
}

/// `SMSG_WEATHER` — the zone's weather change (`instant` on zone entry, a ramp otherwise).
fn weather(
    weather_type: u32,
    grade: f32,
    sound_id: u32,
    instant: bool,
    out: &mut MessageWriter<WeatherMessage>,
) {
    out.write(WeatherMessage {
        weather_type,
        grade,
        sound_id,
        instant,
    });
}

/// `SMSG_INIT_WORLD_STATES` / `SMSG_UPDATE_WORLD_STATE` — both wires funnel into the one setter,
/// as the reference's own handler does.
///
/// An **init clears the table first** and records its `(map, zone)` as the world-state UI's display
/// filter — the reference's `0x4c5650`, which runs before the pair loop (rationale on
/// [`crate::world_state`]). The order below
/// is that handler's: clear + scope, then the packet's pairs.
fn world_states(
    scope: Option<(u32, u32)>,
    states: Vec<(u32, u32)>,
    world_states: &mut WorldStates,
) {
    if let Some((map, zone)) = scope {
        debug!(
            "world states: map {map} zone {zone}, {} entries",
            states.len()
        );
        world_states.init_scope(map, zone);
    }
    world_states.write(&states);
}
