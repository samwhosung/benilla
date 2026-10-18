//! The screen-coordinate law: the reference's screen-scale cells (`0x41ad10`) and the conversions
//! every script function builds from them between script units, the numbers Lua sees, and layout
//! units, the space the resolver works in, where the screen is `K1` by `K2` and its diagonal is 1
//! (benilla-re `screencoord`).
//!
//! The client computes on the x87 at 53-bit precision (`0x40410c`), which `f64` matches; each
//! `as f32` below is one of the reference's `fstp dword` stores, and nothing else rounds.

use crate::layout::Rect;

/// The screen-scale cells `0x41ad10(a)` writes from the aspect `a`, with `L = 1/√(a² + 1)` at 53
/// bits: `A = RN24(0.75·a)` (`[0x832a4c]`), `K1 = RN24(a·L)` (`[0x832a44]`, multiplying the unrounded
/// `L`, `0x41ad3e` `fst`) and `K2 = RN24(L)` (`[0x832a48]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutUnits {
    a: f32,
    k1: f32,
    k2: f32,
}

impl LayoutUnits {
    /// The cells for the screen aspect `a`, width over height, as `0x41ad10` stores them.
    pub fn from_aspect(a: f32) -> LayoutUnits {
        let a64 = f64::from(a);
        let l = 1.0 / (a64 * a64 + 1.0).sqrt();
        LayoutUnits {
            a: (a64 * 0.75) as f32,
            k1: (l * a64) as f32,
            k2: l as f32,
        }
    }

    /// `K1`, the screen's width in layout units (`0x41ae60(1.0)`).
    #[inline]
    pub fn k1(self) -> f32 {
        self.k1
    }

    /// `K2`, the screen's height in layout units (`0x41ae70(1.0)`).
    #[inline]
    pub fn k2(self) -> f32 {
        self.k2
    }

    /// `1024·A`, exact: `0x41ad70` then `fmul [0x7ffd68]` (1024.0).
    #[inline]
    fn scale(self) -> f64 {
        f64::from(self.a) * 1024.0
    }

    /// The setter steps before their last store: `K1 · RN24(v / (1024·A))` at 53 bits (`fdivr`,
    /// `fstp dword`, `0x41ae60`), which a texture's and a font string's size slots return in `st0`
    /// (`0x77074e`, `0x772957`..`0x77296c`).
    #[inline]
    pub fn store_wide(self, v: f32) -> f64 {
        let q = (f64::from(v) / self.scale()) as f32;
        f64::from(self.k1) * f64::from(q)
    }

    /// `S(v)`, a script number as the setters store it in layout units, on both axes
    /// (`SetWidth` `0x7a1fcb`..`0x7a1feb`, `SetHeight`, `SetPoint` `0x7a28a3`..`0x7a28e3`).
    #[inline]
    pub fn store(self, v: f32) -> f32 {
        self.store_wide(v) as f32
    }

    /// `G(l)`, a stored layout length as a getter pushes it, on both axes: `RN24(1024·A·l) / K1`,
    /// the quotient at 53 bits (`GetWidth` `0x7a1ef4`..`0x7a1f01`, `GetPoint`, `0x41ae40`).
    #[inline]
    pub fn load(self, l: f32) -> f64 {
        let p = (self.scale() * f64::from(l)) as f32;
        f64::from(p) / f64::from(self.k1)
    }

    /// `G(S(v))`: a script number a setter stored, as its getter answers it. benilla keeps the
    /// script number and takes both steps at the read, so new cells read as a rebuild would.
    #[inline]
    pub fn round_trip(self, v: f32) -> f64 {
        self.load(self.store(v))
    }

    /// `E(e, s)`, a resolved edge as `GetLeft`, `GetRight`, `GetTop` and `GetBottom` push it:
    /// divided by the region's scale `[+0x7c]` at 53 bits, then the getter steps
    /// (`0x7a1a54`..`0x7a1a78`).
    #[inline]
    pub fn edge(self, e: f32, s: f32) -> f64 {
        self.edge_wide(f64::from(e), s)
    }

    /// [`Self::edge`] of a length the caller computed at 53 bits, as the layout cache's writer
    /// takes a difference of two edges (`0x49102d`..`0x49103c`).
    #[inline]
    pub fn edge_wide(self, e: f64, s: f32) -> f64 {
        let p = (e / f64::from(s) * self.scale()) as f32;
        f64::from(p) / f64::from(self.k1)
    }

    /// One axis of `GetCenter` (`0x7a18e8`..`0x7a1953`): `((hi − lo)·0.5 + lo) / s` at 53 bits,
    /// then the getter steps, both axes through `K1`.
    #[inline]
    pub fn center(self, lo: f32, hi: f32, s: f32) -> f64 {
        let (lo, hi) = (f64::from(lo), f64::from(hi));
        let mid = (hi - lo) * 0.5 + lo;
        let p = (mid / f64::from(s) * self.scale()) as f32;
        f64::from(p) / f64::from(self.k1)
    }

    /// `GetWidth`'s span of a region with no size (`0x7a1ee6`..`0x7a1eec`): `RN24(hi − lo)` through
    /// the getter steps, not divided by the scale.
    #[inline]
    pub fn span(self, lo: f32, hi: f32) -> f64 {
        self.load((f64::from(hi) - f64::from(lo)) as f32)
    }
}

/// A stored script length, as the reference keeps it in layout units: a number a script or an
/// `<AbsValue>`/`<AbsDimension>` gave, kept as given and stored by `S` at each read, so new cells
/// read as the rebuild they stand for would store it, or a value the engine or a `<RelValue>`
/// wrote in layout units directly, as a constructor's default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    /// A script number, `S` taken at the read.
    Script(f32),
    /// A layout length, stored as it is.
    Layout(f32),
}

impl std::ops::Neg for Len {
    type Output = Len;
    /// The same length the other way, in its own unit.
    fn neg(self) -> Len {
        match self {
            Len::Script(v) => Len::Script(-v),
            Len::Layout(l) => Len::Layout(-l),
        }
    }
}

impl Default for Len {
    /// A script 0, the cleared cell.
    fn default() -> Len {
        Len::Script(0.0)
    }
}

impl std::hash::Hash for Len {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Len::Script(v) => (0u8, v.to_bits()).hash(state),
            Len::Layout(l) => (1u8, l.to_bits()).hash(state),
        }
    }
}

impl Len {
    /// The length in layout units, as the reference holds it.
    #[inline]
    pub fn layout(self, units: LayoutUnits) -> f32 {
        match self {
            Len::Script(v) => units.store(v),
            Len::Layout(l) => l,
        }
    }

    /// The length as its getter answers it, `G` of the stored value.
    #[inline]
    pub fn get(self, units: LayoutUnits) -> f64 {
        units.load(self.layout(units))
    }

    /// The number held, in whichever unit it is: for a value already brought into layout units,
    /// as the resolver's stored input is ([`crate::layout::LayoutInput::stored`]).
    #[inline]
    pub fn raw(self) -> f32 {
        match self {
            Len::Script(v) | Len::Layout(v) => v,
        }
    }

    /// Whether it is exactly zero in either unit, the size sentinel (`0x7ffd74`).
    #[inline]
    pub fn is_zero(self) -> bool {
        self.raw() == 0.0
    }

    /// Bit-exact identity, its unit included, as the layout gate's fingerprint compares.
    #[inline]
    pub fn bits(self) -> u64 {
        match self {
            Len::Script(v) => u64::from(v.to_bits()),
            Len::Layout(l) => (1 << 32) | u64::from(l.to_bits()),
        }
    }

    /// The length in script units for a consumer that lays out or draws in them: a script number
    /// as given, a layout length through `G`.
    #[inline]
    pub fn script(self, units: LayoutUnits) -> f32 {
        match self {
            Len::Script(v) => v,
            Len::Layout(l) => units.load(l) as f32,
        }
    }
}

/// The layout space of a screen root `w` by `h` script units wide and tall: its extent in layout
/// units, `S(w)` by `RN24(K2 · RN24(h / 768))`, and the map from layout units back to the root's,
/// which the renderer and the hit tests read. The interface's own root is `768·a` by 768
/// ([`LayoutSpace::for_window`]), whose extent is exactly `K1` by `K2`, the root rect `0x764284`
/// stores; any other root, as the tests lay out against, keeps its own size through the same law.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutSpace {
    units: LayoutUnits,
    root_w: f32,
    root_h: f32,
    extent_x: f32,
    extent_y: f32,
}

impl LayoutSpace {
    /// The space of a root `w` by `h` script units under the cells `units`.
    pub fn new(units: LayoutUnits, w: f32, h: f32) -> LayoutSpace {
        let extent_y = (f64::from(units.k2) * f64::from((f64::from(h) / 768.0) as f32)) as f32;
        LayoutSpace {
            units,
            root_w: w,
            root_h: h,
            extent_x: units.store(w),
            extent_y,
        }
    }

    /// The interface's root for a window aspect `a`: `768·a` by 768 script units, so its extent is
    /// `K1` by `K2` (`1024·A` is `RN24(768·a)` exactly, and `S` of it is `K1`).
    pub fn for_window(a: f32) -> LayoutSpace {
        LayoutSpace::new(LayoutUnits::from_aspect(a), 768.0 * a, 768.0)
    }

    #[inline]
    pub fn units(&self) -> LayoutUnits {
        self.units
    }

    /// The root in layout units, `(0, 0, extent_y, extent_x)`.
    #[inline]
    pub fn root(&self) -> Rect {
        Rect::new(0.0, 0.0, self.extent_y, self.extent_x)
    }

    /// The root's extent in layout units, the clamp's bounds (`0x767aea`, `0x767b19`).
    #[inline]
    pub fn extent(&self) -> (f32, f32) {
        (self.extent_x, self.extent_y)
    }

    /// A layout x as a fraction of the root's width, in the root's units.
    #[inline]
    pub fn x_to_root(&self, l: f32) -> f32 {
        Self::to_root(l, self.extent_x, self.root_w)
    }

    /// A layout y as a fraction of the root's height, in the root's units.
    #[inline]
    pub fn y_to_root(&self, l: f32) -> f32 {
        Self::to_root(l, self.extent_y, self.root_h)
    }

    #[inline]
    fn to_root(l: f32, extent: f32, root: f32) -> f32 {
        if extent == 0.0 {
            return 0.0;
        }
        (f64::from(l) / f64::from(extent) * f64::from(root)) as f32
    }

    /// A point in the root's units as the client's cursor holds it in layout units: its fraction of
    /// the root, stored as a float (`[root+0x1118]`), times the extent, stored again (`0x41ad80`).
    #[inline]
    pub fn cursor_layout(&self, x: f32, y: f32) -> (f32, f32) {
        let to = |v: f32, root: f32, extent: f32| {
            if root == 0.0 {
                return 0.0;
            }
            let f = (f64::from(v) / f64::from(root)) as f32;
            (f64::from(extent) * f64::from(f)) as f32
        };
        (
            to(x, self.root_w, self.extent_x),
            to(y, self.root_h, self.extent_y),
        )
    }

    /// A layout rect in the root's units: the root maps onto the root, each axis by its fraction.
    #[inline]
    pub fn rect_to_root(&self, r: Rect) -> Rect {
        Rect::new(
            self.y_to_root(r.bottom),
            self.x_to_root(r.left),
            self.y_to_root(r.top),
            self.x_to_root(r.right),
        )
    }

    /// `GetScreenWidth` and `GetScreenHeight`: the root's extent through [`LayoutUnits::load`]
    /// (the glue's `0x46dd30` and `0x46dd80`); in game, `ui_scale` is `UIParent`'s scale
    /// (`0x494590`), which divides the extent first, stored as a float (`0x48b499`, `0x48b4e9`).
    /// The height divides by `K1` as the width does.
    pub fn screen_size(&self, ui_scale: Option<f32>) -> (f64, f64) {
        let side = |extent: f32| {
            let e = ui_scale.map_or(extent, |s| (f64::from(extent) / f64::from(s)) as f32);
            self.units.load(e)
        };
        (side(self.extent_x), side(self.extent_y))
    }

    /// `GetCursorPosition` (the glue's `0x46dad0`, the game's `0x48b820`) of a cursor at `(x, y)`
    /// in the root's units, y up: its fractions of the root, as floats, scaled to layout units and
    /// stored as floats (`0x41ad80`), then through [`LayoutUnits::load`], both axes by `K1`.
    pub fn cursor(&self, x: f32, y: f32) -> (f64, f64) {
        let frac = |v: f32, root: f32| {
            if root == 0.0 {
                0.0
            } else {
                (f64::from(v) / f64::from(root)) as f32
            }
        };
        let fx = frac(x, self.root_w);
        let fy = frac(y, self.root_h);
        let xl = (f64::from(self.extent_x) * f64::from(fx)) as f32;
        let yl = (f64::from(self.extent_y) * f64::from(fy)) as f32;
        (self.units.load(xl), self.units.load(yl))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(w: f32, h: f32) -> LayoutSpace {
        LayoutSpace::for_window(w / h)
    }

    /// `0x41ad10` at 4:3: `A` 1.0, `K1` 0.8 and `K2` one ulp under 0.6 (`0x3f199999`, where the
    /// file's image holds `0x3f19999a`).
    #[test]
    fn the_cells_at_four_thirds() {
        let u = LayoutUnits::from_aspect(4.0 / 3.0);
        assert_eq!(
            u,
            LayoutUnits {
                a: 1.0,
                k1: f32::from_bits(0x3f4c_cccd),
                k2: f32::from_bits(0x3f19_9999),
            }
        );
        assert_eq!(u.store(1280.0), (1.25_f64 * f64::from(0.8_f32)) as f32);
        // `GetPushedTextOffset` of the ctor's 0.001 (`0x3a83126f`).
        assert!((u.load(f32::from_bits(0x3a83_126f)) - 1.28).abs() < 1e-6);
    }

    /// The interface's root is exactly `K1` by `K2` layout units at every aspect.
    #[test]
    fn the_window_root_spans_k1_by_k2() {
        for (w, h) in [
            (1024.0, 768.0),
            (1920.0, 1080.0),
            (1366.0, 768.0),
            (1680.0, 1050.0),
        ] {
            let s = at(w, h);
            let u = s.units();
            assert_eq!(s.extent(), (u.k1(), u.k2()), "{w}x{h}");
            assert_eq!(s.root(), Rect::new(0.0, 0.0, u.k2(), u.k1()));
            // The root maps back onto itself.
            assert_eq!(s.x_to_root(u.k1()), 768.0 * (w / h));
            assert_eq!(s.y_to_root(u.k2()), 768.0);
        }
    }

    /// `GetScreenWidth`/`GetScreenHeight` (`0x46dd30`, `0x46dd80`, `0x48b480`, `0x48b4d0`): 1024
    /// wide at 4:3 but 767.9999427795418 tall, so a `floor` of the height is 767.
    #[test]
    fn the_screen_size_takes_the_references_rounding() {
        assert_eq!(
            at(1024.0, 768.0).screen_size(None),
            (1024.0, 767.9999427795418)
        );
        assert_eq!(at(1024.0, 768.0).screen_size(None).1.floor(), 767.0);
        assert_eq!(
            at(1920.0, 1080.0).screen_size(None),
            (1365.333426704711, 767.9999824928667)
        );
        assert_eq!(
            at(1366.0, 768.0).screen_size(None),
            (1365.9999734688627, 767.9999649898396)
        );
        assert_eq!(at(1366.0, 768.0).screen_size(None).0.floor(), 1365.0);
        assert_eq!(
            at(1680.0, 1050.0).screen_size(None),
            (1228.8000575804501, 768.000053981672)
        );
        // In game, `UIParent`'s scale divides the extent first.
        assert_eq!(
            at(1920.0, 1080.0).screen_size(Some(0.5)),
            (2730.666853409422, 1535.9999649857334)
        );
        assert_eq!(
            at(1920.0, 1080.0).screen_size(Some(0.8)),
            (1706.6667833808888, 960.00001313035)
        );
    }

    /// The cursor: both axes divide by `K1`, so the 4:3 centre's y is under 384.
    #[test]
    fn the_cursor_takes_the_references_rounding() {
        let s = at(1024.0, 768.0);
        assert_eq!(s.cursor(512.0, 384.0), (512.0, 383.9999713897709));
    }

    /// A set value reads back through `G(S(v))`: at 4:3 `SetWidth(100)` stores 0.078125 and reads
    /// 99.99999850988391.
    #[test]
    fn a_set_value_round_trips_through_the_cells() {
        let cases = [
            ((1024.0, 768.0), [99.99999850988391, 768.0000190734861, 0.5]),
            (
                (1920.0, 1080.0),
                [100.00000136774479, 767.9999824928667, 0.5],
            ),
            (
                (1366.0, 768.0),
                [100.00000273516879, 767.9999649898396, 0.5000000341896098],
            ),
        ];
        for ((w, h), want) in cases {
            let u = at(w, h).units();
            let got = [100.0, 768.0, 0.5].map(|v| u.load(u.store(v)));
            assert_eq!(got, want, "{w}x{h}");
        }
        assert_eq!(at(1024.0, 768.0).units().store(100.0), 0.078125);
    }

    /// The edge and centre getters divide by the scale before the getter steps: at 16:9 under a
    /// 0.8 `UIParent` a full-screen child's right edge is 1706.6666433238222 while the screen width
    /// is 1706.6667833808888, as the screen getter stores `K1/u` as a float and the edge does not;
    /// the top agrees with the height. The span is not divided.
    #[test]
    fn the_edge_getters_divide_by_the_scale_at_53_bits() {
        let s = at(1920.0, 1080.0);
        let u = s.units();
        assert_eq!(u.edge(u.k1(), 0.8), 1706.6666433238222);
        assert_eq!(u.edge(u.k2(), 0.8), 960.00001313035);
        assert_eq!(u.span(0.0, u.k1()), 1365.333426704711);
        // 4:3: a frame at scale 0.9 offset 100 from the left.
        let u = at(1024.0, 768.0).units();
        let left = (0.9_f64 * f64::from(u.store(100.0))) as f32;
        assert_eq!(u.edge(left, 0.9), 99.99999850988391);
        // The full screen's centre.
        assert_eq!(u.center(0.0, u.k1(), 1.0), 512.0);
        assert_eq!(u.center(0.0, u.k2(), 1.0), 383.9999713897709);
    }

    /// A root other than the window's keeps its own size through the law.
    #[test]
    fn a_test_root_keeps_its_size() {
        let s = LayoutSpace::new(LayoutUnits::from_aspect(800.0 / 600.0), 800.0, 600.0);
        assert_eq!(s.extent(), (0.625, 0.468_749_97));
        assert_eq!(s.screen_size(None), (799.9999880790713, 599.9999529123313));
        assert_eq!(s.rect_to_root(s.root()), Rect::new(0.0, 0.0, 600.0, 800.0));
    }
}
