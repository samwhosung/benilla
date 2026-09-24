//! **The cinematic frame's key-down consumption** — what makes a fly-by something you *watch*.
//!
//! `CinematicFrame` is fullscreen, keyboard-enabled, and carries an `OnKeyDown` that answers
//! ESCAPE. In the reference that is enough to swallow **every** key while it is up, because the
//! key-down walk's gate is EXISTENCE, not handling (`0x76b7d0`): a shown keyboard frame with the
//! slot set consumes the key whatever its script does with it, and a 1.12 handler has no way to
//! signal "not handled" (`0x76ba25`).
//!
//! The reference's own Lua is the proof, and it is why these tests exist: that same `OnKeyDown` has
//! to call `RunBinding("SCREENSHOT")` **by hand** to get one key back. It would not need to if
//! unhandled keys fell through to their bindings.
//!
//! benilla had the walk (decision 1319) but fed it only ten key names, so ESCAPE was consumed and
//! `W` was not — and the player could walk around underneath their own intro cinematic.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The frame tree a cinematic actually runs against: `UIParent` (whose `ShowUIPanel` the frame's
/// `CINEMATIC_START` arm calls) and the cinematic frame itself.
fn ui_with_the_cinematic_frame() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua"); // StaticPopup's money row, or UiPanels errors at load
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

/// Show the cinematic frame the way the engine does — the `CINEMATIC_START` edge `feed_ui` fires.
fn start_cinematic(s: &mut UiScript) {
    s.set_in_cinematic(true);
    s.fire_event("CINEMATIC_START", vec![]);
    s.resolve();
}

/// With a fly-by on screen, the movement keys are the cinematic frame's, not the world's.
///
/// `frame_key_input` returning `true` is exactly the consumption that suppresses the key's binding
/// and the gameplay readers ([`super::UiKeyboardCapture`]) — so this *is* the assertion that you
/// cannot walk during a cinematic.
#[test]
fn a_playing_cinematic_swallows_the_movement_keys() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();

    // Before it starts, the frame is hidden and declines: the world keeps its keys.
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

/// The two keys the reference deliberately keeps working, and the third it does not.
///
/// ESCAPE ends the shot; the screenshot binding is re-run by hand from inside the handler
/// (`RunBinding("SCREENSHOT")`) precisely *because* consumption is unconditional. Both are
/// consumed at the walk either way — "keeps working" means the frame's own script acts on them,
/// never that they fall through.
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

/// The host feed itself: every key the walk is supposed to carry must actually *have* a name, or
/// the delivery in [`super::input`] is a silent no-op for it. This is the half that was missing —
/// the walk was faithful, the feed was ten keys wide.
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

/// **The one key a cinematic gives back.** The module doc above says the reference's `OnKeyDown`
/// calls `RunBinding("SCREENSHOT")` by hand *because* the walk's gate is existence — and benilla
/// shipped that transcribed line against a `RunBinding` that did not exist in the whole codebase.
/// Every press of the screenshot key during a fly-by raised `attempt to call global 'RunBinding'`
/// and took no picture, and the three tests above stayed green through all of it because they only
/// ever asked about keys the frame *swallows*.
#[test]
fn the_screenshot_key_is_handed_back_to_its_binding() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();
    // The binding table the passthrough reads: `GetBindingKey("SCREENSHOT")` has to answer, or the
    // arm is skipped and the test proves nothing.
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

    // Consumed like every other key — the frame is up, so the binding layer must not also see it.
    assert!(
        s.frame_key_input(&key),
        "the frame consumes {key} like any other key"
    );
    // …and handed straight back to its command by hand. This is the whole point of the arm: the
    // request is what the host runs, and an error here means no screenshot.
    assert_eq!(
        s.take_keybind_requests(),
        vec![benilla_ui::script::keybind::KeybindRequest::Run(
            "SCREENSHOT".into()
        )],
        "the screenshot key must reach its binding through RunBinding"
    );

    // ESCAPE is still the skip, and still queues nothing on the binding channel.
    assert!(s.frame_key_input("ESCAPE"));
    assert!(s.take_keybind_requests().is_empty());
}

/// **The HUD hide, end to end — and the frame that must survive it.**
///
/// This is the chain decision 1734 restored, and every link was broken until it did:
/// `CINEMATIC_START` → `CinematicFrame`'s arm calls `ShowUIPanel` → its `area = "full"` row routes
/// to `SetFullScreenFrame` → which hides `UIParent` → which cascades to every frame declaring
/// `parent="UIParent"`. Before, `SetFullScreenFrame` had that line dropped, and there was almost
/// nothing parented to cascade to; the engine hid the HUD with `UiHidden` instead, and paid for it
/// with a mouse that could not find the cinematic frame.
///
/// It replaces a log line. 1699 verified the takeover by watching `cinematic: HUD hidden for
/// playback` go past, because `UiHidden` hides at the *draw* and leaves every widget answering
/// `IsVisible() == true` — the predicate was useless. Hiding through the real cascade makes
/// `IsVisible` the honest question again, so the check is an assertion instead of a log.
#[test]
fn a_cinematic_hides_the_hud_through_uiparent_and_spares_the_cinematic_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = ui_with_the_cinematic_frame();
    let visible = |s: &UiScript, f: &str| {
        s.eval::<i64>(&format!("return {f}:IsVisible() and 1 or 0"))
            .unwrap()
            == 1
    };

    // A shown child of UIParent stands in for the HUD: `UiPanels.xml` gives us one without
    // dragging the whole action bar in, and what is under test is the cascade, not which frame.
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

/// **The screenshot confirmation survives the HUD hide — because the reference seats it outside
/// the HUD.** `WorldFrame.xml`'s own header states the law in one line: *"Children of the world
/// frame are visible even when the UI is turned off."* `ScreenshotStatus` is declared inside that
/// frame, and the case that needs it is this file's: `CinematicFrame`'s `OnKeyDown` hands the
/// SCREENSHOT key back to its binding by hand (1701) precisely so a fly-by can be captured, so the
/// one moment the confirmation is most certainly wanted is the one where `UIParent` is hidden.
///
/// Ours sat on `UIParent` until decision 1757, on a note that read the two seats as equivalent
/// because both frames are full-screen and their CENTERs coincide — true of the geometry, and
/// silent about the hide.
#[test]
fn the_screenshot_confirmation_shows_during_a_cinematic() {
    benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches (decision 1848).
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

    // The engine's own report of a finished capture (`crate::screenshot` → SCREENSHOT_SUCCEEDED).
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
