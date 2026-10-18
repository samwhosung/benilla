//! The frame state verbs' arguments: draw layers, scale and alpha, as 1.12.1 reads them.

use super::common::script;
use crate::script::*;

/// The texture paths a frame named `name` draws.
fn drawn(s: &UiScript, name: &str) -> Vec<String> {
    s.extract()
        .iter()
        .filter(|q| s.quad_owner_name(q.target).as_deref() == Some(name))
        .filter_map(|q| match &q.content {
            QuadContent::Texture { path: Some(p), .. } => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// `Enable/DisableDrawLayer` start from ARTWORK (`0x775640`) and replace it only with a string that
/// names a layer (`0x6f18b0` writes on a hit alone), so no argument, nil, a number, a table and an
/// unknown name all take ARTWORK; the name is matched case-blind.
#[test]
fn draw_layer_verbs_default_to_artwork() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Layered")
        f:SetPoint("BOTTOMLEFT", 0, 0); f:SetWidth(100); f:SetHeight(100)
        local a = f:CreateTexture(nil, "ARTWORK"); a:SetAllPoints(f); a:SetTexture("Art")
        local o = f:CreateTexture(nil, "OVERLAY"); o:SetAllPoints(f); o:SetTexture("Over")
    "#,
    )
    .unwrap();
    s.resolve();
    let both = vec!["Art".to_string(), "Over".to_string()];
    assert_eq!(drawn(&s, "Layered"), both);
    for arg in ["", "nil", "5", "{}", "\"bogus\""] {
        s.run(&format!("f:DisableDrawLayer({arg})")).unwrap();
        assert_eq!(
            drawn(&s, "Layered"),
            vec!["Over".to_string()],
            "DisableDrawLayer({arg}) takes ARTWORK"
        );
        s.run(&format!("f:EnableDrawLayer({arg})")).unwrap();
        assert_eq!(
            drawn(&s, "Layered"),
            both,
            "EnableDrawLayer({arg}) restores it"
        );
    }
    s.run(r#"f:DisableDrawLayer("overlay")"#).unwrap();
    assert_eq!(drawn(&s, "Layered"), vec!["Art".to_string()]);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `SetScale` (`0x775140`): a non-number raises the usage line (`0x775250`), a scale below 0 or
/// NaN raises `Scale must be > 0` naming the frame or `<unnamed>` (`0x77520b`) and stores nothing,
/// 0 is stored, and a change under 2^-22 is dropped (`0x76ac1a`).
#[test]
fn set_scale_gates_its_argument_as_the_client_does() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Scaled")
        f:SetPoint("CENTER", 0, 0); f:SetWidth(100); f:SetHeight(100)
        f:CreateTexture(nil, "ARTWORK"):SetAllPoints(f)
        u = CreateFrame("Frame")
        f:SetScale(0.5)
    "#,
    )
    .unwrap();
    let err = |code: &str| s.run(code).unwrap_err().to_string();
    let e = err("f:SetScale(-1)");
    assert!(e.contains("Scaled:SetScale(): Scale must be > 0"), "{e}");
    let e = err("f:SetScale(0/0)");
    assert!(e.contains("Scaled:SetScale(): Scale must be > 0"), "{e}");
    let e = err("u:SetScale(-0.5)");
    assert!(e.contains("<unnamed>:SetScale(): Scale must be > 0"), "{e}");
    let e = err("f:SetScale(\"x\")");
    assert!(e.contains("Usage: Scaled:SetScale(scale)"), "{e}");
    let e = err("f:SetScale()");
    assert!(e.contains("Usage: Scaled:SetScale(scale)"), "{e}");
    assert_eq!(s.eval::<f64>("return f:GetScale()").unwrap(), 0.5);

    s.run("f:SetScale(\"2\")").unwrap();
    assert_eq!(s.eval::<f64>("return f:GetScale()").unwrap(), 2.0);
    s.run("f:SetScale(1)  f:SetScale(1 + 1e-7)").unwrap();
    assert_eq!(s.eval::<f64>("return f:GetScale()").unwrap(), 1.0);
    s.run("f:SetScale(1 + 4e-7)").unwrap();
    assert_eq!(
        s.eval::<f64>("return f:GetScale()").unwrap(),
        f64::from((1.0f64 + 4e-7) as f32)
    );

    s.run("f:SetScale(0)").unwrap();
    assert_eq!(s.eval::<f64>("return f:GetScale()").unwrap(), 0.0);
    s.resolve();
    let _ = s.extract();
}

/// `Frame:SetAlpha` (`0x774e90`) stores `trunc(clamp(a) * 255)`, NaN as 1, and `GetAlpha`
/// (`0x774dc0`) reads the byte times the `f64` 1/255; a non-number raises the usage line
/// (`0x774f76`). The byte reaches the draw.
#[test]
fn frame_alpha_is_the_clients_truncated_byte() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Faded")
        f:SetPoint("CENTER", 0, 0); f:SetWidth(10); f:SetHeight(10)
        local t = f:CreateTexture(nil, "ARTWORK"); t:SetAllPoints(f); t:SetTexture("Art")
        u = CreateFrame("Frame")
    "#,
    )
    .unwrap();
    let alpha = |s: &UiScript, a: &str| -> f64 {
        s.run(&format!("f:SetAlpha({a})")).unwrap();
        s.eval::<f64>("return f:GetAlpha()").unwrap()
    };
    assert_eq!(alpha(&s, "0.5"), 0.4980392156862745);
    assert_eq!(alpha(&s, "1"), 1.0);
    assert_eq!(alpha(&s, "0.999"), 254.0 * (1.0 / 255.0));
    assert_eq!(alpha(&s, "0"), 0.0);
    assert_eq!(alpha(&s, "0/0"), 1.0);
    assert_eq!(alpha(&s, "2"), 1.0);
    assert_eq!(alpha(&s, "-1"), 0.0);
    assert_eq!(alpha(&s, "\"0.5\""), 0.4980392156862745);

    s.resolve();
    let drawn: Vec<f32> = s
        .extract()
        .iter()
        .filter(|q| s.quad_owner_name(q.target).as_deref() == Some("Faded"))
        .filter(|q| matches!(q.content, QuadContent::Texture { .. }))
        .map(|q| q.alpha)
        .collect();
    assert_eq!(drawn, vec![127.0 / 255.0]);

    let e = s.run("f:SetAlpha(nil)").unwrap_err().to_string();
    assert!(e.contains("Usage: Faded:SetAlpha(alpha 0 to 1)"), "{e}");
    let e = s.run("u:SetAlpha({})").unwrap_err().to_string();
    assert!(e.contains("Usage: <unnamed>:SetAlpha(alpha 0 to 1)"), "{e}");
}

/// Re-setting a parent's alpha to the byte it holds leaves a child's own later alpha alone, in
/// Lua as in the walk (`0x76a69a`).
#[test]
fn a_parents_unchanged_alpha_does_not_recascade() {
    let s = script();
    s.run(
        r#"
        p = CreateFrame("Frame", "AlphaP")
        c = CreateFrame("Frame", "AlphaC", p)
        g = CreateFrame("Frame", "AlphaG", c)
        p:SetAlpha(0.5)
        c:SetAlpha(1)
        p:SetAlpha(0.5)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64, f64)>("return p:GetAlpha(), c:GetAlpha(), g:GetAlpha()")
            .unwrap(),
        (0.4980392156862745, 1.0, 1.0)
    );
}

/// A frame's XML `alpha` (`0x769a10`) is skipped when empty, otherwise read by `SStrToFloat`, so
/// text with no number is 0, and handed to the alpha walk clamped and truncated as Lua's is.
#[test]
fn xml_alpha_takes_the_reference_parse_and_byte() {
    let s = script();
    let doc = crate::framexml::parse(
        r#"<Ui>
             <Frame name="XmlQuarter" alpha="0.25"/>
             <Frame name="XmlWord" alpha="half"/>
             <Frame name="XmlEmpty" alpha=""/>
             <Frame name="XmlOver" alpha="3"/>
           </Ui>"#,
    )
    .unwrap();
    let report = crate::loader::load(&s, &doc, &|_| None);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(
        s.eval::<(f64, f64, f64, f64)>(
            "return XmlQuarter:GetAlpha(), XmlWord:GetAlpha(), XmlEmpty:GetAlpha(), XmlOver:GetAlpha()"
        )
        .unwrap(),
        (63.0 / 255.0, 0.0, 1.0, 1.0)
    );
}
