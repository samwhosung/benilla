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
    assert_eq!(s.eval::<i64>("return t:GetNumPoints()").unwrap(), 2);
    assert_eq!(
        num(&mut s, "t:GetLeft()"),
        Some(225.0),
        "right of the label"
    );

    s.run(r#"b:SetFontString(b:CreateFontString("New", "ARTWORK"))"#)
        .unwrap();
    assert_eq!(
        s.eval::<i64>("return t:GetNumPoints()").unwrap(),
        1,
        "the anchor to the destroyed label is freed"
    );
    assert_eq!(
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
    assert_eq!(
        num(&mut s, "t:GetLeft()"),
        Some(225.0),
        "the unhook queued no resolve"
    );
    // Its own invalidate resolves it from the anchor left: centred under the button's top.
    s.run("t:SetWidth(12)").unwrap();
    resolve_matches_scratch(&mut s, "the texture's own invalidate");
    assert_eq!(num(&mut s, "t:GetLeft()"), Some(194.0));
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

/// A ScrollFrame's child is placed by the frame's override, yet its own anchors stand under it:
/// one to a destroyed label is freed too.
#[test]
fn a_scroll_childs_own_anchor_to_a_destroyed_label_is_freed() {
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
    assert_eq!(left, Some(10.0), "placed by the override");

    s.run(r#"b:SetFontString(b:CreateFontString("New", "ARTWORK"))"#)
        .unwrap();
    assert_eq!(s.eval::<i64>("return c:GetNumPoints()").unwrap(), 0);
    resolve_matches_scratch(&mut s, "the label replaced");
    assert_eq!(num(&mut s, "c:GetLeft()"), left);
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}
