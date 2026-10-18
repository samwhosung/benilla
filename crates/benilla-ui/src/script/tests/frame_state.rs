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
