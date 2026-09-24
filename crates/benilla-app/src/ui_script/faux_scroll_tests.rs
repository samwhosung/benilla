//! The **reference's own** faux-scroll kit — `Interface\FrameXML\UIPanelTemplates`, off the
//! player's chain since 1837, our transcription deleted by 1860 — under its own
//! names — `FauxScrollFrame_Update` / `_GetOffset` / `_SetOffset` / `_OnVerticalScroll` and
//! `ScrollFrame_OnLoad` (a name the shipped 1.12 UI defines is FrameXML's).
//!
//! These drive the kit the way an addon does — a bare instance of `FauxScrollFrameTemplate` with
//! rows of its own — rather than through one of our six owner windows, because the API is now a
//! public surface and its edges (a list that overflows, one that exactly fits, one that shrinks
//! under a scrolled offset, the shrink/widen tail, the return value) belong to it and not to any
//! window. The windows' own tests still cover their wiring.
//!
//! The last test is the one worth reading: it drives the reference's *whole* path — bar drag →
//! `SetVerticalScroll` → the frame's `<OnVerticalScroll>` → `FauxScrollFrame_OnVerticalScroll` —
//! on a real `<ScrollFrame>` with a scroll child, which is exactly the shape 27 corpus addons
//! write. `<ScrollChild>` landed with 1205, so the template's own child arrives through
//! inheritance now (`framexml.rs merge` clones the base's children) — which is why the fixture
//! here must be a `<ScrollFrame>`: `merge` takes the OVERRIDING node's tag, and a `<Frame>` would
//! skip the loader's ScrollChild pass and leave `$parentScrollChildFrame` nil for the reference's
//! `FauxScrollFrame_Update`, which touches it unguarded.

use benilla_ui::script::UiScript;

use super::test_ui::load_ui as load_xml;

/// A document written here in the test, loaded against the templates already registered — the
/// cross-file template registry is on the `Model`, so an inline doc reaches `ScrollTemplates.xml`'s
/// `FauxScrollFrameTemplate` exactly as `TrainerFrame.xml` does.
fn load_inline(s: &UiScript, xml: &str) {
    let doc = benilla_ui::framexml::parse(xml).unwrap();
    let report = benilla_ui::loader::load(s, &doc, &|_| None);
    assert!(
        report.errors.is_empty(),
        "loader errors: {:?}",
        report.errors
    );
}

/// The kit plus a list an addon might own: five visible rows, a moving highlight, and a faux
/// scroll frame over them.
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

/// `ScrollFrame_OnLoad` ran off the template: a fresh frame is at the top, with an empty range and
/// both arrows greyed. (The reference's own `UIPanelScrollFrameTemplate` OnLoad — the name we now
/// wear instead of a `Benilla*` one.)
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

/// **More rows than fit**: the bar comes up, its range is the overflow in pixels, its step is one
/// row, and `FauxScrollFrame_Update` returns the reference's `showScrollBar`.
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

/// **Exactly fitting** is the boundary the reference draws at `numItems > numToDisplay`: five rows
/// in five slots shows nothing, and there is no off-by-one that leaves a dead bar up.
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

    // One more row and it appears — the same call, one item along.
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_Update(TestScroll, 6, 5, 16)")
            .unwrap(),
        1
    );
    assert!(s
        .eval::<bool>("return TestScrollScrollBar:IsVisible()")
        .unwrap());
}

/// **Fewer rows than fit**, arrived at by the list SHRINKING under a scrolled offset.
///
/// **The clamp lives on the BAR, not on `frame.offset` — the reference's own arrangement, and ours
/// diverged from it until 1860.** `FauxScrollFrame_SetOffset` is two lines in the reference
/// (`frame.offset = offset`, UIPanelTemplates.lua:239-241): it does not move the thumb, so the
/// stored offset and the bar can disagree until something scrolls. `FauxScrollFrame_Update` then
/// re-ranges the bar (`SetMinMaxValues(0, (numItems - numToDisplay) * valueStep)`), and it is the
/// slider's own clamp — firing OnValueChanged into the owner's `<OnVerticalScroll>` — that walks
/// a too-deep offset back. Our deleted copy wrote `frame.offset` directly instead, which is why
/// this test used to read a clamped offset immediately.
#[test]
fn a_shrinking_list_clamps_the_offset_back_into_range() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("FauxScrollFrame_Update(TestScroll, 20, 5, 16)")
        .unwrap();
    // The scroll range comes off the child's RESOLVED height, one solve behind the `SetHeight`
    // the update just did (0251's lag). The app resolves every frame; a test has to say so.
    s.resolve();
    s.run("FauxScrollFrame_SetOffset(TestScroll, 12)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        12
    );

    // The list drops to 8 rows: the deepest legal offset is now 3, and the BAR is what says so.
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
    // Scrolling now cannot go deeper than that range, which is how the reference walks a stale
    // offset back — the offset itself is untouched until it does.
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

    // Down to fewer than fit: offset 0, bar gone, and nothing left pointing past the end.
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

/// The reference's **shrink/widen tail** — the six trailing arguments 28 corpus addons pass and
/// benilla's own windows never do. Rows narrow when the bar takes the gutter and widen back when
/// it leaves (ref UIPanelTemplates.lua l.205-223).
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

    // The list fits again: every row goes back to the wide measurement.
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

/// Dragging the bar moves the offset a whole row at a time and repaints the owner's list — the
/// value model in one test (pixels on the bar, rows in the offset, `floor(v/step + 0.5)` between).
#[test]
fn dragging_the_bar_steps_the_offset_by_rows_and_repaints() {
    benilla_formats::wow_data_or_skip!();
    let mut s = harness();
    s.run("TestRepaints = 0 function TestUpdate() TestRepaints = TestRepaints + 1 end")
        .unwrap();
    s.run("FauxScrollFrame_Update(TestScroll, 12, 5, 16)")
        .unwrap();
    s.resolve(); // the range is a solve behind the update's SetHeight (0251)
    let before = s.eval::<i64>("return TestRepaints").unwrap();

    s.run("TestScrollScrollBar:SetValue(48)").unwrap();
    assert_eq!(
        s.eval::<i64>("return FauxScrollFrame_GetOffset(TestScroll)")
            .unwrap(),
        3
    );
    // The drag repainted. NOT an exact count: the reference's `FauxScrollFrame_OnVerticalScroll`
    // re-sets the bar it was called from (`scrollbar:SetValue(arg1)`, UIPanelTemplates.lua:230),
    // so one drag can round-trip the value-changed chain more than once. Our deleted copy took the
    // offset straight off the slider and could promise one repaint per row; the reference cannot,
    // and pinning the number here would be pinning that re-entry rather than the contract (1860).
    assert!(
        s.eval::<i64>("return TestRepaints").unwrap() > before,
        "the drag repainted the owner's list"
    );

    // A sub-row nudge on the BAR reaches nothing at all: `FauxScrollFrame_Update` set the bar's
    // step to one row, and `SetValue` quantises onto `min + n·step` before its change compare
    // (2133, `0x789930`), so 51px resolves to the 48 the bar already holds — no
    // `OnValueChanged`, no `SetVerticalScroll`, no repaint. This is the snap a 1.12 list
    // scrollbar has and ours did not: the bar cannot come to rest between two rows.
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

    // The unconditional repaint is still there — it lives one level up, on the path a wheel or a
    // `SetVerticalScroll` takes. `FauxScrollFrame_OnVerticalScroll` ends in a bare
    // `updateFunction();` (UIPanelTemplates.lua:228-232) with no compare against the previous
    // offset, so a sub-row scroll through the FRAME repaints even though the bar does not move.
    // Our deleted kit repainted only when the row actually changed; the reference does not, and
    // that difference is the migration's, not a regression to chase (1860).
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

/// **The addon path, end to end** — the reference's wiring rather than ours: a real `<ScrollFrame>`
/// on `FauxScrollFrameTemplate` whose `<OnVerticalScroll>` calls `FauxScrollFrame_OnVerticalScroll`,
/// driven by a drag of the shared bar.
///
/// The `SetScrollChild` line predates 1205 (the loader's `<ScrollChild>`) and stays as the
/// addon-shaped way of seating a child by hand; the range it asserts is that child's overflow
/// (1338). The offset itself never depended on the range — the engine stores what the bar hands it
/// — so what this drives is bar value → `SetVerticalScroll` → `<OnVerticalScroll>`
/// → `FauxScrollFrame_OnVerticalScroll`'s `floor(v / step + 0.5)`, the reference's path end to end.
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

    // The instance's own tag wins over the template's on inherit, so this really is a ScrollFrame.
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

/// **The ScrollingEdit trio answers to the REFERENCE's names, and to a bare call.**
///
/// `InviteOMatic` wires `ScrollingEdit_OnTextChanged` straight into an EditBox's `OnTextChanged`
/// and raised `attempt to call global` the moment anyone typed — found by the use-probe, since it
/// only fires on input. Both helpers take an OPTIONAL scroll frame and fall back to
/// `this:GetParent()` (ref `UIPanelTemplates.lua` l.307-310); an addon wiring the bare name depends
/// on that fallback, so the no-argument path is what this test drives.
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

    // Bare call with `this` set to the edit box — the addon's exact wiring.
    s.run(
        "this = SeEdit; ScrollingEdit_OnTextChanged(); ScrollingEdit_OnCursorChanged(0, 42, 0, 14); this = nil",
    )
    .unwrap();
    assert!(
        s.errors().is_empty(),
        "bare calls must not raise: {:?}",
        s.errors()
    );

    // OnCursorChanged records where the caret is, which is what an OnUpdate would follow.
    assert_eq!(s.eval::<f32>("return SeEdit.cursorOffset").unwrap(), 42.0);
    assert_eq!(s.eval::<f32>("return SeEdit.cursorHeight").unwrap(), 14.0);

    // ...and the explicit-frame form works too (the reference's first branch).
    s.run("ScrollingEdit_OnTextChanged(SeScroll)").unwrap();
    assert!(s.errors().is_empty(), "explicit form: {:?}", s.errors());
}
