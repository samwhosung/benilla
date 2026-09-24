//! The two enchant-apply confirms (EnchantConfirm.xml): the Lua wiring between the
//! events `spell::targeting`'s item-bind gate fires and the shared StaticPopup engine, driven
//! exactly as that gate drives it.
//!
//! What these pin is the seam, not the gate: that `BIND_ENCHANT` and `REPLACE_ENCHANT` reach a
//! popup at all, that the replace text takes its two enchant names in the reference's order (old
//! then new), and that each Yes lands on the right one of the two Lua globals — because
//! `BindEnchant()` and `ReplaceEnchant()` mean opposite things on the app side (re-run the gate vs
//! bind outright), and swapping them would silently skip a question.

use benilla_ui::script::{EnchantConfirm, ScriptValue, UiScript};

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

/// `BIND_ENCHANT` (event 402, no args) raises the bind warning, and its Okay calls `BindEnchant()`
/// — which on the app side re-enters `0x495d60` with the confirmed flag, not a send.
#[test]
fn the_bind_confirm_shows_and_its_okay_queues_bind_enchant() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("BIND_ENCHANT", vec![]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "BIND_ENCHANT shows the bind warning"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Enchanting this item will bind it to you."
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_enchant_confirms(), vec![EnchantConfirm::Bind]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
}

/// `REPLACE_ENCHANT(old, new)` fills the GlobalStrings template in the reference's argument order
/// — the fire site pushes the enchant already on the item first, then the one about to land.
#[test]
fn the_replace_confirm_names_the_old_enchant_first() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event(
        "REPLACE_ENCHANT",
        vec![
            ScriptValue::Str("Crusader".into()),
            ScriptValue::Str("Agility +15".into()),
        ],
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Do you want to replace \"Crusader\" with \"Agility +15\"?"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_enchant_confirms(), vec![EnchantConfirm::Replace]);
}

/// No is silent, and so is ESC — declining sends nothing and tears nothing down, because the gate
/// returned before `BindTarget` and the targeting word is still standing.
#[test]
fn declining_either_confirm_queues_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("BIND_ENCHANT", vec![]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_enchant_confirms(), vec![]);

    s.fire_event(
        "REPLACE_ENCHANT",
        vec![ScriptValue::Str("a".into()), ScriptValue::Str("b".into())],
    );
    s.run("ToggleGameMenu()").unwrap();
    assert_eq!(s.take_enchant_confirms(), vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
}

/// `CURRENT_SPELL_CAST_CHANGED` takes both away — the reference's only teardown for them, and the
/// one that matters: the pending cast the popup is asking about is what makes the question mean
/// anything, so a cancelled or replaced cast must not leave a live Yes button behind.
#[test]
fn a_changed_pending_cast_dismisses_both() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("BIND_ENCHANT", vec![]);
    s.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(s.take_enchant_confirms(), vec![]);

    s.fire_event(
        "REPLACE_ENCHANT",
        vec![ScriptValue::Str("a".into()), ScriptValue::Str("b".into())],
    );
    s.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(s.take_enchant_confirms(), vec![]);
}
