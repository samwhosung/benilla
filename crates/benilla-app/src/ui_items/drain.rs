//! The per-frame drains that send the queued Lua container intents as `ClientCommand`s, locking
//! the slots each send touches in [`crate::pending_item_ops::PendingItemOps`].

use bevy::prelude::*;

use benilla_protocol::messages::BAG_PLAYER_INVENTORY;
use benilla_ui::script::{UiScript, EQUIPMENT_BAG};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;

use super::{slot_guid, slot_guid_count, wire_pos, INVTYPE_AMMO};

/// The one auto-equip sender, as in the reference (`AutoEquipCursorItem`, `0x5e1480`); answers
/// whether it sent. An ammo item loads by entry with `CMSG_SET_AMMO` and stays in its bag. An
/// unbound, equippable `bonding == 2` item raises `AUTOEQUIP_BIND_CONFIRM` and sends nothing
/// (`0x5e163b`) unless `suppress`, the reference's flag, which the accept's re-issue sets.
pub(crate) fn send_auto_equip(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    bag_index: u8,
    slot: u8,
    guid: Option<u64>,
    suppress: bool,
) -> bool {
    if !suppress {
        if let Some(guid) = guid {
            if gate.equip_binds(script, objects, items, commands, guid) {
                gate.defer_equip(
                    script,
                    crate::ui_bind_confirm::PendingEquip::AutoEquip {
                        bag_index,
                        slot,
                        guid,
                    },
                );
                return false;
            }
        }
    }
    let ammo_entry = guid.and_then(|guid| {
        let entry = objects.object(guid)?.object_entry()?;
        let t = items.template(entry, guid, commands)?;
        (t.inventory_type == INVTYPE_AMMO).then_some(entry)
    });
    if let Some(entry) = ammo_entry {
        debug!("ui_items: set ammo entry {entry} (wire {bag_index}/{slot})");
        let _ = commands.0.send(ClientCommand::SetAmmo { entry });
    } else {
        debug!("ui_items: autoequip wire {bag_index}/{slot}");
        let _ = commands
            .0
            .send(ClientCommand::AutoEquipItem { bag_index, slot });
    }
    true
}

/// Sends the auto-equips `AutoEquipCursorItem` queued through [`send_auto_equip`]. It takes no
/// pending lock, so the source slot does not dim while the equip is in flight.
pub(super) fn drain_container_autoequips(
    script: Option<NonSendMut<UiScript>>,
    items: Res<Items>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    for (bag, slot) in script.take_container_autoequips() {
        let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
            debug!("ui_items: autoequip ({bag}, {slot}) out of range — ignored");
            continue;
        };
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        let guid = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, bag, slot0, &objects));
        send_auto_equip(
            &mut script,
            &mut gate,
            &objects,
            &items,
            &commands,
            bag_index,
            wire_slot,
            guid,
            false,
        );
    }
}

/// Sends the auto-stores `PutItemInBag`/`PutItemInBackpack` queued. The destination is a bag, not
/// a slot (`0x4c7c00` to `0x5e12e0`): `CMSG_AUTOSTORE_BAG_ITEM` names `(srcbag, srcslot, dstbag)`
/// and the server picks the slot. A split carry sends `CMSG_SPLIT_ITEM` instead (the reference's
/// fork on `[0xb4b40c]`). Takes no pending lock.
pub(super) fn drain_bag_autostores(
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for a in script.take_bag_autostores() {
        let (Some((src_bag, src_slot)), Some((dst_bag, _))) =
            (wire_pos(a.src_bag, a.src_slot), wire_pos(a.dst_bag, 1))
        else {
            debug!("ui_items: bag autostore {a:?} out of range — ignored");
            continue;
        };
        match a.count {
            None => {
                debug!(
                    "ui_items: autostore lua {}/{} → bag {} (wire {src_bag}/{src_slot} → {dst_bag})",
                    a.src_bag, a.src_slot, a.dst_bag
                );
                let _ = commands.0.send(ClientCommand::AutoStoreBagItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                });
            }
            Some(n) => {
                let count = n.min(u32::from(u8::MAX)) as u8;
                debug!(
                    "ui_items: autostore split lua {}/{} → bag {} × {count} (wire {src_bag}/{src_slot} → {dst_bag})",
                    a.src_bag, a.src_slot, a.dst_bag
                );
                let _ = commands.0.send(ClientCommand::SplitItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                    // The reference's literal for no destination slot.
                    dst_slot: 0xFF,
                    count,
                });
            }
        }
    }
}

/// Sends the doll right-clicks `UseInventoryItem` queued through [`super::send_item_use`], as the
/// reference's doll click reaches the same `CGItem::Use` a bag click does (`0x4c7af0`): a worn
/// quest-starter offers its quest. Ids outside 1..=19 are refused.
pub(super) fn drain_inventory_uses(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    targeting: crate::spell::cast_target::CastTargeting,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    for id in script.take_inventory_uses() {
        if !(1..=19).contains(&id) {
            debug!("ui_items: UseInventoryItem({id}) out of range — ignored");
            continue;
        }
        let slot = (id - 1) as u8;
        let (guid, start_quest, spell_index, use_spell, entry, is_charter) = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &ladder.objects))
            .and_then(|guid| {
                let entry = ladder.objects.object(guid)?.object_entry()?;
                let t = ladder.items.template(entry, guid, &ladder.commands)?;
                Some((
                    Some(guid),
                    t.start_quest,
                    t.use_spell_index().unwrap_or(0),
                    t.use_spell.map(|u| u.spell_id),
                    Some(entry),
                    t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                ))
            })
            .unwrap_or((None, 0, 0, None, None, false));
        debug!("ui_items: use equipped item, lua slot {id} (wire 255/{slot})");
        super::send_item_use(
            super::ItemUse {
                guid,
                start_quest,
                bag_index: BAG_PLAYER_INVENTORY,
                slot,
                entry: entry.unwrap_or(0),
                spell_index,
                use_spell,
                on_object: None,
                is_charter,
            },
            &targeting.context(),
            &mut ladder,
            &mut script,
            &mut gate,
            false,
            &mut ui_errors,
        );
    }
}

/// Sends paper-doll repair-mode clicks as `CMSG_REPAIR_ITEM`; the cursor route supplies the
/// equipped item's guid, as the 1.12 client does rather than an inventory position.
pub(super) fn drain_inventory_repairs(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    objects: Objects,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(vendor) = merchant.vendor else {
        return;
    };
    for id in script.take_inventory_repairs() {
        if !(1..=19).contains(&id) {
            debug!("ui_items: repair equipped lua slot {id} out of range — ignored");
            continue;
        }
        let slot = (id - 1) as u8;
        match self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &objects))
        {
            Some(item_guid) => {
                debug!("ui_items: repair equipped lua slot {id} (item {item_guid:#x})");
                let _ = commands
                    .0
                    .send(ClientCommand::RepairItem { vendor, item_guid });
            }
            None => debug!("ui_items: repair equipped empty lua slot {id} — ignored"),
        }
    }
}

pub(super) fn drain_container_uses(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    bank: Res<crate::ui_bank::BankOpen>,
    mut equip_sound: MessageWriter<crate::sound::AutoEquipSound>,
    mut item_text: ResMut<crate::ui_item_text::ItemTextOpen>,
    targeting: crate::spell::cast_target::CastTargeting,
    mut pending_items: ResMut<PendingItemOps>,
    // The loot latch the open arm sets, so `SMSG_LOOT_RESPONSE` admits an item loot.
    mut loot_latch: ResMut<crate::ui_loot::LootLatch>,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    // Repair-mode clicks (the reference's `0x4f9c7b` route) repair the one item. The reference's
    // affordability check (error 0x25) is not built; the server refuses instead.
    for (bag, slot) in script.take_container_repairs() {
        let Some(vendor) = merchant.vendor else {
            continue;
        };
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        let item_guid = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, bag, slot0, &ladder.objects));
        match item_guid {
            Some(guid) => {
                debug!("ui_items: repair lua bag {bag} slot {slot} (item {guid:#x})");
                let _ = ladder.commands.0.send(ClientCommand::RepairItem {
                    vendor,
                    item_guid: guid,
                });
            }
            None => debug!("ui_items: repair on empty slot (bag {bag} slot {slot}) — ignored"),
        }
    }
    for (bag, slot) in script.take_container_uses() {
        // Any right-click, a sell or a deposit included, cancels an armed gift wrap: the
        // reference's use path clears the cursor first (`0x4fa198`).
        if let Some(w) = script.cancel_gift_wrap() {
            debug!(
                "ui_items: right-click cancels the armed gift wrap on bag {} slot {}",
                w.bag, w.slot
            );
        }
        let slot0 = u8::try_from(slot.saturating_sub(1)).ok();
        // With a merchant open, the click sells the whole stack (`CMSG_SELL_ITEM`, count 0).
        if let (true, Some(vendor)) = (merchant.is_open(), merchant.vendor) {
            let item_guid = self_q
                .iter()
                .next()
                .and_then(|store| slot_guid(&store.0, bag, slot0.unwrap_or(0), &ladder.objects));
            match item_guid {
                Some(guid) => {
                    debug!("ui_items: sell lua bag {bag} slot {slot} (item {guid:#x})");
                    let _ = ladder.commands.0.send(ClientCommand::SellItem {
                        vendor,
                        item_guid: guid,
                        count: 0,
                    });
                }
                None => debug!("ui_items: sell on empty slot (bag {bag} slot {slot}) — ignored"),
            }
            continue;
        }
        let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
            debug!("ui_items: UseContainerItem({bag}, {slot}) out of range — ignored");
            continue;
        };
        // With the bank open, a vault or bank-bag item withdraws (`CMSG_AUTOSTORE_BANK_ITEM`) and
        // any other deposits (`CMSG_AUTOBANK_ITEM`): the reference's bank flag, set for bags -1
        // and 5..10 (`0x4f9820`), picks the opcode (`0x4fa2f2`). Doll clicks come through
        // `drain_inventory_uses`, so worn gear keeps its plain use.
        if bank.is_open() {
            let withdrawing = bag == super::BANK_CONTAINER || (5..=10).contains(&bag);
            if withdrawing {
                debug!("ui_items: withdraw (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoStoreBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
            } else {
                debug!("ui_items: deposit (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
            }
            continue;
        }
        // `None` (empty, or the template in flight) falls through to a plain use.
        let clicked = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, bag, slot0.unwrap_or(0), &ladder.objects))
            .and_then(|guid| {
                let obj = ladder.objects.object(guid)?;
                let inst_flags = obj.item_flags().unwrap_or(0);
                let item_text_id = obj.item_text_id().unwrap_or(0);
                let entry = obj.object_entry()?;
                let t = ladder.items.template(entry, guid, &ladder.commands)?;
                Some(Clicked {
                    guid,
                    entry,
                    item_text_id,
                    inventory_type: t.inventory_type,
                    display_info_id: t.display_info_id,
                    start_quest: t.start_quest,
                    spell_index: t.use_spell_index().unwrap_or(0),
                    use_spell: t.use_spell.map(|u| u.spell_id),
                    unwraps_gift: t.unwraps_gift(inst_flags),
                    begins_gift_wrap: t.begins_gift_wrap(inst_flags),
                    opens_loot: t.opens_loot(),
                    page_text: t.page_text,
                    is_charter: t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                })
            });

        // The reference's equip-vs-use fork (`0x4fa3b9`): an equippable item auto-equips unless
        // its `StartQuest` (`[rec+0x1a8]`) is set (`0x4fa3bd`). Everything below is the use
        // dispatcher `0x5d8d00`, in its own order.
        if let Some(c) = clicked.filter(|c| c.start_quest == 0 && c.inventory_type != 0) {
            // This path never moves the cursor, so it plays the equip sound itself; a deferred
            // equip plays none, as the reference's sound rides the arm that sends.
            if send_auto_equip(
                &mut script,
                &mut gate,
                &ladder.objects,
                &ladder.items,
                &ladder.commands,
                bag_index,
                wire_slot,
                Some(c.guid),
                false,
            ) {
                equip_sound.write(crate::sound::AutoEquipSound {
                    display_id: c.display_info_id,
                });
            }
            continue;
        }
        // A wrapped gift unwraps with `CMSG_OPEN_ITEM` (`0x5d8d92`, emitter `0x5edd60`), first in
        // the dispatcher; vmangos swaps the entry back from `character_gifts`.
        if let Some(c) = clicked.filter(|c| c.unwraps_gift) {
            debug!(
                "ui_items: unwrap gift {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                c.guid
            );
            let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                bag_index,
                slot: wire_slot,
            });
            continue;
        }
        // Wrapping paper (`0x5d8d9d`'s other branch, `0x5edea0`) sends nothing: its slot locks and
        // the cursor becomes mode 2 until a left-click on a container slot sends `CMSG_WRAP_ITEM`.
        if let Some(c) = clicked.filter(|c| c.begins_gift_wrap) {
            debug!(
                "ui_items: arm gift wrap with {:#x} (lua bag {bag} slot {slot})",
                c.guid
            );
            script.arm_gift_wrap(bag, slot);
            continue;
        }
        // A quest-starter (`0x5d8dd2`) offers its quest, ahead of the readable and open arms.
        if let Some(c) = clicked.filter(|c| c.start_quest != 0) {
            debug!(
                "ui_items: quest-starter {:#x} offers quest {}",
                c.guid, c.start_quest
            );
            super::send_item_use(
                super::ItemUse {
                    guid: Some(c.guid),
                    start_quest: c.start_quest,
                    bag_index,
                    slot: wire_slot,
                    entry: c.entry,
                    spell_index: c.spell_index,
                    use_spell: c.use_spell,
                    on_object: None,
                    is_charter: c.is_charter,
                },
                &targeting.context(),
                &mut ladder,
                &mut script,
                &mut gate,
                false,
                &mut ui_errors,
            );
            continue;
        }
        // A book, a template with `PageText` (`0x5d8e4c`), opens the reader with no packet
        // (`0x4e32e0`). It precedes the open arm, the inverse of the tooltip: an item both
        // readable and lootable shows `<Right Click to Open>` and reads on click.
        if let Some(c) = clicked.filter(|c| c.page_text != 0) {
            if item_text.toggle_closed(c.guid) {
                debug!("ui_items: re-click closes the book {:#x}", c.guid);
            } else {
                debug!(
                    "ui_items: read book {:#x} (page {}, lua bag {bag} slot {slot})",
                    c.guid, c.page_text
                );
                item_text.open_pages(c.guid);
            }
            continue;
        }
        // A letter from mail (`ITEM_FIELD_ITEM_TEXT_ID`) opens the reader with no packet; vmangos's
        // `CMSG_READ_ITEM` wants a template `PageText`, so the text comes by item-text query.
        if let Some(c) = clicked.filter(|c| c.item_text_id != 0) {
            if item_text.toggle_closed(c.guid) {
                debug!("ui_items: re-click closes the letter {:#x}", c.guid);
            } else {
                debug!(
                    "ui_items: read letter {:#x} (text {}, lua bag {bag} slot {slot})",
                    c.guid, c.item_text_id
                );
                item_text.open_letter(c.guid, c.item_text_id);
            }
            continue;
        }
        // A lootable template (`0x5d8f7c`, emitter `0x5edc80`) sends `CMSG_OPEN_ITEM`, which the
        // server answers with a loot window on the item (`SpellHandler.cpp:227`). Unlike the
        // tooltip line, the send tests neither the lock id nor the unlocked bit, so a locked box
        // sends and the server's `EQUIP_ERR_ITEM_LOCKED` is the "Item is locked" line.
        if let Some(c) = clicked.filter(|c| c.opens_loot) {
            debug!(
                "ui_items: open item {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                c.guid
            );
            // The loot latch on the item's guid, before the send (`0x5edcc0`); the admission gate
            // refuses the type-1 `SMSG_LOOT_RESPONSE` without it. No kneel (`0x612710`).
            loot_latch.0 = Some(c.guid);
            // The grey lock, also before the send (lock setter `0x4953e0` at `0x5edcd9`); it holds
            // until the item empties, the server refuses, or `SMSG_LOOT_RELEASE_RESPONSE` unlocks
            // it by guid. The unwrap arm's emitter (`0x5edd60`) takes neither lock nor latch.
            let (guid, count) = self_q
                .iter()
                .next()
                .map(|store| slot_guid_count(Some(store), bag, slot, &ladder.objects))
                .unwrap_or((0, 0));
            pending_items.add([(bag, slot, guid, count)]);
            script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
            let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                bag_index,
                slot: wire_slot,
            });
            continue;
        }
        // A plain use (food, potions, the hearthstone); quest-starters took the arm above.
        debug!("ui_items: use item (lua bag {bag} → wire {bag_index}/{wire_slot})");
        super::send_item_use(
            super::ItemUse {
                guid: clicked.map(|c| c.guid),
                start_quest: 0,
                bag_index,
                slot: wire_slot,
                entry: clicked.map_or(0, |c| c.entry),
                spell_index: clicked.map_or(0, |c| c.spell_index),
                use_spell: clicked.and_then(|c| c.use_spell),
                on_object: None,
                is_charter: clicked.is_some_and(|c| c.is_charter),
            },
            &targeting.context(),
            &mut ladder,
            &mut script,
            &mut gate,
            false,
            &mut ui_errors,
        );
    }
}

/// One clicked bag slot, resolved once: the fields the reference's fork chain tests.
#[derive(Clone, Copy)]
struct Clicked {
    guid: u64,
    entry: u32,
    /// Instance `ITEM_FIELD_ITEM_TEXT_ID`, nonzero on a letter made from mail.
    item_text_id: u32,
    inventory_type: u32,
    display_info_id: u32,
    start_quest: u32,
    spell_index: u8,
    /// The template's on-use spell, the key of the cast tail's in-flight guard.
    use_spell: Option<u32>,
    unwraps_gift: bool,
    begins_gift_wrap: bool,
    opens_loot: bool,
    /// The template's `PageText`, nonzero on a book.
    page_text: u32,
    is_charter: bool,
}

/// Sends the gift wraps and the moves `PickupContainerItem`/`SplitContainerItem` queued, the moves
/// through [`send_container_move`].
pub(super) fn drain_container_moves(
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    mut pending: ResMut<PendingItemOps>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    let store = self_q.iter().next();
    // A left-click that spent an armed paper: paper first, then the target. Eligibility is the
    // server's, refused with an `ERR_CANT_WRAP_*` reason.
    for (gift_bag, gift_slot, item_bag, item_slot) in script.take_container_wraps() {
        let (Some((gift_bag_index, gift_wire_slot)), Some((item_bag_index, item_wire_slot))) =
            (wire_pos(gift_bag, gift_slot), wire_pos(item_bag, item_slot))
        else {
            debug!(
                "ui_items: WrapItem({gift_bag}/{gift_slot} → {item_bag}/{item_slot}) out of \
                 range — ignored"
            );
            continue;
        };
        debug!(
            "ui_items: wrap {item_bag}/{item_slot} with the paper at {gift_bag}/{gift_slot} \
             (wire {gift_bag_index}/{gift_wire_slot} → {item_bag_index}/{item_wire_slot})"
        );
        let _ = commands.0.send(ClientCommand::WrapItem {
            gift_bag: gift_bag_index,
            gift_slot: gift_wire_slot,
            item_bag: item_bag_index,
            item_slot: item_wire_slot,
        });
    }
    for mv in script.take_container_moves() {
        send_container_move(
            &mut script,
            &mut gate,
            &objects,
            &items,
            &commands,
            store,
            &mut pending,
            mv,
            false,
        );
    }
}

/// The equip deferral's positions (`0x5e0c40`): slots 0..=22 (equipment and equipped bags) and
/// 63..=68 (bank bags) of the player's own array; the backpack and keyring are not.
fn is_equip_position(bag_index: u8, slot: u8) -> bool {
    bag_index == BAG_PLAYER_INVENTORY && (slot <= 22 || (63..=68).contains(&slot))
}

/// One container move: the soulbind deferral, then the send and a lock on both ends. A whole stack
/// is `CMSG_SWAP_INV_ITEM` when both ends are in the player's own array, else `CMSG_SWAP_ITEM`
/// (destination first, `Server/Packets/Item.cpp:30-36`); a split is `CMSG_SPLIT_ITEM`. The
/// accept's re-issue runs it with `suppress`. False when deferred behind `EQUIP_BIND_CONFIRM`.
pub(crate) fn send_container_move(
    script: &mut UiScript,
    gate: &mut crate::ui_bind_confirm::BindGate,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    store: Option<&ObjectStore>,
    pending: &mut PendingItemOps,
    mv: benilla_ui::script::ContainerMove,
    suppress: bool,
) -> bool {
    {
        let (Some(src), Some(dst)) = (
            wire_pos(mv.src_bag, mv.src_slot),
            wire_pos(mv.dst_bag, mv.dst_slot),
        ) else {
            debug!("ui_items: container move {mv:?} out of range — ignored");
            return false;
        };
        // The deferral (`0x5e0c40`) asks when exactly one end is an equip position, about the item
        // at the other end, which the swap equips; an unequip to an empty slot asks nothing.
        if !suppress
            && mv.count.is_none()
            && is_equip_position(dst.0, dst.1) != is_equip_position(src.0, src.1)
        {
            let (item_bag, item_slot) = if is_equip_position(dst.0, dst.1) {
                (mv.src_bag, mv.src_slot)
            } else {
                (mv.dst_bag, mv.dst_slot)
            };
            let slot0 = u8::try_from(item_slot.saturating_sub(1)).unwrap_or(0);
            let guid = store.and_then(|s| slot_guid(&s.0, item_bag, slot0, objects));
            if let Some(guid) = guid {
                if gate.equip_binds(script, objects, items, commands, guid) {
                    gate.defer_equip(
                        script,
                        crate::ui_bind_confirm::PendingEquip::Swap {
                            src_bag: mv.src_bag,
                            src_slot: mv.src_slot,
                            dst_bag: mv.dst_bag,
                            dst_slot: mv.dst_slot,
                        },
                    );
                    return false;
                }
            }
        }
        match mv.count {
            None => {
                let (src_wire_bag, src_slot) = src;
                let (dst_wire_bag, dst_slot) = dst;
                if src_wire_bag == BAG_PLAYER_INVENTORY && dst_wire_bag == BAG_PLAYER_INVENTORY {
                    debug!(
                        "ui_items: swap backpack lua {}→{} (wire 255 slot {src_slot}↔{dst_slot})",
                        mv.src_slot, mv.dst_slot
                    );
                    let _ = commands
                        .0
                        .send(ClientCommand::SwapInvItem { src_slot, dst_slot });
                } else {
                    debug!(
                        "ui_items: swap whole-space lua {}/{}→{}/{} (wire {src_wire_bag}/{src_slot}→{dst_wire_bag}/{dst_slot})",
                        mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
                    );
                    let _ = commands.0.send(ClientCommand::SwapItem {
                        dst_bag: dst_wire_bag,
                        dst_slot,
                        src_bag: src_wire_bag,
                        src_slot,
                    });
                }
            }
            Some(n) => {
                let (src_bag, src_slot) = src;
                let (dst_bag, dst_slot) = dst;
                let count = n.min(u32::from(u8::MAX)) as u8;
                debug!(
                    "ui_items: split lua {}/{}→{}/{} × {count} (wire {src_bag}/{src_slot}→{dst_bag}/{dst_slot})",
                    mv.src_bag, mv.src_slot, mv.dst_bag, mv.dst_slot
                );
                let _ = commands.0.send(ClientCommand::SplitItem {
                    src_bag,
                    src_slot,
                    dst_bag,
                    dst_slot,
                    count,
                });
            }
        }
        // Both ends lock on their current (guid, count), which the resolving clear watches.
        let (src_guid, src_count) = slot_guid_count(store, mv.src_bag, mv.src_slot, objects);
        let (dst_guid, dst_count) = slot_guid_count(store, mv.dst_bag, mv.dst_slot, objects);
        pending.add([
            (mv.src_bag, mv.src_slot, src_guid, src_count),
            (mv.dst_bag, mv.dst_slot, dst_guid, dst_count),
        ]);
        // One argless event per locked end, as the reference fires per slot.
        for _ in 0..2 {
            script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
        }
    }
    true
}

/// Sends the destroys `DeleteCursorItem` queued as `CMSG_DESTROYITEM` (count 0 is the whole stack
/// on both sides) and locks the one slot.
pub(super) fn drain_container_destroys(
    script: Option<NonSendMut<UiScript>>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    mut pending: ResMut<PendingItemOps>,
) {
    let Some(mut script) = script else {
        return;
    };
    let store = self_q.iter().next();
    for (bag, slot, count) in script.take_container_destroys() {
        let Some((bag_index, wire_slot)) = wire_pos(bag, slot) else {
            debug!("ui_items: destroy ({bag}, {slot}) out of range — ignored");
            continue;
        };
        let count = count.min(u32::from(u8::MAX)) as u8;
        debug!("ui_items: destroy lua {bag}/{slot} × {count} (wire {bag_index}/{wire_slot})");
        let _ = commands.0.send(ClientCommand::DestroyItem {
            bag_index,
            slot: wire_slot,
            count,
        });
        let (guid, stack) = slot_guid_count(store, bag, slot, &objects);
        pending.add([(bag, slot, guid, stack)]);
        script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ItemInfo, ObjectFields, ITEM_FLAG_LOOTABLE};
    use bevy::ecs::system::RunSystemOnce;

    /// Small Barnacled Clam.
    const CLAM_ENTRY: u32 = 7973;
    const CLAM: u64 = 0x4000_0000_0000_1939;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    /// `OBJECT_FIELD_ENTRY` on the item object.
    const F_OBJECT_ENTRY: u16 = 3;

    /// Right-clicks backpack slot 1, holding a lootable template, and runs the click dispatcher.
    fn open_the_clam() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
            .init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_action::CastErrors>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, CLAM as u32),
                (F_PACK_SLOT_1 + 1, (CLAM >> 32) as u32),
            ])),
        ));
        // The item object and its lootable template, so the open arm claims the click.
        crate::items::test_spawn_item(
            app.world_mut(),
            CLAM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, CLAM_ENTRY)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: ITEM_FLAG_LOOTABLE,
                ..crate::items::test_template("Small Barnacled Clam")
            }),
        );

        let script = UiScript::new().unwrap();
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_uses)
            .unwrap();
        (app, rx)
    }

    /// vmangos answers with a type-1 `SMSG_LOOT_RESPONSE` on the item's guid, refused against a
    /// cold latch: without the latch the clam greys and no window opens.
    #[test]
    fn the_open_item_send_arms_the_loot_latch_on_the_items_own_guid() {
        let (app, rx) = open_the_clam();
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::OpenItem {
                    bag_index: 255,
                    slot: 23
                })
            ),
            "the open arm ships CMSG_OPEN_ITEM for backpack slot 1"
        );
        assert_eq!(
            app.world().resource::<crate::ui_loot::LootLatch>().0,
            Some(CLAM),
            "…having first latched the item's own guid, or the type-1 answer is refused"
        );
        // The grey lock, the reference's other pre-send write.
        assert!(
            app.world().resource::<PendingItemOps>().contains(0, 1),
            "the slot greys at the click"
        );
    }

    /// The unwrap arm's other side: wrapping paper arms the wrap cursor and sends nothing.
    #[test]
    fn a_wrapper_right_click_arms_the_cursor_and_ships_no_packet() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
                                       // Slot 1 becomes wrapping paper: WRAPPER on the
                                       // template, no WRAPPED bit on the instance.
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                flags: benilla_protocol::messages::ITEM_FLAG_WRAPPER,
                ..crate::items::test_template("Red Ribboned Wrapping Paper")
            }),
        );
        {
            let script = app.world().non_send_resource::<UiScript>();
            script.run("UseContainerItem(0, 1)").unwrap();
        }
        app.world_mut()
            .run_system_once(drain_container_uses)
            .unwrap();

        assert!(
            rx.try_recv().is_err(),
            "the begin-wrap arm is purely local — nothing goes on the wire until a container \
             click spends it"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.gift_wrap_armed(),
            Some(benilla_ui::script::PendingWrap { bag: 0, slot: 1 }),
            "…and the paper is armed"
        );
    }
}

/// The answers to `EquipPendingItem`/`CancelPendingEquip`. Accept re-runs the original sender with
/// `suppress` set, as the reference's `0x5e1be0` does: 1.12 has no confirm opcode, and a slot that
/// changed under the dialog is judged afresh. Cancel sends nothing and frees the record.
///
/// Deviation: the reference's cancel also unlocks both occupants (`UnlockItem`, `0x495420`), which
/// its deferral locked; ours locks nothing while the question is up, because a [`PendingItemOps`]
/// lock clears only against a wire op and a deferred action has none.
pub(super) fn drain_bind_confirm_answers(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut pending: ResMut<PendingItemOps>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    let store = self_q.iter().next();
    for answer in script.take_pending_equip_answers() {
        // An unfiled index does nothing: `0x5e1be0` bounds-checks it and returns.
        let Some(rec) = gate.equips.take(answer.index) else {
            debug!(
                "ui_items: bind answer for index {} — no such pending equip",
                answer.index
            );
            continue;
        };
        if !answer.accept {
            debug!("ui_items: pending equip {} cancelled", answer.index);
            continue;
        }
        match rec {
            crate::ui_bind_confirm::PendingEquip::Swap {
                src_bag,
                src_slot,
                dst_bag,
                dst_slot,
            } => {
                send_container_move(
                    &mut script,
                    &mut gate,
                    &objects,
                    &items,
                    &commands,
                    store,
                    &mut pending,
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
            crate::ui_bind_confirm::PendingEquip::AutoEquip {
                bag_index,
                slot,
                guid,
            } => {
                send_auto_equip(
                    &mut script,
                    &mut gate,
                    &objects,
                    &items,
                    &commands,
                    bag_index,
                    slot,
                    Some(guid),
                    true,
                );
            }
        }
    }
}

/// `ConfirmBindOnUse()`: re-issues the pending use through the cast ladder with `suppress` set.
/// The pending state is one cell, taken on the first accept, so a doubled accept does nothing.
pub(super) fn drain_bind_on_use_confirms(
    script: Option<NonSendMut<UiScript>>,
    targeting: crate::spell::cast_target::CastTargeting,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_bind_on_use_confirms() == 0 {
        return;
    }
    let Some(it) = gate.on_use.0.take() else {
        return;
    };
    debug!(
        "ui_items: bind-on-use confirmed for wire {}/{}",
        it.bag_index, it.slot
    );
    super::send_item_use(
        it,
        &targeting.context(),
        &mut ladder,
        &mut script,
        &mut gate,
        true,
        &mut ui_errors,
    );
}

#[cfg(test)]
mod bind_confirm_tests {
    use super::*;
    use benilla_protocol::messages::{ItemInfo, ObjectFields};
    use benilla_ui::script::{ContainerSlot, ContainerState};
    use bevy::ecs::system::RunSystemOnce;

    /// Flurry Axe, a real `item_template` row: quality 4, bonding 2 (bind on equip).
    const FLURRY_AXE: u32 = 871;
    const AXE_GUID: u64 = 0x4000_0000_0000_0871;
    /// `ITEM_FIELD_FLAGS` (field 21); bit 0 is soulbound (`0x5da2c0`).
    const F_ITEM_FLAGS: u16 = 21;
    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    const F_OBJECT_ENTRY: u16 = 3;

    fn load_ui(s: &UiScript) {
        // Chain files, so through the chain-aware reader.
        for file in [
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            r"Interface\FrameXML\UIParent.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            "Interface\\FrameXML\\StaticPopup.xml",
        ] {
            crate::ui_script::load_ui_for_test(s, file);
        }
    }

    fn bag_with_the_axe(quality: u32) -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Axe_01".into()),
                count: 1,
                quality: Some(quality),
                item_id: FLURRY_AXE,
                link: Some("|cffa335ee|Hitem:871:0:0:0|h[Flurry Axe]|h|r".into()),
                locked: false,
                equip_slots: vec![16], // MainHandSlot
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// Places the axe from backpack slot 1 on the empty main hand and runs the move drain.
    fn place_the_axe_on_the_doll() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(2, 4, false)
    }

    fn place_the_axe_with_bonding(
        bonding: u32,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(bonding, 4, false)
    }

    /// The place, over everything the gate reads; `quality` must not matter.
    fn place_the_axe_with(
        bonding: u32,
        quality: u32,
        already_bound: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));

        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[
                (F_OBJECT_ENTRY, FLURRY_AXE),
                (F_ITEM_FLAGS, u32::from(already_bound)),
            ]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality,
                bonding,
                ..crate::items::test_template("Flurry Axe")
            }),
        );

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(quality)));
        let doll: benilla_ui::script::InventorySlots = Default::default();
        script.set_inventory_slots(doll);
        // Pick up and drop on the main hand: a queued move and a pending CURSOR_UPDATE.
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("PickupInventoryItem(16)").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        (app, rx)
    }

    fn shown(app: &mut App, which: &str) -> bool {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<bool>(&format!(
                "return StaticPopup_FindVisible(\"{which}\") ~= nil"
            ))
            .unwrap()
    }

    /// The question is queued behind the place's own `CURSOR_UPDATE`, whose `StaticPopup_Hide`
    /// (`UIParent.lua:357-359`) would otherwise cancel it unseen: benilla drains a step after the
    /// input pass, where the reference defers inside the call. A later cursor change retires it.
    #[test]
    fn placing_a_boe_on_the_doll_asks_and_survives_its_own_cursor_update() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred place sends nothing"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "the question is queued, so it is not up in the drain's own frame"
        );

        // The next tick flushes the place's CURSOR_UPDATE, then the question.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "EQUIP_BIND"),
            "the dialog survives its own place's CURSOR_UPDATE and shows"
        );
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<i64>("return StaticPopup_FindVisible(\"EQUIP_BIND\").data")
                .unwrap(),
            0,
            "carrying the pending-array index, which is 0 for the first record"
        );

        // A quiet frame changes nothing.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "EQUIP_BIND"), "and stays up");

        // A later cursor change retires it, and its OnHide cancels the record.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .fire_event("CURSOR_UPDATE", vec![]);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a later cursor change still retires the stale question"
        );
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and the record it was holding is freed, not leaked"
        );
        assert!(
            rx.try_iter().next().is_none(),
            "a retired question never sends"
        );
    }

    /// The accept re-issues the place (1.12 has no confirm opcode); `suppress` stops a second ask.
    #[test]
    fn accepting_re_issues_the_place_and_does_not_ask_again() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "EQUIP_BIND"));

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();

        // Backpack slot 1 is 255/23 and the main hand 255/15, both in the player's own array.
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::SwapInvItem {
                    src_slot: 23,
                    dst_slot: 15
                }]
            ),
            "the accept sends the original swap, once: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0,
            "and frees its record — the OnHide's cancel finds nothing left to cancel"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and no second question is queued: the re-issue was suppressed"
        );
    }

    /// The stock popup cancels twice, from `OnCancel` and `OnHide`; the second finds the record
    /// already free.
    #[test]
    fn cancelling_sends_nothing_and_the_doubled_cancel_is_harmless() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 2)")
            .unwrap();
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .take_pending_equip_answers()
                .len(),
            2,
            "OnCancel and OnHide both fire"
        );
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert!(rx.try_iter().next().is_none(), "a cancel never sends");
    }

    /// The equip predicate is `bonding == 2` alone, with no quality leg (`0x5e0e54`).
    #[test]
    fn only_bind_on_equip_defers_the_place() {
        benilla_formats::wow_data_or_skip!();
        for (bonding, why) in [
            (0u32, "no bind"),
            (1, "bind on PICKUP is the loot arm's value, not this one"),
            (3, "bind on USE is the use arm's"),
            (4, "quest item"),
        ] {
            let (mut app, rx) = place_the_axe_with_bonding(bonding);
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .tick(0.01);
            assert!(
                !shown(&mut app, "EQUIP_BIND"),
                "bonding {bonding} must not ask ({why})"
            );
            assert!(
                rx.try_iter().next().is_some(),
                "bonding {bonding} places straight through ({why})"
            );
        }
    }

    #[test]
    fn a_white_bind_on_equip_item_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 1, false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "EQUIP_BIND"),
            "quality 1 is irrelevant to the equip arm"
        );
        assert!(rx.try_iter().next().is_none());
    }

    /// `0x5da2c0`, the predicate the tooltip's Soulbound override also uses.
    #[test]
    fn an_already_bound_item_places_without_asking() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "there is nothing left to bind"
        );
        assert!(rx.try_iter().next().is_some(), "and it places");
    }

    /// The auto-equip arm (`0x5e1480`, event 289): the same predicate and re-issue, its own dialog.
    #[test]
    fn auto_equipping_a_boe_asks_and_the_accept_re_issues() {
        benilla_formats::wow_data_or_skip!();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(4)));
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("AutoEquipCursorItem()").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_autoequips)
            .unwrap();

        assert!(
            rx.try_iter().next().is_none(),
            "the deferred equip sends nothing"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "AUTOEQUIP_BIND"),
            "the auto-equip arm raises its OWN dialog, not the placement one"
        );
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "and the two are distinct entries"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AUTOEQUIP_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 23
                }]
            ),
            "the accept re-issues the auto-equip on the item's wire position: {sent:?}"
        );
        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            0
        );
    }

    /// The `0x5ea930` conjunct, driven by its level leg.
    #[test]
    fn an_unusable_item_is_never_asked_about() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, false);
        // A real player level (the fixture's 0 declines to judge) and a template it refuses.
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 10,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            });
            s.set_item_template(
                FLURRY_AXE,
                benilla_ui::script::ItemTemplateView {
                    required_level: 60,
                    allowable_class: -1,
                    allowable_race: -1,
                    ..Default::default()
                },
            );
        }
        // Re-run the place now that the item is out of reach.
        {
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.run("PickupContainerItem(0, 1)").unwrap();
            s.run("PickupInventoryItem(16)").unwrap();
        }
        let _ = rx.try_iter().count();
        app.world_mut()
            .run_system_once(drain_container_moves)
            .unwrap();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            !shown(&mut app, "EQUIP_BIND"),
            "a level-60 axe on a level-10 player asks nothing"
        );
        assert!(
            rx.try_iter().next().is_some(),
            "it places, and the server refuses it — which is where that refusal belongs"
        );
    }

    /// Right-clicks a bind-on-use item in a bag (the use arm, `0x5d8d00`, event 290);
    /// `no_use_spell` drops its on-use spell, since four of `0x5d91d3`'s five predecessors are
    /// failed spell lookups and the reference still asks.
    fn right_click_a_bind_on_use_item(
        no_use_spell: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
            .init_resource::<PendingItemOps>()
            .init_resource::<crate::ui_bind_confirm::PendingEquips>()
            .init_resource::<crate::ui_bind_confirm::PendingBindOnUse>()
            .init_resource::<crate::ui_loot::LootLatch>()
            .init_resource::<crate::target::Selection>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::spell::cast_target::AutoSelfCast>()
            .init_resource::<crate::net::Reputations>()
            .init_resource::<crate::player::Player>()
            .init_resource::<crate::spell::PendingCast>()
            .init_resource::<crate::spell::QueuedMeleeSpell>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_action::CastErrors>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, AXE_GUID as u32),
                (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        let mut items = app.world_mut().resource_mut::<Items>();
        let mut template = crate::items::test_template("A Bind-On-Use Thing");
        template.bonding = 3;
        template.quality = 3;
        template.inventory_type = 0; // not equippable, so the equip fork above cannot claim it
        if !no_use_spell {
            template.spells = vec![benilla_protocol::messages::ItemSpellEntry {
                index: 0,
                spell_id: 4321,
                trigger: 0, // ON_USE
                charges: 0,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }];
            template.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
        }
        items.insert_template(FLURRY_AXE, Some(template));

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(3)));
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_uses)
            .unwrap();
        (app, rx)
    }

    #[test]
    fn right_clicking_a_bind_on_use_item_asks_before_using_it() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        assert!(
            rx.try_iter().next().is_none(),
            "the deferred use sends nothing"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "USE_BIND"), "USE_BIND is raised");
        assert_eq!(
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<String>(
                    "return getglobal(StaticPopup_FindVisible(\"USE_BIND\"):GetName() .. \"Text\"):GetText()"
                )
                .unwrap(),
            "Using this item will bind it to you.",
            "the real GlobalStrings USE_NO_DROP"
        );

        // Accept → ConfirmBindOnUse() → the use is re-issued with suppress set.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"USE_BIND\"), 1)")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_on_use_confirms)
            .unwrap();
        assert!(
            rx.try_iter().count() > 0,
            "the accept re-issues the use rather than sending a confirm packet"
        );
    }

    #[test]
    fn a_bind_on_use_item_with_no_on_use_spell_still_asks() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(true);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(
            shown(&mut app, "USE_BIND"),
            "the reference's bind arm sits BELOW the on-use lookup's failure exits, not above them"
        );
        assert!(rx.try_iter().next().is_none());
    }
}
