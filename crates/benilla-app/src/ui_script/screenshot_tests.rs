//! Print screen's UI half — the stock `WorldFrame.xml`'s `ScreenshotStatus` — and above
//! all **B261's third clause: the "Screen Captured" line must not be in the file it announces.**
//!
//! That contract is an ORDERING, so it is tested as one. Two paths could put text in a picture and
//! both are pinned here: the ordinary press (nothing is shown until the engine reports back, which
//! it can only do after the readback) and the double press inside the 1.5 s fade (the previous
//! shot's line is still up, and `TakeScreenshot` takes it down *before* asking).

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
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
        r"Interface\FrameXML\WorldFrame.xml",
    ] {
        load_xml(&s, f);
    }
    s
}

fn shown(s: &UiScript) -> bool {
    s.eval::<bool>("return ScreenshotStatus:IsVisible()")
        .unwrap()
}

fn text(s: &UiScript) -> String {
    s.eval::<String>("return ScreenshotStatusText:GetText() or \"\"")
        .unwrap()
}

fn alpha(s: &UiScript) -> f64 {
    s.eval::<f64>("return ScreenshotStatus:GetAlpha()").unwrap()
}

/// **The bug, as a test.** A press asks the engine and shows NOTHING; the line appears only when
/// the engine reports back, which in the real client is frames later — so the frame that was
/// captured cannot contain it.
#[test]
fn the_capture_is_asked_for_silently_and_only_the_answer_speaks() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    assert!(!shown(&s), "the status line starts hidden");

    s.run("TakeScreenshot()").unwrap();
    assert_eq!(
        s.take_screenshot_asks(),
        1,
        "the binding body reaches the engine verb"
    );
    assert!(
        !shown(&s),
        "NOTHING is on screen at the moment of capture — this is B261's whole contract"
    );

    // The engine's answer, one or more frames later.
    s.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
    assert!(shown(&s));
    assert_eq!(text(&s), "Screen Captured");
    assert_eq!(alpha(&s), 1.0);
}

/// **The second leak path, and the reason `TakeScreenshot` hides rather than waiting for the
/// fade.** Press again while the last confirmation is still on screen and it comes off BEFORE the
/// engine is asked — otherwise that shot would have "Screen Captured" printed across it.
#[test]
fn a_second_press_inside_the_fade_clears_the_line_before_capturing() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
    s.tick(0.5);
    assert!(shown(&s), "half a second in, the line is still up");
    assert!(alpha(&s) < 1.0, "and already fading");

    s.run("TakeScreenshot()").unwrap();
    assert!(
        !shown(&s),
        "hidden BEFORE the engine is asked, not after it answers"
    );
    assert_eq!(s.take_screenshot_asks(), 1);

    // The new answer restarts the fade at full rather than inheriting the old alpha.
    s.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
    assert_eq!(alpha(&s), 1.0);
}

/// The 1.5 s fade: alpha falls with elapsed time and the frame takes itself off screen at the end.
#[test]
fn the_line_fades_out_over_the_reference_s_second_and_a_half() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());

    s.tick(0.75);
    let half = alpha(&s);
    assert!(
        (half - 0.5).abs() < 1e-3,
        "halfway through 1.5 s the line is at half alpha, got {half}"
    );

    s.tick(0.75);
    assert!(!shown(&s), "and it is gone at the end of the fade");
}

/// The failure path says so, in the reference's own words.
#[test]
fn a_failed_capture_shows_the_failure_string() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("SCREENSHOT_FAILED", Vec::new());
    assert!(shown(&s));
    assert_eq!(text(&s), "Screen Capture Failed");
}
