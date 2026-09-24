//! `GameTooltipTemplate` as an ADDON sees it — the stock `Interface\FrameXML\GameTooltip.xml`
//! driven the way the corpus drives it.
//!
//! **This is not a test of our tooltip window** (that is `tooltip_anchor_tests` /
//! `tooltip_compare_tests`, and they are the regression suite this refactor had to leave
//! untouched). It is a test of the *template*, which is a different surface with a different
//! consumer: `GameTooltipTemplate` is the single most-wanted missing template in the addon corpus
//! — 27 addons name it in an XML `inherits=` and two more reach it through `CreateFrame`
//! (`addon_harness::inherits_demand` / `template_demand` over the 218-addon corpus).
//!
//! Every shape below is a real addon's, quoted, not one invented to be convenient:
//!
//! - **The declaration** is `<GameTooltip name="…" inherits="GameTooltipTemplate">` — all 27 use
//!   the `<GameTooltip>` element (not `<Frame>`), none use a comma-separated `inherits` list, and
//!   the two `CreateFrame` sites both pass `kind = "GameTooltip"` with `parent = nil`
//!   (BetterCharacterStats/helper.lua l.3, BigWigs/Raids/Naxxramas/Loatheb.lua l.75).
//! - **The `$parent` names are the point.** 120 corpus lines read
//!   `getglobal(tip:GetName().."TextLeft"..i)` or the hardcoded equivalent; a template whose
//!   children came out named after the *template* would be useless, and a tooltip with no plate is
//!   invisible. Those are the two failures these tests exist to catch.
//! - **The bare `GameTooltip_OnLoad()`** an addon writes in its own `<OnLoad>` (TipBuddy.xml
//!   l.1402, atsw.xml l.3417) is the reference's zero-argument spelling; our corpus convention
//!   takes the frame as a leading parameter, and nothing lets us edit an addon.
//! - **`$parentStatusBar` is load-bearing for exactly one addon and completely for it**: TipBuddy
//!   both `parent=`s and `relativeTo=`s a `TipBuddyTooltipStatusBar` it never declares
//!   (TipBuddy.xml l.2320, l.2591) — a name only the template can publish.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// An **addon's** document text, loaded after the shipped files exactly as `LoadAddOn` would —
/// the template registry is persistent, so `inherits=` reaches what `GameTooltip.xml` registered.
/// Returns the loader's own report so a test can assert on warnings as well as errors.
fn load_addon_xml(s: &UiScript, text: &str) -> benilla_ui::loader::LoadReport {
    let doc = benilla_ui::framexml::parse(text).unwrap();
    benilla_ui::loader::load(s, &doc, &|_| None)
}

/// Fonts + UIParent + the tooltip file, which is where both templates live.
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

/// Answer every pending FontString measure the way the app's font atlas does — 6 px/char, 14 px a
/// line. A tooltip that declares no `<Size>` (which is 43 of the corpus's 44 `<GameTooltip>`
/// elements, and the reference template too) has NO rect at all until its text is measured, so
/// without this the auto-size never runs and nothing resolves, let alone draws.
fn answer_measures(s: &mut UiScript) {
    let answers: Vec<(u32, f32, f32, u64)> = s
        .fontstrings_needing_measure()
        .into_iter()
        .map(|r| (r.id, r.text.chars().count() as f32 * 6.0, 14.0, r.key))
        .collect();
    s.set_measured_text_unwrapped(&answers);
}

/// Every texture path the resolved frame tree actually draws — `<Texture>` regions AND the
/// `<Backdrop>` plate pieces, which are a quad kind of their own (`QuadContent::Backdrop`, emitted
/// from the frame's own draw slot rather than from a region). The plate is the whole point here,
/// so a filter that saw only `Texture` would report an empty list for a perfectly drawn tooltip.
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

/// **The test this file exists for.** `MyTipTextLeft1` must exist; `GameTooltipTemplateTextLeft1`
/// must not.
///
/// The failure it pins is not "no tooltip": it is a tooltip whose lines are called
/// `GameTooltipTemplateTextLeft1`, so `getglobal(tip:GetName().."TextLeft1")` is nil, so the
/// addon's very next line dies — with the declaration itself having succeeded. 120 corpus lines
/// write that idiom.
///
/// Our line pairs are engine-created (`script/tooltip.rs` `ensure_lines`) rather than declared in
/// the template the way the reference's are, so they are named from the frame's own name by
/// construction — this asserts that the template route does not disturb it, and that the template
/// itself never publishes a line region.
#[test]
fn an_addon_tooltip_from_the_template_names_its_lines_after_the_caller() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // AtlasLoot/Core/AtlasLoot.xml l.576's declaration, renamed.
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

    // AtlasLoot.lua l.3220-3236's own sequence: own the frame, clear, add lines, show.
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
    // The idiom, verbatim — MikScrollingBattleText.lua l.1824's spelling.
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

/// The **plate** is what an addon actually gets from the template, and losing it is silent: the
/// declaration succeeds, the lines populate, and the tooltip renders as floating text over the
/// world with no backdrop behind it.
///
/// Strip `<Backdrop>` out of `GameTooltipTemplate` and this is the assertion that goes red.
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
    // The template's children resolve against the CALLER's name — the status bar the reference's
    // template declares (`$parentStatusBar`), which TipBuddy anchors to by that global.
    assert!(
        s.eval::<bool>(r#"return getglobal("MyTipStatusBar") ~= nil"#)
            .unwrap(),
        "$parentStatusBar resolves against the CALLER's name"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// TipBuddy's shape: an addon re-declares `<OnLoad>` to add its own line and opens it with the
/// **reference's** zero-argument `GameTooltip_OnLoad();` (TipBuddy.xml l.1402, atsw.xml l.3417).
///
/// Our helpers take the frame as a leading parameter (the corpus-wide convention shift noted at
/// the head of `GameTooltip.xml`), so a bare call arrives with `self = nil` — a load-time error,
/// and the plate it was about to tint never gets its colours. Nothing lets us edit an addon, so
/// the helper reads `this` when the argument is absent, which is what the reference reads anyway.
///
/// Asserted from both sides: no error, and the tint actually landed.
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

    // The tint the bare call was there to apply (ref-GameTooltip.lua l.79-82's two colours),
    // QUANTIZED: the reference's backdrop colour field is a packed `0xAARRGGBB` byte quad and the
    // setter converts `×255 + 0.5` through `__ftol` (`SetBackdropColor 0x777d30`), so
    // `0.09` stores as 23 and reads back as `23/255`. This used to compare against `0.09` exactly,
    // which was our lossless `[f32; 4]` showing through a store the client cannot make.
    let q = |x: f32| f32::from((x * 255.0 + 0.5) as u8) / 255.0;
    let (r, g, b) = s
        .eval::<(f32, f32, f32)>("return MyTip:GetBackdropColor()")
        .unwrap();
    assert_eq!(
        (r, g, b),
        (q(0.09), q(0.09), q(0.19)),
        "GameTooltip_OnLoad tinted the plate — TOOLTIP_DEFAULT_BACKGROUND_COLOR"
    );

    // TipBuddy also routes its OnHide through the same helper, bare (TipBuddy.xml l.1412).
    s.run("MyTip:Show() MyTip:Hide()").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The Lua route, which two addons take: `CreateFrame("GameTooltip", name, nil,
/// "GameTooltipTemplate")` — BetterCharacterStats/helper.lua l.3 and
/// BigWigs/Raids/Naxxramas/Loatheb.lua l.75, both with `kind = "GameTooltip"` and `parent = nil`.
///
/// The same template through the other door, so the same two things must hold: the caller's name
/// owns the children, and the plate came along.
#[test]
fn createframe_with_the_template_is_the_same_tooltip() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    // BetterCharacterStats/helper.lua l.3, verbatim in shape (its `getglobal(...) or` guard and
    // the WorldFrame owner both collapse to this in a fresh VM).
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

/// The **scanner** shape — the majority use: an off-screen tooltip nobody ever looks at, filled
/// only so its FontStrings can be read back and parsed.
///
/// CT_BarMod.lua l.1659-1669 blanks `TextLeft1`, fills the tooltip, and reads the FontString back
/// as its "did it populate?" sentinel; l.1747-1770 then walks `1..NumLines()` over both columns.
/// MikScrollingBattleText.lua l.1821-1858 is the same with `SetOwner(UIParent, "ANCHOR_NONE")` and
/// an explicit `Hide()` between passes (l.1832). Both read `TextRight` as well as `TextLeft`.
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

    // CT_BarMod.lua l.1663-1665's read, verbatim in shape.
    assert_eq!(
        s.eval::<String>("return CTTooltipTextLeft1:GetText()")
            .unwrap(),
        "Shadow Bolt",
        "the bare-global spelling resolves too (CT_BarMod, TipBuddy and Outfitter all use it)"
    );
    // CT_BarMod.lua l.1752-1754's loop body, over the right column.
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

    // MikScrollingBattleText.lua l.1832's `tooltip:Hide()` — the scanner's way out, and what keeps
    // it off screen. (`SetText` SHOWS the plate by design in our engine — the byte-pinned
    // `0x531b90` note in tooltip/verbs.rs, and PaperDoll's empty-slot text depends on it — so a
    // scanner is off screen because it hides again and because its owner anchor is ANCHOR_NONE,
    // not because filling it is invisible.)
    s.run("CTTooltip:Hide()").unwrap();
    assert!(
        !s.eval::<bool>("return CTTooltip:IsShown()").unwrap(),
        "the scanner puts the plate away between passes"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `$parentStatusBar` belongs to the TEMPLATE, and TipBuddy is why.
///
/// TipBuddy replaces the whole unit tooltip and never declares a status bar of its own; it
/// `parent=`s a frame to `TipBuddyTooltipStatusBar` (TipBuddy.xml l.2320) and `relativeTo=`s an
/// anchor at it (l.2591), then drives it from Lua (`:SetValue`, `:Show`, `:Hide`, `:IsVisible` —
/// TipBuddy.lua l.177, l.1462-1464). Every one of those is a name only the template can publish.
///
/// This is the assertion that goes red if the status bar is moved back onto the `GameTooltip`
/// instance, which is where it lived while it was inlined.
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
    // TipBuddy.lua l.1462-1464 / l.177 — it shows, hides and drives the bar directly.
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

/// The instance attributes the corpus actually overrides on top of the template: `frameStrata`
/// (13 sites — `TOOLTIP`, and `LOW` at Necrosis.xml l.1332) and `hidden="false"` (PowerAuras.xml
/// l.20, SimpleActionSets.xml l.349, against the template's `hidden="true"`).
///
/// The splice puts the template's attributes first and the instance's on top, so both of these are
/// the instance's to win — and `hidden="false"` in particular must not be read as "hidden", since
/// PowerAuras never calls `Show()`.
///
/// **It is also the falsifier for `GameTooltip_OnLoad`'s body.** Our copy used to end with a
/// `self:Hide()` the reference's does not have (ref-GameTooltip.lua l.79-82 is the whole function);
/// it was invisible on our four tooltips, every one of which carries `hidden="true"` anyway, and it
/// hid this one. Put the line back and this assertion goes red — which is the only place anything
/// would notice.
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

/// **`GameTooltipTextLeft1` is a real global from LOAD, hidden — before any line exists.**
///
/// Ours were engine-created purely on demand, so a cold VM had no such global. The corpus pattern
/// that breaks on is a **guard**, not a read: `Participant/Resurrection.lua:294` and
/// `CT_RaidAssist/CT_RADetectSpells.lua:47` both write
/// `... and GameTooltipTextLeft1:IsVisible() then` to ask whether the tooltip is currently showing
/// anything. On the reference that is safe and answers false; for us it raised on the check itself,
/// which is the worst shape — the addon was being careful and got punished for it.
///
/// `ref-GameTooltipTemplate.xml` declares 30 pairs, all `hidden="true"` (l.17-627). We now declare
/// the same 30, and the engine's existing adoption path takes them over when lines are added.
///
/// The second half of the assertion is what makes this safe: a declared-but-unfilled pair must not
/// count as a line. `NumLines()` reads `num_lines`, and `layout_tooltips` sizes from `num_lines`,
/// so 30 hidden pairs add no height and no width.
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

    // The corpus guard itself, verbatim in shape, on a tooltip that has never shown anything.
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

    // And the pairs still work as lines once filled — the adoption path, not a parallel set.
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
