use std::collections::HashMap;

use crate::layout::{self, Anchor, LayoutInput, Point, Rect};
use crate::order::ZTarget;
use crate::widget::{FrameHandle, FrameKind, KindState, RegionHandle};

use super::backdrop;
use super::types::RegionData;
use super::{ExtractedQuad, MeasureRequest, Model, QuadContent, UiScript, SCREEN};

/// A frame's `effective_alpha` and `effective_scale`, worn by the quads it emits itself.
#[derive(Clone, Copy)]
pub(super) struct FramePaint {
    pub(super) alpha: f32,
    pub(super) scale: f32,
}

/// A 128-bit fingerprint of what [`UiScript::resolve_layout`] reads: tier 2 of the change gate,
/// behind the mutation epoch ([`Model::touch_layout`]). Two lanes, as a collision keeps a stale
/// rect; fed in `HashMap` order, stable for an unmutated map, so a reorder only costs a resolve.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputFingerprint(u64, u64);

impl Default for InputFingerprint {
    fn default() -> Self {
        // Non-zero bases (FNV-1a's and a mix constant), so leading zeros still move both lanes.
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
        // Bit pattern, not value: the solve is bit-exact, so -0.0 and NaN payloads are distinct.
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

    /// The per-node `u64`; never [`NO_NODE`], or a present node would read as absent.
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

/// The sentinel in [`LayoutScope::last`] and `now` for an id not in that pass's graph.
const NO_NODE: u64 = 0;

/// A frame's per-node hash; the full derive and the incremental pass must agree bit for bit.
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

/// A region's per-node input hash: its anchors, explicit size and measured text extent.
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
    // The art, only where an axis authored 0 takes its span from it (`content_span`): a new
    // texture moves such a region, while a sized region's art cannot move its rect.
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

/// A texture's content-derived span in FrameXML units. `CSimpleTexture::GetWidth` (`0x770720`,
/// height `0x770790`) returns the authored value unless it is exactly 0.0, else the texel extent
/// through the `<AbsDimension>` converter, so one texel is one unit; a `<Color>` texture spans 8.
/// `None` when there is nothing to measure: the axis stays 0, as in the reference with no texture.
pub(super) fn content_span(
    data: &RegionData,
    probe: Option<&crate::script::TextureSizeProbe>,
) -> Option<(f32, f32)> {
    if data.fill.is_some() {
        return Some((SOLID_TEXTURE_TEXELS, SOLID_TEXTURE_TEXELS));
    }
    texel_span(probe, data.texture.as_deref()?)
}

/// [`content_span`] for a path: SimpleHTML reserves an image's height the same way (`0x770790`).
pub(super) fn texel_span(
    probe: Option<&crate::script::TextureSizeProbe>,
    path: &str,
) -> Option<(f32, f32)> {
    let (w, h) = probe?(path)?;
    Some((w as f32, h as f32))
}

/// The edge of the 8×8 surface the colour `SetTexture` makes (`0x770360`, sized by `0x44a900`).
const SOLID_TEXTURE_TEXELS: f32 = 8.0;

/// The floor under a FontString's derived span: `CSimpleFontString::GetWidth` (`0x772930`, height
/// `0x772a60`) returns at least one FrameXML unit, not one line, so empty text reads back 1, not 0.
pub(super) const FONTSTRING_MIN_SPAN: f32 = 1.0;

/// Marks a [`LayoutScope::node_of`] index as one into `regions` rather than `plan`.
const REGION_TAG: u32 = 0x8000_0000;

/// Per-node layout scope, so a resolve touches only what moved. Indexed densely by layout id, as
/// frames and regions share one counter (`Model::next_id`); nothing allocates on a steady frame.
#[derive(Default)]
pub(crate) struct LayoutScope {
    /// Per id: the hash the last converged resolve saw, or [`NO_NODE`]; a mismatch seeds dirty.
    last: Vec<u64>,
    /// How many ids `last` holds a hash for; a pass matching fewer lost a node: full scope.
    last_count: usize,
    /// The screen `last` was hashed under, every chain's root, so a move is full scope.
    last_screen: Option<Rect>,
    /// This pass's hash per id (parallel to `last`); swapped into `last` on convergence.
    now: Vec<u64>,
    /// Per id: index into this pass's node roster, or [`u32::MAX`].
    node_of: Vec<u32>,
    /// Per id: the head of its dependents' list in `dep_to`/`dep_next`, or [`u32::MAX`].
    dep_head: Vec<u32>,
    dep_to: Vec<u32>,
    dep_next: Vec<u32>,
    /// Edge slots [`Self::retarget`] freed, for [`Self::edge`] to reuse rather than grow `dep_to`.
    dep_free: Vec<u32>,
    /// Per id: in the dirty closure.
    dirty: Vec<bool>,
    /// The closure's worklist.
    stack: Vec<u32>,
    /// The cached frame roster, `(handle, id, ScrollFrame anchor override)`, kept across resolves.
    plan: Vec<(FrameHandle, u32, Option<Anchor>)>,
    /// The cached roster of live, anchored regions.
    regions: Vec<RegionRow>,
    /// An incremental pass's `(id, hash)` writes to [`Self::last`], applied only on convergence.
    staged: Vec<(u32, u64)>,
}

/// One live, anchored region as the round loop consumes it, built once per derivation.
#[derive(Clone, Copy)]
struct RegionRow {
    rh: RegionHandle,
    /// The layout id: the solver's array index and the scope's node key.
    id: u32,
    /// The owning frame, which supplies the region's scale and so is a layout dependency.
    owner: FrameHandle,
    is_fontstring: bool,
}

impl LayoutScope {
    /// Size the per-id arrays for `n` ids and clear all but `last`, ahead of a full derivation.
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

    /// The incremental entry: the cached graph survives, and only the dirty marks reset.
    fn begin_incremental(&mut self) {
        self.dirty.clear();
        self.dirty.resize(self.node_of.len(), false);
        self.stack.clear();
        self.staged.clear();
    }

    /// Whether `id` is a node of the cached graph, the only kind a precise touch may name.
    pub(crate) fn has_node(&self, id: u32) -> bool {
        self.node_of
            .get(id as usize)
            .is_some_and(|&n| n != u32::MAX)
    }

    /// Record that node `from` reads `to`'s rect, so a change at `to` reaches `from`.
    #[inline]
    fn edge(&mut self, from: u32, to: u32) {
        let Some(head) = self.dep_head.get_mut(to as usize) else {
            return; // an id with no slot can never turn dirty, so it constrains nothing
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

    /// Re-point node `id`'s outgoing edges after an anchor retarget. `old` and `new` are its full
    /// anchor-target lists, so duplicate anchors stay exact. `false` means a conservative touch:
    /// `id` has no node, or a ScrollFrame override, not its anchors, supplies its edges.
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

    /// Unlink one `to → from` edge, if the list holds one.
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

    /// Close the dirty set under "depends on": whatever reads a dirty rect, transitively.
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

    /// Mark every node dirty, for what a per-node diff cannot see (a death, a screen move).
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

    /// [`Self::commit`] for an incremental pass: only the re-hashed slots. `last_count` and
    /// `last_screen` stay, since a structural change always takes the full path.
    fn commit_incremental(&mut self) {
        for &(id, h) in &self.staged {
            self.last[id as usize] = h;
        }
        self.staged.clear();
    }

    /// Forget the memory, so the next resolve derives the graph in full scope.
    pub(crate) fn invalidate(&mut self) {
        self.last_count = 0;
        self.last_screen = None;
        self.last.clear();
        // The roster and `node_of` too: `has_node` must not vouch for a node the memory forgot.
        self.plan.clear();
        self.regions.clear();
        self.node_of.clear();
        self.staged.clear();
    }
}

/// `WOW_LAYOUT_PROF=1`: the per-solve `[layout-prof]` and `[layout-pre]` lines on stderr.
fn layout_prof_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_LAYOUT_PROF").as_deref() == Ok("1"))
}

/// `WOW_LAYOUT_VERIFY=1`, on in this crate's tests: every shortcut of the resolve (a gate skip, a
/// tier-1-quiet frame, an incremental pass, the measure ledger) is checked against the full answer.
fn layout_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(test) || std::env::var("WOW_LAYOUT_VERIFY").as_deref() == Ok("1"))
}

/// `WOW_LAYOUT_PROF=1`'s `[layout-pre]` line: µs per preamble phase, per resolve past tier 1.
#[derive(Default)]
struct PreambleProf {
    on: bool,
    last: Option<std::time::Instant>,
    /// The GameTooltip auto-size pre-pass (`layout_tooltips`).
    tooltip: u128,
    /// `OnSizeChanged`'s "before" snapshot.
    watched: u128,
    /// The ids rebuild and the scale and clamp sync into `layout_inputs`.
    ids: u128,
    /// The ScrollFrame override map.
    scroll: u128,
    /// `region_resolved`'s liveness retain.
    retain: u128,
    /// `LayoutScope::begin_full`.
    begin: u128,
    /// The fingerprint and scope walk over every live frame.
    fp_frames: u128,
    /// The same over every anchored region.
    fp_regions: u128,
    /// An incremental pass's whole preamble: re-hash the named nodes, seed the moved ones, close.
    seed: u128,
    /// A flag of its own, since `seed` rounds to 0 µs on most incremental passes.
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

    /// Print the line once the gate's verdict is known; `skips=1` returned without solving.
    fn report(&self, skips: bool, frames: usize, regions: usize) {
        if !self.on {
            return;
        }
        let total = self.tooltip
            + self.watched
            + self.ids
            + self.scroll
            + self.retain
            + self.begin
            + self.fp_frames
            + self.fp_regions
            + self.seed;
        eprintln!(
            "[layout-pre] skips={} incr={} frames={frames} anchored={regions} total_us={total} \
             tooltip={} watched={} ids={} scroll={} retain={} begin={} \
             fp_frames={} fp_regions={} seed={}",
            u8::from(skips),
            u8::from(self.incremental),
            self.tooltip,
            self.watched,
            self.ids,
            self.scroll,
            self.retain,
            self.begin,
            self.fp_frames,
            self.fp_regions,
            self.seed
        );
    }
}

/// `OnSizeChanged`'s "after": queue `(id, width, height)` for each watched frame whose size moved
/// by the `ApplyRect 0x76b580` test ([`crate::layout::size_changed`], `ε = _DAT_008029d4`), so a
/// pure move never fires. A frame with no rect at entry compares against the zero rect
/// `CSimpleFrame`'s ctor leaves; one that loses its rect had no `ApplyRect` and fires nothing.
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
    /// [`UiScript::resolve`]'s body on a bare `Model`, so a Lua binding can force a fresh resolve,
    /// as `UpdateScrollChildRect` must report a range reflecting a same-tick `SetHeight`.
    pub(super) fn resolve_layout(model: &mut Model) {
        Self::resolve_layout_inner(model);
        if !model.layout_verify_recheck {
            return;
        }
        // ── `WOW_LAYOUT_VERIFY`: the incremental pass's falsifier ─────────────
        // Re-run the frame from scratch and require identical rects: this catches a write that
        // moved a node without naming it, or changed an anchor's target as if value-only.
        model.layout_verify_recheck = false;
        let before = (model.resolved.clone(), model.region_resolved.clone());
        // The meters keep the incremental pass's values, which tests assert on, not the re-run's.
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
        // No layout touch since the last converged resolve: nothing the fingerprint reads moved,
        // so skip everything. Under verify, fall through so tier 2 checks the claim.
        let verify = layout_verify_enabled();
        let tier1_clean = model.layout_epoch_resolved == Some(model.layout_epoch);
        if tier1_clean && !verify {
            return;
        }
        let epoch_at_entry = model.layout_epoch;
        // Every call past tier 1 counts, verify's re-walks included: the walk is the cost.
        model.layout_gate_walks = model.layout_gate_walks.wrapping_add(1);
        let mut pre = PreambleProf::new(layout_prof_enabled());
        // The GameTooltip pre-pass writes tooltip sizes and offsets from cached measures.
        super::tooltip::layout_tooltips(model);
        pre.tooltip = pre.lap();
        // ── `OnSizeChanged`'s "before" ───────────────────────────────────────────────────────
        // The reference fires it per rect application (`ApplyRect 0x76b580`); our batch fixpoint
        // fires on the entry-vs-convergence diff, never on a half-solved round. Only frames with
        // a handler are snapshotted, from `SetScript`'s list.
        let watched: Vec<(FrameHandle, Option<Rect>)> = model
            .on_size_changed_frames
            .iter()
            .map(|&h| (h, model.resolved.get(&h).copied()))
            .collect();
        pre.watched = pre.lap();
        // ── The graph: the ledger's cache, or a fresh derivation ─────────────────────────────
        // A `Some` ledger claims every write since the graph was built named its node and kept
        // the graph's shape, so the cached roster, edges and hashes stand; `None` derives.
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
        // ── The incremental seed ─────────────────────────────────────────────
        // Re-hash only the named nodes and seed those whose hash moved. It runs before the gate
        // because on this path it is the verdict: an empty seed set means nothing moved.
        let mut scope_nodes = 0usize;
        let mut full_scope = false;
        if let Some(touched) = &touched {
            scope.begin_incremental();
            for &id in touched {
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
                // A named write that did not move the hash (a same-value `SetPoint`) seeds nothing.
                if now != scope.last.get(id as usize).copied().unwrap_or(NO_NODE) {
                    scope.staged.push((id, now));
                    scope.seed(id);
                }
            }
            scope.close();
            pre.seed = pre.lap();
            pre.incremental = true;
        }
        // An incremental pass has no fingerprint; its verdict is `staged`, exactly the seed set.
        let gate_skips = match fp {
            Some(_) => *layout_fingerprint == fp,
            None => scope.staged.is_empty(),
        };
        pre.report(gate_skips, scope.plan.len(), scope.regions.len());
        // Under verify, a frame tier 1 judged quiet must pass tier 2 too; a divergence is a write
        // path missing its `touch_layout()`.
        if tier1_clean && fp.is_some() {
            assert!(
                gate_skips,
                "WOW_LAYOUT_VERIFY: the layout epoch judged this frame quiet but the input \
                 fingerprint moved — a layout write path is missing its touch_layout()"
            );
        }
        // Nothing read moved since the last converged resolve, whose rects stand: close tier 1.
        if gate_skips {
            *layout_epoch_resolved = Some(epoch_at_entry);
            // Re-arm the ledger: the graph is as it was, so the cache is still trustworthy.
            *layout_touched = Some(Vec::new());
            if !verify {
                return;
            }
        }
        // Under verify a skippable resolve runs anyway, against the rects it must reproduce.
        let verify_against = gate_skips.then(|| (resolved.clone(), region_resolved.clone()));
        // Cleared until the fixpoint converges: a cycle-bailed pass leaves its rects mid-flight
        // for the next frame to carry further, so it must re-enter both tiers dirty.
        *layout_fingerprint = None;
        *layout_epoch_resolved = None;
        // Counted on the gate's decision, so verify's forced runs leave it alone.
        if !gate_skips {
            *layout_solves += 1;
        }
        // ── The scope: which nodes this solve may touch ────
        // A node with unchanged inputs and no moved dependency recomputes to the rect it holds, so
        // the solve runs over the dirty closure and feeds other rects in as externals. Full scope
        // when there is no memory yet, the screen moved, a node died (fewer nodes match `last`
        // than `last_count`), or under verify. The incremental pass seeded its closure above.
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
        // A dirty frame is solved, a dirty region swept, and whatever they read is seeded at its
        // cached rect. A dirty region is seeded too, since the frame pass runs before the sweep.
        let mut solve_frames: Vec<u32> = Vec::new();
        let mut sweep_regions: Vec<u32> = Vec::new();
        let mut seed_frames: Vec<u32> = Vec::new();
        let mut seed_regions: Vec<u32> = Vec::new();
        // Whether a node in the closure reads `id`; a rect nothing dirty reads is not seeded.
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
        // This solve's width, beside `layout_solves` (how often) and `layout_rounds` (how deep).
        *layout_last_scope = (solve_frames.len(), sweep_regions.len());
        let round_cap = scope.plan.len() + region_data.len() + 2;
        // `WOW_LAYOUT_PROF=1`: rounds, graph size, and the frame-solve and region-sweep split.
        let prof = layout_prof_enabled();
        let mut t_frames = std::time::Duration::ZERO;
        let mut t_regions = std::time::Duration::ZERO;
        let mut n_regions_swept = 0u64;
        for round in 0..round_cap {
            *layout_rounds += 1;
            let t_round = prof.then(std::time::Instant::now);
            let mut changed = false;

            // Seed the screen root and every read rect this pass does not recompute.
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

            // Second pass: anchored regions, through the same `crate::layout` math as frames. An
            // anchor may target a frame or a sibling region; one whose target has no rect yet pins
            // nothing this round, and the fixpoint re-sweeps until every link settles.
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
                // A row whose data has gone is skipped until the next derivation drops it.
                let Some(data) = region_data.get(&rh) else {
                    continue;
                };
                if prof {
                    n_regions_swept += 1;
                }
                // The owner supplies the region's scale and nothing else: no fallback edges.
                let scale = arena.frame(owner).map(|f| f.effective_scale).unwrap_or(1.0);
                // An axis authored 0, width or height, takes the host-measured extent: the
                // reference sizes a zero width to the unwrapped line. The measure is used without
                // a key check, so changed text keeps its last box while the re-measure is in
                // flight; empty text is never measured, so it counts as zero, never a stale box.
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
                // Then the floor: `CSimpleFontString::GetWidth 0x772930` (height `0x772a60`)
                // returns at least one unit on both legs, past the `jp 0x772957` that skips the
                // measure, so a FontString's span is never 0 and a single anchor resolves it.
                if is_fontstring {
                    width = width.max(FONTSTRING_MIN_SPAN);
                    height = height.max(FONTSTRING_MIN_SPAN);
                }
                // A texture's unset axis takes its texel extent (`content_span`): the reference's
                // size getter is virtual for both region classes. It must apply before the
                // resolve, as `combine_edge` reads a zero span as no size and leaves an edge unset.
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
                    // The screen root is a frame-pass external only: a region anchor to id 0 finds
                    // no rect here, though the solver holds one.
                    (id != SCREEN).then(|| solver.rect(id)).flatten()
                });
                // An axis resolves, or the region does not. The reference resolver core
                // `[0x7671a0, 0x76761f)` reads no parent or owner pointer, only the region's own
                // anchors and `combineEdge`/`combineCenter` over its own size; `assemble 0x767a20`
                // fails on any unset edge, and `0x768d20` latches the region unresolvable. An
                // anchor-less region is anchored at creation (`region::implicit_creation_anchor`),
                // and a texture with no art has a zero span and resolves to nothing (`0x767440`).
                let axis = |lo: Option<f32>, hi: Option<f32>| -> Option<(f32, f32)> {
                    match (lo, hi) {
                        (Some(l), Some(h)) => Some((l, h)),
                        _ => None,
                    }
                };
                let vertical = axis(edges[0], edges[2]);
                let horizontal = axis(edges[1], edges[3]);
                let (Some((bottom, top)), Some((left, right))) = (vertical, horizontal) else {
                    // Unresolvable: drop its rect, so it stops drawing instead of going stale.
                    if region_resolved.remove(&rh).is_some() {
                        changed = true;
                    }
                    continue;
                };
                let rect = Rect::new(bottom, left, top, right);
                // Into the solver too: a later region in this sweep that anchors here needs it.
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
                    // `anchored` is the sweep's roster, `solved` and `swept` this pass's scope.
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
                // Adopted only on convergence; an incremental pass adopts only what it re-hashed.
                if touched.is_some() {
                    scope.commit_incremental();
                } else {
                    scope.commit(scope_nodes, *screen);
                }
                // ── Re-arm the ledger ─────────────────────────────────────────────────────────
                // The graph now describes a converged model, so writes may name nodes against it.
                // Only here: the cycle bail below leaves it `None`.
                *layout_touched = Some(Vec::new());
                // Under verify, `resolve_layout` re-derives this frame from scratch and compares.
                *layout_verify_recheck = touched.is_some() && verify;
                // A converged solve closes tier 1, at `epoch_at_entry` rather than the live epoch,
                // so a touch during this pass (the tooltip pre-pass, a minted id) re-opens it. The
                // fingerprint hashes inputs only, so the next unmutated resolve would match it.
                *layout_epoch_resolved = Some(epoch_at_entry);
                queue_size_changes(&watched, resolved, frame_to_id, pending_size_changed);
                return;
            }
            if round + 1 == round_cap {
                // `Model::record_warning` inlined, as the model is destructured. Verify's re-run
                // truncates `warnings` but not the log, so a verify build may count a row twice.
                let msg = format!(
                    "layout: anchor graph did not converge in {round_cap} rounds — \
                     an anchor cycle? (rects left at their last pass)"
                );
                diagnostics.record(super::diagnostics::DiagnosticKind::Warning, &msg);
                warnings.push(msg);
            }
        }
        // The cycle bail: its rects are this frame's, so their size changes still fire, and the
        // scope forgets a graph that never settled.
        scope.invalidate();
        queue_size_changes(&watched, resolved, frame_to_id, pending_size_changed);
    }

    /// Derive the layout graph from scratch: the roster, the reverse edges, the per-node hashes and
    /// the tier-2 fingerprint. Runs only when the ledger cannot vouch for the cache.
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

        // One id per live frame, its layout input's scale and clamp synced from the arena.
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
            // Clamp-to-screen is a frame property (geometry flags bit 4), so every placement path
            // clamps alike, against the live window size.
            input.clamp = clamp;
            if clamp {
                input.extent_x = screen.right;
                input.extent_y = screen.top;
            }
        }
        pre.ids = pre.lap();

        // A live ScrollFrame's live child is anchored, for this solve only, TOPLEFT to its TOPLEFT
        // at the raw, unnegated `(horizontal, vertical)` offsets, as the reference's re-anchor
        // `0x787100` passes them to `SetPoint`: y up, so a positive vertical lifts the child. Its
        // authored anchors and size are untouched, so `SetScrollChild(nil)` needs no restore.
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

        // The reference resolves one graph in which frames and regions anchor each other; ours
        // alternates a frame solve and a region sweep until a round changes no rect, compared
        // exactly. The round cap is the node count, so only a cycle exhausts it, warned like the
        // reference's resolving-flag bail. Every rect is recomputed from inputs and externals, so
        // last frame's rects are a legal seed: only dead or unanchored regions drop.
        region_resolved.retain(|rh, _| {
            arena.region(*rh).is_some()
                && region_data.get(rh).is_some_and(|d| !d.anchors.is_empty())
        });
        pre.retain = pre.lap();

        // ── The change gate ───────────────────────────────────────────────────────────────────
        // Fingerprint what the rounds read, to skip them when nothing moved since the last
        // converged resolve: the screen; each live frame's id, synced `LayoutInput` and
        // ScrollFrame override; each anchored region's anchors, explicit size, measured extent
        // and liveness. Seeded rects are outputs, a pure function of these, so they are not hashed.
        // The same walk hashes each node alone into `LayoutScope::now` and records its edges.
        // Sized past `next_id` for the ids the region walk below may mint.
        scope.begin_full(*next_id as usize + region_data.len() + 1);
        pre.begin = pre.lap();
        let mut fp = InputFingerprint::default();
        fp.rect(*screen);
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
            // A frame reads its anchor targets' rects, or only its ScrollFrame override's target,
            // as the round loop's `set_frame_anchored` does.
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
        // The region roster, built in the same walk: live, anchored regions only.
        scope.regions.reserve(region_data.len());
        for (&rh, data) in region_data.iter() {
            // Anchor-less entries are not layout inputs: hashing them would make a paint-only
            // setter that creates one (`SetTexture`'s `or_default()`) read as a layout change.
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
                // Mint an id for a region that never needed one, without `Model::region_id`'s
                // epoch bump, which would re-dirty the running resolve.
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
                // A region reads its anchor targets' rects and its owner's scale.
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
            // Liveness only (`owner` and `kind` never change): `WidgetArena::destroy` leaves
            // `region_data` behind, so membership alone would miss a destroyed region.
            fp.feed(u64::from(live.is_some()));
        }
        // The count of anchored entries fed, not the map's len.
        fp.feed(fed_regions);
        pre.fp_regions = pre.lap();
        fp
    }

    /// FontStrings with text whose stored measure is stale, for the host to measure: call after
    /// [`UiScript::resolve`], answer with [`UiScript::set_measured_text`], and resolve again.
    pub fn fontstrings_needing_measure(&mut self) -> Vec<MeasureRequest> {
        let mut model = self.model_mut();
        // Every FontString with text, sized or not: `GetStringWidth` answers the natural width.
        // The ledger (`Model::touch_measure`) re-arms empty, so a second ask this frame sees only
        // the writes since the first; `None` walks every region.
        let ledger = model.measure_dirty.take();
        model.measure_dirty = Some(Vec::new());
        let needy: Vec<RegionHandle> = match ledger {
            Some(mut dirty) => {
                dirty.sort_unstable_by_key(|rh| rh.fingerprint_bits());
                dirty.dedup();
                dirty.retain(|&rh| stale_measure_key(&model, rh).is_some());
                // Under verify, the full walk runs beside the drain and panics on any stale region
                // the ledger missed.
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
        // Re-enroll every stale region: a request is not an answer, and nothing else asks again.
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

    /// Push one [`QuadContent::Backdrop`] per piece of frame `fh`'s backdrop, at the frame slot's
    /// `z`: behind its regions, and background before border by the stable sort.
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
            // The piece's screen bounding box: the render is axis-aligned, so the reference's slant
            // under unequal top and bottom insets is not drawn (stock insets are symmetric).
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

/// The [`MeasureRequest`] region `rh` needs now, if any; shared with the synchronous measure
/// ([`super::measure::ensure_measured`]), as both must build the same cache key.
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

/// `Some((key, scale))` iff `rh` is a FontString with text whose stored measure no longer matches
/// its key: the one staleness test, allocation-free as the sweep asks it of every FontString.
fn stale_measure_key(model: &Model, rh: RegionHandle) -> Option<(u64, f32)> {
    let region = model.arena.region(rh)?;
    if !matches!(region.kind, crate::widget::RegionKind::FontString) {
        return None;
    }
    // The host measures at the drawn size, so the key carries the owner's scale.
    let scale = model
        .arena
        .frame(region.owner)
        .map(|f| f.effective_scale)
        .unwrap_or(1.0);
    let d = model.region_data.get(&rh)?;
    if d.text.as_deref().is_none_or(|t| t.is_empty()) {
        return None;
    }
    // The metric reads in `region.rs` check the stored measure against this same key.
    let key = d.measure_key(scale);
    if d.measured.map(|m| m.key) == Some(key) {
        return None;
    }
    Some((key, scale))
}
