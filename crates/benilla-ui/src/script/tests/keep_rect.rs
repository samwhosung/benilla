//! The drain's keep test (`0x768d20`): a resolve whose four new edges all moved less than 1e-5
//! (`[0x80c5c8]`, `0x768d6e`–`0x768dc0`) against the cached rect keeps that rect and returns at
//! `0x768dc2`, before the store and `ApplyRect`, so nothing moves: no rect, no `OnSizeChanged`, no
//! hit rect, no dependent. The threshold is in the reference's layout units, the screen diagonal
//! as 1: at 1024×768 the layout unit is 1280, so 0.0128 here.

use super::common::script;
use crate::script::UiScript;

/// 1e-5 layout units at 1024×768, in ours.
const KEEP: f64 = 0.0128;

/// A mouse-enabled panel spanned by two anchors, `Panel` at `(100, 100)`–`(300, 150)`, with a
/// child pinned to its top right and an `OnSizeChanged` counter.
fn panel() -> UiScript {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        fires = 0
        Panel = CreateFrame("Frame", "Panel")
        Panel:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        Panel:SetPoint("TOPRIGHT", nil, "BOTTOMLEFT", 300, 150)
        Panel:EnableMouse(true)
        Panel:SetScript("OnSizeChanged", function() fires = fires + 1 end)
        Pin = CreateFrame("Frame", "Pin")
        Pin:SetPoint("BOTTOMLEFT", Panel, "TOPRIGHT", 0, 0)
        Pin:SetWidth(10); Pin:SetHeight(10)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1, "the birth");
    s
}

fn right(s: &mut UiScript, name: &str) -> f64 {
    s.eval::<f64>(&format!("return {name}:GetRight()")).unwrap()
}

fn set_right(s: &mut UiScript, x: f64) {
    s.run(&format!(
        r#"Panel:SetPoint("TOPRIGHT", nil, "BOTTOMLEFT", {x}, 150)"#
    ))
    .unwrap();
    s.resolve();
}

/// An anchor offset jittered by 1e-6 layout units keeps the cached rect: the right edge reads
/// back unmoved, `OnSizeChanged` stays quiet though the width moved past `ApplyRect`'s 2⁻²², the
/// hit rect does not grow over the cursor, and the frame pinned to it does not move.
#[test]
fn a_jitter_under_the_keep_threshold_keeps_the_rect() {
    let mut s = panel();
    set_right(&mut s, 300.0 + KEEP / 10.0);
    assert_eq!(right(&mut s, "Panel"), 300.0, "the cached rect stands");
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 1, "no ApplyRect");
    assert_eq!(
        s.hit_test_name(300.0006, 120.0),
        None,
        "the hit rect is the kept rect's"
    );
    assert_eq!(
        right(&mut s, "Pin"),
        310.0,
        "a kept rect moves no dependent"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// An edge moved just over the threshold applies, and one just under keeps.
#[test]
fn a_move_just_over_the_threshold_applies() {
    let mut s = panel();
    set_right(&mut s, 300.0125);
    assert_eq!(right(&mut s, "Panel"), 300.0, "0.0125 is under 0.0128");
    set_right(&mut s, 300.0131);
    let r = right(&mut s, "Panel");
    assert!((r - 300.0131).abs() < 1e-4, "0.0131 applies: {r}");
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2, "ApplyRect ran");
    assert!((right(&mut s, "Pin") - 310.0131).abs() < 1e-4);
    assert_eq!(
        s.hit_test_name(300.0128, 120.0).as_deref(),
        Some("Panel"),
        "the applied rect's hit rect"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The comparison is against the rect last stored, not the last one computed, so a slow drift
/// accumulates until it crosses the threshold and then lands whole.
#[test]
fn a_slow_drift_accumulates_and_applies_once_it_crosses() {
    let mut s = panel();
    for x in [300.004, 300.008, 300.012] {
        set_right(&mut s, x);
        assert_eq!(right(&mut s, "Panel"), 300.0, "{x}: under the threshold");
    }
    set_right(&mut s, 300.016);
    let r = right(&mut s, "Panel");
    assert!(
        (r - 300.016).abs() < 1e-4,
        "0.016 from the cached edge: {r}"
    );
    assert_eq!(s.eval::<i64>("return fires").unwrap(), 2);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Regions resolve through the same `0x768d20`, so a texture keeps its rect too.
#[test]
fn a_region_keeps_its_rect_under_the_threshold() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Holder:SetAllPoints(nil)
        Tex = Holder:CreateTexture("Tex", "ARTWORK")
        Tex:SetPoint("BOTTOMLEFT", Holder, "BOTTOMLEFT", 100, 100)
        Tex:SetPoint("TOPRIGHT", Holder, "BOTTOMLEFT", 300, 150)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(right(&mut s, "Tex"), 300.0);
    s.run(r#"Tex:SetPoint("TOPRIGHT", Holder, "BOTTOMLEFT", 300.005, 150)"#)
        .unwrap();
    s.resolve();
    assert_eq!(right(&mut s, "Tex"), 300.0, "kept");
    s.run(r#"Tex:SetPoint("TOPRIGHT", Holder, "BOTTOMLEFT", 300.02, 150)"#)
        .unwrap();
    s.resolve();
    let r = right(&mut s, "Tex");
    assert!((r - 300.02).abs() < 1e-4, "applied: {r}");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// An effective-scale move clears the cached rect's resolved bit (`0x768360`, reached from the
/// scale cascade `0x76acd5`): the next resolve stores the rect without the keep test, so a frame
/// holding a kept rect lands on its exact edge, and so do its regions, which the cascade reaches
/// (`0x76ad06`).
#[test]
fn a_scale_move_stores_the_rect_without_the_keep_test() {
    let mut s = panel();
    s.run(
        r#"
        Mark = Panel:CreateTexture("Mark", "ARTWORK")
        Mark:SetPoint("BOTTOMLEFT", Panel, "BOTTOMLEFT", 0, 0)
        Mark:SetWidth(10); Mark:SetHeight(10)
        Panel:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100.004, 100)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return Mark:GetLeft()").unwrap(),
        100.0,
        "the region reads the kept rect"
    );
    assert_eq!(
        s.eval::<f64>("return Panel:GetLeft()").unwrap(),
        100.0,
        "kept"
    );
    // 1 + 2⁻²⁰: past the cascade's 2⁻²² gate, and moving the left edge far less than the keep's.
    s.run("Panel:SetScale(1.00000095367431640625)").unwrap();
    s.resolve();
    let left = s.eval::<f64>("return Panel:GetLeft()").unwrap();
    assert!(
        (left - 100.004).abs() < 1e-4,
        "the scale move re-stores the computed rect: {left}"
    );
    let left = s.eval::<f64>("return Mark:GetLeft()").unwrap();
    assert!((left - 100.004).abs() < 1e-4, "and the region's: {left}");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A FontString's new measure is benilla's moment for the reference's `SetText`/`SetFont`, which
/// clear the resolved bit (`0x771ea4`, `0x7715e0`): its rect is stored without the keep test.
#[test]
fn a_new_measure_stores_the_fontstring_rect_without_the_keep_test() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        Holder = CreateFrame("Frame", "Holder")
        Holder:SetAllPoints(nil)
        Label = Holder:CreateFontString("Label", "ARTWORK"); Label:SetFont("Fonts\\FRIZQT__.TTF", 12)
        Label:SetText("abc")
        Label:SetPoint("BOTTOMLEFT", Holder, "BOTTOMLEFT", 100, 100)
    "#,
    )
    .unwrap();
    s.resolve();
    let r = s.fontstrings_needing_measure()[0].clone();
    s.set_measured_text_unwrapped(&[(r.id, 50.0, 12.0, r.key)]);
    s.resolve();
    assert_eq!(right(&mut s, "Label"), 150.0);
    s.run(r#"Label:SetPoint("BOTTOMLEFT", Holder, "BOTTOMLEFT", 100.004, 100)"#)
        .unwrap();
    s.resolve();
    assert_eq!(right(&mut s, "Label"), 150.0, "kept");
    s.run(r#"Label:SetText("abd")"#).unwrap();
    s.resolve();
    let r = s.fontstrings_needing_measure()[0].clone();
    s.set_measured_text_unwrapped(&[(r.id, 50.001, 12.0, r.key)]);
    s.resolve();
    let right = right(&mut s, "Label");
    assert!(
        (right - 150.005).abs() < 1e-4,
        "the new measure re-stores the computed rect: {right}"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
