//! The keyed M2Track sampler shared by the particle and ribbon parsers. Sequence-timeline keys
//! (`gseq == 0xffff`) sit in each sequence's absolute `[start, end]` band, as bone keys do: an
//! effect whose sequence 0 spans `[1000, 2600]` bursts at 1000 ms, its first instant. Parsing
//! rebases them onto sequence 0's band, the one an effect or doodad plays, so seconds since spawn
//! sample them directly; a model keying an emitter per sequence is sampled on sequence 0 alone.
//!
//! A Wrath (264) file carries 20-byte nested tracks (each sequence its own key clip) instead of
//! the 28-byte flat ones: the clips are read through `benilla-m2`'s Wrath reader, which rebases
//! clip `s` by that sequence's band start, and the combined absolute keys take the same band
//! rebase a pre-Wrath track does.

use benilla_m2::M2Layout;

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
/// Pre-Wrath records carry `start`/`end` on the shared timeline; a Wrath record owns a clock
/// (`length` @+4), so band 0 is `[0, length0]`, as `band_starts` accumulates.
pub(crate) fn seq0_band(bytes: &[u8], lay: &M2Layout) -> (u32, u32) {
    let Some((n_seq, o_seq)) = lay.array_at(bytes, lay.sequences) else {
        return (0, u32::MAX);
    };
    let (n_seq, o_seq) = (n_seq as usize, o_seq as usize);
    if n_seq == 0 || o_seq + lay.sequence_stride > bytes.len() {
        return (0, u32::MAX);
    }
    if lay.wrath {
        (0, le_u32(bytes, o_seq + 4))
    } else {
        (le_u32(bytes, o_seq + 4), le_u32(bytes, o_seq + 8))
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

/// The first value of an M2Track, `elem` bytes, decoded by `read`, or `default`: the flat
/// `values[0]` pre-Wrath, the first non-empty per-sequence clip's first value in Wrath.
pub(crate) fn track_first<T: Copy>(
    b: &[u8],
    track: usize,
    elem_size: usize,
    default: T,
    read: impl Fn(&[u8], usize) -> T,
    lay: &M2Layout,
) -> T {
    if lay.wrath {
        let band_starts = benilla_m2::band_starts_for(b, lay);
        let t = benilla_m2::track_read_wrath(
            b,
            track,
            elem_size,
            |b, o| (o.saturating_add(elem_size) <= b.len()).then(|| read(b, o)),
            &band_starts,
        );
        return t.keys.first().map(|&(_, v)| v).unwrap_or(default);
    }
    if track + 0x1c > b.len() {
        return default;
    }
    let n = le_u32(b, track + 0x14);
    let ofs = le_u32(b, track + 0x18) as usize;
    if n == 0 || ofs + elem_size > b.len() {
        return default;
    }
    read(b, ofs)
}

/// Read an M2Track's keys, `elem`-byte values decoded by `read`; sequence keys are rebased to
/// `band`, global-sequence ones keep their clock, and an empty or truncated track is `default`.
/// In a Wrath file the track is the 20-byte nested shape, read per `band_starts` (see the module
/// note) and then given the same `band` rebase.
pub(crate) fn track_keys_with<V: TrackValue>(
    b: &[u8],
    track: usize,
    default: V,
    lay: &M2Layout,
    band: (u32, u32),
    elem: usize,
    read: impl Fn(&[u8], usize) -> V,
) -> ValueTrack<V> {
    if lay.wrath {
        let band_starts = benilla_m2::band_starts_for(b, lay);
        // The nested reader passes value offsets it has not range-checked, so the adapter
        // bounds-checks before `read` indexes.
        let t = benilla_m2::track_read_wrath(
            b,
            track,
            elem,
            |b, o| (o.saturating_add(elem) <= b.len()).then(|| read(b, o)),
            &band_starts,
        );
        if t.keys.is_empty() {
            return ValueTrack::constant(default);
        }
        let mut keys = t.keys;
        if t.gseq == 0xffff {
            rebase_keys_to_band(&mut keys, band.0, band.1);
        }
        return ValueTrack {
            keys,
            interp: t.interp,
        };
    }
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

    /// A minimal v264 file: two sequences (lengths 1000, 500 → band starts 0, 1000) and one
    /// 20-byte nested f32 track with one key per clip.
    fn wrath_fixture() -> (Vec<u8>, M2Layout) {
        let mut b = vec![0u8; 0x640];
        b[0..4].copy_from_slice(b"MD20");
        b[4..8].copy_from_slice(&264u32.to_le_bytes());
        // sequences @ 0x1c/0x20 → two 0x40 records at 0x400.
        b[0x1c..0x20].copy_from_slice(&2u32.to_le_bytes());
        b[0x20..0x24].copy_from_slice(&0x400u32.to_le_bytes());
        b[0x400 + 4..0x400 + 8].copy_from_slice(&1000u32.to_le_bytes());
        b[0x440 + 4..0x440 + 8].copy_from_slice(&500u32.to_le_bytes());
        // Track @0x500: interp 1, gseq 0xffff, ts (2, 0x600), values (2, 0x610).
        b[0x500..0x502].copy_from_slice(&1u16.to_le_bytes());
        b[0x502..0x504].copy_from_slice(&0xffffu16.to_le_bytes());
        b[0x504..0x508].copy_from_slice(&2u32.to_le_bytes());
        b[0x508..0x50c].copy_from_slice(&0x600u32.to_le_bytes());
        b[0x50c..0x510].copy_from_slice(&2u32.to_le_bytes());
        b[0x510..0x514].copy_from_slice(&0x610u32.to_le_bytes());
        // Outer arrays: clip 0 (1, 0x620)/(1, 0x630), clip 1 (1, 0x624)/(1, 0x634).
        b[0x600..0x604].copy_from_slice(&1u32.to_le_bytes());
        b[0x604..0x608].copy_from_slice(&0x620u32.to_le_bytes());
        b[0x608..0x60c].copy_from_slice(&1u32.to_le_bytes());
        b[0x60c..0x610].copy_from_slice(&0x624u32.to_le_bytes());
        b[0x610..0x614].copy_from_slice(&1u32.to_le_bytes());
        b[0x614..0x618].copy_from_slice(&0x630u32.to_le_bytes());
        b[0x618..0x61c].copy_from_slice(&1u32.to_le_bytes());
        b[0x61c..0x620].copy_from_slice(&0x634u32.to_le_bytes());
        // Clip keys: (200 ms, 5.0) in seq 0, (100 ms, 7.0) in seq 1 (→ 1100 absolute).
        b[0x620..0x624].copy_from_slice(&200u32.to_le_bytes());
        b[0x624..0x628].copy_from_slice(&100u32.to_le_bytes());
        b[0x630..0x634].copy_from_slice(&5.0f32.to_le_bytes());
        b[0x634..0x638].copy_from_slice(&7.0f32.to_le_bytes());
        let lay = M2Layout::of(&b).expect("a v264 layout");
        (b, lay)
    }

    #[test]
    fn wrath_keys_rebase_each_clip_by_its_band() {
        let (b, lay) = wrath_fixture();
        assert_eq!(benilla_m2::band_starts_for(&b, &lay), vec![0u32, 1000]);
        assert_eq!(seq0_band(&b, &lay), (0, 1000), "band 0 is [0, length0]");
        // The joined absolute keys (200, 1100) rebase onto band 0: 1100 closes the band at 1000.
        let t = track_keys_with(&b, 0x500, -1.0, &lay, (0, 1000), 4, |b, o| {
            f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        });
        assert_eq!(t.keys, vec![(200u32, 5.0f32), (1000, 7.0)]);
        assert_eq!(t.interp, 1);
        // The first value is the first clip's first key, not sequence order's.
        let first = track_first(
            &b,
            0x500,
            4,
            -1.0,
            |b, o| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]),
            &lay,
        );
        assert_eq!(first, 5.0);
    }

    #[test]
    fn wrath_gseq_track_keeps_its_loop_clock() {
        let (mut b, lay) = wrath_fixture();
        b[0x502..0x504].copy_from_slice(&1u16.to_le_bytes()); // gseq 1
        let t = track_keys_with(&b, 0x500, -1.0, &lay, (0, 1000), 4, |b, o| {
            f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        });
        assert_eq!(t.keys, vec![(200u32, 5.0f32)], "clip 0 only, unshifted");
    }
}
