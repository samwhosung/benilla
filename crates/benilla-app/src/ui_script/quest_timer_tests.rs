//! The timed-quest countdown window against the transcribed reference behaviour (
//! closing B234): the extracted 1.12 `QuestTimerFrame.lua` is the spec — one `SecondsToTime` line
//! per timed quest, the frame's height `45 + 16·n`, hidden entirely at zero timers, and every row
//! mapping back to its quest for the click and the hover.
//!
//! Three halves, and the last two are the load-bearing ones (`mirror_timer_tests`' lesson, and
//! 0675's): the state tests read the Lua back, which is blind to what actually *paints*; the
//! [`draw list`](UiScript::extract) test is what would catch a window that is shown, correctly
//! filled and invisible; and the geometry diff is what would catch every number being wrong while
//! all of the above passes.

use benilla_ui::script::{
    QuestLogEntryView, QuestLogObjectiveView, QuestLogState, ScriptValue, UiScript,
};

use super::test_ui::load_ui as load_xml;

/// Fonts + UIParent (SecondsToTime lives there) + the window itself. Deliberately NOT the whole
/// manifest: the window must stand up on its own dependencies, and a harness that loads everything
/// hides a missing one.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The window's title is `text="QUEST_TIMERS"`, resolved against the global at load — without
    // the player's own strings it draws the KEY, which is exactly what a `text=` attribute does
    // with an unknown name.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `SecondsToTime`, which the reference's `QuestTimerFrame_Update` formats every row with, and
    // `UIParent_ManageFramePositions`, which its OnShow/OnHide call.
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    // `MAX_QUESTS`, the loop bound the reference's repaint hides its spare rows with. 1.12
    // declares it on QuestLogFrame.lua:2 and so do we (1751 window 16) — a nil there is
    // `'for' limit must be a number` on the first repaint, not a missing row.
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

/// A quest-log row. `timer` is the descriptor slot's absolute deadline (0 = untimed).
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

/// The reference's whole visible contract in one pass: nothing at all with no timed quest; the
/// window with one line per timed quest, in log order, captioned by `SecondsToTime`; the height
/// `45 + 16·n`; and back to hidden when the last timer goes.
#[test]
fn the_window_follows_the_timed_quests() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    // An untimed log: QUEST_LOG_UPDATE fires, the frame stays down.
    s.set_quest_log(log(vec![quest(783, "A Threat Within", 0)]));
    s.set_server_unix_time(1_000_000.0);
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(!shown(&s, "QuestTimerFrame"), "no timed quest, no window");

    // Two timed quests either side of an untimed one. 14:20 and 45 s out.
    s.set_quest_log(log(vec![
        quest(1, "Deliver the Message", 1_000_860),
        quest(2, "A Threat Within", 0),
        quest(3, "Escort the Caravan", 1_000_045),
    ]));
    s.fire_event("QUEST_LOG_UPDATE", vec![]);

    assert!(shown(&s, "QuestTimerFrame"));
    assert!(shown(&s, "QuestTimer1") && shown(&s, "QuestTimer2"));
    assert!(!shown(&s, "QuestTimer3"), "only two timers are live");
    // SecondsToTime's shipped spelling, plural rule and trailing space, all of it (ref
    // UIParent.lua:1004-1031) — over the reference's own `−1`, so a 860-second
    // gap reads 859 and a 45-second one 44.
    assert_eq!(row_text(&s, 1), "14 Mins 19 Secs ");
    assert_eq!(row_text(&s, 2), "44 Secs ");
    assert_eq!(
        s.eval::<f64>("return QuestTimerFrame:GetHeight()").unwrap(),
        45.0 + 16.0 * 2.0
    );

    // Row 2 belongs to the THIRD log entry — the untimed row in the middle must not shift it.
    assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(2)").unwrap(), 3);

    // The clock alone moves and the OnUpdate repaints — no QUEST_LOG_UPDATE, which is the whole
    // point of the engine owning the subtraction.
    s.set_server_unix_time(1_000_030.0);
    s.tick(0.2);
    assert_eq!(row_text(&s, 2), "14 Secs ");

    // The last timer expires out of the list: the window goes with it.
    s.set_quest_log(log(vec![quest(2, "A Threat Within", 0)]));
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(!shown(&s, "QuestTimerFrame"));
}

/// The countdown must actually reach the screen. The state test above passes just as happily for a
/// window that is shown, correctly filled, and drawing nothing — the failure mode `cast_tests`
/// and `mirror_timer_tests` were both written for.
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

/// PLAYER_ENTERING_WORLD is the other event the reference registers (`QuestTimerFrame.lua:3`) —
/// a world entry with a timed quest already in the log must paint without waiting for the log to
/// change again.
#[test]
fn a_world_entry_paints_a_quest_already_running() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_quest_log(log(vec![quest(1, "Deliver the Message", 1_000_090)]));
    s.set_server_unix_time(1_000_000.0);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![ScriptValue::Bool(true)]);
    assert!(shown(&s, "QuestTimerFrame"));
    // Singular at exactly one minute — SecondsToTime's `_P1` plural rule, the ref's own; 90 s of
    // gap reads 89 through the `−1`.
    assert_eq!(row_text(&s, 1), "1 Min 29 Secs ");
}
