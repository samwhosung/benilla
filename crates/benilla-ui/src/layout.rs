//! The frame anchor resolver: the 1.12 client's `CLayoutFrame` rect resolution, transcribed
//! function by function, with the reference's operations in the reference's order.
//!
//! The client computes on the x87 at 53-bit precision, which `f64` matches, and narrows to `f32`
//! only at its stores (each sub-edge before a `combine_*`, each final edge, the `width * scale`
//! span), so the arithmetic here runs in `f64` with an explicit `as f32` at exactly those sites.
//!
//! Rects are `[bottom, left, top, right]` (frame `+0x64..+0x70`, `GetRect` `0x768320`) in screen
//! pixels, y-up.

use std::collections::VecDeque;

// ── Constants ────────────────────────────────────────────────────────────────────────────────

/// The unset-coordinate sentinel, `+Inf` (`0xcf550c`, set at runtime from `0x81d56c`).
const UNSET: f64 = f64::INFINITY;

/// The least width or height change that fires `OnSizeChanged`: 2^-22, bits `0x34800000` at
/// `0x8029d4`, tested in `ApplyRect` (`0x76b580`).
pub const SIZE_EPS: f64 = 2.384_185_791_015_625e-7;

/// The least edge move against the cached rect for the client to re-store it and `ApplyRect`, 1e-5
/// (`0x80c5c8`, in `0x768d20`). Recorded, not applied: the caller's own dirty gate decides.
pub const RESOLVE_EPS: f64 = f32::from_bits(0x3727_c5ac) as f64;

/// Point id to X column: left 0, center 1, right 2.
const X_COL: [u8; 9] = [0, 1, 2, 0, 1, 2, 0, 1, 2];
/// Point id to Y row: top 0, center 1, bottom 2.
const Y_ROW: [u8; 9] = [0, 0, 0, 1, 1, 1, 2, 2, 2];

// ── Public types ─────────────────────────────────────────────────────────────────────────────

/// The nine anchor points; the discriminant is the client's point id, the slot in its
/// `anchorPoints[9]` (tables at `0x81c3b8..0x81c3fc`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Point {
    TopLeft = 0,
    Top = 1,
    TopRight = 2,
    Left = 3,
    Center = 4,
    Right = 5,
    BottomLeft = 6,
    Bottom = 7,
    BottomRight = 8,
}

impl Point {
    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    pub const fn from_id(id: u8) -> Option<Point> {
        Some(match id {
            0 => Point::TopLeft,
            1 => Point::Top,
            2 => Point::TopRight,
            3 => Point::Left,
            4 => Point::Center,
            5 => Point::Right,
            6 => Point::BottomLeft,
            7 => Point::Bottom,
            8 => Point::BottomRight,
            _ => return None,
        })
    }
}

/// An anchor target, which the caller maps to a frame or an external rect (`CAnchor+0xc`).
pub type Handle = u32;

/// A resolved rect, `[bottom, left, top, right]` in screen pixels, y-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub bottom: f32,
    pub left: f32,
    pub top: f32,
    pub right: f32,
}

impl Rect {
    pub const fn new(bottom: f32, left: f32, top: f32, right: f32) -> Rect {
        Rect {
            bottom,
            left,
            top,
            right,
        }
    }

    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.bottom, self.left, self.top, self.right]
    }

    #[inline]
    pub const fn from_array(a: [f32; 4]) -> Rect {
        Rect::new(a[0], a[1], a[2], a[3])
    }

    #[inline]
    pub fn width(self) -> f32 {
        self.right - self.left
    }

    #[inline]
    pub fn height(self) -> f32 {
        self.top - self.bottom
    }
}

/// One anchor, a client `CAnchor` (`xOff +4`, `yOff +8`, `relativeTo +0xc`, `relativePoint +0x10`):
/// `point` on this frame pins to `relative_point` on the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub point: Point,
    pub relative_to: Handle,
    pub relative_point: Point,
    pub x_off: f32,
    pub y_off: f32,
}

impl Anchor {
    pub fn new(
        point: Point,
        relative_to: Handle,
        relative_point: Point,
        x_off: f32,
        y_off: f32,
    ) -> Anchor {
        Anchor {
            point,
            relative_to,
            relative_point,
            x_off,
            y_off,
        }
    }
}

/// A frame's layout input, the geometry fields the client's resolver reads: `anchorPoints[9]`
/// (`G+0x4`), `width` (`G+0x50`), `height` (`G+0x54`), `layoutScale` (`G+0x58`) and the clamp flag
/// (bit 4 of `G+0x3c`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutInput {
    /// Slotted by `point`, the later of two on one point winning (`SetPoint`, `0x767c70`).
    pub anchors: Vec<Anchor>,
    /// `0.0` is the no-size sentinel (`0x7ffd74`): the axis derives from the anchors.
    pub width: f32,
    pub height: f32,
    /// The effective scale, parent times own (`0x76ac90`), applied only to the anchor offsets and
    /// the size span.
    pub scale: f32,
    /// Clamp the rect into `[0, extent]` on each axis.
    pub clamp: bool,
    /// The clamp's X extent; the client's default is `0.8` (`0x41ae60`).
    pub extent_x: f32,
    /// The clamp's Y extent; the client's default is `0.6` (`0x41ae70`).
    pub extent_y: f32,
}

impl Default for LayoutInput {
    fn default() -> LayoutInput {
        LayoutInput {
            anchors: Vec::new(),
            width: 0.0,
            height: 0.0,
            scale: 1.0,
            clamp: false,
            extent_x: 0.8,
            extent_y: 0.6,
        }
    }
}

impl LayoutInput {
    /// A frame with the given anchors and explicit size, scale `1.0`, no clamp.
    pub fn sized(anchors: Vec<Anchor>, width: f32, height: f32) -> LayoutInput {
        LayoutInput {
            anchors,
            width,
            height,
            ..LayoutInput::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolveOutcome {
    Resolved(Rect),
    /// An edge stayed `+Inf`: the client's `assemble` returns 0 and its caller sets flags bit 3.
    Unresolvable,
}

// ── Leaf kernels: each returns the un-narrowed `f64` ─────────────────────────────────────────

/// `combineEdge` (`0x767440`): an edge from the opposite edge and the signed `size * scale` span,
/// else from the axis center. The sentinels compare by IEEE equality, as the client's `fcomp`
/// does, so a NaN never matches one.
#[allow(clippy::float_cmp)]
pub fn combine_edge(center: f32, opp: f32, span: f32) -> f64 {
    let (c, o, s) = (f64::from(center), f64::from(opp), f64::from(span));
    if o != UNSET && s != 0.0 {
        return o + s;
    }
    if c != UNSET {
        if s != 0.0 {
            return s * 0.5 + c;
        }
        if o != UNSET {
            return (c - o) + c;
        }
    }
    UNSET
}

/// `combineCenter` (`0x7672d0`): an axis center from the low and high edges and the signed span.
#[allow(clippy::float_cmp)]
pub fn combine_center(lo: f32, hi: f32, span: f32) -> f64 {
    let (l, h, s) = (f64::from(lo), f64::from(hi), f64::from(span));
    if l != UNSET {
        if h != UNSET {
            return (l + h) * 0.5;
        }
        if s != 0.0 {
            return s * 0.5 + l;
        }
    }
    if h != UNSET && s != 0.0 {
        return h - s * 0.5;
    }
    UNSET
}

/// `CAnchor::ResolveX` (`0x7a2f90`): the anchor's X against its target's cached `rect`, which is
/// `None` when `GetRect` fails. `mirror` is the target's normalize predicate (vtable `+0x24`, 0 for
/// an ordinary frame at `0x46ff60`), which shifts the edges to start at 0, each stored as `f32`.
pub fn anchor_resolve_x(
    scale: f32,
    x_off: f32,
    rel_point: u32,
    rect: Option<[f32; 4]>,
    mirror: bool,
) -> f64 {
    let Some([_bottom, mut left, _top, mut right]) = rect else {
        return UNSET;
    };
    if mirror {
        right = (-f64::from(left) + f64::from(right)) as f32;
        left = 0.0;
    }
    if rel_point > 8 {
        return UNSET;
    }
    let (s, xo) = (f64::from(scale), f64::from(x_off));
    let (l, r) = (f64::from(left), f64::from(right));
    match X_COL[rel_point as usize] {
        0 => s * xo + l,             // relLeft
        1 => (r + l) * 0.5 + s * xo, // center
        _ => s * xo + r,             // relRight
    }
}

/// `CAnchor::ResolveY` (`0x7a3070`), the Y counterpart of [`anchor_resolve_x`].
pub fn anchor_resolve_y(
    scale: f32,
    y_off: f32,
    rel_point: u32,
    rect: Option<[f32; 4]>,
    mirror: bool,
) -> f64 {
    let Some([mut bottom, _left, mut top, _right]) = rect else {
        return UNSET;
    };
    if mirror {
        top = (-f64::from(bottom) + f64::from(top)) as f32;
        bottom = 0.0;
    }
    if rel_point > 8 {
        return UNSET;
    }
    let (s, yo) = (f64::from(scale), f64::from(y_off));
    let (b, t) = (f64::from(bottom), f64::from(top));
    match Y_ROW[rel_point as usize] {
        0 => s * yo + t,             // relTop
        1 => (t + b) * 0.5 + s * yo, // center
        _ => s * yo + b,             // relBottom
    }
}

/// [`assemble_rect`]'s result: `ok` when every edge resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssembleRect {
    pub ok: bool,
    /// `[bottom, left, top, right]`, meaningful only when `ok`.
    pub rect: [f32; 4],
}

/// `assemble` (`0x767a20`): fails if any edge is `+Inf`, else with `clamp` (flags bit 4) shifts
/// the rect into `[0, extent]` on each axis, low edge first, keeping its size.
#[allow(clippy::float_cmp)]
pub fn assemble_rect(edges: [f32; 4], clamp: bool, extent_x: f32, extent_y: f32) -> AssembleRect {
    let [mut bottom, mut left, mut top, mut right] = edges;
    let inf = f32::INFINITY;
    if bottom == inf || left == inf || top == inf || right == inf {
        return AssembleRect {
            ok: false,
            rect: edges,
        };
    }
    if clamp {
        if (left as f64) < 0.0 {
            right = (f64::from(right) - f64::from(left)) as f32;
            left = 0.0;
        }
        if (bottom as f64) < 0.0 {
            top = (f64::from(top) - f64::from(bottom)) as f32;
            bottom = 0.0;
        }
        if (right as f64) > (extent_x as f64) {
            let over = f64::from(right) - f64::from(extent_x);
            left = (f64::from(left) - over) as f32;
            right = extent_x;
        }
        if (top as f64) > (extent_y as f64) {
            let over = f64::from(top) - f64::from(extent_y);
            bottom = (f64::from(bottom) - over) as f32;
            top = extent_y;
        }
    }
    AssembleRect {
        ok: true,
        rect: [bottom, left, top, right],
    }
}

/// `ApplyRect`'s `OnSizeChanged` test (`0x76b580`): width or height moved by at least
/// [`SIZE_EPS`], the deltas taken in `f64` from the `f32` edges.
pub fn size_changed(old: Rect, new: Rect) -> bool {
    let dw =
        (f64::from(new.right) - f64::from(new.left)) - (f64::from(old.right) - f64::from(old.left));
    let dh =
        (f64::from(new.top) - f64::from(new.bottom)) - (f64::from(old.top) - f64::from(old.bottom));
    dw.abs() >= SIZE_EPS || dh.abs() >= SIZE_EPS
}

// ── The composed resolver: `assemble` over six recursion-guarded edge resolvers ──────────────

/// An [`Anchor`] with its target's rect looked up, `None` where `GetRect` would fail.
#[derive(Clone, Copy)]
struct AnchorRel {
    x_off: f32,
    y_off: f32,
    rel_point: u32,
    rel_rect: Option<[f32; 4]>,
    mirror: bool,
}

struct EdgeInput {
    anchors: [Option<AnchorRel>; 9],
    width: f32,
    height: f32,
    scale: f32,
    clamp: bool,
    extent_x: f32,
    extent_y: f32,
}

// The recursion-guard bits of the client's `G+0x28`.
const G_LEFT: u32 = 0x01;
const G_TOP: u32 = 0x02;
const G_RIGHT: u32 = 0x04;
const G_BOTTOM: u32 = 0x08;
const G_XC: u32 = 0x10;
const G_YC: u32 = 0x20;

// The point ids each edge scans (`.rdata` 0x81c3b8..0x81c3f4).
const IDS_LEFT: [usize; 3] = [0, 3, 6];
const IDS_RIGHT: [usize; 3] = [2, 5, 8];
const IDS_TOP: [usize; 3] = [0, 1, 2];
const IDS_BOTTOM: [usize; 3] = [6, 7, 8];
const IDS_XC: [usize; 3] = [1, 4, 7];
const IDS_YC: [usize; 3] = [3, 4, 5];

/// `anchorScanX` (`0x7671a0`): the first of the edge's anchors whose X resolves, else `+Inf`.
#[allow(clippy::float_cmp)]
fn anchor_scan_x(ids: &[usize; 3], f: &EdgeInput) -> f64 {
    for &id in ids {
        if let Some(a) = f.anchors[id] {
            let v = anchor_resolve_x(f.scale, a.x_off, a.rel_point, a.rel_rect, a.mirror);
            if v != UNSET {
                return v;
            }
        }
    }
    UNSET
}

/// `anchorScanY` (`0x7671f0`).
#[allow(clippy::float_cmp)]
fn anchor_scan_y(ids: &[usize; 3], f: &EdgeInput) -> f64 {
    for &id in ids {
        if let Some(a) = f.anchors[id] {
            let v = anchor_resolve_y(f.scale, a.y_off, a.rel_point, a.rel_rect, a.mirror);
            if v != UNSET {
                return v;
            }
        }
    }
    UNSET
}

/// The edge resolvers' span, `size * scale` narrowed to `f32`, negated for LEFT and BOTTOM. The
/// client reads `size` through a virtual getter: a frame's is the authored field (`0x768420`), but
/// a texture (`0x770720`) and a font string (`0x772930`) replace an authored `0.0` with their
/// content's extent, so a region's [`LayoutInput`] must already carry that extent.
fn size_span(size: f32, scale: f32, negate: bool) -> f32 {
    let span = (f64::from(size) * f64::from(scale)) as f32;
    if negate {
        -span
    } else {
        span
    }
}

/// LEFT (`0x7673d0`): its anchor scan, else `combineEdge(Xcenter, RIGHT, -width * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_left(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_LEFT != 0 {
        return UNSET;
    }
    *guard |= G_LEFT;
    let v = anchor_scan_x(&IDS_LEFT, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.width, f.scale, true);
        let opp = resolve_right(f, guard) as f32;
        let center = resolve_xcenter(f, guard) as f32;
        combine_edge(center, opp, span)
    };
    *guard &= !G_LEFT;
    r
}

/// RIGHT (`0x767540`): its anchor scan, else `combineEdge(Xcenter, LEFT, width * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_right(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_RIGHT != 0 {
        return UNSET;
    }
    *guard |= G_RIGHT;
    let v = anchor_scan_x(&IDS_RIGHT, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.width, f.scale, false);
        let opp = resolve_left(f, guard) as f32;
        let center = resolve_xcenter(f, guard) as f32;
        combine_edge(center, opp, span)
    };
    *guard &= !G_RIGHT;
    r
}

/// TOP (`0x7674d0`): its anchor scan, else `combineEdge(Ycenter, BOTTOM, height * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_top(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_TOP != 0 {
        return UNSET;
    }
    *guard |= G_TOP;
    let v = anchor_scan_y(&IDS_TOP, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.height, f.scale, false);
        let opp = resolve_bottom(f, guard) as f32;
        let center = resolve_ycenter(f, guard) as f32;
        combine_edge(center, opp, span)
    };
    *guard &= !G_TOP;
    r
}

/// BOTTOM (`0x7675b0`): its anchor scan, else `combineEdge(Ycenter, TOP, -height * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_bottom(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_BOTTOM != 0 {
        return UNSET;
    }
    *guard |= G_BOTTOM;
    let v = anchor_scan_y(&IDS_BOTTOM, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.height, f.scale, true);
        let opp = resolve_top(f, guard) as f32;
        let center = resolve_ycenter(f, guard) as f32;
        combine_edge(center, opp, span)
    };
    *guard &= !G_BOTTOM;
    r
}

/// Xcenter (`0x767260`): its anchor scan, else `combineCenter(LEFT, RIGHT, width * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_xcenter(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_XC != 0 {
        return UNSET;
    }
    *guard |= G_XC;
    let v = anchor_scan_x(&IDS_XC, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.width, f.scale, false);
        let lo = resolve_left(f, guard) as f32;
        let hi = resolve_right(f, guard) as f32;
        combine_center(lo, hi, span)
    };
    *guard &= !G_XC;
    r
}

/// Ycenter (`0x767360`): its anchor scan, else `combineCenter(BOTTOM, TOP, height * scale)`.
#[allow(clippy::float_cmp)]
fn resolve_ycenter(f: &EdgeInput, guard: &mut u32) -> f64 {
    if *guard & G_YC != 0 {
        return UNSET;
    }
    *guard |= G_YC;
    let v = anchor_scan_y(&IDS_YC, f);
    let r = if v != UNSET {
        v
    } else {
        let span = size_span(f.height, f.scale, false);
        let lo = resolve_bottom(f, guard) as f32;
        let hi = resolve_top(f, guard) as f32;
        combine_center(lo, hi, span)
    };
    *guard &= !G_YC;
    r
}

/// `assemble` (`0x767a20`) composed, resolving LEFT, BOTTOM, RIGHT, TOP in the client's order; the
/// `+Inf` test reads the un-narrowed `f64` edge, as the client's `fcomp` precedes its `fst`.
#[allow(clippy::float_cmp)]
fn resolve_edges(f: &EdgeInput) -> ResolveOutcome {
    let mut guard = 0u32;
    let left = resolve_left(f, &mut guard);
    let bottom = resolve_bottom(f, &mut guard);
    let right = resolve_right(f, &mut guard);
    let top = resolve_top(f, &mut guard);
    if left == UNSET || bottom == UNSET || right == UNSET || top == UNSET {
        return ResolveOutcome::Unresolvable;
    }
    let edges = [bottom as f32, left as f32, top as f32, right as f32];
    let a = assemble_rect(edges, f.clamp, f.extent_x, f.extent_y);
    ResolveOutcome::Resolved(Rect::from_array(a.rect))
}

/// Resolve one frame's rect; `target_rect` gives each anchor target's resolved rect, and `None`
/// makes that anchor `+Inf`, as a failed `GetRect` does.
pub fn resolve_rect(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> ResolveOutcome {
    resolve_edges(&build_edge_input(frame, target_rect))
}

fn build_edge_input(
    frame: &LayoutInput,
    mut target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> EdgeInput {
    let mut slots: [Option<AnchorRel>; 9] = [None; 9];
    for a in &frame.anchors {
        slots[a.point.id() as usize] = Some(AnchorRel {
            x_off: a.x_off,
            y_off: a.y_off,
            rel_point: a.relative_point.id() as u32,
            rel_rect: target_rect(a.relative_to).map(Rect::to_array),
            // An ordinary frame's normalize predicate returns 0 (`0x46ff60`).
            mirror: false,
        });
    }
    EdgeInput {
        anchors: slots,
        width: frame.width,
        height: frame.height,
        scale: frame.scale,
        clamp: frame.clamp,
        extent_x: frame.extent_x,
        extent_y: frame.extent_y,
    }
}

/// Resolve one frame's edges, `[bottom, left, top, right]`, each `None` where it stayed unset;
/// `assemble` and its clamp are not applied. The region pass reads it axis by axis.
#[allow(clippy::float_cmp)]
pub fn resolve_rect_edges(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> [Option<f32>; 4] {
    let input = build_edge_input(frame, target_rect);
    let mut guard = 0u32;
    let left = resolve_left(&input, &mut guard);
    let bottom = resolve_bottom(&input, &mut guard);
    let right = resolve_right(&input, &mut guard);
    let top = resolve_top(&input, &mut guard);
    let cvt = |v: f64| if v == UNSET { None } else { Some(v as f32) };
    [cvt(bottom), cvt(left), cvt(top), cvt(right)]
}

// ── Graph driver ─────────────────────────────────────────────────────────────────────────────

/// Resolves anchored frames in dependency order, a driver of benilla's own: the client resolves
/// lazily through a dependency-ordered deferred queue (`0x7680e0`, ordered by `0x7681b0`). Handles
/// index its arrays directly, so they must be dense. A round: [`begin`](Self::begin),
/// [`set_external`](Self::set_external) and [`set_frame`](Self::set_frame), [`solve`](Self::solve).
#[derive(Clone, Debug, Default)]
pub struct LayoutSolver {
    /// Overwritten in place, so each anchors `Vec` keeps its allocation.
    input: Vec<LayoutInput>,
    is_frame: Vec<bool>,
    /// Externals as seeded, frames as they resolve.
    rect: Vec<Option<Rect>>,
    /// Kahn in-degree and reverse edges; each inner `Vec` is cleared, never freed.
    indeg: Vec<u32>,
    dependents: Vec<Vec<Handle>>,
    /// The handles written this round, the ones `begin` resets.
    touched: Vec<Handle>,
    marked: Vec<bool>,
    /// Ascending: cycle members resolve in this order, and the result depends on it.
    live: Vec<Handle>,
    queue: VecDeque<Handle>,
    emitted: Vec<bool>,
    order: Vec<Handle>,
    cycle: Vec<Handle>,
    unresolvable: Vec<Handle>,
}

impl LayoutSolver {
    pub fn new() -> LayoutSolver {
        LayoutSolver::default()
    }

    fn ensure(&mut self, h: Handle) {
        let need = h as usize + 1;
        if self.input.len() < need {
            self.input.resize_with(need, LayoutInput::default);
            self.is_frame.resize(need, false);
            self.rect.resize(need, None);
            self.indeg.resize(need, 0);
            self.dependents.resize_with(need, Vec::new);
            self.marked.resize(need, false);
            self.emitted.resize(need, false);
        }
    }

    /// Record `h` for [`begin`](Self::begin) to reset.
    fn touch(&mut self, h: Handle) {
        let i = h as usize;
        if !self.marked[i] {
            self.marked[i] = true;
            self.touched.push(h);
        }
    }

    /// Start a round, resetting only the slots the last round touched.
    pub fn begin(&mut self) {
        for &h in &self.touched {
            let i = h as usize;
            self.is_frame[i] = false;
            self.rect[i] = None;
            self.indeg[i] = 0;
            self.dependents[i].clear();
            self.marked[i] = false;
            self.emitted[i] = false;
        }
        self.touched.clear();
        self.live.clear();
        self.queue.clear();
        self.order.clear();
        self.cycle.clear();
        self.unresolvable.clear();
    }

    /// Seed a rect this solve does not compute, such as the screen root (`CSimpleTop`); also
    /// publishes one mid-sweep for later nodes of the same pass.
    pub fn set_external(&mut self, h: Handle, rect: Rect) {
        self.ensure(h);
        self.touch(h);
        self.rect[h as usize] = Some(rect);
    }

    /// Register a frame to solve, copying `src` into its slot; a steady round allocates nothing.
    pub fn set_frame(&mut self, h: Handle, src: &LayoutInput) {
        self.ensure(h);
        self.touch(h);
        let dst = &mut self.input[h as usize];
        dst.anchors.clear();
        dst.anchors.extend_from_slice(&src.anchors);
        Self::copy_fields(dst, src);
        self.is_frame[h as usize] = true;
        self.live.push(h);
    }

    /// [`set_frame`](Self::set_frame) with `anchor` as the only anchor: a ScrollFrame child's.
    pub fn set_frame_anchored(&mut self, h: Handle, src: &LayoutInput, anchor: Anchor) {
        self.ensure(h);
        self.touch(h);
        let dst = &mut self.input[h as usize];
        dst.anchors.clear();
        dst.anchors.push(anchor);
        Self::copy_fields(dst, src);
        self.is_frame[h as usize] = true;
        self.live.push(h);
    }

    fn copy_fields(dst: &mut LayoutInput, src: &LayoutInput) {
        dst.width = src.width;
        dst.height = src.height;
        dst.scale = src.scale;
        dst.clamp = src.clamp;
        dst.extent_x = src.extent_x;
        dst.extent_y = src.extent_y;
    }

    /// Resolve every registered frame, anchor targets first; cycle members then resolve
    /// best-effort, ascending by handle, against the rects known by then.
    pub fn solve(&mut self) {
        self.live.sort_unstable();

        // Only frame-to-frame anchors order the pass. A self-anchor counts, so a self-anchored
        // frame lands in `cycle`; a target named twice counts once.
        for idx in 0..self.live.len() {
            let h = self.live[idx];
            let mut seen: [Handle; 9] = [0; 9];
            let mut n_seen = 0usize;
            for a_idx in 0..self.input[h as usize].anchors.len() {
                let d = self.input[h as usize].anchors[a_idx].relative_to;
                if !self.is_frame.get(d as usize).copied().unwrap_or(false) {
                    continue;
                }
                if seen[..n_seen].contains(&d) {
                    continue;
                }
                if n_seen < seen.len() {
                    seen[n_seen] = d;
                    n_seen += 1;
                }
                self.indeg[h as usize] += 1;
                self.dependents[d as usize].push(h);
            }
        }

        for &h in &self.live {
            if self.indeg[h as usize] == 0 {
                self.queue.push_back(h);
            }
        }
        while let Some(h) = self.queue.pop_front() {
            self.order.push(h);
            self.emitted[h as usize] = true;
            for i in 0..self.dependents[h as usize].len() {
                let dep = self.dependents[h as usize][i];
                let e = &mut self.indeg[dep as usize];
                *e -= 1;
                if *e == 0 {
                    self.queue.push_back(dep);
                }
            }
        }

        for i in 0..self.order.len() {
            let h = self.order[i];
            self.resolve_one(h);
        }

        // A frame never emitted is on or behind a cycle.
        for idx in 0..self.live.len() {
            let h = self.live[idx];
            if !self.emitted[h as usize] {
                self.cycle.push(h);
                self.resolve_one(h);
            }
        }
    }

    fn resolve_one(&mut self, h: Handle) {
        let outcome = {
            let input = &self.input[h as usize];
            let rects = &self.rect;
            resolve_rect(input, |t| rects.get(t as usize).copied().flatten())
        };
        match outcome {
            ResolveOutcome::Resolved(r) => self.rect[h as usize] = Some(r),
            ResolveOutcome::Unresolvable => self.unresolvable.push(h),
        }
    }

    /// The rect at `h`: an external as seeded, a frame as resolved.
    #[inline]
    pub fn rect(&self, h: Handle) -> Option<Rect> {
        self.rect.get(h as usize).copied().flatten()
    }

    /// Frames on or behind an anchor cycle, ascending.
    pub fn cycle(&self) -> &[Handle] {
        &self.cycle
    }

    /// Frames that did not resolve; may overlap [`cycle`](Self::cycle).
    pub fn unresolvable(&self) -> &[Handle] {
        &self.unresolvable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Handle = 0;
    fn screen_rect() -> Rect {
        Rect::new(0.0, 0.0, 600.0, 800.0)
    }

    fn bits(r: Rect) -> [u32; 4] {
        [
            r.bottom.to_bits(),
            r.left.to_bits(),
            r.top.to_bits(),
            r.right.to_bits(),
        ]
    }
    fn assert_rect(got: ResolveOutcome, want: Rect) {
        match got {
            ResolveOutcome::Resolved(r) => {
                assert_eq!(bits(r), bits(want), "got {r:?} want {want:?}")
            }
            ResolveOutcome::Unresolvable => panic!("unresolvable, wanted {want:?}"),
        }
    }

    // ── Leaf kernels ─────────────────────────────────────────────────────────────────────────
    // Expected values are hand-computed from the formulas, not taken from the client.

    #[test]
    fn combine_edge_legs() {
        assert_eq!(combine_edge(10.0, 50.0, 7.0), 57.0); // opp+span
        assert_eq!(combine_edge(10.0, f32::INFINITY, 7.0), 7.0 * 0.5 + 10.0); // span/2+center
        assert_eq!(combine_edge(10.0, 50.0, 0.0), (10.0 - 50.0) + 10.0); // 2c-opp = -30
        assert_eq!(
            combine_edge(f32::INFINITY, f32::INFINITY, 0.0),
            f64::INFINITY
        );
        assert_eq!(combine_edge(f32::INFINITY, 50.0, 0.0), f64::INFINITY);
        assert_eq!(combine_edge(10.0, f32::INFINITY, 0.0), f64::INFINITY);
        // -0.0 span == 0.0 (IEEE) -> the 2c-opp leg
        assert_eq!(combine_edge(10.0, 50.0, -0.0), (10.0 - 50.0) + 10.0);
    }

    #[test]
    fn combine_center_legs() {
        assert_eq!(combine_center(10.0, 50.0, 7.0), (10.0 + 50.0) * 0.5); // both -> midpoint
        assert_eq!(combine_center(10.0, f32::INFINITY, 7.0), 7.0 * 0.5 + 10.0); // span/2+lo
        assert_eq!(combine_center(f32::INFINITY, 50.0, 7.0), 50.0 - 7.0 * 0.5); // hi-span/2
        assert_eq!(
            combine_center(f32::INFINITY, f32::INFINITY, 7.0),
            f64::INFINITY
        );
        assert_eq!(combine_center(10.0, f32::INFINITY, 0.0), f64::INFINITY);
        // both finite -> midpoint regardless of span
        assert_eq!(combine_center(10.0, 50.0, -0.0), 30.0);
    }

    #[test]
    fn anchor_resolve_x_columns() {
        let r = [3.0f32, 100.0, 480.0, 260.0]; // [bottom, left, top, right]
                                               // point ids 0, 1, 2 (left, center, right), xOff 5
        assert_eq!(anchor_resolve_x(1.0, 5.0, 0, Some(r), false), 5.0 + 100.0);
        assert_eq!(
            anchor_resolve_x(1.0, 5.0, 1, Some(r), false),
            (260.0 + 100.0) * 0.5 + 5.0
        );
        assert_eq!(anchor_resolve_x(1.0, 5.0, 2, Some(r), false), 5.0 + 260.0);
        assert_eq!(
            anchor_resolve_x(2.0, 5.0, 0, Some(r), false),
            2.0 * 5.0 + 100.0
        );
        // GetRect-fail (None) -> +Inf; out-of-range point -> +Inf
        assert_eq!(anchor_resolve_x(1.0, 5.0, 0, None, false), f64::INFINITY);
        assert_eq!(anchor_resolve_x(1.0, 5.0, 9, Some(r), false), f64::INFINITY);
        // mirror: left->0, right->right-left; center = (r'+0)/2 + xOff
        let rp = (260.0f32 - 100.0f32) as f64;
        assert_eq!(
            anchor_resolve_x(1.0, 0.0, 1, Some(r), true),
            (rp + 0.0) * 0.5
        );
    }

    #[test]
    fn anchor_resolve_y_rows() {
        let r = [3.0f32, 100.0, 480.0, 260.0];
        // relTop, center, relBottom (rows: pt 0->top, 3->center, 6->bottom)
        assert_eq!(anchor_resolve_y(1.0, 5.0, 0, Some(r), false), 5.0 + 480.0);
        assert_eq!(
            anchor_resolve_y(1.0, 5.0, 3, Some(r), false),
            (480.0 + 3.0) * 0.5 + 5.0
        );
        assert_eq!(anchor_resolve_y(1.0, 5.0, 6, Some(r), false), 5.0 + 3.0);
    }

    // ── assemble / clamp ────────────────────────────────────────────────────────────────────
    #[test]
    fn assemble_passthrough_and_fail() {
        let a = assemble_rect([3.0, 100.0, 480.0, 260.0], false, 0.8, 0.6);
        assert!(a.ok);
        assert_eq!(a.rect, [3.0, 100.0, 480.0, 260.0]);
        let bad = assemble_rect([3.0, f32::INFINITY, 480.0, 260.0], false, 0.8, 0.6);
        assert!(!bad.ok);
    }

    #[test]
    fn assemble_clamp_low_edge() {
        // left < 0 -> shift to 0, carry span to right
        let a = assemble_rect([0.1, -0.2, 0.5, 0.3], true, 0.8, 0.6);
        assert!(a.ok);
        assert_eq!(a.rect[1], 0.0); // left
                                    // right = 0.3 - (-0.2) computed via f64 then narrowed
        let want_right = (f64::from(0.3f32) - f64::from(-0.2f32)) as f32;
        assert_eq!(a.rect[3].to_bits(), want_right.to_bits());
    }

    #[test]
    fn assemble_clamp_high_edge() {
        // right > extent_x -> shift back, right = extent
        let a = assemble_rect([0.1, 0.1, 0.5, 1.5], true, 0.8, 0.6);
        assert!(a.ok);
        assert_eq!(a.rect[3], 0.8);
        let over = f64::from(1.5f32) - f64::from(0.8f32);
        let want_left = (f64::from(0.1f32) - over) as f32;
        assert_eq!(a.rect[1].to_bits(), want_left.to_bits());
    }

    // ── Composed resolver ─────────────────────────────────────────────────────────────────────

    /// The rect the running reference client's `assemble` (`0x767a20`) produced.
    #[test]
    fn oracle_single_topleft_explicit_size() {
        let f = LayoutInput::sized(
            vec![Anchor::new(
                Point::TopLeft,
                SCREEN,
                Point::TopLeft,
                10.0,
                -5.0,
            )],
            200.0,
            50.0,
        );
        // The reference run's screen root: top 600, right 800.
        let got = resolve_rect(&f, |_| Some(Rect::new(0.0, 0.0, 600.0, 800.0)));
        assert_rect(got, Rect::new(545.0, 10.0, 595.0, 210.0));
    }

    #[test]
    fn single_anchor_all_nine_points_explicit_size() {
        // Each point pinned to the same point on the screen: every leg must keep the 100x40 size.
        let screen = screen_rect();
        for &p in &[
            Point::TopLeft,
            Point::Top,
            Point::TopRight,
            Point::Left,
            Point::Center,
            Point::Right,
            Point::BottomLeft,
            Point::Bottom,
            Point::BottomRight,
        ] {
            let f = LayoutInput::sized(vec![Anchor::new(p, SCREEN, p, 0.0, 0.0)], 100.0, 40.0);
            let out = resolve_rect(&f, |_| Some(screen));
            let r = match out {
                ResolveOutcome::Resolved(r) => r,
                ResolveOutcome::Unresolvable => panic!("{p:?} unresolvable"),
            };
            assert_eq!(r.width().to_bits(), 100.0f32.to_bits(), "{p:?} width");
            assert_eq!(r.height().to_bits(), 40.0f32.to_bits(), "{p:?} height");
        }
    }

    #[test]
    fn two_edge_derivation_no_size() {
        // TOPLEFT + BOTTOMRIGHT to screen corners -> all edges direct, w/h ignored.
        let f = LayoutInput::sized(
            vec![
                Anchor::new(Point::TopLeft, SCREEN, Point::TopLeft, 10.0, -10.0),
                Anchor::new(Point::BottomRight, SCREEN, Point::BottomRight, -20.0, 30.0),
            ],
            0.0, // no explicit width
            0.0, // no explicit height
        );
        let got = resolve_rect(&f, |_| Some(screen_rect()));
        // left=0+10, top=600-10, right=800-20, bottom=0+30
        assert_rect(got, Rect::new(30.0, 10.0, 590.0, 780.0));
    }

    #[test]
    fn center_anchor_derives_edges() {
        let f = LayoutInput::sized(
            vec![Anchor::new(Point::Center, SCREEN, Point::Center, 0.0, 0.0)],
            100.0,
            40.0,
        );
        let got = resolve_rect(&f, |_| Some(screen_rect()));
        // centered at (400, 300): [bottom 280, left 350, top 320, right 450]
        assert_rect(got, Rect::new(280.0, 350.0, 320.0, 450.0));
    }

    #[test]
    fn scale_interaction() {
        let f = LayoutInput {
            anchors: vec![Anchor::new(
                Point::TopLeft,
                SCREEN,
                Point::TopLeft,
                10.0,
                -5.0,
            )],
            width: 200.0,
            height: 50.0,
            scale: 2.0,
            ..LayoutInput::default()
        };
        let got = resolve_rect(&f, |_| Some(screen_rect()));
        // left=2*10=20, top=600+2*(-5)=590, right=20+200*2=420, bottom=590-50*2=490
        assert_rect(got, Rect::new(490.0, 20.0, 590.0, 420.0));
    }

    #[test]
    fn chained_frames_via_solver() {
        const A: Handle = 1;
        const B: Handle = 2;
        let mut s = LayoutSolver::new();
        s.begin();
        s.set_external(SCREEN, screen_rect());
        // Register B before A to prove topological ordering, not registration ordering.
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(
                    Point::TopLeft,
                    SCREEN,
                    Point::TopLeft,
                    100.0,
                    -100.0,
                )],
                50.0,
                50.0,
            ),
        );
        s.solve();
        assert!(s.cycle().is_empty());
        assert!(s.unresolvable().is_empty());
        // A: left 100, top 500, right 150, bottom 450
        assert_eq!(
            bits(s.rect(A).expect("A resolved")),
            bits(Rect::new(450.0, 100.0, 500.0, 150.0))
        );
        // B hangs off A's bottom-right corner (150, 450), size 20
        assert_eq!(
            bits(s.rect(B).expect("B resolved")),
            bits(Rect::new(430.0, 150.0, 450.0, 170.0))
        );
    }

    #[test]
    fn no_anchor_is_unresolvable() {
        // A frame with no SetPoint cannot resolve (every edge derives circularly to +Inf).
        let f = LayoutInput::sized(vec![], 100.0, 40.0);
        assert_eq!(
            resolve_rect(&f, |_| Some(screen_rect())),
            ResolveOutcome::Unresolvable
        );
    }

    #[test]
    fn unset_sentinel_propagates() {
        // The one anchor's target has no rect, so its edges are +Inf and nothing else pins them.
        let f = LayoutInput::sized(
            vec![Anchor::new(
                Point::TopLeft,
                SCREEN,
                Point::TopLeft,
                0.0,
                0.0,
            )],
            100.0,
            40.0,
        );
        assert_eq!(resolve_rect(&f, |_| None), ResolveOutcome::Unresolvable);
    }

    #[test]
    fn dependent_of_unresolvable_is_unresolvable() {
        const A: Handle = 1; // no anchors -> unresolvable
        const B: Handle = 2; // anchored to A
        let mut s = LayoutSolver::new();
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(A, &LayoutInput::sized(vec![], 100.0, 40.0));
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::TopLeft, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        assert!(s.unresolvable().contains(&A));
        assert!(s.unresolvable().contains(&B));
        assert!(s.rect(A).is_none());
        assert!(s.rect(B).is_none());
    }

    #[test]
    fn cycle_detected() {
        const A: Handle = 1;
        const B: Handle = 2;
        let mut s = LayoutSolver::new();
        s.begin();
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, B, Point::TopLeft, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::TopLeft, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        assert!(s.cycle().contains(&A));
        assert!(s.cycle().contains(&B));
        assert!(s.unresolvable().contains(&A));
        assert!(s.unresolvable().contains(&B));
    }

    #[test]
    fn self_anchor_is_a_cycle() {
        const A: Handle = 1;
        let mut s = LayoutSolver::new();
        s.begin();
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        assert!(s.cycle().contains(&A));
    }

    #[test]
    fn size_changed_epsilon() {
        let r = Rect::new(0.0, 0.0, 40.0, 100.0);
        // 1e-7 is under SIZE_EPS.
        let tiny = Rect::new(0.0, 0.0, 40.0, 100.0 + 1e-7);
        assert!(!size_changed(r, tiny));
        let big = Rect::new(0.0, 0.0, 40.0, 100.001);
        assert!(size_changed(r, big));
    }

    /// The buffers live across rounds, so `begin` must leave nothing behind: round 2 anchors to a
    /// handle only round 1 registered and must find it unknown.
    #[test]
    fn begin_leaves_no_state_from_the_previous_round() {
        const A: Handle = 1;
        const B: Handle = 2;
        let mut s = LayoutSolver::new();

        // Round 1: A anchored to the screen, B hanging off A.
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(
                    Point::TopLeft,
                    SCREEN,
                    Point::TopLeft,
                    0.0,
                    0.0,
                )],
                100.0,
                40.0,
            ),
        );
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        let a_first = s.rect(A).expect("A resolved in round 1");
        assert!(s.rect(B).is_some());

        // Round 2: only B, whose target A is no longer registered.
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        assert!(
            s.rect(A).is_none(),
            "round 1's A rect must not survive begin"
        );
        assert!(
            s.rect(B).is_none(),
            "B must not resolve against a stale target"
        );
        assert!(s.unresolvable().contains(&B));
        assert!(s.cycle().is_empty(), "cycle list must be cleared too");

        // Round 3: the original pair again, which must match round 1.
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(
                    Point::TopLeft,
                    SCREEN,
                    Point::TopLeft,
                    0.0,
                    0.0,
                )],
                100.0,
                40.0,
            ),
        );
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                20.0,
                20.0,
            ),
        );
        s.solve();
        assert_eq!(
            bits(s.rect(A).expect("A resolved in round 3")),
            bits(a_first)
        );
        assert!(s.unresolvable().is_empty());
    }

    /// The override keeps the size and scale but replaces the anchors, and the reused anchors `Vec`
    /// must not leak the last round's.
    #[test]
    fn set_frame_anchored_replaces_only_the_anchors() {
        const A: Handle = 1;
        let mut s = LayoutSolver::new();
        let input = LayoutInput::sized(
            vec![
                Anchor::new(Point::TopLeft, SCREEN, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, SCREEN, Point::BottomRight, 0.0, 0.0),
            ],
            100.0,
            40.0,
        );

        // Plain: both anchors pin all four edges, so the size is ignored.
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(A, &input);
        s.solve();
        assert_eq!(
            bits(s.rect(A).expect("resolved")),
            bits(Rect::new(0.0, 0.0, 600.0, 800.0))
        );

        // Overridden: one TOPLEFT pin at (10, -5), so the 100x40 size decides the other edges.
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame_anchored(
            A,
            &input,
            Anchor::new(Point::TopLeft, SCREEN, Point::TopLeft, 10.0, -5.0),
        );
        s.solve();
        assert_eq!(
            bits(s.rect(A).expect("resolved")),
            bits(Rect::new(555.0, 10.0, 595.0, 110.0))
        );
    }
}
