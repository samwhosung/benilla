//! The chat window's **move, resize and lock**, end to end.
//!
//! Like `chat_options_tests` next door, these drive the shipped `ChatFrame.xml` from the mouse
//! event inward — a press on the real grip button, a drag of the real window body, a click on the
//! real menu row — because what is being built is a *path*, not a function. Every hop it names is
//! a hop where the feature could be absent while every unit underneath it passed: is the grip a
//! hit target at all? does the lock stop it? does the clamp hold? does the geometry reach the file
//! and come back?
//!
//! The manifest is `chat_options_tests`' — `benilla.toc`'s own order, up to `ChatFrame.xml`.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The chat dock with everything its tab menu and its grips reach under it.
fn chat_ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        r"Interface\FrameXML\UIParent.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\ColorPickerFrame.xml",
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        // `FCF_ValidateChatFramePosition` (a tab-drag stop) reads `MainMenuBar:GetHeight()`; the
        // bar's own load-time chain precedes it, as in the action-bar harness.
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, file);
    }
    super::fire_chat_login(&mut s);
    s.set_screen_size(1600.0, 900.0);
    s.resolve();
    s
}

/// A frame's centre, through the real layout.
fn centre(s: &mut UiScript, frame: &str) -> (f32, f32) {
    s.resolve();
    let (x, y): (f64, f64) = s
        .eval(&format!("return {frame}:GetCenter()"))
        .unwrap_or_else(|e| panic!("{frame}:GetCenter(): {e}"));
    (x as f32, y as f32)
}

fn width(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetWidth()").unwrap() as f32
}

fn height(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetHeight()").unwrap() as f32
}

fn left(s: &mut UiScript) -> f32 {
    s.resolve();
    s.eval::<f64>("return ChatFrame1:GetLeft()").unwrap() as f32
}

/// Right-click a frame's centre through the real hit path.
fn right_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    s.resolve();
}

/// Left-click a frame's centre through the real hit path.
fn left_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.resolve();
}

/// Press a grip and walk the cursor to `(x, y)` — the real gesture, without releasing.
/// The tabs ship hidden; a stationary hover over the dock reveals them (`FCF_OnUpdate`), and
/// the reference moves a window by dragging its TAB (ChatTabTemplate's OnDragStart →
/// `StartMoving`), so every drag here starts with the reveal.
fn reveal(s: &mut UiScript) {
    let (cx, cy) = centre(s, "ChatFrame1");
    s.mouse_move(cx, cy);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
}

/// Drag `ChatFrame1Tab` by `dx, dy` — the reference's move gesture — and release.
fn drag_tab(s: &mut UiScript, dx: f32, dy: f32) {
    reveal(s);
    let (tx, ty) = centre(s, "ChatFrame1Tab");
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + 5.0, ty); // past the 4px threshold: OnDragStart -> StartMoving
    s.mouse_move(tx + dx, ty + dy);
    s.mouse_button(tx + dx, ty + dy, "LeftButton", false);
    s.resolve();
}

fn grab_grip(s: &mut UiScript, grip: &str) -> (f32, f32) {
    let (x, y) = centre(s, grip);
    s.mouse_button(x, y, "LeftButton", true);
    s.resolve();
    (x, y)
}

/// The reference's stock state: **both dock windows ship locked**, and that is what
/// `GetChatWindowInfo` answers, because it is the chat-cache's own `LOCKED 1`.
#[test]
fn both_dock_windows_ship_locked() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = chat_ui();
    for i in 1..=2 {
        assert_eq!(
            s.eval::<i64>(&format!(
                "local _,_,_,_,_,_,_,locked = GetChatWindowInfo({i}) return locked"
            ))
            .unwrap(),
            1,
            "window {i} ships locked (the boot init's LOCKED 1)"
        );
        assert!(
            s.eval::<bool>(&format!("return ChatFrame{i}.isLocked ~= nil"))
                .unwrap(),
            "…and the frame was seated from it by FloatingChatFrame_Update"
        );
    }
    assert!(s
        .eval::<Option<i64>>("return FCF_Get_ChatLocked()")
        .unwrap()
        .is_none());
}

/// **The tab menu's Lock row: it is there, it toggles, and its label flips.**
///
/// The reference's FIRST row (FloatingChatFrame.lua l.236-246): `UNLOCK_WINDOW` when the frame is
/// locked, `LOCK_WINDOW` when it is not, `notCheckable`.
#[test]
fn the_tab_menus_lock_row_toggles_the_window_and_flips_its_label() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    reveal(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    assert!(s
        .eval::<bool>("return DropDownList1Button1:GetText() == UNLOCK_WINDOW")
        .unwrap());
    assert_eq!(
        s.eval::<i64>("return DropDownList1Button1.notCheckable")
            .unwrap(),
        1,
        "a verb, not a tick"
    );
    left_click(&mut s, "DropDownList1Button1");
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.isLocked")
            .unwrap()
            .is_none(),
        "the row unlocked the window"
    );
    assert!(
        s.eval::<Option<i64>>("local _,_,_,_,_,_,_,l = GetChatWindowInfo(1) return l")
            .unwrap()
            .is_none(),
        "…and wrote it through SetChatWindowLocked, so it can be persisted"
    );
    right_click(&mut s, "ChatFrame1Tab");
    assert!(s
        .eval::<bool>("return DropDownList1Button1:GetText() == LOCK_WINDOW")
        .unwrap());
    left_click(&mut s, "DropDownList1Button1");
    assert_eq!(
        s.eval::<i64>("return ChatFrame1.isLocked").unwrap(),
        1,
        "and back"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A locked window refuses the grip.** Two things have to hold and only one of them is the
/// early return: the grip must not take the click at all (or it would shadow the chat line under
/// it — see the template's note), and `FCF_Resize` must refuse even if something did reach it.
#[test]
fn a_locked_window_refuses_a_grip_drag() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    let before = width(&mut s);
    assert_eq!(before, 430.0, "the authored width");
    // The reference's grips take the mouse whatever the lock says — the refusal is
    // `FCF_Resize`'s `isLocked` gate, not a disabled button.
    assert!(s
        .eval::<bool>("return ChatFrame1ResizeBottomRight:IsMouseEnabled()")
        .unwrap());
    let (x, y) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    s.mouse_move(x + 60.0, y - 40.0);
    assert_eq!(width(&mut s), before, "a locked window does not resize");
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.resizing")
            .unwrap()
            .is_none(),
        "FCF_Resize returned before StartSizing"
    );
    s.mouse_button(x + 60.0, y - 40.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **An unlocked window resizes from its BOTTOMRIGHT grip, and stops at the reference's bounds.**
///
/// 430×120 authored, `<ResizeBounds>` 296×75 … 608×400. The clamp is the half a naive pump gets
/// wrong twice over: without the rebate the size pins at the bound while the anchor keeps sliding,
/// so a window held past its minimum stops shrinking and starts *walking*.
#[test]
fn an_unlocked_bottom_right_grip_resizes_and_clamps_at_the_bounds() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    assert!(
        s.eval::<bool>("return ChatFrame1ResizeBottomRight:IsMouseEnabled()")
            .unwrap(),
        "unlocking hands the edges to the mouse"
    );
    let (x, y) = centre(&mut s, "ChatFrame1ResizeBottomRight");
    assert_eq!(
        s.hit_test_name(x, y).as_deref(),
        Some("ChatFrame1ResizeBottomRight"),
        "the grip is the topmost hit — a lowered frame level would put the window on top of it"
    );

    let bottom_before: f64 = s.eval("return ChatFrame1:GetBottom()").unwrap();
    grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    // Right and DOWN: the BOTTOMRIGHT corner follows the cursor, so both grow.
    s.mouse_move(x + 50.0, y - 30.0);
    assert_eq!(width(&mut s), 480.0, "the right edge followed the cursor");
    assert_eq!(height(&mut s), 150.0, "and the bottom edge with it");
    let bottom_after: f64 = s.eval("return ChatFrame1:GetBottom()").unwrap();
    assert!(
        (bottom_after - (bottom_before - 30.0)).abs() < 1e-3,
        "the planted TOP edge stayed put: {bottom_before} -> {bottom_after}"
    );

    // Past the maximum: the edge lands ON the bound and stays there.
    s.mouse_move(x + 900.0, y - 900.0);
    assert_eq!(width(&mut s), 608.0, "maxResize x");
    assert_eq!(height(&mut s), 400.0, "maxResize y");
    // Past the minimum, from there.
    s.mouse_move(x - 900.0, y + 900.0);
    assert_eq!(width(&mut s), 296.0, "minResize x");
    assert_eq!(height(&mut s), 75.0, "minResize y");

    s.run("ChatFrame1:StopMovingOrSizing()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The release ends the resize even when the cursor is nowhere near the grip.**
///
/// The failure this pins is not hypothetical: drag past `maxResize` and the grip *stops following*
/// (the clamp plants the edge while the cursor runs on), so the release lands on empty screen. If
/// `OnMouseUp` went to whatever is under the cursor rather than to the button that took the press,
/// `FCF_StopResize` would never run and the window would stay glued to the mouse for the rest of
/// the session.
#[test]
fn the_release_ends_the_resize_from_anywhere_on_screen() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let (x, y) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");

    s.mouse_move(x + 900.0, y);
    assert_eq!(width(&mut s), 608.0, "held against the maximum");
    // The cursor is 900px past the grip; release there.
    s.mouse_button(x + 900.0, y, "LeftButton", false);
    s.resolve();
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.resizing")
            .unwrap()
            .is_none(),
        "FCF_StopResize ran"
    );

    // The falsifier: with the drag still live, walking back would shrink the window.
    s.mouse_move(x - 200.0, y);
    assert_eq!(
        width(&mut s),
        608.0,
        "the drag is over; the window is not moving"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The window moves by a body drag, and only while unlocked.** A plain click is untouched — the
/// gesture has to cross the drag threshold before anything moves, which is what keeps 0843's
/// held-spell dismissal working on the same button.
#[test]
fn the_tab_drag_moves_an_unlocked_window_and_a_locked_one_stays_put() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    let start = left(&mut s);
    drag_tab(&mut s, 80.0, 0.0);
    assert_eq!(
        left(&mut s),
        start,
        "a locked window does not drag — ChatTabTemplate's OnDragStart returns on isLocked"
    );
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    reveal(&mut s);
    let (tx, ty) = centre(&mut s, "ChatFrame1Tab");
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + 5.0, ty);
    s.mouse_move(tx + 80.0, ty);
    assert_eq!(
        left(&mut s),
        start + 75.0,
        "the window followed the cursor from where the drag started"
    );
    s.mouse_button(tx + 80.0, ty, "LeftButton", false);
    s.mouse_move(tx + 400.0, ty);
    assert_eq!(
        left(&mut s),
        start + 75.0,
        "OnDragStop ended it — a move that outlived the button would keep going"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The dragged window is **user-placed**, and `UIParent_ManageFramePositions` therefore leaves it
/// alone. Without that the next stance-bar show/hide would `ClearAllPoints` the window and drop it
/// back on the bottom-stack seat.
#[test]
fn a_moved_window_is_user_placed_and_the_managed_pass_skips_it() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("UIParent_ManageFramePositions()").unwrap();
    let seated = left(&mut s);
    assert!(
        !s.eval::<bool>("return ChatFrame1:IsUserPlaced()").unwrap(),
        "nothing has placed it yet"
    );
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    drag_tab(&mut s, 120.0, 0.0);
    let moved = left(&mut s);
    assert_eq!(
        moved,
        seated + 115.0,
        "120 less the 5px spent on the threshold"
    );
    assert!(
        s.eval::<bool>("return ChatFrame1:IsUserPlaced()").unwrap(),
        "the drag set the userPlaced bit"
    );
    s.run("UIParent_ManageFramePositions()").unwrap();
    assert_eq!(
        left(&mut s),
        moved,
        "the managed pass does not re-seat a frame the player placed"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **The geometry survives a relog** — the whole round trip, in one test: drag and resize the
/// shipped window, snapshot it through the engine seam, write the file text, read it back, and
/// seat it into a *fresh* VM whose windows are all on their authored anchors.
#[test]
fn the_geometry_round_trips_through_the_save_file() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();

    // Resize from the bottom-right, then move the whole window.
    let (gx, gy) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    s.mouse_move(gx + 70.0, gy - 40.0);
    s.mouse_button(gx + 70.0, gy - 40.0, "LeftButton", false);
    drag_tab(&mut s, 90.0, 60.0);

    let want = (
        width(&mut s),
        height(&mut s),
        left(&mut s),
        s.eval::<f64>("return ChatFrame1:GetBottom()").unwrap() as f32,
    );
    assert_ne!(want.0, 430.0, "the resize happened");
    assert_ne!(want.2, 32.0, "the move happened");

    // The engine seam, then the file.
    let saved = s.user_placed_layouts();
    assert_eq!(saved.len(), 1, "one window was placed: {saved:?}");
    assert_eq!(saved[0].name, "ChatFrame1");
    let text = crate::ui_layout::render(&saved);
    let read_back = crate::ui_layout::parse(&text);
    assert_eq!(
        read_back, saved,
        "the file expresses what the seam produced"
    );

    // A fresh VM — the relog. Everything starts on its authored anchors.
    let mut fresh = chat_ui();
    assert_eq!(width(&mut fresh), 430.0);
    fresh.restore_user_placed_layouts(read_back);
    fresh.resolve();
    assert_eq!(
        (
            width(&mut fresh),
            height(&mut fresh),
            left(&mut fresh),
            fresh.eval::<f64>("return ChatFrame1:GetBottom()").unwrap() as f32
        ),
        want,
        "the window came back exactly where it was left"
    );
    assert!(
        fresh
            .eval::<bool>("return ChatFrame1:IsUserPlaced()")
            .unwrap(),
        "a restored window keeps the bit, so the managed pass keeps its hands off"
    );
    assert!(
        fresh.errors().is_empty(),
        "script errors: {:?}",
        fresh.errors()
    );
}

/// The **grip art rides the window's own reveal**, which is what the reference's
/// `CHAT_FRAME_TEXTURES` is for: the eight pieces and the background are tinted and faded as one,
/// so the handles appear with the box and are invisible without it.
#[test]
fn the_grip_art_follows_the_windows_reveal_and_tint() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.mouse_move(1500.0, 850.0);
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    for piece in ["Background", "ResizeBottomRightTexture", "ResizeTopTexture"] {
        assert_eq!(
            s.eval::<f64>(&format!("return ChatFrame1{piece}:GetAlpha()"))
                .unwrap(),
            0.0,
            "{piece} is invisible at rest"
        );
    }
    reveal(&mut s);
    for piece in ["Background", "ResizeBottomRightTexture", "ResizeTopTexture"] {
        let alpha: f64 = s
            .eval(&format!("return ChatFrame1{piece}:GetAlpha()"))
            .unwrap();
        assert!(
            (alpha - 0.25).abs() < 1e-6,
            "{piece} reveals with the box (DEFAULT_CHATFRAME_ALPHA): {alpha}"
        );
    }
    s.run("FCF_SetWindowColor(ChatFrame1, 1, 0, 0)").unwrap();
    let (r, g, b): (f64, f64, f64) = s
        .eval("return ChatFrame1ResizeTopTexture:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0));
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **A drag in flight pins the chrome open.** The cursor moves during a resize, which keeps
/// resetting the stationary-hover delay — without the reference's own `or chatFrame.resizing`
/// clause the box would fade out from under the hand holding it.
#[test]
fn a_drag_in_flight_holds_the_chrome_visible() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    reveal(&mut s);
    grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    assert_eq!(
        s.eval::<i64>("return ChatFrame1.resizing").unwrap(),
        1,
        "FCF_Resize marked the frame"
    );
    s.mouse_move(1500.0, 820.0);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    let alpha: f64 = s.eval("return ChatFrame1Background:GetAlpha()").unwrap();
    assert!(
        (alpha - 0.25).abs() < 1e-6,
        "the resize holds the reveal open with the cursor off the window (`or chatFrame.resizing`): {alpha}"
    );
    s.mouse_button(1500.0, 820.0, "LeftButton", false);
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.resizing")
            .unwrap()
            .is_none(),
        "FCF_StopResize cleared it"
    );
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    let alpha: f64 = s.eval("return ChatFrame1Background:GetAlpha()").unwrap();
    assert!(
        alpha < 1e-6,
        "and the box fades once the drag ends: {alpha}"
    );
}

/// The **blanket** `CHAT_LOCKED` switch overrides the per-window lock, both ways — the reference's
/// `FCF_Get_ChatLocked()` gate, which is the first line of both verbs.
#[test]
fn the_chat_locked_global_overrides_the_per_window_lock() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let before = width(&mut s);
    s.run("FCF_Set_ChatLocked(1)").unwrap();
    assert_eq!(s.eval::<String>("return CHAT_LOCKED").unwrap(), "1");
    let (x, y) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    s.mouse_move(x + 50.0, y - 30.0);
    assert_eq!(
        width(&mut s),
        before,
        "the blanket switch wins over an unlocked window — FCF_Resize's first gate"
    );
    s.mouse_button(x + 50.0, y - 30.0, "LeftButton", false);
    s.run("FCF_Set_ChatLocked(nil)").unwrap();
    let (x, y) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");
    s.mouse_move(x + 50.0, y - 30.0);
    assert_eq!(width(&mut s), before + 50.0, "and lets go when cleared");
    s.mouse_button(x + 50.0, y - 30.0, "LeftButton", false);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **ChatFrame2 is not independently movable or resizable**, whatever its own lock says: it takes
/// its rect from ChatFrame1's corners, so a drag of its own would stretch it off the dock. The
/// reference's third gate — `isDocked and chatFrame ~= DEFAULT_CHAT_FRAME`.
#[test]
fn the_docked_combat_log_cannot_be_moved_or_resized_on_its_own() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SelectDockFrame(ChatFrame2) FCF_SetLocked(ChatFrame2, nil)")
        .unwrap();
    s.resolve();
    let before: f64 = s.eval("return ChatFrame2:GetWidth()").unwrap();
    let (x, y) = grab_grip(&mut s, "ChatFrame2ResizeBottomRight");
    s.mouse_move(x + 50.0, y - 30.0);
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return ChatFrame2:GetWidth()").unwrap(),
        before,
        "a docked window that is not the default one owns no geometry — FCF_Resize's third gate"
    );
    s.mouse_button(x + 50.0, y - 30.0, "LeftButton", false);
    s.run("FCF_SelectDockFrame(ChatFrame1) FCF_SetLocked(ChatFrame1, nil)")
        .unwrap();
    drag_tab(&mut s, 60.0, 0.0);
    let one: f64 = s.eval("return ChatFrame1:GetLeft()").unwrap();
    let two: f64 = s.eval("return ChatFrame2:GetLeft()").unwrap();
    assert_eq!(one, two, "the dock moved as one");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
