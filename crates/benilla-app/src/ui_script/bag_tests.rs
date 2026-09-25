use benilla_ui::script::{
    ContainerMove, ContainerSlot, ContainerState, ItemTemplateView, QuadContent, SoundRequest,
    UiScript,
};

use super::test_ui::{
    bag_slot_button, bag_window, centre_of, click, hover, load_ui as load_xml, unhover, BAG_UI,
};

/// Is bag `id` open? Asked of `IsBagOpen`: the twelve `ContainerFrame`s are recycled.
fn open(s: &UiScript, id: i64) -> bool {
    super::test_ui::bag_open(s, id)
}

/// The name of the frame showing bag `id`, which the caller has just opened.
fn window(s: &UiScript, id: i64) -> String {
    bag_window(s, id).unwrap_or_else(|| panic!("bag {id} is not open"))
}

/// The stock bag buttons are `MainMenuBarArtFrame`'s children, a level above the bar's art.
#[test]
fn bag_bar_icons_draw_above_the_action_bar_art() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // At this size the bag bar sits over the right end of the XP bar's notched art.
    s.set_screen_size(1600.0, 900.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    s.resolve();
    let quads = s.extract();

    // Occluded: a higher-z textured quad other than the slot's own ring covers the icon's centre.
    let occluded = quads
        .iter()
        .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("UI-PaperDoll-Slot-Bag")))
        .filter(|icon| {
            let r = icon.rect.expect("a resolved icon rect");
            let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
            quads.iter().any(|q| {
                q.z > icon.z
                    && matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if !p.contains("UI-Quickslot2"))
                    && q.rect.is_some_and(|qr| qr.left <= cx && cx <= qr.right && qr.bottom <= cy && cy <= qr.top)
            })
        })
        .count();
    assert_eq!(
        occluded, 0,
        "a bag-slot icon is painted over by the action-bar art (the seat-above-the-bar fix regressed)"
    );
}

/// Stock `ContainerFrame_OnShow` and `OnHide` play the sounds (`ContainerFrame.lua:140`, `:120`).
#[test]
fn backpack_toggle_plays_open_and_close_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    s.set_money(0);
    // `ToggleBag` opens nothing without slots (its `size > 0` guard).
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );

    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );
    assert!(!open(&s, 0));

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(open(&s, 0));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igBackPackOpen".into())],
        "opening the backpack plays igBackPackOpen"
    );

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(!open(&s, 0));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igBackPackClose".into())],
        "closing the backpack plays igBackPackClose"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `ToggleBackpack` (`ContainerFrame.lua:67-82`): with bag 0 shut it opens bag 0 alone, even
/// beside another open bag; with bag 0 open it hides every container window.
#[test]
fn b_opens_the_backpack_alone_and_closes_every_bag() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 0), "backpack opens");
    assert!(
        !open(&s, 2),
        "the equipped bag stays shut — this is the backpack's own toggle, not open-all"
    );
    assert!(
        !open(&s, 1) && !open(&s, 3) && !open(&s, 4),
        "and the empty slots have no window to show either way"
    );

    s.run("ToggleBag(2)").unwrap();
    assert!(open(&s, 2));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "bag 0 open ⇒ the toggle hides every container window"
    );

    // Bag 0 shut with bag 2 open is still the open arm: the condition reads bag 0.
    s.run("ToggleBag(2)").unwrap();
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "with only another bag open, B opens the backpack beside it"
    );

    s.run("CloseAllWindows()").unwrap();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "CloseAllWindows hides every bag window"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `OpenAllBags` (`ContainerFrame.lua:662-700`) toggles by a count: all open closes the lot,
/// anything less opens it, `forceOpen` always opens; the keyring is swept but never counted.
#[test]
fn shift_b_toggles_every_bag_at_once() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );

    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 0) && open(&s, 2));
    assert!(
        !open(&s, 1) && !open(&s, 3) && !open(&s, 4),
        "empty bag slots (no container) have no window to show"
    );
    assert!(
        !open(&s, -2),
        "the keyring is not one of your bags — open-all never opens it"
    );

    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(!open(&s, 0) && !open(&s, 2));

    // One of two open is still the open arm: the count decides.
    s.run("ToggleBag(0)").unwrap();
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "1 of 2 open is not 'all open': the lot opens"
    );

    // An open keyring is swept but not counted, so this reads two of two and closes.
    s.run("ToggleKeyRing()").unwrap();
    assert!(open(&s, -2));
    s.run("OpenAllBags()").unwrap();
    let _ = s.take_sounds();
    assert!(
        !open(&s, 0) && !open(&s, 2),
        "the keyring does not count as an open bag"
    );
    assert!(!open(&s, -2), "…but it IS swept by the same pass");

    // `forceOpen` skips the close arm.
    s.run("OpenAllBags(1)").unwrap();
    s.run("OpenAllBags(1)").unwrap();
    let _ = s.take_sounds();
    assert!(
        open(&s, 0) && open(&s, 2),
        "forceOpen means open, never toggle"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An open bag lights its bar button and any close clears it: `ContainerFrame_OnShow` and `OnHide`
/// write the ring (`ContainerFrame.lua:124-131`, `:84-95`), so it tracks every path.
#[test]
fn bag_bar_buttons_light_while_their_bag_is_open() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        2,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );
    let checked = |s: &mut UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}:GetChecked() and true or false"))
            .unwrap()
    };

    s.run("OpenAllBags()").unwrap();
    assert!(
        checked(&mut s, "MainMenuBarBackpackButton"),
        "backpack ring lights"
    );
    assert!(checked(&mut s, "CharacterBag1Slot"), "bag 2's ring lights");
    assert!(
        !checked(&mut s, "CharacterBag0Slot"),
        "empty slot stays dark"
    );

    // Exactly two rings emit; the backpack's covers its 37-wide button at the art frame's
    // BOTTOMRIGHT (-6, 2): x 981..1018, y 2..39 at this size.
    s.resolve();
    let rings: Vec<_> = s
        .extract()
        .into_iter()
        .filter(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("CheckButtonHilight"))
        })
        .collect();
    assert_eq!(rings.len(), 2, "toggle + bag 2 rings emit, nothing else");
    let toggle_ring = rings
        .iter()
        .find_map(|q| q.rect.filter(|r| r.left == 981.0))
        .expect("the toggle's ring at the art frame's corner");
    assert_eq!(
        (
            toggle_ring.left,
            toggle_ring.bottom,
            toggle_ring.right,
            toggle_ring.top
        ),
        (981.0, 2.0, 1018.0, 39.0)
    );

    s.run("CloseBag(2)").unwrap();
    assert!(
        !checked(&mut s, "CharacterBag1Slot"),
        "closing bag 2 clears its ring"
    );
    assert!(
        checked(&mut s, "MainMenuBarBackpackButton"),
        "the backpack ring stays"
    );

    // A right-click on the bar slot reopens bag 2: `PaperDollItemSlotButton_OnLoad` registers both
    // buttons (`PaperDollFrame.lua:86`) and `BagSlotButton_OnClick` reads neither.
    let (cx, cy): (f64, f64) = s
        .eval(
            "return (CharacterBag1Slot:GetLeft() + CharacterBag1Slot:GetRight()) / 2, \
                    (CharacterBag1Slot:GetTop() + CharacterBag1Slot:GetBottom()) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    assert!(
        checked(&mut s, "CharacterBag1Slot"),
        "a RIGHT-click on the bar slot reopens bag 2, exactly as a left one does"
    );

    s.run("CloseAllWindows()").unwrap();
    assert!(
        !checked(&mut s, "MainMenuBarBackpackButton") && !checked(&mut s, "CharacterBag1Slot"),
        "close-all clears every ring"
    );
    let _ = s.take_sounds();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A slot in the right half of the screen hangs its tooltip left
/// (`ContainerFrameItemButton_OnEnter`, `ContainerFrame.lua:602-612`).
#[test]
fn bag_tooltip_hangs_left_when_the_slot_sits_in_the_right_half() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The stock tooltip sizes from its lines, so reading its rect needs a text measurer.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_ThrowingKnife_02".into()),
            count: 200,
            quality: Some(1),
            item_id: 2947,
            link: Some("|cffffffff|Hitem:2947|h[Small Throwing Knife]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
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
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();

    assert_eq!(s.eval::<f64>("return GetScreenWidth()").unwrap(), 1024.0);
    // The bag anchors bottom-right, so every slot is in the right half.
    let btn = bag_slot_button(&s, 0, 1);
    let ok: bool = s
        .eval(&format!("return {btn}:GetRight() >= GetScreenWidth() / 2"))
        .unwrap();
    assert!(ok, "fixture: the slot must sit in the right half");

    hover(&mut s, &btn);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return GameTooltip:IsVisible()").unwrap());
    s.resolve();
    // `ANCHOR_LEFT` seats the tooltip's BOTTOMRIGHT on the slot's TOPLEFT.
    let ok: bool = s
        .eval(&format!(
            "return GameTooltip:GetRight() <= {btn}:GetLeft() \
               and GameTooltip:GetRight() <= GetScreenWidth()"
        ))
        .unwrap();
    assert!(ok, "tooltip hangs LEFT of a right-half slot");
}

/// Stock `ContainerFrameItemButton_OnUpdate` re-runs `OnEnter` while it owns the tooltip
/// (`ContainerFrame.lua:645-660`); hiding the tooltip drops ownership.
#[test]
fn hovered_bag_tooltip_fills_itself_when_the_stats_land() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Sword_04".into()),
            count: 1,
            quality: Some(1),
            item_id: 25,
            link: Some("|cffffffff|Hitem:25|h[Worn Shortsword]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
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
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.resolve();
    let btn = bag_slot_button(&s, 0, 1);

    hover(&mut s, &btn);
    assert!(s.eval::<bool>("return GameTooltip:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        1,
        "in-flight template: the name-only fallback line"
    );
    assert_eq!(s.take_item_stat_asks(), vec![25], "the miss asks the app");

    // The template lands; the next frame's `OnUpdate` repaints the open tooltip.
    s.set_item_template(
        25,
        ItemTemplateView {
            name: "Worn Shortsword".into(),
            quality: 1,
            inventory_type: 21,
            class: 2,
            subclass: 7,
            damages: vec![(1.0, 3.0, 0)],
            delay_ms: 1900,
            ..Default::default()
        },
    );
    s.tick(0.016);
    assert!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap() > 1,
        "the stats landing repainted the open tooltip"
    );
    let has_damage: bool = s
        .eval(
            "for i = 1, GameTooltip:NumLines() do \
               local fs = getglobal(\"GameTooltipTextLeft\" .. i) \
               if fs and string.find(fs:GetText() or \"\", \"Damage\") then return true end \
             end return false",
        )
        .unwrap();
    assert!(
        has_damage,
        "the repaint carries the stat head's damage line"
    );

    unhover(&mut s);
    s.tick(0.016);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "hide drops ownership; OnUpdate never resurrects"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// At a vendor a bag hover shows `SellPrice` times the stack (`0x52b650`, `0x52e376`) or the
/// `ITEM_UNSELLABLE` line, and arms the sell cursor (`ShowContainerSellCursor`, `0x4fa460`);
/// leaving resets it.
#[test]
fn vendor_bag_hover_shows_sell_price_and_arms_the_pouch_cursor() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::{MerchantState, ScriptValue, UiCursorMode};

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Pelt_Wolf_01".into()),
            count: 4,
            quality: Some(1),
            item_id: 2318,
            link: Some("|cffffffff|Hitem:2318|h[Light Leather]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Key_03".into()),
            count: 1,
            quality: Some(1),
            item_id: 9999,
            link: Some("|cffffffff|Hitem:9999|h[Shadowforge Key]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
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
    // Sellable stack: 13c each × 4 = 52c. Unsellable: SellPrice 0.
    s.set_item_template(
        2318,
        ItemTemplateView {
            name: "Light Leather".into(),
            quality: 1,
            sell_price: 13,
            ..Default::default()
        },
    );
    s.set_item_template(
        9999,
        ItemTemplateView {
            name: "Shadowforge Key".into(),
            quality: 1,
            sell_price: 0,
            ..Default::default()
        },
    );
    s.set_merchant(Some(MerchantState::default()));
    // `MERCHANT_SHOW` opens the bags (the window's OnShow calls `OpenBackpack`); a toggle here
    // would take `ToggleBackpack`'s close-all arm.
    s.fire_event("MERCHANT_SHOW", vec![ScriptValue::Str("Vendor".into())]);
    s.take_sounds();
    s.resolve();
    let b1 = bag_slot_button(&s, 0, 1);
    let b2 = bag_slot_button(&s, 0, 2);

    hover(&mut s, &b1);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>(
            "return GameTooltipMoneyFrame:IsShown() \
             and GameTooltipMoneyFrameCopperButton:GetText() == '52'",
        )
        .unwrap());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Buy),
        "the pouch cursor is armed over a sellable item"
    );

    unhover(&mut s);
    assert_eq!(s.ui_cursor(), None, "ResetCursor on leave");
    assert!(s
        .eval::<bool>("return not GameTooltipMoneyFrame:IsShown()")
        .unwrap());

    hover(&mut s, &b2);
    let has_line: bool = s
        .eval(
            "for i = 1, GameTooltip:NumLines() do \
               if (getglobal('GameTooltipTextLeft' .. i):GetText() or '') == 'No sell price' \
                 then return true end \
             end return false",
        )
        .unwrap();
    assert!(has_line, "SellPrice 0 shows the ITEM_UNSELLABLE line");
    assert!(s
        .eval::<bool>("return not GameTooltipMoneyFrame:IsShown()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A readable bag item (a letter's permanent copy) shows the inspect cursor on hover
/// (`ContainerFrame.lua:638`, `this.readable`); a plain item does not.
#[test]
fn readable_letter_hover_shows_the_inspect_magnifier() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::UiCursorMode;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Note_01".into()),
            count: 1,
            quality: Some(1),
            item_id: 8383,
            link: Some("|cffffffff|Hitem:8383|h[Plain Letter]|h|r".into()),
            readable: true,
            ..Default::default()
        },
    );
    slots.insert(
        2,
        ContainerSlot {
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            ..Default::default()
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
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();
    let b1 = bag_slot_button(&s, 0, 1);
    let b2 = bag_slot_button(&s, 0, 2);

    hover(&mut s, &b1);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert_eq!(
        s.ui_cursor(),
        Some(UiCursorMode::Inspect),
        "the magnifier over the letter"
    );
    unhover(&mut s);
    assert_eq!(s.ui_cursor(), None, "ResetCursor on leave");

    hover(&mut s, &b2);
    assert_eq!(s.ui_cursor(), None, "no magnifier over the jerky");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Real input, so the template's `RegisterForDrag`/`OnDragStart`/`OnReceiveDrag` wiring runs.
#[test]
fn drag_across_two_slots_queues_the_same_move_a_click_pickup_would() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(3),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
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
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.take_sounds();
    s.resolve();

    let b1 = bag_slot_button(&s, 0, 1);
    let b5 = bag_slot_button(&s, 0, 5);
    let (x1, y1): (f64, f64) = s
        .eval(&format!(
            "return ({b1}:GetLeft() + {b1}:GetRight()) / 2, \
                        ({b1}:GetTop() + {b1}:GetBottom()) / 2"
        ))
        .unwrap();
    let (x5, y5): (f64, f64) = s
        .eval(&format!(
            "return ({b5}:GetLeft() + {b5}:GetRight()) / 2, \
                        ({b5}:GetTop() + {b5}:GetBottom()) / 2"
        ))
        .unwrap();

    s.mouse_button(x1 as f32, y1 as f32, "LeftButton", true);
    s.mouse_move(x5 as f32, y5 as f32);
    let consumed = s.mouse_button(x5 as f32, y5 as f32, "LeftButton", false);
    assert!(consumed, "the drag release lands on a mouse-enabled frame");

    assert!(s.cursor_item().is_none(), "placed onto the empty slot 5");
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 5,
            count: None,
        }]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn a_second_bag_window_feeds_and_paints_via_the_bag_bar() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
            count: 1,
            quality: Some(2),
            item_id: 200,
            link: Some("|cffffffff|Hitem:200|h[Shiny Gem]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
            cooldown: None,
            readable: false,
            creator: None,
            flags: 0,
            enchants: Vec::new(),
        },
    );
    s.set_container(
        1,
        Some(ContainerState {
            name: Some("Small Pouch".into()),
            num_slots: 6,
            slots,
        }),
    );

    assert!(!open(&s, 1), "hidden by default");
    // `CharacterBag0Slot` is bag 1: the stock handlers compute
    // `this:GetID() - CharacterBag0Slot:GetID() + 1`.
    s.run("CharacterBag0Slot:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(open(&s, 1), "the bag-bar click opened bag 1's window");
    let w = window(&s, 1);
    assert_eq!(
        s.eval::<String>(&format!("return {w}Name:GetText()"))
            .unwrap(),
        "Small Pouch",
        "the title reads the live GetBagName"
    );

    s.resolve();
    let painted = s.extract().iter().any(|q| {
        matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("INV_Misc_Gem_01"))
    });
    assert!(painted, "bag 1's slot 1 icon is on screen");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An equipped bag's window height follows stock `ContainerFrame_GenerateFrame`:
/// `top + ((rows - 1) * 41 - 9) + 10`, where `top` is 72 for a size % 4 == 2 bag, 86 for a single
/// full row and 94 otherwise, and a one-row bag drops the middle term.
#[test]
fn equipped_bag_window_snug_fits_its_row_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    // (bag, slots, height): 6 is 72+32+10, 8 is 94+32+10, 10 is 72+73+10, 20 is 94+155+10, and the
    // one-row 4 and 2 are 86+10 and 72+10. Bag 1 stays at 6: the last assertion reads it.
    for (bag, size, expected) in [
        (1, 6, 114.0),
        (2, 8, 136.0),
        (3, 10, 155.0),
        (4, 20, 259.0),
        (2, 4, 96.0),
        (3, 2, 82.0),
    ] {
        s.set_container(
            bag,
            Some(ContainerState {
                name: Some(format!("Bag {bag}")),
                num_slots: size,
                slots: std::collections::HashMap::new(),
            }),
        );
        // The window is sized at generation, so each resize reopens it.
        if open(&s, bag) {
            s.run(&format!("ToggleBag({bag})")).unwrap();
        }
        s.run(&format!("ToggleBag({bag})")).unwrap();
        let _ = s.take_sounds();
        let frame = window(&s, bag);
        let h = s
            .eval::<f64>(&format!("return {frame}:GetHeight()"))
            .unwrap();
        assert!(
            (h - expected).abs() < 0.5,
            "bag {bag} ({size} slots): height {h}, expected {expected}"
        );
        s.run(&format!("ToggleBag({bag})")).unwrap();
        let _ = s.take_sounds();
    }
    s.run("ToggleBag(1)").unwrap();
    let _ = s.take_sounds();
    let w1 = window(&s, 1);
    let h6 = s.eval::<f64>(&format!("return {w1}:GetHeight()")).unwrap();
    assert!(
        h6 < 200.0,
        "a 6-slot bag must not fill the old 260 slab, got {h6}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Open a backpack holding a five-stack in slot 1; returns that slot button's centre.
fn open_backpack_with_a_five_stack(s: &mut UiScript) -> (f32, f32) {
    for file in BAG_UI {
        load_xml(s, file);
    }
    load_xml(s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(s, "Interface\\FrameXML\\StackSplitFrame.xml");
    s.set_money(0);

    let mut slots = std::collections::HashMap::new();
    slots.insert(
        1,
        ContainerSlot {
            duration_ms: None,
            petition: None,
            already_bound: false,
            bar_placeable: true,
            durability: None,
            texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
            count: 5,
            quality: Some(1),
            item_id: 117,
            link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
            locked: false,
            equip_slots: Vec::new(),
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
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    s.resolve();

    let btn = bag_slot_button(s, 0, 1);
    centre_of(s, &btn)
}

/// The shift fork (`ContainerFrame.lua:567-578`) opens the split frame without a pickup.
#[test]
fn shift_click_on_a_stack_opens_the_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    assert!(!s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "shift-click opened the split frame"
    );
    assert!(
        s.cursor_item().is_none(),
        "the shift fork never picks the stack up"
    );
    assert_eq!(s.eval::<i64>("return StackSplitFrame.maxStack").unwrap(), 5);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The split dialog's plate fills the 172×96 frame: stock `StackSplitFrame.xml` sizes it 256×32
/// with no anchors, and an anchor-less region gets an implicit `SetAllPoints` at creation, its size
/// unread; the TexCoords crop 172×96 out of the 256×128 art.
#[test]
fn the_split_frame_plate_fills_the_dialog() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();

    let (left, right, top, bottom) = s
        .eval::<(f64, f64, f64, f64)>(
            "return StackSplitFrame:GetLeft(), StackSplitFrame:GetRight(), \
                    StackSplitFrame:GetTop(), StackSplitFrame:GetBottom()",
        )
        .unwrap();
    assert!(
        (right - left - 172.0).abs() < 0.5 && (top - bottom - 96.0).abs() < 0.5,
        "the dialog frame is 172×96, got {}×{}",
        right - left,
        top - bottom
    );
    let plate = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.contains("UI-MoneyFrame"))
        })
        .expect("the plate is on screen");
    let r = plate.rect.expect("the plate has a rect");
    for (edge, got, want) in [
        ("left", r.left, left as f32),
        ("right", r.right, right as f32),
        ("top", r.top, top as f32),
        ("bottom", r.bottom, bottom as f32),
    ] {
        assert!(
            (got - want).abs() < 0.5,
            "plate {edge} = {got}, frame {edge} = {want} — the plate must fill the dialog"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The split dialog raises over a same-stratum window shown before it, as an addon bag window is
/// (`frameStrata="HIGH"`, the dialog's own): stock `toplevel="true"` makes its `Show` raise it.
#[test]
fn the_split_frame_raises_over_a_same_stratum_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.run(
        r#"
        local w = CreateFrame("Frame", "FakeBagnon", UIParent)
        w:SetFrameStrata("HIGH")
        w:SetPoint("BOTTOMLEFT", 0, 0)
        w:SetWidth(1024); w:SetHeight(768)
        local bg = w:CreateTexture(nil, "BACKGROUND")
        bg:SetTexture("Interface\\FakeBagnonBG")
        bg:SetAllPoints()
        w:Show()
    "#,
    )
    .unwrap();
    s.resolve();

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();

    assert!(
        s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "the spinner opened"
    );
    let (dialog, bagnon) = s
        .eval::<(i64, i64)>("return StackSplitFrame:GetFrameLevel(), FakeBagnon:GetFrameLevel()")
        .unwrap();
    assert!(
        dialog > bagnon,
        "Show must raise the toplevel dialog over the same-stratum window \
         (dialog level {dialog}, window level {bagnon})"
    );
    let order: Vec<String> = s
        .extract()
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. }
                if p.contains("FakeBagnonBG") || p.contains("UI-MoneyFrame") =>
            {
                Some(p.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(order.len(), 2, "both textures on screen: {order:?}");
    assert!(
        order[0].contains("FakeBagnonBG"),
        "the plate must draw over the window, not under it: {order:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Digits reach the dialog's `OnChar` (`enableKeyboard`): the first replaces the seeded 1, later
/// ones append unless they would exceed the stack, and ENTER commits as Okay does.
#[test]
fn typing_a_number_into_the_split_spinner_sets_the_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);

    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        1,
        "opens seeded at 1"
    );

    // The dialog consumes the digit, so it does not also fire action button 3.
    assert!(s.char_input("3"), "the spinner consumed the digit");
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        3,
        "the first digit REPLACES the seed rather than appending to it"
    );
    assert_eq!(
        s.eval::<String>("return StackSplitText:GetText()").unwrap(),
        "3",
        "and the label follows"
    );

    // A digit that would exceed the stack is ignored, not clamped: stock `StackSplitFrame_OnChar`
    // writes `this.split` only when `split <= this.maxStack` (`StackSplitFrame.lua:72`).
    assert!(s.char_input("7"));
    assert_eq!(
        s.eval::<i64>("return StackSplitFrame.split").unwrap(),
        3,
        "37 against a 5-stack keeps the 3 — the ref ignores the digit, it does not clamp"
    );
    assert!(s.frame_key_input("BACKSPACE"), "the dialog took BACKSPACE");
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 1);

    assert!(s.char_input("4"));
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 4);
    assert!(s.key_input("ENTER"), "ENTER is consumed by the dialog");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "ENTER commits and closes, like Okay"
    );
    let held = s.cursor_item().expect("ENTER committed the split");
    assert_eq!(held.count, Some(4), "the typed count is what got picked up");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Okay only picks the split up (`SplitContainerItem` is a pickup); the next placement queues the
/// move with the split count.
#[test]
fn split_okay_then_a_placement_queues_the_split_move() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    click(&mut s, "StackSplitRightButton", "LeftButton");
    click(&mut s, "StackSplitRightButton", "LeftButton");
    assert_eq!(s.eval::<i64>("return StackSplitFrame.split").unwrap(), 3);
    click(&mut s, "StackSplitOkayButton", "LeftButton");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "Okay hides the spinner"
    );
    let held = s.cursor_item().expect("Okay picked up the split carry");
    assert_eq!((held.bag, held.slot, held.count), (0, 1, Some(3)));
    assert!(
        s.take_container_moves().is_empty(),
        "no move yet — only a pickup"
    );

    let b5 = bag_slot_button(&s, 0, 5);
    click(&mut s, &b5, "LeftButton");
    assert!(s.cursor_item().is_none());
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 1,
            dst_bag: 0,
            dst_slot: 5,
            count: Some(3),
        }]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A plain click hides an open split frame (`ContainerFrame.lua:581`), even on an unrelated slot.
#[test]
fn a_plain_click_hides_an_open_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let (x, y) = open_backpack_with_a_five_stack(&mut s);
    s.set_modifiers(true, false, false);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    let b9 = bag_slot_button(&s, 0, 9);
    click(&mut s, &b9, "LeftButton");
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "the plain click on an unrelated slot hid the spinner"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `ContainerFrame_UpdateCooldown` reads `GetContainerItemCooldown` into
/// `CooldownFrame_SetTimer`; a `BAG_UPDATE_COOLDOWN` with the cooldown gone hides the sweep.
#[test]
fn bag_slot_cooldown_sweeps_through_the_xml() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.tick(100.0); // a nonzero clock epoch

    let potion = |cooldown| ContainerSlot {
        durability: None,
        texture: Some("Interface\\Icons\\INV_Potion_49".into()),
        count: 3,
        quality: Some(1),
        item_id: 118,
        cooldown,
        ..Default::default()
    };
    let backpack = |cooldown| {
        let mut slots = std::collections::HashMap::new();
        slots.insert(1, potion(cooldown));
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    };
    // 12 s left of the potion's 60 s: started at `GetTime` 52 (an absolute-start triple).
    s.set_container(0, Some(backpack(Some((52_000, 60_000, true)))));
    s.run("OpenAllBags()").unwrap();
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(0)]);
    // The next paint's `OnUpdateModel` scrubs the pane's sequence 0 to the elapsed fraction.
    super::test_ui::cooldown_facts(&mut s);
    s.tick(0.0);
    s.resolve();

    let sweep = |s: &UiScript| super::test_ui::cooldown_play_any(s);
    let play = sweep(&s).expect("the bag slot sweeps");
    assert_eq!(play, (0, 800), "48 of 60 s elapsed: sequence 0 at 800 ms");

    s.set_container(0, Some(backpack(None)));
    s.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
    assert_eq!(sweep(&s), None, "cold again after the refresh");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock plain `SetText` tooltips (`MainMenuBarBagButtons.xml:91-99`, `.lua:86-96`) with the
/// binding key appended; `TOGGLEBAG` numbers the slots right to left, so the one beside the
/// backpack is `TOGGLEBAG4` (F11 by default).
#[test]
fn the_bar_bag_buttons_name_themselves_on_hover() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    // `GetBindingKey` reads the registered command set, seeded as the app seeds it.
    s.register_bindings(&crate::bindings::registry_commands());
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.resolve();

    hover(&mut s, "MainMenuBarBackpackButton");
    let line = s
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert!(
        line.starts_with("Backpack") && line.contains("(B)"),
        "the backpack names itself and its TOGGLEBACKPACK key: {line:?}"
    );
    assert!(
        s.eval::<bool>("return GameTooltip.default == nil").unwrap(),
        "a bag button's plate is owner-anchored, never the default corner"
    );
    assert!(
        s.eval::<bool>("return GameTooltip:IsOwned(MainMenuBarBackpackButton)")
            .unwrap(),
        "…owned by the button it opened from"
    );

    hover(&mut s, "CharacterBag0Slot");
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Equip Container",
        "an empty slot says what belongs in it"
    );

    // An equipped bag shows its own item tooltip (the `SetInventoryItem` arm); bar slot 1 is
    // inventory slot 20.
    let mut inv: benilla_ui::script::InventorySlots = Default::default();
    inv[20] = Some(benilla_ui::script::InvSlotView {
        duration_ms: None,
        already_bound: false,
        bar_placeable: true,
        durability: None,
        flags: 0,
        item_id: 4496,
        icon: Some("Interface\\Icons\\INV_Misc_Bag_08".into()),
        count: 1,
        contents_count: Some(0), // an equipped bag, whose subclass row carries no count gate
        quality: 1,
        name: Some("Small Brown Pouch".into()),
        link: Some("|cffffffff|Hitem:4496:0:0:0|h[Small Brown Pouch]|h|r".into()),
        locked: false,
        equip_slots: vec![20],
        creator: None,
        enchants: Vec::new(),
    });
    s.set_inventory_slots(inv);
    hover(&mut s, "CharacterBag0Slot");
    let line = s
        .eval::<String>("return GameTooltipTextLeft1:GetText()")
        .unwrap();
    assert!(
        line.starts_with("Small Brown Pouch"),
        "an equipped slot shows that bag, not the empty-slot fallback: {line:?}"
    );
    assert!(
        line.contains("(F11)"),
        "…and the reference appends this slot's own TOGGLEBAG4 key: {line:?}"
    );

    unhover(&mut s);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "leaving hides the plate"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A keyring container (id -2) fed as the app feeds it, `size` being what `keyring_size(level)`
/// gives, with a key in slot 1 when `occupied`.
fn keyring(size: u32, occupied: bool) -> ContainerState {
    let mut slots = std::collections::HashMap::new();
    if occupied {
        slots.insert(
            1,
            ContainerSlot {
                item_id: 7146,
                count: 1,
                quality: Some(1),
                texture: Some("Interface\\Icons\\INV_Misc_Key_07".into()),
                link: Some("|cffffffff|Hitem:7146:0:0:0|h[The Scarlet Key]|h|r".into()),
                ..Default::default()
            },
        );
    }
    ContainerState {
        name: Some("Keyring".into()),
        num_slots: size,
        slots,
    }
}

/// Seat a player at `level`: stock `GetKeyRingSize` (`ContainerFrame.lua:773-786`: 4, then 8 at
/// 40, 12 at 50, 16 above 60) reads `UnitLevel("player")`, so a fixture left at 0 gets 4 slots.
fn seat_player_at_level(s: &mut UiScript, level: u32) {
    let mut player = benilla_ui::script::UnitState {
        level,
        ..Default::default()
    };
    player.name = Some("Tester".into());
    s.set_unit("player", Some(player));
}

/// How many drawn quads carry a texture path containing `needle`.
fn drawn_with(s: &mut UiScript, needle: &str) -> usize {
    s.resolve();
    s.extract()
        .iter()
        .filter(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains(needle)))
        .count()
}

/// The bar and bag files the keyring spans, plus `UIErrorsFrame.xml`: `PutKeyInKeyRing` reports a
/// full ring through `UIErrorsFrame`.
fn keyring_surface(s: &UiScript) {
    for file in BAG_UI {
        load_xml(s, file);
    }
    load_xml(s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(s, "Interface\\FrameXML\\UIErrorsFrame.xml");
}

/// The first key shows the keyring button, swaps the bar's two right-hand strips to the keyring
/// plate and slides the performance meter from -227 to -235 (`MainMenuBar_UpdateKeyRing`,
/// `MainMenuBar.lua:174-183`).
#[test]
fn the_first_key_puts_the_keyring_on_the_bar() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);

    s.set_has_key(false);
    s.run("MainMenuBar_UpdateKeyRing()").unwrap();
    assert!(
        !s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "no key ⇒ no keyring button"
    );
    assert_eq!(
        drawn_with(&mut s, "UI-MainMenuBar-KeyRing"),
        0,
        "no key ⇒ every bar strip wears the ordinary dwarf plate"
    );

    // A key lands: `HasKey`, then `BAG_UPDATE` to `MainMenuBarArtFrame`'s `OnEvent`, as at runtime.
    s.set_has_key(true);
    s.set_container(-2, Some(keyring(8, true)));
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    assert!(
        s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "the first key reveals the keyring button"
    );
    assert_eq!(
        drawn_with(&mut s, "UI-MainMenuBar-KeyRing"),
        2,
        "exactly the two RIGHT-hand strips swap to the keyring plate"
    );
    for (strip, top, bottom) in [
        ("MainMenuBarTexture2", 0.6640625, 1.0),
        ("MainMenuBarTexture3", 0.1640625, 0.5),
    ] {
        let (t, b) = s
            .eval::<(f64, f64)>(&format!(
                "local _, top, _, bottom = {strip}:GetTexCoord() return top, bottom"
            ))
            .unwrap();
        assert!(
            (t - top).abs() < 1e-9 && (b - bottom).abs() < 1e-9,
            "{strip} keeps the reference's own band — got {t}..{b}, want {top}..{bottom}"
        );
    }
    // The stock bar puts `KeyRingButton`'s left edge 234 from the bar's right (backpack at -6,
    // 37-wide buttons, -5 gaps), into the socket painted on the keyring plate.
    let bar_right = s.eval::<f64>("return MainMenuBar:GetRight()").unwrap();
    let button_left = s.eval::<f64>("return KeyRingButton:GetLeft()").unwrap();
    assert!(
        (bar_right - button_left - 235.0).abs() < 1.5,
        "the keyring button must land in the plate's painted socket — the ref's own -234, got {}",
        bar_right - button_left
    );

    // The performance meter clears the new socket (`MainMenuBar.lua:180`: -227 to -235).
    let perf_right = s
        .eval::<f64>("return MainMenuBarPerformanceBarFrame:GetRight()")
        .unwrap();
    let bar_right = s.eval::<f64>("return MainMenuBar:GetRight()").unwrap();
    assert!(
        (bar_right - perf_right - 235.0).abs() < 0.01,
        "the meter slid to -235 with the keyring up (got {})",
        bar_right - perf_right
    );

    // The latch is one-way: `MainMenuBar_UpdateKeyRing` only ever shows the button, and
    // `SHOW_KEYRING` is a saved variable (`MainMenuBar.lua:174-183`, `MainMenuBar.xml:323-336`).
    s.set_has_key(false);
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    assert!(
        s.eval::<bool>("return KeyRingButton:IsShown()").unwrap(),
        "losing the last key leaves the button on the bar — the reference's one-way latch"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The keyring button opens a container titled "Keyring", stitched from the `-Keyring` plate, with
/// the level-gated slot count and its own sounds (`ContainerFrame.lua:116-138`).
#[test]
fn the_keyring_button_opens_a_keyring_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);
    s.set_has_key(true);
    // Level 44: 8 slots (the 40..49 rung). The level decides; the feed carries the same 8.
    seat_player_at_level(&mut s, 44);
    s.set_container(-2, Some(keyring(8, true)));
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    let _ = s.take_sounds();

    assert!(!open(&s, -2));
    s.run("KeyRingButton:Click()").unwrap();
    assert!(open(&s, -2), "the button opens the keyring");
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("KeyRingOpen".into())],
        "the keyring has its own open kit, not the backpack's"
    );
    let w = window(&s, -2);
    assert_eq!(
        s.eval::<String>(&format!("return {w}Name:GetText()"))
            .unwrap(),
        "Keyring",
        "titled from the KEYRING string, not a bag item's name"
    );
    assert!(
        drawn_with(&mut s, "UI-Bag-Components-Keyring") > 0,
        "stitched from the keyring plate, not the ordinary bag sheet"
    );
    let shown = s
        .eval::<i64>(&format!(
            "local n = 0 for i = 1, MAX_CONTAINER_ITEMS do \
             if getglobal('{w}Item' .. i):IsShown() then n = n + 1 end end return n"
        ))
        .unwrap();
    assert_eq!(shown, 8, "only the level-unlocked keyring slots are drawn");
    assert_eq!(
        s.eval::<i64>("return BenillaGetContainerItemID(KEYRING_CONTAINER, 1)")
            .unwrap(),
        7146
    );

    s.run("KeyRingButton:Click()").unwrap();
    assert!(!open(&s, -2));
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("KeyRingClose".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Dropping a held key on the button files it in the first free keyring slot (stock
/// `PutKeyInKeyRing`); a full keyring refuses the drop.
#[test]
fn a_key_dropped_on_the_button_files_itself() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    keyring_surface(&s);
    s.set_money(0);
    s.set_has_key(true);

    // A 4-slot keyring with slot 1 taken, and a key picked out of the backpack.
    s.set_container(-2, Some(keyring(4, true)));
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::from([(
                3,
                ContainerSlot {
                    item_id: 7146,
                    count: 1,
                    ..Default::default()
                },
            )]),
        }),
    );
    s.fire_event("BAG_UPDATE", vec![benilla_ui::script::ScriptValue::Int(-2)]);
    s.run("PickupContainerItem(0, 3)").unwrap();
    assert!(s.eval::<bool>("return CursorHasItem()").unwrap());

    s.run("KeyRingButton:Click()").unwrap();
    assert_eq!(
        s.take_container_moves(),
        vec![ContainerMove {
            src_bag: 0,
            src_slot: 3,
            dst_bag: -2,
            dst_slot: 2,
            count: None,
        }],
        "filed into the FIRST FREE keyring slot (1 is taken), not the backpack"
    );

    // Full: each slot needs a texture, since `PutKeyInKeyRing` reads a slot as free from
    // `GetContainerItemInfo`'s first return, the icon (`ContainerFrame.lua:744-746`).
    let mut full = keyring(4, true);
    for slot in 2..=4 {
        full.slots.insert(
            slot,
            ContainerSlot {
                item_id: 7146,
                count: 1,
                texture: Some("Interface\\Icons\\INV_Misc_Key_07".into()),
                ..Default::default()
            },
        );
    }
    s.set_container(-2, Some(full));
    s.run("PickupContainerItem(0, 3)").unwrap();
    // Stock `PutKeyInKeyRing` reports through `NO_EMPTY_KEYRING_SLOTS`, which 1.12 never defines
    // (`ContainerFrame.lua:753`), so nothing prints: a 1.12 bug, kept.
    s.run("KeyRingButton:Click()").unwrap();
    assert!(
        s.take_container_moves().is_empty(),
        "a full keyring queues no move"
    );
    s.resolve();
    assert!(
        !s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t.contains("keyring is full")
        )),
        "the reference's own dangling string name prints nothing — un-quirking it is a decision, \
         not a default"
    );
    // Nor does it raise: `AddMessage` reads its text through `lua_isstring` (`0x6f3510`), whose
    // failure edge jumps to the function's epilogue (`0x79562c`).
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Stock `ItemAnim_OnEvent` puts the pushed icon on the pane of the button owning that slot and
/// plays sequence 0 once; `ItemAnim_OnAnimFinished` hides it. The pane authored no size, so its
/// rect is the file's bounding box in layout units.
#[test]
fn an_item_push_drops_its_icon_into_the_bag_that_took_it() {
    use benilla_ui::widget::{ModelFileFacts, SequenceFacts};

    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    // `ForcedBackpackItem.m2` (`benilla-extract m2seq`, `m2batch`): one sequence (id 0, 1000 ms,
    // clamp) and a 0.02707 × 0.07962 box, which is 42.41 × 124.72 at 16:9, where a layout unit is
    // 768·√(a²+1) = 1566.4 FrameXML units.
    const CARD: &str = r"Interface\ItemAnimations\ForcedBackpackItem.mdx";
    s.set_model_facts(
        CARD,
        ModelFileFacts {
            sequences: vec![SequenceFacts {
                anim_id: 0,
                duration_ms: 1000,
                looping: false,
            }],
            bbox: ([0.0, 0.0, 0.0], [0.02707, 0.07962, 0.0]),
            cameras: 0,
        },
    );
    s.resolve();

    let shown = |s: &UiScript, name: &str| {
        s.eval::<bool>(&format!("return {name}ItemAnim:IsShown()"))
            .unwrap()
    };
    // The shown card panes, as the renderer sees them: `(icon, play head)` per visible pane.
    type Card = (Option<String>, Option<(u16, u32)>);
    let cards = |s: &mut UiScript| -> Vec<Card> {
        let heads = s.visible_model_panes();
        s.extract()
            .into_iter()
            .filter_map(|q| match q.content {
                QuadContent::ModelPane {
                    handle,
                    model: Some(m),
                    icon,
                    ..
                } if m == CARD => Some((
                    icon,
                    heads
                        .iter()
                        .find(|p| p.handle == handle)
                        .and_then(|p| p.play.map(|ph| (ph.anim_id, ph.cursor_ms))),
                )),
                _ => None,
            })
            .collect()
    };

    for b in [
        "MainMenuBarBackpackButton",
        "CharacterBag1Slot",
        "KeyRingButton",
    ] {
        assert!(!shown(&s, b), "{b}'s card starts hidden");
    }
    assert!(cards(&mut s).is_empty());

    // `arg1` is the inventory-slot id `ItemAnim_OnEvent` compares against the button's: bag 2 is
    // `CharacterBag1Slot`, 21.
    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(21),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Bag_08".into()),
        ],
    );
    assert!(
        shown(&s, "CharacterBag1Slot"),
        "the card that took it plays"
    );
    assert!(
        !shown(&s, "MainMenuBarBackpackButton") && !shown(&s, "KeyRingButton"),
        "…and nobody else's"
    );
    s.resolve();
    let live = cards(&mut s);
    assert_eq!(
        live,
        vec![(
            Some(r"Interface\Icons\INV_Misc_Bag_08".to_string()),
            Some((0, 0))
        )],
        "one pane, the pushed icon on it, sequence 0 at 0"
    );
    // The implicit rect hangs off the button's BOTTOMRIGHT (-10, 0), as the stock template anchors.
    let (w, h): (f32, f32) = s
        .eval("return CharacterBag1SlotItemAnim:GetWidth(), CharacterBag1SlotItemAnim:GetHeight()")
        .unwrap();
    assert!(
        (w - 42.41).abs() < 0.05 && (h - 124.72).abs() < 0.05,
        "the card's rect is the file's box: {w} × {h}"
    );
    let (dx, dy): (f32, f32) = s
        .eval(
            "local a, b = CharacterBag1SlotItemAnim, CharacterBag1Slot \
             return a:GetRight() - b:GetRight(), a:GetBottom() - b:GetBottom()",
        )
        .unwrap();
    assert!(
        (dx + 10.0).abs() < 0.01 && dy.abs() < 0.01,
        "BOTTOMRIGHT (−10, 0) off the button: ({dx}, {dy})"
    );

    // The clock runs the file's 1000 ms clamp.
    s.tick(0.5);
    assert_eq!(cards(&mut s)[0].1, Some((0, 500)));
    s.tick(0.55);
    assert!(
        !shown(&s, "CharacterBag1Slot"),
        "one play, then gone (the stock OnAnimFinished hides it)"
    );
    assert!(cards(&mut s).is_empty());

    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(21),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Bag_09".into()),
        ],
    );
    s.resolve();
    assert_eq!(
        cards(&mut s),
        vec![(
            Some(r"Interface\Icons\INV_Misc_Bag_09".to_string()),
            Some((0, 0))
        )]
    );

    // The keyring (-2) is a destination of its own.
    s.fire_event(
        "ITEM_PUSH",
        vec![
            benilla_ui::script::ScriptValue::Int(-2),
            benilla_ui::script::ScriptValue::Str("Interface\\Icons\\INV_Misc_Key_03".into()),
        ],
    );
    assert!(shown(&s, "KeyRingButton"), "the keyring card plays");
    assert!(
        !shown(&s, "MainMenuBarBackpackButton"),
        "and the backpack's does not"
    );

    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An addon that replaces the global `ToggleBackpack`, as Bagnon's `Overrides.lua` does, receives
/// the backpack button's click and suppresses the stock toggle.
#[test]
fn an_addon_that_hooks_toggle_backpack_receives_the_click() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);

    // Bagnon's idiom: capture the original, replace the global.
    s.run(
        r#"
        HOOK_RAN = 0
        local original = ToggleBackpack
        ToggleBackpack = function() HOOK_RAN = HOOK_RAN + 1 end
    "#,
    )
    .unwrap();

    s.run("MainMenuBarBackpackButton:Click()").unwrap();

    assert_eq!(
        s.eval::<i64>("return HOOK_RAN").unwrap(),
        1,
        "the click must reach the addon's replacement, not our own function"
    );
    assert!(
        !open(&s, 0),
        "the stock bag must not open when an addon has taken the verb over"
    );

    assert!(
        s.eval::<bool>("return type(OpenAllBags) == 'function' and type(ToggleBag) == 'function'")
            .unwrap(),
        "OpenAllBags and ToggleBag are the ref's names and addons hook them too"
    );
}

/// The bag slots carry the stock names, `CharacterBag0Slot` to `CharacterBag3Slot` (0-based, so
/// `CharacterBag0Slot` is bag 1), and their icons the derived `$parentIconTexture`: Bagnon reads
/// `CharacterBag0Slot`'s `OnClick`, and `Bartender2/Alias.lua:352` takes the icon's name.
#[test]
fn the_bag_slots_carry_the_references_names_and_icon_names() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");

    for i in 0..4 {
        assert!(
            s.eval::<bool>(&format!("return CharacterBag{i}Slot ~= nil"))
                .unwrap(),
            "CharacterBag{i}Slot must exist — 8 corpus addons index these"
        );
        assert!(
            s.eval::<bool>(&format!(
                "return getglobal('CharacterBag{i}SlotIconTexture') ~= nil"
            ))
            .unwrap(),
            "and its icon under the derived name Bartender2 aliases"
        );
    }

    // The stock handlers translate with `this:GetID() - CharacterBag0Slot:GetID() + 1`; the ids
    // are inventory slots, `GetInventorySlotInfo("Bag0Slot")` being 20.
    assert_eq!(
        s.eval::<i64>("return CharacterBag0Slot:GetID()").unwrap(),
        20,
        "CharacterBag0Slot's id is its INVENTORY slot — 8 corpus addons compute `GetID() - 19`"
    );
    assert_eq!(
        s.eval::<i64>("return CharacterBag3Slot:GetID() - CharacterBag0Slot:GetID() + 1")
            .unwrap(),
        4,
        "the reference's own translation: the fourth button is bag 4"
    );

    assert!(
        s.eval::<bool>("return getglobal('CharacterBag0Slot'):GetScript('OnClick') ~= nil")
            .unwrap(),
        "Bagnon captures this OnClick to replace the bag behaviour"
    );
}

/// `ContainerFrame_OnShow`/`OnHide` write the ring, and stock `BackpackButton_OnClick` re-derives
/// it after the CheckButton's own flip.
#[test]
fn the_backpack_buttons_ring_follows_its_own_window_through_real_clicks() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    // Stock `ToggleBag` opens nothing for a size-0 container, so the backpack is fed.
    s.set_container(
        0,
        Some(benilla_ui::script::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: std::collections::HashMap::new(),
        }),
    );
    s.set_container(
        1,
        Some(benilla_ui::script::ContainerState {
            name: Some("Pouch".into()),
            num_slots: 6,
            slots: std::collections::HashMap::new(),
        }),
    );
    let checked = |s: &mut UiScript| {
        s.eval::<bool>("return MainMenuBarBackpackButton:GetChecked() and true or false")
            .unwrap()
    };
    s.run("ToggleBag(1)").unwrap();
    assert!(!open(&s, 0));
    assert!(!checked(&mut s));

    // A real click: the CheckButton's own flip fires only on real input.
    s.resolve();
    let r: Vec<f32> = s
        .eval(
            "local f = MainMenuBarBackpackButton \
             return { f:GetLeft() + f:GetWidth() / 2, f:GetBottom() + f:GetHeight() / 2 }",
        )
        .unwrap();
    s.mouse_move(r[0], r[1]);
    s.mouse_button(r[0], r[1], "LeftButton", true);
    s.mouse_button(r[0], r[1], "LeftButton", false);
    assert!(
        open(&s, 0),
        "the click opens the backpack (ToggleBackpack's open arm reads bag 0, not 'anything open')"
    );
    assert!(open(&s, 1), "…and leaves the bag that was already up alone");
    assert!(
        checked(&mut s),
        "open ⇒ lit, written by the window's OnShow over the widget flip"
    );

    // Bag 0 is open now, so the second click is the close-all arm.
    s.mouse_button(r[0], r[1], "LeftButton", true);
    s.mouse_button(r[0], r[1], "LeftButton", false);
    assert!(!open(&s, 0));
    assert!(!open(&s, 1), "the close arm takes every container down");
    assert!(
        !checked(&mut s),
        "shut ⇒ dark, written by the window's OnHide"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
