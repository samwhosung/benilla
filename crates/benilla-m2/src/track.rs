//! M2Track readers: the typed [`M2Track`] (with the [`M2ScalarTrack`] / [`M2Vec3Track`] /
//! [`M2QuatTrack`] aliases) plus the track-parsing helpers the header walk (`crate::parse_m2`)
//! calls for the M2Color, M2TextureWeight, and M2TextureTransform tracks.

use benilla_bytes::ByteExt;

/// One typed `M2Track`, fully read: the interpolation type, the global-sequence tag, the
/// per-sequence key **ranges**, and the keys as **absolute global-timeline milliseconds** paired
/// with values. The v256 track (stride `0x1c`): `interp`@0, `global_seq`@2, interpolation_ranges
/// `M2Array`@`0x04/0x08`, timestamps `M2Array`@`0x0c/0x10` (u32 ms), values `M2Array`@`0x14/0x18`.
/// A sequence-timeline track (`gseq == 0xffff`) keys inside each sequence's absolute time band; a
/// global-sequence track loops on `global_sequences[gseq]`'s own clock (`0x713d50`). Key count is
/// `min(timestamps, values)` — vanilla art occasionally pads one array.
#[derive(Clone, Debug)]
pub struct M2Track<V> {
    /// Interpolation type: `0` = step (nearest-previous key), nonzero = linear (the scalar sampler
    /// `0x71af20`'s `cmp word[track],0` two-way dispatch).
    pub interp: u16,
    /// Global-sequence index, `0xffff` = an ordinary sequence-timeline track.
    pub gseq: u16,
    /// The per-sequence **key-index window** `(lo, hi)`, one entry per sequence in file order —
    /// the array the reference's key search indexes by the playing sequence's slot before it ever
    /// looks at a timestamp (`0x713d50`: `[track+8][idx*8+{0,4}]`; an empty array is the
    /// `[track+4]==0` fallback, "search the whole key list").
    ///
    /// These are **brackets**, not the playable key set: `hi` routinely points at a key in a LATER
    /// sequence's band, which is why selecting in-clip keys through this window instead of by
    /// timestamp froze creatures at a garbage pose (benilla decision 0133). Its load-bearing use is
    /// the other one — a band that keys nothing resolves to `keys[lo]` (`lo >= hi` ⇒ the degenerate
    /// `{lo, lo, 0}` result), so this array says *exactly* what the reference holds there instead
    /// of leaving it to a nearest-key approximation.
    pub ranges: Vec<(u32, u32)>,
    /// `(absolute ms, value)` keys, file order (time-ascending within a band).
    pub keys: Vec<(u32, V)>,
}

impl<V> Default for M2Track<V> {
    fn default() -> Self {
        Self {
            interp: 0,
            gseq: 0,
            ranges: Vec::new(),
            keys: Vec::new(),
        }
    }
}

/// A scalar (`fix16`, `0..=1`) track — the M2Color **alpha** and M2TextureWeight **weight** tracks.
pub type M2ScalarTrack = M2Track<f32>;
/// A `C3Vector` track — the M2TextureTransform **translation** / **scaling** tracks.
pub type M2Vec3Track = M2Track<[f32; 3]>;
/// A quaternion (4×f32, raw v256 floats) track — the M2TextureTransform **rotation** track.
pub type M2QuatTrack = M2Track<[f32; 4]>;

impl<V: Copy + PartialEq> M2Track<V> {
    /// All key values equal (or a single key): the track is a constant — `Some(value)`; `None` for a
    /// keyless or genuinely time-varying track.
    pub fn constant(&self) -> Option<V> {
        let (_, first) = *self.keys.first()?;
        self.keys.iter().all(|&(_, v)| v == first).then_some(first)
    }
}

/// Read one `M2Track<V>` (see [`M2Track`]): the shared `interp`/`gseq`/timestamp walk with a
/// per-value reader. Out-of-range reads yield an empty key list (= "no keys", the factor/transform
/// does not apply) — real art relies on that tolerance.
fn track_read<V>(
    b: &[u8],
    track_ofs: usize,
    val_size: usize,
    read_val: impl Fn(&[u8], usize) -> Option<V>,
) -> M2Track<V> {
    let (Some(interp), Some(gseq)) = (b.u16_at(track_ofs), b.u16_at(track_ofs + 2)) else {
        return M2Track::default();
    };
    let Some(((tn, to), (vn, vo))) = b
        .u32_at(track_ofs + 0x0c)
        .zip(b.u32_at(track_ofs + 0x10))
        .zip(b.u32_at(track_ofs + 0x14).zip(b.u32_at(track_ofs + 0x18)))
    else {
        return M2Track::default();
    };
    let n = tn.min(vn) as usize;
    let (to, vo) = (to as usize, vo as usize);
    let keys = (0..n)
        .map_while(|i| b.u32_at(to + i * 4).zip(read_val(b, vo + i * val_size)))
        .collect();
    // The per-sequence `(lo, hi)` key-index windows (`M2Array`@0x04/0x08, 8-byte entries) — see
    // [`M2Track::ranges`]. A short/absent array reads as empty, which is the reference's own
    // `[track+4] == 0` "no ranges" fallback.
    let ranges = match b.u32_at(track_ofs + 0x04).zip(b.u32_at(track_ofs + 0x08)) {
        Some((rn, ro)) => (0..rn as usize)
            .map_while(|i| {
                let e = ro as usize + i * 8;
                b.u32_at(e).zip(b.u32_at(e + 4))
            })
            .collect(),
        None => Vec::new(),
    };
    M2Track {
        interp,
        gseq,
        ranges,
        keys,
    }
}

fn rd_vec3(b: &[u8], o: usize) -> Option<[f32; 3]> {
    Some([b.f32_at(o)?, b.f32_at(o + 4)?, b.f32_at(o + 8)?])
}

/// Read a scalar `fix16` track (**`int16`**/32767 values). Used for the colour-**alpha** and
/// transparency-**weight** tracks that gate batch visibility and drive the animated material combine.
///
/// **The key is SIGNED** — `movsx`, not `movzx` (`dest = (f32)(int16 P[k0]) * (1/0x7fff)`, bytes
/// `movsx edx,word[P+k0*2]; fild; fmul [0x811610]; fstp dest` at `0x715b2f`–`0x715b46`, dispatched
/// at the M2Color-alpha site `0x715b21` (`colors[]` stride 0x38, track @ +0x1c) and the
/// transparency-weight site `0x715ce2`). Read unsigned, the authored "hide me" key `0x8001` decodes
/// as `+1.00006` instead of `−1.0`, so a batch the reference culls (`A ≤ 0`, `0x707b3a`–`0x707b5c`)
/// draws at full alpha instead: that is how Zul'Farrak's troll gate drew its BURNT twin on top
/// of its intact self and z-fought. Values outside `[0, 1]` are the
/// artist's own encoding, not a data quirk — the combine consumes them as signed floats and the
/// cull tests `≤ 0`.
pub(crate) fn track_fix16(b: &[u8], track_ofs: usize) -> M2ScalarTrack {
    track_read(b, track_ofs, 2, |b, o| {
        b.u16_at(o).map(|v| f32::from(v as i16) / 32767.0)
    })
}

/// Read a timed `C3Vector` (3×f32) track — the texture-transform **translation**/**scaling**
/// tracks and the M2Color **RGB** track.
pub(crate) fn track_vec3_timed(b: &[u8], track_ofs: usize) -> M2Vec3Track {
    track_read(b, track_ofs, 12, rd_vec3)
}

/// Read a timed quaternion (4×f32, raw v256 floats — vanilla does not compress quat keys) track —
/// the texture-transform **rotation** track.
pub(crate) fn track_quat(b: &[u8], track_ofs: usize) -> M2QuatTrack {
    track_read(b, track_ofs, 16, |b, o| {
        Some([
            b.f32_at(o)?,
            b.f32_at(o + 4)?,
            b.f32_at(o + 8)?,
            b.f32_at(o + 12)?,
        ])
    })
}

/// One key of a **cubic** M2 track — the reference's `M2SplineKey<T>`: the value plus its in/out
/// tangents, `{value@+0, in_tan@+1·sizeof(T), out_tan@+2·sizeof(T)}` (the vec3 key is stride
/// `0x24` `{value@+0, inTan@+0xc, outTan@+0x18}`, the scalar-float key stride `0xc`
/// `{value@+0, inTan@+4, outTan@+8}`).
///
/// **The wide key is the stride whatever `interp` says.** The reference's cubic element loops
/// address every key as `payload + k*0x24` (`0x716b51`) / `payload + k*0xc` (`0x7173cc`) *before*
/// the four-way interp dispatch, and the STEP/LINEAR legs then read the `value` sub-field of that
/// same wide key. Real art relies on it: `Cameras\FlyByDwarf.m2`'s roll track is authored
/// `interp = 0` and still carries a 12-byte key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M2SplineKey<V> {
    pub value: V,
    pub in_tan: V,
    pub out_tan: V,
}

/// A cubic `C3Vector` track — the M2Camera **position**/**target** tracks.
pub type M2Vec3SplineTrack = M2Track<M2SplineKey<[f32; 3]>>;
/// A cubic scalar-float track — the M2Camera **roll** track.
pub type M2ScalarSplineTrack = M2Track<M2SplineKey<f32>>;

/// A value a cubic track can hold: the four basis weights are scalars, so the leaf only has to
/// know how to build `Σ wᵢ·Pᵢ` componentwise.
pub trait CubicValue: Copy {
    /// `w0·p0 + w1·p1 + w2·p2 + w3·p3`.
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self;
    /// The LINEAR leg, in the reference's own form `a + (b − a)·t` (`0x716cf1`) — not the
    /// algebraically equal `(1−t)·a + t·b`, which rounds differently in f32.
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl CubicValue for f32 {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        w[0] * p[0] + w[1] * p[1] + w[2] * p[2] + w[3] * p[3]
    }
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl CubicValue for [f32; 3] {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        std::array::from_fn(|j| w[0] * p[0][j] + w[1] * p[1][j] + w[2] * p[2][j] + w[3] * p[3][j])
    }
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
    }
}

impl<V: CubicValue> M2Track<M2SplineKey<V>> {
    /// Sample this cubic track at absolute global-timeline `ms`, **end-clamped** at both ends.
    ///
    /// The four-way `interp` dispatch is the reference's own, and it is the *cubic element loops'*
    /// dispatch — not the two-way `cmp word[track],0; jne <linear>` the bone/TRS loops collapse to
    /// (the switch is per-loop, by track type). The bases, with `t` the key-interval fraction:
    ///
    /// - `0` **STEP** — `value[k0]`, no tangent read (`0x716b5f`).
    /// - `1` **LINEAR** — `value[k0] + (value[k1] − value[k0])·t` (`0x716cf1`).
    /// - `2` **BÉZIER** — the cubic Bernstein basis over control points
    ///   `{value[k0], outTan[k0], inTan[k1], value[k1]}`: `B0 = (1−t)³`, `B1 = 3t(1−t)²`,
    ///   `B2 = 3t²(1−t)`, `B3 = t³` (`0x716c41`). This is what every shipped `Cameras\*.m2`
    ///   fly-by authors on its position and target tracks.
    /// - `3` **HERMITE** — `h00·value[k0] + h10·outTan[k0] + h01·value[k1] + h11·inTan[k1]` with
    ///   `h00 = 2t³−3t²+1`, `h10 = t³−2t²+t`, `h01 = 3t²−2t³`, `h11 = t³−t²` (`0x716b9e`).
    ///
    /// Note which tangent each control point comes from: the **outgoing** tangent of the key
    /// being left and the **incoming** tangent of the key being entered. Swapping them looks
    /// almost right and drifts wrong exactly where the path curves hardest.
    pub fn sample_ms(&self, ms: u32) -> Option<V> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        if ms <= first.0 {
            return Some(first.1.value);
        }
        if ms >= last.0 {
            return Some(last.1.value);
        }
        // The key pair bracketing `ms` (keys are time-ascending within a band).
        let k1 = self.keys.partition_point(|&(t, _)| t <= ms);
        let (t0, a) = self.keys[k1 - 1];
        let (t1, b) = self.keys[k1];
        let t = if t1 > t0 {
            (ms - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        let (t2, t3) = (t * t, t * t * t);
        Some(match self.interp {
            0 => a.value,
            1 => V::lerp(a.value, b.value, t),
            2 => V::combine(
                [
                    (1.0 - t) * (1.0 - t) * (1.0 - t),
                    3.0 * t * (1.0 - t) * (1.0 - t),
                    3.0 * t2 * (1.0 - t),
                    t3,
                ],
                [a.value, a.out_tan, b.in_tan, b.value],
            ),
            _ => V::combine(
                [
                    2.0 * t3 - 3.0 * t2 + 1.0,
                    t3 - 2.0 * t2 + t,
                    3.0 * t2 - 2.0 * t3,
                    t3 - t2,
                ],
                [a.value, a.out_tan, b.value, b.in_tan],
            ),
        })
    }
}

fn rd_spline<V>(
    b: &[u8],
    o: usize,
    step: usize,
    rd: impl Fn(&[u8], usize) -> Option<V>,
) -> Option<M2SplineKey<V>> {
    Some(M2SplineKey {
        value: rd(b, o)?,
        in_tan: rd(b, o + step)?,
        out_tan: rd(b, o + 2 * step)?,
    })
}

/// Read a cubic `C3Vector` track (key stride `0x24`) — the M2Camera position/target tracks.
pub(crate) fn track_spline_vec3(b: &[u8], track_ofs: usize) -> M2Vec3SplineTrack {
    track_read(b, track_ofs, 0x24, |b, o| rd_spline(b, o, 12, rd_vec3))
}

/// Read a cubic scalar-float track (key stride `0xc`) — the M2Camera roll track.
pub(crate) fn track_spline_f32(b: &[u8], track_ofs: usize) -> M2ScalarSplineTrack {
    track_read(b, track_ofs, 0xc, |b, o| {
        rd_spline(b, o, 4, |b, o| b.f32_at(o))
    })
}
