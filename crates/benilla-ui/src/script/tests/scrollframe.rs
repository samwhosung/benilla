//! ScrollFrame: the scroll-child layout override, the live scroll ranges, the clip on `extract`
//! and the clip-gated `hit_test`.

use super::common::script;
use crate::layout::Rect;
use crate::script::QuadContent;

/// The rect of the first Texture quad whose path is `marker`.
fn marker_rect(quads: &[crate::script::ExtractedQuad], marker: &str) -> Option<Rect> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } if p == marker => q.rect,
        _ => None,
    })
}

/// The clip of the first Texture quad whose path is `marker`; `None` if there is no such quad.
fn marker_clip(quads: &[crate::script::ExtractedQuad], marker: &str) -> Option<Option<Rect>> {
    quads.iter().find_map(|q| match &q.content {
        QuadContent::Texture { path: Some(p), .. } if p == marker => Some(q.clip),
        _ => None,
    })
}

#[test]
fn scrollframe_methods_exist_only_on_scrollframes() {
    let s = script();
    let ok: bool = s
        .eval(
            r#"
        local sf = CreateFrame("ScrollFrame", "SfDuck")
        local plain = CreateFrame("Frame")
        -- Duck-typing: addons branch on `if frame.SetScrollChild then` — a plain frame says nil.
        return (type(sf.SetScrollChild) == "function") and (plain.SetScrollChild == nil)
            and (type(sf.SetVerticalScroll) == "function") and (plain.SetVerticalScroll == nil)
            and (type(sf.GetVerticalScrollRange) == "function")
            -- the horizontal trio lives in the same class table, not on every Frame
            and (type(sf.SetHorizontalScroll) == "function") and (plain.SetHorizontalScroll == nil)
            and (type(sf.GetHorizontalScroll) == "function") and (plain.GetHorizontalScroll == nil)
            and (type(sf.GetHorizontalScrollRange) == "function")
            and (plain.GetHorizontalScrollRange == nil)
            and (type(sf.Show) == "function") -- base methods still reachable through the fallback
    "#,
        )
        .unwrap();
    assert!(ok);
}

#[test]
fn get_scroll_child_roundtrips_wrapper_and_name_and_clears_on_nil() {
    let s = script();
    s.run(
        r#"
        local sf = CreateFrame("ScrollFrame", "SF")
        assert(sf:GetScrollChild() == nil, "no child yet")

        local child = CreateFrame("Frame", "Child")
        sf:SetScrollChild(child)
        assert(sf:GetScrollChild() == child, "stable wrapper identity round-trips")

        sf:SetScrollChild(nil)
        assert(sf:GetScrollChild() == nil, "nil clears")

        -- A name string resolves like every other frame-target arg (SetParent, SetPoint's relativeTo).
        local byName = CreateFrame("Frame", "ByName")
        sf:SetScrollChild("ByName")
        assert(sf:GetScrollChild() == byName)
    "#,
    )
    .unwrap();
}

/// A positive offset lifts the child; offsets past the range or below zero are stored and applied
/// as given, since `0x786db0` never reads the range.
#[test]
fn scroll_child_top_tracks_vertical_scroll_unclamped_then_restores_on_clear() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0); // screen rect: bottom 0, left 0, top 600, right 800

    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)  -- screen top 600 -> frame top 500
        frame:SetWidth(300); frame:SetHeight(200)  -- frame bottom 300

        local child = CreateFrame("Frame", "Child")
        -- Authored anchor: relative to the screen (child has no parent) — overridden while scrolled,
        -- and must be exactly what re-applies once the scroll child is cleared.
        child:SetPoint("TOPLEFT", 20, -20)
        child:SetWidth(300); child:SetHeight(600)
        local marker = child:CreateTexture(nil, "ARTWORK")
        marker:SetTexture("marker:child")
        marker:SetAllPoints()  -- templateless Lua regions carry no implicit anchor (decision 1310)

        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    s.resolve();

    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(500.0),
        "vertical=0 -> child top = frame top"
    );

    s.run("SF:SetVerticalScroll(40)").unwrap();
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(540.0),
        "vertical=40 -> child top = frame top + 40 (a positive offset lifts the child)"
    );

    // The range is 600 - 200 = 400; keeping the bar inside it is the FrameXML slider's job.
    s.run("SF:SetVerticalScroll(450)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScroll()").unwrap(),
        450.0,
        "stored verbatim, the range of 400 notwithstanding"
    );
    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(950.0),
        "applied verbatim: frame top 500 + 450"
    );
    s.run("SF:SetVerticalScroll(-30)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScroll()").unwrap(),
        -30.0
    );
    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(470.0),
        "a negative offset lowers the child: frame top 500 - 30"
    );

    // Clearing the child stops the override, and its own untouched anchor applies again.
    s.run("SF:SetScrollChild(nil)").unwrap();
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(580.0),
        "authored anchor survives: screen top 600 - 20"
    );
}

/// `0x786db0` acts only when the offset differs from the stored one; a value past the range is
/// still a change.
#[test]
fn set_vertical_scroll_fires_on_vertical_scroll_only_on_a_change() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        Fires = {}
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        frame:SetScript("OnVerticalScroll", function() table.insert(Fires, arg1) end)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
        frame:SetVerticalScroll(0)      -- already 0: nothing
        frame:SetVerticalScroll(40)     -- a change
        frame:SetVerticalScroll(40)     -- the same again: nothing
        frame:SetVerticalScroll(700)    -- past the range (400): a change, stored as given
        frame:SetVerticalScroll(0)      -- back
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return table.concat(Fires, \",\")")
            .unwrap(),
        "40,700,0"
    );
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
}

#[test]
fn vertical_scroll_range_is_live_and_zero_when_unresolved_or_childless_or_shorter() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        0.0,
        "unresolved -> 0"
    );

    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        400.0,
        "600 - 200"
    );

    s.run("Child:SetWidth(300); Child:SetHeight(150)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        0.0,
        "child shorter than frame -> 0, not negative"
    );

    s.run("SF:SetScrollChild(nil)").unwrap();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        0.0,
        "no child -> 0"
    );
}

/// The range is `child:GetHeight() - self:GetHeight()` in local units, like the offset, which
/// becomes the child's anchor y-offset and is scaled with it.
#[test]
fn vertical_scroll_range_is_local_units_on_a_scaled_frame() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        frame:SetScale(0.5)
        local child = CreateFrame("Frame", "Child", frame)
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        400.0,
        "600 - 200 in the frame's own units, NOT (600 - 200) * 0.5"
    );
    assert_eq!(
        s.eval::<f32>("return Child:GetHeight() - SF:GetHeight()")
            .unwrap(),
        400.0,
        "…which is exactly what the widget contract says it is"
    );
    s.run("SF:SetVerticalScroll(400)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScroll()").unwrap(),
        400.0,
        "the full range is reachable, not clamped away"
    );
    assert_eq!(
        s.eval::<f32>("return Child:GetBottom() - SF:GetBottom()")
            .unwrap(),
        0.0,
        "at the end, the child's bottom sits on the frame's"
    );
}

/// `0x786db0` fires `OnVerticalScroll` with the unclamped stored value `[+0x328]`, under the
/// handler conventions of `0x704f10`.
#[test]
fn vertical_scroll_fires_as_given_and_update_rect_fires_range_changed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)

        seen_v = nil
        frame:SetScript("OnVerticalScroll", function(self, offset)
            seen_v = offset
            assert(self == frame and arg1 == offset, "handler-firing conventions carry the value")
        end)
        seen_lo, seen_hi = nil, nil
        frame:SetScript("OnScrollRangeChanged", function(self, lo, hi)
            seen_lo, seen_hi = lo, hi
        end)
    "#,
    )
    .unwrap();
    s.resolve();

    s.run("SF:SetVerticalScroll(9999)").unwrap();
    assert_eq!(
        s.eval::<f32>("return seen_v").unwrap(),
        9999.0,
        "fires with the value as given — the range (400) is not consulted"
    );

    s.run("SF:UpdateScrollChildRect()").unwrap();
    let (lo, hi): (f32, f32) = s.eval("return seen_lo, seen_hi").unwrap();
    assert_eq!((lo, hi), (0.0, 400.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `UpdateScrollChildRect` resolves for itself, so a resize earlier in the same tick counts; the
/// app resolves only once per frame.
#[test]
fn update_scroll_child_rect_sees_a_same_tick_resize_with_no_intervening_resolve() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(200) -- starts exactly frame-height: range 0
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    s.resolve(); // the "last frame" resolve, before this tick's resize

    s.run(
        r#"
        seen_hi = nil
        SF:SetScript("OnScrollRangeChanged", function(self, lo, hi) seen_hi = hi end)
        Child:SetWidth(300); Child:SetHeight(600) -- grows well past the frame — all in this ONE tick
        SF:UpdateScrollChildRect()
    "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return seen_hi").unwrap(),
        400.0,
        "UpdateScrollChildRect must resolve the JUST-set height itself, not report the stale \
         (pre-resize) range from the last full UiScript::resolve"
    );
    assert_eq!(
        s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap(),
        400.0
    );
}

#[test]
fn extract_clips_the_scroll_childs_whole_subtree_and_leaves_a_sibling_unclipped() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)  -- top 500
        frame:SetWidth(300); frame:SetHeight(200)  -- bottom 300, right 300

        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
        local cm = child:CreateTexture(nil, "ARTWORK")
        cm:SetTexture("marker:child")

        -- a grandchild FRAME (a real arena descendant of the scroll child) with its own region
        local grand = CreateFrame("Frame", "Grand", child)
        grand:SetPoint("TOPLEFT", child, "TOPLEFT", 0, 0)
        grand:SetWidth(50); grand:SetHeight(50)
        local gm = grand:CreateTexture(nil, "ARTWORK")
        gm:SetTexture("marker:grand")

        -- a sibling entirely outside the ScrollFrame
        local sib = CreateFrame("Frame", "Sib")
        sib:SetPoint("TOPLEFT", 0, 0)
        sib:SetWidth(50); sib:SetHeight(50)
        local sm = sib:CreateTexture(nil, "ARTWORK")
        sm:SetTexture("marker:sib")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();

    let sf_rect = Rect::new(300.0, 0.0, 500.0, 300.0);
    assert_eq!(marker_clip(&quads, "marker:child"), Some(Some(sf_rect)));
    assert_eq!(
        marker_clip(&quads, "marker:grand"),
        Some(Some(sf_rect)),
        "a descendant of the scroll child clips too, not just the child itself"
    );
    assert_eq!(
        marker_clip(&quads, "marker:sib"),
        Some(None),
        "a sibling outside the ScrollFrame is unclipped"
    );
}

#[test]
fn nested_scrollframes_intersect_their_clip_rects() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local outer = CreateFrame("ScrollFrame", "Outer")
        outer:SetPoint("TOPLEFT", 0, -50)   -- top 550
        outer:SetWidth(400); outer:SetHeight(300)  -- bottom 250, right 400
        local om = outer:CreateTexture(nil, "ARTWORK"); om:SetTexture("marker:outer")

        local c1 = CreateFrame("Frame", "C1")
        c1:SetWidth(400); c1:SetHeight(1000)
        outer:SetScrollChild(c1)
        local c1m = c1:CreateTexture(nil, "ARTWORK"); c1m:SetTexture("marker:c1")

        -- The inner ScrollFrame is a real arena child of C1 (so walking its ancestors reaches C1,
        -- Outer's registered scroll child) and sits far enough right that its own rect pokes past
        -- Outer's right edge — an intersection distinct from either rect alone.
        local inner = CreateFrame("ScrollFrame", "Inner", c1)
        inner:SetPoint("TOPLEFT", "C1", "TOPLEFT", 350, -30)
        inner:SetWidth(200); inner:SetHeight(150)
        local im = inner:CreateTexture(nil, "ARTWORK"); im:SetTexture("marker:inner")

        local c2 = CreateFrame("Frame", "C2", inner)
        c2:SetWidth(200); c2:SetHeight(500)
        inner:SetScrollChild(c2)
        local leaf = c2:CreateTexture(nil, "ARTWORK"); leaf:SetTexture("marker:leaf")
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();

    let outer_rect = Rect::new(250.0, 0.0, 550.0, 400.0);
    // Inner's own rect: TOPLEFT at C1's (350, -30), so top 520 and left 350, sized 200x150.
    let inner_rect = Rect::new(370.0, 350.0, 520.0, 550.0);
    let intersection = Rect::new(
        inner_rect.bottom.max(outer_rect.bottom),
        inner_rect.left.max(outer_rect.left),
        inner_rect.top.min(outer_rect.top),
        inner_rect.right.min(outer_rect.right),
    );
    // Outer cuts inner's right edge from 550 to 400; inner's other edges are tighter.
    assert_eq!(intersection, Rect::new(370.0, 350.0, 520.0, 400.0));

    assert_eq!(
        marker_clip(&quads, "marker:outer"),
        Some(None),
        "Outer itself is not clipped"
    );
    assert_eq!(
        marker_clip(&quads, "marker:c1"),
        Some(Some(outer_rect)),
        "Outer's own registered child clips to Outer's rect"
    );
    assert_eq!(
        marker_clip(&quads, "marker:inner"),
        Some(Some(outer_rect)),
        "Inner (the widget itself) lives inside C1's clipped content -> clipped by Outer too"
    );
    assert_eq!(
        marker_clip(&quads, "marker:leaf"),
        Some(Some(intersection)),
        "Inner's own scroll child intersects both ScrollFrames' rects"
    );
}

#[test]
fn hit_test_denies_a_button_clipped_out_and_admits_it_once_scrolled_into_view() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)  -- top 500
        frame:SetWidth(300); frame:SetHeight(200)  -- bottom 300
        frame:EnableMouse(false)            -- isolate the button's own clip-gated hit (§5's subject)

        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)

        local btn = CreateFrame("Button", "Btn", child)
        btn:SetPoint("TOPLEFT", "Child", "TOPLEFT", 0, -400)
        btn:SetWidth(50); btn:SetHeight(20)
    "#,
    )
    .unwrap();
    s.resolve();

    // At 0 the button spans 80..100, outside the frame's clip of 300..500.
    assert_eq!(
        s.hit_test(25.0, 90.0),
        None,
        "the button's own rect contains the point, but the ScrollFrame's clip excludes it"
    );

    // Scrolled by 250 the button spans 330..350, inside the clip.
    s.run("SF:SetVerticalScroll(250)").unwrap();
    s.resolve();
    assert!(
        s.hit_test(25.0, 340.0).is_some(),
        "scrolled into view, the button hits again"
    );
}

/// The reference measures text synchronously, as stock `QuestLog_UpdateQuestDetails` relies on.
#[test]
fn update_scroll_child_rect_measures_the_childs_text_before_it_reads_the_range() {
    struct Rows;
    impl crate::script::TextMeasure for Rows {
        fn measure(&mut self, req: &crate::script::MeasureRequest) -> (f32, f32, f32) {
            let natural = req.text.chars().count() as f32 * 7.0;
            match req.wrap_width {
                Some(w) if w > 0.0 && natural > w => (w, (natural / w).ceil() * 14.0, natural),
                _ => (natural, 14.0, natural),
            }
        }
    }
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.set_text_measurer(Box::new(Rows));
    s.run(
        r#"
        local sf = CreateFrame("ScrollFrame", "SF", UIParent)
        sf:SetPoint("TOPLEFT", 0, 0); sf:SetWidth(100); sf:SetHeight(50)
        local child = CreateFrame("Frame", "SFChild", sf)
        child:SetWidth(100); child:SetHeight(50); sf:SetScrollChild(child)
        local fs = child:CreateFontString("SFText", "ARTWORK")
        fs:SetPoint("TOPLEFT", 0, 0); fs:SetWidth(70)
        sf:SetScript("OnScrollRangeChanged", function() RANGE = arg2 end)
        fs:SetText(string.rep("x", 40))   -- 280px of ink over a 70px column: four 14px rows
        sf:UpdateScrollChildRect()
        "#,
    )
    .unwrap();
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f64>("return RANGE").unwrap(),
        6.0,
        "56px of measured text under a 50px frame: the range is known at the call, not a frame later"
    );
}

/// The horizontal axis clones the vertical (`0x786d30` against `0x786db0`); `0x787100` passes both
/// offsets to `SetPoint(TOPLEFT, self, TOPLEFT, +h, +v)` unnegated, so a positive horizontal
/// offset pushes the child right.
#[test]
fn scroll_child_left_tracks_horizontal_scroll_unclamped_and_fires_on_change() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);

    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "HSF")
        frame:SetPoint("TOPLEFT", 0, -100)   -- frame left 0, top 500
        frame:SetWidth(300); frame:SetHeight(200)  -- frame right 300

        local child = CreateFrame("Frame", "HChild")
        child:SetPoint("TOPLEFT", 20, -20)
        child:SetWidth(500); child:SetHeight(200)
        local marker = child:CreateTexture(nil, "ARTWORK")
        marker:SetTexture("marker:hchild")
        marker:SetAllPoints()

        frame:SetScrollChild(child)
        hfired = {}
        frame:SetScript("OnHorizontalScroll", function() table.insert(hfired, arg1) end)
    "#,
    )
    .unwrap();
    s.resolve();

    assert_eq!(
        s.eval::<f32>("return HSF:GetHorizontalScroll()").unwrap(),
        0.0,
        "a fresh ScrollFrame is at 0 on both axes"
    );
    assert_eq!(
        marker_rect(&s.extract(), "marker:hchild").map(|r| r.left),
        Some(0.0),
        "horizontal=0 -> child left = frame left"
    );

    s.run("HSF:SetHorizontalScroll(40)").unwrap();
    s.resolve();
    assert_eq!(
        marker_rect(&s.extract(), "marker:hchild").map(|r| r.left),
        Some(40.0),
        "a positive horizontal offset moves the child right"
    );
    s.run("HSF:SetHorizontalScroll(-60)").unwrap();
    s.resolve();
    assert_eq!(
        marker_rect(&s.extract(), "marker:hchild").map(|r| r.left),
        Some(-60.0),
        "a negative horizontal offset scrolls right"
    );

    // Neither `0x786d30` nor `0x786db0` reads the range, so 450 is stored whole.
    assert_eq!(
        s.eval::<f32>("return HSF:GetHorizontalScrollRange()")
            .unwrap(),
        200.0,
        "range = subtree width - frame width"
    );
    s.run("HSF:SetHorizontalScroll(450)").unwrap();
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return HSF:GetHorizontalScroll()").unwrap(),
        450.0,
        "stored verbatim, past the range"
    );

    assert_eq!(s.arity("HSF:GetHorizontalScroll()").unwrap(), 1);
    assert_eq!(s.arity("HSF:GetHorizontalScrollRange()").unwrap(), 1);
    assert_eq!(
        s.eval::<String>("return type(HSF:GetHorizontalScroll())")
            .unwrap(),
        "number"
    );
    assert_eq!(
        s.eval::<String>("return type(HSF:GetHorizontalScrollRange())")
            .unwrap(),
        "number"
    );
    assert_eq!(
        s.arity("HSF:SetHorizontalScroll(450)").unwrap(),
        0,
        "the setter answers nothing"
    );

    // Four setter calls, but the second 450 is no change.
    let fired: Vec<f32> = (1..=s.eval::<i64>("return table.getn(hfired)").unwrap())
        .map(|i| s.eval::<f32>(&format!("return hfired[{i}]")).unwrap())
        .collect();
    assert_eq!(
        fired,
        vec![40.0, -60.0, 450.0],
        "OnHorizontalScroll fires once per ACTUAL change, with the new offset"
    );

    s.run("HSF:SetVerticalScroll(25)").unwrap();
    assert_eq!(
        s.eval::<f32>("return HSF:GetHorizontalScroll()").unwrap(),
        450.0
    );
    assert_eq!(
        s.eval::<f32>("return HSF:GetVerticalScroll()").unwrap(),
        25.0
    );
}

/// The vertical range leaves `0x786e30` on top of the x87 stack and is `fstp`'d first, so it is
/// pushed deepest and lands as `arg2`.
#[test]
fn update_scroll_child_rect_reports_both_ranges_horizontal_first() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "BSF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "BChild")
        child:SetPoint("TOPLEFT", 0, 0)
        child:SetWidth(500); child:SetHeight(600)
        frame:SetScrollChild(child)
        seen = nil
        frame:SetScript("OnScrollRangeChanged", function() seen = { arg1, arg2 } end)
    "#,
    )
    .unwrap();
    s.resolve();
    s.run("BSF:UpdateScrollChildRect()").unwrap();
    assert_eq!(
        s.eval::<f32>("return seen[1]").unwrap(),
        200.0,
        "arg1 is the HORIZONTAL range (500 - 300)"
    );
    assert_eq!(
        s.eval::<f32>("return seen[2]").unwrap(),
        400.0,
        "arg2 is the VERTICAL range (600 - 200)"
    );
    assert!(s.take_errors().is_empty());
}
