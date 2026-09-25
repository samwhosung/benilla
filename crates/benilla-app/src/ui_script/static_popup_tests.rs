//! The stock StaticPopup engine (`StaticPopup.lua`): timeout, StartDelay, cancels, Escape, the
//! countdown text and addressing an instance by data.

use benilla_ui::script::UiScript;

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

/// Expiry runs OnCancel with "timeout", then hides the dialog (`StaticPopup.lua:1716`).
#[test]
fn timeout_expires_into_a_timeout_cancel() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run(
        r#"reason = "unset"
           StaticPopupDialogs["TEST_TIMEOUT"] = {
               text = "expiring", button1 = "OK", timeout = 2,
               OnCancel = function(data, r) reason = r or "none" end,
           }
           StaticPopup_Show("TEST_TIMEOUT")"#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.tick(1.0);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "still counting"
    );
    s.tick(1.5);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "expiry hides the dialog"
    );
    assert_eq!(
        s.eval::<String>("return reason").unwrap(),
        "timeout",
        "expiry runs OnCancel with reason 'timeout'"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// StartDelay holds button1 disabled, then swaps the real text in (`StaticPopup.lua:1764`); only
/// the kinds named at `StaticPopup.lua:1778`, RECOVER_CORPSE among them, render its countdown.
#[test]
fn start_delay_gates_button1_then_swaps_the_text_in() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run(
        r#"StaticPopupDialogs["RECOVER_CORPSE"] = {
               StartDelay = function() return 2 end,
               delayText = "%d %s until resurrection",
               text = "Resurrect now?",
               button1 = "Accept", timeout = 0, whileDead = 1,
           }
           StaticPopup_Show("RECOVER_CORPSE")"#,
    )
    .unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(
        !s.eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
            .unwrap(),
        "button1 starts disabled under StartDelay"
    );
    s.tick(0.5);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "2 Seconds until resurrection",
        "the delayText countdown renders (ceil of 1.5s)"
    );
    s.tick(1.6);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Resurrect now?",
        "delay expiry swaps the real text in"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
            .unwrap(),
        "delay expiry enables button1"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Showing a dialog cancels a visible `cancels` kind with "override" (`StaticPopup.lua:1475`).
#[test]
fn showing_a_dialog_cancels_its_named_victim_with_override() {
    benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run(
        r#"victim_reason = "unset"
           StaticPopupDialogs["TEST_VICTIM"] = {
               text = "victim", button1 = "OK", timeout = 0,
               OnCancel = function(data, r) victim_reason = r or "none" end,
           }
           StaticPopupDialogs["TEST_CANCELLER"] = {
               text = "canceller", button1 = "OK", timeout = 0,
               cancels = "TEST_VICTIM",
           }
           StaticPopup_Show("TEST_VICTIM")
           StaticPopup_Show("TEST_CANCELLER")"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return victim_reason").unwrap(),
        "override",
        "the victim's OnCancel ran with reason 'override'"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "TEST_CANCELLER",
        "the canceller owns the (single) dialog instance now"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Escape closes only `hideOnEscape` dialogs (`StaticPopup.lua:1879`).
#[test]
fn escape_skips_dialogs_without_hide_on_escape() {
    benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run(
        r#"StaticPopupDialogs["TEST_STICKY"] = {
               text = "cannot escape me", button1 = "OK", timeout = 0, whileDead = 1,
           }
           StaticPopup_Show("TEST_STICKY")"#,
    )
    .unwrap();
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "a non-hideOnEscape dialog ignores ESC (the DEATH popup law)"
    );
    s.run(
        r#"StaticPopup_Hide("TEST_STICKY")
           StaticPopupDialogs["TEST_STICKY"].hideOnEscape = 1
           StaticPopup_Show("TEST_STICKY")"#,
    )
    .unwrap();
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the escapable variant closes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// DEATH's text re-renders every tick, in minutes from 60 s up (`StaticPopup.lua:1727`).
#[test]
fn the_death_countdown_text_rerenders_each_tick() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.run(
        r#"StaticPopupDialogs["DEATH"] = {
               text = "%d %s until release",
               button1 = "Release Spirit",
               OnShow = function()
                   this.timeleft = 90
               end,
               timeout = 0, whileDead = 1,
           }
           StaticPopup_Show("DEATH")"#,
    )
    .unwrap();
    s.tick(0.1);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "2 Minutes until release",
        "above 60s renders ceil-minutes"
    );
    s.tick(31.0);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "59 Seconds until release",
        "below 60s renders seconds"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `StaticPopup_FindVisible` matches data only for a `multiple` kind (`StaticPopup.lua:1421`);
/// `StaticPopup_Hide` given data hides only the instance carrying it (`StaticPopup.lua:1706`).
#[test]
fn hide_and_find_address_one_instance_by_data_only_for_a_multiple_dialog() {
    benilla_formats::wow_data_or_skip!();
    let s = setup();
    s.run(
        r#"
        StaticPopupDialogs["T_MULTI"] = { text = "multi", button1 = "Ok", multiple = 1, timeout = 0 }
        StaticPopupDialogs["T_ONE"]   = { text = "one",   button1 = "Ok", timeout = 0 }
        a = StaticPopup_Show("T_MULTI") a.data = "alpha"
        b = StaticPopup_Show("T_MULTI") b.data = "beta"
        c = StaticPopup_Show("T_ONE")   c.data = "gamma"
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());

    // Both ways round, so finding the first instance alone cannot pass.
    for (data, want) in [("alpha", "a"), ("beta", "b")] {
        assert!(
            s.eval::<bool>(&format!(
                "return StaticPopup_FindVisible(\"T_MULTI\", {data:?}) == {want}"
            ))
            .unwrap(),
            "FindVisible picks the instance carrying {data:?}"
        );
    }
    assert!(s
        .eval::<bool>(r#"return StaticPopup_FindVisible("T_MULTI", "nobody") == nil"#)
        .unwrap());

    s.run(r#"StaticPopup_Hide("T_MULTI", "alpha")"#).unwrap();
    assert!(s
        .eval::<bool>("return not a:IsShown() and b:IsShown()")
        .unwrap());

    // No data hides every instance of the kind.
    s.run(r#"StaticPopup_Hide("T_MULTI")"#).unwrap();
    assert!(s.eval::<bool>("return not b:IsShown()").unwrap());

    assert!(s
        .eval::<bool>(r#"return StaticPopup_FindVisible("T_ONE", "wrong") == c"#)
        .unwrap());

    // An unknown kind answers nil (`StaticPopup.lua:1416`).
    assert!(s
        .eval::<bool>(r#"return StaticPopup_FindVisible("T_NOPE") == nil"#)
        .unwrap());
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Five stock `UIParent.lua` event arms raise their dialogs, which call engine verbs.
#[test]
fn the_verb_dialogs_open_from_their_events_and_call_their_verbs() {
    benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::ScriptValue;
    let mut s = setup();
    load_xml(&s, r"Interface\FrameXML\UIParent.xml"); // the arms
    s.set_money(50_000);
    s.fire_event("CONFIRM_PET_UNLEARN", vec![ScriptValue::Int(12_345)]);
    s.tick(0.0);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "CONFIRM_PET_UNLEARN"
    );
    assert_eq!(
        s.eval::<f64>("return StaticPopup1MoneyFrame.staticMoney")
            .unwrap(),
        12_345.0
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(s.take_pet_unlearn_confirms(), 1);
    s.set_instance_boot_secs(30);
    s.fire_event("INSTANCE_BOOT_START", vec![]);
    s.tick(0.0);
    assert!(s
        .eval::<bool>("return StaticPopup_Visible(\"INSTANCE_BOOT\") ~= nil")
        .unwrap());
    s.fire_event("INSTANCE_BOOT_STOP", vec![]);
    s.tick(0.0);
    assert!(!s
        .eval::<bool>("return StaticPopup_Visible(\"INSTANCE_BOOT\") ~= nil")
        .unwrap());
    s.set_area_spirit_healer(true, 20);
    s.fire_event("AREA_SPIRIT_HEALER_IN_RANGE", vec![]);
    s.tick(0.0);
    assert!(s
        .eval::<bool>("return StaticPopup_Visible(\"AREA_SPIRIT_HEAL\") ~= nil")
        .unwrap());
    // The shipped dialog accepts on show and its one button cancels (`StaticPopup.lua:1224`).
    assert!(
        s.take_area_spirit_accepts() >= 1,
        "showing the dialog IS the accept"
    );
    s.run("StaticPopup_OnClick(StaticPopup_FindVisible(\"AREA_SPIRIT_HEAL\"), 1)")
        .unwrap();
    assert_eq!(
        s.take_area_spirit_accepts(),
        0,
        "the button cancels; it does not accept again"
    );
    s.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
    s.tick(0.0);
    assert!(!s
        .eval::<bool>("return StaticPopup_Visible(\"AREA_SPIRIT_HEAL\") ~= nil")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
