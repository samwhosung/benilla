//! The stock taxi map (`TaxiFrame.xml`) fed a two-node snapshot shaped as `crate::ui_taxi`'s feed
//! pushes it: `TAXIMAP_OPENED` shows it and builds the node buttons, a click takes the node, and
//! `TAXIMAP_CLOSED` hides it.

use benilla_ui::script::{ScriptValue, TaxiNodeType, TaxiUiNode, TaxiUiState, UiScript};

use super::test_ui::load_ui as load_xml;

/// The taxi window and its dependencies.
fn taxi_script() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml"); // TaxiNodeOnButtonEnter's tooltip
    load_xml(&s, "Interface\\FrameXML\\UIErrorsFrame.xml");
    // UIErrorsFrame takes `DrawOneHopLines`' refusal; a missing `ERR_TAXINOPATHS` draws blank.
    // Without UIPanelTemplates, `TaxiCloseButton` loses its `UIPanelCloseButton` handler.
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.lua");
    load_xml(&s, "Interface\\FrameXML\\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\TaxiFrame.xml");
    s
}

/// Seat the flight master as "npc": the label reads `UnitName("npc")` (`TaxiFrame.lua:27`).
fn seat_flight_master(s: &mut UiScript, name: &str) {
    let npc = benilla_ui::script::UnitState {
        name: Some(name.into()),
        ..Default::default()
    };
    s.set_unit("npc", Some(npc));
}

/// Stormwind (Current) and the Sentinel Hill hop (Reachable, 110 copper, one route), as
/// `ui_taxi`'s `build_nodes` builds them.
fn menu() -> TaxiUiState {
    TaxiUiState {
        art: "Interface\\TaxiFrame\\TAXIMAP0".into(),
        nodes: vec![
            TaxiUiNode {
                name: "Stormwind, Elwynn".into(),
                node_type: TaxiNodeType::Current,
                pos: (0.43, 0.33),
                cost: 0,
                routes: vec![],
            },
            TaxiUiNode {
                name: "Sentinel Hill, Westfall".into(),
                node_type: TaxiNodeType::Reachable,
                pos: (0.41, 0.25),
                cost: 110,
                routes: vec![[0.43, 0.33, 0.41, 0.25]],
            },
        ],
    }
}

#[test]
fn shipped_taxi_frame_drives_end_to_end() {
    benilla_formats::wow_data_or_skip!();
    let mut s = taxi_script();

    assert!(!s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());

    s.set_taxi(Some(menu()));
    seat_flight_master(&mut s, "Dungar Longdrink");
    s.fire_event(
        "TAXIMAP_OPENED",
        vec![ScriptValue::Str("Dungar Longdrink".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<String>("return TaxiMerchant:GetText()").unwrap(),
        "Dungar Longdrink"
    );

    // Node buttons are created on demand, one per node (`TaxiFrame.lua:39`): no third exists.
    assert!(s.eval::<bool>("return TaxiButton1:IsVisible()").unwrap());
    assert!(s.eval::<bool>("return TaxiButton2:IsVisible()").unwrap());
    assert!(s.eval::<bool>("return TaxiButton3 == nil").unwrap());
    assert!(s.eval::<bool>("return TaxiButton50 == nil").unwrap());

    // A click on node 2, Sentinel Hill, calls `TakeTaxiNode`.
    s.resolve();
    let (cx, cy) = s
        .eval::<(f32, f32)>("return TaxiButton2:GetCenter()")
        .unwrap();
    s.mouse_button(cx, cy, "LeftButton", true);
    s.mouse_button(cx, cy, "LeftButton", false);
    assert_eq!(s.take_taxi_node(), vec![2]);
    assert!(s.take_taxi_node().is_empty(), "drained");

    s.set_taxi(None);
    s.fire_event("TAXIMAP_CLOSED", vec![]);
    assert!(!s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// With no reachable node, `DrawOneHopLines` posts `ERR_TAXINOPATHS` and hides the window
/// (`TaxiFrame.lua:161`).
#[test]
fn no_single_hop_destination_posts_the_error_and_closes() {
    benilla_formats::wow_data_or_skip!();
    let mut s = taxi_script();
    s.set_taxi(Some(TaxiUiState {
        art: "Interface\\TaxiFrame\\TAXIMAP0".into(),
        nodes: vec![TaxiUiNode {
            name: "Stormwind, Elwynn".into(),
            node_type: TaxiNodeType::Current,
            pos: (0.43, 0.33),
            cost: 0,
            routes: vec![],
        }],
    }));
    seat_flight_master(&mut s, "Dungar Longdrink");
    s.fire_event(
        "TAXIMAP_OPENED",
        vec![ScriptValue::Str("Dungar Longdrink".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(!s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());
    s.resolve();
    let quads = s.extract();
    let has_refusal = quads.iter().any(|q| match &q.content {
        benilla_ui::script::QuadContent::Text { text: Some(t), .. } => {
            t.contains("don\u{2019}t know any flight locations")
        }
        _ => false,
    });
    assert!(
        has_refusal,
        "ERR_TAXINOPATHS posted to UIErrorsFrame: {quads:?}"
    );
}

/// The close button hides the window, whose OnHide calls `CloseTaxiMap()` (`TaxiFrame.xml:167`).
#[test]
fn close_button_queues_the_intent_and_hides() {
    benilla_formats::wow_data_or_skip!();
    let mut s = taxi_script();
    s.set_taxi(Some(menu()));
    seat_flight_master(&mut s, "Dungar Longdrink");
    s.fire_event(
        "TAXIMAP_OPENED",
        vec![ScriptValue::Str("Dungar Longdrink".into())],
    );
    assert!(s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());

    // `TaxiCloseButton` is a plain `UIPanelCloseButton` (`TaxiFrame.xml:133`).
    s.run("TaxiCloseButton:Click()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.take_taxi_close());
    assert!(!s.eval::<bool>("return TaxiFrame:IsVisible()").unwrap());
}
