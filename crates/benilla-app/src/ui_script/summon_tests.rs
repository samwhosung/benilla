//! The summon confirm (ConfirmSummon.xml): the dialog `ui_summon`'s feed raises,
//! the countdown line the popup engine composes from the four engine globals, the combat lock on
//! its Accept, and the one call that becomes `CMSG_SUMMON_RESPONSE`.
//!
//! This dialog is the only one in the folder whose event carries **no arguments** — everything on
//! screen is read back out of the engine every tick — so these tests drive the getters' snapshot
//! (`set_summon_confirm`) rather than an event payload, which is exactly how the app drives it.

use benilla_ui::script::{SummonConfirmUiState, UiScript, UnitState};

/// A `"player"` snapshot that exists and is (or is not) fighting — the one field this dialog's
/// OnUpdate reads. `UnitAffectingCombat` answers on `exists && in_combat`, so the flag alone would
/// not be enough.
fn player(in_combat: bool) -> UnitState {
    UnitState {
        exists: true,
        in_combat,
        ..UnitState::default()
    }
}

use super::test_ui::load_ui as load_xml;

/// The app's own pre-state: a live offer from Twomage, out of Stormwind City, with the server's
/// full two-minute window still on the clock.
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
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 120_000,
    });
    s
}

/// The whole arc: `CONFIRM_SUMMON` (no args) raises the dialog, the popup engine's countdown tick
/// composes the line from the three getters, and Accept queues the one `ConfirmSummon()` that
/// becomes `CMSG_SUMMON_RESPONSE`.
///
/// The **text starts blank** and is filled by the tick — that is the engine's countdown contract
/// (`StaticPopup_Show` writes `" "` for this `which`), and the reason this dialog needs the
/// per-tick branch at all rather than `StaticPopup_Show`'s arguments.
#[test]
fn the_confirm_names_the_summoner_and_accept_queues_the_response() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "CONFIRM_SUMMON shows the dialog"
    );
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        " ",
        "a countdown dialog opens blank; its OnUpdate writes the line"
    );

    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Stormwind City.  The spell will be cancelled in 2 Minutes.",
        "the tick composes summoner + area + the count and its unit word"
    );

    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_summon_confirms(), 1);
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "accepting closes it"
    );
}

/// Under a minute the line counts in **seconds**, and the unit word singularises at one — the
/// engine's shared `StaticPopupTimeUnit`, reached through this dialog's own four-argument format.
#[test]
fn the_countdown_line_switches_to_seconds_and_singularises() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Elwynn Forest".into(),
        time_left_ms: 45_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Elwynn Forest.  The spell will be cancelled in 45 Seconds."
    );

    // OnShow seeded 45 and the first tick spent 0.1 of it, so 44.9 stands; spending 44.2 more
    // lands on 0.7 — inside the last second, which `ceil` reports as 1 and the unit word must
    // therefore singularise.
    s.run("StaticPopup_OnUpdate(StaticPopup1, 44.2)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Elwynn Forest.  The spell will be cancelled in 1 Second."
    );
}

/// **A name still in flight paints blank and then fills itself in.** The reference does not hold
/// the event back for the name (the event has no arguments to hold), so the first frames render
/// `""` and the getter's next answer lands on the very next tick — which is only true because the
/// countdown branch re-reads the getters rather than caching `StaticPopup_Show`'s arguments.
#[test]
fn a_summoner_whose_name_is_still_resolving_fills_in_on_a_later_tick() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: String::new(),
        area: "Stormwind City".into(),
        time_left_ms: 30_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        " wants to summon you to Stormwind City.  The spell will be cancelled in 30 Seconds.",
        "no name yet: a blank, not a raise and not a withheld dialog"
    );

    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 30_000,
    });
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<String>("return StaticPopup1Text:GetText()")
            .unwrap(),
        "Twomage wants to summon you to Stormwind City.  The spell will be cancelled in 30 Seconds.",
        "the name query landed; the same open dialog picks it up"
    );
}

/// Accept is **disabled in combat and re-enabled out of it**, with the dialog staying up — this
/// entry's OnUpdate is a lock, not a teardown, which is what makes it different from every other
/// confirm in the folder. Cancel is never touched.
#[test]
fn combat_locks_accept_without_taking_the_dialog_down() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.set_unit("player", Some(player(false)));
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        1,
        "out of combat, Accept is live"
    );

    s.set_unit("player", Some(player(true)));
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        0,
        "in combat, the answer is locked out"
    );
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the lock does not take the question away — that is what makes it a lock"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button2:IsEnabled()")
            .unwrap(),
        1,
        "and Cancel is never touched"
    );

    s.set_unit("player", Some(player(false)));
    s.run("StaticPopup_OnUpdate(StaticPopup1, 0.1)").unwrap();
    assert_eq!(
        s.eval::<i64>("return StaticPopup1Button1:IsEnabled()")
            .unwrap(),
        1,
        "leaving combat re-arms it, with the same dialog still up"
    );
    s.run("StaticPopup_OnClick(StaticPopup1, 1)").unwrap();
    assert_eq!(s.take_summon_confirms(), 1);
}

/// Cancel and ESC both send **nothing**, and so does letting the clock run out: 1.12 has no
/// decline opcode and no `CancelSummon`, so every path but Accept is silence plus the server's own
/// expiry.
#[test]
fn declining_and_expiring_both_send_nothing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = setup();
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnClick(StaticPopup1, 2)").unwrap();
    assert_eq!(s.take_summon_confirms(), 0, "Cancel is silent");

    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("ToggleGameMenu()").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "hideOnEscape takes it down"
    );
    assert_eq!(s.take_summon_confirms(), 0, "ESC is silent");

    // The window running out: the popup engine's own timeout leg hides the dialog, and this entry
    // has no OnCancel, so nothing at all goes to the wire.
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 2_000,
    });
    s.fire_event("CONFIRM_SUMMON", Vec::new());
    s.run("StaticPopup_OnUpdate(StaticPopup1, 5.0)").unwrap();
    assert!(
        !s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "the offer expires off the screen by itself"
    );
    assert_eq!(s.take_summon_confirms(), 0, "and expiring is silent too");
}

/// The three getters answer the reference's own no-request values — two empty strings and a zero —
/// rather than nil, on a VM nothing has ever pushed to. An addon (or the dialog's own `format`)
/// concatenating a nil here would raise.
#[test]
fn the_getters_answer_empties_before_anything_is_pending() {
    let s = UiScript::new().unwrap();
    assert_eq!(
        s.eval::<String>("return GetSummonConfirmSummoner()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<String>("return GetSummonConfirmAreaName()")
            .unwrap(),
        ""
    );
    assert_eq!(
        s.eval::<f64>("return GetSummonConfirmTimeLeft()").unwrap(),
        0.0
    );
}

/// `GetSummonConfirmTimeLeft()` answers whole **seconds, truncated** — the binding's own
/// `/1000` (`0x48b660`), not a round. It matters because the dialog seeds its countdown from this
/// one call: a rounded-up seed would show one second more than the server is holding.
#[test]
fn the_time_left_getter_truncates_to_whole_seconds() {
    let mut s = UiScript::new().unwrap();
    s.set_summon_confirm(SummonConfirmUiState {
        summoner: "Twomage".into(),
        area: "Stormwind City".into(),
        time_left_ms: 1_999,
    });
    assert_eq!(
        s.eval::<f64>("return GetSummonConfirmTimeLeft()").unwrap(),
        1.0
    );
}
