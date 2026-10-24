//! The reference's Catmull-Rom curve object in mode 1 (cubic), measured by arc length: a ship
//! leg's curve and the transfer screen's route. `n` control points make `n − 3` segments, segment `i` running from point `i + 1` to
//! point `i + 2`. The x87 runs at 53 bits: each operation is an `f64`, each store an `f32`.

/// The Catmull-Rom basis at `0xb05e10` (`0x453fa0`): the row `i` weights point `i` of a segment,
/// its coefficients highest power first.
const BASIS: [[f32; 4]; 4] = [
    [-0.5, 1.0, -0.5, 0.0],
    [1.5, -2.5, 0.0, 1.0],
    [-1.5, 2.0, 0.5, 0.0],
    [0.5, -0.5, 0.0, 0.0],
];

/// Its first derivative at `0xb05dd8` (`0x454080`), degree 2, highest power first.
const DERIVATIVE: [[f32; 3]; 4] = [
    [-1.5, 2.0, -0.5],
    [4.5, -5.0, 0.0],
    [-4.5, 4.0, 0.5],
    [1.5, -1.0, 0.0],
];

/// `0x8029c8`, the parameter step of a segment's 20 chords (`0x453760`).
const CHORD_STEP: f32 = 0.05;
/// `0x8029d4`, 2^-22: a shorter vector is left undivided (`0x454580`, `0x5f8cb0`).
pub(super) const SHORT: f64 = 2.384_185_791_015_625e-7;
/// `0x8029d0`, the derivative length below which `0x454580` writes no tangent.
const SHORT_DERIVATIVE: f64 = 0.01;

/// `0x453620`: a row's polynomial in Horner form at `t`, unrounded.
fn horner(row: &[f32], t: f32) -> f64 {
    let t = f64::from(t);
    row[1..]
        .iter()
        .fold(f64::from(row[0]), |w, &c| w * t + f64::from(c))
}

/// The weighted sum of `0x453580` (and `0x453640` for the derivative): each point times its row's
/// weight, the x product kept unrounded into its add, the y and z products stored first, every
/// running sum stored.
fn blend<const N: usize>(rows: &[[f32; N]; 4], p: &[[f32; 3]; 4], t: f32) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for (row, q) in rows.iter().zip(p) {
        let w = horner(row, t);
        out[0] = (w * f64::from(q[0]) + f64::from(out[0])) as f32;
        let wy = (w * f64::from(q[1])) as f32;
        let wz = (w * f64::from(q[2])) as f32;
        out[1] = (f64::from(wy) + f64::from(out[1])) as f32;
        out[2] = (f64::from(wz) + f64::from(out[2])) as f32;
    }
    out
}

/// `√((z² + y²) + x²)` at 53 bits, the order of `0x453760` and `0x454580`.
fn length(v: [f64; 3]) -> f64 {
    ((v[2] * v[2] + v[1] * v[1]) + v[0] * v[0]).sqrt()
}

/// The 53-bit sum of stored lengths from 0.0 (`0x7ffd74`), as `0x453300` and `0x453920` add them.
fn sum(lengths: &[f32]) -> f64 {
    lengths.iter().fold(0.0, |s, &l| s + f64::from(l))
}

/// One leg's curve: its control points as the path gives them, each segment's length and the
/// whole length (`0x4532e0`).
#[derive(Debug, Clone)]
pub(crate) struct Curve {
    points: Vec<[f32; 3]>,
    lengths: Vec<f32>,
    /// `+8`, the float of the 53-bit sum of the segment lengths (`0x4543d0`, `0x453920`).
    pub(crate) length: f32,
}

impl Curve {
    /// `SetSplinePath` (`0x453360`) over `points`, which must number more than 3: the client writes
    /// no length for fewer (`0x4532e3`).
    pub(crate) fn new(points: Vec<[f32; 3]>) -> Self {
        debug_assert!(points.len() > 3);
        let mut curve = Self {
            lengths: Vec::with_capacity(points.len() - 3),
            points,
            length: 0.0,
        };
        for i in 0..curve.points.len() - 3 {
            let len = curve.segment_length(i);
            curve.lengths.push(len);
        }
        curve.length = sum(&curve.lengths) as f32;
        curve
    }

    fn segment(&self, i: usize) -> [[f32; 3]; 4] {
        [
            self.points[i],
            self.points[i + 1],
            self.points[i + 2],
            self.points[i + 3],
        ]
    }

    /// `0x453760`: segment `i` as 20 chords at the `f32` parameter stepped by 0.05 from
    /// `0x3d4ccccd`, the running sum stored after each.
    fn segment_length(&self, i: usize) -> f32 {
        let p = self.segment(i);
        let mut prev = blend(&BASIS, &p, 0.0);
        let mut sum = 0.0f32;
        let mut t = f32::from_bits(0x3d4c_cccd);
        for _ in 0..20 {
            let cur = blend(&BASIS, &p, t);
            let d = [0, 1, 2].map(|k| f64::from(cur[k]) - f64::from(prev[k]));
            sum = (length(d) + f64::from(sum)) as f32;
            t = (f64::from(t) + f64::from(CHORD_STEP)) as f32;
            prev = cur;
        }
        sum
    }

    /// `0x453300(k)`: point `k`'s distance along the curve, the 53-bit sum of the first `k − 1`
    /// segment lengths; 0 for `k` of 0.
    pub(super) fn distance_to(&self, k: usize) -> f64 {
        sum(&self.lengths[..k.saturating_sub(1).min(self.lengths.len())])
    }

    /// `0x453840`: the segment holding the arc-length parameter `t` and the fraction into it. The
    /// lengths are walked until `length·t < running + length[i]` or the last segment, the running
    /// sum stored each time the walk moves on; one segment gives `(0, t)`.
    fn locate(&self, t: f32) -> (usize, f32) {
        let count = self.lengths.len();
        if count == 1 {
            return (0, t);
        }
        let s = f64::from(self.length) * f64::from(t);
        let mut running = 0.0f32;
        let mut i = 0usize;
        while i + 1 < count {
            let next = f64::from(self.lengths[i]) + f64::from(running);
            if s < next {
                break;
            }
            i += 1;
            running = next as f32;
        }
        let frac = ((s - f64::from(running)) / f64::from(self.lengths[i])) as f32;
        (i, frac)
    }

    /// `0x453390(t, out, 1)`: the first control point for `t ≤ 0`, the last for `t ≥ 1`, else the
    /// point at the arc-length parameter `t` (slot 2, `0x453840` then `0x453580`).
    pub(crate) fn point(&self, t: f32) -> [f32; 3] {
        // 0x453393: `t < 0` and `t = 0` take the first point; NaN goes on to the slot.
        if t <= 0.0 {
            return self.points[0];
        }
        // 0x4533c6: `t ≥ 1` takes the last.
        if t >= 1.0 {
            return self.points[self.points.len() - 1];
        }
        let (i, u) = self.locate(t);
        blend(&BASIS, &self.segment(i), u)
    }

    /// The position and tangent of `0x454580`'s frame at the arc-length parameter `t`, for a
    /// frame whose tangent held `tangent` before. The tangent is the unit derivative unless its
    /// dot with the segment's unit chord is below 0.5, which takes the chord; a derivative shorter
    /// than 0.01 leaves `tangent` as it was.
    pub(super) fn frame(&self, t: f32, tangent: [f32; 3]) -> ([f32; 3], [f32; 3]) {
        let (i, u) = self.locate(t);
        let p = self.segment(i);
        let position = blend(&BASIS, &p, u);
        let mut chord = [0, 1, 2].map(|k| (f64::from(p[2][k]) - f64::from(p[1][k])) as f32);
        let chord_len = length(chord.map(f64::from));
        // 0x454606: only a length below 2^-22 is left undivided; equal and NaN divide.
        if chord_len.abs() >= SHORT || chord_len.is_nan() {
            let r = 1.0 / f64::from(chord_len as f32);
            chord = chord.map(|c| (f64::from(c) * r) as f32);
        }
        let d = blend(&DERIVATIVE, &p, u);
        let d_len = length(d.map(f64::from));
        if d_len.abs() < SHORT_DERIVATIVE {
            return (position, tangent);
        }
        let r = 1.0 / d_len;
        let d = d.map(|c| (f64::from(c) * r) as f32);
        let dot = (f64::from(d[2]) * f64::from(chord[2]) + f64::from(d[1]) * f64::from(chord[1]))
            + f64::from(chord[0]) * f64::from(d[0]);
        // 0x4546b4: 0.5 and above, and NaN, take the derivative.
        (position, if dot < 0.5 { chord } else { d })
    }
}
