//! The dismount sound, the one mount-transition sound the client plays: the dismount handler
//! `0x607ce0` always plays a fixed kit, resolved once at startup as the first row named exactly
//! `"SpiritWolf (DONOTRENAME)"` (`0x623110` → `0x8627bc`, `[0xc4e0c8]`), at the dismounting unit. It is not
//! CreatureSoundData, and the attach path `0x607a00` plays nothing, so mounting is silent. A
//! remount (id → id′) is not a dismount.

use bevy::prelude::*;

use crate::net::FieldChanged;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// Play the dismount kit on any streamed unit's `UNIT_FIELD_MOUNTDISPLAYID` edge to zero. The edge
/// stream skips creates, so streaming in unmounted is not a dismount.
fn dismount_sounds(
    mut edges: MessageReader<FieldChanged>,
    poses: Query<&Transform>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let kit = kits.catalog().dismount_kit();
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) || e.new != 0 {
            continue;
        }
        let Ok(transform) = poses.get(e.entity) else {
            continue;
        };
        debug!("dismount kit on {:?} (was mount {})", e.entity, e.old);
        let Some(kit) = kit else {
            continue;
        };
        if let Err(err) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("dismount kit ({kit}): {err:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, dismount_sounds.in_set(WorldStage::Present));
}
