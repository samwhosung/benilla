//! [`super::drain::item_action_route`] — the reference's two-stage equip-vs-use law for an ITEM
//! action. The inventory walk is stubbed, so these pin the *decision*, not the
//! search (`ui_items::find_item` has its own tests for the walk order).

use super::drain::{item_action_route, ItemRoute};
use crate::items::test_template;
use crate::ui_items::ItemSearch;

// `(bag_index, slot, instance guid)` — the triple the walk returns (the guid feeds the
// shared use fork).
const WORN_AT: (u8, u8, u64) = (255, 13, 0xE1); // the trinket-1 doll slot
const IN_BAG: (u8, u8, u64) = (255, 23, 0xB1); // the first backpack slot

/// A consumable (`InventoryType == 0`) is never equipped, wherever it is.
#[test]
fn a_consumable_always_uses() {
    let food = test_template("Tough Jerky"); // inventory_type 0
    assert_eq!(
        item_action_route(&food, |_| Some(IN_BAG)),
        ItemRoute::Use(IN_BAG)
    );
    assert_eq!(item_action_route(&food, |_| None), ItemRoute::Nowhere);
}

/// **The bug this closes.** An equippable item that IS worn uses in place — it does not
/// re-equip. A one-stage `equippable → equip` fork answers `Equip` here forever, and the old
/// bags-only walk answered `Nowhere`, which is what made the button inert.
#[test]
fn an_equipped_trinket_uses_in_place() {
    let mut trinket = test_template("Trinket");
    trinket.inventory_type = 12; // INVTYPE_TRINKET
    let route = item_action_route(&trinket, |s: ItemSearch| {
        // The doll stage finds it; so would the full walk, at the same position.
        s.equipment_only.then_some(WORN_AT).or(Some(WORN_AT))
    });
    assert_eq!(route, ItemRoute::Use(WORN_AT));
}

/// The same trinket sitting in a bag equips instead — and the *same button* then uses it,
/// which is exactly the two-stage behaviour the reference has.
#[test]
fn an_unworn_trinket_equips() {
    let mut trinket = test_template("Trinket");
    trinket.inventory_type = 12;
    let route = item_action_route(&trinket, |s: ItemSearch| {
        (!s.equipment_only).then_some(IN_BAG)
    });
    assert_eq!(route, ItemRoute::Equip(IN_BAG));
}

/// The charge filter is asked for **only** when the template says the item has finite
/// charges, and only on the use leg (`template+0x144 != 0 && != -1`).
#[test]
fn the_charge_filter_rides_only_a_charged_use() {
    let plain = test_template("Potion");
    item_action_route(&plain, |s: ItemSearch| {
        assert!(!s.live_charges_only, "an uncharged item never filters");
        Some(IN_BAG)
    });

    let mut charged = test_template("Wand of Five Uses");
    charged.spell_charges_0 = 5;
    item_action_route(&charged, |s: ItemSearch| {
        assert!(s.live_charges_only, "a charged item skips spent copies");
        Some(IN_BAG)
    });

    // -1 is the "unlimited" sentinel, not a finite count.
    let mut unlimited = test_template("Hearthstone");
    unlimited.spell_charges_0 = -1;
    item_action_route(&unlimited, |s: ItemSearch| {
        assert!(!s.live_charges_only, "-1 means unlimited, not finite");
        Some(IN_BAG)
    });
}
