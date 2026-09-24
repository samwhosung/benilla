//! The keyed M2Track sampler shared by the particle and ribbon parsers. Sequence-timeline keys
//! (`gseq == 0xffff`) sit in each sequence's absolute `[start, end]` band, as bone keys do: an
//! effect whose sequence 0 spans `[1000, 2600]` bursts at 1000 ms, its first instant. Parsing
//! rebases them onto sequence 0's band, the one an effect or doodad plays, so seconds since spawn
//! sample them directly; a model keying an emitter per sequence is sampled on sequence 0 alone.

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// A value a track can hold and lerp: `f32` scalars and the ribbon colour track's `[f32; 3]`.
pub trait TrackValue: Copy {
    /// The empty-track value; the parsers fall back to a constant key instead.
    const ZERO: Self;
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl TrackValue for f32 {
    const ZERO: Self = 0.0;
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl TrackValue for [f32; 3] {
    const ZERO: Self = [0.0; 3];
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
    }
}

/// A keyed M2Track as `(timestamp ms, value)` pairs. Burst emitters key their rate
/// (`BloodSpurt.m2` goes `0 → 200 → 0` over 133 ms), so the first key alone reads 0. No loop
/// wrap: the last key holds, and every looping ambient prop in the shipped corpus keys a constant.
#[derive(Debug, Clone, Default)]
pub struct ValueTrack<V = f32> {
    /// `(timestamp ms, value)` keys, in file order (ascending timestamps).
    pub keys: Vec<(u32, V)>,
    /// Interpolation word (`+0x00`): 0 steps (`values[k0]`), as burst emitters author, else lerps.
    pub interp: u16,
}

impl<V: TrackValue> ValueTrack<V> {
    /// A single-key constant track (the parse fallback shape).
    pub(crate) fn constant(v: V) -> Self {
        Self {
            keys: vec![(0, v)],
            interp: 0,
        }
    }

    /// The first key's value, for constant-track parameters and diagnostics.
    pub fn first(&self) -> V {
        self.keys.first().map_or(V::ZERO, |&(_, v)| v)
    }

    /// Step-sample at `ms` since clip start, the reference sampler's interp-0 leg: the
    /// nearest-previous key, the first before the first, the last held past the end.
    pub fn step_ms(&self, ms: f32) -> V {
        let mut v = self.keys.first().map_or(V::ZERO, |&(_, v)| v);
        for &(t, val) in &self.keys {
            if (t as f32) <= ms {
                v = val;
            } else {
                break;
            }
        }
        v
    }

    /// Sample as the reference's per-frame sampler does (`0x71af20` → `0x713d50`, lerp at
    /// `0x71af76`): step at interp 0, else lerp, held past the last key and extrapolated backward
    /// below the first. Raw values may go negative; the consumer floors at 0 (`SetEmissionRate`).
    pub fn sampled_ms(&self, ms: f32) -> V {
        if self.interp == 0 {
            return self.step_ms(ms);
        }
        let n = self.keys.len();
        if n <= 1 {
            return self.first();
        }
        // The segment starting at the last key at or before `ms`; below the first key, the first.
        let k = self
            .keys
            .iter()
            .rposition(|&(t, _)| (t as f32) <= ms)
            .unwrap_or(0);
        if k + 1 == n {
            return self.keys[n - 1].1;
        }
        let (t0, v0) = self.keys[k];
        let (t1, v1) = self.keys[k + 1];
        let span = (t1.saturating_sub(t0)).max(1) as f32;
        V::lerp(v0, v1, (ms - t0 as f32) / span)
    }

    /// Sample at `ms` since clip start: linear between keys, clamped outside them; values are raw.
    pub fn sample_ms(&self, ms: f32) -> V {
        let Some(&(t0, v0)) = self.keys.first() else {
            return V::ZERO;
        };
        if ms <= t0 as f32 {
            return v0;
        }
        for w in self.keys.windows(2) {
            let ((ta, va), (tb, vb)) = (w[0], w[1]);
            if ms < tb as f32 {
                let span = (tb - ta).max(1) as f32;
                return V::lerp(va, vb, (ms - ta as f32) / span);
            }
        }
        self.keys.last().map_or(V::ZERO, |&(_, v)| v)
    }
}

impl ValueTrack<f32> {
    /// The track's peak, whether it can ever contribute (a burst's first key is 0, its peak not).
    pub fn peak(&self) -> f32 {
        self.keys.iter().fold(f32::MIN, |m, &(_, v)| m.max(v))
    }
}

/// Rebase a track from the global timeline onto a sequence's `[start, end]` band: in-band keys
/// shift by `-start`, the last key at or before `start` becomes `t = 0`, and the first at or
/// after `end` closes the band at `end - start`.
pub(crate) fn rebase_keys_to_band<V: Copy>(keys: &mut Vec<(u32, V)>, start: u32, end: u32) {
    if keys.is_empty() || (start == 0 && keys.last().is_some_and(|&(t, _)| t <= end)) {
        return; // already 0-based and in band, the common ambient-prop shape
    }
    let mut out: Vec<(u32, V)> = Vec::with_capacity(keys.len());
    for &(t, v) in keys.iter() {
        if t <= start {
            // Collapse everything at or before the band start to the t=0 key (last one wins).
            match out.first_mut() {
                Some(first) if first.0 == 0 => *first = (0, v),
                _ => out.insert(0, (0, v)),
            }
        } else if t < end {
            out.push((t - start, v));
        } else {
            out.push((end - start, v));
            break; // first key at/past the end closes the band
        }
    }
    *keys = out;
}

/// The first sequence's absolute time band; a sequence-less model gets the whole timeline.
pub(crate) fn seq0_band(bytes: &[u8]) -> (u32, u32) {
    let (n_seq, o_seq) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
    if n_seq > 0 && o_seq + 0x44 <= bytes.len() {
        (le_u32(bytes, o_seq + 4), le_u32(bytes, o_seq + 8))
    } else {
        (0, u32::MAX)
    }
}

/// A vanilla M2Track's `(gseq, n, timestamps offset, values offset)`.
fn track_arrays(b: &[u8], track: usize) -> Option<(u16, usize, usize, usize)> {
    if track + 0x1c > b.len() {
        return None;
    }
    let gseq = le_u16(b, track + 0x02);
    let tn = le_u32(b, track + 0x0c) as usize;
    let tofs = le_u32(b, track + 0x10) as usize;
    let vn = le_u32(b, track + 0x14) as usize;
    let vofs = le_u32(b, track + 0x18) as usize;
    let n = tn.min(vn);
    (n > 0 && tofs + n * 4 <= b.len()).then_some((gseq, n, tofs, vofs))
}

/// Read an M2Track's keys, `elem`-byte values decoded by `read`; sequence keys are rebased to
/// `band`, global-sequence ones keep their clock, and an empty or truncated track is `default`.
pub(crate) fn track_keys_with<V: TrackValue>(
    b: &[u8],
    track: usize,
    default: V,
    band: (u32, u32),
    elem: usize,
    read: impl Fn(&[u8], usize) -> V,
) -> ValueTrack<V> {
    let Some((gseq, n, tofs, vofs)) = track_arrays(b, track) else {
        return ValueTrack::constant(default);
    };
    if vofs + n * elem > b.len() {
        return ValueTrack::constant(default);
    }
    let mut keys: Vec<(u32, V)> = (0..n)
        .map(|i| (le_u32(b, tofs + i * 4), read(b, vofs + i * elem)))
        .collect();
    if gseq == 0xffff {
        rebase_keys_to_band(&mut keys, band.0, band.1);
    }
    ValueTrack {
        keys,
        interp: le_u16(b, track),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_track_samples_linear_and_clamps() {
        let t = ValueTrack {
            keys: vec![(0, 0.0), (33, 0.0), (67, 100.0), (100, 200.0), (133, 0.0)],
            interp: 1,
        };
        assert_eq!(t.sample_ms(-5.0), 0.0); // clamp before first key
        assert_eq!(t.sample_ms(0.0), 0.0);
        assert!((t.sample_ms(50.0) - 50.0).abs() < 1.0); // rising edge, linear
        assert_eq!(t.sample_ms(100.0), 200.0); // the burst peak
        assert!((t.sample_ms(116.5) - 100.0).abs() < 1.0); // falling edge
        assert_eq!(t.sample_ms(500.0), 0.0); // clamp past last key
        assert_eq!(t.peak(), 200.0);
        assert_eq!(t.first(), 0.0);
        let c = ValueTrack::constant(7.5);
        assert_eq!(c.sample_ms(0.0), 7.5);
        assert_eq!(c.sample_ms(9999.0), 7.5);
        assert_eq!(ValueTrack::<f32>::default().sample_ms(10.0), 0.0);
    }

    #[test]
    fn step_sampling_holds_nearest_previous() {
        let t = ValueTrack {
            keys: vec![(0, 0.0), (67, 30.0)],
            interp: 0,
        };
        assert_eq!(t.step_ms(0.0), 0.0);
        assert_eq!(
            t.step_ms(66.0),
            0.0,
            "step, not lerp — silent until the key"
        );
        assert_eq!(t.step_ms(67.0), 30.0);
        assert_eq!(t.step_ms(1500.0), 30.0, "held past the last key");
        assert_eq!(ValueTrack::<f32>::default().step_ms(10.0), 0.0);
    }

    #[test]
    fn sampled_ms_dispatches_on_the_interp_word() {
        let step = ValueTrack {
            keys: vec![(0, 0.0), (67, 30.0)],
            interp: 0,
        };
        assert_eq!(step.sampled_ms(66.0), 0.0, "interp 0: silent until the key");
        assert_eq!(step.sampled_ms(67.0), 30.0);
        let ramp = ValueTrack {
            keys: vec![(100, 10.0), (200, 110.0), (300, 0.0)],
            interp: 1,
        };
        assert!(
            (ramp.sampled_ms(150.0) - 60.0).abs() < 1e-4,
            "mid-ramp lerp"
        );
        assert_eq!(ramp.sampled_ms(200.0), 110.0);
        assert_eq!(ramp.sampled_ms(999.0), 0.0, "held past the last key");
        assert!(
            (ramp.sampled_ms(0.0) - (-90.0)).abs() < 1e-4,
            "below the first key: backward extrapolation along the first segment, not a clamp"
        );
        let single = ValueTrack {
            keys: vec![(0, 40.0)],
            interp: 1,
        };
        assert_eq!(single.sampled_ms(500.0), 40.0, "single key holds");
    }

    #[test]
    fn vec3_track_lerps_per_channel() {
        let t = ValueTrack {
            keys: vec![(0, [1.0, 0.0, 0.0]), (100, [0.0, 1.0, 0.5])],
            interp: 1,
        };
        assert_eq!(t.sample_ms(0.0), [1.0, 0.0, 0.0]);
        let mid = t.sample_ms(50.0);
        assert!((mid[0] - 0.5).abs() < 1e-6);
        assert!((mid[1] - 0.5).abs() < 1e-6);
        assert!((mid[2] - 0.25).abs() < 1e-6);
        assert_eq!(t.sample_ms(500.0), [0.0, 1.0, 0.5]);
    }

    #[test]
    fn band_rebase_collapses_and_shifts() {
        // MoltenBlast_Impact_Chest em0's shape: seq band [1000, 2600], burst keyed at its start.
        let mut keys = vec![(1000u32, 0.0f32), (1133, 60.0)];
        rebase_keys_to_band(&mut keys, 1000, 2600);
        assert_eq!(keys, vec![(0, 0.0), (133, 60.0)]);
        // A constant single key at t=0 with a late band (Fire_Cast_Hand's rate): hold survives.
        let mut keys = vec![(0u32, 21.4f32)];
        rebase_keys_to_band(&mut keys, 3333, 4333);
        assert_eq!(keys, vec![(0, 21.4)]);
        // Multiple pre-band keys: only the last one (the band-start value) survives, at t=0.
        let mut keys = vec![(0u32, 1.0f32), (500, 2.0), (1200, 3.0), (3000, 4.0)];
        rebase_keys_to_band(&mut keys, 1000, 2000);
        assert_eq!(keys, vec![(0, 2.0), (200, 3.0), (1000, 4.0)]);
        // 0-based in-band track (BloodSpurt): identity.
        let mut keys = vec![(0u32, 100.0f32), (500, -100.0), (667, -100.0)];
        rebase_keys_to_band(&mut keys, 0, 667);
        assert_eq!(keys, vec![(0, 100.0), (500, -100.0), (667, -100.0)]);
    }
}
