//! The death-arc UI (DeathFrame.xml): the DEATH release popup's
//! show/countdown/release flow, its self-resurrect button, the resurrect-offer popup pick, and
//! the spirit-healer XP_LOSS two-step — the Lua wiring between the engine's death events and the
//! StaticPopup engine, driven exactly as `death.rs`'s feed does it (set_death → fire_event).

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

/// PLAYER_DEAD with a running release timer: the DEATH popup shows, its OnShow seeds the countdown
/// from GetReleaseTimeRemaining, the engine's per-tick DEATH text renders, Release Spirit queues
/// the Repop intent, and PLAYER_ALIVE (the release landing) hides it.
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
    // ESC must NOT close it (no hideOnEscape on DEATH — the ref's law).
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the DEATH popup ignores ESC"
    );
    // Button2 (soulstone) is hidden — HasSoulstone() is nil.
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

    // Re-shown (still dead), then the release lands: PLAYER_ALIVE hides it.
    s.fire_event("PLAYER_DEAD", vec![]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    s.fire_event("PLAYER_ALIVE", vec![]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "PLAYER_ALIVE (the release) hides the DEATH popup"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The self-resurrect leg: with `PLAYER_SELF_RES_SPELL` naming a spell, the DEATH
/// popup grows its second button, the button wears the SPELL's name rather than the registry's
/// "Reincarnate" default, and clicking it queues `UseSoulstone` — while button1 still releases.
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
    // Button1 is still the release, and it is NOT the self-res.
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::Repop]);

    // Button2 spends it. The dialog is raised again first, because clicking either button hides it.
    s.fire_event("PLAYER_DEAD", vec![]);
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::UseSoulstone]);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the self-res hides the dialog; the resurrection itself lands as descriptor deltas"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A shaman's Reincarnation is the same path with a different name — and the button is decided by
/// `HasSoulstone()` at SHOW time, so a self-res that arrives after the popup is up does not
/// retro-fit a button onto it (the reference's own StaticPopup engine re-evaluates neither
/// `DisplayButton2` nor `OnShow` per tick).
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

    // The field lands late: the standing dialog does not grow a button…
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
    // …but the OnCancel arm re-asks, so a click on a button that is not there cannot mis-route,
    // and the next raise picks it up.
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

/// The no-timer variant: GetReleaseTimeRemaining() == −1 (an instanceable map) swaps in the
/// DEATH_RELEASE_NOTIMER text and never counts down.
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

/// RESURRECT_REQUEST picks its popup by the offer bits (the ref's three-way), formats the offerer
/// name in, gates Accept behind the recovery delay, and Accept/Decline queue their intents.
#[test]
fn resurrect_request_picks_variant_and_answers() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    // No sickness + timer, delay already elapsed: RESURRECT_NO_SICKNESS with an armed Accept.
    s.set_death(DeathUiState {
        resurrect_sickness: false,
        resurrect_has_timer: true,
        recovery_delay: 0.0,
        ..Default::default()
    });
    s.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str("Pone".into())]);
    assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RESURRECT_NO_SICKNESS"
    );
    s.tick(0.05); // the zero StartDelay expires: real text + enabled button
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Pone wants to resurrect you",
        "the offerer name formats into the no-sickness text"
    );
    assert!(s
        .eval::<bool>("return StaticPopup1Button1:IsEnabled() ~= 0")
        .unwrap());
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::AcceptResurrect]);

    // Sickness variant declines: the intent queues and (still dead) DEATH re-shows.
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
    s.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str("Pone".into())]);
    assert_eq!(
        s.eval::<String>("return StaticPopup1.which").unwrap(),
        "RESURRECT"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::DeclineResurrect]);
    // The re-shown DEATH lands on the free SECOND instance (Show runs inside the decline's
    // OnCancel, before the click's own hide frees instance 1) — the exact case that sized
    // STATICPOPUP_NUMDIALOGS at 2.
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

/// The spirit healer's CONFIRM_XP_LOSS two-step: first Accept rewrites to the "Are you sure" text
/// and keeps the dialog, the second queues AcceptXPLoss; walking out of range auto-hides.
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
    // First Accept: the AGAIN text swaps in, the dialog stays, nothing queues. XP_LOSS's verbatim
    // OnAccept reads `this:GetParent()` (the ref's button-handler context), so the click is driven
    // as the XML OnClick does — with `this` bound to the button.
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
    // Second Accept: the activate intent queues and the dialog closes.
    s.run("this = StaticPopup1Button1 StaticPopup_OnClick(StaticPopup1, 1) this = nil")
        .unwrap();
    assert_eq!(s.take_death_actions(), vec![DeathAction::AcceptXpLoss]);
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());

    // Below the sickness level: the NO_SICKNESS variant. Out of range: the OnUpdate poll hides.
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

/// A cancelled confirm comes back on the next ask: every CONFIRM_XP_LOSS
/// fire re-shows the dialog — the feed fires per SMSG, and this guards the Lua half of that
/// contract. And the XP_LOSS dialogs carry the ref's alert dress (StaticPopup.lua l.1579-1589):
/// width 420 with the AlertIcon shown, reset to 320/hidden for a non-alert entry on the same
/// instance.
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
    // Cancel: nothing queues, nothing on the wire — the dialog just hides.
    s.run("this = StaticPopup1Button2 StaticPopup_OnClick(StaticPopup1, 2) this = nil")
        .unwrap();
    assert!(!s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
    assert!(s.take_death_actions().is_empty());
    // Asking the healer again = a fresh SMSG = the feed fires again: the confirm is back.
    s.fire_event("CONFIRM_XP_LOSS", vec![]);
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "a fresh confirm re-shows after a Cancel (B80's deadlock)"
    );
    s.run("StaticPopup_Hide(\"XP_LOSS\")").unwrap();
    // A non-alert entry on the same instance resets the dress — Show's unconditional reset.
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

/// UnitIsGhost/UnitIsDeadOrGhost — the trio's ghost legs (a ghost has health 1, so UnitIsDead is
/// false for it).
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
    // The trio's shape is 1/nil, never a boolean, so these read the value rather
    // than its truthiness — `== nil` is the comparison a boolean would invert.
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

/// The corpse-run range events: CORPSE_IN_RANGE shows RECOVER_CORPSE with its
/// StartDelay countdown gating Accept, Accept queues the reclaim intent (and keeps the dialog —
/// the server's descriptor deltas close the loop), CORPSE_OUT_OF_RANGE hides it; the instance
/// variant is the buttonless notice.
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

    // The dungeon-corpse notice: buttonless (the ref entry has no button1), whileDead.
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

/// GetCorpseMapPosition: the feed's corpse UV surfaces (0,0) = hidden when absent — the reference
/// WorldMapFrame.lua:443-452 law the map's update block branches on.
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
