//! The bag, cursor and paper-doll verbs' engine half, which the call host applies inside each call
//! ([`crate::script_calls::ScriptCallHost`]): each sends, defers behind its bind question or
//! answers that question before the verb returns, as the reference's bindings do in their own
//! bodies, so a chunk's item packets leave in call order with every other verb's.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ItemCall, UiScript, EQUIPMENT_BAG};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;

use super::drain::{send_auto_equip, send_container_move};
use super::{slot_guid, wire_pos};

/// What the item calls read and send through. The bind-on-use accept's re-issued use is
/// [`super::ScriptItemUse::confirm_bind_on_use`], which holds the cast ladder.
#[derive(SystemParam)]
pub(crate) struct ItemSteps<'w, 's> {
    net: Res<'w, NetCommands>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    equip_sound: MessageWriter<'w, crate::sound::AutoEquipSound>,
    /// Absent in a world with no merchant window, whose repair clicks are dropped.
    merchant: Option<Res<'w, crate::ui_merchant::MerchantOpen>>,
    purse: Option<crate::ui_merchant::RepairPurse<'w, 's>>,
}

impl ItemSteps<'_, '_> {
    /// Apply one item call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: ItemCall) {
        let Self {
            net,
            self_q,
            objects,
            items,
            gate,
            equip_sound,
            merchant,
            purse,
        } = self;
        let store = self_q.iter().next();
        match call {
            ItemCall::Move(mv) => send_container_move(net, mv),
            // `SwapItem`'s question (`0x5e0ee4`), its dialog up inside the call.
            ItemCall::SwapAsk(mv) => {
                gate.defer_equip(
                    script,
                    crate::ui_bind_confirm::PendingEquip::swap(&mv, None),
                );
            }
            ItemCall::SwapWait { mv, entry } => gate.park_swap(&mv, entry, items, net),
            ItemCall::AutoEquipWait { entry } => gate.await_auto_equip(entry, items, net),
            // The destination is a bag, not a slot (`0x4c7c00` into `0x5e12e0`):
            // `CMSG_AUTOSTORE_BAG_ITEM` names `(srcbag, srcslot, dstbag)` and the server picks the
            // slot. A split carry sends `CMSG_SPLIT_ITEM` instead (`0x5e1210`, the fork on
            // `[0xb4b40c]`), its destination slot the reference's `0xFF`. Neither takes a lock.
            ItemCall::AutoStore(a) => {
                let (Some((src_bag, src_slot)), Some((dst_bag, _))) =
                    (wire_pos(a.src_bag, a.src_slot), wire_pos(a.dst_bag, 1))
                else {
                    debug!("ui_items: bag autostore {a:?} out of range — ignored");
                    return;
                };
                let command = match a.count {
                    None => ClientCommand::AutoStoreBagItem {
                        src_bag,
                        src_slot,
                        dst_bag,
                    },
                    Some(n) => ClientCommand::SplitItem {
                        src_bag,
                        src_slot,
                        dst_bag,
                        dst_slot: 0xFF,
                        count: n.min(u32::from(u8::MAX)) as u8,
                    },
                };
                debug!("ui_items: autostore {a:?} as {command:?}");
                let _ = net.0.send(command);
            }
            // Paper first, then the target; eligibility is the server's, refused with an
            // `ERR_CANT_WRAP_*` reason.
            ItemCall::Wrap {
                gift_bag,
                gift_slot,
                item_bag,
                item_slot,
            } => {
                let (Some((gift_bag, gift_slot)), Some((item_bag, item_slot))) =
                    (wire_pos(gift_bag, gift_slot), wire_pos(item_bag, item_slot))
                else {
                    debug!("ui_items: wrap {call:?} out of range — ignored");
                    return;
                };
                let _ = net.0.send(ClientCommand::WrapItem {
                    gift_bag,
                    gift_slot,
                    item_bag,
                    item_slot,
                });
            }
            // A repair-cursor click repairs the one item: the purse test first (`0x4f9cc8`), then
            // `ITEM_REPAIR` as it goes out (`0x4f9ce4`). The doll's clicks (`0x4c7714`,
            // `0x4c79c4`) are the same arm on a worn item.
            ItemCall::Repair { .. } | ItemCall::RepairWorn(_) => {
                let (Some(open), Some(purse), Some(me)) = (merchant.as_deref(), purse, store)
                else {
                    return;
                };
                let Some(vendor) = open.vendor else {
                    return;
                };
                let item_guid = match call {
                    ItemCall::Repair { bag, slot } => u8::try_from(slot.saturating_sub(1))
                        .ok()
                        .and_then(|slot0| slot_guid(&me.0, bag, slot0, objects)),
                    ItemCall::RepairWorn(id) => (1..=19)
                        .contains(&id)
                        .then(|| u8::try_from(id - 1).ok())
                        .flatten()
                        .and_then(|slot0| slot_guid(&me.0, EQUIPMENT_BAG, slot0, objects)),
                    _ => None,
                };
                let Some(guid) = item_guid else {
                    debug!(
                        "ui_items: repair on an empty or unaddressable slot ({call:?}) — ignored"
                    );
                    return;
                };
                if purse.refuses(script, open, me, [guid]) {
                    return;
                }
                debug!("ui_items: repair item {guid:#x} ({call:?})");
                script.queue_sound_kit("ITEM_REPAIR");
                let _ = net.0.send(ClientCommand::RepairItem {
                    vendor,
                    item_guid: guid,
                });
            }
            // `AutoEquipCursorItem` past its question: the call cleared the cursor, keeping the
            // pickup's lock but an arrow's (`0x5e1577`, `0x5e1711`).
            ItemCall::AutoEquip { bag, slot } => {
                let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
                    debug!("ui_items: autoequip ({bag}, {slot}) out of range — ignored");
                    return;
                };
                let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
                let guid = store.and_then(|s| slot_guid(&s.0, bag, slot0, objects));
                send_auto_equip(script, objects, items, net, bag_index, wire_slot, guid);
            }
            // The question the call raised (`0x5e16c6`), filed though the slot moved since: the
            // item is on the cursor, and the answer re-judges the slot.
            ItemCall::EquipAsk { bag, slot, guid } => {
                gate.defer_equip(
                    script,
                    crate::ui_bind_confirm::PendingEquip::AutoEquip { bag, slot, guid },
                );
            }
            // `CMSG_DESTROYITEM`, count 0 the whole stack on both sides; the verb relocked the item
            // (`0x4898bc`).
            ItemCall::Destroy { bag, slot, count } => {
                let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
                    debug!("ui_items: destroy ({bag}, {slot}) out of range — ignored");
                    return;
                };
                let _ = net.0.send(ClientCommand::DestroyItem {
                    bag_index,
                    slot: wire_slot,
                    count: count.min(u32::from(u8::MAX)) as u8,
                });
            }
            ItemCall::EquipAnswer(answer) => {
                answer_pending_equip(
                    script,
                    gate,
                    equip_sound,
                    objects,
                    items,
                    net,
                    store,
                    answer,
                );
            }
            // The host hands this one to the item use, which holds the cast ladder.
            ItemCall::BindOnUse { .. } => {}
        }
    }
}

/// `EquipPendingItem`/`CancelPendingEquip` (`0x5e1be0`). An accept re-runs the original sender
/// with `suppress` set: 1.12 has no confirm opcode, and a slot that changed under the dialog is
/// judged afresh. A cancel sends nothing, unlocks the items the click locked and frees the record:
/// a swap's two ends as they sit now (`0x5e1ca5`, `0x5e1d04`), an auto-equip's one item, its
/// record naming no second container (`0x5e1cd1`). An unfiled index does nothing: `0x5e1be0`
/// bounds-checks it and returns.
#[allow(clippy::too_many_arguments)] // the gate, the sound and the senders' state, one call
fn answer_pending_equip(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    equip_sound: &mut MessageWriter<crate::sound::AutoEquipSound>,
    objects: &Objects,
    items: &Items,
    net: &NetCommands,
    store: Option<&ObjectStore>,
    answer: benilla_ui::script::PendingEquipAnswer,
) {
    use crate::ui_bind_confirm::PendingEquip;
    let Some(rec) = gate.equips.take(answer.index) else {
        debug!(
            "ui_items: bind answer for index {} — no such pending equip",
            answer.index
        );
        return;
    };
    if !answer.accept {
        debug!("ui_items: pending equip {} cancelled", answer.index);
        let at = |bag: i64, slot: u32| {
            let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
            store.and_then(|s| slot_guid(&s.0, bag, slot0, objects))
        };
        let locked = match rec {
            PendingEquip::Swap {
                src_bag,
                src_slot,
                dst_bag,
                dst_slot,
                ..
            } => [at(src_bag, src_slot), at(dst_bag, dst_slot)],
            PendingEquip::AutoEquip { guid, .. } => [Some(guid), None],
        };
        for guid in locked.into_iter().flatten() {
            script.unlock_item(guid);
        }
        return;
    }
    match rec {
        // The accept re-issues `SwapItem` confirmed (`0x5e1c46`): it sends, and its clear
        // (`0x5e0fe7`) drops whatever the cursor holds then, keeping the locks.
        PendingEquip::Swap {
            src_bag,
            src_slot,
            dst_bag,
            dst_slot,
            ..
        } => {
            let mv = benilla_ui::script::ContainerMove {
                src_bag,
                src_slot,
                dst_bag,
                dst_slot,
                count: None,
            };
            if let benilla_ui::script::Swap::Sent(mv) = script.swap_items(mv, true) {
                send_container_move(net, mv);
            }
        }
        // The accept re-issues the pick-up and auto-equip from the slot as it stands (`0x5e1c23`
        // into `0x5e1840`, `suppress` set): `SetCursorItem`'s clear drops and unlocks the held
        // item, then the equip goes out. A slot emptied under the dialog sends nothing
        // (`0x5e18da`).
        PendingEquip::AutoEquip { bag, slot, .. } => {
            let benilla_ui::script::AutoEquip::Sent { bag, slot, guid } =
                script.auto_equip_slot(bag, slot, true)
            else {
                return;
            };
            let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
                return;
            };
            send_auto_equip(
                script,
                objects,
                items,
                net,
                bag_index,
                wire_slot,
                Some(guid),
            );
            // The re-issue's `SetCursorItem` and send clear play their pair inside the answer
            // (`0x494c4a`, `0x49520a`), after the held item's own clear.
            if let Some(display_id) = objects
                .object(guid)
                .and_then(|o| o.object_entry())
                .and_then(|entry| items.template(entry, guid, Ask::Read))
                .map(|t| t.display_info_id)
            {
                equip_sound.write(crate::sound::AutoEquipSound { display_id });
            }
        }
    }
}

/// The bags', the cursor's and the paper doll's appliers, one nested set of the call host: the
/// item uses and the item verbs' sends.
pub(crate) type BagSteps<'w, 's> =
    ParamSet<'w, 's, (super::ScriptItemUse<'w, 's>, ItemSteps<'w, 's>)>;

/// Apply the item calls and the container uses a harness's VM kept
/// ([`UiScript::keep_calls`]), in call order, as the app's host applies each inside its verb;
/// an open goes through the loot window's applier.
#[cfg(test)]
pub(crate) fn apply_kept_calls(world: &mut World) {
    use benilla_ui::script::ScriptCall;
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(
            |mut script: NonSendMut<UiScript>,
             mut set: ParamSet<(
                super::ScriptItemUse,
                ItemSteps,
                crate::ui_loot::LootSteps,
            )>| {
                for call in script.take_script_calls() {
                    match call {
                        ScriptCall::UseContainerItem(call) => {
                            if let Some(open) = set.p0().use_container_item(&mut script, call) {
                                let _ = set.p2().open_item(&mut script, open);
                            }
                        }
                        ScriptCall::Item(ItemCall::BindOnUse { may_cast }) => {
                            set.p0().confirm_bind_on_use(&mut script, may_cast);
                        }
                        ScriptCall::Item(call) => set.p1().apply(&mut script, call),
                        _ => {}
                    }
                }
            },
        )
        .unwrap();
}

/// [`apply_kept_calls`] for a harness without the item use's cast state: the item calls alone.
#[cfg(test)]
pub(crate) fn apply_kept_item_calls(world: &mut World) {
    use benilla_ui::script::ScriptCall;
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(|mut script: NonSendMut<UiScript>, mut steps: ItemSteps| {
            for call in script.take_script_calls() {
                if let ScriptCall::Item(call) = call {
                    steps.apply(&mut script, call);
                }
            }
        })
        .unwrap();
}
