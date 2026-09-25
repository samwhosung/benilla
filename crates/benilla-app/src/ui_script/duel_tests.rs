//! The stock duel popups, driven by the four events `ui_duel`'s feed fires (`UIParent.lua:378`).

use benilla_ui::script::{DuelRequest, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

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
    s
}

#[test]
fn the_challenge_popup_shows_the_name_and_accept_queues_the_accept() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event(
        "DUEL_REQUESTED",
        vec![ScriptValue::Str("Onerogue".to_string())],
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "DUEL_REQUESTED shows the challenge popup"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Onerogue has challenged you to a duel.",
        "arg1 fills the GlobalStrings template's %s"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Accept]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// Escape runs a `hideOnEscape` popup's OnCancel (`StaticPopup.lua:1884`), so it declines too.
#[test]
fn decline_and_escape_both_cancel() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Cancel]);

    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    s.run("ToggleGameMenu()").unwrap();
    assert_eq!(s.take_duel_requests(), vec![DuelRequest::Cancel]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
}

/// `DUEL_OUTOFBOUNDS` has no buttons and a 10 s timeout; `StaticPopup_OnUpdate` renders the count.
#[test]
fn out_of_bounds_counts_down_and_inbounds_dismisses_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("DUEL_OUTOFBOUNDS", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(
        !s.eval::<bool>("return StaticPopup1Button1:IsShown()")
            .unwrap(),
        "a warning, not a question — no buttons"
    );
    s.tick(0.05);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Exiting duel area, you will forfeit in 10 Seconds.",
        "the engine's countdown branch fills %d %s"
    );
    s.fire_event("DUEL_INBOUNDS", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "coming back inside clears the warning"
    );
    assert!(
        s.take_duel_requests().is_empty(),
        "the bounds pair never sends anything"
    );
}

/// `DUEL_FINISHED` hides both popups (`UIParent.lua:390`); a hide runs no OnCancel, so no decline.
#[test]
fn finishing_sweeps_both_dialogs_silently() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("DUEL_REQUESTED", vec![ScriptValue::Str("Twomage".into())]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("DUEL_FINISHED", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the challenge popup goes away with the duel"
    );
    assert!(
        s.take_duel_requests().is_empty(),
        "a hide is not a decline — the duel is already over"
    );

    s.fire_event("DUEL_OUTOFBOUNDS", vec![]);
    s.fire_event("DUEL_FINISHED", vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.take_duel_requests().is_empty());
}

/// The four 1.12 duel verbs; `StartDuel` and `StartDuelUnit` pass their argument to the app.
#[test]
fn the_era_globals_queue_their_intents() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run("AcceptDuel(); CancelDuel(); StartDuel('Onerogue'); StartDuelUnit('target')")
        .unwrap();
    assert_eq!(
        s.take_duel_requests(),
        vec![
            DuelRequest::Accept,
            DuelRequest::Cancel,
            DuelRequest::StartByName("Onerogue".into()),
            DuelRequest::StartByUnit("target".into()),
        ]
    );
}
