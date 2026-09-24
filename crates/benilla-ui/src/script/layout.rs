use std::collections::HashMap;

use crate::layout::{self, Anchor, LayoutInput, Point, Rect};
use crate::order::ZTarget;
use crate::widget::{FrameHandle, FrameKind, KindState, RegionHandle};

use super::backdrop;
use super::types::RegionData;
use super::{ExtractedQuad, MeasureRequest, Model, QuadContent, UiScript, SCREEN};

/// The paint a frame slot's derived quads inherit — its `effective_alpha` and `effective_scale`
/// travelling together (backdrop pieces and message-frame ring lines are emitted BY the frame,
/// so they wear the frame's own factors exactly like [`ExtractedQuad::alpha`]/`scale` on the
/// slot itself).
#[derive(Clone, Copy)]
pub(super) struct FramePaint {
    pub(super) alpha: f32,
    pub(super) scale: f32,
}

/// A 128-bit rolling fingerprint of everything [`UiScript::resolve_layout`] *reads*, so a resolve
/// whose inputs are byte-identical to the last one can be skipped outright.
///
/// ## Tier 2 of a two-tier gate (decision 0740)
///
/// This fingerprint began as the WHOLE gate, chosen over a dirty flag because a flag has sites to
/// miss and a fingerprint is computed FROM the read set — any change moves it by construction.
/// That safety cost ~0.6 ms *every idle frame* at a live pin (hashing ~2k inputs to conclude
/// "quiet"), which is why the mutation epoch ([`Model::touch_layout`], tier 1) now sits in front:
/// a frame with no layout-visible write skips at a `u64` compare and never reaches this hash. The
/// fingerprint remains load-bearing where the flag is structurally weak — a *dirty* frame whose
/// writes were idempotent (the bag-hover re-enter loop clears and rebuilds identical content
/// every frame; the GameTooltip pre-pass rewrites tooltip sizes every call) hashes clean here and
/// still skips the solve. And the flag's own failure mode — a missed touch site, a silently stale
/// layout — is machine-checked: under [`layout_verify_enabled`] (forced on in this crate's tests)
/// a tier-1-quiet frame asserts the fingerprint agrees, so the tiers police each other.
///
/// ## Why 128 bits
///
/// A collision here is not self-healing: the skip keeps the stored value, so a colliding change
/// would stay unresolved until something *else* moved. Two independent lanes put that beyond
/// physical concern (~2^-128 per frame) at the cost of one extra multiply per item.
///
/// ## Order sensitivity is safe
///
/// The accumulator is order-dependent, and it is fed by walking `HashMap`s whose iteration order
/// is arbitrary. That order is nevertheless *stable for an unmutated map*, so an unchanged model
/// reproduces the value; a mutated map may reorder and merely re-resolves. The failure mode is a
/// false DIRTY (a wasted resolve), never a false CLEAN (a stale rect).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputFingerprint(u64, u64);

impl Default for InputFingerprint {
    fn default() -> Self {
        // Distinct non-zero bases (FNV-1a's, and a 64-bit mix constant) so leading zero-valued
        // items still move both lanes.
        InputFingerprint(0xcbf2_9ce4_8422_2325, 0x9e37_79b9_7f4a_7c15)
    }
}

impl InputFingerprint {
    #[inline]
    fn feed(&mut self, v: u64) {
        self.0 = (self.0 ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        self.1 = (self.1 ^ v.rotate_left(29)).wrapping_mul(0xff51_afd7_ed55_8ccd);
    }

    #[inline]
    fn f32(&mut self, v: f32) {
        // Bit pattern, not value: `resolve` is bit-exact float math, so -0.0 and NaN payloads are
        // distinctions that matter here even where `==` would call them equal (or unequal).
        self.feed(u64::from(v.to_bits()));
    }

    #[inline]
    fn anchors(&mut self, anchors: &[Anchor]) {
        self.feed(anchors.len() as u64);
        for a in anchors {
            self.feed(u64::from(a.point.id()));
            self.feed(u64::from(a.relative_to));
            self.feed(u64::from(a.relative_point.id()));
            self.f32(a.x_off);
            self.f32(a.y_off);
        }
    }

    #[inline]
    fn input(&mut self, i: &LayoutInput) {
        self.anchors(&i.anchors);
        self.f32(i.width);
        self.f32(i.height);
        self.f32(i.scale);
        self.feed(u64::from(i.clamp));
        self.f32(i.extent_x);
        self.f32(i.extent_y);
    }

    /// Collapse to the single `u64` [`LayoutScope`] compares per node. Never [`NO_NODE`]: a
    /// present node reading as *absent* would be a false CLEAN — the one direction that ships a
    /// stale rect rather than a wasted resolve.
    #[inline]
    fn finish(self) -> u64 {
        let h = self.0 ^ self.1.rotate_left(32);
        if h == NO_NODE {
            1
        } else {
            h
        }
    }

    #[inline]
    fn rect(&mut self, r: Rect) {
        self.f32(r.bottom);
        self.f32(r.left);
        self.f32(r.top);
        self.f32(r.right);
    }
}

/// The sentinel in [`LayoutScope::last`]/`now` for "this id was not in that pass's graph". A real
/// hash colliding with it costs a spurious DIRTY (a node re-solved for nothing), never a stale
/// rect — the same one-sided failure the fingerprint above is built on.
const NO_NODE: u64 = 0;

/// One FRAME's per-node input hash — [`LayoutScope::last`]'s currency.
///
/// Factored out (decision 1388) because two paths now compute it: the full derive, over every
/// frame, and the incremental pass, over the handful a write named. They must agree *bit for bit* —
/// a hash that differs by path would read as a change on the frame the paths swap, and (worse) as
/// no-change on the frame they swap back. One function is the only way to make that true by
/// construction rather than by matching two listings by eye.
#[inline]
fn frame_node_hash(input: &LayoutInput, over: Option<Anchor>) -> u64 {
    let mut node = InputFingerprint::default();
    node.input(input);
    match over {
        Some(a) => node.anchors(std::slice::from_ref(&a)),
        None => node.feed(u64::MAX),
    }
    node.finish()
}

/// One REGION's per-node input hash — [`frame_node_hash`]'s twin, over the three things the sweep
/// reads out of `RegionData`: its anchors, its explicit size, and its measured text extent.
#[inline]
fn region_node_hash(data: &RegionData) -> u64 {
    let mut node = InputFingerprint::default();
    node.anchors(&data.anchors);
    match data.size {
        Some((w, h)) => {
            node.f32(w);
            node.f32(h);
        }
        None => node.feed(u64::MAX),
    }
    match data.measured {
        Some(m) => {
            node.f32(m.w);
            node.f32(m.h);
        }
        None => node.feed(u64::MAX),
    }
    // **The ART, on a region whose rect is derived from it** (decision 1349): an axis authored `0`
    // takes its span from the content ([`content_span`]), so swapping the texture on such a region
    // MOVES it and the node must re-hash. Fed only on that shape — a sized region's art cannot
    // change its rect, and hashing every icon path would put a string walk in the derive's hot loop
    // for a fact with no reader.
    if !data.anchors.is_empty() && data.size.is_none_or(|(w, h)| w == 0.0 || h == 0.0) {
        node.feed(data.fill.is_some() as u64);
        match &data.texture {
            Some(path) => {
                node.feed(path.len() as u64);
                for b in path.as_bytes() {
                    node.feed(u64::from(*b));
                }
            }
            None => node.feed(u64::MAX),
        }
    }
    node.finish()
}

/// A region's **content-derived span**, in FrameXML units — the client's virtual size getters,
/// which the resolver calls instead of reading a stored width/height (decision 1349).
///
/// `CSimpleTexture::GetWidth 0x770720` returns the authored value only when it is **not exactly
/// `0.0`**; on `0.0` with a texture loaded it returns the texture's own texel extent, through
/// bit-for-bit the same converter the `<AbsDimension>` attribute uses — so **one texel is one
/// FrameXML unit, at every aspect ratio**. `0x770790` is the identical body for height. The
/// colour form generates an **8×8** surface (`0x770360` → `0x44a900` stamps
/// `[tex+0x144] = [tex+0x148] = 8`), so a `<Color>`-only texture spans 8 units.
///
/// `None` = nothing to measure (no art, or no host oracle to measure it with), and the caller
/// leaves the axis at zero — which is the client's answer too when a texture has no `CGxTex*`
/// at all.
///
/// This is the TEXTURE half. A `CSimpleFontString`'s span is derived the same way and lives in
/// the sweep itself, because it needs the measure the sweep already folds in
/// ([`FONTSTRING_MIN_SPAN`]). Between them they are what let the owner-edge fallback go
/// (decision 1664): a region's span comes from its content, never from the frame it hangs on.
pub(super) fn content_span(
    data: &RegionData,
    probe: Option<&crate::script::TextureSizeProbe>,
) -> Option<(f32, f32)> {
    if data.fill.is_some() {
        return Some((SOLID_TEXTURE_TEXELS, SOLID_TEXTURE_TEXELS));
    }
    texel_span(probe, data.texture.as_deref()?)
}

/// [`content_span`]'s texture arm, for a caller holding a path rather than a region — the
/// SimpleHTML flow reservation, which asks the reference's `GetHeight` override the same way
/// (`0x770790`).
pub(super) fn texel_span(
    probe: Option<&crate::script::TextureSizeProbe>,
    path: &str,
) -> Option<(f32, f32)> {
    let (w, h) = probe?(path)?;
    Some((w as f32, h as f32))
}

/// The edge of the surface `CSimpleTexture::SetTexture(const CImVector*)` generates for the colour
/// form — 64 texels, `0x44a900` stamping `[tex+0x144] = [tex+0x148] = 8` (decision 1349 §1).
const SOLID_TEXTURE_TEXELS: f32 = 8.0;

/// The floor under a **FontString's** derived span, in FrameXML units —
/// `CSimpleFontString::GetWidth 0x772930` / `GetHeight 0x772a60` end
/// `fld 1-unit; fcomp candidate; test ah,0x5; jp` and return the floor when the candidate is
/// smaller. One unit,
/// not one line: the reference's own blank tooltip spacer is `" \n"` — a real one-line string —
/// and this floor is what a *genuinely* empty string reads back as, never `0.0`.
pub(super) const FONTSTRING_MIN_SPAN: f32 = 1.0;

/// [`LayoutScope::node_of`] holds one roster index per id, and the two rosters are separate
/// (`plan` for frames, `regions` for regions) — this bit says which one. Node counts run in the
/// thousands, so the top bit is free forever.
const REGION_TAG: u32 = 0x8000_0000;

/// Per-node layout scope — the state that lets a resolve touch only what MOVED (decision 1350,
/// the phase 2 decision 0771 left open).
///
/// 0771 halved the per-content-change cost by folding two whole-UI solves into one; it also said
/// plainly that halving it was not fixing it, because *one* solve is still a fixpoint over every
/// frame and every region for a change that touches ten FontStrings inside one frame. Two later
/// correctness fixes then tripled what a region costs — `0dd6a559a` (a region whose owner has no
/// rect resolves from its own anchors) and 1310 (every anchor-less Texture/FontString gets
/// implicit anchors at creation) each moved a large population out of the sweep's cheap
/// `continue` and into full resolution. Neither is wrong; both are why the whole-UI shape had to
/// go.
///
/// Everything here is indexed **densely by layout id** — frames and regions are minted from one
/// monotonic counter (`Model::next_id`), so a single array covers both, and the solver's own rect
/// array is indexed the same way.
///
/// Nothing in here allocates on a steady frame: every buffer is resized once and refilled.
#[derive(Default)]
pub(crate) struct LayoutScope {
    /// Per id: the input hash the last CONVERGED resolve saw, [`NO_NODE`] if the id was not in
    /// that graph. A mismatch is a dirty SEED — and a birth is a mismatch for free.
    last: Vec<u64>,
    /// How many ids `last` holds a real hash for. Compared against how many of THIS pass's nodes
    /// were also in the last graph: a shortfall means a node DIED, whose dependents we can no
    /// longer enumerate (its edges died with it), so that pass falls back to full scope.
    last_count: usize,
    /// The screen rect the `last` set was hashed under. It is every clamped frame's extent and the
    /// root of every chain, so a move is a full-scope event.
    last_screen: Option<Rect>,
    /// This pass's hash per id (parallel to `last`); swapped into `last` on convergence.
    now: Vec<u64>,
    /// Per id: index into this pass's node roster, or [`u32::MAX`].
    node_of: Vec<u32>,
    /// Per id: head index into `dep_to`/`dep_next` of the nodes that DEPEND on it, or
    /// [`u32::MAX`]. An intrusive list rather than a CSR so the edges are built in one walk.
    dep_head: Vec<u32>,
    dep_to: Vec<u32>,
    dep_next: Vec<u32>,
    /// Edge slots freed by [`Self::retarget`], for [`Self::edge`] to reuse. Without it a session
    /// of hovers would grow `dep_to` by one entry per retarget forever — bounded only by the next
    /// full derivation, which is exactly the event retargeting exists to avoid.
    dep_free: Vec<u32>,
    /// Per id: in the dirty closure.
    dirty: Vec<bool>,
    /// The closure's worklist.
    stack: Vec<u32>,
    /// The frame roster this cached graph describes: `(handle, id, ScrollFrame anchor override)`,
    /// indexed by [`Self::node_of`] (decision 1388).
    ///
    /// It holds no borrow of `layout_inputs`, and that is the whole point. The borrowed form 1350
    /// introduced had to be rebuilt every call, which forced the *derivation* — the ids walk, the
    /// scroll map, the liveness retain, and the two hash sweeps — to be rebuilt with it: ~1.5 ms of
    /// whole-roster work per mutated frame, whatever moved. Owning the row instead lets the roster
    /// outlive the call, and the `layout_inputs` probe it replaces moves to the round loop, where it
    /// is paid once per frame *this solve actually touches* instead of once per frame in the UI.
    plan: Vec<(FrameHandle, u32, Option<Anchor>)>,
    /// The anchored-region roster — [`Self::plan`]'s twin, and the bigger of the two by a factor of
    /// three (10,438 anchored regions against 3,218 frames at the Stormwind pin).
    regions: Vec<RegionRow>,
    /// An incremental pass's staged [`Self::last`] writes, `(id, hash)`. Applied only on
    /// convergence, for the reason [`Self::commit`] is: a pass left mid-flight describes a graph
    /// that never settled, and adopting its hashes would seed the next pass's
    /// "unchanged ⇒ unmoved" argument from a lie.
    staged: Vec<(u32, u64)>,
}

/// One anchored, live region as the round loop consumes it — resolved ONCE per *derivation*
/// instead of re-derived per call (decision 1350) or per round (before it). Before this list the
/// sweep walked `region_data` itself every round and paid, per region per round, an `arena.region`
/// probe for liveness/owner/kind and a `region_to_id` probe to publish the result — and it paid
/// them for the anchor-less entries too, which it then `continue`d past.
///
/// It is also the addressable form the scope needs: a dirty set is a list of INDICES into this,
/// and "sweep only what moved" is `for &i in sweep { … regions[i] … }`.
///
/// 1388 took the `&RegionData` borrow back out of it, which is what lets the roster outlive the
/// call that built it. The sweep re-probes `region_data` for that half — one lookup per region
/// *swept* rather than one per region in the UI.
#[derive(Clone, Copy)]
struct RegionRow {
    rh: RegionHandle,
    /// The layout id — the solver's dense array index, and the scope's node key.
    id: u32,
    /// The owning frame: supplies the fallback edges for the axes this region's own anchors do
    /// not pin, and is therefore a layout DEPENDENCY of it.
    owner: FrameHandle,
    is_fontstring: bool,
}

impl LayoutScope {
    /// Grow every per-id array to cover `n` ids and reset the per-pass ones, ahead of deriving the
    /// graph from scratch. `last` keeps its contents (it is the memory of the previous converged
    /// pass); everything else — the roster, the edges, `node_of` — is about to be rebuilt.
    fn begin_full(&mut self, n: usize) {
        self.last.resize(n, NO_NODE);
        self.now.clear();
        self.now.resize(n, NO_NODE);
        self.node_of.clear();
        self.node_of.resize(n, u32::MAX);
        self.dep_head.clear();
        self.dep_head.resize(n, u32::MAX);
        self.dirty.clear();
        self.dirty.resize(n, false);
        self.dep_to.clear();
        self.dep_next.clear();
        self.dep_free.clear();
        self.stack.clear();
        self.plan.clear();
        self.regions.clear();
        self.staged.clear();
    }

    /// The incremental entry (decision 1388): the roster, the edges and `node_of` ARE the cached
    /// graph and must survive; only the per-pass dirty marks are scratch. This is the whole saving
    /// — `begin_full`'s six resizes and the four whole-roster walks that refill them are what a
    /// mutated frame used to pay to rediscover a graph that had not changed shape.
    fn begin_incremental(&mut self) {
        self.dirty.clear();
        self.dirty.resize(self.node_of.len(), false);
        self.stack.clear();
        self.staged.clear();
    }

    /// Is `id` a node of the cached graph? The precondition every precise touch site
    /// ([`Model::touch_layout_region`]) has to satisfy before it may name a node instead of
    /// invalidating the cache.
    pub(crate) fn has_node(&self, id: u32) -> bool {
        self.node_of
            .get(id as usize)
            .is_some_and(|&n| n != u32::MAX)
    }

    /// Record that node `from` reads `to`'s rect — so a change at `to` reaches `from`.
    #[inline]
    fn edge(&mut self, from: u32, to: u32) {
        let Some(head) = self.dep_head.get_mut(to as usize) else {
            return; // an anchor to an id this pass has no node for (the screen root, a dead
                    // target): it cannot become dirty, so it constrains nothing.
        };
        let e = match self.dep_free.pop() {
            Some(e) => {
                self.dep_to[e as usize] = from;
                self.dep_next[e as usize] = *head;
                e
            }
            None => {
                let e = self.dep_to.len() as u32;
                self.dep_to.push(from);
                self.dep_next.push(*head);
                e
            }
        };
        *head = e;
    }

    /// Re-point node `id`'s outgoing edges after an anchor RETARGET — the write 1388 had to
    /// classify as structural and answer with a whole-graph derivation.
    ///
    /// 1388's reasoning was that "an edge has to disappear and another to appear, and no per-node
    /// hash can say so, so the cached graph must be thrown away". The first half is true and the
    /// second does not follow: the write site knows both edge sets, because a node's outgoing
    /// edges ARE its anchor targets (see the derive's two `scope.edge` loops). What it could not
    /// do was *say* so — this is the vocabulary that was missing, not a cost that was unavoidable.
    ///
    /// It matters because the hottest retarget in the client is `GameTooltip:SetOwner`, which
    /// re-points the plate at the button under the cursor: every bag slot and every spellbook
    /// button crossed in a hover sweep was re-deriving the whole roster.
    ///
    /// `old`/`new` are the node's FULL anchor-target lists, before and after the write — not the
    /// one that moved. That is what makes duplicates exact: a node with two anchors onto the same
    /// target has two edges, and removing one per old entry leaves precisely the right count.
    ///
    /// Returns `false` when it cannot do this safely and the caller must fall back to the
    /// conservative touch. Removing an edge is the only direction that can be WRONG (a missing
    /// edge under-dirties, and under-dirtying ships a stale rect), so every uncertainty is
    /// resolved by refusing: an id the cached graph has no node for, or a frame whose edges come
    /// from a ScrollFrame override rather than from its own anchors (the derive does not consult
    /// the authored anchors at all in that case). An old target with no edge to remove is NOT an
    /// uncertainty — `edge` silently drops an anchor onto an id this pass has no node for, so its
    /// absence is the expected shape, and skipping it removes nothing that exists.
    pub(crate) fn retarget(&mut self, id: u32, old: &[u32], new: &[u32]) -> bool {
        let Some(&n) = self.node_of.get(id as usize) else {
            return false;
        };
        if n == u32::MAX {
            return false;
        }
        if n & REGION_TAG == 0 && self.plan[n as usize].2.is_some() {
            return false; // a ScrollFrame override owns this frame's edges, not its anchors
        }
        for &t in old {
            self.unlink_edge(t, id);
        }
        for &t in new {
            self.edge(id, t);
        }
        true
    }

    /// Unlink ONE `to → from` edge, if the list holds one. The lists are short by construction
    /// (a node's dependents), and even a hub's is walked in microseconds against the ~1.3 ms
    /// derivation this is replacing, so no bound is imposed on the walk.
    fn unlink_edge(&mut self, to: u32, from: u32) {
        let Some(&head) = self.dep_head.get(to as usize) else {
            return;
        };
        let mut prev = u32::MAX;
        let mut e = head;
        while e != u32::MAX {
            let next = self.dep_next[e as usize];
            if self.dep_to[e as usize] == from {
                if prev == u32::MAX {
                    self.dep_head[to as usize] = next;
                } else {
                    self.dep_next[prev as usize] = next;
                }
                self.dep_free.push(e);
                return;
            }
            prev = e;
            e = next;
        }
    }

    /// Mark `id` dirty and queue it, if it is not already.
    #[inline]
    fn seed(&mut self, id: u32) {
        if let Some(d) = self.dirty.get_mut(id as usize) {
            if !*d {
                *d = true;
                self.stack.push(id);
            }
        }
    }

    /// Close the dirty set under "depends on": everything that reads a dirty node's rect, and
    /// everything that reads *those*, must resolve again too.
    fn close(&mut self) {
        while let Some(id) = self.stack.pop() {
            let mut e = self.dep_head[id as usize];
            while e != u32::MAX {
                let to = self.dep_to[e as usize];
                e = self.dep_next[e as usize];
                self.seed(to);
            }
        }
    }

    /// Everything present is dirty — the fallback for the structural cases a per-node diff cannot
    /// speak about (a death, a screen move, the first pass, a graph left mid-flight by a cycle).
    fn dirty_all(&mut self) {
        for (id, &n) in self.node_of.iter().enumerate() {
            if n != u32::MAX {
                self.dirty[id] = true;
            }
        }
    }

    /// Adopt this pass's hashes as the converged memory.
    fn commit(&mut self, count: usize, screen: Rect) {
        std::mem::swap(&mut self.last, &mut self.now);
        self.last_count = count;
        self.last_screen = Some(screen);
    }

    /// [`Self::commit`] for an incremental pass: only the nodes it re-hashed moved, so only their
    /// slots in `last` are rewritten. `last_count` and `last_screen` are untouched by
    /// construction — a node entering or leaving the graph, and the screen moving, are structural
    /// events, and a structural event never reaches this path (it lands in the conservative
    /// `Model::layout_touched = None`).
    fn commit_incremental(&mut self) {
        for &(id, h) in &self.staged {
            self.last[id as usize] = h;
        }
        self.staged.clear();
    }

    /// Forget it: the next resolve must derive the graph and run in full scope. Called wherever a
    /// pass leaves the rects mid-flight (the cycle bail), because the memory would then describe a
    /// graph that never settled.
    pub(crate) fn invalidate(&mut self) {
        self.last_count = 0;
        self.last_screen = None;
        self.last.clear();
        // The roster and `node_of` go with it: `has_node` is the precise touch sites' licence to
        // name a node, and it must not outlive the memory that makes naming meaningful.
        self.plan.clear();
        self.regions.clear();
        self.node_of.clear();
        self.staged.clear();
    }
}

/// `WOW_LAYOUT_VERIFY=1` — the change gate's self-check (decision 0022's instruments-first
/// posture). When set, a resolve the gate wants to SKIP runs in full anyway and asserts it
/// reproduced the previous rects exactly; a divergence means the fingerprint's read set is
/// incomplete, and it fails loudly at the resolve that proves it rather than silently rendering a
/// stale frame days later. It also checks tier 1 against tier 2: a frame the epoch judged quiet
/// whose fingerprint nevertheless moved names a mutation path missing its `touch_layout()`.
///
/// This is what makes the gates' central claims — "the fingerprint covers everything the rounds
/// read" and "every fingerprint-visible write bumps the epoch" — machine-checked instead of
/// argued: it is FORCED ON for this crate's own tests, so every UI test (the shipped windows,
/// driven through real FrameXML + Lua) is a standing probe for a missed input or a missed touch;
/// set the env to extend that to a live run or another crate's suite. Read once; when off it
/// costs a relaxed atomic load per resolve.
/// `WOW_LAYOUT_PROF=1` — per-solve shape reporting (see the read site in [`UiScript::resolve_layout`]).
/// Read once; off, it costs one relaxed atomic load per resolve and no clock reads at all.
fn layout_prof_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_LAYOUT_PROF").as_deref() == Ok("1"))
}

fn layout_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(test) || std::env::var("WOW_LAYOUT_VERIFY").as_deref() == Ok("1"))
}

/// `WOW_LAYOUT_PROF=1`'s **preamble** split — the `[layout-pre]` line, one per LET-THROUGH resolve.
///
/// `[layout-prof]` reports the rounds, which 1350 already made proportional to what moved. This
/// reports everything *before* them: the fixed whole-roster work every let-through solve pays
/// whatever moved, which 1383 priced at ~1.9 ms/solve at a live roster and named as the next cut
/// without ever splitting it. A cut needs to know which of the eight walks it is buying back, so
/// the split is the instrument that has to exist first.
///
/// Off (the default) it takes no clock reads at all — [`Self::lap`] returns on the `on` test, so an
/// unprofiled resolve pays one bool check per phase.
#[derive(Default)]
struct PreambleProf {
    on: bool,
    last: Option<std::time::Instant>,
    /// The GameTooltip auto-size pre-pass (`layout_tooltips`) — a whole-frame-roster filter.
    tooltip: u128,
    /// `OnSizeChanged`'s "before" snapshot — a whole-`scripts`-map filter.
    watched: u128,
    /// The ids rebuild + the per-frame scale/clamp sync into `layout_inputs`.
    ids: u128,
    /// The ScrollFrame override map — another whole-frame-roster walk.
    scroll: u128,
    /// `region_resolved`'s liveness retain.
    retain: u128,
    /// The per-call solve plan rebuild.
    plan: u128,
    /// `LayoutScope::begin` — seven per-id `Vec`s cleared/resized.
    begin: u128,
    /// The fingerprint + scope walk over every live FRAME (double-hashed: aggregate + per-node).
    fp_frames: u128,
    /// The same over every anchored REGION — the biggest roster (10,438 anchored at the SW pin,
    /// and by far the biggest single phase: 0.45–1.0 ms of the ~1.0–1.4 ms walk).
    fp_regions: u128,
    /// The INCREMENTAL pass's whole preamble (decision 1388): re-hash the nodes the ledger named,
    /// seed the ones that moved, close. Every phase above is zero on such a pass, and this is what
    /// replaced them — read the two side by side to see what a derivation costs.
    seed: u128,
    /// Was this an incremental pass? Reported as its own flag rather than inferred from
    /// `seed != 0`, which is what it was first written as — and which read `incr=0` on every
    /// incremental frame of the shipped UI, because the work it times rounds to under a
    /// microsecond. An instrument that says "this never happens" about the path it was built to
    /// measure is worse than no instrument.
    incremental: bool,
}

impl PreambleProf {
    fn new(on: bool) -> Self {
        Self {
            on,
            last: on.then(std::time::Instant::now),
            ..Default::default()
        }
    }

    /// Microseconds since the previous lap (0 when off).
    fn lap(&mut self) -> u128 {
        if !self.on {
            return 0;
        }
        let now = std::time::Instant::now();
        self.last
            .replace(now)
            .map_or(0, |t| now.duration_since(t).as_micros())
    }

    /// One line per let-through resolve, printed once the gate's verdict is known — `skips=1` is
    /// the walk that concluded "nothing moved" and returns without solving, which costs the same
    /// preamble as one that does.
    fn report(&self, skips: bool, frames: usize, regions: usize) {
        if !self.on {
            return;
        }
        let total = self.tooltip
            + self.watched
            + self.ids
            + self.scroll
            + self.retain
            + self.plan
            + self.begin
            + self.fp_frames
            + self.fp_regions
            + self.seed;
        eprintln!(
            "[layout-pre] skips={} incr={} frames={frames} anchored={regions} total_us={total} \
             tooltip={} watched={} ids={} scroll={} retain={} plan={} begin={} \
             fp_frames={} fp_regions={} seed={}",
            u8::from(skips),
            u8::from(self.incremental),
            self.tooltip,
            self.watched,
            self.ids,
            self.scroll,
            self.retain,
            self.plan,
            self.begin,
            self.fp_frames,
            self.fp_regions,
            self.seed
        );
    }
}

/// `OnSizeChanged`'s "after": turn the entry-vs-now diff of the watched frames into queued
/// `(id, width, height)` fires ([`Model::pending_size_changed`]).
///
/// The gate is [`crate::layout::size_changed`] — the byte-verified `ApplyRect 0x76b580` test
/// (`|Δwidth| ≥ ε ∨ |Δheight| ≥ ε`, `ε = _DAT_008029d4`), not a plain `!=`, so a rect that merely
/// *moved* never fires and a sub-ε float wobble never does either.
///
/// A frame with no rect at entry is compared against the ZERO rect, which is the client's own
/// starting state (`CSimpleFrame`'s ctor zeroes the cached rect, so the first `ApplyRect` is a
/// 0×0 → w×h change and does fire). A frame that LOSES its rect queues nothing: no rect was
/// applied, so the reference had no `ApplyRect` to fire from.
fn queue_size_changes(
    watched: &[(FrameHandle, Option<Rect>)],
    resolved: &HashMap<FrameHandle, Rect>,
    frame_to_id: &HashMap<FrameHandle, u32>,
    out: &mut Vec<(u32, f32, f32)>,
) {
    for &(h, before) in watched {
        let Some(&now) = resolved.get(&h) else {
            continue;
        };
        let before = before.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        if !layout::size_changed(before, now) {
            continue;
        }
        let Some(&id) = frame_to_id.get(&h) else {
            continue;
        };
        out.push((id, now.right - now.left, now.top - now.bottom));
    }
}

impl UiScript {
    /// [`UiScript::resolve`]'s body, taking `&mut Model` directly rather than `&mut self` — so a
    /// Lua binding holding only a `Model` borrow (via `lua.app_data_mut`, no `&Lua`-wrapping
    /// `UiScript` in scope) can force a fresh resolve too. [`scrollframe`]'s `UpdateScrollChildRect`
    /// is the motivating caller: it fires `OnScrollRangeChanged` with the CURRENT range, but
    /// "current" was a lie for its own primary use (the Era idiom of resize-then-notify in one
    /// script tick) as long as it only read the LAST full [`UiScript::resolve`]'s cached rects —
    /// those don't yet reflect a `SetHeight` earlier in the SAME tick (Lua runs before the app's own
    /// per-frame `resolve()`, `ui_script/mod.rs`'s drive loop), so the fired range was one tick
    /// stale on every "just resized, tell the scrollbar" call — precisely when a caller most needs
    /// it fresh. Cheap to call on demand: the fixpoint early-exits in one round on a quiet graph
    /// (this function's own doc).
    pub(super) fn resolve_layout(model: &mut Model) {
        Self::resolve_layout_inner(model);
        if !model.layout_verify_recheck {
            return;
        }
        // ── `WOW_LAYOUT_VERIFY`: the incremental pass's falsifier (decision 1388) ─────────────
        // The pass just taken seeded its dirty closure from a LEDGER of named nodes rather than
        // from a fresh derivation, and it reused a roster and an edge set built some frames ago.
        // Two things can be wrong with that and neither is visible from inside it: a write that
        // moved a node's inputs without naming it, and a write that changed an anchor's TARGET
        // while claiming to be value-only (which leaves the edges describing a graph that no
        // longer exists). Both ship a stale rect, silently, days later.
        //
        // So: re-run the same frame from scratch — no ledger, no cache, full scope — and require
        // the rects to be identical. That subsumes both failure modes and every one we have not
        // thought of, because it compares the ANSWER rather than the reasoning. It is forced on
        // for this crate's tests, which drive the real shipped FrameXML, so every UI test is a
        // standing probe for a mis-migrated touch site.
        model.layout_verify_recheck = false;
        let before = (model.resolved.clone(), model.region_resolved.clone());
        // The meters are the incremental pass's, not the re-run's: `layout_solves` is documented
        // verify-independent and `layout_last_scope` is decision 1350's own gate. Counting the
        // falsifier's work in either would make every assertion in the suite read the wrong pass.
        let meters = (
            model.layout_solves,
            model.layout_rounds,
            model.layout_last_scope,
            model.layout_gate_walks,
            model.layout_derives,
        );
        let (queued, warned) = (model.pending_size_changed.len(), model.warnings.len());
        model.layout_touched = None; // no ledger: derive the graph
        model.layout_epoch_resolved = None; // past tier 1
        model.layout_fingerprint = None; // past tier 2
        model.layout_scope.invalidate(); // full scope
        Self::resolve_layout_inner(model);
        model.pending_size_changed.truncate(queued);
        model.warnings.truncate(warned);
        (
            model.layout_solves,
            model.layout_rounds,
            model.layout_last_scope,
            model.layout_gate_walks,
            model.layout_derives,
        ) = meters;
        assert!(
            before.0 == model.resolved && before.1 == model.region_resolved,
            "WOW_LAYOUT_VERIFY: an INCREMENTAL layout pass disagreed with a full derivation of \
             the same frame — a write site named the wrong node, named one while changing an \
             anchor's target, or moved layout inputs without naming any. \
             frames {} -> {}, regions {} -> {}",
            before.0.len(),
            model.resolved.len(),
            before.1.len(),
            model.region_resolved.len(),
        );
    }

    fn resolve_layout_inner(model: &mut Model) {
        // ── Tier 1: the mutation epoch ────────────────────────────────────────────────────────
        // Nothing has called `touch_layout` since the last converged resolve ⇒ the read set the
        // fingerprint below hashes cannot have moved ⇒ skip everything, including the tooltip
        // pre-pass and the fingerprint itself (which was the whole idle cost of this function —
        // ~0.6 ms/frame at the LBRS pin, hashing ~2k inputs per frame to decide "quiet"). Under
        // verify, fall through: tier 2 then proves the claim (`gate_skips` must agree).
        let verify = layout_verify_enabled();
        let tier1_clean = model.layout_epoch_resolved == Some(model.layout_epoch);
        if tier1_clean && !verify {
            return;
        }
        let epoch_at_entry = model.layout_epoch;
        // Past tier 1 ⇒ this call pays the whole-roster preamble, whatever the fingerprint goes on
        // to decide. Counted here rather than at the solve, because the walk IS the cost (1385).
        // Not gated on `verify`: a verify build re-walks skipped resolves and must say so.
        model.layout_gate_walks = model.layout_gate_walks.wrapping_add(1);
        // `WOW_LAYOUT_PROF=1`'s preamble split — see [`PreambleProf`]. Started here, so the clock
        // covers everything the tier-1 gate above did NOT skip.
        let mut pre = PreambleProf::new(layout_prof_enabled());
        // The GameTooltip auto-size + right-flush pre-pass (decision 0274): writes tooltip frame
        // sizes + right-column anchor offsets from the measure round-trip's cached extents, so
        // the graph below solves them like any other frame.
        super::tooltip::layout_tooltips(model);
        pre.tooltip = pre.lap();
        // ── `OnSizeChanged`'s "before" ───────────────────────────────────────────────────────
        // The client fires it from `ApplyRect 0x76b580`, per rect application. Ours is a batch
        // fixpoint, so the faithful *mechanism* — "this frame's resolved size moved" — is the
        // entry-vs-convergence diff, not a per-round one: an intermediate round's half-solved rect
        // is an implementation detail of our solver, and firing on it would hand handlers sizes the
        // reference never produces (and re-fire on the way to the same answer).
        //
        // Snapshotted only for frames that actually CARRY a handler (a handful, usually zero), and
        // only past the tier-1 gate above — an idle frame returns before this line, so the change
        // costs nothing on the quiet path decision 0740 exists to protect.
        //
        // Read off `SetScript`'s maintained list (1446's shape, extended in 1634), NOT by filtering
        // the whole `scripts` map: "a handful, usually zero" was true of the RESULT and never of
        // the search, which cost a string compare per frame in a 3,988-frame roster and was the
        // preamble's biggest phase — larger than everything the graph does with the answer.
        let watched: Vec<(FrameHandle, Option<Rect>)> = model
            .on_size_changed_frames
            .iter()
            .map(|&h| (h, model.resolved.get(&h).copied()))
            .collect();
        pre.watched = pre.lap();
        // ── The graph: the ledger's cache, or a fresh derivation ─────────────────────────────
        // `layout_touched` is tier 1's PRECISE form (decision 1388). `Some` carries the claim that
        // every write since the cached graph was built named its node and left the graph's shape
        // alone — so the roster, the edge set and the per-node hashes in `layout_scope` still
        // describe the live model, and this resolve can seed its dirty closure straight from the
        // list. That skips the derivation below entirely: 1.48 ms of whole-roster walking at the
        // Stormwind pin (3,218 frames, 10,438 anchored regions), which a single moving castbar
        // spark used to pay every frame. `None` derives, exactly as every resolve did before 1388.
        let touched = model.layout_touched.take();
        let fp = touched
            .is_none()
            .then(|| Self::derive_layout_graph(model, &mut pre));

        let Model {
            arena,
            layout_inputs,
            resolved,
            region_data,
            region_resolved,
            frame_to_id,
            screen,
            warnings,
            diagnostics,
            solver,
            layout_scope: scope,
            layout_fingerprint,
            layout_epoch_resolved,
            layout_touched,
            layout_verify_recheck,
            layout_solves,
            layout_last_scope,
            layout_rounds,
            pending_size_changed,
            texture_size_probe,
            ..
        } = model;
        // ── The incremental seed (decision 1388) ─────────────────────────────────────────────
        // Re-hash only the nodes the ledger named, and seed the ones whose hash actually moved.
        // This runs BEFORE the gate's verdict because on this path it IS the verdict: an empty
        // seed set means no named write moved anything, which is the same conclusion tier 2
        // reaches by hashing ten thousand nodes — reached here by hashing the handful that were
        // written, and carrying the scope for the ones that did move as a side effect.
        let mut scope_nodes = 0usize;
        let mut full_scope = false;
        if let Some(touched) = &touched {
            scope.begin_incremental();
            for &id in touched {
                // The node's roster row — which also says whether it is a frame or a region, and
                // where to find the live inputs to re-hash. `has_node` was already true when the
                // write named it; re-checking is a formality that costs one bounds check and
                // survives a roster the ledger somehow outlived.
                let Some(&n) = scope.node_of.get(id as usize) else {
                    continue;
                };
                if n == u32::MAX {
                    continue;
                }
                let now = if n & REGION_TAG != 0 {
                    let rh = scope.regions[(n & !REGION_TAG) as usize].rh;
                    match region_data.get(&rh) {
                        Some(d) => region_node_hash(d),
                        None => continue,
                    }
                } else {
                    let (h, _, over) = scope.plan[n as usize];
                    match layout_inputs.get(&h) {
                        Some(i) => frame_node_hash(i, over),
                        None => continue,
                    }
                };
                // The same absorption the setters' bit-compares give, one level up: a write that
                // named a node but did not move its hash (the tooltip pre-pass rewriting the same
                // size, a re-`SetPoint` to the value already there) seeds nothing.
                if now != scope.last.get(id as usize).copied().unwrap_or(NO_NODE) {
                    scope.staged.push((id, now));
                    scope.seed(id);
                }
            }
            scope.close();
            pre.seed = pre.lap();
            pre.incremental = true;
        }
        // Tier 2's verdict is a derivation's product, so an incremental pass answers with its own —
        // and a strictly finer one. Where the fingerprint says "one of these ten thousand moved",
        // the seed set says which; empty means nothing did, and last frame's rects still stand.
        // `staged` is exactly the seed set (both are pushed on the same compare), so it is the
        // question already answered.
        let gate_skips = match fp {
            Some(_) => *layout_fingerprint == fp,
            None => scope.staged.is_empty(),
        };
        pre.report(gate_skips, scope.plan.len(), scope.regions.len());
        // The tier-1/tier-2 cross-check (only reachable under verify when tier 1 judged quiet):
        // the epoch said nothing layout-visible was written, so the fingerprint must agree — a
        // divergence is a mutation path missing its `Model::touch_layout()`, named here at the
        // frame that proves it instead of shipping as a silently stale rect. An incremental pass
        // has no fingerprint to cross-check; its own, stronger falsifier is the full re-derivation
        // `resolve_layout` runs behind it under verify.
        if tier1_clean && fp.is_some() {
            assert!(
                gate_skips,
                "WOW_LAYOUT_VERIFY: the layout epoch judged this frame quiet but the input \
                 fingerprint moved — a layout write path is missing its touch_layout()"
            );
        }
        // Nothing in the read set moved since the last CONVERGED resolve, so the rects it left are
        // still this frame's answer: close tier 1 and skip. (A converged solve closes it too — see
        // the convergence arm below. This arm is the one that absorbs an epoch bump whose write
        // changed no input: an id mint, an anchor-less region's creation, a re-`SetPoint` the
        // setters' own compare let through.)
        if gate_skips {
            *layout_epoch_resolved = Some(epoch_at_entry);
            // Re-arm the ledger here too (decision 1388). A pass that found nothing to do left the
            // graph exactly as it found it — derived a moment ago on one path, untouched on the
            // other — so the cache is as trustworthy as it was. Dropping it here would make every
            // named-but-idempotent write (the tooltip pre-pass, a re-`SetPoint` to the value
            // already there) cost the NEXT real write a full derivation.
            *layout_touched = Some(Vec::new());
            if !verify {
                return;
            }
        }
        // Under `WOW_LAYOUT_VERIFY=1` a skippable resolve runs anyway, against a copy of the rects
        // it is supposed to reproduce — see [`layout_verify_enabled`].
        let verify_against = gate_skips.then(|| (resolved.clone(), region_resolved.clone()));
        // Cleared, not stored, until the fixpoint actually CONVERGES: a run that exhausts the
        // round cap (a genuine anchor cycle) leaves its rects mid-flight, and the 0294 cross-frame
        // seed is what lets the next frame carry them further. Storing here would freeze a
        // pathological graph at whatever partial state it reached. The epoch mirror follows the
        // same law: a cycle-bailed pass must re-enter tier 1 dirty next frame.
        *layout_fingerprint = None;
        *layout_epoch_resolved = None;
        // Counted on the gate's DECISION, not on the fixpoint running: under
        // `WOW_LAYOUT_VERIFY=1` a skipped resolve still runs below, and a counter that moved for
        // it would mean something different in the two modes — exactly the ambiguity the tests
        // asserting on it exist to remove.
        if !gate_skips {
            *layout_solves += 1;
        }
        // ── The scope: which nodes this solve is actually allowed to touch (decision 1350) ────
        // A node whose own inputs are unchanged, and none of whose dependencies moved, recomputes
        // to the rect it already holds — the 0294 seed property, stated per node instead of for
        // the graph as a whole. So the solve below runs over the dirty CLOSURE and leaves every
        // other rect at its cached value, feeding it to the solver as an external where a dirty
        // node anchors to it.
        //
        // Four structural cases the per-node diff cannot speak about fall back to full scope,
        // because getting them wrong ships a stale rect rather than a slow frame:
        //   * no memory yet (the first resolve, or a pass invalidated below);
        //   * the SCREEN moved — the root external and every clamped frame's extent;
        //   * a node DIED: its reverse edges died with it, so its dependents are unreachable.
        //     Counted, not searched: if fewer of this pass's nodes carry a `last` hash than the
        //     last pass had nodes, something left;
        //   * verify, which runs the scoped pass and then the full one and compares.
        //
        // An INCREMENTAL pass (decision 1388) reaches the same closure by the other end, and has
        // already done so above — it re-hashes only the nodes the ledger named and seeds the ones
        // whose hash moved. None of the four structural cases can reach it: each is a conservative
        // touch, and a conservative touch is what leaves the ledger `None`.
        if touched.is_none() {
            let mut matched = 0usize;
            for (id, &n) in scope.node_of.iter().enumerate() {
                if n == u32::MAX {
                    continue;
                }
                scope_nodes += 1;
                if scope.last.get(id).copied().unwrap_or(NO_NODE) != NO_NODE {
                    matched += 1;
                }
            }
            let a_node_died = matched != scope.last_count;
            full_scope = scope.last.is_empty()
                || scope.last_screen != Some(*screen)
                || a_node_died
                || gate_skips; // the verify path's re-run: it must reproduce the whole graph
            if full_scope {
                scope.dirty_all();
            } else {
                for (id, &n) in scope.node_of.iter().enumerate() {
                    if n != u32::MAX && scope.now[id] != scope.last[id] {
                        scope.stack.push(id as u32);
                        scope.dirty[id] = true;
                    }
                }
                scope.close();
            }
        }
        // The rosters the rounds walk. A frame in the closure is SOLVED; a region in it is SWEPT;
        // anything a closure node anchors to is SEEDED as an external at its cached rect. A dirty
        // REGION is seeded as well as swept: the frame pass runs before the sweep that refreshes
        // it, so it needs last round's value in front of it — which is exactly what the whole-graph
        // seeding did before.
        let mut solve_frames: Vec<u32> = Vec::new();
        let mut sweep_regions: Vec<u32> = Vec::new();
        let mut seed_frames: Vec<u32> = Vec::new();
        let mut seed_regions: Vec<u32> = Vec::new();
        // Is any node that reads `id` in the closure? (Seeding is per-node work too, so a rect
        // nothing dirty reads must not be seeded.)
        let read_by_dirty = |scope: &LayoutScope, id: u32| -> bool {
            let mut e = scope.dep_head[id as usize];
            while e != u32::MAX {
                if scope.dirty[scope.dep_to[e as usize] as usize] {
                    return true;
                }
                e = scope.dep_next[e as usize];
            }
            false
        };
        for id in 0..scope.node_of.len() {
            let n = scope.node_of[id];
            if n == u32::MAX {
                continue;
            }
            let (idx, is_region, dirty) = (n & !REGION_TAG, n & REGION_TAG != 0, scope.dirty[id]);
            #[allow(clippy::cast_possible_truncation)]
            let id32 = id as u32;
            if is_region {
                if dirty {
                    sweep_regions.push(idx);
                    seed_regions.push(idx);
                } else if read_by_dirty(scope, id32) {
                    seed_regions.push(idx);
                }
            } else if dirty {
                solve_frames.push(idx);
            } else if read_by_dirty(scope, id32) {
                seed_frames.push(idx);
            }
        }
        // The meter: how WIDE this solve is, beside `layout_solves` (how often) and
        // `layout_rounds` (how deep). The gate asserts on it, because a scope that tracks the
        // graph is the regression and milliseconds are not evidence of it (decision 0735's lesson,
        // paid for twice).
        *layout_last_scope = (solve_frames.len(), sweep_regions.len());
        let round_cap = scope.plan.len() + region_data.len() + 2;
        // `WOW_LAYOUT_PROF=1` — the per-solve shape: rounds, the graph's size, and the split
        // between the frame solve and the region sweep. The numbers that say whether a solve is
        // expensive because it runs too often or because each pass walks the whole UI.
        let prof = layout_prof_enabled();
        let mut t_frames = std::time::Duration::ZERO;
        let mut t_regions = std::time::Duration::ZERO;
        let mut n_regions_swept = 0u64;
        for round in 0..round_cap {
            *layout_rounds += 1;
            let t_round = prof.then(std::time::Instant::now);
            let mut changed = false;

            // Seed the solver: the screen root, then every rect this pass is NOT recomputing but
            // something in it reads (regions are externals to the frame solve — the fixpoint is
            // what closes the loop between them). Frame ids and region ids come from one monotonic
            // counter, so both live in the solver's single dense rect array and every anchor-target
            // lookup is an array index.
            solver.begin();
            solver.set_external(SCREEN, *screen);
            for &i in &seed_regions {
                let n = scope.regions[i as usize];
                if let Some(r) = region_resolved.get(&n.rh) {
                    solver.set_external(n.id, *r);
                }
            }
            for &i in &seed_frames {
                let (h, id, _) = scope.plan[i as usize];
                if let Some(r) = resolved.get(&h) {
                    solver.set_external(id, *r);
                }
            }
            for &i in &solve_frames {
                // The `layout_inputs` probe the borrowed plan used to hoist out of the round loop
                // (decision 1350) is back — but paid per frame this solve TOUCHES, not per frame in
                // the UI, which is what lets the roster itself outlive the call (1388).
                let (h, id, over) = scope.plan[i as usize];
                let Some(input) = layout_inputs.get(&h) else {
                    continue;
                };
                match over {
                    Some(a) => solver.set_frame_anchored(id, input, a),
                    None => solver.set_frame(id, input),
                }
            }
            solver.solve();
            for &i in &solve_frames {
                let (h, id, _) = scope.plan[i as usize];
                match solver.rect(id) {
                    Some(r) => {
                        if resolved.get(&h) != Some(&r) {
                            resolved.insert(h, r);
                            changed = true;
                        }
                    }
                    None => {
                        if resolved.remove(&h).is_some() {
                            changed = true;
                        }
                    }
                }
            }

            // Second pass — anchored regions. A region that carries `SetPoint` anchors resolves through
            // the same [`crate::layout`] leaf math as frames; any edge the anchors + explicit size don't
            // pin inherits the owner frame's edge (so a single-point FontString gets a real rect, not
            // `+Inf`). Anchor targets may be frames OR **sibling regions by name** (the real XML does
            // this everywhere — the merchant label plate anchors to its `$parentSlot` texture; the
            // owner-fallback that stood in for this was the jutting-plates bug). One region sweep per
            // outer round: a not-yet-resolved chain target leaves this region on its owner-edge
            // fallback for now, the fixpoint re-sweeps until every link has settled.
            //
            // `scratch` is hoisted out of the loop so the per-region `LayoutInput` refills its
            // anchors `Vec` in place instead of allocating one per region per round.
            if let Some(t) = t_round {
                t_frames += t.elapsed();
            }
            let t_reg = prof.then(std::time::Instant::now);
            let mut scratch = LayoutInput::default();
            for &ri in &sweep_regions {
                let RegionRow {
                    rh,
                    id: region_id,
                    owner,
                    is_fontstring,
                } = scope.regions[ri as usize];
                // The roster's mutable half, re-probed here rather than borrowed into the row —
                // once per region SWEPT instead of once per region in the UI (decision 1388). A
                // row whose data has gone is a region the derive will drop on its next run; it
                // resolves to nothing in the meantime, exactly as a dead one does.
                let Some(data) = region_data.get(&rh) else {
                    continue;
                };
                if prof {
                    n_regions_swept += 1;
                }
                // The owner supplies the region's SCALE and nothing else. It used to supply
                // fallback edges too; it does not any more (decision 1664) — see the `axis`
                // closure below for why, and what the two shapes that leaned on it get instead.
                let scale = arena.frame(owner).map(|f| f.effective_scale).unwrap_or(1.0);
                // A FontString with no explicit height takes its host-measured wrapped size
                // (the measure round-trip — the client's layout↔font-engine seam).
                //
                // The stored measure is honoured WITHOUT a key check on purpose
                // ([`RegionData::measure_key`]): it is the LAST-KNOWN box, held so a line whose
                // text just changed does not collapse for the one frame the re-measure is in
                // flight. **Empty text is the case where that promise cannot be kept** — nothing
                // ever measures an empty string (`fontstrings_needing_measure` and
                // `stale_measure_key` both filter it out), so a cell that goes from text to `""`
                // would hold its old box FOREVER. There is no round-trip to wait for: an empty
                // string's extent is known, and it is zero.
                //
                // That forever-stale box is B309. Tooltip line cells are POOLED and reused across
                // hovers (`clear_content` keeps `measured` deliberately, for the hover re-enter
                // loop), so the item tooltip's two blank gold SET spacers (`render.rs`, `0x854b2c`)
                // landed on cells that had carried real text on an earlier hover: each drew a full
                // row out of the dead measure while the plate counted it as zero
                // (`tooltip::cell` calls empty text `(0,0)`), and the set bonuses hung two rows
                // below the backdrop. The Lua-visible getters never disagreed — they key-check
                // (`region::measured_wh`) — which is why only the drawn geometry was wrong.
                let measured = data
                    .measured
                    .filter(|_| data.text.as_deref().is_some_and(|t| !t.is_empty()));
                let mut height = data.size.map_or(0.0, |s| s.1);
                let mut width = data.size.map_or(0.0, |s| s.0);
                if let Some(m) = &measured {
                    if height == 0.0 {
                        height = m.h;
                    }
                    if width == 0.0 {
                        width = m.w;
                    }
                }
                // …and then the FLOOR. `CSimpleFontString::GetWidth 0x772930` takes the
                // authored value when it is not exactly `0.0` and the measured extent otherwise,
                // then returns `max(candidate, ONE FrameXML unit)` — the floor sits past the
                // `jp 0x772957` that skips the measure, so it applies to BOTH legs. `0x772a60` is
                // the identical body for height. **A FontString's span is therefore never `0.0`**,
                // which is what makes a single-anchored FontString always resolve — and what
                // retires the zero-span collapse this sweep
                // used to apply in the `axis` closure below.
                if is_fontstring {
                    width = width.max(FONTSTRING_MIN_SPAN);
                    height = height.max(FONTSTRING_MIN_SPAN);
                }
                // A TEXTURE's unset axis is the client's *other* content-derived span: the art's
                // own texel extent ([`content_span`]). It is the same shape as the FontString
                // measure above — the reference's size getter is virtual, and both region classes
                // override it — and it has to be applied HERE, before the resolve, because
                // `combine_edge` reads a zero span as "no size at all" and leaves the opposite edge
                // unresolved.
                //
                // That vacuum is what B342 fell into. `page_text` 2654 (*A Treatise on Military
                // Ranks*) carries `<IMG src="…\PvPRankAlliance" align="left"/>` — **no width=, no
                // height=** — so SimpleHTML sized the block `0 × 0`, one TOPLEFT anchor pinned the
                // corner, and the owner-edge fallback below stretched a 128×128 crest across the
                // rest of the 270×304 page with the text over it. The reference draws it at 128×128
                // units, because a texel is a FrameXML unit.
                if !is_fontstring && (width == 0.0 || height == 0.0) {
                    if let Some((cw, ch)) = content_span(data, texture_size_probe.as_ref()) {
                        if width == 0.0 {
                            width = cw;
                        }
                        if height == 0.0 {
                            height = ch;
                        }
                    }
                }
                scratch.anchors.clear();
                scratch.anchors.extend_from_slice(&data.anchors);
                scratch.width = width;
                scratch.height = height;
                scratch.scale = scale;
                let edges = layout::resolve_rect_edges(&scratch, |id| {
                    // SCREEN is a FRAME-pass external only. The map-pair lookup this replaces
                    // consulted `id_to_frame` and `id_to_region`, and neither can hold the
                    // reserved id 0 (`next_id` starts at 1) — so a region anchor to the screen
                    // root has always fallen through to the owner-edge fallback below. Preserved
                    // deliberately: the solver's dense array DOES carry the screen rect at 0, and
                    // quietly starting to resolve it here would be a behaviour change riding in
                    // on a performance fix.
                    (id != SCREEN).then(|| solver.rect(id)).flatten()
                });
                // **An axis resolves, or the region does not** — there is no owner-edge fallback
                // and no zero-span collapse (decision 1664). A complete memory-operand enumeration
                // of the real resolver core `[0x7671a0, 0x76761f)` finds no parent or owner pointer
                // in it at all: the only fallbacks are the object's own nine anchor slots and
                // `combineEdge`/`combineCenter` over its own opposite edge/centre ± its own size.
                // `assemble 0x767a20` returns 0 the moment any edge is still UNSET, and
                // `0x768d20` latches the region unresolvable.
                //
                // The two shapes that leaned on the fallback both have their own answer now, which
                // is why deleting it is a fidelity fix and not a hole:
                //
                // * **No anchors at all.** The client anchors those at CREATION, per region type —
                //   a texture gets `SetAllPoints(parent)`, a FontString one justify-selected
                //   `SetPoint` — so by the time the resolver sees them they are pinned like
                //   anything else ([`super::region::implicit_creation_anchor`], decision 1310).
                //   The ones that reach here still unanchored are the shapes the reference leaves
                //   rect-less too: a templateless `CreateTexture`, a title region, a plain frame.
                // * **A pinned edge with a zero span.** The span is content-derived on both region
                //   classes — the texture's texel extent above, the FontString's measured extent
                //   floored at one unit — so `combineEdge`'s zero-span leg is nearly unreachable
                //   for a region at all (`0x767440`). What is left is a texture
                //   with no art whatever, which the reference resolves to nothing and draws as a
                //   degenerate all-zero quad.
                let axis = |lo: Option<f32>, hi: Option<f32>| -> Option<(f32, f32)> {
                    match (lo, hi) {
                        (Some(l), Some(h)) => Some((l, h)),
                        _ => None,
                    }
                };
                let vertical = axis(edges[0], edges[2]);
                let horizontal = axis(edges[1], edges[3]);
                let (Some((bottom, top)), Some((left, right))) = (vertical, horizontal) else {
                    // Unresolvable: drop any rect it used to have, the way the frame pass does —
                    // a region that stops resolving must stop drawing, not keep a stale position.
                    if region_resolved.remove(&rh).is_some() {
                        changed = true;
                    }
                    continue;
                };
                let rect = Rect::new(bottom, left, top, right);
                // Publish into the solver as well as the model: a later region in THIS same sweep
                // that anchors to this one must see the fresh rect (the sweep has always worked
                // that way — it read the same `region_resolved` map it was writing).
                solver.set_external(region_id, rect);
                if region_resolved.get(&rh) != Some(&rect) {
                    region_resolved.insert(rh, rect);
                    changed = true;
                }
            }

            if let Some(t) = t_reg {
                t_regions += t.elapsed();
            }
            if !changed {
                if prof {
                    // `anchored` (live + anchored regions) is reported beside `regions_total`
                    // deliberately: the sweep never touched the difference, so a solve's real
                    // population was invisible in this line — which is exactly why two commits
                    // that moved a large batch of regions from "skipped" to "resolved" (1350's
                    // pin) showed up as a cost with no counter behind it. `solved`/`swept` are
                    // this pass's SCOPE: with the graph settled they should be a handful.
                    eprintln!(
                        "[layout-prof] rounds={} frames={} regions_total={} anchored={} \
                         solved={} swept={} scope={} regions_swept={} frame_us={} region_us={}",
                        round + 1,
                        scope.plan.len(),
                        region_data.len(),
                        scope.regions.len(),
                        solve_frames.len(),
                        sweep_regions.len(),
                        if full_scope { "full" } else { "dirty" },
                        n_regions_swept,
                        t_frames.as_micros(),
                        t_regions.as_micros()
                    );
                }
                if let Some((frames_before, regions_before)) = &verify_against {
                    assert!(
                        frames_before == resolved && regions_before == region_resolved,
                        "WOW_LAYOUT_VERIFY: the change gate skipped a resolve that would have \
                         MOVED something — the fingerprint's read set is incomplete. \
                         frames {} -> {} changed, regions {} -> {} changed",
                        frames_before.len(),
                        resolved.len(),
                        regions_before.len(),
                        region_resolved.len(),
                    );
                }
                *layout_fingerprint = fp;
                // The scope's memory is adopted only HERE, on the converged path, for the same
                // reason the fingerprint is: a pass left mid-flight describes a graph that never
                // settled, and seeding the next pass's "unchanged ⇒ unmoved" argument from it
                // would be seeding it from a lie.
                //
                // An incremental pass adopts only the nodes it re-hashed — the rest of `last` is
                // already the memory of the derivation this pass is standing on (decision 1388).
                if touched.is_some() {
                    scope.commit_incremental();
                } else {
                    scope.commit(scope_nodes, *screen);
                }
                // ── Re-arm the ledger ─────────────────────────────────────────────────────────
                // The graph in `scope` now describes a CONVERGED model, so the next frame's writes
                // may name their nodes against it. Empty, not `None`: `None` is the standing claim
                // that the cache is untrustworthy, and it has just been made trustworthy.
                //
                // Armed only on convergence, and only here — the cycle bail below leaves it `None`
                // along with everything else, because a graph that never settled is exactly the
                // one a ledger must not vouch for.
                *layout_touched = Some(Vec::new());
                // Under verify, tell `resolve_layout` to re-derive this same frame from scratch and
                // compare rects — the incremental pass's falsifier. Costs nothing in production,
                // where `layout_verify_enabled` is false and nothing ever reads this.
                *layout_verify_recheck = touched.is_some() && verify;
                // **A CONVERGED SOLVE CLOSES TIER 1** (decision 1385) — unconditionally, not only
                // on the `gate_skips` re-run. The fingerprint above is hashed over INPUTS alone,
                // and the rounds just drove those inputs to their fixpoint, so the fp stored on
                // the line above is exactly the one the next mutation-free resolve recomputes:
                // there is nothing left for a settling pass to discover. Closing here is what
                // makes a per-frame layout write cost ONE whole-roster walk instead of three
                // (solve, settle, skip) — the castbar's spark, and every addon that animates a
                // region, used to pay all three every frame.
                //
                // `epoch_at_entry` (not the live epoch) keeps it conservative: anything that
                // touched the layout WHILE this pass ran — the tooltip pre-pass, a lazily minted
                // region — leaves the epoch ahead of the stored value, so tier 1 re-opens next
                // call exactly as it should. The cycle-bail path below never reaches here, so a
                // graph left mid-flight still re-resolves.
                *layout_epoch_resolved = Some(epoch_at_entry);
                queue_size_changes(&watched, resolved, frame_to_id, pending_size_changed);
                return;
            }
            if round + 1 == round_cap {
                // `Model::record_warning`'s two halves, spelled out because this pass holds the
                // model destructured and cannot call it: the host drain, and the retained log
                // (2135). `WOW_LAYOUT_VERIFY`'s falsifier re-run truncates `warnings` and does
                // NOT roll the log back — the log dedupes, so a verify build reads `×2` on a row
                // that happened once, and that build is not a production path.
                let msg = format!(
                    "layout: anchor graph did not converge in {round_cap} rounds — \
                     an anchor cycle? (rects left at their last pass)"
                );
                diagnostics.record(super::diagnostics::DiagnosticKind::Warning, &msg);
                warnings.push(msg);
            }
        }
        // The cycle bail (the loop ran out of rounds and warned above): the rects it leaves are
        // still the ones every reader will see this frame, so the sizes that moved get their
        // `OnSizeChanged` here exactly as on the converged path — and the scope forgets, so the
        // next resolve rebuilds the whole graph rather than trusting a half-solved one.
        scope.invalidate();
        queue_size_changes(&watched, resolved, frame_to_id, pending_size_changed);
    }

    /// Derive the layout graph from scratch — the roster of live frames and anchored regions, the
    /// reverse-edge set between them, the per-node input hashes, and the aggregate fingerprint
    /// that is tier 2 of the change gate.
    ///
    /// Every resolve did this, until decision 1388. It is ~1.48 ms at the Stormwind pin, 79% of it
    /// in two phases (`fp_regions` 919 µs, `retain` 255 µs) that rediscover a graph which had not
    /// changed shape. It now runs only when the ledger cannot vouch for the cache: the first
    /// resolve, a cycle-bailed one, and any frame in which a write moved the roster, retargeted an
    /// anchor, or could not name what it touched.
    fn derive_layout_graph(model: &mut Model, pre: &mut PreambleProf) -> InputFingerprint {
        model.layout_derives = model.layout_derives.wrapping_add(1);
        let Model {
            arena,
            layout_inputs,
            region_data,
            region_resolved,
            frame_to_id,
            id_to_region,
            region_to_id,
            next_id,
            screen,
            layout_scope: scope,
            ..
        } = model;

        // One id per live frame, and its layout input with scale synced from the arena. A flat
        // `Vec`, not a map: this list is walked (never probed) by every round below, and it is
        // rebuilt on each call anyway — a `HashMap` bought nothing and cost 1881 hashes per call.
        let mut ids: Vec<(FrameHandle, u32)> = Vec::with_capacity(frame_to_id.len());
        for (&h, &id) in frame_to_id.iter() {
            if arena.frame(h).is_some() {
                ids.push((h, id));
            }
        }
        for &(h, _) in &ids {
            let (scale, clamp) = arena
                .frame(h)
                .map(|f| (f.effective_scale, f.clamped_to_screen))
                .unwrap_or((1.0, false));
            let input = layout_inputs.entry(h).or_default();
            input.scale = scale;
            // Clamp-to-screen is a FRAME property (geometry flags bit4 — SetClampedToScreen, the
            // XML attribute, the GameTooltip-kind default), synced like scale so EVERY placement
            // path — SetOwner's anchor law, a caller's SetPoint, the cursor-seated world plates —
            // resolves through the same clamp, with extents tracking the live window size.
            input.clamp = clamp;
            if clamp {
                input.extent_x = screen.right;
                input.extent_y = screen.top;
            }
        }
        pre.ids = pre.lap();

        // The ScrollFrame mechanism (decision 0112): a live ScrollFrame with a live scroll child
        // overrides the child's own anchors for this solve — `SetScrollChild` pins the child TOPLEFT
        // to the scrollframe's TOPLEFT, offset by the two live scroll offsets
        // (`(horizontal, vertical)`, both RAW and unnegated, exactly as the reference's re-anchor
        // `0x787100` hands them to `SetPoint`). XML y-positive-up, so a positive vertical lifts the
        // child, bringing content below the fold into view — `frame top 500, vertical 40 ⇒ child top
        // 540`; x grows right, so a positive horizontal pushes the child right and it is a NEGATIVE
        // horizontal that scrolls right (see `ScrollFrameState::horizontal`). This is a LOCAL map, consulted only
        // while building each round's graph below — the child's authored `LayoutInput.anchors` are
        // never touched, so `SetScrollChild(nil)` needs no restore: the override just stops being
        // computed. The child's own width/height (from its own `LayoutInput`) stay whatever they are;
        // only its anchors are replaced.
        let mut scroll_child_anchor: HashMap<FrameHandle, Anchor> = HashMap::new();
        for &(h, id) in &ids {
            let Some(frame) = arena.frame(h) else {
                continue;
            };
            if frame.kind != FrameKind::ScrollFrame {
                continue;
            }
            let KindState::Scroll(state) = &frame.kind_state else {
                continue;
            };
            let Some(child) = state.child else { continue };
            if arena.frame(child).is_none() {
                continue; // a stale/destroyed child contributes no override
            }
            scroll_child_anchor.insert(
                child,
                Anchor::new(
                    Point::TopLeft,
                    id,
                    Point::TopLeft,
                    state.horizontal,
                    state.vertical,
                ),
            );
        }
        pre.scroll = pre.lap();

        // Alternating frame/region rounds, run to a CHANGE-DRIVEN FIXPOINT: the real client
        // resolves ONE layout graph in which frames and regions anchor each other freely (the
        // gossip option rows anchor to the greeting FontString's laid-out bottom; the quest log's
        // detail pane chains ~15 regions deep with item BUTTONS anchored to the tail). Our frame
        // pass is a batch graph solve, so each round re-solves it with the latest REGION rects as
        // externals, re-resolves the regions against the new frame rects, and stops when a full
        // round changed nothing. The old fixed budget (2 frame rounds × 3 region rounds) silently
        // left chain links past ~6 on owner-edge fallbacks and dropped tail-anchored frames to the
        // screen origin — decision 0088 §2's finding, which forced windows into invented fixed
        // offsets instead of ref-verbatim chains. The round cap is the node count (an honest round
        // binds at least one new link), so only a genuine anchor CYCLE can exhaust it — warned,
        // mirroring the client's resolving-flag cycle bail. Convergence compares exact rects: each
        // pass recomputes from the same inputs through the same arithmetic, so stabilized values
        // repeat bit-for-bit (no epsilon drift).
        //
        // The fixpoint CARRIES ACROSS FRAMES (decision 0294): every rect is recomputed purely from
        // (inputs, externals), never from its own prior value, so last frame's converged rects are a
        // legal seed — a quiet frame re-verifies in ONE round instead of re-propagating every anchor
        // chain link-by-link (measured: the full default UI held 5–10 rounds × ~7 ms/round at
        // opt-level 0, every frame, ~72 ms of a quiet frame — the director's 60→50 fps). Only
        // regions that left the model (or lost their anchors) drop; a from-scratch resolve is just
        // the empty-seed special case, so first-frame behavior is unchanged.
        region_resolved.retain(|rh, _| {
            arena.region(*rh).is_some()
                && region_data.get(rh).is_some_and(|d| !d.anchors.is_empty())
        });
        pre.retain = pre.lap();

        // ── The change gate ───────────────────────────────────────────────────────────────────
        // Fingerprint exactly what the rounds below read, and skip them outright when nothing has
        // moved since the last converged resolve. Measured on the shipped default UI: the layout
        // inputs are byte-identical on 34 of 35 idle frames and 39 of 41 with three windows open,
        // because almost all per-frame UI traffic (StatusBar values, cooldown sweeps, chat fades,
        // colour changes) is read by EXTRACT, not by the anchor solve.
        //
        // The read set, in the order the rounds consume it:
        //   * `screen`                      — the root external, and the clamp extents;
        //   * `plan`                        — every live frame's id, its (already scale/clamp-
        //                                     synced) `LayoutInput`, and its ScrollFrame override,
        //                                     which together distil `frame_to_id`, frame liveness,
        //                                     `effective_scale`, `clamped_to_screen`, and the
        //                                     ScrollFrame child + vertical offset;
        //   * `region_data`                 — each region's anchors, explicit size, and measured
        //                                     text extent;
        //   * `arena.region(rh)`            — liveness and owner (a dead region drops out of the
        //                                     sweep; the owner supplies the fallback edges).
        //
        // **INPUTS ONLY — the 0294 seeds are deliberately NOT hashed** (decision 1385). They are
        // the previous pass's OUTPUT, and 0294's own property ("every rect is recomputed purely
        // from (inputs, externals), never from its own prior value") makes a converged pass's
        // rects a pure function of the inputs: the seeds change how many ROUNDS convergence
        // takes, never what it converges to. Hashing them therefore added no verdict the input
        // half does not already give — and it cost two extra whole-roster walks per mutated
        // frame, because a solve necessarily outgrows the seeds it hashed, so neither that pass
        // nor the settling pass behind it could close tier 1. (`resolved`, the FRAME rects, was
        // never in the read set at all — the seed half was asymmetric as well as redundant.)
        //
        // ── …and, in the same walk, the SCOPE (decision 1350) ─────────────────────────────────
        // Each node's own inputs are hashed a second time on their own, into
        // [`LayoutScope::now`], and its anchor targets are recorded as reverse edges. That turns
        // the gate's one verdict ("something moved") into the far more useful one ("*these* moved,
        // and here is everything downstream of them"), for the cost of a second accumulator over
        // a read set we were walking anyway. Everything the aggregate `fp` feeds still feeds it,
        // byte for byte, so tiers 1 and 2 are untouched.
        // Sized past `next_id` because the region walk below MINTS ids (see there) — every id it
        // can mint is one more than the anchored regions it walks.
        scope.begin_full(*next_id as usize + region_data.len() + 1);
        pre.begin = pre.lap();
        let mut fp = InputFingerprint::default();
        fp.rect(*screen);
        // The frame roster is built HERE rather than in a walk of its own (decision 1388): it is
        // the same `ids` list, the same `layout_inputs` probe and the same `scroll_child_anchor`
        // lookup this walk already needs, so a separate pass over every live frame bought nothing
        // but a second traversal (77 µs of the old preamble) and a borrow that stopped the roster
        // outliving the call.
        for &(h, id) in &ids {
            let Some(input) = layout_inputs.get(&h) else {
                continue;
            };
            let over = scroll_child_anchor.get(&h).copied();
            #[allow(clippy::cast_possible_truncation)]
            let i = scope.plan.len() as u32;
            fp.feed(u64::from(id));
            fp.input(input);
            match over {
                Some(a) => fp.anchors(std::slice::from_ref(&a)),
                None => fp.feed(u64::MAX),
            }
            if let Some(slot) = scope.now.get_mut(id as usize) {
                *slot = frame_node_hash(input, over);
                scope.node_of[id as usize] = i;
            }
            scope.plan.push((h, id, over));
            // A frame reads the rect of every anchor target it has — its OVERRIDE's target when a
            // ScrollFrame supplies one (the authored anchors are not consulted at all in that
            // case, exactly as the round loop's `set_frame_anchored` does not consult them).
            match over {
                Some(a) => scope.edge(id, a.relative_to),
                None => {
                    for a in &input.anchors {
                        scope.edge(id, a.relative_to);
                    }
                }
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            fp.feed(scope.plan.len() as u64);
        }
        pre.fp_frames = pre.lap();
        let mut fed_regions = 0u64;
        // The round loop's region roster, built in this same walk (decision 1350) — see
        // [`RegionRow`]. Only LIVE, anchored regions: an anchor-less one is invisible to the
        // rounds and a destroyed one has already been dropped from `region_resolved` by the retain
        // above, so both are exactly what the sweep used to `continue` past.
        scope.regions.reserve(region_data.len());
        for (&rh, data) in region_data.iter() {
            // Anchor-less entries are invisible to the rounds — the sweep `continue`s on empty
            // anchors and the seed retain drops them — so they are not inputs and must not be
            // hashed: a paint-only setter creating a region's entry (`SetTexture`'s
            // `or_default()`) would otherwise read as a layout change no touch site can own.
            if data.anchors.is_empty() {
                continue;
            }
            fed_regions += 1;
            let live = arena.region(rh);
            fp.feed(rh.fingerprint_bits());
            fp.anchors(&data.anchors);
            match data.size {
                Some((w, h)) => {
                    fp.f32(w);
                    fp.f32(h);
                }
                None => fp.feed(u64::MAX),
            }
            match data.measured {
                Some(m) => {
                    fp.f32(m.w);
                    fp.f32(m.h);
                }
                None => fp.feed(u64::MAX),
            }
            if let Some(r) = live {
                // Mint the layout id here if the region has never needed one. The sweep used to
                // resolve an id-less region and simply not publish its rect (`if let Some(&id) =
                // region_to_id.get(..)`) — an EditBox's implicit text FontString is exactly that
                // shape, created and anchored by `SetTextInsets` and given an id only when extract
                // or a measure first asks. The scope addresses nodes BY id, so it needs one; and
                // minting it is pure addressing — it moves no rect, so unlike `Model::region_id`
                // this does not touch the layout epoch (doing so would re-dirty the very resolve
                // that is running).
                let id = match region_to_id.get(&rh) {
                    Some(&id) => id,
                    None => {
                        let id = *next_id;
                        *next_id += 1;
                        region_to_id.insert(rh, id);
                        id_to_region.insert(id, rh);
                        id
                    }
                };
                #[allow(clippy::cast_possible_truncation)]
                let idx = scope.regions.len() as u32 | REGION_TAG;
                if let Some(slot) = scope.now.get_mut(id as usize) {
                    *slot = region_node_hash(data);
                    scope.node_of[id as usize] = idx;
                }
                // A region reads its anchor targets' rects, and — for any axis its anchors do not
                // pin — its OWNER's rect and scale. Both are dependencies; the owner one is the
                // edge that carries a window moving to every unpinned texture inside it.
                for a in &data.anchors {
                    scope.edge(id, a.relative_to);
                }
                if let Some(&owner_id) = frame_to_id.get(&r.owner) {
                    scope.edge(id, owner_id);
                }
                scope.regions.push(RegionRow {
                    rh,
                    id,
                    owner: r.owner,
                    is_fontstring: matches!(r.kind, crate::widget::RegionKind::FontString),
                });
            }
            // Liveness only. The sweep also reads the region's `owner` and `kind`, but both are
            // fixed at creation and can never change under a live handle; the owner's contribution
            // to the result — its resolved rect and its `effective_scale` — rides the frame half
            // (the scale is synced into `layout_inputs` by the preamble above, and the rect is the
            // frame pass's own output). A DESTROYED region does still matter: `WidgetArena::destroy`
            // drops the arena entry but leaves `region_data` behind, so membership alone would miss
            // it and the sweep's `continue` would go unnoticed.
            fp.feed(u64::from(live.is_some()));
        }
        // The count of FED entries (not the map's len — see the vacuous-entry skip above), so an
        // entry leaving the anchored set is a change even if a sibling enters the same frame.
        fp.feed(fed_regions);
        pre.fp_regions = pre.lap();
        fp
    }

    /// FontStrings whose layout needs a host text measurement (an auto-sized axis — explicit
    /// width and/or height of 0, the client's size-to-text idiom — text present, cache stale) —
    /// the engine side of the measure round-trip. Call after [`UiScript::resolve`]; answer with
    /// [`UiScript::set_measured_text`] and resolve again (the second solve is cheap and the cache
    /// keys keep this empty on quiet frames). A zero WIDTH auto-sizes exactly like a zero height
    /// (the real client sizes the rect to the unwrapped line — `<Size x="0" y="16"/>` labels like
    /// the mail window's "From:" anchor their RIGHT edge and grow leftward); gating on height
    /// alone left those rects zero-width and their anchored neighbours overlapping.
    pub fn fontstrings_needing_measure(&mut self) -> Vec<MeasureRequest> {
        let mut model = self.model_mut();
        // ANY FontString with text — not only the auto-sized ones. A region with both axes
        // declared needs no measure for its *layout*, but `GetStringWidth` still has to answer
        // with the string's natural width, and only a measure can supply it (decision 0997: a
        // kit that reads that number and then SETS a width on the string would otherwise stop
        // receiving measures the instant it did so, and start reading its own box back).
        //
        // Asked off the MEASURE LEDGER, not the roster (the 1388 shape one lane over): the
        // sweep existed to *discover* silent writes — `SetText` touches no layout until the
        // extent moves — and discovery cost two whole-roster walks per frame at a city pin
        // (~300 µs of the `resolve` lap, decision 1370) to conclude, almost always, that
        // nothing was written. The write sites announce themselves now
        // ([`Model::touch_measure`]); draining the ledger re-arms it, so the second ask of the
        // same frame (`fill_measures`, inside `resolve`) sees only what was written *between*
        // the asks — same-frame semantics unchanged. `None` (a fan-out that could not name its
        // regions, or the initial state) is the old whole-roster walk, verbatim.
        let ledger = model.measure_dirty.take();
        model.measure_dirty = Some(Vec::new());
        let needy: Vec<RegionHandle> = match ledger {
            Some(mut dirty) => {
                dirty.sort_unstable_by_key(|rh| rh.fingerprint_bits());
                dirty.dedup();
                dirty.retain(|&rh| stale_measure_key(&model, rh).is_some());
                // The completeness falsifier (on for every benilla-ui test): the ledger's whole
                // correctness claim is that its enrolled sites cover every measure-key writer,
                // and that claim is checked, not argued — the old discovery walk runs beside
                // the drain and any row it finds that the ledger missed is a stale label
                // waiting to ship, so it panics with the region named. The other direction is
                // legal by construction (an enrolled same-value write drops out at the
                // staleness check above).
                if layout_verify_enabled() {
                    let roster: Vec<RegionHandle> = model
                        .region_data
                        .iter()
                        .filter(|(_, d)| d.text.as_deref().is_some_and(|t| !t.is_empty()))
                        .map(|(&rh, _)| rh)
                        .filter(|&rh| stale_measure_key(&model, rh).is_some())
                        .collect();
                    for rh in &roster {
                        assert!(
                            dirty.contains(rh),
                            "measure ledger missed a write: region {rh:?} (id {:?}) needs a \
                             measure the drained ledger never named — a measure-key write site \
                             is not enrolled in Model::touch_measure",
                            model.region_to_id.get(rh),
                        );
                    }
                }
                dirty
            }
            None => model
                .region_data
                .iter()
                .filter(|(_, d)| d.text.as_deref().is_some_and(|t| !t.is_empty()))
                .map(|(&rh, _)| rh)
                .filter(|&rh| stale_measure_key(&model, rh).is_some())
                .collect(),
        };
        // Re-enroll everything still stale: a request handed out is not an answer. The old
        // discovery walk re-found an unanswered region every frame until `set_measured_text`
        // landed (the no-measurer engine-test path answers a frame later; a dropped request
        // would otherwise go stale FOREVER now that nothing walks the roster), so the ledger
        // keeps asking too. With a synchronous measurer the answer lands before the next drain
        // and the staleness check drops the re-enrolled handle for free.
        if let Some(list) = &mut model.measure_dirty {
            list.extend(needy.iter().copied());
        }
        let mut out = Vec::with_capacity(needy.len());
        for rh in needy {
            if let Some(req) = measure_request_for(&mut model, rh) {
                out.push(req);
            }
        }
        out
    }

    /// Push one [`QuadContent::Backdrop`] per piece of frame `fh`'s installed backdrop (no-op if the
    /// frame has none). Pieces carry the frame slot's `z` — behind the frame's regions (which sort
    /// after it) and, among themselves, bg-then-border in paint order (the stable z-sort keeps the
    /// emission order). Each piece's rect is its screen bounding box; the app resolves the texture
    /// from the piece path and multiplies the tint by the [`FramePaint`]'s alpha.
    pub(super) fn emit_backdrop(
        model: &Model,
        fh: FrameHandle,
        fr: Rect,
        z: u64,
        paint: FramePaint,
        clip: Option<Rect>,
        out: &mut Vec<ExtractedQuad>,
    ) {
        let Some(bd) = model.backdrops.get(&fh) else {
            return;
        };
        for piece in backdrop::pieces(fr, bd) {
            // The piece's screen bounding box (the render is axis-aligned; the BR-inset bug's slant,
            // invisible for the symmetric insets every shipping backdrop uses, collapses to the box).
            let xs = piece.corners.map(|c| c[0]);
            let ys = piece.corners.map(|c| c[1]);
            let (left, right) = (
                xs.iter().copied().fold(f32::MAX, f32::min),
                xs.iter().copied().fold(f32::MIN, f32::max),
            );
            let (bottom, top) = (
                ys.iter().copied().fold(f32::MAX, f32::min),
                ys.iter().copied().fold(f32::MIN, f32::max),
            );
            let path = if piece.is_bg {
                bd.bg_file.clone()
            } else {
                bd.edge_file.clone()
            };
            let Some(path) = path else { continue };
            let color = if piece.is_bg {
                bd.bg_color
            } else {
                bd.border_color
            };
            out.push(ExtractedQuad {
                target: ZTarget::Frame(fh),
                z,
                rect: Some(Rect::new(bottom, left, top, right)),
                alpha: paint.alpha,
                content: QuadContent::Backdrop {
                    path,
                    color,
                    uvs: piece.uvs,
                    tile: piece.tile,
                },
                clip,
                scale: paint.scale,
            });
        }
    }
}

/// The [`MeasureRequest`] region `rh` needs right now, or `None` if it needs none — it is not a
/// FontString, it holds no text, or its stored measure is already this exact string's.
///
/// Hoisted out of [`UiScript::fontstrings_needing_measure`] so the **synchronous** measure
/// ([`super::measure::ensure_measured`]) builds its request with the identical recipe. The two must
/// agree byte for byte on the cache key, or a same-tick measure and the batch pass would each think
/// the other's answer was stale and re-measure forever.
pub(super) fn measure_request_for(model: &mut Model, rh: RegionHandle) -> Option<MeasureRequest> {
    let (key, scale) = stale_measure_key(model, rh)?;
    let d = model.region_data.get(&rh)?;
    let text = d.text.clone().filter(|t| !t.is_empty())?;
    let wrap_width = d.size.map(|s| s.0).filter(|w| *w > 0.0);
    let font = d.font_path.clone();
    let height = d.font_height;
    let text_height = d.text_height;
    let outline = d.outline;
    let id = model.region_id(rh);
    Some(MeasureRequest {
        id,
        font,
        height,
        text_height,
        wrap_width,
        outline,
        scale,
        text,
        key,
    })
}

/// The staleness half of [`measure_request_for`], allocation-free: `Some((key, scale))` iff `rh`
/// is a FontString holding text whose stored measure no longer matches its current key. This is
/// the ONE place the "does this region need a measure?" question is answered — the per-frame
/// sweep ([`super::UiScript::fontstrings_needing_measure`]) asks it for every FontString, so it
/// must not clone; the request build above pays the clones only after this says stale.
fn stale_measure_key(model: &Model, rh: RegionHandle) -> Option<(u64, f32)> {
    let region = model.arena.region(rh)?;
    if !matches!(region.kind, crate::widget::RegionKind::FontString) {
        return None;
    }
    // The owner's effective_scale: the host measures at the drawn raster size
    // ([`MeasureRequest::scale`]), and the key carries it so a SetScale re-measures.
    let scale = model
        .arena
        .frame(region.owner)
        .map(|f| f.effective_scale)
        .unwrap_or(1.0);
    let d = model.region_data.get(&rh)?;
    if d.text.as_deref().is_none_or(|t| t.is_empty()) {
        return None;
    }
    // The shared key recipe ([`RegionData::measure_key`]) — the metric reads (region.rs) check the
    // stored measure against the same hash.
    let key = d.measure_key(scale);
    if d.measured.map(|m| m.key) == Some(key) {
        return None;
    }
    Some((key, scale))
}
