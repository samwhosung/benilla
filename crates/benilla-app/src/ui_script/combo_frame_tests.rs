//! The stock combo-point dots (`ComboFrame.xml`). `GetComboPoints` holds two gates the Lua does
//! not: rogue or druid only, and the points must be banked on the current target.

use benilla_ui::script::{PlayerReqState, UiScript, UnitState};

const MOB_A: u64 = 0xF130_0000_0000_0001;
const MOB_B: u64 = 0xF130_0000_0000_0002;

use super::test_ui::load_ui as load_xml;

/// `UIFrameFade` comes with UIParent.xml, and TargetFrame.xml loads first because the dots
/// anchor `relativeTo="TargetFrame"`, resolved at load.
fn load_combo_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitPopup.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, "Interface\\FrameXML\\BuffFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\UnitFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\CombatFeedback.xml");
    load_xml(&s, "Interface\\FrameXML\\PlayerFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PartyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\TargetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\PetFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\ComboFrame.xml");
    s.resolve();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

fn play_as(s: &mut UiScript, class_id: u32, target: u64) {
    s.set_player_req_state(PlayerReqState {
        class_id,
        ..Default::default()
    });
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            guid: target,
            ..Default::default()
        }),
    );
}

/// Run 2 s of fades, past the 0.4 + 0.3 + 0.4 s highlight and shine chain (`ComboFrame.lua:3`).
fn settle(s: &mut UiScript) {
    s.eval::<()>("for i = 1, 40 do UIFrameFadeUpdate(0.05) end")
        .unwrap();
}

fn shown(s: &mut UiScript) -> bool {
    s.eval("return ComboFrame:IsShown()").unwrap()
}

fn highlight_alpha(s: &mut UiScript, i: u32) -> f64 {
    s.eval(&format!("return ComboPoint{i}Highlight:GetAlpha()"))
        .unwrap()
}

/// The highlight arrives through `UIFrameFade`, so every alpha read settles the fades first.
#[test]
fn combo_frame_follows_the_point_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A); // rogue

    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(!shown(&mut s), "no points ⇒ hidden");

    s.set_combo_points(1, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(shown(&mut s), "one point ⇒ shown");

    settle(&mut s);
    let lit = highlight_alpha(&mut s, 1);
    assert!(lit > 0.99, "point 1 lit, got alpha {lit}");
    assert_eq!(
        highlight_alpha(&mut s, 2),
        0.0,
        "point 2 stays dark at one combo point"
    );

    s.set_combo_points(5, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    settle(&mut s);
    let all_lit: bool = s
        .eval(
            r#"
            for i = 1, MAX_COMBO_POINTS do
                if getglobal("ComboPoint" .. i .. "Highlight"):GetAlpha() < 0.99 then
                    return false
                end
            end
            return true
        "#,
        )
        .unwrap();
    assert!(all_lit, "all five dots lit at five points");

    // The server clears the count on spend or expiry; that falling edge alone takes the dots down.
    s.set_combo_points(0, 0);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(!shown(&mut s), "zero points ⇒ hidden again");
}

/// `PLAYER_TARGET_CHANGED` repaints too; `COMBO_FRAME_LAST_NUM_POINTS` stops lit dots re-flaring.
#[test]
fn a_repaint_at_the_same_count_does_not_re_flare() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A);
    s.set_combo_points(2, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    settle(&mut s);

    let shine: f64 = s.eval("return ComboPoint2Shine:GetAlpha()").unwrap();
    assert_eq!(shine, 0.0, "the shine fades back out");

    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    let fading: bool = s
        .eval("return UIFrameIsFading(ComboPoint1Highlight) ~= nil")
        .unwrap();
    assert!(!fading, "an unchanged count re-flares nothing");
    assert!(
        highlight_alpha(&mut s, 2) > 0.99,
        "and leaves the lit dots lit"
    );
}

/// `GetComboPoints` (`0x51a190`) answers 0 outside rogue and druid (classes 4 and 11), though
/// the server banks a point for a warrior's Overpower on a dodge.
#[test]
fn a_warriors_overpower_point_lights_no_dot() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 1, MOB_A); // warrior
    s.set_combo_points(1, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(!shown(&mut s), "a warrior's banked point shows no dots");
    let points: i64 = s.eval("return GetComboPoints()").unwrap();
    assert_eq!(points, 0, "and the binding itself reports zero");

    play_as(&mut s, 11, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(shown(&mut s), "a druid sees the same banked point");
}

/// `0x51a190` answers 0 unless `PLAYER_FIELD_COMBO_TARGET` is the current target, so the dots
/// follow the selection while the banked count stays put.
#[test]
fn re_targeting_empties_the_dots_without_the_count_moving() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A);
    s.set_combo_points(3, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    settle(&mut s);
    assert!(shown(&mut s), "three points on the selected mob ⇒ shown");

    play_as(&mut s, 4, MOB_B);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "points banked elsewhere ⇒ no dots");
    let points: i64 = s.eval("return GetComboPoints()").unwrap();
    assert_eq!(points, 0, "the binding hides them, the wire still has them");

    play_as(&mut s, 4, MOB_A);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    settle(&mut s);
    assert!(shown(&mut s), "re-selecting the banked mob refills them");
    assert!(highlight_alpha(&mut s, 3) > 0.99, "all three, not just one");

    s.set_unit("target", None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "no target ⇒ no dots");
}
