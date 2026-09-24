//! The duel UI (DuelFrame.xml): the challenge popup's show/accept/decline, the
//! out-of-bounds warning's countdown text and its in-bounds dismissal, and the DUEL_FINISHED
//! sweep — the Lua wiring between the four engine events `ui_duel`'s feed fires and the shared
//! StaticPopup engine, driven exactly as that feed drives it.

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

/// `DUEL_REQUESTED(name)` raises the challenge popup with the challenger's name filled in, and
/// Accept queues `AcceptDuel()`.
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

/// Decline queues `CancelDuel()` — and so does ESC, because the entry is `hideOnEscape` and the
/// engine routes an escape through OnCancel.
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

/// The out-of-bounds warning is a bare countdown: no buttons, its text rendered by the
/// StaticPopup engine's per-tick branch, and `DUEL_INBOUNDS` takes it away.
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

/// `DUEL_FINISHED` sweeps **both** dialogs and sends nothing — the case that matters is a duel
/// ending while the challenge popup is still up (the other side cancelled first), which must not
/// leave a stale popup or fire a second cancel.
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

/// The four Era globals queue their intents and nothing else — `StartDuel`/`StartDuelUnit` carry
/// their argument through so the app can resolve it.
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
