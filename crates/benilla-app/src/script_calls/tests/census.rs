//! The census of the sends a call makes while a handler it signalled is held
//! ([`benilla_ui::script::census`]). `SignalEvent` (`0x703e50`) runs every listener before the
//! verb goes on, so such a send goes out ahead of what the handler sends in 1.12.1. The kinds that
//! still do are listed, each with the test that shows it; a kind made faithful leaves the list,
//! and none joins it.

use super::*;
use crate::script_calls::census::shows_only;

/// A frame noting every event, so each signal a call makes is held for a listener.
fn listening(f: &mut Frame) {
    run(
        f,
        r#"local all = CreateFrame("Frame")
        all:RegisterAllEvents()
        all:SetScript("OnEvent", function() end)"#,
    );
    benilla_ui::script::census::take();
}

/// The bar's equip of a helm with an item held: see [`SENDS_AHEAD`].
#[test]
fn a_bar_equip_sends_ahead_of_its_held_cursor_signals() {
    let mut f = frame(false);
    with_bar_helm(&mut f, 0);
    listening(&mut f);
    run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");
    shows_only("UseAction");
}

/// The selection, cast, attack and chat verbs send nothing while a handler they signalled waits.
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
        listening(&mut f);
        run_pressed(&mut f, lua);
        assert_eq!(
            benilla_ui::script::census::take(),
            Default::default(),
            "{lua}"
        );
    }
}
