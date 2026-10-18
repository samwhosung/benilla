//! ScrollFrame: the scroll-child layout override, the stored scroll ranges, the clip on `extract`
//! and the clip-gated `hit_test`.

use super::common::script;
use crate::layout::Rect;
use crate::script::{QuadContent, UiScript};

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
        marker:SetAllPoints()  -- templateless Lua regions carry no implicit anchor

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

    // Clearing the child leaves its one point alone and invalidates nothing (`0x786ce6`): it
    // stays at the scroll frame's corner, and follows the frame when the frame moves.
    s.run("SF:SetScrollChild(nil)").unwrap();
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        marker_rect(&quads, "marker:child").map(|r| r.top),
        Some(470.0),
        "the re-anchor's point stands: frame top 500 - 30"
    );
    s.run(r#"SF:SetPoint("TOPLEFT", 0, -150)"#).unwrap();
    s.resolve();
    assert_eq!(
        marker_rect(&s.extract(), "marker:child").map(|r| r.top),
        Some(420.0),
        "still on the frame's corner: frame top 450 - 30"
    );
}

/// The re-anchor (`0x787100`) is a real write on the child: its points are cleared (`0x787110`)
/// and one TOPLEFT on the scroll frame's TOPLEFT is set at the offsets (`0x787136`), at
/// `SetScrollChild` and at each offset change; the authored points are gone.
#[test]
fn the_scroll_child_holds_one_topleft_on_the_scroll_frame() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        sf = CreateFrame("ScrollFrame", "SF")
        sf:SetPoint("TOPLEFT", 0, -100); sf:SetWidth(300); sf:SetHeight(200)
        c = CreateFrame("Frame", "Child")
        c:SetPoint("CENTER", UIParent); c:SetPoint("BOTTOM", 0, 10)
        c:SetWidth(100); c:SetHeight(400)
        sf:SetScrollChild(c)
    "#,
    )
    .unwrap();
    let point = |s: &mut UiScript| {
        s.eval::<(i64, String, bool, String, f64, f64)>(
            "local p, r, rp, x, y = c:GetPoint(1); return c:GetNumPoints(), p, r == sf, rp, x, y",
        )
        .unwrap()
    };
    assert_eq!(
        point(&mut s),
        (1, "TOPLEFT".into(), true, "TOPLEFT".into(), 0.0, 0.0)
    );
    s.run("sf:SetVerticalScroll(30); sf:SetHorizontalScroll(-12)")
        .unwrap();
    assert_eq!(
        point(&mut s),
        (1, "TOPLEFT".into(), true, "TOPLEFT".into(), -12.0, 30.0),
        "scrolling moves the one point"
    );
    s.resolve();
    assert_eq!(s.eval::<f64>("return c:GetTop()").unwrap(), 530.0);
    assert_eq!(s.eval::<f64>("return c:GetLeft()").unwrap(), -12.0);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
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

/// The recompute measures `max(0, child − frame)`, and leaves the stored range alone when the
/// frame has no rect (`0x787186`) or no scroll child (`0x786e42`).
#[test]
fn the_recompute_measures_the_overhang_and_skips_an_unresolved_or_childless_frame() {
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
    let range =
        |s: &crate::script::UiScript| s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap();
    assert_eq!(range(&s), 0.0, "the ctor's zero");

    s.resolve();
    s.tick(0.016);
    assert_eq!(range(&s), 400.0, "600 - 200");

    s.run("Child:SetHeight(150) SF:UpdateScrollChildRect()")
        .unwrap();
    s.tick(0.016);
    assert_eq!(range(&s), 0.0, "a shorter child: 0, not negative");

    s.run("Child:SetHeight(600) SF:UpdateScrollChildRect()")
        .unwrap();
    s.tick(0.016);
    assert_eq!(range(&s), 400.0);
    s.run("SF:SetScrollChild(nil)").unwrap();
    s.tick(0.016);
    assert_eq!(range(&s), 400.0, "no child: the stored range stands");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
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
    s.tick(0.016);
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
fn vertical_scroll_fires_as_given_and_the_marked_range_fires_range_changed() {
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
        frame:SetScript("OnVerticalScroll", function() local self, offset = this, arg1
            seen_v = offset
            assert(self == frame and arg1 == offset, "handler-firing conventions carry the value")
        end)
        seen_lo, seen_hi = nil, nil
        frame:SetScript("OnScrollRangeChanged", function() local self, lo, hi = this, arg1, arg2
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
    s.tick(0.016);
    let (lo, hi): (f32, f32) = s.eval("return seen_lo, seen_hi").unwrap();
    assert_eq!((lo, hi), (0.0, 400.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The pump drains the layout before its walk (`0x7656ba`), so a resize made since the last tick
/// counts in this tick's recompute, with no resolve of the host's in between.
#[test]
fn the_recompute_sees_a_same_tick_resize_with_no_intervening_resolve() {
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
    s.resolve();
    s.tick(0.016); // the first recompute: 0 stays 0, nothing fires

    s.run(
        r#"
        seen_hi = nil
        SF:SetScript("OnScrollRangeChanged", function() local self, lo, hi = this, arg1, arg2 seen_hi = hi end)
        Child:SetWidth(300); Child:SetHeight(600) -- grows well past the frame — all in this ONE tick
        SF:UpdateScrollChildRect()
    "#,
    )
    .unwrap();
    s.tick(0.016);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
    assert_eq!(
        s.eval::<f32>("return seen_hi").unwrap(),
        400.0,
        "the drain before the walk resolves the JUST-set height, so the recompute does not \
         report the stale (pre-resize) range from the last full UiScript::resolve"
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
        frame:EnableMouse(false)            -- isolate the button's own clip-gated hit

        -- Under its ScrollFrame, as a `<ScrollChild>` is: its rect is the hit clip.
        local child = CreateFrame("Frame", "Child", frame)
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

/// The probe `0x76b020` climbs a scroll subtree's parent chain to the first frame without the
/// scroll flag and clips to its rect; when the chain runs out first (`0x76b063`), as it does for a
/// scroll child with no parent, nothing in the subtree is hit, though it draws.
#[test]
fn a_parentless_scroll_childs_subtree_takes_no_hit() {
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
        local btn = CreateFrame("Button", "Btn", child)
        btn:SetPoint("TOPLEFT", "Child", "TOPLEFT", 0, -10)
        btn:SetWidth(50); btn:SetHeight(20)
        local t = btn:CreateTexture(nil, "ARTWORK"); t:SetTexture("marker:btn"); t:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        marker_clip(&s.extract(), "marker:btn"),
        Some(Some(Rect::new(300.0, 0.0, 500.0, 300.0))),
        "it draws, clipped to the ScrollFrame"
    );
    assert_eq!(s.hit_test(25.0, 480.0), None);
    // Parented under the ScrollFrame, the same button hits.
    s.run("Child:SetParent(SF) SF:SetScrollChild(Child)")
        .unwrap();
    s.resolve();
    assert_eq!(s.hit_test_name(25.0, 480.0).as_deref(), Some("Btn"));
}

/// The hit clip is one rect, the first unflagged ancestor's (`0x76b069`): inside a nested
/// ScrollFrame, that is the outer ScrollFrame, so a button past the inner one's edge but inside
/// the outer one's is hit where it is not drawn. The draw clip intersects both (`0x787220`).
#[test]
fn a_nested_scroll_childs_hit_clips_to_the_outer_scroll_frame_alone() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local outer = CreateFrame("ScrollFrame", "Outer")
        outer:SetPoint("TOPLEFT", 0, 0)
        outer:SetWidth(400); outer:SetHeight(400)   -- top 600, bottom 200
        outer:EnableMouse(false)
        local c1 = CreateFrame("Frame", "C1", outer)
        c1:SetWidth(400); c1:SetHeight(400)
        outer:SetScrollChild(c1)
        local inner = CreateFrame("ScrollFrame", "Inner", c1)
        inner:SetPoint("TOPLEFT", "C1", "TOPLEFT", 0, 0)
        inner:SetWidth(100); inner:SetHeight(100)   -- x 0..100, y 500..600
        local c2 = CreateFrame("Frame", "C2", inner)
        c2:SetWidth(300); c2:SetHeight(300)
        inner:SetScrollChild(c2)
        local btn = CreateFrame("Button", "Btn", c2)
        btn:SetPoint("TOPLEFT", "C2", "TOPLEFT", 150, -10)  -- x 150..200, y 570..590
        btn:SetWidth(50); btn:SetHeight(20)
        local t = btn:CreateTexture(nil, "ARTWORK"); t:SetTexture("marker:btn"); t:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        marker_clip(&s.extract(), "marker:btn"),
        Some(Some(Rect::new(500.0, 0.0, 600.0, 100.0))),
        "drawn inside the inner ScrollFrame only"
    );
    assert_eq!(s.hit_test_name(175.0, 580.0).as_deref(), Some("Btn"));
    assert_eq!(
        s.hit_test(175.0, 100.0),
        None,
        "outside the outer ScrollFrame"
    );
}

/// The callback clips to the ScrollFrame's `[+0xd8]` (`0x76b220`), its mouse rect, which its
/// hit-rect insets shrink (`0x76b580`).
#[test]
fn the_draw_clip_is_the_scroll_frames_mouse_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        frame:SetHitRectInsets(10, 20, 30, 40)
        local child = CreateFrame("Frame", "Child", frame)
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
        local t = child:CreateTexture(nil, "ARTWORK")
        t:SetTexture("marker:child"); t:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    assert_eq!(
        marker_clip(&s.extract(), "marker:child"),
        Some(Some(Rect::new(340.0, 10.0, 470.0, 280.0)))
    );
}

/// A nested ScrollFrame wholly outside its outer one leaves a clip with no positive span, and the
/// callback then draws nothing (`0x78750e`–`0x787526`), not the strip between the two rects.
#[test]
fn a_nested_scroll_frame_outside_its_outer_one_draws_no_child() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local outer = CreateFrame("ScrollFrame", "Outer")
        outer:SetPoint("TOPLEFT", 0, 0)
        outer:SetWidth(200); outer:SetHeight(200)   -- x 0..200, y 400..600
        local c1 = CreateFrame("Frame", "C1", outer)
        c1:SetWidth(600); c1:SetHeight(600)
        outer:SetScrollChild(c1)
        local inner = CreateFrame("ScrollFrame", "Inner", c1)
        inner:SetPoint("TOPLEFT", "C1", "TOPLEFT", 300, 0)  -- x 300..400: right of Outer
        inner:SetWidth(100); inner:SetHeight(100)
        local im = inner:CreateTexture(nil, "ARTWORK"); im:SetTexture("marker:inner")
        im:SetAllPoints()
        local c2 = CreateFrame("Frame", "C2", inner)
        c2:SetWidth(100); c2:SetHeight(100)
        inner:SetScrollChild(c2)
        local leaf = c2:CreateTexture(nil, "ARTWORK"); leaf:SetTexture("marker:leaf")
        leaf:SetAllPoints()
    "#,
    )
    .unwrap();
    s.resolve();
    let quads = s.extract();
    assert_eq!(
        marker_clip(&quads, "marker:inner"),
        Some(Some(Rect::new(400.0, 0.0, 600.0, 200.0))),
        "the inner ScrollFrame itself is Outer's content, clipped away by Outer's rect"
    );
    assert_eq!(
        marker_clip(&quads, "marker:leaf"),
        None,
        "its child is not drawn"
    );
}

/// The probe reads the unflagged ancestor's `[+0xd8]` without its valid bit (`0x76b069`): under
/// an ancestor that never resolved, that is the ctor's zero rect, which only the origin is in.
#[test]
fn a_hit_under_an_unplaced_ancestor_clips_to_the_zero_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local holder = CreateFrame("Frame", "Holder")   -- no points: never resolves
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("BOTTOMLEFT", 0, 0)
        frame:SetWidth(100); frame:SetHeight(100)
        local child = CreateFrame("Frame", "Child", holder)
        child:SetWidth(100); child:SetHeight(100)
        frame:SetScrollChild(child)
        local btn = CreateFrame("Button", "Btn", child)
        btn:SetPoint("BOTTOMLEFT", "SF", "BOTTOMLEFT", 0, 0)
        btn:SetWidth(50); btn:SetHeight(50)
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(
        s.eval::<bool>("return Holder:GetLeft() == nil").unwrap(),
        "the holder has no rect"
    );
    assert_eq!(s.hit_test_name(0.0, 0.0).as_deref(), Some("Btn"));
    assert_eq!(s.hit_test(10.0, 10.0), None);
}

/// A ScrollFrame whose rect does not resolve draws no scroll child (`0x76b220` fails at
/// `0x787253`), though the child itself resolves.
#[test]
fn a_scroll_frame_with_no_rect_draws_no_scroll_child() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local base = CreateFrame("Frame", "Base")
        base:SetPoint("TOPLEFT", 0, 0); base:SetWidth(10); base:SetHeight(10)
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child", frame)
        child:SetWidth(300); child:SetHeight(600)
        local t = child:CreateTexture(nil, "ARTWORK")
        t:SetTexture("marker:child"); t:SetAllPoints()
        frame:SetScrollChild(child)
        -- The re-anchor's TOPLEFT on the unplaced ScrollFrame, replaced by a placed one.
        child:ClearAllPoints(); child:SetPoint("TOPLEFT", "Base", "TOPLEFT")
    "#,
    )
    .unwrap();
    s.resolve();
    assert!(
        s.eval::<bool>("return SF:GetLeft() == nil and Child:GetLeft() ~= nil")
            .unwrap(),
        "the ScrollFrame has no rect and its child has one"
    );
    assert_eq!(marker_clip(&s.extract(), "marker:child"), None);
}

/// The reference measures text synchronously and drains the layout before the walk (`0x7656ba`),
/// as stock `QuestLog_UpdateQuestDetails` relies on: the text set before the tick is in the range.
#[test]
fn the_recompute_measures_the_childs_text_before_it_reads_the_range() {
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
        local fs = child:CreateFontString("SFText", "ARTWORK"); fs:SetFont("Fonts\\FRIZQT__.TTF", 12)
        fs:SetPoint("TOPLEFT", 0, 0); fs:SetWidth(70)
        sf:SetScript("OnScrollRangeChanged", function() RANGE = arg2 end)
        fs:SetText(string.rep("x", 40))   -- 280px of ink over a 70px column: four 14px rows
        sf:UpdateScrollChildRect()
        "#,
    )
    .unwrap();
    s.tick(0.016);
    assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
    assert_eq!(
        s.eval::<f64>("return RANGE").unwrap(),
        6.0,
        "56px of measured text under a 50px frame: the recompute measures it, not a frame later"
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
    s.tick(0.016);

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
fn the_range_notify_reports_both_ranges_horizontal_first() {
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
    s.tick(0.016);
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

// ── The stored ranges (`[+0x31c]`, `[+0x320]`), recomputed in the frame's walk slot ──────────

/// A shown 300×200 scroll frame `SF` and a 600-tall `Child`, not yet set; `FIRES` counts
/// `OnScrollRangeChanged` and `RANGE` keeps its last `(xRange, yRange)`.
const RANGED: &str = r#"
    FIRES, RANGE = 0, { -1, -1 }
    local frame = CreateFrame("ScrollFrame", "SF")
    frame:SetPoint("TOPLEFT", 0, -100)
    frame:SetWidth(300); frame:SetHeight(200)
    frame:SetScript("OnScrollRangeChanged", function()
        FIRES = FIRES + 1
        RANGE = { arg1, arg2 }
    end)
    local child = CreateFrame("Frame", "Child", frame)
    child:SetWidth(300); child:SetHeight(600)
"#;

/// `(FIRES, yRange as fired, GetVerticalScrollRange())`.
fn ranged(s: &crate::script::UiScript) -> (u32, f32, f32) {
    s.eval("return FIRES, RANGE[2], SF:GetVerticalScrollRange()")
        .unwrap()
}

/// `UpdateScrollChildRect` only marks the range (`0x791943`); the frame's update (`0x787140`)
/// recomputes a marked range after its `OnUpdate` (`0x787152`–`0x7871a0`) and fires
/// `OnScrollRangeChanged` only when a range moved by 2⁻²² or more (`0x786ed7`–`0x786efd`).
#[test]
fn the_range_is_recomputed_in_the_walk_and_fires_once_per_change() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(RANGED).unwrap();
    s.run("SF:SetScrollChild(Child)").unwrap();
    s.resolve();
    assert_eq!(ranged(&s), (0, -1.0, 0.0), "nothing until the walk");
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0), "SetScrollChild marked it");

    s.run("SF:UpdateScrollChildRect()").unwrap();
    assert_eq!(ranged(&s).0, 1, "the call fires nothing");
    s.tick(0.016);
    assert_eq!(
        ranged(&s),
        (1, 400.0, 400.0),
        "an unchanged range fires nothing"
    );

    s.run("Child:SetHeight(700) SF:UpdateScrollChildRect()")
        .unwrap();
    assert_eq!(ranged(&s), (1, 400.0, 400.0), "stored until the walk");
    s.tick(0.016);
    assert_eq!(ranged(&s), (2, 500.0, 500.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The getters answer the stored value (`0x7917d0`, `[+0x320]` at `0x791873`), which the ctor
/// zeroes (`0x786a63`) and only a marked recompute moves: a child resized with no mark leaves it.
#[test]
fn the_getters_answer_the_stored_range() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(RANGED).unwrap();
    s.run("SF:SetScrollChild(Child)").unwrap();
    s.resolve();
    assert_eq!(
        ranged(&s).2,
        0.0,
        "the ctor's zero before the first recompute"
    );
    s.tick(0.016);
    assert_eq!(ranged(&s).2, 400.0);
    s.run("Child:SetHeight(900)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(
        ranged(&s),
        (1, 400.0, 400.0),
        "a child's resize marks nothing"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The update runs only for a shown frame, so a hidden one keeps its range and fires nothing, and
/// the mark waits for the show; a resize while hidden is marked the same way.
#[test]
fn a_hidden_scroll_frame_keeps_its_range_until_shown() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(RANGED).unwrap();
    s.run("SF:SetScrollChild(Child)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0));

    s.run("SF:Hide() Child:SetHeight(700) SF:UpdateScrollChildRect()")
        .unwrap();
    s.resolve();
    s.tick(0.016);
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0), "hidden: stale, and silent");
    s.run("SF:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(
        ranged(&s),
        (2, 500.0, 500.0),
        "the mark held until the show"
    );

    s.run("SF:Hide() SF:SetHeight(300)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s).0, 2);
    s.run("SF:Show()").unwrap();
    s.tick(0.016);
    assert_eq!(
        ranged(&s),
        (3, 400.0, 400.0),
        "the resize while hidden marked it"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A change in the frame's own size marks the range (`0x7871f0`, `0x787205`), with no
/// `UpdateScrollChildRect`: a frame whose code never calls it still gets its range.
#[test]
fn a_resize_marks_the_range() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(RANGED).unwrap();
    s.run("SF:SetScrollChild(Child)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0));
    s.run("SF:SetHeight(300)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (2, 300.0, 300.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The mark rides the frame's `OnSizeChanged` override, behind `ApplyRect`'s gate (`0x76b5e7`,
/// `0x76b602`), which is 2⁻²² in the reference's units, the screen diagonal as 1: a jitter under it
/// marks nothing, so a child's unmarked resize stays unreported until a real resize.
#[test]
fn a_sub_threshold_jitter_does_not_mark_the_range() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(RANGED).unwrap();
    s.run("SF:SetScrollChild(Child)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0));
    s.run("Child:SetHeight(700) SF:SetHeight(200.0001)")
        .unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (1, 400.0, 400.0), "a jitter marks nothing");
    s.run("SF:SetHeight(201)").unwrap();
    s.resolve();
    s.tick(0.016);
    assert_eq!(ranged(&s), (2, 499.0, 499.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

// ── The range in the scroll frame's scale, and the offset tolerance ─────────────────────────

/// The vertical range of `SF` after a recompute.
fn y_range(s: &mut UiScript) -> f32 {
    s.resolve();
    s.tick(0.016);
    s.eval::<f32>("return SF:GetVerticalScrollRange()").unwrap()
}

/// `0x786e30` takes the content box less the frame's size and multiplies it by the reciprocal of
/// the scroll frame's own scale (`+0x7c`, `0x786e9f`); the child's scale is never read. A
/// half-scale 600-tall child is 300 tall on screen, 100 over a 200-tall unscaled frame.
#[test]
fn the_range_is_in_the_scroll_frames_scale_alone() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        child:SetScale(0.5)
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    assert_eq!(y_range(&mut s), 100.0, "(300 - 200) / 1");

    // A parentless child keeps scale 1 under a double-scale frame: (600 - 400) / 2.
    s.run("Child:SetScale(1) SF:SetScale(2) SF:UpdateScrollChildRect()")
        .unwrap();
    assert_eq!(y_range(&mut s), 100.0, "(600 - 400) / 2");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `0x786f80` joins a child frame by its own shown bit (`+0xd0`, `0x7870e0`) and the scroll child
/// itself with no shown test (`0x786fb0`), so once laid out, a hidden scroll child's shown tall
/// descendant still makes a range (a hidden frame's layout is deferred, its rect kept).
#[test]
fn a_hidden_scroll_childs_shown_descendant_counts() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(100)
        local tall = CreateFrame("Frame", "Tall", child)
        tall:SetPoint("TOPLEFT", child, "TOPLEFT", 0, 0)
        tall:SetWidth(300); tall:SetHeight(600)
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    assert_eq!(y_range(&mut s), 400.0, "600 - 200");
    s.run("Child:Hide() SF:UpdateScrollChildRect()").unwrap();
    assert_eq!(y_range(&mut s), 400.0, "through the hidden child");
    s.run("Tall:Hide() SF:UpdateScrollChildRect()").unwrap();
    assert_eq!(y_range(&mut s), 0.0, "a hidden descendant adds nothing");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// `0x786db0`/`0x786d30` drop a change under 2^-22 in the layout's units (`0x786dc2`), storing and
/// firing nothing, and store and fire a NaN every time (the unordered compare, `0x786dcd`).
#[test]
fn an_offset_change_under_the_tolerance_is_dropped_and_nan_always_fires() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        V, H = 0, 0
        local frame = CreateFrame("ScrollFrame", "SF")
        frame:SetPoint("TOPLEFT", 0, -100)
        frame:SetWidth(300); frame:SetHeight(200)
        frame:SetScript("OnVerticalScroll", function() V = V + 1 end)
        frame:SetScript("OnHorizontalScroll", function() H = H + 1 end)
        local child = CreateFrame("Frame", "Child")
        child:SetWidth(300); child:SetHeight(600)
        frame:SetScrollChild(child)
    "#,
    )
    .unwrap();
    // 2^-22 times the layout unit of an 800x600 screen, 768 * 5/3.
    let eps = crate::layout::SIZE_EPS * 1280.0;
    for (set, get, count) in [
        ("SetVerticalScroll", "GetVerticalScroll", "V"),
        ("SetHorizontalScroll", "GetHorizontalScroll", "H"),
    ] {
        s.run(&format!("SF:{set}(10)")).unwrap();
        s.run(&format!("SF:{set}({})", 10.0 + eps / 2.0)).unwrap();
        assert_eq!(s.eval::<f64>(&format!("return {count}")).unwrap(), 1.0);
        assert_eq!(
            s.eval::<f64>(&format!("return SF:{get}()")).unwrap(),
            10.0,
            "{set}: the sub-tolerance change is not stored"
        );
        s.run(&format!("SF:{set}({})", 10.0 + eps * 2.0)).unwrap();
        assert_eq!(s.eval::<f64>(&format!("return {count}")).unwrap(), 2.0);
        s.run(&format!("SF:{set}(0/0) SF:{set}(0/0)")).unwrap();
        assert_eq!(
            s.eval::<f64>(&format!("return {count}")).unwrap(),
            4.0,
            "{set}: NaN fires each time"
        );
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
