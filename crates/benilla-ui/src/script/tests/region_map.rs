//! The Region map ([`crate::script::region_map`]): one function per name for frames and regions
//! alike, shared by identity and stopping at the 19.

use crate::script::UiScript;

/// A VM with one frame, one texture and one fontstring, each reachable by a global name.
fn vm() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        Host = CreateFrame("Frame", "Host")
        Host:SetWidth(200) Host:SetHeight(120)
        Host:SetPoint("CENTER", nil, "CENTER", 0, 0)
        Tex = Host:CreateTexture("HostTex", "ARTWORK")
        Tex:SetWidth(64) Tex:SetHeight(32)
        Tex:SetPoint("TOPLEFT", Host, "TOPLEFT", 0, 0)
        Str = Host:CreateFontString("HostStr", "OVERLAY")
        Str:SetWidth(50) Str:SetHeight(10)
        Str:SetPoint("BOTTOMRIGHT", Host, "BOTTOMRIGHT", 0, 0)
        "#,
    )
    .unwrap();
    s
}

/// In the client, Frame's table carries none of the 19 and its lookup tail-calls Region's.
#[test]
fn the_region_map_is_one_function_on_every_widget() {
    let s = vm();
    for m in crate::script::REGION_MAP_METHODS {
        let same: bool = s
            .eval(&format!(
                "return Host.{m} == Tex.{m} and Tex.{m} == Str.{m} and Host.{m} ~= nil"
            ))
            .unwrap();
        assert!(
            same,
            "{m} must be ONE function on frame, texture and string"
        );
    }
}

#[test]
fn a_method_pulled_off_a_frame_works_on_a_texture_and_a_fontstring() {
    let s = vm();
    // Quiver's shape: `Api._Height = WorldFrame.GetHeight`, applied to a Texture and a FontString.
    let (th, sh): (f32, f32) = s
        .eval(
            r#"
            local _Height = Host.GetHeight
            return _Height(Tex), _Height(Str)
            "#,
        )
        .unwrap();
    assert_eq!((th, sh), (32.0, 10.0));

    let (tw, sw): (f32, f32) = s
        .eval(
            r#"
            local _Width = Host.GetWidth
            return _Width(Tex), _Width(Str)
            "#,
        )
        .unwrap();
    assert_eq!((tw, sw), (64.0, 50.0));

    s.run("local _SetW = Host.SetWidth _SetW(Tex, 99)").unwrap();
    assert_eq!(s.eval::<f32>("return Tex:GetWidth()").unwrap(), 99.0);
}

#[test]
fn a_method_pulled_off_a_texture_works_on_a_frame() {
    let s = vm();
    let h: f32 = s.eval("local g = Tex.GetHeight return g(Host)").unwrap();
    assert_eq!(h, 120.0);
    let name: String = s.eval("local n = Str.GetName return n(Host)").unwrap();
    assert_eq!(name, "Host");
    let kind: String = s
        .eval("local t = Tex.GetObjectType return t(Host)")
        .unwrap();
    assert_eq!(kind, "Frame");
}

/// The shared function keeps a per-kind arm; a region's `GetParent` is its owner, never nil.
#[test]
fn one_name_still_dispatches_per_kind() {
    let s = vm();
    let (f, t, g): (String, String, String) = s
        .eval("return Host:GetObjectType(), Tex:GetObjectType(), Str:GetObjectType()")
        .unwrap();
    assert_eq!(
        (f.as_str(), t.as_str(), g.as_str()),
        ("Frame", "Texture", "FontString")
    );
    let owner: String = s.eval("return Tex:GetParent():GetName()").unwrap();
    assert_eq!(owner, "Host");
    assert!(s.eval::<bool>("return Host:GetParent() == nil").unwrap());
}

/// `GetNumPoints` is on the Region map, so every widget answers it.
#[test]
fn get_num_points_answers_on_a_frame_too() {
    let s = vm();
    assert_eq!(s.eval::<i64>("return Host:GetNumPoints()").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return Tex:GetNumPoints()").unwrap(), 1);
    s.run("Host:SetPoint(\"TOPLEFT\", nil, \"TOPLEFT\", 0, 0)")
        .unwrap();
    assert_eq!(s.eval::<i64>("return Host:GetNumPoints()").unwrap(), 2);
    s.run("Host:ClearAllPoints()").unwrap();
    assert_eq!(s.eval::<i64>("return Host:GetNumPoints()").unwrap(), 0);
}

/// Frame, Texture and FontString each register their own copy of these six (Texture `SetAlpha`
/// `0x79b580`, FontString's `0x79cb70`, Frame's `0x774e90`), so `WorldFrame.Show(tex)` fails.
#[test]
fn the_six_look_alikes_are_not_shared() {
    let s = vm();
    for m in [
        "Show",
        "Hide",
        "IsShown",
        "IsVisible",
        "SetAlpha",
        "GetAlpha",
    ] {
        let shared: bool = s.eval(&format!("return Host.{m} == Tex.{m}")).unwrap();
        assert!(
            !shared,
            "{m} is registered per class in 1.12 and must NOT be hoisted onto the Region map"
        );
    }
}

/// The bridge resolves the receiver before either arm runs, and its error names the reason.
#[test]
fn a_non_widget_receiver_raises_and_names_the_reason() {
    let s = vm();
    let err = s
        .eval::<f32>("return Host.GetHeight({})")
        .unwrap_err()
        .to_string();
    assert!(err.contains("T[0] identity"), "unexpected error: {err}");
}
