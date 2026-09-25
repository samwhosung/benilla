//! The quest log window, engine-only: stock `QuestLogFrame.xml` fed a synthetic eight-quest log.
//! A missing template only warns under `load_xml`, so every list loads the `UIPanelTemplates` pair
//! the list and detail scroll frames inherit (`QuestLogFrame.xml:589`, `:606`).

use benilla_ui::script::{
    ExtractedQuad, PartyMemberInfo, PartyState, QuadContent, QuestItemView, QuestLogDetail,
    QuestLogEntryView, QuestLogObjectiveView, QuestLogState, SoundRequest, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// The rect of the frame sized `w` by `h`, from the `QuadContent::Frame` quad every frame emits.
fn frame_rect(quads: &[ExtractedQuad], w: f32, h: f32) -> benilla_ui::layout::Rect {
    quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Frame => q
                .rect
                .filter(|r| (r.width() - w).abs() < 0.5 && (r.height() - h).abs() < 0.5),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no bare-frame quad sized {w}x{h}"))
}

thread_local! {
    /// The detail every fixture row carries, so any selection reads it.
    static DETAIL_FIXTURE: QuestLogDetail = QuestLogDetail {
            description: "Speak with Marshal McBride.".into(),
            objectives_text: "Report to Marshal McBride.".into(),
            required_money: 0,
            reward_money: 40,
            choices: vec![],
            rewards: vec![QuestItemView {
                item_id: 2024,
                name: Some("Militia Hammer".into()),
                texture: Some("Interface\\Icons\\INV_Hammer_15".into()),
                count: 1,
                quality: 1,
                usable: true,
                // What `GetQuestLogItemLink` returns for a ctrl or shift click.
                link: Some(HAMMER_LINK.into()),
            }],
            reward_spell: None,
        };
}

/// Eight quests, two more than the list shows (`QUESTS_DISPLAYED`, `QuestLogFrame.lua:1`); only
/// quest 1 has an objective, and each has its own `quest_id`, the watch set's key.
fn eight_entries() -> QuestLogState {
    let entries = (1..=8)
        .map(|i| QuestLogEntryView {
            quest_id: i,
            title: format!("Quest {i}"),
            level: 5,
            complete: 0,
            objectives: if i == 1 {
                vec![QuestLogObjectiveView {
                    text: "Kobold Vermin slain: 3/10".into(),
                    kind: "monster".into(),
                    finished: false,
                    cur: 3,
                    req: 10,
                }]
            } else {
                vec![]
            },
            detail: Some(DETAIL_FIXTURE.with(Clone::clone)),
            ..Default::default()
        })
        .collect();
    QuestLogState {
        num_quests: 8,
        entries,
        hidden_quest_ids: Vec::new(),
    }
}

/// The fixture reward's escaped link, a white (quality 1) 1.12 item.
const HAMMER_LINK: &str = "|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r";

#[test]
fn shipped_questlog_frame_loads_clean() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
}

/// Open, read the list and the detail, wheel the list, abandon through the confirm, close.
#[test]
fn shipped_questlog_frame_drives_end_to_end() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(eight_entries());

    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());

    // `ToggleQuestLog`, the `L` binding's entry point.
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestLogOpen".into())],
        "opening the quest log plays igQuestLogOpen"
    );

    // With no selection the first quest selects itself (`QuestLogFrame.lua:294-296`).
    assert!(s
        .eval::<String>("return QuestLogTitle1NormalText:GetText()")
        .unwrap()
        .contains("Quest 1"));
    assert_eq!(
        s.eval::<String>("return QuestLogQuestCount:GetText()")
            .unwrap(),
        "Quests: |cffffffff8/20|r"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    assert_eq!(
        s.eval::<String>("return QuestLogQuestTitle:GetText()")
            .unwrap(),
        "Quest 1"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogObjective1:GetText()")
            .unwrap(),
        "Kobold Vermin slain: 3/10"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogItem1Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );
    assert_ne!(
        s.eval::<String>("return QuestLogRewardTitleText:GetText()")
            .unwrap(),
        "",
        "the Rewards header shows once there's a reward to show"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogDescriptionTitle:GetText()")
            .unwrap(),
        "Description"
    );
    // No choices: REWARD_ITEMS_ONLY (`QuestFrame.lua:469`).
    assert_eq!(
        s.eval::<String>("return QuestLogItemReceiveText:GetText()")
            .unwrap(),
        "You will receive:"
    );
    // Hidden, not blanked: the choose text is only shown or hidden (`QuestFrame.lua:369`, `:412`).
    assert!(
        !s.eval::<bool>("return QuestLogItemChooseText:IsShown()")
            .unwrap(),
        "no choices: the choose text is hidden"
    );
    // Choices and rewards share one pool: item 1 is the reward, item 2 unused.
    assert!(!s.eval::<bool>("return QuestLogItem2:IsShown()").unwrap());

    // Wheel down over row 3, a list row (the detail pane shows quest 1).
    s.resolve();
    let quads = s.extract();
    let (wx, wy) = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t.contains("Quest 3") => q
                .rect
                .map(|r| ((r.left + r.right) * 0.5, (r.bottom + r.top) * 0.5)),
            _ => None,
        })
        .expect("a 'Quest 3' row text quad");
    let before = s
        .eval::<String>("return QuestLogTitle1NormalText:GetText()")
        .unwrap();
    s.mouse_wheel(wx, wy, -1.0);
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    // A notch moves the bar half its height (`UIPanelTemplates.lua:150-157`), several rows.
    assert_ne!(
        s.eval::<String>("return QuestLogTitle1NormalText:GetText()")
            .unwrap(),
        before,
        "wheel-down scrolled the list"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    // Abandon, then No: the ABANDON_QUEST popup closes and queues nothing.
    s.run("QuestLogFrameAbandonButton:Click()").unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return GetAbandonQuestName()").unwrap(),
        "Quest 1"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Abandon \"Quest 1\"?"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());
    assert!(
        s.take_quest_log_abandons().is_empty(),
        "No queues no abandon intent"
    );
    // Only the popup's own open and close sounds (`StaticPopup.lua:1832`, `:1841`).
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igMainMenuOpen".into()),
            SoundRequest::KitName("igMainMenuClose".into()),
        ],
        "No: dialog open/close kits only, no abandon kit"
    );

    // Yes: the index marked at click time drains.
    s.run("QuestLogFrameAbandonButton:Click()").unwrap();
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_quest_log_abandons(), vec![1]);
    // `StaticPopup_OnClick` runs OnAccept before it hides (`StaticPopup.lua:1849-1867`).
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igMainMenuOpen".into()),
            SoundRequest::KitName("igQuestLogAbandonQuest".into()),
            SoundRequest::KitName("igMainMenuClose".into()),
        ]
    );
    assert!(!s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());

    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestLogClose".into())],
        "closing the quest log plays igQuestLogClose"
    );
}

/// With chat closed, a title shift-click toggles the watch (`QuestLogFrame.lua:481-500`): the
/// row's check and the tracker, `QuestWatchFrame`.
#[test]
fn shift_click_toggles_the_watch_checkbox_and_the_tracker_hud() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The check seats off measured widths during the update (`QuestLogFrame.lua:224-233`).
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    // A shift-click reads `ChatFrameEditBox:IsVisible()` unguarded (`QuestLogFrame.lua:478`).
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(eight_entries());
    // The app fires QUEST_LOG_UPDATE when the log lands; its `QuestWatch_Update` hides an empty
    // tracker (`QuestLogFrame.lua:58-60`, `:672-676`).
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(!s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
    let checkbox_shown = |s: &mut UiScript| {
        s.resolve();
        s.extract().iter().any(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("UI-CheckBox-Check"))
        })
    };
    assert!(!checkbox_shown(&mut s), "no checkbox before any watch");

    // Row 1, quest 1, the one with an objective: 300x16 at the window's TOPLEFT +19,-75
    // (`QuestLogFrame.xml:4-7`, `:535-539`).
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    let (rx, ry) = (win.left + 19.0 + 150.0, win.top - 75.0 - 8.0);
    s.set_modifiers(true, false, false);
    s.mouse_button(rx, ry, "LeftButton", true);
    s.mouse_button(rx, ry, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.errors().is_empty(),
        "shift-click errors: {:?}",
        s.errors()
    );

    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
    assert!(checkbox_shown(&mut s), "watched row shows its checkbox");
    // The check seats at the title's width + 24 from the row's left (`QuestLogFrame.lua:228`), 4
    // past the ink after the text's 20 inset (`QuestLogFrame.xml:97-99`); a loose band, no metrics.
    let reqs = s.fontstrings_needing_measure();
    let answers: Vec<(u32, f32, f32, u64)> = reqs
        .iter()
        .map(|r| (r.id, r.text.chars().count() as f32 * 6.0, 12.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    let check = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("UI-CheckBox-Check"))
        })
        .expect("check quad");
    let check_left = check.rect.expect("check rect resolved").left;
    let row_left = win.left + 19.0;
    assert!(
        check_left > row_left + 40.0 && check_left < row_left + 150.0,
        "check seats after the title ink, got left = {check_left} (row_left = {row_left})"
    );

    // The tracker's line pool: the title, then each objective.
    assert!(s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return QuestWatchLine1:GetText()")
            .unwrap(),
        "Quest 1"
    );
    assert_eq!(
        s.eval::<String>("return QuestWatchLine2:GetText()")
            .unwrap(),
        " - Kobold Vermin slain: 3/10"
    );
    // A manual watch never expires: `QUEST_WATCH_NO_EXPIRE` (`QuestLogFrame.lua:498`, `:776`).
    assert!(s
        .eval::<bool>(
            "return QUEST_WATCH_LIST[1] ~= nil and QUEST_WATCH_LIST[1].timer == QUEST_WATCH_NO_EXPIRE"
        )
        .unwrap());

    // Unwatching the last quest hides the whole tracker frame.
    s.set_modifiers(true, false, false);
    s.mouse_button(rx, ry, "LeftButton", true);
    s.mouse_button(rx, ry, "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(!checkbox_shown(&mut s), "unwatching clears the checkbox");
    assert!(!s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
}

/// A shift-click on a quest with no objectives, or past five watches, refuses the watch with a red
/// line on `UIErrorsFrame` and returns before selecting the row (`QuestLogFrame.lua:489-497`).
#[test]
fn watch_guards_no_op_without_erroring() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIErrorsFrame.xml"); // the guards' red-line surface
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    // A shift-click reads `ChatFrameEditBox:IsVisible()` unguarded (`QuestLogFrame.lua:478`).
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();

    // Rows are 300x16 from the window's TOPLEFT +19,-75, each overlapping the last by 1, a 15 px
    // pitch (`QuestLogFrame.xml:535-548`).
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    let row_center = |n: u32| win.top - 75.0 - 15.0 * (n - 1) as f32 - 8.0;
    let x = win.left + 19.0 + 150.0;

    // Quest 2 has no objectives.
    s.set_modifiers(true, false, false);
    s.mouse_button(x, row_center(2), "LeftButton", true);
    s.mouse_button(x, row_center(2), "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.errors().is_empty(),
        "no-objectives guard errors: {:?}",
        s.errors()
    );
    assert!(!s.eval::<bool>("return IsQuestWatched(2)").unwrap());
    // `UIErrorsFrame` is a MessageFrame: its lines are read off the drawn quads.
    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t == "This quest has no objectives to track"
        )),
        "the ref's QUEST_WATCH_NO_OBJECTIVES red line surfaces"
    );
    assert_eq!(
        s.eval::<i64>("return GetQuestLogSelection()").unwrap(),
        1,
        "a shift-click never selects — stock QuestLogTitleButton_OnClick's shift branch returns \
         before the plain-click arm's QuestLog_SetSelection (QuestLogFrame.lua:472-500)"
    );

    // Fill the five watches with quests 2-6: the objectives guard is the click handler's
    // (`QuestLogFrame.lua:489`), not `AddQuestWatch`'s.
    for i in 2..=6 {
        s.run(&format!("AddQuestWatch({i})")).unwrap();
    }
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);

    // Quest 1 has an objective, so this click reaches the full-list guard.
    s.set_modifiers(true, false, false);
    s.mouse_button(x, row_center(1), "LeftButton", true);
    s.mouse_button(x, row_center(1), "LeftButton", false);
    s.set_modifiers(false, false, false);
    assert!(
        s.errors().is_empty(),
        "too-many guard errors: {:?}",
        s.errors()
    );
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Text { text: Some(t), .. } if t == "You may only watch 5 quests at a time"
        )),
        "the ref's QUEST_WATCH_TOO_MANY red line surfaces"
    );
}

/// Progress arms a 300 s watch (`MAX_QUEST_WATCH_TIMER`, `QuestLogFrame.lua:752-770`), fresh
/// progress re-arms it, and expiry unwatches and hides the tracker (`:774-786`).
#[test]
fn progress_auto_watches_for_five_minutes() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    // The options window sets `AUTO_QUEST_WATCH`, as the reference's `UIOptionsFrame_Init` does.
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    s.set_quest_log(eight_entries());

    // The app fires this with the log index `AutoQuestWatch_Update` takes; the 1.12 client passes
    // the index `0x4df880` returns, a separate one that is 0 for an unwatched quest.
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(s.errors().is_empty(), "auto-watch errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
    assert!(
        s.eval::<bool>(
            "return QUEST_WATCH_LIST[1] ~= nil and QUEST_WATCH_LIST[1].index == 1 \
             and QUEST_WATCH_LIST[1].timer == MAX_QUEST_WATCH_TIMER"
        )
        .unwrap(),
        "the reference's QUEST_WATCH_LIST carries the armed timer (QuestLogFrame.lua:752-769)"
    );

    s.tick(299.0);
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    s.tick(299.0);
    assert!(
        s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "re-armed by fresh progress"
    );
    s.tick(2.0);
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "expired after 300 s without progress"
    );
    assert!(!s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "expiry errors: {:?}", s.errors());
}

/// `AUTO_QUEST_WATCH` gates the auto-watch as a "1"/"0" string (`QuestLogFrame.lua:68`).
/// Deviation: it defaults to the string "1", because 1.12 assigns the number 1
/// (`UIOptionsFrame.lua:122`), which that `== "1"` check reads as off.
#[test]
fn the_auto_watch_flag_is_the_references_uvar_and_gates_the_watch() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    // The options window sets `AUTO_QUEST_WATCH`, as the reference's `UIOptionsFrame_Init` does.
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "KeyBindingsPage.xml");
    load_xml(&s, "OptionsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    s.set_quest_log(eight_entries());
    assert_eq!(s.eval::<String>("return AUTO_QUEST_WATCH").unwrap(), "1");

    s.run(r#"AUTO_QUEST_WATCH = "0""#).unwrap();
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "the flag off means progress watches nothing"
    );

    s.run(r#"AUTO_QUEST_WATCH = "1""#).unwrap();
    s.fire_event(
        "QUEST_WATCH_UPDATE",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s.errors().is_empty(), "auto-watch errors: {:?}", s.errors());
}

/// A row's tag paints in parentheses on its right-flush `$parentTag`, and a Failed or Complete
/// state replaces it (`QuestLogFrame.lua:188-195`); headers carry none.
#[test]
fn the_row_tag_is_its_own_right_flush_string_and_the_state_word_wins() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // The state words are the GlobalStrings `COMPLETE` and `FAILED`.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    let mut state = eight_entries();
    // Row 1 tagged, row 2 tagged and complete, row 3 failed, row 4 a header.
    state.entries[0].tag = Some("Elite".into());
    state.entries[1].tag = Some("Raid".into());
    state.entries[1].complete = 1;
    state.entries[2].complete = -1;
    state.entries[3].is_header = true;
    state.entries[3].title = "Elwynn Forest".into();
    state.entries[3].tag = None;
    s.set_quest_log(state);
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // A blank FontString reads back nil (`FontString:GetText`, `0x79d690`).
    let tag = |s: &mut UiScript, i: u32| {
        s.eval::<Option<String>>(&format!("return QuestLogTitle{i}Tag:GetText()"))
            .unwrap()
    };
    assert_eq!(
        tag(&mut s, 1).as_deref(),
        Some("(Elite)"),
        "the engine's tag, in parentheses"
    );
    assert_eq!(
        tag(&mut s, 2).as_deref(),
        Some("(Complete)"),
        "COMPLETE overwrites the quest's own tag — ref l.190-192"
    );
    assert_eq!(
        tag(&mut s, 3).as_deref(),
        Some("(Failed)"),
        "FAILED, same override"
    );
    assert_eq!(tag(&mut s, 4), None, "a header carries no tag");

    // The title is the indented bare name; the state word is only on the tag.
    let title1 = s
        .eval::<String>("return QuestLogTitle1NormalText:GetText()")
        .unwrap();
    assert_eq!(title1, "  Quest 1");
    let title2 = s
        .eval::<String>("return QuestLogTitle2NormalText:GetText()")
        .unwrap();
    assert_eq!(title2, "  Quest 2", "no \"(Complete)\" on the title");

    // Anchored RIGHT, -2 (`QuestLogFrame.xml:10-21`).
    let row_right = s.eval::<f32>("return QuestLogTitle1:GetRight()").unwrap();
    let tag_right = s
        .eval::<f32>("return QuestLogTitle1Tag:GetRight()")
        .unwrap();
    assert!(
        (row_right - tag_right - 2.0).abs() < 0.5,
        "tag right-flush at row right -2 (row {row_right}, tag {tag_right})"
    );
}

#[test]
fn empty_quest_log_hides_rows_and_disables_abandon() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(QuestLogState::default());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // QUESTLOG_NO_QUESTS_TEXT (`GlobalStrings.lua:3225`); nothing hides the label, whose only
    // mention is `QuestLogFrame.xml:427`.
    assert_eq!(
        s.eval::<String>("return QuestLogNoQuestsText:GetText()")
            .unwrap(),
        "No Active Quests"
    );
    assert!(s
        .eval::<bool>("return QuestLogNoQuestsText:IsVisible()")
        .unwrap());
    assert!(!s.eval::<bool>("return QuestLogTitle1:IsVisible()").unwrap());
    assert!(!s
        .eval::<bool>("return QuestLogFrameAbandonButton:IsEnabled() ~= 0")
        .unwrap());
    // An empty log hides the whole detail pane (`QuestLog_Update`, `QuestLogFrame.lua:109-113`).
    assert!(
        !s.eval::<bool>("return QuestLogDetailScrollFrame:IsVisible()")
            .unwrap(),
        "no selection: the detail pane is hidden, header and all"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogQuestCount:GetText()")
            .unwrap(),
        "Quests: |cffffffff0/20|r"
    );
}

/// `QuestFrameItems_Update` lays choices out two per row, anchors "You will also receive:" under
/// the last choice row's left item, then the reward (`QuestFrame.lua:311-522`).
#[test]
fn reward_rows_follow_the_refs_two_per_row_layout() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    let mut state = eight_entries();
    // Every row carries the detail, and the window paints the auto-selected one.
    let detail = QuestLogDetail {
        description: "Speak with Marshal McBride.".into(),
        objectives_text: "Report to Marshal McBride.".into(),
        required_money: 0,
        reward_money: 150, // 1s 50c
        choices: vec![
            QuestItemView {
                item_id: 0,
                name: Some("Worn Sword".into()),
                texture: None,
                count: 1,
                quality: 1,
                usable: true,
                ..Default::default()
            },
            QuestItemView {
                item_id: 0,
                name: Some("Worn Mace".into()),
                texture: None,
                count: 1,
                quality: 1,
                usable: true,
                ..Default::default()
            },
        ],
        rewards: vec![QuestItemView {
            item_id: 0,
            name: Some("Militia Hammer".into()),
            texture: None,
            count: 1,
            quality: 1,
            usable: true,
            ..Default::default()
        }],
        reward_spell: None,
    };
    for e in &mut state.entries {
        e.detail = Some(detail.clone());
    }
    s.set_quest_log(state);
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // One pool: items 1-2 are the choices, 3 the reward, 4 the first unused slot.
    assert!(s.eval::<bool>("return QuestLogItem1:IsShown()").unwrap());
    assert!(s.eval::<bool>("return QuestLogItem2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestLogItem4:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestLogItem1Name:GetText()")
            .unwrap(),
        "Worn Sword"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogItem2Name:GetText()")
            .unwrap(),
        "Worn Mace"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogItemChooseText:GetText()")
            .unwrap(),
        "You will be able to choose one of these rewards:"
    );

    // With choices, REWARD_ITEMS anchors under the last choice row's odd (left) item, +3,-5
    // (`QuestFrame.lua:461-467`).
    assert_eq!(
        s.eval::<String>("return QuestLogItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:"
    );
    assert_eq!(
        s.eval::<String>(
            "local p, rel, rp = QuestLogItemReceiveText:GetPoint() \
             return p .. '|' .. rel:GetName() .. '|' .. rp"
        )
        .unwrap(),
        "TOPLEFT|QuestLogItem1|BOTTOMLEFT"
    );
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local _, _, _, x, y = QuestLogItemReceiveText:GetPoint() return x, y"
        )
        .unwrap(),
        (3.0, -5.0)
    );

    assert!(s.eval::<bool>("return QuestLogItem3:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestLogItem4:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestLogItem3Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );

    assert_eq!(
        s.eval::<String>("return QuestLogRewardTitleText:GetText()")
            .unwrap(),
        "Rewards"
    );
}

/// Answer every pending measure with `FixedWidthFont(6.0)`'s metrics: 6 px a glyph, 12 px a line.
fn answer_measures(s: &mut UiScript) {
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .into_iter()
        .map(|r| {
            let ink = r.text.chars().count() as f32 * 6.0;
            match r.wrap_width {
                Some(w) if w > 0.0 && ink > w => (r.id, w, (ink / w).ceil() * 12.0, r.key),
                _ => (r.id, ink, 12.0, r.key),
            }
        })
        .collect();
    s.set_measured_text_unwrapped(&answers);
}

fn overflowing_entry() -> QuestLogState {
    let objectives = (1..=10)
        .map(|i| QuestLogObjectiveView {
            text: format!("Objective {i} of 10 slain: 0/5"),
            kind: "monster".into(),
            finished: false,
            cur: 0,
            req: 5,
        })
        .collect();
    QuestLogState {
        num_quests: 1,
        hidden_quest_ids: Vec::new(),
        entries: vec![QuestLogEntryView {
            quest_id: 1,
            title: "A Very Long Quest".into(),
            level: 5,
            complete: 0,
            objectives,
            detail: Some(QuestLogDetail {
                description: "A very long description. ".repeat(20),
                objectives_text: "Report back once every objective below is complete.".into(),
                required_money: 0,
                reward_money: 0,
                choices: vec![],
                rewards: vec![QuestItemView {
                    item_id: 0,
                    name: Some("Militia Hammer".into()),
                    texture: None,
                    count: 1,
                    quality: 1,
                    usable: true,
                    ..Default::default()
                }],
                reward_spell: None,
            }),
            ..Default::default()
        }],
    }
}

/// Quads under the scroll child carry `QuestLogDetailScrollFrame`'s rect as their clip; the
/// window's own art outside it carries none.
#[test]
fn overflowing_detail_content_clips_to_the_scrollframe_rect() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(overflowing_entry());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.resolve();
    let quads = s.extract();
    let scroll_rect = frame_rect(&quads, 300.0, 261.0);

    let reward_plate_clips: Vec<Option<benilla_ui::layout::Rect>> = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-QuestItemNameFrame") => {
                Some(q.clip)
            }
            _ => None,
        })
        .collect();
    assert!(
        !reward_plate_clips.is_empty(),
        "expected at least one reward-row name-plate quad in the fixture"
    );
    for clip in reward_plate_clips {
        assert_eq!(
            clip,
            Some(scroll_rect),
            "a reward row (inside the scroll child's subtree) clips to the ScrollFrame's own rect"
        );
    }

    let book_icon_clip = quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } if p.contains("UI-QuestLog-BookIcon") => {
            Some(q.clip)
        }
        _ => None,
    });
    assert_eq!(
        book_icon_clip,
        Some(None),
        "the window's own chrome sits outside the scroll child and is unclipped"
    );
}

/// The detail pane's scroll frame takes the wheel (`UIPanelTemplates.xml:208-210`).
#[test]
fn wheel_over_the_detail_pane_changes_vertical_scroll() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(overflowing_entry());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The scroll range spans the measured content, so answer the measures as the app does.
    s.resolve();
    answer_measures(&mut s);
    s.resolve(); // the measures reach the scroll range and the bar

    s.resolve();
    let quads = s.extract();
    let scroll_rect = frame_rect(&quads, 300.0, 261.0);
    // Near the pane's top-left, clear of the reward row.
    let (x, y) = (scroll_rect.left + 10.0, scroll_rect.top - 10.0);

    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        0.0
    );
    s.mouse_wheel(x, y, -1.0); // -1 is down
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    let after = s
        .eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
        .unwrap();
    assert!(
        after > 0.0,
        "wheel-down over the detail pane increased its scroll offset, got {after}"
    );
}

/// `QuestLog_UpdateQuestDetails` resets the detail scroll unless given `doNotScroll`
/// (`QuestLogFrame.lua:348`, `:455-457`), which the `QUEST_LOG_UPDATE` refresh passes (`:62`).
#[test]
fn selection_change_resets_detail_scroll_but_a_quest_log_update_refresh_does_not() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(overflowing_entry());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The scroll range spans the measured content, so answer the measures as the app does.
    s.resolve();
    answer_measures(&mut s);
    s.resolve(); // the measures reach the scroll range and the bar
    s.resolve(); // the range reads resolved rects

    // Scroll through the bar, as the wheel does: a reselect snaps back with the bar's
    // `SetValue(0)`, which changes nothing on a bar already at 0.
    s.run("QuestLogDetailScrollFrameScrollBar:SetValue(10)")
        .unwrap();
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        10.0
    );

    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "refresh errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        10.0,
        "QUEST_LOG_UPDATE's data-refresh path must not yank the scroll position"
    );

    s.run("QuestLogTitle1:Click()").unwrap();
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScrollFrame:GetVerticalScroll()")
            .unwrap(),
        0.0,
        "a manual reselect snaps the detail pane's scroll back to the top"
    );
}

/// The reward row's OnEnter calls `SetQuestLogItem` (`QuestLogFrame.xml:113`): a cold store shows
/// the name and asks once, and after the answer a re-hover shows the full item lines.
#[test]
fn reward_row_hover_serves_the_shared_item_tooltip() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ItemTemplateView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve();

    let icon_rect = s
        .extract()
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("INV_Hammer_15"))
        })
        .and_then(|q| q.rect)
        .expect("the reward row icon");
    // Extracted rects and mouse_move share one y-up space.
    let (cx, cy) = (
        (icon_rect.left + icon_rect.right) * 0.5,
        (icon_rect.bottom + icon_rect.top) * 0.5,
    );
    let has_text = |s: &mut UiScript, t: &str| {
        s.resolve();
        s.extract()
            .iter()
            .any(|q| matches!(&q.content, QuadContent::Text { text: Some(x), .. } if x == t))
    };

    s.mouse_move(cx, cy);
    assert!(s.errors().is_empty(), "hover errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "tooltip shows on hover"
    );
    assert_eq!(
        s.take_item_stat_asks(),
        vec![2024],
        "the miss recorded the ask"
    );

    s.set_item_template(
        2024,
        ItemTemplateView {
            name: "Militia Hammer".into(),
            quality: 1,
            inventory_type: 21, // Main Hand
            class: 2,
            subclass: 4, // mace
            damages: vec![(4.0, 9.0, 0)],
            delay_ms: 2200,
            ..Default::default()
        },
    );
    s.mouse_move(0.0, 0.0);
    s.mouse_move(cx, cy);
    assert!(s.errors().is_empty(), "re-hover errors: {:?}", s.errors());
    assert!(
        has_text(&mut s, "Main Hand"),
        "slot line from the shared store"
    );
    assert!(
        has_text(&mut s, "4 - 9 Damage"),
        "damage line from the shared store"
    );
}

/// A child takes its parent's strata and level + 1 at creation, so the DIALOG popup's buttons draw
/// above its own backdrop.
#[test]
fn popup_children_inherit_the_dialog_stratum() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    s.run(
        "StaticPopupDialogs[\"TEST_STRATUM\"] = { text = \"Abandon?\", button1 = \"Yes\", \
         button2 = \"No\", timeout = 0 }\n\
         StaticPopup_Show(\"TEST_STRATUM\")",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    s.resolve();
    let quads = s.extract();
    let backdrop_z = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Backdrop { path, .. } if path.contains("UI-DialogBox-Background") => {
                Some(q.z)
            }
            _ => None,
        })
        .expect("the popup backdrop");
    let button_z = quads
        .iter()
        .find_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } if p.contains("UI-DialogBox-Button-Up") => {
                Some(q.z)
            }
            _ => None,
        })
        .expect("a popup button face");
    assert!(
        button_z > backdrop_z,
        "the buttons draw ABOVE their dialog's backdrop (button z {button_z:x} vs backdrop z {backdrop_z:x})"
    );
}

/// `QuestLogRewardItem_OnClick` (`QuestLogFrame.lua:542-552`): ctrl tries the reward on, shift
/// posts its link with chat open, and a plain click does nothing.
#[test]
fn reward_rows_preview_and_post_and_a_plain_click_stays_inert() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    // A shift-click reads `ChatFrameEditBox:IsVisible()` unguarded (`QuestLogFrame.lua:548`).
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\DressUpFrame.xml"); // DressUpItemLink lives here
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit its menus build from
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml"); // ChatFrameEditBox lives here
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestLogItem1:IsShown()").unwrap(),
        "the fixture's one fixed reward row is laid out"
    );
    let _ = s.take_dressup_intents();

    s.run("QuestLogItem1:Click()").unwrap();
    assert!(
        s.take_dressup_intents().is_empty(),
        "a plain click never opens the dressing room"
    );
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "",
        "a plain click posts nothing"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Shift with chat open inserts the link (`QuestLogFrame.lua:547-550`).
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.run("QuestLogItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        HAMMER_LINK,
        "shift-click posted the reward's link"
    );
    assert!(
        s.take_dressup_intents().is_empty(),
        "the shift arm never opens the room"
    );

    // Ctrl: `DressUpItemLink` (`QuestLogFrame.lua:543-546`) dresses the room, then tries it on.
    s.set_modifiers(false, true, false);
    s.run("QuestLogItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![
            benilla_ui::script::DressUpIntent::Dress,
            benilla_ui::script::DressUpIntent::TryOn(2024)
        ],
        "ctrl-click opened the room wearing the reward"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With chat open a title shift-click inserts the quest name, indent trimmed and with no link in
/// 1.12 (`QuestLogFrame.lua:478-480`); with chat closed it toggles the watch (`:481-500`).
#[test]
fn shift_click_on_a_title_posts_the_quest_name_with_chat_open_and_watches_with_it_closed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    // A shift-click reads `ChatFrameEditBox:IsVisible()` unguarded (`QuestLogFrame.lua:478`).
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit its menus build from
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml"); // ChatFrameEditBox lives here
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Row 1's centre: 300x16 at the window's TOPLEFT +19,-75.
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    let (rx, ry) = (win.left + 19.0 + 150.0, win.top - 75.0 - 8.0);
    let shift_click = |s: &mut UiScript| {
        s.set_modifiers(true, false, false);
        s.mouse_button(rx, ry, "LeftButton", true);
        s.mouse_button(rx, ry, "LeftButton", false);
        s.set_modifiers(false, false, false);
    };

    shift_click(&mut s);
    assert!(
        s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "with chat closed, shift-click still toggles the quest watch"
    );
    shift_click(&mut s); // and off again
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());

    assert!(s.focus_editbox("ChatFrameEditBox"));
    shift_click(&mut s);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "Quest 1",
        "the trimmed quest name landed in the chat box (ref's gsub result, no row indent)"
    );
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "with chat open the watch must NOT toggle — the ref makes watching the else"
    );
    // Either way the click ends by selecting the row (`QuestLogFrame.lua:503-504`).
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// ── The Share Quest button ──

/// A party of `n` others; `GetNumPartyMembers` is that list's length.
fn party(n: usize) -> PartyState {
    PartyState {
        members: (0..n)
            .map(|i| PartyMemberInfo {
                name: format!("Mate{i}"),
                guid: 0x300 + i as u64,
            })
            .collect(),
        ..PartyState::default()
    }
}

/// The eight quests with `pushable` set on `ids`; the app reads it from `QUEST_FLAGS_SHARABLE`.
fn entries_sharable(ids: &[u32]) -> QuestLogState {
    let mut state = eight_entries();
    for e in &mut state.entries {
        e.pushable = ids.contains(&e.quest_id);
    }
    state
}

/// Share needs both a sharable selection and a party (`QuestLogFrame.lua:299-305`); each half is
/// moved alone.
#[test]
fn share_quest_needs_both_a_sharable_selection_and_a_party() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(entries_sharable(&[1]));
    s.set_party(PartyState::default());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);
    // `GetQuestLogPushable` answers 1 or nil.
    assert!(
        s.eval::<bool>("return GetQuestLogPushable() ~= nil")
            .unwrap(),
        "quest 1 carries the sharable bit"
    );
    assert!(
        !s.eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
            .unwrap(),
        "solo: sharable is not enough"
    );

    // PARTY_MEMBERS_CHANGED alone re-tests it (`QuestLogFrame.lua:71-80`).
    s.set_party(party(2));
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(
        s.eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
            .unwrap(),
        "PARTY_MEMBERS_CHANGED alone re-tests the button"
    );

    s.run("SelectQuestLogEntry(2)").unwrap();
    s.run("QuestLog_Update()").unwrap();
    assert!(
        s.eval::<bool>("return GetQuestLogPushable() == nil")
            .unwrap(),
        "quest 2 has no sharable bit"
    );
    assert!(
        !s.eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
            .unwrap(),
        "in a party: an unsharable selection is still dark"
    );

    s.run("SelectQuestLogEntry(1)").unwrap();
    s.run("QuestLog_Update()").unwrap();
    assert!(s
        .eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
        .unwrap());
    s.set_party(PartyState::default());
    s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
    assert!(!s
        .eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
        .unwrap());
}

/// An empty log disables Share before either other test (`QuestLogFrame.lua:299-300`).
#[test]
fn share_quest_is_dark_on_an_empty_log_even_in_a_party() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(QuestLogState::default());
    s.set_party(party(4));
    s.run("ToggleQuestLog()").unwrap();
    assert!(!s
        .eval::<bool>("return QuestFramePushQuestButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The click queues the selected quest's id, not the row under the mouse.
#[test]
fn share_quest_click_queues_the_selected_quests_id() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest_log(entries_sharable(&[1, 5]));
    s.set_party(party(1));
    s.run("ToggleQuestLog()").unwrap();

    s.run("SelectQuestLogEntry(5)").unwrap();
    s.run("QuestLog_Update()").unwrap();
    s.run("QuestFramePushQuestButton:Click()").unwrap();
    assert_eq!(s.take_quest_log_pushes(), vec![5]);
    assert!(s.take_quest_log_pushes().is_empty(), "drained");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A tagged row's title is capped at `275 - 15 - tagWidth` (`QuestLogFrame.lua:197-205`). Its
/// declared 10-unit height arms the ellipsis, which `0x771ec0` gates on a box with both width and
/// height, so the capped title paints as one truncated line.
#[test]
fn the_stock_title_width_cap_bites_and_is_sticky_per_row_button() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The cap reads `questTitleTag:GetWidth()` inside the update, so the harness needs a measurer.
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    // A long Dungeon-tagged title.
    const LONG: &str = "The Left Piece of Lord Valthalak's Amulet";
    let mut state = eight_entries();
    state.entries[0].title = LONG.into();
    state.entries[0].tag = Some("Dungeon".into());
    state.entries[1].title = LONG.into();
    state.entries[1].tag = None;
    s.set_quest_log(state.clone());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let width =
        |s: &mut UiScript, q: &str| s.eval::<f32>(&format!("return {q}:GetWidth()")).unwrap();
    let ink = |s: &mut UiScript, q: &str| {
        s.eval::<f32>(&format!("return {q}:GetStringWidth()"))
            .unwrap()
    };

    let tag_w = width(&mut s, "QuestLogTitle1Tag");
    let capped = width(&mut s, "QuestLogTitle1NormalText");
    assert!(
        (capped - (275.0 - 15.0 - tag_w)).abs() < 0.5,
        "the row's title is capped at 275-15-tagWidth ({tag_w} wide): got {capped}"
    );
    let natural = ink(&mut s, "QuestLogTitle1NormalText");
    assert!(natural > capped, "the fixture title ({natural} px) must overflow the {capped} px cap or this test proves nothing");
    assert!(
        (s.eval::<f32>("return QuestLogTitle1NormalText:GetHeight()")
            .unwrap()
            - 10.0)
            .abs()
            < 0.5,
        "the ButtonText's declared height (QuestLogFrame.xml:94-96)"
    );

    // An untagged title keeps its width; the reset fires only above 275 (`QuestLogFrame.lua:220`).
    assert!(
        (width(&mut s, "QuestLogTitle2NormalText") - ink(&mut s, "QuestLogTitle2NormalText")).abs()
            < 0.5,
        "an untagged title keeps its natural width"
    );

    // The cap sticks to the row button, as in the reference: `GetWidth()` then answers the set
    // width, so the `> 275` reset never fires when an untagged quest takes the row.
    state.entries[0].tag = None;
    s.set_quest_log(state);
    s.run("QuestLog_Update()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        (width(&mut s, "QuestLogTitle1NormalText") - capped).abs() < 0.5,
        "the capped box survives the row going untagged (the ref's own 275 reset never fires)"
    );
}
