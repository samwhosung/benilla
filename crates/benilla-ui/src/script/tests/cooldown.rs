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
    // An empty slot reaches no cooldown query, so the enable stays the binding's 0 (`0x4e6f0d`).
    assert!(s
        .eval::<bool>("local st, d, e = GetActionCooldown(3); return st == 0 and d == 0 and e == 0")
        .unwrap());

    s.tick(100.0); // an arbitrary clock epoch
    s.set_action_state(
        3,
        Some(ActionState {
            usable: false,
            not_enough_mana: true,
            in_range: Some(false),
            has_range: true,
            spell: Some(133),
            form: None,
            profession: None,
            is_attack: false,
            equipped: false,
            item: None,
            // 4 s remaining of a 10 s cooldown, running: started at GetTime 94.
            cooldown: Some((94_000, 10_000, true)),
            no_cooldown_query: false,
        }),
    );

    assert!(s
        .eval::<bool>("local u, oom = IsUsableAction(3); return u == nil and oom == 1")
        .unwrap());
    assert!(s.eval::<bool>("return IsActionInRange(3) == 0").unwrap());
    assert!(s.eval::<bool>("return ActionHasRange(3) == 1").unwrap());
    assert!(s.eval::<bool>("return IsCurrentAction(3) == nil").unwrap());
    // Checked while its spell is the one in flight, read live off the cast state.
    s.set_cast_view(CastView {
        slot: Some(133),
        ..Default::default()
    });
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
            use_spell: None,
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

/// `GetActionCooldown` (`0x4e6ed0`) starts its enable at 0 (`0x4e6f0d`) and only the cooldown query
/// writes it, to 1 before any leg (`0x6e1406`): a slot that reaches no query answers `(0, 0, 0)`,
/// one that reaches it with nothing running `(0, 0, 1)`.
#[test]
fn get_action_cooldown_answers_enable_0_for_a_slot_that_reaches_no_query() {
    let mut s = script();
    let triple = |s: &UiScript, n: u32| {
        s.eval::<(f64, f64, i64)>(&format!("return GetActionCooldown({n})"))
            .unwrap()
    };
    s.set_action_state(
        1,
        Some(ActionState {
            spell: Some(133),
            ..Default::default()
        }),
    );
    s.set_action_state(
        2,
        Some(ActionState {
            no_cooldown_query: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        triple(&s, 1),
        (0.0, 0.0, 1),
        "a spell with no cooldown running"
    );
    assert_eq!(triple(&s, 2), (0.0, 0.0, 0), "a slot that reaches no query");
    assert_eq!(triple(&s, 3), (0.0, 0.0, 0), "an empty slot");
    for n in [0, 121] {
        assert_eq!(triple(&s, n), (0.0, 0.0, 0), "slot {n}, out of 1..120");
    }
}

/// `IsCurrentAction`'s item arm (`0x4e53c8`-`0x4e5428`): an item slot is current while the
/// in-flight cast's item object has the slot's entry, and it answers from that arm alone, never
/// the spell, form or profession arms.
#[test]
fn is_current_action_holds_an_item_slot_against_the_in_flight_item() {
    let mut s = script();
    s.set_action_state(
        1,
        Some(ActionState {
            item: Some(6948),
            ..Default::default()
        }),
    );
    let current = |s: &UiScript| s.eval::<bool>("return IsCurrentAction(1) == 1").unwrap();
    assert!(!current(&s), "nothing in flight");
    s.set_cast_view(CastView {
        slot: Some(8690),
        item: Some(6948),
        ..Default::default()
    });
    assert!(current(&s), "the Hearthstone's own cast");
    s.set_cast_view(CastView {
        slot: Some(8690),
        item: Some(5512),
        ..Default::default()
    });
    assert!(!current(&s), "another item's cast");
    s.set_cast_view(CastView {
        slot: Some(8690),
        ..Default::default()
    });
    assert!(!current(&s), "the spell in flight with no item object");
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
        name: None,
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

/// A held action (enable 0) answers the call's own now (`0x6e1517`, `0x6e15a1`): two polls with
/// no feed change in between return starts as far apart as `GetTime()`.
#[test]
fn a_held_action_answers_the_polls_now() {
    let mut s = script();
    s.tick(50.0);
    s.set_action_state(
        3,
        Some(ActionState {
            cooldown: Some((0, 1, false)),
            ..Default::default()
        }),
    );
    let poll = |s: &UiScript| {
        s.eval::<(f64, f64, i32)>("return GetActionCooldown(3)")
            .unwrap()
    };
    let first = poll(&s);
    assert_eq!(first, (50.0, 0.001, 0));
    s.tick(0.02);
    let second = poll(&s);
    assert_eq!(second.0, s.eval::<f64>("return GetTime()").unwrap());
    assert!((second.0 - first.0 - 0.02).abs() < 1e-9, "20 ms apart");
}

/// `GetInventoryItemCooldown` (`0x4c8a60`) answers the item query only for a worn item of the
/// active player whose template names a block-0 spell (`+0x11c`), and three zeros, enable included,
/// for every other case (`0x4c8bdf`). An on-use spell with nothing running answers enabled.
#[test]
fn get_inventory_item_cooldown_answers_the_item_query_under_the_clients_gates() {
    use std::collections::HashMap;
    let mut s = script();
    s.tick(100.0);
    let item = |spell_0| {
        Some(InvSlotView {
            item_id: 14_000,
            spell_0,
            ..Default::default()
        })
    };
    let mut inv: InventorySlots = Default::default();
    inv[13] = item(true); // running: 4 s into 10 s
    inv[14] = item(true); // on-use, cold
    inv[15] = item(true); // no answer: no on-use spell, or a spent charged template
    inv[16] = item(false); // has an answer, but block 0 names no spell
    s.set_inventory_slots(inv);
    s.set_worn_cooldowns(HashMap::from([
        (13, (96_000, 10_000, true)),
        (14, (0, 0, true)),
        (16, (96_000, 10_000, true)),
    ]));
    let poll = |s: &UiScript, unit: &str, slot: i32| {
        s.eval::<(f64, f64, i32)>(&format!(
            "return GetInventoryItemCooldown('{unit}', {slot})"
        ))
        .unwrap()
    };
    assert_eq!(
        poll(&s, "player", 13),
        (96.0, 10.0, 1),
        "a running cooldown"
    );
    assert_eq!(poll(&s, "player", 14), (0.0, 0.0, 1), "cold is enabled");
    assert_eq!(poll(&s, "player", 15), (0.0, 0.0, 0), "no answer: all zero");
    assert_eq!(poll(&s, "player", 16), (0.0, 0.0, 0), "no spell in block 0");
    assert_eq!(poll(&s, "player", 12), (0.0, 0.0, 0), "an empty slot");
    assert_eq!(poll(&s, "player", 0), (0.0, 0.0, 0), "the ammo slot (-1)");
    assert_eq!(
        poll(&s, "target", 13),
        (0.0, 0.0, 0),
        "not the active player"
    );
    // Past its end the same entry reads cold, enabled.
    s.tick(7.0);
    assert_eq!(poll(&s, "player", 13), (0.0, 0.0, 1));
    let err = |code: &str| s.run(code).unwrap_err().to_string();
    assert!(err("GetInventoryItemCooldown({}, 1)")
        .contains("Usage: GetInventoryItemCooldown(unit, slot)"));
    assert!(err("GetInventoryItemCooldown('player', 30)")
        .contains("Invalid inventory slot in GetInventoryItemCooldown"));
}
