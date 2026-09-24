//! The runtime `SetParent` law — strata/level re-assignment, the hide→show round-trip, and the
//! binding's error surface (`SetParent 0x7a1550`). Every test drives the
//! production Lua binding; the arena split
//! (`reparent_begin`/`reparent_finish`) is exercised through it.

use super::common::script;

/// `strata := parent.strata`, `level := parent.level + 1` (`0x76ab5a`/`0x76ab65`) — and the
/// subtree is NOT re-based (`propagate = 0`): an existing child keeps its absolute level, landing
/// below its own parent, exactly as shipped.
#[test]
fn reparent_relevels_the_moved_frame_only() {
    let s = script();
    s.run(
        r#"
        High = CreateFrame("Frame", "High")
        High:SetFrameStrata("DIALOG")
        High:SetFrameLevel(6)
        Panel = CreateFrame("Frame", "Panel")     -- MEDIUM 0
        Child = CreateFrame("Frame", "Child", Panel)  -- MEDIUM 1 (creation inherit)
        Panel:SetParent(High)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return Panel:GetFrameStrata()").unwrap(),
        "DIALOG",
        "strata := parent's, forced onto the moved frame"
    );
    assert_eq!(
        s.eval::<i64>("return Panel:GetFrameLevel()").unwrap(),
        7,
        "level := parent.level + 1"
    );
    assert_eq!(
        s.eval::<String>("return Child:GetFrameStrata()").unwrap(),
        "DIALOG",
        "the strata force recurses over the subtree"
    );
    assert_eq!(
        s.eval::<i64>("return Child:GetFrameLevel()").unwrap(),
        1,
        "the child keeps its absolute level — BELOW its parent's new 7; the client ships that"
    );
}

/// `SetParent(nil)` is a RESET — strata MEDIUM, level 0 (`0x76aba3`/`0x76abac`) — never "keep
/// current".
#[test]
fn reparent_to_nil_resets_strata_and_level() {
    let s = script();
    s.run(
        r#"
        High = CreateFrame("Frame", "SPHigh")
        High:SetFrameStrata("TOOLTIP")
        High:SetFrameLevel(9)
        F = CreateFrame("Frame", "SPF", High)
        F:SetParent(nil)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<String>("return F:GetFrameStrata()").unwrap(),
        "MEDIUM"
    );
    assert_eq!(s.eval::<i64>("return F:GetFrameLevel()").unwrap(), 0);
}

/// A reparent of an effectively-visible frame is a hide→show round-trip: `OnHide` fires (under the
/// OLD parent), then `OnShow` refires down the subtree — and an `OnShow` doing
/// `SetFrameLevel(GetParent():GetFrameLevel()+1)` lands the child back above its parent. That
/// hand-repair is AtlasLoot's own idiom and the reason its browse panel works on the reference —
/// the round-trip is what makes it run.
#[test]
fn a_visible_reparent_refires_onhide_then_onshow() {
    let s = script();
    s.run(
        r#"
        log = {}
        Old = CreateFrame("Frame", "RTOld")
        New = CreateFrame("Frame", "RTNew")
        New:SetFrameLevel(4)
        Mover = CreateFrame("Frame", "RTMover", Old)
        Kid = CreateFrame("Frame", "RTKid", Mover)
        Mover:SetScript("OnHide", function()
            table.insert(log, "hide:" .. Mover:GetParent():GetName())
        end)
        Mover:SetScript("OnShow", function()
            table.insert(log, "show:" .. Mover:GetParent():GetName())
        end)
        Kid:SetScript("OnShow", function()
            Kid:SetFrameLevel(Kid:GetParent():GetFrameLevel() + 1)
            table.insert(log, "kidshow")
        end)
        Mover:SetParent(New)
        "#,
    )
    .unwrap();
    let log: Vec<String> = s.eval("return log").unwrap();
    assert_eq!(
        log,
        vec!["hide:RTOld", "show:RTNew", "kidshow"],
        "OnHide observes the OLD parent, OnShow the new, and the refire walks the subtree"
    );
    assert_eq!(
        s.eval::<i64>("return RTMover:GetFrameLevel()").unwrap(),
        5,
        "the moved frame sits at parent+1"
    );
    assert_eq!(
        s.eval::<i64>("return RTKid:GetFrameLevel()").unwrap(),
        6,
        "the child's own OnShow hand-repaired it above its parent — the propagate-0 law's \
         period-correct workaround"
    );
}

/// A reparent of a hidden frame fires neither event, and does not recompute effective visibility:
/// the `+0xd4` cascade only runs inside the show half (`0x76abfd` gates on the captured `ebx`), so
/// a shown-but-invisible frame moved under a visible parent STAYS effectively invisible until
/// something shows it. Shipped behaviour, byte-verified — not an oversight.
#[test]
fn a_hidden_reparent_fires_nothing_and_leaves_visibility_stale() {
    let s = script();
    s.run(
        r#"
        fired = 0
        Hidden = CreateFrame("Frame", "STHidden")
        Hidden:Hide()
        Vis = CreateFrame("Frame", "STVis")
        F = CreateFrame("Frame", "STF", Hidden)   -- shown bit true, chain hidden
        F:SetScript("OnShow", function() fired = fired + 1 end)
        F:SetScript("OnHide", function() fired = fired + 1 end)
        F:SetParent(Vis)
        "#,
    )
    .unwrap();
    assert_eq!(
        s.eval::<i64>("return fired").unwrap(),
        0,
        "neither event fired"
    );
    assert!(
        s.eval::<bool>("return STF:IsShown() and not STF:IsVisible()")
            .unwrap(),
        "shown bit intact, effective visibility stale-false under the visible new parent"
    );
    // An explicit Show is what recomputes it — but Show() no-ops while the bit is already set, so
    // a real transition needs the toggle. The Hide half fires no OnHide (the frame was already
    // effectively invisible — transition-gated); only the Show half's false→true fires.
    s.run("STF:Hide(); STF:Show()").unwrap();
    assert!(s.eval::<bool>("return STF:IsVisible()").unwrap());
    assert_eq!(
        s.eval::<i64>("return fired").unwrap(),
        1,
        "the toggle's OnShow alone"
    );
}

/// The binding's error surface: a cycle RAISES (`0x87cb14` — including `newParent == self`), an
/// unresolvable name raises `Couldn't find region named`, and an ABSENT argument is the same raise
/// — not the nil path. The same-parent call is a total no-op that fires nothing.
#[test]
fn the_binding_raises_on_cycle_bad_name_and_absent_argument() {
    let s = script();
    s.run(
        r#"
        A = CreateFrame("Frame", "ErrA")
        B = CreateFrame("Frame", "ErrB", A)
        "#,
    )
    .unwrap();
    let cycle = s.run("A:SetParent(B)").unwrap_err().to_string();
    assert!(
        cycle.contains("Would create a loop parenting to ErrB"),
        "cycle raises: {cycle}"
    );
    let self_cycle = s.run("A:SetParent(A)").unwrap_err().to_string();
    assert!(self_cycle.contains("Would create a loop"), "{self_cycle}");
    let bad = s.run("A:SetParent('NoSuchFrame')").unwrap_err().to_string();
    assert!(
        bad.contains("Couldn't find region named 'NoSuchFrame'"),
        "{bad}"
    );
    let absent = s.run("A:SetParent()").unwrap_err().to_string();
    assert!(absent.contains("Couldn't find region named"), "{absent}");

    // Same parent: the total no-op — no events, no level rewrite.
    s.run(
        r#"
        B:SetFrameLevel(9)
        n = 0
        B:SetScript("OnHide", function() n = n + 1 end)
        B:SetScript("OnShow", function() n = n + 1 end)
        B:SetParent(A)
        "#,
    )
    .unwrap();
    assert_eq!(s.eval::<i64>("return n").unwrap(), 0);
    assert_eq!(
        s.eval::<i64>("return B:GetFrameLevel()").unwrap(),
        9,
        "0x76ab20 skips everything — the level is not re-derived"
    );
}

/// **A reparent under a scale change still moves the child's resolved rect** — the falsifier for
/// decision 2314's claim that `SetParent` is a value-only write the ledger can NAME.
///
/// The gate's two tiers police each other under `WOW_LAYOUT_VERIFY`, which is forced on for this
/// crate's tests: an incremental pass that names its nodes is re-run from scratch and the rects
/// must be identical. So this test does not have to assert the epoch bookkeeping — it only has to
/// make the reparent *observable in geometry*, which is what the conservative touch used to buy
/// wholesale. A child anchored inside a parent inherits `parentScale · ownScale` as its
/// `LayoutInput.scale`; move it to a parent with a different scale and both its offsets and its
/// span change.
#[test]
fn a_reparent_under_a_scale_change_moves_the_childs_rect() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        one = CreateFrame("Frame", "ScaleOne")
        one:SetPoint("BOTTOMLEFT", 0, 0); one:SetWidth(800); one:SetHeight(600)
        two = CreateFrame("Frame", "ScaleTwo")
        two:SetPoint("BOTTOMLEFT", 0, 0); two:SetWidth(800); two:SetHeight(600)
        two:SetScale(2)
        kid = CreateFrame("Frame", "ScaleKid", one)
        kid:SetPoint("BOTTOMLEFT", one, "BOTTOMLEFT", 10, 20); kid:SetWidth(100); kid:SetHeight(50)
    "#,
    )
    .unwrap();
    s.resolve();
    let at_one: (f32, f32, f32) = s
        .eval("return ScaleKid:GetLeft(), ScaleKid:GetBottom(), ScaleKid:GetWidth()")
        .unwrap();

    // The whole point: the write between the two reads is the reparent, and the read after it must
    // see it. Scale 2 doubles the offsets and the span in screen units — and `GetWidth` reports in
    // the frame's OWN scaled space, so the width reads back unchanged while the edges move.
    s.run("kid:SetParent(two)").unwrap();
    s.resolve();
    let at_two: (f32, f32, f32) = s
        .eval("return ScaleKid:GetLeft(), ScaleKid:GetBottom(), ScaleKid:GetWidth()")
        .unwrap();

    assert_eq!(at_one, (10.0, 20.0, 100.0), "seated in the unscaled parent");
    assert_eq!(
        at_two,
        (10.0, 20.0, 100.0),
        "own-space coordinates are scale-relative, so these do not move"
    );
    // …but the SCREEN rect does. Read it off the extraction, which is in screen units.
    let widths: Vec<f32> = s
        .extract()
        .iter()
        .filter_map(|q| match q.target {
            crate::order::ZTarget::Frame(_) => q.rect.map(|r| r.right - r.left),
            _ => None,
        })
        .collect();
    assert!(
        widths.iter().any(|w| (w - 200.0).abs() < 1e-3),
        "the reparented child should be 100 x scale 2 = 200 screen units wide; got {widths:?}"
    );
}
