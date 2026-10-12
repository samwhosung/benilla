//! Colour and coordinate channels are a bare `lua_tonumber` (`0x6f3620`) with no type gate: a
//! numeric string is its number (`luaV_tonumber` `0x6f7c20`, `luaO_str2d` `0x6f5900`), anything
//! else 0, and the setter never raises. Sites: `Button:SetTextColor` `0x780ee0`,
//! `SetHighlightTextColor` `0x7815e0`, `SetDisabledTextColor` `0x781260`, the Font object setters
//! `0x79f4d0` and `0x79f730`, `FontString:SetTextColor` `0x79d9c0` and `SetShadowColor` `0x79dd40`,
//! `Texture:SetVertexColor` `0x79abd0`, `SetTexCoord` `0x79beb0`, `SetGradient` `0x79ae30`,
//! `SetGradientAlpha` `0x79b180`, `StatusBar:SetStatusBarColor` `0x78fc20` and
//! `ColorSelect:SetColorRGB` `0x78eae0`.

use super::common::script;

/// Numeric strings and a hostile one; each reads as the number on the right.
const CHANNELS: &str = r#""1", "0.5", "abc""#;

/// `set(r, g, b)` with string channels, read back through `get` as three numbers.
fn check(setup: &str, call: &str, get: &str) {
    check_as(setup, call, get, (1.0, 0.5, 0.0));
}

fn check_as(setup: &str, call: &str, get: &str, want: (f64, f64, f64)) {
    let s = script();
    s.run(setup).unwrap();
    s.run(&call.replace("ARGS", CHANNELS)).unwrap();
    let (r, g, b) = s
        .eval::<(f64, f64, f64)>(&format!("local r, g, b = {get} return r, g, b"))
        .unwrap();
    assert_eq!((r, g, b), want, "{call}");
}

#[test]
fn text_and_shadow_colour_setters_coerce_numeric_strings() {
    check(
        r#"B = CreateFrame("Button", "B")"#,
        "B:SetTextColor(ARGS)",
        "B:GetTextColor()",
    );
    check(
        r#"CreateFont("Fnt")"#,
        "Fnt:SetTextColor(ARGS)",
        "Fnt:GetTextColor()",
    );
    check(
        r#"CreateFont("Fnt")"#,
        "Fnt:SetShadowColor(ARGS)",
        "Fnt:GetShadowColor()",
    );
    let setup = r#"F = CreateFrame("Frame", "F") FS = F:CreateFontString("FS")"#;
    check(setup, "FS:SetTextColor(ARGS)", "FS:GetTextColor()");
    check(setup, "FS:SetShadowColor(ARGS)", "FS:GetShadowColor()");
}

#[test]
fn button_highlight_and_disabled_colours_coerce_numeric_strings() {
    let s = script();
    s.run(r#"B = CreateFrame("Button", "B")"#).unwrap();
    s.run(&format!(
        "B:SetHighlightTextColor({CHANNELS}) B:SetDisabledTextColor({CHANNELS})"
    ))
    .unwrap();
    let t: mlua::Table = s.lua().globals().get("B").unwrap();
    let h = crate::script::object::frame_handle_of(s.lua(), &t).unwrap();
    let model = s.model_ref();
    let Some(crate::widget::KindState::Button(bs)) = model.arena.frame(h).map(|f| &f.kind_state)
    else {
        panic!("not a button");
    };
    assert_eq!(bs.highlight_color, Some([1.0, 0.5, 0.0, 1.0]));
    assert_eq!(bs.disabled_color, Some([1.0, 0.5, 0.0, 1.0]));
}

#[test]
fn vertex_bar_and_picker_colours_coerce_numeric_strings() {
    check(
        r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T")"#,
        "T:SetVertexColor(ARGS)",
        "T:GetVertexColor()",
    );
    check(
        r#"S = CreateFrame("StatusBar", "S") S:SetStatusBarTexture("Interface\\Bar")"#,
        "S:SetStatusBarColor(ARGS)",
        "S:GetStatusBarColor()",
    );
    // The widget quantizes each channel to a byte: 0.5 is 128 / 255.
    check_as(
        r#"C = CreateFrame("ColorSelect", "C")"#,
        "C:SetColorRGB(ARGS)",
        "C:GetColorRGB()",
        (1.0, 128.0 / 255.0, 0.0),
    );
}

#[test]
fn tex_coord_and_gradients_coerce_numeric_strings() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T")"#)
        .unwrap();
    s.run(r#"T:SetTexCoord("0.25", "0.75", "0.5", "1")"#)
        .unwrap();
    assert_eq!(
        s.eval::<Vec<f64>>("return {T:GetTexCoord()}").unwrap(),
        vec![0.25, 0.5, 0.25, 1.0, 0.75, 0.5, 0.75, 1.0]
    );
    s.run(r#"T:SetTexCoord("0", "1", "abc", "{}")"#).unwrap();
    assert_eq!(
        s.eval::<Vec<f64>>("return {T:GetTexCoord()}").unwrap(),
        vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0]
    );

    let t: mlua::Table = s.lua().globals().get("T").unwrap();
    let rh = crate::script::region::region_handle_of(s.lua(), &t).unwrap();
    let gradient = || {
        let g = s
            .model_ref()
            .region_data
            .get(&rh)
            .and_then(|d| d.gradient)
            .expect("a gradient");
        (g.start, g.end)
    };
    s.run(r#"T:SetGradient("VERTICAL", "1", "0.5", "abc", "0.25", "0.75", "1")"#)
        .unwrap();
    assert_eq!(
        gradient(),
        ([1.0, 0.5, 0.0, 1.0], [0.25, 0.75, 1.0, 1.0]),
        "SetGradient"
    );
    s.run(r#"T:SetGradientAlpha("HORIZONTAL", "1", "0.5", "abc", "0.5", "0.25", "0.75", "1", "0.25")"#)
        .unwrap();
    assert_eq!(
        gradient(),
        ([1.0, 0.5, 0.0, 0.5], [0.25, 0.75, 1.0, 0.25]),
        "SetGradientAlpha"
    );
    // The alphas are `lua_isnumber`-gated with default 1 (`0x79b30d`), not a bare read.
    s.run(r#"T:SetGradientAlpha("VERTICAL", 0, 0, 0, nil, 1, 1, 1, "x")"#)
        .unwrap();
    assert_eq!(gradient(), ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]));
}

/// `SetTexture`'s colour form (`0x79bb40`): g and b are a bare read, alpha `lua_isnumber`-gated,
/// and `luaO_str2d` skips the blanks around a number.
#[test]
fn texture_colour_form_coerces_numeric_strings() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T")"#)
        .unwrap();
    s.run(r#"T:SetTexture(1, " 0.5 ", "abc", "0.25")"#).unwrap();
    let t: mlua::Table = s.lua().globals().get("T").unwrap();
    let rh = crate::script::region::region_handle_of(s.lua(), &t).unwrap();
    assert_eq!(
        s.model_ref().region_data.get(&rh).and_then(|d| d.fill),
        Some([1.0, 0.5, 0.0, 0.25])
    );
}
