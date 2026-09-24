//! The generic keyed-loop bake shared by the material-alpha (`mat_anim`) and texture-transform
//! (`tex_anim`) channels: ONE kernel-faithful sampler (k0 = last key ≤ t, step holds, linear
//! lerps, clamp past the last key — no wrap-lerp) and ONE clock resolution (gseq wrap vs
//! per-sequence band rebase), so the sampling semantics (`0x713d50`, and the step-boundary fix)
//! live in exactly one place.
//!
//! **A track bakes per SEQUENCE, not once.** A sequence-timeline track keys on one absolute
//! timeline that every sequence slices a band out of, so "the loop this track plays" is a question
//! about *which sequence is playing* — the reference re-reads it every frame from the playing
//! sequence's own key window (`0x713d50`: window = `ranges[seqSlot]`, and a
//! collapsed window `lo >= hi` resolves to the single key `keys[lo]`). Baking only the first
//! sequence's band was correct for a placed doodad — which arms `animations[0]` once and loops it
//! forever — and wrong for anything that changes sequence, i.e. every creature: a voidwalker's two
//! upper armour pieces are weight `0` in Stand/Walk/Run and `1` only in Death, and a seq-0-only
//! bake freezes whichever value Stand happened to hold.

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

/// The sequence a sequence-timeline track bakes against: the model's **file** sequence slot (which
/// is what indexes the track's [`M2Track::ranges`] — NOT an index into a filtered animation list),
/// that sequence's absolute `(start_ms, end_ms)` band, and whether the band LOOPS (sequence flags
/// bit 0 CLEAR) — which is the baked loop's clock law, see [`KeyAnim::wrap`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeqSlot {
    pub index: usize,
    pub band: (u32, u32),
    pub looping: bool,
}

/// One baked keyed loop, seconds: sample at `t mod period` / `min(t, period)` per [`Self::wrap`]
/// (linear or step per [`Self::step`]), holding the first/last key outside the keyed span.
/// `period == 0` ⇒ a constant (single key).
#[derive(Clone, Debug, PartialEq)]
pub struct KeyAnim<V> {
    /// Loop period (secs): the global sequence's duration, or the sequence band's length.
    /// `0.0` for a constant.
    pub period: f32,
    /// Step interpolation (`interp == 0`): hold each key until the next; else linear.
    pub step: bool,
    /// **The clock law**, decided at bake time from what this loop is baked against — never by the
    /// consumer, which is how it used to be got wrong. `true` = WRAP (`t mod period`): a global
    /// sequence (its own free-running clock) or a sequence band whose sequence loops, where the
    /// modulo is the kernel's own re-fire of a windowed track every pass. `false` = CLAMP
    /// (`min(t, period)`): a band whose sequence is one-shot, where the animation clock parks at
    /// (or just past) the band end and the track must hold its TAIL value there.
    ///
    /// The distinction is only ever visible at the very end of a one-shot clip — and that is
    /// exactly where it is load-bearing, because Bevy's `ActiveAnimation::update` returns early on
    /// completion *without* its own modulo, so a finished `RepeatAnimation::Never` clip parks at
    /// `seek_time >= clip_duration == period`. Wrapping that aliases the end of the animation back
    /// onto its **opening** value: an elemental's Death sequence fades every batch to alpha 0 and
    /// then — one frame later, for ever — snapped back to a fully opaque body frozen in mid-air.
    pub wrap: bool,
    /// **Which clock feeds the loop** — the other half of the clock law, also decided at bake
    /// time. `true` = a global-sequence loop: the reference's cursor is
    /// `(sceneClock − instanceAttachTime) % duration` (`0x71437b`: one free-running per-scene ms
    /// clock, snapshotted ONCE per model instance at attach `0x70ea00` —
    /// `CM2Model+0x68`). Per-INSTANCE anchoring, stamped at attach; sequence tracks instead
    /// re-arm their cursor at every play. `false` = a sequence-band loop: clocked by the host's
    /// playing-clip time. Consumers route via [`Self::clock`], passing their instance's anchored
    /// gseq cursor — the spell-fx lane passes the raw scene clock (its pooled-instance
    /// emulation). Decisions 0855/0856: sampling a gseq loop on a per-play clock pinned every
    /// Arcane Intellect cast's twinkles to phase 0 (clustered, low, identical cast to cast).
    pub gseq: bool,
    /// `(secs from loop start, value)`, time-ascending.
    pub keys: Vec<(f32, V)>,
}

impl<V> KeyAnim<V> {
    /// The sampling time for this loop, given the caller's two clocks: `band_t` — the host's
    /// playing-clip / spawn-age seconds (the per-play arm cursor) — and `gseq_now`, the
    /// instance's global-sequence cursor (`sceneClock − attachTime`; the spell-fx pool lane
    /// passes the raw scene clock — see [`Self::gseq`]). A gseq loop reads `gseq_now`
    /// (pre-wrapped in f64, so a long-uptime cursor keeps millisecond precision through the f32
    /// narrowing); everything else keeps its band clock.
    pub fn clock(&self, band_t: f32, gseq_now: f64) -> f32 {
        if self.gseq && self.period > 0.0 {
            (gseq_now % f64::from(self.period)) as f32
        } else {
            band_t
        }
    }
}

impl<V: Lerp> KeyAnim<V> {
    /// Sample at `elapsed` seconds on this loop's own clock ([`Self::wrap`]); `empty` is the
    /// channel's identity for the keyless case (the bake never emits that — each channel's
    /// `sample` supplies it).
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
        // k0 = the last key with time ≤ t (the kernel's search), mirroring `BoneScaleAnim::sample`.
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

/// The reference's own track read at absolute time `t_ms`, for the key window `[lo, hi]` — the
/// transcription of `0x713d50` + the scalar sampler's two-way interp
/// dispatch (`0x71af20`, `cmp word[track],0`):
///
/// - a **collapsed window** (`lo >= hi`) resolves to `keys[lo]` outright — the degenerate
///   `{lo, lo, 0.0}` result, and the reason a band that keys nothing still has an authored value;
/// - otherwise `k0` = the last key in the window at or before `t_ms` (clamped up to `lo`), `k1 =
///   k0 + 1`, and the value is `keys[k0]` for a step track or past the last key, else the lerp.
///
/// **One named deviation:** the reference computes the fraction unclamped, so a `t_ms` outside
/// `[ts[k0], ts[k1]]` extrapolates. That can only arise from a window whose brackets don't contain
/// the band, and an extrapolated *fix16 factor* lands outside `[0, 1]` — where a negative value
/// would cull the batch (`A <= 0`) on a data quirk rather than on authoring. We clamp the fraction
/// to `[0, 1]`, i.e. hold at the bracket instead of running past it.
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

/// Bake one typed track to a [`KeyAnim`] for one sequence, or `None` when it contributes nothing
/// the static path doesn't already handle. `proj` maps the on-disk key value into the baked domain
/// (identity for scalars; vec3 → the UV xy). The two predicates name the channel's semantics:
///
/// - `drop_constant(c)`: a **whole-track** constant `c` vanishes — because `c` is the channel
///   identity, or because the caller's *static* path already folded it away (the alpha combine's
///   constant-0 cull). Judged on the whole track, which is what that static path looked at.
/// - `is_identity(v)`: `v` contributes nothing at runtime — applied to a **band** that bakes down
///   to a constant, where the static path has NOT looked. A held non-identity value still bakes,
///   including a held alpha 0, which must keep hiding the batch every frame of that sequence.
///
/// `gseq_durations` is the model's global-sequence table (ms); `seq` the sequence to bake for
/// (see [`SeqSlot`]). A `gseq`-tagged track ignores `seq` entirely — it runs on the global
/// sequence's own free clock, the same loop in every animation.
/// One baked loop **per file sequence slot** — the shape a channel needs when its track is keyed
/// differently in different sequences, which the reference reads as a matter of course (it samples
/// the *playing* sequence's own key window every frame, `ranges[seqSlot]`; `0x713d50`) and
/// benilla's UV/tint lanes did not.
///
/// Its whole reason to exist is [`Self::uniform`]. The UV and tint loops are consumed through a
/// registry keyed by MATERIAL, which is shared by every instance of a batch and so has no sequence
/// to key on — a design that is exactly right while every slot bakes the same loop, and silently
/// wrong when they don't. `uniform()` is the question "may this batch keep the shared lane?", asked
/// once at bake time instead of assumed: `Some` ⇒ yes, and the consumer carries one loop as before;
/// `None` ⇒ the batch's animation depends on which sequence its instance is playing, so it needs a
/// per-instance consumer (decision 1408).
///
/// The corpus population is small and real (`benilla-extract uvslotscan`): 22 of the 32 multi-slot
/// UV batch-channels ship a **dead slot 0** beside a live later slot — the BRM lava bubbles keying
/// their whole flipbook inside the 50 %-weighted variation 1, bug B98.
#[derive(Clone, Debug, PartialEq)]
pub struct SeqLoops<V> {
    /// One entry per **file** sequence slot, `None` where that slot's key window moves nothing.
    per_seq: Vec<Option<KeyAnim<V>>>,
}

impl<V: PartialEq> SeqLoops<V> {
    /// Build from one entry per file sequence slot; `None` when no slot animates at all (the
    /// overwhelming majority — the caller then carries no set).
    pub fn new(per_seq: Vec<Option<KeyAnim<V>>>) -> Option<Self> {
        per_seq
            .iter()
            .any(Option::is_some)
            .then_some(Self { per_seq })
    }

    /// The one loop every slot bakes to, or `None` when the slots disagree — including the
    /// asymmetric case a dead slot 0 makes (`None` in one slot, a loop in another), which is the
    /// whole population this type was added for.
    pub fn uniform(&self) -> Option<&KeyAnim<V>> {
        let first = self.per_seq.first()?;
        self.per_seq
            .iter()
            .all(|s| s == first)
            .then_some(first.as_ref())
            .flatten()
    }

    /// This batch's loop in sequence slot `seq` — slot 0 when unknown or out of range, the same
    /// degrade [`super::mat_anim::AlphaAnim::seq`] makes and for the same reason (a consumer with
    /// no live sequence is one that arms slot 0 and never leaves it).
    pub fn seq(&self, seq: Option<usize>) -> Option<&KeyAnim<V>> {
        seq.and_then(|i| self.per_seq.get(i))
            .or_else(|| self.per_seq.first())?
            .as_ref()
    }

    /// Every slot's loop, file order — the census instrument's read
    /// (`benilla-extract uvslotscan`), which asks this type rather than a hand-rolled twin of the
    /// sampler (the `idleslotscan` rule: an instrument that re-implements the gate can drift from
    /// it).
    pub fn slots(&self) -> &[Option<KeyAnim<V>>] {
        &self.per_seq
    }
}

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
    // A constant has no clock at all (`period == 0` short-circuits the sampler), so `wrap` and
    // `gseq` are arbitrary — `true`/`false` keep it the identity value it has always been.
    let constant = |v: V| KeyAnim {
        period: 0.0,
        step,
        wrap: true,
        gseq: false,
        keys: vec![(0.0, v)],
    };
    // An all-equal track is a constant in every band: dropped when the static path owns it, else a
    // period-0 key. Judged before the band split, because that is the shape the static path tested.
    let (_, first) = keys[0];
    if keys.iter().all(|&(_, v)| v == first) {
        if drop_constant(first) {
            return None;
        }
        return Some(constant(first));
    }
    if track.gseq != 0xffff {
        // Global-sequence clock: keys are already loop-relative ms; wrap on the table duration
        // (a 0/absent duration degrades to the last key's time — a defensive clamp, not observed).
        let period_ms = gseq_durations
            .get(track.gseq as usize)
            .copied()
            .filter(|&d| d > 0)
            .unwrap_or_else(|| keys.last().map(|&(t, _)| t).unwrap_or(0).max(1));
        return Some(KeyAnim {
            period: period_ms as f32 / 1000.0,
            step,
            // A global sequence runs its own free clock, the same loop in every animation — it
            // always wraps, whatever the playing sequence's own loop flag says.
            wrap: true,
            gseq: true,
            keys: keys.iter().map(|&(t, v)| (t as f32 / 1000.0, v)).collect(),
        });
    }
    // Sequence-timeline track: rebuild the function the reference samples across THIS sequence's
    // band. Its key window is the track's own per-sequence `(lo, hi)` pair; an absent ranges array
    // is the reference's `[track+4] == 0` fallback — search the whole key list.
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
        // A zero-length band samples one instant — nothing to loop.
        return (!is_identity(head)).then(|| constant(head));
    }
    // The band's own keys, plus both endpoints: the reference's function on `[start, end]` is
    // piecewise between consecutive keys, so sampling it AT the keys (and at the band edges, where
    // it is mid-segment toward an out-of-band bracket) reproduces it exactly.
    let mut baked: Vec<(f32, V)> = vec![(0.0, head)];
    for (k, &(t, v)) in keys.iter().enumerate() {
        if k >= lo && k <= hi && t > start && t < end {
            baked.push(((t - start) as f32 / 1000.0, v));
        }
    }
    if let Some(tail) = at(end) {
        baked.push(((end - start) as f32 / 1000.0, tail));
    }
    // A band the track never actually moves through is a constant hold — the ordinary case for a
    // batch authored to appear in one animation and hide in every other.
    if baked.iter().all(|&(_, v)| v == head) {
        return (!is_identity(head)).then(|| constant(head));
    }
    Some(KeyAnim {
        period: ((end - start) as f32 / 1000.0).max(0.001),
        step,
        // The band's own loop flag IS this loop's clock: a one-shot sequence holds its tail.
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

    /// The whole point of the type: **an asymmetric pair is not uniform.** A dead slot 0 beside a
    /// live slot 1 is the shape 22 of the corpus's 32 multi-slot UV batch-channels ship, and the
    /// shared-material lane must be refused for it — a `uniform()` that only compared the *present*
    /// loops would wave the BRM lava bubbles straight back through (bug B98).
    #[test]
    fn a_dead_slot_zero_beside_a_live_slot_is_not_uniform() {
        let set = SeqLoops::new(vec![None, Some(loop_of(3.3, &[(0.0, 0.0), (1.0, 0.6)]))])
            .expect("one slot animates");
        assert!(set.uniform().is_none());
        assert!(set.seq(Some(0)).is_none(), "slot 0 is the dead hold");
        assert!(set.seq(Some(1)).is_some(), "slot 1 is the animation");
    }

    /// The population the shared lane was built for, and keeps: every slot the same loop. `uniform`
    /// hands back that loop, so the caller carries one and nothing changes.
    #[test]
    fn matching_slots_collapse_to_the_one_shared_loop() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), Some(l.clone())]).expect("animates");
        assert_eq!(set.uniform(), Some(&l));
    }

    /// Two loops that differ are refused even though both are live — the DIFFERS class, where the
    /// slot-0 pin renders a real but *wrong* animation rather than a frozen one
    /// (`Deterrence_State_Base` tints red→blue in Stand and green→red in Hold).
    #[test]
    fn differing_live_slots_are_not_uniform_either() {
        let set = SeqLoops::new(vec![
            Some(loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)])),
            Some(loop_of(2.0, &[(0.0, 1.0), (2.0, 0.0)])),
        ])
        .expect("animates");
        assert!(set.uniform().is_none());
    }

    /// Nothing anywhere ⇒ no set at all: the caller carries nothing, as it always has for the
    /// ~98.6 % of batches with no texture transform.
    #[test]
    fn a_set_with_no_live_slot_is_no_set() {
        assert!(SeqLoops::<f32>::new(vec![None, None]).is_none());
    }

    /// An out-of-range or unknown sequence degrades to slot 0 — the same rule `AlphaAnim::seq`
    /// makes, so a consumer with no live sequence reads what a slot-0-arming consumer would.
    #[test]
    fn an_unknown_sequence_degrades_to_slot_zero() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), None]).expect("animates");
        assert_eq!(set.seq(None), Some(&l));
        assert_eq!(set.seq(Some(9)), Some(&l));
    }
}
