//! One anchor slot per point, as the reference keeps them (`anchorPoints[9]`, frame `+0x28`):
//! `SetPoint`'s unchanged-anchor test (`0x767c70`) and `GetPoint`'s walk in point order
//! (`0x7a2340`).

use super::common::script;
use crate::script::{Model, UiScript};

/// The mutation epoch, bumped by every write into the layout read set.
fn epoch(s: &UiScript) -> u64 {
    s.lua()
        .app_data_ref::<Model>()
        .expect("model app_data")
        .layout_epoch
}

/// A frame and a texture on it, each with two anchors set out of point order.
fn setup(s: &mut UiScript) {
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Slots")
        f:SetPoint("BOTTOMRIGHT", nil, "BOTTOMRIGHT", -20, 30)
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 10, -10)
        t = f:CreateTexture(nil, "ARTWORK")
        t:ClearAllPoints()
        t:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", -4, 4)
        t:SetPoint("TOPLEFT", f, "TOPLEFT", 4, -4)
        "#,
    )
    .expect("setup");
    s.resolve();
}

/// Re-applying an anchor that is not the last one set matches the one held for its point, so the
/// call returns before the store and the queue (`0x767ca9`..`0x767cde` → `0x767d5e`).
#[test]
fn re_setting_an_earlier_anchor_touches_no_layout() {
    let mut s = script();
    setup(&mut s);
    for (who, call) in [
        (
            "frame",
            r#"f:SetPoint("BOTTOMRIGHT", nil, "BOTTOMRIGHT", -20, 30)"#,
        ),
        (
            "region",
            r#"t:SetPoint("BOTTOMRIGHT", f, "BOTTOMRIGHT", -4, 4)"#,
        ),
    ] {
        let before = epoch(&s);
        s.run(call).expect("re-set");
        assert_eq!(epoch(&s), before, "{who}: an unchanged anchor is a no-op");
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Both offsets compare within 2⁻²² of the reference's layout unit (`[0x8029d4]`), so float noise
/// in a computed offset changes nothing and is not stored; a step past it is a change. At 800×600
/// the unit is 1280 FrameXML units, so the threshold is about 3.05e-4.
#[test]
fn an_offset_within_the_threshold_is_unchanged() {
    let mut s = script();
    setup(&mut s);
    for (who, noisy, getter) in [
        (
            "frame",
            r#"f:SetPoint("TOPLEFT", nil, "TOPLEFT", 10.0001, -10.0001)"#,
            "local _, _, _, x = f:GetPoint(1); return x",
        ),
        (
            "region",
            r#"t:SetPoint("TOPLEFT", f, "TOPLEFT", 4.0001, -4.0001)"#,
            "local _, _, _, x = t:GetPoint(1); return x",
        ),
    ] {
        let before = epoch(&s);
        let x0: f64 = s.eval(getter).unwrap();
        s.run(noisy).expect("noisy re-set");
        assert_eq!(
            epoch(&s),
            before,
            "{who}: noise under the threshold is a no-op"
        );
        let x1: f64 = s.eval(getter).unwrap();
        assert_eq!(
            x1, x0,
            "{who}: and the held offset is kept, not overwritten"
        );
    }
    let before = epoch(&s);
    s.run(r#"f:SetPoint("TOPLEFT", nil, "TOPLEFT", 10.001, -10)"#)
        .unwrap();
    assert_ne!(epoch(&s), before, "a step past the threshold is a change");
    let x: f64 = s
        .eval("local _, _, _, x = f:GetPoint(1); return x")
        .unwrap();
    assert_eq!(x as f32, 10.001_f32);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `GetPoint(n)` answers the n-th occupied slot by point id, TOPLEFT 0 through BOTTOMRIGHT 8 (the
/// ids of the name table `0x811a38`, whose entries run alphabetically), whatever order the anchors
/// were set in (`0x7a23f5`..`0x7a240b`).
#[test]
fn get_point_counts_in_point_order() {
    let mut s = script();
    setup(&mut s);
    for who in ["f", "t"] {
        let (p1, p2): (String, String) = s
            .eval(&format!("return ({who}:GetPoint(1)), ({who}:GetPoint(2))"))
            .unwrap();
        assert_eq!(
            (p1.as_str(), p2.as_str()),
            ("TOPLEFT", "BOTTOMRIGHT"),
            "{who}"
        );
        let p: String = s.eval(&format!("return ({who}:GetPoint())")).unwrap();
        assert_eq!(p, "TOPLEFT", "{who}: the default is the first");
    }
    // SetAllPoints, then a TOPLEFT re-set: the reference still answers TOPLEFT first.
    let (p1, p2): (String, String) = s
        .eval(
            r#"
            local g = CreateFrame("Frame")
            g:SetAllPoints(f)
            g:SetPoint("TOPLEFT", f, "TOPLEFT", 1, -1)
            return (g:GetPoint(1)), (g:GetPoint(2))
            "#,
        )
        .unwrap();
    assert_eq!((p1.as_str(), p2.as_str()), ("TOPLEFT", "BOTTOMRIGHT"));
}

/// `n` is `lua_isnumber`-tested and truncated (`0x6f34d0`, `0x40a2b0`); an index at or below the
/// first, or a non-number, answers the first anchor. Past the last, no value is pushed
/// (`0x7a240d`).
#[test]
fn get_point_index_and_past_the_last() {
    let mut s = script();
    setup(&mut s);
    for who in ["f", "t"] {
        let n: i64 = s
            .eval(&format!(
                "local function n(...) return arg.n end; return n({who}:GetPoint(3))"
            ))
            .unwrap();
        assert_eq!(n, 0, "{who}: past the last anchor answers nothing");
        for arg in ["0", "-4", "{}", "'x'", "1.9"] {
            let p: String = s.eval(&format!("return ({who}:GetPoint({arg}))")).unwrap();
            assert_eq!(p, "TOPLEFT", "{who}:GetPoint({arg})");
        }
        for arg in ["2", "'2'", "2.5"] {
            let p: String = s.eval(&format!("return ({who}:GetPoint({arg}))")).unwrap();
            assert_eq!(p, "BOTTOMRIGHT", "{who}:GetPoint({arg})");
        }
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `GetNumPoints` counts the occupied slots (`0x7a2311`..`0x7a231b`): a re-set point keeps one.
#[test]
fn get_num_points_counts_occupied_slots() {
    let mut s = script();
    setup(&mut s);
    s.run(
        r#"
        f:SetPoint("TOPLEFT", nil, "TOPLEFT", 50, -50)
        t:SetPoint("TOPLEFT", f, "TOPLEFT", 8, -8)
        "#,
    )
    .unwrap();
    let (nf, nt): (i64, i64) = s.eval("return f:GetNumPoints(), t:GetNumPoints()").unwrap();
    assert_eq!((nf, nt), (2, 2));
    let n: i64 = s
        .eval("f:ClearAllPoints(); return f:GetNumPoints()")
        .unwrap();
    assert_eq!(n, 0);
}

/// The compare runs on the stored values, each offset quantized to `f32` layout units as the
/// binding stores it (`0x7a28a3`..`0x7a28e3`), not on the raw difference. At 1280×1024 (aspect
/// 1.25, about 1229.4 FrameXML units per layout unit) three `f32` ulps above 650 move the stored
/// value by 2⁻²² or more, though the raw step (1.83e-4) is under the threshold's 2.93e-4.
#[test]
fn the_threshold_compares_the_quantized_stored_offsets() {
    let mut s = script();
    s.set_screen_size(1280.0, 1024.0);
    s.run(r#"f = CreateFrame("Frame"); f:SetPoint("TOPLEFT", nil, "TOPLEFT", 650, 0)"#)
        .unwrap();
    s.resolve();
    let before = epoch(&s);
    s.run(r#"f:SetPoint("TOPLEFT", nil, "TOPLEFT", 650.0001831054688, 0)"#)
        .unwrap();
    assert_ne!(
        epoch(&s),
        before,
        "the stored offsets differ by 2^-22 or more"
    );
    let x: f64 = s
        .eval("local _, _, _, x = f:GetPoint(1); return x")
        .unwrap();
    crate::assert_near!(x, 650.0001831054688);

    // And the other way: five ulps above 700 (a raw 3.05e-4, past the raw threshold) store
    // within 2^-22 of 700's value, so the call is a no-op.
    s.run(r#"f:SetPoint("TOPLEFT", nil, "TOPLEFT", 700, 0)"#)
        .unwrap();
    s.resolve();
    let before = epoch(&s);
    s.run(r#"f:SetPoint("TOPLEFT", nil, "TOPLEFT", 700.0003051757812, 0)"#)
        .unwrap();
    assert_eq!(epoch(&s), before, "the stored offsets are within 2^-22");
}
