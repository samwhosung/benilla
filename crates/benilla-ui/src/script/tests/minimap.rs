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

    // Empty restores the engine's circle, never "no mask".
    s.run(r#"m:SetMaskTexture("")"#).unwrap();
    assert_eq!(s.minimap_mask_texture(), None);

    // `lua_isstring(2)` (`0x4ee532`): a missing or nil path raises `0x84c7fc` and keeps the mask.
    s.run(r#"m:SetMaskTexture("Interface\\Foo")"#).unwrap();
    for chunk in [
        "m:SetMaskTexture()",
        "m:SetMaskTexture(nil)",
        "m:SetMaskTexture({})",
    ] {
        // Through `luaL_error` (`0x4ee555`), at the caller's line.
        assert_eq!(
            crate::script::lua_message(&s.run(chunk).expect_err(chunk)),
            "[string \"(chunk)\"]:1: Usage: TestMinimap:SetMaskTexture(\"file\")",
            "{chunk}"
        );
    }
    assert_eq!(s.minimap_mask_texture().as_deref(), Some("Interface\\Foo"));
    s.run(r#"CreateFrame("Minimap"):SetMaskTexture(5)"#)
        .unwrap();
    assert_eq!(
        crate::script::lua_message(
            &s.run(r#"CreateFrame("Minimap"):SetMaskTexture(false)"#)
                .unwrap_err()
        ),
        "[string \"(chunk)\"]:1: Usage: <unnamed>:SetMaskTexture(\"file\")"
    );

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
    assert_eq!(s.minimap_mask_texture().as_deref(), Some("Interface\\B"));

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
