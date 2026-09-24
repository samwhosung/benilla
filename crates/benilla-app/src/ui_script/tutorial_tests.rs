//! The stock `TutorialFrame.xml` driven engine-only: an alert button per
//! `TUTORIAL_TRIGGER`, the window a click opens over the published id's strings, the
//! `FlagTutorial` it makes, the unticked checkbox's `ClearTutorials` on hide, and
//! `CINEMATIC_STOP` clicking the Welcome alert.

use benilla_ui::script::{ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

fn session() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function PlaySound() end").unwrap();
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Localization.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\TutorialFrame.xml",
    ] {
        load_xml(&s, f);
    }
    s.resolve();
    s
}

fn visible(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsVisible()"))
        .unwrap()
}

fn trigger(s: &mut UiScript, published: i64) {
    s.fire_event("TUTORIAL_TRIGGER", vec![ScriptValue::Int(published)]);
}

/// A trigger queues an alert button; its click opens the window on the id's title and text and
/// acknowledges the id (0-based on the drain); a second trigger queues a second button.
#[test]
fn a_trigger_queues_an_alert_whose_click_opens_the_window_and_flags_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    assert!(!visible(&s, "TutorialFrameAlertButton1"));
    assert!(!visible(&s, "TutorialFrame"));

    trigger(&mut s, 41);
    assert!(
        visible(&s, "TutorialFrameAlertButton1"),
        "the alert button shows"
    );
    assert_eq!(
        s.eval::<String>("return TutorialFrameAlertButton1.tooltip")
            .unwrap(),
        "Elite Quests"
    );
    trigger(&mut s, 42);
    assert!(
        visible(&s, "TutorialFrameAlertButton2"),
        "a second id takes the next button"
    );

    s.run("TutorialFrameAlertButton1:Click()").unwrap();
    assert!(visible(&s, "TutorialFrame"), "the click opens the window");
    assert_eq!(
        s.eval::<String>("return TutorialFrameTitle:GetText()")
            .unwrap(),
        "Elite Quests"
    );
    assert!(s
        .eval::<String>("return TutorialFrameText:GetText()")
        .unwrap()
        .starts_with("You have accepted an elite quest"));
    assert_eq!(
        s.take_tutorial_flag_requests(),
        vec![40],
        "FlagTutorial(41) → the 0-based id"
    );
    assert!(
        !visible(&s, "TutorialFrameAlertButton1"),
        "the clicked alert is spent; the other stays"
    );
    assert!(visible(&s, "TutorialFrameAlertButton2"));

    // Okay hides it; the checkbox is ticked, so nothing is cleared.
    s.run("TutorialFrameOkayButton:Click()").unwrap();
    assert!(!visible(&s, "TutorialFrame"));
    assert_eq!(s.take_tutorial_clears(), 0);
}

/// Unticking "Display Tips" and closing calls `ClearTutorials` and drops every alert.
#[test]
fn an_unticked_checkbox_clears_the_tutorials_on_hide() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    trigger(&mut s, 41);
    trigger(&mut s, 42);
    s.run("TutorialFrameAlertButton1:Click()").unwrap();
    let _ = s.take_tutorial_flag_requests();
    s.run("TutorialFrameCheckButton:SetChecked(nil) TutorialFrame:Hide()")
        .unwrap();
    assert_eq!(s.take_tutorial_clears(), 1, "ClearTutorials() on hide");
    assert!(
        !visible(&s, "TutorialFrameAlertButton2"),
        "every alert dropped"
    );
}

/// `CINEMATIC_STOP` clicks the Welcome alert (id 42) if it is queued — the first thing a new
/// character sees after the intro — and moves the window to the screen's centre.
#[test]
fn cinematic_stop_opens_the_welcome_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    trigger(&mut s, 42);
    trigger(&mut s, 1);
    s.fire_event("CINEMATIC_STOP", vec![]);
    assert!(visible(&s, "TutorialFrame"));
    assert_eq!(
        s.eval::<String>("return TutorialFrameTitle:GetText()")
            .unwrap(),
        "Welcome to World of Warcraft!"
    );
    assert_eq!(s.take_tutorial_flag_requests(), vec![41]);
    assert!(
        visible(&s, "TutorialFrameAlertButton2"),
        "the Questgivers alert stays queued"
    );
}
