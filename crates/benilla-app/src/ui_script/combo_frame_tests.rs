//! The combo-point dots — the reference's own `ComboFrame.xml`/`.lua`,
//! executed off the player's chain since 1751's tenth window, driven through the real loader: the show/hide edges, the per-point highlight/shine fade chain, the
//! "only newly-earned points flare" rule that `COMBO_FRAME_LAST_NUM_POINTS` exists to enforce, and
//! the two gates that live in `GetComboPoints` rather than in this Lua — rogue-or-druid only, and
//! the points must be banked on the CURRENT target.

use benilla_ui::script::{PlayerReqState, UiScript, UnitState};

/// The unit the points get banked on in these tests, and a second one to re-target to.
const MOB_A: u64 = 0xF130_0000_0000_0001;
const MOB_B: u64 = 0xF130_0000_0000_0002;

use super::test_ui::load_ui as load_xml;

/// The manifest order this file actually ships in: `UiPanels.xml` for `UIFrameFade`, and the
/// reference's own `TargetFrame.xml` for the `TargetFrame` the dots anchor to — `relativeTo` is
/// resolved at LOAD, so that file has to precede this one (1751).
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

/// Put the player in a class that can see combo points, with `guid` selected — the state
/// `GetComboPoints` demands before it will report anything at all.
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

/// Run every queued fade past its longest leg (0.4 s highlight → 0.3 s shine in → 0.4 s shine out).
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

/// Zero points hides the frame; the first point shows it and lights exactly one dot; the count
/// rising lights the rest; the drop back to zero hides it again. The alpha readings run the fade to
/// completion first, since the highlight arrives via `UIFrameFade`, not a straight `SetAlpha`.
#[test]
fn combo_frame_follows_the_point_count() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A); // rogue, mob A selected

    // Nothing banked: hidden, and the event on zero is a no-op that keeps it hidden.
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(!shown(&mut s), "no points ⇒ hidden");

    // One point — a rogue's first builder.
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

    // Up to five: every dot lit, none left behind.
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

    // The spend/expiry edge: back to zero hides the frame. This is the one the wire depends on —
    // the server clears the byte on its own, so the falling edge is the only thing that ever takes
    // the dots down.
    s.set_combo_points(0, 0);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(!shown(&mut s), "zero points ⇒ hidden again");
}

/// A repaint at an UNCHANGED count must not re-flare the dots that were already lit — the whole
/// job of `COMBO_FRAME_LAST_NUM_POINTS`. `PLAYER_TARGET_CHANGED` is registered alongside the
/// combo event, so re-targeting mid-window fires exactly this repaint.
#[test]
fn a_repaint_at_the_same_count_does_not_re_flare() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A);
    s.set_combo_points(2, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    settle(&mut s);

    // Both settled: highlights up, shines burned out.
    let shine: f64 = s.eval("return ComboPoint2Shine:GetAlpha()").unwrap();
    assert_eq!(shine, 0.0, "the shine fades back out");

    // A re-select of the SAME unit at the same count: no new fade is queued for either dot.
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

/// **A warrior sees nothing** — `GetComboPoints 0x51a190`'s class gate (`cmp al,4 / cmp al,0xb`).
/// The server really does bank a point for them on a victim's dodge, and that byte really does
/// reach us: it is what greys Overpower through the usable walk's leg 5. It just never reaches the
/// UI. Every class outside {rogue, druid} takes the same path; the wire state here is identical to
/// the rogue ladder's first rung, so only the class differs.
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

    // A druid is on the other side of the same gate (class 11 — cat form's builders).
    play_as(&mut s, 11, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    assert!(shown(&mut s), "a druid sees the same banked point");
}

/// **Combo points are per target** — `0x51a190` compares `PLAYER_FIELD_COMBO_TARGET` against the
/// current-target GUID global and pushes 0 on a mismatch. So re-targeting empties the dots without
/// the server touching the count, and selecting the banked unit again refills them. Losing the
/// target entirely (guid 0) empties them too.
#[test]
fn re_targeting_empties_the_dots_without_the_count_moving() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = load_combo_frame();
    play_as(&mut s, 4, MOB_A);
    s.set_combo_points(3, MOB_A);
    s.fire_event("PLAYER_COMBO_POINTS", vec![]);
    settle(&mut s);
    assert!(shown(&mut s), "three points on the selected mob ⇒ shown");

    // Select a different mob. The wire count is untouched — only the reader's answer changes.
    play_as(&mut s, 4, MOB_B);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "points banked elsewhere ⇒ no dots");
    let points: i64 = s.eval("return GetComboPoints()").unwrap();
    assert_eq!(points, 0, "the binding hides them, the wire still has them");

    // Select the banked mob again: all three come straight back.
    play_as(&mut s, 4, MOB_A);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    settle(&mut s);
    assert!(shown(&mut s), "re-selecting the banked mob refills them");
    assert!(highlight_alpha(&mut s, 3) > 0.99, "all three, not just one");

    // And no target at all is a mismatch like any other.
    s.set_unit("target", None);
    s.fire_event("PLAYER_TARGET_CHANGED", vec![]);
    assert!(!shown(&mut s), "no target ⇒ no dots");
}
