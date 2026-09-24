//! GameObject **display-slot** sounds — `GameObjectDisplayInfo.Sound[0..9]`, the ten kit columns a
//! door/chest/goober plays as its animation runs.
//!
//! **One function in the reference reads those columns, and one thing reaches it.** `0x5f4010` is
//! the binary's only reader of `Sound[slot]` (whole-image census of the store `0xc0dce4` and its
//! index `0xc0dcec`: six references, four gameplay readers, three of them the ModelName column),
//! and its only caller is the GameObject M2 **animation-event** dispatcher `0x5f3e20` — registered
//! per object at create (`0x5f7d1f` → vtable `+0x30`) on the **family-A** types only, which is
//! exactly benilla's [`crate::go_anim::GoAnim`] population. `$GO0..5` address slots 0..5
//! (Stand/Open/Loop/Close/Destroy/Opened), `$GC0..3` the four Custom slots 6..9. So a display slot
//! is audible only when the object's own model authors the matching event keyframe *and* the clip
//! carrying it is playing: **there is no state-transition sound path** (benilla decisions
//! 1090/1867).
//!
//! **The kit's `SoundEntries` flag `0x200` picks the lane** — `0x5f4051 call 0x458830`, whose only
//! other consumer image-wide is a spell-visual path (`0x60edf0`). Clear ⇒ a positioned
//! **one-shot** (`0x458870`). Set ⇒ the **ambient emitter
//! pool** (`0x461d80`), the same 32-entry table placed doodads' `$DSL` registers into, with the
//! handle cached in `[handler+0x18]` — one per object, shared with that object's own `$DSL` arm.
//! The loop is dropped again by `0x5f40c0`, called from the state-machine dispatch `0x5f3cb0`
//! ([`crate::go_anim::GoStateDispatch`]) and by the object's teardown; nothing else stops it, and
//! there is no `$DSE` arm on this dispatcher at all.
//!
//! **What the corpus actually contains** (`benilla-extract goslotscan`, the instrument this module
//! is checked against): 213 display rows carry a kit; per slot, *filled → reached by an authored
//! tag* is Stand 24→18, Open 99→80, Loop 28→14, Close 42→38, Destroy 26→14, **Opened 0→0**,
//! Custom0 77→56, Custom1 10→3, Custom2 5→1, Custom3 1→0. Thirty-seven live pairs name a looping
//! kit — every campfire, brazier, torch, fountain, hologram and Stratholme portal — which is the
//! whole reason the loop lane is not optional. Slot 5 Opened is empty data in 1.12.1: no display
//! row fills it, so it costs nothing and can never be heard.

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

/// The display→sound-slots table (only displays with any non-zero slot; ~a third of the 1638).
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

/// The GO display-slot an M2 animation-event tag addresses — the reference's GO event dispatcher
/// `0x5f3e20` (the 1086 fold-back): `$GO0..5`
/// → `Sound[0..5]`, `$GC0..3` → the Custom slots `Sound[6..9]`. Every other tag is not this
/// channel's (`$SND`/`$DSO`/`$DSL` carry a literal kit id and ride the generic
/// [`crate::sound::anim_events`] arms; `$SHK` is camera shake, no audio).
fn go_event_slot(ident: &[u8; 4]) -> Option<usize> {
    match ident {
        [b'$', b'G', b'O', d @ b'0'..=b'5'] => Some((d - b'0') as usize),
        [b'$', b'G', b'C', d @ b'0'..=b'3'] => Some(6 + (d - b'0') as usize),
        _ => None,
    }
}

/// Play the display-slot kits a GameObject's animation events name, and drop its ambient loop when
/// the state machine dispatches — the two halves of `0x5f4010`'s lifecycle, in one system so the
/// release can never land *after* the register that a state change is about to produce.
///
/// That ordering is structural, not scheduled: the release rides
/// [`crate::go_anim::GoStateDispatch`], written the frame the machine arms a new clip, while the
/// new clip's own `$GOn` cannot fire before the frame *after* its arm ([`crate::creature_anim`]'s
/// scan rule — an arm frame fires nothing). Releases are drained first here, so whichever order
/// the scheduler picks for this system against [`crate::go_anim`], a release always precedes the
/// register it precedes in the reference.
///
/// The load-bearing tenants: the fishing bobber's bite — Custom0's `$GC0` at t≈3.87 s → display
/// 668 `Sound6` = kit 3355 "Fishing Hooked", fired **once per 0xB3** (the completion retire
/// re-arms Stand before a second pass, decision 1100), beside the server's explicit
/// `SMSG_PLAY_OBJECT_SOUND(3355)` ~200 ms earlier — and every lit prop in the world, whose
/// `CampFireSmallLoop`/`TorchLoop`/`ElvenFountainSmallA` take the pool lane and hum until the
/// object's state changes under them.
pub(super) fn go_display_sounds(
    mut dispatched: MessageReader<GoStateDispatch>,
    mut events: MessageReader<crate::creature_anim::AnimSoundEvent>,
    // `GlobalTransform`: a GameObject is a root entity today, but the event's position is the
    // model's placement in the reference either way, and reading the world pose cannot be wrong.
    gos: Query<(&NetEntity, &GlobalTransform)>,
    go_sounds: Option<Res<GoSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    mut pool: ResMut<AmbientEmitterPool>,
    // Kit ids already complained about. A `$GOn` on a looping rest clip re-fires every band pass
    // — the centaur teleporter's Closed band is 0.334 s — so an unresolvable id would warn three
    // times a second for the object's whole life. The same flood [`super::anim_events`] learned
    // to avoid at 420 lines in one run past Darnassus.
    mut complained: Local<std::collections::HashSet<u32>>,
) {
    // `0x5f3cb0`'s first act is `0x5f3cc8 call 0x5f40c0` — release the object's display-sound loop
    // — and it runs before the machine picks the new substate's animation. Draining it here even
    // when nothing else is resolvable keeps the two halves from drifting apart.
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
        // A creature clip authoring a `$GO*`/`$GC*` tag doesn't resolve here: the query wants a
        // GameObject's display row, and only GO entities carry one in the display-slot table.
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
        // `0x458830` fails a null id and `0x5f4010` returns — an unfilled column is silence, not a
        // fallback. 97 shipped models author a `$GO0` against a zero Stand column (the bobber
        // among them); that is the reference being quiet, not a miss.
        if kit == 0 {
            continue;
        }
        // **Where the key fired, not where the object stands** (decision 1904): `0x5f3e20`'s
        // `[ebp+0x10]` is the kernel's `eventWorldPos` and both lanes below take it verbatim —
        // `0x458870(id, pos, -1, 1.0f)` for the one-shot, `0x461d80(id, pos, 0)` for the pool. It
        // is the difference between a portal's hum coming from the portal and from the model's
        // pivot: 82 of the 135 shipped `$GC0` records sit off their origin, out to 63.4 yd on
        // `orc_waterwheel.m2`, and 83 of 177 `$GO0`s do.
        let pos = ev.pos.unwrap_or_else(|| transform.translation());
        // The lane select. A looping kit is NOT a looping channel here: it is a *registration* in
        // the shared emitter pool, which is what makes one hum follow you down a row of braziers
        // instead of thirty channels stacking — and what makes a re-crossing of the marker (the
        // per-pass event re-fire of an armed looping clip) a no-op instead of a restart.
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

    /// The dispatcher's slot table (`0x5f3e20`): `$GO0..5` are the
    /// first six display slots, `$GC0..3` the four Custom slots 6..9 — the bobber's splash is
    /// `$GC0` → slot 6. Out-of-range digits and other families are not this channel.
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
