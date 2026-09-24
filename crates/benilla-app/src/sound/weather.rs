//! Weather sound (decision 0070 slice 4, re-founded by 0338): the wire's SoundEntries kit
//! (`SMSG_WEATHER.soundId` — in 1.12 a real loop kit, 8533..8558, vmangos `Weather::GetSound`;
//! 0 = clear) is **not its own loop** — it is an *input to the zone-ambience selector*. The client
//! runs ONE ambience channel: its selector (`0x460bd0`) returns the raw weather SoundEntries while
//! the **zonetext indoor bit is clear**, and the area's own `AmbienceID` row while it is set — the
//! keep-flag `[0xb06d44]`, written `dl=1` by the indoor area feeder (`0x67e7d4`) and `dl=0` by the
//! outdoor one (`0x67e919`); every swap rides the standard **5.0 s crossfade** (`0x460b00`). That
//! content REPLACE — never a volume duck — is why rain goes quiet inside the Goldshire inn. The
//! packet's `grade` drives **rendering only** (`0x67baf0`: grade never reaches a volume call).
//!
//! This module only publishes the current wire kit ([`WeatherAmbience`]); the channel — selection,
//! crossfades, volumes — is `super::zone`'s ambience machine, the benilla twin of `0x460b00`.

use bevy::prelude::*;

use benilla_world::schedule::WorldStage;
use benilla_world::weather::WeatherMessage;

/// The weather loop's SoundEntries kit from the last `SMSG_WEATHER` (0 = clear skies). Consumed by
/// the ambience selector in [`super::zone`].
#[derive(Resource, Default)]
pub(super) struct WeatherAmbience(pub(super) u32);

/// Track the wire's weather sound kit.
fn track_weather_kit(mut kit: ResMut<WeatherAmbience>, mut msgs: MessageReader<WeatherMessage>) {
    for m in msgs.read() {
        if kit.0 != m.sound_id {
            info!("weather sound kit {} (grade {:.2})", m.sound_id, m.grade);
            kit.0 = m.sound_id;
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WeatherAmbience>()
        .add_systems(Update, track_weather_kit.in_set(WorldStage::Present));
}
