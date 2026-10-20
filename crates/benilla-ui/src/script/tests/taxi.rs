//! The taxi bindings stock `TaxiFrame.lua` reads over a pushed snapshot, and the node and close
//! intents.

use super::common::script;
use crate::script::*;

#[test]
fn taxi_snapshot_surfaces_and_intents_drain() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_taxi(Some(TaxiUiState {
        art: "Interface\\TaxiFrame\\TAXIMAP1".into(),
        nodes: vec![
            TaxiUiNode {
                name: "Crossroads, The Barrens".into(),
                node_type: TaxiNodeType::Current,
                pos: (0.5, 0.6),
                cost: 0,
                routes: vec![],
                num_routes: 0,
            },
            TaxiUiNode {
                name: "Orgrimmar, Durotar".into(),
                node_type: TaxiNodeType::Reachable,
                pos: (0.55, 0.8),
                cost: 110,
                routes: vec![[0.5, 0.6, 0.55, 0.8]],
                num_routes: 1,
            },
        ],
    }));

    s.run(
        r#"
        f = CreateFrame("Frame", "TaxiHost")
        f:SetWidth(316); f:SetHeight(352); f:SetPoint("CENTER", 0, 0)
        map = f:CreateTexture("TaxiMapTex", "OVERLAY")
        SetTaxiMap(map)

        assert(NumTaxiNodes() == 2)
        assert(TaxiNodeGetType(1) == "CURRENT")
        assert(TaxiNodeGetType(2) == "REACHABLE")
        assert(TaxiNodeGetType(3) == "NONE")           -- out of range hides, ref-style
        local x, y = TaxiNodePosition(2)
        assert(math.abs(x - 0.55) < 1e-6 and math.abs(y - 0.8) < 1e-6)
        assert(TaxiNodeName(2) == "Orgrimmar, Durotar")
        assert(TaxiNodeCost(2) == 110)
        TaxiNodeSetCurrent(2)                          -- faithful no-op
        assert(GetNumRoutes(1) == 0 and GetNumRoutes(2) == 1)
        assert(math.abs(TaxiGetSrcX(2, 1) - 0.5) < 1e-6)
        assert(math.abs(TaxiGetDestY(2, 1) - 0.8) < 1e-6)
        assert(UnitOnTaxi("player") == nil)          -- 1/nil, never a boolean

        TakeTaxiNode(2)
        CloseTaxiMap()
    "#,
    )
    .unwrap();

    assert_eq!(s.take_taxi_node(), vec![2]);
    assert!(s.take_taxi_close());
    assert!(!s.take_taxi_close(), "the close flag drains");

    s.resolve();
    assert!(
        s.extract().iter().any(|q| matches!(
            &q.content,
            QuadContent::Texture { path: Some(p), .. } if p == "Interface\\TaxiFrame\\TAXIMAP1"
        )),
        "the continent art draws on the SetTaxiMap target"
    );

    s.set_on_taxi(true);
    s.run(r#"assert(UnitOnTaxi("player") == 1)"#).unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Slot 0, a slot past the count and any slot of an empty list (no map built, count 0 from
/// `0x4db36d`): `TaxiNodePosition` and `TaxiNodeCost` raise `Invalid taxi node slot` (`0x84b008`)
/// on the unsigned compare of the slot less 1 with the count (`0x4dcbc5`, `0x4dcc63`), and
/// `TaxiNodeName` answers `INVALID` (`0x4dc388`, `0x84afc8`).
#[test]
fn out_of_range_taxi_slots_raise_or_answer_invalid() {
    let mut s = script();
    let node = TaxiUiNode {
        name: "Crossroads, The Barrens".into(),
        node_type: TaxiNodeType::Current,
        pos: (0.5, 0.6),
        ..Default::default()
    };
    for (map, slots) in [(true, [0, -1, 3]), (false, [0, 1, 3])] {
        s.set_taxi(map.then(|| TaxiUiState {
            art: String::new(),
            nodes: vec![node.clone(), node.clone()],
        }));
        for slot in slots {
            for verb in ["TaxiNodePosition", "TaxiNodeCost"] {
                let e = s
                    .run(&format!("{verb}({slot})"))
                    .expect_err("raises")
                    .to_string();
                assert!(
                    e.contains("Invalid taxi node slot"),
                    "map {map}, {verb}({slot}): {e}"
                );
            }
            assert_eq!(
                s.eval::<String>(&format!("return TaxiNodeName({slot})"))
                    .unwrap(),
                "INVALID",
                "map {map}, slot {slot}"
            );
        }
    }
    s.set_taxi(Some(TaxiUiState {
        art: String::new(),
        nodes: vec![node],
    }));
    assert_eq!(
        s.eval::<String>("return TaxiNodeName(1)").unwrap(),
        "Crossroads, The Barrens"
    );
}

/// `TaxiGetSrcX` to `TaxiGetDestY` read both arguments with a bare `lua_tonumber` and `_ftol`
/// (`0x4dcd9d`-`0x4dcdbb`): a non-number is 0, so no slot and a 0 answer, and 2.9 is 2.
#[test]
fn route_line_coordinates_read_their_arguments_as_bare_numbers() {
    let mut s = script();
    s.set_taxi(Some(TaxiUiState {
        art: String::new(),
        nodes: vec![
            TaxiUiNode {
                name: "A".into(),
                node_type: TaxiNodeType::Current,
                ..Default::default()
            },
            TaxiUiNode {
                name: "B".into(),
                routes: vec![[0.25, 0.5, 0.75, 1.0]],
                num_routes: 1,
                ..Default::default()
            },
        ],
    }));
    s.run(
        r#"
        assert(TaxiGetSrcX(nil, 1) == 0 and TaxiGetSrcY("x", {}) == 0 and TaxiGetDestX() == 0)
        assert(TaxiGetDestY(2, nil) == 0)
        assert(TaxiGetSrcX(2.9, 1.5) == 0.25 and TaxiGetSrcY("2", "1") == 0.5)
        assert(TaxiGetDestX(2, 1) == 0.75 and TaxiGetDestY(2, 1) == 1)
    "#,
    )
    .unwrap();
}

/// `SetTaxiMap` does nothing before a map's texture has loaded (`0x4dca5c`), whatever it is
/// handed; with one it resolves its receiver as a member function does: a non-table raises
/// `0x847ef8`, a table with no object `0x847ec0`, and an object that is no Texture `0x847e98`.
#[test]
fn set_taxi_map_tests_the_map_texture_before_its_receiver() {
    let mut s = script();
    s.run("SetTaxiMap(nil) SetTaxiMap({}) SetTaxiMap(5)")
        .unwrap();
    s.set_taxi(Some(TaxiUiState {
        art: "Interface\\TaxiFrame\\TAXIMAP1".into(),
        nodes: vec![],
    }));
    for (arg, want) in [
        (
            "nil",
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ),
        ("{}", "Attempt to find 'this' in non-framescript object"),
        (
            "CreateFrame(\"Frame\")",
            "Wrong object type for member function",
        ),
    ] {
        let e = s
            .run(&format!("SetTaxiMap({arg})"))
            .expect_err("raises")
            .to_string();
        assert!(e.contains(want), "SetTaxiMap({arg}): {e}");
    }
}
