//! The stock respec confirm, `CONFIRM_TALENT_WIPE`, driven as `ui_talent_wipe`'s feed and the
//! app's NPC range guard drive it.

use benilla_ui::script::{ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// A respec question pending and in range, so `CheckTalentMasterDist()` holds. MoneyFrame.xml
/// loads before StaticPopup.xml, whose coin row inherits `SmallMoneyFrameTemplate`.
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
    s.set_talent_master_pending(true);
    s
}

/// Accept's one `ConfirmTalentWipe()` becomes the outbound `MSG_TALENT_WIPE_CONFIRM`.
#[test]
fn the_confirm_shows_and_accept_queues_the_wipe() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(15_000)]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "CONFIRM_TALENT_WIPE shows the dialog"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to unlearn all of your talents?  The cost will increase each time you do it.",
        "the GlobalStrings sentence, which names no number of its own"
    );

    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_talent_wipe_confirms(), 1);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// The cost, `arg1`, goes to the dialog's money frame, not its text (`UIParent.lua:536`).
#[test]
fn the_cost_lands_in_the_money_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    assert!(
        !s.eval::<bool>("return StaticPopup1MoneyFrame:IsVisible()")
            .unwrap(),
        "the coin row is hidden until an entry asks for it"
    );

    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(15_000)]);
    assert!(
        s.eval::<bool>("return StaticPopup1MoneyFrame:IsVisible()")
            .unwrap(),
        "a hasMoneyFrame entry shows the coin row"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1MoneyFrameGoldButton:GetText()")
            .unwrap(),
        "1",
        "15000 copper is 1 gold"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1MoneyFrameSilverButton:GetText()")
            .unwrap(),
        "50",
        "…50 silver"
    );

    // The cost climbs with every reset; a second question repaints the row.
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(50_000)]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1MoneyFrameGoldButton:GetText()")
            .unwrap(),
        "5"
    );
}

/// There is no decline: `MSG_TALENT_WIPE_CONFIRM` asks one way and answers the other.
#[test]
fn declining_sends_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(15_000)]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_talent_wipe_confirms(), 0);

    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(15_000)]);
    s.run("ToggleGameMenu()").unwrap();
    assert_eq!(s.take_talent_wipe_confirms(), 0);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "hideOnEscape takes it down"
    );
}

/// The popup's OnUpdate hides it once `CheckTalentMasterDist()` fails (`StaticPopup.lua:1297`).
#[test]
fn leaving_the_trainers_range_hides_the_confirm() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(15_000)]);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "in range, the tick leaves it alone"
    );

    s.set_talent_master_pending(false);
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "out of range, the dialog takes itself down"
    );
    assert_eq!(s.take_talent_wipe_confirms(), 0, "and sends nothing");
}
