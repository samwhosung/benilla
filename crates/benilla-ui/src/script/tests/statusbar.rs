//! StatusBar behaviour (`LoadXML` `0x782ef0`).

use super::common::script;
use crate::script::*;

#[test]
fn statusbar_methods_exist_only_on_statusbars() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        local bar = CreateFrame("StatusBar", "SbDuck")
        local plain = CreateFrame("Frame")
        -- Duck-typing: addons branch on `if frame.SetValue then` — a plain frame must say nil.
        return (type(bar.SetValue) == "function") and (plain.SetValue == nil)
            and (type(bar.SetMinMaxValues) == "function") and (plain.SetMinMaxValues == nil)
            and (type(bar.Show) == "function") -- base methods still reachable through the fallback
    "#,
        )
        .unwrap();
    assert!(ok);
}

#[test]
fn statusbar_value_clamps_and_minmax_swaps() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbClamp")
        b:SetMinMaxValues(0, 100)
        b:SetValue(250)
        assert(b:GetValue() == 100, "clamped to max")
        b:SetValue(-5)
        assert(b:GetValue() == 0, "clamped to min")
        -- A reversed pair is swapped (LoadXML `0x782ef0`'s behavior, one rule for both paths).
        b:SetMinMaxValues(80, 20)
        local mn, mx = b:GetMinMaxValues()
        assert(mn == 20 and mx == 80, "reversed pair swapped")
        assert(b:GetValue() == 20, "held value re-clamped into the new range")
    "#,
    )
    .unwrap();
}

#[test]
fn statusbar_setvalue_fires_onvaluechanged() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbEvt")
        b:SetMinMaxValues(0, 10)
        seen = {}
        b:SetScript("OnValueChanged", function(self, value)
            table.insert(seen, value)
            assert(self == b and arg1 == value, "handler-firing conventions carry the value")
        end)
        b:SetValue(4)
        b:SetValue(4)          -- no change, no fire
        b:SetMinMaxValues(0, 3) -- re-clamp 4 -> 3: a value change, fires
        assert(table.getn(seen) == 2 and seen[1] == 4 and seen[2] == 3,
               "fired once per actual change: " .. table.getn(seen))
    "#,
    )
    .unwrap();
}

#[test]
fn statusbar_bar_region_scales_by_fraction_on_extract() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbFill")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture(0.2, 0.7, 0.2)
        b:SetMinMaxValues(0, 100)
        b:SetValue(25)
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    // The bar-fill texture: a solid-color region whose rect is 25% of the frame width.
    let bar = quads
        .iter()
        .find(|q| {
            matches!(
                &q.content,
                QuadContent::Texture {
                    path: None,
                    color: Some(_),
                    ..
                }
            )
        })
        .expect("bar fill quad");
    let r = bar.rect.expect("bar rect resolved");
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 150.0, 100.0, 120.0),
        "horizontal fill: left-pinned, 25% of 200px"
    );

    // Vertical: fills bottom-up.
    s.run(r#"SbFill:SetOrientation("VERTICAL")"#).unwrap();
    let quads = s.extract();
    let bar = quads
        .iter()
        .find(|q| {
            matches!(
                &q.content,
                QuadContent::Texture {
                    path: None,
                    color: Some(_),
                    ..
                }
            )
        })
        .expect("bar fill quad");
    let r = bar.rect.expect("bar rect resolved");
    assert_eq!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 300.0, 100.0, 105.0),
        "vertical fill: bottom-pinned, 25% of 20px"
    );
}

/// The fill crops its art, never squeezes it (`0x770410`): `SetValue` sets the UV `u1` to the fill
/// fraction `(value - min) / (max - min)` and shrinks the quad by the same fraction.
#[test]
fn statusbar_bar_region_crops_its_texture_rather_than_stretching_it() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbCrop")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        b:SetMinMaxValues(0, 100)
        b:SetValue(25)
    "#,
    )
    .unwrap();
    let uv = |s: &UiScript| -> [f32; 4] {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(_),
                    tex_coords,
                    ..
                } => tex_coords.map(|tc| tc.edges()),
                _ => None,
            })
            .expect("the bar fill always carries its crop")
    };
    s.resolve();
    assert_eq!(
        uv(&s),
        [0.0, 0.25, 0.0, 1.0],
        "horizontal: u cropped to the fill fraction, v whole"
    );

    // Vertical fills bottom-up, so the art's bottom edge is pinned.
    s.run(r#"SbCrop:SetOrientation("VERTICAL")"#).unwrap();
    assert_eq!(
        uv(&s),
        [0.0, 1.0, 0.75, 1.0],
        "vertical: v cropped up from the bottom, u whole"
    );

    // An empty bar samples nothing; a full one samples everything.
    s.run("SbCrop:SetValue(0)").unwrap();
    assert_eq!(uv(&s), [0.0, 1.0, 1.0, 1.0], "empty");
    s.run("SbCrop:SetValue(100)").unwrap();
    assert_eq!(uv(&s), [0.0, 1.0, 0.0, 1.0], "full");
}

#[test]
fn statusbar_get_texture_returns_the_bar_region() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbTex")
        assert(b:GetStatusBarTexture() == nil, "no bar before one is set")
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        local t = b:GetStatusBarTexture()
        assert(t ~= nil and t == b:GetStatusBarTexture(), "stable region wrapper")
        b:SetStatusBarColor(1, 0, 0)
        local r, g, bb, a = b:GetStatusBarColor()
        assert(r == 1 and g == 0 and bb == 0 and a == 1)
    "#,
    )
    .unwrap();
}
