//! The census of the sends and the writes a call makes while a handler it signalled is held
//! ([`benilla_ui::script::census`]). `SignalEvent` (`0x703e50`) runs every listener before the
//! verb goes on, so such a send goes out ahead of what the handler sends in 1.12.1, and such a
//! write is read by the handler as it stands later. The kinds that still do are listed, each with
//! the test that shows it; a kind made faithful leaves the list, and none joins it. No send
//! goes ahead of a handler the reference runs first, so the list of those is empty.

use super::selection::{queue_strike, swinging_at_old};
use super::*;
use crate::script_calls::census::{shows_no_send, shows_writes_only, writes_after};

/// A frame noting every event, so each signal a call makes is held for a listener, and the
/// census watching the writes while the guard lives.
fn listening(f: &mut Frame) -> benilla_ui::script::census::WriteWatch {
    run(
        f,
        r#"local all = CreateFrame("Frame")
        all:RegisterAllEvents()
        all:SetScript("OnEvent", function() end)"#,
    );
    let watch = benilla_ui::script::census::watch_writes();
    benilla_ui::script::census::take();
    benilla_ui::script::census::take_writes();
    watch
}

/// The bar's equip of a helm with an item held: `SetCursorItem`'s clear and put signal
/// (`0x494bb6`, `0x494c54`) and run their listeners before `AutoEquipCursorItem` sends
/// (`0x5e1757`), whose own clear signals (`0x5e1763`) after the send. A listener's roll shows each
/// point's place on the wire.
#[test]
fn a_bar_equip_signals_its_cursor_set_ahead_of_its_send() {
    let mut f = frame(false);
    with_bar_helm(&mut f, 0);
    run(
        &mut f,
        r#"local fr = CreateFrame("Frame")
        fr:RegisterEvent("CURSOR_UPDATE")
        fr:SetScript("OnEvent", function() if ARMED then RandomRoll(1, 2) end end)"#,
    );
    run(&mut f, "PickupContainerItem(0, 1)");
    f.rx.try_iter().for_each(drop);
    benilla_ui::script::census::take();
    run(&mut f, "ARMED = true UseAction(1, 0)");
    let wire: Vec<_> =
        f.rx.try_iter()
            .map(|c| match c {
                ClientCommand::RandomRoll { .. } => "roll",
                ClientCommand::AutoEquipItem { .. } => "equip",
                _ => "other",
            })
            .collect();
    assert_eq!(wire, ["roll", "roll", "equip", "roll"]);
    // The send is handed over in place, so no handler is held when it goes out.
    shows_no_send();
}

/// A deselect while swinging reports the queued strike's stop first (`StopAttack`, `0x493a08`),
/// ahead of the release's usable refresh (`0x493a69`) and `PLAYER_TARGET_CHANGED` (`0x493a73`).
/// The report waits for the point and the bar's states are written before it: see
/// [`WRITES_AFTER`](crate::script_calls::census::WRITES_AFTER).
#[test]
fn a_teardowns_strike_cancel_report_waits_behind_the_bars_usable_refresh() {
    let mut f = frame(true);
    swinging_at_old(&mut f);
    queue_strike(&mut f);
    let _watch = listening(&mut f);
    run(&mut f, "ClearTarget()");
    shows_writes_only("ClearTarget:action_states");
}

/// An engine writer's deselect (`SelectTarget(0)`, `0x490927`) is the same teardown as an engine
/// turn: its strike cancel's report waits behind the bar's usable refresh.
#[test]
fn an_engine_deselects_strike_cancel_report_waits_behind_the_bars_usable_refresh() {
    let mut f = frame(true);
    swinging_at_old(&mut f);
    queue_strike(&mut f);
    let _watch = listening(&mut f);
    f.app
        .world_mut()
        .run_system_once(|mut vm: VmWith<()>| vm.engine_clear_target(crate::lanes::mark()))
        .expect("the turn runs");
    shows_writes_only("turn@script_calls.rs:action_states");
}

/// The selection, cast, attack and chat verbs send and write nothing after a handler they
/// signalled that waits.
#[test]
fn the_other_verbs_send_nothing_ahead_of_a_held_handler() {
    for lua in [
        r#"TargetUnit("party1")"#,
        "ClearTarget()",
        "TargetLastTarget()",
        "TargetNearestEnemy()",
        "AttackTarget()",
        r#"TargetUnit("party1") AttackTarget() TargetUnit("player")"#,
        r#"CastSpellByName("Heal")"#,
        r#"SendChatMessage("hi", "SAY")"#,
        "RandomRoll(1, 100)",
    ] {
        let mut f = frame(true);
        spawn_mob(&mut f, &[]);
        let _watch = listening(&mut f);
        run_pressed(&mut f, lua);
        assert_eq!(
            benilla_ui::script::census::take(),
            Default::default(),
            "{lua}"
        );
        assert_eq!(writes_after(), Vec::<String>::new(), "{lua}");
    }
}
