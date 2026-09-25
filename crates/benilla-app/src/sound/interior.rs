//! WMO interior audio: the `WMOAreaTable` row for the group the camera eye is in (exact group
//! row, then whole-WMO default, then name-set 0), published as an override layer. Zone music,
//! ambience, intro and reverb take its nonzero fields over the terrain `AreaTable` chain; zero
//! fields fall through to it (the reference's rule for a zero interior field is untraced).

use bevy::prelude::*;

use benilla_world::schedule::WorldStage;

/// The audio FKs of the interior the eye is in; `None` is the open world.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentInterior(pub(crate) Option<InteriorAudio>);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct InteriorAudio {
    /// `SoundProviderPreferences` FKs, `[dry, underwater]`.
    pub(crate) sound_provider: [u32; 2],
    /// `SoundAmbience` FK.
    pub(crate) ambience: u32,
    /// `ZoneMusic` FK.
    pub(crate) zone_music: u32,
    /// `ZoneIntroMusicTable` FK: the entry fanfare.
    pub(crate) intro_sound: u32,
}

/// Resolve the eye's interior keys to the audio row; log transitions by interior name.
fn resolve_interior(
    world: benilla_world::world_point::WorldPoint,
    mut current: ResMut<CurrentInterior>,
) {
    let row = world.interior_row();
    let audio = row.as_ref().map(|r| InteriorAudio {
        sound_provider: r.sound_provider,
        ambience: r.ambience,
        zone_music: r.zone_music,
        intro_sound: r.intro_sound,
    });
    if current.0 != audio {
        match &row {
            Some(r) if !r.name.is_empty() => info!("interior: {}", r.name),
            Some(_) => info!("interior: (unnamed)"),
            None => info!("interior: outside"),
        }
        current.0 = audio;
    }
}

/// `OnExit(InWorld)`: clear the override so the glue and the next login do not inherit it.
fn leave_world(mut current: ResMut<CurrentInterior>) {
    current.0 = None;
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<CurrentInterior>()
        .add_systems(
            Update,
            resolve_interior
                .run_if(super::world_audio_live)
                .in_set(WorldStage::Present)
                .after(benilla_world::wmo_portal::WmoPvsSet),
        )
        .add_systems(
            OnExit(crate::char_select::ClientState::InWorld),
            leave_world,
        );
}
