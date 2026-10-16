//! ITEM_COOLDOWN_TIME (`0x854adc`) on the item tooltip: the unit `0x52fa50` picks, the line's seat
//! after the set block (`0x52e0fd..0x52e13b`), the absence of any floor, and the three setters that
//! carry a cooldown into the builder (`SetBagItem`, `SetInventoryItem`, `SetAction`'s item leg).

use std::collections::HashMap;

use super::{lines_of, script as item_script};
use crate::script::*;

/// The GetTime clock the cooldowns below are measured against.
const NOW_S: f32 = 10.0;

fn script() -> UiScript {
    let mut s = item_script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        ITEM_COOLDOWN_TIME_DAYS  = "[CD %d d]"
        ITEM_COOLDOWN_TIME_HOURS = "[CD %d h]"
        ITEM_COOLDOWN_TIME_MIN   = "[CD %d m]"
        ITEM_COOLDOWN_TIME_SEC   = "[CD %d s]"
        local a = CreateFrame("Button", "Slot"); a:SetPoint("CENTER", 0, 0)
        a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
    s.tick(NOW_S);
    s
}

fn blade() -> ItemTemplateView {
    ItemTemplateView {
        name: "Timed Blade".into(),
        quality: 2,
        ..Default::default()
    }
}

/// A backpack holding the blade in slot 1 with a cooldown that has `remaining_ms` left at
/// [`NOW_S`], or none.
fn put_blade(s: &mut UiScript, remaining_ms: Option<u32>) {
    s.set_item_template(8200, blade());
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 8200,
                    count: 1,
                    quality: Some(2),
                    link: Some("|cff1eff00|Hitem:8200:0:0:0|h[Timed Blade]|h|r".into()),
                    cooldown: remaining_ms.map(|ms| (i64::from(NOW_S as u32) * 1000, ms, true)),
                    ..Default::default()
                },
            )]),
        }),
    );
}

fn hover_bag(s: &mut UiScript) -> Vec<String> {
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') TT:SetBagItem(0, 1)")
        .unwrap();
    lines_of(s).into_iter().map(|(t, _)| t).collect()
}

/// `0x52fa50` picks the unit by the immediates 86400000, 3600000 and 60000, rounds days, hours and
/// minutes up and seconds down, and the line is white.
#[test]
fn the_cooldown_line_picks_its_unit_and_rounds_as_the_formatter_does() {
    let mut s = script();
    for (ms, want) in [
        (1, "[CD 0 s]"),
        (1_999, "[CD 1 s]"),
        (59_999, "[CD 59 s]"),
        (60_000, "[CD 1 m]"),
        (60_001, "[CD 2 m]"),
        (3_599_999, "[CD 60 m]"),
        (3_600_000, "[CD 1 h]"),
        (3_600_001, "[CD 2 h]"),
        (86_399_999, "[CD 24 h]"),
        (86_400_000, "[CD 1 d]"),
        (86_400_001, "[CD 2 d]"),
    ] {
        put_blade(&mut s, Some(ms));
        let lines = hover_bag(&mut s);
        assert_eq!(lines.last().map(String::as_str), Some(want), "{ms} ms");
    }
    let lines = lines_of(&mut s);
    assert_eq!(lines.last().unwrap().1, [1.0, 1.0, 1.0, 1.0], "white");
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// An item's line has no floor, where a spell's needs a recovery of a minute: five seconds left
/// draws, and a cold slot or an expired cooldown draws nothing.
#[test]
fn the_item_cooldown_line_has_no_floor_and_needs_a_running_cooldown() {
    let mut s = script();
    put_blade(&mut s, Some(5_000));
    assert_eq!(hover_bag(&mut s), ["Timed Blade", "[CD 5 s]"]);
    put_blade(&mut s, None);
    assert_eq!(hover_bag(&mut s), ["Timed Blade"], "no cooldown, no line");
    // Started at 10 s for 0 ms: not running.
    put_blade(&mut s, Some(0));
    assert_eq!(
        hover_bag(&mut s),
        ["Timed Blade"],
        "an elapsed one, no line"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// The line sits after the set block and above the flavor text (`0x52e0fd`, after `0x52e0f5`).
#[test]
fn the_cooldown_line_follows_the_set_block_and_precedes_the_description() {
    let mut s = script();
    s.set_item_template(
        8200,
        ItemTemplateView {
            item_set: 161,
            description: "Ticks.".into(),
            ..blade()
        },
    );
    s.set_item_set(
        161,
        ItemSetView {
            name: "Tick Set".into(),
            members: vec![(8200, Some("Timed Blade".into()))],
            bonuses: vec![(2, "+1 Tick.".into())],
            ..Default::default()
        },
    );
    s.set_container(
        0,
        Some(ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots: HashMap::from([(
                1,
                ContainerSlot {
                    item_id: 8200,
                    count: 1,
                    quality: Some(2),
                    link: Some("|cff1eff00|Hitem:8200:0:0:0|h[Timed Blade]|h|r".into()),
                    cooldown: Some((10_000, 90_000, true)),
                    ..Default::default()
                },
            )]),
        }),
    );
    let lines = hover_bag(&mut s);
    let at = |needle: &str| {
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in {lines:?}"))
    };
    assert!(
        at("+1 Tick.") < at("[CD 2 m]"),
        "after the set block: {lines:?}"
    );
    assert!(
        at("[CD 2 m]") < at("\"Ticks.\""),
        "above the description: {lines:?}"
    );
    assert_eq!(at("\"Ticks.\""), lines.len() - 1);
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// A template source holds no instance, so no cooldown reaches the builder.
#[test]
fn a_template_source_draws_no_cooldown_line() {
    let mut s = script();
    put_blade(&mut s, Some(30_000));
    s.run("TT:SetOwner(Slot, 'ANCHOR_RIGHT') SetItemByIdForTest(TT, 8200)")
        .unwrap();
    let lines: Vec<String> = lines_of(&mut s).into_iter().map(|(t, _)| t).collect();
    assert_eq!(lines, ["Timed Blade"]);
}

/// `SetInventoryItem` queries the cooldown by the item's entry (`0x5331ec`): a worn trinket on a
/// cooldown, the keyring and the vault by their container slots; an inspected unit's record has
/// none.
#[test]
fn set_inventory_item_carries_the_worn_cooldown() {
    let mut s = script();
    s.set_item_template(8200, blade());
    let mut inv: InventorySlots = Default::default();
    inv[13] = Some(InvSlotView {
        item_id: 8200,
        name: Some("Timed Blade".into()),
        quality: 2,
        ..Default::default()
    });
    inv[14] = inv[13].clone();
    s.set_inventory_slots(inv);
    // Live id 13 runs 3 minutes, from 10 s; id 14 holds the same item, cold.
    s.set_worn_cooldowns(HashMap::from([(13, (10_000, 180_000, true))]));
    let hover = |s: &mut UiScript, call: &str| -> Vec<String> {
        s.run(&format!("TT:SetOwner(Slot, 'ANCHOR_RIGHT') {call}"))
            .unwrap();
        lines_of(s).into_iter().map(|(t, _)| t).collect()
    };
    assert_eq!(
        hover(&mut s, "TT:SetInventoryItem('player', 13)"),
        ["Timed Blade", "[CD 3 m]"]
    );
    assert_eq!(
        s.eval::<String>(
            "TT:SetOwner(Slot, 'ANCHOR_RIGHT') \
             local _, cd = TT:SetInventoryItem('player', 13) return tostring(cd)"
        )
        .unwrap(),
        "1",
        "the line sets the refresh flag"
    );
    assert_eq!(
        hover(&mut s, "TT:SetInventoryItem('player', 14)"),
        ["Timed Blade"]
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}

/// `SetAction`'s item leg passes the action's cooldown into the builder (`0x5324d5`) and answers
/// the flag from the line; an item the player does not hold has no instance.
#[test]
fn set_action_item_leg_carries_the_action_cooldown() {
    let mut s = script();
    put_blade(&mut s, None);
    let put = |s: &mut UiScript, id: u32, item: u32, cooldown| {
        s.set_action(
            id,
            Some(ActionSlot {
                kind: 0x80,
                action: item,
                ..Default::default()
            }),
        );
        s.set_action_state(
            id,
            Some(ActionState {
                usable: true,
                cooldown,
                ..Default::default()
            }),
        );
    };
    put(&mut s, 1, 8200, Some((10_000, 45_000, true)));
    put(&mut s, 2, 8200, None);
    s.set_item_template(8201, blade());
    put(&mut s, 3, 8201, Some((10_000, 45_000, true)));
    let hover = |s: &mut UiScript, id: u32| -> (Vec<String>, String) {
        let flag = s
            .eval::<String>(&format!(
                "TT:SetOwner(Slot, 'ANCHOR_RIGHT') return tostring(TT:SetAction({id}))"
            ))
            .unwrap();
        (lines_of(s).into_iter().map(|(t, _)| t).collect(), flag)
    };
    assert_eq!(
        hover(&mut s, 1),
        (vec!["Timed Blade".into(), "[CD 45 s]".into()], "1".into())
    );
    assert_eq!(hover(&mut s, 2), (vec!["Timed Blade".into()], "nil".into()));
    assert_eq!(
        hover(&mut s, 3),
        (vec!["Timed Blade".into()], "nil".into()),
        "an item not in the bags builds with p6 = 0"
    );
    assert!(s.take_errors().is_empty(), "{:?}", s.take_errors());
}
