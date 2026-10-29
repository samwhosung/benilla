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
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

use super::kit::{self, KitRef, SoundKits};
use super::{SoundConfig, SoundOutput};
use crate::query_cache::Ask;

/// Drain the VM's queued `PlaySound` intents into the kit player; drained even with no catalog,
/// so the queue cannot grow unbounded. Left at the frame: no script reads what a play does.
fn drain_ui_sounds(
    // With the volumes, a CVar knob the call host's callbacks write; the drain runs no Lua.
    mut vm: crate::script_calls::VmWith<Res<'static, SoundConfig>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
) {
    vm.turn(|script, config| {
        let Some(script) = script else {
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
    })
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

    let mut play = |payload: &CursorPayload, gesture: CursorGesture| {
        let (item_id, kit_id) = gesture_plan(payload, gesture);
        // The template is cached by now (the bag drew the icon); one still in flight is silent,
        // like the client's null-record return.
        let display_id = item_id
            .and_then(|id| items.template(id, 0, Ask::Read))
            .map(|t| t.display_info_id);
        if let (Some(display_id), Some(displays), Some(sounds)) = (display_id, &displays, &sounds) {
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
        if let Some(kit_id) = kit_id {
            play_generic_kit(&mut kits, &assets, &mut out, &config, kit_id);
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

/// What a cursor gesture plays, in order: the item display's own pickup or put-down for the item
/// entry, then a generic kit.
/// - An item plays its display's sound alone (`SetCursorItem`, `ClearCursor`'s item arm).
/// - A preview (modes 5, 7, 9) grabs with its display's pickup alone (`0x495154`), and clears with
///   its put-down, then `INTERFACESOUND_CURSORDROPOBJECT` (`0x49526a`, `0x495291`).
/// - A spell, macro (`0x494f80`), pet action (`0x494e20`) or stabled pet (`0x495010`) plays the
///   generic grab or drop.
/// - Coins play `LOOTWINDOWCOINSOUND` both ways, never the generic drop (`0x494cfe`, `0x49523a`).
fn gesture_plan(payload: &CursorPayload, gesture: CursorGesture) -> (Option<u32>, Option<u32>) {
    let loss = matches!(gesture, CursorGesture::Loss);
    let preview_drop = loss.then_some(INTERFACESOUND_CURSORDROPOBJECT);
    match payload {
        CursorPayload::Item(i) => (Some(i.item_id), None),
        CursorPayload::Merchant(m) => (Some(m.item_id), preview_drop),
        // Mode 7 lifts only an item action, its id the item entry.
        CursorPayload::Action(a) => (Some(a.action), preview_drop),
        CursorPayload::Ammo(a) => (Some(a.item_id), preview_drop),
        CursorPayload::Money(_) => (None, Some(LOOTWINDOWCOINSOUND)),
        CursorPayload::Spell(_)
        | CursorPayload::Macro(_)
        | CursorPayload::PetAction(_)
        | CursorPayload::StablePet(_) => (
            None,
            Some(if loss {
                INTERFACESOUND_CURSORDROPOBJECT
            } else {
                INTERFACESOUND_CURSORGRABOBJECT
            }),
        ),
    }
}

/// Play a cursor kit by id, 2D.
fn play_generic_kit(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    kit_id: u32,
) {
    if let Err(e) = kit::play_kit(
        kits,
        assets,
        out,
        config,
        Vec3::ZERO,
        KitRef::Id(kit_id),
        None,
        kit::SoundCategory::Sfx,
    ) {
        debug!("sound(ui): generic cursor kit {kit_id} — {e:#}");
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
                .in_set(crate::ui_script::UiDrain),
        );
}

#[cfg(test)]
mod tests {
    use super::{
        gesture_plan, CursorGesture, INTERFACESOUND_CURSORDROPOBJECT,
        INTERFACESOUND_CURSORGRABOBJECT,
    };
    use benilla_ui::script::{ActionSlot, CursorAmmo, CursorPayload, CursorSpell, UiScript};

    /// A preview grabs with its display's pickup alone (`0x495154`) and clears with its put-down,
    /// then the generic drop (`0x49526a`, `0x495291`); a spell takes the generic pair.
    #[test]
    fn previews_play_their_display_then_the_generic_drop() {
        // A bar item lifted as mode 7.
        let mut s = UiScript::new().unwrap();
        s.set_action(
            1,
            Some(ActionSlot {
                texture: None,
                kind: 0x80,
                action: 159,
                count: 1,
                consumable: false,
                use_spell: None,
            }),
        );
        s.run("PickupAction(1)").unwrap();
        let action = s.cursor_payload().expect("mode 7 held");
        assert!(matches!(action, CursorPayload::Action(_)));
        let ammo = CursorPayload::Ammo(CursorAmmo {
            item_id: 2512,
            texture: None,
        });
        assert_eq!(
            gesture_plan(&action, CursorGesture::Gain),
            (Some(159), None)
        );
        assert_eq!(
            gesture_plan(&action, CursorGesture::Loss),
            (Some(159), Some(INTERFACESOUND_CURSORDROPOBJECT))
        );
        assert_eq!(
            gesture_plan(&ammo, CursorGesture::Loss),
            (Some(2512), Some(INTERFACESOUND_CURSORDROPOBJECT))
        );
        let spell = CursorPayload::Spell(CursorSpell {
            spell_id: 133,
            texture: None,
            passive: false,
        });
        assert_eq!(
            gesture_plan(&spell, CursorGesture::Gain),
            (None, Some(INTERFACESOUND_CURSORGRABOBJECT))
        );
    }
}
