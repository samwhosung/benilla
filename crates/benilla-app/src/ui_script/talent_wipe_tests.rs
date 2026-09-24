//! The class trainer's respec confirm (TalentWipeConfirm.xml): the dialog
//! `ui_talent_wipe`'s feed raises, the money frame that carries the cost, its Accept, and the range
//! poll that takes it away — driven exactly as that feed and the app's NPC-session guard drive it.

use benilla_ui::script::{ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// The app's own pre-state: a question is pending and in range, which is what
/// `CheckTalentMasterDist()` reports while the dialog is up. MoneyFrame.xml loads FIRST because
/// the dialog's coin row inherits `SmallMoneyFrameTemplate` — the TOC's own order (1580).
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

/// `CONFIRM_TALENT_WIPE(cost)` raises the dialog, and Accept queues the one `ConfirmTalentWipe()`
/// that becomes the outbound `MSG_TALENT_WIPE_CONFIRM`. The whole point of the arc: before this
/// wiring the trainer's line produced no dialog and no packet at all — the packet that asks was
/// parsed as an unknown opcode and dropped.
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

/// The cost rides the event's `arg1` into the dialog's MONEY frame, not into its text — so the
/// coin row is what tells the player a respec costs 1g 50s. This is the first StaticPopup entry to
/// raise one, and the engine's `hasMoneyFrame` leg is what shows it.
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

    // A second, dearer question repaints the same row — the cost climbs with every reset, so a
    // stale number here would be the one thing the dialog exists to say, said wrong.
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    s.fire_event("CONFIRM_TALENT_WIPE", vec![ScriptValue::Int(50_000)]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1MoneyFrameGoldButton:GetText()")
            .unwrap(),
        "5"
    );
}

/// Cancel and ESC both send **nothing**: declining a respec is silent on the wire (there is no
/// decline opcode — the question is one direction of a two-way opcode and the answer is the
/// other), so the only observable is that no confirm was queued.
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

/// Walking away takes the question off screen: the entry's OnUpdate polls
/// `CheckTalentMasterDist()`, which the app drives from the shared NPC-session range guard — the
/// same guard, and the same byte-verified distance, that the innkeeper's question runs on. While it
/// holds, ticking changes nothing; the frame it goes false, the dialog hides itself.
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
