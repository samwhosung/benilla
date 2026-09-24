//! **The Region map is ONE function per name, reached by frames and regions alike** — the
//! [`crate::script::region_map`] law, and the regression guard for bug B267.
//!
//! Its header carries the byte evidence (`Frame ∩ Region = ∅`, parsed out of the two `.data`
//! method tables). What is pinned here is the two things a reader of the code could not otherwise
//! check: that the sharing is real *by identity*, and that it stops exactly at the 19.
//!
//! The reported symptom this file exists for: Quiver's `Api/Index.wow.lua` opens
//! `_Height = WorldFrame.GetHeight`, applies it to a Texture and a FontString, and benilla raised
//! `stale or invalid frame handle` — inside the addon's `VARIABLES_LOADED` handler, four lines
//! before it published `Quiver.CastPetAction`. The addon's own field was nil at macro time because
//! the handler never finished, which is what got filed.

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

/// Every one of the 19 is the **same Lua value** on a Frame, a Texture and a FontString.
///
/// Not a stand-in for the behaviour tests below: identity is what makes the pulled-off-method
/// idiom work *for a method the test author never thought of*. In the client it holds because
/// Frame's table does not carry any of the 19 and its lookup tail-calls Region's; here it holds
/// because one function is written into every table that chain reaches.
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

/// **The bug, reduced to its four lines.** A method pulled off a frame and applied to a region.
#[test]
fn a_method_pulled_off_a_frame_works_on_a_texture_and_a_fontstring() {
    let s = vm();
    // Quiver's own shape: `Api._Height = WorldFrame.GetHeight`, applied to `r.Icon` (a Texture)
    // and `r.Label` (a FontString), from a table built at file scope.
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

    // …and the setter half, which is the same split and would have been the next wall.
    s.run("local _SetW = Host.SetWidth _SetW(Tex, 99)").unwrap();
    assert_eq!(s.eval::<f32>("return Tex:GetWidth()").unwrap(), 99.0);
}

/// The other direction — a method pulled off a **region** and applied to a **frame**. Same one
/// function, so this cannot be made to work by special-casing the reported direction.
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

/// Each receiver keeps its OWN behaviour behind the shared name — the per-kind arms are not
/// collapsed into one. `GetObjectType` is the cleanest witness (three different answers), and
/// `GetParent` is the one whose two arms genuinely differ (a region's owner is never nil).
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
    // The region arm answers its OWNER; the frame arm answers nil for a parentless top frame.
    let owner: String = s.eval("return Tex:GetParent():GetName()").unwrap();
    assert_eq!(owner, "Host");
    assert!(s.eval::<bool>("return Host:GetParent() == nil").unwrap());
}

/// `GetNumPoints` — on the Region map, so **every** widget answers it. The frame arm did not
/// exist until the map was collapsed to one implementation; the region arm shipped alone.
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

/// **The negative case, and the control that keeps it honest.** These six look like they belong to
/// the Region map and do not: Frame, Texture and FontString each register their *own* at different
/// addresses — Texture `SetAlpha 0x79b580` vs FontString `0x79cb70`, and Frame's own `0x774e90`
/// twin. So `WorldFrame.Show(someTexture)` fails on the real client, and hoisting them here to make
/// more addon code work would be the superset decision 1189 had to take back out.
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

/// A receiver that is not a widget at all raises, and says *why* — the bridge resolves the
/// receiver before either arm runs, so this error must not be flattened into a generic one.
#[test]
fn a_non_widget_receiver_raises_and_names_the_reason() {
    let s = vm();
    let err = s
        .eval::<f32>("return Host.GetHeight({})")
        .unwrap_err()
        .to_string();
    assert!(err.contains("T[0] identity"), "unexpected error: {err}");
}
