//! `SetTexCoord` and font objects.

use super::common::script;
use crate::script::*;

#[test]
fn set_tex_coord_changes_extracted_uv() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "TcF")
        f:SetWidth(100); f:SetHeight(100); f:SetPoint("CENTER", 0, 0)
        t = f:CreateTexture("TcTex", "ARTWORK")
        t:SetTexture("Interface\\Foo")
        t:SetTexCoord(0.1, 0.6, 0.2, 0.8)
    "#,
    )
    .unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .expect("the Foo texture quad");
    assert!(
        matches!(&tex.content, QuadContent::Texture { tex_coords: Some(TexCoords::Rect(tc)), .. } if *tc == [0.1, 0.6, 0.2, 0.8]),
        "SetTexCoord surfaces on the extracted quad, got {:?}",
        tex.content
    );

    // `GetTexCoord` returns eight values in `SetTexCoord`'s corner order, `ULx, ULy, LLx, LLy,
    // URx, URy, LRx, LRy`: the reference has no 4-value getter.
    let got: (f32, f32, f32, f32, f32, f32, f32, f32) = s.eval("return t:GetTexCoord()").unwrap();
    assert_eq!(got, (0.1, 0.2, 0.1, 0.8, 0.6, 0.2, 0.6, 0.8));

    // The 8-argument form (UL, LL, UR, LR) lands in screen order [TL, TR, BR, BL]; a 90° turn here.
    s.run("t:SetTexCoord(0,1, 1,1, 0,0, 1,0)").unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .unwrap();
    assert!(
        matches!(
            &tex.content,
            QuadContent::Texture { tex_coords: Some(TexCoords::Corners(c)), .. }
                if *c == [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]
        ),
        "the affine form carries corners in screen winding, got {:?}",
        tex.content
    );
    // A no-arg reset returns to the full texture.
    s.run("t:SetTexCoord()").unwrap();
    s.resolve();
    let tex = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Texture { path: Some(p), .. } if p.contains("Foo")))
        .unwrap();
    assert!(matches!(
        &tex.content,
        QuadContent::Texture {
            tex_coords: None,
            ..
        }
    ));
}

#[test]
fn set_font_object_repoints_fontstring() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.register_font_object(
        "Big",
        FontObject {
            font: Some("Fonts\\MORPHEUS.TTF".into()),
            height: Some(18.0),
            color: Some([0.0, 0.0, 0.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
        },
    );
    s.register_font_object(
        "Small",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(10.0),
            color: Some([1.0, 1.0, 1.0, 1.0]),
            outline: Outline::None,
            justify_h: None,
            justify_v: None,
            shadow: None,
        },
    );
    s.run(
        r#"
        f = CreateFrame("Frame", "FoF")
        f:SetWidth(120); f:SetHeight(30); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString("FoText", "ARTWORK")
        fs:SetText("Hi")
        fs:SetFontObject("Big")
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return fs:GetFontObject():GetName()")
            .unwrap(),
        "Big"
    );

    let resolved = |s: &UiScript| -> (Option<String>, Option<f32>, Option<[f32; 4]>) {
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text {
                    font,
                    font_height,
                    color,
                    ..
                } => Some((font, font_height, color)),
                _ => None,
            })
            .expect("a text quad")
    };
    s.resolve();
    assert_eq!(
        resolved(&s),
        (
            Some("Fonts\\MORPHEUS.TTF".into()),
            Some(18.0),
            Some([0.0, 0.0, 0.0, 1.0])
        )
    );

    s.run("fs:SetFontObject('Small')").unwrap();
    s.resolve();
    assert_eq!(
        resolved(&s),
        (
            Some("Fonts\\FRIZQT__.TTF".into()),
            Some(10.0),
            Some([1.0, 1.0, 1.0, 1.0])
        )
    );

    assert!(s.run("fs:SetFontObject('Nope')").is_err());
}

/// A FontString's `justifyV` defaults to MIDDLE, the client's default.
#[test]
fn justify_v_defaults_middle_and_overrides() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "JvF")
        f:SetWidth(100); f:SetHeight(30); f:SetPoint("CENTER", 0, 0)
        fs = f:CreateFontString("JvText", "ARTWORK")
        fs:SetText("Hi")
    "#,
    )
    .unwrap();
    let justify_v = |s: &UiScript| {
        s.extract()
            .into_iter()
            .find_map(|q| match q.content {
                QuadContent::Text { justify_v, .. } => Some(justify_v),
                _ => None,
            })
            .expect("a text quad")
    };
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Middle, "the client default");

    s.run("fs:SetJustifyV('BOTTOM')").unwrap();
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Bottom);

    // A font object's justification applies only on an axis the string has not set itself: a
    // local setter clears that axis's inherit bit (`+0x124`) for good. XML is unaffected, as the
    // loader applies `inherits=` before the element's own `justifyV=`.
    s.register_font_object(
        "TopFont",
        FontObject {
            font: Some("Fonts\\FRIZQT__.TTF".into()),
            height: Some(12.0),
            color: None,
            outline: Outline::None,
            justify_h: None,
            justify_v: Some(JustifyV::Top),
            shadow: None,
        },
    );
    s.run("fs:SetFontObject('TopFont')").unwrap();
    s.resolve();
    assert_eq!(justify_v(&s), JustifyV::Bottom, "severance is permanent");

    // A string that never set the axis takes the object's justification.
    s.run(
        r#"
        fresh = f:CreateFontString(nil, "ARTWORK")
        fresh:SetText("Fresh")
        fresh:SetFontObject('TopFont')
    "#,
    )
    .unwrap();
    s.resolve();
    let fresh = s
        .extract()
        .into_iter()
        .find_map(|q| match q.content {
            QuadContent::Text {
                text: Some(ref t),
                justify_v,
                ..
            } if t == "Fresh" => Some(justify_v),
            _ => None,
        })
        .expect("a text quad");
    assert_eq!(fresh, JustifyV::Top);
}
