//! The social window (`FriendsFrame.xml`), fed as `ui_social` feeds it: a pushed [`SocialState`]
//! snapshot, then the list event.

use benilla_ui::script::{FriendInfo, SocialRequest, SocialState, UiScript, WhoInfo};

/// The window's own manifest slice, in `load_default_ui` order.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_social_ui(&mut s);
    s
}

fn who(name: &str, level: u32, class: &str, zone: &str) -> WhoInfo {
    WhoInfo {
        name: name.to_string(),
        guild: String::new(),
        level,
        race: "Human".to_string(),
        class: class.to_string(),
        zone: zone.to_string(),
    }
}

fn who_name(s: &UiScript, row: u32) -> String {
    s.eval::<String>(&format!("return WhoFrameButton{row}Name:GetText()"))
        .unwrap()
}

fn friend(name: &str, level: u32, class: &str, area: &str, connected: bool) -> FriendInfo {
    FriendInfo {
        name: name.to_string(),
        level,
        class: class.to_string(),
        area: area.to_string(),
        connected,
        status: String::new(),
    }
}

/// Pushes a snapshot and fires the list event after it, as `feed_social` does.
fn push(s: &mut UiScript, state: SocialState, event: &str) {
    s.set_social(state);
    s.fire_event(event, Vec::new());
}

/// With no guild seated, `IsInGuild()` is nil and `FriendsFrame_OnShow`'s `InGuildCheck()` greys
/// the guild tab (`FriendsFrame.lua:78`, `:920-922`).
#[test]
fn the_window_opens_on_friends_with_the_guild_tab_disabled() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    assert!(s.eval::<bool>("return FriendsFrame:IsVisible()").unwrap());
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return FriendsFrameTitleText:GetText()")
            .unwrap(),
        "Friends List"
    );
    assert_eq!(
        s.eval::<i64>("return IsInGuild() and 1 or 0").unwrap(),
        0,
        "no guild is seated, which is what makes the next assertion mean something"
    );
    assert_eq!(
        s.eval::<i64>("return FriendsFrameTab3.isDisabled or 0")
            .unwrap(),
        1,
        "the guild tab is disabled, not absent"
    );
    // `ToggleFriendsFrame(3)` returns early without a guild (`FriendsFrame.lua:750`).
    s.run("ToggleFriendsFrame(3)").unwrap();
    assert!(
        !s.eval::<bool>("return GuildFrame:IsVisible()").unwrap(),
        "a guildless character cannot open the guild pane at all"
    );
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    // Big window tabs (20px ends) along the bottom; the Friends/Ignore pair is compact (16px).
    assert_eq!(
        s.eval::<f64>("return FriendsFrameTab1Left:GetWidth()")
            .unwrap(),
        20.0,
        "the window tab strip keeps the big tab art"
    );
    assert_eq!(
        s.eval::<f64>("return FriendsFrameToggleTab1Left:GetWidth()")
            .unwrap(),
        16.0,
        "the in-panel toggle pair is the compact tab art"
    );
    // The selected tab again closes the window (`FriendsFrame.lua:753-755`).
    s.run("ToggleFriendsFrame(1)").unwrap();
    assert!(!s.eval::<bool>("return FriendsFrame:IsVisible()").unwrap());
}

/// The tabs inherit through `FriendsFrameTabTemplate`, which adds only `<OnClick>`; handlers
/// replace per name (`0x76a0d0`), so the base template's `<OnShow>` fit still runs two hops down.
#[test]
fn the_social_tabs_fit_their_labels_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// The big tab's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Synchronous, like the app's `AtlasMeasurer`: the reference fits the tab inline.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    super::test_ui::load_social_ui(&mut s);
    s.run("ToggleFriendsFrame(1)").unwrap();
    s.resolve();

    for i in 1..=4 {
        let (label, width): (f64, f64) = s
            .eval(&format!(
                "return FriendsFrameTab{i}Text:GetStringWidth(), FriendsFrameTab{i}:GetWidth()"
            ))
            .unwrap();
        assert!(label > 0.0, "tab {i} measured its label");
        assert_eq!(
            width,
            label + SIDES,
            "tab {i} is its text plus the two end slices, from the base template's OnShow"
        );
        assert_ne!(width, 115.0, "tab {i} is still at the authored pre-fit");
    }
    assert!(s.errors().is_empty(), "no handler errors: {:?}", s.errors());
}

/// Fails if `GetFriendInfo`'s six returns come back out of order.
#[test]
fn friend_rows_render_the_online_and_offline_templates() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    push(
        &mut s,
        SocialState {
            friends: vec![
                FriendInfo {
                    status: "<AFK>".to_string(),
                    ..friend("Onerogue", 60, "Rogue", "Elwynn Forest", true)
                },
                friend("Twomage", 0, "", "", false),
            ],
            selected_friend: 1,
            ..Default::default()
        },
        "FRIENDLIST_UPDATE",
    );

    assert_eq!(
        s.eval::<String>("return FriendsFrameFriendButton1ButtonTextNameLocation:GetText()")
            .unwrap(),
        "Onerogue |cffffffff- Elwynn Forest|r <AFK>"
    );
    assert_eq!(
        s.eval::<String>("return FriendsFrameFriendButton1ButtonTextInfo:GetText()")
            .unwrap(),
        "Level 60 Rogue"
    );
    assert_eq!(
        s.eval::<String>("return FriendsFrameFriendButton2ButtonTextNameLocation:GetText()")
            .unwrap(),
        "|cff999999Twomage - Offline|r"
    );
    assert!(
        s.eval::<bool>("return FriendsFrameFriendButton2:IsVisible()")
            .unwrap(),
        "both rows shown"
    );
    assert!(
        !s.eval::<bool>("return FriendsFrameFriendButton3:IsVisible()")
            .unwrap(),
        "rows past the list are hidden"
    );
}

#[test]
fn the_friend_buttons_follow_the_selection() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();

    push(&mut s, SocialState::default(), "FRIENDLIST_UPDATE");
    for button in ["SendMessage", "GroupInvite", "RemoveFriend"] {
        assert!(
            !s.eval::<bool>(&format!(
                "return FriendsFrame{button}Button:IsEnabled() ~= 0"
            ))
            .unwrap(),
            "{button} is disabled with no friends"
        );
    }

    push(
        &mut s,
        SocialState {
            friends: vec![friend("Twomage", 0, "", "", false)],
            selected_friend: 1,
            ..Default::default()
        },
        "FRIENDLIST_UPDATE",
    );
    assert!(
        s.eval::<bool>("return FriendsFrameRemoveFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "an offline friend can still be removed"
    );
    assert!(
        !s.eval::<bool>("return FriendsFrameSendMessageButton:IsEnabled() ~= 0")
            .unwrap(),
        "…but not whispered"
    );
}

/// Remove Friend queues the row index (the app resolves the guid); Group Invite queues the name.
#[test]
fn the_friend_buttons_queue_their_verbs() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    push(
        &mut s,
        SocialState {
            friends: vec![
                friend("Onerogue", 60, "Rogue", "Elwynn Forest", true),
                friend("Twomage", 40, "Mage", "Westfall", true),
            ],
            selected_friend: 2,
            ..Default::default()
        },
        "FRIENDLIST_UPDATE",
    );
    let _ = s.take_social_requests();

    s.run("FriendsFrame_RemoveFriend()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::RemoveFriendIndex(2)]
    );

    s.run("FriendsFrame_GroupInvite()").unwrap();
    assert!(s.take_party_requests().iter().any(|r| matches!(
        r,
        benilla_ui::script::PartyRequest::InviteName(n) if n == "Twomage"
    )));

    // Stock `ChatFrame_OpenChat` leaves the text for the box's next `OnUpdate` to apply.
    s.run("FriendsFrame_SendMessage()").unwrap();
    s.tick(0.05);
    assert!(s
        .eval::<bool>("return ChatFrameEditBox:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<(String, String)>("return ChatFrameEditBox.chatType, ChatFrameEditBox.tellTarget")
            .unwrap(),
        ("WHISPER".to_string(), "Twomage".to_string())
    );
}

#[test]
fn the_ignore_list_is_the_other_half_of_tab_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    push(
        &mut s,
        SocialState {
            ignores: vec!["Spammer".to_string(), "Ninja".to_string()],
            selected_ignore: 1,
            ..Default::default()
        },
        "IGNORELIST_UPDATE",
    );

    s.run("FriendsFrameToggleTab2:Click()").unwrap();
    assert!(s
        .eval::<bool>("return IgnoreListFrame:IsVisible()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return FriendsFrameTitleText:GetText()")
            .unwrap(),
        "Ignore List"
    );
    assert_eq!(
        s.eval::<String>("return FriendsFrameIgnoreButton1ButtonTextName:GetText()")
            .unwrap(),
        "Spammer"
    );
    assert_eq!(
        s.eval::<String>("return FriendsFrameIgnoreButton2ButtonTextName:GetText()")
            .unwrap(),
        "Ninja"
    );

    let _ = s.take_social_requests();
    s.run("FriendsFrame_UnIgnore()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::DelIgnore("Spammer".to_string())]
    );

    // Through the ignore frame's own toggle pair: the friends frame's copy hides with its list.
    s.run("IgnoreFrameToggleTab1:Click()").unwrap();
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    // Stock `ShowIgnorePanel` only shows the window (`FriendsFrame.lua:781-788`).
    s.run("ShowIgnorePanel()").unwrap();
    assert!(s.eval::<bool>("return FriendsFrame:IsVisible()").unwrap());
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
}

/// The wire sends class before race, and `GetWhoInfo` returns race before class.
#[test]
fn who_rows_fill_their_columns() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    push(
        &mut s,
        SocialState {
            who: vec![WhoInfo {
                name: "Tigole".to_string(),
                guild: "Legacy of Steel".to_string(),
                level: 40,
                race: "Human".to_string(),
                class: "Rogue".to_string(),
                zone: "Westfall".to_string(),
            }],
            who_total: 1,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );

    assert!(s.eval::<bool>("return WhoFrame:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Name:GetText()")
            .unwrap(),
        "Tigole"
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Level:GetText()")
            .unwrap(),
        "40"
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Class:GetText()")
            .unwrap(),
        "Rogue",
        "class, not race — the API returns race first but the column is class"
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Variable:GetText()")
            .unwrap(),
        "Westfall",
        "the variable column defaults to Zone (dropdown entry 1)"
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameTotals:GetText()").unwrap(),
        "1 Person Found  ",
        "singular template for one hit"
    );
}

#[test]
fn the_who_dropdown_switches_the_variable_column() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    push(
        &mut s,
        SocialState {
            who: vec![WhoInfo {
                name: "Tigole".to_string(),
                guild: "Legacy of Steel".to_string(),
                level: 40,
                race: "Human".to_string(),
                class: "Rogue".to_string(),
                zone: "Westfall".to_string(),
            }],
            who_total: 1,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );
    let _ = s.take_social_requests();

    s.run("UIDropDownMenu_SetSelectedID(WhoFrameDropDown, 2); WhoList_Update()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Variable:GetText()")
            .unwrap(),
        "Legacy of Steel"
    );

    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::SortWho("level".to_string())]
    );
}

/// The reference's seven-slot sort chain (`SortWho`, `0x5ad890`): a repeated header reverses and
/// an earlier key breaks ties. Rows are read with no tick: `WHO_LIST_UPDATE` fires inside the call.
#[test]
fn the_who_headers_sort_and_a_repeated_click_reverses() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    push(
        &mut s,
        SocialState {
            who: vec![
                who("Galas", 60, "Warrior", "Elwynn Forest"),
                who("Erdrin", 12, "Mage", "Elwynn Forest"),
            ],
            who_total: 2,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );
    let _ = s.take_social_requests();
    assert_eq!(who_name(&s, 1), "Galas", "the server's order, unsorted");

    // Header 1 is Name (`FriendsFrame.xml:1313`).
    s.run("WhoFrameColumnHeader1:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Erdrin", "Name, ascending");
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::SortWho("name".to_string())],
        "and the app hears the click too"
    );

    s.run("WhoFrameColumnHeader1:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Galas", "the same header again reverses");

    // Header 3 is Level (`FriendsFrame.xml:1394`).
    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Erdrin", "Level, ascending");
    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Galas", "and Level reverses too");

    // Name, promoted from behind, keeps the direction it was left in.
    s.run("WhoFrameColumnHeader1:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Galas", "Name, still descending");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

#[test]
fn the_who_buttons_need_a_selected_row() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    let rows = vec![
        WhoInfo {
            name: "Tigole".to_string(),
            level: 40,
            ..Default::default()
        },
        WhoInfo {
            name: "Furor".to_string(),
            level: 41,
            ..Default::default()
        },
    ];
    push(
        &mut s,
        SocialState {
            who: rows.clone(),
            who_total: 2,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );
    assert!(
        !s.eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "nothing selected yet"
    );

    s.run("WhoFrameButton2:Click()").unwrap();
    assert!(s
        .eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
        .unwrap());
    let _ = s.take_social_requests();
    s.run("WhoFrameAddFriendButton:Click()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::AddFriend("Furor".to_string())]
    );

    push(
        &mut s,
        SocialState {
            who: rows,
            who_total: 2,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );
    // Stock `WhoList_Update` keys the buttons on `selectedWho`, which an answer never clears.
    assert!(
        s.eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "a fresh answer keeps the selection"
    );
}

/// The who frame's show and hide route answers with `SetWhoToUI` (`FriendsFrame.xml:1711-1716`).
#[test]
fn showing_the_who_frame_claims_the_next_answer() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    assert!(
        s.take_social_requests()
            .contains(&SocialRequest::SetWhoToUi(true)),
        "showing the frame routes results to it"
    );
    s.run("HideUIPanel(FriendsFrame)").unwrap();
    assert!(
        s.take_social_requests()
            .contains(&SocialRequest::SetWhoToUi(false)),
        "hiding it routes them back to chat"
    );
}

#[test]
fn the_who_edit_box_sends_its_filter() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    let _ = s.take_social_requests();
    s.run("WhoFrameEditBox:SetText(\"z-\\\"Elwynn Forest\\\" 1-10\")")
        .unwrap();
    s.run("WhoFrameEditBox_OnEnterPressed()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::Who("z-\"Elwynn Forest\" 1-10".to_string())]
    );
}

#[test]
fn add_friend_without_a_target_opens_the_name_dialog() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    s.run("FriendsFrameAddFriendButton:Click()").unwrap();

    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "no cooperable target → the dialog"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1EditBox:IsVisible()")
            .unwrap(),
        "and it has an edit box"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Enter name of friend to add:"
    );

    let _ = s.take_social_requests();
    s.run("StaticPopup1EditBox:SetText(\"Onerogue\")").unwrap();
    // Through the button: the stock `OnAccept` reads `this:GetParent()`, which a click seats.
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::AddFriend("Onerogue".to_string())]
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// The stock `ADD_FRIEND`'s `OnHide` empties the box (`StaticPopup.lua:788-793`).
#[test]
fn the_name_dialog_reopens_empty() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    s.run("StaticPopup_Show(\"ADD_FRIEND\")").unwrap();
    s.run("StaticPopup1EditBox:SetText(\"Onerogue\")").unwrap();
    s.run("StaticPopup_Hide(\"ADD_FRIEND\")").unwrap();
    s.run("StaticPopup_Show(\"ADD_FRIEND\")").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1EditBox:GetText()")
            .unwrap(),
        ""
    );
}

/// A who hit has no unit token, so the shared `FRIEND` menu is addressed by name.
#[test]
fn right_clicking_a_who_row_opens_the_friend_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    push(
        &mut s,
        SocialState {
            who: vec![WhoInfo {
                name: "Tigole".to_string(),
                level: 40,
                ..Default::default()
            }],
            who_total: 1,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );

    s.run("WhoFrameButton1:Click(\"RightButton\")").unwrap();
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the menu opens"
    );
    assert_eq!(
        s.eval::<String>("return FriendsDropDown.name").unwrap(),
        "Tigole",
        "addressed by name, not by a unit token"
    );
    assert!(
        !s.eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "right-click does not select"
    );

    let whisper = r#"
        for i = 1, UIDROPDOWNMENU_MAXBUTTONS do
            local b = getglobal("DropDownList1Button" .. i)
            if b and b:IsVisible() and b.value == "WHISPER" then b:Click() return 1 end
        end
        return nil"#;
    assert_eq!(
        s.eval::<Option<i64>>(whisper).unwrap(),
        Some(1),
        "the menu has a Whisper row"
    );
    // The stock `WHISPER` row is `ChatFrame_SendTell(name)`, applied at the box's next `OnUpdate`.
    s.tick(0.05);
    assert_eq!(
        s.eval::<(String, String)>("return ChatFrameEditBox.chatType, ChatFrameEditBox.tellTarget")
            .unwrap(),
        ("WHISPER".to_string(), "Tigole".to_string())
    );
}

/// Stock `FriendsFrame_ShowDropdown` opens only for a connected friend (`FriendsFrame.lua:35-37`).
#[test]
fn right_clicking_an_offline_friend_opens_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    push(
        &mut s,
        SocialState {
            friends: vec![friend("Twomage", 0, "", "", false)],
            selected_friend: 1,
            ..Default::default()
        },
        "FRIENDLIST_UPDATE",
    );
    s.run("FriendsFrameFriendButton1:Click(\"RightButton\")")
        .unwrap();
    assert!(
        !s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "nothing to offer an offline friend"
    );
}

/// `FriendsList_Update` reads the selection back right after `SetSelectedFriend`.
#[test]
fn selecting_a_row_reads_back_in_the_same_tick() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ToggleFriendsFrame(1)").unwrap();
    push(
        &mut s,
        SocialState {
            friends: vec![
                friend("Onerogue", 60, "Rogue", "Elwynn Forest", true),
                friend("Twomage", 40, "Mage", "Westfall", true),
            ],
            selected_friend: 1,
            ..Default::default()
        },
        "FRIENDLIST_UPDATE",
    );
    let _ = s.take_social_requests();

    s.run("FriendsFrameFriendButton2:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetSelectedFriend()").unwrap(),
        2,
        "the getter answers this tick, not after the app's next push"
    );
    assert!(s
        .take_social_requests()
        .contains(&SocialRequest::SelectFriend(2)));
}

/// `WhoListScrollFrame` is 287 tall (`FriendsFrame.xml:1661`), so its overflow `n * 16 - 287` is
/// 15px short of the bar's `(n - 17) * 16`; the reference stores the scroll value unclamped
/// (`0x786db0`), so the bar's end reaches the last row.
#[test]
fn the_who_list_reaches_its_last_row_at_the_bars_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("ShowWhoPanel()").unwrap();
    let who: Vec<WhoInfo> = (1..=49)
        .map(|i| WhoInfo {
            name: format!("Who{i:02}"),
            guild: String::new(),
            level: 60,
            race: "Human".to_string(),
            class: "Warrior".to_string(),
            zone: "Elwynn Forest".to_string(),
        })
        .collect();
    push(
        &mut s,
        SocialState {
            who,
            who_total: 49,
            ..Default::default()
        },
        "WHO_LIST_UPDATE",
    );
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The control: the overflow is shorter than the bar's range.
    let (_, bar_max) = s
        .eval::<(f64, f64)>("return WhoListScrollFrameScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!(bar_max, 512.0, "(49 − 17) × 16");
    let overflow = s
        .eval::<f64>("return WhoListScrollFrame:GetVerticalScrollRange()")
        .unwrap();
    assert_eq!(
        overflow, 497.0,
        "49 × 16 − 287: the frame is taller than its rows"
    );

    s.run("WhoListScrollFrameScrollBar:SetValue(512)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(WhoListScrollFrame)")
            .unwrap(),
        32
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameButton1Name:GetText()")
            .unwrap(),
        "Who33"
    );
    assert_eq!(
        s.eval::<String>("return WhoFrameButton17Name:GetText()")
            .unwrap(),
        "Who49",
        "the last hit is on the last row"
    );
}
