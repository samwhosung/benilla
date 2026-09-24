//! **What one widget method lookup costs** — the bench behind the `__index` table.
//!
//! Every `frame:SetPoint(...)` in FrameXML and in every addon begins with a table index on the
//! wrapper, which misses and falls to the metatable's `__index`. That path is on the hottest edge
//! this engine has: Questie redraws its map notes with ~19 widget calls per cluster over hundreds
//! of clusters, and a live sample of a quest-accept spike put **20 % of the whole UI tick** inside
//! the metamethod dispatch alone — before any method body ran.
//!
//! The two rows below are the measurement that claim rests on, and the regression guard for it:
//!
//! - `miss` — `local m = f.NoSuchMethod`, the duck-type probe addons use (`if f.SetValue then`).
//!   It is the WORST case: a dispatcher walks every registry in the kind chain and then the shared
//!   table before answering nil.
//! - `hit` — `local m = f.GetWidth`, the lookup every real call pays.
//!
//! Run: `cargo test --release -p benilla-ui --lib -- --ignored --nocapture dispatch_bench`.

use std::time::Instant;

use super::common::script;

/// Lookups per timed loop — big enough that the loop's own overhead is a rounding error at the
/// nanosecond scale these rows live at.
const N: usize = 400_000;

/// Timed repeats per row; the reported number is the fastest (see [`dispatch_bench`]'s `floor`).
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

    // A plain Lua table with a table `__index` — the floor this path could reach, and what the
    // reference client's flat `{name, lua_CFunction}` probe costs in C.
    // **The MINIMUM of [`ROUNDS`] runs, never the mean.** This machine builds while it measures —
    // another build on the same machine saturates ten cores — and a mean of a contended loop reads
    // whatever else was running. The minimum is the closest thing to the uncontended cost that a
    // shared machine can report, and it is what makes two rows of one run comparable at all.
    let floor = |what: &str, chunk: &str| {
        let mut best = f64::INFINITY;
        for _ in 0..ROUNDS {
            let t = Instant::now();
            s.run(chunk).expect("bench chunk");
            best = best.min(ns_per(t, N));
        }
        println!("[dispatch] {what:>26}  {best:7.1} ns");
    };

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

    // The whole call, not just the lookup — what an addon's `f:GetWidth()` actually costs, split
    // so the two taxes are separable: `GetFrameLevel` is a frame-only binding and measures one
    // Rust hop, while `GetWidth` is one of [`crate::script::region_map`]'s 19 shared names and
    // pays a SECOND hop — the bridge re-enters Lua to call the per-side arm.
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

    // **The settle tax.** Every geometry GETTER (`GetWidth`, `GetHeight`, `GetLeft`, `GetPoint`,
    // `GetCenter`) calls `layout_methods::settle`, i.e. a whole `resolve_layout`. The tier-1 epoch
    // gate makes that free while nothing has moved — which is what the `GetWidth() call` row above
    // measures — but ANY layout write in between reopens it, so the read-after-write idiom pays a
    // resolve per read. That idiom is not exotic: Astrolabe repositions a minimap icon with
    // `GetWidth` → `ClearAllPoints` → `SetPoint`, per icon, and Questie places dozens.
    //
    // Here the model holds four frames, so a resolve is nearly free and the gap is the mechanism
    // alone. In a live client the roster is thousands of frames and ten thousand anchored regions
    // — the same gap, three orders of magnitude wider.
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

    // **The control that says WHERE the bridged calls' extra microsecond goes.** `__direct` is one
    // mlua closure; `__wrapped` is one mlua closure that calls another through `Function::call` —
    // the exact shape `region_map`'s shared arm has. The gap between these two rows is what a
    // re-entrant Lua call costs, and it is the number that decides whether those 19 names are worth
    // restructuring into a single Rust body with an internal branch (the reference's own shape: one
    // C function doing a virtual dispatch on the receiver).
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

        // The same pair again in the SHAPE the bridge actually has: a variadic `MultiValue` whose
        // first value is the receiver table. `Table` and `MultiValue` are the two conversions the
        // scalar controls above leave out, and between them they are most of the gap.
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
