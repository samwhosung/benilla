use std::collections::HashMap;

use crate::layout::{self, Anchor, LayoutInput, Rect};
use crate::widget::{FrameHandle, RegionHandle};

use super::model::{node_deferred, IdMap, HOLD_FORCE, HOLD_HELD, HOLD_QUEUED, HOLD_QUEUED_FORCE};
use super::types::RegionData;
use super::{MeasureRequest, Model, UiScript, SCREEN};

/// Node `id`'s [`Model::layout_hold`] bits, for a caller holding the model destructured.
#[inline]
fn hold_get(hold: &[u8], id: u32) -> u8 {
    hold.get(id as usize).copied().unwrap_or(0)
}

fn hold_set(hold: &mut Vec<u8>, id: u32, bits: u8) {
    let i = id as usize;
    if hold.len() <= i {
        hold.resize(i + 1, 0);
    }
    hold[i] |= bits;
}

fn hold_clear(hold: &mut [u8], id: u32, bits: u8) {
    if let Some(b) = hold.get_mut(id as usize) {
        *b &= !bits;
    }
}

/// Node `id`'s anchors: a frame's layout input's, a region's own.
fn node_anchors(
    layout_inputs: &HashMap<FrameHandle, LayoutInput>,
    region_data: &HashMap<RegionHandle, RegionData>,
    id_to_frame: &IdMap<FrameHandle>,
    id_to_region: &IdMap<RegionHandle>,
    id: u32,
) -> layout::Anchors {
    if let Some(h) = id_to_frame.get(&id) {
        return layout_inputs.get(h).map(|i| i.anchors).unwrap_or_default();
    }
    id_to_region
        .get(&id)
        .and_then(|rh| region_data.get(rh))
        .map(|d| d.anchors)
        .unwrap_or_default()
}

/// A frame's alpha and `effective_scale`, worn by the quads it emits itself.
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

    /// The anchors in point order, then their count.
    #[inline]
    fn anchors<'a>(&mut self, anchors: impl IntoIterator<Item = &'a Anchor>) {
        let mut n = 0u64;
        for a in anchors {
            self.feed(u64::from(a.point.id()));
            self.feed(u64::from(a.relative_to));
            self.feed(u64::from(a.relative_point.id()));
            self.f32(a.x_off);
            self.f32(a.y_off);
            n += 1;
        }
        self.feed(n);
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

// A resolve's per-id marks ([`LayoutScope::marks`]).
/// Invalidated: solved from its inputs (`0x7680e0`).
const MARK_INVALID: u8 = 1;
/// Its resolved bit cleared: stored without the keep test.
const MARK_FORCED: u8 = 2;
/// Undeferred for this resolve: a show, a load's end, a read or an anchor read pulled it.
const MARK_PULLED: u8 = 4;
/// Its invalidate passes to what anchors to it (`0x768178`).
const MARK_PASSING: u8 = 8;
/// Resolved for the show that started this resolve ([`Model::layout_chain`]).
const MARK_CHAIN: u8 = 16;
/// Collected as a seed already.
const MARK_SEEN: u8 = 32;

/// A frame's per-node hash; the full derive and the incremental pass must agree bit for bit.
#[inline]
fn frame_node_hash(input: &LayoutInput) -> u64 {
    let mut node = InputFingerprint::default();
    node.input(input);
    node.finish()
}

/// The measured extent the region sweep sizes a FontString by: the stored measure while the text
/// is non-empty, used without a key check, so changed text keeps its last box while the
/// re-measure is in flight; empty text is never measured, so it has none, never a stale box.
#[inline]
pub(super) fn effective_measure(data: &RegionData) -> Option<super::MeasuredText> {
    data.measured
        .filter(|_| data.text.as_deref().is_some_and(|t| !t.is_empty()))
}

/// A region's per-node input hash: its anchors, explicit size, owner, whose effective scale the
/// sweep reads (`None` for an owner with no layout id), and, where an axis authored 0 takes its
/// span from it, its effective measure. Not a Texture's art, which only a resolve that reaches the
/// node reads, so art set since moves no node, as `SetTexture` invalidates none (`0x770200`).
#[inline]
pub(super) fn region_node_hash(data: &RegionData, owner: Option<u32>) -> u64 {
    let mut node = InputFingerprint::default();
    node.anchors(&data.anchors);
    node.feed(owner.map_or(u64::MAX, u64::from));
    match data.size {
        Some((w, h)) => {
            node.f32(w);
            node.f32(h);
        }
        None => node.feed(u64::MAX),
    }
    let unsized_axis = data.size.is_none_or(|(w, h)| w == 0.0 || h == 0.0);
    match effective_measure(data).filter(|_| unsized_axis) {
        Some(m) => {
            node.f32(m.w);
            node.f32(m.h);
        }
        None => node.feed(u64::MAX),
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
    if data.tex.fill().is_some() {
        return Some((SOLID_TEXTURE_TEXELS, SOLID_TEXTURE_TEXELS));
    }
    texel_span(probe, data.tex.path().as_deref()?)
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

/// A font string's size slot (`0x772930`, height `0x772a60`) in layout units: its stored size, or
/// when that is 0 its text's extent (`0x772890`), raised to `S(1)`, at 53 bits, when not above it or
/// a NaN (`0x772971`, `0x77297a`), so a single anchor always fixes the opposite edge.
fn fontstring_slot(units: layout::LayoutUnits, authored: f32, measure: Option<f32>) -> f64 {
    let stored = units.store(authored);
    let candidate = if stored == 0.0 {
        measure.map_or(0.0, |m| f64::from(units.store(m)))
    } else {
        f64::from(stored)
    };
    let floor = units.store_wide(FONTSTRING_MIN_SPAN);
    if floor >= candidate || candidate.is_nan() {
        floor
    } else {
        candidate
    }
}

/// A texture's size slot (`0x770720`, height `0x770790`) in layout units: its stored size `stored`,
/// or when that is 0 and it has art, the art's texel extent through the setter steps, returned at
/// 53 bits (`0x77074e`); 0 with no art.
fn texture_slot(units: layout::LayoutUnits, stored: f32, content: Option<f32>) -> f64 {
    if stored == 0.0 {
        content.map_or(0.0, |t| units.store_wide(t))
    } else {
        f64::from(stored)
    }
}

/// Marks a [`LayoutScope::node_of`] index as one into `regions` rather than `plan`.
const REGION_TAG: u32 = 0x8000_0000;

/// Per-node layout scope, so a resolve touches only what moved. Indexed densely by layout id, as
/// frames and regions share one counter (`Model::next_id`); nothing allocates on a steady frame.
///
/// The graph is derived in full once, then kept: a write names the ids it touched
/// ([`Model::touch_layout_frame`]), and the next resolve re-derives just those nodes against the
/// model ([`UiScript::rederive_named`]), joining, leaving or re-pointing them in place, the way a
/// retained layout tree marks a node dirty rather than rebuilding itself.
#[derive(Clone, Default)]
pub(crate) struct LayoutScope {
    /// Per id: the hash the last converged resolve saw, or [`NO_NODE`]; a mismatch seeds dirty.
    last: Vec<u64>,
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
    /// Edge slots [`Self::unlink_edge`] freed, for [`Self::edge`] to reuse rather than grow `dep_to`.
    dep_free: Vec<u32>,
    /// Per id: the targets of the edges it recorded, in order, so a re-derive unlinks exactly those.
    out: Vec<Vec<u32>>,
    /// Per id: in the dirty closure.
    dirty: Vec<bool>,
    /// The dirty ids in the order they were marked.
    dirty_list: Vec<u32>,
    /// The closure's worklist.
    stack: Vec<u32>,
    /// Per id, this resolve's `MARK_*` bits, cleared through `marked`.
    marks: Vec<u8>,
    marked: Vec<u32>,
    /// The cached frame roster, `(handle, id)`, kept across resolves.
    plan: Vec<(FrameHandle, u32)>,
    /// The cached roster of live, anchored regions.
    regions: Vec<RegionRow>,
    /// An incremental pass's `(id, hash)` writes to [`Self::last`], applied only on convergence.
    staged: Vec<(u32, u64)>,
    /// A re-derive's target list, reused.
    targets: Vec<u32>,
}

/// One live, anchored region as the round loop consumes it, a row of the cached roster.
#[derive(Clone, Copy, PartialEq)]
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
        // `last` only grows: a dead node past the new count keeps its hash, so the scope still
        // seeds what anchored to it.
        if self.last.len() < n {
            self.last.resize(n, NO_NODE);
        }
        self.now.clear();
        self.now.resize(n, NO_NODE);
        self.node_of.clear();
        self.node_of.resize(n, u32::MAX);
        self.dep_head.clear();
        self.dep_head.resize(n, u32::MAX);
        // Cleared in place, so a re-derivation reuses each list's allocation.
        self.out.iter_mut().for_each(Vec::clear);
        self.out.resize_with(n, Vec::new);
        self.dirty.clear();
        self.dirty.resize(n, false);
        self.dirty_list.clear();
        self.dep_to.clear();
        self.dep_next.clear();
        self.dep_free.clear();
        self.stack.clear();
        self.plan.clear();
        self.regions.clear();
        self.staged.clear();
    }

    /// The incremental entry: the cached graph survives, grown to `n` ids, and only the dirty
    /// marks reset.
    fn begin_incremental(&mut self, n: usize) {
        if self.node_of.len() < n {
            self.node_of.resize(n, u32::MAX);
            self.dep_head.resize(n, u32::MAX);
            self.out.resize_with(n, Vec::new);
        }
        if self.last.len() < self.node_of.len() {
            self.last.resize(self.node_of.len(), NO_NODE);
        }
        self.dirty.clear();
        self.dirty.resize(self.node_of.len(), false);
        self.dirty_list.clear();
        self.stack.clear();
        self.staged.clear();
    }

    /// Clear the last resolve's marks and size them for `n` ids.
    fn begin_marks(&mut self, n: usize) {
        for &id in &self.marked {
            self.marks[id as usize] = 0;
        }
        self.marked.clear();
        if self.marks.len() < n {
            self.marks.resize(n, 0);
        }
    }

    /// Add `bits` to id `id`'s marks.
    #[inline]
    fn mark(&mut self, id: u32, bits: u8) {
        let i = id as usize;
        if self.marks.len() <= i {
            self.marks.resize(i + 1, 0);
        }
        if self.marks[i] == 0 {
            self.marked.push(id);
        }
        self.marks[i] |= bits;
    }

    #[inline]
    fn has(&self, id: u32, bits: u8) -> bool {
        self.marks.get(id as usize).is_some_and(|m| m & bits != 0)
    }

    #[inline]
    fn is_dirty(&self, id: u32) -> bool {
        self.dirty.get(id as usize).copied().unwrap_or(false)
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

    /// [`Self::edge`], remembered as one of `from`'s own, for a later [`Self::relink`].
    #[inline]
    fn link(&mut self, from: u32, to: u32) {
        self.edge(from, to);
        if let Some(out) = self.out.get_mut(from as usize) {
            out.push(to);
        }
    }

    /// Point node `id`'s edges at `targets`, unlinking the ones it recorded; `true` when they
    /// differed, which the caller seeds, as the node now reads other rects.
    fn relink(&mut self, id: u32, targets: &[u32]) -> bool {
        let Some(out) = self.out.get_mut(id as usize) else {
            return false;
        };
        if out.as_slice() == targets {
            return false;
        }
        let old = std::mem::take(out);
        for &t in &old {
            self.unlink_edge(t, id);
        }
        let mut old = old;
        old.clear();
        old.extend_from_slice(targets);
        self.out[id as usize] = old;
        for &t in targets {
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
                self.dirty_list.push(id);
            }
        }
    }

    /// A re-derived node's verdict: stage and seed it when its hash or its edges moved. Every edge
    /// source (anchor targets, the owner, the override) is in the hash, so `relinked` only guards
    /// against a hash that stops covering one. A node `held` under its deferral is staged but not
    /// seeded: its resolve waits, so its rect stays and nothing reading it moves.
    #[inline]
    fn note(&mut self, id: u32, hash: u64, relinked: bool, held: bool) {
        if relinked || hash != self.last.get(id as usize).copied().unwrap_or(NO_NODE) {
            self.staged.push((id, hash));
            if !held {
                self.seed(id);
            }
        }
    }

    /// The nodes this graph records reading `id`'s rect. It describes the model while the ledger
    /// is armed (`Model::layout_touched` is `Some`), but for the nodes named since.
    pub(crate) fn readers_of(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let mut e = self.dep_head.get(id as usize).copied().unwrap_or(u32::MAX);
        std::iter::from_fn(move || {
            if e == u32::MAX {
                return None;
            }
            let from = self.dep_to[e as usize];
            e = self.dep_next[e as usize];
            Some(from)
        })
    }

    /// Whether `id` is a node whose converged hash is `hash`: a write that leaves it there moved
    /// nothing the resolve reads.
    pub(crate) fn holds(&self, id: u32, hash: u64) -> bool {
        self.node_of
            .get(id as usize)
            .is_some_and(|&n| n != u32::MAX)
            && self.last.get(id as usize) == Some(&hash)
    }

    /// Drop region node `id` from the roster with its edges, seeding it so the closure revisits
    /// what read its rect; nothing invalidates those, so they keep their state. Returns the row
    /// it held.
    fn remove_region(&mut self, id: u32, n: u32) -> RegionRow {
        let idx = (n & !REGION_TAG) as usize;
        let row = self.regions.swap_remove(idx);
        if let Some(moved) = self.regions.get(idx) {
            #[allow(clippy::cast_possible_truncation)]
            {
                self.node_of[moved.id as usize] = idx as u32 | REGION_TAG;
            }
        }
        self.node_of[id as usize] = u32::MAX;
        self.relink(id, &[]);
        self.staged.push((id, NO_NODE));
        self.seed(id);
        row
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

    /// Mark every node dirty, for what a per-node diff cannot see (no memory, a screen move).
    fn dirty_all(&mut self) {
        for (id, &n) in self.node_of.iter().enumerate() {
            if n != u32::MAX && !self.dirty[id] {
                self.dirty[id] = true;
                #[allow(clippy::cast_possible_truncation)]
                self.dirty_list.push(id as u32);
            }
        }
    }

    /// Adopt this pass's hashes as the converged memory.
    fn commit(&mut self, screen: Rect) {
        std::mem::swap(&mut self.last, &mut self.now);
        self.last_screen = Some(screen);
    }

    /// [`Self::commit`] for an incremental pass: only the re-hashed slots. `last_screen` stays,
    /// since a screen move always takes the derivation.
    fn commit_incremental(&mut self) {
        for &(id, h) in &self.staged {
            self.last[id as usize] = h;
        }
        self.staged.clear();
    }

    /// Forget the memory, so the next resolve derives the graph in full scope.
    pub(crate) fn invalidate(&mut self) {
        self.last_screen = None;
        self.last.clear();
        self.plan.clear();
        self.regions.clear();
        self.node_of.clear();
        self.staged.clear();
    }

    /// `WOW_LAYOUT_VERIFY`'s graph check: where this cached graph differs from `fresh`, derived
    /// from scratch on the same model, in roster, per-node hash, edges or ScrollFrames.
    fn diverges_from(&self, fresh: &LayoutScope) -> Option<(u32, String)> {
        let at = |v: &[u32], id: usize| v.get(id).copied().unwrap_or(u32::MAX);
        let hash = |s: &LayoutScope, id: usize| s.last.get(id).copied().unwrap_or(NO_NODE);
        let sorted = |v: Option<&Vec<u32>>| {
            let mut v = v.cloned().unwrap_or_default();
            v.sort_unstable();
            v
        };
        let incoming = |s: &LayoutScope, id: usize| {
            let mut v = Vec::new();
            let mut e = at(&s.dep_head, id);
            while e != u32::MAX {
                v.push(s.dep_to[e as usize]);
                e = s.dep_next[e as usize];
            }
            v.sort_unstable();
            v
        };
        let n = self
            .node_of
            .len()
            .max(fresh.node_of.len())
            .max(self.last.len())
            .max(fresh.last.len());
        for id in 0..n {
            let (a, b) = (at(&self.node_of, id), at(&fresh.node_of, id));
            let row = |s: &LayoutScope, n: u32| -> String {
                match n {
                    u32::MAX => "absent".into(),
                    n if n & REGION_TAG != 0 => {
                        let r = s.regions[(n & !REGION_TAG) as usize];
                        format!("region {:?} owner {:?}", r.rh, r.owner)
                    }
                    n => format!("frame {:?}", s.plan[n as usize].0),
                }
            };
            let same_node = match (a, b) {
                (u32::MAX, u32::MAX) => true,
                (u32::MAX, _) | (_, u32::MAX) => false,
                (a, b) if (a ^ b) & REGION_TAG != 0 => false,
                (a, b) if a & REGION_TAG != 0 => {
                    self.regions[(a & !REGION_TAG) as usize]
                        == fresh.regions[(b & !REGION_TAG) as usize]
                }
                (a, b) => self.plan[a as usize] == fresh.plan[b as usize],
            };
            if !same_node {
                return Some((
                    id as u32,
                    format!(
                        "id {id}: the cached graph holds {}, a fresh derivation {}",
                        row(self, a),
                        row(fresh, b)
                    ),
                ));
            }
            if hash(self, id) != hash(fresh, id) {
                return Some((
                    id as u32,
                    format!(
                        "id {id} ({}): the cached hash is stale against a fresh derivation",
                        row(self, a)
                    ),
                ));
            }
            let (oa, ob) = (sorted(self.out.get(id)), sorted(fresh.out.get(id)));
            if a != u32::MAX && oa != ob {
                return Some((
                    id as u32,
                    format!(
                        "id {id} ({}): reads {oa:?} in the cached graph, {ob:?} fresh",
                        row(self, a)
                    ),
                ));
            }
            let (ia, ib) = (incoming(self, id), incoming(fresh, id));
            if ia != ib {
                return Some((
                    id as u32,
                    format!("id {id}: read by {ia:?} in the cached graph, {ib:?} fresh"),
                ));
            }
        }
        None
    }
}

/// Every frame's and region's deferral bit, for `WOW_LAYOUT_VERIFY`'s re-run to hold against.
fn deferral_state(model: &Model) -> (Vec<FrameHandle>, Vec<RegionHandle>) {
    // In arena order, which the re-run does not change.
    let frames: Vec<FrameHandle> = model
        .arena
        .iter_frames()
        .filter(|(_, f)| f.layout_deferred)
        .map(|(h, _)| h)
        .collect();
    let mut regions: Vec<RegionHandle> = model
        .region_to_id
        .keys()
        .copied()
        .filter(|&rh| model.arena.region(rh).is_some_and(|r| r.layout_deferred))
        .collect();
    regions.sort_unstable_by_key(|rh| rh.fingerprint_bits());
    (frames, regions)
}

/// The layout id of the first frame or region whose rect differs between the model and `frames`
/// and `regions`, in id order.
fn first_rect_diff(
    model: &Model,
    frames: &HashMap<FrameHandle, Rect>,
    regions: &HashMap<RegionHandle, Rect>,
) -> Option<u32> {
    let mut ids: Vec<u32> = Vec::new();
    for (h, r) in &model.frame_rects {
        if frames.get(h) != Some(r) {
            ids.extend(model.frame_to_id.get(h));
        }
    }
    for h in frames.keys().filter(|h| !model.frame_rects.contains_key(h)) {
        ids.extend(model.frame_to_id.get(h));
    }
    for (h, r) in &model.region_rects {
        if regions.get(h) != Some(r) {
            ids.extend(model.region_to_id.get(h));
        }
    }
    for h in regions
        .keys()
        .filter(|h| !model.region_rects.contains_key(h))
    {
        ids.extend(model.region_to_id.get(h));
    }
    let differs = frames != &model.frame_rects || regions != &model.region_rects;
    ids.into_iter().min().or(differs.then_some(u32::MAX))
}

/// A node's layout inputs as the model holds them, for `WOW_LAYOUT_VERIFY`'s graph report.
fn node_inputs(model: &Model, id: u32) -> String {
    if let Some(h) = model.id_to_frame.get(&id) {
        let f = model.arena.frame(*h);
        return format!(
            "frame {:?} {:?}, effective scale {:?}, clamped {:?}, unsettled {}",
            f.and_then(|f| f.name.clone()),
            model.layout_inputs.get(h),
            f.map(|f| f.effective_scale),
            f.map(|f| f.clamped_to_screen),
            model.layout_unsettled.contains(&id),
        );
    }
    let Some(rh) = model.id_to_region.get(&id) else {
        return "no live frame or region".into();
    };
    let owner = model.arena.region(*rh).map(|r| r.owner);
    let owner_id = owner.and_then(|o| model.frame_to_id.get(&o));
    match model.region_data.get(rh) {
        Some(d) => format!(
            "region owner {owner:?} (id {owner_id:?}), anchors {:?}, size {:?}, measured {:?} \
             (text {} bytes), texture {:?}, fill {}, unsettled {}",
            d.anchors,
            d.size,
            d.measured.map(|m| (m.w, m.h)),
            d.text.as_deref().map_or(0, str::len),
            d.tex.path(),
            d.tex.fill().is_some(),
            model.layout_unsettled.contains(&id),
        ),
        None => format!("region owner {owner:?} with no data"),
    }
}

/// `WOW_LAYOUT_PROF=1`: the per-solve `[layout-prof]` and `[layout-pre]` lines on stderr.
fn layout_prof_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_LAYOUT_PROF").as_deref() == Ok("1"))
}

/// `WOW_LAYOUT_VERIFY=1`, on in this crate's tests: every shortcut of the resolve (a gate skip, a
/// tier-1-quiet frame, an incremental pass, the measure ledger) is checked against the full answer.
pub(super) fn layout_verify_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| cfg!(test) || std::env::var("WOW_LAYOUT_VERIFY").as_deref() == Ok("1"))
}

/// `WOW_LAYOUT_PROF=1`'s `[layout-pre]` line: µs per preamble phase, per resolve past tier 1.
#[derive(Default)]
struct PreambleProf {
    on: bool,
    last: Option<std::time::Instant>,
    /// The GameTooltip cursor pre-pass (`anchor_cursor_tooltips`).
    tooltip: u128,
    /// `OnSizeChanged`'s "before" snapshot.
    watched: u128,
    /// The ids rebuild and the scale and clamp sync into `layout_inputs`.
    ids: u128,
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
            + self.retain
            + self.begin
            + self.fp_frames
            + self.fp_regions
            + self.seed;
        eprintln!(
            "[layout-pre] skips={} incr={} frames={frames} anchored={regions} total_us={total} \
             tooltip={} watched={} ids={} retain={} begin={} \
             fp_frames={} fp_regions={} seed={}",
            u8::from(skips),
            u8::from(self.incremental),
            self.tooltip,
            self.watched,
            self.ids,
            self.retain,
            self.begin,
            self.fp_frames,
            self.fp_regions,
            self.seed
        );
    }
}

/// `OnSizeChanged`'s "after": queue `(id, width, height)` for each watched frame whose size moved
/// by the `ApplyRect 0x76b580` test ([`crate::layout::size_changed`] at [`layout::SIZE_EPS`] on the
/// layout rects), so a pure move never fires. The size is the layout rect's own, `right − left` and
/// `top − bottom` rounded to f32 (`0x76b623`–`0x76b63e`), with no getter steps and no scale
/// divide, which slot `+0x48` (`0x76b660`) pushes as doubles under `"%f%f"` (`0x878830`). A frame
/// with no rect at entry compares against the zero rect `CSimpleFrame`'s ctor leaves; one that
/// loses its rect had no `ApplyRect` and fires nothing.
fn queue_size_changes(
    watched: &[(FrameHandle, Option<Rect>)],
    resolved: &HashMap<FrameHandle, Rect>,
    frame_to_id: &HashMap<FrameHandle, u32>,
    eps: f64,
    out: &mut Vec<(u32, f32, f32)>,
) {
    for &(h, before) in watched {
        let Some(&now) = resolved.get(&h) else {
            continue;
        };
        let before = before.unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        if !layout::size_changed(before, now, eps) {
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
    /// as a scroll frame's range recompute must reflect a same-tick `SetHeight`.
    pub(super) fn resolve_layout(model: &mut Model) {
        // A tooltip sized before its lines were measured, in a VM with no font engine.
        super::tooltip::size_owed(model);
        let solves = model.layout_solves;
        Self::resolve_layout_inner(model);
        // A solve may have resized a frame: its backdrop re-runs its texcoords (`0x76b580`).
        if model.layout_solves != solves {
            super::backdrop::after_resolve(model);
            super::messageframe::after_resolve(model);
        }
        if !model.layout_verify_recheck {
            return;
        }
        // ── `WOW_LAYOUT_VERIFY`: the incremental pass's falsifier ─────────────
        model.layout_verify_recheck = false;
        if let Some(why) = Self::scratch_divergence(model) {
            panic!("WOW_LAYOUT_VERIFY: {why}");
        }
    }

    /// Re-solve the model from scratch on a fresh graph and compare it with the converged cached
    /// one: the rects, then the graph itself (roster, per-node hashes, edges). This catches a write
    /// that moved a node without naming it, and a re-derive that joined, dropped or re-pointed one
    /// wrong. The cached graph is put back after, so a run of incremental passes is checked step
    /// by step, not reset at each; the meters keep the incremental pass's values.
    pub(super) fn scratch_divergence(model: &mut Model) -> Option<String> {
        let cached = std::mem::take(&mut model.layout_scope);
        let before = (model.frame_rects.clone(), model.region_rects.clone());
        let kept_before = model.region_kept_rects.clone();
        let meters = (
            model.layout_solves,
            model.layout_rounds,
            model.layout_last_scope,
            model.layout_gate_walks,
            model.layout_derives,
        );
        let (queued, warned) = (model.pending_size_changed.len(), model.warnings.len());
        // The deferral state the pass left, which a re-run of its converged model must keep: it
        // reaches nothing, so it holds and pulls nothing.
        let chain = model.layout_chain.take();
        let deferral_before = (deferral_state(model), model.layout_hold.clone());
        model.layout_touched = None; // no ledger: derive the graph
        model.layout_epoch_resolved = None; // past tier 1
        model.layout_fingerprint = None; // past tier 2
        Self::resolve_layout_inner(model); // a fresh scope: full scope
        let graph = cached.diverges_from(&model.layout_scope);
        model.layout_scope = cached;
        model.layout_chain = chain;
        let deferral_after = (deferral_state(model), model.layout_hold.clone());
        model.pending_size_changed.truncate(queued);
        model.warnings.truncate(warned);
        (
            model.layout_solves,
            model.layout_rounds,
            model.layout_last_scope,
            model.layout_gate_walks,
            model.layout_derives,
        ) = meters;
        // The first node whose rect differs, with its inputs; the graph's divergence, if any,
        // usually names the cause.
        let report = |id: u32| format!("id {id}, inputs now: {}", node_inputs(model, id));
        let graph = graph.map(|(id, why)| format!("{why} ({})", report(id)));
        if let Some(id) = first_rect_diff(model, &before.0, &before.1) {
            let rect = |frames: &HashMap<FrameHandle, Rect>,
                        regions: &HashMap<RegionHandle, Rect>| {
                match (model.id_to_frame.get(&id), model.id_to_region.get(&id)) {
                    (Some(h), _) => frames.get(h).copied(),
                    (_, Some(rh)) => regions.get(rh).copied(),
                    _ => None,
                }
            };
            return Some(format!(
                "an INCREMENTAL layout pass disagreed with a full derivation of the same frame — \
                 a write site named the wrong node or moved layout inputs without naming any. \
                 First rect to differ: {}, incremental {:?}, fresh {:?}. Graph: {}",
                report(id),
                rect(&before.0, &before.1),
                rect(&model.frame_rects, &model.region_rects),
                graph.as_deref().unwrap_or("same as fresh"),
            ));
        }
        if deferral_before != deferral_after {
            return Some(format!(
                "an INCREMENTAL layout pass left deferral state a full derivation of the same \
                 model moved: a node it should have held or pulled. Graph: {}",
                graph.as_deref().unwrap_or("same as fresh"),
            ));
        }
        if kept_before != model.region_kept_rects {
            return Some(format!(
                "an INCREMENTAL layout pass kept failed textures' quads {kept_before:?}, a full \
                 derivation {:?}. Graph: {}",
                model.region_kept_rects,
                graph.as_deref().unwrap_or("same as fresh"),
            ));
        }
        graph.map(|why| {
            format!(
                "the cached layout graph diverged from a fresh derivation after an INCREMENTAL \
                 pass — a write changed a node's roster, edges or inputs without naming it, or \
                 the re-derive mis-applied one. {why}"
            )
        })
    }

    fn resolve_layout_inner(model: &mut Model) {
        // Text that emptied or filled since the last resolve: named now where its node's hash
        // moved, ahead of the epoch's read, as if named at the write.
        if !model.content_writes.is_empty() {
            for rh in std::mem::take(&mut model.content_writes) {
                model.touch_layout_content(rh);
            }
        }
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
        // The GameTooltip pre-pass places each cursor-anchored plate.
        super::tooltip::anchor_cursor_tooltips(model);
        pre.tooltip = pre.lap();
        // The invalidations since the last resolve, the pre-pass's included, each answered by this
        // one, as the reference drains its whole queue (`0x768ed0`).
        let pending = std::mem::take(&mut model.layout_pending);
        // ── `OnSizeChanged`'s "before" ───────────────────────────────────────────────────────
        // The reference fires it per rect application (`ApplyRect 0x76b580`); our batch fixpoint
        // fires on the entry-vs-convergence diff, never on a half-solved round. Only frames with
        // a handler are snapshotted, from `SetScript`'s list.
        let watched: Vec<(FrameHandle, Option<Rect>)> = model
            .on_size_changed_frames
            .iter()
            .map(|&h| (h, model.frame_rects.get(&h).copied()))
            .collect();
        // Both thresholds in the layout units the rects are in (`0x8029d4`, `0x80c5c8`).
        let size_eps = layout::SIZE_EPS;
        let resolve_eps = layout::RESOLVE_EPS;
        let space = model.layout_space();
        pre.watched = pre.lap();
        // The graph's own clears of a resolved bit (a moved effective scale) are writes made now.
        let forced_mark = model.layout_force.len();
        // ── The graph: the ledger's cache, or a fresh derivation ─────────────────────────────
        // A `Some` ledger claims every write since the graph was built named its node and kept
        // the graph's shape, so the cached roster, edges and hashes stand; `None` derives.
        // The named nodes re-derive in place; a change the cached graph cannot take derives.
        let incremental = match model.layout_touched.take() {
            Some(named) => Self::rederive_named(model, named, &mut pre),
            None => false,
        };
        let fp = (!incremental).then(|| Self::derive_layout_graph(model, &mut pre));

        let Model {
            arena,
            layout_inputs,
            frame_rects: resolved,
            region_data,
            region_rects: region_resolved,
            region_kept_rects: region_kept,
            frame_to_id,
            id_to_frame,
            id_to_region,
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
            layout_force,
            layout_screen,
            layout_unsettled,
            layout_hold,
            layout_pass,
            layout_pull,
            layout_chain,
            next_id,
            region_to_id,
            ..
        } = model;
        let mut full_scope = false;
        // The screen root in layout units, `K1` by `K2` at the interface's root (`0x764284`).
        let screen = &space.root();
        // The nodes this resolve stores without the keep test; a write that cleared the resolved
        // bit of no node (an unanchored region's measure) leaves nothing to store.
        let mut forced = std::mem::take(layout_force);
        let mut pending = pending;

        // ── Deferral (`0x2` of `G+0x3c`, `0x768060`) ─────────────────────────────────────────
        // A deferred node's resolve waits, pending, until its show, a read, or an anchor read of
        // a node that resolves pulls it; the pulled ones are this resolve's to solve.
        scope.begin_marks((*next_id as usize).max(scope.node_of.len()));
        let mut passing = std::mem::take(layout_pass);
        for id in std::mem::take(layout_pull) {
            scope.mark(id, MARK_PULLED);
            let f = hold_get(layout_hold, id);
            if f & HOLD_HELD != 0 {
                if f & HOLD_FORCE != 0 {
                    forced.push(id);
                } else {
                    pending.push(id);
                }
                hold_clear(layout_hold, id, HOLD_HELD | HOLD_FORCE);
            }
        }
        // Queued and deferred since: off the queue, pending (`0x768074`–`0x7680b3`). A resolved bit
        // the derivation cleared is a write now, so a deferred one passes it on.
        pending.retain(|&id| {
            hold_clear(layout_hold, id, HOLD_QUEUED);
            if scope.has(id, MARK_PULLED) || !node_deferred(arena, id_to_frame, id_to_region, id) {
                return true;
            }
            hold_set(layout_hold, id, HOLD_HELD);
            false
        });
        // A moved effective scale reaches each of the frame's regions itself (`0x76ad06`), which
        // clears its resolved bit and invalidates it under its own deferral, whatever the frame's.
        let scaled: Vec<u32> = forced[forced_mark.min(forced.len())..]
            .iter()
            .copied()
            .filter(|id| id_to_frame.contains_key(id))
            .collect();
        for &id in &scaled {
            let Some(f) = id_to_frame.get(&id).and_then(|&h| arena.frame(h)) else {
                continue;
            };
            for rh in &f.regions {
                if let Some(&rid) = region_to_id.get(rh) {
                    forced.push(rid);
                }
            }
        }
        let mut at = 0usize;
        forced.retain(|&id| {
            let derived = at >= forced_mark;
            at += 1;
            hold_clear(layout_hold, id, HOLD_QUEUED_FORCE);
            if scope.has(id, MARK_PULLED) || !node_deferred(arena, id_to_frame, id_to_region, id) {
                return true;
            }
            if derived && hold_get(layout_hold, id) & HOLD_HELD == 0 {
                passing.push(id);
            }
            hold_set(layout_hold, id, HOLD_HELD | HOLD_FORCE);
            false
        });
        passing.sort_unstable();
        passing.dedup();
        for &id in &passing {
            scope.mark(id, MARK_PASSING);
        }
        // A show's resolve: its roots, and what their reads pull, in `ApplyRect` order.
        let mut chain = layout_chain.take();
        if let Some(roots) = &chain {
            for &id in roots {
                scope.mark(id, MARK_CHAIN);
            }
        }

        // Each is reached now, so it holds what its inputs give again.
        for id in forced.iter().chain(&pending) {
            layout_unsettled.remove(id);
        }
        // An invalidated region with no anchors is no node, and its resolve can only fail
        // (`0x768d55`): its valid bit clears, a Texture keeps its last quad. So does one whose
        // owner's scale moved, which the cascade invalidates (`0x76ad06`).
        let fail_loose = |rh: RegionHandle,
                          arena: &crate::widget::WidgetArena,
                          region_resolved: &mut HashMap<RegionHandle, Rect>,
                          region_kept: &mut HashMap<RegionHandle, Rect>| {
            let Some(r) = arena.region(rh) else { return };
            if region_data.get(&rh).is_some_and(|d| !d.anchors.is_empty()) {
                return;
            }
            if let Some(rect) = region_resolved.remove(&rh) {
                if !matches!(r.kind, crate::widget::RegionKind::FontString) {
                    region_kept.insert(rh, rect);
                }
            }
        };
        for &id in &pending {
            if let Some(&rh) = id_to_region.get(&id) {
                fail_loose(rh, arena, region_resolved, region_kept);
            }
        }
        for &id in &forced {
            if let Some(&rh) = id_to_region.get(&id) {
                fail_loose(rh, arena, region_resolved, region_kept);
            }
        }
        for &id in &scaled {
            if let Some(f) = id_to_frame.get(&id).and_then(|&h| arena.frame(h)) {
                // Whatever each region's deferral (`0x7683a2`–`0x7683c3`).
                for &rh in &f.regions {
                    fail_loose(rh, arena, region_resolved, region_kept);
                }
            }
        }
        forced.retain(|&id| {
            scope
                .node_of
                .get(id as usize)
                .is_some_and(|&n| n != u32::MAX)
        });
        pending.retain(|&id| {
            scope
                .node_of
                .get(id as usize)
                .is_some_and(|&n| n != u32::MAX)
        });
        // An incremental pass has no fingerprint; its verdict is `staged`, exactly the seed set.
        // A forced or invalidated node is solved even with its inputs unmoved, and an invalidate
        // passed on reaches what anchors to its node.
        let gate_skips = forced.is_empty()
            && pending.is_empty()
            && passing.is_empty()
            && match fp {
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
            *layout_chain = chain.clone();
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
        // A node with unchanged inputs and no moved dependency recomputes to the rect it holds (the
        // keep test holds a recompute against the rect it kept, so a re-run is a no-op), so
        // the solve runs over the dirty closure and feeds other rects in as externals. Full scope
        // when there is no memory yet, the screen moved, or under verify. An incremental pass
        // seeded its closure in `rederive_named`.
        if !incremental {
            // A node that died past the new id count has no edges recorded (the derivation sized
            // them to the count), so only a full scope reaches what anchored to it.
            let died_past_end = scope
                .last
                .iter()
                .skip(scope.node_of.len())
                .any(|&h| h != NO_NODE);
            full_scope = scope.last.is_empty()
                || scope.last_screen != Some(*screen)
                || died_past_end
                || gate_skips; // the verify path's re-run: it must reproduce the whole graph
            if full_scope {
                scope.dirty_all();
            } else {
                // A node held under its deferral waits whatever moved its inputs.
                for id in 0..scope.node_of.len() {
                    #[allow(clippy::cast_possible_truncation)]
                    let id32 = id as u32;
                    if scope.node_of[id] != u32::MAX
                        && scope.now[id] != scope.last[id]
                        && !(hold_get(layout_hold, id32) & HOLD_HELD != 0
                            && node_deferred(arena, id_to_frame, id_to_region, id32))
                    {
                        scope.seed(id32);
                    }
                }
                // A node that died has no hash left to differ, so its id is seeded instead: the
                // derivation recorded the edges of whatever still anchors to it, and the closure
                // re-resolves those against its missing rect, as a full scope would.
                for id in 0..scope.node_of.len() {
                    if scope.last[id] != NO_NODE && scope.node_of[id] == u32::MAX {
                        #[allow(clippy::cast_possible_truncation)]
                        scope.seed(id as u32);
                    }
                }
                scope.close();
            }
        }
        // A forced or invalidated node, and whatever reads it, is solved: the store it owes moves
        // its rect. So is what an invalidate passed on reaches.
        for &id in &forced {
            scope.mark(id, MARK_INVALID | MARK_FORCED);
            scope.seed(id);
        }
        for &id in &pending {
            scope.mark(id, MARK_INVALID);
            scope.seed(id);
        }
        for &id in &passing {
            scope.seed(id);
        }
        scope.close();
        // A held node an anchor read may pull joins the solve, with what reads it: a node that
        // resolves reads its targets, and a pulled target resolves first (`0x7a2fc0`).
        let mut k = 0;
        while k < scope.dirty_list.len() {
            let id = scope.dirty_list[k];
            k += 1;
            let anchors = node_anchors(layout_inputs, region_data, id_to_frame, id_to_region, id);
            for t in anchors.iter().map(|a| a.relative_to) {
                if hold_get(layout_hold, t) & HOLD_HELD != 0 && !scope.is_dirty(t) {
                    scope.seed(t);
                    scope.close();
                }
            }
        }
        // A dirty frame is solved, a dirty region swept, and whatever they read is seeded at its
        // cached rect. A dirty region is seeded too, since the frame pass runs before the sweep.
        let mut solve_frames: Vec<u32> = Vec::new();
        let mut sweep_regions: Vec<u32> = Vec::new();
        let mut seed_frames: Vec<u32> = Vec::new();
        let mut seed_regions: Vec<u32> = Vec::new();
        // A cleared region is no node but keeps its rect, which what anchors to it still reads.
        let mut seed_loose: Vec<(u32, Rect)> = Vec::new();
        scope.dirty_list.sort_unstable();
        for k in 0..scope.dirty_list.len() {
            let id = scope.dirty_list[k];
            let Some(&n) = scope.node_of.get(id as usize) else {
                continue;
            };
            if n == u32::MAX {
                continue;
            }
            let idx = n & !REGION_TAG;
            if n & REGION_TAG != 0 {
                sweep_regions.push(idx);
                seed_regions.push(idx);
            } else {
                solve_frames.push(idx);
            }
        }
        // What a dirty node reads that is not dirty itself, each once.
        for k in 0..scope.dirty_list.len() {
            let id = scope.dirty_list[k] as usize;
            for j in 0..scope.out.get(id).map_or(0, Vec::len) {
                let t = scope.out[id][j];
                if scope.is_dirty(t) || scope.has(t, MARK_SEEN) {
                    continue;
                }
                scope.mark(t, MARK_SEEN);
                match scope.node_of.get(t as usize).copied() {
                    None => {}
                    Some(u32::MAX) => {
                        if scope.dep_head.get(t as usize).copied().unwrap_or(u32::MAX) != u32::MAX {
                            if let Some(r) =
                                id_to_region.get(&t).and_then(|rh| region_resolved.get(rh))
                            {
                                seed_loose.push((t, *r));
                            }
                        }
                    }
                    Some(n) if n & REGION_TAG != 0 => seed_regions.push(n & !REGION_TAG),
                    Some(n) => seed_frames.push(n),
                }
            }
        }
        // This solve's width, beside `layout_solves` (how often) and `layout_rounds` (how deep).
        *layout_last_scope = (solve_frames.len(), sweep_regions.len());
        // ── The keep test's cached rects ──────────────────────────────────────────────────────
        // `0x768d20` holds each new rect against the one its last store left (`[+0x40..+0x4c]`)
        // and keeps that one when every edge moved less than 1e-5 layout units, so a slow drift
        // builds until it crosses. The cache is the rect at this resolve's entry: a mid-pass round
        // is our fixpoint's, not a store. A forced node, a frame with no rect, a region whose
        // owner's scale moved (the cascade reaches its regions, `0x76ad06`), has no cache; a
        // node pulled mid-resolve is forced from then on, so each round applies the marks.
        let entry_frames: Vec<Option<Rect>> = solve_frames
            .iter()
            .map(|&i| resolved.get(&scope.plan[i as usize].0).copied())
            .collect();
        let entry_regions: Vec<Option<Rect>> = sweep_regions
            .iter()
            .map(|&i| region_resolved.get(&scope.regions[i as usize].rh).copied())
            .collect();
        solver.set_keep_eps(resolve_eps);
        let round_cap = scope.plan.len() + region_data.len() + 2;
        // `WOW_LAYOUT_PROF=1`: rounds, graph size, and the frame-solve and region-sweep split.
        let prof = layout_prof_enabled();
        let mut t_frames = std::time::Duration::ZERO;
        let mut t_regions = std::time::Duration::ZERO;
        let mut n_regions_swept = 0u64;
        // Whether node `id` is deferred in this resolve: its bit set and nothing pulled it.
        let deferral = |scope: &LayoutScope,
                        arena: &crate::widget::WidgetArena,
                        layout_hold: &[u8],
                        id: u32|
         -> (bool, bool) {
            let deferred =
                !scope.has(id, MARK_PULLED) && node_deferred(arena, id_to_frame, id_to_region, id);
            (deferred, hold_get(layout_hold, id) & HOLD_HELD != 0)
        };
        for round in 0..round_cap {
            *layout_rounds += 1;
            let t_round = prof.then(std::time::Instant::now);
            let mut changed = false;

            // Seed the screen root and every read rect this pass does not recompute.
            solver.begin();
            solver.set_external(SCREEN, *screen);
            // The screen the last converged resolve read: a move reaches what anchors to it.
            solver.set_entry(SCREEN, *layout_screen, false);
            for &i in &seed_regions {
                let n = scope.regions[i as usize];
                if let Some(r) = region_resolved.get(&n.rh) {
                    solver.set_external(n.id, *r);
                }
                let (d, held) = deferral(scope, arena, layout_hold, n.id);
                solver.set_deferral(n.id, d, held);
            }
            for &(id, r) in &seed_loose {
                solver.set_external(id, r);
                let (d, held) = deferral(scope, arena, layout_hold, id);
                solver.set_deferral(id, d, held);
            }
            for (&i, &raw) in sweep_regions.iter().zip(&entry_regions) {
                let row = scope.regions[i as usize];
                let owner_forced = frame_to_id
                    .get(&row.owner)
                    .is_some_and(|&o| scope.has(o, MARK_FORCED));
                let cached = (!scope.has(row.id, MARK_FORCED) && !owner_forced)
                    .then_some(raw)
                    .flatten();
                solver.set_entry(
                    row.id,
                    cached,
                    scope.has(row.id, MARK_INVALID) || owner_forced,
                );
            }
            for &i in &seed_frames {
                let (h, id) = scope.plan[i as usize];
                if let Some(r) = resolved.get(&h) {
                    solver.set_external(id, *r);
                }
                let (d, held) = deferral(scope, arena, layout_hold, id);
                solver.set_deferral(id, d, held);
            }
            for (&i, &raw) in solve_frames.iter().zip(&entry_frames) {
                let (h, id) = scope.plan[i as usize];
                let Some(input) = layout_inputs.get(&h) else {
                    continue;
                };
                solver.set_frame(id, &input.stored(&space));
                let cached = (!scope.has(id, MARK_FORCED)).then_some(raw).flatten();
                solver.set_entry(id, cached, scope.has(id, MARK_INVALID));
                let (d, held) = deferral(scope, arena, layout_hold, id);
                solver.set_deferral(id, d, held);
            }
            for &id in &passing {
                solver.set_passes(id);
            }
            solver.solve();
            for &i in &solve_frames {
                let (h, id) = scope.plan[i as usize];
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
                let (deferred, held) = deferral(scope, arena, layout_hold, region_id);
                solver.set_deferral(region_id, deferred, held);
                let reached = solver.reached(region_id, data.anchors.iter().map(|a| a.relative_to));
                if reached && deferred {
                    solver.hold_reached(region_id);
                }
                // Not reached since its entry, or deferred: it keeps its state, rect or none (see
                // `LayoutSolver::set_entry`).
                if !reached || deferred {
                    match solver.entry(region_id) {
                        Some(kept) => {
                            solver.set_external(region_id, kept);
                            if region_resolved.get(&rh) != Some(&kept) {
                                region_resolved.insert(rh, kept);
                                changed = true;
                            }
                        }
                        None => {
                            if region_resolved.remove(&rh).is_some() {
                                changed = true;
                            }
                        }
                    }
                    continue;
                }
                // The owner supplies the region's scale and nothing else: no fallback edges.
                let scale = arena.frame(owner).map(|f| f.effective_scale).unwrap_or(1.0);
                // The size slots the resolver calls (`0x767579`, `0x7675e9`), each class its own,
                // in layout units. A texture reads its art only for an axis stored 0, loading it
                // first if it must (`0x44b3b0`), so only a resolve that reaches the node reads it.
                let units = space.units();
                let (aw, ah) = data.size.unwrap_or((0.0, 0.0));
                let size = if is_fontstring {
                    let measured = effective_measure(data);
                    (
                        fontstring_slot(units, aw, measured.map(|m| m.w)),
                        fontstring_slot(units, ah, measured.map(|m| m.h)),
                    )
                } else {
                    let (sw, sh) = (units.store(aw), units.store(ah));
                    let content = (sw == 0.0 || sh == 0.0)
                        .then(|| content_span(data, texture_size_probe.as_ref()))
                        .flatten();
                    (
                        texture_slot(units, sw, content.map(|c| c.0)),
                        texture_slot(units, sh, content.map(|c| c.1)),
                    )
                };
                scratch.anchors = data.anchors.stored(units);
                scratch.scale = scale;
                let (edges, reads) = layout::resolve_rect_edges_reads_sized(&scratch, size, |id| {
                    // The screen root is a frame-pass external only: a region anchor to id 0 finds
                    // no rect here, though the solver holds one.
                    (id != SCREEN).then(|| solver.rect(id)).flatten()
                });
                solver.note_reads(region_id, &data.anchors, reads);
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
                let resolved_edges = vertical.zip(horizontal);
                let Some(((bottom, top), (left, right))) = resolved_edges else {
                    // The failed resolve (`0x768d55`) clears the valid bit, so the getters and a
                    // FontString's draw lose the rect; a Texture keeps the quad its last good
                    // resolve wrote (`0x770670`), as its emit checks only its texture (`0x7706e0`).
                    if let Some(last) = region_resolved.remove(&rh) {
                        changed = true;
                        if !is_fontstring {
                            region_kept.insert(rh, last);
                        }
                    }
                    continue;
                };
                let rect = Rect::new(bottom, left, top, right);
                // The keep test, as for a frame: the same `0x768d20` resolves regions.
                let rect = solver
                    .entry(region_id)
                    .filter(|&c| layout::keeps_rect(c, rect, resolve_eps))
                    .unwrap_or(rect);
                // Into the solver too: a later region in this sweep that anchors here needs it.
                solver.set_external(region_id, rect);
                if region_resolved.get(&rh) != Some(&rect) {
                    region_resolved.insert(rh, rect);
                    changed = true;
                }
                region_kept.remove(&rh);
            }

            if let Some(t) = t_reg {
                t_regions += t.elapsed();
            }

            // ── The round's deferral: what it reached deferred, and what its reads pulled ────
            // A deferred node reached is pending and passes its invalidate on, once.
            for k in 0..solver.newly_held().len() {
                let id = solver.newly_held()[k];
                hold_set(layout_hold, id, HOLD_HELD);
                if !scope.has(id, MARK_PASSING) {
                    scope.mark(id, MARK_PASSING);
                    passing.push(id);
                    changed = true;
                }
            }
            // A read of a deferred target clears its bit for good and resolves it first, if it
            // is pending (`0x7a2fb8` → `0x768060(0)` → `0x7680e0(1)`); here, in the next round,
            // after which its readers resolve again against it.
            for k in 0..solver.deferred_reads().len() {
                let (reader, t) = solver.deferred_reads()[k];
                if scope.has(t, MARK_PULLED) {
                    continue;
                }
                scope.mark(t, MARK_PULLED);
                if let Some(f) = id_to_frame.get(&t).and_then(|&h| arena.frame_mut(h)) {
                    f.layout_deferred = false;
                } else if let Some(r) = id_to_region.get(&t).and_then(|&rh| arena.region_mut(rh)) {
                    r.layout_deferred = false;
                }
                if scope.has(reader, MARK_CHAIN) {
                    scope.mark(t, MARK_CHAIN);
                    if let Some(order) = chain.as_mut() {
                        let at = order
                            .iter()
                            .position(|&c| c == reader)
                            .unwrap_or(order.len());
                        order.insert(at, t);
                    }
                }
                let f = hold_get(layout_hold, t);
                if f & HOLD_HELD == 0 {
                    continue;
                }
                hold_clear(layout_hold, t, HOLD_HELD | HOLD_FORCE);
                layout_unsettled.remove(&t);
                scope.mark(
                    t,
                    MARK_INVALID | if f & HOLD_FORCE != 0 { MARK_FORCED } else { 0 },
                );
                if let Some(&rh) = id_to_region.get(&t) {
                    fail_loose(rh, arena, region_resolved, region_kept);
                }
                changed = true;
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
                *layout_screen = Some(*screen);
                // Adopted only on convergence; an incremental pass adopts only what it re-hashed.
                if incremental {
                    scope.commit_incremental();
                } else {
                    scope.commit(*screen);
                }
                // ── Re-arm the ledger ─────────────────────────────────────────────────────────
                // The graph now describes a converged model, so writes may name nodes against it.
                // Only here: the cycle bail below leaves it `None`.
                *layout_touched = Some(Vec::new());
                // Under verify, `resolve_layout` re-derives this frame from scratch and compares.
                *layout_verify_recheck = incremental && verify;
                // A converged solve closes tier 1, at `epoch_at_entry` rather than the live epoch,
                // so a touch during this pass (the tooltip pre-pass, a minted id) re-opens it. The
                // fingerprint hashes inputs only, so the next unmutated resolve would match it.
                *layout_epoch_resolved = Some(epoch_at_entry);
                *layout_chain = chain;
                queue_size_changes(
                    &watched,
                    resolved,
                    frame_to_id,
                    size_eps,
                    pending_size_changed,
                );
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
        *layout_chain = chain;
        queue_size_changes(
            &watched,
            resolved,
            frame_to_id,
            size_eps,
            pending_size_changed,
        );
    }

    /// Derive the layout graph from scratch: the roster, the reverse edges, the per-node hashes and
    /// the tier-2 fingerprint. Runs only when the ledger cannot vouch for the cache.
    fn derive_layout_graph(model: &mut Model, pre: &mut PreambleProf) -> InputFingerprint {
        model.layout_derives = model.layout_derives.wrapping_add(1);
        let space = model.layout_space();
        let Model {
            arena,
            layout_inputs,
            region_data,
            region_rects: region_resolved,
            region_kept_rects: region_kept,
            frame_to_id,
            id_to_region,
            region_to_id,
            next_id,
            layout_scope: scope,
            layout_force,
            ..
        } = model;

        // One id per live frame, its layout input's scale and clamp synced from the arena.
        let mut ids: Vec<(FrameHandle, u32)> = Vec::with_capacity(frame_to_id.len());
        for (&h, &id) in frame_to_id.iter() {
            if arena.frame(h).is_some() {
                ids.push((h, id));
            }
        }
        // In id order, not the map's: the roster, and every id minted below, are the same each
        // run.
        ids.sort_unstable_by_key(|&(_, id)| id);
        for &(h, id) in &ids {
            let (scale, clamp) = arena
                .frame(h)
                .map(|f| (f.effective_scale, f.clamped_to_screen))
                .unwrap_or((1.0, false));
            let input = layout_inputs.entry(h).or_default();
            // A moved effective scale clears the resolved bit (`0x768360` from `0x76acd5`).
            if input.scale.to_bits() != scale.to_bits() {
                layout_force.push(id);
            }
            input.scale = scale;
            // Clamp-to-screen is a frame property (geometry flags `0x10`), so every placement path
            // clamps alike, against the root's layout extent ([`LayoutInput::stored`]).
            input.clamp = clamp;
        }
        pre.ids = pre.lap();

        // The reference resolves one graph in which frames and regions anchor each other; ours
        // alternates a frame solve and a region sweep until a round changes no rect, compared
        // exactly. The round cap is the node count, so only a cycle exhausts it, warned like the
        // reference's resolving-flag bail. Every rect is recomputed from inputs and externals and
        // held against its own cached rect, so last frame's rects are a legal seed: only dead
        // regions drop. A cleared one keeps its rect until its own next invalidate.
        region_resolved.retain(|rh, _| arena.region(*rh).is_some() && region_data.contains_key(rh));
        region_kept.retain(|rh, _| arena.region(*rh).is_some() && region_data.contains_key(rh));
        pre.retain = pre.lap();

        // ── The change gate ───────────────────────────────────────────────────────────────────
        // Fingerprint what the rounds read, to skip them when nothing moved since the last
        // converged resolve: the screen; each live frame's id and synced `LayoutInput`; each
        // anchored region's node hash (anchors, explicit size, measured
        // extent, owner) and liveness. A Texture's art is no input: the sweep reads it afresh only
        // for a node a write named or the resolve reaches. Seeded rects are outputs, a
        // function of these and of the rects the keep test held them against, which a re-run with
        // the same inputs keeps again, so they are not hashed.
        // The same walk hashes each node alone into `LayoutScope::now` and records its edges.
        // Sized past `next_id` for the ids the region walk below may mint.
        scope.begin_full(*next_id as usize + region_data.len() + 1);
        pre.begin = pre.lap();
        let mut fp = InputFingerprint::default();
        // The root and the cells, which every stored size and offset is read under.
        fp.rect(space.root());
        let units = space.units();
        fp.f32(units.k1());
        fp.f32(units.k2());
        fp.f32(units.store(1.0));
        for &(h, id) in &ids {
            let Some(input) = layout_inputs.get(&h) else {
                continue;
            };
            #[allow(clippy::cast_possible_truncation)]
            let i = scope.plan.len() as u32;
            fp.feed(u64::from(id));
            fp.input(input);
            if let Some(slot) = scope.now.get_mut(id as usize) {
                *slot = frame_node_hash(input);
                scope.node_of[id as usize] = i;
            }
            scope.plan.push((h, id));
            // A frame reads its anchor targets' rects.
            for a in &input.anchors {
                scope.link(id, a.relative_to);
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
        // In handle order, not the map's, so an id minted here is the same each run.
        let mut rows: Vec<RegionHandle> = region_data.keys().copied().collect();
        rows.sort_unstable_by_key(|rh| rh.fingerprint_bits());
        for rh in rows {
            let data = &region_data[&rh];
            // Anchor-less entries are not layout inputs: hashing them would make a paint-only
            // setter that creates one (`SetTexture`'s `or_default()`) read as a layout change.
            if data.anchors.is_empty() {
                continue;
            }
            fed_regions += 1;
            let live = arena.region(rh);
            let owner_id = live.and_then(|r| frame_to_id.get(&r.owner).copied());
            // The node hash covers every input the sweep reads (anchors, size, measure, the
            // owner), so the two tiers read one set.
            let node = region_node_hash(data, owner_id);
            fp.feed(rh.fingerprint_bits());
            fp.feed(node);
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
                    *slot = node;
                    scope.node_of[id as usize] = idx;
                }
                // A region reads its anchor targets' rects and its owner's scale.
                for a in &data.anchors {
                    scope.link(id, a.relative_to);
                }
                if let Some(owner_id) = owner_id {
                    scope.link(id, owner_id);
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

    /// Re-derive each named node against the model, in the cached graph: a node joins the roster
    /// with its edges (a frame or region born, a region anchored), leaves it with its edges
    /// unlinked (a region freed or cleared), or keeps its slot with its edges re-pointed and its
    /// inputs re-synced, exactly as [`Self::derive_layout_graph`] would build it. Each node whose
    /// hash or edges moved is seeded, and the closure taken. `false` gives up for the derivation:
    /// a frame leaving the roster.
    fn rederive_named(model: &mut Model, mut named: Vec<u32>, pre: &mut PreambleProf) -> bool {
        let Model {
            arena,
            layout_inputs,
            region_data,
            region_rects: region_resolved,
            region_kept_rects: region_kept,
            frame_to_id,
            id_to_frame,
            id_to_region,
            next_id,
            layout_scope: scope,
            layout_force,
            layout_hold,
            ..
        } = model;
        // Pending under its deferral: its resolve waits, whatever moved its inputs.
        let held = |layout_hold: &[u8], arena: &crate::widget::WidgetArena, id: u32| {
            hold_get(layout_hold, id) & HOLD_HELD != 0
                && node_deferred(arena, id_to_frame, id_to_region, id)
        };
        scope.begin_incremental(*next_id as usize);
        named.sort_unstable();
        named.dedup();
        let mut targets = std::mem::take(&mut scope.targets);
        let mut at = 0;
        while at < named.len() {
            let id = named[at];
            at += 1;
            let Some(&n) = scope.node_of.get(id as usize) else {
                continue; // past every minted id: never a node
            };
            let is_region_node = n != u32::MAX && n & REGION_TAG != 0;
            // A frame: every live frame with an id is a node.
            if let Some(&h) = id_to_frame.get(&id) {
                let Some(frame) = arena.frame(h).filter(|_| frame_to_id.get(&h) == Some(&id))
                else {
                    if n != u32::MAX {
                        scope.targets = targets;
                        return false;
                    }
                    continue;
                };
                if is_region_node {
                    scope.targets = targets;
                    return false;
                }
                // The derivation's sync of the arena's scale and clamp into the layout input.
                let (scale, clamp) = (frame.effective_scale, frame.clamped_to_screen);
                let input = layout_inputs.entry(h).or_default();
                if input.scale.to_bits() != scale.to_bits() {
                    layout_force.push(id);
                }
                input.scale = scale;
                input.clamp = clamp;
                targets.clear();
                targets.extend(input.anchors.iter().map(|a| a.relative_to));
                let hash = frame_node_hash(input);
                let relinked = if n == u32::MAX {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        scope.node_of[id as usize] = scope.plan.len() as u32;
                    }
                    scope.plan.push((h, id));
                    scope.relink(id, &targets);
                    true
                } else {
                    scope.relink(id, &targets)
                };
                let held = held(layout_hold, arena, id);
                scope.note(id, hash, relinked, held);
                continue;
            }
            // A region: a node while it lives and has an anchor.
            let live = id_to_region.get(&id).and_then(|&rh| {
                let r = arena.region(rh)?;
                let data = region_data.get(&rh).filter(|d| !d.anchors.is_empty())?;
                Some((rh, r, data))
            });
            let Some((rh, r, data)) = live else {
                if is_region_node {
                    // A cleared region leaves the graph but keeps its rect; a dead one drops it.
                    let row = scope.remove_region(id, n);
                    if arena.region(row.rh).is_none() || !region_data.contains_key(&row.rh) {
                        region_resolved.remove(&row.rh);
                        region_kept.remove(&row.rh);
                    }
                } else if n != u32::MAX {
                    scope.targets = targets;
                    return false;
                }
                continue;
            };
            if n != u32::MAX && !is_region_node {
                scope.targets = targets;
                return false;
            }
            let owner_id = frame_to_id.get(&r.owner).copied();
            targets.clear();
            targets.extend(data.anchors.iter().map(|a| a.relative_to));
            targets.extend(owner_id);
            let hash = region_node_hash(data, owner_id);
            let row = RegionRow {
                rh,
                id,
                owner: r.owner,
                is_fontstring: matches!(r.kind, crate::widget::RegionKind::FontString),
            };
            let relinked = if n == u32::MAX {
                #[allow(clippy::cast_possible_truncation)]
                {
                    scope.node_of[id as usize] = scope.regions.len() as u32 | REGION_TAG;
                }
                scope.regions.push(row);
                scope.relink(id, &targets);
                true
            } else {
                scope.regions[(n & !REGION_TAG) as usize] = row;
                scope.relink(id, &targets)
            };
            let held = held(layout_hold, arena, id);
            scope.note(id, hash, relinked, held);
        }
        scope.targets = targets;
        scope.close();
        pre.seed = pre.lap();
        pre.incremental = true;
        true
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
            None => {
                let mut all: Vec<RegionHandle> = model
                    .region_data
                    .iter()
                    .filter(|(_, d)| d.text.as_deref().is_some_and(|t| !t.is_empty()))
                    .map(|(&rh, _)| rh)
                    .filter(|&rh| stale_measure_key(&model, rh).is_some())
                    .collect();
                // In handle order, not the map's: a request mints its region's id.
                all.sort_unstable_by_key(|rh| rh.fingerprint_bits());
                all
            }
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
}

/// The [`MeasureRequest`] region `rh` needs now, if any; shared with the synchronous measure
/// ([`super::measure::ensure_measured`]), as both must build the same cache key.
pub(super) fn measure_request_for(model: &mut Model, rh: RegionHandle) -> Option<MeasureRequest> {
    let (key, scale) = stale_measure_key(model, rh)?;
    // A string with no font measures 0 without asking the font engine (`0x772bd0`, `0x7728b6`,
    // `0x7729d4`); it can hold text, as XML `text=` stores it unchecked (`0x77103a`).
    if !super::font_block::has_font(model, rh) {
        let id = model.region_id(rh);
        let zero = super::MeasuredText {
            w: 0.0,
            h: 0.0,
            natural_w: 0.0,
            key,
        };
        super::measure::store_measured(model, rh, id, zero);
        return None;
    }
    let wrap_width = model.region_data.get(&rh)?.wrap_width();
    measure_request_at(model, rh, wrap_width, key, scale)
}

/// The [`MeasureRequest`] for `rh`'s text laid out in `wrap_width` (`None`, unwrapped) under
/// `key`, whatever measure it holds; `None` for no text.
pub(super) fn measure_request_at(
    model: &mut Model,
    rh: RegionHandle,
    wrap_width: Option<f32>,
    key: u64,
    scale: f32,
) -> Option<MeasureRequest> {
    let d = model.region_data.get(&rh)?;
    let text = d.text.clone().filter(|t| !t.is_empty())?;
    let font = d.font_path.clone();
    let height = d.font_height;
    let text_height = d.text_height();
    let outline = d.outline;
    let spacing = d.line_gap();
    let shadow_x = d.shadow_x();
    let max_lines = d.max_lines;
    let non_space_wrap = model.non_space_wrap_of(rh);
    let id = model.region_id(rh);
    Some(MeasureRequest {
        id,
        font,
        height,
        text_height,
        wrap_width,
        outline,
        spacing,
        shadow_x,
        non_space_wrap,
        max_lines,
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
