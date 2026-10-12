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

/// A frame's message lines as `(text, rgb bytes, id, alpha)`.
fn lines(s: &crate::script::UiScript, name: &str) -> Vec<(String, [u8; 3], u32, f32)> {
    let m = s.model_ref();
    let h = m.arena.lookup(name).expect("named frame");
    m.arena
        .frame(h)
        .and_then(|f| f.kind_state.message_lines())
        .map(|l| {
            l.iter()
                .map(|l| (l.text.clone(), l.color, l.id, l.alpha))
                .collect()
        })
        .unwrap_or_default()
}

/// `MessageFrame:AddMessage` (`0x795590`) gates r, g and b with `lua_isnumber` (`0x79565f`-
/// `0x795687`) and reads the alpha under its own gate (`0x795759`): "abc" or a table is white and
/// opaque, a numeric string is its number.
#[test]
fn message_frame_add_message_gates_its_channels_with_is_number() {
    let s = script();
    s.run(r#"CreateFrame("MessageFrame", "MF")"#).unwrap();
    s.run(
        r#"
        MF:AddMessage("a", "abc", 1, 1)
        MF:AddMessage("b", "0.5", "1", "0", "0.5")
        MF:AddMessage("c", 1, 1, 1, {})
        MF:AddMessage("d", {}, 1, 1)
    "#,
    )
    .unwrap();
    let got = lines(&s, "MF");
    assert_eq!(got[0].1, [255, 255, 255], "a failed gate is white");
    assert_eq!(got[1].1, [128, 255, 0]);
    assert_eq!(
        got[1].3,
        128.0 / 255.0,
        "a numeric alpha string counts, byte-quantized"
    );
    assert_eq!(got[2].3, 1.0, "a table alpha stages 1.0");
    assert_eq!(got[3].1, [255, 255, 255]);
}

/// `ScrollingMessageFrame:AddMessage` (`0x792900`) gates the same three, then reads the id at 6
/// (or r's slot at 3 when the gate failed) through `lua_isnumber` and `_ftol`; `UpdateColorByID`
/// (`0x7932b0`) gates all four and does nothing if any fails.
#[test]
fn scrolling_message_frame_gates_colour_and_id_with_is_number() {
    let s = script();
    s.run(r#"CreateFrame("ScrollingMessageFrame", "SMF")"#)
        .unwrap();
    s.run(
        r#"
        SMF:AddMessage("a", "0.5", "1", "0", "7")
        SMF:AddMessage("b", "abc", 1, 1, 8)
        SMF:AddMessage("c", "9")
        SMF:AddMessage("d", 1, 1, 1, 2.9)
    "#,
    )
    .unwrap();
    let got = lines(&s, "SMF");
    assert_eq!((got[0].1, got[0].2), ([128, 255, 0], 7), "numeric strings");
    assert_eq!((got[1].1, got[1].2), ([255, 255, 255], 0), "id is r's slot");
    assert_eq!((got[2].1, got[2].2), ([255, 255, 255], 9));
    assert_eq!(got[3].2, 2, "the id is chopped by _ftol");
    s.run(r#"SMF:UpdateColorByID(7, 0, "abc", 1)"#).unwrap();
    assert_eq!(
        lines(&s, "SMF")[0].1,
        [128, 255, 0],
        "a failed gate is a no-op"
    );
    s.run(r#"SMF:UpdateColorByID("7", "0", "0.5", 0)"#).unwrap();
    assert_eq!(lines(&s, "SMF")[0].1, [0, 128, 0]);
}

/// The text-colour alpha is `lua_isnumber`-gated with default 1.0 (`0x781037`, `0x79f5a2`,
/// `0x78fd77`, `0x79db17`, `0x79de97`, `0x79f802`): a table or a boolean neither raises nor
/// sticks, and a numeric string is its number.
#[test]
fn colour_alphas_are_gated_not_raised() {
    let s = script();
    s.run(
        r#"
        B = CreateFrame("Button", "B")
        F = CreateFrame("Frame", "F")
        FS = F:CreateFontString("FS")
        CreateFont("Fnt")
        S = CreateFrame("StatusBar", "S") S:SetStatusBarTexture("Interface\\Bar")
        T = F:CreateTexture("T")
    "#,
    )
    .unwrap();
    for (call, get) in [
        ("B:SetTextColor(1, 1, 1, ARG)", "B:GetTextColor()"),
        ("FS:SetTextColor(1, 1, 1, ARG)", "FS:GetTextColor()"),
        ("FS:SetShadowColor(1, 1, 1, ARG)", "FS:GetShadowColor()"),
        ("Fnt:SetTextColor(1, 1, 1, ARG)", "Fnt:GetTextColor()"),
        ("Fnt:SetShadowColor(1, 1, 1, ARG)", "Fnt:GetShadowColor()"),
        ("S:SetStatusBarColor(1, 1, 1, ARG)", "S:GetStatusBarColor()"),
    ] {
        for (arg, want) in [
            (r#""0.5""#, 0.5),
            ("{}", 1.0),
            ("true", 1.0),
            (r#""abc""#, 1.0),
            ("nil", 1.0),
            ("0.25", 0.25),
        ] {
            s.run(&call.replace("ARG", arg))
                .unwrap_or_else(|e| panic!("{call} with {arg} raised: {e}"));
            let a = s
                .eval::<f64>(&format!("local _, _, _, a = {get} return a"))
                .unwrap();
            assert_eq!(a, want, "{call} with {arg}");
        }
    }
    // A Font object's colour is replaced whole (`0x79f5f5`...`0x79f65f`): a bare call resets the
    // alpha rather than keeping it.
    s.run("Fnt:SetTextColor(1, 1, 1, 0.25) Fnt:SetTextColor(1, 1, 1)")
        .unwrap();
    assert_eq!(
        s.eval::<f64>("local _, _, _, a = Fnt:GetTextColor() return a")
            .unwrap(),
        1.0
    );
}

/// `SetVertexColor` (`0x79abd0`) stages alpha 1.0 but re-tests the gate at the end (`0x79adfe`):
/// anything but a number lends the region's current alpha back (`0x79ae0a`), and never raises.
#[test]
fn set_vertex_color_keeps_the_alpha_unless_given_a_number() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T")"#)
        .unwrap();
    let alpha = || {
        s.eval::<f64>("local _, _, _, a = T:GetVertexColor() return a")
            .unwrap()
    };
    s.run("T:SetVertexColor(1, 1, 1, 0.5)").unwrap();
    assert_eq!(alpha(), 0.5);
    for arg in ["", ", nil", ", {}", ", true", r#", "x""#] {
        s.run(&format!("T:SetVertexColor(0, 0, 0{arg})"))
            .unwrap_or_else(|e| panic!("SetVertexColor(0, 0, 0{arg}) raised: {e}"));
        assert_eq!(alpha(), 0.5, "SetVertexColor(0, 0, 0{arg}) keeps it");
    }
    s.run(r#"T:SetVertexColor(0, 0, 0, "0.25")"#).unwrap();
    assert_eq!(alpha(), 0.25, "a numeric string is a number");
}

/// `lua_isnumber` on the first argument picks `SetTexture`'s colour form (`0x79bbd5`), so a
/// numeric string is a fill, not a path.
#[test]
fn set_texture_takes_a_numeric_string_as_a_colour() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T")"#)
        .unwrap();
    s.run(r#"T:SetTexture("1", "0.5", 0)"#).unwrap();
    let t: mlua::Table = s.lua().globals().get("T").unwrap();
    let rh = crate::script::region::region_handle_of(s.lua(), &t).unwrap();
    let m = s.model_ref();
    let d = m.region_data.get(&rh).unwrap();
    assert_eq!(d.fill, Some([1.0, 0.5, 0.0, 1.0]));
    assert_eq!(d.texture, None, "a numeric string is not a path");
}

/// `GameTooltip:SetText`'s alpha (`0x531d23`) is `lua_isnumber`-gated too.
#[test]
fn tooltip_set_text_alpha_accepts_a_numeric_string() {
    let s = script();
    s.run(r#"TT = CreateFrame("GameTooltip", "TT")"#).unwrap();
    s.run(r#"TT:SetText("x", 1, 1, 1, "0.5")"#).unwrap();
    assert_eq!(
        s.eval::<f64>("local _, _, _, a = TTTextLeft1:GetTextColor() return a")
            .unwrap(),
        0.5
    );
    s.run(r#"TT:SetText("x", 1, 1, 1, {})"#).unwrap();
    assert_eq!(
        s.eval::<f64>("local _, _, _, a = TTTextLeft1:GetTextColor() return a")
            .unwrap(),
        1.0
    );
}
