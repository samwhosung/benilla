//! The shipped **questgiver window** open/close sound, driven engine-only (no Bevy): the real
//! `Interface\FrameXML\QuestFrame.xml` loaded behind `UIParent.xml` and shown/hidden through the
//! wire events. The window-sound convention's machine check for the quest arc, the
//! sibling of the merchant/gossip/bag/loot sound tests.

use benilla_ui::script::{QuestPanel, QuestState, ScriptValue, SoundRequest, UiScript};

use super::test_ui::load_ui as load_xml;

/// The questgiver window's open/close kits — the window-sound convention. The real
/// QuestFrame.lua plays igQuestListOpen in QuestFrame_OnShow (l.285) and igQuestListClose in
/// QuestFrame_OnHide (l.294), wired via the frame OnShow/OnHide (QuestFrame.xml l.1012/1015). A
/// questgiver event (QUEST_DETAIL here) → ShowUIPanel → Show() fires OnShow; QUEST_FINISHED →
/// HideUIPanel → Hide() fires OnHide. Nothing queues at load (the frame is authored hidden="true").
/// Same pair as gossip — questgiver and gossip are the same "list" surface to the client.
#[test]
fn questgiver_show_hide_plays_open_and_close_kits() {
    benilla_formats::wow_data_or_skip!();
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
    // The BenillaMoney_* purse helpers the quest reward/progress panels repaint through live in
    // MerchantFrame.xml (the same documented cross-window dep the bag tests load).
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    // Hidden at load: no open sound (never transitions on startup).
    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );

    // A quest-details panel arrives (SMSG_QUESTGIVER_QUEST_DETAILS → QUEST_DETAIL) and shows the
    // window through ShowUIPanel.
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Speak with the captain.".into(),
        objectives: "Report to the captain.".into(),
        ..QuestState::default()
    }));
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "the questgiver window opened onto the left slot"
    );
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListOpen".into())],
        "opening the questgiver window plays igQuestListOpen"
    );

    // QUEST_FINISHED hides it through HideUIPanel → OnHide.
    s.fire_event("QUEST_FINISHED", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "QUEST_FINISHED hid the questgiver window"
    );
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestListClose".into())],
        "closing the questgiver window plays igQuestListClose"
    );
}

/// The four-child-panel restructure (this pass): each questgiver event shows exactly ONE of the four
/// real sub-panels and hides the other three — mirroring the ref's own `QuestFrame*Panel_OnShow`
/// (each hides its three siblings on show). No sound on a panel SWITCH (only the outer window's
/// OnShow/OnHide play kits, per the ref `QuestFrame_OnShow`/`OnHide` — a panel change while the window
/// stays open is silent).
#[test]
fn panel_events_show_exactly_one_child_panel_and_hide_the_others() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    let panels = [
        "QuestFrameGreetingPanel",
        "QuestFrameDetailPanel",
        "QuestFrameProgressPanel",
        "QuestFrameRewardPanel",
    ];
    let is_shown =
        |s: &UiScript, name: &str| s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap();

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Greeting,
        greeting: "What can I do for you?".into(),
        active_titles: vec!["Report to Goldshire".into()],
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestFrameGreetingPanel",
            "only the greeting panel is shown after QUEST_GREETING"
        );
    }
    assert_eq!(
        s.eval::<String>("return QuestFrameNpcNameText:GetText()")
            .unwrap(),
        "Deputy Willem"
    );
    // No sound on the panel event itself — the window was already open (well, first open here DOES
    // play the open kit; drain it so the assertion below is about the SWITCH, not this first show).
    s.take_sounds();

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestFrameDetailPanel",
            "only the detail panel is shown after QUEST_DETAIL"
        );
    }
    assert!(
        s.take_sounds().is_empty(),
        "a panel switch with the window already open plays no kit (only OnShow/OnHide of the window do)"
    );
}

/// The reward grid, ported from the ref's shared `QuestFrameItems_Update` — on ONE numbered pool
/// per panel (`QuestDetailItem1..10`), the reference's own shape: choices first, then the
/// reward-spell row, then fixed rewards, each positioned at runtime. The fixture is the one the
/// task's capture drives — 2 choices + 1 fixed reward + money — so the pool reads
/// `Item1`/`Item2` = the choices, `Item3` = the fixed reward, `Item4`+ unused. It used to run on
/// two 6-deep pools named `QuestDetailChoice*`/`QuestDetailReward*`, names the reference does not
/// have and a split the ref's own function cannot be written against.
#[test]
fn detail_panel_reward_grid_follows_the_refs_two_per_row_layout() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    let choice = |name: &str, quality: u32| benilla_ui::script::QuestItemView {
        item_id: 0,
        name: Some(name.into()),
        texture: None,
        count: 1,
        quality,
        usable: true,
        ..Default::default()
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill the kobolds infesting the mine.".into(),
        objectives: "Slay 10 Kobold Vermin.".into(),
        choices: vec![choice("Worn Sword", 1), choice("Worn Mace", 1)],
        rewards: vec![choice("Militia Hammer", 1)],
        reward_money: 150,
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Marshal McBride".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return QuestDetailItem1:IsShown()").unwrap());
    assert!(s.eval::<bool>("return QuestDetailItem2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestDetailItem4:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestDetailItem1Name:GetText()")
            .unwrap(),
        "Worn Sword"
    );
    assert_eq!(
        s.eval::<String>("return QuestDetailItemChooseText:GetText()")
            .unwrap(),
        "You will be able to choose one of these rewards:"
    );
    assert_eq!(
        s.eval::<String>("return QuestDetailItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:",
        "choices present -> the ref's \"also\" wording (QuestFrame.lua:461-462)"
    );
    assert!(s.eval::<bool>("return QuestDetailItem3:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestDetailItem4:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestDetailItem3Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );
    assert_eq!(
        s.eval::<String>("return QuestDetailRewardTitleText:GetText()")
            .unwrap(),
        "Rewards"
    );

    // The detail panel's choice rows are informational only (ref: only the REWARD panel's rows
    // select). They DO carry an OnClick now (`QuestItem_OnClick`, the ref's own
    // QuestItemTemplate script — the ctrl/shift fork), but it has no select
    // arm at all: an unmodified click must not raise and must not set an itemChoice.
    s.run("QuestDetailItem1:Click()").ok();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // The reward panel's `itemChoice` is set only by its own OnShow (stock QuestFrame.lua:104,
    // `= 0`) and a reward-row click; on the detail panel it is simply unset.
    assert_eq!(
        s.eval::<i64>("return QuestFrameRewardPanel.itemChoice or 0")
            .unwrap(),
        0,
        "a detail-panel row never selects a reward"
    );
}

/// The reward panel's choice rows ARE selectable (ref `QuestRewardItem_OnClick`) — clicking one moves
/// the highlight and arms `GetQuestReward`'s 1-based→0-based conversion.
#[test]
fn reward_panel_choice_click_selects_and_completes_with_zero_based_index() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    let choice = |name: &str| benilla_ui::script::QuestItemView {
        item_id: 0,
        name: Some(name.into()),
        texture: None,
        count: 1,
        quality: 1,
        usable: true,
        ..Default::default()
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        title: "A Threat Within".into(),
        body: "Well done.".into(),
        choices: vec![choice("Worn Sword"), choice("Worn Mace")],
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Marshal McBride".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_COMPLETE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        !s.eval::<bool>("return QuestRewardItemHighlight:IsShown()")
            .unwrap(),
        "no row picked yet"
    );
    // Completing without a choice picked is a no-op (ref: QuestChooseRewardError guard).
    s.run("QuestFrameCompleteQuestButton:Click()").unwrap();
    assert!(
        s.take_quest_actions().is_empty(),
        "must pick a choice first"
    );

    s.run("QuestRewardItem2:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestRewardItemHighlight:IsShown()")
            .unwrap(),
        "picking a choice row shows the highlight"
    );

    s.run("QuestFrameCompleteQuestButton:Click()").unwrap();
    assert_eq!(
        s.take_quest_actions(),
        vec![benilla_ui::script::QuestAction::Reward(1)],
        "row 2 (1-based) -> wire index 1 (0-based)"
    );
}

/// The greeting panel's own Goodbye button (ref `QuestFrameGreetingGoodbyeButton`, a widget the flat
/// v1 layout never carried) — a plain client-side close, no `DeclineQuest()` call (ref l.748-752).
#[test]
fn greeting_goodbye_button_closes_the_window() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Greeting,
        greeting: "Hello".into(),
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestFrame:IsVisible()").unwrap());

    s.run("QuestFrameGreetingGoodbyeButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        !s.eval::<bool>("return QuestFrame:IsVisible()").unwrap(),
        "Goodbye closes the questgiver window"
    );
}

/// A rect-level regression guard for the exact bug the capture-loop caught: `BenillaQuestPanelButtonTemplate`
/// is a virtual template declared `hidden="true"` (never itself drawn) — an instance that doesn't
/// explicitly `:Show()` stays invisible even though it isn't declared `hidden="true"` itself. Every
/// panel's action buttons must show, extracted at real on-window rects (never dropped/hidden).
///
/// It runs the **instant-text** arm, which is not the shipped one: `QUEST_FADING_DISABLE` boots at
/// the reference's `"0"` since 1804 (it was pinned `"1"` by direction from 2026-07-17 until then),
/// so the flag is planted below. That arm is the one this guard wants — it is the shorter path to
/// a settled panel, and its one-frame Accept-disabled window is a state the rects have to survive.
/// The write-on arm is `write_on_still_fades_when_instant_text_is_off`'s subject.
#[test]
fn detail_panel_action_buttons_resolve_to_real_onscreen_rects() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    // Planted, not shipped: instant text is OFF out of the box (see the doc comment).
    s.eval::<()>(r#"QUEST_FADING_DISABLE = "1""#).unwrap();
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return QuestFrameDeclineButton:IsShown()")
        .unwrap());

    s.resolve();
    let button_art = |s: &mut UiScript| {
        let mut v = Vec::new();
        for q in s.extract() {
            if let benilla_ui::script::QuadContent::Texture { path: Some(p), .. } = &q.content {
                if p.starts_with("Interface\\Buttons\\UI-Panel-Button-") {
                    v.push((
                        p.clone(),
                        q.rect.expect("button quad carries a resolved rect"),
                    ));
                }
            }
        }
        v
    };

    // The instant-text arm (QUEST_FADING_DISABLE planted "1" above): OnShow still disables Accept
    // and zeroes the block for the ref's one-frame window — the reveal edge just starts at 1024.
    let writing = button_art(&mut s);
    assert_eq!(
        writing.len(),
        2,
        "Accept + Decline both extract a real rect (not dropped for being unshown)"
    );
    assert!(
        writing
            .iter()
            .any(|(p, _)| p.ends_with("UI-Panel-Button-Disabled")),
        "Accept draws the Disabled art inside the one-frame arm window"
    );
    for (_, r) in &writing {
        assert!(
            r.top <= 512.0 && r.bottom >= 0.0 && r.left >= 0.0 && r.right <= 384.0,
            "button {r:?} must land inside the 384x512 window, not off-screen/at the origin"
        );
    }

    // First tick: instant text keeps the writing sound — exactly one quill scratch — then the
    // gradient runs off, the objectives/rewards block SNAPS to opaque (no fade) and Accept wakes.
    let scratches = |sounds: Vec<SoundRequest>| {
        sounds
            .iter()
            .filter(|r| **r == SoundRequest::KitName("WriteQuest".into()))
            .count()
    };
    s.tick(0.05);
    assert!(s.errors().is_empty(), "tick errors: {:?}", s.errors());
    assert_eq!(
        scratches(s.take_sounds()),
        1,
        "instant text keeps the writing sound: one scratch on the wake tick"
    );
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() ~= 0")
        .unwrap());
    assert_eq!(
        s.eval::<f32>("return TextAlphaDependentFrame:GetAlpha()")
            .unwrap(),
        1.0,
        "the block snaps straight to opaque — no QUESTINFO_FADE_IN ramp in instant mode"
    );
    s.tick(0.5);
    assert_eq!(
        scratches(s.take_sounds()),
        0,
        "the quill stops after the single instant-mode scratch"
    );
    s.resolve();
    let written = button_art(&mut s);
    assert!(
        written
            .iter()
            .all(|(p, _)| p.ends_with("UI-Panel-Button-Up")),
        "both buttons draw live art once the text has written on: {written:?}"
    );
}

/// The ref write-on runs verbatim: with QUEST_FADING_DISABLE = "0" — the ref's own default, and
/// ours too since 1804 (it was pinned "1" from 2026-07-17 until then, which is why this file
/// still says the value out loud rather than leaning on the shipped one) — the description writes
/// on at 40 chars/s scratching the quill each tick, Accept stays dead mid-write, and the
/// objectives/rewards block FADES in over QUESTINFO_FADE_IN rather than snapping.
#[test]
fn write_on_still_fades_when_instant_text_is_off() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    s.eval::<()>(r#"QUEST_FADING_DISABLE = "0""#).unwrap();
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Mid-write (reveal edge at char 4 of 13): the quill scratches, Accept stays dead.
    s.tick(0.1);
    assert!(
        s.take_sounds()
            .iter()
            .any(|r| *r == SoundRequest::KitName("WriteQuest".into())),
        "the write-on scratches the WriteQuest quill each tick"
    );
    assert!(!s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() ~= 0")
        .unwrap());

    // Half a second more runs the 13-char gradient off (40 chars/s): Accept wakes and the block
    // is mid-FADE — armed at 0 and ramping over QUESTINFO_FADE_IN, not snapped to opaque.
    s.tick(0.5);
    assert!(s.errors().is_empty(), "tick errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestFrameAcceptButton:IsEnabled() ~= 0")
        .unwrap());
    let alpha = s
        .eval::<f32>("return TextAlphaDependentFrame:GetAlpha()")
        .unwrap();
    assert!(
        alpha < 1.0,
        "fading mode ramps the block in (got alpha {alpha})"
    );
    s.tick(1.1);
    assert_eq!(
        s.eval::<f32>("return TextAlphaDependentFrame:GetAlpha()")
            .unwrap(),
        1.0,
        "the fade completes at opaque"
    );
}

/// The giver window's title bar shows the NPC name — the panel-open events carry it as arg1 and
/// the in-place QUEST_ITEM_UPDATE refresh (how a late ask-once name arrives in live play) must
/// update it too (the 0112-era capture showed a permanently blank bar).
#[test]
fn npc_name_reaches_the_title_bar_on_open_and_on_refresh() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill things.".into(),
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Marshal McBride".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_DETAIL", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return getglobal('QuestFrameNpcNameText'):GetText() or ''")
            .unwrap(),
        "Marshal McBride",
        "the panel-open event's arg1 lands in the title bar"
    );
    // And it RENDERS: the quad extracts with a rect near the window top (the 0112-era capture
    // showed a blank bar — pin the whole path, not just the Lua-visible text).
    s.resolve();
    let name_quad = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content,
                benilla_ui::script::QuadContent::Text { text: Some(t), .. } if t == "Marshal McBride")
        })
        .expect("the NPC-name FontString extracts a quad");
    assert!(
        name_quad.rect.is_some(),
        "the NPC-name quad carries a resolved rect (got None — under-constrained)"
    );

    // The late-name path: open with an empty name, the refresh brings it.
    s.run("getglobal('QuestFrameNpcNameText'):SetText('')")
        .unwrap();
    s.fire_event(
        "QUEST_ITEM_UPDATE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert_eq!(
        s.eval::<String>("return getglobal('QuestFrameNpcNameText'):GetText() or ''")
            .unwrap(),
        "Marshal McBride",
        "the QUEST_ITEM_UPDATE refresh also carries the name"
    );
}

/// The gossip window's twin law, on the greeting panel's quest-title rows: a title long enough to
/// WRAP at the row label's 275 px must grow its row, or the static/Lua-chained rows below print
/// through it (the shape of the gossip overlap the director reported). Same deterministic
/// 6 px/char × 14 px/line measure fake as the gossip row test.
#[test]
fn greeting_panel_title_rows_grow_to_their_wrapped_titles() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Stock sizes each row at show time — `SetHeight(GetTextHeight() + 2)` right after SetText
    // (QuestFrame.lua:245) — which the reference can do because its text measure is synchronous.
    // So is the app's (measure.rs); the harness installs the same seam's stand-in.
    s.set_text_measurer(Box::new(super::FixedWidthFont(7.0)));
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Greeting,
        greeting: "What can I do for you?".into(),
        active_titles: vec![
            "Deliver Thomas' Report to Marshal Dughan in Goldshire before the Defias move again"
                .into(),
            "Investigate the Echo Ridge Mine and report back to Marshal McBride at once".into(),
        ],
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Deputy Willem".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_GREETING", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let answer_measures = |s: &mut UiScript| {
        let answers: Vec<(u32, f32, f32, u64)> = s
            .fontstrings_needing_measure()
            .into_iter()
            .map(|r| {
                let ink = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(w) => (r.id, ink.min(w), (ink / w).ceil().max(1.0) * 14.0, r.key),
                    None => (r.id, ink, 14.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    };
    answer_measures(&mut s);
    s.resolve();
    s.tick(0.016);
    answer_measures(&mut s);
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let row = |i: u32| -> (f32, f32, f32) {
        s.eval::<(f32, f32, f32)>(&format!(
            "local b = getglobal('QuestTitleButton{i}')\n\
             return b:GetTop(), b:GetBottom(), b:GetFontString():GetHeight()"
        ))
        .unwrap()
    };
    let (t1, b1, h1) = row(1);
    let (t2, b2, h2) = row(2);
    assert!(h1 >= 24.0 && h2 >= 24.0, "both titles wrap: {h1}, {h2}");
    assert!(
        (t1 - b1 - (h1 + 2.0)).abs() < 0.5,
        "row 1 is its wrapped title + 2: got {}, title {h1}",
        t1 - b1
    );
    assert!(
        (t2 - b2 - (h2 + 2.0)).abs() < 0.5,
        "row 2 is its wrapped title + 2: got {}, title {h2}",
        t2 - b2
    );
    assert!(
        t2 <= b1 + 0.5,
        "row 2 starts at/below row 1's bottom: row1 bottom {b1}, row2 top {t2}"
    );
}

/// The questgiver rows' modifier fork, on the panel where it can do the most
/// damage: the REWARD panel, whose choice rows are also the quest's reward *selection*. Ref
/// `QuestRewardItem_OnClick` (QuestFrame.lua:127-141) — ctrl previews, shift posts the link, and the
/// choice SELECT is the third arm, so neither modified click may also pick the reward. The fixed
/// reward row beside it rides the plainer `QuestItem_OnClick` (l.115-125, the base
/// QuestItemTemplate's own OnClick) and previews the same way with no select arm to disturb.
///
/// Ordering note: the dressing room docks the same left UIPanel slot as the questgiver window
/// (`UiPanels.xml`, `pushable = 2` vs the giver's `0`), so the ctrl arm is exercised last — opening
/// it closes the giver window, exactly as it does in play.
#[test]
fn reward_rows_preview_and_post_without_selecting_the_choice() {
    benilla_formats::wow_data_or_skip!();
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
    load_xml(&s, "ScrollTemplates.xml"); // our scroll kit + the placeholder icon
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\DressUpFrame.xml"); // DressUpItemLink lives here
    load_xml(&s, "Interface\\FrameXML\\UIMenu.xml"); // the kit its menus build from
    load_xml(&s, "Interface\\FrameXML\\ChatFrame.xml"); // ChatFrameEditBox lives here
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\FloatingChatFrame.xml");

    const SWORD: &str = "|cffffffff|Hitem:2299:0:0:0|h[Worn Sword]|h|r";
    const MACE: &str = "|cffffffff|Hitem:2300:0:0:0|h[Worn Mace]|h|r";
    let item = |id: u32, name: &str, link: &str| benilla_ui::script::QuestItemView {
        item_id: id,
        name: Some(name.into()),
        texture: None,
        count: 1,
        quality: 1,
        usable: true,
        link: Some(link.into()),
    };
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Reward,
        title: "A Threat Within".into(),
        body: "Well done.".into(),
        choices: vec![
            item(2299, "Worn Sword", SWORD),
            item(2300, "Worn Mace", MACE),
        ],
        rewards: vec![item(
            2504,
            "Worn Shortbow",
            "|cffffffff|Hitem:2504:0:0:0|h[Worn Shortbow]|h|r",
        )],
        ..QuestState::default()
    }));
    s.set_unit(
        "npc",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Marshal McBride".into()),
            ..Default::default()
        }),
    );
    s.fire_event("QUEST_COMPLETE", vec![]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The regression this fork could break: a PLAIN click still picks the choice (the ref's third
    // arm). Row 2 is picked here so the modified clicks below — aimed at row 1 — would visibly
    // move it if they leaked into the select arm.
    s.run("QuestRewardItem2:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return QuestFrameRewardPanel.itemChoice")
            .unwrap(),
        2,
        "a plain click still selects the reward choice"
    );
    assert!(s
        .eval::<bool>("return QuestRewardItemHighlight:IsShown()")
        .unwrap());
    assert!(
        s.take_dressup_intents().is_empty(),
        "a plain click never opens the dressing room"
    );

    // SHIFT + chat open → the row's full escaped link, and the selection is untouched.
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.run("QuestRewardItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        SWORD,
        "shift-click posted choice 1's link"
    );
    assert_eq!(
        s.eval::<i64>("return QuestFrameRewardPanel.itemChoice")
            .unwrap(),
        2,
        "the shift arm returns — it must NOT also select the clicked choice"
    );

    // A FIXED reward row posts too — same pool, same template now, and the select arm is gated
    // by `button.kind == "choice"` rather than by which pool the row came from.
    s.run("ChatFrameEditBox:SetText(\"\")").unwrap();
    s.set_modifiers(true, false, false);
    s.run("QuestRewardItem3:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "|cffffffff|Hitem:2504:0:0:0|h[Worn Shortbow]|h|r"
    );

    // CTRL → the dressing room wearing the clicked reward, and STILL no reselect. The intent pair
    // is ordered: Dress (the room was closed, so it re-dresses in the player's own gear) then TryOn.
    s.set_modifiers(false, true, false);
    s.run("QuestRewardItem1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.take_dressup_intents(),
        vec![
            benilla_ui::script::DressUpIntent::Dress,
            benilla_ui::script::DressUpIntent::TryOn(2299)
        ],
        "ctrl-click opened the room wearing choice 1"
    );
    assert_eq!(
        s.eval::<i64>("return QuestFrameRewardPanel.itemChoice")
            .unwrap(),
        2,
        "the ctrl arm returns — it must NOT also select the clicked choice"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
