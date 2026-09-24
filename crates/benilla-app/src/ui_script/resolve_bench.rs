//! What a UI change costs on the **full shipped UI** — the headless twin of the live hover
//! recorder (`hover_log`), and the loop that made the tooltip-hover cost reproducible without a
//! play session.
//!
//! The live runs said a hover frame that ran the layout fixpoint cost ~10 ms against ~0.2 ms for
//! one that did not, but neither the synthetic tooltip probes nor the shipped `BagFrame.xml` hover
//! loop reproduced it: with content that does not MOVE, the change gate absorbs everything. The
//! missing ingredient is that a real hover sweep shows a *different item every frame* — line widths
//! that actually change. That is what [`tooltip_change_costs_a_whole_ui_solve`] drives, and with it
//! the cost reproduces headlessly and can be measured against a fix.
//!
//! Run with `cargo test --release -p benilla-app --lib -- --ignored --nocapture resolve_bench`;
//! `WOW_LAYOUT_PROF=1` adds the per-solve shape — and since decision 1350 its `solved=`/`swept=`
//! columns are the ones to read: they are the SCOPE of the solve, and 1350's whole claim is that
//! they stay a handful while `frames=`/`anchored=` grow without bound. A solve whose scope tracks
//! the graph again is precisely the regression this file exists to catch.

use std::time::Instant;

use benilla_ui::script::UiScript;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// The deterministic stand-in for the app's font engine: every string has one size, so a measure
/// answer is stable across frames and only real text changes move the layout.
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

/// The full shipped UI, loaded and settled — the graph a real hover actually pays for.
fn settled_default_ui() -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.set_screen_size(1600.0, 900.0);
    // The in-game UI materializes on world entry (1051), so a player always exists by the time the
    // manifest loads — and the stock macro window's character tab formats `UnitName("player")`
    // into its label inside its own OnLoad. A manifest load with no player is a state the client
    // never reaches.
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

/// One frame in the app's own order (`extract::tick_script`): measure FIRST, then resolve.
fn app_frame(s: &mut UiScript) {
    answer_measures(s);
    s.resolve();
}

/// A frame that also **ticks the VM**, i.e. runs the shipped UI's own `OnUpdate` handlers, in
/// `tick_script`'s real order (tick → measure → resolve).
///
/// [`app_frame`] deliberately models only the measure/resolve half, and the tooltip benches stand
/// in for the handler by calling it themselves. That is fine when the test IS the driver — but a
/// test asking "is anything in the shipped UI writing layout on its own?" has to let the shipped
/// UI actually run, or it answers a question nobody asked.
fn app_frame_ticked(s: &mut UiScript, dt: f32) {
    s.tick(dt);
    answer_measures(s);
    s.resolve();
}

/// A tooltip whose lines change width every call — the hover sweep across a bag grid, where each
/// slot holds a different item.
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

/// The gate the measure-first order exists to hold: a tooltip content change costs the frame
/// **one** layout solve, not two.
///
/// The old order resolved, then measured, then had to resolve AGAIN because the answers moved the
/// anchor solve's read set — and on the shipped UI each of those walks 2,164 frames and sweeps
/// 6,297 regions per round, for a change that touches ten FontStrings inside one frame. Nothing
/// about the second solve is tooltip-sized, and nothing about it is cheap.
#[test]
fn a_tooltip_content_change_costs_exactly_one_layout_solve() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "GateOwner", "gate_change");

    // Settle the newly-created owner + the tooltip's first content.
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
    // …and none of those ten may DERIVE the graph. This is the second shape of
    // the same law `a_region_moving_every_frame_costs_no_graph_derivation_on_the_shipped_ui`
    // guards, and it is worth asserting separately because it arrives by a different road: a
    // hover sweep churns tooltip CONTENT, so its per-frame writes are measure answers and
    // re-anchors of pooled line regions rather than one moving texture. The bag-hover re-enter
    // loop is the most common interactive path in the client; a derivation per frame here is
    // 1.48 ms every frame the cursor sits over a bag.
    assert_eq!(
        s.layout_derivations() - derives_before,
        0,
        "a settled tooltip whose CONTENT changes must not re-derive the layout graph — the line \
         pool is already built, so nothing structural is happening (decision 1388)"
    );
    // …and the measure answers must have landed in that one solve: a frame that resolved before
    // measuring leaves the fresh text unmeasured until the next frame, which is the visible half
    // of the same bug (a tooltip plate one frame behind its content).
    assert_eq!(
        answer_measures(&mut s),
        0,
        "after a settled frame nothing may still be waiting to be measured"
    );
}

/// **The idle law**: the settled shipped UI, with nothing happening, must cost
/// **zero** gate walks per frame — not one cheap one, zero.
///
/// The other guards in this file pin what the ENGINE charges for a change. This one pins that
/// nothing in the shipped UI is quietly making a change every frame in the first place, which is
/// the half a law about the engine cannot see. It is the tripwire for a new always-on toucher
/// being added to `assets/ui/` — the worst shape of this bug, because it costs on *every* frame of
/// *every* session rather than only while something is animating. The sweep behind 1385 found real
/// candidates for it (`TemporaryEnchantFrame`'s OnUpdate is resident for the whole session; the
/// measure round-trip used to bump the epoch on every same-size re-measure), so the shape is not
/// hypothetical.
///
/// A failure here does not name the culprit — `WOW_LAYOUT_TOUCH_TRACE=<secs>:<n>` does, by
/// backtracing the touch sites on a live run.
#[test]
fn the_settled_shipped_ui_costs_no_gate_walk_on_a_quiet_frame() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    // Ticked frames, so the shipped UI's own OnUpdate handlers run — the whole point (see
    // `app_frame_ticked`). A couple of frames of grace first: `settled_default_ui` stops as soon
    // as the measure round-trip goes quiet, which is one edge earlier than the layout gate closing
    // behind it, and the first ticked frames arm handlers that have never run.
    for _ in 0..8 {
        app_frame_ticked(&mut s, 1.0 / 60.0);
    }
    // POSITIVE CONTROL. "Zero walks" only means anything if the shipped UI's handlers actually
    // ran — a tick that silently fired nothing would also read zero, and would make this test a
    // green light for exactly the regression it exists to catch. `BuffFrameUpdateTime` is advanced
    // by `BuffFrame_OnUpdate` on every frame (down by `elapsed`, or up by TOOLTIP_UPDATE_TIME when
    // it crosses), so one tick must move it.
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
         WOW_LAYOUT_TOUCH_TRACE=<secs>:<n> on a live run (decision 1385).",
        s.layout_gate_walks() - before
    );
}

/// **The castbar law** (ledger B283), on the full shipped UI: a region that moves
/// every frame must cost **one** whole-roster gate walk per frame.
///
/// This is the guard for the bug class 1383 named and the castbar then hit. `CastingBar.xml`'s
/// OnUpdate slides `CastingBarSpark` one `SetPoint` per frame for the length of every cast — the
/// reference's own architecture, and the classic addon idiom besides. Measured live at the
/// Stormwind gates it cost **+4.2 ms of CPU per frame** on a default UI: three walks per frame at
/// ~1.0–1.4 ms each (10,438 anchored regions re-hashed every walk), because the fingerprint was
/// hashed over the 0294 seeds as well as the inputs and so no walk could ever close tier 1.
///
/// Three things make this the falsifier rather than a smoke check:
/// * it runs on the **shipped** UI, so the whole-roster term is real — a synthetic two-frame model
///   shows the same *counts* and none of the cost;
/// * the Lua body ends in a frame getter, because that is what a live tick does. A later handler's
///   `GetWidth` forces the synchronous mid-tick solve; modelling the frame as a lone `resolve()`
///   hides two of the three walks;
/// * it asserts on `layout_gate_walks`, not `layout_solves`. A walk that concludes "nothing moved"
///   pays the identical preamble and never reaches the solve counter — one of the castbar's three
///   walks was exactly that, so the old counter under-reported the bug by a third.
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

    // Settle the newly-created bar and spark — a birth is legitimately a wide, multi-walk solve.
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
             the scope is tracking the graph, not the change (decision 1350)"
        );
    }
    let walks = s.layout_gate_walks() - walks_before;
    let solves = s.layout_solves() - solves_before;
    assert_eq!(
        walks, 10,
        "10 frames of one moving region must cost 10 whole-roster gate walks — one each. 30 \
         means the fingerprint is hashing the 0294 seeds again, so every solve outgrows the value \
         it stores and neither it nor the settling walk behind it can close tier 1 (decision \
         1385). At the shipped roster each extra walk is ~1 ms of CPU on every frame of every cast."
    );
    assert_eq!(
        solves, 10,
        "and each of those walks must be a real solve, not a wasted one"
    );
}

/// **The castbar law, part two**: those ten gate walks must cost **zero**
/// derivations of the layout graph.
///
/// 1385 got the moving spark from three whole-roster walks per frame down to one, and the test
/// above is its guard. It did nothing about what the surviving walk *costs*: measured at the
/// Stormwind pin (3,218 frames, 10,438 anchored regions) the preamble ran 1.48 ms, 79% of it in two
/// phases — re-hashing every anchored region (919 µs) and re-filtering every seed rect for liveness
/// (255 µs) — to rediscover a graph whose SHAPE had not changed. At 144 fps that is a fifth of the
/// frame, on every frame of every cast, and it is paid by anything that animates: floating combat
/// text moves up to twenty strings a frame, and a frame getter inside an OnUpdate (`GetWidth` calls
/// `settle`) buys another one each.
///
/// So the counter this asserts on is not `layout_gate_walks` but `layout_derivations`. A moving
/// region names its node, the ledger vouches for the cached roster and edges, and the resolve
/// re-hashes one node instead of 13,656. Zero is the whole claim: **one** derivation per frame
/// would mean a write site somewhere fell back to the conservative `touch_layout` and the ledger
/// is being poisoned every frame, which reads as a perfectly healthy walk count and costs the
/// entire 1.48 ms.
///
/// The positive control is not optional here. "Zero derivations" is exactly what a permanently
/// broken counter also reports, so the test first proves the counter can move — a frame BIRTH is
/// structural by construction and must derive.
///
/// It ticks the VM (`app_frame_ticked`) rather than driving the resolve alone, so the shipped UI's
/// own `OnUpdate` handlers run on every one of these frames. That is the difference between "one
/// synthetic write can use the ledger" and "the ledger survives the real UI": a single handler
/// anywhere in `assets/ui/` falling back to a conservative touch every frame would poison it for
/// everything else, and only a ticked frame can see that.
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

    // The positive control, taken across the birth above: a new frame and a new region move the
    // roster, so they MUST derive. If this reads zero the counter is dead and the real assertion
    // below proves nothing.
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
         (decision 1388)."
    );
}

/// The perf half of decision 1350, asserted as a COUNT: a tooltip content change must cost a
/// solve whose SCOPE is tooltip-sized, on a UI of 3,000 frames and 8,800 anchored regions.
///
/// Milliseconds are deliberately not the gate here. This exact cost has now been chased three
/// times — 0735 (the measure cache), 0771 (the double solve), and this — and on two of those the
/// ms column was contaminated by machine state (0713's stall class) while the work counts were
/// clean. The bound is also what makes the regression *reportable*: 1350's pin found the sweep's
/// cost tripling with no counter behind it, because `regions_swept` counted the entries the sweep
/// skipped as well as the ones it resolved.
#[test]
fn a_tooltip_content_change_solves_a_tooltip_sized_scope() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "ScopeSizeOwner", "scope_size_change");

    // Settle the newly-created owner and the tooltip's first content — a birth is legitimately a
    // wide solve (a new node has no cached rect to trust).
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

/// The scoped resolve's falsifier: on the shipped UI, mid-sweep, a solve that
/// touched only the dirty closure must produce **exactly** the rects a from-scratch whole-graph
/// solve produces.
///
/// This is the gate for the whole change, and it is deliberately run on the frames that never
/// settle — a hover sweep changes content every frame, so `WOW_LAYOUT_VERIFY`'s settled-frame
/// comparison never fires there, and those are the only frames the scope was built for. The
/// comparison is `extract()`, not an internal rect map: a stale rect that no quad carries is not a
/// bug, and a stale rect that one does carry is the bug in the form the screen would show it.
#[test]
fn a_scoped_resolve_reproduces_the_whole_graph_solve() {
    benilla_formats::wow_data_or_skip!();
    let mut s = settled_default_ui();
    install_changing_tooltip(&s, "ScopeOwner", "scope_change");

    for step in 0..12 {
        // A content change, resolved the way the app resolves it — scoped.
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

/// The steady-state cost of the per-frame measure sweep alone: the UI settled, every FontString
/// measured, every key a cache hit — what does *asking* cost? It is paid on EVERY frame, quiet
/// ones included, which made it a suspect on 1350's successor list — and this bench closed it:
/// 0.046 ms/sweep on the full shipped UI in release (2026-08-15; the ~1.0 ms once quoted was a
/// dev-build number). Kept as the lane's regression watch: if this climbs toward a milli-
/// second, the sweep's per-row clones/hash grew or the cache stopped hitting.
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

/// **The hover-shape law** (ledger B06): a tooltip line that changes ROLE between
/// WRAPPED and PLAIN must cost **zero** derivations of the layout graph.
///
/// `a_tooltip_content_change_costs_exactly_one_layout_solve` above asserts the same zero and could
/// not see this, because the tooltip it drives always has the same SHAPE — the same lines, the
/// same wrap flags, only the widths move. A real hover sweep changes shape on every step: one
/// item's line 2 is a wrapping "Use:" description, the next one's is a plain "Main Hand", and a
/// spell's line 2 is a plain mana cost where the next spell's is a wrapping tooltip body.
///
/// `append_line` re-pins the line's wrap column from that flag on every append, and the write is
/// gated on a real change — so it fires exactly on the shape flip and nowhere else. It reached the
/// conservative `Model::touch_layout` (it predates 1388 and was never migrated, being an internal
/// write rather than a `SetWidth` binding), and the whole graph was therefore re-derived on every
/// hover from one item to a differently-shaped one: every live frame's scale re-synced, every seed
/// rect re-filtered, every anchored region re-hashed and its edges rebuilt. Live at 12,465
/// anchored regions the preamble read `incr=0` on every content-change frame.
///
/// The positive control is not optional: "zero derivations" is what a permanently broken counter
/// reports too, so the flip is driven for a while BEFORE the window opens (a birth is legitimately
/// structural) and the counter is proved able to move.
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

    // The positive control, taken across the owner's birth and the line pool's growth — both move
    // the roster, which is the one thing a per-node ledger cannot describe, so both MUST derive.
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
         whole roster on every hover (decision 1388)."
    );
}

/// **The hover-sweep law** (ledger B06): sweeping the cursor across a bag grid or a
/// spellbook page — a NEW tooltip OWNER on every step — must cost **zero** derivations of the
/// layout graph.
///
/// This is the shape the director's report actually has, and it is the one the guard above still
/// could not see: that one keeps a single owner and only changes the tooltip's content. A real
/// sweep does both, and the owner half is the more expensive of the two, because
/// `GameTooltip:SetOwner` RETARGETS the plate's anchor at the button under the cursor. 1388
/// classified a retarget as structural — an edge disappears and another appears, which no per-node
/// hash can describe — and answered it by throwing the cached graph away. That answer cost a full
/// whole-roster derivation on every slot crossed. 1625 keeps the graph and re-points the one
/// node's edges instead.
///
/// The correctness half of that patch is guarded where it can be *seen*, on rects, by
/// `benilla-ui`'s `a_retargeted_anchor_follows_its_new_target` (and by `WOW_LAYOUT_VERIFY` behind
/// every one of that crate's tests). This one guards the COST, on the shipped UI, where the roster
/// is big enough for the derivation to matter.
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

    // The positive control: twelve owner births and the tooltip's line pool growing are all
    // roster moves, so the settling pass MUST derive. Zero here would make the assertion vacuous.
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

    // Two full laps of the twelve owners, so every step is a re-hover of a button the graph
    // already knows: nothing structural is left to discover.
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
         roster on every slot the cursor passes over (decisions 1388, 1625)."
    );
}

/// **The action-bar hover law** (ledger B06): sweeping the cursor across ACTION BAR
/// buttons must cost **zero** derivations of the layout graph.
///
/// The two guards above drive `SetOwner(button, "ANCHOR_RIGHT")` — the bag slot's idiom. The bars
/// do not use it. With `UberTooltips` at its shipped default of `"1"`, every action button routes
/// through `GameTooltip_SetDefaultAnchor` (stock `ActionButton_SetTooltip`, and the stance/pet/bonus bars the
/// same), which is `SetOwner(owner, "ANCHOR_NONE")` followed by an explicit `SetPoint` — a
/// completely different arm of the same verb, and the one that DROPS the tooltip's anchors
/// (`0x52fe90`'s mode-7 leg reaches `0x52fec2 call 0x767ed0` like every mode but PRESERVE — 2176
/// re-confirmed that at the bytes after 2142 predicted the opposite).
///
/// That drop took the conservative touch, so it re-derived the whole graph on every button the
/// cursor crossed — and on nothing else, which is why it survived two rounds of fixing and a
/// live probe: the director's own recording is what named it, every derive frame owned by a
/// `MultiBarBottomLeftButton*` or `BonusActionButton*`. The lesson worth keeping is in the guards
/// as much as the fix: a hover guard that only drives one of two anchor idioms is testing the
/// idiom, not the gesture.
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

    // Positive control across the births and the line pool's growth — both structural.
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
        "24 action-bar hovers derived the layout graph {derives} times. The `ClearAllPoints()` on \
         `GameTooltip_SetDefaultAnchor`'s next line is a retarget to the EMPTY target set and must \
         name its node like any other (decision 1630, extending 1625; 2176 moved the emptying \
         itself off `SetOwner`'s ANCHOR_NONE arm, which the reference leaves alone)."
    );
}

/// **The bag-addon hover law** (ledger B06's third idiom): a hover that owns the
/// tooltip WITH an anchor and then re-points it by hand must cost **zero** derivations.
///
/// The guard above drives `SetOwner(owner, "ANCHOR_NONE")` + `ClearAllPoints()`, and it has always
/// passed — because ANCHOR_NONE already dropped the anchors, so the `ClearAllPoints()` after it
/// finds an empty list and touches nothing at all. It is the third idiom that was never driven:
///
/// ```lua
/// -- Bagnon_Core/core/Item.lua -> Bagnon_Core/core/Utility.lua
/// ContainerFrameItemButton_OnEnter(item)      -- SetOwner(item, "ANCHOR_RIGHT"): sets an anchor
/// Bagnon_AnchorTooltip(item)                  -- ClearAllPoints(), then GetLeft(), then SetPoint
/// ```
///
/// `ClearAllPoints()` with anchors present took the conservative touch, and the `GetLeft()` on the
/// next line settles the layout right there — so the whole graph was re-derived INSIDE the handler,
/// once per bag slot the cursor crossed. Measured live on `Probetwo` with the director's AddOns
/// folder, `WOW_UI_HANDLERS=8` over a 60 Hz sweep of twelve `BagnonItem*` buttons: `OnEnter` self
/// 1.75 ms/frame and `[layout-derive]` naming this site in three of its four samples, against
/// 0.43 ms/frame of total `tick` for the same sweep over stock `ContainerFrame1Item*` with no
/// addons. `derives/frame` read 0.88 with Bagnon and 0.00 without.
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

    // Positive control across the births and the line pool's growth — both structural.
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
         EMPTY set and names its node like any other (decision 2114, completing 1625/1630)."
    );
}
