//! A layout object's destructor (`0x767720` → `0x768e20`) unhooks each object anchored to it with
//! `0x767e70(dependent, dying, 0)`: every anchor slot naming the dying object is freed, and the
//! zero flag skips the invalidate (`0x7680e0`). So `GetNumPoints` (`0x7a2260`) drops and `GetPoint`
//! no longer answers those points, while the dependent keeps its rect until its own next
//! invalidate. A region dies to `Button:SetFontString` (`0x778d3c`), a state-texture setter
//! (`0x77900a`), a backdrop teardown (`0x77f4d0`) and a SimpleHTML re-parse (`0x78a5aa`).

use super::common::script;
use crate::script::UiScript;

/// Resolve, then require the cached graph and its rects to equal a from-scratch solve.
fn resolve_matches_scratch(s: &mut UiScript, what: &str) {
    s.resolve();
    let mut model = s.model_mut();
    if let Some(why) = UiScript::scratch_divergence(&mut model) {
        panic!("{what}: {why}");
    }
}

fn num(s: &mut UiScript, expr: &str) -> Option<f64> {
    s.eval::<Option<f64>>(&format!("return {expr}")).unwrap()
}

/// The issue's sequence: a texture anchored to a button's label and to the button; the label
/// replaced. The anchor to the destroyed label is gone, the other stands, and the texture keeps
/// its rect, which the anchor that is left would move, until its own invalidate.
#[test]
fn a_replaced_label_leaves_its_dependents_anchors() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button", "B")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        b:SetWidth(200); b:SetHeight(40)
        b:SetFontString(b:CreateFontString("Old", "ARTWORK"))
        Old:SetWidth(50); Old:SetHeight(10)
        t = b:CreateTexture("T", "OVERLAY")
        t:SetTexture(1, 1, 1); t:SetWidth(10); t:SetHeight(10)
        t:SetPoint("LEFT", b:GetFontString(), "RIGHT")
        t:SetPoint("TOP", b, "TOP")
    "#,
    )
    .unwrap();
    s.resolve();
    crate::assert_near!(s.eval::<i64>("return t:GetNumPoints()").unwrap(), 2);
    crate::assert_near!(
        num(&mut s, "t:GetLeft()"),
        Some(225.0),
        "right of the label"
    );

    s.run(r#"b:SetFontString(b:CreateFontString("New", "ARTWORK"))"#)
        .unwrap();
    crate::assert_near!(
        s.eval::<i64>("return t:GetNumPoints()").unwrap(),
        1,
        "the anchor to the destroyed label is freed"
    );
    crate::assert_near!(
        s.eval::<i64>("return table.getn({ t:GetPoint(2) })")
            .unwrap(),
        0,
        "and no second point answers"
    );
    assert!(s
        .eval::<bool>(
            r#"local p, r, rp = t:GetPoint(1); return p == "TOP" and r == b and rp == "TOP""#
        )
        .unwrap());

    resolve_matches_scratch(&mut s, "the label replaced");
    crate::assert_near!(
        num(&mut s, "t:GetLeft()"),
        Some(225.0),
        "the unhook queued no resolve"
    );
    // Its own invalidate resolves it from the anchor left: centred under the button's top.
    s.run("t:SetWidth(12)").unwrap();
    resolve_matches_scratch(&mut s, "the texture's own invalidate");
    crate::assert_near!(num(&mut s, "t:GetLeft()"), Some(194.0));
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A texture and a frame each anchored to a backdrop piece and to the frame: the teardown frees
/// the piece's anchors only, and both keep their rects.
#[test]
fn a_backdrop_teardown_frees_the_anchors_to_its_pieces() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        f = CreateFrame("Frame")
        f:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        f:SetWidth(100); f:SetHeight(100)
        f:SetBackdrop({ bgFile = "bg", edgeFile = "edge" })
        local piece = ({ f:GetRegions() })[1]
        local g = CreateFrame("Frame")
        dep = g:CreateTexture(nil, "ARTWORK")
        dep:SetTexture("dep"); dep:SetWidth(10); dep:SetHeight(10)
        dep:SetPoint("TOPLEFT", piece, "TOPLEFT", 5, -5)
        dep:SetPoint("BOTTOM", f, "BOTTOM", 0, 20)
        h = CreateFrame("Frame")
        h:SetWidth(10); h:SetHeight(10)
        h:SetPoint("TOPLEFT", piece, "TOPLEFT", 3, -3)
        h:SetPoint("BOTTOM", f, "BOTTOM", 0, 30)
    "#,
    )
    .unwrap();
    s.resolve();
    let before = (
        num(&mut s, "dep:GetLeft()"),
        num(&mut s, "dep:GetTop()"),
        num(&mut s, "h:GetLeft()"),
        num(&mut s, "h:GetTop()"),
    );
    assert!(before.0.is_some() && before.2.is_some(), "{before:?}");

    s.run("f:SetBackdrop(nil)").unwrap();
    assert_eq!(s.eval::<i64>("return dep:GetNumPoints()").unwrap(), 1);
    assert_eq!(s.eval::<i64>("return h:GetNumPoints()").unwrap(), 1);
    assert!(s
        .eval::<bool>(r#"local p, r = dep:GetPoint(1); return p == "BOTTOM" and r == f"#)
        .unwrap());

    resolve_matches_scratch(&mut s, "the backdrop torn down");
    let after = (
        num(&mut s, "dep:GetLeft()"),
        num(&mut s, "dep:GetTop()"),
        num(&mut s, "h:GetLeft()"),
        num(&mut s, "h:GetTop()"),
    );
    assert_eq!(after, before, "no resolve was queued for either");
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A ScrollFrame's re-anchor clears its child's points (`0x787110`) and sets one on the frame, so
/// the child holds no anchor to a label it was placed on, and the label's destruction leaves it be.
#[test]
fn a_scroll_child_holds_no_anchor_to_a_label_it_was_placed_on() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        b = CreateFrame("Button", "B")
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        b:SetWidth(200); b:SetHeight(40)
        b:SetFontString(b:CreateFontString("Old", "ARTWORK"))
        Old:SetWidth(50); Old:SetHeight(10)
        sf = CreateFrame("ScrollFrame", "SF")
        sf:SetPoint("TOPLEFT", nil, "TOPLEFT", 10, -10)
        sf:SetWidth(50); sf:SetHeight(50)
        c = CreateFrame("Frame", "Child")
        c:SetWidth(5); c:SetHeight(5)
        c:SetPoint("TOPLEFT", Old, "TOPLEFT")
        sf:SetScrollChild(c)
    "#,
    )
    .unwrap();
    s.resolve();
    let left = num(&mut s, "c:GetLeft()");
    crate::assert_near!(left, Some(10.0), "placed by the override");

    assert_eq!(s.eval::<i64>("return c:GetNumPoints()").unwrap(), 1);
    s.run(r#"b:SetFontString(b:CreateFontString("New", "ARTWORK"))"#)
        .unwrap();
    assert!(s
        .eval::<bool>(r#"local p, r = c:GetPoint(1); return c:GetNumPoints() == 1 and r == sf"#)
        .unwrap());
    resolve_matches_scratch(&mut s, "the label replaced");
    assert_eq!(num(&mut s, "c:GetLeft()"), left);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// The re-link into another frame (`0x77fd10`) invalidates nothing: a region moved under a frame of
/// another scale keeps its rect until its own invalidate, which then reads the new owner's scale.
#[test]
fn a_region_reowned_keeps_its_rect_until_its_own_invalidate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        a = CreateFrame("Frame", "A"); a:SetWidth(10); a:SetHeight(10)
        a:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0)
        b = CreateFrame("Frame", "B"); b:SetWidth(10); b:SetHeight(10)
        b:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 0, 0); b:SetScale(2)
        x = CreateFrame("Frame", "X"); x:SetWidth(100); x:SetHeight(100)
        x:SetPoint("BOTTOMLEFT", nil, "BOTTOMLEFT", 100, 100)
        t = a:CreateTexture("T", "ARTWORK"); t:SetTexture(1, 1, 1)
        t:SetWidth(8); t:SetHeight(8); t:SetPoint("BOTTOMLEFT", x, "BOTTOMLEFT", 0, 0)
    "#,
    )
    .unwrap();
    s.resolve();
    crate::assert_near!(num(&mut s, "T:GetRight()"), Some(108.0));
    s.run("t:SetParent(b)").unwrap();
    resolve_matches_scratch(&mut s, "re-owned");
    crate::assert_near!(
        num(&mut s, "T:GetRight()"),
        Some(54.0),
        "the rect it had, read in the new owner's units"
    );
    s.run("t:SetWidth(8)").unwrap();
    resolve_matches_scratch(&mut s, "its own invalidate");
    crate::assert_near!(
        num(&mut s, "T:GetRight()"),
        Some(58.0),
        "8 units at scale 2, from 100"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
