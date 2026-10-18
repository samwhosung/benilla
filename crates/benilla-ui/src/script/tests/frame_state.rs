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
