//! The Minimap widget kind (decision 0203): zoom API + the extracted content hole.

use super::common::script;
use crate::script::*;
use crate::widget::{MINIMAP_DEFAULT_ZOOM, MINIMAP_ENGINE_CHILDREN, MINIMAP_ZOOM_LEVELS};

/// A `<Minimap>`-kind frame carries its zoom out through extraction as [`QuadContent::Minimap`]
/// at the frame's own draw slot, and the zoom API clamps like the client's `set_zoom` (0..=5).
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

    // Defaults + the clamp law. Both indices seed from the CVar default "3", not 0.
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

    // The widget's own slot extracts as the Minimap content hole, carrying the zoom.
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

    // Duck-typing: the zoom API must NOT leak onto other kinds (per-kind method registries).
    s.run(r#"plain = CreateFrame("Frame", "PlainF")"#).unwrap();
    assert!(
        s.eval::<bool>("return plain.SetZoom == nil").unwrap(),
        "SetZoom must resolve nil on a plain Frame"
    );
}

/// The client keeps **two** zoom indices and routes `GetZoom`/`SetZoom` on WMO containment (the
/// inside flag `0xceaa60` → outdoor `0x86f698` / indoor `0x86f69c`). Each persists across the
/// transition: zooming the inn's map right in must not disturb the zoom you left outside.
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

    // Outside: the zoom API drives the outdoor index.
    s.run("m:SetZoom(2)").unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 2);

    // Step inside: the API now reads/writes the indoor index, still at its own untouched default.
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

    // Step back outside: the outdoor zoom is exactly where we left it.
    s.set_minimap_inside(false);
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        2,
        "the outdoor index survived an indoor zoom"
    );
}

/// **The level persists** (decision 1131). `SetZoom` writes the live index *and* the matching CVar
/// — `minimapInsideZoom` while inside a WMO, `minimapZoom` outside — the client's own `set_zoom` →
/// `CVar::Set` pair, which is the whole reason a zoom survives a restart. The host push in the
/// other direction (the seed) does *not* echo back as a change.
#[test]
fn setzoom_persists_the_level_through_the_cvar_it_belongs_to() {
    let mut s = script();
    s.register_cvars([("minimapZoom", "3"), ("minimapInsideZoom", "3")]);
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();

    // The seed is a HOST write: it moves both live indices and queues nothing (the host is the
    // one that just read them off disk — an echo would re-dirty the file it loaded).
    s.set_minimap_zoom(1, 4);
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 1);
    assert!(s.take_cvar_changes().is_empty(), "the seed must not echo");

    // Outdoors, a zoom writes the outdoor CVar and only that one.
    s.run("m:SetZoom(5)").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("minimapZoom".to_string(), "5".to_string())]
    );
    assert_eq!(s.cvar("minimapInsideZoom").as_deref(), Some("3"));

    // Indoors it writes the indoor one — the two levels persist separately, like the indices.
    s.set_minimap_inside(true);
    s.run("m:SetZoom(0)").unwrap();
    assert_eq!(
        s.take_cvar_changes(),
        vec![("minimapInsideZoom".to_string(), "0".to_string())]
    );
    assert_eq!(s.cvar("minimapZoom").as_deref(), Some("5"));

    // A no-op zoom queues nothing: quiet frames stay quiet, and the config stays clean.
    s.run("m:SetZoom(0)").unwrap();
    assert!(s.take_cvar_changes().is_empty());

    // The seed clamps like `set_zoom` does, so a hand-edited config.toml cannot seed out of range.
    s.set_minimap_zoom(99, 99);
    s.set_minimap_inside(false);
    assert_eq!(
        s.eval::<u8>("return m:GetZoom()").unwrap(),
        MINIMAP_ZOOM_LEVELS - 1
    );
}

/// A VM whose host registered nothing (a bare test harness, a glue-only run) still zooms — the
/// engine-side CVar write is a silent no-op there, not a warning: engine writes are code, not UI
/// content, so a miss means this build's host does not back the var.
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

/// **The nine engine-created `Model` children, and why `[9]` is the player arrow.**
///
/// The `CMinimap` ctor `0x4edbc0` builds nine `CSimpleModel` children parented to the Minimap
/// before anything else touches the widget, in three source-ordered groups (`__LINE__` 1424 / 1438
/// / 1450), the last being
/// `[Minimap+0x338]`, the player arrow. Because both linkers append at the tail and the ctor runs
/// before the XML `<Frames>` descent, `({Minimap:GetChildren()})[9]` is that arrow on a stock
/// client — which is exactly what Questie's `QuestieArrow.lua` and pfQuest's `compat/client.lua`
/// index, unguarded, to read the player's heading.
///
/// This test is written the way those addons read it, not the way we store it.
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

    // Questie's `GetPlayerFacing()`, verbatim. It is `0` until the app pushes, which is the
    // client's own ctor default (`[frame+0x39c] = 0` at `0x76c92d`) — not nil, and not an error.
    s.run("function GetPlayerFacing() return ({Minimap:GetChildren()})[9]:GetFacing() end")
        .unwrap();
    s.run(r#"Minimap = m"#).unwrap();
    assert_eq!(s.eval::<f32>("return GetPlayerFacing()").unwrap(), 0.0);

    // `SetPlayerFacing 0x4eb8e0` writes the argument into `[[minimap+0x338]+0x39c]` VERBATIM — no
    // negation, no offset, no unit change (a `mov [ecx+0x39c],edx` of the dword it was handed).
    s.set_minimap_player_facing(2.5);
    assert_eq!(s.eval::<f32>("return GetPlayerFacing()").unwrap(), 2.5);

    // It reaches the arrow through the Minimap's own slot, so an addon appending its own child
    // cannot displace it: a later `CreateFrame(_, _, Minimap)` lands at index 10.
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

    // The ctor only *constructs* them; the model files are `CMinimap::LoadXML 0x4ee2b0`'s to
    // assign, so a Lua-built Minimap with no XML behind it has nine file-less Models — exactly as
    // the reference does.
    assert!(s
        .eval::<Option<String>>("return ({m:GetChildren()})[9]:GetModel()")
        .unwrap()
        .is_none_or(|p| p.is_empty()));
}

/// **`Minimap:SetMaskTexture` is state, and an empty path restores the default rather than
/// unmasking the map.**
///
/// A real 1.12 method (the name is in the 5875 image) with no getter beside it, so the readback
/// here is through the arena — the same shape `simplehtml`'s and `modelframe`'s tests use for a
/// write-only verb. pfUI's `modules/minimap.lua:27` is the caller that matters: swapping this art
/// is how its square minimap becomes square.
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

    // Empty and nil both mean "back to the engine's circle" — never "no mask at all". A
    // nil-means-unmasked reading would hand every mistyped path a silently square minimap.
    s.run(r#"m:SetMaskTexture("")"#).unwrap();
    assert_eq!(s.minimap_mask_texture(), None);
    s.run(r#"m:SetMaskTexture("Interface\\Foo") m:SetMaskTexture(nil)"#)
        .unwrap();
    assert_eq!(s.minimap_mask_texture(), None);

    // There is no getter in 1.12, and we do not invent one (decision 1189).
    assert!(s.eval::<bool>("return m.GetMaskTexture == nil").unwrap());
}

/// `frame_effective_alpha` — the host-side read behind a frame whose pixels the app draws (the
/// stock `MiniMapPing` model, 1974): `None` while hidden, or hidden through a parent; the
/// effective alpha while effectively visible.
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
