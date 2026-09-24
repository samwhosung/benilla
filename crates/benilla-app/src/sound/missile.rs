//! Projectile **flight sound** (`SpellVisual` field 10): the loop a missile carries while it
//! travels — the thrown weapon's `WeaponLoop`, the fireball's `FireMissileLoop`. The client keeps
//! a per-missile loop handle (`CMissile+0x44`) started at launch and killed at
//! arrival; ours is a channel **tracked to the missile entity** (the tracked-loop follow in
//! [`super::kit::pump_channels`] rides it along the flight) begun on [`MissileSound::Start`] and
//! reaped on [`MissileSound::Stop`], which `crate::entities::missile` writes at launch/arrival.
//!
//! `force_loop`: the missile loop is looping by construction in the client (the loop handle), not
//! by the `SoundEntries` 0x200 flag — so we loop unconditionally, the same authority split the
//! creature body-loop uses ([`super::kit::play_kit_ext`]).

use bevy::prelude::*;

use crate::entities::MissileSound;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit_ext, stop_source, KitRef, PlayExtras, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

fn route_missile_sounds(
    mut events: MessageReader<MissileSound>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for ev in events.read() {
        match *ev {
            MissileSound::Start {
                entity,
                kit_sound,
                pos,
            } => {
                if let Err(e) = play_kit_ext(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    listener,
                    KitRef::Id(kit_sound),
                    Some(pos),
                    SoundCategory::Sfx,
                    PlayExtras {
                        // tag the loop to the missile — the pump follows it in flight, and the
                        // loop is looping by construction rather than by the kit's own flag
                        source: Some(entity),
                        force_loop: true,
                        ..default()
                    },
                ) {
                    warn!("missile flight sound {kit_sound}: {e:#}");
                }
            }
            // The missile carries only this one channel — stop everything tagged to it.
            MissileSound::Stop { entity } => {
                stop_source(&mut out, entity);
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, route_missile_sounds.in_set(WorldStage::Present));
}
