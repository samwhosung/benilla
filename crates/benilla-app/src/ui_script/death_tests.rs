//! The stock death popups (`StaticPopup.lua`): release, self-resurrect, resurrect offers, the
//! spirit healer's confirm and the corpse run, driven as the app's death feed drives them.

use benilla_ui::script::{DeathAction, DeathUiState, ScriptValue, UiScript};

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
fn death_popup_counts_down_and_release_queues_repop() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        release_remaining: Some(300.0),
        ..Default::default()
    });
    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "PLAYER_DEAD shows the DEATH popup"
    );
    s.tick(0.05);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "5 Minutes until release",
        "the countdown renders through the engine's DEATH per-tick text"
    );
    // DEATH has no `hideOnEscape` (`StaticPopup.lua:375`).
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the DEATH popup ignores ESC"
    );
    // No self-res owed: `DisplayButton2` is `HasSoulstone()`, nil here.
    assert!(
        !s.eval::<bool>("return StaticPopup1Button2:IsShown()")
            .unwrap(),
        "no soulstone ⇒ single-button release dialog"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::Repop]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "Release Spirit hides the dialog"
    );

    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("PLAYER_ALIVE", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "PLAYER_ALIVE (the release) hides the DEATH popup"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A `PLAYER_SELF_RES_SPELL` owed: `HasSoulstone()` returns its name, which OnShow puts on
/// button 2 (`StaticPopup.lua:383`).
#[test]
fn death_popup_offers_the_self_resurrect_and_spends_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        release_remaining: Some(300.0),
        self_res_label: Some("Use Soulstone".into()),
        ..Default::default()
    });
    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(
        s.eval::<bool>("return StaticPopup1Button2:IsShown()")
            .unwrap(),
        "a self-res owed ⇒ DisplayButton2 shows the second button"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "Use Soulstone",
        "OnShow stamps the Spell.dbc name over the registry's vestigial \"Reincarnate\""
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::Repop]);

    // Either click hides the dialog, so raise it again first.
    s.fire_event("PLAYER_DEAD", vec![]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::UseSoulstone]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the self-res hides the dialog; the resurrection itself lands as descriptor deltas"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Neither `DisplayButton2` nor OnShow re-runs per tick: a late self-res shows on the next raise.
#[test]
fn death_popup_button2_is_decided_at_show_time() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        release_remaining: Some(300.0),
        ..Default::default()
    });
    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(!s
        .eval::<bool>("return StaticPopup1Button2:IsShown()")
        .unwrap());

    s.set_death(DeathUiState {
        release_remaining: Some(300.0),
        self_res_label: Some("Reincarnation".into()),
        ..Default::default()
    });
    s.tick(0.05);
    assert!(
        !s.eval::<bool>("return StaticPopup1Button2:IsShown()")
            .unwrap(),
        "no per-tick re-evaluation — the button set is fixed at StaticPopup_Show"
    );
    // OnCancel asks `HasSoulstone()` again at click time (`StaticPopup.lua:400`).
    s.run("StaticPopup_Hide(\"DEATH\")").unwrap();
    s.fire_event("PLAYER_DEAD", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Button2:GetText()")
            .unwrap(),
        "Reincarnation"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::UseSoulstone]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A release time of -1 (an instance) shows `DEATH_RELEASE_NOTIMER` (`StaticPopup.lua:385`).
#[test]
fn death_popup_no_timer_shows_the_static_text() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        release_remaining: None,
        ..Default::default()
    });
    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.tick(0.05);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "You have died. Release to the nearest graveyard?",
        "−1 picks the no-timer text, untouched by ticks"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `RESURRECT_REQUEST` picks one of three popups by sickness, then timer (`UIParent.lua:259`).
#[test]
fn resurrect_request_picks_variant_and_answers() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        resurrect_sickness: false,
        resurrect_has_timer: true,
        recovery_delay: 0.0,
        ..Default::default()
    });
    s.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str("Healer".into())]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RESURRECT_NO_SICKNESS"
    );
    s.tick(0.05); // the zero StartDelay runs out
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Healer wants to resurrect you",
        "the offerer name formats into the no-sickness text"
    );
    assert!(s
        .eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
        .unwrap());
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::AcceptResurrect]);

    // With sickness: declining while dead re-shows DEATH (`StaticPopup.lua:428`).
    s.set_death(DeathUiState {
        resurrect_sickness: true,
        resurrect_has_timer: true,
        release_remaining: Some(200.0),
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            max_health: 100,
            health: 0,
            dead: true,
            ..Default::default()
        }),
    );
    s.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str("Healer".into())]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RESURRECT"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::DeclineResurrect]);
    // DEATH takes StaticPopup2: OnCancel shows it before the click hides StaticPopup1.
    assert_eq!(
        s.eval::<String>("return StaticPopup_Visible(\"DEATH\") or \"none\"")
            .unwrap(),
        "StaticPopup2",
        "declining while dead re-shows the DEATH popup (the ref's OnCancel)"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the answered RESURRECT dialog itself closed"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The first Accept swaps in the second-ask text and keeps the dialog (`StaticPopup.lua:1138`).
#[test]
fn xp_loss_two_step_confirm_then_range_hide() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        sickness_duration: Some("8 Minutes".into()),
        spirit_healer_in_range: true,
        ..Default::default()
    });
    s.fire_event("CONFIRM_XP_LOSS", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "XP_LOSS"
    );
    let text = s
        .eval::<String>("return StaticPopup1Text:GetText()")
        .unwrap();
    assert!(
        text.contains("afflicted by 8 Minutes of Resurrection Sickness"),
        "the sickness duration formats into CONFIRM_XP_LOSS: {text}"
    );
    // OnAccept reads `this:GetParent()`, so `this` is the button, as in the XML OnClick.
    s.run("this = StaticPopup1Button1 StaticPopup_OnClick(StaticPopup1, 1) this = nil")
        .unwrap();
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.take_death_actions().is_empty());
    let text = s
        .eval::<String>("return StaticPopup1Text:GetText()")
        .unwrap();
    assert!(
        text.starts_with("Remember, if you find your corpse"),
        "the second-ask text: {text}"
    );
    s.run("this = StaticPopup1Button1 StaticPopup_OnClick(StaticPopup1, 1) this = nil")
        .unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::AcceptXpLoss]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    s.set_death(DeathUiState {
        sickness_duration: None,
        spirit_healer_in_range: true,
        ..Default::default()
    });
    s.fire_event("CONFIRM_XP_LOSS", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "XP_LOSS_NO_SICKNESS"
    );
    s.set_death(DeathUiState {
        sickness_duration: None,
        spirit_healer_in_range: false,
        ..Default::default()
    });
    s.tick(0.05);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "leaving the spirit healer's range auto-hides the confirm"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The feed fires `CONFIRM_XP_LOSS` per server message, so a cancelled confirm returns on the next
/// ask. `showAlert` widens to 420 with the icon; every Show resets both (`StaticPopup.lua:1580`).
#[test]
fn xp_loss_cancel_then_reconfirm_reshows_with_the_alert_dress() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        sickness_duration: Some("8 Minutes".into()),
        spirit_healer_in_range: true,
        ..Default::default()
    });
    s.fire_event("CONFIRM_XP_LOSS", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<f64>("return StaticPopup1:GetWidth()").unwrap(),
        420.0,
        "showAlert widens the dialog to the ref's 420"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1AlertIcon:IsShown()")
            .unwrap(),
        "showAlert shows the DialogAlertIcon"
    );
    s.run("this = StaticPopup1Button2 StaticPopup_OnClick(StaticPopup1, 2) this = nil")
        .unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.take_death_actions().is_empty());
    s.fire_event("CONFIRM_XP_LOSS", vec![]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "a fresh confirm re-shows after a Cancel (no deadlock)"
    );
    s.run("StaticPopup_Hide(\"XP_LOSS\")").unwrap();
    s.run(concat!(
        "StaticPopupDialogs[\"TEST_PLAIN\"] = { text = \"x\", button1 = \"OK\", timeout = 0, ",
        "whileDead = 1 } StaticPopup_Show(\"TEST_PLAIN\")"
    ))
    .unwrap();
    assert_eq!(
        s.eval::<f64>("return StaticPopup1:GetWidth()").unwrap(),
        320.0,
        "a non-alert Show resets the width"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1AlertIcon:IsShown()")
            .unwrap(),
        "a non-alert Show hides the icon"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A ghost has 1 health, so `UnitIsDead` is nil for it.
#[test]
fn the_ghost_predicates() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            max_health: 100,
            health: 1,
            dead: false,
            ghost: true,
            ..Default::default()
        }),
    );
    // The three answer 1 or nil, never a boolean: `== nil` fails on a `false`.
    assert!(s
        .eval::<bool>("return UnitIsDead(\"player\") == nil")
        .unwrap());
    assert_eq!(s.eval::<i64>("return UnitIsGhost(\"player\")").unwrap(), 1);
    assert_eq!(
        s.eval::<i64>("return UnitIsDeadOrGhost(\"player\")")
            .unwrap(),
        1
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `StartDelay` gates Accept, and Accept keeps the dialog up (`StaticPopup.lua:1182`).
#[test]
fn corpse_range_events_drive_recover_corpse() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_death(DeathUiState {
        recovery_delay: 2.0,
        ..Default::default()
    });
    s.fire_event("CORPSE_IN_RANGE", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RECOVER_CORPSE"
    );
    assert!(
        !s.eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
            .unwrap(),
        "Accept is delay-gated"
    );
    s.tick(0.5);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "2 Seconds until resurrection"
    );
    s.tick(1.6);
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Resurrect now?"
    );
    assert!(s
        .eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
        .unwrap());
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::RetrieveCorpse]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "RetrieveCorpse returns 1 — the dialog stays until the server answers"
    );
    s.fire_event("CORPSE_OUT_OF_RANGE", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "leaving range hides the recover dialog"
    );

    // RECOVER_CORPSE_INSTANCE has no buttons (`StaticPopup.lua:1196`).
    s.fire_event("CORPSE_IN_INSTANCE", vec![]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RECOVER_CORPSE_INSTANCE"
    );
    assert!(!s
        .eval::<bool>("return StaticPopup1Button1:IsShown()")
        .unwrap());
    s.fire_event("CORPSE_OUT_OF_RANGE", vec![]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// No corpse reads (0, 0), which hides the map's corpse marker (`WorldMapFrame.lua:445`).
#[test]
fn corpse_map_position_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    let (x, y) = s
        .eval::<(f64, f64)>("return GetCorpseMapPosition()")
        .unwrap();
    assert_eq!((x, y), (0.0, 0.0), "no corpse ⇒ the (0,0) hide sentinel");
    s.set_world_map_feed(None, None, 0.0, Some((0.25, 0.75)), Vec::new(), Vec::new());
    let (x, y) = s
        .eval::<(f64, f64)>("return GetCorpseMapPosition()")
        .unwrap();
    assert!((x - 0.25).abs() < 1e-6 && (y - 0.75).abs() < 1e-6);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
