//! Structural layout writes (a node born, freed, re-owned or re-pointed) through the cached graph:
//! every resolve must equal a from-scratch solve of the same model, rects and graph alike.

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

/// A region's owner supplies its scale, so re-owning it under a scaled frame moves its rect.
#[test]
fn a_region_reowned_under_a_scaled_frame_takes_the_new_scale() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        x = CreateFrame("Frame", "X", nil); x:SetWidth(100); x:SetHeight(100)
        x:SetPoint("CENTER", nil, "CENTER", 0, 0)
        a = CreateFrame("Frame", "A", nil); a:SetWidth(10); a:SetHeight(10)
        a:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, 0)
        b = CreateFrame("Frame", "B", nil); b:SetWidth(10); b:SetHeight(10)
        b:SetPoint("TOPLEFT", nil, "TOPLEFT", 0, 0); b:SetScale(2)
        t = a:CreateTexture(nil, "ARTWORK"); t:SetTexture(1, 1, 1)
        t:SetWidth(8); t:SetHeight(8); t:SetPoint("TOPLEFT", x, "TOPLEFT", 5, -5)
        "#,
    )
    .expect("setup");
    s.resolve();
    s.resolve();
    s.run("t:SetParent(b)").expect("re-own");
    resolve_matches_scratch(&mut s, "re-owned region");
}

/// xorshift64*, so a failing seed replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// In `1..=n`, as Lua indexes.
    fn pick(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize + 1
    }

    fn coin(&mut self, in_n: u64) -> bool {
        self.next().is_multiple_of(in_n)
    }

    fn off(&mut self) -> i64 {
        (self.next() % 41) as i64 - 20
    }
}

const POINTS: [&str; 9] = [
    "TOPLEFT",
    "TOP",
    "TOPRIGHT",
    "LEFT",
    "CENTER",
    "RIGHT",
    "BOTTOMLEFT",
    "BOTTOM",
    "BOTTOMRIGHT",
];

/// The Lua state the steps act on: `N` every node in creation order (an anchor only targets an
/// earlier one, so the graph stays acyclic), `F` the frames, `R` the regions, `B` the buttons,
/// `S` the ScrollFrames, `owned[child]` the ScrollFrame that holds it.
const SETUP: &str = r#"
    N, F, R, B, S, at, owned = {}, {}, {}, {}, {}, {}, {}
    function add(o) table.insert(N, o); at[o] = table.getn(N) end
    function earlier(o, k) local i = at[o] or (table.getn(N) + 1); return N[math.mod(k, i - 1) + 1] end
    root = CreateFrame("Frame", "Root"); root:SetWidth(400); root:SetHeight(300)
    root:SetPoint("CENTER", nil, "CENTER", 0, 0)
    add(root); table.insert(F, root)
    spec = { bgFile = "bg", edgeFile = "edge", edgeSize = 8, insets = { left = 2, right = 2, top = 2, bottom = 2 } }
"#;

/// One random structural or value write, as a Lua chunk, run under `pcall` (a step may name a
/// region another step freed, which raises, as it would in the client).
fn step(rng: &mut Rng) -> String {
    let p = POINTS[rng.pick(9) - 1];
    let rp = POINTS[rng.pick(9) - 1];
    let (x, y) = (rng.off(), rng.off());
    let k = rng.next() % 1000;
    let k2 = rng.next() % 1000;
    let w = if rng.coin(4) { 0 } else { rng.pick(60) };
    let h = if rng.coin(4) { 0 } else { rng.pick(60) };
    match rng.next() % 16 {
        // A frame born under any frame, anchored to an earlier node.
        0 | 1 => {
            let kind = if rng.coin(3) { "Button" } else { "Frame" };
            format!(
                "local par = F[math.mod({k}, table.getn(F)) + 1]
                 local f = CreateFrame('{kind}', nil, par); add(f); table.insert(F, f)
                 if '{kind}' == 'Button' then table.insert(B, f) end
                 f:SetWidth({w}); f:SetHeight({h})
                 f:SetPoint('{p}', earlier(f, {k2}), '{rp}', {x}, {y})"
            )
        }
        // A texture or font string born on any frame, anchored to one or two earlier nodes.
        2 | 3 => {
            let (make, art) = if rng.coin(3) {
                ("CreateFontString", "")
            } else if rng.coin(2) {
                ("CreateTexture", "t:SetTexture(1, 0, 0)")
            } else {
                ("CreateTexture", "")
            };
            let second = if rng.coin(2) {
                format!("t:SetPoint('{rp}', earlier(t, {}), '{p}', {y}, {x})", k2 + 7)
            } else {
                String::new()
            };
            format!(
                "local t = F[math.mod({k}, table.getn(F)) + 1]:{make}(nil, 'ARTWORK')
                 add(t); table.insert(R, t); {art}
                 if {w} > 0 then t:SetWidth({w}) end; if {h} > 0 then t:SetHeight({h}) end
                 t:SetPoint('{p}', earlier(t, {k2}), '{rp}', {x}, {y}); {second}"
            )
        }
        // A backdrop built or torn down: nine pieces born or freed at once.
        4 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             if f:GetBackdrop() then f:SetBackdrop(nil) else f:SetBackdrop(spec) end"
        ),
        // A node anchored to a backdrop piece, so a teardown frees what something reads.
        5 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             local piece = ({{ f:GetRegions() }})[1]
             if piece and at[f] then
                 local t = f:CreateTexture(nil, 'OVERLAY'); add(t); table.insert(R, t)
                 t:SetWidth(5); t:SetHeight(5); t:SetPoint('{p}', piece, '{rp}', {x}, {y})
             end"
        ),
        // A button's slot region set, or freed by a nil.
        6 => format!(
            "if table.getn(B) > 0 then
                 local b = B[math.mod({k}, table.getn(B)) + 1]
                 if b:GetNormalTexture() and {k2} < 500 then b:SetNormalTexture(nil)
                 else b:SetNormalTexture('slot' .. {k2}) end
             end"
        ),
        // A frame re-parented, its subtree's scale with it.
        7 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             local par = F[math.mod({k2}, table.getn(F)) + 1]
             if f ~= root and par ~= f then pcall(f.SetParent, f, par) end"
        ),
        // A frame's own scale.
        8 => format!(
            "F[math.mod({k}, table.getn(F)) + 1]:SetScale({})",
            [0.5, 0.75, 1.0, 1.25, 2.0][(k2 % 5) as usize]
        ),
        // A region re-owned.
        9 => format!(
            "if table.getn(R) > 0 then
                 R[math.mod({k}, table.getn(R)) + 1]:SetParent(F[math.mod({k2}, table.getn(F)) + 1])
             end"
        ),
        // An anchor re-pointed, cleared or set to all points, on a frame or a region.
        10 | 11 => {
            let verb = match k % 4 {
                0 => "o:ClearAllPoints()".to_string(),
                1 => format!("o:SetAllPoints(earlier(o, {k2}))"),
                _ => format!("o:SetPoint('{p}', earlier(o, {k2}), '{rp}', {x}, {y})"),
            };
            format!(
                "local o = N[math.mod({k2}, table.getn(N)) + 1]
                 if o ~= root then {verb} end"
            )
        }
        // A size.
        12 => format!(
            "local o = N[math.mod({k}, table.getn(N)) + 1]
             if o ~= root then o:SetWidth({w}); o:SetHeight({h}) end"
        ),
        // A ScrollFrame born, or one taking a newer frame as its child, or scrolling.
        13 => format!(
            "if {k} < 300 or table.getn(S) == 0 then
                 local par = F[math.mod({k2}, table.getn(F)) + 1]
                 local sf = CreateFrame('ScrollFrame', nil, par); add(sf); table.insert(F, sf)
                 table.insert(S, sf); sf:SetWidth(50); sf:SetHeight(50)
                 sf:SetPoint('{p}', earlier(sf, {k}), '{rp}', {x}, {y})
             else
                 local sf = S[math.mod({k2}, table.getn(S)) + 1]
                 if {k} < 650 then
                     for i = table.getn(F), 1, -1 do
                         local c = F[i]
                         if at[c] > at[sf] and not owned[c] then
                             if sf:GetScrollChild() then owned[sf:GetScrollChild()] = nil end
                             sf:SetScrollChild(c); owned[c] = sf
                             break
                         end
                     end
                 elseif {k} < 800 then sf:SetVerticalScroll({x})
                 else sf:SetHorizontalScroll({y}) end
             end"
        ),
        // Clamped to the screen.
        14 => format!(
            "F[math.mod({k}, table.getn(F)) + 1]:SetClampedToScreen({})",
            k2.is_multiple_of(2)
        ),
        // Art on a region whose size may take it.
        _ => format!(
            "if table.getn(R) > 0 then
                 local t = R[math.mod({k}, table.getn(R)) + 1]
                 if t.SetTexture then if {k2} < 500 then t:SetTexture(0, 1, 0) else t:SetTexture(nil) end end
             end"
        ),
    }
}

/// A random run of births, frees, re-parents, re-owns, retargets and scrolls: after every step the
/// incremental resolve equals a from-scratch one, and none of them derived the graph.
#[test]
fn random_structural_writes_resolve_as_a_scratch_solve_would() {
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0xdead_beef_cafe_f00d,
        0x0123_4567_89ab_cdef,
    ] {
        let mut s = script();
        s.set_screen_size(800.0, 600.0);
        s.run(SETUP).expect("setup");
        s.run("raised = 0").expect("counter");
        s.resolve();
        let derived_at = s.layout_derivations();
        let mut rng = Rng(seed);
        for n in 0..400 {
            let chunk = step(&mut rng);
            s.run(&format!(
                "if not pcall(function() {chunk} end) then raised = raised + 1 end"
            ))
            .unwrap_or_else(|e| panic!("seed {seed:#x} step {n}: {e}\n{chunk}"));
            resolve_matches_scratch(&mut s, &format!("seed {seed:#x} step {n}:\n{chunk}"));
        }
        let (nodes, raised, live) = s
            .eval::<(i64, i64, i64)>(
                "local live = 0
                 for _, o in N do if pcall(o.GetObjectType, o) then live = live + 1 end end
                 return table.getn(N), raised, live",
            )
            .expect("count");
        eprintln!("seed {seed:#x}: {nodes} nodes built, {live} alive, {raised} steps raised");
        assert!(
            nodes > 100,
            "seed {seed:#x}: the run built only {nodes} nodes"
        );
        assert!(raised < 40, "seed {seed:#x}: {raised} of 400 steps raised");
        assert_eq!(
            s.layout_derivations(),
            derived_at,
            "seed {seed:#x}: a structural write derived the graph instead of naming its nodes"
        );
    }
}

/// A frame two ScrollFrames claim takes the derivation, whose override the cached graph cannot
/// name; the result still equals a scratch solve, and a lone claim stays incremental.
#[test]
fn a_child_two_scrollframes_claim_falls_back_to_the_derivation() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        a = CreateFrame("ScrollFrame", "SA"); a:SetWidth(50); a:SetHeight(50)
        a:SetPoint("TOPLEFT", nil, "TOPLEFT", 10, -10)
        b = CreateFrame("ScrollFrame", "SB"); b:SetWidth(50); b:SetHeight(50)
        b:SetPoint("TOPLEFT", nil, "TOPLEFT", 200, -10)
        c = CreateFrame("Frame", "C"); c:SetWidth(20); c:SetHeight(20)
        c:SetPoint("CENTER", nil, "CENTER", 0, 0)
        "#,
    )
    .expect("setup");
    s.resolve();
    let derived = s.layout_derivations();
    s.run("a:SetScrollChild(c); a:SetVerticalScroll(5)")
        .expect("one claim");
    resolve_matches_scratch(&mut s, "one claim");
    assert_eq!(s.layout_derivations(), derived, "one claim is named");
    s.run("b:SetScrollChild(c)").expect("two claims");
    resolve_matches_scratch(&mut s, "two claims");
    assert_eq!(
        s.layout_derivations(),
        derived + 1,
        "two claims give up for the derivation"
    );
}
