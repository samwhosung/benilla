//! The keyed **M2Track sampler kernel** shared by the cosmetic-record parsers (`particles`,
//! `ribbons`): `(timestamp ms, value)` key lists read off the vanilla 28-byte track, rebased from
//! the global timeline onto the first sequence's band, sampled on a 0-based clip clock.
//!
//! **Track timestamps are the vanilla global timeline.** A sequence-timeline track
//! (`gseq == 0xffff`) keys inside each sequence's absolute `[start, end]` band (the same law as
//! bone keys, `models/anim.rs`) — an effect model whose seq 0 spans `[1000, 2600]` authors its
//! impact burst at 1000 ms, *the first instant of the clip*. Keyed tracks are therefore
//! **rebased to the first sequence's band** at parse ([`rebase_keys_to_band`]), so a runtime
//! clock of seconds-since-spawn samples them directly. First-sequence only: the effect/doodad
//! load-arm plays exactly that sequence; multi-sequence gating (a creature emitter keyed per gait
//! band) has no driving model yet and stays a named seam.
//!
//! History: the particle emission params were once `value[0]`-baked, which silenced every keyed
//! burst (`BloodSpurt.m2` rates its starflash `0 → 20 → 0`). The ribbon
//! look tracks repeated the exact trap — `HolySmite_Low_Chest.m2` keys its slash ribbons' height
//! `0 → 0.167 → 0`, so the value[0] bake read a permanent zero and Smite's impact slash never
//! drew. One kernel, both lanes, so the next keyed track can't be silently constant-folded.

fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// A value the keyed-track sampler can hold and lerp — `f32` scalars (emission rate, ribbon
/// alpha/heights) and `[f32; 3]` triples (the ribbon colour track's C3Vector).
pub trait TrackValue: Copy {
    /// The empty-track fallback (defensive — the parsers always fall back to a constant key).
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

/// A keyed M2Track, reduced to `(timestamp ms, value)` pairs — enough to sample the **burst**
/// emitters the one-shot combat effects are authored with (0137/0140 fold-back: `BloodSpurt.m2`'s
/// starflash + glowball emitters key their rate `0 → 200 → 0` over the first 133 ms; a `value[0]`
/// bake reads 0 and they never emit at all). The ribbon look tracks (colour/alpha/heights) ride
/// the same kernel — `HolySmite_Low_Chest.m2` keys its slash ribbons' height `0 → 0.167 → 0`, the
/// same value[0]-reads-0 trap. Keys are sampled with linear interpolation, **end-clamped** — no
/// loop wrap: every looping ambient prop in the shipped corpus authors a constant (single-key)
/// value, so a wrap clock has no driving model yet; a persistent looping model with a keyed track
/// would hold its last key (the documented seam).
#[derive(Debug, Clone, Default)]
pub struct ValueTrack<V = f32> {
    /// `(timestamp ms, value)` keys, in file order (ascending timestamps).
    pub keys: Vec<(u32, V)>,
    /// The M2Track's interpolation word (`+0x00`): 0 = STEP (`values[k0]`), else linear. The
    /// corpus splits cleanly on it — burst emitters author 0 (the full count arms AT the key),
    /// continuous ramps author 1 (the pour envelope lerps). [`Self::sampled_ms`] dispatches on it.
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

    /// The first key's value — the old `value[0]` load-time bake, kept for the constant-track
    /// parameters and diagnostics.
    pub fn first(&self) -> V {
        self.keys.first().map_or(V::ZERO, |&(_, v)| v)
    }

    /// STEP-sample at `ms` since clip start: the nearest-previous key's value — the first key's
    /// before the first, the last **held** past the end. This is the interp-0 leg of the
    /// reference sampler ([`Self::sampled_ms`] dispatches here) — a Feint plume keyed
    /// `{0:0, 67:30}` interp 0 is *silent* until 67 ms, then 30, exactly what arms its burst
    /// count. (interp≠0 tracks lerp (`0x71af76`), and the corpus authors its
    /// continuous ramps that way.)
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

    /// Sample as the reference's per-frame track sampler does (`0x71af20` → `0x713d50`): STEP
    /// (`values[k0]`) when the
    /// track's [`Self::interp`] word is 0, else LINEAR between the bracketing keys — **held** at
    /// the last key past the end, and extrapolated **backward** (negative fraction) below the
    /// first key (the sampler does not clamp there). Raw file values — a rate track may go
    /// negative (`BloodSpurt.m2` emitter 0 ends at −100, and the back-extrapolation can dip
    /// below the first key); the consumer floors at 0 exactly like the `SetEmissionRate` guard.
    pub fn sampled_ms(&self, ms: f32) -> V {
        if self.interp == 0 {
            return self.step_ms(ms);
        }
        let n = self.keys.len();
        if n <= 1 {
            return self.first();
        }
        // The bracketing pair: the segment whose start is the last key at/before `ms` — the
        // FIRST segment below the first key (negative fraction), a hold on the last key.
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

    /// Sample at `ms` since clip start: linear between neighbouring keys, clamped to the first/last
    /// key outside the keyed span. Raw file values — a rate track may go negative at its tail
    /// (`BloodSpurt.m2` emitter 0 ends at −100); the consumer floors at 0.
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
    /// The track's peak value — the "can this ever contribute" gate (a burst emitter's
    /// `value[0]` is 0; its peak is not — same for a slash ribbon's height).
    pub fn peak(&self) -> f32 {
        self.keys.iter().fold(f32::MIN, |m, &(_, v)| m.max(v))
    }
}

/// Rebase one keyed track from the vanilla global timeline onto a sequence's `[start, end]` band
/// (clip-relative ms): in-band keys shift by `−start`; of the keys at/before `start` only the
/// last survives, collapsed to `t = 0` (it IS the value the band starts on, per the
/// nearest-previous sampling law); of the keys at/after `end` only the first survives, clamped
/// to `t = end − start` (the value the band ends on). Matches the reference sampling a track
/// with the sequence clock, expressed on a 0-based clip clock.
pub(crate) fn rebase_keys_to_band<V: Copy>(keys: &mut Vec<(u32, V)>, start: u32, end: u32) {
    if keys.is_empty() || (start == 0 && keys.last().is_some_and(|&(t, _)| t <= end)) {
        return; // already 0-based in-band — the common ambient-prop shape
    }
    let mut out: Vec<(u32, V)> = Vec::with_capacity(keys.len());
    for &(t, v) in keys.iter() {
        if t <= start {
            // Collapse everything at/before the band start to the t=0 key (last one wins).
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

/// The **first sequence's absolute time band** (sequences @ `0x1c/0x20`, stride `0x44`, start @
/// +4 / end @ +8 — the same walk as `models/anim.rs`): the window keyed tracks are rebased onto.
/// A sequence-less model keeps raw timestamps (band = whole timeline).
pub(crate) fn seq0_band(bytes: &[u8]) -> (u32, u32) {
    let (n_seq, o_seq) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
    if n_seq > 0 && o_seq + 0x44 <= bytes.len() {
        (le_u32(bytes, o_seq + 4), le_u32(bytes, o_seq + 8))
    } else {
        (0, u32::MAX)
    }
}

/// A vanilla M2Track's key sub-arrays: `(gseq, n, timestamps offset, values offset)` — `None` if
/// the track is out of range or keyless.
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

/// Read a vanilla M2Track's full `(timestamp ms, value)` key list: timestamps `{count @ +0x0c,
/// offset @ +0x10}` zipped with values `{count @ +0x14, offset @ +0x18}` (paired up to the
/// shorter count — equal in well-formed files), each value `elem` bytes decoded by `read`.
/// Sequence-timeline keys are rebased to `band` (module doc; a global-sequence-tagged track keeps
/// its own free clock and is left as-is). Empty/out-of-range → a constant track at `default`.
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

    /// The sampling law: linear between keys, end-clamped, raw values (negatives pass through —
    /// the consumer floors).
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

    /// The step law: nearest-previous key, first value before the first, held past the last —
    /// the emission-rate sampler (`values[k0]`, no lerp).
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

    /// The reference sampler law ([`ValueTrack::sampled_ms`]): interp 0 steps, interp 1 lerps
    /// with hold-last and BACKWARD extrapolation below the first key (no clamp there — the
    /// `0x713d50` negative-fraction edge; the rate consumer's `> 0` floor absorbs the dip).
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

    /// The vec3 instantiation (the ribbon colour track): per-channel lerp, same clamp law.
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

    /// The band rebase: keys before the band collapse to their last value at t=0, in-band keys
    /// shift, keys past the end clamp to the band length, and an already-0-based track is
    /// untouched.
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
