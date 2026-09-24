//! The stock `Interface\FrameXML\UIPanelTemplates.xml` + our `assets/ui/OptionsFrameTemplates.xml`
//! — the reference's SHARED widget kit, driven the way an addon drives it.
//!
//! **These are not tests of a window.** Nothing benilla ships instantiates a single template in
//! either file: their only consumer is a third-party addon writing
//! `CreateFrame(kind, name, parent, "SomeTemplate")`, which decision 1203 made work and which
//! `addon_harness` then ranked — 69 corpus call sites on `UICheckButtonTemplate` alone, 185 sites
//! in total naming templates we had never declared. So every test here goes in through
//! `CreateFrame`'s fourth argument, from Lua, with a caller-chosen name, because that is the entire
//! surface these files exist to serve.
//!
//! What they guard, in the order the harness ranked them:
//!
//! - **The name.** `CreateFrame("CheckButton", "MyCheck", UIParent, "UICheckButtonTemplate")` must
//!   publish `MyCheckText`, never `UICheckButtonTemplateText` — `getglobal(this:GetName().."Text")`
//!   is the next line every one of those 69 sites writes. 1203 pinned the *mechanism*; this pins it
//!   through the real templates, including the two-level `$parent` case
//!   (`MyScrollScrollBarScrollUpButton`).
//! - **The art.** Each state texture in those files carries the reference's `inherits=` *and* the
//!   art that `inherits=` resolves to, because our loader does not expand `inherits=` in
//!   state-texture position (the deviation is stated at the head of `UIPanelTemplates.xml`). The
//!   extract assertions below are that deviation's falsifier: strip the inline `file=` back to the
//!   reference's `inherits=`-only form and every checkbox and panel button in the corpus goes
//!   invisible with nothing erroring.
//! - **The behaviour.** `SetChecked`/`GetChecked` across a click, the close button hiding its
//!   parent, the edit box round-tripping text, the scroll frame's range reaching its bar.
//! - **The names are 1.12's.** The last test re-reads both files against
//!   `reference/1.12-globals.tsv` — a template we invent under a plausible-looking name is a name
//!   an addon can never have meant.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The manifest prefix these two files sit on: fonts, `UIParent` (the parent every addon passes),
/// `HideUIPanel` (the close button's OnClick), the tooltip (the options widgets' hover) and the
/// scroll kit (`ScrollFrame_OnLoad`), then the two files under test in manifest order.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    for file in [
        "Interface\\FrameXML\\Fonts.xml",
        r"Interface\FrameXML\UIParent.xml",
        r"Interface\FrameXML\MoneyFrame.lua",
        r"Interface\FrameXML\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "ScrollTemplates.xml",
        r"Interface\FrameXML\UIPanelTemplates.lua",
        r"Interface\FrameXML\UIPanelTemplates.xml",
        r"Interface\FrameXML\CharacterFrameTemplates.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\LocaleProperties.lua",
        "Interface\\FrameXML\\StaticPopup.xml",
        r"Interface\FrameXML\OptionsFrameTemplates.xml",
        // `UIOptionsCheckButtonTemplate`'s home. It was ours, in a one-template
        // `OptionsFrameTemplates.xml`, until 2115 put the reference's own hidden Interface
        // Options window on the manifest — the template's actual home, and the whole point of
        // that record: an addon that names it gets the reference's own declaration. The dropdown
        // kit comes with it because the window has four.
        r"Interface\FrameXML\UIDropDownMenu.xml",
        r"Interface\FrameXML\UIOptionsFrame.xml",
    ] {
        load_xml(&s, file);
    }
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// Every texture path the resolved frame tree actually draws.
fn drawn_textures(s: &mut UiScript) -> Vec<String> {
    s.resolve();
    s.extract()
        .into_iter()
        .filter_map(|q| match q.content {
            benilla_ui::script::QuadContent::Texture { path: Some(p), .. } => Some(p),
            _ => None,
        })
        .collect()
}

/// **The test this whole file exists for.** An addon's own line, verbatim, and then the global it
/// reads back on the next line.
///
/// The failure this pins is not "no checkbox": it is a checkbox whose label region is called
/// `UICheckButtonTemplateText`, so `getglobal(this:GetName().."Text")` is nil, so the addon's
/// `SetText` dies — with the `CreateFrame` itself having succeeded and returned an object.
#[test]
fn a_check_button_from_the_template_names_its_label_after_the_caller() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(r#"MyCheck = CreateFrame("CheckButton", "MyCheck", UIParent, "UICheckButtonTemplate")"#)
        .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert!(
        s.eval::<bool>(r#"return getglobal("MyCheckText") ~= nil"#)
            .unwrap(),
        "the label publishes under the CALLER's name — this is the line an addon writes next"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("UICheckButtonTemplateText") == nil"#)
            .unwrap(),
        "and never under the template's"
    );

    // The 69-call-site idiom, end to end.
    s.run(r#"getglobal("MyCheckText"):SetText("Show my thing")"#)
        .unwrap();
    assert_eq!(
        s.eval::<String>(r#"return getglobal("MyCheckText"):GetText()"#)
            .unwrap(),
        "Show my thing"
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return MyCheck:GetWidth(), MyCheck:GetHeight()")
            .unwrap(),
        (32.0, 32.0),
        "the template's own 32x32"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The checkbox **paints**, and it paints the reference's art.
///
/// This is the falsifier for `UIPanelTemplates.xml`'s stated deviation: the reference declares its
/// state textures with `inherits=` alone, our loader does not expand `inherits=` there, and a
/// transcription that copied the reference literally would create a frame with no state textures at
/// all — no error, no warning, 69 invisible checkboxes.
#[test]
fn the_templated_check_button_draws_the_reference_checkbox_art() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(
        r#"MyCheck = CreateFrame("CheckButton", "MyCheck", UIParent, "UICheckButtonTemplate")
           MyCheck:SetPoint("TOPLEFT", 20, -20)"#,
    )
    .unwrap();
    assert!(
        s.eval::<bool>("return MyCheck:GetNormalTexture() ~= nil")
            .unwrap(),
        "the state-texture slot exists at all"
    );

    let drawn = drawn_textures(&mut s);
    assert!(
        drawn
            .iter()
            .any(|p| p == r"Interface\Buttons\UI-CheckBox-Up"),
        "the unchecked box is on screen: {drawn:?}"
    );
    assert!(
        !drawn
            .iter()
            .any(|p| p == r"Interface\Buttons\UI-CheckBox-Check"),
        "and the tick is not, until it is checked"
    );

    s.run("MyCheck:SetChecked(1)").unwrap();
    let drawn = drawn_textures(&mut s);
    assert!(
        drawn
            .iter()
            .any(|p| p == r"Interface\Buttons\UI-CheckBox-Check"),
        "checked draws the tick: {drawn:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// A click toggles, and the addon's `OnClick` sees the NEW state — the widget contract every
/// options panel in the corpus is written against (`if this:GetChecked() then …`).
#[test]
fn a_templated_check_button_toggles_before_its_on_click_runs() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(
        r#"MyCheck = CreateFrame("CheckButton", "MyCheck", UIParent, "UICheckButtonTemplate")
           MySeen = {}
           MyCheck:SetScript("OnClick", function()
               table.insert(MySeen, this:GetChecked() and 1 or 0)
           end)"#,
    )
    .unwrap();
    assert!(
        !s.eval::<bool>("return MyCheck:GetChecked() and true or false")
            .unwrap(),
        "a fresh box is unchecked"
    );

    s.run("MyCheck:Click()").unwrap();
    assert!(s
        .eval::<bool>("return MyCheck:GetChecked() and true or false")
        .unwrap());
    s.run("MyCheck:Click()").unwrap();
    assert!(!s
        .eval::<bool>("return MyCheck:GetChecked() and true or false")
        .unwrap());

    assert_eq!(
        s.eval::<Vec<i64>>("return MySeen").unwrap(),
        vec![1, 0],
        "the handler read the post-toggle state both times, not the pre-toggle one"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `UIPanelButtonTemplate` — 19 corpus call sites, and the shared original of nine private copies
/// in our own `assets/ui`. Its label is a `<ButtonText>` rather than a layer FontString, so the
/// addon idiom is `btn:SetText(...)` and the published global is still `$parentText`.
#[test]
fn a_panel_button_from_the_template_labels_and_paints() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(
        r#"MyBtn = CreateFrame("Button", "MyBtn", UIParent, "UIPanelButtonTemplate")
           MyBtn:SetWidth(90) MyBtn:SetHeight(22)
           MyBtn:SetPoint("TOPLEFT", 20, -20)
           MyBtn:SetText("Okay")"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>(r#"return getglobal("MyBtnText"):GetText()"#)
            .unwrap(),
        "Okay",
        "<ButtonText name=\"$parentText\"> publishes against the caller"
    );
    assert!(s
        .eval::<bool>(r#"return getglobal("UIPanelButtonTemplateText") == nil"#)
        .unwrap());

    let drawn = drawn_textures(&mut s);
    assert!(
        drawn
            .iter()
            .any(|p| p == r"Interface\Buttons\UI-Panel-Button-Up"),
        "the button face is on screen: {drawn:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `UIPanelCloseButton` — 8 call sites across 4 corpus addons, and the one template here whose
/// whole point is its script: `HideUIPanel(this:GetParent())`, resolved at click time against
/// the chain's `UIParent.xml`.
#[test]
fn the_templated_close_button_hides_the_frame_it_sits_on() {
    let _data = benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(
        r#"MyPanel = CreateFrame("Frame", "MyPanel", UIParent)
           MyPanel:SetWidth(200) MyPanel:SetHeight(100)
           MyPanel:SetPoint("CENTER", 0, 0)
           MyPanel:Show()
           MyClose = CreateFrame("Button", "MyClose", MyPanel, "UIPanelCloseButton")
           MyClose:SetPoint("TOPRIGHT", 0, 0)"#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return MyClose:GetWidth(), MyClose:GetHeight()")
            .unwrap(),
        (32.0, 32.0)
    );
    assert!(s.eval::<bool>("return MyPanel:IsShown()").unwrap());

    s.run("MyClose:Click()").unwrap();
    assert!(
        !s.eval::<bool>("return MyPanel:IsShown()").unwrap(),
        "the template's own OnClick reached HideUIPanel"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `InputBoxTemplate` — 5 call sites across 3 corpus addons. Its three border slices are `<Layers>`
/// regions (so they publish by name), and its direct-child `<FontString>` is the box's *text*
/// region, not a layer.
#[test]
fn an_input_box_from_the_template_carries_its_border_and_takes_text() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(
        r#"MyBox = CreateFrame("EditBox", "MyBox", UIParent, "InputBoxTemplate")
           MyBox:SetWidth(120) MyBox:SetHeight(20)
           MyBox:SetPoint("TOPLEFT", 20, -20)
           MyBox:SetText("hello")"#,
    )
    .unwrap();
    for slice in ["Left", "Right", "Middle"] {
        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("MyBox{slice}") ~= nil"#))
                .unwrap(),
            "the {slice} border slice publishes against the caller's name"
        );
    }
    assert_eq!(s.eval::<String>("return MyBox:GetText()").unwrap(), "hello");

    let drawn = drawn_textures(&mut s);
    assert!(
        drawn
            .iter()
            .any(|p| p == r"Interface\Common\Common-Input-Border"),
        "the input border draws: {drawn:?}"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `UIPanelScrollFrameTemplate` — 6 call sites across 2 corpus addons, and the template with the
/// deepest naming problem: its scroll bar's arrow buttons are `$parentScrollUpButton` inside
/// `$parentScrollBar`, so a caller named `MyScroll` must reach
/// `MyScrollScrollBarScrollUpButton` through **two** levels of `$parent` — which is precisely what
/// `ScrollFrame_OnLoad` getglobals on the way past.
///
/// The `UpdateScrollChildRect` leg is also the falsifier for the `<ThumbTexture>` publication:
/// `ScrollFrame_OnScrollRangeChanged` reaches the thumb through
/// `getglobal(bar:GetName().."ThumbTexture")`, which resolves only because our loader publishes a
/// named `<ThumbTexture>` as a global. If that publication regressed, this call would raise on a
/// nil index and `s.errors()` would not be empty.
///
/// (This used to claim the opposite — that we diverged to `GetThumbTexture()` "because our loader
/// never publishes a named `<ThumbTexture>`", and called it "this file's one Lua divergence". It
/// was already false when written: the XML has used `getglobal` throughout, at HEAD and now.
/// Corrected rather than deleted, because a doc comment asserting a divergence that does not exist
/// is exactly how someone later introduces a real one by "restoring" it.)
#[test]
fn a_scroll_frame_from_the_template_wires_its_bar_two_parents_deep() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(
        r#"MyScroll = CreateFrame("ScrollFrame", "MyScroll", UIParent, "UIPanelScrollFrameTemplate")
           MyScroll:SetWidth(290) MyScroll:SetHeight(80)
           MyScroll:SetPoint("TOPLEFT", 20, -20)"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    for child in [
        "ScrollBar",
        "ScrollBarScrollUpButton",
        "ScrollBarScrollDownButton",
    ] {
        assert!(
            s.eval::<bool>(&format!(r#"return getglobal("MyScroll{child}") ~= nil"#))
                .unwrap(),
            "MyScroll{child} — the caller's name won through every $parent level"
        );
    }
    // ScrollFrame_OnLoad ran off the template: empty range, both arrows greyed, offset 0.
    assert_eq!(
        s.eval::<(f64, f64)>("return MyScrollScrollBar:GetMinMaxValues()")
            .unwrap(),
        (0.0, 0.0)
    );
    assert!(!s
        .eval::<bool>("return MyScrollScrollBarScrollUpButton:IsEnabled() ~= 0")
        .unwrap());
    assert_eq!(s.eval::<i64>("return MyScroll.offset").unwrap(), 0);

    // A scroll child taller than the window, then the range change the reference's own handler
    // consumes — the leg that touches the thumb.
    s.run(
        r#"MyScrollChild = CreateFrame("Frame", "MyScrollChild", MyScroll)
           MyScrollChild:SetWidth(290) MyScrollChild:SetHeight(192)
           MyScroll:SetScrollChild(MyScrollChild)"#,
    )
    .unwrap();
    s.resolve();
    s.run("MyScroll:UpdateScrollChildRect()").unwrap();
    assert!(
        s.errors().is_empty(),
        "ScrollFrame_OnScrollRangeChanged ran clean: {:?}",
        s.errors()
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return MyScrollScrollBar:GetMinMaxValues()")
            .unwrap(),
        (0.0, 112.0),
        "192px of content in an 80px window — the bar's range is the overflow"
    );
    assert!(
        s.eval::<bool>("return MyScrollScrollBarScrollDownButton:IsEnabled() ~= 0")
            .unwrap(),
        "there is somewhere to scroll to, so the down arrow woke"
    );

    // And the frame's own <OnVerticalScroll> seats the bar and re-enables the up arrow.
    s.run("MyScroll:SetVerticalScroll(48)").unwrap();
    assert_eq!(
        s.eval::<f64>("return MyScrollScrollBar:GetValue()")
            .unwrap(),
        48.0
    );
    assert!(s
        .eval::<bool>("return MyScrollScrollBarScrollUpButton:IsEnabled() ~= 0")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `UIOptionsCheckButtonTemplate` — 8 call sites, all in one addon's options panel, and a
/// two-generation chain: it overrides the size of `OptionsCheckButtonTemplate`, which adds the hit
/// rect and the click sound to `UICheckButtonTemplate`. The chain has to resolve through
/// `CreateFrame`'s fourth argument the same way it resolves through XML `inherits=`.
#[test]
fn the_options_check_button_resolves_its_whole_inheritance_chain() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run(
        r#"MyOpt = CreateFrame("CheckButton", "MyOpt", UIParent, "UIOptionsCheckButtonTemplate")"#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    assert_eq!(
        s.eval::<(f64, f64)>("return MyOpt:GetWidth(), MyOpt:GetHeight()")
            .unwrap(),
        (26.0, 26.0),
        "UIOptionsFrame.xml's own size override, the LAST link in the chain"
    );
    let (_, right, _, _) = s
        .eval::<(f64, f64, f64, f64)>("return MyOpt:GetHitRectInsets()")
        .unwrap();
    assert_eq!(
        right, -100.0,
        "OptionsCheckButtonTemplate's label-catching hit rect, the MIDDLE link"
    );
    assert!(
        s.eval::<bool>(r#"return getglobal("MyOptText") ~= nil"#)
            .unwrap(),
        "and UICheckButtonTemplate's label, the ROOT link — still named after the caller"
    );

    // The middle link's OnClick is the reference's option-toggle sound pair.
    let _ = s.take_sounds();
    s.run("MyOpt:Click()").unwrap();
    assert!(
        !s.take_sounds().is_empty(),
        "clicking an options checkbox makes the reference's toggle sound"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

// **RETIRED with its subject.** `every_template_these_files_declare_is_a_real_1_12_name`
// swept the `virtual="true"` templates of the files we shipped under a reference name and required
// each to be a real 1.12 global (`reference/1.12-globals.tsv`) — the guard against inventing a
// plausible-but-absent template name that no addon could ever reach for. 1846 took
// `UIPanelTemplates.xml` to the chain and left it one subject, `OptionsFrameTemplates.xml`'s single
// `UIOptionsCheckButtonTemplate`; 2115 took that one too, by loading the reference's own
// `UIOptionsFrame.xml`. Nothing under `assets/ui` now declares a template under a reference name —
// what is left there is deliberately benilla-shaped (`BenillaScrollBarTemplate`,
// `OptionsCheckboxRowTemplate`, `BenillaScriptLogRowTemplate`), which is a different claim and the
// sweep's own doc said so. This is 1751 §5 working as written: a drift instrument loses its subject
// as the copies retire. `the_options_check_button_resolves_its_whole_inheritance_chain` above still
// proves the template resolves — off the chain now.

/// **`PanelTemplates_TabResize`'s `tab` argument is OPTIONAL, and omitting it means `this`.**
///
/// The reference opens with `if ( tab ) then tabName = tab:GetName(); else tabName =
/// this:GetName(); tab = this; end`. Ours had only the first half, so `tab:GetName()` on a nil
/// threw — and nothing in our tree noticed, because every caller we wrote passed a tab explicitly.
/// The stock files do not: `Blizzard_MacroUI.xml`'s two tabs each call
/// `PanelTemplates_TabResize(0)` from their own `OnLoad` and nothing else, so both threw at load
/// and the window came up with unsized tabs.
///
/// Driven through a real `OnLoad` rather than a direct call, because `this` is exactly what is
/// under test: a plain function call would not set it.
#[test]
fn tab_resize_falls_back_to_this_when_no_tab_is_passed() {
    let _data = benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <Button name="ProbeTab" inherits="CharacterFrameTabButtonTemplate" text="Macros">
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
                <Scripts><OnLoad>PanelTemplates_TabResize(0)</OnLoad></Scripts>
            </Button>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load_in(&s, &doc, "test", &|_: &str| None);
    assert!(
        report.errors.is_empty(),
        "the OnLoad must not throw: {:?}",
        report.errors
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    // It sized the tab it was never handed: wider than the bare label, because the end slices are
    // added on top.
    let width = s.eval::<f64>("return ProbeTab:GetWidth()").unwrap();
    assert!(width > 0.0, "the tab got a width, got {width}");
}

/// **The reference's own window tab fits its text in `<OnShow>`, on the first show, with no
/// settle** — the property that let benilla's own copy of this template retire.
///
/// The reference fits a tab exactly once, from the template's `<OnShow>`
/// (`CharacterFrameTemplates.xml:77-80`): `PanelTemplates_TabResize(0)`, whose `width` is
/// `tabText:GetWidth() + padding`. That needs a `GetStringWidth` that answers during the Lua call
/// that asked. Ours landed a frame late when our template was written, so our copy re-fit from
/// `OnUpdate` until the measure settled; the engine has a synchronous measurer now
/// (`script::measure`), so the stock handler is the whole fit. The falsifier is a tab still wearing
/// the template's authored 115 after one show — which is exactly what an OnShow-only fit on the old
/// engine produced.
///
/// It also pins **1004's structural half, now on the reference's own file**: the highlight anchors
/// `LEFT +10` / `RIGHT −10`, so it spans the tab at every width and `TabResize`'s own
/// `highlightTexture:SetWidth(tabWidth)` — and the `<OnShow>`'s second line, `SetWidth(
/// GetTextWidth() + 30)` — are dead against two opposing anchors, here as in the reference.
///
/// **What went with our template is 1002's clamp** (superseded by 1993): a tab may now grow past
/// its window's drawn edge exactly as it does in the real client, whose only caps are the numbers
/// each window passes `TabResize` itself. That is asserted here by its absence — the long label
/// below overflows the 160-wide probe window and nothing stops it.
#[test]
fn the_stock_tab_fits_its_text_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `2 * $parentLeft:GetWidth()` — the template's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = harness();
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    // The highlight authors NO `<Size>`, so its height is content-derived off the art's texel
    // extent — `UI-Character-Tab-Highlight.blp` is 128x32. Without a probe
    // the Y axis never resolves and the region has no rect at all, which is a property of an
    // engine-less VM, not of the template.
    s.set_texture_size_probe(Box::new(|_| Some((128, 32))));
    let doc = benilla_ui::framexml::parse(
        r#"<Ui>
            <Frame name="ProbeWindow" parent="UIParent" hidden="true">
                <Size><AbsDimension x="160" y="200"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
                <Frames>
                    <Button name="ProbeTab" inherits="CharacterFrameTabButtonTemplate"
                            text="A Very Long Tab Label Indeed">
                        <Anchors>
                            <Anchor point="TOPLEFT"><Offset><AbsDimension x="10" y="-10"/></Offset></Anchor>
                        </Anchors>
                    </Button>
                </Frames>
            </Frame>
        </Ui>"#,
    )
    .unwrap();
    let report = benilla_ui::loader::load_in(&s, &doc, "test", &|_: &str| None);
    assert!(report.errors.is_empty(), "load: {:?}", report.errors);

    // The first show, and nothing else: no tick, no measure round trip to pump.
    s.run("ProbeWindow:Show()").unwrap();
    assert!(
        s.errors().is_empty(),
        "the stock OnShow must not raise: {:?}",
        s.errors()
    );
    s.resolve();

    let (label, width) = s
        .eval::<(f64, f64)>("return ProbeTabText:GetStringWidth(), ProbeTab:GetWidth()")
        .unwrap();
    assert!(label > 0.0, "the label measured synchronously, got {label}");
    assert_eq!(
        width,
        label + SIDES,
        "the tab is its text plus the two end slices, from OnShow alone"
    );
    assert_ne!(width, 115.0, "…and not the template's authored pre-fit");

    // No settle: a frame later it is the same number, because nothing re-fits it.
    for _ in 0..3 {
        s.tick(0.016);
    }
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return ProbeTab:GetWidth()").unwrap(),
        width,
        "the fit is once, in OnShow — nothing may move it per frame"
    );

    // 1004, structurally: the highlight is the tab inset 10 on each side, at this width.
    // Lit, so it joins the resolved tree — the way the dropdown kit's checked rows are lit.
    s.run("ProbeTab:LockHighlight()").unwrap();
    s.resolve();
    let (tl, tr, hl, hr) = s
        .eval::<(f64, f64, f64, f64)>(
            "return ProbeTab:GetLeft(), ProbeTab:GetRight(), \
             ProbeTabHighlightTexture:GetLeft(), ProbeTabHighlightTexture:GetRight()",
        )
        .unwrap();
    assert_eq!(
        (hl - tl, tr - hr),
        (10.0, 10.0),
        "the highlight spans the tab's own edges — the OnShow SetWidth is inert against them"
    );

    // 1002's clamp is gone with our template: the reference has no such guarantee, and the long
    // label really does run past the window's right edge now.
    let right = s.eval::<f64>("return ProbeWindow:GetRight()").unwrap();
    assert!(
        tl + width > right,
        "the probe label was meant to overflow: left {tl} + width {width} <= right {right}"
    );
}
