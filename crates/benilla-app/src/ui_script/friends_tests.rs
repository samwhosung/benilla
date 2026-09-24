//! The social window (decision 0668, `FriendsFrame.xml`): the tab strip, the two list tabs and
//! their toggle pair, the who list's columns, and the verbs each button queues — driven exactly
//! as `ui_social`'s feed drives it (a pushed [`SocialState`] snapshot, then the list event).
//!
//! What these guard that the Rust-side unit tests structurally cannot: the *window* is Lua over a
//! snapshot, so a getter whose returns are in the wrong order, a row template whose FontString is
//! misnamed, or a button wired to the wrong verb are all invisible to `ui_social`'s tests and
//! green in the parse sweep. Each test here fails on exactly one of those.

use benilla_ui::script::{FriendInfo, SocialRequest, SocialState, UiScript, WhoInfo};

/// The window's own manifest slice, in `load_default_ui` order.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    super::test_ui::load_social_ui(&mut s);
    s
}

/// One `/who` row as the app's feed would have resolved it.
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

/// The name painted into who row `row`.
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

/// Push a snapshot and fire the list event that follows it, exactly as `feed_social` does.
fn push(s: &mut UiScript, state: SocialState, event: &str) {
    s.set_social(state);
    s.fire_event(event, Vec::new());
}

/// The window loads, opens on the Friends tab, and the GUILD tab comes up disabled — the
/// GUILDLESS case of `InGuildCheck`.
///
/// **This assertion changed meaning with decision 1257 and was kept rather than deleted.** It used
/// to pin a hardcoded `PanelTemplates_DisableTab(this, 3)`, because there was no guild arc and the
/// tab was permanently dead. The tab is live now, and this is the other half of its law: with no
/// guild seated in the engine, `IsInGuild()` answers nil, `FriendsFrame_OnShow` runs
/// `InGuildCheck()`, and the tab greys — which is exactly what the reference does for a character
/// in no guild. The IN-guild half is `guild_tests`. Note that this file seats no guild fixture at
/// all: it is asserting against the REAL `script::guild` bindings' empty state, which is the state
/// every character starts a session in.
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
    // …and the tab refuses to be reached by NUMBER either, which is the second of the two locks
    // (`ToggleFriendsFrame`'s own `tab == 3 and not IsInGuild()` early-out).
    s.run("ToggleFriendsFrame(3)").unwrap();
    assert!(
        !s.eval::<bool>("return GuildFrame:IsVisible()").unwrap(),
        "a guildless character cannot open the guild pane at all"
    );
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    // The two tab kinds are different templates and must not be swapped: the strip along the
    // bottom is the big window tab (20px end slices), the Friends/Ignore pair inside tab 1 is the
    // ref's compact TabButtonTemplate (16px). Getting this wrong is invisible to every behaviour
    // assertion — it only shows up as the wrong-looking window.
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
    // …and toggling the same tab again closes the window (the ref's own second branch).
    s.run("ToggleFriendsFrame(1)").unwrap();
    assert!(!s.eval::<bool>("return FriendsFrame:IsVisible()").unwrap());
}

/// **The social window's tabs fit their labels on the first show — through TWO inheritance hops.**
///
/// `FriendsFrameTab1..4` inherit `FriendsFrameTabTemplate`, which inherits
/// `CharacterFrameTabButtonTemplate` (the reference's own file, on the chain since 1993). The
/// middle template declares an `<OnClick>` and nothing else, and handler replacement is **per
/// handler name** (`0x76a0d0`) — so the base template's
/// `<OnShow>` fit still runs, two hops down. That is the arrangement this pins: a row of tabs
/// still wearing the base template's authored 115 would mean the OnShow was lost on the way.
#[test]
fn the_social_tabs_fit_their_labels_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `2 * FriendsFrameTab1Left:GetWidth()` — the big tab's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The app installs `AtlasMeasurer`; the reference's fit is inline, so a harness that models
    // the async round trip models a configuration the app does not have (1848's correction).
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

/// A friend row shows name/zone/status on its top line and "Level N Class" underneath; an
/// OFFLINE friend takes the greyed offline template instead. This is the test that fails if
/// `GetFriendInfo`'s six returns ever come back in the wrong order.
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

/// The three friend buttons follow the SELECTED friend: an offline one can be removed but not
/// whispered or invited, and an empty list disables all three.
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

/// Remove Friend addresses the selected ROW (the wire removes by guid, which the app resolves
/// from this index), and Group Invite addresses the NAME.
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

    // Send Message is the reference's `ChatFrame_OpenChat("/w Twomage ")`: the box shows with
    // the text PENDING (`editBox.setText = 1`), applied by its next OnUpdate, whose OnTextSet
    // runs the parse that turns the line into a whisper to the friend (1959).
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

/// The Ignore toggle-tab swaps tab 1's list without leaving the tab, and the ignore rows render
/// their names. `ShowIgnorePanel()` (what `/ignore` with no name runs) lands on the same list.
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

    // Remove Player un-ignores the selected name.
    let _ = s.take_social_requests();
    s.run("FriendsFrame_UnIgnore()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::DelIgnore("Spammer".to_string())]
    );

    // Back to the friends half — through the IGNORE frame's own Friends tab, since the friends
    // frame's copy is hidden with it (the ref ships the pair twice for exactly this reason).
    s.run("IgnoreFrameToggleTab1:Click()").unwrap();
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
    // The reference's `ShowIgnorePanel` shows the WINDOW (its tab switch is commented out in the
    // stock file), so the list that was up stays up — the friends list here (1959).
    s.run("ShowIgnorePanel()").unwrap();
    assert!(s.eval::<bool>("return FriendsFrame:IsVisible()").unwrap());
    assert!(s
        .eval::<bool>("return FriendsListFrame:IsVisible()")
        .unwrap());
}

/// The who list fills its four columns, the variable column follows the dropdown, and the totals
/// line uses the singular/plural template. The class/race swap between the wire and the Lua API
/// makes the column order worth pinning.
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

/// The variable column really is variable: picking Guild re-paints it and sorts by that key.
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

    // A column header sorts by its own key.
    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(
        s.take_social_requests(),
        vec![SocialRequest::SortWho("level".to_string())]
    );
}

/// **B365's retest, pinned.** The Who list's column headers sort, the same header clicked twice
/// REVERSES, and the earlier click survives as a tie-breaker — the reference's seven-slot chain
/// (`SortWho 0x5ad890`, decision 2030).
///
/// What makes this the *window's* test rather than the chain's: every assertion reads the painted
/// row straight after `Click()`, with **no feed tick in between**. That can only pass if
/// `WHO_LIST_UPDATE` fires synchronously from inside the binding, the way `SignalEvent` does — the
/// half of the bug the director could see, since our old sort redrew a tick later and only ever
/// ascended.
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

    // Header 3 is Level (`:1394`) — ascending puts the level 12 first.
    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Erdrin", "Level, ascending");
    s.run("WhoFrameColumnHeader3:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Galas", "and Level reverses too");

    // Back to Name: promoted from behind, it keeps the descending direction it was left in
    // rather than flipping — the chain's memory, not a per-click toggle.
    s.run("WhoFrameColumnHeader1:Click()").unwrap();
    assert_eq!(who_name(&s, 1), "Galas", "Name, still descending");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The who buttons need a selected row, and selecting one enables both. A fresh answer clears
/// the selection — row 3 of the last query is not row 3 of this one.
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
    // The reference keeps the selection across answers: `WhoList_Update` enables the buttons
    // whenever `WhoFrame.selectedWho` is set, and its WHO_LIST_UPDATE arm clears nothing. (Our
    // transcription dropped it; 1959.)
    assert!(
        s.eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "a fresh answer keeps the selection"
    );
}

/// The Who frame tells the engine where the NEXT answer goes: showing it claims the results,
/// hiding it hands them back to the chat frame. Without this, a `/who` typed with the window
/// closed would print nothing at all.
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

/// The `/who` edit box sends its filter verbatim — the string the app parses into wire fields.
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

/// The Add Friend button with no friendly target opens the name-entry dialog — the first
/// customer of the popup engine's `hasEditBox` capability. Accepting sends what was typed.
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
    // Through the button: the stock dialog's OnAccept reads `this:GetParent()`, which only a
    // real click seats.
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

/// A re-opened dialog starts EMPTY. Left filled, a reflexive Enter would befriend whoever was
/// typed last time.
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

/// Right-clicking a `/who` row opens the shared FRIEND menu addressed by NAME — the point of
/// that menu, since a who hit has no unit token behind it. Whisper and Invite are the rows that
/// survive UnitPopup's gating, and each one acts on the name.
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
    // …and a right-click must NOT also select the row (the ref's two branches are exclusive).
    assert!(
        !s.eval::<bool>("return WhoFrameAddFriendButton:IsEnabled() ~= 0")
            .unwrap(),
        "right-click does not select"
    );

    // Whisper acts on the name.
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
    // The stock WHISPER row is `ChatFrame_SendTell(name)`: the chat box opens on the tell, its
    // pending text applied by the box's next OnUpdate (1959).
    s.tick(0.05);
    assert_eq!(
        s.eval::<(String, String)>("return ChatFrameEditBox.chatType, ChatFrameEditBox.tellTarget")
            .unwrap(),
        ("WHISPER".to_string(), "Tigole".to_string())
    );
}

/// An OFFLINE friend's right-click opens nothing — there is no verb to offer them, which is the
/// ref's own `connected` gate on the whole menu.
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

/// Selecting a row queues the selection to the app AND reads back immediately — the same-tick
/// read `FriendsList_Update` does right after `SetSelectedFriend`.
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

/// **B363 — the who list reaches its last rows.** Liho's `/who` found 49, showed 17, and the knob
/// travelled while the rows stayed. On the stock window the mechanism is a one-row loss:
/// `WhoListScrollFrame` is 287 tall (stock `FriendsFrame.xml` l.1661) against seventeen rows of
/// sixteen, so the child's overflow past the frame, `n × 16 − 287`, sits fifteen pixels under the
/// bar's `(n − 17) × 16`, and an engine that clamped `SetVerticalScroll` into that overflow
/// stopped the row offset at `n − 18`. The reference stores the bar's value as given (decision
/// 2017). Drives the bar to its end and reads the seventeenth row: the forty-ninth name.
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

    // The reference's own numbers, and the control: the overflow is shorter than the bar.
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
