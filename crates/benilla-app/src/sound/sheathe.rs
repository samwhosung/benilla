//! Draw/stow sounds — the sheath swap moment routed through `SheatheSoundLookups`.
//!
//! The trigger is [`SheathSwapMessage`] — the *ceremony's* hand-touches-weapon moment, fired once
//! **per arm** as that arm's one-shot crosses its authored swap point. Ceremony-only: snap
//! transitions (attack auto-draw, every reactive stow, remote units) are **silent**, like the
//! client's — the sound lives in the ceremony playback, and `bInstant` paths play no clip
//! (director-verified on the ref). NOT the `$SHL`/`$SHR` anim tags: those exist only on Sheath
//! (89, the back-stow ceremony) — **HipSheath (90, every 1H draw) carries no sound events at
//! all** (probe-verified on HumanMale; director-caught silence on a sword-and-board draw), so
//! the event track cannot be the client's trigger. The moving slot resolves its item through
//! `SheatheSoundLookups` (`(class, subclass, material)` → stow/draw kit pair — metal/wood
//! weapons, shields); an empty slot is silent.
//!
//! The pick's only real input is the item's **`Material`**: the 5875 table is one
//! row per weapon subclass per material, and every row of a material carries the same kit pair —
//! metal 698/700, wood 697/699 — so the subclass is inert and the material decides everything. It
//! rides the wire both ways (`SMSG_ITEM_QUERY_SINGLE_RESPONSE` for players, the
//! `UNIT_VIRTUAL_ITEM_INFO` byte triple for creatures) and reaches here through `Wielded`, so
//! nothing is guessed. Shields land on their own class-4 rows, whose material is a don't-care the
//! lookup's fallback resolves.

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
        // One message per arm, naming the slot whose model just moved — so a sword-and-board draw
        // is a ring plus a shield thunk (two messages, one clip each), and the second movement of
        // a melee → ranged toggle rings the bow rather than the swords it just put away.
        let item = match swap.slot {
            0 => wielded.main,
            1 => wielded.off,
            _ => wielded.ranged,
        };
        let Some((class, subclass)) = item else {
            continue; // empty slot — nothing to ring
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
