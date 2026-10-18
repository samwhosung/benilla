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

/// `0x783270` keeps the low bound only when it is not above the high one, else takes the high one:
/// a reversed pair collapses onto `max`, never swaps. `SetValue` (`0x7832d0`) clamps.
#[test]
fn statusbar_value_clamps_and_a_reversed_pair_collapses_onto_max() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbClamp")
        b:SetMinMaxValues(0, 100)
        b:SetValue(250)
        assert(b:GetValue() == 100, "clamped to max")
        b:SetValue(-5)
        assert(b:GetValue() == 0, "clamped to min")
        b:SetMinMaxValues(80, 20)
        local mn, mx = b:GetMinMaxValues()
        assert(mn == 20 and mx == 20, "the low bound is the max: " .. mn .. ", " .. mx)
        assert(b:GetValue() == 20, "held value re-clamped into the new range")
        b:SetMinMaxValues(100, 0)
        mn, mx = b:GetMinMaxValues()
        assert(mn == 0 and mx == 0 and b:GetValue() == 0, "(100, 0) is (0, 0)")
    "#,
    )
    .unwrap();
}

/// `0x7832d0` fires on a stored value only, an equal one returning (`0x783342`); `0x783270` clears
/// the has-value bit and re-sets the held value (`0x7832af`, `0x7832bc`), so once the bar has a
/// value every `SetMinMaxValues` fires, moved or not.
#[test]
fn statusbar_set_min_max_refires_onvaluechanged_for_the_held_value() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbEvt")
        b:SetMinMaxValues(0, 10)
        seen = {}
        b:SetScript("OnValueChanged", function() local self, value = this, arg1
            table.insert(seen, value)
            assert(self == b and arg1 == value, "handler-firing conventions carry the value")
        end)
        b:SetValue(4)
        b:SetValue(4)            -- equal: no fire
        b:SetMinMaxValues(0, 10) -- unchanged range, held 4: fires
        b:SetMinMaxValues(0, 3)  -- re-clamp 4 -> 3: fires
        assert(table.getn(seen) == 3 and seen[1] == 4 and seen[2] == 4 and seen[3] == 3,
               "fired " .. table.getn(seen))
    "#,
    )
    .unwrap();
}

/// `SetValue` returns at once without a range (`0x7832dc`), storing nothing; a range set on a bar
/// with no value fires nothing (`0x7832a5`), and the first stored value always fires.
#[test]
fn statusbar_value_before_a_range_is_dropped() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbFresh")
        seen = 0
        b:SetScript("OnValueChanged", function() seen = seen + 1 end)
        b:SetValue(5)
        assert(b:GetValue() == 0 and seen == 0, "no range: not stored, not fired")
        b:SetMinMaxValues(0, 10)
        assert(b:GetValue() == 0 and seen == 0, "a range alone fires nothing")
        b:SetValue(0)
        assert(seen == 1, "the first value fires though it equals the field")
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
        -- A solid bar is a texture filled with a colour, installed as the object: the bar setter
        -- has no colour form (`0x78f870`).
        local fill = b:CreateTexture(nil, "ARTWORK")
        fill:SetTexture(0.2, 0.7, 0.2)
        b:SetStatusBarTexture(fill)
        b:SetMinMaxValues(0, 100)
        b:SetValue(25)
    "#,
    )
    .unwrap();
    s.tick(0.0);
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
    crate::assert_near!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 150.0, 100.0, 120.0),
        "horizontal fill: left-pinned, 25% of 200px"
    );

    // Vertical: fills bottom-up.
    s.run(r#"SbFill:SetOrientation("VERTICAL")"#).unwrap();
    s.tick(0.0);
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
    crate::assert_near!(
        (r.left, r.right, r.bottom, r.top),
        (100.0, 300.0, 100.0, 105.0),
        "vertical fill: bottom-pinned, 25% of 20px"
    );
}

/// The fill crops its art, never squeezes it: the bar's update builds the coordinates from scratch
/// (`0x7833c0` → `0x770410`), 0 to the fill across when HORIZONTAL and 0 to the fill down when
/// VERTICAL, so a vertical bar shows the top of its art; the texture's rect follows them.
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
    s.tick(0.0);
    assert_eq!(
        uv(&s),
        [0.0, 0.25, 0.0, 1.0],
        "horizontal: u cropped to the fill fraction, v whole"
    );

    // Vertical: `{0, 0, fill, 1}` (`0x78342f`-`0x783436`), v from the art's top.
    s.run(r#"SbCrop:SetOrientation("VERTICAL")"#).unwrap();
    s.tick(0.0);
    assert_eq!(
        uv(&s),
        [0.0, 1.0, 0.0, 0.25],
        "vertical: v cut to the fill from the top, u whole"
    );

    // An empty bar samples nothing; a full one samples everything.
    s.run("SbCrop:SetValue(0)").unwrap();
    s.tick(0.0);
    assert_eq!(uv(&s), [0.0, 1.0, 0.0, 0.0], "empty");
    s.run("SbCrop:SetValue(100)").unwrap();
    s.tick(0.0);
    assert_eq!(uv(&s), [0.0, 1.0, 0.0, 1.0], "full");
}

/// The update writes the coordinates from scratch (`0x770410` stores all four corners), so an
/// authored or addon `SetTexCoord` on the bar holds only until the next update that marks the fill
/// (`+0x314 & 1`): a range, a stored value, the orientation or a texture.
#[test]
fn statusbar_fill_replaces_a_set_tex_coord_at_its_next_marked_update() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbOwn")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        b:GetStatusBarTexture():SetTexCoord(0.5, 1, 0.25, 0.75)
        b:SetMinMaxValues(0, 100)
        b:SetValue(40)
    "#,
    )
    .unwrap();
    let fill = |s: &UiScript| -> ([f32; 4], f32) {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(_),
                    tex_coords,
                    ..
                } => Some((
                    tex_coords.map_or([0.0, 1.0, 0.0, 1.0], |tc| tc.edges()),
                    (q.rect.expect("bar rect resolved").width() * 100.0).round() / 100.0,
                )),
                _ => None,
            })
            .expect("the bar quad")
    };
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 0.4, 0.0, 1.0], 80.0),
        "the base is not kept"
    );
    s.run("SbOwn:GetStatusBarTexture():SetTexCoord(0, 1, 0, 1)")
        .unwrap();
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 1.0, 0.0, 1.0], 200.0),
        "unmarked: the addon's holds"
    );
    s.run("SbOwn:SetValue(40)").unwrap();
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 1.0, 0.0, 1.0], 200.0),
        "an equal value stores nothing"
    );
    s.run("SbOwn:SetValue(60)").unwrap();
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 0.6, 0.0, 1.0], 120.0),
        "a stored value marks it"
    );
    s.run("SbOwn:Hide(); SbOwn:SetValue(10); SbOwn:GetStatusBarTexture():SetTexCoord(0, 1, 0, 1)")
        .unwrap();
    s.tick(0.0);
    s.run("SbOwn:Show()").unwrap();
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 0.1, 0.0, 1.0], 20.0),
        "a hidden bar is not walked; the mark waits for it to show"
    );
}

/// `0x7833c0` sets the bar texture's coordinates only with a range and a value (`0x7833e6`,
/// `0x7833ea`); before both it returns, so the whole texture draws across the bar.
#[test]
fn statusbar_without_a_range_and_a_value_draws_its_whole_texture() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbWhole")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        b:SetWidth(200); b:SetHeight(20)
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
    "#,
    )
    .unwrap();
    let fill = |s: &UiScript| -> ([f32; 4], f32) {
        s.extract()
            .iter()
            .find_map(|q| match &q.content {
                QuadContent::Texture {
                    path: Some(_),
                    tex_coords,
                    ..
                } => Some((
                    tex_coords.map_or([0.0, 1.0, 0.0, 1.0], |tc| tc.edges()),
                    q.rect.expect("bar rect resolved").width(),
                )),
                _ => None,
            })
            .expect("the bar quad")
    };
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 1.0, 0.0, 1.0], 200.0),
        "no range, no value"
    );
    s.run("SbWhole:SetValue(25)").unwrap();
    s.tick(0.0);
    assert_eq!(
        fill(&s),
        ([0.0, 1.0, 0.0, 1.0], 200.0),
        "a value before a range"
    );
    s.run("SbWhole:SetMinMaxValues(0, 100)").unwrap();
    s.tick(0.0);
    assert_eq!(fill(&s), ([0.0, 1.0, 0.0, 1.0], 200.0), "a range, no value");
    s.run("SbWhole:SetValue(25)").unwrap();
    s.tick(0.0);
    assert_eq!(fill(&s), ([0.0, 0.25, 0.0, 1.0], 50.0), "both: the fill");
}

#[test]
fn statusbar_get_texture_returns_the_bar_region() {
    let s = script();
    // 255 read back times the float at `0x81c6b4`.
    s.lua()
        .globals()
        .set("ONE", crate::color::channel(255))
        .unwrap();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbTex")
        assert(b:GetStatusBarTexture() == nil, "no bar before one is set")
        b:SetStatusBarTexture("Interface\\TargetingFrame\\UI-StatusBar")
        local t = b:GetStatusBarTexture()
        assert(t ~= nil and t == b:GetStatusBarTexture(), "stable region wrapper")
        b:SetStatusBarColor(1, 0, 0)
        local r, g, bb, a = b:GetStatusBarColor()
        assert(r == ONE and g == 0 and bb == 0 and a == ONE)
    "#,
    )
    .unwrap();
}

/// NaN through the setters as the x87 compares take it: `SetMinMaxValues` keeps a NaN low bound,
/// as `min > max` is false (`0x783276`, `test ah,0x41`); `SetValue`'s clamps keep a NaN, which is
/// then never equal to the held value (`0x783342`), so each NaN is stored and fires.
#[test]
fn statusbar_nan_keeps_its_low_bound_and_always_stores() {
    let s = script();
    s.run(
        r#"
        local b = CreateFrame("StatusBar", "SbNan")
        Fires = 0
        b:SetScript("OnValueChanged", function() Fires = Fires + 1 end)
        local nan = 0 / 0
        b:SetMinMaxValues(0, 10); b:SetValue(5)
        assert(Fires == 1, "the first value: " .. Fires)
        b:SetValue(nan)
        local v = b:GetValue()
        assert(v ~= v and Fires == 2, "a NaN is stored and fires")
        b:SetValue(nan)
        assert(Fires == 3, "a NaN is never the held value")
        b:SetMinMaxValues(nan, 10)
        local mn, mx = b:GetMinMaxValues()
        assert(mn ~= mn and mx == 10, "NaN is not above 10: the low bound is kept")
        assert(Fires == 4, "the range re-sets the held NaN, which fires")
        b:SetMinMaxValues(3, nan)
        mn, mx = b:GetMinMaxValues()
        assert(mn == 3 and mx ~= mx, "3 is not above NaN either")
    "#,
    )
    .unwrap();
}
