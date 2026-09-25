//! The unit right-click popups, reached by right-clicking a target frame, a chat name or the pet
//! frame through the real hit paths. All drive `UnitPopup.lua`; a chat name goes through
//! `SetItemRef`'s player branch to the FRIEND dropdown in `FriendsFrame.xml`.

use benilla_ui::script::{FollowRequest, PartyRequest, UiScript, UnitState};

use super::test_ui::load_ui as load_xml;

/// The files the popups need, in `benilla.toc`'s order, ending with `FriendsFrame.xml`, home of
/// `FriendsFrame_ShowDropdown` and `FriendsDropDown`.
fn load_popup_frames(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        // `SmallMoneyFrame_OnLoad`, which the chain's StaticPopup money rows call at load.
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        // `StaticPopupDialogs` and `PanelTemplates_*`, which FriendsFrame.xml reaches at load.
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, read at file scope below
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\ItemRef.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
        "ScrollTemplates.xml",
        "Interface\\FrameXML\\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\FriendsFrame.xml",
        // Declares `ChatFrameEditBox`, which the rename dialog's OnHide refocuses.
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// The FRIEND and PLAYER menus' labels and the target-frame newbie tip, as the stock
/// `GlobalStrings.lua` has them.
fn bake_strings(s: &UiScript) {
    s.run(
        r#"
        WHISPER = "Whisper"
        PARTY_INVITE = "Invite"
        TRADE = "Trade"
        DUEL = "Duel"
        -- The inspect row's label: the value at `Interface\FrameXML\GlobalStrings.lua:2327` off
        -- the 1.12.1 patch chain, which is what the app itself runs at boot
        -- (`load_global_strings`); this stub only stands in for it here.
        INSPECT = "Inspect"
        -- The follow row's label, likewise the real
        -- `GlobalStrings.lua:1981` value off the 1.12.1 patch chain.
        FOLLOW = "Follow"
        CANCEL = "Cancel"
        RAID_TARGET_ICON = "Raid Target Icon"
        -- The newbie tooltip the stock unit frame raises on a HOVER, which every test in this file
        -- takes on its way to a right-click. `UnitFrame_OnEnter` (ref `UnitFrame.lua:58-65`) runs
        -- the detailed-tip branch whenever `SHOW_NEWBIE_TIPS == "1"` — 1.12's own default
        -- (`UIOptionsFrame.lua:100`, which the app loads off the patch chain; this harness loads
        -- no options file, so `SHOW_NEWBIE_TIPS` is nil and the branch is not reached
        -- here). For a player-controlled target that is not us that branch calls
        -- `GameTooltip_AddNewbieTip(PLAYER_OPTIONS_LABEL, 1, 1, 1, NEWBIE_TOOLTIP_PLAYEROPTIONS)`.
        -- Both are nil in a bare harness, and `GameTooltip:SetText(nil)` raises. Verbatim from the
        -- real `Interface\FrameXML\GlobalStrings.lua` off the 1.12.1 chain (l.3081 and l.2755).
        -- The stock unit frame reaches this arm, so both strings must exist.
        PLAYER_OPTIONS_LABEL = "Player Options"
        NEWBIE_TOOLTIP_PLAYEROPTIONS = "Right-click to bring up special commands for interacting with another player. You can inspect their equipment, issue a party invite, initiate a trade, or challenge a player to a duel. A group leader can promote or remove that player from the group."
    "#,
    )
    .unwrap();
}

/// Right-clicking a chat player name opens the FRIEND dropdown (`FriendsFrame_ShowDropdown`),
/// whose Invite invites by name; a left-click on the name whispers instead.
#[test]
fn chat_name_right_click_opens_the_invite_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "no menu before any click"
    );

    // Right-click the |Hplayer:Bob|h link the chat frame emits (ChatFrame OnHyperlinkClick →
    // SetItemRef(link, text, "RightButton")).
    s.run(r#"SetItemRef("player:Bob", "|Hplayer:Bob|h[Bob]|h", "RightButton")"#)
        .unwrap();
    s.resolve();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the chat-name right-click opens the FRIEND dropdown"
    );
    // Title, Whisper, Invite, Target, Cancel: the FRIEND menu (`UnitPopup.lua:74`) with its two
    // guild rows hidden (no guild, no guild frame).
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        5,
        "title + Whisper + Invite + Target + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button1:GetText()")
            .unwrap(),
        "Bob",
        "the clicked name titles the menu"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Invite"
    );

    // Click Invite through the real hit path → UnitPopup_OnClick → InviteByName("Bob").
    let (ix, iy) = s
        .eval::<(f64, f64)>("return DropDownList1Button3:GetCenter()")
        .unwrap();
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::InviteName("Bob".into())],
        "Invite on a chat name queues an invite-by-name"
    );

    // A left-click on the name whispers instead: the stock `ChatFrame_SendTell` opens the box in
    // WHISPER mode at the name.
    s.run(r#"SetItemRef("player:Carol", "|Hplayer:Carol|h[Carol]|h", "LeftButton")"#)
        .unwrap();
    s.tick(0.05);
    assert_eq!(
        s.eval::<(String, String)>("return ChatFrameEditBox.chatType, ChatFrameEditBox.tellTarget")
            .unwrap(),
        ("WHISPER".to_string(), "Carol".to_string()),
        "left-click a chat name opens a whisper"
    );
    assert!(
        s.eval::<bool>("return ChatFrameEditBox:IsVisible()")
            .unwrap(),
        "and the box is open"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Solo, right-clicking a friendly player target opens the PLAYER menu; Invite invites the unit.
#[test]
fn solo_target_right_click_invites_a_player() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    // A same-faction friendly PLAYER target (is_player + reaction 5 → UnitCanCooperate == 1).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "a friendly player target opens the PLAYER menu solo"
    );
    // Title, Whisper, Inspect, Invite, Trade, Follow, Duel, Cancel: the PLAYER menu in
    // `UnitPopup.lua:72`'s order, the raid-target row hidden without a party.
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        8,
        "title + Whisper + Inspect + Invite + Trade + Follow + Duel + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Inspect"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button4:GetText()")
            .unwrap(),
        "Invite"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Trade"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button6:GetText()")
            .unwrap(),
        "Follow"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button7:GetText()")
            .unwrap(),
        "Duel"
    );

    // Click Invite → UnitPopup_OnClick → InviteToParty("target") → InviteUnit.
    let (ix, iy) = s
        .eval::<(f64, f64)>("return DropDownList1Button4:GetCenter()")
        .unwrap();
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.take_party_requests(),
        vec![PartyRequest::InviteUnit("target".into())],
        "Invite on a player target queues an invite by unit"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking the Trade row calls `InitiateTrade("target")` (`UnitPopup.lua:544`).
#[test]
fn solo_target_trade_click_queues_an_initiate() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button5:GetText()")
            .unwrap(),
        "Trade",
        "Trade is the fifth row (title + Whisper + Inspect + Invite + Trade) — Inspect sits \
         ahead of it"
    );

    let (tx, ty) = s
        .eval::<(f64, f64)>("return DropDownList1Button5:GetCenter()")
        .unwrap();
    s.mouse_button(tx as f32, ty as f32, "LeftButton", true);
    s.mouse_button(tx as f32, ty as f32, "LeftButton", false);
    assert_eq!(
        s.take_trade_initiates(),
        vec!["target".to_string()],
        "clicking Trade queues an initiate against the target token"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking Follow calls `FollowByName("Ally", 1)` (`UnitPopup.lua:623`): the `1` asks for an
/// exact match, so the menu never prefix-matches onto a bystander as `/follow rag` may.
#[test]
fn solo_target_follow_click_queues_an_exact_by_name_follow() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();
    assert!(s.errors().is_empty(), "load errors: {:?}", s.errors());

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button6:GetText()")
            .unwrap(),
        "Follow",
        "Follow is the sixth row (title + Whisper + Inspect + Invite + Trade + Follow)"
    );

    let (fx, fy) = s
        .eval::<(f64, f64)>("return DropDownList1Button6:GetCenter()")
        .unwrap();
    s.mouse_button(fx as f32, fy as f32, "LeftButton", true);
    s.mouse_button(fx as f32, fy as f32, "LeftButton", false);
    assert_eq!(
        s.take_follow_requests(),
        vec![FollowRequest::ByName {
            name: "Ally".into(),
            exact: true,
        }],
        "the popup follows by NAME and exactly — not by unit token, and not prefix-matched"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Clicking Inspect calls `InspectUnit("target")` (`UnitPopup.lua:552`), stubbed here;
/// `inspect_tests.rs` covers the window.
#[test]
fn solo_target_inspect_click_reaches_inspect_unit() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_strings(&s);
    load_popup_frames(&s);
    s.run("BENILLA_TEST_INSPECTED = nil; function InspectUnit(unit) BENILLA_TEST_INSPECTED = unit end")
        .unwrap();

    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Ally".into()),
            health: 40,
            max_health: 40,
            is_player: true,
            player_controlled: true,
            reaction: 5,
            ..UnitState::default()
        }),
    );
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    s.resolve();

    let (cx, cy) = s
        .eval::<(f64, f64)>("return TargetFrame:GetCenter()")
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Inspect",
        "Inspect is the third row (title + Whisper + Inspect)"
    );

    let (ix, iy) = s
        .eval::<(f64, f64)>("return DropDownList1Button3:GetCenter()")
        .unwrap();
    s.mouse_button(ix as f32, iy as f32, "LeftButton", true);
    s.mouse_button(ix as f32, iy as f32, "LeftButton", false);
    assert_eq!(
        s.eval::<String>("return BENILLA_TEST_INSPECTED").unwrap(),
        "target",
        "clicking Inspect calls InspectUnit against the target token"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The PET menu ─────────────────────────────────────────────────────────────────────

/// The pet menu's files, with `StaticPopup.xml` for the rename and abandon dialogs.
fn load_pet_menu_frames(s: &UiScript) {
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\BasicControls.xml", // `TEXT`, read at file scope below
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIMenu.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        "Interface\\FrameXML\\PetFrame.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ActionBarFrame.xml",
        "Interface\\FrameXML\\BonusActionBarFrame.xml",
        // Declares `ChatFrameEditBox`, which the rename dialog's OnHide refocuses.
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(s, file);
    }
}

/// The PET rows' labels and both dialogs' text as the stock strings have them
/// (`GlobalStrings.lua:3`, `:3028-3052`), and a `ToggleCharacter` that records its panel.
fn bake_pet_strings(s: &UiScript) {
    s.run(
        r#"
        PET_PAPERDOLL = "Pet Details"
        PET_RENAME = "Rename"
        PET_ABANDON = "Abandon"
        PET_DISMISS = "Dismiss"
        CANCEL = "Cancel"
        OKAY = "Okay"
        ACCEPT = "Accept"
        YES = "Yes"
        NO = "No"
        ABANDON_PET = "Are you sure you want to permanently abandon your pet?"
        PET_RENAME_LABEL = "Enter desired name of pet:"
        PET_RENAME_CONFIRMATION = "Name your pet '%s'?"
        -- ToggleCharacter lives in CharacterFrame.xml, which the paperdoll row needs and this
        -- isolation prefix does not load. Recorded rather than stubbed away, so the row's click
        -- is still proven to reach the right panel name.
        function ToggleCharacter(tab) BENILLA_TEST_TOGGLED = tab end
    "#,
    )
    .unwrap();
}

/// A pet that exists, so `UnitExists("pet")` passes the dropdown's own gate.
fn a_pet(s: &mut UiScript, name: &str) {
    // A player first: `PetFrame` is a child of `PlayerFrame` (`PetFrame.xml:4`), and a unit
    // frame whose unit does not exist is hidden, taking the pet frame with it.
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Me".into()),
            health: 100,
            max_health: 100,
            level: 60,
            is_player: true,
            player_controlled: true,
            ..UnitState::default()
        }),
    );
    s.fire_event(
        "UNIT_HEALTH",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
    s.set_unit(
        "pet",
        Some(UnitState {
            exists: true,
            name: Some(name.into()),
            health: 100,
            max_health: 100,
            level: 20,
            ..UnitState::default()
        }),
    );
    s.fire_event(
        "UNIT_PET",
        vec![benilla_ui::script::ScriptValue::Str("player".into())],
    );
}

/// Right-clicks the centre of the pet frame's hit rect, the portrait half (`PetFrame.xml:15-17`).
/// The frame's own centre lies on `PetFrameHealthBar` (`PetFrame.xml:125-135`), mouse-enabled by
/// its `TextStatusBar` OnEnter and forwarding no `OnMouseUp`, so a click there opens nothing, in
/// the reference too.
fn right_click_the_pet_frame(s: &mut UiScript) {
    s.resolve();
    let (cx, cy) = s
        .eval::<(f64, f64)>(
            "local il, ir, it, ib = PetFrame:GetHitRectInsets() \
             return (PetFrame:GetLeft() + il + PetFrame:GetRight() - ir) / 2, \
                    (PetFrame:GetBottom() + ib + PetFrame:GetTop() - it) / 2",
        )
        .unwrap();
    s.mouse_button(cx as f32, cy as f32, "RightButton", true);
    s.mouse_button(cx as f32, cy as f32, "RightButton", false);
    s.resolve();
}

/// A hunter's pet shows Abandon and hides Dismiss, a warlock's demon the reverse:
/// `PetCanBeAbandoned` forks the menu (`UnitPopup.lua:402-417`).
#[test]
fn the_pet_menu_forks_between_abandon_and_dismiss() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");

    // A freshly tamed hunter pet: both bits set.
    s.set_pet_menu(true, true);
    right_click_the_pet_frame(&mut s);
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "right-clicking the pet frame opens the PET menu"
    );
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        5,
        "title + Pet Details + Rename + Abandon + Cancel — no Dismiss"
    );
    for (row, text) in [
        (2, "Pet Details"),
        (3, "Rename"),
        (4, "Abandon"),
        (5, "Cancel"),
    ] {
        assert_eq!(
            s.eval::<String>(&format!("return DropDownList1Button{row}:GetText()"))
                .unwrap(),
            text
        );
    }

    // The same pet after one rename: the server clears only the rename bit, so only that row goes.
    s.run("CloseDropDownMenus()").unwrap();
    s.set_pet_menu(true, false);
    right_click_the_pet_frame(&mut s);
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        4,
        "title + Pet Details + Abandon + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button3:GetText()")
            .unwrap(),
        "Abandon",
        "Rename is gone and Abandon has moved up into its row"
    );

    // A warlock's demon: neither bit. The menu flips whole.
    s.run("CloseDropDownMenus()").unwrap();
    s.set_pet_menu(false, false);
    right_click_the_pet_frame(&mut s);
    assert_eq!(
        s.eval::<i64>("return DropDownList1.numButtons").unwrap(),
        3,
        "title + Dismiss + Cancel"
    );
    assert_eq!(
        s.eval::<String>("return DropDownList1Button2:GetText()")
            .unwrap(),
        "Dismiss"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Dismiss sends at once; Abandon waits behind the `ABANDON_PET` confirm (`UnitPopup.lua:590-593`).
#[test]
fn dismiss_sends_immediately_and_abandon_waits_for_the_confirm() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Snuffles");

    // A demon: Dismiss is row 2, and clicking it queues the verb with no dialog in between.
    s.set_pet_menu(false, false);
    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 2);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Dismiss asks nothing"
    );
    assert_eq!(s.take_pet_gives_up(), (0, 1), "one dismiss, no abandon");

    // A hunter pet: Abandon is row 4, and clicking it only opens the confirm.
    s.set_pet_menu(true, true);
    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 4);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Abandon opens the ABANDON_PET confirm"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Are you sure you want to permanently abandon your pet?"
    );
    assert_eq!(
        s.take_pet_gives_up(),
        (0, 0),
        "and sends NOTHING until it is accepted"
    );

    click_frame(&mut s, "StaticPopup1Button2");
    assert_eq!(s.take_pet_gives_up(), (0, 0));
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 4);
    click_frame(&mut s, "StaticPopup1Button1");
    assert_eq!(s.take_pet_gives_up(), (1, 0), "one abandon");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The rename chains two dialogs, `RENAME_PET` then `PETRENAMECONFIRM` (`StaticPopup.lua:1069`,
/// `:365`), and only the confirm sends. The confirm opens while the name dialog is still up, so
/// it lands in the second popup instance.
#[test]
fn renaming_a_pet_reads_the_name_back_before_sending_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");
    s.set_pet_menu(true, true);

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 3);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Enter desired name of pet:"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1EditBox:IsVisible()")
            .unwrap(),
        "the name dialog carries an edit box"
    );

    s.run(r#"StaticPopup1EditBox:SetText("Rexxar")"#).unwrap();
    click_frame(&mut s, "StaticPopup1Button1");
    assert_eq!(
        s.take_pet_renames(),
        Vec::<String>::new(),
        "accepting the NAME dialog sends nothing — it only asks again"
    );
    assert!(
        s.eval::<bool>("return StaticPopup2:IsVisible()").unwrap(),
        "the confirm opens in the second instance, while the first is still up"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup2Text:GetText()")
            .unwrap(),
        "Name your pet 'Rexxar'?",
        "and it reads the typed name back"
    );

    click_frame(&mut s, "StaticPopup2Button1");
    assert_eq!(
        s.take_pet_renames(),
        vec!["Rexxar".to_string()],
        "only the CONFIRM sends, and it sends the name from the box"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The Pet Details row calls `ToggleCharacter("PetPaperDollFrame")` (`UnitPopup.lua:594-595`).
#[test]
fn the_pet_details_row_opens_the_pet_paper_doll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    bake_pet_strings(&s);
    load_pet_menu_frames(&s);
    a_pet(&mut s, "Bruce");
    s.set_pet_menu(true, true);

    right_click_the_pet_frame(&mut s);
    click_row(&mut s, 2);
    assert_eq!(
        s.eval::<String>("return BENILLA_TEST_TOGGLED").unwrap(),
        "PetPaperDollFrame"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Click an open dropdown row through the real hit path.
fn click_row(s: &mut UiScript, row: u32) {
    click_frame(s, &format!("DropDownList1Button{row}"));
}

/// Click any named frame through the real hit path.
fn click_frame(s: &mut UiScript, name: &str) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!("return {name}:GetCenter()"))
        .unwrap();
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    s.resolve();
}
