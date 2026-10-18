//! Widget colours are 1.12.1's packed byte colour (`CImVector`): every setter clamps (NaN to 1) and
//! rounds half up to a byte, every colour getter reads the byte times the float at `0x81c6b4`, and
//! the alpha-only setters store the low byte of the truncated product.

use super::common::script;
use crate::color::channel;

/// Read `code`'s four returns as doubles.
fn four(s: &crate::script::UiScript, code: &str) -> (f64, f64, f64, f64) {
    s.eval(&format!("return {code}")).unwrap()
}

/// `SetVertexColor` (`0x79abd0`) packs 0.5 to 128 (`0x79ad96`) and `GetVertexColor` (`0x79aa50`)
/// reads it as 128 times the float scale, not 128/255; out-of-range and NaN channels clamp.
#[test]
fn a_texture_colour_reads_back_as_its_bytes() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T") S = F:CreateTexture("S")"#)
        .unwrap();
    s.run("T:SetVertexColor(0.5, 0.5, 0.5)").unwrap();
    assert_eq!(
        four(&s, "T:GetVertexColor()"),
        (channel(128), channel(128), channel(128), channel(255))
    );
    assert_eq!(channel(128), 0.5019596815109253);
    s.run("T:SetVertexColor(0/0, -1, 1.5, 2)").unwrap();
    assert_eq!(
        four(&s, "T:GetVertexColor()"),
        (channel(255), 0.0, channel(255), channel(255))
    );
    // White with an alpha that rounds to 254 stores nothing on an empty slot (`0x77f766`), so it
    // reads opaque; over a colour already held it is stored.
    s.run("S:SetVertexColor(1, 1, 1, 0.997)").unwrap();
    assert_eq!(four(&s, "S:GetVertexColor()").3, channel(255));
    s.run("T:SetVertexColor(1, 1, 1, 0.997)").unwrap();
    assert_eq!(four(&s, "T:GetVertexColor()").3, channel(254));
}

/// `FontString:GetTextColor` (`0x79d840`) pushes red as the double product but rounds green, blue
/// and alpha to `f32`s first (`0x79d92a`, `0x79d958`, `0x79d969`); `SetTextColor` clamps.
#[test]
fn a_fontstrings_text_colour_rounds_three_channels_to_floats() {
    let s = script();
    s.run(r#"F = CreateFrame("Frame", "F") FS = F:CreateFontString("FS")"#)
        .unwrap();
    s.run("FS:SetTextColor(2, 0, 0)").unwrap();
    assert_eq!(
        four(&s, "FS:GetTextColor()"),
        (0.9999978030100465, 0.0, 0.0, 0.9999977946281433)
    );
    s.run("FS:SetTextColor(0/0, -1, 1.5)").unwrap();
    let (r, g, b, _) = four(&s, "FS:GetTextColor()");
    assert_eq!(
        (r, g, b),
        (channel(255), 0.0, f64::from(channel(255) as f32))
    );
}

/// `Font:SetAlpha` (`0x79f420`) stores `__ftol(a * 255)`'s low byte with no clamp and `GetAlpha`
/// (`0x79f4a0`) reads it times the float scale; a non-number raises the usage line (`0x87c51c`).
#[test]
fn a_font_alpha_wraps_and_reads_through_the_float_scale() {
    let s = script();
    s.run(r#"CreateFont("AlphaFont")"#).unwrap();
    for (a, byte) in [
        ("0.5", 127),
        ("1", 255),
        ("2", 254),
        ("-0.1", 231),
        ("0/0", 0),
    ] {
        s.run(&format!("AlphaFont:SetAlpha({a})")).unwrap();
        assert_eq!(
            s.eval::<f64>("return AlphaFont:GetAlpha()").unwrap(),
            channel(byte),
            "SetAlpha({a})"
        );
    }
    let e = s.run("AlphaFont:SetAlpha(nil)").unwrap_err().to_string();
    assert!(e.contains("Usage: AlphaFont:SetAlpha(alpha)"), "{e}");
}

/// `Texture:SetAlpha` (`0x79b580`) and `FontString:SetAlpha` (`0x79cb70`) store the same wrapped
/// byte (`0x79b638`, `mov [ebp-1],al`), read times the double 1/255 (`0x79b754`); a non-number
/// raises the usage line.
#[test]
fn a_region_alpha_wraps_and_raises_on_a_non_number() {
    let s = script();
    s.run(
        r#"F = CreateFrame("Frame", "F") T = F:CreateTexture("T") FS = F:CreateFontString("FS")"#,
    )
    .unwrap();
    for region in ["T", "FS"] {
        for (a, byte) in [
            ("0.5", 127.0),
            ("1.5", 126.0),
            ("-0.1", 231.0),
            ("0/0", 0.0),
        ] {
            s.run(&format!("{region}:SetAlpha({a})")).unwrap();
            assert_eq!(
                s.eval::<f64>(&format!("return {region}:GetAlpha()"))
                    .unwrap(),
                byte * (1.0 / 255.0),
                "{region}:SetAlpha({a})"
            );
        }
        let e = s
            .run(&format!("{region}:SetAlpha({{}})"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains(&format!("Usage: {region}:SetAlpha(alpha)")),
            "{e}"
        );
    }
}

/// The backdrop getters read each byte times the float scale (`0x777c98`), so white is 0.9999978;
/// the Button, StatusBar and gradient setters pack as every colour setter does.
#[test]
fn backdrop_button_bar_and_gradient_colours_are_bytes() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame", "F")
        F:SetBackdrop({ bgFile = "bg" })
        B = CreateFrame("Button", "B")
        S = CreateFrame("StatusBar", "S") S:SetStatusBarTexture("Interface\\Bar")
        T = F:CreateTexture("T")
    "#,
    )
    .unwrap();
    let one = channel(255);
    assert_eq!(four(&s, "F:GetBackdropColor()"), (one, one, one, one));
    s.run("B:SetTextColor(0.5, 2, -1)").unwrap();
    assert_eq!(four(&s, "B:GetTextColor()"), (channel(128), one, 0.0, one));
    s.run("S:SetStatusBarColor(0.25, 0.5, 0.75, 0.5)").unwrap();
    assert_eq!(
        four(&s, "S:GetStatusBarColor()"),
        (channel(64), channel(128), channel(191), channel(128))
    );
    s.run(r#"T:SetGradientAlpha("VERTICAL", 0.5, 0, 0, 0.5, 2, 1, 1, 1)"#)
        .unwrap();
    let rh = {
        let t: mlua::Table = s.lua().globals().get("T").unwrap();
        crate::script::region::region_handle_of(s.lua(), &t).unwrap()
    };
    let g = s.model_ref().region_data[&rh].gradient.unwrap();
    assert_eq!(
        (g.start, g.end),
        (
            crate::color::Rgba8::rgba(128, 0, 0, 128),
            crate::color::Rgba8::WHITE
        )
    );
}

/// An XML `<Color>` (`0x6f23d0`) reads each present, non-empty attribute with `SStrToFloat`, so
/// text with no number is 0, and packs it as the setters do: `r="2"` clamps to 255.
#[test]
fn an_xml_colour_clamps_and_parses_as_the_client_does() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui><Frame name="XC"><Layers><Layer>
             <FontString name="XCText"><Color r="2" g="abc" b="0.5" a=""/></FontString>
           </Layer></Layers></Frame></Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let (r, g, b, a) = four(&s, "XCText:GetTextColor()");
    assert_eq!(
        (r, g, b, a),
        (
            channel(255),
            0.0,
            f64::from(channel(128) as f32),
            f64::from(channel(255) as f32)
        )
    );
}
