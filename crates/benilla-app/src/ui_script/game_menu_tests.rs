//! The shipped `assets/ui/GameMenuFrame.xml` — the frame ESC opens.
//!
//! What these guard, in order: the ladder geometry (the ERA menu shape minus its AddOns rung —
//! 200×267, seven rungs in three 20-gapped sections, the era layout engine's own numbers
//! precomputed in the XML's header note); the greyed pending entries; the live buttons' wire
//! intents and sounds;
//! the ESC ladder's two new rungs (open when nothing is left to eat, close before everything
//! else); the micro button's `clicked` toggle; the native-center rule that makes the menu take
//! the screen (windows close on the way in, and nothing opens while it is up); and the camp/quit
//! countdown dialogs end to end. The Options button's own click path is options_tests'.
//!
//! The **window** most of these park in the menu's way is a bag, and since decision 1751 that is
//! the reference's own `ContainerFrame1..12` off the player's installed patch chain — so those
//! tests take [`bag_harness_with`], gate on `wow_data_or_skip!`, and ask [`bag_open`] rather than
//! naming a frame (the twelve windows are recycled). The bag BAR the greying test measures —
//! `MainMenuBarBackpackButton` and `CharacterBag0Slot`..`3Slot` — is still ours, unchanged.

use benilla_ui::script::{
    ContainerSlot, ContainerState, LootRow, LootState, SessionRequest, SoundRequest, UiScript,
};

use super::test_ui::{bag_open, load_ui as load_xml, BAG_UI};

/// The engine the menu needs behind it: fonts, the panel manager + popup engine, the shared widget
/// kit, and the menu. `extra` adds the files a given test wants in the way (a bag, a loot window).
///
/// `UIPanelTemplates.xml` is not optional and never was in production: `GameMenuButtonTemplate`
/// lives there (the reference's own file for it) since the colour picker needed it from above
/// GameMenuFrame.xml's deliberately-LAST seat in the manifest. Without it every rung comes out
/// sizeless and the ladder geometry below reads nil — which is exactly how the move was caught.
///
/// **`extra` is DEDUPED against this base list**, and that is what lets a caller hand it
/// [`BAG_UI`] whole (which names `Fonts.xml`, `MoneyFrame.xml`, `UiPanels.xml` and
/// `UIPanelTemplates.xml` for its own reasons) without loading a file — and re-running its
/// OnLoads — twice. Order is still the caller's: the first mention of a file is where it lands.
fn harness_with(extra: &[&str]) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let files: Vec<&str> = [
        "Interface\\FrameXML\\Fonts.xml",
        // `GameMenuFrame` and the panels it opens declare `parent="UIParent"`, resolved at LOAD
        // — UIParent must already be there, as it is in the manifest.
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

/// [`harness_with`], with the reference's bag stack folded in — the harness for every test below
/// that parks a BAG in the menu's way. Since decision 1751 those windows are the reference's own
/// `ContainerFrame1..12`, executed off the player's installed patch chain, so `BAG_UI` names
/// `Interface\FrameXML\ContainerFrame.xml` and every caller opens with `wow_data_or_skip!`.
///
/// `before` loads AHEAD of the stack, for the one file that has to: `ActionBar.xml` declares
/// `MainMenuBarArtFrame`, which `BagFrame.xml`'s bag bar anchors into and seats itself above.
/// `after` is the ordinary tail (a merchant, a loot window).
fn bag_harness_with(before: &[&str], after: &[&str]) -> UiScript {
    let files: Vec<&str> = before
        .iter()
        .copied()
        .chain(BAG_UI.iter().copied())
        .chain(after.iter().copied())
        .collect();
    harness_with(&files)
}

/// The seven buttons, top to bottom — the ERA ladder, its own shape (the director's call on the
/// 0951 review; GameMenuFrame.xml's header SCOPE note quotes the era source), minus two seats:
/// 0997's carved Key Bindings seat left with its standalone window (decision 1008 folded key
/// bindings into the Options window, where the era menu always pointed), and the AddOns rung
/// left with the in-game panel when the director made the char-select AddOns screen the only
/// addon UI (it had been live since 1197).
const LADDER: [&str; 7] = [
    "GameMenuButtonOptions",
    "GameMenuButtonEditMode",
    "GameMenuButtonSupport",
    "GameMenuButtonMacros",
    "GameMenuButtonLogout",
    "GameMenuButtonQuit",
    "GameMenuButtonContinue",
];

/// The ladder geometry — the era layout engine's own numbers (MainMenuFrameTemplates: padding
/// 32/28/28/28, spacing 0, AddSection gap 20) over our seven rungs: 200×267, each button
/// 144×21 at x=28, tops at 32/73/94/115/156/177/218 — three sections split by the 20-unit
/// gaps after Options, after Macros, and before Return to Game. First thing to break if the
/// era shape is ever "tidied".
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

/// The two entries with nothing behind them — Edit Mode, Support — are DISABLED (the pending
/// idiom: grey label, `-Disabled` art, exactly how the era menu greys a dead entry). Everything
/// else in the ladder is live. **Macros left this list in decision 0983**, when the macro window
/// landed behind it. (AddOns had left it in 1197 and then left the ladder entirely with its
/// panel — the char-select AddOns screen is the only addon UI.)
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
    // The labels read the era strings (Edit Mode through the HUD_EDIT_MODE_MENU global — the
    // label has to come from the string rather than a literal).
    assert_eq!(
        s.eval::<String>("return GameMenuButtonEditMode:GetText()")
            .unwrap(),
        "Edit Mode"
    );
    // Continue's label is the reference GlobalString, read back through the global; Options is
    // the one literal (1.12 GlobalStrings has no GAMEOPTIONS_MENU — the XML says so).
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

/// The ESC ladder's two new rungs (`ToggleGameMenu`, UIParent.lua l.1485-1495): a press with
/// nothing left to eat OPENS the menu (igMainMenuOpen), and the next press closes it
/// (igMainMenuQuit — the reference's own choice of kit) without reaching any rung below.
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

    // A press with a window open is eaten by CloseAllWindows — the menu stays down.
    s.run("MainMenuBarBackpackButton:Click()").unwrap();
    let _ = s.take_sounds();
    s.run("ToggleGameMenu()").unwrap();
    assert!(!bag_open(&s, 0), "the press closed the bag");
    assert!(
        !s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap(),
        "and did NOT also open the menu — one eater per press"
    );

    // Nothing left: the next press opens it.
    let _ = s.take_sounds();
    s.run("ToggleGameMenu()").unwrap();
    assert!(s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuOpen".into())));

    // And the press after that closes it again.
    s.run("ToggleGameMenu()").unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuQuit".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The micro button's form (`ToggleGameMenu(1)`, l.1466-1480) is a plain TOGGLE, not the ladder:
/// clicking it with a window open closes the window AND opens the menu in one go — the press-eating
/// rule is the ESC key's, not the button's.
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

/// The native-center rule (`ShowUIPanel` l.697-702 + `CanOpenPanels`): opening the menu closes the
/// panel slots and the bags, and while it holds the center NOTHING else opens — which is what makes
/// "you can't open your bags with the ESC menu up" true rather than merely greyed.
#[test]
fn the_open_menu_takes_the_screen_and_refuses_every_other_panel() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = bag_harness_with(
        &[],
        &[
            "ScrollTemplates.xml", // our scroll kit + the placeholder icon
            "Interface\\FrameXML\\CharacterFrameTemplates.xml",
            "Interface\\FrameXML\\MerchantFrame.xml",
            // The loot window is the reference's own since 1751 — `test_ui::LOOT_UI` carries
            // what it needs and why, and PartyFrame's `MAX_PARTY_MEMBERS` is needed at LOAD.
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

    // The refusal itself: a panel asked to show while the menu is up simply doesn't.
    s.run("ShowUIPanel(LootFrame)").unwrap();
    assert!(
        !s.eval::<bool>("return LootFrame:IsVisible()").unwrap(),
        "ShowUIPanel refuses a left-area panel behind the menu"
    );

    // …and works again the moment the menu goes down.
    s.run("HideUIPanel(GameMenuFrame)").unwrap();
    s.run("ShowUIPanel(LootFrame)").unwrap();
    assert!(s.eval::<bool>("return LootFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The three live buttons: each plays its own reference kit and does its one thing. Logout and Exit
/// Game both queue on the session seam — the difference between them is entirely app-side (whether
/// the completed logout ends the process), which is why they look identical from here.
#[test]
fn the_live_buttons_queue_their_intents_and_play_their_kits() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    // Return to Game — just closes, no intent.
    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonContinue:Click()").unwrap();
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuContinue".into())));
    assert!(s.take_session_requests().is_empty());

    // Logout — the request, and the menu goes away behind it.
    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonLogout:Click()").unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Logout]);
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuLogout".into())));
    assert!(!s.eval::<bool>("return GameMenuFrame:IsVisible()").unwrap());

    // Exit Game — the same shape, its own kit.
    s.run("ToggleGameMenu()").unwrap();
    let _ = s.take_sounds();
    s.run("GameMenuButtonQuit:Click()").unwrap();
    assert_eq!(s.take_session_requests(), vec![SessionRequest::Quit]);
    assert!(s
        .take_sounds()
        .contains(&SoundRequest::KitName("igMainMenuQuit".into())));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The camp countdown end to end (ref StaticPopup.lua l.564-582 + UIParent.lua l.304-315): the
/// server's non-instant answer becomes `PLAYER_CAMPING`, the dialog counts down from the server's
/// own 20 s, and closing it EARLY calls the logout off. The `%d %s` text is written by the popup
/// engine's countdown branch, not by the entry — a dialog that opened blank and stayed blank would
/// mean CAMP fell out of that which-list.
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

    // One tick of the engine's countdown fills the text from CAMP_TIMER.
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.5)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "20 Seconds until logout",
        "the countdown text is the engine's, from the server's 20 s clock"
    );
    // Cancel is the only button (the ref's CAMP_NOW is commented out in 1.12).
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

    // Closing it early cancels the logout — twice over in the reference's own shape (the button's
    // OnAccept, and the OnHide guard that catches every other way out); a doubled cancel is
    // harmless on the wire, and losing it would strand the character mid-countdown.
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

/// A countdown that RUNS OUT must not cancel itself: the engine zeroes `timeleft` before hiding, and
/// the entry's OnHide guard reads exactly that. (Get this wrong and every logout in the field is
/// cancelled by its own dialog at t=0 — the character never leaves.)
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

/// The quit dialog is the camp dialog under another name, plus the one thing a quit can do without
/// the server: "Exit now". And `LOGOUT_CANCEL` (the server's cancel ack) takes either one down.
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

    // The server's own cancel ack hides whichever is up, without queueing anything further.
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

/// The reference's own OnShow guard on both live buttons: a menu re-opened over a running countdown
/// shows them dead, because re-arming a logout that is already counting is meaningless.
///
/// Opened with the micro button's form on purpose — the ESC key can't get you here at all, which is
/// the next test.
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

    // With the countdown gone, a re-opened menu has them back.
    s.run("HideUIPanel(GameMenuFrame)").unwrap();
    s.fire_event("LOGOUT_CANCEL", vec![]);
    s.run("ToggleGameMenu(1)").unwrap();
    assert!(s
        .eval::<bool>("return GameMenuButtonLogout:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// ESC with a countdown up never reaches the menu: the popup rung is FIRST in the ladder
/// (`StaticPopup_EscapePressed`, l.1482) and CAMP is `hideOnEscape`, so the press dismisses the
/// dialog — which, per the entry's own OnHide, calls the logout off. Pressing ESC to "get out of"
/// a logout you didn't mean is therefore the whole gesture, and it must not also open the menu.
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

/// The world map is subject to the menu like everything else (director-reported: it was "the only
/// one not blocked"). It was a plain toplevel showing itself with a bare `WorldMapFrame:Show()`,
/// so the panel manager never saw it and `CanOpenPanels()` never got a say; it is now the ref's own
/// `area = "full"` row and its toggle goes through ShowUIPanel.
///
/// Both directions matter, and the second is the one a bare `IsVisible()` check would miss: the map
/// must also VACATE the full-screen slot when it closes, or the stale slot refuses every panel
/// afterwards.
#[test]
fn the_world_map_cannot_open_behind_the_menu_and_gives_its_slot_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness_with(&[
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml", // the map's continent/zone pickers initialize into it at OnLoad
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The stock map's OnShow/OnHide call `UpdateMicroButtons` unguarded (1980), which the
        // micro menu defines over the action bar it sits in — the spellbook kit's cluster.
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

    // Opens normally, and takes the full-screen slot.
    s.run("ToggleWorldMap()").unwrap();
    assert!(s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(s
        .eval::<bool>("return GetFullScreenFrame():GetName() == \"WorldMapFrame\"")
        .unwrap());

    // Closing gives the slot back — a stale full-screen frame would block every later panel.
    s.run("ToggleWorldMap()").unwrap();
    assert!(!s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(
        s.eval::<bool>("return GetFullScreenFrame() == nil")
            .unwrap(),
        "the map vacated the full-screen slot"
    );

    // With the menu up, it must not open — from the M binding or from the micro button, both of
    // which are this same ToggleWorldMap.
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

    // And it opens again the moment the menu is gone.
    s.run("ToggleGameMenu(1)").unwrap();
    s.run("ToggleWorldMap()").unwrap();
    assert!(s.eval::<bool>("return WorldMapFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The other direction of the same rule (ref `ShowUIPanel` l.668-675): with the map holding the
/// full screen, nothing opens behind IT either — and ESC closes the map first, so the press that
/// would have opened the menu goes to the map instead (one eater per press).
#[test]
fn nothing_opens_behind_the_world_map_and_escape_closes_it_first() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness_with(&[
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        // The action-bar cluster the micro menu sits in — the stock map's OnShow/OnHide call
        // `UpdateMicroButtons` unguarded (1980).
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
        // The loot window is the reference's own since 1751 — see `test_ui::LOOT_UI` for what
        // each of these buys; `PartyFrame`'s MAX_PARTY_MEMBERS is needed at LOAD time.
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

    // ESC: the map's own rung eats the press (it stands in for the reference frame's OnKeyDown).
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

/// The bag row under an open menu, pinned at the QUAD level — because "greyed out" and "gone" are
/// the same thing to an `IsEnabled()` assertion, and the difference is what the director saw: the
/// first cut of `Disable_BagButtons` disabled buttons whose art was their NormalTexture, and a
/// disabled button with no DisabledTexture draws no state texture at all
/// (`ButtonState::region_visible`, the byte rule at `SetState 0x779790`) — so the backpack icon
/// vanished off the bar instead of dimming.
///
/// What must hold with the menu up: every one of the five buttons still DRAWS its art, and every
/// one is tinted grey. And on the way back out, tinted white again.
#[test]
fn the_bag_row_greys_under_the_menu_without_any_of_it_disappearing() {
    let _data = benilla_formats::wow_data_or_skip!();
    // ActionBar.xml goes AHEAD of the bag stack: it declares MainMenuBarArtFrame, which the bag
    // bar anchors into and seats itself above (`BenillaActionBarArt_SeatAbove`, nil-guarded in
    // BagFrame.xml precisely because most harnesses load no bar).
    let mut s = bag_harness_with(
        &[
            "Interface\\FrameXML\\Cooldown.xml",
            "Interface\\FrameXML\\ActionButtonTemplate.xml",
            "Interface\\FrameXML\\TextStatusBar.lua",
            "Interface\\FrameXML\\TextStatusBar.xml",
            "Interface\\FrameXML\\Fonts.xml",
            r"Interface\FrameXML\UIParent.xml",
            "ScrollTemplates.xml", // our scroll kit + the placeholder icon
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

    // The backpack icon + the four slot rings, by name.
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
    // The backpack icon specifically: still its own art, and carrying SetDesaturation's greyscale
    // flag to the renderer (before it, the grey was the ref's no-shader 0.5 tint).
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

/// A one-item backpack (the escape/bag tests' fixture, duplicated so this file is self-contained).
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

/// The menu wears the shared era-window scale on show (`ERA_WINDOW_SCALE`, OptionsFrame.xml —
/// the stand-in for the era client's UIParent px-per-unit): in the era the menu and the options
/// window draw at the SAME density (the menu has no scale of its own; options' checkFit caps at
/// 1), and ours drew the menu 28% larger relative to the options window until it rode the same
/// knob ("esc menu scale too big" — director, 2026-08-04). Loaded WITH OptionsFrame.xml, the
/// real load order; the bare-menu harness elsewhere exercises the `or 1` guard implicitly.
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
    let ran = s.eval::<f64>("return BENILLA_MENU_ONSHOW or -99").unwrap();
    println!("handler ran probe: {ran}");
    let k = s.eval::<f64>("return GameMenuFrame:GetScale()").unwrap();
    let want = s.eval::<f64>("return ERA_WINDOW_SCALE").unwrap();
    assert!((k - want).abs() < 1e-6, "menu scale {k} != knob {want}");
    assert!((want - 0.78).abs() < 1e-6, "the knob itself moved: {want}");
}
