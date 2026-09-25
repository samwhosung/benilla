//! The stock summon confirm, `CONFIRM_SUMMON`, raised by `ui_summon`'s feed. The event has no
//! args: the popup reads the summoner, area and time left from engine getters every tick, so these
//! tests drive the getters' snapshot.

use benilla_ui::script::{SummonConfirmUiState, UiScript, UnitState};

/// `UnitAffectingCombat` needs `exists` as well as `in_combat`.
fn player(in_combat: bool) -> UnitState {
    UnitState {
        exists: true,
        in_combat,
        ..UnitState::default()
    }
}

use super::test_ui::load_ui as load_xml;

/// A live offer with the server's full two-minute window left.
fn setup() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 120_000,
    });
    s
}

/// `StaticPopup_Show` writes " " for a countdown kind (`StaticPopup.lua:1567`) and each tick
/// composes the line; Accept's `ConfirmSummon()` becomes `CMSG_SUMMON_RESPONSE`.
#[test]
fn the_confirm_names_the_summoner_and_accept_queues_the_response() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "CONFIRM_SUMMON shows the dialog"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        " ",
        "a countdown dialog opens blank; its OnUpdate writes the line"
    );

    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Stormwind City.  The spell will be cancelled in 2 Minutes.",
        "the tick composes summoner + area + the count and its unit word"
    );

    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_summon_confirms(), 1);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// Under a minute the line counts seconds, singular at one (`StaticPopup.lua:1748`).
#[test]
fn the_countdown_line_switches_to_seconds_and_singularises() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Elwynn Forest".into(),
        time_left_ms: 45_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Elwynn Forest.  The spell will be cancelled in 45 Seconds."
    );

    // 45 - 0.1 - 44.2 leaves 0.7 s, which `ceil` shows as 1.
    s.run("StaticPopup_OnUpdate(StaticPopup1, 44.2)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Elwynn Forest.  The spell will be cancelled in 1 Second."
    );
}

/// A name still resolving paints blank; the tick re-reads the getters, so it fills in on arrival.
#[test]
fn a_summoner_whose_name_is_still_resolving_fills_in_on_a_later_tick() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: String::new(),
        area: "Stormwind City".into(),
        time_left_ms: 30_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        " wants to summon you to Stormwind City.  The spell will be cancelled in 30 Seconds.",
        "no name yet: a blank, not a raise and not a withheld dialog"
    );

    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 30_000,
    });
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Stormwind City.  The spell will be cancelled in 30 Seconds.",
        "the name query landed; the same open dialog picks it up"
    );
}

/// OnUpdate disables Accept in combat but leaves the dialog up (`StaticPopup.lua:1346`).
#[test]
fn combat_locks_accept_without_taking_the_dialog_down() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_unit("player", Some(player(false)));
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        1,
        "out of combat, Accept is live"
    );

    s.set_unit("player", Some(player(true)));
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        0,
        "in combat, the answer is locked out"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the lock does not take the question away — that is what makes it a lock"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button2:IsEnabled()")
            .unwrap(),
        1,
        "and Cancel is never touched"
    );

    s.set_unit("player", Some(player(false)));
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        1,
        "leaving combat re-arms it, with the same dialog still up"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_summon_confirms(), 1);
}

/// 1.12 has no decline opcode and no `CancelSummon`: only Accept sends.
#[test]
fn declining_and_expiring_both_send_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_summon_confirms(), 0, "Cancel is silent");

    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "hideOnEscape takes it down"
    );
    assert_eq!(s.take_summon_confirms(), 0, "ESC is silent");

    // Expiry hides the dialog, and the entry has no OnCancel.
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 2_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 5.0)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the offer expires off the screen by itself"
    );
    assert_eq!(s.take_summon_confirms(), 0, "and expiring is silent too");
}

/// With no offer the getters answer "", "" and 0, as the reference does, never nil.
#[test]
fn the_getters_answer_empties_before_anything_is_pending() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<String>("return GetSummonConfirmSummoner()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<String>("return GetSummonConfirmAreaName()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<f64>("return GetSummonConfirmTimeLeft()").unwrap(),
        0.0
    );
}

/// The binding truncates ms to whole seconds (`0x48b660`); the dialog seeds its countdown from it.
#[test]
fn the_time_left_getter_truncates_to_whole_seconds() {
    let mut s = UiScript::new().unwrap();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 1_999,
    });
    assert_eq!(
        s.eval::<f64>("return GetSummonConfirmTimeLeft()").unwrap(),
        1.0
    );
}
