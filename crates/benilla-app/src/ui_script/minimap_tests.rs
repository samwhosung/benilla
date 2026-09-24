//! The stock `Minimap.xml` (1974) driven engine-only: the +/- zoom buttons must re-sync their
//! enabled state when the active zoom index switches (stepping inside/outside a WMO flips to the
//! other, independent level). Regression guard for the director-caught stale-button bug (2026-07-09):
//! `ZoomIn` greyed from an outdoor max-zoom stayed greyed indoors at the middle default level.

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
    // Host globals the cluster's OnLoad/clicks lean on that a bare engine doesn't install.
    s.run("function GetMinimapZoneText() return '' end")
        .unwrap();
    s.run("function PlaySound() end").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // The shipped load order provides GameTooltip before the cluster; Minimap_Update's tooltip
    // half (the PVP tint slice, decision 0287) touches it from OnLoad on.
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\UIDropDownMenu.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\BattlefieldFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Minimap.xml");

    // OnLoad seeds the +/- state from the live zoom. The CVar default is 3 — a middle level, so
    // BOTH buttons start enabled (the old hardcoded `MinimapZoomOut:Disable()` assumed zoom 0).
    assert_eq!(s.eval::<u8>("return Minimap:GetZoom()").unwrap(), 3);
    assert!(enabled(&s, "MinimapZoomIn"));
    assert!(enabled(&s, "MinimapZoomOut"));

    // Outdoors, zoom fully in (index 5 = max): the click handler greys ZoomIn.
    s.run("Minimap:SetZoom(5)").unwrap();
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert!(!enabled(&s, "MinimapZoomIn"), "at max zoom ZoomIn disables");
    assert!(enabled(&s, "MinimapZoomOut"));

    // Step INSIDE: the API now routes to the indoor index, still at its own untouched default 3.
    // The app fires MINIMAP_UPDATE_ZOOM on the transition; the cluster must re-sync to the new level.
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

    // Indoors, zoom fully out (index 0 = widest): ZoomOut greys.
    s.run("Minimap:SetZoom(0)").unwrap();
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert!(
        !enabled(&s, "MinimapZoomOut"),
        "at min zoom ZoomOut disables"
    );
    assert!(enabled(&s, "MinimapZoomIn"));

    // Step back OUTSIDE: the outdoor index is exactly where we left it (5 = max), so ZoomIn is
    // greyed again and the indoor min-zoom greying of ZoomOut is gone.
    s.set_minimap_inside(false);
    s.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    assert_eq!(s.eval::<u8>("return Minimap:GetZoom()").unwrap(), 5);
    assert!(!enabled(&s, "MinimapZoomIn"), "outdoor max-zoom survived");
    assert!(enabled(&s, "MinimapZoomOut"));
}

/// The tracking icon (ref-Minimap.xml l.109-174, transcribed verbatim): hidden at load, and the
/// verbatim OnEvent follows `GetTrackingTexture()` across `PLAYER_AURAS_CHANGED` — the event the
/// aura feed fires beside `UNIT_AURA` on every display-state change (`ui_aura`). Show with a
/// tracking state pushed, hide when it clears.
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

    // The feed pushes a tracking aura (a miner's Find Minerals) and fires the rebuild event.
    s.set_tracking(Some(TrackingState {
        spell_id: 2580,
        name: Some("Find Minerals".into()),
        icon: Some("Interface\\Icons\\Trade_Mining".into()),
        cancelable: true,
    }));
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(vis(&s), "a live tracking texture shows the icon");

    // Tracking cancelled/expired: the same event path hides it.
    s.set_tracking(None);
    s.fire_event("PLAYER_AURAS_CHANGED", vec![]);
    assert!(!vis(&s), "no tracking texture hides the frame again");
}

/// A session with the minimap cluster + the time-of-day indicator (`GameTime.xml`) loaded, the
/// game clock parked at `hour:minute` — the shape `crate::minimap::feed_game_time` pushes.
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
    // The player's own strings and locale rather than a hand-typed copy of three of them: the
    // window is the reference's file since 1751 window 5, and `TIME_TWENTYFOURHOURS` and
    // `TwentyFourHourTime` are exactly what it reads. `Localization.xml` only DEFINES
    // `LocalizeFrames`; the reference calls it from `UIParent_OnEvent`'s VARIABLES_LOADED arm, and
    // this session has no UIParent, so it calls it directly.
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\Localization.xml");
    s.run("LocalizeFrames()").unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    // `TEXT()`, which the reference's own tooltip and label formatting passes every string through.
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

/// `GameTimeTexture`'s current texcoord window, as the `(left, right, top, bottom)` rect this
/// file reasons in.
///
/// `GetTexCoord` answers EIGHT values since decision 1840 — `ULx, ULy, LLx, LLy, URx, URy, LRx,
/// LRy` — so the rect is positions 1, 5, 2, 4. Folded here rather than at each call site because
/// the window really is axis-aligned and every assertion below is about its edges.
fn tod_window(s: &UiScript) -> (f64, f64, f64, f64) {
    let (ulx, uly, _, lly, urx, ..): (f64, f64, f64, f64, f64, f64, f64, f64) =
        s.eval("return GameTimeTexture:GetTexCoord()").unwrap();
    (ulx, urx, uly, lly)
}

/// `GameTimeFrame_Update`'s law, exactly: the 50-px window over the 128×64 UI-TOD-Indicator
/// sits on the LEFT half (the sun) through the game day and slides +0.5 to the RIGHT half (the
/// moon) outside it — night is before 5:30 AM or from 9:00 PM, boundaries included exactly as
/// the ref compares (`< DAWN or >= DUSK`).
#[test]
fn game_time_frame_slides_the_sun_moon_window_on_the_game_clock() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = game_time_session(10, 30);
    // OnLoad seeded `timeOfDay = 0` and 10:30 ≠ 0, so the very first update already seated the
    // window — no OnUpdate tick needed for the initial state.
    let day = (0.0, 50.0 / 128.0, 0.0, 50.0 / 64.0);
    assert_eq!(tod_window(&s), day, "mid-morning shows the sun half");

    // 21:00 exactly is night (`>= DUSK`): the OnUpdate re-read slides the window +0.5.
    s.run("__benilla_game_hour = 21; __benilla_game_minute = 0")
        .unwrap();
    s.tick(0.016);
    assert_eq!(
        tod_window(&s),
        (0.5, 0.5 + 50.0 / 128.0, 0.0, 50.0 / 64.0),
        "9:00 PM sharp is the moon half"
    );

    // 5:29 is still night; 5:30 exactly is day (`< DAWN`).
    s.run("__benilla_game_hour = 5; __benilla_game_minute = 29")
        .unwrap();
    s.tick(0.016);
    assert_eq!(tod_window(&s).0, 0.5, "5:29 AM is still the moon");
    s.run("__benilla_game_minute = 30").unwrap();
    s.tick(0.016);
    assert_eq!(tod_window(&s), day, "5:30 AM sharp flips to the sun");
}

/// Hovering the indicator through the REAL pointer path (hit-test → OnEnter) shows the game-time
/// tooltip, live-updates it while owned (the `IsOwned` refresh branch), and hides it on leave.
/// This also pins the two loader-side pieces this frame leans on: the `<Scripts>` walker's
/// mouse auto-enable (the frame declares no enableMouse, like the reference — without the law the
/// hit-test never captures) and the `<HitRectInsets>` hull.
#[test]
fn hovering_the_indicator_shows_and_live_updates_the_game_time_tooltip() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = game_time_session(21, 7);
    s.resolve();

    // Hover the middle of the frame's hit rect: the resolved rect inset by (l=6, r=0, t=5, b=10).
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
    // TwentyFourHourTime = 1 (enGB LocalizeFrames, per GameTime.xml's header): 21:07, not 9:07 PM.
    let text = |s: &UiScript| {
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap()
    };
    assert_eq!(text(&s), "21:07");

    // The minute ticks while hovered: GameTimeFrame_Update's IsOwned branch refreshes in place.
    s.run("__benilla_game_minute = 8").unwrap();
    s.tick(0.016);
    assert_eq!(text(&s), "21:08", "the owned tooltip follows the clock");

    // Leave: a point past the left inset — outside the hit hull but still inside the raw 50×50
    // rect — must ALSO leave; the insets are part of the reference geometry.
    s.mouse_move(l + 2.0, y);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "the 6-px left inset band is not hoverable"
    );
}

/// **The ping's click path, driven by a real mouse event** (decision 1596).
///
/// The point of the test is the *path*, not the arithmetic: `Minimap_OnClick` is only reached if
/// the widget is mouse-enabled, hit-tests, and its `OnMouseUp` fires — and what it parks has to be
/// centre-relative UI units, because that is the contract the app's conversion is written against.
/// Calling `Minimap_OnClick()` from `s.run` would prove none of it (1234 §2).
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

    // Nothing is parked until somebody clicks.
    assert_eq!(s.take_minimap_ping_request(), None);

    // A click 20 UI units right and 12 up of the centre — inside the 70-unit disc. The stock
    // `Minimap_OnClick` adds its `CURSOR_OFFSET_X/Y` (−7, −9) to the cursor before subtracting
    // the centre, so what it parks is the click less that offset.
    s.mouse_button(cx + 20.0, cy + 12.0, "LeftButton", true);
    s.mouse_button(cx + 20.0, cy + 12.0, "LeftButton", false);
    let (dx, dy) = s
        .take_minimap_ping_request()
        .expect("OnMouseUp → Minimap_OnClick → PingLocation");
    assert!((dx - 13.0).abs() < 0.01, "x right of centre, less 7: {dx}");
    assert!((dy - 3.0).abs() < 0.01, "y UP from centre, less 9: {dy}");
    // Draining is a drain: the app must not see the same click twice.
    assert_eq!(s.take_minimap_ping_request(), None);

    // A click that misses the widget entirely never reaches the handler.
    s.mouse_button(10.0, 10.0, "LeftButton", true);
    s.mouse_button(10.0, 10.0, "LeftButton", false);
    assert_eq!(s.take_minimap_ping_request(), None, "off-widget is no ping");
}

/// `Minimap:GetPingPosition()` answers **two numbers always** (the reference recomputes them
/// from statics nothing clears — `0x4eefd0`), and the stock
/// `Minimap_OnUpdate` leans on exactly that: while its 5 s timer runs it multiplies the answer
/// with no nil test. The pair is the app's last publish, `(0, 0)` before any.
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

    // The stock lifetime, end to end: MINIMAP_PING shows the model frame and starts the 5 s
    // timer; OnUpdate re-seats it from GetPingPosition every frame; past 5 s it "fades" and
    // hides. The frame's own file draws in it (`crate::ui_models`, decision 2008).
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

/// The meeting-stone icon (stock `Minimap.xml`, 1974): hidden until `IsInMeetingStoneQueue()`
/// answers across `MEETINGSTONE_CHANGED`, its hover the cached status text, hidden again when
/// the queue empties — and a click raises the leave-queue question.
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

/// **The ping's pixels are the stock `<Model>`'s own file** (decision 2008): shown by
/// `MINIMAP_PING` with the file's facts landed, the extract publishes ONE tile request for the
/// pane — at its device size, the reference's unit ladder, and the composite's rect/key/alpha
/// — and pushes no quad of its own. Once the renderer has handed back a cell, the cell draws as
/// one premultiplied quad over the pane's rect **on the very next frame, with nothing else in
/// the interface moving** (decision 2023): the composite is the renderer's per-frame output in
/// the overlay lane, never a product of the memoized conversion — which is exactly what the
/// first shape got wrong, and why this test used to re-ping to "move the pane" before asking
/// for the quad. Drives the real UI pass in the headless harness the clip-plumb tests
/// use.
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

    // The file's facts, as `benilla-extract m2seq` reads them off the install: the ping's
    // `SetSequence(0)` from its OnLoad was queued behind them and replays now.
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
    // The pass's own handover (2168) — the tick half writes it, the paint half reads it.
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
    // The extract, then the composite off the bridge — the app's own order (the appender runs
    // in the `UiQuadAppend` window, after `sync_tiles`, which packs the cells this harness hands
    // over by hand below). The lane is cleared by hand between frames, as `clear_ui_overlays`
    // does at the top of every append window.
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
        // `scale="0.4"`: 1280 · 0.4 = 512 px per model unit — 1596's own "1 model unit at 512 px".
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
        // The composite's own inputs: the pane's rect in the quad pass's space (y-down window
        // px), its callback rank, its own alpha.
        assert!(
            (req.rect.width() - 50.0).abs() < 1e-3 && (req.rect.height() - 50.0).abs() < 1e-3,
            "{:?}",
            req.rect
        );
        assert!((req.alpha - 1.0).abs() < 1e-6);
        assert!(premultiplied(&app).is_empty(), "no cell yet: nothing drawn");
    }

    // The renderer hands a cell back. NOTHING else changes — no ping, no frame moves, the
    // memoized conversion skips — and the next frame draws the cell anyway.
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
    // And every quiet frame after it — the lane is re-emitted per frame, not on change.
    next_frame(&mut app);
    assert_eq!(
        premultiplied(&app).len(),
        1,
        "still drawn on the frame after"
    );

    // A fresh ping moves the pane: the conversion runs again, the request follows the rect, and
    // the composite follows the request — still exactly one quad.
    {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        // The stock handler seats the frame from `GetPingPosition()`, not the event's args.
        script.set_minimap_ping((-0.125, 0.25));
        ping_at(&mut script, -0.125, 0.25);
    }
    next_frame(&mut app);
    let moved = premultiplied(&app);
    assert_eq!(moved.len(), 1, "one quad after the pane moved");
    assert_ne!(moved[0].rect, q.rect, "the composite followed the pane");
}
