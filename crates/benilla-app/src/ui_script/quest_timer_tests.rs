//! The stock timed-quest window (`QuestTimerFrame.lua`): one `SecondsToTime` row per timed quest,
//! height `45 + 16 * n`, hidden at zero timers, each row mapped back to its quest.

use benilla_ui::script::{
    QuestLogEntryView, QuestLogObjectiveView, QuestLogState, ScriptValue, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// The window's own dependencies only, so a missing one shows.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The title's `text="QUEST_TIMERS"` resolves at load; an unknown name draws as the key.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `SecondsToTime` for every row, `UIParent_ManageFramePositions` for OnShow and OnHide.
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    // QuestLogFrame.xml brings `MAX_QUESTS` (`QuestLogFrame.lua:2`), the repaint's loop bound.
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestTimerFrame.xml");
    s
}

/// A quest-log row; `timer` is the absolute deadline, 0 for untimed.
fn quest(id: u32, title: &str, timer: u32) -> QuestLogEntryView {
    QuestLogEntryView {
        quest_id: id,
        title: title.into(),
        level: 10,
        timer,
        objectives: vec![QuestLogObjectiveView {
            text: "Bloodscalp Scout slain: 0/8".into(),
            kind: "monster".into(),
            finished: false,
            cur: 0,
            req: 8,
        }],
        ..Default::default()
    }
}

fn log(entries: Vec<QuestLogEntryView>) -> QuestLogState {
    QuestLogState {
        num_quests: entries.len() as u32,
        entries,
        hidden_quest_ids: Vec::new(),
    }
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsShown()"))
        .unwrap()
}

fn row_text(s: &UiScript, i: u32) -> String {
    s.eval::<String>(&format!("return QuestTimer{i}Text:GetText()"))
        .unwrap()
}

#[test]
fn the_window_follows_the_timed_quests() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    s.set_quest_log(log(vec![quest(783, "A Threat Within", 0)]));
    s.set_server_unix_time(1_000_000.0);
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(!shown(&s, "QuestTimerFrame"), "no timed quest, no window");

    // Two timed quests around an untimed one, 860 s and 45 s out.
    s.set_quest_log(log(vec![
        quest(1, "Deliver the Message", 1_000_860),
        quest(2, "A Threat Within", 0),
        quest(3, "Escort the Caravan", 1_000_045),
    ]));
    s.fire_event("QUEST_LOG_UPDATE", vec![]);

    assert!(shown(&s, "QuestTimerFrame"));
    assert!(shown(&s, "QuestTimer1") && shown(&s, "QuestTimer2"));
    assert!(!shown(&s, "QuestTimer3"), "only two timers are live");
    // The reference's timers read 1 s short, formatted by `SecondsToTime` (`UIParent.lua:1004`).
    assert_eq!(row_text(&s, 1), "14 Mins 19 Secs ");
    assert_eq!(row_text(&s, 2), "44 Secs ");
    assert_eq!(
        s.eval::<f64>("return QuestTimerFrame:GetHeight()").unwrap(),
        45.0 + 16.0 * 2.0
    );

    // Row 2 maps to the third log entry, past the untimed one.
    assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(2)").unwrap(), 3);

    // The clock alone moves; OnUpdate repaints with no QUEST_LOG_UPDATE (`QuestTimerFrame.lua:35`).
    s.set_server_unix_time(1_000_030.0);
    s.tick(0.2);
    assert_eq!(row_text(&s, 2), "14 Secs ");

    s.set_quest_log(log(vec![quest(2, "A Threat Within", 0)]));
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(!shown(&s, "QuestTimerFrame"));
}

#[test]
fn the_countdown_reaches_the_draw_list() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_quest_log(log(vec![quest(1, "Deliver the Message", 1_000_860)]));
    s.set_server_unix_time(1_000_000.0);
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    s.tick(0.0);

    let quads = s.extract();
    let text: Vec<String> = quads
        .iter()
        .filter_map(|q| match &q.content {
            benilla_ui::script::QuadContent::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        text.iter().any(|t| t.contains("14 Mins")),
        "the countdown never reached the draw list: {text:?}"
    );
    assert!(
        text.iter().any(|t| t == "Quest Timers"),
        "the QUEST_TIMERS title never reached the draw list: {text:?}"
    );
}

/// The window also repaints on `PLAYER_ENTERING_WORLD` (`QuestTimerFrame.lua:3`).
#[test]
fn a_world_entry_paints_a_quest_already_running() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_quest_log(log(vec![quest(1, "Deliver the Message", 1_000_090)]));
    s.set_server_unix_time(1_000_000.0);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Bool(true)]);
    assert!(shown(&s, "QuestTimerFrame"));
    // One minute takes the singular `MINUTES_ABBR`, not its `_P1` plural; 90 s reads 89.
    assert_eq!(row_text(&s, 1), "1 Min 29 Secs ");
}
