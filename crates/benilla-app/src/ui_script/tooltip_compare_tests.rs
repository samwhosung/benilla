//! The shopping-compare + chat-link tooltips over the REAL shipped XMLs (decision 0274 P4,
//! re-based by 2202, scoped back to the reference by 2210): a VENDOR row hover raises
//! `ShoppingTooltip1/2` through the stock `MerchantFrame.xml`, their armed `SetInventoryItem`
//! renders the byte law's compare shape over the template's own adopted small-font ladder, and
//! `SetItemRef` fills the parked `ItemRefTooltip`.
//!
//! **1.12.1 has no hover compare, and neither does benilla now.** `SHOW_COMPARE_TOOLTIP` (event
//! 377) has zero fire sites in 5875, so `PaperDollFrame.lua:621-640` is dead code there and the
//! vendor row (plus the auction row, 1971) is the only live consumer of the shopping plates
//! (`SetMerchantCompareItem 0x536080`). benilla fired that event until 2202 and then drove
//! the plates itself on a shift-held hover until 2210; both were supersets of a client that
//! compares only where its own FrameXML asks. The plates' geometry and lifetime belong to that
//! FrameXML — this engine seats none of them.
//!
//! **The hover sources are the REFERENCE's own code since 1751**, so these tests drive the MOUSE
//! ([`super::test_ui::hover`]) rather than calling handlers: `MerchantItemButton`'s OnEnter, and
//! stock `PaperDollItemSlotButton_OnEnter` (`PaperDollFrame.lua:739-764`) for the doll slots
//! (`CharacterHeadSlot` and kin — the names are unchanged; the mouse is what reaches them).

use benilla_ui::script::{
    ContainerSlot, ContainerState, InvSlotView, InventorySlots, ItemTemplateView, UiScript,
    UnitState,
};

use super::test_ui::{bag_slot_button, hover, BAG_UI, CHARACTER_UI};

/// The two shared lists overlap heavily — `GlobalStrings`, the fonts, `UIParent`, the tooltip,
/// `Cooldown`, the action bar and `Interface\FrameXML\PaperDollFrame.xml` are in both — and
/// loading one file twice redeclares its frames. So a list is walked once and anything already
/// loaded is skipped, which also keeps the ORDER the first list asked for.
fn load_once(s: &UiScript, seen: &mut Vec<&'static str>, files: &[&'static str]) {
    for f in files {
        if seen.contains(f) {
            continue;
        }
        seen.push(f);
        super::test_ui::load_ui_strict(s, f);
    }
}

/// The window set the compare flow crosses **without a bag window**: the whole character window
/// (the listener's doll slots), plus [`ROUTER_UI`].
///
/// **Needs client data.** It always did — `Interface\FrameXML\ItemRef.xml` has been a chain entry
/// since it was pointed there — but the comment here claimed "deliberately install-free" until
/// 1751 made the claim impossible to miss. Callers open with `wow_data_or_skip!`.
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

/// The two reference files this harness carries beyond the character window: `MerchantFrame.xml`,
/// the other window the compare flow crosses, and `ItemRef.xml`, which declares the chat-link
/// router's own `ItemRefTooltip`. Both were in this harness's hand-copied list before 1751; the
/// manifest's order is the one kept (`FrameXML.toc` 63 → 77).
const ROUTER_UI: [&str; 4] = [
    "ScrollTemplates.xml", // our scroll kit + the placeholder icon
    "Interface\\FrameXML\\CharacterFrameTemplates.xml",
    "Interface\\FrameXML\\MerchantFrame.xml",
    "Interface\\FrameXML\\ItemRef.xml",
];

/// A level-60 player carrying **both** halves of the race and class pairs: `UnitRace`/`UnitClass`
/// answer `(localized, file)` or `nil, nil` — the binding `zip`s them — and stock
/// `PaperDollFrame_SetLevel` formats all three into `CharacterLevelText` unguarded
/// (`PaperDollFrame.lua:100-104`) on every show of the character window.
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

/// [`harness`] with the REFERENCE's bag windows (1751) — the hover source. `BAG_UI` is the ordered
/// set a test needs before it can open one; `CHARACTER_UI` leads because the manifest seats the
/// character block above the containers (`FrameXML.toc` 53-58 → 65) and because it carries
/// `PaperDollFrame.xml`, which `MainMenuBarBagButtons` inherits its `BagSlotButtonTemplate` body
/// from.
///
/// **Needs client data**: both lists name chain entries, so callers open with `wow_data_or_skip!`.
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

/// An equipped helm in the head slot + a better helm in the backpack, both with templates —
/// the compare pair.
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

/// A chat item link through the ref router: `SetItemRef` shows the parked `ItemRefTooltip`
/// (BOTTOM +80, ANCHOR_PRESERVE keeps the XML seat) with the linked item's law; the corner
/// close button hides it.
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
    // The close button (ref ItemRefCloseButton): HideUIPanel drops it.
    s.run("HideUIPanel(ItemRefTooltip)").unwrap();
    assert!(
        !s.eval::<bool>("return ItemRefTooltip:IsShown()").unwrap(),
        "the close path hides the link tooltip"
    );
    assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
}

/// Hovering the EQUIPPED item itself (a paper-doll slot) renders the INSTANCE — the live
/// durability pair off the slot view, never the template's authored max/max (ref
/// PaperDollItemSlotButton_OnEnter l.741: `SetInventoryItem`, not an id/template render;
/// director-caught: broken gear read 100% in the char window while the bag read it right).
#[test]
fn doll_hover_renders_the_live_instance() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness_with_bags();
    s.set_unit("player", Some(player()));
    seed_items(&mut s);
    // Break the equipped helm: instance pair (0, 40); the template stays authored-full.
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

    // Arm a compare first through a BAG hover (the stale-arm hazard the doll hover must clear) —
    // the reference's own `ContainerFrameItemButton_OnEnter` since 1751, reached by moving the
    // mouse onto the button rather than by calling the handler.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    let btn = bag_slot_button(&s, 0, 1);
    hover(&mut s, &btn);
    // The doll slot needs its window open to be hoverable at all.
    s.run(r#"ToggleCharacter("PaperDollFrame")"#).unwrap();
    s.take_sounds();

    // The doll hover: the live pair, not the template's 40/40. Stock
    // `PaperDollItemSlotButton_OnEnter` reads `this`, so the mouse is what reaches it — and moving
    // OFF the bag button is part of what this test needs anyway.
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

/// **The compare 1.12.1 actually has**, end to end over the stock file: hovering a vendor row
/// raises `ShoppingTooltip1/2` against what is worn, headed by the gray `Currently Equipped`.
///
/// `MerchantFrame.xml`'s `MerchantItemButton` OnEnter is the whole law and this exercises it as
/// written — `SetMerchantCompareItem` as the predicate, `SetOwner(GameTooltip, "ANCHOR_NONE")`,
/// `TOPLEFT`/`GameTooltip` `TOPRIGHT` (0, −10), the same call again, `Show()`; plate 2 the same
/// off plate 1 at (0, 0). The engine supplies only the two bindings and the compare render; the
/// geometry, the second fill and both `Show`s are the reference's Lua. Two worn rings against a
/// ring on the shelf, so BOTH plates answer — the case that pins the offset walk.
///
/// And it pins the CONTENT the plate carries, because that is where we were wrong (2216): the
/// compare call sites pass p4 = 0, so a plate is the worn item's ordinary tooltip with one gray
/// line on top — an EPIC ring's name reads purple, not white, and its flavour text is not cut.
#[test]
fn shipped_merchant_row_raises_the_compare_plates() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // Two worn rings, a third on the shelf: offset 1 and 2 both find a candidate.
    let mut inv: InventorySlots = Default::default();
    // The worn ring under plate 1 is EPIC and carries flavour text — the two things the old
    // conflated compare flag took away.
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

    // The mouse, not the handler: `MerchantItemButton`'s OnEnter reads `this`.
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
    // p4 = 0 over the stock file: the plate is the FULL tooltip, in the worn item's own colours.
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
    // The plate wears the template's own small-font ladder (10px), the main tooltip its header face.
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
    // Leaving the row hides the tooltip, and the stock OnHide takes both plates with it.
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

/// An armour template in one `InventoryType` — the CLASS is what the selection law compares by
/// (the class test at `0x536262` in `SetMerchantCompareItem 0x536080`).
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
