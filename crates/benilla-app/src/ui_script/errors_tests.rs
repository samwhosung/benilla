//! The stock errors frame (`UIErrorsFrame.xml`), a `<MessageFrame>`, read from its drawn quads:
//! the widget draws its own message lines, so there is no Lua state to read.

use benilla_ui::script::{ExtractedQuad, QuadContent, ScriptValue, UiScript};

use super::test_ui::load_ui as load_xml;

/// The drawn lines, top row first, each with its colour and alpha and its band's bottom.
fn toast_lines(s: &mut UiScript) -> Vec<(String, [f32; 4], f32)> {
    s.resolve();
    let mut v: Vec<(String, [f32; 4], f32)> = s
        .extract()
        .iter()
        .filter_map(|q| match (&q.content, q.rect) {
            (
                QuadContent::Text {
                    text: Some(t),
                    color: Some(c),
                    ..
                },
                Some(r),
            ) if !t.is_empty() => Some((t.clone(), *c, r.bottom)),
            _ => None,
        })
        .collect();
    v.sort_by(|a, b| b.2.total_cmp(&a.2));
    v
}

#[test]
fn info_and_error_messages_stack_hold_and_expire() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "Interface\\FrameXML\\UIErrorsFrame.xml");

    assert!(toast_lines(&mut s).is_empty());

    // A quest progress toast, yellow (`UIErrorsFrame.lua:12`).
    s.fire_event(
        "UI_INFO_MESSAGE",
        vec![ScriptValue::Str("Tough Wolf Meat: 2/8".into())],
    );
    assert!(s.errors().is_empty(), "info errors: {:?}", s.errors());
    let lines = toast_lines(&mut s);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].0, "Tough Wolf Meat: 2/8");
    assert_eq!(lines[0].1, [1.0, 1.0, 0.0, 1.0], "UI_INFO_MESSAGE yellow");

    // insertMode="TOP": the red error (`UIErrorsFrame.lua:14`) lands above the toast.
    s.fire_event(
        "UI_ERROR_MESSAGE",
        vec![ScriptValue::Str("You are too far away!".into())],
    );
    let lines = toast_lines(&mut s);
    assert_eq!(
        lines.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(),
        ["You are too far away!", "Tough Wolf Meat: 2/8"]
    );
    // 0.1 draws as 26/255: `AddMessage` rounds each channel to a byte (`ftol(v*255 + 0.5)`).
    assert_eq!(
        lines[0].1,
        [1.0, 26.0 / 255.0, 26.0 / 255.0, 1.0],
        "UI_ERROR_MESSAGE red, byte-quantized"
    );

    // Hold 5 s (displayDuration="5"), then the default 3 s fade; the tick that ends the hold
    // still draws full.
    s.tick(4.0);
    assert_eq!(toast_lines(&mut s).len(), 2);
    s.tick(2.0); // the hold ends on this tick; the fade starts on the next
    assert_eq!(toast_lines(&mut s).len(), 2);
    s.tick(1.5); // half the fade: dimmer, still drawing
    let mid = toast_lines(&mut s);
    assert_eq!(mid.len(), 2);
    assert!(
        mid[0].1[3] < 1.0 && mid[0].1[3] > 0.0,
        "mid-ramp alpha: {:?}",
        mid[0].1
    );
    s.tick(2.0); // fade done: the line is freed, not blanked
    assert!(toast_lines(&mut s).is_empty());
    assert!(s.errors().is_empty(), "expiry errors: {:?}", s.errors());
}

/// `UIErrorsFrame` overlaps the left-slot panels and outranks them by stratum, HIGH
/// (`UIErrorsFrame.xml:4`); within one stratum a panel's child frames would win on level.
#[test]
fn an_error_toast_draws_over_an_open_panel_window() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.lua");
    load_xml(&s, r"Interface\FrameXML\MoneyFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\GlobalStrings.lua");
    load_xml(&s, r"Interface\FrameXML\UIParent.xml");
    load_xml(&s, "Interface\\FrameXML\\GameTooltip.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\BasicControls.xml");
    load_xml(&s, r"Interface\FrameXML\LocaleProperties.lua");
    load_xml(&s, r"Interface\FrameXML\StaticPopup.xml");
    load_xml(&s, "Interface\\FrameXML\\UIErrorsFrame.xml");
    load_xml(&s, "Interface\\FrameXML\\Cooldown.xml");
    load_xml(&s, "ScrollTemplates.xml"); // ours: the scroll kits
    load_xml(&s, "Interface\\FrameXML\\CharacterFrameTemplates.xml");
    load_xml(&s, "Interface\\FrameXML\\MerchantFrame.xml");
    // A missing template only warns at load, so an under-loaded list fails at its first update.
    load_xml(&s, "Interface\\FrameXML\\BasicControls.xml");
    load_xml(&s, "Interface\\FrameXML\\ItemButtonTemplate.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestFrame.xml");
    load_xml(&s, r"Interface\FrameXML\MainMenuBarMicroButtons.xml");
    load_xml(&s, "Interface\\FrameXML\\QuestLogFrame.xml");

    // A left-slot panel opens, then the toast: stratum, not order, must decide.
    s.eval::<()>("ShowUIPanel(QuestLogFrame)").unwrap();
    s.fire_event(
        "UI_ERROR_MESSAGE",
        vec![ScriptValue::Str("Out of range.".into())],
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert!(s.eval::<bool>("return QuestLogFrame:IsVisible()").unwrap());

    s.resolve();
    let quads = s.extract();
    const STRATUM_SHIFT: u32 = 60;
    let toast = quads
        .iter()
        .filter_map(|q| match &q.content {
            QuadContent::Text { text: Some(t), .. } if t == "Out of range." => Some(q.z),
            _ => None,
        })
        .min()
        .expect("the toast text must draw");
    let panel_ceiling = quads.iter().map(|q| q.z).filter(|z| *z < toast).count();
    assert!(
        panel_ceiling > 0,
        "sanity: the panel must be drawing something at all"
    );
    // Only other frames count: the frame's own declared `<FontString>` sorts above its message
    // lines but carries no text.
    let above: Vec<&ExtractedQuad> = quads
        .iter()
        .filter(|q| q.z > toast)
        // `RaidWarningFrame` is HIGH and toplevel too (`RaidWarning.xml:4`) but sits below this
        // frame on screen, and its `<FontString>` carries no text.
        .filter(|q| {
            !matches!(
                s.quad_owner_name(q.target).as_deref(),
                Some("UIErrorsFrame") | Some("RaidWarningFrame")
            )
        })
        .collect();
    assert!(
        above.is_empty(),
        "nothing may draw over the error toast while a panel is open: {above:?}"
    );
    assert_eq!(
        toast >> STRATUM_SHIFT,
        4,
        "HIGH is stratum 4 — the ref's own for UIErrorsFrame"
    );
}
