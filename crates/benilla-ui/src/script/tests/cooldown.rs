//! The cooldown bindings: per-action state and the `GetTime`-space triples stock `Cooldown.lua`
//! consumes (its `<Model>` widget runs in `tests::model_clock`).

use super::common::script;
use crate::script::*;

/// The 1/nil conventions, `IsActionInRange`'s tri-state, and a cooldown that goes cold at expiry.
#[test]
fn action_state_bindings_answer_the_reference_conventions() {
    let mut s = script();

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

    assert!(s
        .eval::<bool>("local u, oom = IsUsableAction(3); return u == nil and oom == 1")
        .unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == 0").unwrap());
    assert!(s.eval::<bool>("return ActionHasRange(3) == 1").unwrap());
    assert!(s.eval::<bool>("return IsCurrentAction(3) == 1").unwrap());
    assert!(s
        .eval::<bool>("return IsAutoRepeatAction(3) == nil")
        .unwrap());
    // `IsConsumableAction` reads the slot, not this state: it arrives with the icon.
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

    assert!(s
        .eval::<bool>(
            "local st, d, e = GetActionCooldown(3); \
             return math.abs(st - 94) < 0.001 and d == 10 and e == 1"
        )
        .unwrap());

    // It ends at 94 + 10 = 104: live at 103.9 and cold after, so a re-fed finished pair cannot
    // replay the sweep.
    s.tick(3.9);
    assert!(s
        .eval::<bool>("local st, d = GetActionCooldown(3); return d == 10")
        .unwrap());
    s.tick(0.2);
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 1")
        .unwrap());

    // An on-hold cooldown (enable 0) never goes cold on its own; it waits for the event.
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

    s.set_action_state(3, None);
    assert!(s.eval::<bool>("return IsCurrentAction(3) == nil").unwrap());
}

/// The absolute start anchors the sweep: re-pushing a running cooldown (a kill flipping `usable`)
/// keeps it, so the sweep does not reset, and a re-arm moves it even at the same duration.
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

    s.tick(8.0);
    s.set_action_state(3, Some(cooling(true)));
    assert!(s
        .eval::<bool>(
            "local st, d = GetActionCooldown(3); \
             return math.abs(st - 100) < 1e-3 and d == 15"
        )
        .unwrap());

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

    // The same triples re-pushed 5 s later: the anchors hold at 50.
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
