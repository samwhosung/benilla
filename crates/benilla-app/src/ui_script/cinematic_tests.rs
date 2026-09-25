//! The cinematic frame swallows every key while it is up: the key-down walk gates on a shown
//! keyboard frame with the slot set, not on handling (`0x76b7d0`), and a 1.12 handler cannot
//! decline a key (`0x76ba25`), so the stock `OnKeyDown` re-runs `SCREENSHOT` by hand.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The cinematic frame over `UIParent`, whose `ShowUIPanel` its `CINEMATIC_START` arm calls.
fn ui_with_the_cinematic_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua"); // StaticPopup's money row
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\CinematicFrame.xml");
    s.resolve();
    s
}

/// The engine's `CINEMATIC_START` edge, as `feed_ui` fires it.
fn start_cinematic(s: &mut UiScript) {
    s.set_in_cinematic(true);
    s.fire_event("CINEMATIC_START", vec![]);
    s.resolve();
}

/// `frame_key_input` returning true keeps the key from its binding and the gameplay readers
/// ([`super::UiKeyboardCapture`]).
#[test]
fn a_playing_cinematic_swallows_the_movement_keys() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();

    for key in ["W", "A", "S", "D", "SPACE", "1"] {
        assert!(
            !s.frame_key_input(key),
            "{key} must reach the world while no cinematic is playing"
        );
    }

    start_cinematic(&mut s);
    assert_eq!(
        s.eval::<i64>("return CinematicFrame:IsVisible() and 1 or 0")
            .unwrap(),
        1,
        "the frame itself is up — `InCinematic()` would only echo the flag this test just set"
    );
    for key in ["W", "A", "S", "D", "SPACE", "1", "F1", "P"] {
        assert!(
            s.frame_key_input(key),
            "{key} must be swallowed by the cinematic frame"
        );
    }
}

#[test]
fn escape_is_consumed_and_acted_on_rather_than_falling_through() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();
    start_cinematic(&mut s);

    assert!(
        s.frame_key_input("ESCAPE"),
        "ESCAPE is consumed like any key"
    );
    assert!(
        s.take_session_requests()
            .iter()
            .any(|r| matches!(r, benilla_ui::script::SessionRequest::StopCinematic)),
        "and its handler asked the engine to stop the cinematic"
    );
}

/// A key with no name is silently never delivered to a keyboard frame ([`super::input`]).
#[test]
fn the_host_has_a_reference_name_for_the_keys_it_now_delivers() {
    use crate::bindings::chord::key_token;
    use bevy::prelude::KeyCode;

    for (code, name) in [
        (KeyCode::KeyW, "W"),
        (KeyCode::KeyA, "A"),
        (KeyCode::KeyS, "S"),
        (KeyCode::KeyD, "D"),
        (KeyCode::Space, "SPACE"),
        (KeyCode::Digit1, "1"),
        (KeyCode::F1, "F1"),
    ] {
        assert_eq!(
            key_token(code),
            Some(name),
            "{code:?} must reach a keyboard frame under the reference's own name"
        );
    }
}

#[test]
fn the_screenshot_key_is_handed_back_to_its_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();
    // `GetBindingKey("SCREENSHOT")` must answer, or the handler's screenshot arm is skipped.
    s.register_bindings(&crate::bindings::registry_commands());
    s.seed_binding_set(1, None);
    s.load_binding_set(1);
    let key = s
        .keybind_snapshot()
        .into_iter()
        .find(|(name, _)| name == "SCREENSHOT")
        .and_then(|(_, keys)| keys.into_iter().next())
        .expect("SCREENSHOT ships a default chord");

    start_cinematic(&mut s);
    let _ = s.take_keybind_requests();

    assert!(
        s.frame_key_input(&key),
        "the frame consumes {key} like any other key"
    );
    assert_eq!(
        s.take_keybind_requests(),
        vec![benilla_ui::script::keybind::KeybindRequest::Run(
            "SCREENSHOT".into()
        )],
        "the screenshot key must reach its binding through RunBinding"
    );

    // ESCAPE still skips, and queues nothing on the binding channel.
    assert!(s.frame_key_input("ESCAPE"));
    assert!(s.take_keybind_requests().is_empty());
}

/// `ShowUIPanel`'s `area = "full"` row (`UIParent.lua:31`) routes to `SetFullScreenFrame`, which
/// hides `UIParent` and so every frame parented to it; `CinematicFrame` declares no parent.
#[test]
fn a_cinematic_hides_the_hud_through_uiparent_and_spares_the_cinematic_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();
    let visible = |s: &UiScript, f: &str| {
        s.eval::<i64>(&format!("return {f}:IsVisible() and 1 or 0"))
            .unwrap()
            == 1
    };

    // `StaticPopup1`, a shown child of `UIParent`, stands in for the HUD.
    s.eval::<i64>("StaticPopup1:Show() return 0").unwrap();
    s.resolve();
    assert!(visible(&s, "UIParent"), "the HUD's parent starts visible");
    assert!(visible(&s, "StaticPopup1"), "and so does its child");

    start_cinematic(&mut s);
    assert!(!visible(&s, "UIParent"), "SetFullScreenFrame hid UIParent");
    assert!(
        !visible(&s, "StaticPopup1"),
        "and the hide cascaded to its children — this is the whole point of the 72 restored \
         parent= declarations, and the assertion that fails if one is dropped again"
    );
    assert_eq!(
        s.eval::<i64>("return StaticPopup1:IsShown() and 1 or 0")
            .unwrap(),
        1,
        "cascaded, not shown=false: the frame keeps its own state and gets it back untouched"
    );
    assert!(
        visible(&s, "CinematicFrame"),
        "**and the fly-by itself survives** — the reference declares CinematicFrame with no \
         parent precisely so the frame being shown escapes the hide that showing it performs"
    );

    s.set_in_cinematic(false);
    s.fire_event("CINEMATIC_STOP", vec![]);
    s.resolve();
    assert!(visible(&s, "UIParent"), "and the world's UI comes back");
    assert!(visible(&s, "StaticPopup1"), "with the child that was up");
}

/// `ScreenshotStatus` is a child of `WorldFrame`, whose children stay visible with the UI hidden
/// (`WorldFrame.xml`).
#[test]
fn the_screenshot_confirmation_shows_during_a_cinematic() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The UI loads on world entry, so a player always exists by then.
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

    start_cinematic(&mut s);
    assert_eq!(visible(&s, "UIParent"), 0, "the fly-by hid the HUD");

    // The engine's report of a finished capture (`crate::screenshot`).
    s.fire_event("SCREENSHOT_SUCCEEDED", vec![]);
    s.resolve();
    assert_eq!(
        visible(&s, "ScreenshotStatus"),
        1,
        "\"Screen Captured\" is readable over the fly-by — a confirmation that cannot appear \
         while the UI is hidden is no confirmation, and the cinematic is the case the reference \
         hands the key back for"
    );
    assert_eq!(
        s.eval::<String>("return ScreenshotStatusText:GetText()")
            .unwrap(),
        "Screen Captured",
        "with the reference's own string"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
