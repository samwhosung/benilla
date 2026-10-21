//! What a cast attempt reports, read on the script's next line: 1.12.1 makes every report inside
//! the call (`TryCastSpell 0x6e4b60`, reached through direct calls from `CastSpellByName 0x4b4ab0`
//! and `UseAction 0x4e5ee0`), so the line after a cast reads the state it left.

use super::*;

/// The `CMSG_CAST_SPELL`s and `CMSG_CANCEL_CAST`s sent, in order.
fn casts_and_cancels(f: &Frame) -> Vec<(&'static str, u32)> {
    f.rx.try_iter()
        .filter_map(|c| match c {
            ClientCommand::CastSpell { spell_id, .. } => Some(("cast", spell_id)),
            ClientCommand::CancelCast { spell_id } => Some(("cancel", spell_id)),
            _ => None,
        })
        .collect()
}

/// A cast that goes out is set in flight inside the attempt (`0x6e5026` → `SetInFlightSpell
/// 0x6e4ad0`), so on the next line `SpellStopCasting()` (`0x6e6e80`) finds it through
/// `IsCastInFlight 0x6e3d30`, aborts it with `CMSG_CANCEL_CAST` (`0x6e6ec3` → `0x6e4970`) and
/// answers 1.
#[test]
fn spell_stop_casting_on_the_next_line_stops_the_cast_just_sent() {
    let mut f = frame(true);
    run_hosted(
        &mut f,
        r#"BEFORE = SpellStopCasting()
        CastSpellByName("Heal")
        STOPPED = SpellStopCasting()"#,
    );
    assert_eq!(global(&mut f, "BEFORE"), None, "nothing in flight yet");
    assert_eq!(global(&mut f, "STOPPED"), Some(1));
    assert_eq!(
        casts_and_cancels(&f),
        vec![("cast", HEAL), ("cancel", HEAL)]
    );
}

/// `IsCurrentAction` (`0x4e72f0`) reads the slot's live state (`ActionSlotIsCurrent 0x4e53a0`
/// compares the in-flight spell at `0x4e54c1`), so stock `ActionButtonUp`, which asks it straight
/// after `UseAction` to `SetChecked` the button (`ActionButton.lua:54-55`), sees the cast just
/// sent, with no frame between.
#[test]
fn is_current_action_on_the_next_line_reads_the_use_just_made() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    let world = f.app.world_mut();
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .buttons
        .insert(
            0,
            ActionButton {
                slot: 0,
                action: HEAL,
                kind: ACTION_KIND_SPELL,
            },
        );
    crate::ui_action::run_feed_action_state_once(world);
    run_hosted(
        &mut f,
        "BEFORE = IsCurrentAction(1) UseAction(1) AFTER = IsCurrentAction(1)",
    );
    assert_eq!(global(&mut f, "BEFORE"), None);
    assert_eq!(global(&mut f, "AFTER"), Some(1), "checked on the press");
    assert_eq!(casts(&f), vec![(HEAL, None)]);
}

/// What `REPORTS` held when the script set `SEEN` to it, joined with commas.
fn seen(f: &mut Frame) -> String {
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .eval::<Option<String>>("return SEEN")
        .expect("the global reads")
        .unwrap_or_default()
}

/// The `REPORTS` snapshot a chunk takes on its next line.
const SNAPSHOT: &str = r#"SEEN = table.concat(REPORTS, ",")"#;

/// Us dead, a corpse: TryCast's dead rung (`0x6e4c49`) refuses every cast with 0x13.
fn dead(f: &mut Frame) {
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (HEALTH, 0),
            (28, 100),
            (FLAGS, 0x8),
        ])));
}

/// A refused cast is reported inside `TryCastSpell` (`0x6e4c66` → `ReportCastFailure 0x6e1a00`):
/// the line first (`DisplayError`, `0x6e21dd`), then, with nothing in flight, `SPELLCAST_FAILED`
/// (`0x6e2228`-`0x6e224a`), so the script's next line reads both, and nothing goes out.
#[test]
fn a_refused_cast_shows_its_line_and_fails_inside_the_call() {
    let mut f = frame(true);
    dead(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
    assert_eq!(seen(&mut f), "SPELL_FAILED_CASTER_DEAD,SPELLCAST_FAILED");
    assert_eq!(casts_and_cancels(&f), vec![]);
    assert_eq!(reports(&mut f).len(), 2);
    f.app
        .world_mut()
        .run_system_once(crate::ui_cast::feed_cast_reports)
        .unwrap();
    assert_eq!(
        reports(&mut f),
        Vec::<String>::new(),
        "and the frame's feed signals nothing again"
    );
}

/// The refusal's combat-log line is written inside the report as well: after `DisplayError`
/// (`0x6e21dd`), `0x6e21fc` calls `LogCastFailure 0x62c360`, whose `AddCombatLogLine 0x626850`
/// fires `CHAT_MSG_SPELL_FAILED_LOCALPLAYER` at once (`0x49b1b0` → `0x49b0b0`), ahead of the
/// report's `SPELLCAST_FAILED` (`0x6e224a`). So the script's next line reads all three, and the
/// chat feed is left only the line's tee to the log files.
#[test]
fn a_refused_casts_log_line_fires_inside_the_call_between_its_line_and_its_failure() {
    let mut f = frame(true);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay {
            id: HEAL,
            name: "Heal".into(),
            ..Default::default()
        },
    );
    dead(&mut f);
    run(
        &mut f,
        r#"SPELLFAILCASTSELF = "You fail to cast %s: %s."
        local c = CreateFrame("Frame")
        c:RegisterEvent("CHAT_MSG_SPELL_FAILED_LOCALPLAYER")
        c:SetScript("OnEvent", function() table.insert(REPORTS, event .. ":" .. arg1) end)"#,
    );
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
    assert_eq!(
        seen(&mut f),
        "SPELL_FAILED_CASTER_DEAD,\
         CHAT_MSG_SPELL_FAILED_LOCALPLAYER:You fail to cast Heal: SPELL_FAILED_CASTER_DEAD.,\
         SPELLCAST_FAILED"
    );
    assert_eq!(
        f.app
            .world()
            .resource::<crate::ui_chat::ChatLog>()
            .pending_kinds(),
        ["tee"],
        "no event waits for the chat feed"
    );
}

/// Heal and Mark of the Wild in the book, neither in the catalog.
fn with_heal_and_motw(f: &mut Frame) {
    let slot = |spell_id: u32, name: &str| SpellSlotView {
        spell_id,
        name: name.into(),
        ..Default::default()
    };
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Book".into(),
                texture: None,
                offset: 0,
                num_spells: 2,
            }],
            slots: vec![slot(HEAL, "Heal"), slot(MOTW, "Mark of the Wild")],
        });
}

/// The tail fires nothing while another spell is in flight (`0x6e2228` → `0x6e224f`): a second
/// spell pressed over the cast just sent shows "Another action is in progress" (0x61, `0x6e4d97`)
/// and no event; the same spell pressed again is the same-spell bail (`0x6e4d43`), silent.
#[test]
fn a_refusal_over_a_cast_in_flight_fires_no_event() {
    let mut f = frame(true);
    with_heal_and_motw(&mut f);
    run_hosted(
        &mut f,
        &format!(
            r#"CastSpellByName("Heal") CastSpellByName("Heal")
            CastSpellByName("Mark of the Wild") {SNAPSHOT}"#
        ),
    );
    assert_eq!(seen(&mut f), "SPELL_FAILED_SPELL_IN_PROGRESS");
    assert_eq!(casts_and_cancels(&f), vec![("cast", HEAL)]);
}

/// Every cancel of the cursor is `StopTargeting 0x6e4900`, the cancel with its failed flag
/// (`0x6e490b`, `0x6e4940(1, 1, 0x1c)`): the word drops with no packet and the record's caster
/// fires `SPELLCAST_FAILED`, inside the call. Its two script entries: `SpellStopTargeting()`
/// (`0x6e6e4a`) and a new cast's supersede (`TryCast`, `0x6e4d71`), which fires it ahead of the
/// cast that goes on.
#[test]
fn a_cancelled_cursor_fails_inside_the_call() {
    let mut f = frame(true);
    with_the_cursor_up(&mut f);
    run_hosted(&mut f, &format!("SpellStopTargeting() {SNAPSHOT}"));
    assert_eq!(seen(&mut f), "SPELLCAST_FAILED");
    assert!(!cursor_is_up(&f));
    assert_eq!(
        casts_and_cancels(&f),
        vec![],
        "a cursor's cancel sends nothing"
    );

    let mut f = frame(true);
    with_heal_and_motw(&mut f);
    with_the_cursor_up(&mut f);
    run_hosted(
        &mut f,
        &format!(r#"CastSpellByName("Mark of the Wild") {SNAPSHOT}"#),
    );
    assert_eq!(seen(&mut f), "SPELLCAST_FAILED", "the supersede's event");
    assert!(!cursor_is_up(&f));
    assert_eq!(
        casts_and_cancels(&f),
        vec![("cast", MOTW)],
        "and the cast goes on"
    );
}

/// The pair `0x4b3250` signals, in its order, and the cast's events, as a script sees them: `A`
/// for `ACTIONBAR_UPDATE_STATE` (`0x4e5bf0`), `C` for `CURRENT_SPELL_CAST_CHANGED` (`0x4b325a`),
/// the rest by name.
fn record_changes(f: &mut Frame) {
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .run(
            r#"CHANGES = {}
            local c = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_UPDATE_STATE", "CURRENT_SPELL_CAST_CHANGED",
                    "SPELLCAST_FAILED", "SPELLCAST_STOP", "UI_ERROR_MESSAGE"}) do
                c:RegisterEvent(e)
            end
            c:SetScript("OnEvent", function()
                if event == "ACTIONBAR_UPDATE_STATE" then table.insert(CHANGES, "A")
                elseif event == "CURRENT_SPELL_CAST_CHANGED" then table.insert(CHANGES, "C")
                elseif event == "UI_ERROR_MESSAGE" then table.insert(CHANGES, arg1)
                else table.insert(CHANGES, event) end
            end)"#,
        )
        .unwrap();
}

/// `CHANGES` as the chunk's last line saw it.
const CHANGES: &str = r#"SEEN = table.concat(CHANGES, ",") CHANGES = {}"#;

/// A spell with no target word sends from the arm (`0x6e5338` → `0x6e5349`), and the attempt
/// signals the pair four times inside the call: the push (`0x6e5026`), the arm (`0x6e5333`), the
/// send (`0x6e589d`) and its end (`0x6e51e3`). The spell is on no action bar slot, and still
/// signals: the pair is the cast's, not the bar's.
#[test]
fn a_cast_from_no_bar_slot_signals_its_four_changes_inside_the_call() {
    let mut f = frame(false);
    with_book_spell(
        &mut f,
        HEAL,
        "Heal",
        benilla_formats::SpellDisplay::default(),
    );
    record_changes(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {CHANGES}"#));
    assert_eq!(seen(&mut f), "A,C,A,C,A,C,A,C");
    assert_eq!(casts(&f), vec![(HEAL, None)]);
}

/// A cast bound to the selection goes through `BindTarget`'s merge, which signals once more before
/// the send (`0x6e60cb`): five pairs.
#[test]
fn a_cast_bound_to_the_selection_signals_five_changes() {
    let mut f = frame(true);
    // A friendly player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PLAYER_CONTROLLED`), whom
    // `CanAssist` passes.
    with_factions(&mut f, FRIEND, &[(2, 0x19), (FLAGS, 0x8)]);
    with_book_spell(
        &mut f,
        MOTW,
        "Mark of the Wild",
        benilla_formats::SpellDisplay {
            implicit_target_a1: 21,
            ..Default::default()
        },
    );
    record_changes(&mut f);
    run_hosted(
        &mut f,
        &format!(r#"CastSpellByName("Mark of the Wild") {CHANGES}"#),
    );
    assert_eq!(seen(&mut f), "A,C,A,C,A,C,A,C,A,C");
    assert_eq!(casts(&f), vec![(MOTW, Some(OLD))]);
}

/// A refusal ahead of the push signals no change, only its line and event; a cursor cancelled
/// signals its word's clear (`0x6e495c`) and the slot's pop (`0x6e49d6`) ahead of its
/// `SPELLCAST_FAILED` (`0x6e4a50`).
#[test]
fn a_refusal_signals_no_change_and_a_cancel_signals_two() {
    let mut f = frame(true);
    dead(&mut f);
    record_changes(&mut f);
    run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {CHANGES}"#));
    assert_eq!(seen(&mut f), "SPELL_FAILED_CASTER_DEAD,SPELLCAST_FAILED");

    let mut f = frame(true);
    with_the_cursor_up(&mut f);
    record_changes(&mut f);
    run_hosted(&mut f, &format!("SpellStopTargeting() {CHANGES}"));
    assert_eq!(seen(&mut f), "A,C,A,C,SPELLCAST_FAILED");
}

/// Attack toggled off is StopAttack (`0x6131a0` → `0x5ecac0`): `CMSG_ATTACKSTOP`, then
/// `CancelQueuedCast` (`0x6e6f30`), which cancels the strike in the slot as `SpellStopCasting`
/// does (`0x6e6f68`): its `CMSG_CANCEL_CAST`, its pop's pair and `SPELLCAST_STOP`, inside the
/// call, so the stock bar unchecks the strike's button with the swing.
#[test]
fn an_attack_toggled_off_cancels_its_queued_strike_with_a_pair_then_stop() {
    const HEROIC_STRIKE: u32 = 78;
    let mut f = frame(true);
    with_factions(&mut f, FOE, &[(HEALTH, 100)]);
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::Engaged(OLD));
    world
        .resource_mut::<crate::spell::QueuedMeleeSpell>()
        .arm(HEROIC_STRIKE);
    world
        .resource_mut::<crate::spell::PendingCast>()
        .fill_record();
    record_changes(&mut f);
    run_hosted(&mut f, &format!("AttackTarget() {CHANGES}"));
    assert_eq!(seen(&mut f), "A,C,SPELLCAST_STOP");
    assert_eq!(casts_and_cancels(&f), vec![("cancel", HEROIC_STRIKE)]);
    assert_eq!(
        f.app
            .world()
            .resource::<crate::spell::QueuedMeleeSpell>()
            .current(),
        None
    );
}

/// A refusal whose report leaves before its line and its tail (`report_reaches_pop`): a form the
/// spell needs with no name to show, "Must be in %s" (0x56, `0x6e20c6`), refused by the attempt
/// itself, shows nothing and fires nothing; with its form named the line shows under errorId
/// `0xd6` (`0x6e1be9`), whose string the recorder defines as its own key, and the tail, nothing
/// in flight, fires `SPELLCAST_FAILED`.
#[test]
fn a_local_refusal_that_skips_its_text_skips_its_tail() {
    for (named, expected) in [
        (false, ""),
        (true, "ERR_SPELL_FAILED_SHAPESHIFT_FORM_S,SPELLCAST_FAILED"),
    ] {
        let mut f = frame(true);
        with_book_spell(
            &mut f,
            HEAL,
            "Heal",
            benilla_formats::SpellDisplay {
                stances: 1,
                ..Default::default()
            },
        );
        if named {
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::Spells>()
                .forms
                .insert(
                    1,
                    benilla_formats::ShapeshiftForm {
                        name: "Cat Form".into(),
                        ..Default::default()
                    },
                );
        }
        run_hosted(&mut f, &format!(r#"CastSpellByName("Heal") {SNAPSHOT}"#));
        assert_eq!(seen(&mut f), expected, "named {named}");
        assert_eq!(casts_and_cancels(&f), vec![], "named {named}");
    }
}

/// The auto-repeat toggle is `UseAction`'s alone (`0x4e60d1`-`0x4e60da`, `0x6e9fd0` against the
/// slot's spell): `CastSpellByName` of the running repeat spell goes through `TryCast`, which never
/// reads the key, so it casts again and the repeat runs on; the bar's press stops it (`0x6ea080`).
#[test]
fn only_the_bars_press_toggles_a_running_repeat_off() {
    const SHOOT: u32 = 5019;
    let mut f = frame(true);
    with_book_spell(
        &mut f,
        SHOOT,
        "Shoot",
        benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes_ex2: 0x20,
            ..Default::default()
        },
    );
    f.app
        .world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .buttons
        .insert(
            0,
            ActionButton {
                slot: 0,
                action: SHOOT,
                kind: ACTION_KIND_SPELL,
            },
        );
    let key = |f: &Frame| f.app.world().resource::<crate::spell::AutoRepeatActive>().0;
    run_hosted(&mut f, r#"CastSpellByName("Shoot")"#);
    assert_eq!(key(&f), Some(SHOOT), "the first cast starts the repeat");
    let _ = f.rx.try_iter().count();

    run_hosted(&mut f, r#"CastSpellByName("Shoot")"#);
    let sent: Vec<ClientCommand> = f.rx.try_iter().collect();
    assert!(
        !sent
            .iter()
            .any(|c| matches!(c, ClientCommand::CancelAutoRepeat)),
        "{sent:?}"
    );
    assert!(sent.iter().any(|c| matches!(
        c,
        ClientCommand::CastSpell {
            spell_id: SHOOT,
            ..
        }
    )));
    assert_eq!(key(&f), Some(SHOOT), "the repeat runs on");

    run_hosted(&mut f, "UseAction(1)");
    assert!(f
        .rx
        .try_iter()
        .any(|c| matches!(c, ClientCommand::CancelAutoRepeat)));
    assert_eq!(key(&f), None, "the bar's press stops it");
}
