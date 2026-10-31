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
    /// The bags the bind answer's re-issue reads, the host being out while it applies.
    bags: Option<super::BagLive<'w, 's>>,
    /// The tables a record's copy is built through, for a lookup the VM's copy missed
    /// ([`super::feed::seat_record`]).
    records: super::feed::RecordViews<'w>,
}

impl ItemSteps<'_, '_> {
    /// Apply one item call.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: ItemCall) {
        if let ItemCall::EquipAnswer(answer) = call {
            return self.answer_unhosted(script, answer);
        }
        let Self {
            net,
            self_q,
            objects,
            items,
            gate,
            equip_sound,
            merchant,
            purse,
            bags,
            records,
        } = self;
        let store = self_q.iter().next();
        // The verbs' own reads of the bags, with the host out while it applies.
        let body = |script: &mut UiScript| match call {
            ItemCall::Move(mv) => send_container_move(net, mv),
            // `SwapItem`'s question (`0x5e0ee4`), its dialog up inside the call.
            ItemCall::SwapAsk(mv) => {
                gate.defer_equip(
                    script,
                    crate::ui_bind_confirm::PendingEquip::swap(&mv, None),
                );
            }
            // The question's lookup missed the VM's copy with no host lent to read the cache
            // through (an engine turn's call): a record the cache holds is acted on inside the
            // call (`0x5e163b`), its copy seated and the verb run again on it; only a record the
            // cache lacks parks the move or waits (`0x5e0dbe`, `0x5e1603`).
            ItemCall::SwapWait { mv, entry } => {
                if super::feed::seat_record(script, records, items, entry) {
                    if let Err(e) = script.replay_swap(mv) {
                        debug!("ui_items: swap on cached record {entry} raised: {e}");
                    }
                } else {
                    gate.park_swap(&mv, entry, items, net);
                }
            }
            ItemCall::AutoEquipWait { entry } => {
                if super::feed::seat_record(script, records, items, entry) {
                    if let Err(e) = script.replay_auto_equip(entry) {
                        debug!("ui_items: auto-equip on cached record {entry} raised: {e}");
                    }
                } else {
                    gate.await_auto_equip(entry, items, net);
                }
            }
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
            // `AutoEquipCursorItem` past its question, ahead of its clear, which keeps the
            // pickup's lock but an arrow's (`0x5e1577`, `0x5e1711`). When `0x5e1840` put the item
            // on the cursor in the same call, `SetCursorItem`'s pickup (`0x494c4a`) and the clear's
            // put-down (`0x49520a`) both land inside it, so the frame's cursor-sound diff sees
            // nothing and the pair plays here.
            ItemCall::AutoEquip {
                bag,
                slot,
                ammo,
                picked,
            } => {
                let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
                    debug!("ui_items: autoequip ({bag}, {slot}) out of range — ignored");
                    return;
                };
                let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
                let guid = store.and_then(|s| slot_guid(&s.0, bag, slot0, objects));
                send_auto_equip(net, bag_index, wire_slot, ammo);
                let display_id = guid.filter(|_| picked).and_then(|guid| {
                    let entry = objects.object(guid)?.object_entry()?;
                    Some(items.template(entry, guid, Ask::Read)?.display_info_id)
                });
                if let Some(display_id) = display_id {
                    equip_sound.write(crate::sound::AutoEquipSound { display_id });
                }
            }
            // By entry, the stack staying in its bag; the call cleared the cursor.
            ItemCall::SetAmmo { entry } => {
                debug!("ui_items: set ammo entry {entry}");
                let _ = net.0.send(ClientCommand::SetAmmo { entry });
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
            // The host runs the answer's steps itself, signalling between them; a host-less
            // harness runs them here ([`Self::answer_unhosted`]).
            ItemCall::EquipAnswer(_) => {}
            // The host hands this one to the item use, which holds the cast ladder.
            ItemCall::BindOnUse { .. } => {}
        };
        match bags.as_mut() {
            Some(bags) => script.reading(bags, body),
            None => body(script),
        }
    }
}

/// The end of a pending equip's record a cancel releases (`0x5e1be0`): the first container's slot
/// (`0x5e1c50`-`0x5e1ca5`), then the second's (`0x5e1caa`-`0x5e1d04`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EquipEnd {
    First,
    Second,
}

/// `EquipPendingItem`/`CancelPendingEquip` (`0x5e1be0`), step by step, for the host to signal
/// between: an index nobody filed does nothing (`0x5e1bee`); an accept re-runs the original sender
/// with `suppress` set ([`ItemSteps::replay_pending_equip`]), as 1.12 has no confirm opcode and a
/// slot that changed under the dialog is judged afresh; a cancel sends nothing and unlocks the
/// items the click locked, each end as its slot holds it when it is reached
/// ([`ItemSteps::release_pending_end`]); and the record is freed last (`0x5e1d09`), after every
/// listener of the answer's signals ran, so one that answers or asks again finds it filed.
impl ItemSteps<'_, '_> {
    /// The record filed at `index`, as the table holds it now.
    pub(crate) fn pending_equip(&self, index: u32) -> Option<crate::ui_bind_confirm::PendingEquip> {
        self.gate.equips.get(index)
    }

    /// The cancel's release of `end` of the record at `index`, read from the table as it stands
    /// (`0x5e1caa` reads it again past the first end's signal): the item the end's slot holds
    /// unlocked, which signals `ITEM_LOCK_CHANGED` (`0x495420`). An auto-equip's record names one
    /// item and no second container (`0x5e1cd1`), and a record a listener freed releases nothing.
    /// Whether it signalled.
    pub(crate) fn release_pending_end(
        &mut self,
        script: &mut UiScript,
        index: u32,
        end: EquipEnd,
    ) -> bool {
        use crate::ui_bind_confirm::PendingEquip;
        let store = self.self_q.iter().next();
        let at = |bag: i64, slot: u32| {
            let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
            store.and_then(|s| slot_guid(&s.0, bag, slot0, &self.objects))
        };
        let guid = match (self.gate.equips.get(index), end) {
            (
                Some(PendingEquip::Swap {
                    src_bag, src_slot, ..
                }),
                EquipEnd::First,
            ) => at(src_bag, src_slot),
            (
                Some(PendingEquip::Swap {
                    dst_bag, dst_slot, ..
                }),
                EquipEnd::Second,
            ) => at(dst_bag, dst_slot),
            (Some(PendingEquip::AutoEquip { guid, .. }), EquipEnd::First) => Some(guid),
            _ => None,
        };
        let Some(guid) = guid.filter(|&g| g != 0) else {
            return false;
        };
        script.unlock_item(guid);
        true
    }

    /// The accept's replay of `rec`, with the bags read as the host is out.
    pub(crate) fn replay_pending_equip(
        &mut self,
        script: &mut UiScript,
        rec: crate::ui_bind_confirm::PendingEquip,
    ) {
        let on_taxi = self
            .self_q
            .iter()
            .next()
            .is_some_and(|s| s.0.unit_flags() & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0);
        let body = |script: &mut UiScript| replay(script, rec, on_taxi);
        match self.bags.as_mut() {
            Some(bags) => script.reading(bags, body),
            None => body(script),
        }
    }

    /// The record at `index` freed (`0x5e1d09`-`0x5e1d29`), whatever it holds now.
    pub(crate) fn free_pending_equip(&mut self, index: u32) {
        self.gate.equips.take(index);
    }

    /// The answer's steps in order with nobody to signal between them, for a harness with no host.
    fn answer_unhosted(
        &mut self,
        script: &mut UiScript,
        answer: benilla_ui::script::PendingEquipAnswer,
    ) {
        let Some(rec) = self.pending_equip(answer.index) else {
            debug!(
                "ui_items: bind answer for index {} — no such pending equip",
                answer.index
            );
            return;
        };
        if answer.accept {
            self.replay_pending_equip(script, rec);
        } else {
            debug!("ui_items: pending equip {} cancelled", answer.index);
            for end in [EquipEnd::First, EquipEnd::Second] {
                self.release_pending_end(script, answer.index, end);
            }
        }
        self.free_pending_equip(answer.index);
    }
}

/// The accept's re-issue of `rec` (`0x5e1c05`-`0x5e1c46`), `on_taxi` our taxi flag.
fn replay(script: &mut UiScript, rec: crate::ui_bind_confirm::PendingEquip, on_taxi: bool) {
    use crate::ui_bind_confirm::PendingEquip;
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
            script.swap_items(
                benilla_ui::script::ContainerMove {
                    src_bag,
                    src_slot,
                    dst_bag,
                    dst_slot,
                    count: None,
                },
                true,
            );
        }
        // The accept re-issues the pick-up and auto-equip from the slot as it stands (`0x5e1c23`
        // into `0x5e1840`, `suppress` set): `SetCursorItem`'s clear drops and unlocks the held
        // item, then the equip goes out ahead of its clear. A slot emptied under the dialog sends
        // nothing (`0x5e18da`).
        PendingEquip::AutoEquip { bag, slot, .. } => {
            // The setter's control gate reads our taxi flag off the object (`0x494ba6`), here
            // with the host out applying the accept.
            script.set_on_taxi(on_taxi);
            script.auto_equip_slot(bag, slot, true);
        }
    }
}

/// Apply the item calls and the container uses a harness's VM kept
/// ([`UiScript::keep_calls`]), in call order, as the app's host applies each inside its verb;
/// an open goes through the loot window's applier, its tail and the loot calls it leaves run
/// after it. A call kept while one applies (a send the accept hands over in place) applies
/// after, until none is kept.
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
                loop {
                    let calls = script.take_script_calls();
                    if calls.is_empty() {
                        break;
                    }
                    for call in calls {
                        match call {
                            ScriptCall::UseContainerItem(call) => {
                                if let Some(open) = set.p0().use_container_item(&mut script, call)
                                {
                                    if let Some(tail) = set.p2().open_item(&mut script, open) {
                                        tail(&mut script);
                                    }
                                }
                            }
                            ScriptCall::Item(ItemCall::BindOnUse { may_cast }) => {
                                set.p0().confirm_bind_on_use(&mut script, may_cast);
                            }
                            ScriptCall::Item(call) => set.p1().apply(&mut script, call),
                            ScriptCall::Loot(call) => {
                                if let Some(tail) = set.p2().apply(&mut script, call) {
                                    tail(&mut script);
                                }
                            }
                            _ => {}
                        }
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
        .run_system_once(
            |mut script: NonSendMut<UiScript>, mut steps: ItemSteps| loop {
                let calls = script.take_script_calls();
                if calls.is_empty() {
                    break;
                }
                for call in calls {
                    if let ScriptCall::Item(call) = call {
                        steps.apply(&mut script, call);
                    }
                }
            },
        )
        .unwrap();
}
