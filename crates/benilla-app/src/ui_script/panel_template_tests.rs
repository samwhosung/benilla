//! The stock shared widget kit (`UIPanelTemplates.xml`, `OptionsFrameTemplates.xml`,
//! `UIOptionsFrame.xml`'s check button, the tab template) driven as an addon drives it: through
//! `CreateFrame`'s template argument, from Lua, under the caller's own name.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// The files under test on the manifest prefix they sit on, in manifest order.
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
        // `UIOptionsCheckButtonTemplate`'s home (`UIOptionsFrame.xml:6`), after the dropdown kit
        // its window uses.
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

/// The label publishes as `MyCheckText`, the global an addon reads next through
/// `getglobal(this:GetName().."Text")`.
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

/// Its face is `<NormalTexture inherits="UIPanelButtonUpTexture"/>` (`UIPanelTemplates.xml:23`),
/// which the loader expands, and its `<ButtonText>` still publishes as `$parentText`.
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

/// Its OnClick is `HideUIPanel(this:GetParent())` (`UIPanelTemplates.xml:96-98`).
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

/// Its border slices are named `<Layers>` regions; its direct-child `<FontString>` is the box's
/// text, not a layer (`UIPanelTemplates.xml:226-278`).
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

/// The arrows sit two `$parent` levels deep (`MyScrollScrollBarScrollUpButton`), where
/// `ScrollFrame_OnLoad` looks them up, and `ScrollFrame_OnScrollRangeChanged` reaches the thumb
/// by `getglobal(bar:GetName().."ThumbTexture")` (`UIPanelTemplates.lua:244-283`), so a named
/// `<ThumbTexture>` must publish as a global.
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
    // `ScrollFrame_OnLoad` ran: empty range, arrows disabled, offset 0.
    assert_eq!(
        s.eval::<(f64, f64)>("return MyScrollScrollBar:GetMinMaxValues()")
            .unwrap(),
        (0.0, 0.0)
    );
    assert!(!s
        .eval::<bool>("return MyScrollScrollBarScrollUpButton:IsEnabled() ~= 0")
        .unwrap());
    assert_eq!(s.eval::<i64>("return MyScroll.offset").unwrap(), 0);

    // A child taller than the frame changes the range, and the handler touches the thumb.
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

    // The frame's `<OnVerticalScroll>` seats the bar and re-enables the up arrow.
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

/// Three links: `UIOptionsCheckButtonTemplate`'s size (`UIOptionsFrame.xml:6-10`) over
/// `OptionsCheckButtonTemplate`'s hit rect and click sound (`OptionsFrameTemplates.xml:40-51`) over
/// `UICheckButtonTemplate`.
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

    // The middle link's OnClick plays the option toggle sounds.
    let _ = s.take_sounds();
    s.run("MyOpt:Click()").unwrap();
    assert!(
        !s.take_sounds().is_empty(),
        "clicking an options checkbox makes the reference's toggle sound"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// `PanelTemplates_TabResize`'s `tab` defaults to `this` (`UIPanelTemplates.lua:33-38`), and stock
/// tabs call it without one from `OnLoad` or `OnShow`; a real `OnLoad` is what sets `this`.
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
    let width = s.eval::<f64>("return ProbeTab:GetWidth()").unwrap();
    assert!(width > 0.0, "the tab got a width, got {width}");
}

/// The stock tab fits its text once, in `<OnShow>` (`CharacterFrameTemplates.xml:77-80`), so the
/// text must measure synchronously. Its highlight is anchored 10 in from each side (`:94-107`),
/// which leaves both `SetWidth` calls on it inert, and nothing caps a tab at its window's edge.
#[test]
fn the_stock_tab_fits_its_text_on_the_first_show() {
    let _data = benilla_formats::wow_data_or_skip!();
    /// `2 * $parentLeft:GetWidth()`: the template's two 20-unit end slices.
    const SIDES: f64 = 40.0;
    let mut s = harness();
    s.set_text_measurer(Box::new(super::FixedWidthFont(6.0)));
    // The highlight has no `<Size>`, so its height comes from its art, which is 128x32
    // (`UI-Character-Tab-Highlight.blp`); a VM with no engine needs this probe for it.
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

    // One show, no tick.
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

    for _ in 0..3 {
        s.tick(0.016);
    }
    s.resolve();
    assert_eq!(
        s.eval::<f64>("return ProbeTab:GetWidth()").unwrap(),
        width,
        "the fit is once, in OnShow — nothing may move it per frame"
    );

    // Lit, so the highlight joins the resolved tree.
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

    // No clamp: the long label runs past the window's right edge, as in 1.12.
    let right = s.eval::<f64>("return ProbeWindow:GetRight()").unwrap();
    assert!(
        tl + width > right,
        "the probe label was meant to overflow: left {tl} + width {width} <= right {right}"
    );
}
