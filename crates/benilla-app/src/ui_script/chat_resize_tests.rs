//! The chat window's move, resize and lock, driven from the mouse through the stock frames.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

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
        // A tab-drag stop's `FCF_ValidateChatFramePosition` reads `MainMenuBar:GetHeight()`.
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

fn right_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "RightButton", true);
    s.mouse_button(x, y, "RightButton", false);
    s.resolve();
}

fn left_click(s: &mut UiScript, frame: &str) {
    let (x, y) = centre(s, frame);
    s.mouse_button(x, y, "LeftButton", true);
    s.mouse_button(x, y, "LeftButton", false);
    s.resolve();
}

/// The tabs ship hidden until a stationary hover reveals them (`FCF_OnUpdate`), and a window
/// moves by its tab (`OnDragStart` calls `StartMoving`), so every drag starts here.
fn reveal(s: &mut UiScript) {
    let (cx, cy) = centre(s, "ChatFrame1");
    s.mouse_move(cx, cy);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
}

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

/// The stock chat cache carries `LOCKED 1` for both dock windows.
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

/// The lock verb is the menu's first row (`FloatingChatFrame.lua:239`).
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

#[test]
fn a_locked_window_refuses_a_grip_drag() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    let before = width(&mut s);
    assert_eq!(before, 430.0, "the authored width");
    // The grips take the mouse whatever the lock; `FCF_Resize`'s `isLocked` gate refuses.
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

/// Authored 430x120; `<ResizeBounds>` 296x75 to 608x400 (`FloatingChatFrame.xml`).
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
    // Right and down: the `BOTTOMRIGHT` corner follows the cursor, so both grow.
    s.mouse_move(x + 50.0, y - 30.0);
    assert_eq!(width(&mut s), 480.0, "the right edge followed the cursor");
    assert_eq!(height(&mut s), 150.0, "and the bottom edge with it");
    let bottom_after: f64 = s.eval("return ChatFrame1:GetBottom()").unwrap();
    assert!(
        (bottom_after - (bottom_before - 30.0)).abs() < 1e-3,
        "the planted TOP edge stayed put: {bottom_before} -> {bottom_after}"
    );

    s.mouse_move(x + 900.0, y - 900.0);
    assert_eq!(width(&mut s), 608.0, "maxResize x");
    assert_eq!(height(&mut s), 400.0, "maxResize y");
    s.mouse_move(x - 900.0, y + 900.0);
    assert_eq!(width(&mut s), 296.0, "minResize x");
    assert_eq!(height(&mut s), 75.0, "minResize y");

    s.run("ChatFrame1:StopMovingOrSizing()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `OnMouseUp` goes to the button that took the press, not to the frame under the cursor.
#[test]
fn the_release_ends_the_resize_from_anywhere_on_screen() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let (x, y) = grab_grip(&mut s, "ChatFrame1ResizeBottomRight");

    s.mouse_move(x + 900.0, y);
    assert_eq!(width(&mut s), 608.0, "held against the maximum");
    s.mouse_button(x + 900.0, y, "LeftButton", false);
    s.resolve();
    assert!(
        s.eval::<Option<i64>>("return ChatFrame1.resizing")
            .unwrap()
            .is_none(),
        "FCF_StopResize ran"
    );

    // With the drag still live, walking back would shrink the window.
    s.mouse_move(x - 200.0, y);
    assert_eq!(
        width(&mut s),
        608.0,
        "the drag is over; the window is not moving"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

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

/// `UIParent_ManageFramePositions` skips a user-placed frame, the chat frames only outside simple
/// chat (`UIParent.lua:1690-1692`).
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

#[test]
fn the_geometry_round_trips_through_the_save_file() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();

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

    let saved = s.user_placed_layouts();
    assert_eq!(saved.len(), 1, "one window was placed: {saved:?}");
    assert_eq!(saved[0].name, "ChatFrame1");
    let text = crate::ui_layout::render(&saved);
    let read_back = crate::ui_layout::parse(&text);
    assert_eq!(
        read_back, saved,
        "the file expresses what the seam produced"
    );

    // A fresh VM is the relog: every window starts on its authored anchors.
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

/// `CHAT_FRAME_TEXTURES` fades and tints the background and the eight grip pieces as one.
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

/// `FCF_OnUpdate`'s `or chatFrame.resizing` clause keeps the box open with the cursor off it.
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

/// `FCF_Get_ChatLocked()` is `FCF_Resize`'s first gate, ahead of the window's own lock.
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

/// A docked non-default window takes its rect from `ChatFrame1`, so `FCF_Resize`'s third gate
/// (`isDocked and chatFrame ~= DEFAULT_CHAT_FRAME`) refuses it.
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
