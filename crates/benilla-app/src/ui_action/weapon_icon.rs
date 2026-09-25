//! The spells that show an equipped weapon's icon: the melee auto-attack (`0x4e6870`) and the
//! ranged auto-repeat shots (`0x4e6990`). A weapon swap changes the icon without touching the
//! action table, so the feed refreshes these every frame.

use benilla_formats::SpellDisplay;

use crate::creature_anim::UNIT_FLAG_DISARMED;
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};

const EQUIPMENT_SLOT_MAINHAND: u8 = 15;

/// The ranged helper's read, `[ecx+0x88]` in `0x4e6990` (17 × 8).
const EQUIPMENT_SLOT_RANGED: u8 = 17;

/// A thrown weapon keeps the spell's own icon (`0x4e6990` tests `0x5d9f90 == 0x10`).
const ITEM_SUBCLASS_THROWN: u32 = 16;

/// The unarmed auto-attack icon, hardcoded at `0x84bf58`, in place of Attack's own `Temp`.
const SPELL_RESET_ICON: &str = "Interface\\Buttons\\Spell-Reset";

/// The disarmed guard's class test (`0x4e68df`): a main hand holding a non-weapon keeps its icon.
const ITEM_CLASS_WEAPON: u32 = 2;

/// The main-hand item's `(class, icon)`; the icon is `None` until its display row loads.
fn main_hand_item(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<(u32, Option<String>)> {
    let guid = store.0.player_inv_slot(EQUIPMENT_SLOT_MAINHAND)?;
    let entry = objects.object(guid)?.object_entry()?;
    let template = items.template(entry, guid, commands)?;
    let (class, display) = (template.class, template.display_info_id);
    let icon = icons
        .and_then(|i| i.catalog.get(display))
        .and_then(|d| d.icon.clone());
    Some((class, icon))
}

/// The melee auto-attack icon (`0x4e6870`), first match wins: the form's `AttackIconID`
/// (`+0x34`, `0x4e68af`-`0x4e68da`), `Spell-Reset` while disarmed (`0x4e68df`), the main-hand
/// icon, then `Spell-Reset` for an empty hand.
pub(crate) fn melee_auto_attack_icon(
    store: &ObjectStore,
    forms: &std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> String {
    let form = store.0.unit_shapeshift_form();
    if form != 0 {
        if let Some(icon) = forms
            .get(&u32::from(form))
            .and_then(|f| f.attack_icon.clone())
        {
            return icon;
        }
    }
    let main = main_hand_item(store, objects, items, icons, commands);
    // The disarmed guard (`0x4e68df`): a disarmed weapon hand shows `Spell-Reset`, as its swing
    // is unarmed.
    if store.0.unit_flags() & UNIT_FLAG_DISARMED != 0
        && main
            .as_ref()
            .is_some_and(|&(class, _)| class == ITEM_CLASS_WEAPON)
    {
        return SPELL_RESET_ICON.to_string();
    }
    main.and_then(|(_, icon)| icon)
        .unwrap_or_else(|| SPELL_RESET_ICON.to_string())
}

/// The ranged weapon's icon (`0x4e6990`). `None` (no weapon, thrown, unstreamed) shows the
/// spell's own icon, never `Spell-Reset` (`0x4e6a44`).
pub(crate) fn ranged_weapon_icon(
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    let guid = store.0.player_inv_slot(EQUIPMENT_SLOT_RANGED)?;
    let entry = objects.object(guid)?.object_entry()?;
    let template = items.template(entry, guid, commands)?;
    if template.subclass == ITEM_SUBCLASS_THROWN {
        return None;
    }
    let display = template.display_info_id;
    icons?.catalog.get(display)?.icon.clone()
}

/// Whether `spell` shows a weapon's icon; the per-frame icon refresh keys on it.
pub(super) fn substitutes_weapon_icon(spell: &SpellDisplay) -> bool {
    spell.is_melee_auto_attack() || spell.ranged_icon_substitution()
}

/// The weapon icon `spell` shows on the bar; `None` shows the spell's own.
pub(super) fn auto_attack_icon(
    spell: &SpellDisplay,
    store: Option<&ObjectStore>,
    forms: &std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>,
    objects: &Objects,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Option<String> {
    let store = store?;
    if spell.is_melee_auto_attack() {
        return Some(melee_auto_attack_icon(
            store, forms, objects, items, icons, commands,
        ));
    }
    if spell.ranged_icon_substitution() {
        return ranged_weapon_icon(store, objects, items, icons, commands);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use benilla_formats::{ItemDisplay, ItemDisplayCatalog, ShapeshiftForm};
    use benilla_protocol::messages::{ItemInfo, ObjectFields};

    use super::*;
    use crate::items::TestDeps;

    /// Raw indices: `PLAYER_FIELD_INV_SLOT_HEAD + 2×15` (the main hand), `UNIT_FIELD_FLAGS`, and
    /// `UNIT_FIELD_BYTES_1`, whose third byte is the form.
    const INV_SLOT_MAINHAND: u16 = 486 + 2 * 15;
    const UNIT_FLAGS: u16 = 46;
    const UNIT_BYTES_1: u16 = 138;
    /// `UNIT_FLAG_DISARMED`.
    const DISARMED: u32 = 0x0020_0000;
    const SWORD_ICON: &str = "Interface\\Icons\\INV_Sword_04";
    const BEAR_ICON: &str = "Interface\\Icons\\Ability_Racial_BearForm";

    /// One resolve. `hand` is the main-hand item's class (`None` for empty), always with the
    /// sword's display, so only the law moves the answer.
    fn icon(flags: u32, hand: Option<u32>, form: u8) -> String {
        let mut deps = TestDeps::new();
        let mut pairs = vec![(UNIT_FLAGS, flags), (UNIT_BYTES_1, u32::from(form) << 16)];
        if let Some(class) = hand {
            pairs.push((INV_SLOT_MAINHAND, 0x2a));
            deps.spawn_item(0x2a, ObjectFields::from_pairs(&[(3, 500)]));
            deps.items.insert_template(
                500,
                Some(ItemInfo {
                    class,
                    subclass: 7,
                    display_info_id: 950,
                    ..crate::items::test_template("Worn Shortsword")
                }),
            );
        }
        let store = ObjectStore(ObjectFields::from_pairs(&pairs));
        let icons =
            ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::from([(
                950u32,
                ItemDisplay {
                    icon: Some(SWORD_ICON.into()),
                    ..Default::default()
                },
            )])));
        // Form 1 carries an attack icon (a bear's swipe); every other form id has none.
        let forms = HashMap::from([(
            1u32,
            ShapeshiftForm {
                attack_icon: Some(BEAR_ICON.into()),
                ..Default::default()
            },
        )]);
        deps.with_objects(|objects, items, commands| {
            melee_auto_attack_icon(&store, &forms, objects, items, Some(&icons), commands)
        })
    }

    /// The disarmed guard (`0x4e68df`).
    #[test]
    fn a_disarmed_character_shows_spell_reset_though_armed() {
        assert_eq!(icon(0, Some(2), 0), SWORD_ICON);
        assert_eq!(icon(DISARMED, Some(2), 0), SPELL_RESET_ICON);
        // The class half: a disarmed non-weapon (class 4) keeps its own icon.
        assert_eq!(icon(DISARMED, Some(4), 0), SWORD_ICON);
        // An empty hand shows `Spell-Reset` with or without the flag.
        assert_eq!(icon(0, None, 0), SPELL_RESET_ICON);
        assert_eq!(icon(DISARMED, None, 0), SPELL_RESET_ICON);
    }

    /// `0x4e6870` tests the form before the disarmed guard.
    #[test]
    fn the_form_icon_outranks_the_disarmed_guard() {
        assert_eq!(icon(DISARMED, Some(2), 1), BEAR_ICON);
        assert_eq!(icon(0, Some(2), 1), BEAR_ICON, "and outranks the weapon");
        // A form with no attack icon falls through.
        assert_eq!(icon(DISARMED, Some(2), 2), SPELL_RESET_ICON);
        assert_eq!(icon(0, Some(2), 2), SWORD_ICON);
    }
}
