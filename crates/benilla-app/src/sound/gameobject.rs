//! GameObject display-slot sounds: the ten `GameObjectDisplayInfo.Sound[0..9]` kits a door,
//! chest or goober plays as its animation runs.
//!
//! The reference's only reader of the columns, `0x5f4010`, is called only from the GameObject M2
//! animation-event dispatcher `0x5f3e20` (registered at create, `0x5f7d1f`, on the types
//! [`crate::go_anim::GoAnim`] animates). So a slot sounds only when the model authors the event
//! keyframe and its clip plays; there is no state-transition sound path.
//!
//! The kit's `SoundEntries` flag `0x200` (`0x5f4051 call 0x458830`) picks the lane: clear is a
//! positioned one-shot (`0x458870`), set registers into the 32-entry ambient emitter pool
//! (`0x461d80`), one handle per object shared with its `$DSL`. Only `0x5f40c0`, from the state
//! dispatch `0x5f3cb0` and teardown, drops that loop; this dispatcher has no `$DSE` arm.
//! Slot 5 (Opened) is empty in every 1.12.1 display row.

use bevy::prelude::*;

use benilla_formats::GameObjectSounds;
use benilla_protocol::EntityKind;

use crate::go_anim::GoStateDispatch;
use crate::net::NetEntity;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::emitter_pool::AmbientEmitterPool;
use super::kit::{kit_looping, play_kit, KitRef, SoundCategory, SoundKits};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The display → sound-slots table, holding only displays with a non-zero slot.
#[derive(Resource)]
pub(super) struct GoSounds(GameObjectSounds);

fn load_go_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_gameobject_sounds(&mut chain)
    };
    match loaded {
        Ok(s) => {
            info!("sound: {} GameObject displays with sound slots", s.len());
            commands.insert_resource(GoSounds(s));
        }
        Err(e) => warn!("sound: GameObject sound slots failed to load: {e:#}"),
    }
}

/// The display slot an M2 event tag addresses in `0x5f3e20`: `$GO0..5` → `Sound[0..5]`,
/// `$GC0..3` → the Custom slots `Sound[6..9]`.
fn go_event_slot(ident: &[u8; 4]) -> Option<usize> {
    match ident {
        [b'$', b'G', b'O', d @ b'0'..=b'5'] => Some((d - b'0') as usize),
        [b'$', b'G', b'C', d @ b'0'..=b'3'] => Some(6 + (d - b'0') as usize),
        _ => None,
    }
}

/// Play the display-slot kits a GameObject's animation events name, and drop its ambient loop when
/// the state machine dispatches.
///
/// Releases drain first so one always precedes the register it precedes in the reference: a
/// dispatch lands the frame a new clip is armed, and that clip's `$GOn` cannot fire before the
/// next frame (an arm frame fires nothing).
pub(super) fn go_display_sounds(
    mut dispatched: MessageReader<GoStateDispatch>,
    mut events: MessageReader<crate::creature_anim::AnimSoundEvent>,
    // The world pose: the event position is the model's placement in the reference.
    gos: Query<(&NetEntity, &GlobalTransform)>,
    go_sounds: Option<Res<GoSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    mut pool: ResMut<AmbientEmitterPool>,
    // Kit ids already warned about: a `$GOn` on a looping rest clip re-fires every band pass
    // (0.334 s on the centaur teleporter), so an unresolvable id would warn several times a second.
    mut complained: Local<std::collections::HashSet<u32>>,
) {
    // `0x5f3cb0` first releases the loop (`0x5f3cc8 call 0x5f40c0`), before it picks the new
    // substate's animation; drained even when nothing else resolves.
    for d in dispatched.read() {
        super::emitter_pool::release(&mut pool, d.0);
    }
    if events.is_empty() {
        return;
    }
    let (Some(go_sounds), Some(mut kits), Some(assets)) = (go_sounds, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    for ev in events.read() {
        let Some(slot) = go_event_slot(&ev.ident) else {
            continue;
        };
        // Only a GameObject has a display row in this table; a creature's `$GO*` tag is ignored.
        let Ok((net, transform)) = gos.get(ev.entity) else {
            continue;
        };
        if net.kind != EntityKind::GameObject {
            continue;
        }
        let kit = net
            .display_id
            .and_then(|d| go_sounds.0.slots(d))
            .map(|s| s[slot])
            .unwrap_or(0);
        // An unfilled column is silence: `0x458830` fails a null id and `0x5f4010` returns.
        if kit == 0 {
            continue;
        }
        // Where the key fired, not the object's origin: both lanes take `0x5f3e20`'s event world
        // position (`[ebp+0x10]`) verbatim, `0x458870(id, pos, -1, 1.0f)` and
        // `0x461d80(id, pos, 0)`. Many shipped events sit off their origin, up to 63.4 yd.
        let pos = ev.pos.unwrap_or_else(|| transform.translation());
        // A looping kit registers in the shared emitter pool, not a channel of its own, so a row
        // of braziers is one hum and a re-fired marker is a no-op, not a restart.
        if kit_looping(&kits, kit) {
            super::emitter_pool::register(&mut pool, ev.entity, kit, pos, listener);
            continue;
        }
        if let Err(e) = play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            KitRef::Id(kit),
            Some(pos),
            SoundCategory::Sfx,
        ) {
            if complained.insert(kit) {
                warn!(
                    "GO display sound (slot {slot}, kit {kit}): {e:#} (further reports for this \
                     kit suppressed)"
                );
            }
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_go_sounds.after(AssetSet::Open))
        // In `Present`, with the GO scanner that feeds it and the state machine that dispatches.
        .add_systems(Update, go_display_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::go_event_slot;

    /// The dispatcher's slot table (`0x5f3e20`).
    #[test]
    fn event_tags_map_to_display_slots() {
        assert_eq!(go_event_slot(b"$GO0"), Some(0));
        assert_eq!(go_event_slot(b"$GO5"), Some(5));
        assert_eq!(go_event_slot(b"$GC0"), Some(6)); // the bobber splash
        assert_eq!(go_event_slot(b"$GC3"), Some(9));
        assert_eq!(go_event_slot(b"$GO6"), None);
        assert_eq!(go_event_slot(b"$GC4"), None);
        assert_eq!(go_event_slot(b"$SND"), None);
        assert_eq!(go_event_slot(b"$FSD"), None);
    }
}
