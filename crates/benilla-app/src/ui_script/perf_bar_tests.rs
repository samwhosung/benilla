//! The stock latency meter (`MainMenuBar.xml:344`): the tinted bar in the main bar's last empty
//! recess, its 10 s poll and its hover tooltip.

use benilla_ui::script::{QuadContent, UiScript};

use super::test_ui::load_ui as load_xml;

/// A 1024-wide screen, so the 1024-wide bar spans x 0..1024 and its offsets are screen coordinates.
fn harness(extra: &[&str]) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.lua");
    load_xml(&s, "Interface\\FrameXML\\TextStatusBar.xml");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, "Interface\\FrameXML\\MainMenuBar.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, "Interface\\FrameXML\\ActionBarFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\BonusActionBarFrame.xml");
    for f in extra {
        load_xml(&s, f);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// The meter texture's quad: its draw-order index, its rect and its tint.
fn bar_quad(s: &mut UiScript) -> (usize, [f32; 4], Option<[f32; 4]>) {
    s.resolve();
    let quads = s.extract();
    let i = quads
        .iter()
        .position(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("UI-MainMenuBar-PerformanceBar"))
        })
        .expect("the performance bar's texture quad");
    let rect = quads[i].rect.expect("a resolved rect");
    let color = match &quads[i].content {
        QuadContent::Texture { color, .. } => *color,
        _ => unreachable!(),
    };
    (i, [rect.left, rect.bottom, rect.right, rect.top], color)
}

/// `MainMenuBar.xml:344`: a 16x64 frame at the bar's BOTTOMRIGHT (-227, -10) with a 20x66 texture
/// off its TOPRIGHT, in the recess between the last micro button and the bags.
#[test]
fn the_meter_sits_in_the_bar_recess_the_reference_leaves_for_it() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    s.resolve();

    let (left, bottom, w, h) = s
        .eval::<(f64, f64, f64, f64)>(
            "return MainMenuBarPerformanceBarFrame:GetLeft(), MainMenuBarPerformanceBarFrame:GetBottom(), \
             MainMenuBarPerformanceBarFrame:GetWidth(), MainMenuBarPerformanceBarFrame:GetHeight()",
        )
        .unwrap();
    assert_eq!((w, h), (16.0, 64.0), "frame size");
    assert_eq!(left, 781.0, "1024 − 227 − 16");
    assert_eq!(bottom, -10.0, "hangs 10 below the bar's bottom");

    // 20x66 off the frame's TOPRIGHT (797, 54): 4 past its left edge, 2 below its bottom.
    let (_, rect, _) = bar_quad(&mut s);
    assert_eq!(
        rect,
        [777.0, -12.0, 797.0, 54.0],
        "[left, bottom, right, top]"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The meter is LOW strata (`MainMenuBar.xml:344`): it paints under the bar art, through its slot.
#[test]
fn the_meter_paints_under_the_bar_art_it_shows_through() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    let (meter, _, _) = bar_quad(&mut s);

    let quads = s.extract();
    let art = quads
        .iter()
        .position(|q| {
            matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                if p.contains("UI-MainMenuBar-Dwarf"))
        })
        .expect("the bar's own dwarf art");
    assert!(
        meter < art,
        "the meter must draw before (under) the bar art — it is seen through the art's \
         transparent slot, not over it"
    );

    // The button is HIGH (`MainMenuBar.xml:395`), as the LOW frame under the art gets no mouse.
    assert_eq!(
        s.eval::<String>("return MainMenuBarPerformanceBarFrameButton:GetFrameStrata()")
            .unwrap(),
        "HIGH"
    );
    assert_eq!(
        s.eval::<String>("return MainMenuBarPerformanceBarFrame:GetFrameStrata()")
            .unwrap(),
        "LOW",
        "the parent's LOW must not have been dragged up by its HIGH child"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `MainMenuBar.xml:375`: every 10 s the poll tints the bar green to 300 ms, yellow to 600, red
/// beyond; the tint is the whole readout, the bar never changes height.
#[test]
fn the_meter_tints_by_latency_on_the_reference_thresholds() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);

    // updateInterval starts at 0 (`MainMenuBar.xml:373`), so the first tick polls: 0 ms, green.
    s.tick(0.016);
    let (_, _, color) = bar_quad(&mut s);
    assert_eq!(color, Some([0.0, 1.0, 0.0, 1.0]), "0 ms ⇒ green");

    for (latency, want, band) in [
        (299, [0.0, 1.0, 0.0, 1.0], "under LOW ⇒ green"),
        (301, [1.0, 1.0, 0.0, 1.0], "past LOW ⇒ yellow"),
        (601, [1.0, 0.0, 0.0, 1.0], "past MEDIUM ⇒ red"),
    ] {
        s.set_latency_ms(Some(latency));
        poll_beat(&mut s);
        let (_, _, color) = bar_quad(&mut s);
        assert_eq!(color, Some(want), "{latency} ms: {band}");
    }

    s.set_latency_ms(Some(0));
    s.tick(1.0);
    let (_, _, color) = bar_quad(&mut s);
    assert_eq!(
        color,
        Some([1.0, 0.0, 0.0, 1.0]),
        "still red until the beat"
    );
    poll_beat(&mut s);
    let (_, _, color) = bar_quad(&mut s);
    assert_eq!(
        color,
        Some([0.0, 1.0, 0.0, 1.0]),
        "the beat repaints it green"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The drawn colour of the tooltip's `NEWBIE_TOOLTIP_LATENCY` line, found by its text.
fn newbie_line_color(s: &mut UiScript) -> Option<[f32; 4]> {
    let want = s
        .eval::<String>("return NEWBIE_TOOLTIP_LATENCY")
        .expect("the string is declared with the meter");
    s.resolve();
    s.extract().iter().find_map(|q| match &q.content {
        QuadContent::Text {
            text: Some(t),
            color,
            ..
        } if *t == want => Some(*color),
        _ => None,
    })?
}

/// One poll beat: OnUpdate polls only on a tick that finds the interval spent, so one tick drains
/// it and the next polls (`MainMenuBar.xml:376`).
fn poll_beat(s: &mut UiScript) {
    s.tick(11.0);
    s.tick(11.0);
}

/// The app pushes the averaged RTT; both bandwidth returns stay 0, as benilla counts no throughput.
#[test]
fn get_net_stats_reports_the_pushed_latency() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[]);
    assert_eq!(
        s.eval::<(f64, f64, f64)>("return GetNetStats()").unwrap(),
        (0.0, 0.0, 0.0),
        "nothing measured yet"
    );
    s.set_latency_ms(Some(42));
    assert_eq!(
        s.eval::<(f64, f64, f64)>("return GetNetStats()").unwrap(),
        (0.0, 0.0, 42.0)
    );
    // A disconnect clears the samples; no sample reads 0, as in the reference.
    s.set_latency_ms(None);
    assert_eq!(
        s.eval::<f64>("local _, _, ms = GetNetStats() return ms")
            .unwrap(),
        0.0
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The hover tooltip is `GameTooltip_AddNewbieTip` (`MainMenuBar.xml:400`); each poll rewrites it.
#[test]
fn hovering_the_meter_shows_the_live_latency() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness(&[r"Interface\FrameXML\UIParent.xml"]);
    s.set_latency_ms(Some(42));
    // The 1.12 default (`UIOptionsFrame.lua:100`); the options file is not loaded here.
    s.run("SHOW_NEWBIE_TIPS = \"1\"").unwrap();
    s.resolve();

    assert_eq!(
        s.hit_test_name(789.0, 20.0).as_deref(),
        Some("MainMenuBarPerformanceBarFrameButton"),
        "the button over the meter takes the mouse"
    );

    s.run("this = MainMenuBarPerformanceBarFrameButton MainMenuBarPerformanceBarFrameButton:GetScript(\"OnEnter\")()")
        .unwrap();
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Latency: 42ms"
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        2,
        "the number, then the explanation — the ref's newbie branch"
    );
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft2:GetText()")
            .unwrap(),
        s.eval::<String>("return NEWBIE_TOOLTIP_LATENCY").unwrap(),
        "line 2 is the ref's own NEWBIE_TOOLTIP_LATENCY, verbatim"
    );
    // The newbie branch anchors the plate at the default corner (`GameTooltip.lua:109`).
    assert_eq!(
        s.eval::<i64>("return GameTooltip.default").unwrap(),
        1,
        "the default-corner anchor, not ANCHOR_RIGHT off the button"
    );
    // The explanation line is `NORMAL_FONT_COLOR` (`GameTooltip.lua:112`).
    assert_eq!(
        newbie_line_color(&mut s),
        Some([1.0, 0.82, 0.0, 1.0]),
        "NORMAL_FONT_COLOR (Fonts.xml l.37)"
    );

    // Still hovering: the next beat rewrites the plate (`MainMenuBar.xml:388`).
    s.set_latency_ms(Some(7));
    poll_beat(&mut s);
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Latency: 7ms"
    );
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        2,
        "the refresh rebuilds BOTH lines — a held-open plate never decays to one"
    );

    s.run("this = MainMenuBarPerformanceBarFrameButton MainMenuBarPerformanceBarFrameButton:GetScript(\"OnLeave\")()").unwrap();
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "leaving hides the plate"
    );
    s.set_latency_ms(Some(999));
    poll_beat(&mut s);
    assert!(
        !s.eval::<bool>("return GameTooltip:IsVisible()").unwrap(),
        "an unhovered meter never re-opens the tooltip"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn the_meter_adds_its_two_frames_to_the_bar() {
    benilla_formats::wow_data_or_skip!();
    let s = harness(&[]);
    for name in [
        "MainMenuBarPerformanceBarFrame",
        "MainMenuBarPerformanceBarFrameButton",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} must exist"
        );
    }
    assert_eq!(
        s.eval::<String>("return MainMenuBarPerformanceBarFrameButton:GetParent():GetName()")
            .unwrap(),
        "MainMenuBarPerformanceBarFrame"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}
