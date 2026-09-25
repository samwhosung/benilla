//! UI sounds: the app side of the Lua `PlaySound` seam, and the per-item gesture sounds. All play
//! 2D on SFX after the UI input pass, so a click sounds the frame its handler acts.
//!
//! - [`drain_ui_sounds`] plays the [`SoundRequest`]s the script queues, as the client's
//!   `PlaySoundById`/`ByName` path.
//! - A cursor payload change is the pickup or put-down gesture, played engine-side by the
//!   reference from `SetCursorItem` (`0x494c4a`) and `ClearCursor` (`0x49520a`). An item resolves
//!   `ItemGroupSounds[ItemDisplayInfo[displayId].group_sounds].kit[gesture]` (`0x457ff0`); other
//!   payloads play the generic grab/drop pair; never both. A bag swap plays one sound, the held
//!   item's put-down: the place branch never calls `SetCursorItem` (`0x5e0c40`).
//! - Taking a loot row plays that item's pickup kit at the click, before the send (`0x4c2790`).

use bevy::prelude::*;

use benilla_formats::{ItemGesture, ItemGroupSoundsCatalog};
use benilla_ui::script::{CursorPayload, SoundRequest, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use super::kit::{self, KitRef, SoundKits};
use super::{SoundConfig, SoundOutput};

/// Drain the VM's queued `PlaySound` intents into the kit player; drained even with no catalog,
/// so the queue cannot grow unbounded.
fn drain_ui_sounds(
    script: Option<NonSendMut<UiScript>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    let Some(mut script) = script else {
        return;
    };
    let requests = script.take_sounds();
    if requests.is_empty() {
        return;
    }
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        debug!(
            "sound(ui): {} play(s) dropped — no kit catalog",
            requests.len()
        );
        return;
    };
    for req in requests {
        let kit_ref = match &req {
            SoundRequest::KitId(id) => KitRef::Id(*id),
            SoundRequest::KitName(name) => KitRef::Name(name),
            SoundRequest::File(path) => {
                // `PlaySoundFile`: by path, with no kit gates or variation.
                if let Err(e) = kit::play_file(
                    &mut kits,
                    &assets,
                    &mut out,
                    &config,
                    path,
                    kit::SoundCategory::Sfx,
                ) {
                    debug!("sound(ui): {req:?} — {e:#}");
                }
                continue;
            }
        };
        // 2D, on the SFX slider (the client's SoundVolume bucket).
        if let Err(e) = kit::play_kit(
            &mut kits,
            &assets,
            &mut out,
            &config,
            Vec3::ZERO,
            kit_ref,
            None,
            kit::SoundCategory::Sfx,
        ) {
            debug!("sound(ui): {req:?} — {e:#}");
        }
    }
}

/// The `ItemGroupSounds.dbc` catalog: the pickup, put-down and use kits per item sound group.
#[derive(Resource)]
struct ItemSounds(ItemGroupSoundsCatalog);

fn load_item_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_item_group_sounds(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            info!("sound: {} item sound groups", catalog.len());
            commands.insert_resource(ItemSounds(catalog));
        }
        Err(e) => warn!("sound: ItemGroupSounds failed to load — item drags silent: {e:#}"),
    }
}

/// `INTERFACESOUND_CURSORGRABOBJECT`/`DROPOBJECT`: the generic cursor gesture pair for a non-item
/// payload, by kit id (`0x495190`).
const INTERFACESOUND_CURSORGRABOBJECT: u32 = 902;
/// `LOOTWINDOWCOINSOUND`: kit 895, the coin a money pickup plays.
const LOOTWINDOWCOINSOUND: u32 = 895;
const INTERFACESOUND_CURSORDROPOBJECT: u32 = 903;

/// Which half of a gesture pair plays: `Gain` (`SetCursorItem`, kit index 0) or `Loss`
/// (`ClearCursor`, kit index 1).
#[derive(Clone, Copy)]
enum CursorGesture {
    Gain,
    Loss,
}

/// Play the cursor-payload gesture sound on every transition: an item lands through
/// `SetCursorItem` → `SndInterfacePlayItemSound(ecx=0)` and leaves through `ClearCursor` →
/// `(ecx=1)`. Another Some → Some change plays the outgoing loss then the incoming gain, as a
/// displaced action lands on the cursor (`PlaceAction`, `0x4e62e0`); the same item is no change.
/// A missing link (template, display, group 0, kit 0) is silent, as in the client.
fn play_item_gesture_sounds(
    script: Option<NonSend<UiScript>>,
    mut prev: Local<crate::ui_script::VmMemo<Option<CursorPayload>>>,
    items: Res<Items>,
    displays: Option<Res<ItemDisplays>>,
    sounds: Option<Res<ItemSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    net: Res<NetCommands>,
) {
    let Some(script) = script else { return };
    let prev = prev.get(&script);
    let now = script.cursor_payload();
    if *prev == now {
        return;
    }
    // Tracked even without catalogs, so a late-loading catalog never replays a stale gesture.
    let old = std::mem::replace(&mut *prev, now.clone());
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };

    let mut play = |payload: &CursorPayload, gesture: CursorGesture| match payload {
        // Mode 5 takes the item arm: the vendor grab `0x4950f0` is handed the row's
        // `ItemDisplayInfo` id, so it plays the item's own pickup and put-down.
        CursorPayload::Item(_) | CursorPayload::Merchant(_) => {
            let item_id = match payload {
                CursorPayload::Item(i) => i.item_id,
                CursorPayload::Merchant(m) => m.item_id,
                _ => unreachable!("guarded by the arm's own pattern"),
            };
            let (Some(displays), Some(sounds)) = (&displays, &sounds) else {
                return;
            };
            // The template is cached by now (the bag drew the icon); one still in flight is
            // silent, like the client's null-record return.
            let Some(display_id) = items
                .template(item_id, 0, &net)
                .map(|t| t.display_info_id)
            else {
                return;
            };
            let item_gesture = match gesture {
                CursorGesture::Gain => ItemGesture::Pickup,
                CursorGesture::Loss => ItemGesture::PutDown,
            };
            play_item_gesture(
                display_id,
                item_gesture,
                displays,
                sounds,
                &mut kits,
                &assets,
                &mut out,
                &config,
            );
        }
        // The generic pair: the macro grab (mode 8, `0x494f60`/`0x494f80`) reaches the same
        // pair as spells and actions, and the pet-action builder `0x494e20` names it outright.
        CursorPayload::Spell(_)
        | CursorPayload::Action(_)
        | CursorPayload::Macro(_)
        | CursorPayload::PetAction(_)
        // Mode 10: the stabled-pet grab `0x495010` calls the same generic path.
        | CursorPayload::StablePet(_)
        // Mode 2: money pickup and drop both play `LOOTWINDOWCOINSOUND`, never the generic drop
        // kit (`0x494cfe`/`0x49523a`).
        | CursorPayload::Money(_) => {
            let kit_id = match (&payload, gesture) {
                (CursorPayload::Money(_), _) => LOOTWINDOWCOINSOUND,
                (_, CursorGesture::Gain) => INTERFACESOUND_CURSORGRABOBJECT,
                (_, CursorGesture::Loss) => INTERFACESOUND_CURSORDROPOBJECT,
            };
            if let Err(e) = kit::play_kit(
                &mut kits,
                &assets,
                &mut out,
                &config,
                Vec3::ZERO,
                KitRef::Id(kit_id),
                None,
                kit::SoundCategory::Sfx,
            ) {
                debug!("sound(ui): generic cursor kit {kit_id} — {e:#}");
            }
        }
    };

    match (&old, &now) {
        (None, None) => {}
        (None, Some(n)) => play(n, CursorGesture::Gain),
        (Some(o), None) => play(o, CursorGesture::Loss),
        (Some(CursorPayload::Item(o)), Some(CursorPayload::Item(n))) if o.item_id == n.item_id => {}
        (Some(o), Some(n)) => {
            play(o, CursorGesture::Loss);
            play(n, CursorGesture::Gain);
        }
    }
}

/// Resolve an item's gesture kit through `ItemDisplayInfo` and `ItemGroupSounds` and play it 2D;
/// a missing link is silent, as in the client.
fn play_item_gesture(
    display_id: u32,
    gesture: ItemGesture,
    displays: &ItemDisplays,
    sounds: &ItemSounds,
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
) {
    let group = displays
        .catalog
        .get(display_id)
        .map_or(0, |d| d.group_sounds);
    let Some(kit) = sounds.0.kit(group, gesture) else {
        return;
    };
    if let Err(e) = kit::play_kit(
        kits,
        assets,
        out,
        config,
        Vec3::ZERO,
        KitRef::Id(kit),
        None,
        kit::SoundCategory::Sfx,
    ) {
        debug!("sound(ui): item {gesture:?} kit {kit} — {e:#}");
    }
}

/// A looted row's pickup sound, written by [`crate::ui_loot::drain_loot`]. The client plays it at
/// the loot-slot click (`0x4c2790`); `SMSG_ITEM_PUSH` itself is silent, so a purchase makes no
/// pickup sound.
#[derive(Message, Clone, Copy)]
pub(crate) struct LootPickupSound {
    pub(crate) display_id: u32,
}

/// Play the per-item pickup kit for each looted row; with a catalog missing the queue is dropped.
fn play_loot_pickup_sounds(
    mut reqs: MessageReader<LootPickupSound>,
    displays: Option<Res<ItemDisplays>>,
    sounds: Option<Res<ItemSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    if reqs.is_empty() {
        return;
    }
    let (Some(displays), Some(sounds), Some(mut kits), Some(assets)) =
        (displays, sounds, kits, assets)
    else {
        reqs.clear();
        return;
    };
    for req in reqs.read() {
        play_item_gesture(
            req.display_id,
            ItemGesture::Pickup,
            &displays,
            &sounds,
            &mut kits,
            &assets,
            &mut out,
            &config,
        );
    }
}

/// The auto-equip gesture pair, written by [`crate::ui_items`] when a right-click auto-equips a
/// bag item. The client's `UseContainerItem` (`0x4fa0e0`) runs a synthetic `SetCursorItem` then
/// `ClearCursor` (`0x494c4a`, `0x49520a`), so pickup then put-down play; benilla's right-click
/// never touches the cursor, so the pair is emitted here.
#[derive(Message, Clone, Copy)]
pub(crate) struct AutoEquipSound {
    pub(crate) display_id: u32,
}

/// Play the pickup-then-place pair for each [`AutoEquipSound`]; with a catalog missing the queue
/// is dropped.
fn play_auto_equip_sounds(
    mut reqs: MessageReader<AutoEquipSound>,
    displays: Option<Res<ItemDisplays>>,
    sounds: Option<Res<ItemSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
) {
    if reqs.is_empty() {
        return;
    }
    let (Some(displays), Some(sounds), Some(mut kits), Some(assets)) =
        (displays, sounds, kits, assets)
    else {
        reqs.clear();
        return;
    };
    for req in reqs.read() {
        for gesture in [ItemGesture::Pickup, ItemGesture::PutDown] {
            play_item_gesture(
                req.display_id,
                gesture,
                &displays,
                &sounds,
                &mut kits,
                &assets,
                &mut out,
                &config,
            );
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_message::<LootPickupSound>()
        .add_message::<AutoEquipSound>()
        .add_systems(Startup, load_item_sounds.after(AssetSet::Open))
        .add_systems(
            Update,
            (
                drain_ui_sounds,
                play_item_gesture_sounds,
                play_loot_pickup_sounds,
                play_auto_equip_sounds,
            )
                .after(crate::ui_script::UiInput),
        );
}
