//! The dismount sound — the ONE genuine mount-transition sound the client plays (decision 0441
//! fold-back): the dismount handler `0x607ce0`
//! unconditionally tail-plays a FIXED global SoundEntries kit, resolved once at startup by
//! name-match — `"SpiritWolf_DONOTRENAME"` (`0x623110` → `0x8627bc`) — positioned at the
//! dismounting unit. It is NOT CreatureSoundData: no mount/dismount column exists, and the
//! attach path `0x607a00` plays nothing at all — mount-UP is silent (any summon whoosh rides
//! the summoning spell's own visual kit, spell-node, untraced). Fires on the live
//! mounted→unmounted transition of any visible unit; first sight of an unmounted unit records
//! silently, and a remount (id→id′) is not a dismount.

use bevy::prelude::*;

use crate::net::FieldChanged;
use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The client's fixed dismount kit, by SoundEntries name (the reference resolves it once at
/// startup by name-match — the kit predates mounts as a spirit-wolf sound, hence the odd name;
/// the mechanism is byte-verified, its in-game character is the director's to judge on a live
/// dismount). This literal is the 5875 DBC's exact `Name` column value (one row, extracted
/// through the patch chain this session), and the client's constant at `0x8627bc` reads the same;
/// `SpiritWolf_DONOTRENAME` is a transcription-level difference, the row is unambiguous.
const DISMOUNT_KIT: &str = "SpiritWolf (DONOTRENAME)";

/// Play the fixed dismount kit on a live mounted→unmounted transition of any streamed unit —
/// the `UNIT_FIELD_MOUNTDISPLAYID` edge with a zero NEW value (the field-edge
/// stream is create-suppressed, so streaming in unmounted is not a dismount by construction).
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
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) || e.new != 0 {
            continue;
        }
        let Ok(transform) = poses.get(e.entity) else {
            continue;
        };
        debug!("dismount kit on {:?} (was mount {})", e.entity, e.old);
        if let Err(err) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Name(DISMOUNT_KIT),
            Some(transform.translation),
            SoundCategory::Sfx,
        ) {
            warn!("dismount kit ({DISMOUNT_KIT}): {err:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, dismount_sounds.in_set(WorldStage::Present));
}
