//! The Minimap widget kind: the zoom API and the extracted content hole.

use super::common::script;
use crate::script::*;
use crate::widget::{MINIMAP_DEFAULT_ZOOM, MINIMAP_ENGINE_CHILDREN, MINIMAP_ZOOM_LEVELS};

/// The zoom rides extraction as [`QuadContent::Minimap`]; `SetZoom` clamps to 0..=5.
#[test]
fn minimap_zoom_api_and_extract() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        m = CreateFrame("Minimap", "TestMinimap")
        m:SetWidth(140); m:SetHeight(140); m:SetPoint("TOPRIGHT", 0, 0)
    "#,
    )
    .unwrap();

    // Both indices seed from the CVar default "3", not 0.
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        MINIMAP_DEFAULT_ZOOM
    );
    assert_eq!(
        s.eval::<u8>("return m:GetZoomLevels()").unwrap(),
        MINIMAP_ZOOM_LEVELS
    );
    s.run("m:SetZoom(3)").unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 3);
    s.run("m:SetZoom(99)").unwrap();
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        MINIMAP_ZOOM_LEVELS - 1,
        "SetZoom clamps at levels-1 like the client's 0x6daa10"
    );
    s.run("m:SetZoom(-2)").unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 0);

    s.resolve();
    let mm = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Minimap { .. }))
        .expect("the Minimap content quad");
    assert!(
        matches!(
            &mm.content,
            QuadContent::Minimap {
                zoom: 0,
                inside_zoom: 3
            }
        ),
        "extract carries both live indices: the outdoor one we drove to 0, the indoor one still at \
         its untouched default, got {:?}",
        mm.content
    );
    assert!(
        mm.rect.is_some(),
        "a sized+anchored Minimap resolves a rect"
    );

    // The zoom methods exist only on the Minimap kind; addons duck-type on them.
    s.run(r#"plain = CreateFrame("Frame", "PlainF")"#).unwrap();
    assert!(
        s.eval::<bool>("return plain.SetZoom == nil").unwrap(),
        "SetZoom must resolve nil on a plain Frame"
    );
}

/// Two zoom indices, outdoor `0x86f698` and indoor `0x86f69c`, picked by the WMO inside flag
/// `0xceaa60`; each persists across the transition.
#[test]
fn minimap_indoor_and_outdoor_zoom_indices_are_independent() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        m = CreateFrame("Minimap", "TestMinimap")
        m:SetWidth(140); m:SetHeight(140); m:SetPoint("TOPRIGHT", 0, 0)
    "#,
    )
    .unwrap();

    s.run("m:SetZoom(2)").unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 2);

    s.set_minimap_inside(true);
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        MINIMAP_DEFAULT_ZOOM,
        "indoors reads the separate indoor index, not the outdoor 2"
    );
    s.run("m:SetZoom(5)").unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 5);

    // Both indices ride out through extraction, whatever the flag says.
    s.resolve();
    let mm = s
        .extract()
        .into_iter()
        .find(|q| matches!(&q.content, QuadContent::Minimap { .. }))
        .expect("the Minimap content quad");
    assert!(
        matches!(
            &mm.content,
            QuadContent::Minimap {
                zoom: 2,
                inside_zoom: 5
            }
        ),
        "extract carries both indices independently, got {:?}",
        mm.content
    );

    s.set_minimap_inside(false);
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        2,
        "the outdoor index survived an indoor zoom"
    );
}

/// `SetZoom` writes the live index and its CVar (`minimapInsideZoom` inside a WMO, `minimapZoom`
/// outside), as the client's `set_zoom` calls `CVar::Set`; the host's seed does not echo back.
#[test]
fn setzoom_persists_the_level_through_the_cvar_it_belongs_to() {
    let mut s = script();
    s.register_cvars([("minimapZoom", "3"), ("minimapInsideZoom", "3")]);
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();

    // The seed is a host write: it moves both indices and queues nothing.
    s.set_minimap_zoom(1, 4);
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 1);
    assert!(s.take_cvar_changes().is_empty(), "the seed must not echo");

    s.run("m:SetZoom(5)").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("minimapZoom".to_string(), "5".to_string())]
    );
    assert_eq!(s.cvar("minimapInsideZoom").as_deref(), Some("3"));

    s.set_minimap_inside(true);
    s.run("m:SetZoom(0)").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("minimapInsideZoom".to_string(), "0".to_string())]
    );
    assert_eq!(s.cvar("minimapZoom").as_deref(), Some("5"));

    // A no-op zoom queues nothing.
    s.run("m:SetZoom(0)").unwrap();
    assert!(s.take_cvar_changes().is_empty());

    // The seed clamps like `set_zoom`.
    s.set_minimap_zoom(99, 99);
    s.set_minimap_inside(false);
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        MINIMAP_ZOOM_LEVELS - 1
    );
}

/// With no host CVars (a bare harness, a glue-only run) the engine-side write is a silent no-op.
#[test]
fn zooming_without_a_registered_cvar_table_is_silent() {
    let mut s = script();
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap") m:SetZoom(4)"#)
        .unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 4);
    assert!(s.take_cvar_changes().is_empty());
    assert!(
        s.take_warnings().is_empty(),
        "no warning for an engine write"
    );
}

/// The `CMinimap` ctor (`0x4edbc0`) builds nine `CSimpleModel` children before the XML `<Frames>`
/// descent, the last being the player arrow `[Minimap+0x338]`, so `({Minimap:GetChildren()})[9]`
/// is the arrow; Questie and pfQuest read the player's heading from it.
#[test]
fn a_minimap_is_born_with_nine_model_children_and_the_ninth_is_the_player_arrow() {
    let mut s = script();
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();

    assert_eq!(
        s.eval::<usize>("return m:GetNumChildren()").unwrap(),
        MINIMAP_ENGINE_CHILDREN,
        "a fresh Minimap has the ctor's nine and nothing else"
    );
    assert_eq!(
        s.eval::<String>("return ({m:GetChildren()})[9]:GetObjectType()")
            .unwrap(),
        "Model",
        "all nine are Models — index 9 included"
    );
    assert!(s
        .eval::<bool>(
            "local n = 0 for _, c in ipairs({m:GetChildren()}) do \
             if c:GetObjectType() == 'Model' then n = n + 1 end end return n == 9"
        )
        .unwrap());

    // Questie's `GetPlayerFacing()`; 0 until the app pushes, the ctor default (`0x76c92d`).
    s.run("function GetPlayerFacing() return ({Minimap:GetChildren()})[9]:GetFacing() end")
        .unwrap();
    s.run(r#"Minimap = m"#).unwrap();
    assert_eq!(s.eval::<f32>("return GetPlayerFacing()").unwrap(), 0.0);

    // `SetPlayerFacing 0x4eb8e0` stores the argument into `[[minimap+0x338]+0x39c]` unchanged.
    s.set_minimap_player_facing(2.5);
    assert_eq!(s.eval::<f32>("return GetPlayerFacing()").unwrap(), 2.5);

    // An addon's own child lands at index 10 and does not displace the arrow.
    s.run(r#"extra = CreateFrame("Frame", nil, m)"#).unwrap();
    s.set_minimap_player_facing(-1.25);
    assert_eq!(s.eval::<f32>("return GetPlayerFacing()").unwrap(), -1.25);
    assert_eq!(
        s.eval::<usize>("return m:GetNumChildren()").unwrap(),
        MINIMAP_ENGINE_CHILDREN + 1
    );
    assert_eq!(
        s.eval::<String>("return ({m:GetChildren()})[10]:GetObjectType()")
            .unwrap(),
        "Frame"
    );

    // The model files come from `CMinimap::LoadXML` (`0x4ee2b0`), so a Lua-built one has none.
    assert!(s
        .eval::<Option<String>>("return ({m:GetChildren()})[9]:GetModel()")
        .unwrap()
        .is_none_or(|p| p.is_empty()));
}

/// A 1.12 method with no getter, so the test reads the arena; pfUI's `modules/minimap.lua:27`
/// squares its minimap with it.
#[test]
fn set_mask_texture_is_state_and_empty_restores_the_default() {
    let s = script();
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    assert_eq!(s.minimap_mask_texture(), None, "fresh = the engine default");

    s.run(r#"m:SetMaskTexture("Interface\\AddOns\\pfUI\\img\\minimap")"#)
        .unwrap();
    assert_eq!(
        s.minimap_mask_texture().as_deref(),
        Some("Interface\\AddOns\\pfUI\\img\\minimap")
    );

    // Empty and nil both restore the engine's circle, never "no mask".
    s.run(r#"m:SetMaskTexture("")"#).unwrap();
    assert_eq!(s.minimap_mask_texture(), None);
    s.run(r#"m:SetMaskTexture("Interface\\Foo") m:SetMaskTexture(nil)"#)
        .unwrap();
    assert_eq!(s.minimap_mask_texture(), None);

    assert!(s.eval::<bool>("return m.GetMaskTexture == nil").unwrap());
}

/// The host-side alpha read for a frame the app draws itself, such as the stock `MiniMapPing`.
#[test]
fn frame_effective_alpha_reads_the_shown_frames_alpha() {
    let s = UiScript::new().unwrap();
    s.run(
        r#"p = CreateFrame("Frame", "PingParent")
           f = CreateFrame("Model", "PingModel", p) f:SetAlpha(0.5) f:Hide()"#,
    )
    .unwrap();
    assert_eq!(s.frame_effective_alpha("PingModel"), None, "hidden");
    assert_eq!(s.frame_effective_alpha("NoSuchFrame"), None);
    s.run("f:Show()").unwrap();
    let a = s.frame_effective_alpha("PingModel").expect("shown");
    assert!((a - 0.5).abs() < 1e-6, "the frame's alpha: {a}");
    s.run("p:Hide()").unwrap();
    assert_eq!(
        s.frame_effective_alpha("PingModel"),
        None,
        "hidden through the parent"
    );
}
