//! The Minimap widget kind: the zoom API and the extracted content hole.

use super::common::script;
use crate::script::*;
use crate::widget::{MINIMAP_DEFAULT_ZOOM, MINIMAP_ENGINE_CHILDREN, MINIMAP_ZOOM_LEVELS};

/// The zoom rides extraction as [`QuadContent::Minimap`]; `SetZoom` caps at 5.
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
        s.eval::<u32>("return m:GetZoom()").unwrap(),
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
        "SetZoom caps at levels-1 (`0x6da8fd`)"
    );
    s.run("m:SetZoom(0)").unwrap();
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
        s.eval::<u32>("return m:GetZoom()").unwrap(),
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

    // The seed is the CVar's integer, raw, as the world entry copies it (`0x6d9013`).
    s.set_minimap_zoom(99, 99);
    s.set_minimap_inside(false);
    assert_eq!(s.eval::<u32>("return m:GetZoom()").unwrap(), 99);
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

    // `CreateFrame`'s builder runs `CMinimap::LoadXML` (`0x4ee2b0`) too, so the stock models.
    assert_eq!(
        s.eval::<String>("return ({m:GetChildren()})[9]:GetModel()")
            .unwrap(),
        crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL
    );
}

/// `SetMaskTexture` (`0x4ee4a0`), a 1.12 method with no getter, so the test reads the statics;
/// pfUI's `modules/minimap.lua:27` squares its minimap with it. The load runs inside the call: a
/// file that does not resolve stores the green fallback and then raises, and an empty name stores
/// no texture at all.
#[test]
fn set_mask_texture_loads_inside_the_call_and_raises_on_a_missing_file() {
    use crate::widget::{MinimapArt, MinimapArtSlot};
    let mut s = script();
    s.set_texture_probe(Box::new(|path: &str| !path.contains("Nope") && path != "5"));
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    let mask = |s: &UiScript| s.minimap_art(MinimapArtSlot::Mask).map(|(_, a)| a);
    assert_eq!(mask(&s), None, "fresh: the process's mask stands");

    s.run(r#"m:SetMaskTexture("Interface\\AddOns\\pfUI\\img\\minimap")"#)
        .unwrap();
    assert_eq!(
        mask(&s),
        Some(MinimapArt::File(
            "Interface\\AddOns\\pfUI\\img\\minimap".into()
        ))
    );

    // A missing file: the green solid is stored, then the error raises at the caller's line.
    let err = s.run(r#"m:SetMaskTexture("Interface\\Nope")"#).unwrap_err();
    assert_eq!(
        crate::script::lua_message(&err),
        "[string \"(chunk)\"]:1: TestMinimap:SetMaskTexture(): Couldn't load the file Interface\\Nope"
    );
    assert_eq!(mask(&s), Some(MinimapArt::Missing));
    // A number is a name (`lua_isstring`), loaded as its text.
    let err = s
        .run(r#"CreateFrame("Minimap"):SetMaskTexture(5)"#)
        .unwrap_err();
    assert_eq!(
        crate::script::lua_message(&err),
        "[string \"(chunk)\"]:1: <unnamed>:SetMaskTexture(): Couldn't load the file 5"
    );

    // An empty name loads nothing and raises nothing: the map stops drawing (`0x4ec968`).
    s.run(r#"m:SetMaskTexture("")"#).unwrap();
    assert_eq!(mask(&s), Some(MinimapArt::Null));

    // `lua_isstring(2)` (`0x4ee532`): a missing or nil path raises `0x84c7fc` and keeps the mask.
    s.run(r#"m:SetMaskTexture("Interface\\Foo")"#).unwrap();
    for chunk in [
        "m:SetMaskTexture()",
        "m:SetMaskTexture(nil)",
        "m:SetMaskTexture({})",
    ] {
        assert_eq!(
            crate::script::lua_message(&s.run(chunk).expect_err(chunk)),
            "[string \"(chunk)\"]:1: Usage: TestMinimap:SetMaskTexture(\"file\")",
            "{chunk}"
        );
    }
    assert_eq!(mask(&s), Some(MinimapArt::File("Interface\\Foo".into())));
    assert_eq!(
        crate::script::lua_message(
            &s.run(r#"CreateFrame("Minimap"):SetMaskTexture(false)"#)
                .unwrap_err()
        ),
        "[string \"(chunk)\"]:1: Usage: <unnamed>:SetMaskTexture(\"file\")"
    );

    // Each call is stamped, so the host applies it once.
    let (a, _) = s.minimap_art(MinimapArtSlot::Mask).unwrap();
    s.run(r#"m:SetMaskTexture("Interface\\Foo")"#).unwrap();
    let (b, _) = s.minimap_art(MinimapArtSlot::Mask).unwrap();
    assert!(b > a, "a repeat is a new call");

    assert!(s.eval::<bool>("return m.GetMaskTexture == nil").unwrap());
}

/// The host-side alpha read for a frame the app draws itself, such as the stock `MiniMapPing`.
#[test]
fn frame_alpha_reads_the_shown_frames_alpha() {
    let s = UiScript::new().unwrap();
    s.run(
        r#"p = CreateFrame("Frame", "PingParent")
           f = CreateFrame("Model", "PingModel", p) f:SetAlpha(0.5) f:Hide()"#,
    )
    .unwrap();
    assert_eq!(s.frame_alpha("PingModel"), None, "hidden");
    assert_eq!(s.frame_alpha("NoSuchFrame"), None);
    s.run("f:Show()").unwrap();
    let a = s.frame_alpha("PingModel").expect("shown");
    assert_eq!(a, 127.0 / 255.0, "the frame's alpha byte: {a}");
    s.run("p:Hide()").unwrap();
    assert_eq!(
        s.frame_alpha("PingModel"),
        None,
        "hidden through the parent"
    );
}

/// `PingLocation` (`0x4eeca0`) turns its two offsets into fractions of the frame's own given size,
/// the first over the height and negated, the second over the width, and offsets the player's
/// world point by them times the zoom diameter, world X by the second and Y by the first, with no
/// disc test and whatever the frame's scale. The ping applies inside the call (`0x4ee330`):
/// `MINIMAP_PING` fires before it returns.
#[test]
fn ping_location_pings_the_world_point_its_fractions_name_inside_the_call() {
    let mut s = script();
    s.run(
        r#"m = CreateFrame("Minimap", "TestMinimap")
           m:SetWidth(140) m:SetHeight(100)
           seen = {}
           local f = CreateFrame("Frame")
           f:RegisterEvent("MINIMAP_PING")
           f:SetScript("OnEvent", function()
               local gx, gy = m:GetPingPosition()
               table.insert(seen, {arg1, arg2, arg3, gx, gy})
           end)"#,
    )
    .unwrap();

    // No player object: nothing happens (`0x4eecdf`), and the getter answers zeros (`0x4eeff6`).
    s.run("m:PingLocation(10, 20)").unwrap();
    assert!(s.take_minimap_pings_sent().is_empty());
    assert_eq!(s.eval::<usize>("return table.getn(seen)").unwrap(), 0);
    assert_eq!(
        s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap(),
        (0.0, 0.0)
    );

    s.set_minimap_player(Some((100.0, 200.0)));
    let d = 2.0 * f64::from(crate::widget::minimap_radius_yd(3, 3, false));
    let k = 1.0 / d;
    // Before any ping the stored point is world (0, 0), which the getter does not detect.
    let (gx, gy) = s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap();
    assert!(
        (gx - 200.0 * k).abs() < 1e-6 && (gy + 100.0 * k).abs() < 1e-6,
        "{gx} {gy}"
    );

    s.run("m:PingLocation(10, 20) after = table.getn(seen)")
        .unwrap();
    assert_eq!(
        s.eval::<usize>("return after").unwrap(),
        1,
        "fired inside the call"
    );
    let sent = s.take_minimap_pings_sent();
    let want = (100.0 + (20.0 / 140.0) * d, 200.0 - (10.0 / 100.0) * d);
    assert_eq!(sent.len(), 1);
    assert!(
        (f64::from(sent[0].0) - want.0).abs() < 1e-3
            && (f64::from(sent[0].1) - want.1).abs() < 1e-3,
        "the axes swap: x over the height, y over the width: {sent:?} vs {want:?}"
    );
    let ev = s
        .eval::<(String, f64, f64, f64, f64)>(
            "local e = seen[1] return e[1], e[2], e[3], e[4], e[5]",
        )
        .unwrap();
    assert_eq!(ev.0, "player");
    assert!(
        (ev.1 - 0.1).abs() < 1e-6 && (ev.2 - 20.0 / 140.0).abs() < 1e-6,
        "{ev:?}"
    );
    assert!(
        (ev.3 - 0.1).abs() < 1e-6 && (ev.4 - 20.0 / 140.0).abs() < 1e-6,
        "{ev:?}"
    );

    // The scale does not enter: the fractions are of the frame's own size.
    s.run("m:SetScale(0.5) m:PingLocation(10, 20)").unwrap();
    let scaled = s.take_minimap_pings_sent();
    assert_eq!(scaled, sent, "same fractions at scale 0.5");

    // Past the rim still pings: no disc test.
    s.run("m:PingLocation(0, 130)").unwrap();
    assert_eq!(s.take_minimap_pings_sent().len(), 1, "outside the disc");

    // Without two numbers, the player's own point, and no `this` is needed.
    for chunk in [
        "m:PingLocation()",
        "m:PingLocation(5)",
        "m.PingLocation()",
        "m:PingLocation(nil, 3)",
    ] {
        s.run(chunk).unwrap();
        assert_eq!(s.take_minimap_pings_sent(), vec![(100.0, 200.0)], "{chunk}");
    }
    // With two numbers, the member prologue runs.
    let err = crate::script::lua_message(&s.run("m.PingLocation(nil, 1, 2)").unwrap_err());
    assert!(
        err.ends_with("Attempt to find 'this' in non-table object (used '.' instead of ':' ?)"),
        "{err}"
    );

    // The getter recomputes from the stored point at every call, as the player moves.
    s.run("m:PingLocation(0, 70)").unwrap();
    s.take_minimap_pings_sent();
    let before = s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap();
    s.set_minimap_player(Some((110.0, 200.0)));
    let after = s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap();
    assert!(
        (after.1 - (before.1 - 10.0 * k)).abs() < 1e-6,
        "{before:?} → {after:?}"
    );

    // A member's ping applies the same way, with its token.
    s.apply_minimap_ping((100.0, 200.0), "party2");
    assert_eq!(
        s.eval::<String>("return seen[table.getn(seen)][1]")
            .unwrap(),
        "party2"
    );
}

/// The two places the ping's numbers round to a float: `MINIMAP_PING`'s first number is rounded
/// before it is negated (`0x4ee473`) and its second stays a double (`0x4ee476`); `GetPingPosition`
/// rounds its second (`0x4ef05b`) and not its first.
#[test]
fn the_ping_numbers_round_to_a_float_where_the_reference_stores_one() {
    let mut s = script();
    s.run(
        r#"m = CreateFrame("Minimap", "TestMinimap")
           local f = CreateFrame("Frame")
           f:RegisterEvent("MINIMAP_PING")
           f:SetScript("OnEvent", function() a1, a2 = arg2, arg3 end)"#,
    )
    .unwrap();
    s.set_minimap_player(Some((0.0, 0.0)));
    s.apply_minimap_ping((10.0, -7.0), "party1");
    let k = 1.0 / (2.0 * f64::from(crate::widget::minimap_radius_yd(3, 3, false)));
    let (first, second) = (7.0 * k, 10.0 * k);
    let rounded = |v: f64| f64::from(v as f32);
    assert!(
        rounded(first) != first && rounded(second) != second,
        "the case tells them apart"
    );
    let (a1, a2) = s.eval::<(f64, f64)>("return a1, a2").unwrap();
    assert_eq!((a1, a2), (rounded(first), second), "the event");
    let (g1, g2) = s.eval::<(f64, f64)>("return m:GetPingPosition()").unwrap();
    assert_eq!((g1, g2), (first, rounded(second)), "the getter");
}

/// The zoom cells, the indoor byte and the mask are statics (`0x86f698`, `0x86f69c`, `0xceaa60`,
/// `0xbc7968`): every Minimap frame shows the one zoom and the one mask, and the zoom and ping
/// getters take no `this` (`0x4eebe0`, `0x4eec20`, `0x4eec60`, `0x4eefd0`).
#[test]
fn the_minimap_state_is_one_set_of_statics_and_its_getters_take_no_receiver() {
    let mut s = script();
    s.run(
        r#"a = CreateFrame("Minimap", "MapA")
           b = CreateFrame("Minimap", "MapB")
           plain = CreateFrame("Frame", "PlainFrame")"#,
    )
    .unwrap();
    s.run("a:SetZoom(1)").unwrap();
    assert_eq!(s.eval::<u8>("return b:GetZoom()").unwrap(), 1, "one zoom");
    for chunk in [
        "return a.GetZoom()",
        "return a.GetZoom(plain)",
        "return a.GetZoom(5)",
    ] {
        assert_eq!(s.eval::<u8>(chunk).unwrap(), 1, "{chunk}");
    }
    assert_eq!(
        s.eval::<u8>("return a.GetZoomLevels(plain)").unwrap(),
        MINIMAP_ZOOM_LEVELS
    );
    s.run("a.SetZoom(plain, 4)").unwrap();
    assert_eq!(s.eval::<u8>("return b:GetZoom()").unwrap(), 4);
    assert_eq!(s.arity("a.GetPingPosition()").unwrap(), 2);
    assert_eq!(s.arity("a.GetPingPosition(plain)").unwrap(), 2);

    // The indoor byte is one flag for both frames.
    s.set_minimap_inside(true);
    s.run("b:SetZoom(2)").unwrap();
    assert_eq!(s.eval::<u8>("return a:GetZoom()").unwrap(), 2);
    s.set_minimap_inside(false);
    assert_eq!(s.eval::<u8>("return a:GetZoom()").unwrap(), 4);

    // One mask: the later frame's call wins.
    s.run(r#"a:SetMaskTexture("Interface\\A") b:SetMaskTexture("Interface\\B")"#)
        .unwrap();
    assert_eq!(
        s.minimap_art(crate::widget::MinimapArtSlot::Mask)
            .map(|(_, a)| a),
        Some(crate::widget::MinimapArt::File("Interface\\B".into()))
    );

    // The setters keep the member prologue, with the reference's three texts.
    for (chunk, want) in [
        (
            r#"a.SetMaskTexture("x")"#,
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ),
        (
            r#"a.SetMaskTexture({}, "x")"#,
            "Attempt to find 'this' in non-framescript object",
        ),
        (
            r#"a.SetMaskTexture(plain, "x")"#,
            "Wrong object type for member function",
        ),
    ] {
        let err = crate::script::lua_message(&s.run(chunk).expect_err(chunk));
        assert!(err.ends_with(want), "{chunk}: {err}");
    }
}

/// `SetZoom` (`0x4eec60`): `lua_isnumber(2)` or `Usage: SetZoom(level)` (`0x84c96c`), then `__ftol`
/// (`0x40a2b0`) and `SetZoomIndex` (`0x6da8e0`), whose unsigned compare against 5 (`0x6da8fd`,
/// `0x6da902`) makes a negative level the top one; the CVar is written only on a change.
#[test]
fn set_zoom_truncates_caps_unsigned_and_writes_its_cvar_only_on_a_change() {
    let mut s = script();
    s.register_cvars([("minimapZoom", "3"), ("minimapInsideZoom", "3")]);
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    for (level, want) in [
        ("-1", 5),
        ("0", 0),
        ("-4", 5),
        ("-0.9", 0),
        ("4.9", 4),
        ("'2'", 2),
        ("2^32 + 1", 1),
        ("0/0", 0),
        ("1/0", 0),
        ("-2^31", 5),
    ] {
        s.run(&format!("m:SetZoom({level})")).unwrap();
        assert_eq!(
            s.eval::<u8>("return m:GetZoom()").unwrap(),
            want,
            "SetZoom({level})"
        );
    }
    for chunk in [
        "m:SetZoom()",
        "m:SetZoom(nil)",
        "m:SetZoom({})",
        "m:SetZoom('x')",
    ] {
        assert_eq!(
            crate::script::lua_message(&s.run(chunk).expect_err(chunk)),
            "[string \"(chunk)\"]:1: Usage: SetZoom(level)",
            "{chunk}"
        );
    }

    // -1 is 5, and the CVar reads it.
    s.take_cvar_changes();
    s.run("m:SetZoom(-1)").unwrap();
    assert_eq!(s.cvar("minimapZoom").as_deref(), Some("5"));
    // An unchanged index writes nothing, whatever the CVar holds by then.
    s.run(r#"SetCVar("minimapZoom", "1")"#).unwrap();
    s.take_cvar_changes();
    s.run("m:SetZoom(5)").unwrap();
    assert!(s.take_cvar_changes().is_empty(), "no change, no write");
    assert_eq!(s.cvar("minimapZoom").as_deref(), Some("1"));
}

/// Every world entry runs the minimap initializer after `PLAYER_ENTERING_WORLD` (`0x4ed9d0` at
/// `0x49096f`): each zoom CVar's integer goes into its cell (`0x6d9008`-`0x6d901f`) and
/// `MINIMAP_UPDATE_ZOOM` fires (`0x6d902a`), so a `SetCVar` takes effect at the next entry.
#[test]
fn every_world_entry_reseeds_the_zoom_from_its_cvars_and_signals_it() {
    let mut s = script();
    s.register_cvars([("minimapZoom", "3"), ("minimapInsideZoom", "3")]);
    s.run(
        r#"m = CreateFrame("Minimap", "TestMinimap")
           seen = {}
           local f = CreateFrame("Frame")
           f:RegisterEvent("PLAYER_ENTERING_WORLD")
           f:RegisterEvent("MINIMAP_UPDATE_ZOOM")
           f:SetScript("OnEvent", function()
               table.insert(seen, event .. " " .. m:GetZoom())
           end)
           SetCVar("minimapZoom", "0")
           SetCVar("minimapInsideZoom", "4")"#,
    )
    .unwrap();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 3, "not yet");
    s.fire_world_enter();
    assert_eq!(
        s.eval::<String>("return table.concat(seen, ',')").unwrap(),
        "PLAYER_ENTERING_WORLD 3,MINIMAP_UPDATE_ZOOM 0"
    );
    s.set_minimap_inside(true);
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 4);

    // A worldport's entry reseeds again; the buttons' own writes keep the CVar, so it restores.
    s.set_minimap_inside(false);
    s.run("m:SetZoom(2)").unwrap();
    s.fire_world_enter();
    assert_eq!(s.eval::<u8>("return m:GetZoom()").unwrap(), 2);
}

/// The world entry's copy is raw (`0x6d9013`): a CVar of `-1` or `9` stands in its cell, and
/// `GetZoom` answers the cell as unsigned (`fild` of the cell over a zero high dword, `0x4eec31`).
/// Only the radius read caps (a `Deviation:`, `0x6da9d5`); `SetZoom` then caps as it always does.
#[test]
fn the_zoom_cell_keeps_the_raw_cvar_integer_and_only_the_radius_read_caps() {
    let mut s = script();
    s.register_cvars([("minimapZoom", "3"), ("minimapInsideZoom", "3")]);
    s.run(
        r#"m = CreateFrame("Minimap", "TestMinimap")
           SetCVar("minimapZoom", "-1")
           SetCVar("minimapInsideZoom", "9")"#,
    )
    .unwrap();
    s.fire_world_enter();
    assert_eq!(
        s.eval::<f64>("return m:GetZoom()").unwrap(),
        4_294_967_295.0
    );
    assert_eq!(
        s.minimap_radius_yd(),
        crate::widget::minimap_radius_yd(5, 3, false),
        "the outdoor table's last entry"
    );
    s.set_minimap_inside(true);
    assert_eq!(s.eval::<f64>("return m:GetZoom()").unwrap(), 9.0);
    assert_eq!(s.minimap_radius_yd(), 25.0, "the indoor table's last entry");
    // `SetZoom`'s compare is against the raw cell, so 5 is a change and writes.
    s.take_cvar_changes();
    s.run("m:SetZoom(5)").unwrap();
    assert_eq!(s.eval::<f64>("return m:GetZoom()").unwrap(), 5.0);
    assert_eq!(s.cvar("minimapInsideZoom").as_deref(), Some("5"));
}

/// `SetIconTexture` (`0x4ee640`) and `SetBlipTexture` (`0x4ee7e0`) are `SetMaskTexture`'s body on
/// the landmark and dot atlases (`0xbc827c`, `0xbc822c`), each with its own texts.
#[test]
fn the_icon_and_blip_setters_replace_their_atlases_with_their_own_texts() {
    use crate::widget::{MinimapArt, MinimapArtSlot};
    let mut s = script();
    s.set_texture_probe(Box::new(|path: &str| !path.contains("Nope")));
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    let art = |s: &UiScript, slot| s.minimap_art(slot).map(|(_, a)| a);
    for (name, slot) in [
        ("SetIconTexture", MinimapArtSlot::Icons),
        ("SetBlipTexture", MinimapArtSlot::Blips),
    ] {
        s.run(&format!(r#"m:{name}("Interface\\AddOns\\X\\atlas")"#))
            .unwrap();
        assert_eq!(
            art(&s, slot),
            Some(MinimapArt::File("Interface\\AddOns\\X\\atlas".into())),
            "{name}"
        );
        let err = s.run(&format!(r#"m:{name}("Nope")"#)).unwrap_err();
        assert_eq!(
            crate::script::lua_message(&err),
            format!("[string \"(chunk)\"]:1: TestMinimap:{name}(): Couldn't load the file Nope")
        );
        assert_eq!(art(&s, slot), Some(MinimapArt::Missing), "{name}");
        let err = s.run(&format!("m:{name}()")).unwrap_err();
        assert_eq!(
            crate::script::lua_message(&err),
            format!("[string \"(chunk)\"]:1: Usage: TestMinimap:{name}(\"file\")")
        );
        s.run(&format!(r#"m:{name}("")"#)).unwrap();
        assert_eq!(art(&s, slot), Some(MinimapArt::Null), "{name}");
        let err = s
            .run(&format!(r#"m.{name}(UIParent or {{}}, "x")"#))
            .unwrap_err();
        assert!(
            crate::script::lua_message(&err)
                .ends_with("Attempt to find 'this' in non-framescript object"),
            "{name}: {err}"
        );
    }
    // Each slot is its own: the mask stands.
    assert_eq!(art(&s, MinimapArtSlot::Mask), None);
}

/// The ctor's nine children (`0x4edbc0`): all anchored CENTER on the minimap's CENTER, the eight
/// arrow frames the disc's size, scaled 0.6 and hidden, the first five a level above the other
/// four, and the player arrow left shown at 0.001 layout units off centre, scaled `G48 · 5/3`.
#[test]
fn the_engine_children_are_anchored_sized_and_hidden_as_the_ctor_leaves_them() {
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"p = CreateFrame("Frame", "Holder") p:SetFrameLevel(4)
           m = CreateFrame("Minimap", "TestMinimap", p)
           m:SetWidth(140) m:SetHeight(140) m:SetPoint("CENTER", 0, 0)
           kids = {m:GetChildren()}"#,
    )
    .unwrap();
    s.resolve();
    let level = s.eval::<i64>("return m:GetFrameLevel()").unwrap();
    for i in 1..=9 {
        let get = |what: &str| format!("local c = kids[{i}] return {what}");
        let (point, rel, rel_point, x, y) = s
            .eval::<(String, String, String, f64, f64)>(&get(
                "(function() local a, r, b, x, y = c:GetPoint(1) return a, r:GetName(), b, x, y end)()",
            ))
            .unwrap();
        assert_eq!(
            (point.as_str(), rel.as_str(), rel_point.as_str()),
            ("CENTER", "TestMinimap", "CENTER"),
            "child {i}"
        );
        let shown = s.eval::<bool>(&get("c:IsShown() == 1")).unwrap();
        let child_level = s.eval::<i64>(&get("c:GetFrameLevel()")).unwrap();
        let scale = s.eval::<f64>(&get("c:GetModelScale()")).unwrap();
        if i < 9 {
            // 0.11 layout units at 4:3, where a layout unit is 1280 FrameXML units.
            let w = s.eval::<f64>(&get("c:GetWidth()")).unwrap();
            let h = s.eval::<f64>(&get("c:GetHeight()")).unwrap();
            assert!(
                (w - 140.8).abs() < 1e-3 && (h - 140.8).abs() < 1e-3,
                "child {i}: {w}x{h}"
            );
            assert!(!shown, "child {i} is hidden");
            assert!((scale - 0.6).abs() < 1e-6, "child {i}: {scale}");
            assert!((x, y) == (0.0, 0.0), "child {i}: {x},{y}");
            let want = if i <= 5 { level + 2 } else { level + 1 };
            assert_eq!(child_level, want, "child {i}");
        } else {
            assert!(shown, "the player arrow stays shown");
            assert!(
                (x - 1.28).abs() < 1e-3 && (y - 1.28).abs() < 1e-3,
                "{x},{y}"
            );
            assert!((scale - 1.0).abs() < 1e-6, "G48 · 5/3 is 1 at 4:3: {scale}");
            assert_eq!(child_level, level + 1);
        }
    }
}

/// `SetArrowModel` (`0x4ee980`) puts its file on the eight arrow frames and re-arms their art, and
/// answers no error for a file that does not load (the reference's error cube; benilla's pane
/// draws nothing there, #330); `SetPlayerModel`
/// (`0x4eeab0`) sets the player arrow alone and raises when it is left with no model, as `""`
/// leaves it (`0x4ee286`).
#[test]
fn the_model_setters_put_their_files_on_the_engine_children() {
    let s = script();
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    s.run(r#"m:SetArrowModel("Nope\\Arrow.mdx")"#).unwrap();
    for i in 1..=8 {
        let c = s.minimap_child("TestMinimap", i).unwrap();
        assert_eq!(c.path.as_deref(), Some("Nope\\Arrow.mdx"), "child {i}");
        assert_eq!(c.sequence, if i <= 5 { 165 } else { 166 }, "child {i}");
    }
    assert_eq!(
        s.minimap_child("TestMinimap", 9).unwrap().path.as_deref(),
        Some(crate::widget::MINIMAP_DEFAULT_PLAYER_MODEL),
        "the player arrow keeps its own file"
    );

    s.run(r#"m:SetPlayerModel("Interface\\Minimap\\MinimapArrow.mdx")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>("return ({m:GetChildren()})[9]:GetModel()")
            .unwrap(),
        "Interface\\Minimap\\MinimapArrow.mdx"
    );
    let err = s.run(r#"m:SetPlayerModel("")"#).unwrap_err();
    assert_eq!(
        crate::script::lua_message(&err),
        "[string \"(chunk)\"]:1: TestMinimap:SetPlayerModel(): Couldn't load the file "
    );
    assert!(s
        .eval::<Option<String>>("return ({m:GetChildren()})[9]:GetModel()")
        .unwrap()
        .is_none());
    for (chunk, want) in [
        (
            "m:SetArrowModel()",
            "Usage: TestMinimap:SetArrowModel(\"file\")",
        ),
        (
            "m:SetPlayerModel({})",
            "Usage: TestMinimap:SetPlayerModel(\"file\")",
        ),
        (
            r#"m.SetArrowModel("x")"#,
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ),
    ] {
        let err = crate::script::lua_message(&s.run(chunk).expect_err(chunk));
        assert!(err.ends_with(want), "{chunk}: {err}");
    }
}

/// The render's arrow half (`0x4ec950`): a rebuilt list shows one rim frame per record at the
/// level base plus its slot, playing its art, and hides the rest; every frame aims each record's
/// frame at `(C − A·sin b, C + A·cos b)`, facing `b`. Indoors only a record with `+0x44` shows
/// (`0x4ed308`). A party or pet frame shows while it has a bearing (`0x4ed3c0`) and keeps the
/// `GroupArrow` the model load armed (`0x4ee1de`).
#[test]
fn the_arrow_push_shows_levels_arms_and_aims_the_children() {
    use crate::script::MinimapRimArrow;
    let mut s = script();
    s.set_screen_size(1024.0, 768.0);
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap") m:SetArrowModel("A.mdx")"#)
        .unwrap();
    let level = s.eval::<i64>("return m:GetFrameLevel()").unwrap();
    let shown = |s: &UiScript, i: usize| {
        s.eval::<bool>(&format!("return ({{m:GetChildren()}})[{i}]:IsShown() == 1"))
            .unwrap()
    };
    let rim = [
        MinimapRimArrow {
            bearing: std::f32::consts::FRAC_PI_2,
            slot: -2,
            indoors: false,
        },
        MinimapRimArrow {
            bearing: 0.0,
            slot: 1,
            indoors: true,
        },
    ];
    let mut group = [None; 5];
    group[0] = Some(std::f32::consts::PI);
    group[4] = Some(0.0);
    s.set_minimap_arrows(&rim, true, false, &group);
    assert!(shown(&s, 6) && shown(&s, 7) && !shown(&s, 8));
    assert!(shown(&s, 1) && !shown(&s, 2) && shown(&s, 5));
    let at = |s: &UiScript, i: usize| s.minimap_child("TestMinimap", i).unwrap();
    // The level base is the minimap's level plus 3: a landmark at +1, the gossip marker at +4.
    for (i, want) in [(6, level + 1), (7, level + 4)] {
        let got = s
            .eval::<i64>(&format!(
                "return ({{m:GetChildren()}})[{i}]:GetFrameLevel()"
            ))
            .unwrap();
        assert_eq!(got, want, "child {i}");
    }
    assert_eq!(at(&s, 6).sequence, 166, "a landmark plays Arrow");
    assert_eq!(at(&s, 7).sequence, 168, "a static slot plays GuideArrow");
    assert_eq!(at(&s, 5).sequence, 165, "the pet keeps GroupArrow");
    // West of north at a quarter turn: the left of the disc, half the disc in.
    let (c, a) = (0.055_f32, 0.044_f32);
    let p = at(&s, 6).position;
    assert!(
        (p.0 - (c - a)).abs() < 1e-6 && (p.1 - c).abs() < 1e-6,
        "{p:?}"
    );
    assert!((at(&s, 6).facing - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    let p = at(&s, 1).position;
    assert!(
        (p.0 - c).abs() < 1e-6 && (p.1 - (c - a)).abs() < 1e-6,
        "south: {p:?}"
    );

    // Not rebuilt: the frames keep their show state and art, and are re-aimed.
    s.run("({m:GetChildren()})[6]:SetSequence(0)").unwrap();
    let turned = [
        MinimapRimArrow {
            bearing: 0.0,
            ..rim[0]
        },
        rim[1],
    ];
    s.set_minimap_arrows(&turned, false, false, &group);
    assert_eq!(at(&s, 6).sequence, 0, "no re-arm without a rebuild");
    assert!(shown(&s, 7) && !shown(&s, 8));
    assert!(at(&s, 6).facing.abs() < 1e-6);

    // Indoors a rebuild shows only the records flagged `+0x44`.
    s.set_minimap_arrows(&rim, true, true, &group);
    assert!(!shown(&s, 6) && shown(&s, 7));
    // The corpse, static slot 2: the top rim level, `CorpseArrow`, shown indoors.
    let corpse = [MinimapRimArrow {
        bearing: 1.0,
        slot: 2,
        indoors: true,
    }];
    s.set_minimap_arrows(&corpse, true, true, &group);
    assert!(shown(&s, 6) && !shown(&s, 7));
    assert_eq!(
        s.eval::<i64>("return ({m:GetChildren()})[6]:GetFrameLevel()")
            .unwrap(),
        level + 5
    );
    assert_eq!(at(&s, 6).sequence, 167);
    // A party record with no arrow hides its frame.
    s.set_minimap_arrows(&rim, false, true, &[None; 5]);
    assert!(!shown(&s, 1) && !shown(&s, 5));
}

/// The ctor's sizes ride `G48 · 5/3` (`0x4edc42`): at 16:9 the disc is still 140.8 FrameXML units
/// (0.11 base layout units, `0x4ede9e`), the rim centre `C` is 0.055 base and the radius `A` 0.044
/// base (`0x4ed452`), and the player arrow's model scale is the base itself (`0x4edfec`), so each
/// fails at 16:9 without the base, where at 4:3 the base is 1.
#[test]
fn at_sixteen_by_nine_the_children_scale_by_the_ctors_base() {
    use crate::script::MinimapRimArrow;
    let mut s = script();
    s.set_screen_size(1920.0, 1080.0);
    s.run(r#"m = CreateFrame("Minimap", "TestMinimap")"#)
        .unwrap();
    let a = 1920.0_f32 / 1080.0;
    let base = 5.0 / 3.0 / (a * a + 1.0).sqrt();
    for i in 1..=8 {
        let w = s
            .eval::<f32>(&format!("return ({{m:GetChildren()}})[{i}]:GetWidth()"))
            .unwrap();
        assert!((w - 140.8).abs() < 1e-3, "child {i}: {w}");
    }
    let player = s.minimap_child("TestMinimap", 9).unwrap();
    assert!((player.scale - base).abs() < 1e-6, "{}", player.scale);
    s.set_minimap_arrows(
        &[MinimapRimArrow {
            bearing: 0.0,
            slot: -2,
            indoors: false,
        }],
        true,
        false,
        &[None; 5],
    );
    let p = s.minimap_child("TestMinimap", 6).unwrap().position;
    let (c, r) = (0.055 * base, 0.044 * base);
    assert!(
        (p.0 - c).abs() < 1e-6 && (p.1 - (c + r)).abs() < 1e-6,
        "{p:?}"
    );
}
