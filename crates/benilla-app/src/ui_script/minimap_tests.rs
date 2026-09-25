//! The stock `Minimap.xml` driven engine-only. Indoor and outdoor zoom are independent levels, so
//! the +/- buttons must re-sync when a WMO transition switches between them.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

fn enabled(s: &UiScript, button: &str) -> bool {
    s.eval::<bool>(&format!("return {button}:IsEnabled() ~= 0"))
        .unwrap()
}

#[test]
fn minimap_zoom_buttons_resync_when_switching_inside_and_outside() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // Host globals the cluster calls that a bare engine does not install.
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `GameTooltip` before the cluster, as shipped: `Minimap_Update` touches it from `OnLoad` on.
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");

    // The zoom CVar defaults to 3, a middle level, so both buttons start enabled.
    assert_eq!(s.eval::<u8>("return Minimap:GetZoom()").unwrap(), 3);
    assert!(enabled(&s, "MinimapZoomIn"));
    assert!(enabled(&s, "MinimapZoomOut"));

    // Outdoors at 5, the top level: `MINIMAP_UPDATE_ZOOM` greys ZoomIn (`Minimap.lua:56-64`).
    s.run("Minimap:SetZoom(5)").unwrap();
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert!(!enabled(&s, "MinimapZoomIn"), "at max zoom ZoomIn disables");
    assert!(enabled(&s, "MinimapZoomOut"));

    // Indoors reads the indoor level, still 3; the app fires `MINIMAP_UPDATE_ZOOM` on the switch.
    s.set_minimap_inside(true);
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert_eq!(
        s.eval::<u8>("return Minimap:GetZoom()").unwrap(),
        3,
        "indoors reads the separate indoor index"
    );
    assert!(
        enabled(&s, "MinimapZoomIn"),
        "the stale outdoor max-zoom greying must clear — this is the reported bug"
    );
    assert!(enabled(&s, "MinimapZoomOut"));

    s.run("Minimap:SetZoom(0)").unwrap();
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert!(
        !enabled(&s, "MinimapZoomOut"),
        "at min zoom ZoomOut disables"
    );
    assert!(enabled(&s, "MinimapZoomIn"));

    s.set_minimap_inside(false);
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert_eq!(s.eval::<u8>("return Minimap:GetZoom()").unwrap(), 5);
    assert!(!enabled(&s, "MinimapZoomIn"), "outdoor max-zoom survived");
    assert!(enabled(&s, "MinimapZoomOut"));
}

/// Stock `MiniMapTrackingFrame` (`Minimap.xml:109-174`) follows `GetTrackingTexture()` on
/// `PLAYER_AURAS_CHANGED`, which the aura feed fires beside `UNIT_AURA`.
#[test]
fn tracking_frame_follows_get_tracking_texture_across_player_auras_changed() {
    let _data = benilla_formats::wow_data_or_skip!();
    use benilla_ui::script::TrackingState;

    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");

    let vis = |s: &UiScript| {
        s.eval::<bool>("return MiniMapTrackingFrame:IsVisible()")
            .unwrap()
    };
    assert!(!vis(&s), "no tracking at load — the frame starts hidden");

    // A tracking aura: Find Minerals, spell 2580.
    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        name: Some("Find Minerals".into()),
        icon: Some("Interface\\Icons\\Trade_Mining".into()),
        cancelable: true,
    }));
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(vis(&s), "a live tracking texture shows the icon");

    s.set_tracking(None);
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(!vis(&s), "no tracking texture hides the frame again");
}

/// The cluster and the stock `GameTime.xml`, the clock at `hour:minute` as the app pushes it.
fn game_time_session(hour: u32, minute: u32) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    s.run(&format!(
        "__benilla_game_hour = {hour}; __benilla_game_minute = {minute}"
    ))
    .unwrap();
    // `GameTime.lua` reads `TwentyFourHourTime`, which `LocalizeFrames` sets; the reference calls
    // that on `VARIABLES_LOADED` (`UIParent.lua:231-232`), and this session has no UIParent.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Localization.xml");
    s.run("LocalizeFrames()").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT()`, which the tooltip formatting goes through.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTime.xml");
    s
}

/// `GameTimeTexture`'s texcoords as `(left, right, top, bottom)`: `GetTexCoord` answers eight
/// values, `ULx, ULy, LLx, LLy, URx, URy, LRx, LRy`.
fn tod_window(s: &UiScript) -> (f64, f64, f64, f64) {
    let (ulx, uly, _, lly, urx, ..): (f64, f64, f64, f64, f64, f64, f64, f64) =
        s.eval("return GameTimeTexture:GetTexCoord()").unwrap();
    (ulx, urx, uly, lly)
}

/// Stock `GameTimeFrame_Update` slides the 50px window +0.5 onto the moon when
/// `time < DAWN or time >= DUSK`, 5:30 and 21:00 (`GameTime.lua:2-18`).
#[test]
fn game_time_frame_slides_the_sun_moon_window_on_the_game_clock() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = game_time_session(10, 30);
    // `OnLoad` seeds `timeOfDay = 0`, so its own update already seats the window.
    let day = (0.0, 50.0 / 128.0, 0.0, 50.0 / 64.0);
    assert_eq!(tod_window(&s), day, "mid-morning shows the sun half");

    s.run("__benilla_game_hour = 21; __benilla_game_minute = 0")
        .unwrap();
    s.tick(0.016);
    assert_eq!(
        tod_window(&s),
        (0.5, 0.5 + 50.0 / 128.0, 0.0, 50.0 / 64.0),
        "9:00 PM sharp is the moon half"
    );

    s.run("__benilla_game_hour = 5; __benilla_game_minute = 29")
        .unwrap();
    s.tick(0.016);
    assert_eq!(tod_window(&s).0, 0.5, "5:29 AM is still the moon");
    s.run("__benilla_game_minute = 30").unwrap();
    s.tick(0.016);
    assert_eq!(tod_window(&s), day, "5:30 AM sharp flips to the sun");
}

/// `GameTimeFrame` declares no `enableMouse`: its `<Scripts>` enable the mouse, and its
/// `<HitRectInsets>` trim the hit rect (`GameTime.xml:15-17`).
#[test]
fn hovering_the_indicator_shows_and_live_updates_the_game_time_tooltip() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = game_time_session(21, 7);
    s.resolve();

    // The middle of the hit rect, inset by l=6, r=0, t=5, b=10.
    let (l, r, t, b) = (
        s.eval::<f32>("return GameTimeFrame:GetLeft()").unwrap(),
        s.eval::<f32>("return GameTimeFrame:GetRight()").unwrap(),
        s.eval::<f32>("return GameTimeFrame:GetTop()").unwrap(),
        s.eval::<f32>("return GameTimeFrame:GetBottom()").unwrap(),
    );
    let (x, y) = ((l + 6.0 + r) * 0.5, (b + 10.0 + t - 5.0) * 0.5);
    s.mouse_move(x, y);
    assert!(
        s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "OnEnter owns the tooltip (the Scripts-walker auto-enable capturing at {x},{y})"
    );
    // The enGB `LocalizeFrames` sets `TwentyFourHourTime = 1` (`Localization.lua:20`): 21:07.
    let text = |s: &UiScript| {
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap()
    };
    assert_eq!(text(&s), "21:07");

    // While owned, `GameTimeFrame_Update` refreshes the tooltip (`GameTime.lua:20-22`).
    s.run("__benilla_game_minute = 8").unwrap();
    s.tick(0.016);
    assert_eq!(text(&s), "21:08", "the owned tooltip follows the clock");

    // Inside the raw 50x50 rect but in the left inset: not hoverable.
    s.mouse_move(l + 2.0, y);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the 6-px left inset band is not hoverable"
    );
}

/// A real click reaches `Minimap_OnClick` via `OnMouseUp` and parks centre-relative UI units.
#[test]
fn a_click_on_the_minimap_parks_a_centre_relative_ping_request() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");
    s.resolve();

    let cx = s
        .eval::<f32>("local x = Minimap:GetCenter(); return x")
        .unwrap();
    let cy = s
        .eval::<f32>("local _, y = Minimap:GetCenter(); return y")
        .unwrap();
    assert!(cx > 0.0 && cy > 0.0, "the widget resolved: ({cx}, {cy})");

    assert_eq!(s.take_minimap_ping_request(), None);

    // 20 right and 12 up, inside the 70-unit disc; stock `Minimap_OnClick` first adds
    // `CURSOR_OFFSET_X/Y`, -7 and -9 (`Minimap.lua:3-4`, `:133-134`).
    s.mouse_button(cx + 20.0, cy + 12.0, "LeftButton", true);
    s.mouse_button(cx + 20.0, cy + 12.0, "LeftButton", false);
    let (dx, dy) = s
        .take_minimap_ping_request()
        .expect("OnMouseUp → Minimap_OnClick → PingLocation");
    assert!((dx - 13.0).abs() < 0.01, "x right of centre, less 7: {dx}");
    assert!((dy - 3.0).abs() < 0.01, "y UP from centre, less 9: {dy}");
    assert_eq!(s.take_minimap_ping_request(), None);

    s.mouse_button(10.0, 10.0, "LeftButton", true);
    s.mouse_button(10.0, 10.0, "LeftButton", false);
    assert_eq!(s.take_minimap_ping_request(), None, "off-widget is no ping");
}

/// The reference answers from statics nothing clears (`0x4eefd0`), and stock `Minimap_OnUpdate`
/// uses the pair with no nil test; here it is the app's last publish, `(0, 0)` before any.
#[test]
fn get_ping_position_answers_two_numbers_always() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");

    assert_eq!(s.arity("Minimap:GetPingPosition()").unwrap(), 2);
    assert_eq!(
        s.eval::<f32>("return (Minimap:GetPingPosition())").unwrap(),
        0.0,
        "before any ping: a number, not nil"
    );
    s.set_minimap_ping((0.25, -0.125));
    let x = s
        .eval::<f32>("local x = Minimap:GetPingPosition(); return x")
        .unwrap();
    let y = s
        .eval::<f32>("local _, y = Minimap:GetPingPosition(); return y")
        .unwrap();
    assert!(
        (x - 0.25).abs() < 1e-6 && (y + 0.125).abs() < 1e-6,
        "{x} {y}"
    );

    // `MINIMAP_PING` shows the frame for 5 s, re-seated from `GetPingPosition` each frame, then
    // fades over 0.5 s (`Minimap.lua:1-2`, `:68-82`).
    assert!(
        !s.eval::<bool>("return MiniMapPing:IsVisible()").unwrap(),
        "hidden until a ping"
    );
    s.fire_event(
        "MINIMAP_PING",
        vec![
            benilla_ui::script::ScriptValue::Str("player".into()),
            benilla_ui::script::ScriptValue::Number(0.25),
            benilla_ui::script::ScriptValue::Number(-0.125),
        ],
    );
    assert!(s.eval::<bool>("return MiniMapPing:IsVisible()").unwrap());
    s.tick(1.0);
    assert!(
        s.eval::<bool>("return MiniMapPing:IsVisible()").unwrap(),
        "held at 1 s"
    );
    let cx = s
        .eval::<f32>(
            "local x = MiniMapPing:GetCenter(); local mx = Minimap:GetCenter(); return x - mx",
        )
        .unwrap();
    assert!(
        (cx - 0.25 * 140.0).abs() < 0.5,
        "re-seated at the normalized offset times the width: {cx}"
    );
    s.tick(4.2);
    s.tick(0.6);
    assert!(
        !s.eval::<bool>("return MiniMapPing:IsVisible()").unwrap(),
        "5 s hold + 0.5 s fade, then hidden"
    );
}

/// Stock `MiniMapMeetingStoneFrame` follows `IsInMeetingStoneQueue()` on `MEETINGSTONE_CHANGED`.
#[test]
fn the_meeting_stone_icon_follows_the_queue_across_meetingstone_changed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Localization.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\BattlefieldFrame.xml",
        "Interface\\FrameXML\\Minimap.xml",
    ] {
        load_xml(&s, f);
    }
    s.resolve();
    let vis = |s: &UiScript| {
        s.eval::<bool>("return MiniMapMeetingStoneFrame:IsVisible()")
            .unwrap()
    };
    assert!(!vis(&s), "hidden at load");

    s.set_meeting_stone(1519, Some("Looking for more for Stormwind City".into()));
    s.fire_event("MEETINGSTONE_CHANGED", vec![]);
    assert!(vis(&s), "a queued area shows the icon");
    s.run("this = MiniMapMeetingStoneFrame; MiniMapMeetingStoneFrame:GetScript('OnEnter')()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Looking for more for Stormwind City"
    );
    s.run("MiniMapMeetingStoneFrame:Click()").unwrap();
    assert!(
        s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap(),
        "a click asks CONFIRM_LEAVE_QUEUE"
    );
    s.run("StaticPopup1Button1:Click()").unwrap();
    assert_eq!(
        s.take_meeting_stone_cancels(),
        1,
        "Accept is CancelMeetingStoneRequest()"
    );

    s.set_meeting_stone(0, Some("Looking for more for Unknown".into()));
    s.fire_event("MEETINGSTONE_CHANGED", vec![]);
    assert!(!vis(&s), "area 0 hides it again");
}

/// A shown ping pane publishes one tile request and no quad; once the renderer hands back a cell,
/// the cell draws every frame in the overlay lane, even when nothing in the interface moves.
#[test]
fn a_shown_ping_pane_asks_for_a_tile_and_draws_its_cell() {
    use bevy::prelude::*;
    use bevy::window::PrimaryWindow;

    use benilla_ui::widget::{ModelFileFacts, SequenceFacts};

    use crate::ui_models::{Cell, UiModelTiles};
    use crate::ui_pass::UiQuads;

    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");

    // The file's facts, as `benilla-extract m2seq` reads them; the `OnLoad`'s `SetSequence(0)`
    // waited for them and replays now.
    const PING: &str = r"Interface\MiniMap\Ping\MinimapPing.mdx";
    let seq = |anim_id, duration_ms, looping| SequenceFacts {
        anim_id,
        duration_ms,
        looping,
    };
    s.set_model_facts(
        PING,
        ModelFileFacts {
            sequences: vec![seq(127, 1333, false), seq(0, 833, true), seq(1, 333, false)],
            bbox: ([0.0; 3], [0.0; 3]),
            cameras: 0,
        },
    );
    assert!(
        s.visible_model_panes().is_empty(),
        "hidden until a ping: nothing to paint"
    );
    let ping_at = |s: &mut UiScript, nx: f64, ny: f64| {
        s.fire_event(
            "MINIMAP_PING",
            vec![
                benilla_ui::script::ScriptValue::Str("player".into()),
                benilla_ui::script::ScriptValue::Number(nx),
                benilla_ui::script::ScriptValue::Number(ny),
            ],
        );
    };
    ping_at(&mut s, 0.25, -0.125);
    let panes = s.visible_model_panes();
    assert_eq!(panes.len(), 1, "the ping pane is on the paint list");
    let pane = panes[0];
    assert_eq!(
        pane.play.map(|p| (p.anim_id, p.cursor_ms)),
        Some((0, 0)),
        "the OnLoad's SetSequence(0) replayed over the seed: Stand, at 0"
    );

    let mut app = App::new();
    app.insert_non_send_resource(s);
    app.init_resource::<UiQuads>();
    // The pass's handover: the tick half writes it, the paint half reads it.
    app.init_resource::<crate::ui_script::UiPassState>();
    app.init_resource::<Assets<Image>>();
    app.init_resource::<crate::portrait::PortraitImages>();
    app.init_resource::<crate::portrait::BoothPanes>();
    app.init_resource::<UiModelTiles>();
    app.init_resource::<crate::minimap::MinimapWidget>();
    app.init_resource::<crate::ui_script::UiFrameCost>();
    app.init_resource::<crate::ui_script::UiCostWanted>();
    app.init_resource::<Time>();
    app.init_resource::<Time<Real>>();
    app.init_resource::<crate::ui_script::UiClock>();
    app.init_resource::<crate::ui_script::UiScaleCvar>();
    app.world_mut().spawn((
        Window {
            resolution: UVec2::new(1024, 768).into(),
            ..default()
        },
        PrimaryWindow,
    ));
    // The app's order: extract, then compose after `sync_tiles`, whose cells this harness hands
    // over by hand; the lane is cleared between frames, as `clear_ui_overlays` does.
    app.add_systems(
        Update,
        (
            (super::extract::tick_script, super::extract::paint_script).chain(),
            crate::ui_models::compose_tiles,
        )
            .chain(),
    );
    app.update();

    let premultiplied = |app: &App| {
        let quads = app.world().resource::<UiQuads>();
        assert!(
            quads.quads.iter().all(|q| !q.premultiplied),
            "the base lane never carries a tile: the composite is not the extract's"
        );
        quads
            .overlays
            .iter()
            .filter(|q| q.premultiplied)
            .cloned()
            .collect::<Vec<_>>()
    };
    let next_frame = |app: &mut App| {
        app.world_mut().resource_mut::<UiQuads>().overlays.clear();
        app.update();
    };
    {
        let tiles = app.world().resource::<UiModelTiles>();
        let req = tiles
            .requests
            .get(&pane.handle)
            .expect("one request for the ping pane");
        assert_eq!(req.path, PING);
        // `<Size>50×50</Size>` at s = 1 (768-tall window), DPI 1: 50 device px a side.
        assert_eq!(req.size_px, UVec2::new(50, 50));
        // `scale="0.4"`: 1280 * 0.4 = 512 px per model unit.
        assert!(
            (req.px_per_unit - 512.0).abs() < 1e-3,
            "{}",
            req.px_per_unit
        );
        // A layout unit, and a particle's unit: 768 · √((4/3)² + 1) = 1280 at 4:3.
        assert!(
            (req.pos_px_per_unit - 1280.0).abs() < 0.1,
            "{}",
            req.pos_px_per_unit
        );
        assert!(
            (req.star_px_per_unit - 1280.0).abs() < 0.1,
            "{}",
            req.star_px_per_unit
        );
        assert_eq!(req.icon, None);
        // The pane's rect in the quad pass's space (y-down window px), and its own alpha.
        assert!(
            (req.rect.width() - 50.0).abs() < 1e-3 && (req.rect.height() - 50.0).abs() < 1e-3,
            "{:?}",
            req.rect
        );
        assert!((req.alpha - 1.0).abs() < 1e-6);
        assert!(premultiplied(&app).is_empty(), "no cell yet: nothing drawn");
    }

    // The renderer hands a cell back and nothing else changes; the next frame still draws it.
    {
        let atlas = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let mut tiles = app.world_mut().resource_mut::<UiModelTiles>();
        tiles.atlas = Some(atlas);
        tiles.atlas_size = UVec2::splat(512);
        tiles.cells.insert(
            pane.handle,
            Cell {
                origin: UVec2::new(2, 2),
                size: UVec2::new(50, 50),
            },
        );
    }
    next_frame(&mut app);
    let drawn = premultiplied(&app);
    assert_eq!(drawn.len(), 1, "the cell, once, on a quiet frame");
    let q = &drawn[0];
    assert!((q.rect.width() - 50.0).abs() < 1e-3 && (q.rect.height() - 50.0).abs() < 1e-3);
    let [tl, _, br, _] = q.uv.corners;
    assert!((tl[0] - 2.0 / 512.0).abs() < 1e-6 && (tl[1] - 2.0 / 512.0).abs() < 1e-6);
    assert!((br[0] - 52.0 / 512.0).abs() < 1e-6 && (br[1] - 52.0 / 512.0).abs() < 1e-6);
    assert_eq!(q.color, [1.0, 1.0, 1.0, 1.0], "the frame's own alpha");
    assert!(q.texture.is_some());
    // The lane is re-emitted every frame, not on change.
    next_frame(&mut app);
    assert_eq!(
        premultiplied(&app).len(),
        1,
        "still drawn on the frame after"
    );

    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        // `Minimap_OnUpdate` re-seats the frame from `GetPingPosition()` (`Minimap.lua:74`).
        script.set_minimap_ping((-0.125, 0.25));
        ping_at(&mut script, -0.125, 0.25);
    }
    next_frame(&mut app);
    let moved = premultiplied(&app);
    assert_eq!(moved.len(), 1, "one quad after the pane moved");
    assert_ne!(moved[0].rect, q.rect, "the composite followed the pane");
}
