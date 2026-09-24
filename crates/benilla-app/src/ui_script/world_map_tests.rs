//! The world map's **POI layer** — the icons `GetNumMapLandmarks`/`GetMapLandmarkInfo` feed into
//! `WorldMapFrame_Update`'s frame pool. Today the only landmark is the guard's directions marker
//! (`SMSG_GOSSIP_POI` → [`crate::poi_marker`]); the `AreaPOI.dbc` rows are 0203's deferred slice
//! and arrive through the same list, so these tests pin the *pool*, not the marker.
//!
//! Driven through the reference's own `WorldMapFrame.xml` off the player's chain (1980) in a bare
//! engine (no Bevy) — the panel-test idiom: push host state, run the repaint, read the frames back.

use benilla_ui::script::{
    BattlefieldFlagView, BattlefieldPositionView, QuadContent, UiScript, WorldMapLandmarkView,
    ARROW_MODEL,
};

/// The map plus everything its OnLoad touches, in the shipped list's own order.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml", // the map's continent/zone pickers initialize into it at OnLoad
        "ScrollTemplates.xml",
        // The stock update walks MAX_PARTY_MEMBERS and MAX_RAID_MEMBERS, which the party and
        // raid files own.
        r"Interface\FrameXML\PartyMemberFrame.lua",
        r"Interface\FrameXML\RaidFrame.lua",
        // The reference's own map, which <Include>s its blip templates itself.
        r"Interface\FrameXML\WorldMapFrame.xml",
    ] {
        // `test_ui::load_ui`, not a local read: a manifest entry carrying a path separator is the
        // REFERENCE's own file and must come off the player's chain, which
        // `std::fs::read_to_string` under `assets/ui` cannot do — it goes looking for
        // `assets/ui/Interface/FrameXML/...` and fails. The shared loader resolves both shapes, and
        // its own doc already records this consolidation happening once before. Hand-rolling it
        // here is what made this kit break the moment a file it loads migrated (1751).
        super::test_ui::load_ui(&s, file);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The stock per-frame update, driven the way its `OnUpdate` script runs it: `this` is the map
/// button and the elapsed time its one argument. Everything on the sheet — the arrow, the party
/// and raid blips, the battleground teammates and flags, the corpse — is seated here.
/// The map arrow's file facts (`MinimapArrow.m2`: one looping 3.333 s Stand keying no bone; the
/// header box `x ∈ [−0.0127, 0.0135]`, `y ∈ [−0.0118, 0.0145]`), handed to the
/// engine the way the app does once the asset lands (2007/2015).
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

fn update(s: &mut UiScript) {
    s.run("this = WorldMapButton WorldMapButton_OnUpdate(0.1) this = nil")
        .unwrap();
}

fn landmark(name: &str, icon: u32, uv: (f32, f32)) -> WorldMapLandmarkView {
    WorldMapLandmarkView {
        name: name.into(),
        description: String::new(),
        texture_index: icon,
        uv,
    }
}

/// A pushed landmark becomes a shown POI frame at its UV, wearing its own cell of the 8×8
/// `POIIcons` atlas. Icon **6** is `ICON_POI_REDFLAG` — the red flag every 5875-era
/// `points_of_interest` row ships, so this is the guard-directions case exactly.
#[test]
fn a_landmark_draws_its_poi_icon_at_its_map_position() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_world_map_landmarks(vec![landmark("Stormwind Warrior Trainer", 6, (0.25, 0.5))]);
    s.run("WorldMapFrame_Update()").unwrap();

    assert_eq!(
        s.eval::<i64>("return GetNumMapLandmarks()").unwrap(),
        1,
        "the host reports the one landmark"
    );
    assert!(
        s.eval::<bool>("return WorldMapFramePOI1:IsShown()")
            .unwrap(),
        "the pool grew a frame for it and showed it"
    );

    // Cell 6 of the 8×8 grid = column 6, row 0.
    // `GetTexCoord` answers EIGHT (UL, LL, UR, LR as x,y pairs) since 1840; the old
    // `(l, r, t, b)` rect is `ULx, URx, ULy, LLy` — positions 1, 5, 2, 4.
    let (l, t, _, b, r, ..): (f32, f32, f32, f32, f32, f32, f32, f32) = s
        .eval("return WorldMapFramePOI1Texture:GetTexCoord()")
        .unwrap();
    assert_eq!(
        (l, r, t, b),
        (0.75, 0.875, 0.0, 0.125),
        "POIIcons cell 6 — the red flag"
    );

    // Seated at UV × the 1002×668 detail frame, from its TOPLEFT (y down).
    let (x, y): (f32, f32) = s
        .eval(
            "local _, _, _, ox, oy = WorldMapFramePOI1:GetPoint(1) \
             return ox, oy",
        )
        .unwrap();
    assert_eq!((x, y), (0.25 * 1002.0, -0.5 * 668.0));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The pool grows and is reused, never shrunk: a busier map's tail parks hidden rather than being
/// destroyed, and the same frame is re-seated when the list shrinks back.
#[test]
fn the_poi_pool_grows_and_parks_its_tail() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_world_map_landmarks(vec![
        landmark("The Bank", 6, (0.1, 0.1)),
        landmark("The Inn", 6, (0.2, 0.2)),
        landmark("The Auction House", 6, (0.3, 0.3)),
    ]);
    s.run("WorldMapFrame_Update()").unwrap();
    assert_eq!(s.eval::<i64>("return NUM_WORLDMAP_POIS").unwrap(), 3);
    assert!(s
        .eval::<bool>("return WorldMapFramePOI3:IsShown()")
        .unwrap());

    s.set_world_map_landmarks(vec![landmark("The Inn", 6, (0.2, 0.2))]);
    s.run("WorldMapFrame_Update()").unwrap();
    assert_eq!(
        s.eval::<i64>("return NUM_WORLDMAP_POIS").unwrap(),
        3,
        "the pool never shrinks"
    );
    assert!(s
        .eval::<bool>("return WorldMapFramePOI1:IsShown()")
        .unwrap());
    assert!(
        !s.eval::<bool>("return WorldMapFramePOI2:IsShown()")
            .unwrap(),
        "slot 2 parked hidden"
    );
    assert!(!s
        .eval::<bool>("return WorldMapFramePOI3:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<String>("return WorldMapFramePOI1.name").unwrap(),
        "The Inn",
        "slot 1 was re-seated, not left on the stale landmark"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Hovering a POI names it in the map's area label; the description line beneath carries the
/// landmark's status only when it has one — the guard's directions never do, a battleground
/// node's "In Conflict" would — and is blanked otherwise (stock `WorldMapPOI_OnEnter`), which
/// reads back as **nil**, not `""`.
#[test]
fn hovering_a_poi_names_it_and_adds_a_status_line_only_when_there_is_one() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_world_map_landmarks(vec![landmark("Lion's Pride Inn", 6, (0.5, 0.5))]);
    s.run("WorldMapFrame_Update()").unwrap();
    s.run("this = WorldMapFramePOI1 this:GetScript(\"OnEnter\")() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return WorldMapFrameAreaLabel:GetText()")
            .unwrap(),
        "Lion's Pride Inn"
    );
    assert_eq!(
        s.eval::<Option<String>>("return WorldMapFrameAreaDescription:GetText()")
            .unwrap(),
        None,
        "no description → a blank line that reads back NIL: `FontString:GetText 0x79d690` \
         substitutes nil for an empty string (decision 2110), and Cartographer 2.02's world-map \
         hover reads exactly this as \"this POI has no status line\""
    );

    let mut with_status = landmark("Stables", 6, (0.5, 0.5));
    with_status.description = "In Conflict".into();
    s.set_world_map_landmarks(vec![with_status]);
    s.run("WorldMapFrame_Update()").unwrap();
    s.run("this = WorldMapFramePOI1 this:GetScript(\"OnEnter\")() this = nil")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return WorldMapFrameAreaDescription:GetText()")
            .unwrap(),
        "In Conflict"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `GetMapLandmarkInfo` hands back the reference's own five values, and a landmark with no
/// description answers **nil** there rather than an empty string (VERIFIED, `0x4a8740`) — the
/// guard's marker never carries one.
#[test]
fn the_landmark_getter_returns_the_references_five_values() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    let mut with_status = landmark("Stables", 9, (0.25, 0.75));
    with_status.description = "In Conflict".into();
    s.set_world_map_landmarks(vec![landmark("Woo Ping", 6, (0.5, 0.5)), with_status]);

    let (name, desc_is_nil, icon, x, y): (String, bool, i64, f32, f32) = s
        .eval("local n, d, t, x, y = GetMapLandmarkInfo(1) return n, d == nil, t, x, y")
        .unwrap();
    assert_eq!(
        (name.as_str(), desc_is_nil, icon, x, y),
        ("Woo Ping", true, 6, 0.5, 0.5)
    );

    let desc: String = s
        .eval("local _, d = GetMapLandmarkInfo(2) return d")
        .unwrap();
    assert_eq!(
        desc, "In Conflict",
        "a landmark that HAS one still answers it"
    );
    assert!(
        s.eval::<bool>("return GetMapLandmarkInfo(3) == nil")
            .unwrap(),
        "past the end is nil"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// No landmarks → no frames, and the repaint stays quiet. This is the common case by far: the map
/// is open far more often than a guard has just given directions.
#[test]
fn no_landmarks_draws_nothing() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run("WorldMapFrame_Update()").unwrap();
    assert_eq!(s.eval::<i64>("return GetNumMapLandmarks()").unwrap(), 0);
    assert_eq!(s.eval::<i64>("return NUM_WORLDMAP_POIS").unwrap(), 0);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B320 — party members are on the map.** One frame per `party1..4` slot, placed from
/// `GetPlayerMapPosition` through the same `(0,0)`-means-hide law the player and corpse blips
/// already obey, and hidden again the moment a slot stops answering.
///
/// The seating is the reference's: `CENTER` against `WorldMapDetailFrame`'s `TOPLEFT`, x scaled by
/// its width and y by **minus** its height — UV v runs down the sheet where frame y runs up, and
/// getting that sign wrong mirrors every blip about the top edge without failing anything else.
#[test]
fn party_blips_sit_at_their_map_positions_and_hide_when_absent() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    // `IsShown`, not `IsVisible`: the map itself is closed in this harness (the POI test's own
    // idiom), so every child would read invisible through its hidden ancestor.
    let shown = |s: &UiScript, i: u32| -> bool {
        s.eval::<bool>(&format!("return WorldMapParty{i}:IsShown()"))
            .unwrap()
    };

    // Nobody in the party: every slot answers the hide sentinel.
    update(&mut s);
    for i in 1..=4 {
        assert!(!shown(&s, i), "slot {i} hides while the party is empty");
    }

    // Two members on the displayed map, one of them off it (the None the app pushes for a member
    // whose position projects outside the rect, or whom we hold no position for at all).
    s.set_world_map_feed(
        None,
        Some((0.5, 0.5)),
        0.0,
        None,
        vec![Some((0.25, 0.75)), None, Some((0.5, 0.125))],
        Vec::new(),
    );
    update(&mut s);
    let diag = s
        .eval::<String>(
            r#"return "p1="..tostring(GetPlayerMapPosition("party1")).." p3="..tostring(GetPlayerMapPosition("party3"))"#,
        )
        .unwrap();
    assert!(
        shown(&s, 1) && shown(&s, 3),
        "the two placed members show ({diag})"
    );
    assert!(!shown(&s, 2), "a member with no position stays hidden");
    assert!(!shown(&s, 4), "a slot past the roster's end stays hidden");

    // The seating, read back against the detail frame's own box.
    let (w, h) = s
        .eval::<(f64, f64)>(
            "return WorldMapDetailFrame:GetWidth(), WorldMapDetailFrame:GetHeight()",
        )
        .unwrap();
    let (blip_x, blip_y) = s
        .eval::<(f64, f64)>("return WorldMapParty1:GetCenter()")
        .unwrap();
    let (sheet_left, sheet_top) = s
        .eval::<(f64, f64)>("return WorldMapDetailFrame:GetLeft(), WorldMapDetailFrame:GetTop()")
        .unwrap();
    assert!(
        (blip_x - (sheet_left + 0.25 * w)).abs() < 0.5,
        "u scales by the sheet's width"
    );
    assert!(
        (blip_y - (sheet_top - 0.75 * h)).abs() < 0.5,
        "v runs DOWN from the sheet's top"
    );

    // The party breaks up: the blips go with it.
    s.set_world_map_feed(None, Some((0.5, 0.5)), 0.0, None, Vec::new(), Vec::new());
    update(&mut s);
    for i in 1..=4 {
        assert!(!shown(&s, i), "slot {i} hides when the party is gone");
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The player arrow is the reference's own** (1980): `WorldMapFrame_OnLoad` creates it
/// (`CreateWorldMapArrowFrame(WorldMapFrame)`, an anonymous `Model` pane holding the minimap
/// arrow), and the update seats it at the player's UV on the detail sheet — the same point the
/// `WorldMapPlayer` mouseover button is seated at — turns it to the facing and shows it; off-map
/// hides it. The pane extracts as the minimap-arrow model the app draws as a sprite.
#[test]
fn the_player_arrow_is_the_stock_model_pane_seated_and_turned_by_the_update() {
    let _data = benilla_formats::wow_data_or_skip!();
    // The whole manifest, not the kit: the arrow's quad is read off the render list, which
    // needs the map VISIBLE — and showing it is `ShowUIPanel`'s full-screen route through
    // `UIParent` (see the furniture test below for the same setup and why the player exists).
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    arrow_facts(&mut s);
    s.resolve();
    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();
    s.set_world_map_feed(None, Some((0.25, 0.75)), 1.25, None, Vec::new(), Vec::new());
    update(&mut s);
    s.resolve();
    let arrow = |s: &mut UiScript| {
        s.extract().into_iter().find(|q| {
            matches!(&q.content, QuadContent::ModelPane { model: Some(m), .. } if m == ARROW_MODEL)
        })
    };
    let pane = arrow(&mut s).expect("the arrow pane is in the render list");
    assert!(
        matches!(&pane.content, QuadContent::ModelPane { facing, .. } if (*facing - 1.25).abs() < 1e-6),
        "turned to the facing: {:?}",
        pane.content
    );
    let rect = pane.rect.expect("…with a resolved rect");
    // The implicit rect (2015): the file's box in layout units — at 16:9 a layout unit is
    // 768·√((16/9)²+1) = 1566.4 FrameXML units, so 0.0262 × 0.0263 reads 41.0 × 41.2.
    assert!(
        (rect.right - rect.left - 0.0262 * 1566.4).abs() < 0.2
            && (rect.top - rect.bottom - 0.0263 * 1566.4).abs() < 0.2,
        "sized by the file's box: {rect:?}"
    );
    // …and the model is re-centred on it every update (`0x4a7b20`'s ½·GetWidth, ½·GetHeight in
    // layout units = half the box).
    assert!(
        matches!(&pane.content, QuadContent::ModelPane { position, .. }
            if (position.0 - 0.0131).abs() < 1e-4 && (position.1 - 0.01315).abs() < 1e-4),
        "{:?}",
        pane.content
    );
    // The seat, read back in Lua: the arrow is anonymous, so it is found among the map's
    // children by kind and compared against the mouseover button the update seats at the same
    // point.
    let (dx, dy) = s
        .eval::<(f64, f64)>(
            r#"local px, py = WorldMapPlayer:GetCenter()
               for _, child in ipairs({ WorldMapFrame:GetChildren() }) do
                   if child:GetObjectType() == "Model" and child:IsShown() then
                       local ax, ay = child:GetCenter()
                       return ax - px, ay - py
                   end
               end
               return 1e9, 1e9"#,
        )
        .unwrap();
    assert!(
        dx.abs() < 0.5 && dy.abs() < 0.5,
        "seated with WorldMapPlayer ({dx}, {dy})"
    );

    // Off the displayed map: the (0,0) sentinel hides it.
    s.set_world_map_feed(None, None, 0.0, None, Vec::new(), Vec::new());
    update(&mut s);
    s.resolve();
    assert!(arrow(&mut s).is_none(), "off-map hides the arrow");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Battleground teammates and the flag carrier** (1980) come through the position family the
/// stock update polls after the party arm: each teammate takes the next `WorldMapRaid` frame
/// past the raid's own, carrying its name for the tooltip; the carrier takes `WorldMapFlag1`
/// wearing the token's texture; the `(0, 0)` sentinel hides; and the empty push — leaving the
/// battleground — hides them all.
#[test]
fn battleground_teammates_and_the_flag_draw_from_the_position_family() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_world_map_feed(None, Some((0.5, 0.5)), 0.0, None, Vec::new(), Vec::new());
    s.set_battlefield_positions(
        vec![
            BattlefieldPositionView {
                uv: (0.25, 0.75),
                name: Some("Probe".into()),
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
    update(&mut s);
    assert!(
        s.eval::<bool>(r#"return WorldMapRaid1:IsShown() and WorldMapRaid1.name == "Probe""#)
            .unwrap(),
        "the placed teammate shows on the first raid frame with its name"
    );
    assert!(
        !s.eval::<bool>("return WorldMapRaid2:IsShown()").unwrap(),
        "the (0,0) teammate hides"
    );
    assert!(s.eval::<bool>("return WorldMapFlag1:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return WorldMapFlag1Texture:GetTexture()")
            .unwrap(),
        r"Interface\WorldStateFrame\HordeFlag"
    );
    assert!(!s.eval::<bool>("return WorldMapFlag2:IsShown()").unwrap());
    let (w, h) = s
        .eval::<(f64, f64)>(
            "return WorldMapDetailFrame:GetWidth(), WorldMapDetailFrame:GetHeight()",
        )
        .unwrap();
    let (sheet_left, sheet_top) = s
        .eval::<(f64, f64)>("return WorldMapDetailFrame:GetLeft(), WorldMapDetailFrame:GetTop()")
        .unwrap();
    let (bx, by) = s
        .eval::<(f64, f64)>("return WorldMapRaid1:GetCenter()")
        .unwrap();
    assert!(
        (bx - (sheet_left + 0.25 * w)).abs() < 0.5,
        "u scales by the sheet's width"
    );
    assert!(
        (by - (sheet_top - 0.75 * h)).abs() < 0.5,
        "v runs DOWN from the sheet's top"
    );

    s.set_battlefield_positions(Vec::new(), None, 1.0);
    update(&mut s);
    assert!(
        !s.eval::<bool>("return WorldMapRaid1:IsShown() or WorldMapFlag1:IsShown()")
            .unwrap(),
        "leaving the battleground hides them"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Opening the map must not hide the map's own furniture.** (Director report: the world showed
/// through at the left and right of the sheet, where black bars had always been.)
///
/// `WorldMapFrame` is an `area = "full"` panel, so `ShowUIPanel` routes it to `SetFullScreenFrame`
/// — which hides `UIParent` (decision 1734 restored that line). Anything the map needs *while it
/// is up* therefore must not hang off `UIParent`, and the reference is emphatic about it in three
/// places: `BlackoutWorld` is a texture inside `WorldMapFrame` itself, `WorldMapTooltip` is
/// `parent="WorldMapFrame"` where the shared `GameTooltip` is `parent="UIParent"`, and the
/// dropdown lists carry `toplevel="true"` with no parent at all.
#[test]
fn the_maps_own_furniture_survives_the_hide_that_showing_it_performs() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();

    let visible = |s: &UiScript, f: &str| {
        s.eval::<i64>(&format!(
            "local x = getglobal('{f}') if not x then return -1 end return x:IsVisible() and 1 or 0"
        ))
        .unwrap()
    };

    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();

    assert_eq!(visible(&s, "WorldMapFrame"), 1, "the map itself is up");
    assert_eq!(
        visible(&s, "UIParent"),
        0,
        "and it took the screen from the HUD"
    );
    assert_eq!(
        visible(&s, "BlackoutWorld"),
        1,
        "the blackout is up WITH it — it is what stops the 3D world showing through the margins \
         beside the 4:3 sheet, and a blackout that hides when the map opens is no blackout"
    );

    // The map's own header carries two `UIDropDownMenu` pickers, and every menu in the game is
    // seated on the shared `DropDownList1`. `ToggleDropDownMenu` ends in `listFrame:Show()`, so
    // the property that decides whether a continent list can ever appear over the map is whether
    // that Show survives — which is what a `parent="UIParent"` took away. Asserted at the Show
    // rather than through `ToggleDropDownMenu` on purpose: the toggle bails early on an empty
    // list (`numButtons == 0`), so driving it here would pass on a host with no continents
    // pushed and prove nothing about the seat.
    s.run("DropDownList1:Show()").unwrap();
    s.resolve();
    assert_eq!(
        visible(&s, "DropDownList1"),
        1,
        "the shared dropdown list can open over a full-screen panel — the continent and zone \
         pickers in the map's own header have no other seat"
    );

    s.run("CloseDropDownMenus()").unwrap();
    s.run("HideUIPanel(WorldMapFrame)").unwrap();
    s.resolve();
    assert_eq!(visible(&s, "UIParent"), 1, "and the HUD comes back after");
    assert_eq!(visible(&s, "BlackoutWorld"), 0, "with the blackout down");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **Hovering the player arrow on a scaled map names the player** (director report: an empty
/// tooltip). On a window narrower than 4:3 the stock `SetupFullscreenScale` scales the map
/// under 1 (0.802 here); `WorldMapUnit_OnEnter` then asks `MouseIsOver(WorldMapPlayer)` for the
/// text, which read the cursor against the blip's own-unit edges without the reference's scale
/// division and answered nil — a tooltip with no lines.
#[test]
fn hovering_the_player_blip_on_a_scaled_map_names_the_player() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(821.0, 768.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();
    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();
    assert!(
        s.eval::<f64>("return WorldMapFrame:GetScale()").unwrap() < 0.99,
        "the narrow window scales the map under 1"
    );
    s.set_world_map_feed(None, Some((0.5, 0.5)), 0.0, None, Vec::new(), Vec::new());
    update(&mut s);
    s.resolve();
    let (px, py, eff) = s
        .eval::<(f64, f64, f64)>(
            "local x, y = WorldMapPlayer:GetCenter() return x, y, WorldMapPlayer:GetEffectiveScale()",
        )
        .unwrap();
    s.mouse_move((px * eff) as f32, (py * eff) as f32);
    assert_eq!(
        s.hit_test_name((px * eff) as f32, (py * eff) as f32)
            .as_deref(),
        Some("WorldMapPlayer")
    );
    assert!(s.eval::<bool>("return WorldMapTooltip:IsShown()").unwrap());
    assert_eq!(
        s.eval::<String>("return WorldMapTooltipTextLeft1:GetText()")
            .unwrap(),
        "Probefour"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The player arrow draws over the zone's explored-area overlays** — the director's report: on a
/// zone sheet the arrow was nowhere, and only the zoomed-out continent sheet (which has no
/// overlays) showed it.
///
/// The stock file creates those overlays as `WorldMapDetailFrame:CreateTexture(…, "ARTWORK")`
/// (`WorldMapFrame.lua:108`) and the arrow as a `Model` child of `WorldMapFrame`
/// (`WorldMapFrame.lua:15`) — two frames at the SAME level, and inside one `(strata, level)`
/// bucket the draw layer outranks the frame. A frame's own slot is layer 0, so
/// the arrow lost to every one of those ARTWORK textures. Reproduced here with the reference's own
/// call, and asserted as the render list's order rather than a z-key's internals.
#[test]
fn the_player_arrow_draws_over_the_zones_explored_overlays() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());
    s.resolve();
    arrow_facts(&mut s);
    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();
    s.set_world_map_feed(None, Some((0.5, 0.5)), 0.0, None, Vec::new(), Vec::new());
    update(&mut s);
    // The overlay the stock file lays over a zone sheet, in its own words — covering the whole
    // detail frame, which is what an explored zone's art amounts to.
    s.run(
        r#"local o = WorldMapDetailFrame:CreateTexture("WorldMapOverlay1", "ARTWORK")
           o:SetTexture("Interface\\WorldMap\\Elwynn\\ElwynnForest1")
           o:SetAllPoints(WorldMapDetailFrame)
           o:Show()"#,
    )
    .unwrap();
    s.resolve();

    let quads = s.extract();
    let arrow = quads
        .iter()
        .find(|q| {
            matches!(&q.content, QuadContent::ModelPane { model: Some(m), .. } if m == ARROW_MODEL)
        })
        .expect("the arrow pane is in the render list");
    let overlay = quads
        .iter()
        .find(|q| {
            matches!(
                &q.content,
                QuadContent::Texture { path: Some(p), .. } if p.contains("ElwynnForest1")
            )
        })
        .expect("the overlay texture is in the render list");
    assert!(
        arrow.z > overlay.z,
        "the arrow must paint after the zone overlay (arrow {:#x}, overlay {:#x})",
        arrow.z,
        overlay.z
    );
}

/// **A click over a map POI still answers the map's UV** — the director's report: alt-clicking to
/// jump did nothing while the cursor was over a town like Goldshire, "like the name display is
/// blocking it".
///
/// It was: the stock file makes every POI icon a child of `WorldMapButton`
/// (`WorldMapFrame.lua:178`), so the topmost frame over a town is the icon, and the UV query
/// demanded the button itself. The player's own blip (a child of the button too) is the same
/// story, which is what this drives — it needs no landmark data to be real.
#[test]
fn a_click_over_a_blip_still_answers_the_maps_uv() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    assert!(super::load_default_ui(&s).is_empty());
    s.resolve();
    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();
    s.set_world_map_feed(None, Some((0.4, 0.6)), 0.0, None, Vec::new(), Vec::new());
    update(&mut s);
    s.resolve();

    let (px, py, eff) = s
        .eval::<(f64, f64, f64)>(
            "local x, y = WorldMapPlayer:GetCenter() return x, y, WorldMapPlayer:GetEffectiveScale()",
        )
        .unwrap();
    let (x, y) = ((px * eff) as f32, (py * eff) as f32);
    assert_eq!(
        s.hit_test_name(x, y).as_deref(),
        Some("WorldMapPlayer"),
        "the blip is what the mouse is over"
    );
    let (u, v) = s
        .world_map_uv_at(x, y)
        .expect("the click still lands on the map");
    // The blip sits where the feed put it, so the UV under it is that position.
    assert!(
        (u - 0.4).abs() < 0.01 && (v - 0.6).abs() < 0.01,
        "the UV under the blip is the blip's own: {u}, {v}"
    );
    // …and a click on the map's own chrome is still not a map click.
    let (cx, cy, ceff) = s
        .eval::<(f64, f64, f64)>(
            "local x, y = WorldMapFrameCloseButton:GetCenter() return x, y, WorldMapFrameCloseButton:GetEffectiveScale()",
        )
        .unwrap();
    assert!(
        s.world_map_uv_at((cx * ceff) as f32, (cy * ceff) as f32)
            .is_none(),
        "the close button is not the map"
    );
}

/// **The real-data hover** — the player's own DBCs, through the reference's own OnUpdate, to the
/// string the label ends up wearing.
///
/// Every other world-map test in the tree stands on a hand-written catalog, which can only ever
/// prove the engine's *arithmetic*. What a player hovers is the catalog
/// [`crate::ui_world_map::build_catalog`] builds out of `WorldMapArea` × `AreaTable` ×
/// `WorldMapOverlay`, indexed the way the engine indexes it, addressed by the UV
/// `WorldMapButton_OnUpdate` computes from the live cursor through a scaled frame. Three joins,
/// none of them covered, and the director's report ("hovering Dire Maul on the Feralas map names
/// it in the reference and not in benilla") is exactly a break in one of them.
///
/// Feralas, the 5875 numbers: `WorldMapArea` 121, the `DIREMAUL` overlay's hit rect
/// `(top 235, left 525, bottom 325, right 655)` px of the 1002×668 detail frame, naming
/// `AreaTable` 2577 "Dire Maul" behind explore bit 953. The cursor goes at the map UV the
/// director's own screenshot reports (58.8, 42.6), converted to a screen point through the
/// button's rect — so the test crosses the same scale conversion the live frame does.
#[test]
fn the_real_feralas_catalog_names_dire_maul_under_the_cursor() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    let mut chain = benilla_formats::open_chain(&data).expect("chain");
    let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
    let maps =
        benilla_assets::MapCatalogRes(benilla_formats::load_map_catalog(&mut chain).expect("Map"));
    let (views, _) =
        crate::ui_world_map::build_catalog(&mut chain, &areas, &maps).expect("the real catalog");
    // Feralas' place in the pushed catalog, found the way Lua would.
    let (ci, zi) = views
        .iter()
        .enumerate()
        .find_map(|(ci, c)| {
            c.zones
                .iter()
                .position(|z| z.name == "Feralas")
                .map(|zi| (ci + 1, zi + 1))
        })
        .expect("Feralas is in the catalog");
    s.set_world_map_catalog(views);
    s.set_world_map_explored(vec![u32::MAX; 64]); // a fully-discovered character
    s.run(&format!("SetMapZoom({ci}, {zi})")).unwrap();
    s.run("ToggleWorldMap()").unwrap();
    s.resolve();

    // (58.8 %, 42.6 %) of the detail frame, in screen units — the same conversion
    // `WorldMapButton_OnUpdate` inverts.
    let (l, r, t, b, eff) = s
        .eval::<(f32, f32, f32, f32, f32)>(
            "return WorldMapButton:GetLeft(), WorldMapButton:GetRight(), WorldMapButton:GetTop(), \
             WorldMapButton:GetBottom(), WorldMapButton:GetEffectiveScale()",
        )
        .unwrap();
    let (x, y) = ((l + (r - l) * 0.588) * eff, (t - (t - b) * 0.426) * eff);
    s.mouse_move(x, y);
    update(&mut s);

    assert_eq!(
        s.eval::<String>("return WorldMapFrameAreaLabel:GetText()")
            .unwrap(),
        "Dire Maul",
        "the stock OnUpdate puts the hovered sub-area's name in the label"
    );
}

/// **A layout row an addon stamped does not outlive the addon** — director report: the world map
/// stopped opening full screen, "leaving the left and bottom open", on a character whose addons
/// were all disabled.
///
/// `Cartographer/Modules/LookNFeel.lua` windows the map on enable — `SetMovable(true)`,
/// `SetResizable(true)`, `SetWidth(1024)`, `SetHeight(768)`, `StartMoving(); StopMovingOrSizing()`,
/// then `SetPoint("CENTER", UIParent, "CENTER", db.x, db.y)` — and the drag entry stamps the
/// userPlaced bit on the way through (`0x7652b0` @`0x7652e5`, unconditionally). benilla's layout
/// cache was gated on that bit **alone**, so it persisted the row and re-seated it at every login
/// on every character, replacing the stock `setAllPoints="true"` forever; the blackout, which
/// `WorldMapFrame_OnLoad` sizes to the screen and anchors BOTTOMLEFT of the map, slid up-and-right
/// with it and left the world showing at the left and the bottom.
///
/// The reference gates both ends on `movable|resizable` as well (`0x490e97 test ah,0x3` at the
/// writer; `0x490600 test ah,0x1` / `0x490689 test ah,0x2` at the two arms of the apply), and
/// stock `WorldMapFrame` carries neither flag — so the stale row is inert the first session the
/// addon does not load, and falls out of the file at that logout.
#[test]
fn a_stale_layout_row_cannot_seat_a_stock_frame() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();

    // The row verbatim as an affected `benilla-config/layout/<realm>-<character>.txt` carries it:
    // Cartographer's own saved `LookNFeel` profile offsets, and the size it forces.
    let row = || benilla_ui::script::FrameLayout {
        name: "WorldMapFrame".into(),
        width: 1024.0,
        height: 768.0,
        points: vec![benilla_ui::script::LayoutPoint {
            point: "CENTER".into(),
            relative_to: Some("UIParent".into()),
            relative_point: "CENTER".into(),
            x: -0.6369222,
            y: 53.861614,
        }],
    };

    s.restore_user_placed_layouts([row()]);
    s.resolve();
    s.run("ShowUIPanel(WorldMapFrame)").unwrap();
    s.resolve();

    let (w, h, sw, sh) = s
        .eval::<(f64, f64, f64, f64)>(
            "return WorldMapFrame:GetWidth(), WorldMapFrame:GetHeight(), \
             GetScreenWidth(), GetScreenHeight()",
        )
        .unwrap();
    assert_eq!(
        (w, h),
        (sw, sh),
        "the stale row seated nothing — the map is still the screen, as its setAllPoints authored it"
    );
    assert!(
        !s.eval::<bool>("return WorldMapFrame:IsUserPlaced()")
            .unwrap(),
        "and the apply's userPlaced stamp lives inside the position arm it never took"
    );
    assert!(
        s.user_placed_layouts().is_empty(),
        "so the row falls out of the file at this logout instead of being written again"
    );

    // The positive control: the gate is the frame's own flags, not a special case for this frame.
    // With the addon's `SetMovable`/`SetResizable` in place the same row seats exactly as before.
    s.run("WorldMapFrame:SetMovable(true) WorldMapFrame:SetResizable(true)")
        .unwrap();
    s.restore_user_placed_layouts([row()]);
    s.resolve();
    let (w, h) = s
        .eval::<(f64, f64)>("return WorldMapFrame:GetWidth(), WorldMapFrame:GetHeight()")
        .unwrap();
    assert_eq!(
        (w, h),
        (1024.0, 768.0),
        "a movable+resizable frame takes it"
    );
    assert!(
        s.eval::<bool>("return WorldMapFrame:IsUserPlaced()")
            .unwrap(),
        "and the position arm stamps the bit it was written under"
    );
    let saved = s.user_placed_layouts();
    assert_eq!(saved.len(), 1, "which is what puts the row back: {saved:?}");
    assert_eq!(saved[0], row(), "unchanged through the round trip");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The full-screen quads follow a resize** — `BlackoutWorld` behind the map, and the cinematic
/// letterbox bars. Stock `WorldMapFrame_OnLoad` and `CinematicFrame_OnLoad` size them ONCE from
/// `GetScreenWidth()`/`GetScreenHeight()` and nothing in FrameXML touches them again (no
/// `DISPLAY_SIZE_CHANGED` listener); 2242 fixed the load edge, and this is the other edge. A window
/// grown after the load left the quad at the old size and the world showed through beside it.
#[test]
fn the_fullscreen_quads_follow_a_resize() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // A player exists by the time the manifest loads (see the test above).
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let failures = super::load_default_ui(&s);
    assert!(failures.is_empty(), "manifest load errors: {failures:#?}");
    s.resolve();

    let width = |s: &UiScript, f: &str| s.eval::<f64>(&format!("return {f}:GetWidth()")).unwrap();
    assert_eq!(width(&s, "BlackoutWorld"), 1600.0, "the load edge sized it");

    // Grown, still wider than 4:3 so every one of the three takes its recompute branch.
    assert!(s.set_screen_size(1920.0, 1080.0), "the size changed");
    super::manifest::on_screen_resized(&s);
    s.resolve();

    let screen = s.eval::<f64>("return GetScreenWidth()").unwrap();
    assert_eq!(screen, 1920.0);
    for quad in ["BlackoutWorld", "UpperBlackBar", "LowerBlackBar"] {
        assert_eq!(
            width(&s, quad),
            screen,
            "{quad} still spans the OLD screen after a resize — the world shows through beside it"
        );
    }
    assert_eq!(
        s.eval::<f64>("return BlackoutWorld:GetHeight()").unwrap(),
        1080.0,
        "…and the blackout is the new height too"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
