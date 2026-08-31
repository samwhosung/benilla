//! The shipped **quest log window** driven end-to-end, engine-only (no Bevy): the real
//! `assets/ui/QuestLogFrame.xml` loaded behind `UiPanels.xml`/`MerchantFrame.xml` (the money helpers
//! its reward rows reuse) and fed a synthetic 8-entry log + a resolved detail — mirroring
//! `quest_tests.rs`/`bag_tests.rs`'s engine-only harness for the quest-log slice (decision 0088 arc).

use benilla_ui::script::{
    ExtractedQuad, QuadContent, QuestItemView, QuestLogDetail, QuestLogEntryView,
    QuestLogObjectiveView, QuestLogState, SoundRequest, UiScript,
};

/// Load one shipped `assets/ui/<file>` into `s`, panicking on any loader error (the panel/quest
/// tests' loader, duplicated so this file is self-contained).
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

/// Find a bare frame's own rect via its `QuadContent::Frame` entry (`panel_tests.rs`'s helper,
/// duplicated) — used to locate the window so the wheel test's coordinates land inside it.
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

/// 8 flat quest entries (exercises the 6-row faux-scroll), the first carrying one objective line
/// (the auto-picked first selection reads it), + a resolved detail for that selection (no choices,
/// 1 fixed reward + money) — the fixture every test below shares. Each entry gets a distinct
/// `quest_id` (the watch set's stable key — `benilla-ui`'s `quest_log.rs` module doc) so the
/// watch/tracker tests below can tell entries apart across a scroll/reselect.
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
            ..Default::default()
        })
        .collect();
    QuestLogState {
        num_quests: 8,
        entries,
        detail: Some(QuestLogDetail {
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
                // The reward row's ctrl/shift payload (`GetQuestLogItemLink`, decisions 1059/1060)
                // — `ui_quest_log.rs` builds it through `ui_items::item_link` once the template
                // lands, so the fixture carries that exact shape.
                link: Some(HAMMER_LINK.into()),
            }],
        }),
    }
}

/// The fixture reward's full escaped link — a real 1.12 item, quality white.
const HAMMER_LINK: &str = "|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r";

/// The loader itself: every file the window depends on parses and materializes with no errors.
#[test]
fn shipped_questlog_frame_loads_clean() {
    let s = UiScript::new().unwrap();
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");
}

/// The whole contract in one end-to-end drive: open plays the kit and renders row 1 + the count +
/// the auto-picked first selection + the pushed detail text; the wheel scrolls the list; the abandon
/// two-step (mark on click, confirm Yes/No) drains (or doesn't) the right intent; close plays the
/// close kit.
#[test]
fn shipped_questlog_frame_drives_end_to_end() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(eight_entries());

    // Hidden at load: no sound queued (never transitions on startup).
    assert!(
        s.take_sounds().is_empty(),
        "no sound at load (never transitions)"
    );
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());

    // ToggleQuestLog() (the 'L' binding's entry point) opens it through ShowUIPanel.
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestLogOpen".into())],
        "opening the quest log plays igQuestLogOpen"
    );

    // Row 1 carries entry 1's title (indented), the count line reads "Quests: |cffffffff8/20|r", and nothing having been
    // selected, the first non-header entry auto-selects (pin §2's SetFirstValidSelection).
    assert!(s
        .eval::<String>("return QuestLogTitle1Text:GetText()")
        .unwrap()
        .contains("Quest 1"));
    assert_eq!(
        s.eval::<String>("return QuestLogCount:GetText()").unwrap(),
        "Quests: |cffffffff8/20|r"
    );
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    // The detail pane carries the pushed title + objective text.
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
    // The reward row (QuestFrame.xml's item-row pattern, reused) + its money.
    assert_eq!(
        s.eval::<String>("return QuestLogReward1Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );
    assert_ne!(
        s.eval::<String>("return QuestLogRewardTitleText:GetText()")
            .unwrap(),
        "",
        "the Rewards header shows once there's a reward to show"
    );
    // Lua-managed Description header (no longer a static `text=` — QuestLogFrame.xml's header
    // comment on why it used to float over the empty-log parchment).
    assert_eq!(
        s.eval::<String>("return QuestLogDescriptionTitle:GetText()")
            .unwrap(),
        "Description"
    );
    // No choices in this fixture: "You will receive:" (REWARD_ITEMS_ONLY), not "...also...", and
    // the choose-text/choice rows stay blank/hidden (QuestFrame.lua:454-473).
    assert_eq!(
        s.eval::<String>("return QuestLogItemReceiveText:GetText()")
            .unwrap(),
        "You will receive:"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogItemChooseText:GetText()")
            .unwrap(),
        ""
    );
    assert!(!s.eval::<bool>("return QuestLogChoice1:IsShown()").unwrap());

    // Wheel DOWN over an actual list ROW (delta -1) advances the faux-scroll offset by one: row 1 now
    // shows entry 2's title. Aiming over a row (not the empty margin) is the case that matters — a spin
    // over a row bubbles up the parent chain to the window's OnMouseWheel; when that handler lived only
    // on the sibling catcher, wheeling over a row did nothing (the trainer window hit the same bug).
    // Row 3 ("Quest 3") is unambiguously a list row — the detail pane shows the selected Quest 1.
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
    s.mouse_wheel(wx, wy, -1.0);
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    assert!(
        s.eval::<String>("return QuestLogTitle1Text:GetText()")
            .unwrap()
            .contains("Quest 2"),
        "wheel-down scrolled the list by one row"
    );
    // Selection itself is untouched by scrolling.
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);

    // Abandon, No path: marks the selection, shows the registry's ABANDON_QUEST entry on the
    // shared StaticPopup engine (decision 0308 §3), but No drains nothing and hides it.
    s.run("BenillaQuestLogAbandonButton_OnClick()").unwrap();
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
    // No plays the dialog's own open/close pair only (ref StaticPopup_OnShow/OnHide,
    // StaticPopup.lua:1832/1841) — never the abandon kit.
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igMainMenuOpen".into()),
            SoundRequest::KitName("igMainMenuClose".into()),
        ],
        "No: dialog open/close kits only, no abandon kit"
    );

    // Abandon, Yes path: the pinned index (1, the selection at click time) drains, plus the kit.
    s.run("BenillaQuestLogAbandonButton_OnClick()").unwrap();
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_quest_log_abandons(), vec![1]);
    // Show → igMainMenuOpen; the ref OnClick runs OnAccept FIRST (the abandon kit), THEN hides
    // (igMainMenuClose) — StaticPopup.lua:1850-1868's order, which the old shim had reversed.
    assert_eq!(
        s.take_sounds(),
        vec![
            SoundRequest::KitName("igMainMenuOpen".into()),
            SoundRequest::KitName("igQuestLogAbandonQuest".into()),
            SoundRequest::KitName("igMainMenuClose".into()),
        ]
    );
    assert!(!s.eval::<bool>("return StaticPopup1:IsShown()").unwrap());

    // ToggleQuestLog() again closes it through HideUIPanel, playing the close kit.
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(!s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());
    assert_eq!(
        s.take_sounds(),
        vec![SoundRequest::KitName("igQuestLogClose".into())],
        "closing the quest log plays igQuestLogClose"
    );
}

/// The quest-watch tracker slice: SHIFT+left-click toggles the watch (the ref's own fork,
/// QuestLogFrame.lua:469-505 — driven through the `set_modifiers` mirror the cursor arc landed),
/// toggling both the row's watch checkbox (a `SetTexture` toggle — bare Texture regions have no
/// Show/Hide, same limit as the FontString regions elsewhere in this window) and the
/// always-on-screen tracker HUD (`QuestWatchFrame`), which the log window itself doesn't
/// own or gate on visibility.
#[test]
fn shift_click_toggles_the_watch_checkbox_and_the_tracker_hud() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Nothing watched yet — the tracker HUD is hidden and row 1 shows no checkbox texture.
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

    // Shift-click row 1 ("Quest 1", the entry `eight_entries` gives one objective — a zero-objective
    // quest can't be watched, ref QUEST_WATCH_NO_OBJECTIVES). Row 1 sits at the list's TOPLEFT+(19,
    // -75), 300×16 (`QuestLogFrame.xml`'s own row-chain comment) — its center is inside the button
    // but outside the checkbox's outside-the-row gutter, so this can't be mistaken for a checkbox hit.
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
    // The check SEATS just past the title's ink (ref QuestLog.lua:224 — dummy width + 24 from the
    // row's left, netting ink end + 4 behind the text's own +20 inset), NOT the first build's
    // outside-the-row gutter at row_left − 14. The engine-only harness has no font atlas, so
    // answer the measure round-trip with a deterministic 6 px/char fake (the app-side extract
    // answers from the real atlas) — the assertion pins the ANCHOR GRAPH seating the check at
    // measured ink + 4, in a loose band that bakes no glyph metrics into the test.
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

    // The tracker HUD is now visible: the flat line pool (ref MAX_QUESTWATCH_LINES) carries the
    // watched quest's title on line 1 and its objective on line 2, ref-colored (title dark-gold —
    // objectives incomplete; objective 0.8-gray — unfinished).
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
    // A manual watch is permanent: no auto-watch timer entry rides it.
    assert!(s
        .eval::<bool>("return BENILLA_QUEST_WATCH_TIMERS[\"Quest 1\"] == nil")
        .unwrap());

    // Shift-click again unwatches: the checkbox clears and — nothing left watched — the whole HUD
    // hides (the ref hides the FRAME and returns before touching the lines; stale text stays,
    // drawn by nothing).
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

/// Shift-clicking a quest with zero objectives (ref `QUEST_WATCH_NO_OBJECTIVES`) and shift-clicking
/// past `GetNumQuestWatches() >= 5` (ref `QUEST_WATCH_TOO_MANY`) both refuse the watch and put the
/// ref's red line on the errors frame through its own `UIErrorsFrame:AddMessage`; the click still
/// selects the row exactly like any other click (pin §5).
#[test]
fn watch_guards_no_op_without_erroring() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "ErrorsFrame.xml"); // the guards' red-line surface
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();

    // Row 1's TOPLEFT is the window's (19,-75); each of the 6 visible rows is 300×16 chained with a
    // 1px OVERLAP, not a gap (`QuestLogFrame.xml`'s row-chain comment) — pitch 16-1=15px. Row n's
    // center, screen space.
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    let row_center = |n: u32| win.top - 75.0 - 15.0 * (n - 1) as f32 - 8.0;
    let x = win.left + 19.0 + 150.0;

    // Row 2 ("Quest 2") carries no objectives in this fixture — shift-clicking it can't watch.
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
    // Read the toast off the drawn quads: `UIErrorsFrame` is a real `<MessageFrame>` and its
    // lines are the widget's own bands, not named FontStrings.
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
        2,
        "the click still selects"
    );

    // Fill the watch list to the cap (5) with quests 2-6 via the Lua API directly — the engine-level
    // AddQuestWatch (`quest_log.rs`) has no objectives guard of its own (that guard is this file's,
    // in the click handler only — mirroring the ref's own split between the native call and
    // `QuestLogTitleButton_OnClick`'s Lua-side checks), so a zero-objective quest CAN be watched this
    // way. This deliberately leaves quest 1 (the fixture's only objective-bearing entry) unwatched.
    for i in 2..=6 {
        s.run(&format!("AddQuestWatch({i})")).unwrap();
    }
    assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);

    // Row 1 ("Quest 1", unwatched, HAS an objective) — shift-clicking it now clears the
    // no-objectives guard but hits the watch-list-full guard instead: still a no-op.
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

/// The auto quest watch (ref `AUTO_QUEST_WATCH` default-on, QuestLogFrame.lua:702-786 —
/// "Quests are automatically watched for 5 minutes when you achieve a quest objective."): the
/// engine's per-quest `BENILLA_QUEST_PROGRESS(logIndex)` arms a 300 s timed watch (the native
/// QUEST_WATCH_UPDATE carries the WATCH-LIST index per the §5-verified byte law — repaint only,
/// the shipped 1.12 auto-watch chain being broken at that seam), fresh progress
/// re-arms it, and expiry (the watch frame's OnUpdate) unwatches and hides the empty HUD.
#[test]
fn progress_auto_watches_for_five_minutes() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");
    s.set_quest_log(eight_entries());

    // A kill credit lands: the engine fires the event with the quest's 1-based log index.
    s.fire_event(
        "BENILLA_QUEST_PROGRESS",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(s.errors().is_empty(), "auto-watch errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s
        .eval::<bool>("return QuestWatchFrame:IsVisible()")
        .unwrap());
    assert!(s
        .eval::<bool>("return BENILLA_QUEST_WATCH_TIMERS[\"Quest 1\"] ~= nil")
        .unwrap());

    // 299 s in it still holds; fresh progress re-arms the timer; expiry unwatches and hides.
    s.tick(299.0);
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    s.fire_event(
        "BENILLA_QUEST_PROGRESS",
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

/// The flag that gates the above is the **reference's** uvar under the reference's name and value
/// shape (decision 1136 — it was `BENILLA_AUTO_QUEST_WATCH`, a boolean, while nothing could change
/// it): `AUTO_QUEST_WATCH`, a `"1"`/`"0"` string, which is what the Interface page's row writes.
/// Off, progress no longer watches anything; back on, it does again.
///
/// The default asserted first is the one 1.12 *advertises* and does not deliver: it assigns the
/// NUMBER `1` (UIOptionsFrame.lua:122) while every consumer compares `== "1"`, so the shipped
/// client's auto-watch is dead until the player toggles the box. Ours ships the string.
#[test]
fn the_auto_watch_flag_is_the_references_uvar_and_gates_the_watch() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");
    s.set_quest_log(eight_entries());
    assert_eq!(s.eval::<String>("return AUTO_QUEST_WATCH").unwrap(), "1");

    s.run(r#"AUTO_QUEST_WATCH = "0""#).unwrap();
    s.fire_event(
        "BENILLA_QUEST_PROGRESS",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(
        !s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "the flag off means progress watches nothing"
    );

    s.run(r#"AUTO_QUEST_WATCH = "1""#).unwrap();
    s.fire_event(
        "BENILLA_QUEST_PROGRESS",
        vec![benilla_ui::script::ScriptValue::Int(1)],
    );
    assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
    assert!(s.errors().is_empty(), "auto-watch errors: {:?}", s.errors());
}

/// The empty-log v1 simplification (this file's XML header comment): zero entries hides every row,
/// disables Abandon, and shows the centered empty-state message instead of `EmptyQuestLogFrame`.
#[test]
fn empty_quest_log_hides_rows_and_disables_abandon() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(QuestLogState::default());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // A bare FontString region has no Show/Hide/IsVisible in this engine (only Frames/Buttons do —
    // `region.rs`'s method table) — the empty-state message is a toggled SetText, read back here.
    assert_eq!(
        s.eval::<String>("return QuestLogEmptyText:GetText()")
            .unwrap(),
        "Your quest log is empty."
    );
    assert!(!s.eval::<bool>("return QuestLogTitle1:IsVisible()").unwrap());
    assert!(!s
        .eval::<bool>("return QuestLogAbandonButton:IsEnabled()")
        .unwrap());
    // The Description header is now Lua-managed (BenillaQuestLogDetail_Clear blanks it) rather than
    // a static `text=` — it used to float "Description" over the empty-log parchment with no
    // selection (this task's fix; QuestLogFrame.xml's header comment on the FontString).
    assert_eq!(
        s.eval::<String>("return QuestLogDescriptionTitle:GetText()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<String>("return QuestLogCount:GetText()").unwrap(),
        "Quests: |cffffffff0/20|r"
    );
}

/// `BenillaQuestLogRewards_Update`'s ported `QuestFrameItems_Update` layout (QuestFrame.lua:311-522)
/// — 2 choices + 1 fixed reward + money, the same shape the capture fixture (`ui-questlog`) drives:
/// choice rows 2-per-row, "You will also receive:" (choices present) anchored under the LEFT column
/// of the last choice row, the fixed reward below that, unused rows/choices past the count hidden.
#[test]
fn reward_rows_follow_the_refs_two_per_row_layout() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    let mut state = eight_entries();
    state.detail = Some(QuestLogDetail {
        description: "Speak with Marshal McBride.".into(),
        objectives_text: "Report to Marshal McBride.".into(),
        required_money: 0,
        reward_money: 150, // 1s50c, the fixture's own amount
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
    });
    s.set_quest_log(state);
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Both choice rows show, the rest of the 6-row cap stays hidden.
    assert!(s.eval::<bool>("return QuestLogChoice1:IsShown()").unwrap());
    assert!(s.eval::<bool>("return QuestLogChoice2:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestLogChoice3:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestLogChoice1Name:GetText()")
            .unwrap(),
        "Worn Sword"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogChoice2Name:GetText()")
            .unwrap(),
        "Worn Mace"
    );
    assert_eq!(
        s.eval::<String>("return QuestLogItemChooseText:GetText()")
            .unwrap(),
        "You may choose one of these rewards:"
    );

    // With choices present, the receive text reads "...also..." (QuestFrame.lua:461-462) — it
    // anchors under choice row 1 (the left/odd column: index=2 is even -> anchorIndex-1 -> 1, this
    // function's own logic). GetPoint() isn't wired for FontString regions in this engine (only
    // Frame/Button — `region.rs` has no GetPoint), so the anchor CHAIN itself is verified via the
    // mandatory capture-loop screenshot (this task's report), not introspected here.
    assert_eq!(
        s.eval::<String>("return QuestLogItemReceiveText:GetText()")
            .unwrap(),
        "You will also receive:"
    );

    // One fixed reward shows, chained under the receive text (its own SetPoint target).
    assert!(s.eval::<bool>("return QuestLogReward1:IsShown()").unwrap());
    assert!(!s.eval::<bool>("return QuestLogReward2:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return QuestLogReward1Name:GetText()")
            .unwrap(),
        "Militia Hammer"
    );

    assert_eq!(
        s.eval::<String>("return QuestLogRewardTitleText:GetText()")
            .unwrap(),
        "Rewards"
    );
}

/// Stock 1.12 leaves the quality-colored FontString alone and reds the two textures behind it.
/// Both legs reset because the fixed row widgets are reused when the selected quest changes.
#[test]
fn reward_rows_tint_unusable_items_and_reset_reused_rows() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    let detail = |choice_usable, reward_usable| QuestLogDetail {
        description: String::new(),
        objectives_text: String::new(),
        required_money: 0,
        reward_money: 0,
        choices: vec![QuestItemView {
            name: Some("Militia Warhammer".into()),
            quality: 1,
            usable: choice_usable,
            ..Default::default()
        }],
        rewards: vec![QuestItemView {
            name: Some("Militia Hammer".into()),
            quality: 1,
            usable: reward_usable,
            ..Default::default()
        }],
    };
    let mut state = eight_entries();
    state.detail = Some(detail(false, true));
    s.set_quest_log(state.clone());
    s.run("ToggleQuestLog()").unwrap();

    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogChoice1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogChoice1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<(f32, f32, f32)>(
            "local r, g, b = QuestLogChoice1Name:GetTextColor() return r, g, b"
        )
        .unwrap(),
        (1.0, 1.0, 1.0),
        "usability must not replace the item-quality text color"
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogReward1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogReward1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![1.0, 1.0, 1.0, 1.0]
    );

    state.detail = Some(detail(true, false));
    s.set_quest_log(state);
    s.fire_event("QUEST_LOG_UPDATE", vec![]);

    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogChoice1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![1.0, 1.0, 1.0, 1.0],
        "a reused choice row must clear its old red tint"
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogChoice1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogReward1IconTexture:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert_eq!(
        s.eval::<Vec<f32>>("return {QuestLogReward1NameFrame:GetVertexColor()}")
            .unwrap(),
        vec![0.9, 0.0, 0.0, 1.0]
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// One entry whose selected detail overflows the 261px-tall `QuestLogDetailScroll`: 10
/// objective lines (each a FIXED 12px slot, `QuestLogObjective1`'s own XML comment) plus a
/// long description and a reward row push the child's summed height (`BenillaQuestLogDetail_
/// ResizeChild`) well past the pane — load-bearing for the scroll/clip tests below even
/// engine-only (no font atlas, so every auto-height FontString measures 0 here — this fixture
/// overflows on the FIXED-height portions alone: 10×12px objective rows + the reward row).
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
        entries: vec![QuestLogEntryView {
            quest_id: 1,
            title: "A Very Long Quest".into(),
            level: 5,
            complete: 0,
            objectives,
            ..Default::default()
        }],
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
        }),
    }
}

/// The ScrollFrame clip (decision 0112 §4): every quad in the moved detail chain's subtree —
/// `QuestLogDetailChild` and its descendants (here, a reward row's icon-slot texture) —
/// carries `QuestLogDetailScroll`'s own resolved rect as its clip, so content that overflows
/// the pane never draws past its bottom edge; a sibling entirely outside the scroll child (the
/// window's own book-icon chrome) stays unclipped, mirroring `scrollframe.rs`'s own unit test for
/// the mechanism, just through the shipped window instead of a synthetic fixture.
#[test]
fn overflowing_detail_content_clips_to_the_scrollframe_rect() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

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

/// Wheeling over the detail pane changes `GetVerticalScroll()` — the ScrollFrame is
/// mouse-wheel-interactive by construction (decision 0112, no wheel-catcher needed), and is
/// declared after `QuestLogWheelCatcher` in this window's `<Frames>` so it out-ranks the
/// catcher within its own rect (this window's own XML comment on hit-test z-order).
#[test]
fn wheel_over_the_detail_pane_changes_vertical_scroll() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(overflowing_entry());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    s.resolve();
    let quads = s.extract();
    let scroll_rect = frame_rect(&quads, 300.0, 261.0);
    // Near the pane's own top-left, clear of the reward row (which sits further down the chain).
    let (x, y) = (scroll_rect.left + 10.0, scroll_rect.top - 10.0);

    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScroll:GetVerticalScroll()")
            .unwrap(),
        0.0
    );
    s.mouse_wheel(x, y, -1.0); // WoW convention: -1 = down
    assert!(s.errors().is_empty(), "wheel errors: {:?}", s.errors());
    let after = s
        .eval::<f32>("return QuestLogDetailScroll:GetVerticalScroll()")
        .unwrap();
    assert!(
        after > 0.0,
        "wheel-down over the detail pane increased its scroll offset, got {after}"
    );
}

/// Pin §3/§5's doNotScroll asymmetry (QUESTLOG-PIN.md, `QuestLogFrame.lua:348/455-457`): a manual
/// reselect always snaps the detail pane's scroll back to the top, but the `QUEST_LOG_UPDATE`
/// data-refresh path must NOT — a quest updating while already being read shouldn't yank the
/// reader's scroll position out from under them.
#[test]
fn selection_change_resets_detail_scroll_but_a_quest_log_update_refresh_does_not() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

    s.set_quest_log(overflowing_entry());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s.resolve(); // GetVerticalScrollRange (SetVerticalScroll's clamp) reads resolved rects.

    s.run("QuestLogDetailScroll:SetVerticalScroll(10)").unwrap();
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScroll:GetVerticalScroll()")
            .unwrap(),
        10.0
    );

    // A data refresh (fired the same way the app's own quest-log feed does) must not reset it.
    s.fire_event("QUEST_LOG_UPDATE", vec![]);
    assert!(s.errors().is_empty(), "refresh errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScroll:GetVerticalScroll()")
            .unwrap(),
        10.0,
        "QUEST_LOG_UPDATE's data-refresh path must not yank the scroll position (pin §5)"
    );

    // A manual reselect (the row-click path — no doNotScroll) DOES reset it.
    s.run(r#"BenillaQuestLogTitle_OnClick(QuestLogTitle1, "LeftButton")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "click errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return QuestLogDetailScroll:GetVerticalScroll()")
            .unwrap(),
        0.0,
        "a manual reselect snaps the detail pane's scroll back to the top"
    );
}

/// The SHARED item tooltip on a quest reward row (the director's "same tooltip as vendor items"):
/// hovering the reward fires the row's OnEnter → `SetItemById` → the ask-once store. First hover
/// (store cold) shows the fallback name line AND records the ask; after the app's push, a re-hover
/// renders the full stat head — the identical lines a vendor row/bag slot gets for this item.
#[test]
fn reward_row_hover_serves_the_shared_item_tooltip() {
    use benilla_ui::script::ItemTemplateView;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "GameTooltip.xml");
    // LootFrame.xml is here for BENILLA_LOOT_QUALITY_COLORS (the quality→colour table); it
    // needs the dropdown kit its GroupLootDropDown initializes against (benilla.toc l.64 vs 383).
    load_xml(&s, "UIDropDownMenu.xml");
    load_xml(&s, "LootFrame.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");

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
    // Extracted rects and mouse_move share the same y-up UI space (the merchant hover test's
    // own convention).
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

    // Cold store: the hover shows the tooltip (fallback name only) and records the ask.
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

    // The app answers; a re-hover renders the full stat head.
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

/// A DIALOG-strata popup's CHILDREN inherit the stratum (and parent level + 1) at creation —
/// the client's CreateFrame law. Regression: the popup's translucent backdrop used to draw OVER
/// its own Yes/No buttons (children stuck in MEDIUM while the popup sat in DIALOG — the abandon
/// dialog's dark buttons, pixel-diffed against the raw UI-DialogBox-Button-Up art).
#[test]
fn popup_children_inherit_the_dialog_stratum() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
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

/// The quest log's reward rows carry the same modifier fork as the questgiver's (decisions
/// 1059/1060) — ref `QuestLogRewardItem_OnClick` (QuestLogFrame.lua:542-552), the OnClick the ref's
/// own `QuestLogRewardItemTemplate` adds (ref QuestLogFrame.xml:118-120). There is no third arm
/// here: a quest-log reward row is never selectable, so a plain click stays inert — which is the
/// regression this pins, since the row had NO OnClick at all before this arc.
///
/// Ordering note: the dressing room docks the same left UIPanel slot as the log window, so the ctrl
/// arm runs last — opening the room closes the log, exactly as it does in play.
#[test]
fn reward_rows_preview_and_post_and_a_plain_click_stays_inert() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");
    load_xml(&s, "UIParent.xml"); // BenillaChatEdit_InsertLink lives here
    load_xml(&s, "DressUpFrame.xml"); // DressUpItemLink lives here
    load_xml(&s, "ChatFrame.xml"); // ChatFrameEditBox lives here

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(
        s.eval::<bool>("return QuestLogReward1:IsShown()").unwrap(),
        "the fixture's one fixed reward row is laid out"
    );
    let _ = s.take_dressup_intents();

    // A PLAIN click is inert: no room, no chat text, no error (the row has no select arm).
    s.run("QuestLogReward1:Click()").unwrap();
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

    // SHIFT + chat open → the reward's full escaped link (ref l.547-550).
    assert!(s.focus_editbox("ChatFrameEditBox"));
    s.set_modifiers(true, false, false);
    s.run("QuestLogReward1:Click()").unwrap();
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

    // CTRL → the room wearing the reward (ref l.543-546): re-dress first, then try it on.
    s.set_modifiers(false, true, false);
    s.run("QuestLogReward1:Click()").unwrap();
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

/// A quest log TITLE row is a quest NAME, not an item — ref `QuestLogTitleButton_OnClick`
/// (QuestLogFrame.lua:469-505): with the chat edit box up, shift-click inserts the row's own text
/// with its leading indent trimmed (l.478-480, plain text — 1.12 has no `|Hquest:` link); the
/// quest-watch toggle is that branch's `else` (l.481-500). Both halves in one test because it is the
/// *fork* that matters: watching is the behaviour that already shipped, and the insert must not have
/// eaten it. The select-and-repaint tail (l.503-504) runs either way.
#[test]
fn shift_click_on_a_title_posts_the_quest_name_with_chat_open_and_watches_with_it_closed() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Fonts.xml");
    load_xml(&s, "MoneyFrame.xml");
    load_xml(&s, "UiPanels.xml");
    load_xml(&s, "MerchantFrame.xml");
    load_xml(&s, "QuestLogFrame.xml");
    load_xml(&s, "UIParent.xml"); // BenillaChatEdit_InsertLink lives here
    load_xml(&s, "ChatFrame.xml"); // ChatFrameEditBox lives here

    s.set_quest_log(eight_entries());
    s.run("ToggleQuestLog()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Row 1's centre — the same seat `shift_click_toggles_the_watch_checkbox_and_the_tracker_hud`
    // computes (list TOPLEFT+(19,-75), 300×16).
    s.resolve();
    let win = frame_rect(&s.extract(), 384.0, 512.0);
    let (rx, ry) = (win.left + 19.0 + 150.0, win.top - 75.0 - 8.0);
    let shift_click = |s: &mut UiScript| {
        s.set_modifiers(true, false, false);
        s.mouse_button(rx, ry, "LeftButton", true);
        s.mouse_button(rx, ry, "LeftButton", false);
        s.set_modifiers(false, false, false);
    };

    // Chat CLOSED → the watch toggle, exactly as before.
    shift_click(&mut s);
    assert!(
        s.eval::<bool>("return IsQuestWatched(1)").unwrap(),
        "with chat closed, shift-click still toggles the quest watch"
    );
    shift_click(&mut s); // and back off, so the assertion below is unambiguous
    assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());

    // Chat OPEN → the bare quest name, and NO watch.
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
    // The tail runs either way (ref l.503-504): the click still selects the row.
    assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 1);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
