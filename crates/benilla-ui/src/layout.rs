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
/// `0x8029d4`, tested in `ApplyRect` (`0x76b5e7`, `0x76b602`) in the reference's layout units, the
/// screen diagonal as 1; in benilla's units that is this times the layout unit
/// (`Model::size_eps`).
pub const SIZE_EPS: f64 = 2.384_185_791_015_625e-7;

/// The least edge move against the cached rect for the drain's resolve to store the new rect and
/// `ApplyRect`, 1e-5 (`0x3727c5ac` at `0x80c5c8`, tested in `0x768d20`), in the reference's layout
/// units; in benilla's, this times the layout unit (`Model::resolve_eps`). See [`keeps_rect`].
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

    /// Bit-exact equality, as the layout gate's fingerprint compares, so the two always agree.
    pub fn bits_eq(&self, other: &Anchor) -> bool {
        self.point == other.point
            && self.relative_to == other.relative_to
            && self.relative_point == other.relative_point
            && self.x_off.to_bits() == other.x_off.to_bits()
            && self.y_off.to_bits() == other.y_off.to_bits()
    }
}

/// The client's aspect globals, both written only by `0x41ad10(a)`: `[0x832a4c] = f32(a · 0.75)`
/// (`0x41ad24`) and `[0x832a44] = f32(a / √(a² + 1))`, the quotient kept unrounded before its
/// multiply (`0x41ad3e` `fst`, `0x41ad44`, `0x41ad47`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutUnits {
    g3: f32,
    g44: f32,
}

impl LayoutUnits {
    /// From the screen aspect `a`, width over height.
    pub fn from_aspect(a: f32) -> LayoutUnits {
        let a64 = f64::from(a);
        LayoutUnits {
            g3: (a64 * 0.75) as f32,
            g44: ((1.0 / (a64 * a64 + 1.0).sqrt()) * a64) as f32,
        }
    }

    /// A Lua offset as `SetPoint`'s binding stores it, in layout units (`0x7a28a3`..`0x7a28e3`):
    /// `f32(f32(v / ([0x832a4c] · 1024)) · [0x832a44])`.
    pub fn store(self, v: f32) -> f32 {
        let q = (f64::from(v) / (f64::from(self.g3) * 1024.0)) as f32;
        (f64::from(self.g44) * f64::from(q)) as f32
    }

    /// A stored length as a getter pushes it, the inverse of [`Self::store`]
    /// (`GetPushedTextOffset` `0x782237`..`0x782251`): `f32(g3 · 1024 · v) / g44` as an f64.
    pub fn load(self, v: f32) -> f64 {
        let q = (f64::from(self.g3) * 1024.0 * f64::from(v)) as f32;
        f64::from(q) / f64::from(self.g44)
    }
}

/// A region's anchors, one slot per point: the client's `anchorPoints[9]` (`G+0x4`, a frame's
/// `+0x28`), indexed by point id. Setting a point replaces its slot (`SetPoint` stores at
/// `0x767d33`), so the order anchors were set in is not kept, and every walk runs in point order,
/// TOPLEFT to BOTTOMRIGHT, as `GetPoint` (`0x7a23f5`) and `GetNumPoints` (`0x7a2311`) walk them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchors([Option<Anchor>; 9]);

impl Anchors {
    pub const fn new() -> Anchors {
        Anchors([None; 9])
    }

    /// The anchor held for `point`.
    #[inline]
    pub fn get(&self, point: Point) -> Option<&Anchor> {
        self.0[point.id() as usize].as_ref()
    }

    /// Store `a` in its point's slot, returning the anchor it replaces.
    #[inline]
    pub fn insert(&mut self, a: Anchor) -> Option<Anchor> {
        self.0[a.point.id() as usize].replace(a)
    }

    /// `SetPoint`'s store (`0x767c70`): the anchor held for `a.point` stands when it has the same
    /// target (`0x767ca9`) and relative point (`0x767cb1`) and both offsets, as the client stores
    /// them ([`LayoutUnits::store`]), differ by less than 2⁻²² (`0x767cb6`..`0x767cde`), and the
    /// call then stores nothing; else `a` replaces it. Returns whether it stored. A NaN offset
    /// never matches, as the client's unordered compare.
    pub fn set(&mut self, a: Anchor, units: LayoutUnits) -> bool {
        let near = |p: f32, q: f32| {
            (f64::from(units.store(p)) - f64::from(units.store(q))).abs() < SIZE_EPS
        };
        let unchanged = self.get(a.point).is_some_and(|h| {
            h.relative_to == a.relative_to
                && h.relative_point == a.relative_point
                && near(h.x_off, a.x_off)
                && near(h.y_off, a.y_off)
        });
        if !unchanged {
            self.insert(a);
        }
        !unchanged
    }

    /// Empty every slot, returning whether any was held (`ClearAllPoints`, `0x767620`).
    #[inline]
    pub fn clear(&mut self) -> bool {
        let held = !self.is_empty();
        self.0 = [None; 9];
        held
    }

    /// Empty every slot naming a target in `dying`, returning whether any did: the unhook a dying
    /// layout object runs on each dependent (`0x767e70`, from `0x768e20`).
    #[inline]
    pub fn release_targets(&mut self, dying: &[u32]) -> bool {
        let mut freed = false;
        for slot in &mut self.0 {
            if slot.is_some_and(|a| dying.contains(&a.relative_to)) {
                *slot = None;
                freed = true;
            }
        }
        freed
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }

    /// The occupied slots, `GetNumPoints`'s count.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.iter().filter(|s| s.is_some()).count()
    }

    /// The held anchors in point order.
    #[inline]
    pub fn iter(&self) -> std::iter::Flatten<std::slice::Iter<'_, Option<Anchor>>> {
        self.0.iter().flatten()
    }

    /// The offsets of the anchor held for `point`; the point is not writable here, so an anchor
    /// never leaves its slot.
    #[inline]
    pub fn offset_mut(&mut self, point: Point) -> Option<(&mut f32, &mut f32)> {
        self.0[point.id() as usize]
            .as_mut()
            .map(|a| (&mut a.x_off, &mut a.y_off))
    }

    /// The `n`-th held anchor in point order, from 0.
    #[inline]
    pub fn nth(&self, n: usize) -> Option<&Anchor> {
        self.iter().nth(n)
    }

    /// The first held anchor in point order.
    #[inline]
    pub fn first(&self) -> Option<&Anchor> {
        self.iter().next()
    }

    /// The targets of the anchors in `reads`, a bit per point id, in point order.
    #[inline]
    pub fn read_targets(&self, reads: u16) -> impl Iterator<Item = Handle> + '_ {
        self.0
            .iter()
            .enumerate()
            .filter(move |(id, _)| reads & (1 << id) != 0)
            .filter_map(|(_, a)| a.map(|a| a.relative_to))
    }

    /// Slot-for-slot [`Anchor::bits_eq`].
    pub fn bits_eq(&self, other: &Anchors) -> bool {
        self.0.iter().zip(&other.0).all(|(a, b)| match (a, b) {
            (Some(a), Some(b)) => a.bits_eq(b),
            (None, None) => true,
            _ => false,
        })
    }
}

/// Each anchor into its point's slot, a later one on the same point replacing an earlier one.
impl FromIterator<Anchor> for Anchors {
    fn from_iter<I: IntoIterator<Item = Anchor>>(iter: I) -> Anchors {
        let mut out = Anchors::new();
        for a in iter {
            out.insert(a);
        }
        out
    }
}

impl<const N: usize> From<[Anchor; N]> for Anchors {
    fn from(a: [Anchor; N]) -> Anchors {
        a.into_iter().collect()
    }
}

impl<'a> IntoIterator for &'a Anchors {
    type Item = &'a Anchor;
    type IntoIter = std::iter::Flatten<std::slice::Iter<'a, Option<Anchor>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A frame's layout input, the geometry fields the client's resolver reads: `anchorPoints[9]`
/// (`G+0x4`), `width` (`G+0x50`), `height` (`G+0x54`), `layoutScale` (`G+0x58`) and the clamp flag
/// (mask `0x10` of `G+0x3c`, `0x768ccd`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutInput {
    pub anchors: Anchors,
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
            anchors: Anchors::new(),
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
    pub fn sized(
        anchors: impl IntoIterator<Item = Anchor>,
        width: f32,
        height: f32,
    ) -> LayoutInput {
        LayoutInput {
            anchors: anchors.into_iter().collect(),
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

/// `assemble` (`0x767a20`): fails if any edge is `+Inf`, else with `clamp` (flags `0x10`) shifts
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

/// `ApplyRect`'s `OnSizeChanged` test (`0x76b580`): width or height moved by at least `eps`,
/// [`SIZE_EPS`] in the units the rects are in, the deltas taken in `f64` from the `f32` edges.
pub fn size_changed(old: Rect, new: Rect, eps: f64) -> bool {
    let dw =
        (f64::from(new.right) - f64::from(new.left)) - (f64::from(old.right) - f64::from(old.left));
    let dh =
        (f64::from(new.top) - f64::from(new.bottom)) - (f64::from(old.top) - f64::from(old.bottom));
    dw.abs() >= eps || dh.abs() >= eps
}

/// The drain's keep test (`0x768d20`, `0x768d6e`–`0x768dc0`): every edge of `new` moved less than
/// `eps` from the cached rect `old`, the deltas taken in `f64` from the `f32` edges. A kept rect is
/// not stored and `ApplyRect` does not run (the return at `0x768dc2`); an edge at `eps` or past it,
/// or a NaN, applies.
pub fn keeps_rect(old: Rect, new: Rect, eps: f64) -> bool {
    let near = |a: f32, b: f32| (f64::from(a) - f64::from(b)).abs() < eps;
    near(new.left, old.left)
        && near(new.right, old.right)
        && near(new.bottom, old.bottom)
        && near(new.top, old.top)
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
    /// The point ids whose anchor a scan read, a bit each: the anchors whose `ResolveX` or
    /// `ResolveY` ran, each of which resolves a deferred target first (`0x7a2fb8`, `0x7a3098`).
    reads: std::cell::Cell<u16>,
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
            f.reads.set(f.reads.get() | 1 << id);
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
            f.reads.set(f.reads.get() | 1 << id);
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

/// [`resolve_rect`] with the anchors its scans read, a bit per point id ([`Anchors::read_targets`]).
/// A scan stops at the first anchor that resolves (`0x7671a0`), so an anchor behind it is read only
/// if another edge's scan reaches it.
pub fn resolve_rect_reads(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> (ResolveOutcome, u16) {
    let input = build_edge_input(frame, target_rect);
    let outcome = resolve_edges(&input);
    (outcome, input.reads.get())
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
        reads: std::cell::Cell::new(0),
    }
}

/// Resolve one frame's edges, `[bottom, left, top, right]`, each `None` where it stayed unset;
/// `assemble` and its clamp are not applied. The region pass reads it axis by axis.
pub fn resolve_rect_edges(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> [Option<f32>; 4] {
    resolve_rect_edges_reads(frame, target_rect).0
}

/// [`resolve_rect_edges`] with the anchors its scans read, as [`resolve_rect_reads`].
#[allow(clippy::float_cmp)]
pub fn resolve_rect_edges_reads(
    frame: &LayoutInput,
    target_rect: impl FnMut(Handle) -> Option<Rect>,
) -> ([Option<f32>; 4], u16) {
    let input = build_edge_input(frame, target_rect);
    let mut guard = 0u32;
    let left = resolve_left(&input, &mut guard);
    let bottom = resolve_bottom(&input, &mut guard);
    let right = resolve_right(&input, &mut guard);
    let top = resolve_top(&input, &mut guard);
    let cvt = |v: f64| if v == UNSET { None } else { Some(v as f32) };
    (
        [cvt(bottom), cvt(left), cvt(top), cvt(right)],
        input.reads.get(),
    )
}

// ── Graph driver ─────────────────────────────────────────────────────────────────────────────

/// Resolves anchored frames in dependency order, a driver of benilla's own: the client resolves
/// lazily through a dependency-ordered deferred queue (`0x7680e0`, ordered by `0x7681b0`). Handles
/// index its arrays directly, so they must be dense. A round: [`begin`](Self::begin),
/// [`set_external`](Self::set_external) and [`set_frame`](Self::set_frame), [`solve`](Self::solve).
#[derive(Clone, Debug, Default)]
pub struct LayoutSolver {
    /// Overwritten in place, so a steady round allocates nothing.
    input: Vec<LayoutInput>,
    is_frame: Vec<bool>,
    /// Externals as seeded, frames as they resolve.
    rect: Vec<Option<Rect>>,
    /// Per node, the rect it held when this resolve began, its valid bit set, else `None`
    /// ([`set_entry`](Self::set_entry)): what [`keeps_rect`] holds a new rect against, what a node
    /// the resolve need not revisit keeps, and what a target's change is measured from.
    cached: Vec<Option<Rect>>,
    /// Per node, whether [`set_entry`](Self::set_entry) gave its entry state. A frame without one
    /// is solved from its inputs alone, as if invalidated.
    has_entry: Vec<bool>,
    /// Per frame, whether it was invalidated (`0x7680e0`) since its entry state.
    pending: Vec<bool>,
    /// Per node, its deferral flag for this solve (`0x2` of `G+0x3c`, [`set_deferral`]): it keeps
    /// its entry state, and an invalidate that reaches it passes on instead.
    ///
    /// [`set_deferral`]: Self::set_deferral
    deferred: Vec<bool>,
    /// Per node, pending under its deferral at entry (`0x4` with `0x2`): an invalidate reaching it
    /// only writes the drain's retry counter, `+0x38 = 6` (`0x76814b`–`0x768166`).
    held: Vec<bool>,
    /// Per node, whether its invalidate passes to what anchors to it in this solve, each of which
    /// is then reached (`0x768178`–`0x76819c`).
    passes: Vec<bool>,
    /// The deferred nodes this solve reached, now pending and passing on.
    newly_held: Vec<Handle>,
    /// `(reader, target)` for each anchor a resolving node read whose target is deferred: the read
    /// resolves the target first (`0x7a2fb8` → `0x768060(0)`), which the caller does in its next
    /// round.
    deferred_reads: Vec<(Handle, Handle)>,
    /// The keep test's threshold in the rects' units; 0 keeps nothing.
    keep_eps: f64,
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
            self.cached.resize(need, None);
            self.has_entry.resize(need, false);
            self.pending.resize(need, false);
            self.deferred.resize(need, false);
            self.held.resize(need, false);
            self.passes.resize(need, false);
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
            self.cached[i] = None;
            self.has_entry[i] = false;
            self.pending[i] = false;
            self.deferred[i] = false;
            self.held[i] = false;
            self.passes[i] = false;
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
        self.newly_held.clear();
        self.deferred_reads.clear();
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
        dst.anchors = src.anchors;
        Self::copy_fields(dst, src);
        self.is_frame[h as usize] = true;
        self.live.push(h);
    }

    /// The state node `h` was in when this resolve began, its rect while its valid bit is set,
    /// and whether it was invalidated since (`0x7680e0`: its own write, a cleared valid bit).
    ///
    /// The reference resolves only what is invalidated: by its own write, or by a target's
    /// resolve that stored a new rect (`0x768cf0`). A node the resolve does not reach keeps this
    /// state, whatever its inputs now give, as a write that invalidates nothing (`ClearAllPoints`
    /// `0x767620`, a dying target's unhook `0x767e70`, `SetTexture` `0x770200`) leaves it. The
    /// rect is also what the keep test ([`set_keep_eps`](Self::set_keep_eps)) holds a new one
    /// against.
    pub fn set_entry(&mut self, h: Handle, rect: Option<Rect>, invalidated: bool) {
        self.ensure(h);
        self.touch(h);
        let i = h as usize;
        self.cached[i] = rect;
        self.has_entry[i] = true;
        self.pending[i] = invalidated;
    }

    /// Node `h`'s deferral bit for this solve and whether it is pending under it, for a node this
    /// solve resolves, keeps or seeds alike: a seeded one is only read, and a read of it is
    /// reported in [`deferred_reads`](Self::deferred_reads).
    pub fn set_deferral(&mut self, h: Handle, deferred: bool, held: bool) {
        self.ensure(h);
        self.touch(h);
        self.deferred[h as usize] = deferred;
        self.held[h as usize] = held;
    }

    /// Node `h`'s invalidate passes to what anchors to it in this solve: it was invalidated under
    /// its deferral (`0x768178`).
    pub fn set_passes(&mut self, h: Handle) {
        self.ensure(h);
        self.touch(h);
        self.passes[h as usize] = true;
    }

    /// Whether `h` is deferred in this solve.
    #[inline]
    pub fn deferred(&self, h: Handle) -> bool {
        self.deferred.get(h as usize).copied().unwrap_or(false)
    }

    /// A deferred node `h` the resolve reached: an invalidate marks it pending and passes on to its
    /// dependents, once, and queues no resolve (`0x76814b`, `0x768178`–`0x76819c`). It keeps its
    /// entry state.
    pub fn hold_reached(&mut self, h: Handle) {
        let i = h as usize;
        if !self.held[i] && !self.passes[i] {
            self.passes[i] = true;
            self.newly_held.push(h);
        }
    }

    /// Report each anchor of `reader` its resolve read, `reads` a bit per point id, whose target is
    /// deferred.
    pub fn note_reads(&mut self, reader: Handle, anchors: &Anchors, reads: u16) {
        for t in anchors.read_targets(reads) {
            if self.deferred(t) {
                self.deferred_reads.push((reader, t));
            }
        }
    }

    /// The deferred nodes this solve reached ([`hold_reached`](Self::hold_reached)).
    pub fn newly_held(&self) -> &[Handle] {
        &self.newly_held
    }

    /// The reads of deferred targets ([`note_reads`](Self::note_reads)).
    pub fn deferred_reads(&self) -> &[(Handle, Handle)] {
        &self.deferred_reads
    }

    /// Whether `h` now holds a rect it did not hold at entry: a resolve that stored a new rect,
    /// which invalidates what anchors to `h`. A frame given no entry counts any rect as new; an
    /// external given none never changes.
    #[inline]
    pub fn changed(&self, h: Handle) -> bool {
        let i = h as usize;
        let Some(now) = self.rect.get(i).copied().flatten() else {
            return false;
        };
        if self.has_entry[i] {
            self.cached[i] != Some(now)
        } else {
            self.is_frame[i]
        }
    }

    /// Whether the resolve reaches node `h`, by the rule [`set_entry`] documents: it has no entry
    /// state, it was invalidated, or an anchor target stored a new rect or passed on an invalidate
    /// it took while deferred (`0x768178`–`0x76819c`). Only then does it take the rect its inputs
    /// give it, or fail, unless it is deferred itself ([`hold_reached`]). `reads` lists its anchor
    /// targets.
    ///
    /// [`set_entry`]: Self::set_entry
    /// [`hold_reached`]: Self::hold_reached
    pub fn reached(&self, h: Handle, mut reads: impl Iterator<Item = Handle>) -> bool {
        let i = h as usize;
        !self.has_entry.get(i).copied().unwrap_or(false)
            || self.pending[i]
            || reads.any(|t| self.changed(t) || self.passes.get(t as usize) == Some(&true))
    }

    /// The entry rect of node `h`, which it keeps when the resolve does not reach it.
    #[inline]
    pub fn entry(&self, h: Handle) -> Option<Rect> {
        self.cached.get(h as usize).copied().flatten()
    }

    /// The keep test's threshold, [`RESOLVE_EPS`] in the rects' units; kept across rounds.
    pub fn set_keep_eps(&mut self, eps: f64) {
        self.keep_eps = eps;
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
            let anchors = self.input[h as usize].anchors;
            for d in anchors.iter().map(|a| a.relative_to) {
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
        let i = h as usize;
        let anchors = self.input[i].anchors;
        let reached = self.reached(h, anchors.iter().map(|a| a.relative_to));
        if reached && self.deferred[i] {
            self.hold_reached(h);
        }
        if !reached || self.deferred[i] {
            // Not reached, or deferred: the entry state stands, rect or none.
            self.rect[i] = self.cached[i];
            if self.rect[i].is_none() {
                self.unresolvable.push(h);
            }
            return;
        }
        let (outcome, reads) = {
            let input = &self.input[i];
            let rects = &self.rect;
            resolve_rect_reads(input, |t| rects.get(t as usize).copied().flatten())
        };
        self.note_reads(h, &anchors, reads);
        match outcome {
            ResolveOutcome::Resolved(r) => {
                let kept = self.cached[i].filter(|&c| keeps_rect(c, r, self.keep_eps));
                self.rect[i] = Some(kept.unwrap_or(r));
            }
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

    /// `0x41ad10` at 4:3: `[0x832a4c] = 1.0` and `[0x832a44] = 0.8` (as `f32`), so an offset
    /// stores as `f32(f32(v / 1024) · 0.8)`.
    #[test]
    fn layout_units_at_four_thirds() {
        let u = LayoutUnits::from_aspect(4.0 / 3.0);
        assert_eq!(u, LayoutUnits { g3: 1.0, g44: 0.8 });
        assert_eq!(u.store(1280.0), (1.25_f64 * f64::from(0.8_f32)) as f32);
        // `GetPushedTextOffset` of the ctor's 0.001 (`0x3a83126f`).
        assert!((u.load(f32::from_bits(0x3a83_126f)) - 1.28).abs() < 1e-6);
    }

    fn bits(r: Rect) -> [u32; 4] {
        [
            r.bottom.to_bits(),
            r.left.to_bits(),
            r.top.to_bits(),
            r.right.to_bits(),
        ]
    }

    /// One slot per point, walked in point order whatever order the points were set in.
    #[test]
    fn anchors_keep_one_slot_per_point_in_point_order() {
        let br = Anchor::new(Point::BottomRight, SCREEN, Point::BottomRight, -1.0, 1.0);
        let tl = Anchor::new(Point::TopLeft, SCREEN, Point::TopLeft, 1.0, -1.0);
        let tl2 = Anchor::new(Point::TopLeft, 7, Point::Center, 2.0, -2.0);
        let a = Anchors::from([br, tl, tl2]);
        assert_eq!(a.len(), 2);
        assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![tl2, br]);
        assert_eq!(a.nth(1), Some(&br));
        assert_eq!(a.nth(2), None);
    }

    /// `SetPoint`'s compare (`0x767cb6`..`0x767cde`): under the threshold on both axes with the
    /// same target and relative point stands; past it, NaN, or another target or relative point
    /// replaces. At 4:3 a layout unit is 1280 FrameXML units, so 2⁻²² is about 3.05e-4.
    #[test]
    fn anchors_set_keeps_an_anchor_within_the_threshold() {
        let eps = LayoutUnits::from_aspect(4.0 / 3.0);
        let held = Anchor::new(Point::Center, SCREEN, Point::Center, 10.0, 20.0);
        let mut a = Anchors::from([held]);
        let with = |x: f32, y: f32| Anchor::new(Point::Center, SCREEN, Point::Center, x, y);
        assert!(!a.set(with(10.0001, 19.9999), eps));
        assert_eq!(a.get(Point::Center), Some(&held), "nothing is stored");
        for (x, y) in [(10.001, 20.0), (10.0, 19.999), (f32::NAN, 20.0)] {
            let mut b = a;
            assert!(b.set(with(x, y), eps), "({x}, {y})");
            assert!(b.get(Point::Center).unwrap().x_off.to_bits() == x.to_bits());
        }
        let mut b = a;
        assert!(b.set(
            Anchor::new(Point::Center, 3, Point::Center, 10.0, 20.0),
            eps
        ));
        let mut b = a;
        assert!(b.set(
            Anchor::new(Point::Center, SCREEN, Point::Top, 10.0, 20.0),
            eps
        ));
        let mut b = a;
        assert!(b.set(
            Anchor::new(Point::Top, SCREEN, Point::Center, 10.0, 20.0),
            eps
        ));
        assert_eq!(b.len(), 2);
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
            anchors: Anchors::from([Anchor::new(
                Point::TopLeft,
                SCREEN,
                Point::TopLeft,
                10.0,
                -5.0,
            )]),
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

    /// `0x768d20`'s four strict `<` tests: every edge under `eps` keeps, one at `eps` applies.
    #[test]
    fn keeps_rect_is_strict_on_every_edge() {
        let old = Rect::new(0.0, 0.0, 40.0, 100.0);
        let eps = 0.5;
        assert!(keeps_rect(old, Rect::new(0.25, -0.25, 40.25, 99.75), eps));
        for moved in [
            Rect::new(0.5, 0.0, 40.0, 100.0),
            Rect::new(0.0, -0.5, 40.0, 100.0),
            Rect::new(0.0, 0.0, 40.5, 100.0),
            Rect::new(0.0, 0.0, 40.0, 100.5),
            Rect::new(f32::NAN, 0.0, 40.0, 100.0),
        ] {
            assert!(!keeps_rect(old, moved, eps), "{moved:?}");
        }
        assert!(!keeps_rect(old, old, 0.0), "a zero threshold keeps nothing");
    }

    /// A frame whose new rect is within the threshold of its cached one resolves to the cached
    /// rect, and its dependents read that; past it, the new rect stands.
    #[test]
    fn the_solver_keeps_a_cached_rect_under_the_threshold() {
        const A: Handle = 1;
        const B: Handle = 2;
        let solve = |x: f32| {
            let mut s = LayoutSolver::new();
            s.begin();
            s.set_keep_eps(0.0128);
            s.set_external(SCREEN, screen_rect());
            s.set_frame(
                A,
                &LayoutInput::sized(
                    vec![Anchor::new(
                        Point::BottomLeft,
                        SCREEN,
                        Point::BottomLeft,
                        x,
                        0.0,
                    )],
                    20.0,
                    20.0,
                ),
            );
            s.set_entry(A, Some(Rect::new(0.0, 10.0, 20.0, 30.0)), true);
            s.set_frame(
                B,
                &LayoutInput::sized(
                    vec![Anchor::new(
                        Point::BottomLeft,
                        A,
                        Point::BottomRight,
                        0.0,
                        0.0,
                    )],
                    5.0,
                    5.0,
                ),
            );
            s.solve();
            (s.rect(A).unwrap(), s.rect(B).unwrap())
        };
        let (a, b) = solve(10.01);
        assert_eq!(a, Rect::new(0.0, 10.0, 20.0, 30.0));
        assert_eq!(b.left, 30.0);
        let (a, b) = solve(10.02);
        assert_eq!(a.left, 10.02);
        assert_eq!(b.left, a.right);
    }

    /// A frame with no anchors keeps the rect it entered with until it is invalidated, and what
    /// anchors to a frame whose resolve fails keeps its own: a failed resolve reaches nothing.
    #[test]
    fn a_node_not_reached_keeps_its_entry_state() {
        const A: Handle = 1;
        const B: Handle = 2;
        let held = Rect::new(0.0, 10.0, 20.0, 30.0);
        let below = Rect::new(-5.0, 30.0, 0.0, 35.0);
        let solve = |a_invalidated: bool| {
            let mut s = LayoutSolver::new();
            s.begin();
            s.set_external(SCREEN, screen_rect());
            s.set_frame(A, &LayoutInput::sized(vec![], 20.0, 20.0));
            s.set_entry(A, Some(held), a_invalidated);
            s.set_frame(
                B,
                &LayoutInput::sized(
                    vec![Anchor::new(Point::TopLeft, A, Point::BottomRight, 0.0, 0.0)],
                    5.0,
                    5.0,
                ),
            );
            s.set_entry(B, Some(below), false);
            s.solve();
            (s.rect(A), s.rect(B))
        };
        assert_eq!(solve(false), (Some(held), Some(below)), "nothing reached A");
        assert_eq!(
            solve(true),
            (None, Some(below)),
            "A fails, and B is not reached"
        );
    }

    /// A target that stores a new rect reaches what anchors to it, which resolves again.
    #[test]
    fn a_target_with_a_new_rect_reaches_its_dependents() {
        const A: Handle = 1;
        const B: Handle = 2;
        let mut s = LayoutSolver::new();
        s.begin();
        s.set_external(SCREEN, screen_rect());
        s.set_frame(
            A,
            &LayoutInput::sized(
                vec![Anchor::new(
                    Point::BottomLeft,
                    SCREEN,
                    Point::BottomLeft,
                    10.0,
                    0.0,
                )],
                20.0,
                20.0,
            ),
        );
        s.set_entry(A, None, true);
        s.set_frame(
            B,
            &LayoutInput::sized(
                vec![Anchor::new(
                    Point::BottomLeft,
                    A,
                    Point::BottomRight,
                    0.0,
                    0.0,
                )],
                5.0,
                5.0,
            ),
        );
        s.set_entry(B, Some(Rect::new(0.0, 0.0, 1.0, 1.0)), false);
        s.solve();
        assert_eq!(s.rect(B), Some(Rect::new(0.0, 30.0, 5.0, 35.0)));
    }

    /// A deferred node a changed target reaches keeps its entry rect, is held, and passes the
    /// invalidate on, so what anchors to it is reached and reports reading it (`0x768178`,
    /// `0x7a2fb8`). One held at entry passes nothing.
    #[test]
    fn a_deferred_node_reached_holds_and_passes_on() {
        const A: Handle = 1;
        const B: Handle = 2;
        const C: Handle = 3;
        let solve = |b_held: bool| {
            let mut s = LayoutSolver::new();
            s.begin();
            s.set_external(SCREEN, screen_rect());
            let anchored = |to: Handle| {
                LayoutInput::sized(
                    vec![Anchor::new(
                        Point::BottomLeft,
                        to,
                        Point::BottomRight,
                        0.0,
                        0.0,
                    )],
                    5.0,
                    5.0,
                )
            };
            s.set_frame(
                A,
                &LayoutInput::sized(
                    vec![Anchor::new(
                        Point::BottomLeft,
                        SCREEN,
                        Point::BottomLeft,
                        10.0,
                        0.0,
                    )],
                    20.0,
                    20.0,
                ),
            );
            s.set_entry(A, Some(Rect::new(0.0, 0.0, 20.0, 20.0)), true);
            s.set_frame(B, &anchored(A));
            s.set_entry(B, Some(Rect::new(0.0, 20.0, 5.0, 25.0)), false);
            s.set_deferral(B, true, b_held);
            s.set_frame(C, &anchored(B));
            s.set_entry(C, Some(Rect::new(0.0, 25.0, 5.0, 30.0)), false);
            s.solve();
            (
                s.rect(B),
                s.newly_held().to_vec(),
                s.deferred_reads().to_vec(),
                s.rect(C),
            )
        };
        let (b, held, reads, c) = solve(false);
        assert_eq!(
            b,
            Some(Rect::new(0.0, 20.0, 5.0, 25.0)),
            "B keeps its entry"
        );
        assert_eq!(held, vec![B]);
        assert_eq!(reads, vec![(C, B)], "C was reached and read B");
        assert_eq!(c, Some(Rect::new(0.0, 25.0, 5.0, 30.0)));
        let (_, held, reads, _) = solve(true);
        assert!(
            held.is_empty() && reads.is_empty(),
            "held already: nothing passes"
        );
    }

    /// The reads a resolve reports are the anchors its edge scans reached: a scan stops at the
    /// first anchor that resolves (`0x7671a0`), so a later anchor on the same edges is not read.
    #[test]
    fn the_resolve_reports_the_anchors_its_scans_read() {
        let f = LayoutInput::sized(
            vec![
                Anchor::new(Point::TopLeft, 5, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::Left, 6, Point::Left, 0.0, 0.0),
                Anchor::new(Point::BottomRight, 7, Point::BottomRight, 0.0, 0.0),
            ],
            0.0,
            0.0,
        );
        let (out, reads) = resolve_rect_reads(&f, |_| Some(screen_rect()));
        assert!(matches!(out, ResolveOutcome::Resolved(_)));
        let read: Vec<Handle> = f.anchors.read_targets(reads).collect();
        assert_eq!(
            read,
            vec![5, 7],
            "LEFT is behind TOPLEFT and no centre is derived"
        );
        let (_, reads) = resolve_rect_reads(&f, |t| (t != 5).then(screen_rect));
        let read: Vec<Handle> = f.anchors.read_targets(reads).collect();
        assert_eq!(
            read,
            vec![5, 6, 7],
            "TOPLEFT unresolved: the scan reaches LEFT"
        );
    }

    #[test]
    fn size_changed_epsilon() {
        let r = Rect::new(0.0, 0.0, 40.0, 100.0);
        // 1e-7 is under SIZE_EPS.
        let tiny = Rect::new(0.0, 0.0, 40.0, 100.0 + 1e-7);
        assert!(!size_changed(r, tiny, SIZE_EPS));
        let big = Rect::new(0.0, 0.0, 40.0, 100.001);
        assert!(size_changed(r, big, SIZE_EPS));
        // In units of 1280 to the reference's one, 1e-4 is under the threshold.
        let jitter = Rect::new(0.0, 0.0, 40.0, 100.0001);
        assert!(!size_changed(r, jitter, SIZE_EPS * 1280.0));
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
}
