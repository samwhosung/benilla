//! The shipped **questgiver window** open/close sound, driven engine-only (no Bevy): the real
//! `assets/ui/QuestFrame.xml` loaded behind `UiPanels.xml` and shown/hidden through the wire events.
//! The window-sound convention's machine check for the quest arc (decision 0090), the sibling of the
//! merchant/gossip/bag/loot sound tests.

use benilla_ui::script::{QuestPanel, QuestState, ScriptValue, SoundRequest, UiScript};

/// Load one shipped `assets/ui/<file>` into `s`, panicking on any loader error (the panel tests'
/// loader, duplicated so this file is self-contained).
fn load_xml(s: &UiScript, file: &str) {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/ui")
            .join(file),
    )
    .unwrap();
    let doc = benilla_ui::framexml::parse(&text).unwrap();
    let report = benilla_ui::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "{file}: loader errors: {:?}",
        report.errors
    );
}

/// The questgiver window's open/close kits — the window-sound convention (decision 0090). The real
/// QuestFrame.lua plays igQuestListOpen in QuestFrame_OnShow (l.285) and igQuestListClose in
/// QuestFrame_OnHide (l.294), wired via the frame OnShow/OnHide (QuestFrame.xml l.1012/1015). A
/// questgiver event (QUEST_DETAIL here) → ShowUIPanel → Show() fires OnShow; QUEST_FINISHED →
/// HideUIPanel → Hide() fires OnHide. Nothing queues at load (the frame is authored hidden="true").
/// Same pair as gossip — questgiver and gossip are the same "list" surface to the client.
#[test]
fn questgiver_show_hide_plays_open_and_close_kits() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    // The BenillaMoney_* purse helpers the quest reward/progress panels repaint through live in
    // MerchantFrame.xml (the same documented cross-window dep the bag tests load).
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

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
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

    let panels = [
        "QuestGreetingPanel",
        "QuestDetailPanel",
        "QuestProgressPanel",
        "QuestRewardPanel",
    ];
    let is_shown =
        |s: &UiScript, name: &str| s.eval::<bool>(&format!("return {name}:IsShown()")).unwrap();

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Greeting,
        greeting: "What can I do for you?".into(),
        active_titles: vec!["Report to Goldshire".into()],
        ..QuestState::default()
    }));
    s.fire_event(
        "QUEST_GREETING",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestGreetingPanel",
            "only the greeting panel is shown after QUEST_GREETING"
        );
    }
    assert_eq!(
        s.eval::<String>("return QuestNpcNameText:GetText()")
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
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for name in panels {
        assert_eq!(
            is_shown(&s, name),
            name == "QuestDetailPanel",
            "only the detail panel is shown after QUEST_DETAIL"
        );
    }
    assert!(
        s.take_sounds().is_empty(),
        "a panel switch with the window already open plays no kit (only OnShow/OnHide of the window do)"
    );
}

/// The reward/choice grid, ported from the ref's shared `QuestFrameItems_Update` (the SAME idiom the
/// quest log's own reward rows already prove out) — now on REAL per-panel widgets
/// (`QuestDetailChoice*`/`QuestDetailReward*`), exercised on the Detail panel with the
/// fixture shape the task's capture drives: 2 choices + 1 fixed reward + money.
#[test]
fn detail_panel_reward_grid_follows_the_refs_two_per_row_layout() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

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
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s
        .eval::<bool>("return QuestDetailChoice1:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return QuestDetailChoice2:IsShown()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return QuestDetailChoice3:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return QuestDetailChoice1Name:GetText()")
            .unwrap(),
        "Worn Sword"
    );
    assert_eq!(
        s.eval::<String>("return QuestDetailItemChooseText:GetText()")
            .unwrap(),
        "You may choose one of these rewards:"
    );
    assert_eq!(
        s.eval::<String>("return QuestDetailItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:",
        "choices present -> the ref's \"also\" wording (QuestFrame.lua:461-462)"
    );
    assert!(s
        .eval::<bool>("return QuestDetailReward1:IsShown()")
        .unwrap());
    assert!(!s
        .eval::<bool>("return QuestDetailReward2:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return QuestDetailReward1Name:GetText()")
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
    // QuestItemTemplate script — the ctrl/shift fork, decisions 1059/1060), but it has no select
    // arm at all: an unmodified click must not raise and must not set an itemChoice.
    s.run("QuestDetailChoice1:Click()").ok();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return QuestFrame.itemChoice").unwrap(),
        0,
        "a detail-panel row never selects a reward"
    );
}

/// Stock 1.12 reds the icon and name-frame behind unusable reward rows. The quality-colored name
/// stays unchanged, and reused rows must restore both texture tints.
#[test]
fn questgiver_reward_rows_tint_unusable_items_and_reset_reused_rows() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "QuestFrame.xml");

    let item = |name: &str, usable| benilla_ui::script::QuestItemView {
        name: Some(name.into()),
        quality: 1,
        usable,
        ..Default::default()
    };
    let state = |panel, choice_usable, reward_usable| QuestState {
        panel,
        choices: vec![item("Militia Warhammer", choice_usable)],
        rewards: vec![item("Militia Hammer", reward_usable)],
        ..QuestState::default()
    };

    s.set_quest(Some(state(QuestPanel::Detail, false, false)));
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestDetailChoice1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestDetailChoice1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<(f32, f32, f32)>(
            "local r, g, b = QuestDetailChoice1Name:GetTextColor() return r, g, b"
        )
        .unwrap(),
        (1.0, 1.0, 1.0),
        "usability must not replace the item-quality text color"
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestDetailReward1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestDetailReward1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );

    s.set_quest(Some(state(QuestPanel::Detail, true, true)));
    s.fire_event("QUEST_ITEM_UPDATE", vec![]);
    for region in [
        "QuestDetailChoice1IconTexture",
        "QuestDetailChoice1NameFrame",
        "QuestDetailReward1IconTexture",
        "QuestDetailReward1NameFrame",
    ] {
        assert_eq!(
            s.eval::<Vec<f32>>(&format!("return {{{region}:GetVertexColor()}}"))
                .unwrap(),
            vec![1.0, 1.0, 1.0, 1.0],
            "a reused detail row must clear its old red tint"
        );
    }

    s.set_quest(Some(state(QuestPanel::Reward, false, false)));
    s.fire_event(
        "QUEST_COMPLETE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    for region in [
        "QuestRewardChoice1IconTexture",
        "QuestRewardChoice1NameFrame",
        "QuestRewardReward1IconTexture",
        "QuestRewardReward1NameFrame",
    ] {
        assert_eq!(
            s.eval::<Vec<f32>>(&format!("return {{{region}:GetVertexColor()}}"))
                .unwrap(),
            vec![0.9, 0.0, 0.0, 1.0],
            "the reward panel must tint unusable rows"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The reward panel's choice rows ARE selectable (ref `QuestRewardItem_OnClick`) — clicking one moves
/// the highlight and arms `GetQuestReward`'s 1-based→0-based conversion.
#[test]
fn reward_panel_choice_click_selects_and_completes_with_zero_based_index() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

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
    s.fire_event(
        "QUEST_COMPLETE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        !s.eval::<bool>("return QuestRewardChoiceHighlight:IsShown()")
            .unwrap(),
        "no row picked yet"
    );
    // Completing without a choice picked is a no-op (ref: QuestChooseRewardError guard).
    s.run("QuestRewardCompleteButton:Click()").unwrap();
    assert!(
        s.take_quest_actions().is_empty(),
        "must pick a choice first"
    );

    s.run("QuestRewardChoice2:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestRewardChoiceHighlight:IsShown()")
            .unwrap(),
        "picking a choice row shows the highlight"
    );

    s.run("QuestRewardCompleteButton:Click()").unwrap();
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
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Greeting,
        greeting: "Hello".into(),
        ..QuestState::default()
    }));
    s.fire_event(
        "QUEST_GREETING",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestFrame:IsVisible()").unwrap());

    s.run("QuestGreetingGoodbyeButton:Click()").unwrap();
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
#[test]
fn detail_panel_action_buttons_resolve_to_real_onscreen_rects() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestAcceptButton:IsShown()")
        .unwrap());
    assert!(s
        .eval::<bool>("return QuestDeclineButton:IsShown()")
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

    // The instant-text arm (QUEST_FADING_DISABLE pinned "1"): OnShow still disables Accept and
    // zeroes the block for the ref's one-frame window — the reveal edge just starts at 1024.
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
        .eval::<bool>("return QuestAcceptButton:IsEnabled()")
        .unwrap());
    assert_eq!(
        s.eval::<f32>("return QuestDetailTextAlphaFrame:GetAlpha()")
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

/// The ref write-on survives verbatim behind the flag: with QUEST_FADING_DISABLE = "0" (the
/// ref's own default, our pin is "1"), the description writes on at 40 chars/s scratching the
/// quill each tick, Accept stays dead mid-write, and the objectives/rewards block FADES in over
/// QUESTINFO_FADE_IN rather than snapping.
#[test]
fn write_on_still_fades_when_instant_text_is_off() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");
    s.eval::<()>(r#"QUEST_FADING_DISABLE = "0""#).unwrap();
    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill kobolds.".into(),
        ..QuestState::default()
    }));
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
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
        .eval::<bool>("return QuestAcceptButton:IsEnabled()")
        .unwrap());

    // Half a second more runs the 13-char gradient off (40 chars/s): Accept wakes and the block
    // is mid-FADE — armed at 0 and ramping over QUESTINFO_FADE_IN, not snapped to opaque.
    s.tick(0.5);
    assert!(s.errors().is_empty(), "tick errors: {:?}", s.errors());
    assert!(s
        .eval::<bool>("return QuestAcceptButton:IsEnabled()")
        .unwrap());
    let alpha = s
        .eval::<f32>("return QuestDetailTextAlphaFrame:GetAlpha()")
        .unwrap();
    assert!(
        alpha < 1.0,
        "fading mode ramps the block in (got alpha {alpha})"
    );
    s.tick(1.1);
    assert_eq!(
        s.eval::<f32>("return QuestDetailTextAlphaFrame:GetAlpha()")
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
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

    s.set_quest(Some(QuestState {
        panel: QuestPanel::Detail,
        title: "A Threat Within".into(),
        body: "Kill things.".into(),
        ..QuestState::default()
    }));
    s.fire_event(
        "QUEST_DETAIL",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<String>("return getglobal('QuestNpcNameText'):GetText() or ''")
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
    s.run("getglobal('QuestNpcNameText'):SetText('')").unwrap();
    s.fire_event(
        "QUEST_ITEM_UPDATE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert_eq!(
        s.eval::<String>("return getglobal('QuestNpcNameText'):GetText() or ''")
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
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml"); // the shared scroll kit the window rides
    load_xml(&s, "QuestFrame.xml");

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
    s.fire_event(
        "QUEST_GREETING",
        vec![ScriptValue::Str("Deputy Willem".into())],
    );
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
    assert!(h1 >= 28.0 && h2 >= 28.0, "both titles wrap: {h1}, {h2}");
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

/// The questgiver rows' modifier fork (decisions 1059/1060), on the panel where it can do the most
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
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "QuestFrame.xml");
    load_xml(&s, "UIParent.xml"); // BenillaChatEdit_InsertLink lives here
    load_xml(&s, "DressUpFrame.xml"); // DressUpItemLink lives here
    load_xml(&s, "ChatFrame.xml"); // ChatFrameEditBox lives here

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
    s.fire_event(
        "QUEST_COMPLETE",
        vec![ScriptValue::Str("Marshal McBride".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The regression this fork could break: a PLAIN click still picks the choice (the ref's third
    // arm). Row 2 is picked here so the modified clicks below — aimed at row 1 — would visibly
    // move it if they leaked into the select arm.
    s.run("QuestRewardChoice2:Click()").unwrap();
    assert_eq!(
        s.eval::<i64>("return QuestFrame.itemChoice").unwrap(),
        2,
        "a plain click still selects the reward choice"
    );
    assert!(s
        .eval::<bool>("return QuestRewardChoiceHighlight:IsShown()")
        .unwrap());
    assert!(
        s.take_dressup_intents().is_empty(),
        "a plain click never opens the dressing room"
    );

    // SHIFT + chat open → the row's full escaped link, and the selection is untouched.
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.run("QuestRewardChoice1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        SWORD,
        "shift-click posted choice 1's link"
    );
    assert_eq!(
        s.eval::<i64>("return QuestFrame.itemChoice").unwrap(),
        2,
        "the shift arm returns — it must NOT also select the clicked choice"
    );

    // A FIXED reward row (plain QuestItemTemplate → QuestItem_OnClick) posts too.
    s.run("ChatFrameEditBox:SetText(\"\")").unwrap();
    s.set_modifiers(true, false, false);
    s.run("QuestRewardReward1:Click()").unwrap();
    s.set_modifiers(false, false, false);
    assert_eq!(
        s.eval::<String>("return ChatFrameEditBox:GetText()")
            .unwrap(),
        "|cffffffff|Hitem:2504:0:0:0|h[Worn Shortbow]|h|r"
    );

    // CTRL → the dressing room wearing the clicked reward, and STILL no reselect. The intent pair
    // is ordered: Dress (the room was closed, so it re-dresses in the player's own gear) then TryOn.
    s.set_modifiers(false, true, false);
    s.run("QuestRewardChoice1:Click()").unwrap();
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
        s.eval::<i64>("return QuestFrame.itemChoice").unwrap(),
        2,
        "the ctrl arm returns — it must NOT also select the clicked choice"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
