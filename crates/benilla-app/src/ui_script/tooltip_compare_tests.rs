//! The shopping-compare and chat-link tooltips over the stock files. 1.12.1 has no hover compare:
//! `SHOW_COMPARE_TOOLTIP` (event 377) has no fire site, so `PaperDollFrame.lua:621` is dead code
//! and the vendor and auction rows are the plates' only consumers (`SetMerchantCompareItem`,
//! `0x536080`). The stock hover handlers read `this`, so the tests move the mouse.

use benilla_ui::script::{
    ContainerSlot, ContainerState, InvSlotView, InventorySlots, ItemTemplateView, UiScript,
    UnitState,
};

use super::test_ui::{bag_slot_button, hover, BAG_UI, CHARACTER_UI};

/// Loads each file once across the overlapping lists, in first-seen order: loading a file twice
/// redeclares its frames.
fn load_once(s: &UiScript, seen: &mut Vec<&'static str>, files: &[&'static str]) {
    for f in files {
        if seen.contains(f) {
            continue;
        }
        seen.push(f);
        super::test_ui::load_ui_strict(s, f);
    }
}

/// The character window plus [`ROUTER_UI`], no bag window; needs client data.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let mut seen = Vec::new();
    load_once(&s, &mut seen, CHARACTER_UI);
    load_once(&s, &mut seen, &ROUTER_UI);
    s.set_money(0);
    s.set_unit("player", Some(player()));
    s
}

/// `MerchantFrame.xml`, then `ItemRef.xml` with its `ItemRefTooltip`, in `FrameXML.toc` order.
const ROUTER_UI: [&str; 4] = [
    "ScrollTemplates.xml", // our scroll kits
    "Interface\\FrameXML\\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\MerchantFrame.xml",
    "Interface\\FrameXML\\ItemRef.xml",
];

/// Race and class carry both halves: stock `PaperDollFrame_SetLevel` formats them unguarded on
/// every show of the character window (`PaperDollFrame.lua:101`).
fn player() -> UnitState {
    UnitState {
        exists: true,
        level: 60,
        race: Some("Human".into()),
        race_file: Some("Human".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        ..UnitState::default()
    }
}

/// [`harness`] with the stock bag windows. `CHARACTER_UI` leads, as in `FrameXML.toc`, because
/// `BagSlotButtonTemplate` inherits `PaperDollItemSlotButtonTemplate` from `PaperDollFrame.xml`.
fn harness_with_bags() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let mut seen = Vec::new();
    load_once(&s, &mut seen, CHARACTER_UI);
    load_once(&s, &mut seen, BAG_UI);
    load_once(&s, &mut seen, &ROUTER_UI);
    s.set_money(0);
    s
}

/// The compare pair: a helm in the head slot and a better one in the backpack.
fn seed_items(s: &mut UiScript) {
    let mut inv: InventorySlots = Default::default();
    inv[1] = Some(InvSlotView {
        duration_ms: None,
        already_bound: false,
        bar_placeable: true,
        durability: None,
        flags: 0,
        item_id: 1234,
        icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
        count: 1,
        contents_count: None,
        quality: 2,
        name: Some("Test Helm".into()),
        link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
        locked: false,
        equip_slots: vec![1],
        creator: None,
        enchants: Vec::new(),
    });
    s.set_inventory_slots(inv);
    s.set_item_template(
        1234,
        ItemTemplateView {
            name: "Test Helm".into(),
            quality: 2,
            class: 4,
            subclass: 1,
            inventory_type: 1,
            armor: 40,
            description: "Snug.".into(),
            ..Default::default()
        },
    );
    s.set_item_template(
        2000,
        ItemTemplateView {
            name: "Another Helm".into(),
            quality: 3,
            class: 4,
            subclass: 1,
            inventory_type: 1,
            armor: 55,
            ..Default::default()
        },
    );
    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Helmet_02".into()),
            count: 1,
            quality: Some(3),
            item_id: 2000,
            link: Some("|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r".into()),
            locked: false,
            equip_slots: vec![1],
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }),
    );
}

/// `SetItemRef` shows `ItemRefTooltip` with `ANCHOR_PRESERVE`, keeping its XML seat, `BOTTOM` +80.
#[test]
fn item_ref_tooltip_renders_a_chat_link() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    seed_items(&mut s);
    s.run(
        r#"SetItemRef("item:2000", "|cff0070dd|Hitem:2000:0:0:0|h[Another Helm]|h|r", "LeftButton")"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "link errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p, rel, rp, x, y = ItemRefTooltip:GetPoint() \
             return ItemRefTooltip:IsShown() \
               and ItemRefTooltipTextLeft1:GetText() == \"Another Helm\" \
               and p == \"BOTTOM\" and y == 80",
        )
        .unwrap();
    assert!(ok, "the link tooltip shows at its parked seat");
    // What `ItemRefCloseButton`'s `OnClick` runs.
    s.run("HideUIPanel(ItemRefTooltip)").unwrap();
    assert!(
        !s.eval::<bool>("return ItemRefTooltip:IsShown()").unwrap(),
        "the close path hides the link tooltip"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// A doll slot hovers through `SetInventoryItem` (`PaperDollFrame.lua:741`), so it shows the
/// instance's live durability, not the template's maximum.
#[test]
fn doll_hover_renders_the_live_instance() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness_with_bags();
    s.set_unit("player", Some(player()));
    seed_items(&mut s);
    // A broken helm: instance pair (0, 40), the template at full.
    let mut inv: InventorySlots = Default::default();
    inv[1] = Some(InvSlotView {
        duration_ms: None,
        already_bound: false,
        bar_placeable: true,
        durability: Some((0, 40)),
        flags: 0,
        item_id: 1234,
        icon: Some("Interface\\Icons\\INV_Helmet_01".into()),
        count: 1,
        contents_count: None,
        quality: 2,
        name: Some("Test Helm".into()),
        link: Some("|cff1eff00|Hitem:1234:0:0:0|h[Test Helm]|h|r".into()),
        locked: false,
        equip_slots: vec![1],
        creator: None,
        enchants: Vec::new(),
    });
    s.set_inventory_slots(inv);
    s.set_item_template(
        1234,
        ItemTemplateView {
            name: "Test Helm".into(),
            quality: 2,
            class: 4,
            subclass: 1,
            inventory_type: 1,
            armor: 40,
            max_durability: 40,
            ..Default::default()
        },
    );

    // A bag hover first arms a compare, which the doll hover must clear.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    let btn = bag_slot_button(&s, 0, 1);
    hover(&mut s, &btn);
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.take_sounds();

    hover(&mut s, "CharacterHeadSlot");
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let found: String = s
        .eval(
            "for i = 1, GameTooltip:NumLines() do \
               local t = getglobal(\"GameTooltipTextLeft\" .. i):GetText() \
               if t and string.find(t, \"Durability\") then return t end \
             end \
             return \"<none>\"",
        )
        .unwrap();
    assert_eq!(
        found, "Durability 0 / 40",
        "the doll hover carries the instance's live pair"
    );

    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// `MerchantItemButton`'s `OnEnter` (`MerchantFrame.xml:67`) seats plate 1 at the tooltip's
/// `TOPRIGHT` (0, -10) and plate 2 off plate 1. The compare call passes p4 = 0, so a plate is the
/// worn item's ordinary tooltip under a gray `Currently Equipped` line.
#[test]
fn shipped_merchant_row_raises_the_compare_plates() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Two worn rings, a third on the shelf: offset 1 and 2 both find a candidate.
    let mut inv: InventorySlots = Default::default();
    // The ring under plate 1 is epic with flavour text, both of which the plate keeps.
    for (slot, id, name, quality) in [
        (11usize, 7000u32, "Old Loop", 4u32),
        (12, 7001, "Older Loop", 1),
    ] {
        inv[slot] = Some(InvSlotView {
            item_id: id,
            count: 1,
            quality: quality as i32,
            name: Some(name.into()),
            ..Default::default()
        });
        s.set_item_template(
            id,
            ItemTemplateView {
                quality,
                description: "Round.".into(),
                ..armor_template(name, 11)
            },
        );
    }
    s.set_inventory_slots(inv);
    s.set_item_template(8000, armor_template("Shiny Loop", 11));
    s.set_merchant(Some(benilla_ui::script::MerchantState {
        items: vec![benilla_ui::script::MerchantItem {
            name: Some("Shiny Loop".into()),
            texture: Some("Interface\\Icons\\INV_Jewelry_Ring_03".into()),
            price: 100,
            quantity: 1,
            num_available: -1,
            item_id: 8000,
            max_stack: Some(1),
            ..Default::default()
        }],
        ..Default::default()
    }));
    s.fire_event("MERCHANT_SHOW", vec![]);
    s.take_sounds();

    hover(&mut s, "MerchantItem1ItemButton");
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    let ok: bool = s
        .eval(
            "local p1, r1, rp1, x1, y1 = ShoppingTooltip1:GetPoint() \
             local p2, r2, rp2, x2, y2 = ShoppingTooltip2:GetPoint() \
             return GameTooltipTextLeft1:GetText() == \"Shiny Loop\" \
               and ShoppingTooltip1:IsShown() and ShoppingTooltip2:IsShown() \
               and ShoppingTooltip1TextLeft1:GetText() == \"Currently Equipped\" \
               and ShoppingTooltip1TextLeft2:GetText() == \"Old Loop\" \
               and ShoppingTooltip2TextLeft2:GetText() == \"Older Loop\" \
               and p1 == \"TOPLEFT\" and r1:GetName() == \"GameTooltip\" \
               and rp1 == \"TOPRIGHT\" and x1 == 0 and y1 == -10 \
               and p2 == \"TOPLEFT\" and r2:GetName() == \"ShoppingTooltip1\" \
               and rp2 == \"TOPRIGHT\" and x2 == 0 and y2 == 0",
        )
        .unwrap();
    assert!(
        ok,
        "the vendor row raises both plates at the stock geometry"
    );
    let ok: bool = s
        .eval(
            "local r, g, b = ShoppingTooltip1TextLeft2:GetTextColor() \
             local flavour = nil \
             for i = 1, ShoppingTooltip1:NumLines() do \
               if getglobal(\"ShoppingTooltip1TextLeft\"..i):GetText() == \"\\\"Round.\\\"\" then flavour = i end \
             end \
             return flavour ~= nil and math.abs(r - 0.639) < 0.01 \
               and math.abs(g - 0.208) < 0.01 and math.abs(b - 0.933) < 0.01",
        )
        .unwrap();
    assert!(
        ok,
        "the epic ring's plate reads purple and keeps its flavour text — no compact cut"
    );
    // The plate's small-font ladder is 10 px; the main tooltip's header face is larger.
    let ok: bool = s
        .eval(
            "local _, sh = ShoppingTooltip1TextLeft1:GetFont() \
             local _, mh = GameTooltipTextLeft1:GetFont() \
             return sh == 10 and mh > sh",
        )
        .unwrap();
    assert!(
        ok,
        "the template's small-font ladder rides the compare plate"
    );
    hover(&mut s, "UIParent");
    let ok: bool = s
        .eval(
            "return not GameTooltip:IsShown() \
               and not ShoppingTooltip1:IsShown() and not ShoppingTooltip2:IsShown()",
        )
        .unwrap();
    assert!(ok, "the plates leave with the tooltip that owns them");
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// An armour template; `SetMerchantCompareItem` selects by item class (`0x536262`).
fn armor_template(name: &str, inventory_type: u32) -> ItemTemplateView {
    ItemTemplateView {
        name: name.into(),
        quality: 1,
        class: 4,
        subclass: 1,
        inventory_type,
        armor: 5,
        ..Default::default()
    }
}
