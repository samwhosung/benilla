//! The **item** seam — `0x495d60`, the whole local law for an item target, plus the two confirm
//! popups that hang off it.
//!
//! Reached from two byte-identical rungs: the bag click (`PickupContainerItem 0x4f9b30` @ `4f9c54`)
//! and the paper-doll click (`0x4c7300` @ `4c76df`) — *if IsTargeting and `TargetingWantsItem
//! 0x6e6330`, then `0x495d60(itemGuidLo, itemGuidHi)` and return; nothing is picked up*. The VM
//! half of that reroute lives in `benilla_ui`'s cursor seam; this module is `0x495d60` itself.

use bevy::prelude::*;

use crate::net::SelfPlayer;

use super::TargetingWants;

/// The item guid a confirm popup is standing over — the reference's `0xb4e3c0`/`0xb4e3c4` pair,
/// written by BOTH confirm exits of `0x495d60` (`49608f`/`4960bb`) and read back by whichever Lua
/// answer arrives: `BindEnchant 0x48d2e0` re-invokes the gate with it, `ReplaceEnchant 0x48d300`
/// re-resolves it and calls the binder outright.
///
/// It is a *separate* global from the targeting word in the reference, and it stays separate here,
/// because it holds no cancel logic of its own: the reference never clears it, and never needs to,
/// since `BindTarget 0x6e5b40` re-checks `test $0x4010, [0xcecac0]` before it binds anything
/// (`6e5f1e`). Ours is inert the same way — every reader goes through
/// [`SpellTargeting::pending_for`], so once the word is gone a stale guid can do nothing.
#[derive(Resource, Default)]
pub(crate) struct EnchantConfirmItem(Option<u64>);

/// What the clicked item is, as `0x495d60` reads it off the live object — everything past the
/// template's three type fields comes from the item OBJECT's descriptor, which for our own bags is
/// fully streamed.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClickedItem {
    /// `0x5d9f30` / `0x5d9f90` / `0x5d9ff0` — the item template's Class, SubClass and
    /// InventoryType (each resolved through the item cache `0x55ba30`).
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    pub(crate) inventory_type: u32,
    /// `0x5da2c0` — "has this item already been through the bind question?": `ITEM_FIELD_FLAGS &
    /// 1` (already soulbound), **or** any of its seven live `ITEM_FIELD_ENCHANTMENT` slots naming
    /// a row with [`benilla_formats::EnchantCatalog::binds_the_item`] (`5da300`–`5da320`).
    pub(crate) already_bound: bool,
    /// `ITEM_FIELD_ENCHANTMENT[slot]` for PERM (0) and TEMP (1), as the replace check reads them
    /// (`495eec: movl 0x40(%ecx,%eax,4)` with `eax = 3*slot`). `None` for an empty slot, a
    /// negative id (the ref's `jl`), or an id that names no `SpellItemEnchantment` row.
    pub(crate) existing_enchant: [Option<u32>; 2],
}

/// The enchant an ENCHANT_ITEM effect would apply — `SpellItemEnchantment[EffectMiscValue[i]]`,
/// resolved once at `495e93` and read by BOTH confirms.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NewEnchant {
    pub(crate) id: u32,
    /// [`benilla_formats::EnchantCatalog::binds_the_item`] — `Flags & 1`.
    pub(crate) binds: bool,
}

/// What `0x495d60` decides for one clicked item. Four exits, and the three non-`Bind` ones all
/// return **before** `BindTarget`, so the targeting word survives every one of them — that is what
/// keeps the cursor up through a mis-click *and* through both confirm popups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemBind {
    /// `496056: call 0x6e5b40` — bind the item and send.
    Bind,
    /// `496068` — `0x6e1a00(spell, 0x0a)`, no packet.
    Refuse(u8),
    /// `496087` — park the guid at `0xb4e3c0/0xb4e3c4` and fire event **402 `BIND_ENCHANT`**
    /// (`4960a3`, no args): *"Enchanting this item will bind it to you."* Its Yes is
    /// `BindEnchant 0x48d2e0`, which re-invokes this very gate with the confirmed flag set.
    ConfirmBind,
    /// `4960b3` — park the guid and fire event **403 `REPLACE_ENCHANT`** (`4960e4`, format
    /// `"%s%s"` = the OLD then the NEW enchant name): *"Do you want to replace "%s" with "%s"?"*.
    /// Its Yes is `ReplaceEnchant 0x48d300`, which skips the gate entirely and calls the binder.
    ConfirmReplace { existing: u32, new: u32 },
}

/// `0x495d60` — the whole local law for an item target, run at BIND time (the click), not at hover
/// time. The reference walks the spell's three effects; for each `ENCHANT_ITEM` (53) /
/// `ENCHANT_ITEM_TEMPORARY` (54) one it runs, **in this order**:
///
/// 1. **The equipped-item gate**. `EquippedItemSubClassMask [+0xec] != 0` ⇒ the
///    item's class must equal `EquippedItemClass [+0xe8]` **and** `(1 << subclass)` must be in the
///    mask (`495e10`–`495e28`); `EquippedItemInventoryTypeMask [+0xf0] != 0` ⇒ `(1 <<
///    InventoryType)` must be in it (`495e4d`–`495e70`). Either miss is
///    [`ItemBind::Refuse`] — the client's own "Invalid target" red line, no packet, cursor kept.
/// 2. **The bind confirm** (`495e93`–`495ec6`): the enchant this effect would apply
///    must name a real `SpellItemEnchantment` row whose `Flags & 1` is set, the item must not be
///    [`ClickedItem::already_bound`], the confirmed flag must be clear, and the item's
///    InventoryType must be nonzero (`495ebc` — an *equippable* item; a lockbox or a reagent is
///    never asked). All four ⇒ [`ItemBind::ConfirmBind`].
/// 3. **The replace confirm** (`495ecc`–`495f1c`): the slot is PERM for effect 53 and TEMP for 54
///    (`495ed3: cmpl $0x35 / setne` — the ONE place the two effect types diverge), and if that slot
///    already holds an enchant that names a row, **and** the new enchant names one too, ⇒
///    [`ItemBind::ConfirmReplace`]. Note this leg does NOT consult the confirmed flag: answering
///    the bind popup Yes re-enters here and can raise the replace popup next, which is exactly the
///    reference's two-popup chain.
///
/// Otherwise [`ItemBind::Bind`]. A spell with no enchant effect (the bare-`Targets 0x10` rows that
/// are Disenchant and kin, and every `0x4000` lockbox opener) has no leg to fail at all — the
/// reference walks its loop and falls straight through to `496056: call 0x6e5b40`, and so do we.
///
/// One narrowing, measured rather than assumed: the reference tests **all three** effect slots,
/// [`benilla_formats::SpellDisplay`] carries only slot 0, and across the whole 363-row item-target
/// family not one row hides its enchant effect in slot 1 or 2 — pinned by the formats-side family
/// test, which fails if that ever stops being true.
pub(crate) fn item_bind_verdict(
    def: &benilla_formats::SpellDisplay,
    item: &ClickedItem,
    new: Option<NewEnchant>,
    confirmed: bool,
) -> ItemBind {
    let perm = def.effects[0] == benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;
    if !perm && def.effects[0] != benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY {
        return ItemBind::Bind;
    }
    if def.equipped_item_subclass_mask != 0 {
        let class_ok = i64::from(item.class) == i64::from(def.equipped_item_class);
        let sub_ok =
            item.subclass < 32 && def.equipped_item_subclass_mask & (1u32 << item.subclass) != 0;
        if !(class_ok && sub_ok) {
            return ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET);
        }
    }
    if def.equipped_item_inventory_type_mask != 0
        && !(item.inventory_type < 32
            && def.equipped_item_inventory_type_mask & (1u32 << item.inventory_type) != 0)
    {
        return ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET);
    }
    if let Some(new) = new {
        if new.binds && !item.already_bound && !confirmed && item.inventory_type != 0 {
            return ItemBind::ConfirmBind;
        }
        if let Some(existing) = item.existing_enchant[usize::from(!perm)] {
            return ItemBind::ConfirmReplace {
                existing,
                new: new.id,
            };
        }
    }
    ItemBind::Bind
}

/// The item half's commit — the bag and paper-doll click seams (`PickupContainerItem 0x4f9b30`
/// @ `4f9c54`–`4f9c6d` and its byte-identical doll twin `0x4c7300` @ `4c76df`–`4c76fb`: *if
/// IsTargeting and TargetingWantsItem, then `0x495d60(itemGuidLo, itemGuidHi)` and return —
/// nothing is picked up*). The VM half of that reroute lives in `benilla_ui`'s cursor seam; this
/// drain is `0x495d60` itself: resolve the clicked slot's live item, run [`item_bind_verdict`],
/// and on a pass do what `496056` does — hand the item to the ONE binder, which fills the word's
/// item bit and lets `SendCast 0x6e54f0` commit. Same block, two opcodes: `CMSG_CAST_SPELL` for
/// an enchant off the Craft window, `CMSG_USE_ITEM` for a poison bottle's own ON_USE.
///
/// It also drains the **two confirm popups' answers**, because the reference routes both back
/// through this same gate rather than to a second machine:
/// `BindEnchant 0x48d2e0` re-invokes `0x495d60` with its third parameter 1, and
/// `ReplaceEnchant 0x48d300` skips the gate and calls `0x6e5b40` directly. So a Yes is just
/// another item ask over the parked guid, and there is one code path for all three entries.
///
/// The post-send tail is the ground commit's: arm the pending cast + the GCD, and
/// clear the word. A click on an EMPTY slot binds nothing and keeps the mode — the ref's
/// `0x495d60` returns at its own null-item guard (`495da1`) — and so does a refusal or either
/// confirm, all three of which return before `BindTarget`.
pub(crate) fn commit_item_cast_on_pick(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_q: Query<&crate::net::ObjectStore, With<SelfPlayer>>,
    enchants: Option<Res<crate::items::Enchants>>,
    mut parked: ResMut<EnchantConfirmItem>,
    mut ladder: crate::spell::CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };
    // The three entries into the ONE gate, in the order the reference reaches them: a fresh click
    // (unconfirmed), then the bind popup's Yes over the parked guid (confirmed). The replace
    // popup's Yes bypasses the gate entirely, so it collects separately.
    let mut asks: Vec<(u64, bool)> = Vec::new();
    let mut bind_outright: Vec<u64> = Vec::new();
    for (bag, slot) in script.take_item_picks() {
        let slot0 = u8::try_from(slot.saturating_sub(1)).unwrap_or(0);
        match self_q
            .iter()
            .next()
            .and_then(|store| crate::ui_items::slot_guid(&store.0, bag, slot0, &ladder.objects))
        {
            Some(guid) => asks.push((guid, false)),
            None => {
                debug!("ui_action: item pick on an empty slot (bag {bag} slot {slot}) — mode kept")
            }
        }
    }
    for answer in script.take_enchant_confirms() {
        let Some(guid) = parked.0 else { continue };
        match answer {
            benilla_ui::script::EnchantConfirm::Bind => asks.push((guid, true)),
            benilla_ui::script::EnchantConfirm::Replace => bind_outright.push(guid),
        }
    }

    for (item_guid, confirmed) in asks {
        let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Item) else {
            continue; // a click raced a cancel — the word is gone, so there is nothing to bind
        };
        // The gate needs the clicked item's template; an unresolved one (never seen in practice —
        // the bag needed it for the icon) binds ungated and lets the server judge, the same
        // permissive shape the rest of the click law uses.
        let entry = ladder
            .objects
            .object(item_guid)
            .and_then(|o| o.object_entry());
        let clicked = match entry {
            Some(entry) => ladder
                .items
                .template(entry, item_guid, &ladder.commands)
                .map(|t| (t.class, t.subclass, t.inventory_type)),
            None => None,
        };
        let def = ladder
            .spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id));
        let (Some(def), Some((class, subclass, inventory_type))) = (def, clicked) else {
            debug!("ui_action: item pick — cast {spell_id} at item {item_guid:#x} (ungated)");
            ladder.commit_targeted(
                spell_id,
                commit,
                crate::spell::cast_send::TargetedBind::Item(item_guid),
            );
            continue;
        };
        let cat = enchants.as_deref();
        let fields = ladder.objects.object(item_guid);
        let item = ClickedItem {
            class,
            subclass,
            inventory_type,
            // `0x5da2c0`: already soulbound, or already carrying an enchant that binds. The
            // item tooltip's Soulbound override reads the SAME predicate — see
            // [`crate::items::already_bound`].
            already_bound: fields.is_some_and(|f| crate::items::already_bound(f, cat)),
            existing_enchant: [0u8, 1]
                .map(|slot| fields.and_then(|f| crate::items::live_enchant(f, slot, cat))),
        };
        // `495e93` — the enchant this cast would apply, resolved once and read by both confirms.
        // `EffectMiscValue` is signed and the ref tests `jl` before its range compare.
        let new = u32::try_from(def.effect_misc_value[0])
            .ok()
            .filter(|&id| cat.is_some_and(|c| c.0.has_row(id)))
            .map(|id| NewEnchant {
                id,
                binds: crate::items::binds(id, cat),
            });
        match item_bind_verdict(def, &item, new, confirmed) {
            ItemBind::Refuse(reason) => {
                debug!(
                    "ui_action: cast {spell_id} refused at the item bind ({reason:#x}) — \
                     the cursor stays up"
                );
                ladder.cast_errors.push_local(spell_id, reason);
            }
            ItemBind::ConfirmBind => {
                debug!("ui_action: item bind confirm for {spell_id} on item {item_guid:#x}");
                parked.0 = Some(item_guid);
                script.fire_event("BIND_ENCHANT", vec![]);
            }
            ItemBind::ConfirmReplace { existing, new } => {
                // `4960d0`/`4960d4` read both names off the rows with no suppression gate — the
                // OLD one first, then the NEW, matching `"Do you want to replace %s with %s?"`.
                let name = |id: u32| {
                    cat.and_then(|c| c.0.name(id))
                        .unwrap_or_default()
                        .to_string()
                };
                debug!("ui_action: replace-enchant confirm {existing} -> {new} on {item_guid:#x}");
                parked.0 = Some(item_guid);
                script.fire_event(
                    "REPLACE_ENCHANT",
                    vec![
                        benilla_ui::script::ScriptValue::Str(name(existing)),
                        benilla_ui::script::ScriptValue::Str(name(new)),
                    ],
                );
            }
            ItemBind::Bind => {
                debug!("ui_action: item pick — cast {spell_id} at item {item_guid:#x}");
                ladder.commit_targeted(
                    spell_id,
                    commit,
                    crate::spell::cast_send::TargetedBind::Item(item_guid),
                );
            }
        }
    }

    // `ReplaceEnchant 0x48d300` — re-resolve the parked guid and call `0x6e5b40` outright. No gate
    // re-run at all: the popup's Yes IS the answer to every question the gate would ask again.
    for item_guid in bind_outright {
        let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Item) else {
            continue;
        };
        debug!("ui_action: replace-enchant accepted — cast {spell_id} at item {item_guid:#x}");
        ladder.commit_targeted(
            spell_id,
            commit,
            crate::spell::cast_send::TargetedBind::Item(item_guid),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_item_bind_gate_mirrors_0x495d60() {
        use benilla_formats::SpellDisplay;
        fn item_target_refusal(
            def: &SpellDisplay,
            class: u32,
            subclass: u32,
            inventory_type: u32,
        ) -> Option<u8> {
            let item = ClickedItem {
                class,
                subclass,
                inventory_type,
                ..Default::default()
            };
            match item_bind_verdict(def, &item, None, false) {
                ItemBind::Refuse(code) => Some(code),
                ItemBind::Bind => None,
                other => panic!("no enchant row resolved — {other:?} is unreachable"),
            }
        }
        // Item classes/subclasses/inventory types, 1.12 values.
        const CLASS_WEAPON: u32 = 2;
        const CLASS_ARMOR: u32 = 4;
        const SUB_DAGGER: u32 = 15;
        const SUB_SHIELD: u32 = 6;
        const INVTYPE_WRIST: u32 = 9;
        const INVTYPE_CHEST: u32 = 5;
        const ENCHANT: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;

        // Enchant Bracer - Minor Health (7418): armor, any subclass, WRIST only.
        let bracer = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            equipped_item_class: CLASS_ARMOR as i32,
            equipped_item_subclass_mask: 0x1f,
            equipped_item_inventory_type_mask: 1 << INVTYPE_WRIST,
            ..Default::default()
        };
        assert_eq!(
            item_target_refusal(&bracer, CLASS_ARMOR, 1, INVTYPE_WRIST),
            None,
            "a bracer takes the bracer enchant"
        );
        assert_eq!(
            item_target_refusal(&bracer, CLASS_ARMOR, 1, INVTYPE_CHEST),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "the inventory-type leg (495e4d) refuses a chestpiece"
        );
        assert_eq!(
            item_target_refusal(&bracer, CLASS_WEAPON, 1, INVTYPE_WRIST),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "the class leg (495e10) refuses a weapon"
        );

        // Instant Poison (8679): weapon class, a subclass mask, NO inventory-type requirement.
        let poison = SpellDisplay {
            effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
            equipped_item_class: CLASS_WEAPON as i32,
            equipped_item_subclass_mask: 0x2a5f3,
            equipped_item_inventory_type_mask: 0,
            ..Default::default()
        };
        // 8679's real mask carries dagger (15), so a rogue's own weapon passes the subclass leg.
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, SUB_DAGGER, 13),
            None
        );
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, 1, 13),
            None,
            "and two-handed axe (1) is in it too — poisons are broad, the class leg is the fence"
        );
        assert_eq!(
            item_target_refusal(&poison, CLASS_ARMOR, SUB_SHIELD, 14),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET),
            "a shield is armor — the class leg alone stops it"
        );

        // Disenchant (13262): an item-targeted spell with NO enchant effect. The reference walks
        // its loop, finds no 53/54 arm, and falls straight through to the bind — anything goes,
        // and the server judges.
        let disenchant = SpellDisplay {
            effects: [99, 0, 0],
            equipped_item_class: -1,
            ..Default::default()
        };
        assert_eq!(
            item_target_refusal(&disenchant, CLASS_ARMOR, 1, INVTYPE_CHEST),
            None
        );
        assert_eq!(
            item_target_refusal(&disenchant, CLASS_WEAPON, SUB_DAGGER, 13),
            None
        );

        // A subclass past the mask's 32 bits can never be in it — shifted, that would be UB-ish
        // nonsense, so the gate refuses instead of wrapping.
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, 40, 13),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET)
        );
    }

    /// `0x495d60`'s two confirm branches and, more importantly, the ORDER they sit in — the equip
    /// gate first (`495e10`), then the bind confirm (`495e93`), then the replace confirm
    /// (`495ecc`), each returning before `BindTarget`.
    ///
    /// The chain this pins is the one that is easy to get subtly wrong: the bind confirm consults
    /// the confirmed flag and the replace confirm does **not**, so answering the bind popup Yes
    /// re-enters the gate and can raise the replace popup next. Getting that backwards would
    /// either swallow the second question or loop on the first.
    #[test]
    fn the_two_confirms_chain_in_the_references_order() {
        use benilla_formats::SpellDisplay;
        const ENCHANT: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;
        const TEMP: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY;
        const INVTYPE_WEAPON: u32 = 13;

        // A permanent weapon enchant whose row binds the item (the ZG/imbue family's `Flags & 1`).
        let perm = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            effect_misc_value: [1900, 0, 0],
            ..Default::default()
        };
        let binder = Some(NewEnchant {
            id: 1900,
            binds: true,
        });
        let plain = Some(NewEnchant {
            id: 1900,
            binds: false,
        });
        let bare = ClickedItem {
            inventory_type: INVTYPE_WEAPON,
            ..Default::default()
        };

        // 1 · The bind confirm, and each of its four legs turned off in turn (`495ea3`-`495ec6`).
        assert_eq!(
            item_bind_verdict(&perm, &bare, binder, false),
            ItemBind::ConfirmBind
        );
        assert_eq!(
            item_bind_verdict(&perm, &bare, plain, false),
            ItemBind::Bind,
            "a row without the flag never asks (495ea3)"
        );
        assert_eq!(
            item_bind_verdict(&perm, &bare, None, false),
            ItemBind::Bind,
            "an EffectMiscValue naming no row never asks (495e9e)"
        );
        assert_eq!(
            item_bind_verdict(
                &perm,
                &ClickedItem {
                    already_bound: true,
                    ..bare
                },
                binder,
                false
            ),
            ItemBind::Bind,
            "0x5da2c0 says it is already bound (495eb3)"
        );
        assert_eq!(
            item_bind_verdict(
                &perm,
                &ClickedItem {
                    inventory_type: 0,
                    ..bare
                },
                binder,
                false
            ),
            ItemBind::Bind,
            "a non-equippable item — a lockbox, a reagent — is never asked (495ec6)"
        );

        // 2 · Yes on the bind popup re-enters with the flag set (`BindEnchant 0x48d2e0`), and the
        // gate falls through to the REPLACE question for the same click.
        let enchanted = ClickedItem {
            existing_enchant: [Some(2564), None],
            ..bare
        };
        assert_eq!(
            item_bind_verdict(&perm, &enchanted, binder, false),
            ItemBind::ConfirmBind,
            "the bind question comes first"
        );
        assert_eq!(
            item_bind_verdict(&perm, &enchanted, binder, true),
            ItemBind::ConfirmReplace {
                existing: 2564,
                new: 1900
            },
            "and its Yes lands on the replace question — the ref's two-popup chain"
        );

        // 3 · The slot fork (`495ed3: cmpl $0x35 / setne`) — the ONE place effect 53 and 54
        // diverge. A permanent enchant asks about PERM; a poison asks about TEMP.
        let temp_spell = SpellDisplay {
            effects: [TEMP, 0, 0],
            effect_misc_value: [1900, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            item_bind_verdict(&temp_spell, &enchanted, plain, false),
            ItemBind::Bind,
            "a poison over a permanently-enchanted weapon replaces nothing — different slot"
        );
        let poisoned = ClickedItem {
            existing_enchant: [None, Some(2564)],
            ..bare
        };
        assert_eq!(
            item_bind_verdict(&temp_spell, &poisoned, plain, false),
            ItemBind::ConfirmReplace {
                existing: 2564,
                new: 1900
            },
            "but over an already-poisoned one it does"
        );
        assert_eq!(
            item_bind_verdict(&perm, &poisoned, plain, false),
            ItemBind::Bind,
            "and the mirror: a permanent enchant ignores the temp slot"
        );

        // 4 · The equip gate still runs FIRST — a refusal beats both confirms (495e10 < 495e93).
        let bracer_only = SpellDisplay {
            effects: [ENCHANT, 0, 0],
            effect_misc_value: [1900, 0, 0],
            equipped_item_class: 4,
            equipped_item_subclass_mask: 0x1f,
            equipped_item_inventory_type_mask: 1 << 9,
            ..Default::default()
        };
        assert_eq!(
            item_bind_verdict(&bracer_only, &enchanted, binder, false),
            ItemBind::Refuse(crate::spell::cast_target::ERR_INVALID_TARGET)
        );
    }
}
