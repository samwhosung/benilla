//! The world-map bindings (decision 0203 phase 2): catalog/feed pushes, the engine-owned
//! selection, and the deferred WORLD_MAP_UPDATE queue.

use super::common::script;
use crate::script::*;

/// A two-continent catalog shaped like the real 5875 one (Kalimdor first — the app's push order
/// defines every Lua-visible index; the sheet rects are the real disjoint kernel outputs). The
/// EK zone grid is sparse: exactly the real Goldshire cell (12735, verified against the shipped
/// Azeroth.zmp) marked as zone 1.
fn push_catalog(s: &mut UiScript) {
    let mut ek_grid = vec![0u16; 128 * 128];
    ek_grid[12735] = 1;
    // Cell 8801 = the child (0.5, 0.5) resolves to through Elwynn's OWN loc rect — a "Stormwind
    // City" peer child. Clicking it from the Elwynn zone map drills into the city (`0x4a67a0`).
    ek_grid[8801] = 2;
    // Cell 9060 = the child (0.82, 0.82) resolves to through Elwynn's own rect — Elwynn itself,
    // the case the zone-level hover must stay silent on (the displayed zone never names itself).
    ek_grid[9060] = 1;
    s.set_world_map_catalog(vec![
        WorldMapContinentView {
            name: "Kalimdor".into(),
            map_file: "Kalimdor".into(),
            world_rect: (0.08882, 0.07910, 0.40020, 0.86952),
            loc_rect: (17066.6, -19733.2, 12799.9, -11733.3),
            zone_grid: Vec::new(),
            zones: vec![
                WorldMapZoneView {
                    name: "Durotar".into(),
                    area_id: 14,
                    map_file: "Durotar".into(),
                    loc_rect: (-1800.0, -6800.0, -3800.0, -8500.0),
                    overlays: Vec::new(),
                },
                WorldMapZoneView {
                    name: "The Barrens".into(),
                    area_id: 17,
                    map_file: "Barrens".into(),
                    loc_rect: (-1000.0, -11000.0, -5500.0, -17000.0),
                    overlays: Vec::new(),
                },
            ],
        },
        WorldMapContinentView {
            name: "Eastern Kingdoms".into(),
            map_file: "Azeroth".into(),
            world_rect: (0.62375, 0.02695, 0.92315, 0.87126),
            loc_rect: (16000.0, -19199.9, 7466.6, -16000.0),
            zone_grid: ek_grid,
            zones: vec![
                WorldMapZoneView {
                    name: "Elwynn Forest".into(),
                    area_id: 12,
                    map_file: "Elwynn".into(),
                    // A clean synthetic loc rect inside the EK continent rect: w=2000, h=1500 → the
                    // continent-highlight formula lands on round-ish expected coords (asserted below).
                    loc_rect: (-8000.0, -10000.0, -400.0, -1900.0),
                    // Three overlays. Two are shaped like real Elwynn rows: Northshire reveals
                    // on explore bit 125, Goldshire on bit 124 (the real AreaTable exploreFlags);
                    // their hit rects (top, left, bottom, right; px of 1002×668) sit apart so a
                    // point is in at most one. The first is a row whose first area slot resolves
                    // to nothing (bit 126; its rect overlaps Northshire's) — the zone-level hover
                    // must walk past it, not stop at it.
                    overlays: vec![
                        WorldMapOverlayView {
                            texture: "Interface\\WorldMap\\Elwynn\\STONECAIRNLAKE".into(),
                            width: 128,
                            height: 128,
                            offset_x: 450,
                            offset_y: 300,
                            explore_bits: vec![126],
                            hit_rect: (300, 450, 400, 560),
                            area_name: None,
                        },
                        WorldMapOverlayView {
                            texture: "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY".into(),
                            width: 200,
                            height: 176,
                            offset_x: 490,
                            offset_y: 200,
                            explore_bits: vec![125],
                            hit_rect: (200, 490, 376, 690),
                            area_name: Some("Northshire Valley".into()),
                        },
                        WorldMapOverlayView {
                            texture: "Interface\\WorldMap\\Elwynn\\GOLDSHIRE".into(),
                            width: 172,
                            height: 128,
                            offset_x: 380,
                            offset_y: 320,
                            explore_bits: vec![124],
                            hit_rect: (400, 300, 520, 460),
                            area_name: Some("Goldshire".into()),
                        },
                    ],
                },
                // A city peer of Elwynn — a WorldMapArea child of the continent with its own art
                // folder, indistinguishable from a zone in code (`0x4a67a0`).
                WorldMapZoneView {
                    name: "Stormwind City".into(),
                    area_id: 1519,
                    map_file: "StormwindCity".into(),
                    loc_rect: (-8200.0, -8900.0, -400.0, -1100.0),
                    overlays: Vec::new(),
                },
            ],
        },
    ]);
}

/// The **orphan list** — `0x4a5d00`'s third array, and in 5875 its entire population: the three
/// battlegrounds, with the real row ids, map ids, area ids and art folders, in `WorldMapArea.dbc`
/// **file order** (the fill pass appends with no sort). Alterac Valley carries one of its three
/// real `WorldMapOverlay` rows over its real AreaBit 954 — the only battleground art the
/// explored-bit gate has anything to say about (WSG and AB key zero overlay rows).
fn push_direct_areas(s: &mut UiScript) {
    let row = |name: &str, area_id: u32, folder: &str, overlays: Vec<WorldMapOverlayView>| {
        WorldMapZoneView {
            name: name.into(),
            area_id,
            map_file: folder.into(),
            // A synthetic rect: nothing engine-side projects through it (the app owns every
            // projection), it is carried because the row is the row.
            loc_rect: (1500.0, 500.0, 1600.0, 600.0),
            overlays,
        }
    };
    s.set_world_map_direct_areas(vec![
        (
            401,
            row(
                "Alterac Valley",
                2597,
                "AlteracValley",
                vec![WorldMapOverlayView {
                    texture: "Interface\\WorldMap\\AlteracValley\\DUNBALDAR".into(),
                    width: 128,
                    height: 128,
                    offset_x: 300,
                    offset_y: 200,
                    explore_bits: vec![954],
                    hit_rect: (200, 300, 328, 428),
                    area_name: Some("Dun Baldar".into()),
                }],
            ),
        ),
        (443, row("Warsong Gulch", 3277, "WarsongGulch", Vec::new())),
        (461, row("Arathi Basin", 3358, "ArathiBasin", Vec::new())),
    ]);
}

/// **The direct-area selection** — the third state, and the whole of what a battleground map is
/// (`[0x845074]`). Inside Warsong Gulch the
/// reference selects `(continent = -2, direct = WorldMapArea 443)`, and `GetMapInfo()` answers the
/// art folder that `Blizzard_BattlefieldMinimap.lua:83-86` needs before it will draw anything at
/// all; a client that models the selection as `(continent, zone)` answers nil there and draws an
/// empty battle map.
#[test]
fn a_direct_area_is_the_third_selection_state() {
    let mut s = script();
    push_catalog(&mut s);
    push_direct_areas(&mut s);

    // The player is in Warsong Gulch: `0x4a6650`'s continent loop misses (map 489 is no
    // continent's) and only then does the orphan loop match, selecting the row's own id.
    s.set_world_map_feed(None, Some((0.5, 0.5)), 0.0, None, Vec::new(), Vec::new());
    s.set_world_map_player_direct_area(Some(443));
    s.run("SetMapToCurrentZone()").unwrap();

    assert_eq!(
        s.eval::<(String, i64, i64)>(
            "return GetMapInfo(), GetCurrentMapContinent(), GetCurrentMapZone()"
        )
        .unwrap(),
        ("WarsongGulch".into(), -1, 0),
        "the ART FOLDER (not the localized \"Warsong Gulch\"), continent -2 + 1, zone -1 + 1"
    );
    assert_eq!(
        s.eval::<(i64, i64)>("local n, h, hpot = GetMapInfo() return h, hpot")
            .unwrap(),
        (0, 0),
        "slots 2/3 are 0 on every instance map: WorldMapContinent.dbc ships only mapIDs 0 and 1"
    );
    assert!(
        s.eval::<bool>("local t = { GetMapZones(GetCurrentMapContinent()) } return t[1] == nil")
            .unwrap(),
        "GetMapZones(-1) takes the unsigned-bound exit — the dropdown that could write a bogus \
         row id has no entries to fire from"
    );

    // Hover and click are refused. Both matter: there is no `.zmp` bitmap for a battleground
    // (`0x4a6ec0`/`0x4a7620`/`0x4a7540` each test `-2` explicitly), and the world sheet's
    // continent walk must not run just because the direct state shares its `continent == 0` cell.
    assert!(s
        .eval::<Option<String>>("return UpdateMapHighlight(0.5, 0.5)")
        .unwrap()
        .is_none());
    s.run("ProcessMapClick(0.72, 0.63)").unwrap(); // EK land, were this the world sheet
    assert_eq!(
        s.eval::<(String, i64)>("return GetMapInfo(), GetCurrentMapContinent()")
            .unwrap(),
        ("WarsongGulch".into(), -1),
        "a click cannot drill off an instance map"
    );

    // ── THE REFRESH TRAP. All five of the reference's in-place refresh sites re-pass
    // `direct != -1 ? direct : zone` (`0x48f9f6`, `0x4a6460`, `0x6d93d8`, `0x6d9a17`, `0x6dac72`);
    // one that re-passed the zone alone would drop the instance map back to the world view on the
    // next world-state push or exploration update — silently, and only inside a battleground.
    s.set_world_map_explored(vec![u32::MAX; 64]);
    s.set_world_map_landmarks(Vec::new());
    s.run("SetMapToCurrentZone()").unwrap();
    s.sync_world_map_to_player_zone(0, 0, Some(443));
    assert_eq!(
        s.eval::<(String, i64)>("return GetMapInfo(), GetCurrentMapContinent()")
            .unwrap(),
        ("WarsongGulch".into(), -1),
        "a refresh re-selects the DIRECT cell, never the (0, 0) pair beside it"
    );

    // Selecting a continent or a zone CLEARS the direct cell: `0x4a67a0`'s other legs all fall
    // into `0x4a67ea mov [0x845074],-1`, and only `ecx == -2` jumps past it.
    s.run("SetMapZoom(2, 1)").unwrap();
    assert_eq!(
        s.eval::<(String, i64, i64)>(
            "return GetMapInfo(), GetCurrentMapContinent(), GetCurrentMapZone()"
        )
        .unwrap(),
        ("Elwynn".into(), 2, 1),
        "the instance map does not shadow later navigation"
    );

    // And the stock zoom-out button's own path off a battleground map: its `else` arm fires
    // because `GetCurrentMapZone()` is 0 there, and `SetMapZoom(0)` is the world view.
    s.run("SetMapToCurrentZone()").unwrap();
    s.run("SetMapZoom(0)").unwrap();
    assert_eq!(
        s.eval::<(Option<String>, i64)>("return GetMapInfo(), GetCurrentMapContinent()")
            .unwrap(),
        (None, 0),
        "zooming out of an instance map lands on the world sheet"
    );
}

/// An instance map's overlays are **admitted**, keyed by the direct area, under exactly the zone's
/// explored-bit gate (`0x4a67a0`'s overlay half, `0x4a6b10 jl 0x4a6b3d`) — no free reveal for a
/// battleground. On 5875 data that means Alterac Valley's three rows and nothing at all for the
/// other two.
#[test]
fn an_instance_map_reveals_its_overlays_under_the_zone_gate() {
    let mut s = script();
    push_catalog(&mut s);
    push_direct_areas(&mut s);

    s.set_world_map_player_direct_area(Some(401));
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(
        s.eval::<i64>("return GetNumMapOverlays()").unwrap(),
        0,
        "nothing explored — the gate applies exactly as on a zone map"
    );

    // AreaBit 954 = word 29, bit 26.
    let mut explored = vec![0u32; 64];
    explored[29] = 1 << 26;
    s.set_world_map_explored(explored);
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return GetMapOverlayInfo(1)").unwrap(),
        "Interface\\WorldMap\\AlteracValley\\DUNBALDAR"
    );

    // Warsong Gulch keys zero `WorldMapOverlay` rows — a fully explored character still sees bare
    // detail tiles there.
    s.set_world_map_player_direct_area(Some(443));
    s.run("SetMapToCurrentZone()").unwrap();
    s.set_world_map_explored(vec![u32::MAX; 64]);
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 0);
}

/// The navigation surface: lists come from the catalog, SetMapZoom's selection reads back
/// synchronously in the same Lua breath (the reference's dropdown-click contract), zone/continent
/// arguments clamp, and GetMapInfo names the displayed art folder (nil at the world level — the
/// reference Lua's own "World" fallback).
#[test]
fn worldmap_navigation_and_map_info() {
    let mut s = script();
    push_catalog(&mut s);

    s.run(
        r#"
        conts = { GetMapContinents() }
        kalimdor_zones = { GetMapZones(1) }
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(String, String)>("return conts[1], conts[2]")
            .unwrap(),
        ("Kalimdor".into(), "Eastern Kingdoms".into()),
        "display names are the Map.dbc localized ones, not the art folders"
    );
    assert_eq!(
        s.eval::<(String, String)>("return kalimdor_zones[1], kalimdor_zones[2]")
            .unwrap(),
        ("Durotar".into(), "The Barrens".into())
    );

    // World level by default: continent 0, nil map info.
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (0, 0)
    );
    assert!(s.eval::<bool>("return GetMapInfo() == nil").unwrap());

    // The synchronous read-back the reference relies on, and the art-folder name per level.
    s.run("SetMapZoom(2)").unwrap();
    assert_eq!(
        s.eval::<(i64, i64, String)>(
            "return GetCurrentMapContinent(), GetCurrentMapZone(), GetMapInfo()"
        )
        .unwrap(),
        (2, 0, "Azeroth".into())
    );
    s.run("SetMapZoom(1, 2)").unwrap();
    assert_eq!(
        s.eval::<String>("return GetMapInfo()").unwrap(),
        "Barrens",
        "a zone selection names the ZONE's art folder"
    );

    // Out-of-range arguments clamp (never a Lua error — addons pass garbage).
    s.run("SetMapZoom(9, 9)").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (2, 2),
        "continent clamps to the catalog, zone to that continent's list (EK has 2 zones)"
    );
}

/// SetMapToCurrentZone lands on the app-fed player zone; the feed's projection + facing surface
/// through GetPlayerMapPosition/GetPlayerFacing — for `"player"` and, since report B320, for the
/// `party1..4` slots too. A `raid` token still answers the off-map sentinel.
#[test]
fn worldmap_current_zone_and_player_feed() {
    let mut s = script();
    push_catalog(&mut s);

    s.set_world_map_feed(
        Some((1, 2)),
        Some((0.25, 0.75)),
        1.5,
        None,
        vec![Some((0.1, 0.2)), None],
        Vec::new(),
    );
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (1, 2)
    );
    let (x, y) = s
        .eval::<(f64, f64)>(r#"return GetPlayerMapPosition("player")"#)
        .unwrap();
    assert!((x - 0.25).abs() < 1e-6 && (y - 0.75).abs() < 1e-6);
    assert!((s.eval::<f64>("return GetPlayerFacing()").unwrap() - 1.5).abs() < 1e-6);
    let (px, py) = s
        .eval::<(f64, f64)>(r#"return GetPlayerMapPosition("party1")"#)
        .unwrap();
    assert!(
        (px - 0.1).abs() < 1e-6 && (py - 0.2).abs() < 1e-6,
        "a party slot reads its own projection (B320)"
    );
    for token in ["party2", "party5", "party0", "raid1", "nonsense"] {
        assert_eq!(
            s.eval::<(f64, f64)>(&format!(r#"return GetPlayerMapPosition("{token}")"#))
                .unwrap(),
            (0.0, 0.0),
            "{token} answers the off-map sentinel"
        );
    }

    // No feed → the world sheet (never an error).
    s.set_world_map_feed(None, None, 0.0, None, Vec::new(), Vec::new());
    s.run("SetMapToCurrentZone()").unwrap();
    assert_eq!(s.eval::<i64>("return GetCurrentMapContinent()").unwrap(), 0);
}

/// The **engine** moves the selection too, with no Lua in the loop — the reference's second
/// writer of `[0x84506c]`/`[0x845070]`: `0x494780`'s `old == 0` side call to the resolver
/// `0x4a6650`, which every exit of ends in the `SetMap` setter `0x4a67a0`.
///
/// We only ever had the Lua writers, so until something opened the map we sat at the world
/// level — and there `GetPlayerMapPosition` answers a world-SHEET uv, which every addon built on
/// Astrolabe rescales as a zone uv. This pins that a fresh session can be on the player's own
/// zone before a single line of FrameXML or addon Lua has asked for it.
#[test]
fn the_engine_can_select_a_zone_with_no_lua_call() {
    let mut s = script();
    push_catalog(&mut s);

    // A fresh VM is the world sheet, and nothing Lua-side has run.
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (0, 0)
    );

    s.sync_world_map_to_player_zone(1, 2, None);

    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (1, 2),
        "the engine's own SetMap is what Lua reads back"
    );
    // The WHOLE selection moved, not just the pair of globals: GetMapInfo names the zone sheet,
    // which is what `WorldMapFrame_Update` loads art from and what Astrolabe keys its scale on.
    assert!(
        s.eval::<Option<String>>("return GetMapInfo()")
            .unwrap()
            .is_some(),
        "a selected zone names its map file; the world level is the nil that mis-scales"
    );

    // It clamps through the same tail as the Lua verbs — an out-of-range pair cannot corrupt
    // the selection (`0x4a67a0` range-checks the zone against the continent's child count).
    s.sync_world_map_to_player_zone(99, 99, None);
    let (c, _) = s
        .eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
        .unwrap();
    assert!(c > 0, "clamped into the catalog, never past it");
}

/// World-level ProcessMapClick picks the continent whose sheet-rect contains the click (the
/// 0x4a7100 AABB walk — the real kernel rects are disjoint); continent-level clicks resolve
/// through the 0x4a6ec0 zone grid; hover names ride the same cell law.
#[test]
fn worldmap_click_containment_and_zone_grid() {
    let mut s = script();
    push_catalog(&mut s);

    // Mid-Kalimdor on the world sheet.
    s.run("ProcessMapClick(0.25, 0.45)").unwrap();
    assert_eq!(s.eval::<i64>("return GetCurrentMapContinent()").unwrap(), 1);

    // Kalimdor ships no grid in this fixture: continent-level clicks are inert.
    s.run("ProcessMapClick(0.5, 0.5)").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (1, 0)
    );

    // Back at the world level, EK land picks continent 2.
    s.run("SetMapZoom(0)").unwrap();
    s.run("ProcessMapClick(0.72, 0.63)").unwrap();
    assert_eq!(s.eval::<i64>("return GetCurrentMapContinent()").unwrap(), 2);

    // Continent level: Goldshire's UV inside the EK rect lands on grid cell 12735 (the real
    // Azeroth.zmp index for that world position) → zone 1. The zone lights up: name + fileName +
    // the six geometry values (`0x4a8494`), all against the zone's loc rect
    // within the continent. cont_w=35199.9, cont_h=23466.6; w=2000, h=1500.
    let (name, file, tpx, tpy, tx, ty, sx, sy) = s
        .eval::<(String, String, f64, f64, f64, f64, f64, f64)>(
            "return UpdateMapHighlight(0.452843, 0.720880)",
        )
        .unwrap();
    assert_eq!(name, "Elwynn Forest");
    assert_eq!(
        file, "Elwynn",
        "fileName = the zone art folder → <file>Highlight"
    );
    assert_eq!(
        tpx, 1.0,
        "continent-branch texPercentageX is the constant 1.0"
    );
    assert!(
        (tpy - 0.75).abs() < 1e-6,
        "texPercentageY = dim/potdim = 96/128 = 0.75, got {tpy}"
    );
    assert!(
        (tx - 2000.0 / 35199.9).abs() < 1e-4,
        "textureX = w/contW, got {tx}"
    );
    assert!(
        (ty - 1500.0 / 23466.6).abs() < 1e-4,
        "textureY = h/contH, got {ty}"
    );
    assert!(
        (sx - 24000.0 / 35199.9).abs() < 1e-4,
        "scrollChildX = (contX0-zoneX0)/contW, got {sx}"
    );
    assert!(
        (sy - 7866.6 / 23466.6).abs() < 1e-4,
        "scrollChildY = (contY0-zoneY0)/contH, got {sy}"
    );
    // Ocean at continent level: off the grid → the nil/zero tail (frame hides the quad).
    assert!(s
        .eval::<Option<String>>("return UpdateMapHighlight(0.99, 0.01)")
        .unwrap()
        .is_none());
    s.run("ProcessMapClick(0.452843, 0.720880)").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (2, 1),
        "the grid click drills into the zone"
    );

    // Now at the ZONE level (Elwynn): a click re-expressed through ELWYNN's own rect lands on cell
    // 8801 = the Stormwind City peer child → drills into the city map. This is the path that was a
    // no-op before (`0x4a75c9`: continent- and zone-level clicks run identical code, only the
    // windowing rect differs).
    s.run("ProcessMapClick(0.5, 0.5)").unwrap();
    assert_eq!(
        s.eval::<(i64, i64)>("return GetCurrentMapContinent(), GetCurrentMapZone()")
            .unwrap(),
        (2, 2),
        "a zone-level click on a city footprint drills into the city"
    );

    // Open ocean at the world level: outside both rects, no selection change.
    s.run("SetMapZoom(0)").unwrap();
    s.run("ProcessMapClick(0.01, 0.99)").unwrap();
    assert_eq!(s.eval::<i64>("return GetCurrentMapContinent()").unwrap(), 0);
}

/// At ZONE level `UpdateMapHighlight` answers a NAME only (report B360; `0x4a812e`):
/// a revealed overlay's sub-area when the cursor is inside its hit rect, else the neighbouring
/// zone or city whose grid cell the cursor is in through the DISPLAYED zone's rect window — never
/// the displayed zone itself — and always a nil fileName + six zeros, so the stock frame hides the
/// highlight quad. Fogged sub-areas have no name; an overlay with no resolvable first area is
/// walked past. The reference's own Duskwood shot: "Deadwind Pass" in the label, no highlight.
#[test]
fn worldmap_zone_level_hover_names_without_highlight() {
    let mut s = script();
    push_catalog(&mut s);
    s.run("SetMapZoom(2, 1)").unwrap(); // the Elwynn zone map

    type Answer = (Option<String>, Option<String>, f64, f64, f64, f64, f64, f64);
    let hover = |s: &mut UiScript, x: f32, y: f32| {
        s.eval::<Answer>(&format!("return UpdateMapHighlight({x}, {y})"))
            .unwrap()
    };
    let name_only =
        |name: &str| -> Answer { (Some(name.into()), None, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0) };
    let miss: Answer = (None, None, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);

    // Nothing explored. (0.5, 0.5) through Elwynn's rect is cell 8801 = the Stormwind City peer:
    // the neighbouring child is NAMED, and nothing else — the B360 answer, Deadwind Pass from
    // Duskwood in the report.
    assert_eq!(hover(&mut s, 0.5, 0.5), name_only("Stormwind City"));
    // Cell 9060 = Elwynn itself: the displayed zone never names itself.
    assert_eq!(hover(&mut s, 0.82, 0.82), miss);
    // An empty cell, no overlay: the tail.
    assert_eq!(hover(&mut s, 0.2, 0.2), miss);
    // Inside Goldshire's hit rect but fogged (bit 124 clear): no name — the pre-search walks the
    // REVEALED list, and cell 8929 under it is empty.
    assert_eq!(hover(&mut s, 0.42, 0.7), miss);

    // Reveal Northshire (bit 125 = word 3, bit 29). Its rect contains (0.5, 0.5): the overlay
    // pre-search runs BEFORE the grid, so the sub-area wins over the city under the same point.
    let mut explored = vec![0u32; 64];
    explored[3] = 1 << 29;
    s.set_world_map_explored(explored.clone());
    assert_eq!(hover(&mut s, 0.5, 0.5), name_only("Northshire Valley"));
    assert_eq!(hover(&mut s, 0.42, 0.7), miss, "Goldshire is still fogged");
    // Reveal Goldshire too (bit 124).
    explored[3] |= 1 << 28;
    s.set_world_map_explored(explored.clone());
    assert_eq!(hover(&mut s, 0.42, 0.7), name_only("Goldshire"));

    // Only the nameless overlay revealed (bit 126 = word 3, bit 30): its rect contains (0.5, 0.5)
    // but its first area resolves to nothing, so the walk passes it and the grid answers.
    let mut explored = vec![0u32; 64];
    explored[3] = 1 << 30;
    s.set_world_map_explored(explored.clone());
    assert_eq!(hover(&mut s, 0.5, 0.5), name_only("Stormwind City"));
    // Nameless first, Northshire second, both revealed: the walk continues to Northshire.
    explored[3] |= 1 << 29;
    s.set_world_map_explored(explored);
    assert_eq!(hover(&mut s, 0.5, 0.5), name_only("Northshire Valley"));

    // Control: back at the continent level the hovered zone still lights up — fileName is the art
    // folder and the quad geometry is non-zero.
    s.run("SetMapZoom(2)").unwrap();
    let (name, file, tpx, _, tx, ty, _, _) = hover(&mut s, 0.452843, 0.720880);
    assert_eq!(
        (name.as_deref(), file.as_deref(), tpx),
        (Some("Elwynn Forest"), Some("Elwynn"), 1.0)
    );
    assert!(tx > 0.0 && ty > 0.0);
}

/// SetMapZoom queues WORLD_MAP_UPDATE through the pending-event queue: nothing fires inside the
/// call (a binding can't re-enter dispatch), the registered frame hears it on the next tick.
#[test]
fn worldmap_update_event_fires_on_next_tick() {
    let mut s = script();
    push_catalog(&mut s);
    // Drain the catalog push's own queued event first.
    s.tick(0.01);
    s.run(
        r#"
        heard = 0
        f = CreateFrame("Frame", "MapListener")
        f:RegisterEvent("WORLD_MAP_UPDATE")
        f:SetScript("OnEvent", function() heard = heard + 1 end)
        SetMapZoom(1)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<i64>("return heard").unwrap(),
        0,
        "nothing fires inside the SetMapZoom call itself"
    );
    s.tick(0.01);
    assert_eq!(s.eval::<i64>("return heard").unwrap(), 1);
    s.tick(0.01);
    assert_eq!(
        s.eval::<i64>("return heard").unwrap(),
        1,
        "the queue drains — no re-fire"
    );
}

/// The exploration fog: overlays reveal per the pushed bitset — none before any push, the
/// matching subset after (bit 125 = Northshire in the fixture), the full info tuple comes back,
/// and re-pushing the same bitset queues no extra repaint.
#[test]
fn worldmap_overlays_reveal_by_explored_bits() {
    let mut s = script();
    push_catalog(&mut s);
    s.run("SetMapZoom(2, 1)").unwrap(); // the Elwynn zone map (EK's only fixture zone)

    // Nothing explored: the zone shows no overlays (all parchment).
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 0);

    // Bit 125 set (word 3, bit 29): Northshire reveals, Goldshire stays fogged.
    let mut explored = vec![0u32; 64];
    explored[3] = 1 << 29;
    s.set_world_map_explored(explored);
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 1);
    let (tex, w, h, ox, oy) = s
        .eval::<(String, i64, i64, i64, i64)>("return GetMapOverlayInfo(1)")
        .unwrap();
    assert_eq!(tex, "Interface\\WorldMap\\Elwynn\\NORTHSHIREVALLEY");
    assert_eq!((w, h, ox, oy), (200, 176, 490, 200));

    // Both bits: both overlays, catalog order; out-of-range asks read nil.
    let mut explored = vec![0u32; 64];
    explored[3] = (1 << 29) | (1 << 28);
    s.set_world_map_explored(explored);
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 2);
    assert!(s
        .eval::<bool>("return GetMapOverlayInfo(3) == nil")
        .unwrap());

    // At the continent level the overlay family reads empty (fog is a zone-map thing).
    s.run("SetMapZoom(2)").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumMapOverlays()").unwrap(), 0);
}
