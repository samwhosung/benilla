//! Projectile flight sound: the `SpellVisual` field 10 loop a missile carries in flight. The
//! reference's first flight step (`0x61e2a0`) starts it (`0x61e79e call 0x4589a0`, handle at
//! `CMissile+0x44`), each later step moves it (`0x61e77c call 0x7a5b10`), and arrival kills it.
//! Here it is a channel tagged to the missile entity from [`MissileSound::Start`] to
//! [`MissileSound::Stop`]. Force-looped: the loop handle makes it loop, not the kit's `0x200`
//! flag.

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
                        // the pump follows the tagged loop in flight
                        source: Some(entity),
                        force_loop: true,
                        ..default()
                    },
                ) {
                    warn!("missile flight sound {kit_sound}: {e:#}");
                }
            }
            // The missile carries only this one channel.
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
