//! The innkeeper bind confirm, the stock `CONFIRM_BINDER` popup, driven as `ui_binder`'s feed
//! and the app's NPC range guard drive it.

use benilla_ui::script::{ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// A bind question pending and in range, so `CheckBinderDist()` holds.
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
    s.set_binder_pending(true);
    s
}

/// Accept's one `ConfirmBinder()` becomes `CMSG_BINDER_ACTIVATE`.
#[test]
fn the_confirm_shows_the_area_and_accept_queues_the_bind() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event(
        "CONFIRM_BINDER",
        vec![ScriptValue::Str("Dolanaar".to_string())],
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "CONFIRM_BINDER shows the dialog"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to make Dolanaar your new home?",
        "arg1 fills the GlobalStrings template's %s"
    );

    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_binder_confirms(), 1);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// There is no decline opcode: Cancel and Escape send nothing.
#[test]
fn declining_sends_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str("Goldshire".into())]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_binder_confirms(), 0);

    s.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str("Goldshire".into())]);
    s.run("ToggleGameMenu()").unwrap();
    assert_eq!(s.take_binder_confirms(), 0);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "hideOnEscape takes it down"
    );
}

/// The popup's OnUpdate hides it once `CheckBinderDist()` fails (`StaticPopup.lua:1329`).
#[test]
fn leaving_the_innkeepers_range_hides_the_confirm() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str("Kharanos".into())]);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "in range, the tick leaves it alone"
    );

    s.set_binder_pending(false);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "out of range, the dialog takes itself down"
    );
    assert_eq!(s.take_binder_confirms(), 0, "and sends nothing");
}
