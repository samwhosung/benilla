//! The stock faux-scroll kit (`UIPanelTemplates.lua`), driven as an addon drives it: a bare
//! `FauxScrollFrameTemplate` with its own rows. The fixture must be a `<ScrollFrame>`, since only
//! that tag gets the template's scroll child, which `FauxScrollFrame_Update` reads unguarded.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// Loads an inline document against the templates already registered.
fn load_inline(s: &UiScript, xml: &str) {
    let doc = benilla_ui::framexml::parse(xml).unwrap();
    let report = benilla_ui::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
}

/// The kit and an addon's list: five rows, a highlight and a faux scroll frame over them.
fn harness() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_inline(
        &s,
        r#"<Ui>
            <Frame name="TestList">
                <Size><AbsDimension x="300" y="120"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
                <Frames>
                    <Button name="TestRow1"><Size><AbsDimension x="300" y="16"/></Size></Button>
                    <Button name="TestRow2"><Size><AbsDimension x="300" y="16"/></Size></Button>
                    <Button name="TestRow3"><Size><AbsDimension x="300" y="16"/></Size></Button>
                    <Button name="TestRow4"><Size><AbsDimension x="300" y="16"/></Size></Button>
                    <Button name="TestRow5"><Size><AbsDimension x="300" y="16"/></Size></Button>
                    <Frame name="TestHighlight"><Size><AbsDimension x="300" y="16"/></Size></Frame>
                    <ScrollFrame name="TestScroll" inherits="FauxScrollFrameTemplate">
                        <Size><AbsDimension x="290" y="80"/></Size>
                        <Anchors><Anchor point="TOPLEFT"/></Anchors>
                        <Scripts>
                            <OnVerticalScroll>FauxScrollFrame_OnVerticalScroll(16, TestUpdate)</OnVerticalScroll>
                        </Scripts>
                    </ScrollFrame>
                </Frames>
            </Frame>
        </Ui>"#,
    );
    s.run("function TestUpdate() end").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    s
}

/// Stock `ScrollFrame_OnLoad` runs off the template (`UIPanelTemplates.lua:244-252`).
#[test]
fn a_fresh_faux_frame_loads_at_the_top_with_no_range() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    assert_eq!(s.eval::<i64>("return TestScroll.offset").unwrap(), 0);
    let (lo, hi) = s
        .eval::<(f64, f64)>("return TestScrollScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!((lo, hi), (0.0, 0.0));
    assert!(
        !s.eval::<bool>("return TestScrollScrollBarScrollUpButton:IsEnabled() ~= 0")
            .unwrap(),
        "up arrow greyed at load (ref UIPanelTemplates.lua l.245-246)"
    );
    assert!(!s
        .eval::<bool>("return TestScrollScrollBarScrollDownButton:IsEnabled() ~= 0")
        .unwrap());
}

/// The bar's range is the overflow in pixels and its step one row (`UIPanelTemplates.lua:189-190`).
#[test]
fn more_rows_than_fit_raise_the_bar_over_the_overflow_range() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    let shown = s
        .eval::<i64>("return FauxScrollFrame_Update(TestScroll, 12, 5, 16)")
        .unwrap();
    assert_eq!(shown, 1, "the ref returns 1 when the bar is up");
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());

    let (lo, hi) = s
        .eval::<(f64, f64)>("return TestScrollScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!(
        (lo, hi),
        (0.0, 112.0),
        "(12 - 5) rows * 16px — the value model is PIXELS (ScrollTemplates.xml's header)"
    );
    assert_eq!(
        s.eval::<f64>("return TestScrollScrollBar:GetValueStep()")
            .unwrap(),
        16.0
    );
    assert!(s
        .eval::<bool>("return TestScrollScrollBar:IsVisible()")
        .unwrap());
    assert!(
        s.eval::<bool>("return TestScroll:IsVisible()").unwrap(),
        "the frame itself too — the ref's own frame:Show(), which is what an addon reads back"
    );
    assert!(
        !s.eval::<bool>("return TestScrollScrollBarScrollUpButton:IsEnabled() ~= 0")
            .unwrap(),
        "at the top: up greyed"
    );
    assert!(s
        .eval::<bool>("return TestScrollScrollBarScrollDownButton:IsEnabled() ~= 0")
        .unwrap());
}

/// The bar shows only for `numItems > numToDisplay` (`UIPanelTemplates.lua:165`).
#[test]
fn a_list_that_exactly_fits_shows_no_bar() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    let shown = s
        .eval::<Option<i64>>("return FauxScrollFrame_Update(TestScroll, 5, 5, 16)")
        .unwrap();
    assert_eq!(shown, None, "the ref returns nil when the bar stays down");
    assert!(!s
        .eval::<bool>("return TestScrollScrollBar:IsVisible()")
        .unwrap());
    assert!(!s.eval::<bool>("return TestScroll:IsVisible()").unwrap());
    let (_, hi) = s
        .eval::<(f64, f64)>("return TestScrollScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!(hi, 0.0, "no overflow, no range");

    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_Update(TestScroll, 6, 5, 16)")
            .unwrap(),
        1
    );
    assert!(s
        .eval::<bool>("return TestScrollScrollBar:IsVisible()")
        .unwrap());
}

/// The clamp lives on the bar: stock `FauxScrollFrame_SetOffset` only stores the offset
/// (`UIPanelTemplates.lua:239-241`), and a scroll inside the re-ranged bar walks it back.
#[test]
fn a_shrinking_list_clamps_the_offset_back_into_range() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("FauxScrollFrame_Update(TestScroll, 20, 5, 16)")
        .unwrap();
    // The range follows the child's resolved height, a solve behind the update's `SetHeight`.
    s.resolve();
    s.run("FauxScrollFrame_SetOffset(TestScroll, 12)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        12
    );

    // Down to 8 rows: the deepest offset is now 3.
    s.run("FauxScrollFrame_Update(TestScroll, 8, 5, 16)")
        .unwrap();
    s.resolve();
    let (lo, hi) = s
        .eval::<(f64, f64)>("return TestScrollScrollBar:GetMinMaxValues()")
        .unwrap();
    assert_eq!(
        (lo, hi),
        (0.0, 48.0),
        "the bar re-ranges to (numItems - numToDisplay) * valueStep = 3 rows"
    );
    s.run("TestScrollScrollBar:SetValue(999)").unwrap();
    s.resolve();
    assert!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap()
            <= 3,
        "a scroll lands inside the re-ranged bar, never past the last full page"
    );
    assert_eq!(
        s.eval::<f64>("return TestScrollScrollBar:GetValue()")
            .unwrap(),
        48.0,
        "and the thumb followed the clamp"
    );

    // Fewer than fit: the stock update sets the bar to 0 (`UIPanelTemplates.lua:169`).
    s.run("FauxScrollFrame_Update(TestScroll, 3, 5, 16)")
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        0
    );
    assert!(!s
        .eval::<bool>("return TestScrollScrollBar:IsVisible()")
        .unwrap());
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The six trailing arguments narrow the rows while the bar shows and widen them back when it
/// hides (`UIPanelTemplates.lua:205-223`).
#[test]
fn the_shrink_widen_tail_resizes_the_rows_and_the_highlight() {
    benilla_formats::wow_data_or_skip!();
    let s = harness();
    s.run(
        "FauxScrollFrame_Update(TestScroll, 12, 5, 16, \"TestRow\", 280, 300, TestHighlight, 276, 296)",
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    for i in 1..=5 {
        assert_eq!(
            s.eval::<f64>(&format!("return TestRow{i}:GetWidth()"))
                .unwrap(),
            280.0,
            "row {i} narrowed for the bar"
        );
    }
    assert_eq!(
        s.eval::<f64>("return TestHighlight:GetWidth()").unwrap(),
        276.0
    );

    s.run(
        "FauxScrollFrame_Update(TestScroll, 4, 5, 16, \"TestRow\", 280, 300, TestHighlight, 276, 296)",
    )
    .unwrap();
    for i in 1..=5 {
        assert_eq!(
            s.eval::<f64>(&format!("return TestRow{i}:GetWidth()"))
                .unwrap(),
            300.0,
            "row {i} widened back"
        );
    }
    assert_eq!(
        s.eval::<f64>("return TestHighlight:GetWidth()").unwrap(),
        296.0
    );
}

/// Pixels on the bar, rows in the offset, `floor(v / step + 0.5)` between
/// (`UIPanelTemplates.lua:231`).
#[test]
fn dragging_the_bar_steps_the_offset_by_rows_and_repaints() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("TestRepaints = 0 function TestUpdate() TestRepaints = TestRepaints + 1 end")
        .unwrap();
    s.run("FauxScrollFrame_Update(TestScroll, 12, 5, 16)")
        .unwrap();
    s.resolve(); // the range is a solve behind the update's SetHeight
    let before = s.eval::<i64>("return TestRepaints").unwrap();

    s.run("TestScrollScrollBar:SetValue(48)").unwrap();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        3
    );
    // Not an exact count: `FauxScrollFrame_OnVerticalScroll` re-sets its own bar
    // (`UIPanelTemplates.lua:230`), so one drag can repaint more than once.
    assert!(
        s.eval::<i64>("return TestRepaints").unwrap() > before,
        "the drag repainted the owner's list"
    );

    // `SetValue` snaps to `min + n * step` before its change test (`0x789930`), so 51px stays at
    // the 48 the bar holds and fires nothing.
    let settled = s.eval::<i64>("return TestRepaints").unwrap();
    s.run("TestScrollScrollBar:SetValue(51)").unwrap();
    assert_eq!(
        s.eval::<f64>("return TestScrollScrollBar:GetValue()")
            .unwrap(),
        48.0,
        "51px is not a lattice point; the bar stays on row 3's"
    );
    assert_eq!(
        s.eval::<i64>("return TestRepaints").unwrap(),
        settled,
        "and a value that did not move fires nothing"
    );

    // A scroll through the frame always repaints: `FauxScrollFrame_OnVerticalScroll` calls the
    // update with no compare (`UIPanelTemplates.lua:228-232`).
    s.run("TestScroll:SetVerticalScroll(51)").unwrap();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        3,
        "51px is still row 3 once rounded (`floor(v/itemHeight + 0.5)`)"
    );
    assert!(
        s.eval::<i64>("return TestRepaints").unwrap() > settled,
        "the reference repaints on every scroll, changed row or not"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

/// The addon path end to end: bar value, `SetVerticalScroll`, the frame's `<OnVerticalScroll>`,
/// then `FauxScrollFrame_OnVerticalScroll`. `SetScrollChild` seats a child by hand, as addons do.
#[test]
fn the_reference_on_vertical_scroll_path_runs_once_a_scroll_child_exists() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    load_inline(
        &s,
        r#"<Ui>
            <ScrollFrame name="AddonScroll" inherits="FauxScrollFrameTemplate">
                <Size><AbsDimension x="290" y="80"/></Size>
                <Anchors><Anchor point="TOPLEFT"/></Anchors>
                <Scripts>
                    <OnVerticalScroll>FauxScrollFrame_OnVerticalScroll(16, AddonScroll_Update)</OnVerticalScroll>
                </Scripts>
            </ScrollFrame>
            <Frame name="AddonScrollChildFrame">
                <Size><AbsDimension x="290" y="192"/></Size>
            </Frame>
        </Ui>"#,
    );
    s.run("AddonRepaints = 0 function AddonScroll_Update() AddonRepaints = AddonRepaints + 1 end")
        .unwrap();

    // An inheriting node keeps its own tag, so this is a ScrollFrame.
    assert!(
        s.eval::<bool>("return type(AddonScroll.SetVerticalScroll) == 'function'")
            .unwrap(),
        "<ScrollFrame inherits=\"FauxScrollFrameTemplate\"> is a ScrollFrame"
    );

    s.run("AddonScroll:SetScrollChild(AddonScrollChildFrame)")
        .unwrap();
    s.resolve();
    s.run("AddonScroll:UpdateScrollChildRect()").unwrap();
    assert_eq!(
        s.eval::<f64>("return AddonScroll:GetVerticalScrollRange()")
            .unwrap(),
        112.0,
        "192px of rows in an 80px window — the same overflow the bar's range carries"
    );

    s.run("FauxScrollFrame_Update(AddonScroll, 12, 5, 16)")
        .unwrap();
    s.run("AddonScrollScrollBar:SetValue(48)").unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(AddonScroll)")
            .unwrap(),
        3,
        "the bar's value reached the frame's OnVerticalScroll and became a row offset"
    );
    assert!(
        s.eval::<i64>("return AddonRepaints").unwrap() >= 1,
        "and the addon's own update function ran"
    );
}

/// With no argument, stock `ScrollingEdit_OnTextChanged` falls back to `this:GetParent()`
/// (`UIPanelTemplates.lua:307-310`), which addons that wire it bare rely on.
#[test]
fn scrolling_edit_helpers_answer_bare_calls_from_a_handler() {
    benilla_formats::wow_data_or_skip!();
    let s = UiScript::new().unwrap();
    load_xml(&s, "Interface\\FrameXML\\Fonts.xml");
    load_xml(&s, "ScrollTemplates.xml");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.lua");
    load_xml(&s, r"Interface\FrameXML\UIPanelTemplates.xml");
    load_inline(
        &s,
        r#"<Ui>
            <ScrollFrame name="SeScroll">
                <Size><AbsDimension x="200" y="100"/></Size>
                <Anchors><Anchor point="CENTER"/></Anchors>
                <ScrollChild>
                    <EditBox name="SeEdit" multiLine="true">
                        <Size><AbsDimension x="200" y="300"/></Size>
                    </EditBox>
                </ScrollChild>
            </ScrollFrame>
        </Ui>"#,
    );

    // A bare call with `this` set, as a handler makes it.
    s.run(
        "this = SeEdit; ScrollingEdit_OnTextChanged(); ScrollingEdit_OnCursorChanged(0, 42, 0, 14); this = nil",
    )
    .unwrap();
    assert!(
        s.errors().is_empty(),
        "bare calls must not raise: {:?}",
        s.errors()
    );

    assert_eq!(s.eval::<f32>("return SeEdit.cursorOffset").unwrap(), 42.0);
    assert_eq!(s.eval::<f32>("return SeEdit.cursorHeight").unwrap(), 14.0);

    s.run("ScrollingEdit_OnTextChanged(SeScroll)").unwrap();
    assert!(s.errors().is_empty(), "explicit form: {:?}", s.errors());
}
