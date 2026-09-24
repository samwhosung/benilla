//! The cooldown **bindings**: the per-action dynamic-state read API and the `GetTime`-space
//! cooldown triples the stock `Cooldown.lua` machine consumes (the widget itself is the
//! reference's own `<Model>` since decision 2019 — `tests::model_clock` runs that file verbatim).

use super::common::script;
use crate::script::*;

/// The per-action dynamic-state API: the 1/nil conventions, IsActionInRange's tri-state, and
/// GetActionCooldown's GetTime-space triple that goes cold at expiry.
#[test]
fn action_state_bindings_answer_the_reference_conventions() {
    let mut s = script();

    // No state pushed: everything nil / cold.
    assert!(s.eval::<bool>("return IsUsableAction(3) == nil").unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == nil").unwrap());
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 1")
        .unwrap());

    s.tick(100.0); // an arbitrary clock epoch
    s.set_action_state(
        3,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            in_range: Some(false),
            has_range: true,
            current: true,
            auto_repeat: false,
            is_attack: false,
            equipped: false,
            // 4 s remaining of a 10 s cooldown, running: started at GetTime 94.
            cooldown: Some((94_000, 10_000, true)),
        }),
    );

    // The 1/nil pairs, exactly as the transcribed `if` reads them.
    assert!(s
        .eval::<bool>("local u, oom = IsUsableAction(3); return u == nil and oom == 1")
        .unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == 0").unwrap());
    assert!(s.eval::<bool>("return ActionHasRange(3) == 1").unwrap());
    assert!(s.eval::<bool>("return IsCurrentAction(3) == 1").unwrap());
    assert!(s
        .eval::<bool>("return IsAutoRepeatAction(3) == nil")
        .unwrap());
    // IsConsumableAction reads the SLOT, not this map — it is a pure query over
    // the item template, so it arrives with the icon it gates the count beside.
    assert!(s
        .eval::<bool>("return IsConsumableAction(3) == nil")
        .unwrap());
    s.set_action(
        3,
        Some(ActionSlot {
            texture: None,
            kind: 0x80,
            action: 117,
            count: 4,
            consumable: true,
        }),
    );
    assert!(s.eval::<bool>("return IsConsumableAction(3) == 1").unwrap());

    // GetActionCooldown: the pushed absolute start, verbatim in seconds.
    assert!(s
        .eval::<bool>(
            "local st, d, e = GetActionCooldown(3); \
             return math.abs(st - 94) < 0.001 and d == 10 and e == 1"
        )
        .unwrap());

    // 5 s later the same stored pair still answers (4 s window has 0 left → but 100+? no: it
    // expires at start+10 = now+4) …after the remaining 4 s pass, the read goes cold — the
    // stale-refeed guard (a re-fed finished pair must not replay the sweep/flash).
    s.tick(3.9);
    assert!(s
        .eval::<bool>("local st, d = GetActionCooldown(3); return d == 10")
        .unwrap());
    s.tick(0.2);
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 1")
        .unwrap());

    // An on-hold (enable == 0) cooldown never goes cold on its own — parked until the event.
    s.set_action_state(
        3,
        Some(ActionState {
            cooldown: Some((100_000, 30_000, false)),
            ..Default::default()
        }),
    );
    s.tick(120.0);
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return d == 30 and e == 0")
        .unwrap());

    // Clearing the state clears the reads.
    s.set_action_state(3, None);
    assert!(s.eval::<bool>("return IsCurrentAction(3) == nil").unwrap());
}

/// The absolute-start triple is the anchor — both director regressions pin here. Reset-on-kill
/// ("the cooldown indicator resets when you kill a mob"): an unrelated field flip re-pushes the
/// same running cooldown, whose triple carries the SAME start, so the sweep holds. The vanished
/// GCD pie ("spamming Rend during Charge never shows the pie"): a fail-clear + re-arm between
/// feeds pushes a triple with a NEW start — under the old `(remaining, duration)` shape the two
/// arms read byte-identical and the seam kept the first, long-elapsed anchor.
#[test]
fn the_absolute_start_triple_holds_the_anchor_and_a_rearm_moves_it() {
    let mut s = script();
    s.tick(100.0);
    let cooling = |usable: bool| ActionState {
        usable,
        cooldown: Some((100_000, 15_000, true)),
        ..Default::default()
    };
    s.set_action_state(3, Some(cooling(false)));
    assert!(s
        .eval::<bool>("local st = GetActionCooldown(3); return math.abs(st - 100) < 1e-3")
        .unwrap());

    // 8 s later the kill drops combat and `usable` flips; the re-push carries the same start.
    // The anchor holds at 100 — the sweep never restarts (the reset-on-kill invariant).
    s.tick(8.0);
    s.set_action_state(3, Some(cooling(true)));
    assert!(s
        .eval::<bool>(
            "local st, d = GetActionCooldown(3); \
             return math.abs(st - 100) < 1e-3 and d == 15"
        )
        .unwrap());

    // A RE-ARM carries a fresh start and moves the anchor — even with the same duration (the
    // vanished-pie shape: same spell, same 15 s, armed again at t=107).
    s.set_action_state(
        3,
        Some(ActionState {
            cooldown: Some((107_000, 15_000, true)),
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>("local st = GetActionCooldown(3); return math.abs(st - 107) < 1e-3")
        .unwrap());
}

/// The same absolute-start law on the other two converters: the stance bar's form cooldown and
/// the bag slot's item cooldown hold their anchors across unchanged re-pushes.
#[test]
fn shapeshift_and_container_cooldowns_keep_their_anchors_too() {
    let mut s = script();
    s.tick(50.0);
    let form = || ShapeshiftFormView {
        spell_id: 2457,
        name: "Battle Stance".into(),
        cooldown: Some((50_000, 10_000, true)),
        ..Default::default()
    };
    s.set_shapeshift_forms(vec![form()]);
    let slot = || ContainerSlot {
        item_id: 118,
        count: 1,
        cooldown: Some((50_000, 30_000, true)),
        ..Default::default()
    };
    let bag = || ContainerState {
        name: Some("Backpack".into()),
        num_slots: 16,
        slots: std::collections::HashMap::from([(1, slot())]),
    };
    s.set_container(0, Some(bag()));
    assert!(s
        .eval::<bool>("local st = GetShapeshiftFormCooldown(1); return math.abs(st - 50) < 1e-3")
        .unwrap());
    assert!(s
        .eval::<bool>("local st = GetContainerItemCooldown(0, 1); return math.abs(st - 50) < 1e-3")
        .unwrap());

    // Re-push both with the SAME raw triples 5 s later: the anchors hold at 50.
    s.tick(5.0);
    s.set_shapeshift_forms(vec![form()]);
    s.set_container(0, Some(bag()));
    assert!(s
        .eval::<bool>("local st = GetShapeshiftFormCooldown(1); return math.abs(st - 50) < 1e-3")
        .unwrap());
    assert!(s
        .eval::<bool>("local st = GetContainerItemCooldown(0, 1); return math.abs(st - 50) < 1e-3")
        .unwrap());
}
