//! Structural layout writes (a node born, freed, re-owned or re-pointed) through the cached graph:
//! every resolve must equal a from-scratch solve of the same model, rects and graph alike.

use super::common::script;
use crate::script::{MeasureRequest, TextMeasure, UiScript};

/// A stand-in font engine, so a text-sized FontString takes a box: 6 wide per character, 12 tall.
struct Block;

impl TextMeasure for Block {
    fn measure(&mut self, req: &MeasureRequest) -> (f32, f32, f32) {
        let w = req.text.chars().count() as f32 * 6.0;
        (w, 12.0, w)
    }
}

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

/// The Lua state the steps act on: `N` every node in creation order, `F` the frames, `R` the
/// regions, `B` the buttons, `S` the ScrollFrames, `owned[child]` the ScrollFrame that holds it.
/// `deps[o][point]` mirrors each anchor's target and `held[c]` each scroll claim, so `target`
/// picks any node, older or newer, whose rect does not already read `o`'s: the graph stays acyclic.
const SETUP: &str = r#"
    N, F, R, B, S, at, owned, deps, held, pieced = {}, {}, {}, {}, {}, {}, {}, {}, {}, {}
    function add(o) table.insert(N, o); at[o] = table.getn(N); deps[o] = {} end
    -- Whether `a`'s rect reads `b`'s, through anchors and scroll overrides.
    function reaches(a, b)
        local seen, stack = {}, { a }
        while table.getn(stack) > 0 do
            local x = table.remove(stack)
            if x == b then return true end
            if not seen[x] then
                seen[x] = true
                for _, t in (deps[x] or {}) do table.insert(stack, t) end
                if held[x] then table.insert(stack, held[x]) end
            end
        end
        return false
    end
    -- A node `o` may anchor to: one of a few picks over the whole roster, else the root.
    function target(o, k)
        local n = table.getn(N)
        for i = 0, 7 do
            local t = N[math.mod(k * 7 + i * 13, n) + 1]
            if t ~= o and not reaches(t, o) then return t end
        end
        return root
    end
    newer = 0
    function point(o, p, t, rp, x, y)
        o:SetPoint(p, t, rp, x, y); deps[o][p] = t
        if at[t] and at[t] > at[o] then newer = newer + 1 end
    end
    function clear(o) o:ClearAllPoints(); deps[o] = {} end
    function all(o, t) o:SetAllPoints(t); deps[o] = { TOPLEFT = t, BOTTOMRIGHT = t } end
    root = CreateFrame("Frame", "UIParent"); root:SetWidth(400); root:SetHeight(300)
    root:SetPoint("CENTER", nil, "CENTER", 0, 0)
    add(root); table.insert(F, root)
    spec = { bgFile = "bg", edgeFile = "edge", edgeSize = 8, insets = { left = 2, right = 2, top = 2, bottom = 2 } }
"#;

/// One write in a batch: a Lua chunk, run under `pcall` (a step may name a region another step
/// freed, which raises, as it would in the client), or the host's UI scale.
enum Write {
    Lua(String),
    UiScale(f32),
}

/// One random structural or value write.
fn step(rng: &mut Rng) -> Write {
    let p = POINTS[rng.pick(9) - 1];
    let rp = POINTS[rng.pick(9) - 1];
    let (x, y) = (rng.off(), rng.off());
    let k = rng.next() % 1000;
    let k2 = rng.next() % 1000;
    let w = if rng.coin(4) { 0 } else { rng.pick(60) };
    let h = if rng.coin(4) { 0 } else { rng.pick(60) };
    let lua = match rng.next() % 21 {
        // A frame born under any frame, anchored to any node it may read.
        0 | 1 => {
            let kind = if rng.coin(3) { "Button" } else { "Frame" };
            format!(
                "local par = F[math.mod({k}, table.getn(F)) + 1]
                 local f = CreateFrame('{kind}', nil, par); add(f); table.insert(F, f)
                 if '{kind}' == 'Button' then table.insert(B, f) end
                 f:SetWidth({w}); f:SetHeight({h})
                 point(f, '{p}', target(f, {k2}), '{rp}', {x}, {y})"
            )
        }
        // A texture or font string born on any frame, anchored once or twice.
        2 | 3 => {
            let (make, art) = if rng.coin(3) {
                ("CreateFontString", "")
            } else if rng.coin(2) {
                ("CreateTexture", "t:SetTexture(1, 0, 0)")
            } else {
                ("CreateTexture", "")
            };
            let second = if rng.coin(2) {
                format!("point(t, '{rp}', target(t, {}), '{p}', {y}, {x})", k2 + 7)
            } else {
                String::new()
            };
            format!(
                "local t = F[math.mod({k}, table.getn(F)) + 1]:{make}(nil, 'ARTWORK')
                 add(t); table.insert(R, t); {art}
                 if {w} > 0 then t:SetWidth({w}) end; if {h} > 0 then t:SetHeight({h}) end
                 point(t, '{p}', target(t, {k2}), '{rp}', {x}, {y}); {second}"
            )
        }
        // A backdrop built or torn down: nine pieces born or freed at once.
        4 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             if f:GetBackdrop() then f:SetBackdrop(nil) else f:SetBackdrop(spec) end"
        ),
        // A node, a texture or a frame, anchored to a fresh backdrop piece (the last region the
        // build links) and, at another point, to any node, so a later teardown frees one of its
        // anchors and it stays a node.
        5 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             f:SetBackdrop(spec)
             local rs = {{ f:GetRegions() }}
             local piece = rs[table.getn(rs)]
             deps[piece] = {{ f }}
             local o
             if {k2} < 600 then
                 o = f:CreateTexture(nil, 'OVERLAY'); add(o); table.insert(R, o)
             else
                 o = CreateFrame('Frame', nil, f); add(o); table.insert(F, o)
             end
             o:SetWidth(5); o:SetHeight(5)
             point(o, '{p}', piece, '{rp}', {x}, {y})
             if '{p}' ~= '{rp}' then point(o, '{rp}', target(o, {k2}), '{p}', {y}, {x}) end
             table.insert(pieced, {{ o, o:GetNumPoints() }})"
        ),
        // A button's slot region set, or freed by a nil; or two freed in one pass.
        6 => format!(
            "local n = table.getn(B)
             if n > 0 then
                 local b = B[math.mod({k}, n) + 1]
                 if b:GetNormalTexture() and {k2} < 400 then b:SetNormalTexture(nil)
                 elseif {k2} < 700 then
                     b:SetNormalTexture(nil); B[math.mod({k2}, n) + 1]:SetNormalTexture(nil)
                 else b:SetNormalTexture('slot' .. {k2}) end
             end"
        ),
        // A frame re-parented, its subtree's scale with it.
        7 => format!(
            "local f = F[math.mod({k}, table.getn(F)) + 1]
             local par = F[math.mod({k2}, table.getn(F)) + 1]
             if f ~= root and par ~= f then pcall(f.SetParent, f, par) end"
        ),
        // A frame's own scale, the root's included: a whole-UI subtree.
        8 => format!(
            "F[math.mod({k}, table.getn(F)) + 1]:SetScale({})",
            [0.5, 0.75, 1.0, 1.25, 2.0][(k2 % 5) as usize]
        ),
        // A region re-owned, and re-anchored in the same pass.
        9 => format!(
            "if table.getn(R) > 0 then
                 local t = R[math.mod({k}, table.getn(R)) + 1]
                 t:SetParent(F[math.mod({k2}, table.getn(F)) + 1])
                 if {k} < 500 then point(t, '{p}', target(t, {k2}), '{rp}', {x}, {y}) end
             end"
        ),
        // An anchor re-pointed, cleared or set to all points, on a frame or a region.
        10 | 11 => {
            let verb = match k % 4 {
                0 => "clear(o)".to_string(),
                1 => format!("all(o, target(o, {k2}))"),
                _ => format!("point(o, '{p}', target(o, {k2}), '{rp}', {x}, {y})"),
            };
            format!(
                "local o = N[math.mod({k2}, table.getn(N)) + 1]
                 if o ~= root then {verb} end"
            )
        }
        // A node leaving the graph and rejoining it in one pass.
        12 => format!(
            "local o = N[math.mod({k}, table.getn(N)) + 1]
             if o ~= root then clear(o); point(o, '{p}', target(o, {k2}), '{rp}', {x}, {y}) end"
        ),
        // A size.
        13 => format!(
            "local o = N[math.mod({k}, table.getn(N)) + 1]
             if o ~= root then o:SetWidth({w}); o:SetHeight({h}) end"
        ),
        // A ScrollFrame born, or one taking a frame it does not read as its child, or scrolling.
        14 | 15 => format!(
            "if {k} < 300 or table.getn(S) == 0 then
                 local par = F[math.mod({k2}, table.getn(F)) + 1]
                 local sf = CreateFrame('ScrollFrame', nil, par); add(sf); table.insert(F, sf)
                 table.insert(S, sf); sf:SetWidth(50); sf:SetHeight(50)
                 point(sf, '{p}', target(sf, {k}), '{rp}', {x}, {y})
             else
                 local sf = S[math.mod({k2}, table.getn(S)) + 1]
                 if {k} < 650 then
                     local n = table.getn(F)
                     for i = 0, n - 1 do
                         local c = F[math.mod({k} + i, n) + 1]
                         if c ~= root and c ~= sf and not owned[c] and not reaches(sf, c) then
                             local old = sf:GetScrollChild()
                             if old then owned[old] = nil; held[old] = nil end
                             sf:SetScrollChild(c); owned[c] = sf; held[c] = sf
                             deps[c] = {{ TOPLEFT = sf }}
                             break
                         end
                     end
                 elseif {k} < 800 then sf:SetVerticalScroll({x})
                 else sf:SetHorizontalScroll({y}) end
             end"
        ),
        // Clamped to the screen.
        16 => format!(
            "F[math.mod({k}, table.getn(F)) + 1]:SetClampedToScreen({})",
            k2.is_multiple_of(2)
        ),
        // Art on a region whose size may take it.
        17 => format!(
            "if table.getn(R) > 0 then
                 local t = R[math.mod({k}, table.getn(R)) + 1]
                 if t.SetTexture then if {k2} < 500 then t:SetTexture(0, 1, 0) else t:SetTexture(nil) end end
             end"
        ),
        // The host's UI scale, `UIParent`'s whole subtree.
        18 => return Write::UiScale([0.64, 0.8, 1.0][(k % 3) as usize]),
        // A font string's text set, emptied or restored: its measured box comes and goes.
        20 => format!(
            "local n = table.getn(R)
             if n > 0 then
                 local t = R[math.mod({k}, n) + 1]
                 if t.SetText then t:SetText(({{ '', 'hello', 'a longer line', nil }})[{}]) end
             end",
            k2 % 4 + 1
        ),
        // Two backdrops torn down or built in one pass.
        _ => format!(
            "local n = table.getn(F)
             for _, f in {{ F[math.mod({k}, n) + 1], F[math.mod({k2}, n) + 1] }} do
                 if f:GetBackdrop() then f:SetBackdrop(nil) else f:SetBackdrop(spec) end
             end"
        ),
    };
    Write::Lua(lua)
}

/// A random run of births, frees, re-parents, re-owns, retargets, scrolls, scale changes and text
/// set, emptied and restored, in batches of one to four writes per resolve: after every resolve
/// the cached graph and its rects equal a from-scratch solve, and none of them derived the graph.
#[test]
fn random_structural_writes_resolve_as_a_scratch_solve_would() {
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0xdead_beef_cafe_f00d,
        0x0123_4567_89ab_cdef,
        0x5555_aaaa_1234_9876,
    ] {
        let mut s = script();
        s.set_screen_size(800.0, 600.0);
        s.set_text_measurer(Box::new(Block));
        s.run(SETUP).expect("setup");
        s.run("raised = 0").expect("counter");
        s.resolve();
        let derived_at = s.layout_derivations();
        let mut rng = Rng(seed);
        let mut writes = 0;
        for n in 0..300 {
            let batch = rng.pick(4);
            let mut log = String::new();
            for _ in 0..batch {
                writes += 1;
                match step(&mut rng) {
                    Write::Lua(chunk) => {
                        s.run(&format!(
                            "if not pcall(function() {chunk} end) then raised = raised + 1 end"
                        ))
                        .unwrap_or_else(|e| panic!("seed {seed:#x} batch {n}: {e}\n{chunk}"));
                        log.push_str(&chunk);
                    }
                    Write::UiScale(v) => {
                        s.set_ui_scale(v);
                        log.push_str(&format!("set_ui_scale({v})"));
                    }
                }
                log.push('\n');
            }
            resolve_matches_scratch(&mut s, &format!("seed {seed:#x} batch {n}:\n{log}"));
        }
        let (nodes, raised, frames, regions, newer) = s
            .eval::<(i64, i64, i64, i64, i64)>(
                "return table.getn(N), raised, table.getn(F), table.getn(R), newer",
            )
            .expect("count");
        // Dependents of a backdrop piece that a later teardown or rebuild freed: each lost the
        // anchor to it, and the scratch solve above agreed with every pass in between.
        let unhooked = s
            .eval::<i64>(
                "local n = 0
                 for _, e in pieced do
                     local ok, now = pcall(e[1].GetNumPoints, e[1])
                     if ok and now < e[2] then n = n + 1 end
                 end
                 return n",
            )
            .expect("unhooked");
        assert!(
            unhooked > 10,
            "seed {seed:#x}: only {unhooked} dependents of a backdrop piece saw it freed"
        );
        eprintln!(
            "seed {seed:#x}: {writes} writes, {nodes} nodes ({frames} frames, {regions} \
             regions), {raised} raised, {newer} anchors to a newer node"
        );
        assert!(
            nodes > 100,
            "seed {seed:#x}: the run built only {nodes} nodes"
        );
        assert!(
            newer > 20,
            "seed {seed:#x}: only {newer} anchors pointed at a newer node"
        );
        assert!(
            raised * 10 < writes,
            "seed {seed:#x}: {raised} of {writes} writes raised"
        );
        assert_eq!(
            s.layout_derivations(),
            derived_at,
            "seed {seed:#x}: a structural write derived the graph instead of naming its nodes"
        );
    }
}

/// A frame two ScrollFrames claim holds the last one's re-anchor (`0x787100`), one real point,
/// through the cached graph: no derivation.
#[test]
fn a_child_two_scrollframes_claim_holds_the_last_re_anchor() {
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
    s.run("b:SetScrollChild(c)").expect("two claims");
    resolve_matches_scratch(&mut s, "two claims");
    assert_eq!(s.layout_derivations(), derived, "every claim is named");
    assert!(s
        .eval::<bool>("local p, r = c:GetPoint(1); return c:GetNumPoints() == 1 and r == b")
        .unwrap());
    crate::assert_near!(s.eval::<f64>("return c:GetLeft()").unwrap(), 200.0);
}

/// A frame whose id is minted after a region on it joined the graph (the engine builds some
/// frames before any id, as the nameplate pool does): the mint re-derives the region, which then
/// reads the owner's scale.
#[test]
fn an_owner_minted_after_its_region_joined_takes_the_owner_edge() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"root = CreateFrame("Frame", "Root"); root:SetWidth(100); root:SetHeight(100)
        root:SetPoint("CENTER", nil, "CENTER", 0, 0)"#,
    )
    .expect("setup");
    s.resolve();
    let (host, rh) = {
        let mut m = s.model_mut();
        let root = m.arena.lookup("Root").expect("root");
        let root_id = m.frame_id(root);
        let host = m
            .arena
            .create(crate::widget::FrameKind::Frame, None, Some(root));
        let rh = m
            .arena
            .create_region(
                host,
                crate::widget::RegionKind::Texture,
                crate::order::DrawLayer::Artwork,
                0,
            )
            .expect("region");
        let d = m.region_data.entry(rh).or_default();
        d.size = Some((
            crate::layout::Len::Script(8.0),
            crate::layout::Len::Script(8.0),
        ));
        d.anchors = crate::layout::Anchors::from([crate::layout::Anchor::new(
            crate::layout::Point::TopLeft,
            root_id,
            crate::layout::Point::TopLeft,
            5.0,
            -5.0,
        )]);
        m.touch_layout_region(rh);
        (host, rh)
    };
    resolve_matches_scratch(&mut s, "a region on an id-less owner");
    {
        let mut m = s.model_mut();
        assert!(
            !m.frame_to_id.contains_key(&host),
            "the owner has no id yet"
        );
        m.set_frame_scale(host, 2.0, false);
        m.frame_id(host);
    }
    resolve_matches_scratch(&mut s, "the owner's id minted");
    let r = s.model_ref().region_rect(rh).expect("rect");
    assert!(
        ((r.right - r.left) - 16.0).abs() < 1e-3,
        "the region takes its now-known owner's scale: {r:?}"
    );
}
