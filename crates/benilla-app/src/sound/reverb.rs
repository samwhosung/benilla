//! Zone reverb (the last 0070 slice-4 piece): the area's EAX preset on the
//! mixer's reverb send.
//!
//! The data chain: [`CurrentArea`] → `AreaTable` cols 5/6 (`SoundProviderPref` /
//! `...Underwater`, parent-inherited — the client's caller `0x67e7f0` walks the same fallback)
//! → `SoundProviderPreferences.dbc` (the EAX listener properties, consumed RAW — no clamps on
//! this path, `0x45a790`) → [`Mixer::set_reverb`]'s Freeverb projection, applied **instantly**
//! on change (`0x45a720`). Submerged, the underwater
//! column wins — pref 11 on 568 areas; dry land uses the dry column (8 dungeon floors in 1.12).
//!
//! Which sounds the wet signal reaches is **not** this module's business and is not "every 3D
//! sound": 3D-open (channel flag bit 27, `0x7a5bf0`) is necessary but not sufficient — the kit's
//! `SoundEntries.EAXDef` decides, and `0` means a NULL `SoundSamplePreferences` slot and a
//! permanently dry channel (the gate lives at [`Mixer::play_3d`]). 2D/UI/music/
//! ambience are structurally dry on top of that. `WMOAreaTable` carries the big interior payload
//! (~4 000 group rows, CAVE/AUDITORIUM/ARENA — 3 687 of them CAVE, which is what the Thunderbrew
//! Distillery's interior groups say).
//!
//! **The whole chain is gated on the `SoundReverb` CVar, and benilla defaults it OFF** —
//! [`SoundConfig::reverb`] carries the why and the confidence: the
//! reference emits the calls but its EAX API has had no hardware to render on since Vista.
//! Everything below stays built and correct — this module resolves the same preset the binary
//! would — but nothing reaches the mixer until the CVar says so.

use bevy::prelude::*;

use benilla_formats::SoundProviderCatalog;

use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::zone::AreaSounds;
use super::{SoundConfig, SoundOutput};

/// The reverb-preset catalog. Absent when the client data didn't load.
#[derive(Resource)]
pub(crate) struct SoundProviders(pub(crate) SoundProviderCatalog);

/// Startup: load the preset catalog.
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

/// Track the applied preset so the send is only retuned on change (0 = none; `None` = unknown —
/// the next [`zone_reverb`] run re-applies whatever it resolves, even the same preset).
#[derive(Resource, Default)]
struct AppliedPreset(Option<u32>);

/// React to area/interior/underwater changes: resolve the preset id and hand the row to the
/// mixer. The WMO interior's preset overrides the terrain area's (this is where
/// ~4 000 CAVE/AUDITORIUM rows live); zero falls through to the area chain.
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
    // `SoundReverb` off ⇒ no preset reaches the backend at all — the client's `0x45a75b` gate,
    // which returns before the marshal rather than applying a silent one. Off is our default and
    // the reference's audible truth: its EAX path needs hardware no machine has
    // had since DirectSound lost hardware mixing in Vista. Flipping the CVar re-applies here,
    // like the client's callback (`0x4574d0`).
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

/// `OnExit(InWorld)`: the world's room dies with the world — dry the send and forget the latch
/// (`None` ⇒ the next login re-applies from its own area, even if it resolves the same preset).
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
