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
