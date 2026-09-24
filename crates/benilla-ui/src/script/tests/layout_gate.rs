//! The layout change gate: a resolve whose inputs match the last converged one is skipped
//! (`script::layout::InputFingerprint`). The tests count solves (`Model::layout_solves`), since
//! equal rects cannot tell a skip from a re-solve.

use super::common::script;
use crate::script::{Model, UiScript};

/// The mutation epoch (tier 1's input): bumped by every write into the layout read set.
fn epoch(s: &UiScript) -> u64 {
    s.lua()
        .app_data_ref::<Model>()
        .expect("model app_data")
        .layout_epoch
}

/// How many times the fixpoint has run.
fn solves(s: &UiScript) -> u64 {
    s.lua()
        .app_data_ref::<Model>()
        .expect("model app_data")
        .layout_solves
}

/// A screen-anchored frame and a child off it, so a move has to propagate.
fn setup(s: &UiScript) {
    s.run(
        r#"
        parent = CreateFrame("Frame", "Parent", nil)
        parent:SetWidth(100); parent:SetHeight(40)
        parent:SetPoint("TOPLEFT", nil, "TOPLEFT", 10, -10)
        child = CreateFrame("Frame", "Child", parent)
        child:SetWidth(20); child:SetHeight(20)
        child:SetPoint("TOPLEFT", parent, "BOTTOMRIGHT", 0, 0)
        "#,
    )
    .expect("setup");
}

#[test]
fn an_unchanged_resolve_does_not_run_the_fixpoint() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);

    s.resolve();
    let after_first = solves(&s);
    assert_eq!(after_first, 1, "the first resolve must run");

    for _ in 0..5 {
        s.resolve();
    }
    assert_eq!(
        solves(&s),
        after_first,
        "resolves with identical inputs must be skipped, not re-run"
    );
}

#[test]
fn moving_a_frame_reopens_the_gate_and_propagates() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.resolve();
    s.resolve(); // settle: the gate is now closed
    let before = solves(&s);

    let child_left = |s: &UiScript| -> f32 { s.eval::<f32>("return Child:GetLeft()").unwrap() };
    let first = child_left(&s);

    s.run("parent:SetPoint(\"TOPLEFT\", nil, \"TOPLEFT\", 60, -10)")
        .expect("move");
    s.resolve();
    assert_eq!(solves(&s), before + 1, "a SetPoint must reopen the gate");
    assert!(
        (child_left(&s) - (first + 50.0)).abs() < 0.001,
        "the move must propagate to the child: {} -> {}",
        first,
        child_left(&s)
    );

    let after = solves(&s);
    s.resolve();
    assert_eq!(
        solves(&s),
        after,
        "the gate must close again after the move"
    );
}

#[test]
fn a_screen_resize_reopens_the_gate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.resolve();
    s.resolve();
    let before = solves(&s);

    s.set_screen_size(1024.0, 768.0);
    s.resolve();
    assert_eq!(
        solves(&s),
        before + 1,
        "a screen-size change must reopen the gate"
    );
}

/// A region's `SetPoint` writes `region_data`, not `layout_inputs`; the region sweep reads it.
#[test]
fn moving_a_region_reopens_the_gate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.run(
        r#"
        tex = parent:CreateTexture(nil, "ARTWORK")
        tex:SetWidth(10); tex:SetHeight(10)
        tex:SetPoint("TOPLEFT", parent, "TOPLEFT", 0, 0)
        "#,
    )
    .expect("region");
    s.resolve();
    s.resolve();
    let before = solves(&s);

    s.run("tex:SetPoint(\"TOPLEFT\", parent, \"TOPLEFT\", 25, 0)")
        .expect("move region");
    s.resolve();
    assert_eq!(
        solves(&s),
        before + 1,
        "a region SetPoint must reopen the gate"
    );
}

/// Hidden frames still resolve (visibility is an extract-time filter), so a hide moves no rect and
/// the gate stays closed.
#[test]
fn hiding_a_frame_does_not_reopen_the_gate() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.resolve();
    s.resolve();
    let before = solves(&s);

    s.run("parent:Hide()").expect("hide");
    s.resolve();
    assert_eq!(
        solves(&s),
        before,
        "visibility does not move rects, so the gate stays closed"
    );
}

/// `ContainerFrameItemButton_OnUpdate` re-runs `OnEnter` every frame while the tooltip is the
/// button's, its throttle commented out (`ContainerFrame.lua:645`), so rebuilding identical
/// tooltip content must neither re-measure nor re-solve.
#[test]
fn the_hover_re_enter_loop_neither_re_measures_nor_re_solves() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local owner = CreateFrame("Button", "Slot")
        owner:SetPoint("TOPLEFT", 100, -100); owner:SetWidth(40); owner:SetHeight(40)
        tt = CreateFrame("GameTooltip", "TT")
        -- One bag-slot OnEnter: the clear (SetOwner) + the content rebuild, verbatim in shape.
        function reenter()
            TT:SetOwner(Slot, "ANCHOR_RIGHT")
            TT:AddLine("Small Shield", 0, 1, 0)
            TT:AddDoubleLine("Shield", "Off Hand", 1, 1, 1, 1, 1, 1)
            TT:AddLine("85 Armor")
            -- A WRAP-flagged line: `clear_content` still drops the wrap-pinned width, which feeds
            -- the measure key, so the re-pin at append time has to restore it byte-identically or
            -- this line alone re-shapes forever (the real item tooltip's long green "Use:" line).
            TT:AddLine("Restores 243 health over 21 sec.", 0, 1, 0, 1)
            TT:AddLine("Durability 45 / 45")
            TT:Show()
        end
        "#,
    )
    .expect("setup");

    // The host's font engine: one fixed size per string.
    let sizes: &[(&str, f32, f32)] = &[
        ("Small Shield", 80.0, 14.0),
        ("Shield", 50.0, 12.0),
        ("Off Hand", 45.0, 12.0),
        ("85 Armor", 60.0, 12.0),
        ("Restores 243 health over 21 sec.", 118.0, 24.0),
        ("Durability 45 / 45", 96.0, 12.0),
    ];
    // One frame in the app's order (`ui_script::extract::tick_script`): tick, resolve, measure
    // round-trip, resolve. Returns how many strings were shaped.
    let frame = |s: &mut UiScript| -> usize {
        s.run("reenter()").expect("re-enter");
        s.resolve();
        let reqs = s.fontstrings_needing_measure();
        let shaped = reqs.len();
        if !reqs.is_empty() {
            let answers: Vec<(u32, f32, f32, u64)> = reqs
                .iter()
                .map(|r| {
                    let (w, h) = sizes
                        .iter()
                        .find(|(t, _, _)| *t == r.text)
                        .map(|&(_, w, h)| (w, h))
                        .unwrap_or_else(|| panic!("unexpected string measured: {:?}", r.text));
                    (r.id, w, h, r.key)
                })
                .collect();
            s.set_measured_text_unwrapped(&answers);
            s.resolve();
        }
        shaped
    };

    // The first frames shape the strings and re-solve while the auto-size pre-pass converges.
    for _ in 0..4 {
        frame(&mut s);
    }

    // Steady state: the hover is parked on an item whose tooltip has not changed.
    let solves_before = solves(&s);
    let mut shaped = 0;
    for _ in 0..10 {
        shaped += frame(&mut s);
    }

    assert_eq!(
        shaped, 0,
        "a re-enter with identical content must re-shape NO text: the measure cache keys on \
         content, so rebuilding the same lines re-validates it"
    );
    assert_eq!(
        solves(&s),
        solves_before,
        "a re-enter with identical content must not reopen the layout gate"
    );
    // Nor may it reach tier 2, which hashes the whole UI: identical content leaves the epoch alone.
    let epoch_before = epoch(&s);
    for _ in 0..10 {
        frame(&mut s);
    }
    assert_eq!(
        epoch(&s),
        epoch_before,
        "a re-enter with identical content must not touch the layout epoch at all — tier 1 has to \
         hold, or every hover frame pays the whole-UI fingerprint"
    );

    // Changed content re-measures only its own line, and a measure that widens the auto-sized
    // plate reopens the gate.
    s.run(
        r#"function reenter()
            TT:SetOwner(Slot, "ANCHOR_RIGHT")
            TT:AddLine("Small Shield", 0, 1, 0)
            TT:AddDoubleLine("Shield", "Off Hand", 1, 1, 1, 1, 1, 1)
            TT:AddLine("85 Armor")
            TT:AddLine("Restores 243 health over 21 sec.", 0, 1, 0, 1)
            TT:AddLine("Durability 44 / 45")
            TT:Show()
        end"#,
    )
    .expect("damage the shield");
    // Wider than the 120 the double line contributes, so the plate itself has to grow.
    let sizes: &[(&str, f32, f32)] = &[("Durability 44 / 45", 150.0, 12.0)];
    s.run("reenter()").expect("re-enter");
    s.resolve();
    let reqs = s.fontstrings_needing_measure();
    assert_eq!(
        reqs.len(),
        1,
        "only the line whose text changed re-measures, got {:?}",
        reqs.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
    assert_eq!(reqs[0].text, "Durability 44 / 45");
    let answers: Vec<(u32, f32, f32, u64)> = reqs
        .iter()
        .map(|r| {
            let (w, h) = sizes
                .iter()
                .find(|(t, _, _)| *t == r.text)
                .map(|&(_, w, h)| (w, h))
                .expect("known string");
            (r.id, w, h, r.key)
        })
        .collect();
    s.set_measured_text_unwrapped(&answers);
    s.resolve();
    assert!(
        solves(&s) > solves_before,
        "changed content must reopen the gate"
    );
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// Whether tier 1, the mutation epoch, is closed: any converged resolve closes it, and a quiet
/// frame then skips at a `u64` compare without hashing the fingerprint.
fn tier_one_closed(s: &UiScript) -> bool {
    let m = s.lua().app_data_ref::<Model>().expect("model app_data");
    m.layout_epoch_resolved == Some(m.layout_epoch)
}

/// A region moved every frame, as `CastingBarSpark:SetPoint` does (`CastingBarFrame.lua:114`),
/// costs exactly one solve per frame: the fingerprint hashes inputs alone, so that solve closes
/// tier 1.
#[test]
fn a_region_moving_every_frame_costs_exactly_one_solve_per_frame() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.run(
        r#"
        spark = parent:CreateTexture(nil, "OVERLAY")
        spark:SetWidth(32); spark:SetHeight(32)
        spark:SetPoint("CENTER", parent, "LEFT", 0, 2)
        "#,
    )
    .expect("spark");
    s.resolve();
    assert!(tier_one_closed(&s), "the setup must settle in one resolve");

    // Ten castbar frames: the spark re-pointed to a new offset, then the per-frame resolve. The
    // offsets start at 1.7, not 0, since the setters' compare absorbs a write of the seed value.
    for frame in 0..10 {
        let before = solves(&s);
        s.run(&format!(
            r#"spark:SetPoint("CENTER", parent, "LEFT", {}, 2)"#,
            f64::from(frame + 1) * 1.7
        ))
        .expect("spark move");
        s.resolve();
        assert_eq!(
            solves(&s) - before,
            1,
            "frame {frame}: a single moving region must cost ONE solve, not the \
             solve+settle+skip trio the seeds-in-the-fingerprint law used to force"
        );
        assert!(
            tier_one_closed(&s),
            "frame {frame}: the converged solve must close tier 1, so a second getter in the \
             same tick skips at the u64 compare instead of re-walking the whole roster"
        );
    }
    assert!(s.errors().is_empty(), "{:?}", s.errors());
}

/// A settled, untouched frame costs no solve, and tier 1 spares it the fingerprint walk too.
#[test]
fn a_quiet_frame_after_a_move_costs_no_solve_at_all() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.resolve();
    s.run(r#"parent:SetPoint("TOPLEFT", nil, "TOPLEFT", 33, -44)"#)
        .expect("move");
    s.resolve();
    let settled = solves(&s);
    for _ in 0..5 {
        s.resolve();
        assert_eq!(solves(&s), settled, "a quiet frame must not solve");
        assert!(tier_one_closed(&s), "and must not reopen tier 1");
    }
}

#[test]
fn a_settled_resolve_closes_tier_one_and_a_real_write_reopens_it() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);

    // One resolve closes the epoch: the fingerprint hashes inputs alone, now at their fixpoint.
    s.resolve();
    assert!(
        tier_one_closed(&s),
        "a converged resolve must close the epoch by itself"
    );

    s.run("parent:SetWidth(150)").expect("resize");
    assert!(
        !tier_one_closed(&s),
        "a real layout write must reopen tier 1"
    );
    s.resolve();
    assert!(tier_one_closed(&s), "and one resolve closes it again");
}

#[test]
fn an_idempotent_setter_call_leaves_tier_one_closed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    s.resolve();
    s.resolve();
    assert!(tier_one_closed(&s));

    // The OnUpdate idiom of re-setting the same geometry: the setters compare before writing.
    s.run(
        r#"
        parent:SetWidth(100); parent:SetHeight(40)
        parent:SetPoint("TOPLEFT", nil, "TOPLEFT", 10, -10)
        child:SetPoint("TOPLEFT", parent, "BOTTOMRIGHT", 0, 0)
        "#,
    )
    .expect("idempotent re-set");
    assert!(
        tier_one_closed(&s),
        "value-identical setter calls must not dirty the epoch"
    );
    let before = solves(&s);
    s.resolve();
    assert_eq!(solves(&s), before, "and the resolve stays skipped");
}

#[test]
fn a_paint_only_write_leaves_tier_one_closed() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    setup(&s);
    // A region with no anchors: a paint setter creates its `region_data` entry, which the resolve
    // sweep skips, so the gate must ignore it.
    s.run(r#"tex = parent:CreateTexture(nil, "ARTWORK")"#)
        .expect("region");
    s.resolve();
    s.resolve();
    assert!(tier_one_closed(&s));

    s.run(r#"tex:SetTexture(1, 0, 0)"#).expect("paint");
    assert!(
        tier_one_closed(&s),
        "creating/painting an anchor-less region is not a layout change"
    );
    let before = solves(&s);
    s.resolve();
    assert_eq!(solves(&s), before);
}

/// A retarget patches one node's edges in the cached graph instead of re-deriving it, so the new
/// edge must land: moving the new target moves the node, and moving the old one does not.
#[test]
fn a_retargeted_anchor_follows_its_new_target() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        a = CreateFrame("Frame", "A", nil); a:SetWidth(10); a:SetHeight(10)
        a:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, 0)
        b = CreateFrame("Frame", "B", nil); b:SetWidth(10); b:SetHeight(10)
        b:SetPoint("TOPLEFT", nil, "TOPLEFT", 100, 0)
        plate = CreateFrame("Frame", "Plate", nil)
        plate:SetWidth(10); plate:SetHeight(10)
        plate:SetPoint("TOPLEFT", a, "TOPRIGHT", 0, 0)
        "#,
    )
    .expect("setup");
    s.resolve();
    s.resolve();

    // The plate now hangs off B, as `GameTooltip:SetOwner` re-points it at the next button.
    s.run(r#"plate:SetPoint("TOPLEFT", b, "TOPRIGHT", 0, 0)"#)
        .expect("retarget");
    s.resolve();
    let after_retarget = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (after_retarget - 110.0).abs() < 0.01,
        "the retargeted plate must sit at B's right edge (110), not {after_retarget}"
    );

    s.run(r#"b:SetPoint("TOPLEFT", nil, "TOPLEFT", 300, 0)"#)
        .expect("move B");
    s.resolve();
    let after_move = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (after_move - 310.0).abs() < 0.01,
        "moving the NEW target must move the plate (310), not leave it at {after_move} — the \
         retarget's edge patch dropped the new edge and the node is under-dirtied (decision 1625)"
    );

    s.run(r#"a:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, -50)"#)
        .expect("move A");
    s.resolve();
    let plate_now = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (plate_now - 310.0).abs() < 0.01,
        "the plate must not follow its OLD target any more (310), got {plate_now}"
    );
}
