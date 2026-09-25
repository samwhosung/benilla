//! What a UI change costs on the full shipped UI, headless: the solves, gate walks and graph
//! derivations a tooltip hover or a moving region pays.
//!
//! Run with `cargo test --release -p benilla-app --lib -- --ignored --nocapture resolve_bench`;
//! `WOW_LAYOUT_PROF=1` adds each solve's shape, whose `solved=`/`swept=` scope should stay small
//! while `frames=`/`anchored=` grow.

use std::time::Instant;

use benilla_ui::script::UiScript;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// A deterministic font engine: 6 units a character, so only a real text change moves the layout.
fn answer_measures(s: &mut UiScript) -> usize {
    let reqs = s.fontstrings_needing_measure();
    let n = reqs.len();
    if n > 0 {
        let answers: Vec<(u32, f32, f32, u64)> = reqs
            .iter()
            .map(|r| {
                #[allow(clippy::cast_precision_loss)]
                let w = r.text.chars().count() as f32 * 6.0;
                match r.wrap_width {
                    Some(ww) if w > ww => (r.id, ww, 12.0 * (w / ww).ceil(), r.key),
                    _ => (r.id, w, 12.0, r.key),
                }
            })
            .collect();
        s.set_measured_text_unwrapped(&answers);
    }
    n
}

/// The full shipped UI, loaded and settled.
fn settled_default_ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The UI loads on world entry, so a player always exists by then.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Probefour".into()),
            level: 60,
            ..Default::default()
        }),
    );
    super::load_default_ui(&s);
    s.set_screen_size(1600.0, 900.0);
    for _ in 0..12 {
        s.resolve();
        if answer_measures(&mut s) == 0 {
            break;
        }
    }
    s
}

/// One frame in the app's order (`extract::tick_script`): measure first, then resolve.
fn app_frame(s: &mut UiScript) {
    answer_measures(s);
    s.resolve();
}

/// A frame that also ticks the VM, so the shipped UI's own `OnUpdate` handlers run, in
/// `tick_script`'s order: tick, measure, resolve.
fn app_frame_ticked(s: &mut UiScript, dt: f32) {
    s.tick(dt);
    answer_measures(s);
    s.resolve();
}

/// A tooltip whose lines change width every call, as in a hover sweep across a bag grid.
fn install_changing_tooltip(s: &UiScript, owner: &str, func: &str) {
    s.run(&format!(
        r#"
        local a = CreateFrame("Button", "{owner}"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        {func}_n = 0
        function {func}()
            {func}_n = {func}_n + 1
            GameTooltip:SetOwner({owner}, "ANCHOR_RIGHT")
            local pad = string.rep("x", math.mod({func}_n, 17) + 1)
            GameTooltip:AddLine("Item " .. pad, 1, 1, 1)
            GameTooltip:AddDoubleLine("Shield", "Off Hand", 1, 1, 1, 1, 1, 1)
            GameTooltip:AddLine("Armor " .. pad)
            GameTooltip:AddLine("Use: restores " .. pad .. " health over 21 sec.", 0, 1, 0, 1)
            GameTooltip:Show()
        end
        "#
    ))
    .unwrap();
}

#[test]
#[ignore = "bench, run explicitly"]
fn tooltip_change_costs_a_whole_ui_solve() {
    let mut s = settled_default_ui();

    let (r0, s0) = (s.layout_rounds(), s.layout_solves());
    let t = Instant::now();
    for _ in 0..20 {
        s.resolve();
    }
    println!(
        "QUIET    {:.3} ms/resolve  solves={} rounds={}",
        ms(t) / 20.0,
        s.layout_solves() - s0,
        s.layout_rounds() - r0
    );

    install_changing_tooltip(&s, "BenchOwner", "change");
    let (r1, s1) = (s.layout_rounds(), s.layout_solves());
    let n = 50u32;
    let t = Instant::now();
    for _ in 0..n {
        s.run("change()").unwrap();
        app_frame(&mut s);
    }
    println!(
        "CHANGED  {:.3} ms/frame  solves/frame={:.2} rounds/frame={:.2}",
        ms(t) / f64::from(n),
        (s.layout_solves() - s1) as f64 / f64::from(n),
        (s.layout_rounds() - r1) as f64 / f64::from(n),
    );
}

/// Measuring before resolving makes a tooltip content change cost one layout solve; measured
/// after, the answers would force a second whole-UI solve.
#[test]
fn a_tooltip_content_change_costs_exactly_one_layout_solve() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "GateOwner", "gate_change");

    for _ in 0..4 {
        s.run("gate_change()").unwrap();
        app_frame(&mut s);
    }
    let before = s.layout_solves();
    let derives_before = s.layout_derivations();
    for _ in 0..10 {
        s.run("gate_change()").unwrap();
        app_frame(&mut s);
    }
    let solves = s.layout_solves() - before;
    assert_eq!(
        solves, 10,
        "10 content changes must cost 10 solves — one each. Two per change means the measure \
         round-trip is running AFTER the resolve again (extract::tick_script's order)."
    );
    // Nor may they derive the graph: a content change writes measure answers and re-anchors
    // pooled line regions, nothing structural.
    assert_eq!(
        s.layout_derivations() - derives_before,
        0,
        "a settled tooltip whose CONTENT changes must not re-derive the layout graph — the line \
         pool is already built, so nothing structural is happening"
    );
    // The answers landed in that one solve, or the plate would lag its text by a frame.
    assert_eq!(
        answer_measures(&mut s),
        0,
        "after a settled frame nothing may still be waiting to be measured"
    );
}

/// With nothing happening, the settled shipped UI costs zero gate walks per frame: this catches a
/// shipped handler writing a layout input every frame. `WOW_LAYOUT_TOUCH_TRACE=<secs>:<n>` names
/// the writer on a live run (`docs/CONTRIBUTING.md`, "Running it unattended").
#[test]
fn the_settled_shipped_ui_costs_no_gate_walk_on_a_quiet_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    // Grace frames: the layout gate closes one edge after measuring goes quiet, and the first
    // ticks arm handlers that have never run.
    for _ in 0..8 {
        app_frame_ticked(&mut s, 1.0 / 60.0);
    }
    // Positive control: zero walks means nothing unless the handlers ran, and
    // `BuffFrame_OnUpdate` moves `BuffFrameUpdateTime` every frame (`BuffFrame.lua:29-33`).
    s.run("__probe_bfut = BuffFrameUpdateTime")
        .expect("read the control");
    app_frame_ticked(&mut s, 1.0 / 60.0);
    s.run(
        "if BuffFrameUpdateTime == __probe_bfut then \
         error('the VM tick ran no shipped OnUpdate — this test proves nothing') end",
    )
    .expect("the shipped UI's OnUpdate handlers must run under app_frame_ticked");

    let before = s.layout_gate_walks();
    for _ in 0..20 {
        app_frame_ticked(&mut s, 1.0 / 60.0);
    }
    assert_eq!(
        s.layout_gate_walks() - before,
        0,
        "20 idle frames of the shipped UI cost {} whole-roster gate walks — something in \
         assets/ui/ is writing a layout input every frame with nothing happening. Name it with \
         WOW_LAYOUT_TOUCH_TRACE=<secs>:<n> on a live run.",
        s.layout_gate_walks() - before
    );
}

/// A region moving every frame, as `CastingBarFrame_OnUpdate` moves the spark
/// (`CastingBarFrame.lua:114`), costs one whole-roster gate walk per frame on the shipped UI. The
/// Lua ends in a frame getter, as a later handler in a live tick would, forcing the mid-tick solve;
/// the count is walks, since a walk that finds nothing moved never reaches the solve counter.
#[test]
fn a_region_moving_every_frame_costs_one_gate_walk_on_the_shipped_ui() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        BenchBar = CreateFrame("Frame", "BenchBar", UIParent)
        BenchBar:SetPoint("CENTER", 0, 0); BenchBar:SetWidth(195); BenchBar:SetHeight(13)
        BenchSpark = BenchBar:CreateTexture(nil, "OVERLAY")
        BenchSpark:SetWidth(32); BenchSpark:SetHeight(32)
        BenchSpark:SetPoint("CENTER", BenchBar, "LEFT", 0, 2)
        bench_spark_n = 0
        -- CastingBarFrame_OnUpdate's body, reduced to what touches layout: ride the spark along
        -- the bar's leading edge. The trailing getter stands in for any LATER handler in the same
        -- tick reading a frame rect — the thing that forces the mid-tick synchronous solve.
        function bench_spark_frame()
            bench_spark_n = bench_spark_n + 1
            BenchSpark:SetPoint("CENTER", BenchBar, "LEFT", bench_spark_n * 1.7, 2)
            local _ = UIParent:GetWidth()
        end
        "#,
    )
    .unwrap();

    // Settle the new bar and spark: a birth is a wide, multi-walk solve.
    for _ in 0..6 {
        s.run("bench_spark_frame()").unwrap();
        app_frame(&mut s);
    }

    let walks_before = s.layout_gate_walks();
    let solves_before = s.layout_solves();
    for step in 0..10 {
        s.run("bench_spark_frame()").unwrap();
        app_frame(&mut s);
        let (frames, regions) = s.layout_last_scope();
        assert!(
            frames < 50 && regions < 200,
            "step {step}: moving ONE region solved {frames} frames and swept {regions} regions — \
             the scope is tracking the graph, not the change"
        );
    }
    let walks = s.layout_gate_walks() - walks_before;
    let solves = s.layout_solves() - solves_before;
    assert_eq!(
        walks, 10,
        "10 frames of one moving region must cost 10 whole-roster gate walks — one each. 30 \
         means the fingerprint is hashing the dirty seeds again, so every solve outgrows the \
         value it stores and neither it nor the settling walk behind it can close tier 1. At the \
         shipped roster each extra walk is ~1 ms of CPU on every frame of every cast."
    );
    assert_eq!(
        solves, 10,
        "and each of those walks must be a real solve, not a wasted one"
    );
}

/// Those walks cost zero derivations of the layout graph: a moving region names its node, so the
/// resolve re-hashes one node, not the roster; a derivation per frame means a write site fell back
/// to the conservative `touch_layout`. The VM ticks, so the shipped handlers are held to it too,
/// and a birth must derive, which proves the counter moves.
#[test]
fn a_region_moving_every_frame_costs_no_graph_derivation_on_the_shipped_ui() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        BenchBar2 = CreateFrame("Frame", "BenchBar2", UIParent)
        BenchBar2:SetPoint("CENTER", 0, 0); BenchBar2:SetWidth(195); BenchBar2:SetHeight(13)
        BenchSpark2 = BenchBar2:CreateTexture(nil, "OVERLAY")
        BenchSpark2:SetWidth(32); BenchSpark2:SetHeight(32)
        BenchSpark2:SetPoint("CENTER", BenchBar2, "LEFT", 0, 2)
        bench_spark2_n = 0
        function bench_spark2_frame()
            bench_spark2_n = bench_spark2_n + 1
            BenchSpark2:SetPoint("CENTER", BenchBar2, "LEFT", bench_spark2_n * 1.7, 2)
            local _ = UIParent:GetWidth()
        end
        "#,
    )
    .unwrap();

    // Positive control: a birth moves the roster, so it must derive.
    let born_at = s.layout_derivations();
    for _ in 0..6 {
        s.run("bench_spark2_frame()").unwrap();
        app_frame_ticked(&mut s, 1.0 / 144.0);
    }
    assert!(
        s.layout_derivations() > born_at,
        "creating a frame and a texture must derive the graph — a birth moves the roster, which \
         is the one thing the per-node ledger cannot describe. Reading zero here means \
         `layout_derivations` never moves and the assertion below is vacuous."
    );

    let derives_before = s.layout_derivations();
    let walks_before = s.layout_gate_walks();
    for _ in 0..10 {
        s.run("bench_spark2_frame()").unwrap();
        app_frame_ticked(&mut s, 1.0 / 144.0);
    }
    let derives = s.layout_derivations() - derives_before;
    let walks = s.layout_gate_walks() - walks_before;
    assert!(
        walks >= 10,
        "the frames must still be reaching the gate at all — {walks} walks over 10 frames means \
         this test stopped exercising the path it is guarding"
    );
    assert_eq!(
        derives, 0,
        "10 frames of one moving region derived the layout graph {derives} times. Each derivation \
         is a walk of the WHOLE roster — every live frame's scale re-synced, every seed rect \
         re-filtered for liveness, all 10,438 anchored regions re-hashed and their edges rebuilt: \
         ~1.48 ms of CPU at the Stormwind pin, on every frame anything moves. A write site is \
         falling back to the conservative `Model::touch_layout` where it could name its node \
         instead."
    );
}

/// A tooltip content change solves a tooltip-sized scope on the full shipped UI, gated on counts,
/// not milliseconds, which machine state skews.
#[test]
fn a_tooltip_content_change_solves_a_tooltip_sized_scope() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "ScopeSizeOwner", "scope_size_change");

    // Settle the new owner: a birth is a wide solve, with no cached rect to trust.
    for _ in 0..6 {
        s.run("scope_size_change()").unwrap();
        app_frame(&mut s);
    }
    for step in 0..10 {
        s.run("scope_size_change()").unwrap();
        app_frame(&mut s);
        let (frames, regions) = s.layout_last_scope();
        assert!(
            frames < 100 && regions < 500,
            "step {step}: a tooltip content change solved {frames} frames and swept {regions} \
             regions — the scope is tracking the graph, not the change (measured at the time of \
             writing: 5 frames, 67 regions, against 3,003 and 8,821 in the graph)"
        );
    }
}

/// Mid-sweep, a scoped solve yields exactly the quads a from-scratch whole-graph solve does. It
/// runs on frames that never settle, where `WOW_LAYOUT_VERIFY`'s settled-frame check never fires,
/// and compares `extract()`, so only a stale rect that paints counts.
#[test]
fn a_scoped_resolve_reproduces_the_whole_graph_solve() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "ScopeOwner", "scope_change");

    for step in 0..12 {
        s.run("scope_change()").unwrap();
        app_frame(&mut s);
        let scoped = s.extract();

        // The same model, re-solved from nothing but its inputs.
        s.force_full_layout_resolve();
        app_frame(&mut s);
        let full = s.extract();

        assert!(
            scoped == full,
            "step {step}: the scoped resolve and the whole-graph resolve disagree — a node the \
             scope judged clean did move. {} quads vs {}",
            scoped.len(),
            full.len(),
        );
    }
}

/// The per-frame measure sweep's steady-state cost, every key a cache hit: 0.046 ms a sweep in
/// release. Near a millisecond, the per-row clones or hash grew or the cache stopped hitting.
#[test]
#[ignore]
fn measure_sweep_steady_state_cost() {
    let mut s = settled_default_ui();
    // One more settle so the sweep below is provably pure cache hits.
    let residual = answer_measures(&mut s);
    let t = Instant::now();
    let mut asked = 0usize;
    const N: u32 = 200;
    for _ in 0..N {
        asked += s.fontstrings_needing_measure().len();
    }
    println!(
        "measure sweep steady state: {:.4} ms/sweep ({} sweeps, {} residual requests, {} pre-settle)",
        ms(t) / f64::from(N),
        N,
        asked,
        residual,
    );
}

/// A tooltip line flipping between wrapped and plain costs zero derivations: `tooltip::append_line`
/// re-pins the wrap column only on a real change, and names its region. A real hover changes
/// shape (a wrapped "Use:" line, then a plain "Main Hand"), which a content-only change never does.
#[test]
fn a_tooltip_line_flipping_wrapped_to_plain_costs_no_graph_derivation() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        FlipOwner = CreateFrame("Button", "FlipOwner"); FlipOwner:SetPoint("CENTER", 0, 0)
        FlipOwner:SetWidth(10); FlipOwner:SetHeight(10)
        flip_n = 0
        -- The two shapes a hover alternates between. The trailing `1` on the wrap arm is
        -- `AddLine`'s positional wrapText flag (the byte-pinned 0x531630 signature) — it is what
        -- pins the line's wrap column, and what un-pins it again on the plain arm.
        function flip_change()
            flip_n = flip_n + 1
            GameTooltip:SetOwner(FlipOwner, "ANCHOR_RIGHT")
            GameTooltip:AddLine("Item head", 1, 1, 1)
            if math.mod(flip_n, 2) == 0 then
                GameTooltip:AddLine("Use: restores health over 21 sec.", 0, 1, 0, 1)
            else
                GameTooltip:AddLine("Main Hand", 1, 1, 1)
            end
            GameTooltip:Show()
        end
        "#,
    )
    .unwrap();

    // Positive control: the owner's birth and the line pool's growth move the roster and derive.
    let born_at = s.layout_derivations();
    for _ in 0..8 {
        s.run("flip_change()").unwrap();
        app_frame(&mut s);
    }
    assert!(
        s.layout_derivations() > born_at,
        "creating the owner and growing the tooltip's line pool must derive the graph — reading \
         zero here means `layout_derivations` never moves and the assertion below is vacuous."
    );

    let derives_before = s.layout_derivations();
    for _ in 0..10 {
        s.run("flip_change()").unwrap();
        app_frame(&mut s);
    }
    let derives = s.layout_derivations() - derives_before;
    assert_eq!(
        derives, 0,
        "10 hovers between a wrapped line and a plain one derived the layout graph {derives} \
         times. The wrap-pin write in `tooltip::append_line` moves ONE region's explicit size and \
         must name it (`touch_layout_region`); the conservative `touch_layout` re-derives the \
         whole roster on every hover."
    );
}

/// A sweep across owners (a new `SetOwner` target every step, as across a bag grid) costs zero
/// derivations: the retarget re-points the one node's edges instead of discarding the graph.
#[test]
fn a_hover_sweep_across_owners_costs_no_graph_derivation() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        for i = 1, 12 do
            local b = CreateFrame("Button", "SweepOwner" .. i)
            b:SetPoint("CENTER", 0, 0); b:SetWidth(10); b:SetHeight(10)
        end
        sweep_n = 0
        -- A different owner AND a different line shape every step: the two halves of a real sweep
        -- across a bag grid, where each slot is its own button and each item its own plate.
        function sweep_change()
            sweep_n = sweep_n + 1
            local slot = math.mod(sweep_n, 12) + 1
            GameTooltip:SetOwner(getglobal("SweepOwner" .. slot), "ANCHOR_RIGHT")
            GameTooltip:AddLine("Item " .. slot, 1, 1, 1)
            if math.mod(sweep_n, 2) == 0 then
                GameTooltip:AddLine("Use: restores health over 21 sec.", 0, 1, 0, 1)
            else
                GameTooltip:AddLine("Main Hand", 1, 1, 1)
            end
            GameTooltip:Show()
        end
        "#,
    )
    .unwrap();

    // Positive control: twelve births and the line pool's growth move the roster and derive.
    let born_at = s.layout_derivations();
    for _ in 0..40 {
        s.run("sweep_change()").unwrap();
        app_frame(&mut s);
    }
    assert!(
        s.layout_derivations() > born_at,
        "creating twelve owners and growing the line pool must derive the graph — reading zero \
         here means `layout_derivations` never moves and the assertion below proves nothing."
    );

    // Two laps of the twelve owners: every step re-hovers a button the graph already knows.
    let derives_before = s.layout_derivations();
    for _ in 0..24 {
        s.run("sweep_change()").unwrap();
        app_frame(&mut s);
    }
    let derives = s.layout_derivations() - derives_before;
    assert_eq!(
        derives, 0,
        "24 hovers across 12 owners derived the layout graph {derives} times — one per slot \
         crossed. `SetOwner` re-points ONE node's anchor and must patch that node's edges \
         (`Model::touch_layout_retarget_frame`); the conservative touch re-derives the whole \
         roster on every slot the cursor passes over."
    );
}

/// An action-bar hover sweep costs zero derivations. With `UberTooltips` at its 1.12 default "1",
/// an action button anchors through `GameTooltip_SetDefaultAnchor` (`ActionButton.lua:365-367`):
/// `SetOwner(owner, "ANCHOR_NONE")`, which drops the tooltip's anchors (`0x52fe90` reaches
/// `0x52fec2 call 0x767ed0` for every mode but PRESERVE), then an explicit `SetPoint`.
#[test]
fn an_action_bar_hover_sweep_costs_no_graph_derivation() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        for i = 1, 12 do
            local b = CreateFrame("Button", "BarOwner" .. i)
            b:SetPoint("CENTER", 0, 0); b:SetWidth(36); b:SetHeight(36)
        end
        bar_n = 0
        -- `GameTooltip_SetDefaultAnchor`'s body, which is what every action button actually runs:
        -- own the tooltip WITHOUT an anchor, then point it by hand.
        function bar_hover()
            bar_n = bar_n + 1
            local owner = getglobal("BarOwner" .. (math.mod(bar_n, 12) + 1))
            GameTooltip:SetOwner(owner, "ANCHOR_NONE")
            GameTooltip:ClearAllPoints()
            GameTooltip:SetPoint("BOTTOMRIGHT", "UIParent", "BOTTOMRIGHT", -70, 80)
            GameTooltip:AddLine("Spell " .. bar_n, 1, 1, 1)
            if math.mod(bar_n, 2) == 0 then
                GameTooltip:AddLine("Blasts the enemy for 20 damage.", 1, 1, 1, 1)
            else
                GameTooltip:AddLine("Instant", 1, 1, 1)
            end
            GameTooltip:Show()
        end
        "#,
    )
    .unwrap();

    // Positive control: the births and the line pool's growth are structural.
    let born_at = s.layout_derivations();
    for _ in 0..40 {
        s.run("bar_hover()").unwrap();
        app_frame(&mut s);
    }
    assert!(
        s.layout_derivations() > born_at,
        "creating twelve buttons must derive the graph — zero here makes the assertion vacuous."
    );

    let derives_before = s.layout_derivations();
    for _ in 0..24 {
        s.run("bar_hover()").unwrap();
        app_frame(&mut s);
    }
    let derives = s.layout_derivations() - derives_before;
    assert_eq!(
        derives, 0,
        "24 action-bar hovers derived the layout graph {derives} times. `SetOwner`'s ANCHOR_NONE \
         arm clears the anchors (`0x52fec2 call 0x767ed0`), and so does the `ClearAllPoints()` \
         on `GameTooltip_SetDefaultAnchor`'s next line: each is a retarget to the EMPTY target \
         set and must name its node like any other."
    );
}

/// A hover that owns the tooltip with an anchor, then clears and re-points it by hand, costs zero
/// derivations: the bag-addon idiom of `SetOwner(item, "ANCHOR_RIGHT")`, then `ClearAllPoints()`,
/// `GetLeft()` and `SetPoint`. Clearing live anchors is a retarget onto the empty set, and the
/// `GetLeft()` settles the layout inside the handler.
#[test]
fn a_bag_addon_hover_sweep_costs_no_graph_derivation() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    s.run(
        r#"
        for i = 1, 12 do
            local b = CreateFrame("Button", "BagOwner" .. i)
            b:SetPoint("CENTER", 0, 0); b:SetWidth(37); b:SetHeight(37)
        end
        bag_n = 0
        function bag_hover()
            bag_n = bag_n + 1
            local item = getglobal("BagOwner" .. (math.mod(bag_n, 12) + 1))
            -- ContainerFrameItemButton_OnEnter's arm: an ANCHORED SetOwner.
            GameTooltip:SetOwner(item, "ANCHOR_RIGHT")
            GameTooltip:AddLine("Item " .. bag_n, 1, 1, 1)
            if math.mod(bag_n, 2) == 0 then
                GameTooltip:AddLine("Use: restores health over 21 sec.", 0, 1, 0, 1)
            else
                GameTooltip:AddLine("Main Hand", 1, 1, 1)
            end
            GameTooltip:Show()
            -- …then Bagnon_AnchorTooltip: drop the anchors it just set, ASK A RESOLVED EDGE
            -- (which settles the layout on the spot), and re-point by hand.
            GameTooltip:ClearAllPoints()
            local left = item:GetLeft() or 0
            if left < (UIParent:GetRight() / 2) then
                GameTooltip:SetPoint("TOPLEFT", item, "BOTTOMRIGHT")
            else
                GameTooltip:SetPoint("TOPRIGHT", item, "BOTTOMLEFT")
            end
        end
        "#,
    )
    .unwrap();

    // Positive control: the births and the line pool's growth are structural.
    let born_at = s.layout_derivations();
    for _ in 0..40 {
        s.run("bag_hover()").unwrap();
        app_frame(&mut s);
    }
    assert!(
        s.layout_derivations() > born_at,
        "creating twelve slots must derive the graph — zero here makes the assertion vacuous."
    );

    let derives_before = s.layout_derivations();
    for _ in 0..24 {
        s.run("bag_hover()").unwrap();
        app_frame(&mut s);
    }
    let derives = s.layout_derivations() - derives_before;
    assert_eq!(
        derives, 0,
        "24 bag-slot hovers derived the layout graph {derives} times — one per slot crossed. \
         `ClearAllPoints` drops a node's whole anchor-target set, which is a retarget onto the \
         EMPTY set and names its node like any other."
    );
}
