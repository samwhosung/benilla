//! Draw and stow sounds through `SheatheSoundLookups`.
//!
//! The trigger is [`SheathSwapMessage`], fired once per arm as that arm's sheath one-shot crosses
//! its swap point. Snap transitions (attack auto-draw, reactive stows, remote units) are silent,
//! as the reference's instant paths play no clip. It is not the `$SHL`/`$SHR` anim tags: HipSheath
//! (90, every one-hand draw) carries no sound events.
//!
//! `SheatheSoundLookups` maps `(class, subclass, material)` to a stow/draw kit pair; in 1.12.1
//! every row of a material carries the same pair (metal 698/700, wood 697/699), so the item's
//! `Material` decides. It rides the wire (`SMSG_ITEM_QUERY_SINGLE_RESPONSE` for players,
//! `UNIT_VIRTUAL_ITEM_INFO` for creatures) into `Wielded`. Shields resolve on their own class-4
//! rows through the lookup's fallback.

use bevy::prelude::*;

use benilla_formats::SheatheSoundCatalog;

use crate::creature_anim::{SheathSwapMessage, Wielded};
use crate::net::NetEntity;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

#[derive(Resource)]
struct SheatheSounds(SheatheSoundCatalog);

fn load_sheathe_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_sheathe_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} sheathe sound rows", cat.len());
            commands.insert_resource(SheatheSounds(cat));
        }
        Err(e) => warn!("sound: sheathe sounds failed to load: {e:#}"),
    }
}

fn sheathe_sounds(
    mut swaps: MessageReader<SheathSwapMessage>,
    units: Query<(&Transform, &Wielded), With<NetEntity>>,
    sounds: Option<Res<SheatheSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if swaps.is_empty() {
        return;
    }
    let (Some(sounds), Some(mut kits), Some(assets)) = (sounds, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for swap in swaps.read() {
        let Ok((transform, wielded)) = units.get(swap.entity) else {
            continue;
        };
        // One message per arm, naming the slot that moved: a sword-and-board draw is two sounds,
        // and a melee → ranged toggle's second movement rings the bow.
        let item = match swap.slot {
            0 => wielded.main,
            1 => wielded.off,
            _ => wielded.ranged,
        };
        let Some((class, subclass)) = item else {
            continue; // an empty slot is silent
        };
        let material = u32::from(wielded.materials[usize::from(swap.slot).min(2)]);
        let Some(pair) = sounds
            .0
            .get(u32::from(class), u32::from(subclass), material)
        else {
            continue;
        };
        let kit = if swap.drawing {
            pair.unsheathe
        } else {
            pair.sheathe
        };
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("sheathe (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_sheathe_sounds.after(AssetSet::Open))
        .add_systems(Update, sheathe_sounds.in_set(WorldStage::Present));
}
