//! Tests for our `GameMenuFrame.xml`, the frame ESC opens. The bags in its way are the stock
//! `ContainerFrame1..12`, recycled, so tests ask [`bag_open`] rather than naming a frame.

use benilla_ui::script::{
    ContainerSlot, ContainerState, LootRow, LootState, SessionRequest, SoundRequest, UiScript,
};

use super::test_ui::{bag_open, load_ui as load_xml, BAG_UI};

/// The menu over what it needs (`GameMenuButtonTemplate` is `UIPanelTemplates.xml:403`), with
/// `extra` in its way, deduped so no file loads twice.
fn harness_with(extra: &[&str]) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let files: Vec<&str> = [
        "Interface\\FrameXML\\Fonts.xml",
        // Before the menu: its `parent="UIParent"` resolves at load.
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
    ]
    .into_iter()
    .chain(extra.iter().copied())
    .chain(std::iter::once("GameMenuFrame.xml"))
    .collect();
    let mut loaded = std::collections::HashSet::new();
    for file in files {
        if loaded.insert(file) {
            load_xml(&s, file);
        }
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

fn harness() -> UiScript {
    harness_with(&[])
}

/// [`harness_with`] over the stock bag stack, with `before` ahead of it and `after` behind.
fn bag_harness_with(before: &[&str], after: &[&str]) -> UiScript {
    let files: Vec<&str> = before
        .iter()
        .copied()
        .chain(BAG_UI.iter().copied())
        .chain(after.iter().copied())
        .collect();
    harness_with(&files)
}

/// Top to bottom. Deviation: the 1.15 era client's ESC ladder, because its settings screens
/// replace 1.12's; without the AddOns rung, as the character-select screen is the only addon UI.
const LADDER: [&str; 7] = [
    "GameMenuButtonOptions",
    "GameMenuButtonEditMode",
    "GameMenuButtonSupport",
    "GameMenuButtonMacros",
    "GameMenuButtonLogout",
    "GameMenuButtonQuit",
    "GameMenuButtonContinue",
];

/// The era layout (MainMenuFrameTemplates: padding 32 top and 28 elsewhere, 20-unit section
/// gaps) over seven 144x21 rungs: 200x267, gaps after Options, after Macros and before Return.
#[test]
fn the_menu_has_the_era_frame_and_button_ladder() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();
    s.resolve();

    let (w, h) = s
        .eval::<(f64, f64)>("return GameMenuFrame:GetWidth(), GameMenuFrame:GetHeight()")
        .unwrap();
    assert_eq!((w, h), (200.0, 267.0), "the era frame size");

    let top = s.eval::<f64>("return GameMenuFrame:GetTop()").unwrap();
    let left = s.eval::<f64>("return GameMenuFrame:GetLeft()").unwrap();
    const TOPS: [f64; 7] = [32.0, 73.0, 94.0, 115.0, 156.0, 177.0, 218.0];
    for (name, down) in LADDER.iter().zip(TOPS) {
        let (bw, bh, btop, bleft) = s
            .eval::<(f64, f64, f64, f64)>(&format!(
                "return {name}:GetWidth(), {name}:GetHeight(), {name}:GetTop(), {name}:GetLeft()"
            ))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            (bw, bh),
            (144.0, 21.0),
            "{name} is a GameMenuButtonTemplate"
        );
        assert!(
            (btop - (top - down)).abs() < 0.001,
            "{name} top: expected {} down {down}, got {}",
            top - down,
            btop
        );
        assert!(
            (bleft - (left + 28.0)).abs() < 0.001,
            "{name} sits at the era 28-unit left padding"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_unbacked_entries_are_disabled_and_the_rest_are_live() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();

    for name in ["GameMenuButtonEditMode", "GameMenuButtonSupport"] {
        assert!(
            !s.eval::<bool>(&format!("return {name}:IsEnabled() ~= 0"))
                .unwrap(),
            "{name} has no panel behind it and must read that way"
        );
    }
    for name in [
        "GameMenuButtonOptions",
        "GameMenuButtonMacros",
        "GameMenuButtonLogout",
        "GameMenuButtonQuit",
        "GameMenuButtonContinue",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name}:IsEnabled() ~= 0"))
                .unwrap(),
            "{name} is live"
        );
    }
    // Edit Mode reads the era's `HUD_EDIT_MODE_MENU` and Continue 1.12's `RETURN_TO_GAME`; Options
    // is a literal, as 1.12's `OPTIONS_MENU` is "Options Menu".
    assert_eq!(
        s.eval::<String>("return GameMenuButtonEditMode:GetText()")
            .unwrap(),
        "Edit Mode"
    );
    assert_eq!(
        s.eval::<String>("return GameMenuButtonContinue:GetText()")
            .unwrap(),
        "Return to Game"
    );
    assert_eq!(
        s.eval::<String>("return GameMenuButtonOptions:GetText()")
            .unwrap(),
        "Options"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ToggleGameMenu`'s ESC arm (`UIParent.lua:1482-1496`).
#[test]
fn escape_opens_the_menu_only_when_nothing_else_wants_the_press_and_then_closes_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_harness_with(
        &[],
        &[
            "ScrollTemplates.xml",
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
        ],
    );
    s.set_money(0);
    s.set_container(0, Some(backpack()));

    // With a window open, `CloseAllWindows` eats the press.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    s.run("ToggleGameMenu()").unwrap();
    assert!(!bag_open(&s, 0), "the press closed the bag");
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "and did NOT also open the menu — one eater per press"
    );

    let _ = s.take_sounds();
    s.run("ToggleGameMenu()").unwrap();
    assert!(s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOpen".into())));

    s.run("ToggleGameMenu()").unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuQuit".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `ToggleGameMenu(1)`, the micro button's form (`UIParent.lua:1466-1480`), is a plain toggle.
#[test]
fn the_clicked_form_closes_everything_and_opens_the_menu_in_one_go() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_harness_with(
        &[],
        &[
            "ScrollTemplates.xml",
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
        ],
    );
    s.set_money(0);
    s.set_container(0, Some(backpack()));
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    assert!(bag_open(&s, 0));

    s.run("ToggleGameMenu(1)").unwrap();
    assert!(!bag_open(&s, 0), "the click closed the bag");
    assert!(
        s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "…and opened the menu in the SAME click"
    );

    s.run("ToggleGameMenu(1)").unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The menu is a native-center panel: showing it closes the panels and bags
/// (`UIParent.lua:698-704`), and while it holds the center `CanOpenPanels` refuses the rest.
#[test]
fn the_open_menu_takes_the_screen_and_refuses_every_other_panel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_harness_with(
        &[],
        &[
            "ScrollTemplates.xml",
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
            // The stock loot window, as `test_ui::LOOT_UI`.
            "Interface\\FrameXML\\UIDropDownMenu.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
            "Interface\\FrameXML\\UnitPopup.xml",
            "Interface\\FrameXML\\TextStatusBar.lua",
            "Interface\\FrameXML\\TextStatusBar.xml",
            "Interface\\FrameXML\\UnitFrame.xml",
            "Interface\\FrameXML\\BuffFrame.xml",
            "Interface\\FrameXML\\PartyFrame.xml",
            "Interface\\FrameXML\\ItemButtonTemplate.xml",
            "Interface\\FrameXML\\LootFrame.xml",
        ],
    );
    s.set_money(0);
    s.set_container(0, Some(backpack()));
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
    assert!(s
        .eval::<bool>("return GetLeftFrame():GetName() == \"LootFrame\"")
        .unwrap());

    s.run("ToggleGameMenu(1)").unwrap();
    assert!(
        s.eval::<bool>("return GetLeftFrame() == nil").unwrap(),
        "the panel slot vacated on the way in"
    );
    assert!(!bag_open(&s, 0), "and the bags closed (CloseAllBags)");
    assert!(
        s.eval::<bool>("return GetCenterFrame():GetName() == \"GameMenuFrame\"")
            .unwrap(),
        "the menu holds the CENTER slot, not the left one"
    );
    assert!(
        !s.eval::<bool>("return CanOpenPanels() and true or false")
            .unwrap(),
        "a native-center frame is up: nothing may open"
    );

    s.run("ShowUIPanel(LootFrame)").unwrap();
    assert!(
        !s.eval::<bool>("return LootFrame:IsVisible()").unwrap(),
        "ShowUIPanel refuses a left-area panel behind the menu"
    );

    s.run("HideUIPanel(GameMenuFrame)").unwrap();
    s.run("ShowUIPanel(LootFrame)").unwrap();
    assert!(s.eval::<bool>("return LootFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Logout and Exit Game queue alike here: whether a finished logout ends the process is app-side.
#[test]
fn the_live_buttons_queue_their_intents_and_play_their_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonContinue:Click()").unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuContinue".into())));
    assert!(s.take_session_requests().is_empty());

    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonLogout:Click()").unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Logout]);
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuLogout".into())));
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());

    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonQuit:Click()").unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Quit]);
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuQuit".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `PLAYER_CAMPING` shows CAMP (`UIParent.lua:304-307`, `StaticPopup.lua:564-583`), whose `%d %s`
/// text the popup engine's countdown branch writes, not the entry (`StaticPopup.lua:1727-1758`).
#[test]
fn player_camping_opens_a_counting_dialog_whose_early_close_cancels() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_CAMPING", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup_Visible(\"CAMP\") or \"\"")
            .unwrap(),
        "StaticPopup1",
        "the camp dialog took an instance"
    );

    // One tick of the countdown fills the text from `CAMP_TIMER`.
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.5)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "20 Seconds until logout",
        "the countdown text is the engine's, from the server's 20 s clock"
    );
    // Cancel is the only button: 1.12 comments out `CAMP_NOW` (`StaticPopup.lua:567`).
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button1:GetText()")
            .unwrap(),
        "Cancel"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1Button2:IsShown()")
            .unwrap(),
        "no second button"
    );

    // An early close cancels from both OnAccept and OnHide, as in the reference; harmless twice.
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert!(
        s.take_session_requests()
            .contains(&SessionRequest::CancelLogout),
        "the early close called the logout off"
    );
    assert!(s
        .eval::<bool>("return StaticPopup_Visible(\"CAMP\") == nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The engine zeroes `timeleft` before hiding (`StaticPopup.lua:1716-1722`), and the entry's OnHide
/// cancels only while `timeleft > 0`.
#[test]
fn a_countdown_that_expires_does_not_cancel_the_logout() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_CAMPING", vec![]);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 25)").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup_Visible(\"CAMP\") == nil")
            .unwrap(),
        "the dialog closed itself at zero"
    );
    assert!(
        !s.take_session_requests()
            .contains(&SessionRequest::CancelLogout),
        "…and did NOT call off the logout it was counting"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// QUIT adds "Exit now" (`ForceQuit`); `LOGOUT_CANCEL`, the server's cancel ack, hides CAMP and
/// QUIT alike (`UIParent.lua:312-316`).
#[test]
fn player_quiting_offers_exit_now_and_logout_cancel_closes_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_QUITING", vec![]);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.5)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "20 Seconds until exit"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button1:GetText()")
            .unwrap(),
        "Exit now"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "Cancel"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert!(s
        .take_session_requests()
        .contains(&SessionRequest::ForceQuit));

    s.fire_event("PLAYER_CAMPING", vec![]);
    assert!(s
        .eval::<bool>("return StaticPopup_Visible(\"CAMP\") ~= nil")
        .unwrap());
    let _ = s.take_session_requests();
    s.fire_event("LOGOUT_CANCEL", vec![]);
    assert!(s
        .eval::<bool>("return StaticPopup_Visible(\"CAMP\") == nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The stock OnShow guard (`GameMenuFrame.xml:134-140`) disables both while CAMP or QUIT counts.
/// Opened with the micro button, because ESC during a countdown dismisses the dialog instead.
#[test]
fn logout_and_exit_read_disabled_while_a_countdown_runs() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_CAMPING", vec![]);
    s.run("ToggleGameMenu(1)").unwrap();
    assert!(
        !s.eval::<bool>("return GameMenuButtonLogout:IsEnabled() ~= 0")
            .unwrap(),
        "Logout is dead while the camp timer runs"
    );
    assert!(!s
        .eval::<bool>("return GameMenuButtonQuit:IsEnabled() ~= 0")
        .unwrap());

    s.run("HideUIPanel(GameMenuFrame)").unwrap();
    s.fire_event("LOGOUT_CANCEL", vec![]);
    s.run("ToggleGameMenu(1)").unwrap();
    assert!(s
        .eval::<bool>("return GameMenuButtonLogout:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `StaticPopup_EscapePressed` is the first rung (`UIParent.lua:1482`) and CAMP is `hideOnEscape`,
/// so ESC dismisses the dialog, whose OnHide cancels the logout.
#[test]
fn escape_during_a_countdown_cancels_it_and_does_not_open_the_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("PLAYER_CAMPING", vec![]);
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup_Visible(\"CAMP\") == nil")
            .unwrap(),
        "the press dismissed the countdown"
    );
    assert!(
        s.take_session_requests()
            .contains(&SessionRequest::CancelLogout),
        "…which called the logout off"
    );
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "and the same press did NOT also open the menu — one eater per press"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The map is a stock `area = "full"` panel (`UIParent.lua:30`), so the menu blocks it, and on
/// close it must vacate the full-screen slot, or the stale slot refuses every later panel.
#[test]
fn the_world_map_cannot_open_behind_the_menu_and_gives_its_slot_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness_with(&[
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml", // the map's zone pickers initialize at OnLoad
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The map calls `UpdateMicroButtons` unguarded (`WorldMapFrame.xml:599`, `:606`).
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        r"Interface\FrameXML\WorldMapFrame.xml",
    ]);

    s.run("ToggleWorldMap()").unwrap();
    assert!(s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(s
        .eval::<bool>("return GetFullScreenFrame():GetName() == \"WorldMapFrame\"")
        .unwrap());

    s.run("ToggleWorldMap()").unwrap();
    assert!(!s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetFullScreenFrame() == nil")
            .unwrap(),
        "the map vacated the full-screen slot"
    );

    // The M binding and the micro button both call `ToggleWorldMap`.
    s.run("ToggleGameMenu(1)").unwrap();
    assert!(s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    s.run("ToggleWorldMap()").unwrap();
    assert!(
        !s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap(),
        "the map must not open behind the game menu"
    );
    assert!(s
        .eval::<bool>("return GetFullScreenFrame() == nil")
        .unwrap());

    s.run("ToggleGameMenu(1)").unwrap();
    s.run("ToggleWorldMap()").unwrap();
    assert!(s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With the map holding the full screen nothing else opens (`UIParent.lua:668-675`), and ESC
/// closes the map without also opening the menu.
#[test]
fn nothing_opens_behind_the_world_map_and_escape_closes_it_first() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness_with(&[
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The action bar the micro buttons sit on: the map calls `UpdateMicroButtons` unguarded.
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        r"Interface\FrameXML\MainMenuBarMicroButtons.xml",
        r"Interface\FrameXML\WorldMapFrame.xml",
        "Interface\\FrameXML\\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\MerchantFrame.xml",
        // The stock loot window, as `test_ui::LOOT_UI`.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, which UnitPopup.lua reads at file scope
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\ItemButtonTemplate.xml",
        "Interface\\FrameXML\\LootFrame.xml",
    ]);
    s.run("ToggleWorldMap()").unwrap();

    s.run("ShowUIPanel(LootFrame)").unwrap();
    assert!(
        !s.eval::<bool>("return LootFrame:IsVisible()").unwrap(),
        "a left-area panel must not open behind the full-screen map"
    );

    s.run("ToggleGameMenu()").unwrap();
    assert!(!s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "…and did not also open the menu"
    );
    assert!(s
        .eval::<bool>("return GetFullScreenFrame() == nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Checked at the quad level, as `IsEnabled()` cannot tell greyed from gone: a disabled button
/// draws no state texture without a DisabledTexture (`SetState`, `0x779790`), and each bag icon is
/// a layer texture that `Disable_BagButtons` desaturates (`MainMenuBarBagButtons.lua:102-113`).
#[test]
fn the_bag_row_greys_under_the_menu_without_any_of_it_disappearing() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The action bar, whose `MainMenuBarArtFrame` parents the bag bar, ahead of the bag stack.
    let mut s = bag_harness_with(
        &[
            "Interface\\FrameXML\\Cooldown.xml",
            "Interface\\FrameXML\\ActionButtonTemplate.xml",
            "Interface\\FrameXML\\TextStatusBar.lua",
            "Interface\\FrameXML\\TextStatusBar.xml",
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\UIParent.xml",
            "ScrollTemplates.xml",
            "Interface\\FrameXML\\GlobalStrings.lua",
            "Interface\\FrameXML\\MainMenuBar.xml",
            "Interface\\FrameXML\\GameTooltip.xml",
            "Interface\\FrameXML\\ActionBarFrame.xml",
            "Interface\\FrameXML\\BonusActionBarFrame.xml",
        ],
        &[
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
        ],
    );
    s.set_money(0);
    s.set_container(0, Some(backpack()));
    s.resolve();

    let art = |s: &UiScript, owner: &str| -> Vec<(String, bool)> {
        s.extract()
            .into_iter()
            .filter(|eq| s.quad_owner_name(eq.target).as_deref() == Some(owner))
            .filter_map(|eq| match &eq.content {
                benilla_ui::script::QuadContent::Texture {
                    path: Some(p),
                    desaturated,
                    ..
                } => Some((p.clone(), *desaturated)),
                _ => None,
            })
            .collect()
    };

    for owner in [
        "MainMenuBarBackpackButton",
        "CharacterBag0Slot",
        "CharacterBag1Slot",
        "CharacterBag2Slot",
        "CharacterBag3Slot",
    ] {
        assert!(
            !art(&s, owner).is_empty(),
            "{owner} draws art before the menu opens"
        );
    }

    s.run("ToggleGameMenu(1)").unwrap();
    s.resolve();
    for owner in [
        "MainMenuBarBackpackButton",
        "CharacterBag0Slot",
        "CharacterBag1Slot",
        "CharacterBag2Slot",
        "CharacterBag3Slot",
    ] {
        let drawn = art(&s, owner);
        assert!(
            !drawn.is_empty(),
            "{owner} must still DRAW under the open menu — greyed is not gone"
        );
    }
    // The backpack icon keeps its art and carries `SetDesaturation`'s greyscale flag.
    let toggle = art(&s, "MainMenuBarBackpackButton");
    assert!(
        toggle
            .iter()
            .any(|(p, _)| p == "Interface\\Buttons\\Button-Backpack-Up"),
        "the backpack image is still on the bar: {toggle:?}"
    );
    assert!(
        toggle
            .iter()
            .any(|(p, grey)| p == "Interface\\Buttons\\Button-Backpack-Up" && *grey),
        "…and it is greyed, not full-bright: {toggle:?}"
    );

    s.run("ToggleGameMenu(1)").unwrap();
    s.resolve();
    let toggle = art(&s, "MainMenuBarBackpackButton");
    assert!(
        toggle
            .iter()
            .any(|(p, grey)| p == "Interface\\Buttons\\Button-Backpack-Up" && !*grey),
        "closing the menu restores full colour: {toggle:?}"
    );
    assert!(
        s.eval::<bool>("return MainMenuBarBackpackButton:IsEnabled() ~= 0")
            .unwrap(),
        "…and the button works again"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A one-item backpack.
fn backpack() -> ContainerState {
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

/// The menu wears the options window's `ERA_WINDOW_SCALE` on show, as the era client draws the two
/// at one density; loaded with OptionsFrame.xml, which defines it.
#[test]
fn the_menu_rides_the_shared_era_window_scale() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness_with(&[
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "KeyBindingsPage.xml",
        "OptionsFrame.xml",
    ]);
    s.run("ShowUIPanel(GameMenuFrame)").unwrap();
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    let k = s.eval::<f64>("return GameMenuFrame:GetScale()").unwrap();
    let want = s.eval::<f64>("return ERA_WINDOW_SCALE").unwrap();
    assert!((k - want).abs() < 1e-6, "menu scale {k} != knob {want}");
    assert!((want - 0.78).abs() < 1e-6, "the knob itself moved: {want}");
}
