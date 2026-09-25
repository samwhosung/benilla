//! UI sounds — the app side of the Lua `PlaySound` seam (the one deliberate
//! UI-crate overlap cut when the sound crate was split), plus the **per-item item-gesture sounds**:
//! the bag-drag pickup/put-down, and the loot-window pickup.
//!
//! Three triggers, all 2D SFX plays, all after the UI input pass so a click's sound plays the
//! same frame its handler acted:
//! - The engine-free binding queues plain [`SoundRequest`]s ([`benilla_ui::script`]'s
//!   outbound-intent seam); [`drain_ui_sounds`] drains them each frame into the kit player (UI
//!   sounds have no world position — the client's `PlaySoundById`/`ByName` path).
//! - The cursor **payload** transition IS the pickup/put-down gesture (the real client plays these
//!   from `SetCursorItem 0x494c4a`/`ClearCursor 0x49520a`, engine-side — never FrameXML):
//!   [`play_item_gesture_sounds`] watches [`UiScript::cursor_payload`] (decision 0216's typed
//!   `CursorPayload`, any arm) each frame. An **Item** arm resolves its kit through the
//!   chain `ItemGroupSounds[ItemDisplayInfo[displayId].group_sounds].kit[gesture]`
//!   ([`play_item_gesture`]; `0x457ff0`). A
//!   **Spell**/**Action** arm (no producer yet — the spellbook/action-bar slices) plays the
//!   generic `INTERFACESOUND_CURSORGRABOBJECT`/`DROPOBJECT` pair (kits 902/903) instead — 0091's
//!   crux: the two are mutually exclusive per transition, never both. A bag swap plays ONE sound
//!   (the held item's put-down): the place branch never calls `SetCursorItem` — the Item→Item hop
//!   0216 §2 shipped is byte-refuted (`0x5e0c40`), so that
//!   transition no longer occurs. The Some→Some loss-then-gain pair below stays live for the
//!   ACTION hop (the bar is client-authoritative and its displaced action DOES land on the
//!   cursor, verified at `PlaceAction 0x4e62e0`) when the action-bar slice arrives.
//! - Taking a loot-window row plays that item's **pickup** kit (gesture 0) — the same per-item
//!   resolution, fired at the loot-slot click before the CMSG send (the real client's loot-list
//!   pickup site, `0x4c2790`). [`crate::ui_loot`] emits
//!   a [`LootPickupSound`] with the row's display id; [`play_loot_pickup_sounds`] plays it. (The
//!   loot *money* coin and the buy/sell coin are the coinage-change watcher instead — [`super::money`]
//!   — because acquiring an item on loot is the only one of the four that plays a per-item sound.)

use bevy::prelude::*;

use benilla_formats::{ItemGesture, ItemGroupSoundsCatalog};
use benilla_ui::script::{CursorPayload, SoundRequest, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::NetCommands;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use super::kit::{self, KitRef, SoundKits};
use super::{SoundConfig, SoundOutput};

/// Drain the VM's queued `PlaySound` intents into the kit player. The queue is drained even when
/// the catalog/assets are absent (headless, missing client data) so it can never grow unbounded —
/// those plays are dropped with a debug line, the same graceful-absence posture as every consumer.
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
                // `PlaySoundFile`: by path, no kit — no gates, no variation (module docs).
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
        // 2D: no position, so the listener is irrelevant (no gate, no rolloff). Interface
        // sounds ride the SFX slider (the client's SoundVolume bucket).
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

/// The `ItemGroupSounds.dbc` catalog — the pickup/put-down/use kit per item sound group. Optional
/// resource (absent ⇒ item drags are silent), the usual graceful-absence posture.
#[derive(Resource)]
struct ItemSounds(ItemGroupSoundsCatalog);

/// Startup: load `ItemGroupSounds.dbc` off the chain (the same shape as `load_sound_kits`).
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

/// `INTERFACESOUND_CURSORGRABOBJECT`/`DROPOBJECT` — the generic non-item cursor-payload gesture
/// pair (sound-kit ids, not `SOUNDKIT.xml` names; `0x495190`, 0091's crux). Plays for a
/// Spell/Action payload transition — an Item transition always plays its own
/// per-item kit instead, never this pair.
const INTERFACESOUND_CURSORGRABOBJECT: u32 = 902;
/// `LOOTWINDOWCOINSOUND` — SoundEntries kit 895, the coin clink the money pickup names (1962).
const LOOTWINDOWCOINSOUND: u32 = 895;
const INTERFACESOUND_CURSORDROPOBJECT: u32 = 903;

/// Which half of a gesture pair a transition plays: the payload landing on the cursor (`Gain`,
/// the client's `SetCursorItem`/kit-index 0) or leaving it (`Loss`, `ClearCursor`/kit-index 1).
#[derive(Clone, Copy)]
enum CursorGesture {
    Gain,
    Loss,
}

/// Play the cursor-payload gesture sound on every transition: an Item arm plays
/// its per-item pickup/put-down kit (exactly the real client's call sites — an item landing on
/// the cursor is `SetCursorItem` → `SndInterfacePlayItemSound(ecx=0)`, clearing (placed, swapped,
/// cancelled onto its own slot, or ESC's `ClearCursor`) is `ClearCursor` → `(ecx=1)`); a Spell/
/// Action arm plays the generic [`INTERFACESOUND_CURSORGRABOBJECT`]/[`INTERFACESOUND_CURSORDROPOBJECT`]
/// pair instead. A same-`item_id` Item→Item transition (not currently producible) is a
/// bookkeeping-only change and plays nothing; any other Some→Some transition plays the outgoing
/// payload's loss sound THEN the incoming payload's gain sound — the `ClearCursor`+`SetCursorItem`
/// pair. Item→Item never occurs anymore (the swap clears); the pair path stays for the
/// byte-verified ACTION hop when the action-bar slice lands (module doc above). Every
/// missing link (template in flight, unknown display, group 0, kit 0, absent catalog) is the
/// client's own silent return, never an error. The previous payload is tracked here (a `Local`),
/// not in the VM — the engine-free model owns the state, the app owns the sound.
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
    // Track the transition even when the catalogs are absent, so a late-loading catalog doesn't
    // replay a stale gesture.
    let old = std::mem::replace(&mut *prev, now.clone());
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };

    let mut play = |payload: &CursorPayload, gesture: CursorGesture| match payload {
        // Mode 5 shares the ITEM arm, not the generic one: the vendor grab `0x4950f0` is handed
        // the row's `ItemDisplayInfo` id precisely so the cursor gets that item's own pickup and
        // putdown sounds, the same pair a bag item gets.
        CursorPayload::Item(_) | CursorPayload::Merchant(_) => {
            let item_id = match payload {
                CursorPayload::Item(i) => i.item_id,
                CursorPayload::Merchant(m) => m.item_id,
                _ => unreachable!("guarded by the arm's own pattern"),
            };
            let (Some(displays), Some(sounds)) = (&displays, &sounds) else {
                return;
            };
            // By gesture time the template is cached (the bag drew the icon from it) — this is a
            // lookup, not an ask; a genuinely in-flight template resolves silent, like the
            // client's null-record return.
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
        // Everything that is not a live item shares the generic grab/drop kit — the reference's
        // own `0x494f60`/`0x494f80` grab path for a macro (mode 8) reaches the same
        // `INTERFACESOUND_CURSOR*` pair the spell and bar-action modes do, and the pet-action
        // builder `0x494e20` names `INTERFACESOUND_CURSORGRABOBJECT` outright.
        CursorPayload::Spell(_)
        | CursorPayload::Action(_)
        | CursorPayload::Macro(_)
        | CursorPayload::PetAction(_)
        // Mode 10 joins them: the stabled-pet grab `0x495010` calls the same generic path and
        // names no per-item kit.
        | CursorPayload::StablePet(_)
        // Mode 2 (1962, 1965): the money pickup AND drop both play `LOOTWINDOWCOINSOUND` — the
        // same kit 895 the purse plays on a change (`sound/money.rs`) — and never the generic drop
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

/// The per-item gesture play, shared by the cursor-drag and loot-pickup triggers: resolve the kit
/// through the byte-verified chain `ItemGroupSounds[ItemDisplayInfo[displayId].group_sounds]
/// .kit[gesture]` and play it 2D on the SFX bucket. Every missing link (unknown display, group 0,
/// kit 0) is the client's own silent return, never an error.
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

/// Request to play an item's **loot pickup** sound — written by [`crate::ui_loot::drain_loot`] when
/// the player takes a loot-window row (carrying that row's display id). The real client plays the
/// per-item `ItemGroupSounds` **pickup** kit (gesture 0) client-side at the loot-slot click, before
/// the CMSG send (`0x4c2790`): looting an item plays its
/// pickup sound, while the `SMSG_ITEM_PUSH` acquire itself is silent — so buying, which also pushes
/// an item, plays no pickup sound (only the coin, via [`super::money`]).
#[derive(Message, Clone, Copy)]
pub(crate) struct LootPickupSound {
    pub(crate) display_id: u32,
}

/// Play the per-item pickup kit for each looted row ([`LootPickupSound`]). Graceful absence: if any
/// catalog is missing the queue is drained and dropped, the same posture as every consumer.
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

/// Request to play the **auto-equip** gesture pair for an item — written by [`crate::ui_items`]
/// when a right-click auto-equips a bag item (the `CMSG_AUTOEQUIP_ITEM` fork of `UseContainerItem`).
/// The real client implements the right-click shortcut as a *synthetic* `SetCursorItem` →
/// `ClearCursor` (`Script::UseContainerItem 0x4fa0e0` → the pickup play at `0x494c4a` then the
/// place play at
/// `0x49520a`), so it plays the item's `ItemGroupSounds` **pickup** kit[0] THEN its **place**
/// kit[1] — the same two sounds a drag-equip makes. A drag *already* plays them via the
/// cursor-payload transitions ([`play_item_gesture_sounds`]); the right-click path never touches
/// the cursor in benilla, so this pair is emitted explicitly to match.
#[derive(Message, Clone, Copy)]
pub(crate) struct AutoEquipSound {
    pub(crate) display_id: u32,
}

/// Play the pickup-then-place pair for each [`AutoEquipSound`]. Graceful absence: if any catalog is
/// missing the queue is drained and dropped, the same posture as every consumer.
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
        // Pickup (grab onto cursor) THEN place (drop into the slot) — the order the real client's
        // synthetic SetCursorItem→ClearCursor runs them.
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
