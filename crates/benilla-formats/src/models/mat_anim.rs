//! The material-alpha bake: each M2 batch's colour-alpha and transparency-weight tracks as loops
//! the runtime samples per instance, one per sequence. The reference's per-batch alpha is
//! `instanceAlpha × colors[colorIndex].alpha × transparency[transLookup[idx]].weight`, both tracks
//! evaluated every frame (`0x707680`); `A ≤ 0` skips the batch before its blend mode is read, and
//! an opaque batch with `0 < A < 1` draws blended (`0x70c20f`).

use benilla_m2::{M2ScalarTrack, M2Vec3Track};

use super::key_anim::{bake_track, KeyAnim, SeqLoops, SeqSlot};

/// One baked scalar loop, the alpha channels' [`KeyAnim`].
pub type ScalarAnim = KeyAnim<f32>;

impl KeyAnim<f32> {
    /// Sample at `elapsed` seconds; `1.0` for an empty loop.
    pub fn sample(&self, elapsed: f32) -> f32 {
        self.sample_or(elapsed, 1.0)
    }
}

/// One baked RGB loop, the `M2Color` tint the reference multiplies into the vertex colour.
pub type RgbAnim = KeyAnim<[f32; 3]>;

impl KeyAnim<[f32; 3]> {
    /// Sample at `elapsed` seconds; white for an empty loop.
    pub fn sample(&self, elapsed: f32) -> [f32; 3] {
        self.sample_or(elapsed, [1.0, 1.0, 1.0])
    }
}

/// A batch's two alpha factors in one sequence, multiplied at sample time; an absent factor is 1
/// or was folded statically.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlphaSeq {
    pub color: Option<ScalarAnim>,
    pub weight: Option<ScalarAnim>,
}

impl AlphaSeq {
    /// The combined factor `elapsed` seconds into this sequence, `shared_now` being the instance's
    /// global-sequence cursor; each factor takes its own clock ([`KeyAnim::clock`]).
    pub fn sample(&self, elapsed: f32, shared_now: f64) -> f32 {
        let at = |a: &ScalarAnim| a.sample(a.clock(elapsed, shared_now));
        let c = self.color.as_ref().map_or(1.0, at);
        let w = self.weight.as_ref().map_or(1.0, at);
        c * w
    }

    fn is_empty(&self) -> bool {
        self.color.is_none() && self.weight.is_none()
    }
}

/// A batch's animated alpha for the whole model, one [`AlphaSeq`] per file sequence slot (the
/// slot [`benilla_m2::M2Track::ranges`] and [`crate::ModelAnimation::seq_index`] use), since the
/// reference reads the playing sequence's window every frame: hidden in Stand, drawn in Death.
/// A consumer that passes no sequence reads slot 0; the placed-doodad lane does, on its spawn
/// clock. The reference reads bone 0's armed sequence, which for a placed doodad is animation id
/// 0, its variation re-rolled every play window (`0x7121a0`).
#[derive(Clone, Debug, PartialEq)]
pub struct AlphaAnim {
    per_seq: Vec<AlphaSeq>,
}

impl AlphaAnim {
    /// `None` when no sequence animates either factor, the common case.
    pub fn new(per_seq: Vec<AlphaSeq>) -> Option<Self> {
        per_seq
            .iter()
            .any(|s| !s.is_empty())
            .then_some(Self { per_seq })
    }

    /// This batch's factors in slot `seq`, slot 0 when unknown or out of range.
    pub fn seq(&self, seq: Option<usize>) -> &AlphaSeq {
        const IDENTITY: &AlphaSeq = &AlphaSeq {
            color: None,
            weight: None,
        };
        seq.and_then(|i| self.per_seq.get(i))
            .or_else(|| self.per_seq.first())
            .unwrap_or(IDENTITY)
    }

    /// The combined factor `elapsed` seconds into slot `seq` ([`AlphaSeq::sample`]).
    pub fn sample(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> f32 {
        self.seq(seq).sample(elapsed, shared_now)
    }

    /// Every slot's pair in file order, for `benilla-extract entityuvscan`.
    pub fn slots(&self) -> &[AlphaSeq] {
        &self.per_seq
    }

    /// Whether any sequence can drive the alpha to zero, culling the batch.
    pub fn ever_hides(&self) -> bool {
        self.per_seq.iter().any(|s| {
            [s.color.as_ref(), s.weight.as_ref()]
                .into_iter()
                .flatten()
                .any(|a| a.keys.iter().any(|&(_, v)| v <= 0.0))
        })
    }
}

/// Bake one scalar track, or `None` with no keys, a constant 1, or a constant ≤ 0 (the static cull
/// already dropped the batch). A dimming constant is kept, and a band holding a non-1 value bakes
/// that hold, a held 0 included: the static cull never saw it.
pub(super) fn bake_scalar_anim(
    track: &M2ScalarTrack,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
) -> Option<ScalarAnim> {
    bake_track(
        track,
        gseq_durations,
        seq,
        |v| v,
        |c| (c - 1.0).abs() < f32::EPSILON || c <= 0.0,
        |v| (v - 1.0).abs() < f32::EPSILON,
    )
}

/// Bake one `M2Color` RGB track only when it varies in time: constants and holds stay with the
/// static vertex-colour bake in `m2_batches`, which a varying track skips, so the two never
/// double-apply.
pub(super) fn bake_rgb_anim(
    track: &M2Vec3Track,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
) -> Option<RgbAnim> {
    bake_track(track, gseq_durations, seq, |v| v, |_| true, |_| true)
}

/// [`bake_rgb_anim`] per file sequence slot, for a tint keyed differently per sequence
/// (`Spells\\Deterrence_State_Base.m2` goes red to blue in Stand, green to red in Hold).
pub(super) fn bake_rgb_seqs(
    track: &M2Vec3Track,
    gseq_durations: &[u32],
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 3]>> {
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| bake_track(track, gseq_durations, Some(slot), |v| v, |_| true, |_| true))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Stormwind mage portal's shimmer is transparency weight, 3 of its 4 tracks time-varying.
    #[test]
    fn mage_portal_bakes_a_time_varying_weight_loop() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\generic\\activedoodads\\mageportals\\StormwindMagePortal01.m2")
            .expect("read StormwindMagePortal01.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let animated: Vec<&ScalarAnim> = subs
            .iter()
            .filter_map(|s| s.alpha_anim.as_ref())
            .filter_map(|a| a.seq(None).weight.as_ref())
            .filter(|w| w.period > 0.0 && w.keys.len() > 1)
            .collect();
        assert!(
            !animated.is_empty(),
            "the portal's shimmer batches carry time-varying weight loops"
        );
        let w = animated[0];
        let (a, b) = (w.sample(0.0), w.sample(w.period * 0.25));
        assert!(
            (a - b).abs() > 1e-3,
            "sampled weight varies over the loop (got {a} vs {b}, period {})",
            w.period
        );
        assert!(
            w.keys.iter().all(|&(_, v)| (0.0..=1.0).contains(&v)),
            "fix16 weights decode into [0, 1]"
        );
    }

    /// `TanarisTrollGate.m2` holds an intact gate and its burnt twin, each with a tiki mask. Its
    /// `M2Color` alpha keys are `0x7fff` (+1.0) on the copy a sequence shows and `0x8001` (−1.0) on
    /// the one it hides; only a signed decode lets the `A ≤ 0` cull hide it.
    #[test]
    fn troll_gate_never_draws_its_burnt_twin_at_the_same_time() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Kalimdor\\Tanaris\\ActiveDoodads\\TrollGate\\TanarisTrollGate.m2")
            .expect("read TanarisTrollGate.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        // File slots: 0 Closed, 1 Open, 2 Opened, 3 Destroy, 4 Close.
        let (closed, destroy) = (Some(0), Some(3));
        let alpha_of = |tex: &str, seq| -> Vec<f32> {
            subs.iter()
                .filter(|s| {
                    s.texture
                        .as_deref()
                        .is_some_and(|t| t.to_ascii_uppercase().contains(tex))
                })
                .map(|s| {
                    s.alpha_anim
                        .as_ref()
                        .map_or(1.0, |a| a.sample(seq, 0.0, 0.0))
                })
                .collect()
        };
        let intact_closed = alpha_of("TANARISTROLLGATE.BLP", closed);
        let burnt_closed = alpha_of("TANARISTROLLGATEBURNT.BLP", closed);
        assert!(
            !intact_closed.is_empty() && !burnt_closed.is_empty(),
            "both gate copies are batches of this model (intact {}, burnt {})",
            intact_closed.len(),
            burnt_closed.len()
        );
        assert!(
            intact_closed.iter().all(|&a| a > 0.0),
            "a shut gate draws its INTACT self (Closed alphas {intact_closed:?})"
        );
        assert!(
            burnt_closed.iter().all(|&a| a <= 0.0),
            "a shut gate hides its BURNT twin — ±1 keys read signed ({burnt_closed:?})"
        );
        assert!(
            alpha_of("TANARISTROLLGATE.BLP", destroy)
                .iter()
                .all(|&a| a <= 0.0),
            "…and Destroy swaps them: the intact copy hides"
        );
        assert!(
            alpha_of("TANARISTROLLGATEBURNT.BLP", destroy)
                .iter()
                .all(|&a| a > 0.0),
            "…and the burnt copy shows"
        );
        for seq in [closed, destroy] {
            let masks = alpha_of("BM_TROLL_TIKI03.BLP", seq);
            assert_eq!(
                masks.len(),
                2,
                "the model authors the mask twice ({masks:?})"
            );
            assert_eq!(
                masks.iter().filter(|&&a| a > 0.0).count(),
                1,
                "exactly one mask copy draws per sequence ({masks:?})"
            );
        }
    }

    /// Battle Shout's six additive crescents: staggered alpha pulses, dimming weight constants
    /// (0.2 to 0.4), and RGB tracks cooling from white to red over the 0.9 s clip.
    #[test]
    fn battle_shout_base_bakes_alpha_and_rgb_loops() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Spells\\BattleShout_Cast_Base.m2")
            .expect("read BattleShout_Cast_Base.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        assert_eq!(subs.len(), 6, "six crescent batches");
        for (i, s) in subs.iter().enumerate() {
            let a = s.alpha_anim.as_ref().unwrap_or_else(|| {
                panic!("batch {i}: the colour-alpha/weight loops must bake");
            });
            let seq0 = a.seq(None);
            let c = seq0.color.as_ref().expect("time-varying colour-alpha");
            assert!(
                c.period > 0.0 && c.keys.len() > 1,
                "batch {i}: alpha varies"
            );
            let w = seq0.weight.as_ref().expect("dimming weight constant");
            let wv = w.sample(0.0);
            assert!(
                (0.19..=0.41).contains(&wv),
                "batch {i}: weight dims to 0.2–0.4 (got {wv})"
            );
            let rgb = s.rgb_anim.as_ref().unwrap_or_else(|| {
                panic!("batch {i}: the time-varying RGB tint must bake");
            });
            assert!(
                rgb.period > 0.0 && rgb.keys.len() > 1,
                "batch {i}: RGB varies"
            );
            let mid = rgb.sample(rgb.period * 0.5);
            assert!(
                mid[0] > 0.6 && mid[1] < 0.1 && mid[2] < 0.1,
                "batch {i}: mid-life tint is red (got {mid:?})"
            );
            assert!(
                s.vertex_colors.is_empty(),
                "batch {i}: the static vertex tint is skipped when the RGB animates"
            );
        }
    }

    /// The voidwalker's skin batches 0 and 1, two shoulder pieces above the body, have weight 0 in
    /// every ordinary sequence and 1 only in Death/131/132; their twins on the body always draw.
    #[test]
    fn voidwalker_hides_its_death_only_armour_in_every_other_sequence() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Creature\\VoidWalker\\VoidWalker.m2")
            .expect("read VoidWalker.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        // File slots (`benilla-extract m2seq`): 0 Stand (3333..6000 ms), 1 Walk, 2 Run, 5/6 the
        // Stand variations, 10 Death (anim 1, 43333..46333 ms).
        const STAND: usize = 0;
        const DEATH: usize = 10;
        for batch in [0, 1] {
            let a = subs[batch]
                .alpha_anim
                .as_ref()
                .unwrap_or_else(|| panic!("batch {batch} carries the weight track"));
            for step in 0..16u16 {
                let t = 2.667 * f32::from(step) / 16.0;
                assert_eq!(
                    a.sample(Some(STAND), t, 0.0),
                    0.0,
                    "batch {batch} is hidden {t}s into Stand"
                );
            }
            for slot in [1, 2, 7, 8, 9, 11] {
                assert_eq!(
                    a.sample(Some(slot), 0.3, 0.0),
                    0.0,
                    "batch {batch}, sequence {slot}"
                );
            }
            let peak = (0..32u16)
                .map(|s| a.sample(Some(DEATH), 3.0 * f32::from(s) / 32.0, 0.0))
                .fold(0.0f32, f32::max);
            assert!(
                peak > 0.9,
                "batch {batch} appears during Death (peak {peak})"
            );
        }
        for batch in [4, 5] {
            let drawn = subs[batch]
                .alpha_anim
                .as_ref()
                .is_none_or(|a| a.sample(Some(STAND), 0.5, 0.0) > 0.0);
            assert!(drawn, "batch {batch} draws in Stand");
        }
        // Batch 2, the body, is a step track keyed 1.0 at 3333 ms and 0.0 at 44200 ms. The Stand
        // variations (23333..29333 ms) key nothing and sit nearer the 0, but the reference's window
        // says key 0, so the body stays visible.
        let body = subs[2].alpha_anim.as_ref();
        for slot in [5, 6, 7, 8, 9, 12] {
            let v = body.map_or(1.0, |a| a.sample(Some(slot), 0.5, 0.0));
            assert!(
                v > 0.0,
                "the voidwalker body draws in sequence {slot} (got {v})"
            );
        }
    }

    /// The banshee's render batches 20 to 25 are weight 0 in every live sequence and come in across
    /// Death. Render, not skin, indices (a billboard batch splits per bone; `benilla-extract
    /// m2alpha` shows both), each re-checked by its vertex count.
    #[test]
    fn banshee_hides_its_death_only_batches() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Creature\\Banshee\\Banshee.m2")
            .expect("read Banshee.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        assert_eq!(subs.len(), 26, "the banshee's render batch list");
        // File slots (`m2seq`): 0 Stand (10000..13333 ms), 3 Walk, 4 Run, 12 Death.
        const LIVE: [usize; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        const DEATH: usize = 12;
        for (batch, verts) in [(20, 57), (21, 60), (22, 4), (23, 4), (24, 4), (25, 4)] {
            assert_eq!(subs[batch].positions.len(), verts, "batch {batch} identity");
            let a = subs[batch]
                .alpha_anim
                .as_ref()
                .unwrap_or_else(|| panic!("batch {batch} is alpha-keyed"));
            for slot in LIVE {
                for step in 0..8u16 {
                    let t = f32::from(step) * 0.4;
                    assert_eq!(
                        a.sample(Some(slot), t, 0.0),
                        0.0,
                        "batch {batch} is hidden {t}s into sequence {slot}"
                    );
                }
            }
            let peak = (0..64u16)
                .map(|k| a.sample(Some(DEATH), 4.167 * f32::from(k) / 64.0, 0.0))
                .fold(0.0f32, f32::max);
            assert!(peak > 0.3, "batch {batch} appears on death (peak {peak})");
        }
        // Everything else draws while standing but batch 8, the `BITCHFACE5.BLP` scream face:
        // weight 0 standing, pulsed to 0.6 by the attack and emote sequences.
        for (i, sub) in subs.iter().enumerate().take(20).filter(|(i, _)| *i != 8) {
            let v = sub
                .alpha_anim
                .as_ref()
                .map_or(1.0, |a| a.sample(Some(0), 0.5, 0.0));
            assert!(v > 0.0, "batch {i} draws while standing (got {v})");
        }
        let face = subs[8]
            .alpha_anim
            .as_ref()
            .expect("the face batch is alpha-keyed");
        assert_eq!(
            face.sample(Some(0), 0.5, 0.0),
            0.0,
            "the scream face hides while standing"
        );
        let scream = (0..64u16)
            .map(|k| face.sample(Some(1), f32::from(k) / 64.0, 0.0))
            .fold(0.0f32, f32::max);
        assert!(
            scream > 0.5,
            "and pulses in during the emote (peak {scream})"
        );
    }

    fn track(gseq: u16, interp: u16, keys: &[(u32, f32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq,
            ranges: Vec::new(),
            keys: keys.to_vec(),
        }
    }

    /// The same track with per-sequence key windows, which the reference indexes by file slot.
    fn ranged(interp: u16, keys: &[(u32, f32)], ranges: &[(u32, u32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq: 0xffff,
            ranges: ranges.to_vec(),
            keys: keys.to_vec(),
        }
    }

    /// Bake against looping file slot `index` over `band`.
    fn bake_at(t: &M2ScalarTrack, index: usize, band: (u32, u32)) -> Option<ScalarAnim> {
        bake_scalar_anim(
            t,
            &[],
            Some(SeqSlot {
                index,
                band,
                looping: true,
            }),
        )
    }

    /// Bake against a one-shot file slot, the Death-band shape.
    fn bake_at_clamped(t: &M2ScalarTrack, index: usize, band: (u32, u32)) -> Option<ScalarAnim> {
        bake_scalar_anim(
            t,
            &[],
            Some(SeqSlot {
                index,
                band,
                looping: false,
            }),
        )
    }

    #[test]
    fn bake_keeps_only_what_the_static_path_cannot_do() {
        assert_eq!(bake_scalar_anim(&track(0xffff, 1, &[]), &[], None), None);
        assert_eq!(
            bake_scalar_anim(&track(0xffff, 1, &[(0, 1.0)]), &[], None),
            None
        );
        assert_eq!(
            bake_scalar_anim(&track(0xffff, 1, &[(0, 0.0), (500, 0.0)]), &[], None),
            None
        );
        let dim = bake_scalar_anim(&track(0xffff, 1, &[(0, 0.4)]), &[], None).unwrap();
        assert_eq!(dim.period, 0.0);
        assert_eq!(dim.sample(123.0), 0.4);
    }

    #[test]
    fn gseq_track_wraps_the_table_duration() {
        let a = bake_scalar_anim(
            &track(1, 1, &[(0, 0.2), (750, 1.0), (1500, 0.2)]),
            &[9999, 1500],
            None,
        )
        .unwrap();
        assert_eq!(a.period, 1.5);
        assert!((a.sample(0.375) - 0.6).abs() < 1e-4); // linear midpoint
        assert!((a.sample(1.5 + 0.375) - 0.6).abs() < 1e-4); // wraps
    }

    #[test]
    fn gseq_loops_read_the_shared_clock() {
        let g = bake_scalar_anim(
            &track(1, 1, &[(0, 0.2), (750, 1.0), (1500, 0.2)]),
            &[9999, 1500],
            None,
        )
        .unwrap();
        assert!(g.gseq);
        // band_t is ignored; the gseq cursor decides the phase (mid-ramp, 0.6).
        assert!((g.sample(g.clock(0.0, 1500.375)) - 0.6).abs() < 1e-3);
        let b = ranged(1, &[(1000, 0.0), (1500, 1.0)], &[(0, 1)]);
        let b = bake_at(&b, 0, (1000, 2000)).unwrap();
        assert!(!b.gseq);
        assert_eq!(
            b.clock(0.25, 999.0),
            0.25,
            "a band loop keeps its own clock"
        );
    }

    #[test]
    fn sequence_track_bakes_one_band_at_a_time() {
        // Keys 0..2; sequence 0's window is keys 0-1, sequence 1's is keys 1-2.
        let t = ranged(
            1,
            &[(1000, 0.0), (1500, 1.0), (5000, 0.3)],
            &[(0, 1), (1, 2)],
        );
        let a = bake_at(&t, 0, (1000, 2000)).unwrap();
        assert_eq!(a.period, 1.0);
        assert!((a.sample(0.25) - 0.5).abs() < 1e-6);
        // Past the band's last key the value ramps on toward key 2 of a later sequence: the
        // reference bounds `k1 = k0 + 1` by the key count, not the window's `hi` (`0x713d50`).
        let expect = 1.0 + (0.3 - 1.0) * (1900.0 - 1500.0) / (5000.0 - 1500.0);
        assert!(
            (a.sample(0.9) - expect).abs() < 1e-4,
            "got {} want {expect}",
            a.sample(0.9)
        );
        // Band 1 opens mid-segment (5/7 along the 1.0 to 0.3 ramp at 4000 ms) and closes on key 2,
        // sampled just under the period, where the loop has not yet wrapped.
        let b = bake_at(&t, 1, (4000, 5000)).unwrap();
        assert!((b.sample(0.0) - 0.5).abs() < 1e-4, "got {}", b.sample(0.0));
        assert!(
            (b.sample(0.999) - 0.3).abs() < 1e-3,
            "got {}",
            b.sample(0.999)
        );
    }

    #[test]
    fn each_sequence_bakes_its_own_band() {
        let t = ranged(
            1,
            &[(1000, 0.0), (2000, 0.0), (4000, 1.0), (5000, 1.0)],
            &[(0, 1), (2, 3)],
        );
        let stand = bake_at(&t, 0, (1000, 2000)).unwrap();
        assert_eq!(stand.sample(0.0), 0.0, "hidden for the whole first band");
        assert_eq!(stand.sample(0.9), 0.0);
        let death = bake_at(&t, 1, (4000, 5000));
        assert_eq!(death, None, "the second band is a plain visible batch");
    }

    /// A band that keys nothing holds `keys[ranges[slot].lo]`, the reference's collapsed window
    /// (`lo >= hi`, `0x713d50`).
    #[test]
    fn band_empty_track_holds_its_bracket_key() {
        // Slot 1's window has collapsed onto key 0, value 0.
        let t = ranged(1, &[(100, 0.0), (5000, 1.0)], &[(0, 1), (0, 0)]);
        let a = bake_at(&t, 1, (1000, 2000)).unwrap();
        assert_eq!(a.period, 0.0);
        assert_eq!(a.sample(42.0), 0.0);
        let one = ranged(1, &[(100, 1.0), (5000, 0.3)], &[(0, 1), (0, 0)]);
        assert_eq!(bake_at(&one, 1, (1000, 2000)), None);
    }

    /// A step track whose band sits between a 1 and a later 0 holds the 1: the window's low index
    /// is the search result for every time in the band.
    #[test]
    fn empty_band_uses_the_window_low_key_not_the_nearest() {
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1), (0, 1)]);
        // Band 23333..26000 keys nothing and sits nearer the 0.
        let a = bake_at(&t, 1, (23333, 26000));
        assert_eq!(a, None, "the batch stays visible (constant 1 = identity)");
    }

    /// No ranges array is the reference's `[track+4] == 0` fallback: the whole key list.
    #[test]
    fn missing_ranges_search_the_whole_key_list() {
        let t = track(0xffff, 0, &[(0, 1.0), (10_000, 0.0)]);
        let a = bake_at(&t, 7, (20_000, 21_000)).unwrap();
        assert_eq!(a.sample(0.5), 0.0, "past the last key: hold it");
    }

    /// A one-shot band whose weight steps 1 to 0: the clock parks at or just past the band end for
    /// the corpse's life, and the loop must hold the faded tail there.
    #[test]
    fn a_clamped_band_holds_its_faded_tail() {
        // Band 43333..46333 (3 s), the elemental's Death; the body's weight steps to 0 at 867 ms.
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1)]);
        let a = bake_at_clamped(&t, 0, (43333, 46333)).expect("the band moves, so it bakes");
        assert!(!a.wrap, "a one-shot band clamps its clock");
        assert_eq!(a.sample(0.0), 1.0, "the body is still there as death opens");
        assert_eq!(
            a.sample(2.999),
            0.0,
            "…and gone by the end of the animation"
        );
        assert_eq!(a.sample(3.0), 0.0, "t == period must not alias to the head");
        assert_eq!(
            a.sample(3.008),
            0.0,
            "where a finished Bevy clip actually parks"
        );
        assert_eq!(a.sample(600.0), 0.0, "still gone a corpse-decay later");
    }

    #[test]
    fn a_looping_band_still_wraps() {
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1)]);
        let a = bake_at(&t, 0, (43333, 46333)).expect("the band moves, so it bakes");
        assert!(a.wrap, "a looping band wraps its clock");
        assert_eq!(a.sample(2.999), 0.0, "first pass: faded out");
        assert_eq!(a.sample(3.0), 1.0, "second pass: the window re-fires");
    }

    /// `AirElemental.m2`'s Death band fades the body to 0, and the parked clock must still read 0.
    #[test]
    fn air_elemental_death_leaves_no_opaque_body() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Creature\\AirElemental\\AirElemental.m2")
            .expect("read AirElemental.m2");
        let subs = super::super::parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        // Slot 10 is anim id 1 (Death), one-shot, band 43333..46333: a period of exactly 3.0 s.
        const DEATH: usize = 10;
        let faded: Vec<usize> = subs
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.alpha_anim
                    .as_ref()
                    .is_some_and(|a| a.sample(Some(DEATH), 2.999, 0.0) <= 0.0)
            })
            .map(|(i, _)| i)
            .collect();
        assert!(
            faded.len() > 20,
            "the death animation hides most of the body by its end, got {}",
            faded.len()
        );
        for i in faded {
            let a = subs[i].alpha_anim.as_ref().unwrap();
            for t in [3.0f32, 3.008, 10.0, 600.0] {
                assert_eq!(
                    a.sample(Some(DEATH), t, 0.0),
                    0.0,
                    "batch {i} came back at t={t} — the parked clock aliased to the band head"
                );
            }
        }
    }

    #[test]
    fn step_tracks_hold_between_keys() {
        let a = bake_scalar_anim(&track(0, 0, &[(0, 0.2), (1000, 1.0)]), &[2000], None).unwrap();
        assert!(a.step);
        assert_eq!(a.sample(0.999), 0.2);
        assert_eq!(a.sample(1.0), 1.0);
    }

    fn dim(v: f32) -> Option<ScalarAnim> {
        Some(ScalarAnim {
            period: 0.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, v)],
        })
    }

    #[test]
    fn alpha_anim_multiplies_color_and_weight() {
        let both = AlphaAnim::new(vec![AlphaSeq {
            color: dim(0.5),
            weight: dim(0.5),
        }])
        .expect("a dimming pair is worth carrying");
        assert!((both.sample(None, 7.0, 0.0) - 0.25).abs() < 1e-6);
        assert_eq!(AlphaAnim::new(vec![AlphaSeq::default()]), None);
    }

    #[test]
    fn alpha_anim_addresses_by_sequence_slot() {
        let a = AlphaAnim::new(vec![
            AlphaSeq {
                color: None,
                weight: dim(0.0),
            },
            AlphaSeq::default(),
            AlphaSeq {
                color: None,
                weight: dim(0.5),
            },
        ])
        .unwrap();
        assert_eq!(a.sample(Some(0), 0.0, 0.0), 0.0);
        assert_eq!(a.sample(Some(1), 0.0, 0.0), 1.0);
        assert_eq!(a.sample(Some(2), 0.0, 0.0), 0.5);
        assert_eq!(a.sample(Some(99), 0.0, 0.0), 0.0, "out of range ⇒ slot 0");
        assert_eq!(a.sample(None, 0.0, 0.0), 0.0, "unknown sequence ⇒ slot 0");
        assert!(a.ever_hides());
        let never = AlphaAnim::new(vec![AlphaSeq {
            color: None,
            weight: dim(0.5),
        }])
        .unwrap();
        assert!(!never.ever_hides());
    }
}
