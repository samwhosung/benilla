//! The stock `GameTooltipTemplate` as addons use it: declared with `inherits=` or through
//! `CreateFrame`, its lines and status bar named after the caller, its plate drawn. Each shape
//! below is a corpus addon's, cited per test.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// An addon's XML text, loaded after the stock files as `LoadAddOn` would: the template registry
/// persists, so `inherits=` reaches `GameTooltipTemplate`.
fn load_addon_xml(s: &UiScript, text: &str) -> benilla_ui::loader::LoadReport {
    let doc = benilla_ui::framexml::parse(text).unwrap();
    benilla_ui::loader::load(s, &doc, &|_| None)
}

/// Fonts, `UIParent` and the tooltip file, which includes both tooltip templates.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for f in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
    ] {
        load_xml(&s, f);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// Answers every pending FontString measure at 6 px a character, 14 px a line. A tooltip with no
/// `<Size>`, the stock template included, has no rect until its text is measured.
fn answer_measures(s: &mut UiScript) {
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .into_iter()
        .map(|r| (r.id, r.text.chars().count() as f32 * 6.0, 14.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
}

/// Every texture path drawn: `<Texture>` regions and the `<Backdrop>` plate, which is its own
/// quad kind (`QuadContent::Backdrop`).
fn drawn_textures(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    answer_measures(s);
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            benilla_ui::script::QuadContent::Texture { path: Some(p), .. } => Some(p),
            benilla_ui::script::QuadContent::Backdrop { path, .. } => Some(path),
            _ => None,
        })
        .collect()
}

/// An addon tooltip's lines are named after the caller (`MyTipTextLeft1`), never the template, so
/// the corpus idiom `getglobal(tip:GetName().."TextLeft1")` resolves.
#[test]
fn an_addon_tooltip_from_the_template_names_its_lines_after_the_caller() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // AtlasLoot's declaration (`AtlasLoot.xml:576`), renamed.
    let report = load_addon_xml(
        &s,
        r#"<Ui><GameTooltip name="MyTip" inherits="GameTooltipTemplate" parent="UIParent" hidden="true"/></Ui>"#,
    );
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("unknown template")),
        "the template resolves: {:?}",
        report.warnings
    );

    // `AtlasLoot.lua:3220-3236`'s sequence: own the frame, clear, add lines, show.
    s.run(
        r#"MyTip:SetOwner(UIParent, "ANCHOR_RIGHT")
           MyTip:ClearLines()
           MyTip:SetText("Tough Jerky")
           MyTip:AddLine("Drop Rate: 12%", 1, 1, 0)
           MyTip:Show()"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        s.eval::<i64>("return MyTip:NumLines()").unwrap(),
        2,
        "SetText is line 1, AddLine is line 2"
    );
    // The idiom as `MikScrollingBattleText.lua:1824` spells it.
    assert_eq!(
        s.eval::<String>(r#"return getglobal(MyTip:GetName().."TextLeft1"):GetText()"#)
            .unwrap(),
        "Tough Jerky"
    );
    assert_eq!(
        s.eval::<String>(r#"return getglobal("MyTipTextLeft2"):GetText()"#)
            .unwrap(),
        "Drop Rate: 12%"
    );
    // And never under the template's name.
    assert!(
        s.eval::<bool>(r#"return getglobal("GameTooltipTemplateTextLeft1") == nil"#)
            .unwrap(),
        "a line region named after the TEMPLATE is the failure this test exists for"
    );
    // A `virtual="true"` element is registered, never instantiated: no frame by that name either.
    assert!(
        s.eval::<bool>(r#"return getglobal("GameTooltipTemplate") == nil"#)
            .unwrap(),
        "the template is not a frame"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An addon tooltip draws the template's backdrop plate and border (`GameTooltipTemplate.xml:4`).
#[test]
fn an_addon_tooltip_from_the_template_gets_the_plate() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    load_addon_xml(
        &s,
        r#"<Ui><GameTooltip name="MyTip" inherits="GameTooltipTemplate" parent="UIParent" hidden="true"/></Ui>"#,
    );
    s.run(
        r#"MyTip:SetOwner(UIParent, "ANCHOR_RIGHT")
           MyTip:SetText("Tough Jerky")
           MyTip:Show()"#,
    )
    .unwrap();

    let drawn = drawn_textures(&mut s);
    assert!(
        drawn
            .iter()
            .any(|p| p.eq_ignore_ascii_case(r"Interface\Tooltips\UI-Tooltip-Background")),
        "the addon tooltip draws the template's plate: {drawn:?}"
    );
    assert!(
        drawn
            .iter()
            .any(|p| p.eq_ignore_ascii_case(r"Interface\Tooltips\UI-Tooltip-Border")),
        "…and its border: {drawn:?}"
    );
    // The template's `$parentStatusBar` publishes under the caller's name.
    assert!(
        s.eval::<bool>(r#"return getglobal("MyTipStatusBar") ~= nil"#)
            .unwrap(),
        "$parentStatusBar resolves against the CALLER's name"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An addon's own `<OnLoad>` calls `GameTooltip_OnLoad()` with no argument (`TipBuddy.xml:1402`);
/// the stock helper reads `this` (`GameTooltip.lua:79-82`) and tints the plate.
#[test]
fn an_addon_may_call_gametooltip_onload_the_reference_way_with_no_argument() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    let report = load_addon_xml(
        &s,
        r#"<Ui>
             <GameTooltip name="MyTip" inherits="GameTooltipTemplate" frameStrata="TOOLTIP" hidden="true" parent="UIParent">
               <Scripts>
                 <OnLoad>
                   GameTooltip_OnLoad()
                   this:SetOwner(UIParent, "ANCHOR_NONE")
                 </OnLoad>
                 <OnHide>
                   GameTooltip_OnHide()
                 </OnHide>
               </Scripts>
             </GameTooltip>
           </Ui>"#,
    );
    assert!(
        report.errors.is_empty(),
        "a bare GameTooltip_OnLoad() must not error: {:?}",
        report.errors
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // The tint, quantized: `SetBackdropColor` (`0x777d30`) stores a byte per channel, truncating
    // `x * 255 + 0.5`, so 0.09 reads back as 23/255.
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return MyTip:GetBackdropColor()")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (q(0.09), q(0.09), q(0.19)),
        "GameTooltip_OnLoad tinted the plate — TOOLTIP_DEFAULT_BACKGROUND_COLOR"
    );

    // TipBuddy routes its OnHide through the stock helper too, bare (`TipBuddy.xml:1412`).
    s.run("MyTip:Show() MyTip:Hide()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `CreateFrame("GameTooltip", name, nil, "GameTooltipTemplate")`
/// (`BetterCharacterStats/helper.lua:3`) names the template's children after the caller too.
#[test]
fn createframe_with_the_template_is_the_same_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // `helper.lua:3` in shape.
    s.run(
        r#"BCS_Tooltip = getglobal("BetterCharacterStatsTooltip")
                          or CreateFrame("GameTooltip", "BetterCharacterStatsTooltip", nil, "GameTooltipTemplate")
           BCS_Tooltip:SetOwner(UIParent, "ANCHOR_NONE")"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // Its next lines: fill, then read the lines back off a stored name prefix (`BCS_Prefix`).
    s.run(
        r#"BCS_Prefix = "BetterCharacterStatsTooltip"
           BCS_Tooltip:SetText("Equip: Improves your chance to hit by 1%.")"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>(r#"return getglobal(BCS_Prefix .. "TextLeft" .. 1):GetText()"#)
            .unwrap(),
        "Equip: Improves your chance to hit by 1%."
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("BetterCharacterStatsTooltipStatusBar") ~= nil"#)
            .unwrap(),
        "the template's children came through CreateFrame's fourth argument too"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The scanner: an off-screen tooltip filled only to read both columns back
/// (`CT_BarMod.lua:1659-1669`, `MikScrollingBattleText.lua:1821-1858`), hidden between passes.
#[test]
fn the_scanner_shape_reads_both_columns_and_hides_again() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    load_addon_xml(
        &s,
        r#"<Ui><GameTooltip name="CTTooltip" inherits="GameTooltipTemplate"/></Ui>"#,
    );
    s.run(r#"CTTooltip:SetOwner(UIParent, "ANCHOR_NONE")"#)
        .unwrap();

    // The sentinel clear, then the fill (a double line is the `%d yd range` row CT_BarMod hunts).
    s.run(
        r#"CTTooltip:ClearLines()
           CTTooltip:SetText("Shadow Bolt")
           CTTooltip:AddDoubleLine("Rank 4", "30 yd range")"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    // `CT_BarMod.lua:1663-1665`'s read, in shape.
    assert_eq!(
        s.eval::<String>("return CTTooltipTextLeft1:GetText()")
            .unwrap(),
        "Shadow Bolt",
        "the bare-global spelling resolves too (CT_BarMod, TipBuddy and Outfitter all use it)"
    );
    // `CT_BarMod.lua:1752-1754`'s loop body, over the right column.
    let range: String = s
        .eval(
            r#"for y = 1, CTTooltip:NumLines() do
                 local t = getglobal("CTTooltipTextRight" .. y):GetText()
                 if t and t ~= "" then return t end
               end
               return "<none>""#,
        )
        .unwrap();
    assert_eq!(
        range, "30 yd range",
        "the right column is reachable by name"
    );

    // `SetText` (`0x531b90`) shows the plate, as stock `PaperDollFrame.lua:747` relies on, so a
    // scanner stays off screen by hiding again (`MikScrollingBattleText.lua:1832`) and by its
    // `ANCHOR_NONE` owner.
    s.run("CTTooltip:Hide()").unwrap();
    assert!(
        !s.eval::<bool>("return CTTooltip:IsShown()").unwrap(),
        "the scanner puts the plate away between passes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `$parentStatusBar` is the template's (`GameTooltipTemplate.xml:575`): TipBuddy parents and
/// anchors frames to a `TipBuddyTooltipStatusBar` it never declares (`TipBuddy.xml:2320`, `:2591`)
/// and drives it from Lua.
#[test]
fn the_status_bar_is_the_templates_because_tipbuddy_anchors_to_it() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    let report = load_addon_xml(
        &s,
        r#"<Ui>
             <GameTooltip name="TipBuddyTooltip" frameStrata="TOOLTIP" hidden="true" parent="UIParent" inherits="GameTooltipTemplate"/>
             <Frame name="TipBuddy_HealthTextGTT" frameStrata="TOOLTIP" parent="TipBuddyTooltipStatusBar">
               <Anchors>
                 <Anchor point="TOPLEFT" relativeTo="TipBuddyTooltipStatusBar" relativePoint="BOTTOMLEFT"/>
               </Anchors>
             </Frame>
           </Ui>"#,
    );
    assert!(
        report.errors.is_empty(),
        "TipBuddy's anchor at the template's status bar must resolve: {:?}",
        report.errors
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("TipBuddyTooltipStatusBar") ~= nil"#)
            .unwrap(),
        "$parentStatusBar publishes under the CALLER's name"
    );
    // `TipBuddy.lua:1462-1464` and `:177` show, hide and drive the bar directly.
    s.run(
        r#"TipBuddyTooltipStatusBar:SetMinMaxValues(0, 100)
           TipBuddyTooltipStatusBar:SetValue(42)
           TipBuddyTooltipStatusBar:Show()"#,
    )
    .unwrap();
    assert!(s
        .eval::<bool>("return TipBuddyTooltipStatusBar:IsShown()")
        .unwrap());
    assert_eq!(
        s.eval::<f64>("return TipBuddyTooltipStatusBar:GetValue()")
            .unwrap(),
        42.0
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// An instance's `frameStrata` (`Necrosis.xml:1332`) and `hidden="false"` (`PowerAuras.xml:20`)
/// win over the template's; PowerAuras never calls `Show()`, and `GameTooltip_OnLoad` does not
/// hide the frame (`GameTooltip.lua:79-82`).
#[test]
fn an_instance_attribute_beats_the_templates() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    load_addon_xml(
        &s,
        r#"<Ui>
             <GameTooltip name="NecrosisTooltip" frameStrata="LOW" parent="UIParent" inherits="GameTooltipTemplate"/>
             <GameTooltip name="Powa_Tooltip" frameStrata="TOOLTIP" hidden="false" parent="UIParent" inherits="GameTooltipTemplate"/>
           </Ui>"#,
    );
    assert_eq!(
        s.eval::<String>("return NecrosisTooltip:GetFrameStrata()")
            .unwrap(),
        "LOW",
        "the instance's strata overrides the template's TOOLTIP"
    );
    assert!(
        s.eval::<bool>("return Powa_Tooltip:IsShown()").unwrap(),
        "hidden=\"false\" beats the template's hidden=\"true\" — PowerAuras never calls Show()"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `GameTooltipTextLeft1` exists, hidden, from load: the template declares 30 hidden line pairs
/// (`GameTooltipTemplate.xml:17-556`), so a corpus guard such as `GameTooltipTextLeft1:IsVisible()`
/// (`Participant/Resurrection.lua:294`) answers false. A declared pair counts as a line, and adds
/// size, only once filled.
#[test]
fn the_tooltip_line_globals_exist_cold_and_do_not_count_as_lines() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();

    for name in [
        "GameTooltipTextLeft1",
        "GameTooltipTextRight1",
        "GameTooltipTextLeft30",
    ] {
        assert!(
            s.eval::<bool>(&format!("return {name} ~= nil")).unwrap(),
            "{name} must exist before any line is added — it is declared, not grown"
        );
        assert!(
            !s.eval::<bool>(&format!("return {name}:IsVisible()"))
                .unwrap(),
            "{name} must be HIDDEN cold, so the corpus guard answers false instead of raising"
        );
    }

    // The corpus guard itself, in shape, on a tooltip that has never shown anything.
    assert!(
        !s.eval::<bool>("return GameTooltipTextLeft1:IsVisible()")
            .unwrap(),
        "Participant/Resurrection.lua:294's guard"
    );
    // A declared pair is not a line.
    assert_eq!(
        s.eval::<i64>("return GameTooltip:NumLines()").unwrap(),
        0,
        "30 declared pairs must not inflate NumLines"
    );

    // Filled, the declared pair is the line.
    s.run("GameTooltip:SetOwner(UIParent, \"ANCHOR_NONE\") GameTooltip:AddLine(\"Corpse of Bob\") GameTooltip:Show()")
        .unwrap();
    answer_measures(&mut s);
    assert_eq!(s.eval::<i64>("return GameTooltip:NumLines()").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return GameTooltipTextLeft1:GetText()")
            .unwrap(),
        "Corpse of Bob",
        "the DECLARED region must be the one the line stack filled, not a sibling"
    );
    assert!(
        s.eval::<bool>("return GameTooltipTextLeft1:IsVisible()")
            .unwrap(),
        "a filled line shows"
    );
}
