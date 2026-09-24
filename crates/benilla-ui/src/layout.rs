//! The frame anchor/layout resolver — anchor graph → resolved rects.
//!
//! This is a faithful transcription of the WoW 1.12.1 client's `CLayoutFrame` geometry resolver,
//! reverse-engineered bit-exact against `WoW.exe`. The leaf float kernels and the
//! recursion-guarded per-edge composition are transcribed from the reference, function by function
//! and verified bit-exact. Matching the reference's operations and their order — not "improving"
//! the arithmetic — is the goal.
//!
//! ## Precision — why `f64` intermediates, not naive `f32`
//!
//! The client is an x87 binary: every intermediate lives in an FPU register at PC_53 (53-bit
//! mantissa = IEEE `f64` precision) and is narrowed to `f32` *only* at the documented `fstp m32`
//! store sites (each sub-edge result before a `combine_*`; each final edge into the out-rect; the
//! `width·scale` span). A leaf like `(right + left) * 0.5 + xOff·scale` therefore rounds **once**
//! (at the final store), whereas doing every operation in `f32` would round at each step and
//! diverge. So "the client is f32" is true only at the storage boundaries: to be bit-faithful the
//! chain of `+`/`-`/`*0.5` runs in `f64` (host-exact — every operand is `f32`-derived, so each op is
//! exact in `f64`) with an explicit `as f32` at exactly the sites the reference pins.
//!
//! ## Coordinate space
//!
//! Rects are stored in the client's own convention: `[bottom, left, top, right]` (frame-absolute
//! `+0x64/+0x68/+0x6c/+0x70`; `GetRect 0x768320` and the Lua `GetBottom/Left/Top/Right`
//! readers). WoW UI space is **y-up**: `top > bottom`, the screen root's rect is the
//! physical screen (the anchor chain bottoms out at the `CSimpleTop` root), and `width = right −
//! left`, `height = top − bottom`. Coordinates are screen pixels throughout; `scale` (layoutScale =
//! effective scale, `0x76ac90`) scales only the per-anchor offsets and the
//! `width/height` span.

use std::collections::VecDeque;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Constants (objdump-read from `WoW.exe`)
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// UNSET coordinate sentinel: `+Infinity` (the `.data` slot `0xcf550c`, runtime-initialized from
/// `[0x81d56c] = 0x7f800000`). "coordinate is set" ⇔ `x != +Inf`. Intermediates carry
/// this in `f64`; the `f32` narrowing preserves it (`+Inf as f32 == +Inf`).
const UNSET: f64 = f64::INFINITY;

/// `OnSizeChanged` trigger ε — `_DAT_008029d4` = `0x34800000` ≈ 2.384e-7 (applied in
/// `ApplyRect 0x76b580`). `OnSizeChanged(width,height)` fires iff `|Δwidth| ≥ ε` OR
/// `|Δheight| ≥ ε` between the old and new resolved rect. See [`size_changed`].
pub const SIZE_EPS: f64 = 2.384_185_791_015_625e-7;

/// Recompute change threshold ε — `_DAT_0080c5c8` = `0x3727c5ac` ≈ 9.96e-6 (applied in
/// `recompute 0x768d20`): an edge must move at least this far vs the cached rect for a re-store +
/// `ApplyRect`. Benilla owns its own caching (the caller's dirty gate); this const records the client's
/// documented threshold for fidelity work that needs it.
pub const RESOLVE_EPS: f64 = f32::from_bits(0x3727_c5ac) as f64;

/// point-id → X column (table `0x81c3b8`..):
/// `{0,3,6}` (LEFT column) → 0, `{1,4,7}` (center) → 1, `{2,5,8}` (RIGHT column) → 2.
const X_COL: [u8; 9] = [0, 1, 2, 0, 1, 2, 0, 1, 2];
/// point-id → Y row: `{0,1,2}` (TOP row) → 0, `{3,4,5}` (center) → 1, `{6,7,8}` (BOTTOM row) → 2.
const Y_ROW: [u8; 9] = [0, 0, 0, 1, 1, 1, 2, 2, 2];

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Public types
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The nine anchor points, with the client's exact discriminants (static tables
/// `0x81c3b8–0x81c3fc`): the id is the slot index in the client's `anchorPoints[9]`.
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
    /// The point-id (0..=8), which is also the `anchorPoints[9]` slot index.
    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Build a [`Point`] from its client id, `None` if out of range.
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

/// An opaque handle to an anchor target. This crate does not own the frame arena; the caller maps
/// handles to frames / external rects. (In the client this is the `CAnchor+0xc relativeTo` pointer.)
pub type Handle = u32;

/// A resolved rect in the client's `[bottom, left, top, right]` convention (screen pixels, y-up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub bottom: f32,
    pub left: f32,
    pub top: f32,
    pub right: f32,
}

impl Rect {
    /// Construct from the four edges in client order.
    pub const fn new(bottom: f32, left: f32, top: f32, right: f32) -> Rect {
        Rect {
            bottom,
            left,
            top,
            right,
        }
    }

    /// `[bottom, left, top, right]` — the client's cached-rect layout (`+0x64..0x70`).
    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.bottom, self.left, self.top, self.right]
    }

    /// From `[bottom, left, top, right]`.
    #[inline]
    pub const fn from_array(a: [f32; 4]) -> Rect {
        Rect::new(a[0], a[1], a[2], a[3])
    }

    /// `right − left` (`GetWidth` semantics).
    #[inline]
    pub fn width(self) -> f32 {
        self.right - self.left
    }

    /// `top − bottom` (`GetHeight` semantics; y-up).
    #[inline]
    pub fn height(self) -> f32 {
        self.top - self.bottom
    }
}

/// One anchor of a frame — a `CAnchor` record
/// (`{xOff@4, yOff@8, relativeTo@0xc, relativePoint@0x10}`). `point` is the point *on this frame*
/// being pinned (its `anchorPoints[9]` slot); `relative_point` is the point *on the target* it
/// pins to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// The point on *this* frame that is being anchored (its `anchorPoints[9]` slot index).
    pub point: Point,
    /// The target frame/region this anchor is relative to (`CAnchor+0xc`).
    pub relative_to: Handle,
    /// The point on the *target* this anchor pins to (`CAnchor+0x10 relativePoint`).
    pub relative_point: Point,
    /// X offset (`CAnchor+4`), scaled by the requesting frame's `scale`.
    pub x_off: f32,
    /// Y offset (`CAnchor+8`), scaled by `scale`.
    pub y_off: f32,
}

impl Anchor {
    /// Convenience constructor.
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

/// A frame's full layout input: its anchors, its optional explicit size, its scale, and the
/// clamp-to-screen flag + extents. Mirrors the client's geometry subobject fields consulted by the
/// resolver: `anchorPoints[9]` (`G+0x28`), `width`
/// (`G+0x50`), `height` (`G+0x54`), `layoutScale` (`G+0x58`), flags bit4 = clamp (`G+0x3c`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutInput {
    /// The frame's anchors. Placed into the nine `anchorPoints` slots by their `point`; if two share
    /// a `point`, the *last* wins (the client's `SetPoint 0x767c70` overwrites the slot).
    pub anchors: Vec<Anchor>,
    /// Explicit width (`G+0x50`); **`0.0` ⇒ derive from opposing anchors** (the no-explicit-size
    /// sentinel `DAT_007ffd74`).
    pub width: f32,
    /// Explicit height (`G+0x54`); `0.0` ⇒ derive from opposing anchors.
    pub height: f32,
    /// `layoutScale` = effective scale (`parentScale · ownScale`, ε-gated; `0x76ac90`); the
    /// px↔coord multiplier applied to offsets and the size span. Default `1.0`.
    pub scale: f32,
    /// Clamp the assembled rect into `[0, extent]` per axis (`G` flags bit4; `assemble 0x767a20`).
    pub clamp: bool,
    /// Screen X extent, only consulted when `clamp` (client `0x41ae60` return, default `0.8`).
    pub extent_x: f32,
    /// Screen Y extent, only consulted when `clamp` (client `0x41ae70` return, default `0.6`).
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

/// The outcome of resolving one frame's rect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResolveOutcome {
    /// Every edge resolved → the cached rect `[bottom, left, top, right]` (`assemble` returned 1).
    Resolved(Rect),
    /// At least one edge stayed `+Inf` — under-constrained (`assemble 0x767a20` returns 0, caller
    /// sets flags bit3 "resolution-failed").
    Unresolvable,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Leaf float kernels — the owned-fidelity math. Each returns its coordinate in `f64` (the
// un-narrowed x87 `st0` observable); callers narrow to `f32` at the documented store sites.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// `combineEdge 0x767440(center, opp, span)`: resolve one edge from the opposite
/// edge, the axis center, and the signed `size·scale` span, in that fallback order.
///
/// - opposite known and non-zero span → `opp + span` (edge = opposite ± size·scale);
/// - else center known: non-zero span → `span·0.5 + center` (edge = center ± half-size);
///   else opposite known → `(center − opp) + center` (= 2·center − opposite, the mirror);
/// - else → `+Inf` (unresolvable).
///
/// The `+Inf`/`0.0` sentinels are compared with IEEE equality, matching the binary's `fcomp`
/// against the `.data` slots (a NaN operand is unordered ⇒ "not equal to the sentinel").
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

/// `combineCenter 0x7672d0(lo, hi, span)`: resolve an axis center from the low edge,
/// the high edge, and the signed `size·scale` span.
///
/// - low known: high known → `(lo+hi)·0.5`; else non-zero span → `span·0.5 + lo`;
/// - else high known and non-zero span → `hi − span·0.5`;
/// - else → `+Inf`.
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

/// `CAnchor::ResolveX 0x7a2f90(scale)`: resolve this anchor's X coordinate against its
/// `relativeTo`'s cached rect.
///
/// `rect` = the target's cached rect `[bottom, left, top, right]` when `GetRect` succeeded (cache
/// valid), `None` when it failed ⇒ `+Inf`. `mirror` = the target's geometry-vtable `+0x24`
/// normalize predicate (RTL/local-origin): when set, X edges are translated so `left → 0`,
/// `right → right − left` (each stored back as `f32`, the binary's `fstp m32`). For an ordinary
/// frame the predicate returns 0, so `mirror` is `false` (`0x46ff60 = xor eax,eax; ret`).
/// `coord = xOff·scale + edge` for the relativePoint's column
/// (center = mid-X + xOff·scale). `rel_point > 8` ⇒ `+Inf`.
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

/// `CAnchor::ResolveY 0x7a3070(scale)` — the Y twin of [`anchor_resolve_x`]. `mirror`
/// normalizes the Y edges (`bottom → 0`, `top → top − bottom`); `coord = yOff·scale + edge` for the
/// relativePoint's row (center = mid-Y + yOff·scale). `rect` = `[bottom, left, top, right]`.
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

/// The assembled (and possibly clamped) rect + whether every edge resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssembleRect {
    pub ok: bool,
    /// `[bottom, left, top, right]` (meaningful only when `ok`).
    pub rect: [f32; 4],
}

/// `assemble 0x767a20`: given the four resolved edge coordinates `[bottom, left, top,
/// right]` (each `+Inf` ⇒ unresolvable), fail if any is `+Inf`; else optionally clamp into
/// `[0, extent]` per axis.
///
/// Clamp (geom flags bit4): a low edge `< 0` shifts to 0 carrying the span to the high edge; a high
/// edge past the screen extent shifts back carrying to the low edge, high edge = extent. Each store
/// is narrowed to `f32`.
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

/// `OnSizeChanged` fire test (`ApplyRect 0x76b580`): fires iff `|Δwidth| ≥ [`SIZE_EPS`]`
/// OR `|Δheight| ≥ SIZE_EPS`, with `width = right − left`, `height = top − bottom`. Deltas are `f64`
/// of the `f32` edges (the binary's `fsub` of the `fld`'d rect components).
pub fn size_changed(old: Rect, new: Rect) -> bool {
    let dw =
        (f64::from(new.right) - f64::from(new.left)) - (f64::from(old.right) - f64::from(old.left));
    let dh =
        (f64::from(new.top) - f64::from(new.bottom)) - (f64::from(old.top) - f64::from(old.bottom));
    dw.abs() >= SIZE_EPS || dh.abs() >= SIZE_EPS
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The composed resolver — recursion-guarded per-edge sequencing over the leaves above,
// bit-exact over the binary's `assemble 0x767a20` → six per-edge resolvers → anchor scans → leaf
// kernels. Adds NO new arithmetic — only integer control flow + the documented `fstp m32` `f32`
// narrowing at each site.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A resolved anchor for the internal edge tree: like [`Anchor`] but with the target's rect already
/// looked up (`None` ⇒ target had no valid rect ⇒ `GetRect`-fail leg).
#[derive(Clone, Copy)]
struct AnchorRel {
    x_off: f32,
    y_off: f32,
    rel_point: u32,
    rel_rect: Option<[f32; 4]>,
    mirror: bool,
}

/// Internal per-edge resolver input: the nine `anchorPoints` slots (by point-id) with rects
/// resolved, plus the size/scale/clamp fields.
struct EdgeInput {
    anchors: [Option<AnchorRel>; 9],
    width: f32,
    height: f32,
    scale: f32,
    clamp: bool,
    extent_x: f32,
    extent_y: f32,
}

// recursion-guard bits (the binary's `G+0x28` bitfield).
const G_LEFT: u32 = 0x01;
const G_TOP: u32 = 0x02;
const G_RIGHT: u32 = 0x04;
const G_BOTTOM: u32 = 0x08;
const G_XC: u32 = 0x10;
const G_YC: u32 = 0x20;

// per-edge point-id scan tables (`.rdata` 0x81c3b8..0x81c3f4).
const IDS_LEFT: [usize; 3] = [0, 3, 6];
const IDS_RIGHT: [usize; 3] = [2, 5, 8];
const IDS_TOP: [usize; 3] = [0, 1, 2];
const IDS_BOTTOM: [usize; 3] = [6, 7, 8];
const IDS_XC: [usize; 3] = [1, 4, 7];
const IDS_YC: [usize; 3] = [3, 4, 5];

/// `anchorScanX 0x7671a0`: over the edge's three point-ids, the first present anchor whose
/// `ResolveX` is non-`+Inf` wins; else `+Inf`.
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

/// `anchorScanY 0x7671f0` — the Y twin.
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

/// `width·scale` / `height·scale` — the per-edge resolvers' shared span step (`LEFT 0x7673d0` et
/// al.): operands are `f32` fields, the product narrowed to `f32` at the `fstp [esp]` site,
/// negated for LEFT/BOTTOM.
///
/// **`size` is not always the authored field, and this is the trap**: each
/// caller's `call [eax+0x1c]` is **virtual**. A plain frame lands on
/// `0x768420 fld [G+0x50]` — the flat read this signature assumes — but a `CSimpleTexture` lands on
/// `0x770720` and a `CSimpleFontString` on `0x772930`, both of which substitute a **content-derived**
/// extent when the authored value is exactly `0.0` (the texture's own texel size, one texel per
/// FrameXML unit; the string's measured text, floored at one unit). benilla feeds this the authored
/// number only, so a zero-authored *texture* axis yields a zero span here where the client yields a
/// real one. 1349 §4 carries the change and its scope; do not read this kernel as the whole law.
fn size_span(size: f32, scale: f32, negate: bool) -> f32 {
    let span = (f64::from(size) * f64::from(scale)) as f32;
    if negate {
        -span
    } else {
        span
    }
}

/// `LEFT 0x7673d0` — direct anchor scan, else `combineEdge(Xcenter, RIGHT, −width·scale)`.
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

/// `RIGHT 0x767540` — direct anchor scan, else `combineEdge(Xcenter, LEFT, +width·scale)`.
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

/// `TOP 0x7674d0` — direct anchor scan, else `combineEdge(Ycenter, BOTTOM, +height·scale)`.
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

/// `BOTTOM 0x7675b0` — direct anchor scan, else `combineEdge(Ycenter, TOP, −height·scale)`.
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

/// `Xcenter 0x767260` — direct anchor scan, else `combineCenter(LEFT, RIGHT, +width·scale)`.
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

/// `Ycenter 0x767360` — direct anchor scan, else `combineCenter(BOTTOM, TOP, +height·scale)`.
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

/// `assemble 0x767a20` composed: resolve all four edges (each `fst`'d to the `f32` out-rect),
/// returning [`ResolveOutcome::Unresolvable`] if any stayed `+Inf`, else the assembled (and, when
/// `clamp`, screen-clamped) rect. The four top-level resolves share a fresh guard; the binary
/// clears each edge's bit on return, so order is immaterial — we keep the binary's LEFT→BOTTOM→
/// RIGHT→TOP order. The `+Inf` test is on the un-narrowed `f64` edge (the binary's `fcomp` before
/// the `fst`).
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

/// Resolve one frame's rect. `target_rect` supplies each anchor target's already-resolved rect
/// (`None` ⇒ the target has no valid rect ⇒ that anchor contributes `+Inf`, the `GetRect`-fail
/// leg). Anchors are placed into the nine `anchorPoints` slots by their `point`; on a duplicate the
/// later anchor wins (the client's `SetPoint` overwrite).
///
/// This is the fidelity surface: it builds the client's `anchorPoints[9]` view and runs the
/// composed `assemble 0x767a20`.
pub fn resolve_rect(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> ResolveOutcome {
    resolve_edges(&build_edge_input(frame, target_rect))
}

/// Build the client's `anchorPoints[9]` view + size/scale/clamp fields for `frame`, resolving each
/// anchor target's rect through `target_rect`. Shared by [`resolve_rect`] (all-or-nothing) and
/// [`resolve_rect_edges`] (per-edge).
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
            // Ordinary frames' normalize predicate returns 0 (`0x46ff60`); RTL/mirror
            // frames are not modeled here (the leaf `anchor_resolve_*` covers that leg).
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

/// Resolve one frame's four edges, returning each as `Some(edge)` when the anchor graph + explicit
/// size determined it, or `None` when it stayed `+Inf` (unset). The per-edge view **regions** need:
/// a region inherits any edge its anchors + size don't pin from its owner frame (decision 0068's v1
/// region model — see [`crate::script`]'s region resolution), so it must see *which* edges resolved,
/// not the all-or-nothing verdict [`resolve_rect`] gives. Order matches [`Rect`]: `[bottom, left,
/// top, right]`. No `assemble`/clamp is applied (regions never clamp to screen in v1).
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Graph driver — resolve a set of frames in dependency order (anchor targets before dependents),
// detecting cycles. This is benilla's orchestration, not client fidelity math: the client resolves
// lazily-on-read + per-frame-batched via a dependency-ordered deferred queue (`0x7680e0`, ordered
// by `0x7681b0`); we express the same dependency ordering as an explicit topological pass.
//
// ## Why a dense, reusable solver rather than a fresh map-backed graph
//
// The caller re-solves the whole widget set every round of its fixpoint, and the fixpoint runs
// every frame. A map-backed graph rebuilt per round paid, per round: one `LayoutInput` clone
// (heap `Vec<Anchor>`) per frame, a `BTreeMap` insert per node, and a `BTreeMap` probe per anchor
// target during the solve. Measured on the shipped default UI (1881 frames + 5478 regions,
// release): 1.36 ms per resolve, of which ~0.25 ms was graph construction and ~0.64 ms the solve
// itself — paid on a completely quiet frame.
//
// [`Handle`]s are the caller's own dense id space (benilla's `Model::next_id`, monotonic from 1
// with frames and regions sharing the counter), so every per-node map collapses to an array index
// and every buffer can live across calls. `begin` clears only the slots the previous round
// touched, so a round costs O(nodes), never O(id space). Iteration order is kept **ascending by
// handle** — identical to the `BTreeMap` it replaces — because cycle members are resolved
// best-effort in that order and the result depends on it.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A reusable dependency solver over a set of anchored frames plus *external* pre-resolved rects
/// (the screen root — the client's `CSimpleTop`, whose rect is the physical screen — and any fixed
/// targets, such as already-resolved regions).
///
/// One solver is kept alive across frames so its buffers are reused. A round is:
/// [`begin`](Self::begin) → [`set_external`](Self::set_external) /
/// [`set_frame`](Self::set_frame) → [`solve`](Self::solve) → [`rect`](Self::rect).
#[derive(Clone, Debug, Default)]
pub struct LayoutSolver {
    /// Per-handle layout input. Persistent storage: [`set_frame`](Self::set_frame) overwrites in
    /// place so the anchors `Vec` keeps its allocation across rounds.
    input: Vec<LayoutInput>,
    /// Is `input[h]` a frame to solve this round?
    is_frame: Vec<bool>,
    /// Per-handle rect: externals as seeded, frames as they resolve. `None` = not (yet) known.
    rect: Vec<Option<Rect>>,
    /// Kahn in-degree, and the reverse edges (each inner `Vec` is cleared, never freed).
    indeg: Vec<u32>,
    dependents: Vec<Vec<Handle>>,
    /// Handles whose slots were written this round — `begin` resets exactly these.
    touched: Vec<Handle>,
    /// Is `h` already in `touched`?
    marked: Vec<bool>,
    /// The frames to solve, ascending (the `BTreeMap` iteration order this replaces).
    live: Vec<Handle>,
    /// Kahn scratch + output.
    queue: VecDeque<Handle>,
    emitted: Vec<bool>,
    order: Vec<Handle>,
    cycle: Vec<Handle>,
    unresolvable: Vec<Handle>,
}

impl LayoutSolver {
    /// A new, empty solver.
    pub fn new() -> LayoutSolver {
        LayoutSolver::default()
    }

    /// Grow the per-handle arrays to cover `h`.
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

    /// Mark `h` as written this round (so [`begin`](Self::begin) will reset its slot).
    fn touch(&mut self, h: Handle) {
        let i = h as usize;
        if !self.marked[i] {
            self.marked[i] = true;
            self.touched.push(h);
        }
    }

    /// Start a round: reset every slot the previous round touched (O(nodes), not O(id space)).
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

    /// Seed a pre-resolved rect at `h` — an anchor target this solve does not compute (the screen
    /// root, an already-resolved region). Also used to publish a rect mid-sweep so later nodes in
    /// the same pass see it.
    pub fn set_external(&mut self, h: Handle, rect: Rect) {
        self.ensure(h);
        self.touch(h);
        self.rect[h as usize] = Some(rect);
    }

    /// Register a frame to solve, copying `src` into the solver's persistent slot (the anchors
    /// `Vec` is refilled in place, so a steady round allocates nothing).
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

    /// [`set_frame`](Self::set_frame) with `src`'s anchors replaced by exactly `anchor` — the
    /// ScrollFrame child override, which otherwise cost a full input clone plus a
    /// one-element `Vec` per scroll child per round.
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

    /// The non-anchor fields, copied verbatim.
    fn copy_fields(dst: &mut LayoutInput, src: &LayoutInput) {
        dst.width = src.width;
        dst.height = src.height;
        dst.scale = src.scale;
        dst.clamp = src.clamp;
        dst.extent_x = src.extent_x;
        dst.extent_y = src.extent_y;
    }

    /// Resolve every registered frame in dependency order (each anchor target before its
    /// dependents), detecting cycles. Cycle members are still resolved best-effort, ascending by
    /// handle, against whatever rects were available — matching the map-backed driver this
    /// replaces.
    pub fn solve(&mut self) {
        self.live.sort_unstable();

        // Frame→frame anchor edges. A dependency on an external or an unknown handle does not
        // constrain ordering (its rect is fixed / missing, not produced by this pass); only
        // frame→frame edges do. A self-anchor counts as an edge, so a self-referential frame is
        // never emitted ⇒ reported as a cycle. Duplicate targets count once (an anchor slot set is
        // at most 9, so the dedupe is a linear scan, not a set).
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

        // Frames never emitted are on (or behind) a cycle; resolve them best-effort, ascending.
        for idx in 0..self.live.len() {
            let h = self.live[idx];
            if !self.emitted[h as usize] {
                self.cycle.push(h);
                self.resolve_one(h);
            }
        }
    }

    /// Resolve one frame against the rects known so far, recording failure.
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

    /// The rect known at `h`: an external as seeded, or a frame's resolved rect. `None` if the
    /// handle is unknown or its frame was under-constrained.
    #[inline]
    pub fn rect(&self, h: Handle) -> Option<Rect> {
        self.rect.get(h as usize).copied().flatten()
    }

    /// Frames on (or behind) an anchor dependency cycle — no valid topological position. Ascending.
    pub fn cycle(&self) -> &[Handle] {
        &self.cycle
    }

    /// Frames whose `assemble` returned unresolvable (under-constrained, or depending on an
    /// unresolvable/missing target). May overlap [`cycle`](Self::cycle).
    pub fn unresolvable(&self) -> &[Handle] {
        &self.unresolvable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Screen-root handle used across tests; its rect is the physical screen (y-up).
    const SCREEN: Handle = 0;
    fn screen_rect() -> Rect {
        // [bottom, left, top, right]
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
    // Expected outputs are hand-computed from the formulas above — this crate has no `WoW.exe`
    // emulator, so these are spec-derived, not binary-diffed.

    #[test]
    fn combine_edge_legs() {
        // (center, opp, span)
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
                                               // relLeft, center, relRight (point-ids 0,1,2), scale 1, xOff 5
        assert_eq!(anchor_resolve_x(1.0, 5.0, 0, Some(r), false), 5.0 + 100.0);
        assert_eq!(
            anchor_resolve_x(1.0, 5.0, 1, Some(r), false),
            (260.0 + 100.0) * 0.5 + 5.0
        );
        assert_eq!(anchor_resolve_x(1.0, 5.0, 2, Some(r), false), 5.0 + 260.0);
        // scale multiplies the offset
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

    /// A numeric oracle for `assemble 0x767a20`, confirmed against a running `WoW.exe`.
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
        // screen root [0,0,600,800] (probe uses root top=600, right=800)
        let got = resolve_rect(&f, |_| Some(Rect::new(0.0, 0.0, 600.0, 800.0)));
        assert_rect(got, Rect::new(545.0, 10.0, 595.0, 210.0));
    }

    #[test]
    fn single_anchor_all_nine_points_explicit_size() {
        // For each own-point p, anchor p -> the same point on screen, xOff/yOff 0, w100 h40.
        // The anchored corner/edge pins directly; the rest derive from size. We assert the pinned
        // coordinate and the resulting width/height, computed by hand from the formulas above.
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
            // width/height always preserved
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
        // scale 2 -> offsets and size span doubled.
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
        // Anchor to a target with no valid rect (GetRect fail) -> that edge is +Inf -> frame
        // unresolvable (single anchor, so opposite edges can't derive).
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
        // neither can resolve (each depends on the other's rect, unavailable)
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
        // sub-epsilon width change -> not fired
        let tiny = Rect::new(0.0, 0.0, 40.0, 100.0 + 1e-7);
        assert!(!size_changed(r, tiny));
        // supra-epsilon -> fired
        let big = Rect::new(0.0, 0.0, 40.0, 100.001);
        assert!(size_changed(r, big));
    }

    /// The solver's buffers live across rounds, so `begin` must leave NO trace of the previous
    /// one — a stale rect, in-degree, dependent edge, or `is_frame` flag would silently corrupt
    /// the next solve. Round 2 registers a *disjoint* frame set that anchors to a handle round 1
    /// used, and must see it as unknown (not round 1's rect).
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

        // Round 2: ONLY B, still anchored to A — but A is no longer registered and no external
        // seeds it, so B must be unresolvable rather than reusing round 1's stale A rect.
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

        // Round 3: the original pair again — identical results to round 1 (no accumulated
        // in-degree or dependent edges from the rounds between).
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

    /// A frame registered with the ScrollFrame override keeps its size/scale but drops its own
    /// anchors for exactly the override — and the slot's anchor `Vec` is reused,
    /// so the previous round's anchors must not leak through.
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

        // Overridden: the two authored anchors are gone, replaced by one TOPLEFT pin at (10, -5),
        // so the explicit 100x40 size now decides the other two edges.
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
