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
