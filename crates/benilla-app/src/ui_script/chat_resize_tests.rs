//! The chat window's move, resize and lock, driven from the mouse through the stock frames.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

fn chat_ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    for file in [
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        // A tab-drag stop's `FCF_ValidateChatFramePosition` reads `MainMenuBar:GetHeight()`.
        "Interface\\FrameXML\\MainMenuBar.xml",
        "Interface\\FrameXML\\Cooldown.xml",
        "Interface\\FrameXML\\ActionButtonTemplate.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
        "Interface\\FrameXML\\ColorPickerFrame.xml",
        "ScrollTemplates.xml",
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
    reveal_window(s, "ChatFrame1");
}

fn reveal_window(s: &mut UiScript, window: &str) {
    let (cx, cy) = centre(s, window);
    s.mouse_move(cx, cy);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
}

/// A move past the drag-start distance (0.01 of the 1600×900 diagonal, 15.7 root units).
const DRAG_STARTS_AT: f32 = 20.0;

fn drag_tab(s: &mut UiScript, dx: f32, dy: f32) {
    reveal(s);
    let (tx, ty) = centre(s, "ChatFrame1Tab");
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + DRAG_STARTS_AT, ty); // past the drag-start distance: OnDragStart -> StartMoving
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
    s.mouse_move(tx + DRAG_STARTS_AT, ty);
    s.mouse_move(tx + 80.0, ty);
    assert_eq!(
        left(&mut s),
        start + 80.0 - DRAG_STARTS_AT,
        "the window followed the cursor from where the drag started"
    );
    s.mouse_button(tx + 80.0, ty, "LeftButton", false);
    s.mouse_move(tx + 400.0, ty);
    assert_eq!(
        left(&mut s),
        start + 80.0 - DRAG_STARTS_AT,
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
        seated + 120.0 - DRAG_STARTS_AT,
        "120 less the move spent on the drag-start distance"
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
    let crate::ui_layout::Layout::Rect(read_back) = crate::ui_layout::parse(&text) else {
        panic!("written in the rect form:\n{text}");
    };
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
            alpha == benilla_ui::color::alpha_f64(benilla_ui::color::wrap_alpha_byte(0.25)),
            "{piece} reveals with the box (DEFAULT_CHATFRAME_ALPHA): {alpha}"
        );
    }
    s.run("FCF_SetWindowColor(ChatFrame1, 1, 0, 0)").unwrap();
    let (r, g, b): (f64, f64, f64) = s
        .eval("return ChatFrame1ResizeTopTexture:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (benilla_ui::color::quant::read(1.0), 0.0, 0.0));
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
        alpha == benilla_ui::color::alpha_f64(benilla_ui::color::wrap_alpha_byte(0.25)),
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

/// Drag `window`'s tab by `(dx, dy)`: press, past the drag threshold, move, release.
fn drag_named_tab(s: &mut UiScript, window: &str, dx: f32, dy: f32) {
    reveal_window(s, window);
    let tab = format!("{window}Tab");
    let tab = tab.as_str();
    let (tx, ty) = centre(s, tab);
    assert_eq!(
        s.hit_test_name(tx, ty).as_deref(),
        Some(tab),
        "the tab is hit"
    );
    // As a player's mouse runs it: the hover is on the tab before the press, and a frame resolves
    // the moved layout before the release tests the cursor against the tab.
    s.mouse_move(tx, ty);
    s.mouse_button(tx, ty, "LeftButton", true);
    s.mouse_move(tx + DRAG_STARTS_AT, ty);
    s.mouse_move(tx + dx, ty + dy);
    s.resolve();
    s.mouse_button(tx + dx, ty + dy, "LeftButton", false);
    s.resolve();
}

/// `FCF_Tab_OnClick` reads `GetButtonState()` inside `OnClick`: NORMAL on the press (`0x779285`
/// before the push `0x7792b1`) so an undocked tab picks its window up, PUSHED on the release
/// (`0x7793a9` before `0x7793de`) so it drops it. A tab pulled out of the dock drags again, and a
/// plain click picks the window up and puts it down, never leaving it on the cursor.
#[test]
fn an_undocked_tab_drags_again_and_a_plain_click_drops_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SelectDockFrame(ChatFrame2)").unwrap();
    s.resolve();
    drag_named_tab(&mut s, "ChatFrame2", 0.0, -200.0);
    assert!(
        s.eval::<bool>("return not ChatFrame2.isDocked").unwrap(),
        "the first drag pulled the window out of the dock"
    );
    assert!(s.eval::<bool>("return MOVING_CHATFRAME == nil").unwrap());

    let first = centre(&mut s, "ChatFrame2");
    drag_named_tab(&mut s, "ChatFrame2", 0.0, 120.0);
    let second = centre(&mut s, "ChatFrame2");
    assert_eq!(
        second.1,
        first.1 + 120.0,
        "the second drag of the undocked tab moved the window"
    );

    reveal_window(&mut s, "ChatFrame2");
    let (tx, ty) = centre(&mut s, "ChatFrame2Tab");
    s.mouse_move(tx, ty);
    s.mouse_button(tx, ty, "LeftButton", true);
    assert!(
        s.eval::<bool>("return MOVING_CHATFRAME == ChatFrame2")
            .unwrap(),
        "the press picked the window up"
    );
    s.mouse_button(tx, ty, "LeftButton", false);
    assert!(
        s.eval::<bool>("return MOVING_CHATFRAME == nil").unwrap(),
        "the release put it down"
    );
    let before = centre(&mut s, "ChatFrame2");
    s.mouse_move(tx + 90.0, ty + 40.0);
    assert_eq!(
        centre(&mut s, "ChatFrame2"),
        before,
        "the window does not follow a cursor with no button held"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `(left, right, bottom, top)` of `frame`, after a resolve.
fn edges(s: &mut UiScript, frame: &str) -> (f32, f32, f32, f32) {
    s.resolve();
    let (l, r, b, t): (f64, f64, f64, f64) = s
        .eval(&format!(
            "return {frame}:GetLeft(), {frame}:GetRight(), {frame}:GetBottom(), {frame}:GetTop()"
        ))
        .unwrap();
    (l as f32, r as f32, b as f32, t as f32)
}

/// Unlocking a docked window re-anchors it at CENTER (`FloatingChatFrame.lua:785-792`); a grip
/// then moves only the grabbed edge, since the resize starts by pinning the window to one TOPLEFT
/// anchor at its rect (`0x7652b0` → `0x768430`).
#[test]
fn an_unlocked_centred_window_resizes_from_the_grabbed_edge_only() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    reveal(&mut s);
    right_click(&mut s, "ChatFrame2Tab");
    assert!(s
        .eval::<bool>("return DropDownList1Button1:GetText() == UNLOCK_WINDOW")
        .unwrap());
    left_click(&mut s, "DropDownList1Button1");
    assert_eq!(
        s.eval::<(String, i64)>("return (ChatFrame2:GetPoint(1)), ChatFrame2:GetNumPoints()")
            .unwrap(),
        ("CENTER".to_string(), 1),
        "FCF_ToggleLock undocked it and centred it"
    );

    for (grip, dx, dy) in [("Right", 40.0, 0.0), ("Top", 0.0, 30.0)] {
        reveal_window(&mut s, "ChatFrame2");
        let before = edges(&mut s, "ChatFrame2");
        let name = format!("ChatFrame2Resize{grip}");
        let (x, y) = centre(&mut s, &name);
        assert_eq!(s.hit_test_name(x, y).as_deref(), Some(name.as_str()));
        s.mouse_button(x, y, "LeftButton", true);
        s.mouse_move(x + dx, y + dy);
        s.mouse_button(x + dx, y + dy, "LeftButton", false);
        let after = edges(&mut s, "ChatFrame2");
        assert_eq!(
            after,
            (before.0, before.1 + dx, before.2, before.3 + dy),
            "the {grip} grip moved its own edge and no other"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A tab dropped off the bottom of the screen: `FCF_ValidateChatFramePosition`'s
/// `SetPoint("TOPLEFT", …)` (`FloatingChatFrame.lua:1304-1334`) replaces the drag's one TOPLEFT
/// anchor, so the window comes back whole at its own size.
#[test]
fn a_tab_dropped_off_screen_puts_the_window_back_whole() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_ui();
    s.run("FCF_SetLocked(ChatFrame1, nil)").unwrap();
    let (w, h) = (width(&mut s), height(&mut s));
    drag_tab(&mut s, 0.0, -600.0);
    let (n, point, rel, rp) = s
        .eval::<(i64, String, String, String)>(
            "local p, rel, rp = ChatFrame1:GetPoint(1) \
             return ChatFrame1:GetNumPoints(), p, rel:GetName(), rp",
        )
        .unwrap();
    assert_eq!(
        (n, point.as_str(), rel.as_str(), rp.as_str()),
        (1, "TOPLEFT", "UIParent", "TOPLEFT"),
        "the validate's anchor is the only one"
    );
    assert_eq!((width(&mut s), height(&mut s)), (w, h), "at its own size");
    let want_top: f64 = s
        .eval("return MainMenuBar:GetHeight() + ChatFrame1Tab:GetHeight()")
        .unwrap();
    let top: f64 = s.eval("return ChatFrame1:GetTop()").unwrap();
    assert!(
        (top - want_top).abs() < 1e-3,
        "back on screen: {top} vs {want_top}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
