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
            },
            TaxiUiNode {
                name: "Orgrimmar, Durotar".into(),
                node_type: TaxiNodeType::Reachable,
                pos: (0.55, 0.8),
                cost: 110,
                routes: vec![[0.5, 0.6, 0.55, 0.8]],
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
        assert(UnitOnTaxi("player") == nil)          -- 1/nil, never a boolean (2043)

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
