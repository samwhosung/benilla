//! WMO interior audio identity: the `WMOAreaTable` row for the group the camera
//! eye is inside, published as an override layer for the zone schedulers.
//!
//! The chain: `wmo_portal::CurrentWmoInterior` (the faithful down-ray already run for the portal
//! cull — the eye's group, `None` outdoors) → `WMOAreaTable` resolve (exact group row → whole-WMO
//! default → name-set-0 fallback, `benilla_formats::WmoAreaCatalog`) → [`CurrentInterior`]: the
//! interior's audio FKs. Zone music/ambience/intro ([`super::zone`]) and reverb
//! ([`super::reverb`]) treat nonzero fields as overriding the terrain `AreaTable` chain — this is
//! where 1.12's interior soundscape actually lives (the Northshire monk chant is the abbey's
//! whole-WMO `IntroSound` 221; ~4 000 group rows carry CAVE/AUDITORIUM reverb + inn ambience).
//! Zero fields fall through to the terrain chain (INTERIM: the client's exact inherit rule for
//! zero interior FKs is unrecorded; fall-through is the `AreaTable` parent-walk's spirit).

use bevy::prelude::*;

use benilla_world::schedule::WorldStage;

/// The audio FKs of the interior the eye is in (`None` = open world). All-zero fields mean "an
/// interior with no audio identity of its own" — consumers fall through to the terrain chain.
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
    /// `ZoneIntroMusicTable` FK — the entry fanfare.
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

/// `OnExit(InWorld)`: the eye may still be parked inside the old WMO, but the session is over —
/// clear the override so neither the glue layer nor the next login's first frames inherit it.
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
