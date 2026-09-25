//! The item seam, `0x495d60`, and its two confirm popups. The bag click (`PickupContainerItem
//! 0x4f9b30`) and the paper-doll click (`0x4c7300`) both call it when `IsTargeting` and
//! `TargetingWantsItem 0x6e6330` hold, and pick nothing up; the VM half of that reroute is in
//! `benilla_ui`'s cursor seam.

use bevy::prelude::*;

use crate::net::SelfPlayer;

use super::TargetingWants;

/// The item guid a confirm popup stands over (`0xb4e3c0`), written by both confirm exits of
/// `0x495d60`. The reference never clears it, since `BindTarget 0x6e5b40` re-tests the word
/// before binding; here every reader goes through [`SpellTargeting::pending_for`], so a stale guid
/// is inert.
#[derive(Resource, Default)]
pub(crate) struct EnchantConfirmItem(Option<u64>);

/// The clicked item as `0x495d60` reads it: template type fields, the rest off the item object.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ClickedItem {
    /// Template Class, SubClass and InventoryType (`0x5d9f30`, `0x5d9f90`, `0x5d9ff0`, through the
    /// item cache `0x55ba30`).
    pub(crate) class: u32,
    pub(crate) subclass: u32,
    pub(crate) inventory_type: u32,
    /// `0x5da2c0`: `ITEM_FIELD_FLAGS & 1` (soulbound), or any of the seven enchantment slots names
    /// a row that [`benilla_formats::EnchantCatalog::binds_the_item`].
    pub(crate) already_bound: bool,
    /// `ITEM_FIELD_ENCHANTMENT` PERM (0) and TEMP (1); `None` for empty, a negative id, or an id
    /// with no `SpellItemEnchantment` row.
    pub(crate) existing_enchant: [Option<u32>; 2],
}

/// The enchant the cast would apply, `SpellItemEnchantment[EffectMiscValue]`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NewEnchant {
    pub(crate) id: u32,
    /// `Flags & 1`.
    pub(crate) binds: bool,
}

/// `0x495d60`'s verdict. Every exit but `Bind` returns before `BindTarget`, so the word and the
/// cursor survive a refusal and both popups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ItemBind {
    /// `BindTarget 0x6e5b40`: bind the item and send.
    Bind,
    /// `0x6e1a00(spell, 0x0a)`, no packet.
    Refuse(u8),
    /// Park the guid and fire `BIND_ENCHANT` (event 402, no args); its Yes, `BindEnchant
    /// 0x48d2e0`, re-runs the gate confirmed.
    ConfirmBind,
    /// Park the guid and fire `REPLACE_ENCHANT` (event 403, old name then new); its Yes,
    /// `ReplaceEnchant 0x48d300`, skips the gate and binds.
    ConfirmReplace { existing: u32, new: u32 },
}

/// `0x495d60`, run at the click, for an `ENCHANT_ITEM` (53) or `ENCHANT_ITEM_TEMPORARY` (54)
/// effect, in this order:
///
/// 1. The equip gate: with a nonzero `EquippedItemSubClassMask` the class must match and the
///    subclass bit be set; with a nonzero `EquippedItemInventoryTypeMask` the InventoryType bit
///    must be set. A miss is [`ItemBind::Refuse`], "Invalid target", no packet.
/// 2. The bind confirm: the new enchant's row binds, the item is not
///    [`ClickedItem::already_bound`], the confirmed flag is clear and the InventoryType is nonzero.
/// 3. The replace confirm: the slot is PERM for 53 and TEMP for 54, and both it and the new
///    enchant name a row. It ignores the confirmed flag, so the bind popup's Yes can raise it next.
///
/// A spell with no enchant effect (Disenchant, the `0x4000` lockbox openers) binds outright.
/// The reference walks all three effect slots; [`benilla_formats::SpellDisplay`] carries only
/// slot 0, and a formats test fails if any item-target row puts its enchant in slot 1 or 2.
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

/// The bag and paper-doll click commit: resolve the clicked slot's item, run
/// [`item_bind_verdict`] and on a pass bind it for `SendCast 0x6e54f0` (`CMSG_CAST_SPELL` for an
/// enchant, `CMSG_USE_ITEM` for a poison bottle). It also takes both popups' answers: `BindEnchant
/// 0x48d2e0` re-runs `0x495d60` confirmed, `ReplaceEnchant 0x48d300` calls `0x6e5b40` directly. An
/// empty slot (`0x495d60`'s null-item guard), a refusal or a confirm keeps the mode.
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
    // Fresh clicks, then the bind popup's Yes, go through the gate; the replace popup's Yes does
    // not.
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
            continue; // a click raced a cancel
        };
        // An unresolved template binds ungated and the server judges.
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
            // `0x5da2c0`, the same predicate as the tooltip's Soulbound line.
            already_bound: fields.is_some_and(|f| crate::items::already_bound(f, cat)),
            existing_enchant: [0u8, 1]
                .map(|slot| fields.and_then(|f| crate::items::live_enchant(f, slot, cat))),
        };
        // `EffectMiscValue` is signed; the reference rejects a negative one before the row lookup.
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
                // Old name first, then new: "Do you want to replace %s with %s?".
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

    // `ReplaceEnchant 0x48d300`: bind the parked guid with no gate re-run.
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
        // Its real mask carries dagger (15), so a rogue's own weapon passes the subclass leg.
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

        // Disenchant (13262) has no enchant effect: anything binds and the server judges.
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

        // A subclass past the mask's 32 bits is refused, not wrapped.
        assert_eq!(
            item_target_refusal(&poison, CLASS_WEAPON, 40, 13),
            Some(crate::spell::cast_target::ERR_INVALID_TARGET)
        );
    }

    /// The equip gate, then the bind confirm, then the replace confirm; the bind popup's Yes can
    /// raise the replace popup because only the bind confirm reads the confirmed flag.
    #[test]
    fn the_two_confirms_chain_in_the_references_order() {
        use benilla_formats::SpellDisplay;
        const ENCHANT: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM;
        const TEMP: u32 = benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY;
        const INVTYPE_WEAPON: u32 = 13;

        // A permanent weapon enchant whose row binds the item (`Flags & 1`).
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

        // 1. The bind confirm, and each of its four legs turned off in turn.
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

        // 2. The bind popup's Yes re-enters confirmed and reaches the replace question.
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
            "and its Yes lands on the replace question — the reference's two-popup chain"
        );

        // 3. The slot fork, the one place effects 53 and 54 differ: PERM for 53, TEMP for 54.
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

        // 4. The equip gate runs first: a refusal beats both confirms.
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
