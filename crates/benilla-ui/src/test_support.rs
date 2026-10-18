//! Test support for checking geometry under the script-unit law ([`crate::screencoord`]): a value
//! that the law's float stores move by a fraction of a unit compares within [`TOL`], anything that
//! is no float compares exactly. The law's own values are pinned bit for bit where it is tested.

use crate::layout::Rect;

/// The tolerance [`assert_near!`] compares floats within, in script units: far above the law's
/// sub-ulp drift (at most about 2e-4 on a 768-unit screen), far below a unit.
pub const TOL: f64 = 1e-3;

/// A value compared within a tolerance where it is a float, exactly where it is not.
pub trait Near {
    fn near(&self, other: &Self, tol: f64) -> bool;
}

impl Near for f64 {
    fn near(&self, other: &f64, tol: f64) -> bool {
        (self - other).abs() <= tol || (self.is_nan() && other.is_nan()) || self == other
    }
}

impl Near for f32 {
    fn near(&self, other: &f32, tol: f64) -> bool {
        f64::from(*self).near(&f64::from(*other), tol)
    }
}

macro_rules! exact {
    ($($t:ty),*) => {$(
        impl Near for $t {
            fn near(&self, other: &Self, _: f64) -> bool {
                self == other
            }
        }
    )*};
}
exact!(bool, char, i8, i16, i32, i64, u8, u16, u32, u64, usize, String, str);

impl<T: Near + ?Sized> Near for &T {
    fn near(&self, other: &Self, tol: f64) -> bool {
        (**self).near(*other, tol)
    }
}

impl<T: Near> Near for Option<T> {
    fn near(&self, other: &Option<T>, tol: f64) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.near(b, tol),
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T: Near> Near for [T] {
    fn near(&self, other: &[T], tol: f64) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.near(b, tol))
    }
}

impl<T: Near, const N: usize> Near for [T; N] {
    fn near(&self, other: &[T; N], tol: f64) -> bool {
        self[..].near(&other[..], tol)
    }
}

impl<T: Near> Near for Vec<T> {
    fn near(&self, other: &Vec<T>, tol: f64) -> bool {
        self[..].near(&other[..], tol)
    }
}

impl Near for Rect {
    fn near(&self, other: &Rect, tol: f64) -> bool {
        self.to_array().near(&other.to_array(), tol)
    }
}

macro_rules! tuple {
    ($(($($n:tt $t:ident),+))*) => {$(
        impl<$($t: Near),+> Near for ($($t,)+) {
            fn near(&self, other: &Self, tol: f64) -> bool {
                $(self.$n.near(&other.$n, tol))&&+
            }
        }
    )*};
}
tuple! {
    (0 A, 1 B)
    (0 A, 1 B, 2 C)
    (0 A, 1 B, 2 C, 3 D)
    (0 A, 1 B, 2 C, 3 D, 4 E)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G)
    (0 A, 1 B, 2 C, 3 D, 4 E, 5 F, 6 G, 7 H)
}

/// `assert_eq!` with floats compared within [`TOL`] ([`Near`]); the same optional message.
#[macro_export]
macro_rules! assert_near {
    ($left:expr, $right:expr $(,)?) => {
        $crate::assert_near!($left, $right, "")
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {{
        let (left, right) = (&$left, &$right);
        if !$crate::test_support::Near::near(left, right, $crate::test_support::TOL) {
            panic!(
                "assertion `left ≈ right` failed: {}\n  left: {:?}\n right: {:?}",
                format_args!($($arg)+),
                left,
                right
            );
        }
    }};
}
