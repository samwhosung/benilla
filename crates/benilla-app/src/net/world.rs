//! The bridge's world feed: weather goes to the world crate as a [`WeatherMessage`], world states
//! to the [`WorldStates`] table the UI's `$<n>w` tokens read.

use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_world::weather::WeatherMessage;
use bevy::prelude::*;

use super::NetHandlerApp;
use crate::world_state::WorldStates;

/// Registers the two handlers.
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

/// `SMSG_WEATHER`: the zone's weather change, `instant` on zone entry, a ramp otherwise.
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

/// `SMSG_INIT_WORLD_STATES` and `SMSG_UPDATE_WORLD_STATE` share one setter, as in the reference.
/// An init first clears the table and records `(map, zone)` as the UI's display filter, then
/// writes its pairs (reference: `0x4c5650`).
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
