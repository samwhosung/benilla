//! The stock screenshot status line (`WorldFrame.xml`'s `ScreenshotStatus`), which must never be
//! in the picture it announces.

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

/// The line appears only on the engine's answer, frames after the capture.
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

/// `TakeScreenshot` hides a line still fading before it calls `Screenshot()` (`WorldFrame.lua:47`).
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

    // The new answer restarts at full alpha (`WorldFrame.lua:60`).
    s.fire_event("SCREENSHOT_SUCCEEDED", Vec::new());
    assert_eq!(alpha(&s), 1.0);
}

/// `SCREENSHOT_STATUS_FADETIME` is 1.5 s (`WorldFrame.lua:44`).
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

#[test]
fn a_failed_capture_shows_the_failure_string() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.fire_event("SCREENSHOT_FAILED", Vec::new());
    assert!(shown(&s));
    assert_eq!(text(&s), "Screen Capture Failed");
}
