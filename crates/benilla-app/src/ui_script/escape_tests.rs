//! The ESC ladder: stock `ToggleGameMenu`'s `elseif` chain (`UIParent.lua:1465-1497`), which a
//! focused edit box preempts. The bag windows are recycled, so a bag is asked of `IsBagOpen`,
//! never by frame name.

use benilla_ui::script::{ContainerSlot, ContainerState, LootRow, LootState, UiScript};

use super::test_ui::{bag_open, bag_slot_button, click, load_ui as load_xml, BAG_UI};

/// A backpack with one resolved item in slot 1, so it can be picked up.
fn one_item_backpack() -> ContainerState {
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
            count: 1,
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
    ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots,
    }
}

/// `CloseAllWindows` (`UIParent.lua:1491`) closes the bag and the loot panel, whose `OnHide`
/// releases the loot; with no edit box focused the key is left to the binding.
#[test]
fn escape_closes_bag_and_panel_releases_loot_and_keeps_the_held_item() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // `LOOT_UI` whole: a missing `ItemButtonTemplate` is only a load warning, so it fails silently.
    for file in super::test_ui::LOOT_UI {
        if BAG_UI.contains(file) {
            continue; // a file loads once; `BAG_UI` carried it
        }
        load_xml(&s, file);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    // Stock `ContainerFrame.lua` reads `MerchantFrame:IsShown()`.
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));

    // Open the bag and the loot window; drain their open sounds.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    s.set_loot(Some(LootState {
        fishing: false,
        master_candidates: Vec::new(),
        rows: vec![Some(LootRow {
            item_id: 0,
            name: Some("Wool Cloth".into()),
            texture: Some("Interface\\Icons\\INV_Fabric_Wool_01".into()),
            quantity: 1,
            quality: Some(1),
            is_coin: false,
            link: None,
            random_property_id: 0,
        })],
    }));
    s.fire_event("LOOT_OPENED", vec![]);
    let _ = s.take_sounds();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "cursor holds the picked item");
    assert!(bag_open(&s, 0), "bag is open before ESC");
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot holds the left panel slot before ESC"
    );

    assert!(
        !s.key_input("ESCAPE"),
        "no EditBox focused ⇒ ESC is not consumed by the box layer"
    );

    // The binding the host runs on an unconsumed ESC.
    s.run("ToggleGameMenu()").unwrap();

    assert!(!bag_open(&s, 0), "ESC closed the bag");
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "ESC vacated the panel slot"
    );
    assert!(
        !s.eval::<bool>("return LootFrame:IsVisible()").unwrap(),
        "ESC hid the loot window"
    );
    assert!(
        s.take_loot_close(),
        "closing the loot fired the release (OnHide → CloseLoot)"
    );
    // Stock `ToggleGameMenu` has no `ClearCursor` arm, so the held item survives ESC.
    assert!(
        s.cursor_item().is_some(),
        "the held item survives ESC, as it does in the reference"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A focused edit box consumes ESC, so the binding never runs; the chat box's `OnEscapePressed`
/// clears its focus.
#[test]
fn escape_is_consumed_by_a_focused_editbox_and_leaves_windows_open() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit the chat menus build from
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));

    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(
        s.focus_editbox("ChatFrameEditBox"),
        "the chat edit box focuses"
    );
    assert!(s.has_keyboard_focus());

    assert!(s.key_input("ESCAPE"), "a focused EditBox consumes ESCAPE");
    assert!(!s.has_keyboard_focus(), "the box cleared its focus");
    assert!(
        bag_open(&s, 0),
        "the bag stays open (the escape binding never ran)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The options rung (`UIParent.lua:1483-1484`) eats the press, so the menu opens only on the next.
#[test]
fn escape_closes_the_options_window_before_opening_the_menu() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the Keybindings page's faux-scroll kit
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "GameMenuFrame.xml");

    s.run("ShowUIPanel(BenillaOptionsFrame)").unwrap();
    assert!(s
        .eval::<bool>("return BenillaOptionsFrame:IsVisible()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "the menu is down — the options rung is what must eat this press"
    );

    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return BenillaOptionsFrame:IsVisible()")
            .unwrap(),
        "ESC closed the options window"
    );
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "…and did NOT also open the menu — one eater per press"
    );

    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "the next press reaches the ladder's open-the-menu tail"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `CloseAllWindows` hides the bag, and the item button's `OnHide` hides the split frame it opened
/// (`ContainerFrame.xml:35-39`).
#[test]
fn escape_closes_an_open_stack_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml"); // read by the stock bag-slot click
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml"); // ChatFrameEditBox, for the shift fork
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\StackSplitFrame.xml");
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
    assert!(bag_open(&s, 0), "the backpack window is up");

    // A real shift-click: the stock handler reads `this`, and the buttons number backwards.
    let slot1 = bag_slot_button(&s, 0, 1);
    s.set_modifiers(true, false, false);
    click(&mut s, &slot1, "LeftButton");
    s.set_modifiers(false, false, false);
    assert!(s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap());

    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return StackSplitFrame:IsShown()").unwrap(),
        "ESC closed the split frame"
    );
    // The shift fork never picked the item up.
    assert!(s.cursor_item().is_none());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// One rung eats each press, in stock order (`UIParent.lua:1482-1496`): menus, the cast, the
/// windows, then the target. Each binding answers 1 only when it acted.
#[test]
fn escape_ladder_cast_then_windows_then_target_one_eater_per_press() {
    use benilla_ui::script::UnitState;

    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // The dropdown backdrop reads `TOOLTIP_DEFAULT_COLOR`, which `BAG_UI`'s tooltip file sets.
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(bag_open(&s, 0));
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            ..Default::default()
        }),
    );

    // Press 0, mid-cast with a dropdown open: `CloseMenus` eats it.
    s.set_casting(true);
    // Stock `DropDownList` `OnShow` sizes its buttons from `maxWidth`, which
    // `UIDropDownMenu_Initialize` normally sets (`UIDropDownMenuTemplates.xml:237-245`).
    s.run("DropDownList1.maxWidth = 100 DropDownList1:Show()")
        .unwrap();
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return DropDownList1:IsShown()").unwrap(),
        "ESC closed the open dropdown menu"
    );
    assert!(
        !s.take_spell_stop(),
        "the menu press must NOT reach SpellStopCasting — the cast runs on (ref order: \
         CloseMenus sits before the cast rung)"
    );
    assert!(
        bag_open(&s, 0),
        "the menu press must not close windows either"
    );
    assert!(!s.take_target_clear());

    // Press 1, mid-cast: `SpellStopCasting` eats it.
    s.set_casting(true);
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        bag_open(&s, 0),
        "ESC mid-cast is eaten by SpellStopCasting — the bag stays open"
    );
    assert!(
        s.take_spell_stop(),
        "the stop request queued for the app's local cancel"
    );
    assert!(
        !s.take_target_clear(),
        "the same press must NOT also drop the target (no raw-key double-fire)"
    );

    // Press 2, the cast over: `CloseAllWindows` eats it.
    s.set_casting(false);
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !bag_open(&s, 0),
        "idle ESC falls through SpellStopCasting's nil to CloseAllWindows"
    );
    assert!(!s.take_spell_stop(), "no stray stop request when idle");
    assert!(
        !s.take_target_clear(),
        "a press CloseAllWindows ate must not reach ClearTarget"
    );

    // Press 3: `ClearTarget`.
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.take_target_clear(),
        "the bare press reaches ClearTarget — the ladder's last rung"
    );

    // Press 4, no target: `ClearTarget` answers nil; the menu it falls through to is not loaded.
    s.set_unit("target", None);
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.take_target_clear(),
        "ClearTarget answers nil with no target — nothing queued"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The `SpellStopTargeting` rung (`UIParent.lua:1490`) sits after the cast rung and before the
/// windows; idle, both answer nil.
#[test]
fn escape_ladder_targeting_rung_after_cast_before_windows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(bag_open(&s, 0));

    // `SpellIsTargeting()` mirrors the pushed state.
    assert!(!s.eval::<bool>("return SpellIsTargeting()").unwrap_or(false));
    s.set_spell_targeting(true);
    assert!(s.eval::<bool>("return SpellIsTargeting()").unwrap());

    // Press 1, targeting: `SpellStopTargeting` eats it.
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.take_stop_targeting(),
        "the stop-targeting request queued for the app's cancel"
    );
    assert!(
        !s.take_spell_stop(),
        "the cast rung answered nil — nothing casting"
    );
    assert!(
        bag_open(&s, 0),
        "ESC while targeting must NOT also close windows"
    );

    // Press 2, casting and targeting both (a state the app never pushes): the cast rung is first.
    s.set_casting(true);
    s.run("ToggleGameMenu()").unwrap();
    assert!(s.take_spell_stop(), "the cast rung eats first");
    assert!(
        !s.take_stop_targeting(),
        "the same press must not also cancel the targeting"
    );

    // Press 3, idle: both rungs answer nil and `CloseAllWindows` eats it.
    s.set_casting(false);
    s.set_spell_targeting(false);
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s.take_stop_targeting(), "no stray trigger when idle");
    assert!(
        !bag_open(&s, 0),
        "the idle press falls through both spell rungs to CloseAllWindows"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `tinsert(UISpecialFrames, name)` is the 1.12 addon idiom for closing on ESC; stock
/// `CloseWindows` gives each a plain `Hide`, as they hold no panel slot (`UIParent.lua:947-954`).
#[test]
fn an_addon_frame_registered_in_uispecialframes_closes_on_escape() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "GameMenuFrame.xml");

    s.run(
        r#"
        MyAddonWindow = CreateFrame("Frame", "MyAddonWindow", UIParent)
        MyAddonWindow:Show()
        tinsert(UISpecialFrames, "MyAddonWindow")
        "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return MyAddonWindow:IsVisible()").unwrap());

    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return MyAddonWindow:IsVisible()").unwrap(),
        "ESC closed the addon's window"
    );
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "CloseAllWindows returning truthy is what stops the chain (UIParent.lua l.1491)"
    );

    // A name with no frame is skipped: `if ( frame and frame:IsVisible() )`.
    s.run(r#"tinsert(UISpecialFrames, "NoSuchFrameAnywhere") ToggleGameMenu()"#)
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
