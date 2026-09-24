//! The outward half of the container seam (see the parent module doc): the per-frame drains that
//! turn queued Lua intents (`UseContainerItem`, the cursor pick/place/swap/split moves, the
//! delete-confirm popup's destroy) into `ClientCommand`s on the wire, locking the slots each send
//! touches ([`crate::pending_item_ops::PendingItemOps`]) along the way.

use bevy::prelude::*;

use benilla_protocol::messages::BAG_PLAYER_INVENTORY;
use benilla_ui::script::{UiScript, EQUIPMENT_BAG};

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::pending_item_ops::PendingItemOps;

use super::{slot_guid, slot_guid_count, wire_pos, INVTYPE_AMMO};

/// **The one auto-equip sender** — the reference has exactly one (`0x5e1480`
/// `AutoEquipCursorItem`), reached from several verbs, and until decision 1750 benilla had three
/// copies of it. Three copies is what made the soulbind confirm impossible to add correctly: the
/// gate would have had to be written, and kept agreeing, in three places.
///
/// Takes the WIRE position and the item's guid, and answers whether the send happened. The two
/// forks it owns:
///
/// - **ammo**: an ammo-class item loads by entry with `CMSG_SET_AMMO`
///   rather than the equip wire — the stack stays in the bag and `PLAYER_AMMO_ID` references it
///   (decision 0526). A missing template falls back to `CMSG_AUTOEQUIP_ITEM`, whose refusal is at
///   least visible.
/// - **the soulbind deferral** (decision 1750, `0x5e163b`): a not-yet-bound, equippable
///   `bonding == 2` item raises `AUTOEQUIP_BIND_CONFIRM` and sends NOTHING. `suppress` is the
///   reference's own parameter, set on the re-issue `EquipPendingItem` drives — which is what stops
///   the accept from asking the same question again forever.
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

/// Drain the `(bag, slot)` sources `AutoEquipCursorItem` queued (decision 0208 phase 1b: the
/// model-pane's click-with-payload path) and send `CMSG_AUTOEQUIP_ITEM` — the engine's own
/// contract (`cursor::doll::auto_equip_cursor_item`) already guarantees only a whole-stack,
/// CONTAINER-sourced Item payload (`bag >= 0`) ever reaches this queue.
///
/// The same ammo sub-fork as [`drain_container_uses`] (the one auto-equip sender `0x5e1480`
/// forks ammo-class → `CMSG_SET_AMMO`): a dropped ammo-class item loads by entry
/// instead, which is also the wire for the ammo slot's own drop (the XML routes it here via
/// `AutoEquipCursorItem` — decision 0526).
///
/// No pending-lock recording here (unlike the move/split/destroy drains) — matching this
/// codebase's own existing precedent for the SAME wire send: `drain_container_uses`'s
/// equip-vs-use fork already sends `AutoEquipItem` for an equippable bag-slot click with no lock
/// bookkeeping of its own. A real gap either way (an in-flight autoequip's source slot isn't
/// visibly dimmed), pre-existing and out of this slice's scope to fix.
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
        // The dropped item, by guid (source is a 0-based inner slot) — the sender needs it for both
        // of its forks. Unresolved (rare: the bag needed the template for the icon) sends a plain
        // AUTOEQUIP, whose refusal is at least visible.
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

/// Drain the auto-stores `PutItemInBag`/`PutItemInBackpack` queued (`benilla_ui`'s
/// `cursor::bag_verbs`) and send them on the wire.
///
/// **The destination is a BAG, not a slot** — that is the finding this drain exists to carry
/// (`0x4c7c00` → `0x5e12e0`): `CMSG_AUTOSTORE_BAG_ITEM` names `(srcbag, srcslot, dstbag)` and
/// the server picks where inside it the item lands, which is why an ordinary item dropped on a
/// bag BUTTON goes in the bag rather than swapping with it. The destination bag byte is
/// [`wire_pos`]'s own answer for that container's first slot (255 for the backpack, the bag's
/// player-array slot 19..22 / 63..68 for an equipped or bank bag), so one map serves both ends.
///
/// A **split carry** takes `CMSG_SPLIT_ITEM` instead — the reference's own fork on `[0xb4b40c]`
/// — with the literal `0xFF` where a destination slot would go, because that wire has a slot
/// field and this one does not.
///
/// No pending-lock recording, matching [`drain_container_autoequips`]'s own precedent for the same
/// class of send: the destination is not a slot, so there is no second end to lock.
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
                    // `0xFF` — the reference's own literal for "no destination slot"; the split
                    // wire has the field, the verb has no value for it.
                    dst_slot: 0xFF,
                    count,
                });
            }
        }
    }
}

/// Drain the inventory-slot ids `UseInventoryItem` queued (decision 0208 phase 1b: the doll
/// slot's right-click) and route the equipped position (bag 255 plus the 0-based wire slot —
/// `HandleUseItemOpcode` takes equipped positions the same as bag ones, vmangos `ItemHandler.cpp`)
/// through the shared use fork (`super::item_use_command`): the reference's doll click lands in
/// the same `CGItem::Use` a bag click does (`0x4c7af0`), quest fork included — one of the five
/// equippable quest-starters, worn and right-clicked, offers its quest instead of casting nothing.
/// Ids outside 1..=19 (ammo, the bag icons) are a no-op — the engine's own queue never receives
/// them from the shipped XML (only the 19 named slot buttons wire `UseInventoryItem` this slice),
/// but a stray Lua call is still refused rather than sent as nonsense.
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
        // The doll's own slot ids ARE the wire slots (`wire_pos`'s EQUIPMENT_BAG law), so the
        // equipped instance resolves off the player's INV array directly.
        let (guid, start_quest, spell_index, use_spell, entry, is_charter) = self_q
            .iter()
            .next()
            .and_then(|store| slot_guid(&store.0, EQUIPMENT_BAG, slot, &ladder.objects))
            .and_then(|guid| {
                let entry = ladder.objects.object(guid)?.object_entry()?;
                let t = ladder.items.template(entry, guid, &ladder.commands)?;
                // The wire's spell byte is a template BLOCK ordinal (decision 0666) — the
                // template is already in hand here for `start_quest`, so name the real one.
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

pub(super) fn drain_container_uses(
    script: Option<NonSendMut<UiScript>>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    merchant: Res<crate::ui_merchant::MerchantOpen>,
    bank: Res<crate::ui_bank::BankOpen>,
    mut equip_sound: MessageWriter<crate::sound::AutoEquipSound>,
    mut item_text: ResMut<crate::ui_item_text::ItemTextOpen>,
    targeting: crate::spell::cast_target::CastTargeting,
    // The client-side pending ("gray") lock — the right-click-open arm arms it (decision 0916).
    mut pending_items: ResMut<PendingItemOps>,
    // The loot-target latch — the right-click-open arm is one of its five arm sites, and the one
    // that lets `SMSG_LOOT_RESPONSE`'s admission gate recognise an item loot (decision 1531).
    mut loot_latch: ResMut<crate::ui_loot::LootLatch>,
    mut ladder: crate::spell::CastLadder,
    mut ui_errors: ResMut<crate::ui_action::UiErrorKeys>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let Some(mut script) = script else {
        return;
    };
    // Repair-mode clicks (the engine's pickup intercept — the real client's `0x4f9c7b` route):
    // resolve the clicked slot's item guid and send its single-item repair. The affordability
    // pre-check the client does (error 0x25) is left to the server's own refusal.
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
        // **A right-click cancels an armed gift wrap, whatever it then does** (decision 1934):
        // only a LEFT-click on a container slot spends the paper, and the reference's use path
        // clears the cursor on its way. Above every affordance below, because a sell or a
        // deposit is still a right-click. Re-arming is not a special case: a right-click on a
        // second piece of paper cancels the first here and arms itself in the dispatcher.
        if let Some(w) = script.cancel_gift_wrap() {
            debug!(
                "ui_items: right-click cancels the armed gift wrap on bag {} slot {}",
                w.bag, w.slot
            );
        }
        // Lua (bagID, 1-based slot) → the wire's player-array addressing.
        let slot0 = u8::try_from(slot.saturating_sub(1)).ok();
        // Sell affordance (decision 0081 v1): while a merchant is open, a bag-slot click sells the
        // slot's item instead of using/equipping it (`CMSG_SELL_ITEM`, count 0 = the whole stack —
        // the item is addressed by its concrete guid, not a bag slot). An empty slot has no guid, so
        // the click is a harmless no-op.
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
        // The deposit/withdraw affordance (decision 0604): while the bank is open, a container
        // click routes as the reference's at-bank auto-move instead of using/equipping — a bank
        // position (the vault or a bank bag) withdraws (`CMSG_AUTOSTORE_BANK_ITEM`), a carried
        // bag's item deposits (`CMSG_AUTOBANK_ITEM`). Which of the two opcodes the reference
        // fires per direction is INFERRED (0604) — vmangos routes AUTOSTORE by source position,
        // so either choice lands correctly. An empty slot refuses server-side, harmlessly.
        // Doll clicks never reach this drain (they flow through `drain_inventory_uses`), so
        // equipped gear keeps its plain use at the bank, like the reference.
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
        // Everything the reference's click law reads off the clicked slot, resolved once. `None` =
        // an empty slot, or a template still in flight — the click then falls all the way through
        // to a plain USE, whose refusal is at least visible (the template is all but always cached
        // by click time; the bag needed it for the icon).
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
                    // The wire's spell byte is a template BLOCK ordinal (decision 0666) — the
                    // template is right here, so send the real one rather than assuming 0.
                    spell_index: t.use_spell_index().unwrap_or(0),
                    use_spell: t.use_spell.map(|u| u.spell_id),
                    unwraps_gift: t.unwraps_gift(inst_flags),
                    begins_gift_wrap: t.begins_gift_wrap(inst_flags),
                    opens_loot: t.opens_loot(),
                    page_text: t.page_text,
                    is_charter: t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0,
                })
            });

        // The reference's equip-vs-use fork (`0x4fa3b9`/`0x4fa3bd`), with the ammo sub-fork: the
        // auto-equip sender `0x5e1480` sends `CMSG_SET_AMMO` (the item entry) for an ammo-class
        // item, `CMSG_AUTOEQUIP_ITEM` for any other equippable (inventoryType != 0 — weapons,
        // armor, bags). display_id feeds the synthetic pickup→place auto-equip sound (this path
        // never moves the cursor; a drag already gets that pair via the cursor-payload
        // transitions).
        //
        // The arm carries the reference's own **quest guard** (`0x4fa3bd`–`0x4fa3cc`, decision
        // 0664): it equips only when `StartQuest` (`[rec+0x1a8]`) is 0, so a quest-starter falls
        // through *whatever* its inventoryType — the five equippable ones (Pendant of Myzrael,
        // Arena Master, …) offer their quest on a right-click, they don't put themselves on.
        //
        // **Everything below this fork is `0x5d8d00`, the USE dispatcher, in ITS OWN order** — an
        // equippable item never reaches any of it.
        if let Some(c) = clicked.filter(|c| c.start_quest == 0 && c.inventory_type != 0) {
            // Through the one sender (decision 1750): it owns the ammo fork AND the soulbind
            // deferral. A deferred equip plays no sound — the reference's own equip kit rides the
            // arm that sends, and a question is not an equip.
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
        // #2 — the wrapped gift (`0x5d8d92`/`0x5d8d9d` → emitter `0x5edd60`): `CMSG_OPEN_ITEM`
        // unwraps it. FIRST in the dispatcher, ahead of the quest and readable arms, so a gift
        // that also carries letter text unwraps rather than reads. (vmangos answers this one with
        // an entry swap out of `character_gifts`, not a loot window.)
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
        // …and arm #2's OTHER side: a piece of wrapping paper (`0x5d8d9d`'s clear branch →
        // `0x5edea0`). **Purely local — nothing is sent.** The paper's slot locks, the displayed
        // cursor becomes mode 2, and the next LEFT-click on a container slot is what sends
        // `CMSG_WRAP_ITEM` (`ui_items::feed`'s wrap drain, through the engine's
        // `pickup_container_item`). Decision 1934.
        if let Some(c) = clicked.filter(|c| c.begins_gift_wrap) {
            debug!(
                "ui_items: arm gift wrap with {:#x} (lua bag {bag} slot {slot})",
                c.guid
            );
            // `arm_gift_wrap` queues the slot's own `ITEM_LOCK_CHANGED` — the paper dims from
            // the lock the arm takes, exactly as a held item's source slot does.
            script.arm_gift_wrap(bag, slot);
            continue;
        }
        // #3 — the quest-starter (`0x5d8dd2`, decision 0664): the item's own guid is the
        // questgiver. Placed at the reference's position rather than at the tail, so a starter
        // that is *also* readable or lootable offers its quest instead of reading/opening. Routed
        // through the one shared use fork, which is what turns a non-zero StartQuest into the
        // item-guid query.
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
        // #5 — readable: an item TEMPLATE carrying `PageText` (`0x5d8e4c`) — a book — opens the
        // reader on its page chain, client-side, no permission packet (`0x4e32e0(itemGuid)`;
        // decision 1105). Above #6 and above the open arm, which is the reference's own order and
        // the INVERSE of the tooltip's, where OPENABLE wins over READABLE: a template that is both
        // readable and lootable *shows* `<Right Click to Open>` and *reads* on click —
        // byte-verified, not a slip of ours.
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
        // #6 — readable: an item INSTANCE carrying `ITEM_FIELD_ITEM_TEXT_ID` (a mail-made
        // permanent letter) opens the reader — client-side, no permission packet (vmangos'
        // `CMSG_READ_ITEM` handler gates on the *template*'s PageText, which is 0 for the Plain
        // Letter; the text rides the ask-once `CMSG_ITEM_TEXT_QUERY` instead).
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
        // #8 — the open arm (`0x5d8f7c: test al,4` → emitter `0x5edc80`): a **bare** template
        // LOOTABLE test. `CMSG_OPEN_ITEM`, not `CMSG_USE_ITEM` — the server's
        // `HandleOpenItemOpcode` is the only handler that answers with
        // `SendLoot(item guid, LOOT_CORPSE)`, i.e. a loot window over a thing in your bag; sending
        // USE_ITEM instead casts the item's (absent) on-use spell and nothing happens, which was
        // exactly the "no way to open clams" symptom.
        //
        // Deliberately looser than the tooltip line's predicate (`ItemInfo::shows_open_line`): the
        // send consults **neither** LockID nor the instance UNLOCKED bit (VERIFIED both ways — no
        // `[rec+0x1ac]` operand exists anywhere on the send path). So a still-locked junkbox DOES
        // send, and the server's `EQUIP_ERR_ITEM_LOCKED` is where the player's "Item is locked"
        // line comes from. Gating locally would eat the click in silence (decision 0896).
        if let Some(c) = clicked.filter(|c| c.opens_loot) {
            debug!(
                "ui_items: open item {:#x} (lua bag {bag} → wire {bag_index}/{wire_slot})",
                c.guid
            );
            // **The loot latch, armed before the send** — arm site four of five (`0x5edcc0`, in
            // this same emitter `0x5edc80`, immediately ahead of the lock setter and the
            // `0x5edce5 push 0xac`). The latch is
            // the **item's own guid** (`[[edi+8]+0]`) because that is what the answer names:
            // vmangos' `HandleOpenItemOpcode` ends in `SendLoot(pItem->GetObjectGuid(),
            // LOOT_CORPSE)`, so `SMSG_LOOT_RESPONSE` comes back on the item guid with wire type
            // **1** — and 1477's admission gate refuses a type-1 answer against a *cold* latch.
            // Without this arm the window never opens (decision 1531).
            //
            // It arms no pose: predicate B `0x612710` answers false for an ITEM, and
            // [`crate::ui_loot::resolve_loot_kneel`] reaches the same false through a guid the
            // object manager cannot resolve — we stream no item entities.
            loot_latch.0 = Some(c.guid);
            // **The gray lock, armed before the send** — the reference's emitter `0x5edc80` calls
            // the lock setter `0x4953e0` at `0x5edcd9` and only then ships `CMSG_OPEN_ITEM`
            // (decision 0916). So a clam,
            // lockbox or loot bag greys the instant you right-click it and stays grey until the
            // open resolves — the emptied item vanishing (a resolving field update), a refusal
            // (`EQUIP_ERR_ITEM_LOCKED` on a still-locked junkbox), or the window closed with loot
            // left, whose `SMSG_LOOT_RELEASE_RESPONSE` unlocks it by guid (`ui_loot::net`'s
            // `loot_release_response`, the reference's `UnlockItem` at `48f299`).
            //
            // Deliberately NOT armed on the gift-unwrap arm above, which sends the same opcode:
            // its emitter `0x5edd60` contains neither call — no lock setter and no latch write.
            // That asymmetry is the reference's, verified, and copying it is the point.
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
        // The tail: a plain use (food, potions, hearthstone). The quest arm already fired above,
        // so the shared fork's own quest leg is inert here by construction.
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

/// One clicked bag slot, resolved: the live instance's guid and the template scalars the
/// reference's fork chain tests, read once so the chain above is a plain ordered cascade rather
/// than five repeats of the same lookup. Copy-cheap on purpose — no borrow of [`Items`] outlives
/// the resolve.
#[derive(Clone, Copy)]
struct Clicked {
    guid: u64,
    /// `OBJECT_FIELD_ENTRY` — the ammo arm addresses by entry, not by slot (decision 0526).
    entry: u32,
    /// Instance `ITEM_FIELD_ITEM_TEXT_ID` (a mail-made permanent letter); 0 = not a letter.
    item_text_id: u32,
    inventory_type: u32,
    display_info_id: u32,
    start_quest: u32,
    spell_index: u8,
    /// The template's on-use SPELL id — what the cast tail's in-flight guard is keyed on
    /// (decision 0908); `None` = this item casts nothing.
    use_spell: Option<u32>,
    /// `ItemInfo::unwraps_gift` for this instance — dispatcher arm #2.
    unwraps_gift: bool,
    /// `ItemInfo::begins_gift_wrap` — arm #2's OTHER side: a piece of wrapping paper. Arms the
    /// local wrap cursor and sends nothing (decision 1934).
    begins_gift_wrap: bool,
    /// `ItemInfo::opens_loot` for this template — dispatcher arm #8.
    opens_loot: bool,
    /// The template's `PageText` — dispatcher arm #5's book gate (decision 1105); `0` = not a
    /// book. The reader re-reads the head (and the material) off the template itself as it paints,
    /// like the reference, so only the fork's predicate is carried here.
    page_text: u32,
    /// The template's `ITEM_FLAG_CHARTER` — a guild petition (decision 1672).
    is_charter: bool,
}

/// Drain the pick/place/swap/split moves `PickupContainerItem`/`SplitContainerItem` queued and
/// send them on the wire (decision 0216 §6, whole-space since slice 2).
///
/// `count: None` (a whole-stack move/swap): both ends map through [`wire_pos`]. Both landing on
/// [`BAG_PLAYER_INVENTORY`] (the player's own grid — equipment, bag buttons, and the backpack) is
/// a `CMSG_SWAP_INV_ITEM` on the two player-array slots, unchanged; otherwise (either end an
/// equipped bag 1..4) it's the general `CMSG_SWAP_ITEM` — VERIFIED vmangos
/// `Server/Packets/Item.cpp:30-36`: body order is **dstbag, dstslot, srcbag, srcslot** (opcode
/// `0x10C`; the builder's arg order and the golden in `messages/items.rs::swap_item_body_destination_first`
/// already match this). An empty destination is still a swap on either wire (a move).
///
/// `count: Some(n)` (a split placement): both ends map through [`wire_pos`] (all five bags valid,
/// since `SplitContainerItem`'s pickup already resolved a real slot) → `CMSG_SPLIT_ITEM`, `count`
/// clamped to the wire's `u8`.
///
/// Every send locks the Lua-space slots it touches — both ends (decision 0218 §3: "a send locks
/// both ends") — recording each slot's CURRENT item guid as the resolving clear's baseline
/// ([`PendingItemOps::add`]) and firing `ITEM_LOCK_CHANGED` immediately, so a bag window's own
/// synchronous post-click repaint and every later frame agree (only the SOURCE slot's repaint at
/// the exact moment of THIS click can still show briefly stale — see the parent module doc —
/// corrected by this same-frame event).
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
    // **The completed gift wraps** (decision 1934): a left-click that spent an armed paper. Both
    // pairs go out in the wire's own order — the paper first — and the whole eligibility question
    // is the server's: it answers an ineligible target with one of the six `ERR_CANT_WRAP_*`
    // reasons through the ordinary `SMSG_INVENTORY_CHANGE_FAILURE` line.
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

/// **The player-direct slot ranges** the equip deferral elects on (`0x5e0c40`, decision 1750):
/// `[0, 22]` — the 19 equipment slots plus the four equipped-bag slots — and `[63, 68]`, the bank
/// bag slots. A wire position is player-direct when its bag byte is the player's own array AND its
/// slot falls in one of these; the backpack (`23..38`), the keyring and a bag's inner slots are all
/// on the same bag byte and are deliberately NOT in the set, which is what makes the election
/// pick out exactly "one side of this swap is an equip".
fn is_equip_position(bag_index: u8, slot: u8) -> bool {
    bag_index == BAG_PLAYER_INVENTORY && (slot <= 22 || (63..=68).contains(&slot))
}

/// One container move — the whole-stack swap, the split, the pending lock, and (decision 1750) the
/// equip soulbind deferral. Split out of the drain so `EquipPendingItem`'s re-issue runs the very
/// same body with `suppress` set, rather than a second copy of it that has to be kept agreeing.
///
/// Returns whether the move was sent (`false` = deferred behind `EQUIP_BIND_CONFIRM`).
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
        // The equip deferral's ELECTION (`0x5e0c40`): exactly one end is a player-direct equip
        // position, and the item that would bind is then whatever occupies the OTHER end. That
        // asymmetry is the whole rule and it falls out right in both directions — dragging a BoE
        // from a bag onto a worn slot asks about the bag's item, and swapping a worn item out onto
        // a bag slot that already holds a BoE asks about THAT one, because the swap equips it.
        // Unequipping onto an empty slot asks about nothing, which is why it never prompts.
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
        // The pending lock: both ends, baselined on their CURRENT (guid, count) — the resolving
        // clear then watches for either to move (an empty destination baselines (0, 0) and watches
        // for an item to land there).
        let (src_guid, src_count) = slot_guid_count(store, mv.src_bag, mv.src_slot, objects);
        let (dst_guid, dst_count) = slot_guid_count(store, mv.dst_bag, mv.dst_slot, objects);
        pending.add([
            (mv.src_bag, mv.src_slot, src_guid, src_count),
            (mv.dst_bag, mv.dst_slot, dst_guid, dst_count),
        ]);
        // One per locked end, as the reference's own per-slot unlock does — the slot travels in
        // how many times it fires, never in an argument (see `feed`'s note).
        for _ in 0..2 {
            script.fire_event("ITEM_LOCK_CHANGED", Vec::new());
        }
    }
    true
}

/// Drain the `(bag, slot, count)` destroys `DeleteCursorItem` queued (the delete-confirm popup's
/// accept, decision 0216 §3) and send `CMSG_DESTROYITEM`. `count == 0` is the engine's "whole
/// stack" convention — it rides straight onto the wire, which shares the same convention.
///
/// Locks the one slot touched — baselined on its CURRENT `(guid, count)`, same as
/// [`drain_container_moves`] — and fires `ITEM_LOCK_CHANGED` immediately. Unlike a move/split
/// there is no second "displaced" slot: a destroy only ever removes from where it's aimed.
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

    /// "Small Barnacled Clam", entry 7973 — the director's own case, and the `--open-item` probe's.
    const CLAM_ENTRY: u32 = 7973;
    /// The clam's item guid (`HIGHGUID_ITEM` 0x4000…, as the live probe read it back).
    const CLAM: u64 = 0x4000_0000_0000_1939;
    /// `PLAYER_FIELD_PACK_SLOT_1` — backpack slot 1's guid pair, so `slot_guid(0, 0)` resolves.
    const F_PACK_SLOT_1: u16 = 532;
    /// `OBJECT_FIELD_ENTRY` on the item object — what `Items::object(…).object_entry()` reads.
    const F_OBJECT_ENTRY: u16 = 3;

    /// Right-click backpack slot 1 (holding a LOOTABLE template) and run the click dispatcher.
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

        // The player, holding the clam in backpack slot 1.
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(ObjectFields::from_pairs(&[
                (F_PACK_SLOT_1, CLAM as u32),
                (F_PACK_SLOT_1 + 1, (CLAM >> 32) as u32),
            ])),
        ));
        // The item object (an entity in the one index, 2334) and its landed template —
        // LOOTABLE, so the dispatcher's open arm claims the click (`ItemInfo::opens_loot`).
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

    /// **The clam regression (decision 1531).** Arm site four of five: the `CMSG_OPEN_ITEM` send
    /// latches the ITEM's own guid (`0x5edcc0`). It is not cosmetic
    /// and it is not about the pose — vmangos answers this opcode with `SendLoot(item guid,
    /// LOOT_CORPSE)`, i.e. `SMSG_LOOT_RESPONSE` type **1** on that same guid (live-verified by
    /// `benilla-world --open-item`), and 1477's admission gate *refuses* a type-1 answer against a
    /// cold latch. Without the arm the clam greys and no window ever opens, which is exactly what
    /// the director saw.
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
        // The grey lock is the reference's other pre-send write, and still there (decision 0916).
        assert!(
            app.world().resource::<PendingItemOps>().contains(0, 1),
            "the slot greys at the click"
        );
    }

    /// **Right-clicking wrapping paper arms the wrap and sends NOTHING** (decision 1934) — the
    /// same dispatcher arm the wrapped-gift unwrap takes, on its other side. This is the bug:
    /// benilla fell through to `CMSG_USE_ITEM`, which casts a spell the paper does
    /// not have.
    #[test]
    fn a_wrapper_right_click_arms_the_cursor_and_ships_no_packet() {
        let (mut app, rx) = open_the_clam();
        while rx.try_recv().is_ok() {} // drain the clam's own send
                                       // Re-dress backpack slot 1 as a piece of wrapping paper: WRAPPER on the template, and no
                                       // WRAPPED bit on the instance (that combination is the begin-wrap arm; with the bit set it
                                       // would be a present, and would send `CMSG_OPEN_ITEM`).
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

/// **The bind confirmations' answers** (decision 1750) — `EquipPendingItem`/`CancelPendingEquip`
/// and `ConfirmBindOnUse`, drained where the senders they re-issue live.
///
/// Accept is a **re-issue with `suppress` set**, not a confirm packet: 1.12 has no such opcode, and
/// the reference's two verbs both land on `0x5e1be0(index, accept)`, whose accept arm re-runs the
/// original action. Running the very same sender is what makes the re-issue re-read the world — a
/// slot that changed under the open dialog is re-judged rather than sent stale — and `suppress` is
/// what stops the gate from asking the same question forever.
///
/// **Cancel sends nothing and only frees the record.** NAMED DIVERGENCE: the reference's cancel
/// also unlocks the src/dst occupants (`UnlockItem 0x495420`), because its deferral *took* those
/// locks on the way in. benilla's does not take them — [`send_container_move`]'s
/// [`PendingItemOps`] lock is a *pending wire op* lock, and a deferred action has no wire op for it
/// to resolve against, so filing one would leave a lock nothing could clear. The visible
/// consequence is that an item is not dimmed while its bind question is up; the auto-equip path
/// never dimmed at all (a gap this codebase already named), so this keeps the three arms consistent
/// instead of half-fixing one of them.
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
        // An index nobody filed is dropped in silence — `0x5e1be0` bounds-checks against the live
        // element count and returns, so a stray `EquipPendingItem(99)` from an addon does nothing.
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

/// `ConfirmBindOnUse()` — arm 290's accept, its own system because it is the only one of the three
/// that re-issues through the **cast ladder** (an item use IS a cast, decisions 0908/0914). No
/// index and no argument: that arm's pending state is one cell, not an array element. A count
/// rather than a bool for the same reason [`UiScript::take_binder_confirms`] is one; the record is
/// taken on the first, so a doubled accept re-uses nothing.
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

    /// 871 Flurry Axe — a real `item_template` row, quality 4 **bonding 2** (bind on EQUIP): the
    /// equip arms' whole predicate is `bonding == 2`, with no quality leg at all.
    const FLURRY_AXE: u32 = 871;
    const AXE_GUID: u64 = 0x4000_0000_0000_0871;
    /// `ITEM_FIELD_FLAGS` (wire field 21) — bit 0 is soulbound, the first half of `0x5da2c0`.
    const F_ITEM_FLAGS: u16 = 21;
    /// `PLAYER_FIELD_PACK_SLOT_1` — backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    const F_OBJECT_ENTRY: u16 = 3;

    fn load_ui(s: &UiScript) {
        // Through the chain-aware reader: this list names chain files now, and a
        // reader that joins `assets/ui` cannot resolve one (1838, 1887, 1888).
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

    /// Place the axe from backpack slot 1 onto the empty MainHand doll slot, then run the move
    /// drain. Returns the app, the wire receiver, and the script back.
    fn place_the_axe_on_the_doll() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(2, 4, false)
    }

    /// The same place with a chosen `bonding` — the one field the equip arm's predicate reads.
    fn place_the_axe_with_bonding(
        bonding: u32,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        place_the_axe_with(bonding, 4, false)
    }

    /// The place, parameterised over everything the gate looks at: the template's `bonding`, its
    /// `quality` (which this arm must ignore), and whether the instance is already soulbound
    /// (`ITEM_FIELD_FLAGS & 1`, the `0x5da2c0` bit).
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
        // Pick it up, drop it on MainHand — the queued move plus a pending CURSOR_UPDATE.
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

    /// The equip arm end to end, and **the ordering law that makes it work at all** (decision
    /// 1750). Placing a bind-on-equip item into a worn slot sends nothing and asks; the question
    /// is QUEUED, so it lands in the same flush as — and after — the `CURSOR_UPDATE` that same
    /// place queued, whose `StaticPopup_Hide` would otherwise cancel it before the player saw it.
    /// A *later* cursor change still retires it, which is what `UIParent.lua:356-360` is for.
    ///
    /// The first version of this test failed on exactly that cancellation, which is how the
    /// ordering hazard was found rather than assumed: benilla drains a step behind the input pass
    /// that fed it, where the reference defers inside the call that consumed the cursor.
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

        // The next tick flushes both, in queue order: the place's CURSOR_UPDATE first, then the
        // question. This is the whole point of queueing it.
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

        // A LATER cursor change — the player picked something else up — retires it, exactly as the
        // reference's arm intends, and the OnHide cancels the record.
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

    /// Accept: `EquipPendingItem(index)` **re-issues the original action** — there is no confirm
    /// opcode in 1.12 — and the re-issue does NOT ask again, because it carries the reference's own
    /// `suppress` flag. Without that flag the accept would re-enter the same gate and the question
    /// would be unanswerable.
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

        // Backpack slot 1 is wire 255/23, MainHand is wire 255/15 — both player-direct, so the
        // re-issue is CMSG_SWAP_INV_ITEM on the two player-array slots.
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

    /// Cancel sends nothing and frees the record. The doubled `CancelPendingEquip` the reference's
    /// own entry causes — `OnCancel` then `OnHide`, both naming it — is harmless by construction:
    /// the second call finds the element already free.
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
        // Two answers for one index — the entry names CancelPendingEquip twice on this path.
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

    /// The equip predicate is `bonding == 2` and **nothing else about the item** — no quality leg
    /// at all (`0x5e0e54`; benilla was about to carry the loot arm's
    /// `quality >= 2` across). Every other bonding value places straight through.
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

    /// A WHITE bind-on-equip item still asks — the proof that no quality leg exists on this arm.
    /// (The loot arm's `quality >= 2` would have silenced exactly this case.)
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

    /// An item that is ALREADY soulbound never asks — `0x5da2c0`, the same predicate the enchant
    /// cursor and the tooltip's Soulbound override use (decisions 0928, 1562).
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

    /// The AUTO-EQUIP arm (`0x5e1480`, event 289): the same predicate, a different event, and the
    /// same accept-is-a-re-issue. Driven through `AutoEquipCursorItem`, one of the three verbs
    /// benilla used to have three separate senders for — decision 1750 funnelled them into one so
    /// this gate could exist in a single place.
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

    /// The `0x5ea930` conjunct: an item the player **cannot** equip is never asked about — the
    /// question would be confirming an action the server can only refuse. Driven by a required
    /// level above the player's, the gate's first leg.
    #[test]
    fn an_unusable_item_is_never_asked_about() {
        benilla_formats::wow_data_or_skip!();
        let (mut app, rx) = place_the_axe_with(2, 4, false);
        // Re-push the player's requirement state and a template the level leg refuses. (The
        // fixture's own push leaves `level == 0`, the "decline to judge" state, so this test has
        // to establish a real player first for the leg to be reachable at all.)
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

    /// **The USE arm** (`0x5d8d00`, event 290) end to end, and the correction the reference
    /// forced. Right-clicking a bind-on-**use** item in a bag raises `USE_BIND` and sends nothing;
    /// `ConfirmBindOnUse()` re-issues the use with `suppress` set.
    ///
    /// `no_use_spell` drives the case that caught the first version of this code out: `0x5d91d3`
    /// has five predecessors and four are on-use-spell lookup FAILURES, so the reference asks the
    /// bind question even for an item with no usable on-use spell. Gating on the plain-cast route
    /// alone was narrower than the reference; both routes now carry it.
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

    /// The correction, pinned (`0x5d91d3`): an item with **no usable on-use spell** still raises
    /// the bind question. Under the first placement (inside the plain-cast route only) this asked
    /// nothing.
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
