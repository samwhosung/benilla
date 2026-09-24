//! The layout change gate: a resolve whose inputs are byte-identical to the last converged one is
//! skipped outright (`script::layout::InputFingerprint`).
//!
//! These assert on `Model::layout_solves` — the count of times the fixpoint actually ran — rather
//! than on the rects, because rect equality alone cannot distinguish "the gate skipped" from "the
//! gate re-solved and got the same answer". The value of the gate is precisely the difference.

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

/// A frame anchored to the screen, plus a child hanging off it — enough that a move has to
/// propagate, so a wrongly-skipped resolve would be visible in the child too.
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

    // Nothing touched in between: every further resolve is a no-op.
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

    // And it closes again once the move has settled.
    let after = solves(&s);
    s.resolve();
    assert_eq!(
        solves(&s),
        after,
        "the gate must close again after the move"
    );
}

/// Resizing the window moves every top-level frame — the gate must never swallow it.
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

/// A region's own `SetPoint` writes `region_data`, not `layout_inputs` — the half of the read set
/// that is easiest to leave out of a change gate, and the one the region sweep consumes.
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

/// Hiding a frame does NOT move any rect (the client resolves hidden frames too — visibility is an
/// extract-time filter), so the gate is right to stay closed. Pinned deliberately: it is the one
/// place where "nothing to re-solve" is surprising, and a future reader tempted to dirty on
/// show/hide should see that the current model does not need it.
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

/// The hover re-enter loop must be FREE while the content is unchanged.
///
/// `ContainerFrameItemButton_OnUpdate` re-runs `OnEnter` every frame while the tooltip is the
/// button's own — faithful, and UNTHROTTLED in 1.12 (its `updateTooltip` throttle is commented
/// out; the reference's `ContainerFrameItemButton_OnUpdate` ships the same loop). So a bag hover clears and
/// rebuilds the SAME tooltip 60×/sec, and the engine must absorb that: identical content re-derives
/// an identical model, so the measure cache re-validates on its content key and the gate stays shut.
///
/// This is the regression gate for the live report (a bag hover cost +10 CPU ms/frame, ~2 full
/// arena solves + a re-shape of every line, every frame): `clear_content` wiped
/// `RegionData::measured` — an invalidation the content-hash key already performs — so every
/// re-enter re-measured every line, and the non-empty measure list forced a second full resolve
/// that the gate could never close over.
///
/// It drives the app's real per-frame order (`ui_script::extract::tick_script`): Lua tick →
/// resolve → measure round-trip → resolve.
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

    // The host's font engine: every distinct string has one deterministic size. Counts every
    // string it is asked to shape, so the test can assert on shaping work directly.
    let sizes: &[(&str, f32, f32)] = &[
        ("Small Shield", 80.0, 14.0),
        ("Shield", 50.0, 12.0),
        ("Off Hand", 45.0, 12.0),
        ("85 Armor", 60.0, 12.0),
        ("Restores 243 health over 21 sec.", 118.0, 24.0),
        ("Durability 45 / 45", 96.0, 12.0),
    ];
    // One frame of `tick_script`: tick (the re-enter) → resolve → measure round-trip → resolve.
    // Returns how many strings the font engine was asked to shape this frame.
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

    // Settle: the first frames legitimately shape the five strings and re-solve as the auto-size
    // pre-pass converges on the fresh measures.
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
    // …and it must not even reach tier 2. Tier 1 (the mutation epoch) is what makes a quiet frame
    // FREE; tier 2 still hashes the whole UI's read set to decide "quiet" (~0.65 ms/frame measured
    // with `WOW_UI_COST=1` on `WOW_CAPTURE=ui-tooltip`, at solves=0). The re-enter loop used to
    // pay that on every single hover frame, because `clear_content` wiped each line's wrap pin and
    // `append_line` re-pinned the same width — a round trip inside one frame, two epoch bumps.
    // Idempotent content must be epoch-silent, not merely fingerprint-absorbed.
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

    // The gate is shut because nothing MOVED — not because the tooltip went stale. Changed content
    // must still re-measure exactly its own line, and a measure that resizes the auto-sized plate
    // (this one overtakes the double line as the widest) must reopen the gate.
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

/// Tier 1 of the gate: the mutation epoch. **Any CONVERGED resolve closes it**
/// — the fingerprint is hashed over inputs alone, so a solve cannot outgrow the
/// value it stores — and from then on a quiet frame skips at a `u64` compare without computing
/// the fingerprint at all.
fn tier_one_closed(s: &UiScript) -> bool {
    let m = s.lua().app_data_ref::<Model>().expect("model app_data");
    m.layout_epoch_resolved == Some(m.layout_epoch)
}

/// **The castbar law** (ledger B283): a region that MOVES every frame — the classic
/// OnUpdate animation idiom, our own `CastingBarSpark:SetPoint`, and every addon that slides a
/// texture — must cost exactly **one** let-through resolve per frame.
///
/// It used to cost three. The fingerprint was hashed over the 0294 SEEDS as well as the inputs, so
/// a solve necessarily outgrew the fingerprint it had just stored: neither that pass nor the
/// settling pass behind it could close tier 1, and the frame paid solve + settle + skip — three
/// whole-roster walks at ~1.0–1.4 ms each. Measured live at the Stormwind pin: **+4.2 ms of CPU
/// per frame for one 32×32 spark**, on a default UI with no addons loaded.
///
/// This is the regression guard for that whole bug class. `solves` is the honest counter — a
/// wasted walk that concludes "nothing moved" still costs the full preamble — so the assertion is
/// on the count, never on a duration (milliseconds are not evidence of a scope regression).
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

    // Ten frames of the castbar's own inner loop: one region re-pointed to a NEW offset, then the
    // host's per-frame resolve. Nothing else in the model moves.
    //
    // The offsets start at 1.7, not 0 — a `frame * 1.7` sequence opens with a re-write of the
    // seed value, which the setters' bit-exact compare absorbs (no epoch bump, no solve at all).
    // That is the *right* answer and `an_idempotent_setter_call_leaves_tier_one_closed` pins it;
    // here it would have measured the guard instead of the gate.
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

/// The other half of the same law: once it has settled, an untouched frame costs NOTHING — no
/// solve, and (tier 1) not even the fingerprint walk that would decide so.
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

    // ONE resolve closes the epoch: the fingerprint is hashed over inputs alone,
    // and the rounds just drove those inputs to their fixpoint, so there is nothing left for a
    // settling pass to discover. This used to need two.
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

    // The classic per-frame OnUpdate idiom: re-assert the exact same geometry every frame. The
    // setters' compare-before-write keeps the epoch untouched, so an idle UI never re-enters
    // the fingerprint path at all.
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
    // A paint region with NO anchors: its region_data entry is created by a paint setter and
    // must stay invisible to the layout gate (the resolve sweep skips anchor-less entries).
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

/// **The retarget falsifier**: after a node is re-pointed from target A to target
/// B, the cached graph's EDGES must describe the new shape — moving B must move the node.
///
/// This is the one direction that can be silently wrong. 1625 keeps the cached graph across an
/// anchor retarget by patching one node's edges instead of re-deriving all of them, and a patch
/// that dropped the *new* edge would under-dirty: the node would sit at a stale rect while the
/// thing it is anchored to moved. (The other direction — a leftover edge to the OLD target — only
/// costs a needless re-solve, so it is asserted second and separately.)
///
/// It asserts on RECTS, not on counts, because a wrong edge set is a wrong picture: the failure
/// this guards against is a tooltip left behind at the last button's position.
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

    // The retarget: the plate now hangs off B, exactly as `GameTooltip:SetOwner` re-points the
    // plate at the next button under the cursor.
    s.run(r#"plate:SetPoint("TOPLEFT", b, "TOPRIGHT", 0, 0)"#)
        .expect("retarget");
    s.resolve();
    let after_retarget = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (after_retarget - 110.0).abs() < 0.01,
        "the retargeted plate must sit at B's right edge (110), not {after_retarget}"
    );

    // …and it must keep following B. This is the assertion the edge patch exists to satisfy: the
    // graph was NOT re-derived across the retarget, so if the new edge never made it into the
    // cached edge set, B moving reaches nothing and the plate stays put.
    s.run(r#"b:SetPoint("TOPLEFT", nil, "TOPLEFT", 300, 0)"#)
        .expect("move B");
    s.resolve();
    let after_move = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (after_move - 310.0).abs() < 0.01,
        "moving the NEW target must move the plate (310), not leave it at {after_move} — the \
         retarget's edge patch dropped the new edge and the node is under-dirtied (decision 1625)"
    );

    // …and it must NOT follow the old one any more.
    s.run(r#"a:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, -50)"#)
        .expect("move A");
    s.resolve();
    let plate_now = s.eval::<f64>("return Plate:GetLeft()").expect("plate left");
    assert!(
        (plate_now - 310.0).abs() < 0.01,
        "the plate must not follow its OLD target any more (310), got {plate_now}"
    );
}
