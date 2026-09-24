//! The chat tab's **options menu**, end to end (fixing **B246**: *"no chat options
//! at all — background transparency has no home, and chat can be hard to read"*).
//!
//! These drive the shipped `ChatFrame.xml` through the shipped dropdown kit and the shipped colour
//! picker, from the mouse event inward — a right-click on the real tab, a click on the real
//! **Background** row, a drag of the real `OpacitySliderFrame` — because the report is about a
//! path, not about a function. Every hop it names (is the menu reachable? does the row open the
//! picker? does the slider move the chat box? does the value survive?) is a hop where the feature
//! could be absent while every unit underneath it passed.
//!
//! The manifest is `color_picker_tests`' (fonts → panel manager → widget kit → picker) plus
//! `ChatFrame.xml` last, which is `benilla.toc`'s own order.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The GlobalStrings the menu labels itself with. The app runs the real
/// `Interface\FrameXML\GlobalStrings.lua` off the player's own patch chain at boot
/// (`load_global_strings`); these stand in for it here, at their real 1.12.1 values —
/// `BACKGROUND` l.131 (whose own trailing comment in that file reads *"Title in the chat
/// preferences menu"*, i.e. this exact row), `DISPLAY` l.937, `FONT_SIZE` l.1983,
/// `FONT_SIZE_TEMPLATE` l.1984, `CHAT_OPTIONS_LABEL` l.673, `NEWBIE_TOOLTIP_CHATOPTIONS` l.2724.
fn bake_strings(s: &UiScript) {
    s.run(
        r#"
        BACKGROUND = "Background"
        DISPLAY = "Display"
        FONT_SIZE = "Font Size"
        FONT_SIZE_TEMPLATE = "%d pt"
        CHAT_OPTIONS_LABEL = "Chat Options"
        NEWBIE_TOOLTIP_CHATOPTIONS = "Right-click to get a list of customizable options for this window. Left-click and drag to move the window."
    "#,
    )
    .unwrap();
}

/// The chat window with everything its tab menu reaches under it.
fn chat_with_menu() -> UiScript {
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
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        "Interface\\FrameXML\\ColorPickerFrame.xml",
        "Interface\\FrameXML\\UIMenu.xml", // the kit ChatMenu/EmoteMenu/VoiceMacroMenu build from
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\ChatFrame.xml",
        "Interface\\FrameXML\\UIPanelTemplates.lua",
        "Interface\\FrameXML\\UIPanelTemplates.xml",
        "Interface\\FrameXML\\FloatingChatFrame.xml",
    ] {
        load_xml(&s, file);
    }
    bake_strings(&s);
    super::fire_chat_login(&mut s);
    s.set_screen_size(1600.0, 900.0);
    s.resolve();
    s
}

/// Right-click a frame's centre through the real hit path.
/// The tabs ship hidden and the dock reveals them on a stationary hover (`FCF_OnUpdate`); a
/// click on a tab needs that reveal first.
fn reveal_then(s: &mut UiScript) {
    reveal_dock(s);
}

fn right_click(s: &mut UiScript, frame: &str) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!("return {frame}:GetCenter()"))
        .unwrap();
    s.mouse_button(x as f32, y as f32, "RightButton", true);
    s.mouse_button(x as f32, y as f32, "RightButton", false);
    s.resolve();
}

/// Move the mouse onto a frame's centre through the real hit path (fires its `OnEnter`).
fn hover(s: &mut UiScript, frame: &str) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!("return {frame}:GetCenter()"))
        .unwrap();
    s.mouse_move(x as f32, y as f32);
    s.resolve();
}

/// Settle the dock's hover fade all the way open: park the cursor in the middle of the window and
/// run past the 0.2 s stationary arm plus the 0.15 s ramp (`chat_tests`' own idiom).
fn reveal_dock(s: &mut UiScript) {
    let (x, y): (f32, f32) = s
        .eval(
            "return (ChatFrame1:GetLeft() + ChatFrame1:GetRight()) / 2, \
             (ChatFrame1:GetBottom() + ChatFrame1:GetTop()) / 2",
        )
        .unwrap();
    s.mouse_move(x, y);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
}

/// Left-click a frame's centre through the real hit path.
fn left_click(s: &mut UiScript, frame: &str) {
    let (x, y) = s
        .eval::<(f64, f64)>(&format!("return {frame}:GetCenter()"))
        .unwrap();
    s.mouse_button(x as f32, y as f32, "LeftButton", true);
    s.mouse_button(x as f32, y as f32, "LeftButton", false);
    s.resolve();
}

/// **B246's first half: the menu exists and the right button reaches it.**
///
/// Before 1589 the tab registered only `LeftButtonUp`, so the right-click was swallowed by the
/// engine's default click set and no handler ever ran — which is why the report reads "no chat
/// options at all" rather than "the menu is missing a row".
#[test]
fn right_clicking_a_chat_tab_opens_its_options_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "no menu before any click"
    );
    reveal_then(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    assert!(
        s.eval::<bool>("return DropDownList1:IsVisible()").unwrap(),
        "the tab's right-click opens the options menu"
    );
    // The reference's first six rows for the default window (FloatingChatFrame.lua
    // `FCFOptionsDropDown_Initialize`, level 1): the lock verb, Rename, New Window, the Display
    // title, Font Size and Background — a docked non-default window gets Close before Display.
    for (n, key) in [
        (1, "UNLOCK_WINDOW"),
        (2, "RENAME_CHAT_WINDOW"),
        (3, "NEW_CHAT_WINDOW"),
        (4, "DISPLAY"),
        (5, "FONT_SIZE"),
        (6, "BACKGROUND"),
    ] {
        assert!(
            s.eval::<bool>(&format!("return DropDownList1Button{n}:GetText() == {key}"))
                .unwrap(),
            "row {n} is {key}"
        );
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A LEFT click still selects the tab and does not open the menu — the control that must not
/// change. (The reference's own fork: the right-button arm returns before the select.)
#[test]
fn a_left_click_still_selects_the_tab_and_opens_no_menu() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    reveal_then(&mut s);
    left_click(&mut s, "ChatFrame2Tab");
    assert_eq!(
        s.eval::<i64>("return SELECTED_DOCK_FRAME:GetID()").unwrap(),
        2,
        "left-click selects"
    );
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "a left click opens no menu"
    );
    right_click(&mut s, "ChatFrame1Tab");
    assert!(s.eval::<bool>("return DropDownList1:IsVisible()").unwrap());
    left_click(&mut s, "ChatFrame1Tab");
    assert!(
        s.eval::<bool>("return not DropDownList1:IsVisible()")
            .unwrap(),
        "the left click closed the open menu"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// **B246's actual ask: the background slider.** Background → the colour picker with its opacity
/// slider → dragging the thumb moves the chat window's stored alpha *and* the pixels.
///
/// The two numbers this pins are the ones a wrong implementation gets wrong in opposite
/// directions: the slider is **reversed** (0 at the top is fully opaque), so the seed is `1 - a`
/// and the read-back is `1 - value`; and the store is a **byte**, so 0.8 comes back as
/// `204/255`, not as 0.8.
#[test]
fn the_background_row_opens_the_picker_and_its_opacity_slider_drives_the_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    reveal_then(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    assert_eq!(
        s.eval::<f64>("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap(),
        0.0
    );
    assert_eq!(
        s.eval::<f64>("return DropDownList1Button6.opacity")
            .unwrap(),
        1.0,
        "info.opacity is 1 - a (the ref's own 'the slider is reversed')"
    );
    left_click(&mut s, "DropDownList1Button6ColorSwatch");
    assert!(
        s.eval::<bool>("return ColorPickerFrame:IsVisible()")
            .unwrap(),
        "the Background swatch opens the colour picker"
    );
    // The player's hand is on the picker now, off the chat frame: the hover's fade-out runs to
    // its end before the slider is touched, exactly as it does in real time.
    s.mouse_move(1500.0, 850.0);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    assert!(
        s.eval::<bool>("return OpacitySliderFrame:IsVisible()")
            .unwrap(),
        "and the picker wears its opacity slider — B246's 'background slider'"
    );
    assert_eq!(
        s.eval::<f64>("return OpacitySliderFrame:GetValue()")
            .unwrap(),
        1.0,
        "the slider opens seeded from the window"
    );
    s.run("OpacitySliderFrame:SetValue(1 - 0.8)").unwrap();
    let stored: f64 = s
        .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
        .unwrap();
    // 204/255, and the last byte of that is 2133's doing. The slider carries
    // `valueStep="0.01"`, so `SetValue` snaps `1 - 0.8` onto the lattice and rebuilds it as
    // `n·step`; the reconstruction lands a hair BELOW the literal, `1 - value` a hair above 0.8,
    // and the alpha store's floor quantizer then keeps 204 where it used to shed one to 203.
    // The player asked for 0.8 and the store holds 0.8 — the client's own arithmetic, improved
    // by accident.
    assert!(
        (stored - 204.0 / 255.0).abs() < 1e-9,
        "the drag reached the engine store, quantized to its byte: {stored}"
    );
    s.mouse_move(1500.0, 850.0);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    let painted: f64 = s.eval("return ChatFrame1Background:GetAlpha()").unwrap();
    assert!(
        (painted - 0.8).abs() < 1e-6,
        "a window the player made solid stays solid off-hover: {painted}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The default look is **unchanged** by all of the above — the alpha rule generalised, it did not
/// move. A window at the shipped base of 0 still rides the full 0 → `DEFAULT_CHATFRAME_ALPHA`
/// hover ramp, which is the look the director signed off in 0288.
#[test]
fn the_shipped_window_still_fades_zero_to_a_quarter() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    s.mouse_move(1500.0, 850.0);
    for _ in 0..4 {
        s.tick(0.016);
        s.resolve();
    }
    assert_eq!(
        s.eval::<f64>("return ChatFrame1Background:GetAlpha()")
            .unwrap(),
        0.0,
        "at rest: invisible"
    );
    reveal_dock(&mut s);
    let alpha: f64 = s.eval("return ChatFrame1Background:GetAlpha()").unwrap();
    assert!(
        (alpha - 0.25).abs() < 1e-6,
        "hovered: DEFAULT_CHATFRAME_ALPHA — got {alpha}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The **Font Size** submenu — B246's other half, *"chat can be hard to read"*. The four heights
/// are the reference's `CHAT_FONT_HEIGHTS`, the tick follows the font the frame is wearing, and a
/// pick moves both the live font and the stored `SIZE`.
#[test]
fn the_font_size_submenu_resizes_the_window_and_stores_the_pick() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    reveal_then(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    hover(&mut s, "DropDownList1Button5");
    assert_eq!(
        s.eval::<i64>("return DropDownList2.numButtons").unwrap(),
        4,
        "CHAT_FONT_HEIGHTS is 12, 14, 16, 18"
    );
    // The rows come from `for index, value in CHAT_FONT_HEIGHTS` — a `next` walk over a table
    // built with explicit `[n] =` keys, whose order is the VM's hash order and not necessarily
    // ascending; the test reads the rows by their values rather than assuming one.
    let row_with = |s: &UiScript, pt: i64| -> String {
        s.eval::<String>(&format!(
            "for i = 1, DropDownList2.numButtons do \
                 local b = getglobal('DropDownList2Button'..i) \
                 if b.value == {pt} then return b:GetName() end \
             end return ''"
        ))
        .unwrap()
    };
    assert_eq!(
        s.eval::<String>(&format!("return {}:GetText()", row_with(&s, 12)))
            .unwrap(),
        "12 pt",
        "FONT_SIZE_TEMPLATE over the value"
    );
    let ticked = row_with(&s, 14);
    assert!(
        s.eval::<bool>(&format!("return {ticked}Check:IsVisible()"))
            .unwrap(),
        "the tick follows the font the frame is actually wearing (ChatFontNormal, 14)"
    );
    let sixteen = row_with(&s, 16);
    left_click(&mut s, &sixteen);
    let (_, height): (String, f64) = s
        .eval("local f, h = ChatFrame1:GetFont() return f, h")
        .unwrap();
    assert_eq!(height, 16.0, "the live font moved");
    assert_eq!(
        s.eval::<i64>("local _, size = GetChatWindowInfo(1) return size")
            .unwrap(),
        16,
        "and the pick is stored as the cache's SIZE"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The menu belongs to the tab that opened it: window 2's menu writes window 2. This is what the
/// `id` on the tab and the per-tab capsule buy — `FCF_GetCurrentChatFrameID` reads the open menu's
/// parent, so a single shared capsule would have written window 1 whichever tab was clicked.
#[test]
fn each_tabs_menu_writes_its_own_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    reveal_then(&mut s);
    right_click(&mut s, "ChatFrame2Tab");
    assert_eq!(
        s.eval::<i64>("return FCF_GetCurrentChatFrameID()").unwrap(),
        2
    );
    // A docked non-default window's menu carries Close before Display, so Background is row 7.
    assert!(s
        .eval::<bool>("return DropDownList1Button7:GetText() == BACKGROUND")
        .unwrap());
    left_click(&mut s, "DropDownList1Button7ColorSwatch");
    s.run("OpacitySliderFrame:SetValue(0)").unwrap(); // reversed: 0 = fully opaque
    assert_eq!(
        s.eval::<f64>("local _,_,_,_,_,a = GetChatWindowInfo(2) return a")
            .unwrap(),
        1.0,
        "window 2 took the write"
    );
    assert_eq!(
        s.eval::<f64>("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap(),
        0.0,
        "window 1 did not"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// Cancel restores the colour and the alpha the row was opened with — the picker's own
/// `previousValues` contract, which the reference reaches through the same `cancelFunc` field.
#[test]
fn cancelling_the_picker_puts_the_window_back() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    s.run("FCF_SetWindowAlpha(ChatFrame1, 0.4) FCF_SetWindowColor(ChatFrame1, 0.2, 0.4, 0.6)")
        .unwrap();
    let before: f64 = s
        .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
        .unwrap();
    reveal_then(&mut s);
    right_click(&mut s, "ChatFrame1Tab");
    left_click(&mut s, "DropDownList1Button6ColorSwatch");
    s.run("OpacitySliderFrame:SetValue(0)").unwrap();
    assert_eq!(
        s.eval::<f64>("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
            .unwrap(),
        1.0,
        "the drag previewed live"
    );
    left_click(&mut s, "ColorPickerCancelButton");
    let after: f64 = s
        .eval("local _,_,_,_,_,a = GetChatWindowInfo(1) return a")
        .unwrap();
    assert!(
        (after - before).abs() < 1e-9,
        "Cancel restored the alpha: {after} vs {before}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The `UPDATE_CHAT_WINDOWS` seam: the host restores a player's saved file into the engine table
/// and fires the reference's own event; the window re-reads it and repaints. Without this the
/// restore would land in the store and show up only after something else happened to invalidate
/// the fade latch.
#[test]
fn the_restore_event_repaints_the_window_from_the_store() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = chat_with_menu();
    s.set_chat_window_looks([(
        0,
        benilla_ui::script::ChatWindowLook {
            r: 255,
            g: 0,
            b: 0,
            a: 255,
            font_size: 18,
            locked: true,
            docked: Some(1),
            ..Default::default()
        },
    )]);
    // The reference paints colour, alpha and lock from the record on the FIRST
    // `UPDATE_CHAT_WINDOWS` a frame sees (`FloatingChatFrame_Update`'s `not isInitialized`
    // gate) — a login, which is when the app restores the file. The helper already fired that
    // one, so this is the frame meeting the event fresh, as at the next login.
    s.run("ChatFrame1.isInitialized = nil").unwrap();
    s.fire_event("UPDATE_CHAT_WINDOWS", vec![]);
    s.mouse_move(1500.0, 850.0);
    for _ in 0..45 {
        s.tick(0.016);
        s.resolve();
    }
    assert_eq!(
        s.eval::<f64>("return ChatFrame1Background:GetAlpha()")
            .unwrap(),
        1.0,
        "the restored alpha is on screen"
    );
    let (r, g, b): (f64, f64, f64) = s
        .eval("return ChatFrame1Background:GetVertexColor()")
        .unwrap();
    assert_eq!((r, g, b), (1.0, 0.0, 0.0), "and the restored tint");
    // The restore paints through the `doNotSave` arms, but the reference's dock pass that follows
    // (`FCF_DockFrame` → `FCF_SaveDock` → `SetChatWindowDocked`) is a real write — the DOCKED 1 /
    // DOCKED 2 every stock file carries are FrameXML's, not the loader's.
    assert_eq!(s.take_chat_window_changes(), vec![0, 1]);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
