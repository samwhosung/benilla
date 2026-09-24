//! **The ESCAPE ladder** — `ToggleGameMenu`'s `elseif` chain (`UIParent.lua:1465-1497`) and the
//! host-glue precedence around it (a focused EditBox eats the key before the binding ever runs).
//!
//! The window these tests park in the ladder's way is a BAG, and since decision 1751 that is the
//! reference's own `ContainerFrame1..12`, executed off the player's installed patch chain — not
//! `BenillaBagFrame`, which no longer exists. Two consequences run through the file:
//!
//! * a bag test loads [`BAG_UI`] and opens with `wow_data_or_skip!`, because a chain entry needs
//!   client data; and
//! * "is the bag open?" is asked of [`bag_open`] (the reference's own `IsBagOpen` scan), never of
//!   a frame by name — the twelve windows are RECYCLED, so which one a bag lands in is a
//!   coincidence and not a property.
//!
//! The ladder itself is unchanged by that swap: `CloseAllWindows` (ours, `UiPanels.xml`) sweeps
//! `ContainerFrame1..NUM_CONTAINER_FRAMES` where it used to sweep our five named windows.

use benilla_ui::script::{ContainerSlot, ContainerState, LootRow, LootState, UiScript};

use super::test_ui::{bag_open, bag_slot_button, click, load_ui as load_xml, BAG_UI};

/// A one-item backpack (slot 1 holds a resolved item, so it is pickable onto the cursor).
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

/// Load every window and drive the whole ESC-close path (UIParent.lua ToggleGameMenu → l.1491
/// CloseAllWindows): with the bag open, the loot window on
/// the left slot, and an item on the cursor, ESC closes the bag AND the panel slot, releases the
/// loot (OnHide → CloseLoot), and drops the held cursor. Also asserts the host-glue precedence: no
/// EditBox focused ⇒ `key_input("ESCAPE")` does not consume, so the app runs the binding.
///
/// The bag half is the reference's window now (1751), so what `CloseAllWindows` must sweep is a
/// `ContainerFrame` — the same assertion, asked through `IsBagOpen` instead of a frame name.
#[test]
fn escape_closes_bag_and_panel_releases_loot_and_clears_cursor() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // The loot window is the reference's own since 1751, and its dependency list grew with the
    // swap — `LOOT_UI` carries the why for each entry. Loaded whole rather than cherry-picked:
    // `PartyFrame.xml` is needed at LOAD (MAX_PARTY_MEMBERS) and skipping it fails loudly, but
    // `ItemButtonTemplate` is only a warning and skipping it fails silently.
    for file in super::test_ui::LOOT_UI {
        if BAG_UI.contains(file) {
            continue; // a file loads once — the bag chain above carried it
        }
        load_xml(&s, file);
    }
    load_xml(&s, "Interface\\FrameXML\\LootFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    // The reference's own `ContainerFrameItemButton_OnClick` reads `MerchantFrame:IsShown()`
    // on the bag-slot click below; the coin rig this line used to name is gone (1937/1962).
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));

    // Open the bag and the loot window; drain the open kits (not under test here).
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

    // Pick up slot 1 onto the cursor.
    s.run("PickupContainerItem(0, 1)").unwrap();
    assert!(s.cursor_item().is_some(), "cursor holds the picked item");
    assert!(bag_open(&s, 0), "bag is open before ESC");
    assert!(
        s.eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
            .unwrap(),
        "loot holds the left panel slot before ESC"
    );

    // Precedence: no EditBox focused ⇒ ESCAPE is NOT consumed, so the host runs the binding.
    assert!(
        !s.key_input("ESCAPE"),
        "no EditBox focused ⇒ ESC is not consumed by the box layer"
    );

    // The escape binding (what the host runs on the unconsumed ESC).
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
    // **ESC does NOT drop the cursor's item.** Our retired ladder opened with
    // `if CursorHasItem() then ClearCursor() end`; the reference's `ToggleGameMenu`
    // (`UIParent.lua:1465-1497`) has no such arm, and the ESC→ClearCursor wiring is inferred to
    // live in FrameXML rather than the engine — so in 1.12 the held item survives the key
    // (1988).
    assert!(
        s.cursor_item().is_some(),
        "the held item survives ESC, as it does in the reference"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The other precedence side: a focused EditBox consumes ESCAPE (`key_input` returns true), so the
/// host's `!consumed` gate never runs the escape binding while typing — the bag stays open. Drives
/// the shipped chat box (the app's ENTER-to-open path is `focus_editbox`); its own OnEscapePressed
/// clears the focus (chat_tests covers the box's submit/close contract itself).
#[test]
fn escape_is_consumed_by_a_focused_editbox_and_leaves_windows_open() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
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

    // ESCAPE is consumed by the focused box — the host would NOT run the escape binding.
    assert!(s.key_input("ESCAPE"), "a focused EditBox consumes ESCAPE");
    // The box's OnEscapePressed cleared its own focus, but the bag window is untouched.
    assert!(!s.has_keyboard_focus(), "the box cleared its focus");
    assert!(
        bag_open(&s, 0),
        "the bag stays open (the escape binding never ran)"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The options rung (decision 0950; the ref's own `elseif OptionsFrame:IsVisible()` at
/// UIParent.lua l.1483-1484, BETWEEN the popup rung and the menu rung): ESC with the options
/// window up closes it — hide, not cancel-click: benilla applies changes live — and eats the
/// press, so the menu does NOT open on the same stroke. Only the NEXT press, with nothing left
/// to eat, opens the menu (one eater per press, the 0449 law).
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
    load_xml(&s, "ScrollTemplates.xml"); // the Keybindings page's faux-scroll kit (1008)
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

    // Press 1: the options rung eats it — the window closes and the menu stays down.
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

    // Press 2: nothing left to eat — the menu opens.
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "the next press reaches the ladder's open-the-menu tail"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// ESC closes an open stack-split spinner (decision 0216 §6/slice 2, StackSplit.xml): this engine
/// has no plain-frame keyboard capture (the real client's own `StackSplitFrame` OnKeyDown ESCAPE
/// arm can't be driven), so the hook rides `ToggleGameMenu`'s shared chain instead — checked
/// right after the confirm popup and before the world map, both similarly transient overlays.
///
/// The spinner is opened by the REFERENCE's own shift fork now (1751,
/// `ContainerFrame.lua:567-577`), reached by a real SHIFT + left-click on a `ContainerFrame` item
/// button — which is why `ChatFrame.xml` is loaded: that fork's first test is
/// `ChatFrameEditBox:IsShown()` (a shift-click links the item into an open chat box instead of
/// splitting it), and the reference indexes that global unguarded.
#[test]
fn escape_closes_an_open_stack_split_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml"); // ContainerFrameItemButton_OnClick reads MerchantFrame
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml"); // …and ChatFrameEditBox, the shift fork's first test
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

    // Open the spinner with a real SHIFT + left-click on the button holding slot 1 — asked of the
    // buttons' own `GetID()`, never by index (`ContainerFrame_GenerateFrame` numbers them
    // backwards). The click goes through the engine because the reference's handler reads `this`.
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
    // ESC's unconditional cursor-clear (ToggleGameMenu's own first line) ran too — nothing was
    // held anyway (the split branch already released it), so this just confirms no error.
    assert!(s.cursor_item().is_none());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The ESC ladder end-to-end (the ref's `ToggleGameMenu` order, `UIParent.lua:1482-1496`, one
/// eater per press — the director's two-press report, decision 0449): mid-cast with a menu
/// open, a bag open and a target, the press closes the MENU first (`CloseMenus`, `l.1488` —
/// the cast survives), the next only cancels the cast (`SpellStopCasting`, `l.1489`), the next
/// only closes the windows (`CloseAllWindows`, `l.1491`), and only then — nothing left to
/// eat — does one drop the target (`ClearTarget`, `l.1492`). The 1/nil returns are load-bearing
/// at every rung (an unconditional true anywhere would wedge the ladder; the artifact's
/// `elseif` chain is the proof each real binding is conditional).
#[test]
fn escape_ladder_cast_then_windows_then_target_one_eater_per_press() {
    use benilla_ui::script::UnitState;

    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    // GameTooltip.xml (BAG_UI's, for the bag slots' tooltips) also carries TOOLTIP_DEFAULT_COLOR,
    // which the dropdown backdrop's OnLoad reads — so the kit can load straight after it.
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
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

    // Press 0 — mid-cast with a dropdown open (the ref's CloseMenus rung, l.1488): the menu
    // closes and eats the press — the cast, the windows, and the target all survive.
    s.set_casting(true);
    // `maxWidth` before the Show: stock `DropDownList`'s own `<OnShow>` sizes every button from it
    // (ref UIDropDownMenuTemplates.xml:237-245), and `UIDropDownMenu_Initialize` is what normally
    // sets it — so showing the list bare raises `SetWidth(nil)` in the real client too. Our deleted
    // transcription had no such OnShow, which is why a bare Show worked here before. The subject of
    // this test is the ESC ladder, not the menu's construction, so it sets the one field the show
    // path reads rather than standing up a whole anchored dropdown.
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

    // Press 1 — mid-cast (the app's per-frame IsCasting mirror): the cast dies, NOTHING else.
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
        "the same press must NOT also drop the target (the raw-key double-fire 0449 retires)"
    );

    // Press 2 — the cancel resolved (next frame's mirror push): the windows close, target stays.
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

    // Press 3 — nothing left to eat: the target drops.
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.take_target_clear(),
        "the bare press reaches ClearTarget — the ladder's last rung"
    );

    // Press 4 — no target either: the chain runs out. In a full UI this press opens the game menu
    // (decision 0674, `game_menu_tests`); this harness deliberately loads no GameMenuFrame.xml, so
    // what it pins is the rung BELOW it — ClearTarget answering nil rather than eating the press.
    s.set_unit("target", None);
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.take_target_clear(),
        "ClearTarget answers nil with no target — nothing queued"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The SpellStopTargeting rung (`UIParent.lua:1490`, decision 0792): ESC with the ground-target
/// cursor up cancels the targeting and ONLY the targeting — after the cast rung (the artifact's
/// order), before the window close. The 1/nil returns are load-bearing exactly like the cast
/// rung's: an idle press must fall straight through both.
#[test]
fn escape_ladder_targeting_rung_after_cast_before_windows() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in BAG_UI {
        load_xml(&s, file);
    }
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    s.set_money(0);
    s.set_container(0, Some(one_item_backpack()));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    assert!(bag_open(&s, 0));

    // SpellIsTargeting() mirrors the pushed state — the PetFrame-family readers' predicate.
    assert!(!s.eval::<bool>("return SpellIsTargeting()").unwrap_or(false));
    s.set_spell_targeting(true);
    assert!(s.eval::<bool>("return SpellIsTargeting()").unwrap());

    // Press 1 — targeting, no cast in flight: SpellStopTargeting eats the press; the bag stays.
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

    // Press 2 — BOTH casting and targeting pushed (unreachable app state, but the chain's order
    // is the artifact's law): the cast rung sits first (l.1489 before l.1490) and eats alone.
    s.set_casting(true);
    s.run("ToggleGameMenu()").unwrap();
    assert!(s.take_spell_stop(), "the cast rung eats first");
    assert!(
        !s.take_stop_targeting(),
        "the same press must not also cancel the targeting"
    );

    // Press 3 — idle (the app cleared the mode and the cast resolved): both rungs answer nil
    // and the press falls through to CloseAllWindows.
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

/// **`UISpecialFrames` — an addon's own ESC list** (decision 1206).
///
/// `tinsert(UISpecialFrames, "MyFrame")` is *the* 1.12 idiom for "ESC should close my window", and
/// 34 corpus call sites use it. With the table absent that line was
/// `bad argument #1 to 'tinsert' (table expected, got nil)` — 11 addons dead at load, the
/// second-largest wall in the survey.
///
/// The walk is the reference's own (`UIParent.lua` l.947-954, inside `CloseWindows`): a plain
/// `Hide`, never `HideUIPanel`, because these frames hold no panel slot.
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

    // The addon's three lines, verbatim in shape.
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
    // ...and it ATE the press: the menu must not also open, or ESC would close the window and
    // raise the menu in one keystroke.
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "CloseAllWindows returning truthy is what stops the chain (UIParent.lua l.1491)"
    );

    // A name in the table that resolves to nothing is skipped, not an error — the reference's own
    // `if (frame and frame:IsVisible())`.
    s.run(r#"tinsert(UISpecialFrames, "NoSuchFrameAnywhere") ToggleGameMenu()"#)
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
