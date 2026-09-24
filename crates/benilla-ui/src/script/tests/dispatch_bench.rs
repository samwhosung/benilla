//! What one widget method lookup through the wrapper's `__index` costs: every call pays a hit,
//! and a miss (the duck-type probe `if f.SetValue then`) walks every registry in the kind chain.
//!
//! Run: `cargo test --release -p benilla-ui --lib -- --ignored --nocapture dispatch_bench`.

use std::time::Instant;

use super::common::script;

/// Lookups per timed loop, enough that the loop's own overhead is noise at nanosecond scale.
const N: usize = 400_000;

/// Timed repeats per row; each row reports the fastest.
const ROUNDS: usize = 5;

fn ns_per(t: Instant, n: usize) -> f64 {
    t.elapsed().as_secs_f64() * 1e9 / n as f64
}

#[test]
#[ignore = "bench — run explicitly with --release --nocapture"]
fn dispatch_bench() {
    let s = script();
    s.run(
        r#"
        F = CreateFrame("Frame")
        B = CreateFrame("Button")
        T = F:CreateTexture()
        S = F:CreateFontString()
        "#,
    )
    .expect("fixtures");

    // The minimum of `ROUNDS` runs: a mean would measure whatever else was running.
    let floor = |what: &str, chunk: &str| {
        let mut best = f64::INFINITY;
        for _ in 0..ROUNDS {
            let t = Instant::now();
            s.run(chunk).expect("bench chunk");
            best = best.min(ns_per(t, N));
        }
        println!("[dispatch] {what:>26}  {best:7.1} ns");
    };

    // The floor: a plain table with a table `__index`, like the reference's flat C method probe.
    floor(
        "baseline (table meta)",
        &format!(
            "local base = {{ GetWidth = function() end }}
             local o = setmetatable({{}}, {{ __index = base }})
             for _ = 1, {N} do local m = o.GetWidth end"
        ),
    );
    floor(
        "Frame hit",
        &format!("for _ = 1, {N} do local m = F.GetWidth end"),
    );
    floor(
        "Frame miss",
        &format!("for _ = 1, {N} do local m = F.NoSuchMethod end"),
    );
    floor(
        "Button hit",
        &format!("for _ = 1, {N} do local m = B.GetWidth end"),
    );
    floor(
        "Button own hit",
        &format!("for _ = 1, {N} do local m = B.SetText end"),
    );
    floor(
        "Button miss",
        &format!("for _ = 1, {N} do local m = B.NoSuchMethod end"),
    );
    floor(
        "Texture hit",
        &format!("for _ = 1, {N} do local m = T.GetWidth end"),
    );
    floor(
        "Texture miss",
        &format!("for _ = 1, {N} do local m = T.NoSuchMethod end"),
    );
    floor(
        "FontString hit",
        &format!("for _ = 1, {N} do local m = S.GetWidth end"),
    );

    // Whole calls: `GetFrameLevel` is a frame-only binding (one Rust hop); `GetWidth` is one of
    // `region_map`'s shared names, whose bridge re-enters Lua for the per-side arm.
    floor(
        "GetFrameLevel() call",
        &format!("for _ = 1, {N} do local l = F:GetFrameLevel() end"),
    );
    floor(
        "GetWidth() call (bridged)",
        &format!("for _ = 1, {N} do local w = F:GetWidth() end"),
    );
    floor(
        "SetPoint() call (bridged)",
        &format!("for _ = 1, {N} do F:SetPoint(\"CENTER\", 0, 0) end"),
    );

    // Every geometry getter runs `layout_methods::settle`: free while nothing has moved, a whole
    // `resolve_layout` after any layout write, so read-after-write pays one per read.
    floor(
        "write only",
        &format!("for i = 1, {N} do F:SetPoint(\"CENTER\", i, 0) end"),
    );
    floor(
        "write + read (settles)",
        &format!("for i = 1, {N} do F:SetPoint(\"CENTER\", i, 0) local w = F:GetWidth() end"),
    );
    floor(
        "write + read x3 (settles)",
        &format!(
            "for i = 1, {N} do F:SetPoint(\"CENTER\", i, 0) local w = F:GetWidth() local h = F:GetHeight() local l = F:GetLeft() end"
        ),
    );

    // Controls: `__direct` is one mlua closure, `__wrapped` one that calls another through
    // `Function::call` as `region_map`'s shared arm does; their gap is one re-entrant Lua call.
    {
        let lua = s.lua();
        let direct = lua
            .create_function(|_, n: i64| Ok(n + 1))
            .expect("direct fn");
        lua.globals().set("__direct", direct.clone()).expect("set");
        let wrapped = lua
            .create_function(move |_, n: i64| direct.call::<i64>(n))
            .expect("wrapped fn");
        lua.globals().set("__wrapped", wrapped).expect("set");

        // The same pair in the bridge's shape: a variadic `MultiValue` led by the receiver table.
        let mv_direct = lua
            .create_function(|_, args: mlua::MultiValue| Ok(args.len() as i64))
            .expect("mv direct");
        lua.globals()
            .set("__mv_direct", mv_direct.clone())
            .expect("set");
        let mv_relay = lua
            .create_function(move |_, args: mlua::MultiValue| {
                mv_direct.call::<mlua::MultiValue>(args)
            })
            .expect("mv relay");
        lua.globals().set("__mv_relay", mv_relay).expect("set");
    }
    floor(
        "ctrl: 1 mlua hop",
        &format!("for _ = 1, {N} do local v = __direct(1) end"),
    );
    floor(
        "ctrl: mlua hop + relay",
        &format!("for _ = 1, {N} do local v = __wrapped(1) end"),
    );
    floor(
        "ctrl: variadic hop",
        &format!("for _ = 1, {N} do local v = __mv_direct(F, 1, 2) end"),
    );
    floor(
        "ctrl: variadic hop + relay",
        &format!("for _ = 1, {N} do local v = __mv_relay(F, 1, 2) end"),
    );
}
