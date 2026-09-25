//! Zone reverb: the area's EAX preset on the mixer's reverb send.
//!
//! [`CurrentArea`] → `AreaTable` cols 5/6 (dry and underwater provider, parent-inherited as the
//! reference's caller `0x67e7f0` walks it) → `SoundProviderPreferences.dbc`, whose EAX listener
//! properties are used raw (`0x45a790`) → `Mixer::set_reverb`, applied instantly on change
//! (`0x45a720`). Submerged, the underwater column wins.
//!
//! Only 3D-open channels (flag bit 27, `0x7a5bf0`) whose kit has a nonzero
//! `SoundEntries.EAXDef` reach the send (the gate is `Mixer::play_3d`); 2D, UI, music and
//! ambience stay dry. The chain is gated on the `SoundReverb` CVar, off by default
//! ([`SoundConfig::reverb`] says why).

use bevy::prelude::*;

use benilla_formats::SoundProviderCatalog;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::zone::AreaSounds;
use super::{SoundConfig, SoundOutput};

/// The reverb-preset catalog.
#[derive(Resource)]
pub(crate) struct SoundProviders(pub(crate) SoundProviderCatalog);

fn load_providers(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_sound_provider_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} reverb presets", cat.len());
            commands.insert_resource(SoundProviders(cat));
        }
        Err(e) => warn!("sound: reverb presets failed to load: {e:#}"),
    }
}

/// The applied preset, so the send is retuned only on change: 0 is none, `None` re-applies
/// whatever resolves next.
#[derive(Resource, Default)]
struct AppliedPreset(Option<u32>);

/// Resolve the preset for the area, interior and submersion, and hand the row to the mixer. A
/// nonzero WMO interior preset overrides the terrain area's.
fn zone_reverb(
    mut applied: ResMut<AppliedPreset>,
    mut out: NonSendMut<SoundOutput>,
    world: benilla_world::world_point::WorldPoint,
    areas: Option<Res<AreaSounds>>,
    providers: Option<Res<SoundProviders>>,
    config: Res<SoundConfig>,
    interior: Res<super::interior::CurrentInterior>,
) {
    let (Some(areas), Some(providers)) = (areas, providers) else {
        return;
    };
    let column = usize::from(world.submersion().is_water());
    // With `SoundReverb` off no preset reaches the backend (the `0x45a75b` gate returns before
    // the marshal). Flipping the CVar re-applies here, like the callback `0x4574d0`.
    let pref = if config.enabled && config.reverb {
        interior
            .0
            .map(|i| i.sound_provider[column])
            .filter(|p| *p != 0)
            .or_else(|| {
                world
                    .area()
                    .and_then(|id| areas.0.resolve(id))
                    .map(|a| a.sound_provider[column])
            })
            .unwrap_or(0)
    } else {
        0
    };
    if applied.0 == Some(pref) {
        return;
    }
    let Some(mixer) = out.mixer.as_mut() else {
        return;
    };
    let preset = (pref != 0).then(|| providers.0.get(pref)).flatten();
    if pref != 0 && preset.is_none() {
        warn!("reverb: unknown preset {pref}");
    }
    if let Some(p) = preset {
        info!("reverb: {} (decay {:.2}s)", p.name, p.decay_time);
    } else if applied.0.map(|p| p != 0).unwrap_or(false) {
        info!("reverb: off");
    }
    mixer.set_reverb(preset);
    applied.0 = Some(pref);
}

/// `OnExit(InWorld)`: dry the send and forget the latch, so the next login re-applies.
fn leave_world(mut applied: ResMut<AppliedPreset>, mut out: NonSendMut<SoundOutput>) {
    if applied.0.take().is_some_and(|p| p != 0) {
        info!("reverb: off (left world)");
        if let Some(mixer) = out.mixer.as_mut() {
            mixer.set_reverb(None);
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AppliedPreset>()
        .add_systems(Startup, load_providers.after(AssetSet::Open))
        .add_systems(
            Update,
            zone_reverb
                .run_if(super::world_audio_live)
                .in_set(WorldStage::Present),
        )
        .add_systems(
            OnExit(crate::char_select::ClientState::InWorld),
            leave_world,
        );
}
