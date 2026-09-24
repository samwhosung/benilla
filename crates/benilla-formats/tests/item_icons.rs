//! The `ItemDisplayInfo` icon column (5), pinned to the display ids vmangos gives known items.

use benilla_formats::{load_item_display_catalog, open_chain};

#[test]
fn item_icons_resolve_known_display_ids() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let catalog = load_item_display_catalog(&mut chain).expect("load item display catalog");
    assert!(
        catalog.len() > 29_000,
        "5875 ships 29604 display rows, got {}",
        catalog.len()
    );

    // Display ids from vmangos `item_template`.
    for (item, display_id, icon) in [
        ("Worn Shortsword", 1542, "Interface\\Icons\\INV_Sword_04"),
        ("Tough Jerky", 2473, "Interface\\Icons\\INV_Misc_Food_16"),
        (
            "Worn Wooden Shield",
            18730,
            "Interface\\Icons\\INV_Shield_09",
        ),
        ("Hearthstone", 6418, "Interface\\Icons\\INV_Misc_Rune_01"),
    ] {
        assert_eq!(
            catalog.get(display_id).and_then(|d| d.icon.as_deref()),
            Some(icon),
            "{item} ({display_id})"
        );
    }
}
