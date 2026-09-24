//! The keyed-loop bake shared by the material-alpha and texture-transform channels: one sampler,
//! after the reference's key search `0x713d50`, and one clock resolution. A sequence track bakes
//! per sequence, since its keys lie on one timeline that every sequence slices a band from and the
//! reference reads the playing sequence's own key window (`ranges[seqSlot]`) every frame.

use benilla_m2::M2Track;

/// A value a keyed loop can linearly interpolate.
pub trait Lerp: Copy {
    fn lerp(a: Self, b: Self, f: f32) -> Self;
}
impl Lerp for f32 {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        a + (b - a) * f
    }
}
impl Lerp for [f32; 2] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [f32::lerp(a[0], b[0], f), f32::lerp(a[1], b[1], f)]
    }
}
impl Lerp for [f32; 3] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [
            f32::lerp(a[0], b[0], f),
            f32::lerp(a[1], b[1], f),
            f32::lerp(a[2], b[2], f),
        ]
    }
}

impl Lerp for [f32; 4] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [
            f32::lerp(a[0], b[0], f),
            f32::lerp(a[1], b[1], f),
            f32::lerp(a[2], b[2], f),
            f32::lerp(a[3], b[3], f),
        ]
    }
}

/// The sequence a track bakes against: the file slot that indexes [`M2Track::ranges`], its
/// absolute `(start_ms, end_ms)` band, and whether it loops (sequence flags bit 0 clear).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeqSlot {
    pub index: usize,
    pub band: (u32, u32),
    pub looping: bool,
}

/// One baked keyed loop in seconds, holding its end keys outside the keyed span; `period == 0` is
/// a constant.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyAnim<V> {
    /// Seconds: the global sequence's duration or the band's length, `0.0` for a constant.
    pub period: f32,
    /// Step interpolation (`interp == 0`): hold each key until the next; else linear.
    pub step: bool,
    /// Fixed at bake time: `true` wraps (`t mod period`), for a global sequence or a looping
    /// sequence's band, whose window the kernel re-fires every pass; `false` clamps
    /// (`min(t, period)`), for a one-shot band that must hold its tail. A finished Bevy
    /// `RepeatAnimation::Never` clip parks at or past `period` without its modulo, where a wrap
    /// would snap a Death fade-out back to its opening value.
    pub wrap: bool,
    /// A global-sequence loop, whose reference cursor is `(sceneClock − attachTime) % duration`
    /// (`0x71437b`), the attach time stamped once per model instance (`0x70ea00`,
    /// `CM2Model+0x68`); a band loop instead runs on the host's clip time, re-armed every play.
    pub gseq: bool,
    /// `(secs from loop start, value)`, time-ascending.
    pub keys: Vec<(f32, V)>,
}

impl<V> KeyAnim<V> {
    /// The sampling time: a gseq loop reads `gseq_now`, the instance's cursor, wrapped in f64 to
    /// keep millisecond precision; any other reads `band_t`, the clip or spawn-age seconds.
    pub fn clock(&self, band_t: f32, gseq_now: f64) -> f32 {
        if self.gseq && self.period > 0.0 {
            (gseq_now % f64::from(self.period)) as f32
        } else {
            band_t
        }
    }
}

impl<V: Lerp> KeyAnim<V> {
    /// Sample at `elapsed` seconds; `empty` is the channel's identity for a keyless loop, which
    /// the bake never emits.
    pub(crate) fn sample_or(&self, elapsed: f32, empty: V) -> V {
        let wrap = self.wrap;
        let Some(&(t0, v0)) = self.keys.first() else {
            return empty;
        };
        if self.period <= 0.0 || self.keys.len() == 1 {
            return v0;
        }
        let t = if wrap {
            elapsed.rem_euclid(self.period)
        } else {
            elapsed.clamp(0.0, self.period)
        };
        if t <= t0 {
            return v0;
        }
        // k0: the last key at or before t, the kernel's search.
        let mut k0 = 0;
        for (i, &(tk, _)) in self.keys.iter().enumerate() {
            if tk <= t {
                k0 = i;
            } else {
                break;
            }
        }
        let (ta, va) = self.keys[k0];
        // Step, or past the final key: hold (the kernel's search clamps; no wrap-lerp to key 0).
        if self.step || k0 + 1 >= self.keys.len() {
            return va;
        }
        let (tb, vb) = self.keys[k0 + 1];
        if tb <= ta {
            return va;
        }
        V::lerp(va, vb, (t - ta) / (tb - ta))
    }
}

/// The reference's track read at `t_ms` over the key window `[lo, hi]` (`0x713d50`, interp
/// dispatch `0x71af20`): `keys[lo]` for a collapsed window (`lo >= hi`), else the last window key
/// at or before `t_ms`, held for a step track or past the last key, else lerped toward the next.
/// Deviation: the fraction is clamped to `[0, 1]`; the reference extrapolates, which for a window
/// whose brackets miss the band can push a fix16 factor negative and cull the batch on a quirk.
pub(super) fn sample_window<V: Lerp>(
    keys: &[(u32, V)],
    step: bool,
    lo: usize,
    hi: usize,
    t_ms: u32,
) -> Option<V> {
    let last = keys.len().checked_sub(1)?;
    let (lo, hi) = (lo.min(last), hi.min(last));
    if lo >= hi {
        return keys.get(lo).map(|&(_, v)| v);
    }
    let mut k0 = lo;
    for (k, &(ts, _)) in keys.iter().enumerate().take(hi + 1).skip(lo) {
        if ts <= t_ms {
            k0 = k;
        } else {
            break;
        }
    }
    let (ta, va) = keys[k0];
    if step || k0 + 1 > last {
        return Some(va);
    }
    let (tb, vb) = keys[k0 + 1];
    if tb <= ta {
        return Some(va);
    }
    let f = ((t_ms as f32 - ta as f32) / (tb as f32 - ta as f32)).clamp(0.0, 1.0);
    Some(V::lerp(va, vb, f))
}

/// One baked loop per file sequence slot, for a track keyed differently per sequence. The UV and
/// tint lanes share one loop per material, which has no sequence to key on; [`Self::uniform`]
/// tells whether a batch can stay there. A dead slot 0 beside a live later slot is common (22 of
/// the 32 multi-slot UV batch-channels, `benilla-extract uvslotscan`).
#[derive(Clone, Debug, PartialEq)]
pub struct SeqLoops<V> {
    /// One entry per file sequence slot, `None` where that slot's key window moves nothing.
    per_seq: Vec<Option<KeyAnim<V>>>,
}

impl<V: PartialEq> SeqLoops<V> {
    /// `None` when no slot animates, the common case.
    pub fn new(per_seq: Vec<Option<KeyAnim<V>>>) -> Option<Self> {
        per_seq
            .iter()
            .any(Option::is_some)
            .then_some(Self { per_seq })
    }

    /// The loop every slot bakes to, or `None` when the slots disagree, a dead slot beside a live
    /// one included: then the batch needs a per-instance consumer.
    pub fn uniform(&self) -> Option<&KeyAnim<V>> {
        let first = self.per_seq.first()?;
        self.per_seq
            .iter()
            .all(|s| s == first)
            .then_some(first.as_ref())
            .flatten()
    }

    /// The loop in slot `seq`, slot 0 when unknown or out of range.
    pub fn seq(&self, seq: Option<usize>) -> Option<&KeyAnim<V>> {
        seq.and_then(|i| self.per_seq.get(i))
            .or_else(|| self.per_seq.first())?
            .as_ref()
    }

    /// Every slot's loop in file order, for `benilla-extract uvslotscan`.
    pub fn slots(&self) -> &[Option<KeyAnim<V>>] {
        &self.per_seq
    }
}

/// Bake one track to a [`KeyAnim`] for one sequence, or `None` when it adds nothing the static
/// path does not. `drop_constant` drops a whole-track constant (the identity, or one the static
/// path folded); `is_identity` drops a band that holds the identity, so a held alpha 0 still bakes
/// and keeps hiding the batch. A `gseq` track ignores `seq`: one loop in every animation.
pub(crate) fn bake_track<T: Copy, V: Lerp + PartialEq>(
    track: &M2Track<T>,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
    proj: impl Fn(T) -> V,
    drop_constant: impl Fn(V) -> bool,
    is_identity: impl Fn(V) -> bool,
) -> Option<KeyAnim<V>> {
    if track.keys.is_empty() {
        return None;
    }
    let keys: Vec<(u32, V)> = track.keys.iter().map(|&(t, v)| (t, proj(v))).collect();
    let step = track.interp == 0;
    // A constant has no clock (`period == 0`), so `wrap` and `gseq` are arbitrary.
    let constant = |v: V| KeyAnim {
        period: 0.0,
        step,
        wrap: true,
        gseq: false,
        keys: vec![(0.0, v)],
    };
    // An all-equal track is a constant in every band, judged whole as the static path judged it.
    let (_, first) = keys[0];
    if keys.iter().all(|&(_, v)| v == first) {
        if drop_constant(first) {
            return None;
        }
        return Some(constant(first));
    }
    if track.gseq != 0xffff {
        // Keys are loop-relative ms, wrapped on the table duration (else the last key's time).
        let period_ms = gseq_durations
            .get(track.gseq as usize)
            .copied()
            .filter(|&d| d > 0)
            .unwrap_or_else(|| keys.last().map(|&(t, _)| t).unwrap_or(0).max(1));
        return Some(KeyAnim {
            period: period_ms as f32 / 1000.0,
            step,
            // A global sequence always wraps, whatever the playing sequence's loop flag says.
            wrap: true,
            gseq: true,
            keys: keys.iter().map(|&(t, v)| (t as f32 / 1000.0, v)).collect(),
        });
    }
    // A sequence track reads this slot's `(lo, hi)` window; with no ranges array, the whole key
    // list (the reference's `[track+4] == 0` fallback).
    let SeqSlot {
        index,
        band,
        looping,
    } = seq?;
    let (start, end) = band;
    let (lo, hi) = match track.ranges.get(index) {
        Some(&(lo, hi)) => (lo as usize, hi as usize),
        None => (0, keys.len().saturating_sub(1)),
    };
    let at = |t: u32| sample_window(&keys, step, lo, hi, t);
    let head = at(start)?;
    if end <= start {
        // A zero-length band samples one instant.
        return (!is_identity(head)).then(|| constant(head));
    }
    // The band's keys plus both edges: the reference's function is piecewise between keys, so
    // sampling there reproduces it exactly.
    let mut baked: Vec<(f32, V)> = vec![(0.0, head)];
    for (k, &(t, v)) in keys.iter().enumerate() {
        if k >= lo && k <= hi && t > start && t < end {
            baked.push(((t - start) as f32 / 1000.0, v));
        }
    }
    if let Some(tail) = at(end) {
        baked.push(((end - start) as f32 / 1000.0, tail));
    }
    // A band the track never moves through is a constant hold.
    if baked.iter().all(|&(_, v)| v == head) {
        return (!is_identity(head)).then(|| constant(head));
    }
    Some(KeyAnim {
        period: ((end - start) as f32 / 1000.0).max(0.001),
        step,
        // A one-shot sequence holds its tail.
        wrap: looping,
        gseq: false,
        keys: baked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_of(period: f32, keys: &[(f32, f32)]) -> KeyAnim<f32> {
        KeyAnim {
            period,
            step: false,
            wrap: true,
            gseq: false,
            keys: keys.to_vec(),
        }
    }

    #[test]
    fn a_dead_slot_zero_beside_a_live_slot_is_not_uniform() {
        let set = SeqLoops::new(vec![None, Some(loop_of(3.3, &[(0.0, 0.0), (1.0, 0.6)]))])
            .expect("one slot animates");
        assert!(set.uniform().is_none());
        assert!(set.seq(Some(0)).is_none(), "slot 0 is the dead hold");
        assert!(set.seq(Some(1)).is_some(), "slot 1 is the animation");
    }

    #[test]
    fn matching_slots_collapse_to_the_one_shared_loop() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), Some(l.clone())]).expect("animates");
        assert_eq!(set.uniform(), Some(&l));
    }

    #[test]
    fn differing_live_slots_are_not_uniform_either() {
        let set = SeqLoops::new(vec![
            Some(loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)])),
            Some(loop_of(2.0, &[(0.0, 1.0), (2.0, 0.0)])),
        ])
        .expect("animates");
        assert!(set.uniform().is_none());
    }

    #[test]
    fn a_set_with_no_live_slot_is_no_set() {
        assert!(SeqLoops::<f32>::new(vec![None, None]).is_none());
    }

    #[test]
    fn an_unknown_sequence_degrades_to_slot_zero() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), None]).expect("animates");
        assert_eq!(set.seq(None), Some(&l));
        assert_eq!(set.seq(Some(9)), Some(&l));
    }
}
