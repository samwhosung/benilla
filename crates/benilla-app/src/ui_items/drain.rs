//! The per-frame drains that send the queued Lua container intents as `ClientCommand`s. The verbs
//! lock what they touch in the call, as the reference's do; the drains only send.

use bevy::prelude::*;

use benilla_protocol::messages::BAG_PLAYER_INVENTORY;
use benilla_ui::script::{ContainerUse, UiScript, UseArm, UseFork, EQUIPMENT_BAG};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};

use super::{slot_guid, wire_pos, ITEM_CLASS_PROJECTILE};
use crate::query_cache::Ask;

/// The one auto-equip sender (`AutoEquipCursorItem`, `0x5e1480`, past its question). A
/// projectile, the template class `0x5d9f30` reads (`0x5e16ff`, `0x5e1704`), loads by entry with
/// `CMSG_SET_AMMO` and stays in its bag, and the arm's `ClearCursor(1,1)` unlocks it (`0x5e1711`,
/// `0x5e1763`): an item the call's clear left locked, picked up before its template landed, is let
/// go here. Anything else is `CMSG_AUTOEQUIP_ITEM`. The bind-on-equip question is the call's, not
/// the sender's: the VM decided before this ran ([`benilla_ui::script::AutoEquip`]).
pub(crate) fn send_auto_equip(
    script: &mut UiScript,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    bag_index: u8,
    slot: u8,
    guid: Option<u64>,
) {
    let ammo = guid.and_then(|guid| {
        let entry = objects.object(guid)?.object_entry()?;
        // The class getter only reads (`0x5d9f30`).
        let t = items.template(entry, guid, Ask::Read)?;
        (t.class == ITEM_CLASS_PROJECTILE).then_some((guid, entry))
    });
    if let Some((guid, entry)) = ammo {
        debug!("ui_items: set ammo entry {entry} (wire {bag_index}/{slot})");
        let _ = commands.0.send(ClientCommand::SetAmmo { entry });
        if script.item_locked(guid) {
            script.unlock_item(guid);
        }
    } else {
        debug!("ui_items: autoequip wire {bag_index}/{slot}");
        let _ = commands
            .0
            .send(ClientCommand::AutoEquipItem { bag_index, slot });
    }
}

/// Sends the auto-equips `AutoEquipCursorItem` queued through [`send_auto_equip`], and files the
/// bind-on-equip questions it raised instead: those items stay held, so nothing is sent. The
/// verb's clear kept the pickup's lock, an arrow's excepted (`0x5e1577`, `0x5e1711`).
pub(super) fn drain_container_autoequips(
    mut vm: crate::script_calls::VmWith<crate::ui_bind_confirm::BindGate<'static>>,
    items: Res<Items>,
    objects: Objects,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    commands: Res<NetCommands>,
) {
    vm.turn(|script, mut gate| {
        let Some(script) = script else {
            return;
        };
        for (bag, slot, guid) in script.take_container_equip_asks() {
            gate.defer_equip(
                script,
                crate::ui_bind_confirm::PendingEquip::AutoEquip { bag, slot, guid },
            );
        }
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
                script, &objects, &items, &commands, bag_index, wire_slot, guid,
            );
        }
    })
}

/// Sends the auto-stores `PutItemInBag`/`PutItemInBackpack` queued. The destination is a bag, not
/// a slot (`0x4c7c00` to `0x5e12e0`): `CMSG_AUTOSTORE_BAG_ITEM` names `(srcbag, srcslot, dstbag)`
/// and the server picks the slot. A split carry sends `CMSG_SPLIT_ITEM` instead (the reference's
/// fork on `[0xb4b40c]`). Takes no pending lock.
pub(super) fn drain_bag_autostores(
    mut hosted_vm: crate::script_calls::HostedVm,
    commands: Res<NetCommands>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
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
                        "ui_items: autostore lua {}/{} → bag {} (wire {src_bag}/{src_slot} → \
                         {dst_bag})",
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
                        "ui_items: autostore split lua {}/{} → bag {} × {count} (wire \
                         {src_bag}/{src_slot} → {dst_bag})",
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
    })
}

/// What a `UseContainerItem` or `UseInventoryItem` call reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptItemUse<'w, 's> {
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    merchant: Res<'w, crate::ui_merchant::MerchantOpen>,
    equip_sound: MessageWriter<'w, crate::sound::AutoEquipSound>,
    item_text: ResMut<'w, crate::ui_item_text::ItemTextOpen>,
    targeting: crate::spell::cast_target::CastTargeting<'w, 's>,
    // The loot latch the open arm sets, so `SMSG_LOOT_RESPONSE` admits an item loot.
    loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
    ladder: crate::spell::CastLadder<'w, 's>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
}

impl ScriptItemUse<'_, '_> {
    /// `UseInventoryItem(id, onSelf)`, a doll right-click, sent through [`super::send_item_use`],
    /// as the reference's doll click reaches the same `CGItem::Use` a bag click does
    /// (`0x4c7af0`): a worn quest-starter offers its quest. `on_self` binds the player in place of
    /// the selection (`0x4c8e48`). The call refused a locked item. Ids outside 1..=19 are refused.
    pub(crate) fn use_inventory_item(&mut self, script: &mut UiScript, id: u32, on_self: bool) {
        let Self {
            self_q,
            targeting,
            ladder,
            ui_errors,
            gate,
            ..
        } = self;
        if !(1..=19).contains(&id) {
            debug!("ui_items: UseInventoryItem({id}) out of range — ignored");
            return;
        }
        let slot = (id - 1) as u8;
        let (guid, start_quest, spell_index, use_spell, entry, is_charter) = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &ladder.objects))
            .and_then(|guid| {
                let entry = ladder.objects.object(guid)?.object_entry()?;
                // The use only reads (`0x5d8d57`).
                let t = ladder.items.template(entry, guid, Ask::Read)?;
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
            &targeting.context().self_bound(on_self),
            ladder,
            script,
            gate,
            false,
            ui_errors,
        );
    }

    /// `UseContainerItem(bag, slot)` outside repair mode: the arm the call took, sent. The call
    /// chose it and took its lock ([`benilla_ui::script::UseArm`]), so this only sends.
    pub(crate) fn use_container_item(&mut self, script: &mut UiScript, call: ContainerUse) {
        let Self {
            self_q,
            merchant,
            equip_sound,
            item_text,
            targeting,
            loot_latch,
            ladder,
            ui_errors,
            gate,
            ..
        } = self;
        let ContainerUse {
            bag,
            slot,
            guid: item_guid,
            arm,
            asks,
            on_self,
            ..
        } = call;
        // A bind-on-equip equip the call asked about is filed whatever the slot holds now: the
        // reference raised the question inside the call (`0x5e16c6`), the item is on the cursor,
        // and the answer re-judges the slot (`0x5e1c23`) or only unlocks (`0x5e1c50`).
        if asks {
            gate.defer_equip(
                script,
                crate::ui_bind_confirm::PendingEquip::AutoEquip {
                    bag,
                    slot,
                    guid: item_guid,
                },
            );
            return;
        }
        let slot0 = u8::try_from(slot.saturating_sub(1)).ok();
        // The slot must still hold the item the call resolved (`0x4fa1a6`). The reference sends
        // inside the call, so nothing moves between; here the frame may have applied a move since,
        // and then the arm goes out for nothing and its lock is let go.
        let still_there = slot0.is_some_and(|s0| {
            self_q
                .iter()
                .next()
                .and_then(|store| slot_guid(&store.0, bag, s0, &ladder.objects))
                == Some(item_guid)
        }) && ladder.objects.object(item_guid).is_some();
        let pos = wire_pos(bag, slot);
        let (Some((bag_index, wire_slot)), true) = (pos, still_there) else {
            debug!("ui_items: UseContainerItem({bag}, {slot}) no longer holds {item_guid:#x}");
            if arm_locked(arm) {
                script.unlock_item(item_guid);
            }
            return;
        };
        let fork = match arm {
            UseArm::Withdraw => {
                debug!("ui_items: withdraw (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoStoreBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
                return;
            }
            UseArm::Deposit => {
                debug!("ui_items: deposit (lua bag {bag} → wire {bag_index}/{wire_slot})");
                let _ = ladder.commands.0.send(ClientCommand::AutoBankItem {
                    bag: bag_index,
                    slot: wire_slot,
                });
                return;
            }
            // The whole stack sells (`CMSG_SELL_ITEM`, count 0).
            UseArm::Sell => {
                let Some(vendor) = merchant.vendor else {
                    debug!("ui_items: sale of {item_guid:#x} with no vendor open — let go");
                    script.unlock_item(item_guid);
                    return;
                };
                debug!("ui_items: sell lua bag {bag} slot {slot} (item {item_guid:#x})");
                let _ = ladder.commands.0.send(ClientCommand::SellItem {
                    vendor,
                    item_guid,
                    count: 0,
                });
                return;
            }
            UseArm::Item(fork) => fork,
        };
        // `None` while the template is in flight, which the call read as a plain use.
        let clicked = ladder.objects.object(item_guid).and_then(|obj| {
            let entry = obj.object_entry()?;
            // The click only reads (`0x4fa3aa`).
            let t = ladder.items.template(entry, item_guid, Ask::Read)?;
            Some(Clicked {
                guid: item_guid,
                entry,
                item_text_id: obj.item_text_id().unwrap_or(0),
                display_info_id: t.display_info_id,
                start_quest: t.start_quest,
                spell_index: t.use_spell_index().unwrap_or(0),
                use_spell: t.use_spell.map(|u| u.spell_id),
                page_text: t.page_text,
                is_charter: t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
            })
        });
        match (fork, clicked) {
            // The call put the item on the cursor, `SetCursorItem` playing its pickup (`0x494c4a`),
            // and ran `AutoEquipCursorItem`, whose clear plays the put-down (`0x49520a`). Both
            // land inside the call, so the frame's cursor-sound diff sees nothing and this path
            // plays the pair itself. A bind question leaves the item held instead, and the diff
            // plays its pickup.
            (UseFork::Equip, Some(c)) => {
                send_auto_equip(
                    script,
                    &ladder.objects,
                    &ladder.items,
                    &ladder.commands,
                    bag_index,
                    wire_slot,
                    Some(c.guid),
                );
                equip_sound.write(crate::sound::AutoEquipSound {
                    display_id: c.display_info_id,
                });
            }
            // `CMSG_OPEN_ITEM` (emitter `0x5edd60`); vmangos swaps the entry back from
            // `character_gifts`.
            (UseFork::Unwrap, Some(c)) => {
                debug!(
                    "ui_items: unwrap gift {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                    c.guid
                );
                let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                    bag_index,
                    slot: wire_slot,
                });
            }
            // The call armed the wrap; a left-click on a container slot sends `CMSG_WRAP_ITEM`.
            (UseFork::WrapPaper, _) => {}
            (UseFork::Quest, Some(c)) => {
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
                    &targeting.context().self_bound(on_self),
                    ladder,
                    script,
                    gate,
                    false,
                    ui_errors,
                );
            }
            // The reader, with no packet (`0x4e32e0`); a re-click closes it.
            (UseFork::Read, Some(c)) => {
                if item_text.read_pages(c.guid) {
                    debug!("ui_items: re-click closes the book {:#x}", c.guid);
                } else {
                    debug!(
                        "ui_items: read book {:#x} (page {}, lua bag {bag} slot {slot})",
                        c.guid, c.page_text
                    );
                }
            }
            // vmangos's `CMSG_READ_ITEM` wants a template `PageText`, so a letter's text comes by
            // item-text query.
            (UseFork::Letter, Some(c)) => {
                if item_text.read_letter(c.guid, c.item_text_id) {
                    debug!("ui_items: re-click closes the letter {:#x}", c.guid);
                } else {
                    debug!(
                        "ui_items: read letter {:#x} (text {}, lua bag {bag} slot {slot})",
                        c.guid, c.item_text_id
                    );
                }
            }
            // The server answers with a loot window on the item (`SpellHandler.cpp:227`). The send
            // tests neither the lock id nor the unlocked bit, so a locked box sends and the
            // server's `EQUIP_ERR_ITEM_LOCKED` is the "Item is locked" line. The loot latch on the
            // item's guid goes up before the send (`0x5edcc0`), as the call's lock did
            // (`0x5edcd9`); the admission gate refuses the type-1 `SMSG_LOOT_RESPONSE` without it.
            // No kneel (`0x612710`).
            (UseFork::Open, Some(c)) => {
                debug!(
                    "ui_items: open item {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                    c.guid
                );
                loot_latch.0 = Some(c.guid);
                let _ = ladder.commands.0.send(ClientCommand::OpenItem {
                    bag_index,
                    slot: wire_slot,
                });
            }
            // A plain use (food, potions, the hearthstone), or a fork whose template left the
            // cache since the call read it.
            (fork, clicked) => {
                if fork != UseFork::Plain {
                    debug!("ui_items: {fork:?} on {item_guid:#x} with no template — let go");
                    if arm_locked(arm) {
                        script.unlock_item(item_guid);
                    }
                    return;
                }
                debug!("ui_items: use item (lua bag {bag} → wire {bag_index}/{wire_slot})");
                super::send_item_use(
                    super::ItemUse {
                        guid: Some(item_guid),
                        start_quest: 0,
                        bag_index,
                        slot: wire_slot,
                        entry: clicked.map_or(0, |c| c.entry),
                        spell_index: clicked.map_or(0, |c| c.spell_index),
                        use_spell: clicked.and_then(|c| c.use_spell),
                        on_object: None,
                        is_charter: clicked.is_some_and(|c| c.is_charter),
                    },
                    &targeting.context().self_bound(on_self),
                    ladder,
                    script,
                    gate,
                    false,
                    ui_errors,
                );
            }
        }
    }
}

/// Whether the call took a lock for `arm`, which a send that never goes out lets go. A projectile's
/// equip unlocked instead; letting it go again only signals again.
fn arm_locked(arm: UseArm) -> bool {
    matches!(
        arm,
        UseArm::Withdraw
            | UseArm::Deposit
            | UseArm::Sell
            | UseArm::Item(UseFork::Equip | UseFork::Open)
    )
}

/// Sends the repair-mode clicks, bag and paper doll, as `CMSG_REPAIR_ITEM`.
pub(super) fn drain_container_repairs(
    mut hosted_vm: crate::script_calls::HostedVm,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    objects: Objects,
    commands: Res<NetCommands>,
    mut purse: crate::ui_merchant::RepairPurse,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let me = self_q.iter().next();
        // Repair-mode clicks (the reference's `0x4f9c7b` route) repair the one item: the purse test
        // first (`0x4f9cc8`), then `ITEM_REPAIR` as it goes out (`0x4f9ce4`).
        for (bag, slot) in script.take_container_repairs() {
            let (Some(vendor), Some(me)) = (merchant.vendor, me) else {
                continue;
            };
            let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
            match slot_guid(&me.0, bag, slot0, &objects) {
                Some(guid) => {
                    if purse.refuses(script, &merchant, me, [guid]) {
                        continue;
                    }
                    debug!("ui_items: repair lua bag {bag} slot {slot} (item {guid:#x})");
                    script.queue_sound_kit("ITEM_REPAIR");
                    let _ = commands.0.send(ClientCommand::RepairItem {
                        vendor,
                        item_guid: guid,
                    });
                }
                None => debug!("ui_items: repair on empty slot (bag {bag} slot {slot}) — ignored"),
            }
        }
        // The paper doll's repair clicks (`0x4c7714`, `0x4c79c4`), the same arm on a worn item.
        for id in script.take_inventory_repairs() {
            let (Some(vendor), Some(me)) = (merchant.vendor, me) else {
                continue;
            };
            let item_guid = u8::try_from(id.wrapping_sub(1))
                .ok()
                .and_then(|slot0| slot_guid(&me.0, EQUIPMENT_BAG, slot0, &objects));
            match item_guid {
                Some(guid) if (1..=19).contains(&id) => {
                    if purse.refuses(script, &merchant, me, [guid]) {
                        continue;
                    }
                    debug!("ui_items: repair worn lua slot {id} (item {guid:#x})");
                    script.queue_sound_kit("ITEM_REPAIR");
                    let _ = commands.0.send(ClientCommand::RepairItem {
                        vendor,
                        item_guid: guid,
                    });
                }
                _ => debug!(
                    "ui_items: repair on worn lua slot {id}, empty or out of range — ignored"
                ),
            }
        }
    })
}

/// The clicked item's send data, read at the drain.
#[derive(Clone, Copy)]
struct Clicked {
    guid: u64,
    entry: u32,
    /// Instance `ITEM_FIELD_ITEM_TEXT_ID`, nonzero on a letter made from mail.
    item_text_id: u32,
    display_info_id: u32,
    start_quest: u32,
    spell_index: u8,
    /// The template's on-use spell, the key of the cast tail's in-flight guard.
    use_spell: Option<u32>,
    /// The template's `PageText`, nonzero on a book.
    page_text: u32,
    is_charter: bool,
}

/// The arm a right-click on this item takes past the bank and vendor forks, for the feed to push
/// with the slot: `UseContainerItem`'s equip fork (`0x4fa3b9`), an equippable template whose
/// `StartQuest` (`[rec+0x1a8]`) is 0 (`0x4fa3bd`), else the use dispatcher `0x5d8d00`'s arms in
/// its order: a wrapped gift unwraps (`0x5d8d92`), wrapping paper arms (`0x5d8d9d`), a
/// quest-starter offers its quest (`0x5d8dd2`), a `PageText` book reads (`0x5d8e4c`) ahead of the
/// open arm, the inverse of the tooltip, a mail letter reads, and a lootable template opens
/// (`0x5d8f7c`).
pub(crate) fn use_fork(
    t: &benilla_protocol::ItemInfo,
    inst_flags: u32,
    item_text_id: u32,
) -> UseFork {
    if t.inventory_type != 0 && t.start_quest == 0 {
        UseFork::Equip
    } else {
        dispatch_fork(t, inst_flags, item_text_id)
    }
}

/// [`use_fork`] past the equip fork: the dispatcher's own arms, which `UseInventoryItem` reaches
/// directly (`0x4c7af0`).
pub(crate) fn dispatch_fork(
    t: &benilla_protocol::ItemInfo,
    inst_flags: u32,
    item_text_id: u32,
) -> UseFork {
    if t.unwraps_gift(inst_flags) {
        UseFork::Unwrap
    } else if t.begins_gift_wrap(inst_flags) {
        UseFork::WrapPaper
    } else if t.start_quest != 0 {
        UseFork::Quest
    } else if t.page_text != 0 {
        UseFork::Read
    } else if item_text_id != 0 {
        UseFork::Letter
    } else if t.opens_loot() {
        UseFork::Open
    } else {
        UseFork::Plain
    }
}

/// Sends the gift wraps and the moves `PickupContainerItem`/`SplitContainerItem` queued, the moves
/// through [`send_container_move`].
pub(super) fn drain_container_moves(
    mut vm: crate::script_calls::VmWith<crate::ui_bind_confirm::BindGate<'static>>,
    commands: Res<NetCommands>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
) {
    vm.turn(|script, mut gate| {
        let Some(script) = script else {
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
                script, &mut gate, &objects, &items, &commands, store, mv, false,
            );
        }
    })
}

/// The equip deferral's positions (`0x5e0c40`): slots 0..=22 (equipment and equipped bags) and
/// 63..=68 (bank bags) of the player's own array; the backpack and keyring are not.
fn is_equip_position(bag_index: u8, slot: u8) -> bool {
    bag_index == BAG_PLAYER_INVENTORY && (slot <= 22 || (63..=68).contains(&slot))
}

/// One container move: the soulbind deferral, then the send; the cursor verb that queued it locked
/// what it touched. A whole stack
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
                if gate.equip_binds(script, objects, items, guid) {
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
    }
    true
}

/// Sends the destroys `DeleteCursorItem` queued as `CMSG_DESTROYITEM` (count 0 is the whole stack
/// on both sides); the verb relocked the item (`0x4898bc`).
pub(super) fn drain_container_destroys(
    mut hosted_vm: crate::script_calls::HostedVm,
    commands: Res<NetCommands>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
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
        }
    })
}

/// The bag clicks of a harness's VM ([`UiScript::keep_calls`]): the repair-mode drain, then the
/// kept `UseContainerItem` calls through their applier, as the app's host applies each inside its
/// verb.
#[cfg(test)]
fn run_container_clicks(world: &mut World) {
    use benilla_ui::script::ScriptCall;
    use bevy::ecs::system::RunSystemOnce;
    world.run_system_once(drain_container_repairs).unwrap();
    world
        .run_system_once(
            |mut script: NonSendMut<UiScript>, mut items: ScriptItemUse| {
                for call in script.take_script_calls() {
                    if let ScriptCall::UseContainerItem(call) = call {
                        items.use_container_item(&mut script, call);
                    }
                }
            },
        )
        .unwrap();
}

/// A VM with no engine behind it, which keeps its calls for the harness to apply.
#[cfg(test)]
fn harness_vm() -> UiScript {
    let mut script = UiScript::new().unwrap();
    script.keep_calls();
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{ItemInfo, ObjectFields, ITEM_FLAG_LOOTABLE};

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
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
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

        app.insert_non_send_resource(harness_vm());
        feed_clam(&mut app);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        (app, rx)
    }

    /// Push backpack slot 1 as the container feed does: `UseContainerItem` reads the slot's item,
    /// its lock (`0x4fa1c7`, `0x4fa1cd`) and its fork, here from the clam's template as it stands.
    fn feed_clam(app: &mut App) {
        let t = app
            .world()
            .resource::<Items>()
            .template(CLAM_ENTRY, CLAM, Ask::Read)
            .cloned()
            .expect("the clam's template");
        let slot = benilla_ui::script::ContainerSlot {
            item_id: CLAM_ENTRY,
            count: 1,
            guid: CLAM,
            inventory_type: t.inventory_type,
            item_class: Some(t.class),
            use_fork: use_fork(&t, 0, 0),
            ..Default::default()
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    num_slots: 16,
                    slots: std::collections::HashMap::from([(1, slot)]),
                    ..Default::default()
                }),
            );
    }

    /// Open a vendor, or close it with `None`, as the app holds it and as the merchant feed
    /// mirrors it to the VM.
    fn set_vendor(app: &mut App, vendor: Option<u64>) {
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = vendor;
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_merchant(vendor.map(|_| benilla_ui::script::MerchantState::default()));
    }

    /// Open a banker, as the app holds it and as the bank feed mirrors it to the VM.
    fn open_banker(app: &mut App, banker: u64) {
        app.world_mut()
            .resource_mut::<crate::ui_bank::BankOpen>()
            .open(banker);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_bank(Some(benilla_ui::script::BankState::default()));
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
        // The grey lock, the reference's other pre-send write (`0x5edcd9`).
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(CLAM),
            "the slot greys at the click"
        );
    }

    /// A repair-mode right-click on a bag item: `ITEM_REPAIR` as it goes out (`0x4f9ce4`), then
    /// `CMSG_REPAIR_ITEM` with the vendor's and the item's guid, and no use or sale.
    #[test]
    fn a_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut bag = benilla_ui::script::ContainerState {
                num_slots: 16,
                ..Default::default()
            };
            bag.slots.insert(
                1,
                benilla_ui::script::ContainerSlot {
                    item_id: CLAM_ENTRY,
                    count: 1,
                    ..Default::default()
                },
            );
            script.set_container(0, Some(bag));
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() UseContainerItem(0, 1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: CLAM
            })
        ));
        assert!(rx.try_recv().is_err(), "the repair alone: no use or sale");
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// A slot whose item object has not streamed ends the call before any arm (`0x4fa1c7`): no
    /// sale goes out and nothing locks, as `LockItem` on an unresolved guid signals nothing.
    #[test]
    fn a_use_on_an_unstreamed_item_does_nothing() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const GHOST: u64 = 0x4000_0000_0000_0d0d;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, GHOST as u32),
                (F_PACK_SLOT_1 + 1, (GHOST >> 32) as u32),
            ])));
        set_vendor(&mut app, Some(VENDOR));
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.unlock_item(CLAM);
            s.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());
        assert!(rx.try_recv().is_err(), "nothing sold");
        assert!(!app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(GHOST));
    }

    /// Two uses in one frame: the first's sale locks the item in its call (`0x4fa37f`), so the
    /// second call stops at the lock (`0x4fa1cd`).
    #[test]
    fn two_sales_queued_in_one_frame_send_once() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, Some(VENDOR));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1) UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        assert_eq!(
            rx.try_iter().count(),
            1,
            "the second use sees the first sale's lock (`0x4fa37f`, `0x4fa1cd`)"
        );
    }

    /// A use whose arm takes no lock leaves the item free for a later call: the reference sends
    /// the use inside `UseContainerItem`, so a pickup after it in the frame picks up and the use
    /// still goes out.
    #[test]
    fn a_use_then_a_pickup_in_one_frame_sends_the_use() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut().resource_mut::<Items>().insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Clam")
            }),
        );
        feed_clam(&mut app);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.unlock_item(CLAM);
            script
                .run("UseContainerItem(0, 1) PickupContainerItem(0, 1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());
        assert!(rx.try_recv().is_ok(), "the use went out");
        let script = app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_some() && script.item_locked(CLAM),
            "and the pickup held"
        );
    }

    /// The open arm locks the item inside the call (`0x5edcd9`), so a second right-click in the
    /// same frame stops at the lock (`0x4fa1cd`) and one `CMSG_OPEN_ITEM` goes out.
    #[test]
    fn two_opens_in_one_frame_send_once() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.unlock_item(CLAM);
            script.run("UseContainerItem(0, 1)").unwrap();
            assert!(
                script.item_locked(CLAM),
                "locked in the call, before any send"
            );
            script.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());
        assert_eq!(rx.try_iter().count(), 1);
    }

    /// `UseContainerItem` tests the banker (`[0xbdd038]`, `0x4fa322`) before the vendor
    /// (`[0xbddfa0]`, `0x4fa35b`), so with both open the click deposits.
    #[test]
    fn a_banker_takes_the_click_before_a_vendor() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, Some(0xF130_0000_0000_0042));
        open_banker(&mut app, 0xF130_0000_0000_0043);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("UseContainerItem(0, 1)")
            .unwrap();
        run_container_clicks(app.world_mut());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::AutoBankItem { .. })
        ));
        assert!(rx.try_recv().is_err(), "no sale");
    }

    /// An item picked up before its template landed reads as no projectile in the pickup's
    /// clear, which keeps its lock; when `AutoEquipCursorItem`'s send finds it one, the arm's
    /// `ClearCursor(1,1)` lets it go (`0x5e1711`, `0x5e1763`), and `CMSG_SET_AMMO` goes out by
    /// the class (`0x5e16ff`), not the inventory type.
    #[test]
    fn a_projectile_picked_up_unresolved_loads_by_class_and_unlocks() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.unlock_item(CLAM);
            // The slot as the feed pushes it with the template in flight: no class.
            let slot = benilla_ui::script::ContainerSlot {
                item_id: CLAM_ENTRY,
                count: 1,
                guid: CLAM,
                ..Default::default()
            };
            script.set_container(
                0,
                Some(benilla_ui::script::ContainerState {
                    num_slots: 16,
                    slots: std::collections::HashMap::from([(1, slot)]),
                    ..Default::default()
                }),
            );
            script
                .run("PickupContainerItem(0, 1) AutoEquipCursorItem()")
                .unwrap();
            assert!(script.item_locked(CLAM), "the pickup's clear kept the lock");
        }
        app.world_mut().resource_mut::<Items>().insert_template(
            CLAM_ENTRY,
            Some(ItemInfo {
                class: ITEM_CLASS_PROJECTILE,
                ..crate::items::test_template("Odd Arrow")
            }),
        );
        app.world_mut()
            .run_system_once(drain_container_autoequips)
            .unwrap();
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SetAmmo { entry: CLAM_ENTRY })
        ));
        assert!(!app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(CLAM));
    }

    /// The sale and bank arms lock the clicked item after their send (`0x4fa37f`; `0x4fa312`,
    /// `0x4fa34b`), and a locked item's click stops before any arm (`0x4fa1cd`).
    #[test]
    fn the_sale_and_bank_arms_lock_the_item_and_a_locked_click_stops() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let click = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("UseContainerItem(0, 1)")
                .unwrap();
            run_container_clicks(app.world_mut());
        };
        let locked = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(CLAM)
        };
        assert!(locked(&app), "the open click's own lock");
        set_vendor(&mut app, Some(VENDOR));
        click(&mut app);
        assert!(rx.try_recv().is_err(), "a locked item sells nothing");

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        click(&mut app);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::SellItem {
                item_guid: CLAM,
                ..
            })
        ));
        assert!(
            locked(&app),
            "the sale locks it until SMSG_SELL_ITEM or the slot moves"
        );

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(CLAM);
        set_vendor(&mut app, None);
        open_banker(&mut app, 0xF130_0000_0000_0043);
        click(&mut app);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::AutoBankItem { .. })
        ));
        assert!(locked(&app), "the deposit locks it");
    }

    /// The equip arm locks the item after the equip (`0x4fa418`), but not a projectile, which
    /// loads by entry and stays in its bag (`0x4fa40b`): its arm's `ClearCursor(1,1)` unlocks it
    /// instead, signalling as the lock does (`0x5e1763`).
    #[test]
    fn the_equip_arm_locks_all_but_a_projectile() {
        for (class, inventory_type, want) in [(4u32, 1u32, true), (6, 24, false)] {
            let (mut app, rx) = open_the_clam();
            while rx.try_recv().is_ok() {} // drain the clam's own send
            app.world_mut().resource_mut::<Items>().insert_template(
                CLAM_ENTRY,
                Some(ItemInfo {
                    class,
                    inventory_type,
                    ..crate::items::test_template("Worn Thing")
                }),
            );
            feed_clam(&mut app);
            {
                let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
                script.unlock_item(CLAM);
                script
                    .run(
                        "SIGNALS = 0 local f = CreateFrame('Frame') \
                         f:RegisterEvent('ITEM_LOCK_CHANGED') \
                         f:SetScript('OnEvent', function() SIGNALS = SIGNALS + 1 end)",
                    )
                    .unwrap();
                script.run("UseContainerItem(0, 1)").unwrap();
                // The lock, or a projectile's unlock, signals in the call.
                assert_eq!(
                    script.eval::<i64>("return SIGNALS").unwrap(),
                    1,
                    "class {class}"
                );
            }
            run_container_clicks(app.world_mut());
            assert!(rx.try_recv().is_ok(), "class {class}: the equip went out");
            assert_eq!(
                app.world()
                    .non_send_resource::<UiScript>()
                    .item_locked(CLAM),
                want,
                "class {class}"
            );
        }
    }

    /// `CGItem::Use` refuses a locked item first (`0x5d8d32`): a worn item's use from the doll
    /// (`0x4c7af0`) sends nothing while it is locked, which its call reads.
    #[test]
    fn a_locked_worn_item_is_not_used() {
        const HELM: u64 = 0x4000_0000_0000_0777;
        const HELM_ENTRY: u32 = 7777;
        let head = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (head, HELM as u32),
                (head + 1, (HELM >> 32) as u32),
            ])));
        crate::items::test_spawn_item(
            app.world_mut(),
            HELM,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, HELM_ENTRY)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            HELM_ENTRY,
            Some(ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Helm")
            }),
        );
        // The doll as the inventory feed pushes it: the call reads the worn item's lock.
        let mut worn: benilla_ui::script::InventorySlots = Default::default();
        worn[1] = Some(benilla_ui::script::InvSlotView {
            item_id: HELM_ENTRY,
            guid: HELM,
            ..Default::default()
        });
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_inventory_slots(worn);
        let use_head = |app: &mut App| {
            use bevy::ecs::system::RunSystemOnce;
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("UseInventoryItem(1)")
                .unwrap();
            app.world_mut()
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut items: ScriptItemUse| {
                        for call in script.take_script_calls() {
                            if let benilla_ui::script::ScriptCall::UseInventoryItem {
                                slot: id,
                                on_self,
                                ..
                            } = call
                            {
                                items.use_inventory_item(&mut script, id, on_self);
                            }
                        }
                    },
                )
                .unwrap();
        };
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(HELM);
        use_head(&mut app);
        assert!(rx.try_recv().is_err(), "a locked item is not used");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(HELM);
        use_head(&mut app);
        assert!(rx.try_recv().is_ok(), "unlocked, the use goes out");
    }

    /// A repair-mode click on a worn item (`0x4c7714`): `ITEM_REPAIR` as it goes out (`0x4c7780`),
    /// then `CMSG_REPAIR_ITEM` with the vendor's and the worn item's guid.
    #[test]
    fn a_worn_repair_click_sounds_and_ships_the_items_guid() {
        const VENDOR: u64 = 0xF130_0000_0000_0042;
        const HELM: u64 = 0x4000_0000_0000_0777;
        let head = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        app.world_mut()
            .resource_mut::<crate::ui_merchant::MerchantOpen>()
            .vendor = Some(VENDOR);
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::from_pairs(&[
                (head, HELM as u32),
                (head + 1, (HELM >> 32) as u32),
            ])));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            let mut slots: benilla_ui::script::InventorySlots = Default::default();
            slots[1] = Some(benilla_ui::script::InvSlotView {
                item_id: 7,
                ..Default::default()
            });
            script.set_inventory_slots(slots);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            script
                .run("ShowRepairCursor() PickupInventoryItem(1)")
                .unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::RepairItem {
                vendor: VENDOR,
                item_guid: HELM
            })
        ));
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(
            script.take_sounds(),
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
    }

    /// What one repair click did: the packets, the VM's sounds, the `UI_ERROR_MESSAGE` lines and
    /// the message rows queued for their sound.
    struct RepairClick {
        sent: Vec<ClientCommand>,
        sounds: Vec<benilla_ui::script::SoundRequest>,
        shown: Vec<String>,
        sounded: Vec<&'static str>,
    }

    /// A repair-mode click on [`crate::ui_merchant::purse_fixture`]'s sword with `money` in the
    /// purse: in backpack slot 1 through `UseContainerItem`, or `worn` in the main hand (slot 15)
    /// through `PickupInventoryItem`.
    fn click_to_repair_the_sword(money: u32, worn: bool) -> RepairClick {
        use crate::ui_merchant::purse_fixture::{player, record_errors, seat, SWORD_ENTRY};
        use benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
        seat(app.world_mut());
        let slot_field = if worn {
            FIELD_PLAYER_INV_SLOT_HEAD + 2 * 15
        } else {
            F_PACK_SLOT_1
        };
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(me)
            .insert((player(slot_field, money), crate::net::Guid(0x5e1f)));
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.take_sounds();
            record_errors(&script);
            script.set_merchant(Some(benilla_ui::script::MerchantState {
                can_repair: true,
                ..Default::default()
            }));
            let click = if worn {
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[16] = Some(benilla_ui::script::InvSlotView {
                    item_id: SWORD_ENTRY,
                    ..Default::default()
                });
                script.set_inventory_slots(slots);
                "ShowRepairCursor() PickupInventoryItem(16)"
            } else {
                let mut bag = benilla_ui::script::ContainerState {
                    num_slots: 16,
                    ..Default::default()
                };
                bag.slots.insert(
                    1,
                    benilla_ui::script::ContainerSlot {
                        item_id: SWORD_ENTRY,
                        count: 1,
                        ..Default::default()
                    },
                );
                script.set_container(0, Some(bag));
                "ShowRepairCursor() UseContainerItem(0, 1)"
            };
            script.run(click).unwrap();
        }
        run_container_clicks(app.world_mut());
        let sounded = crate::ui_merchant::purse_fixture::sounded(app.world());
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        RepairClick {
            sent: rx.try_iter().collect(),
            sounds: script.take_sounds(),
            shown: crate::ui_merchant::purse_fixture::shown(&script),
            sounded,
        }
    }

    /// The repair arm's purse test, unsigned `cost > money` on the discounted cost: one copper
    /// short shows `ERR_NOT_ENOUGH_MONEY` with its speech row, plays no `ITEM_REPAIR` and sends
    /// nothing; exactly the cost, under the full price, sounds and sends.
    fn the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(worn: bool) {
        use crate::ui_merchant::purse_fixture::{sword_prices, NOT_ENOUGH_MONEY, SWORD, VENDOR};
        let (full, discounted) = sword_prices();

        let short = click_to_repair_the_sword(discounted - 1, worn);
        assert!(
            short.sent.is_empty(),
            "short of {discounted}: nothing goes out"
        );
        assert!(short.sounds.is_empty(), "…no ITEM_REPAIR");
        assert_eq!(short.shown, [NOT_ENOUGH_MONEY], "…the red line");
        assert_eq!(
            short.sounded,
            ["ERR_NOT_ENOUGH_MONEY"],
            "…and its row, which speaks line 0x28"
        );

        let exact = click_to_repair_the_sword(discounted, worn);
        assert!(
            matches!(
                exact.sent.as_slice(),
                [ClientCommand::RepairItem {
                    vendor: VENDOR,
                    item_guid: SWORD
                }]
            ),
            "{discounted} in the purse pays the discounted {discounted}, short of the full {full}"
        );
        assert_eq!(
            exact.sounds,
            vec![benilla_ui::script::SoundRequest::KitName(
                "ITEM_REPAIR".into()
            )]
        );
        assert!(exact.shown.is_empty() && exact.sounded.is_empty());
    }

    /// `UseContainerItem`'s repair arm (`0x4fa22e`–`0x4fa236`, error `0x4fa23a`).
    #[test]
    fn a_bag_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(false);
    }

    /// `PickupInventoryItem`'s repair arm (`0x4c775f`–`0x4c7766`, error `0x4c776a`).
    #[test]
    fn a_worn_repair_the_purse_cannot_cover_is_refused_and_one_it_covers_is_sent() {
        the_purse_test_refuses_one_copper_short_and_pays_the_exact_cost(true);
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
        feed_clam(&mut app);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            // The open click's lock, which a locked item's click would stop at (`0x4fa1cd`).
            script.unlock_item(CLAM);
            script.run("UseContainerItem(0, 1)").unwrap();
        }
        run_container_clicks(app.world_mut());

        assert!(
            rx.try_recv().is_err(),
            "the begin-wrap arm is purely local — nothing goes on the wire until a container \
             click spends it"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.gift_wrap_armed(),
            Some(benilla_ui::script::PendingWrap {
                guid: CLAM,
                bag: 0,
                slot: 1
            }),
            "…and the paper is armed"
        );
    }
}

/// The answers to `EquipPendingItem`/`CancelPendingEquip`. Accept re-runs the original sender with
/// `suppress` set, as the reference's `0x5e1be0` does: 1.12 has no confirm opcode, and a slot that
/// changed under the dialog is judged afresh. Cancel sends nothing, unlocks the items the click
/// locked and frees the record: a swap's two ends as they sit now (`0x5e1ca5`, `0x5e1d04`), an
/// auto-equip's one item, its record naming no second container (`0x5e1cd1`).
pub(super) fn drain_bind_confirm_answers(
    mut vm: crate::script_calls::VmWith<(
        crate::ui_bind_confirm::BindGate<'static>,
        MessageWriter<'static, crate::sound::AutoEquipSound>,
    )>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
) {
    vm.turn(|script, (mut gate, mut equip_sound)| {
        let Some(script) = script else {
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
                let at = |bag: i64, slot: u32| {
                    let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
                    store.and_then(|s| slot_guid(&s.0, bag, slot0, &objects))
                };
                let locked = match rec {
                    crate::ui_bind_confirm::PendingEquip::Swap {
                        src_bag,
                        src_slot,
                        dst_bag,
                        dst_slot,
                    } => [at(src_bag, src_slot), at(dst_bag, dst_slot)],
                    crate::ui_bind_confirm::PendingEquip::AutoEquip { guid, .. } => {
                        [Some(guid), None]
                    }
                };
                for guid in locked.into_iter().flatten() {
                    script.unlock_item(guid);
                }
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
                        script,
                        &mut gate,
                        &objects,
                        &items,
                        &commands,
                        store,
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
                // The accept re-issues the pick-up and auto-equip from the slot as it stands
                // (`0x5e1c23` into `0x5e1840`, `suppress` set): `SetCursorItem`'s clear drops and
                // unlocks the held item, then the equip goes out. A slot emptied under the dialog
                // sends nothing (`0x5e18da`).
                crate::ui_bind_confirm::PendingEquip::AutoEquip { bag, slot, .. } => {
                    if let benilla_ui::script::AutoEquip::Sent { bag, slot, guid } =
                        script.auto_equip_slot(bag, slot, true)
                    {
                        if let Some((bag_index, wire_slot)) = wire_pos(bag, slot) {
                            send_auto_equip(
                                script,
                                &objects,
                                &items,
                                &commands,
                                bag_index,
                                wire_slot,
                                Some(guid),
                            );
                            // The re-issue's `SetCursorItem` and send clear play their pair inside
                            // the answer (`0x494c4a`, `0x49520a`), after the held item's own clear.
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
            }
        }
    })
}

/// `ConfirmBindOnUse()`: re-issues the pending use through the cast ladder with `suppress` set.
/// The pending state is one cell, taken on the first accept, so a doubled accept does nothing.
pub(super) fn drain_bind_on_use_confirms(
    mut vm: crate::script_calls::VmWith<(
        crate::spell::cast_target::CastTargeting<'static, 'static>,
        crate::spell::CastLadder<'static, 'static>,
        crate::ui_bind_confirm::BindGate<'static>,
    )>,
    ui_errors: Res<crate::ui_action::UiErrorKeys>,
) {
    vm.turn(|script, (targeting, mut ladder, mut gate)| {
        let Some(script) = script else {
            return;
        };
        // The first accept spends the pending use, whatever the protected-action gate said; the
        // cast inside it asks tag 2 (`0x5d92c1` → `0x6e4bc1`).
        let Some(may_cast) = script.take_bind_on_use_casts().first().copied() else {
            return;
        };
        let Some(it) = gate.on_use.0.take() else {
            return;
        };
        if super::item_use_refused(script, it.guid) {
            debug!("ui_items: bind-on-use confirmed on a locked item — refused");
            return;
        }
        debug!(
            "ui_items: bind-on-use confirmed for wire {}/{}",
            it.bag_index, it.slot
        );
        ladder.permit(may_cast);
        super::send_item_use(
            it,
            &targeting.context(),
            &mut ladder,
            script,
            &mut gate,
            true,
            &ui_errors,
        );
        ladder.permit(true);
    })
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
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\Fonts.xml",
            "Interface\\FrameXML\\BasicControls.xml",
            "Interface\\FrameXML\\LocaleProperties.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
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
                use_fork: Default::default(),
                item_class: None,
                duration_ms: None,
                petition: None,
                already_bound: false,
                occupied_seats: None,
                bonding: 2,
                use_spell: 0,
                bar_placeable: true,
                consumable: false,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Axe_01".into()),
                count: 1,
                quality: Some(quality),
                inventory_type: 0,
                item_id: FLURRY_AXE,
                link: Some("|cffa335ee|Hitem:871:0:0:0|h[Flurry Axe]|h|r".into()),
                guid: AXE_GUID,
                equip_slots: vec![16], // MainHandSlot
                cooldown: None,
                readable: false,
                creator: None,
                gift_creator: None,
                creator_guid: 0,
                gift_creator_guid: 0,
                flags: 0,
                enchants: Vec::new(),
                spell_0: false,
                spell_charges: [0; 5],
            },
        );
        ContainerState {
            name: None,
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
        app.add_message::<crate::sound::AutoEquipSound>()
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

        let mut script = harness_vm();
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
    /// already free. The cancel unlocks what the place locked (`0x5e1ca5`, `0x5e1d04`): the axe,
    /// and the empty main hand's nothing.
    #[test]
    fn cancelling_sends_nothing_unlocks_and_the_doubled_cancel_is_harmless() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_on_the_doll();
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .item_locked(AXE_GUID),
            "the place's send keeps the axe locked under the question"
        );
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"EQUIP_BIND\"), 2)")
            .unwrap();
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            let answers = s.take_pending_equip_answers();
            assert_eq!(answers.len(), 2, "OnCancel and OnHide both fire");
            // Back in the queue for the drain, as the popup filed them.
            for a in answers {
                s.run(&format!("CancelPendingEquip({})", a.index)).unwrap();
            }
        }
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();
        assert!(rx.try_iter().next().is_none(), "a cancel never sends");
        assert!(!app
            .world()
            .non_send_resource::<UiScript>()
            .item_locked(AXE_GUID));
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
        app.add_message::<crate::sound::AutoEquipSound>()
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
        let mut script = harness_vm();
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
        {
            let script = app.world().non_send_resource::<UiScript>();
            assert_eq!(
                script.cursor_item().map(|i| i.guid),
                Some(AXE_GUID),
                "the axe stays on the cursor under the question (`0x5e16ce`)"
            );
            assert!(script.item_locked(AXE_GUID), "and locked");
        }
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
        let script = app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_none(),
            "the accept's pick-up and send leave the cursor empty"
        );
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
                .drain()
                .count(),
            1,
            "and the re-issue's pickup and put-down play"
        );
    }

    /// The cancel only unlocks (`0x5e1c50`): the axe stays on the cursor, free, nothing is sent.
    #[test]
    fn cancelling_a_boe_auto_equip_leaves_the_item_held_and_unlocked() {
        benilla_formats::wow_data_or_skip!();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
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
        app.world_mut().resource_mut::<Items>().insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(4)));
        script.run("PickupContainerItem(0, 1)").unwrap();
        script.run("AutoEquipCursorItem()").unwrap();
        app.insert_non_send_resource(script);
        app.world_mut()
            .run_system_once(drain_container_autoequips)
            .unwrap();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AUTOEQUIP_BIND\"), 2)")
            .unwrap();
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            for a in s.take_pending_equip_answers() {
                s.run(&format!("CancelPendingEquip({})", a.index)).unwrap();
            }
        }
        app.world_mut()
            .run_system_once(drain_bind_confirm_answers)
            .unwrap();

        assert!(rx.try_iter().next().is_none(), "a cancel sends nothing");
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.cursor_item().map(|i| i.guid),
            Some(AXE_GUID),
            "the axe is still on the cursor"
        );
        assert!(!script.item_locked(AXE_GUID), "and free");
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
        // Re-run the place now that the item is out of reach. The fixture's own place asked
        // first, unjudged; the re-run's pickup retires that question with its `CURSOR_UPDATE`,
        // which fires inside the verb.
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.tick(0.01);
            // The first place left the axe locked for its answer, the swap send's
            // `ClearCursor(0,1)` keeping the pickup's lock (`0x5e0fe7`); the question's cancel
            // would lift it (`0x5e1ca5`).
            s.unlock_item(AXE_GUID);
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

    /// An app with what the bag's use arm reads, the cast ladder's included.
    fn use_arm_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<crate::spell::ScriptCastPermit>()
            .add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
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
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        (app, rx)
    }

    /// `UseContainerItem(bag, slot, onSelf)` (`0x4fa17d`): a set flag makes the player's guid the
    /// use's target (`0x4fa186`, `0x468550`) in place of the selection `[0xb4e2d8]`.
    #[test]
    fn use_container_item_on_self_targets_the_player() {
        const ME: u64 = 0x0000_0000_0000_0007;
        const FRIEND: u64 = 0x0000_0000_0000_0009;
        let target = |chunk: &str| {
            let (mut app, rx) = use_arm_app();
            app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
            app.world_mut()
                .resource_mut::<crate::target::Selection>()
                .guid = Some(FRIEND);
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
            let mut template = crate::items::test_template("A Bandage");
            template.inventory_type = 0;
            template.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(FLURRY_AXE, Some(template));
            let mut script = harness_vm();
            script.set_container(0, Some(bag_with_the_axe(1)));
            script.run(chunk).unwrap();
            app.insert_non_send_resource(script);
            run_container_clicks(app.world_mut());
            rx.try_iter()
                .find_map(|c| match c {
                    ClientCommand::UseItem { target, .. } => Some(target),
                    _ => None,
                })
                .expect(chunk)
        };
        use benilla_protocol::messages::UseItemTarget;
        assert_eq!(
            target("UseContainerItem(0, 1)"),
            UseItemTarget::Unit(FRIEND)
        );
        assert_eq!(
            target("UseContainerItem(0, 1, nil)"),
            UseItemTarget::Unit(FRIEND)
        );
        assert_eq!(target("UseContainerItem(0, 1, 1)"), UseItemTarget::Unit(ME));
    }

    /// Right-clicks a bind-on-use item in a bag (the use arm, `0x5d8d00`, event 290);
    /// `no_use_spell` drops its on-use spell, since four of `0x5d91d3`'s five predecessors are
    /// failed spell lookups and the reference still asks.
    fn right_click_a_bind_on_use_item(
        no_use_spell: bool,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut app, rx) = use_arm_app();
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

        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        script.set_container(0, Some(bag_with_the_axe(3)));
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        run_container_clicks(app.world_mut());
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

    /// The question the call raised is filed whatever the slot holds by the time the drain runs:
    /// the reference asked inside the call (`0x5e16c6`), the item is on the cursor, and the answer
    /// re-judges the slot. A miss here used to unlock the item and leave it held with no record.
    #[test]
    fn an_asked_equip_is_filed_though_the_slot_moved_on() {
        benilla_formats::wow_data_or_skip!();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::AutoEquipSound>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .init_resource::<crate::ui_merchant::MerchantOpen>()
            .init_resource::<crate::ui_bank::BankOpen>()
            .init_resource::<crate::ui_item_text::ItemTextOpen>()
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
            .init_resource::<crate::ui_cast::CastReports>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::spell::AutoRepeatActive>()
            .init_resource::<crate::creature_anim::UnitEdits>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<crate::ui_tradeskill::TradeSkillOpens>()
            .init_resource::<crate::spell::targeting::SpellTargeting>()
            .init_resource::<crate::spell::HeldForPick>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx));
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                ObjectStore(ObjectFields::from_pairs(&[
                    (F_PACK_SLOT_1, AXE_GUID as u32),
                    (F_PACK_SLOT_1 + 1, (AXE_GUID >> 32) as u32),
                ])),
            ))
            .id();
        crate::items::test_spawn_item(
            app.world_mut(),
            AXE_GUID,
            ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, FLURRY_AXE)]),
            false,
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            FLURRY_AXE,
            Some(ItemInfo {
                quality: 4,
                bonding: 2,
                inventory_type: 13,
                ..crate::items::test_template("Flurry Axe")
            }),
        );
        let mut script = harness_vm();
        script.set_screen_size(1024.0, 768.0);
        load_ui(&script);
        let mut bag = bag_with_the_axe(4);
        bag.slots.get_mut(&1).unwrap().use_fork = UseFork::Equip;
        bag.slots.get_mut(&1).unwrap().item_class = Some(2);
        script.set_container(0, Some(bag));
        script.run("UseContainerItem(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        // The frame applied a move before the drain: the slot is empty now.
        app.world_mut()
            .entity_mut(me)
            .insert(ObjectStore(ObjectFields::default()));
        run_container_clicks(app.world_mut());

        assert_eq!(
            app.world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "the question is filed"
        );
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(AXE_GUID));
        assert!(script.item_locked(AXE_GUID), "still locked, still held");
    }

    /// `ConfirmBindOnUse` spends its pending use whatever the protected-action gate says
    /// (`0x5d92c6`-`0x5d92cc` after `CGItem::Use`), but the cast inside asks tag 2: a macro's
    /// accept outside an input event sends nothing, and the pending use is gone for the next.
    #[test]
    fn a_refused_bind_on_use_confirm_spends_the_pending_use_and_sends_nothing() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .tick(0.01);
        assert!(shown(&mut app, "USE_BIND"));
        let _ = rx.try_iter().count();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"RunScript("ConfirmBindOnUse()")"#)
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_on_use_confirms)
            .unwrap();
        let sent: Vec<String> = rx.try_iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(sent, Vec::<String>::new(), "the cast is refused");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("ConfirmBindOnUse()")
            .unwrap();
        app.world_mut()
            .run_system_once(drain_bind_on_use_confirms)
            .unwrap();
        assert_eq!(rx.try_iter().count(), 0, "the pending use was spent");
    }

    /// The confirm's re-issue goes through `CGItem::Use` (`0x5d92c1`), which refuses an item
    /// locked meanwhile (`0x5d8d32`).
    #[test]
    fn a_bind_on_use_confirm_skips_an_item_locked_meanwhile() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = right_click_a_bind_on_use_item(false);
        {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.tick(0.01);
            s.lock_item(AXE_GUID);
            s.run("StaticPopup_OnClick(StaticPopup_FindVisible(\"USE_BIND\"), 1)")
                .unwrap();
        }
        app.world_mut()
            .run_system_once(drain_bind_on_use_confirms)
            .unwrap();
        assert_eq!(rx.try_iter().count(), 0, "a locked item is not used");
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
