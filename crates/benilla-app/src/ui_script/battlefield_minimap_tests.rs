//! The battle map, the `Blizzard_BattlefieldMinimap` addon, loaded as SHIFT-M loads it
//! (`ToggleBattlefieldMinimap`, `UIParent.lua:216-220`). The harness is the whole manifest: the
//! addon reads globals from many of its files.

use benilla_ui::script::{
    BattlefieldFlagView, BattlefieldPositionView, QuadContent, UiScript, UnitState,
    WorldMapContinentView, WorldMapLandmarkView, WorldMapOverlayView, WorldMapZoneView,
    WorldStateUiView, ARROW_MODEL,
};

/// The manifest's one warning. Deviation: `gxRefresh`, read at `OptionsFrame.lua:300`, is not
/// registered, because nothing here can set a refresh rate.
const MANIFEST_WARNINGS: [&str; 1] = ["unknown CVar 'gxRefresh' (not host-registered) — ignored"];

fn quiet(s: &UiScript) {
    assert!(s.errors().is_empty(), "script errors: {:#?}", s.errors());
    assert_eq!(
        s.warnings(),
        MANIFEST_WARNINGS,
        "host warnings beyond the manifest's own"
    );
}

/// The full interface with the addon registered but not loaded. A player exists because stock
/// OnLoads format `UnitName("player")` into their labels.
fn session() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    super::test_ui::seat_chain_addon(&mut s, "Blizzard_BattlefieldMinimap");
    s.resolve();
    quiet(&s);
    s
}

/// `MinimapArrow.m2`'s facts (one looping 3.333 s Stand, the header box), seated after the
/// manifest load that creates the world map's arrow, so only the mini arrow gets them.
fn arrow_facts(s: &mut UiScript) {
    use benilla_ui::widget::{ModelFileFacts, SequenceFacts};
    s.set_model_facts(
        ARROW_MODEL,
        ModelFileFacts {
            sequences: vec![SequenceFacts {
                anim_id: 0,
                duration_ms: 3333,
                looping: true,
            }],
            bbox: ([-0.0127, -0.0118, 0.0], [0.0135, 0.0145, 0.0]),
            cameras: 0,
        },
    );
}

fn overlay(texture: &str, w: u32, h: u32, ox: u32, oy: u32, bit: u32) -> WorldMapOverlayView {
    WorldMapOverlayView {
        texture: format!("Interface\\WorldMap\\WarsongGulch\\{texture}"),
        width: w,
        height: h,
        offset_x: ox,
        offset_y: oy,
        explore_bits: vec![bit],
        hit_rect: (0, 0, 0, 0),
        area_name: None,
    }
}

/// Overlays that run both arms of the stock tile walk (`Blizzard_BattlefieldMinimap.lua:139-168`):
/// `SilverwingHold` (300×200) takes a full and a remainder column and a remainder row, and
/// `WarsongLumberMill` (256×256) zeroes both `mod`s, leaving it to the `== 0 then 256` guard.
fn catalog() -> Vec<WorldMapContinentView> {
    vec![WorldMapContinentView {
        name: "Kalimdor".into(),
        map_file: "Kalimdor".into(),
        world_rect: (0.0, 0.0, 1.0, 1.0),
        loc_rect: (10000.0, -10000.0, 10000.0, -10000.0),
        zone_grid: Vec::new(),
        zones: vec![WorldMapZoneView {
            name: "Warsong Gulch".into(),
            area_id: 3277,
            map_file: "WarsongGulch".into(),
            loc_rect: (1000.0, 0.0, 1000.0, 0.0),
            overlays: vec![
                overlay("SilverwingHold", 300, 200, 100, 50, 1),
                overlay("WarsongLumberMill", 256, 256, 400, 300, 2),
            ],
        }],
    }]
}

/// Put the session in a battleground and press SHIFT-M. The world-state push is the gate:
/// `BattlefieldMinimap_Toggle` shows nothing unless `MiniMapBattlefieldFrame.status == "active"`
/// or `GetNumWorldStateUI() > 0` (`Blizzard_BattlefieldMinimap.lua:21`).
fn open(s: &mut UiScript) {
    s.set_world_map_catalog(catalog());
    s.set_world_map_explored(vec![0b110; 64]);
    s.set_world_map_feed(
        Some((1, 1)),
        Some((0.5, 0.5)),
        0.0,
        None,
        Vec::new(),
        Vec::new(),
    );
    s.set_world_state_ui(vec![WorldStateUiView {
        ui_state: 1,
        text: "Flags captured: 2".into(),
        ..Default::default()
    }]);
    s.tick(0.0);
    s.fire_event("UPDATE_WORLD_STATES", vec![]);
    s.run("ToggleBattlefieldMinimap()").expect("SHIFT-M");
    assert!(
        s.eval::<bool>("return BattlefieldMinimap:IsShown()")
            .unwrap(),
        "the battle map is up"
    );
}

fn update(s: &mut UiScript, elapsed: f64) {
    s.run(&format!(
        "this = BattlefieldMinimap BattlefieldMinimap_OnUpdate({elapsed}) this = nil"
    ))
    .unwrap();
}

fn screen_centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (x, y, eff) = s
        .eval::<(f64, f64, f64)>(&format!(
            "local a, b = {frame}:GetCenter() return a, b, {frame}:GetEffectiveScale()"
        ))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter() — {e}"));
    ((x * eff) as f32, (y * eff) as f32)
}

fn arrow_centre(s: &mut UiScript) -> (f32, f32) {
    s.resolve();
    let (x, y, eff) = s
        .eval::<(f64, f64, f64)>(
            r#"for _, c in ipairs({ BattlefieldMinimap:GetChildren() }) do
                   if c:GetObjectType() == "Model" then
                       local a, b = c:GetCenter()
                       return a, b, c:GetEffectiveScale()
                   end
               end"#,
        )
        .expect("the battle map has a Model child");
    ((x * eff) as f32, (y * eff) as f32)
}

fn shown(s: &UiScript, frame: &str) -> bool {
    s.eval::<bool>(&format!("return {frame}:IsShown()"))
        .unwrap_or_else(|e| panic!("{frame}:IsShown() — {e}"))
}

fn num(s: &UiScript, expr: &str) -> f64 {
    s.eval::<f64>(&format!("return {expr}"))
        .unwrap_or_else(|e| panic!("{expr} — {e}"))
}

#[test]
fn the_addon_loads_on_demand_through_uiparents_own_road() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    assert_eq!(
        s.eval::<Option<i64>>("return IsAddOnLoaded(\"Blizzard_BattlefieldMinimap\")")
            .unwrap(),
        None,
        "nothing is loaded before the first toggle"
    );

    s.run("ToggleBattlefieldMinimap()").unwrap();
    assert_eq!(
        s.eval::<i64>("return IsAddOnLoaded(\"Blizzard_BattlefieldMinimap\")")
            .unwrap(),
        1,
        "the demand load ran"
    );
    assert!(
        !shown(&s, "BattlefieldMinimap"),
        "…and a battle map does not open outside a battle: no active queue, no world-state rows"
    );
    assert_eq!(
        s.eval::<String>("return SHOW_BATTLEFIELD_MINIMAP").unwrap(),
        "0",
        "the RegisterForSave global stays at WorldStateFrame.lua's own boot value"
    );

    let missing: Option<String> = s
        .eval(
            r#"
            local names = {
                "BattlefieldMinimap", "BattlefieldMinimapBackground", "BattlefieldMinimapCorner",
                "BattlefieldMinimapCloseButton", "BattlefieldMinimapCorpse",
                "BattlefieldMinimapTab", "BattlefieldMinimapTabLeft", "BattlefieldMinimapTabMiddle",
                "BattlefieldMinimapTabRight", "BattlefieldMinimapTabFlash",
                "BattlefieldMinimapTabText", "BattlefieldMinimapTabDropDown",
            }
            for i = 1, 12 do table.insert(names, "BattlefieldMinimap" .. i) end
            for i = 1, 4 do table.insert(names, "BattlefieldMinimapParty" .. i) end
            for i = 1, 40 do table.insert(names, "BattlefieldMinimapRaid" .. i) end
            for i = 1, 2 do table.insert(names, "BattlefieldMinimapFlag" .. i) end
            for _, n in ipairs(names) do
                if not getglobal(n) then return n end
            end
            return nil
            "#,
        )
        .unwrap();
    assert_eq!(missing, None, "a declared frame never materialized");

    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapRaid17.unit")
            .unwrap(),
        "raid17"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapRaid17Icon:GetTexture()")
            .unwrap(),
        "Interface\\WorldMap\\WorldMapPartyIcon"
    );

    // The `ADDON_LOADED` arm seeds the saved table from `BattlefieldMinimapDefaults`,
    assert_eq!(
        s.eval::<(f64, bool, bool)>(
            "return BattlefieldMinimapOptions.opacity, BattlefieldMinimapOptions.locked, \
             BattlefieldMinimapOptions.showPlayers"
        )
        .unwrap(),
        (0.7, true, true)
    );
    // seats the tab at the `-225-CONTAINER_OFFSET_X` corner when no position is saved,
    let (point, relative, rel_point, x, y) = s
        .eval::<(String, String, String, f64, f64)>(
            "local p, r, rp, ox, oy = BattlefieldMinimapTab:GetPoint(1) \
             return p, r:GetName(), rp, ox, oy",
        )
        .unwrap();
    assert_eq!(
        (point.as_str(), relative.as_str(), rel_point.as_str()),
        ("BOTTOMLEFT", "UIParent", "BOTTOMRIGHT")
    );
    assert_eq!(
        (x, y),
        (
            -225.0 - num(&s, "CONTAINER_OFFSET_X"),
            num(&s, "BATTLEFIELD_TAB_OFFSET_Y")
        )
    );
    // and runs `BattlefieldMinimap_SetOpacity()` off the seeded slider, so the default 0.7 opacity
    // (alpha `1 - value`) is on before the window first shows.
    assert!(
        (num(&s, "BattlefieldMinimapBackground:GetAlpha()") - 0.3).abs() < 1e-6,
        "the load-time opacity pass reached the border texture"
    );

    open(&mut s);
    assert_eq!(
        s.eval::<String>("return SHOW_BATTLEFIELD_MINIMAP").unwrap(),
        "1"
    );
    assert!(
        shown(&s, "BattlefieldMinimapTab"),
        "the tab comes up with it"
    );
    quiet(&s);
}

/// Overlays are cut into 256-px tiles scaled by `BattlefieldMinimap1:GetWidth()/256` (56/256);
/// every number below is that arithmetic done by hand from [`catalog`].
#[test]
fn the_overlay_textures_are_cut_from_the_revealed_overlays() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    open(&mut s);

    // The `<OnShow>` already ran `SetMapToCurrentZone()` and `BattlefieldMinimap_Update()`.
    assert_eq!(
        s.eval::<String>("return GetMapInfo()").unwrap(),
        "WarsongGulch"
    );
    for i in [1, 7, 12] {
        assert_eq!(
            s.eval::<String>(&format!("return BattlefieldMinimap{i}:GetTexture()"))
                .unwrap(),
            format!("Interface\\WorldMap\\WarsongGulch\\WarsongGulch{i}"),
            "detail tile {i}"
        );
    }

    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 2);
    assert_eq!(
        s.eval::<i64>("return NUM_BATTLEFIELDMAP_OVERLAYS").unwrap(),
        3,
        "2 + 1 tiles: the 300×200 overlay needs two, the 256×256 one needs one"
    );

    // The reference's `GetTexCoord` returns `(ULx, ULy, LLx, LLy, URx, URy, LRx, LRy)`; the
    // `(left, right, top, bottom)` rect is positions 1, 5, 2, 4.
    let tex_rect = |s: &UiScript, n: u32| -> (f64, f64, f64, f64) {
        let (l, t, _, b, r, ..): (f64, f64, f64, f64, f64, f64, f64, f64) = s
            .eval(&format!(
                "return BattlefieldMinimapOverlay{n}:GetTexCoord()"
            ))
            .unwrap();
        (l, r, t, b)
    };
    let seat = |s: &UiScript, n: u32| -> (String, String, f64, f64) {
        s.eval(&format!(
            "local p, r, rp, x, y = BattlefieldMinimapOverlay{n}:GetPoint(1) return p, rp, x, y"
        ))
        .unwrap()
    };
    const SCALE: f64 = 56.0 / 256.0;

    // Tile 1: a full 256-px column, 200 px of a 256-px file tall, at the (100, 50) offset, y down.
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapOverlay1:GetTexture()")
            .unwrap(),
        "Interface\\WorldMap\\WarsongGulch\\SilverwingHold1"
    );
    assert_eq!(
        (
            num(&s, "BattlefieldMinimapOverlay1:GetWidth()"),
            num(&s, "BattlefieldMinimapOverlay1:GetHeight()")
        ),
        (256.0 * SCALE, 200.0 * SCALE)
    );
    assert_eq!(tex_rect(&s, 1), (0.0, 1.0, 0.0, 200.0 / 256.0));
    assert_eq!(
        seat(&s, 1),
        (
            "TOPLEFT".into(),
            "TOPLEFT".into(),
            100.0 * SCALE,
            -(50.0 * SCALE)
        )
    );

    // Tile 2, the remainder column: 44 px in a 64-px power-of-two file, one tile right of tile 1.
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapOverlay2:GetTexture()")
            .unwrap(),
        "Interface\\WorldMap\\WarsongGulch\\SilverwingHold2"
    );
    assert_eq!(
        num(&s, "BattlefieldMinimapOverlay2:GetWidth()"),
        44.0 * SCALE
    );
    assert_eq!(tex_rect(&s, 2), (0.0, 44.0 / 64.0, 0.0, 200.0 / 256.0));
    assert_eq!(
        seat(&s, 2),
        (
            "TOPLEFT".into(),
            "TOPLEFT".into(),
            (100.0 + 256.0) * SCALE,
            -(50.0 * SCALE)
        )
    );

    // Tile 3: 256×256 zeroes both `mod`s, so only the `== 0 then 256` guards keep it whole.
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapOverlay3:GetTexture()")
            .unwrap(),
        "Interface\\WorldMap\\WarsongGulch\\WarsongLumberMill1"
    );
    assert_eq!(
        (
            num(&s, "BattlefieldMinimapOverlay3:GetWidth()"),
            num(&s, "BattlefieldMinimapOverlay3:GetHeight()")
        ),
        (56.0, 56.0)
    );
    assert_eq!(tex_rect(&s, 3), (0.0, 1.0, 0.0, 1.0));
    for n in 1..=3 {
        assert!(
            shown(&s, &format!("BattlefieldMinimapOverlay{n}")),
            "overlay tile {n} is lit"
        );
    }

    // Un-explore the lumber mill: two tiles stay lit and the third parks; the pool never shrinks.
    s.set_world_map_explored(vec![0b010; 64]);
    s.tick(0.0); // the push queues WORLD_MAP_UPDATE, the addon's own repaint event
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 1);
    assert!(shown(&s, "BattlefieldMinimapOverlay1") && shown(&s, "BattlefieldMinimapOverlay2"));
    assert!(
        !shown(&s, "BattlefieldMinimapOverlay3"),
        "the tail parks hidden rather than being destroyed"
    );
    assert_eq!(
        s.eval::<i64>("return NUM_BATTLEFIELDMAP_OVERLAYS").unwrap(),
        3,
        "…and the pool keeps its high-water mark"
    );
    quiet(&s);
}

/// Each repaint sizes an icon `DEFAULT_POI_ICON_SIZE × GetBattlefieldMapIconScale()`
/// (`Blizzard_BattlefieldMinimap.lua:111-112`), seated on the 225×150 window.
#[test]
fn the_poi_pool_grows_from_the_landmarks_and_parks_its_tail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    open(&mut s);
    assert_eq!(s.eval::<i64>("return NUM_BATTLEFIELDMAP_POIS").unwrap(), 0);

    let landmark = |name: &str, icon: u32, uv: (f32, f32)| WorldMapLandmarkView {
        name: name.into(),
        description: String::new(),
        texture_index: icon,
        uv,
    };
    // Icon 6 is `ICON_POI_REDFLAG`, icon 9 the second row's first cell of the 8×8 `POIIcons` atlas.
    s.set_world_map_landmarks(vec![
        landmark("Silverwing Flag", 6, (0.25, 0.5)),
        landmark("Warsong Flag", 9, (0.75, 0.25)),
    ]);
    s.tick(0.0);

    assert_eq!(s.eval::<i64>("return NUM_BATTLEFIELDMAP_POIS").unwrap(), 2);
    assert!(shown(&s, "BattlefieldMinimapPOI1") && shown(&s, "BattlefieldMinimapPOI2"));
    // Cell 6 is column 6, row 0; cell 9 is column 1, row 1; `coordIncrement` is 16/128 = 0.125.
    let cell = |s: &UiScript, n: u32| -> (f64, f64) {
        let (l, t, ..): (f64, f64, f64, f64, f64, f64, f64, f64) = s
            .eval(&format!(
                "return BattlefieldMinimapPOI{n}Texture:GetTexCoord()"
            ))
            .unwrap();
        (l, t)
    };
    assert_eq!(cell(&s, 1), (0.75, 0.0));
    assert_eq!(cell(&s, 2), (0.125, 0.125));
    // CENTER on the window's TOPLEFT at (x × width, -y × height): v runs down, frame y runs up.
    assert_eq!(
        s.eval::<(String, String, String, f64, f64)>(
            "local p, r, rp, x, y = BattlefieldMinimapPOI1:GetPoint(1) return p, r:GetName(), rp, x, y"
        )
        .unwrap(),
        (
            "CENTER".into(),
            "BattlefieldMinimap".into(),
            "TOPLEFT".into(),
            0.25 * 225.0,
            -0.5 * 150.0
        )
    );
    // The default scale leaves the icon at `DEFAULT_POI_ICON_SIZE`.
    assert_eq!(num(&s, "BattlefieldMinimapPOI1:GetWidth()"), 12.0);

    // A battleground whose map row carries a bigger `MinimapIconScale` grows every icon by it.
    s.set_battlefield_positions(Vec::new(), None, 1.5);
    s.run("BattlefieldMinimap_Update()").unwrap();
    assert_eq!(
        (
            num(&s, "BattlefieldMinimapPOI1:GetWidth()"),
            num(&s, "BattlefieldMinimapPOI1:GetHeight()")
        ),
        (18.0, 18.0),
        "DEFAULT_POI_ICON_SIZE × GetBattlefieldMapIconScale()"
    );

    s.set_world_map_landmarks(vec![landmark("Warsong Flag", 9, (0.75, 0.25))]);
    s.tick(0.0);
    assert_eq!(
        s.eval::<i64>("return NUM_BATTLEFIELDMAP_POIS").unwrap(),
        2,
        "the pool never shrinks"
    );
    assert!(shown(&s, "BattlefieldMinimapPOI1"));
    assert_eq!(
        cell(&s, 1),
        (0.125, 0.125),
        "slot 1 was re-seated on the surviving landmark, not left on the stale one"
    );
    assert!(!shown(&s, "BattlefieldMinimapPOI2"), "slot 2 parked hidden");
    quiet(&s);
}

/// With no raid up `playerCount` stays 0, so `BattlefieldMinimapRaid<i>` takes
/// `GetBattlefieldPosition(i)`: the raid frames double as the battleground roster
/// (`Blizzard_BattlefieldMinimap.lua:287-301`). `(0, 0)` hides a blip on every position verb.
#[test]
fn the_battleground_blips_and_the_flag_follow_the_position_family() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    open(&mut s);

    s.set_battlefield_positions(
        vec![
            BattlefieldPositionView {
                uv: (0.25, 0.75),
                name: Some("Alliedguy".into()),
            },
            BattlefieldPositionView {
                uv: (0.0, 0.0),
                name: None,
            },
        ],
        Some(BattlefieldFlagView {
            uv: (0.4, 0.6),
            token: Some("HordeFlag".into()),
        }),
        1.0,
    );
    s.set_world_map_feed(
        Some((1, 1)),
        Some((0.5, 0.25)),
        0.0,
        None,
        vec![Some((0.1, 0.2)), None],
        Vec::new(),
    );
    update(&mut s, 0.1);

    assert!(
        shown(&s, "BattlefieldMinimapRaid1"),
        "the placed teammate shows"
    );
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapRaid1.name")
            .unwrap(),
        "Alliedguy",
        "…carrying the name the tooltip reads for a non-group teammate"
    );
    assert_eq!(
        s.eval::<(String, String, f64, f64)>(
            "local p, r, rp, x, y = BattlefieldMinimapRaid1:GetPoint(1) return p, rp, x, y"
        )
        .unwrap(),
        (
            "CENTER".into(),
            "TOPLEFT".into(),
            0.25 * 225.0,
            -0.75 * 150.0
        )
    );
    assert!(
        !shown(&s, "BattlefieldMinimapRaid2"),
        "the (0,0) teammate hides"
    );
    assert!(
        !shown(&s, "BattlefieldMinimapRaid40"),
        "…and so does every slot past the roster"
    );

    // The party arm reads `GetPlayerMapPosition("party"..i)`, the world map's own feed.
    assert!(shown(&s, "BattlefieldMinimapParty1"));
    assert_eq!(
        s.eval::<(f64, f64)>(
            "local _, _, _, x, y = BattlefieldMinimapParty1:GetPoint(1) return x, y"
        )
        .unwrap(),
        (0.1 * 225.0, -0.2 * 150.0)
    );
    assert!(
        !shown(&s, "BattlefieldMinimapParty2"),
        "a slot with no position hides"
    );

    assert!(shown(&s, "BattlefieldMinimapFlag1"));
    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapFlag1Texture:GetTexture()")
            .unwrap(),
        "Interface\\WorldStateFrame\\HordeFlag"
    );
    assert!(!shown(&s, "BattlefieldMinimapFlag2"));

    // The tooltip prefers the row's `name` over `UnitName` (`Blizzard_BattlefieldMinimap.lua:485`).
    let (bx, by) = screen_centre(&mut s, "BattlefieldMinimapRaid1");
    assert_eq!(
        s.hit_test_name(bx, by).as_deref(),
        Some("BattlefieldMinimapRaid1"),
        "the blip is what the cursor is over"
    );
    s.mouse_move(bx, by);
    assert!(s.eval::<bool>("return GameTooltip:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Alliedguy"
    );
    s.mouse_move(5.0, 5.0);

    // Show Teammates off hides the party and raid blips and leaves the flag carrier up: the
    // `not showPlayers` arm (`Blizzard_BattlefieldMinimap.lua:245-252`) hides only
    // `BattlefieldMinimapParty1..4` and `BattlefieldMinimapRaid1..40`.
    s.run("BattlefieldMinimapOptions.showPlayers = false")
        .unwrap();
    update(&mut s, 0.1);
    for f in ["BattlefieldMinimapRaid1", "BattlefieldMinimapParty1"] {
        assert!(!shown(&s, f), "{f} hides while teammates are switched off");
    }
    assert!(
        shown(&s, "BattlefieldMinimapFlag1"),
        "…but the carrier's flag is not in that arm's two loops, so it stays up"
    );

    s.run("BattlefieldMinimapOptions.showPlayers = true")
        .unwrap();
    update(&mut s, 0.1);
    assert!(shown(&s, "BattlefieldMinimapRaid1"));
    s.set_battlefield_positions(Vec::new(), None, 1.0);
    update(&mut s, 0.1);
    assert!(
        !shown(&s, "BattlefieldMinimapRaid1") && !shown(&s, "BattlefieldMinimapFlag1"),
        "leaving the battleground takes the blips with it"
    );
    quiet(&s);
}

/// The world map and the battle map each own one arrow (`Arrow::World`, `Arrow::Mini`), created
/// once by its own `Create…` verb. The mini's model scale is `G48 · 10/9` and the world map's
/// `G48 · 5/3`, with `G48 = 1/√(aspect²+1)`: 0.6667 against 1.0 at 4:3.
#[test]
fn the_player_arrow_is_the_minis_own_singleton_seated_by_the_update() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    arrow_facts(&mut s);
    open(&mut s);

    assert_eq!(
        s.eval::<i64>(
            r#"local n = 0
               for _, c in ipairs({ BattlefieldMinimap:GetChildren() }) do
                   if c:GetObjectType() == "Model" then n = n + 1 end
               end
               return n"#
        )
        .unwrap(),
        1,
        "exactly one arrow pane, parented to the battle map"
    );

    s.set_world_map_feed(
        Some((1, 1)),
        Some((0.25, 0.75)),
        1.25,
        None,
        Vec::new(),
        Vec::new(),
    );
    update(&mut s, 0.1);

    let seat = s
        .eval::<(bool, String, String, f64, f64)>(
            r#"for _, c in ipairs({ BattlefieldMinimap:GetChildren() }) do
                   if c:GetObjectType() == "Model" then
                       local p, r, rp, x, y = c:GetPoint(1)
                       return c:IsShown(), p, rp, x, y
                   end
               end"#,
        )
        .unwrap();
    assert_eq!(
        seat,
        (
            true,
            "CENTER".into(),
            "TOPLEFT".into(),
            0.25 * 225.0,
            -0.75 * 150.0
        )
    );

    let (cx, cy) = arrow_centre(&mut s);
    let pane = s
        .extract()
        .into_iter()
        .find(|q| {
            matches!(&q.content, QuadContent::ModelPane { model: Some(m), .. } if m == ARROW_MODEL)
                && q.rect.is_some_and(|r| {
                    ((r.left + r.right) / 2.0 - cx).abs() < 0.5
                        && ((r.top + r.bottom) / 2.0 - cy).abs() < 0.5
                })
        })
        .expect("the battle map's arrow pane is in the render list");
    let QuadContent::ModelPane {
        facing,
        model_scale,
        ..
    } = pane.content
    else {
        unreachable!()
    };
    assert!(
        (facing - 1.25).abs() < 1e-6,
        "turned to the player's facing, got {facing}"
    );
    assert!(
        (model_scale - 10.0 / 9.0 * 0.6).abs() < 1e-4,
        "the MINI arrow's own `G48 · 10/9` scale — the world map's would be 1.0 here, got \
         {model_scale}"
    );

    // Off the displayed map, `(0, 0)` calls `ShowMiniWorldMapArrowFrame(nil)`.
    s.set_world_map_feed(Some((1, 1)), None, 0.0, None, Vec::new(), Vec::new());
    update(&mut s, 0.1);
    s.resolve();
    assert!(
        !s.extract().into_iter().any(|q| matches!(
            &q.content,
            QuadContent::ModelPane { model: Some(m), .. } if m == ARROW_MODEL
        ) && q.rect.is_some()),
        "off-map hides the arrow"
    );
    quiet(&s);
}

/// The hover ramp is the reference's own slip (`Blizzard_BattlefieldMinimap.lua:330-356`): frame 1
/// stores the cursor in `CURSOR_OLD_X/Y`, never `BattlefieldMinimap.oldX`, so frame 2 misses and
/// resets `hoverTime`, and `BATTLEFIELD_TAB_SHOW_DELAY` counts from frame 3.
#[test]
fn the_tab_follows_the_window_fades_in_on_hover_and_gates_its_drag_on_the_lock() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    open(&mut s);

    assert_eq!(
        s.eval::<String>("return BattlefieldMinimapTabText:GetText()")
            .unwrap(),
        "Battle Map",
        "the tab wears BATTLEFIELD_MINIMAP, resized by PanelTemplates_TabResize in the OnShow"
    );
    assert_eq!(
        num(&s, "BattlefieldMinimapTab:GetAlpha()"),
        0.0,
        "…and starts invisible: the OnLoad's own SetAlpha(0)"
    );

    let (mx, my) = screen_centre(&mut s, "BattlefieldMinimap");
    s.mouse_move(mx, my);
    update(&mut s, 0.3);
    assert_eq!(
        s.eval::<(Option<i64>, f64, Option<f64>)>(
            "return BattlefieldMinimap.hover, BattlefieldMinimap.hoverTime, BattlefieldMinimap.oldX"
        )
        .unwrap(),
        (Some(1), 0.0, None),
        "frame 1 only starts hovering — and leaves oldX nil, the reference's own slip"
    );
    update(&mut s, 0.3);
    assert_eq!(
        num(&s, "BattlefieldMinimap.hoverTime"),
        0.0,
        "frame 2 misses the nil oldX and RESTARTS the clock"
    );
    update(&mut s, 0.3);
    assert_eq!(
        s.eval::<(f64, Option<i64>)>(
            "return BattlefieldMinimap.hoverTime, BattlefieldMinimap.hasBeenFaded"
        )
        .unwrap(),
        (0.3, Some(1)),
        "frame 3 finally crosses BATTLEFIELD_TAB_SHOW_DELAY and starts the fade"
    );
    // `UIFrameFadeIn` runs on the fade manager's update, from 0 to `DEFAULT_BATTLEFIELD_TAB_ALPHA`
    // (0.75) over `BATTLEFIELD_TAB_FADE_TIME` (0.15 s).
    s.tick(0.05);
    let part_way = num(&s, "BattlefieldMinimapTab:GetAlpha()");
    assert!(
        part_way > 0.0 && part_way < 0.75,
        "part way through the 0.15 s fade, got {part_way}"
    );
    s.tick(0.2);
    assert_eq!(num(&s, "BattlefieldMinimapTab:GetAlpha()"), 0.75);

    s.mouse_move(5.0, 5.0);
    update(&mut s, 0.1);
    assert_eq!(
        s.eval::<Option<i64>>("return BattlefieldMinimap.hover")
            .unwrap(),
        None,
        "the hover state is cleared on the way out"
    );
    s.tick(0.2);
    assert_eq!(num(&s, "BattlefieldMinimapTab:GetAlpha()"), 0.0);

    // A left click on a locked tab returns before `StartMoving`, which sets the user-placed bit.
    assert!(!s
        .eval::<bool>("return BattlefieldMinimapTab:IsUserPlaced()")
        .unwrap());
    s.run("BattlefieldMinimapTab:Click(\"LeftButton\")")
        .unwrap();
    assert!(
        !s.eval::<bool>("return BattlefieldMinimapTab:IsUserPlaced()")
            .unwrap(),
        "a locked tab does not start a drag"
    );
    s.run("BattlefieldMinimapOptions.locked = false BattlefieldMinimapTab:Click(\"LeftButton\")")
        .unwrap();
    assert!(
        s.eval::<bool>("return BattlefieldMinimapTab:IsUserPlaced()")
            .unwrap(),
        "unlocked, the same click starts one"
    );
    s.run("BattlefieldMinimapTab:StopMovingOrSizing()").unwrap();

    // `PLAYER_LOGOUT` writes a user-placed tab's centre into the saved table and hands the seat
    // back to the file's own anchor; a tab nobody moved clears the row instead.
    s.resolve();
    let centre = s
        .eval::<(f64, f64)>("return BattlefieldMinimapTab:GetCenter()")
        .unwrap();
    s.fire_event("PLAYER_LOGOUT", vec![]);
    assert_eq!(
        s.eval::<(f64, f64)>(
            "return BattlefieldMinimapOptions.position.x, BattlefieldMinimapOptions.position.y"
        )
        .unwrap(),
        centre
    );
    assert!(
        !s.eval::<bool>("return BattlefieldMinimapTab:IsUserPlaced()")
            .unwrap(),
        "…and the bit is handed back, so the row is what restores the seat next login"
    );
    s.fire_event("PLAYER_LOGOUT", vec![]);
    assert_eq!(
        s.eval::<Option<i64>>("return BattlefieldMinimapOptions.position ~= nil and 1 or nil")
            .unwrap(),
        None,
        "a tab nobody placed clears the saved row"
    );

    s.run("ToggleBattlefieldMinimap()").unwrap();
    assert!(!shown(&s, "BattlefieldMinimap") && !shown(&s, "BattlefieldMinimapTab"));
    assert_eq!(
        s.eval::<String>("return SHOW_BATTLEFIELD_MINIMAP").unwrap(),
        "0"
    );

    s.run("ToggleBattlefieldMinimap()").unwrap();
    assert!(shown(&s, "BattlefieldMinimap"));
    s.set_world_state_ui(Vec::new());
    s.tick(0.0);
    s.fire_event("PLAYER_ENTERING_WORLD", vec![]);
    assert!(
        !shown(&s, "BattlefieldMinimap"),
        "no world-state rows and no active queue: the window hides itself"
    );
    quiet(&s);
}

/// `BattlefieldMinimap_SetOpacity` reads no CVar: its input is the shared `OpacityFrameSlider`, and
/// what persists is the `SavedVariablesPerCharacter` table and `SHOW_BATTLEFIELD_MINIMAP`. The
/// border and the twelve tiles take `1 - value`; the overlays, close button and corner take a
/// further 0.15 off unless the alpha is under 0.15 (`Blizzard_BattlefieldMinimap.lua:376-390`).
#[test]
fn the_dropdown_and_the_opacity_slider_drive_the_saved_options() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = session();
    open(&mut s);

    // A real right-click: the tab registers `RightButtonUp` (`Blizzard_BattlefieldMinimap.xml:96`).
    s.run("BattlefieldMinimapTab:Click(\"RightButton\")")
        .unwrap();
    assert!(shown(&s, "DropDownList1"), "the menu opened");
    let row = |s: &UiScript, n: u32| -> (String, bool) {
        (
            s.eval(&format!("return DropDownList1Button{n}:GetText()"))
                .unwrap(),
            s.eval(&format!("return DropDownList1Button{n}Check:IsShown()"))
                .unwrap(),
        )
    };
    // Both toggles are checked off `BattlefieldMinimapDefaults`; the opacity row is an action.
    assert_eq!(row(&s, 1), ("Show Teammates".into(), true));
    assert_eq!(row(&s, 2), ("Lock Battle Map".into(), true));
    assert_eq!(row(&s, 3), ("Change Opacity".into(), false));

    s.run("DropDownList1Button1:Click()").unwrap();
    assert!(
        !s.eval::<bool>("return BattlefieldMinimapOptions.showPlayers")
            .unwrap(),
        "row 1 flips Show Teammates"
    );
    s.run("BattlefieldMinimapTab:Click(\"RightButton\")")
        .unwrap();
    assert_eq!(
        row(&s, 1),
        ("Show Teammates".into(), false),
        "…and the menu re-initializes off the new value"
    );
    s.run("DropDownList1Button2:Click()").unwrap();
    assert!(
        !s.eval::<bool>("return BattlefieldMinimapOptions.locked")
            .unwrap(),
        "row 2 flips the lock"
    );

    // Row 3 re-anchors the shared opacity frame on the battle map and points its hooks here.
    s.run("BattlefieldMinimapTab:Click(\"RightButton\")")
        .unwrap();
    s.run("DropDownList1Button3:Click()").unwrap();
    assert!(shown(&s, "OpacityFrame"));
    assert_eq!(
        s.eval::<(String, String, String, f64, f64)>(
            "local p, r, rp, x, y = OpacityFrame:GetPoint(1) return p, r:GetName(), rp, x, y"
        )
        .unwrap(),
        (
            "TOPRIGHT".into(),
            "BattlefieldMinimap".into(),
            "TOPLEFT".into(),
            0.0,
            7.0
        )
    );

    // The slider's `<OnValueChanged>` repaints through `OpacityFrame.opacityFunc`.
    s.run("OpacityFrameSlider:SetValue(0.25)").unwrap();
    let alpha = |s: &UiScript, f: &str| num(s, &format!("{f}:GetAlpha()"));
    for f in [
        "BattlefieldMinimapBackground",
        "BattlefieldMinimap1",
        "BattlefieldMinimap12",
    ] {
        assert!(
            (alpha(&s, f) - 0.75).abs() < 1e-6,
            "{f} takes the plain 1 - value"
        );
    }
    for f in [
        "BattlefieldMinimapOverlay1",
        "BattlefieldMinimapCloseButton",
        "BattlefieldMinimapCorner",
    ] {
        assert!(
            (alpha(&s, f) - 0.6).abs() < 1e-6,
            "{f} takes a further 0.15 off, got {}",
            alpha(&s, f)
        );
    }
    // At the bottom of the second tier the subtraction stops rather than going negative.
    s.run("OpacityFrameSlider:SetValue(0.9)").unwrap();
    assert!((alpha(&s, "BattlefieldMinimapBackground") - 0.1).abs() < 1e-6);
    assert!(
        (alpha(&s, "BattlefieldMinimapOverlay1") - 0.1).abs() < 1e-6,
        "under 0.15 the overlays keep the plain alpha"
    );

    // Dismissing the frame runs `saveOpacityFunc`, which writes the saved table.
    s.run("OpacityFrameCloseButton:Click()").unwrap();
    assert!(!shown(&s, "OpacityFrame"));
    assert!(
        (num(&s, "BattlefieldMinimapOptions.opacity") - 0.9).abs() < 1e-6,
        "the close saved the slider's value for next login"
    );
    quiet(&s);
}
