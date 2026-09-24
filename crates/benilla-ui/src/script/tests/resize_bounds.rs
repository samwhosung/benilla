//! **The resize-bounds quad**, pinned to the reference's own storage (`CLayoutFrame 0x767680`).
//!
//! Every assertion here is one the first cut of this code got *wrong* by writing the plausible
//! thing instead of the read thing — which is why they are pinned rather than trusted:
//!
//! | the plausible thing | the byte thing |
//! |---|---|
//! | an unset bound is "no bound" and reads back `nil` | all four fields are `0.0`, the getters push two numbers |
//! | a floor of `1.0` keeps a drag sane | there is **no floor** — a drag goes through zero into negatives |
//! | a bound is a bound | only an exactly-`0.0` bound is the disable sentinel; a **negative** one clamps |
//! | `min > max` is a caller error to reconcile | nothing reconciles it; **max wins** |
//! | setting a bound fixes a frame already outside it | nothing happens until the next drag tick |
//! | `StartSizing("CENTER")` grips no edge, so it is a no-op | it is the **move** arm, and it is never bounded |

use crate::script::UiScript;

/// A resizable 200×100 frame planted at its BOTTOMLEFT, the shape the sibling sizing test uses.
fn sizer() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    s.run(
        r#"
        f = CreateFrame("Frame", "Sizer", UIParent)
        f:SetResizable(true)
        f:SetWidth(200) f:SetHeight(100)
        f:SetPoint("BOTTOMLEFT", UIParent, "BOTTOMLEFT", 100, 50)
        "#,
    )
    .unwrap();
    s.resolve();
    s
}

/// The getters answer **two numbers**, `0, 0` on a frame nobody bounded — never `nil`.
#[test]
fn a_virgin_frames_bounds_read_back_as_zero_not_nil() {
    let s = sizer();
    for verb in ["GetMinResize", "GetMaxResize"] {
        let (w, h): (f64, f64) = s.eval(&format!("return Sizer:{verb}()")).unwrap();
        assert_eq!((w, h), (0.0, 0.0), "{verb} on an unbounded frame");
        assert_eq!(
            s.eval::<i64>(&format!(
                "local a,b = Sizer:{verb}() return (a and 1 or 0)+(b and 1 or 0)"
            ))
            .unwrap(),
            2,
            "{verb} must push two values, not nils"
        );
    }
    s.run("Sizer:SetMinResize(120, 60) Sizer:SetMaxResize(400, 300)")
        .unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return Sizer:GetMinResize()").unwrap(),
        (120.0, 60.0)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return Sizer:GetMaxResize()").unwrap(),
        (400.0, 300.0)
    );
    // Setting one pair rewrites the other verbatim — the reference reads all four and writes all
    // four, so the untouched pair round-trips rather than being cleared.
    s.run("Sizer:SetMinResize(10, 10)").unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return Sizer:GetMaxResize()").unwrap(),
        (400.0, 300.0)
    );
}

/// The argument gate is `lua_isnumber` — a numeric **string** passes; everything else raises the
/// reference's own `Usage:` text, naming the frame.
#[test]
fn the_setters_gate_is_lua_isnumber_and_the_usage_text_is_the_references() {
    let s = sizer();
    s.run("Sizer:SetMinResize(\"120\", \"60\")").unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return Sizer:GetMinResize()").unwrap(),
        (120.0, 60.0),
        "a numeric string is coerced, as 5.0's lua_isnumber does"
    );
    // Arguments past the third are ignored — no upper arity check.
    s.run("Sizer:SetMaxResize(400, 300, 999, \"x\")").unwrap();
    assert_eq!(
        s.eval::<(f64, f64)>("return Sizer:GetMaxResize()").unwrap(),
        (400.0, 300.0)
    );
    for bad in [
        "Sizer:SetMinResize()",
        "Sizer:SetMinResize(120)",
        "Sizer:SetMinResize(120, nil)",
        "Sizer:SetMinResize(120, true)",
        "Sizer:SetMinResize(120, \"wide\")",
        "Sizer:SetMinResize({}, {})",
    ] {
        let err = s.run(bad).unwrap_err().to_string();
        assert!(
            err.contains("Usage: Sizer:SetMinResize(minWidth, minHeight)"),
            "`{bad}` must raise the reference's Usage text, got: {err}"
        );
    }
    let err = s.run("Sizer:SetMaxResize(1)").unwrap_err().to_string();
    assert!(err.contains("Usage: Sizer:SetMaxResize(maxWidth, maxHeight)"));
}

/// **A drag clamps against the bounds, and the anchor stops exactly on them.**
///
/// The second half is the rebate: without it the size pins at the bound while the delta keeps
/// feeding the anchor, so a window held past its minimum stops shrinking and starts *walking*.
#[test]
fn a_drag_clamps_and_the_planted_edge_stops_on_the_bound() {
    let mut s = sizer();
    s.run("Sizer:SetMinResize(150, 60) Sizer:SetMaxResize(260, 300)")
        .unwrap();

    // RIGHT grip, dragged far right: width saturates at the max, the left edge never moves.
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(900.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 260.0);
    assert_eq!(s.eval::<f32>("return Sizer:GetLeft()").unwrap(), 100.0);
    s.run("Sizer:StopMovingOrSizing()").unwrap();

    // LEFT grip, dragged far right: width saturates at the MIN — and the left edge lands exactly
    // on it (100 + 260 − 150 = 210), not wherever the raw cursor delta would have carried it.
    let right_before = s.eval::<f32>("return Sizer:GetRight()").unwrap();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"LEFT\")").unwrap();
    s.mouse_move(900.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 150.0);
    assert_eq!(
        s.eval::<f32>("return Sizer:GetLeft()").unwrap(),
        right_before - 150.0,
        "the rebate: a saturated grip stops ON the bound instead of walking past it"
    );
    assert_eq!(
        s.eval::<f32>("return Sizer:GetRight()").unwrap(),
        right_before,
        "the ungripped edge must not move"
    );
    // Dragging further while saturated moves nothing at all.
    s.mouse_move(1500.0, 100.0);
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Sizer:GetLeft()").unwrap(),
        right_before - 150.0
    );
    s.run("Sizer:StopMovingOrSizing()").unwrap();
}

/// **`0.0` is the disable sentinel, per field** — and a NEGATIVE bound is live.
#[test]
fn zero_disables_and_a_negative_bound_still_clamps() {
    let mut s = sizer();
    // Height bounded, width explicitly unbounded on both ends.
    s.run("Sizer:SetMinResize(0, 80) Sizer:SetMaxResize(0, 90)")
        .unwrap();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"BOTTOMRIGHT\")").unwrap();
    s.mouse_move(900.0, 100.0);
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Sizer:GetWidth()").unwrap(),
        800.0,
        "a 0 width bound must not clamp — it is the disable sentinel, not a limit"
    );
    assert_eq!(s.eval::<f32>("return Sizer:GetHeight()").unwrap(), 90.0);
    s.run("Sizer:StopMovingOrSizing()").unwrap();

    // A negative maximum is NOT a sentinel: it clamps, and the width really does go negative.
    s.run("Sizer:SetMaxResize(-50, 0) Sizer:SetMinResize(0, 0)")
        .unwrap();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(310.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), -50.0);
}

/// With no bound at all there is **no floor** — the client has no `1.0` and neither do we.
#[test]
fn an_unbounded_drag_goes_through_zero() {
    let mut s = sizer();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(50.0, 100.0);
    s.resolve();
    assert_eq!(
        s.eval::<f32>("return Sizer:GetWidth()").unwrap(),
        -50.0,
        "no SetMinResize means no floor, not a floor of 1"
    );
}

/// **`min > max` ends at `max`** — min is applied first, then max, and nothing reconciles them.
#[test]
fn a_min_above_the_max_resolves_to_the_max() {
    let mut s = sizer();
    s.run("Sizer:SetMinResize(400, 10) Sizer:SetMaxResize(200, 500)")
        .unwrap();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(305.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 200.0);
}

/// **Setting a bound a frame already violates does nothing** until the next drag tick — the setter
/// does not touch the size, and no other write path consults the bounds.
#[test]
fn a_bound_set_late_does_not_resize_the_frame() {
    let mut s = sizer();
    s.run("Sizer:SetMinResize(400, 400)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 200.0);
    assert_eq!(s.eval::<f32>("return Sizer:GetHeight()").unwrap(), 100.0);
    // A programmatic SetWidth past the bound is not clamped either — VERIFIED: no layout vtable
    // in the client overrides the plain, non-clamping SetWidth/SetHeight.
    s.run("Sizer:SetWidth(50)").unwrap();
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 50.0);
    // The first drag tick is what snaps it into range.
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"RIGHT\")").unwrap();
    s.mouse_move(301.0, 100.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetWidth()").unwrap(), 400.0);
}

/// **`StartSizing("CENTER")` translates the frame and is never bounded** — the pump's case 4.
#[test]
fn a_center_grip_moves_the_frame_and_ignores_the_bounds() {
    let mut s = sizer();
    s.run("Sizer:SetMinResize(200, 100) Sizer:SetMaxResize(200, 100)")
        .unwrap();
    s.mouse_move(300.0, 100.0);
    s.run("Sizer:StartSizing(\"CENTER\")").unwrap();
    s.mouse_move(340.0, 130.0);
    s.resolve();
    assert_eq!(
        (
            s.eval::<f32>("return Sizer:GetWidth()").unwrap(),
            s.eval::<f32>("return Sizer:GetHeight()").unwrap()
        ),
        (200.0, 100.0),
        "a CENTER grip resizes nothing, so the bounds are never consulted"
    );
    assert_eq!(s.eval::<f32>("return Sizer:GetLeft()").unwrap(), 140.0);
    s.run("Sizer:StopMovingOrSizing()").unwrap();
    s.mouse_move(500.0, 300.0);
    s.resolve();
    assert_eq!(s.eval::<f32>("return Sizer:GetLeft()").unwrap(), 140.0);
}

/// `<ResizeBounds>` in XML reaches the same four fields — and a block naming only `<minResize>`
/// **resets the max pair to unbounded**, because the client writes all four unconditionally.
#[test]
fn the_xml_element_writes_both_pairs() {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1024.0, 768.0);
    let doc = benilla_ui_doc(
        r#"
        <Frame name="Chatty" resizable="true">
          <Size><AbsDimension x="400" y="200"/></Size>
          <ResizeBounds>
            <minResize><AbsDimension x="296" y="75"/></minResize>
            <maxResize><AbsDimension x="608" y="400"/></maxResize>
          </ResizeBounds>
        </Frame>
        "#,
    );
    load(&s, &doc);
    assert_eq!(
        s.eval::<(f64, f64)>("return Chatty:GetMinResize()")
            .unwrap(),
        (296.0, 75.0)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return Chatty:GetMaxResize()")
            .unwrap(),
        (608.0, 400.0)
    );
    // An authored `<Size>` outside the authored bounds survives load unchanged — neither path
    // clamps, whatever the document order.
    assert_eq!(s.eval::<f32>("return Chatty:GetWidth()").unwrap(), 400.0);

    let partial = benilla_ui_doc(
        r#"
        <Frame name="Half" resizable="true">
          <ResizeBounds><minResize><AbsDimension x="10" y="20"/></minResize></ResizeBounds>
        </Frame>
        "#,
    );
    load(&s, &partial);
    assert_eq!(
        s.eval::<(f64, f64)>("return Half:GetMinResize()").unwrap(),
        (10.0, 20.0)
    );
    assert_eq!(
        s.eval::<(f64, f64)>("return Half:GetMaxResize()").unwrap(),
        (0.0, 0.0),
        "an absent <maxResize> writes 0 — the block is unconditional on both pairs"
    );
}

/// Wrap a fragment in the `<Ui>` root the parser wants.
fn benilla_ui_doc(body: &str) -> String {
    format!("<Ui>{body}</Ui>")
}

fn load(s: &UiScript, src: &str) {
    let doc = crate::framexml::parse(src).expect("parse");
    let report = crate::loader::load_in(s, &doc, "test.xml", &|_| None);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
}
