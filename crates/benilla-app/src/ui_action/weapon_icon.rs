//! **Weapon-icon substitution** — the handful of spells that show an *equipped weapon's* icon
//! instead of their own (`0x4e6870` melee, `0x4e6990` ranged).
//!
//! Two spells' worth of law, but it is character-level rather than spell-level: the melee
//! auto-attack borrows the main hand's icon (or `Spell-Reset` when unarmed), a ranged auto-repeat
//! shot borrows the ranged slot's (unless it is thrown, which keeps the spell's own face). Both
//! track the *equipped item*, which a weapon swap changes without ever touching the action table —
//! which is why [`super::feed`] refreshes these every frame rather than on the identity gate, and
//! why `ui_spellbook` pre-resolves them once per page.

use benilla_formats::SpellDisplay;

use crate::creature_anim::UNIT_FLAG_DISARMED;
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};

/// Equipment slot 15 = `EQUIPMENT_SLOT_MAINHAND` (vmangos `EquipmentSlots`).
const EQUIPMENT_SLOT_MAINHAND: u8 = 15;

/// Equipment slot 17 = `EQUIPMENT_SLOT_RANGED` — the ranged helper `0x4e6990`'s read
/// (`[ecx+0x88]`, `0x88 = 17×8`).
const EQUIPMENT_SLOT_RANGED: u8 = 17;

/// Weapon subclass 16 = thrown — the ranged icon helper's skip (`0x4e6990`'s `0x5d9f90 == 0x10`
/// test): a thrown weapon never substitutes its icon, so Throw keeps the spell's own face.
const ITEM_SUBCLASS_THROWN: u32 = 16;

/// The client's unarmed/disarmed auto-attack icon (the hardcoded string at `0x84bf58`) — what the
/// melee auto-attack shows when there is no main-hand weapon to borrow from, instead of spell
/// 6603's `Temp` placeholder.
const SPELL_RESET_ICON: &str = "Interface\\Buttons\\Spell-Reset";

/// `ItemClass` 2 — **WEAPON**: what the disarmed guard tests on the hand it just fetched
/// (`0x4e68df`'s `GetWeapon(0, 1)` result byte `== 2`). Disarm takes weapons, so a main hand
/// holding anything else keeps showing that item's own icon.
const ITEM_CLASS_WEAPON: u32 = 2;

/// The equipped main-hand item's `(item class, its inventory icon)` — slot 15 → the item's
/// `ItemDisplayInfo` icon, the chain the bags/paper doll use. `None` for an empty hand or an item
/// that hasn't streamed yet; the icon half alone is `None` when the display row or its texture
/// hasn't. Both halves come off ONE walk because the disarmed guard needs the class of the very
/// item whose icon would otherwise be shown.
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

/// The character's melee auto-attack icon (the client's melee helper `0x4e6870`).
/// The helper's four steps, in order:
///
/// 1. the **current shapeshift form's own attack face** when its `SpellShapeshiftForm` row carries
///    one (the `+0x34` AttackIconID read, `0x4e68af`–`0x4e68da` — a cat's paw, a bear's swipe;
///    closing decision 0231's deferred form case);
/// 2. the **disarmed guard** (`0x4e68df`) → [`SPELL_RESET_ICON`], weapon equipped or not
///    (closing 0231's other deferred case);
/// 3. the equipped main-hand weapon's icon;
/// 4. no main-hand item → [`SPELL_RESET_ICON`].
///
/// Character-level — independent of WHICH auto-attack spell (they all show this), so the
/// spellbook can pre-resolve it once for its whole page.
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
    // Precedence step 2 — the **disarmed guard** (`0x4e68df`: `test dword ptr [ecx+0xa0],
    // 0x200000`, then `GetWeapon(0, 1)` and a `== 2` on the returned class byte): while the
    // character is disarmed, a weapon in the main hand shows `Spell-Reset` exactly as an empty
    // hand does — the weapon is equipped and on screen, but the button reads unarmed, because
    // the swing it fires is (closing 0231's deferred case).
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

/// The equipped ranged weapon's inventory icon (slot 17 → `ItemDisplayInfo`), for the ranged
/// icon substitution (`0x4e6990`, decision 0231's deferred case): a **thrown** weapon is skipped
/// (the helper's `0x5d9f90 == 0x10` test), and `None` — missing weapon, thrown, or an unstreamed
/// item — falls back to the spell's OWN icon at the caller, never `Spell-Reset` (the helper's
/// `0x4e6a44` null return hands over to the normal SpellIconID path).
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

/// Whether `spell` substitutes an equipped weapon's icon at all — the two resolvers' shared
/// pre-test (melee: the effect trigger; ranged: the paired attribute bits). The per-frame icon
/// refresh keys on this, so a ranged-weapon swap re-feeds Auto Shot like a main-hand swap
/// re-feeds Attack.
pub(super) fn substitutes_weapon_icon(spell: &SpellDisplay) -> bool {
    spell.is_melee_auto_attack() || spell.ranged_icon_substitution()
}

/// The icon `spell` shows on the action bar when it substitutes an equipped weapon's
/// ([`substitutes_weapon_icon`]): the melee auto-attack shows [`melee_auto_attack_icon`]
/// (weapon or `Spell-Reset`); a ranged auto-repeat shot ([`SpellDisplay::ranged_icon_substitution`])
/// shows [`ranged_weapon_icon`]. `None` for any other spell, for a ranged shot with no
/// substitutable weapon, or when there is no character to read the weapon from — the caller uses
/// the spell's own icon.
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

    /// `PLAYER_FIELD_INV_SLOT_HEAD + 2×15` (the main hand's private item guid), `UNIT_FIELD_FLAGS`
    /// and `UNIT_FIELD_BYTES_1` (the form byte lives in its third byte) — raw wire indices, the
    /// constants being crate-private to benilla-protocol.
    const INV_SLOT_MAINHAND: u16 = 486 + 2 * 15;
    const UNIT_FLAGS: u16 = 46;
    const UNIT_BYTES_1: u16 = 138;
    /// `UNIT_FLAG_DISARMED`.
    const DISARMED: u32 = 0x0020_0000;
    const SWORD_ICON: &str = "Interface\\Icons\\INV_Sword_04";
    const BEAR_ICON: &str = "Interface\\Icons\\Ability_Racial_BearForm";

    /// One `melee_auto_attack_icon` resolve. `hand` is the item CLASS held in the main hand
    /// (`None` = empty), `flags` the descriptor's `UNIT_FIELD_FLAGS`, `form` the shapeshift byte.
    /// The item is always the same sword display, so any change in the answer is the law moving,
    /// not the fixture.
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

    /// **The disarmed guard on the Attack button** (`0x4e68df`, decision 1863 closing 0231's
    /// deferred case): a disarmed character's auto-attack shows `Spell-Reset` with the weapon
    /// still equipped — the same face an empty hand shows, because the swing it fires is the
    /// same bare-handed one.
    #[test]
    fn a_disarmed_character_shows_spell_reset_though_armed() {
        // CONTROL — armed, and the button wears the sword.
        assert_eq!(icon(0, Some(2), 0), SWORD_ICON);
        // The guard.
        assert_eq!(icon(DISARMED, Some(2), 0), SPELL_RESET_ICON);
        // Its `== 2` half (the recursed `GetWeapon(0, 1)` class test): Disarm takes weapons, so a
        // main hand holding anything else keeps showing that item.
        assert_eq!(icon(DISARMED, Some(4), 0), SWORD_ICON);
        // The step-4 fallback is the same string, and is NOT what the guard is being read from:
        // an empty hand shows `Spell-Reset` whether the flag is up or down.
        assert_eq!(icon(0, None, 0), SPELL_RESET_ICON);
        assert_eq!(icon(DISARMED, None, 0), SPELL_RESET_ICON);
    }

    /// The precedence the helper `0x4e6870` reads in: the **form** override is step 1 and the
    /// disarmed guard step 2, so a disarmed bear still swipes with its own paw.
    #[test]
    fn the_form_icon_outranks_the_disarmed_guard() {
        assert_eq!(icon(DISARMED, Some(2), 1), BEAR_ICON);
        assert_eq!(icon(0, Some(2), 1), BEAR_ICON, "and outranks the weapon");
        // A form with no attack icon of its own falls through to the rest of the ladder.
        assert_eq!(icon(DISARMED, Some(2), 2), SPELL_RESET_ICON);
        assert_eq!(icon(0, Some(2), 2), SWORD_ICON);
    }
}
