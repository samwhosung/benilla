//! Particle emitter tracks sampled per sequence: the spawn rate (`+0xdc`) and enabled gate
//! (`+0x1dc`) in [`EmitTiming`], the other nine parameters in [`EmitParams`], each baked one loop
//! per file sequence slot by the reference's key-window kernel (`0x713d50`).

use benilla_m2::M2ScalarTrack;

use crate::models::{bake_track, ScalarAnim, SeqSlot};

/// The spawn rate and enabled gate per file sequence slot. The reference's animate kernel
/// (`0x714260`) samples both through the playing sequence's key window every frame, and the rate
/// is 0 while the gate is off (`0x717d90`, `0x718f32`). A looping slot wraps its band, a clamped
/// one holds its tail, a gseq-tagged track runs on its own clock; `None` or an unknown `seq` is
/// slot 0.
#[derive(Debug, Clone, Default)]
pub struct EmitTiming {
    /// Per file slot; `None` where the track keys nothing (rate 0).
    rate: Vec<Option<ScalarAnim>>,
    /// Per file slot, stepped 0 or 1; `None` is on, the loader default (`0x710092` sets
    /// `block+0x14c = 1`).
    enabled: Vec<Option<ScalarAnim>>,
    /// Per file slot: the band loops when sequence flags bit 0 is clear.
    looping: Vec<bool>,
}

impl EmitTiming {
    /// Bake both tracks per file sequence slot; `gseq` is the global-sequence duration table.
    pub(crate) fn bake(
        rate: &M2ScalarTrack,
        enabled: &M2ScalarTrack,
        slots: &[SeqSlot],
        gseq: &[u32],
    ) -> Self {
        // Keep every shape, constants included (`|_| false` twice): there is no static fallback.
        let per_slot = |t: &M2ScalarTrack| -> Vec<Option<ScalarAnim>> {
            slots
                .iter()
                .map(|&s| bake_track(t, gseq, Some(s), |v| v, |_| false, |_| false))
                .collect()
        };
        Self {
            rate: per_slot(rate),
            enabled: per_slot(enabled),
            looping: slots.iter().map(|s| s.looping).collect(),
        }
    }

    fn idx(&self, seq: Option<usize>) -> usize {
        match seq {
            Some(i) if i < self.looping.len() => i,
            _ => 0,
        }
    }

    /// Whether the gate is on `elapsed` seconds into slot `seq`; `shared_now`, the world clock in
    /// seconds, drives a gseq-tagged gate.
    pub fn emitting(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> bool {
        self.enabled
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .is_none_or(|a| a.sample_or(a.clock(elapsed, shared_now), 1.0) > 0.5)
    }

    /// Particles per second `elapsed` seconds into slot `seq`, floored at 0 as a track tail can go
    /// negative; `shared_now` drives a gseq-tagged rate.
    pub fn rate(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> f32 {
        self.rate
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .map_or(0.0, |a| a.sample_or(a.clock(elapsed, shared_now), 0.0))
            .max(0.0)
    }

    /// The rate's peak over every slot, for the spawn cull: a burst emitter's first key is 0.
    pub fn peak_rate(&self) -> f32 {
        self.rate
            .iter()
            .flatten()
            .flat_map(|a| a.keys.iter().map(|&(_, v)| v))
            .fold(0.0, f32::max)
    }

    /// The first burst on slot `seq` as `(seconds in, particle count)`, on a 60 Hz grid: the
    /// reference fires `ftol(rate)` particles on the rising edge of `enabled != 0 && rate > 0`
    /// (`0x718ed2`-`0x718ef6`). `None` is a shipped shape: `Spells\\Strike_Impact_Chest.m2`'s gold
    /// flare turns its gate off on the key its rate rises.
    pub fn first_burst(&self, seq: Option<usize>) -> Option<(f32, f32)> {
        let i = self.idx(seq);
        let span = |o: &Option<ScalarAnim>| -> f32 {
            o.as_ref()
                .and_then(|a| a.keys.last())
                .map_or(0.0, |&(t, _)| t)
        };
        let end = span(self.rate.get(i)?).max(span(self.enabled.get(i)?));
        const STEP: f32 = 1.0 / 60.0;
        let mut frame = 0;
        loop {
            let t = frame as f32 * STEP;
            if t > end + STEP {
                return None;
            }
            let rate = self.rate(seq, t, 0.0);
            if rate > 0.0 && self.emitting(seq, t, 0.0) {
                return Some((t, rate.trunc()));
            }
            frame += 1;
        }
    }

    /// `Some(rate)` when every slot bakes the same single-key rate, the common shape.
    pub fn constant_rate(&self) -> Option<f32> {
        let mut it = self.rate.iter();
        let first = it.next()?.as_ref()?;
        let &(_, v) = (first.keys.len() == 1).then(|| first.keys.first())??;
        it.all(|a| a.as_ref().is_some_and(|a| a.keys == first.keys))
            .then_some(v)
    }

    /// `(looping, rate keys, enabled keys)` per file slot, in seconds from the band start.
    #[allow(clippy::type_complexity)] // a read-only tuple view for the dumps
    pub fn slot_views(&self) -> Vec<(bool, Option<&[(f32, f32)]>, Option<&[(f32, f32)]>)> {
        fn keys(list: &[Option<ScalarAnim>], i: usize) -> Option<&[(f32, f32)]> {
            list.get(i)
                .and_then(|o| o.as_ref())
                .map(|a| a.keys.as_slice())
        }
        (0..self.looping.len())
            .map(|i| (self.looping[i], keys(&self.rate, i), keys(&self.enabled, i)))
            .collect()
    }

    /// Test/tool constructor: a single always-on slot at a constant `rate`, looping.
    pub fn constant(rate: f32) -> Self {
        Self {
            rate: vec![Some(ScalarAnim {
                period: 0.0,
                step: true,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, rate)],
            })],
            enabled: vec![None],
            looping: vec![true],
        }
    }
}

/// One frame's sample of the emitter's nine parameter tracks (`+0x34..+0x130`, all but rate and
/// gate). The reference samples them every frame into the emitter's animation block
/// (`[model+0x3d0]`, stride 0x16c) as it does the rate (`0x71850d`), and they animate:
/// `Frost_Nova_area`'s emission sphere grows from 0.19 to 13.2 yd.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamsNow {
    /// Initial particle speed (yards/sec).
    pub emission_speed: f32,
    /// Fractional random speed spread (`speed·(1 ± var·noise)`).
    pub speed_variation: f32,
    /// Half-angle of the emission cone, radians (sphere: latitude range; spline: tangent spin ψ).
    pub vertical_range: f32,
    /// Azimuthal spread, radians (sphere: longitude range; spline: scatter jitter).
    pub horizontal_range: f32,
    /// Downward acceleration (yards/sec²), read live every frame by the integrator (`0x7b2680`).
    pub gravity: f32,
    /// Particle lifetime (seconds); a birth keeps the value current at its spawn (the spawn
    /// kernels' `ebp+0xc`).
    pub lifespan: f32,
    /// Plane: full x-extent (±½ rect); sphere: minimum radius; spline: tMin.
    pub area_length: f32,
    /// Plane: full y-extent; sphere: maximum radius; spline: tMax.
    pub area_width: f32,
    /// zSource: velocity pivot at `(0, 0, z)`, 0 for none.
    pub z_source: f32,
}

impl Default for ParamsNow {
    /// The channel defaults a keyless track holds: zeros, except lifespan's loader default 1.0.
    fn default() -> Self {
        Self {
            emission_speed: 0.0,
            speed_variation: 0.0,
            vertical_range: 0.0,
            horizontal_range: 0.0,
            gravity: 0.0,
            lifespan: 1.0,
            area_length: 0.0,
            area_width: 0.0,
            z_source: 0.0,
        }
    }
}

/// The nine parameter tracks, baked per file sequence slot under [`EmitTiming`]'s law; sampled
/// once per frame on the emitter's clock.
#[derive(Debug, Clone, Default)]
pub struct EmitParams {
    /// Per channel in [`ParamsNow`] field order, per slot; `None` holds the channel default.
    channels: [Vec<Option<ScalarAnim>>; 9],
}

impl EmitParams {
    /// Bake the nine tracks (in [`ParamsNow`] field order) against every file sequence slot.
    pub(crate) fn bake(tracks: [&M2ScalarTrack; 9], slots: &[SeqSlot], gseq: &[u32]) -> Self {
        Self {
            channels: tracks.map(|t| {
                slots
                    .iter()
                    .map(|&s| bake_track(t, gseq, Some(s), |v| v, |_| false, |_| false))
                    .collect()
            }),
        }
    }

    /// Every channel `elapsed` seconds into slot `seq`, resolved as [`EmitTiming`] resolves it.
    pub fn sample(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> ParamsNow {
        let d = ParamsNow::default();
        let at = |i: usize, default: f32| -> f32 {
            let ch = &self.channels[i];
            let slot = match seq {
                Some(s) if s < ch.len() => s,
                _ => 0,
            };
            ch.get(slot).and_then(|o| o.as_ref()).map_or(default, |a| {
                a.sample_or(a.clock(elapsed, shared_now), default)
            })
        };
        ParamsNow {
            emission_speed: at(0, d.emission_speed),
            speed_variation: at(1, d.speed_variation),
            vertical_range: at(2, d.vertical_range),
            horizontal_range: at(3, d.horizontal_range),
            gravity: at(4, d.gravity),
            lifespan: at(5, d.lifespan),
            area_length: at(6, d.area_length),
            area_width: at(7, d.area_width),
            z_source: at(8, d.z_source),
        }
    }

    /// The lifespan's peak over every slot, for the spawn cull. A keyless channel is the loader
    /// default; a keyed one folds only its keys, so an authored peak under 1.0 is not masked.
    pub fn peak_lifespan(&self) -> f32 {
        let mut keys = self.channels[5]
            .iter()
            .flatten()
            .flat_map(|a| a.keys.iter().map(|&(_, v)| v))
            .peekable();
        if keys.peek().is_none() {
            return ParamsNow::default().lifespan;
        }
        keys.fold(0.0, f32::max)
    }

    /// Constant channels from one [`ParamsNow`], for tests and tools.
    pub fn constant(now: ParamsNow) -> Self {
        let ch = |v: f32| {
            vec![Some(ScalarAnim {
                period: 0.0,
                step: true,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, v)],
            })]
        };
        Self {
            channels: [
                ch(now.emission_speed),
                ch(now.speed_variation),
                ch(now.vertical_range),
                ch(now.horizontal_range),
                ch(now.gravity),
                ch(now.lifespan),
                ch(now.area_length),
                ch(now.area_width),
                ch(now.z_source),
            ],
        }
    }

    /// `(name, per-slot keys)` per channel.
    #[allow(clippy::type_complexity)] // a read-only tuple view for the dumps, like `slot_views`
    pub fn channel_views(&self) -> [(&'static str, Vec<Option<&[(f32, f32)]>>); 9] {
        const NAMES: [&str; 9] = [
            "speed",
            "speedVar",
            "latitude",
            "longitude",
            "gravity",
            "lifespan",
            "areaLength",
            "areaWidth",
            "zSource",
        ];
        let mut i = 0;
        NAMES.map(|name| {
            let v = self.channels[i]
                .iter()
                .map(|o| o.as_ref().map(|a| a.keys.as_slice()))
                .collect();
            i += 1;
            (name, v)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(interp: u16, keys: &[(u32, f32)], ranges: &[(u32, u32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq: 0xffff,
            ranges: ranges.to_vec(),
            keys: keys.to_vec(),
        }
    }

    fn slots(spec: &[((u32, u32), bool)]) -> Vec<SeqSlot> {
        spec.iter()
            .enumerate()
            .map(|(index, &(band, looping))| SeqSlot {
                index,
                band,
                looping,
            })
            .collect()
    }

    #[test]
    fn step_rate_is_silent_before_its_key_and_holds_after() {
        let t = EmitTiming::bake(
            &track(0, &[(0, 0.0), (67, 30.0)], &[]),
            &M2ScalarTrack::default(),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert_eq!(
            t.rate(None, 0.050, 0.0),
            0.0,
            "before the key: step holds 0"
        );
        assert_eq!(t.rate(None, 0.070, 0.0), 30.0, "at/after the key: 30");
        assert_eq!(t.rate(None, 0.500, 0.0), 30.0, "held to the band end");
        assert!(t.emitting(None, 0.5, 0.0), "no gate track = always on");
    }

    /// BloodSpurt's linear `0, 100, 0` rate (interp 1).
    #[test]
    fn lerp_ramp_interpolates_and_self_closes() {
        let t = EmitTiming::bake(
            &track(1, &[(0, 0.0), (100, 100.0), (200, 0.0)], &[]),
            &M2ScalarTrack::default(),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert!(
            (t.rate(None, 0.050, 0.0) - 50.0).abs() < 1e-4,
            "rising mid-ramp"
        );
        assert!(
            (t.rate(None, 0.150, 0.0) - 50.0).abs() < 1e-4,
            "falling mid-ramp"
        );
        assert_eq!(t.rate(None, 0.500, 0.0), 0.0, "the ramp self-closes");
    }

    #[test]
    fn idle_window_is_off_and_a_clamped_clip_holds_its_tail() {
        // Absolute timeline: clip A (one-shot) band 1000..2000, idle band 2333..2667.
        // Gate keys: on@1000, off@1333, on@3800 (a later clip's window, outside both bands).
        let gate = track(
            0,
            &[(1000, 1.0), (1333, 0.0), (3800, 1.0)],
            &[(0, 1), (1, 1), (2, 2)],
        );
        let t = EmitTiming::bake(
            &track(0, &[(0, 20.0)], &[]),
            &gate,
            &slots(&[
                ((1000, 2000), false),
                ((2333, 2667), true),
                ((3800, 4100), false),
            ]),
            &[],
        );
        // Idle (slot 1): the collapsed window (1,1) is keys[1], off at every time.
        assert!(!t.emitting(Some(1), 0.0, 0.0));
        assert!(!t.emitting(Some(1), 0.25, 0.0));
        assert!(!t.emitting(Some(1), 400.0, 0.0));
        // The clip (slot 0): on, off from 333 ms, and held off past the band end.
        assert!(t.emitting(Some(0), 0.1, 0.0));
        assert!(!t.emitting(Some(0), 0.5, 0.0));
        assert!(
            !t.emitting(Some(0), 1.0, 0.0),
            "t == period must not alias to 0"
        );
        assert!(
            !t.emitting(Some(0), 5.0, 0.0),
            "parked long past the end: still off"
        );
        // The later clip (slot 2): its degenerate window is the on key.
        assert!(t.emitting(Some(2), 0.05, 0.0));
        assert!(t.emitting(None, 0.1, 0.0));
        assert!(!t.emitting(Some(9), 0.5, 0.0));
        // The rate is a whole-track constant: same in every slot.
        assert_eq!(t.constant_rate(), Some(20.0));
        assert_eq!(t.peak_rate(), 20.0);
    }

    /// Frost Nova's authored shape: the sphere radius lerps 0.19 to 13.2 yd over 667 ms and holds.
    #[test]
    fn animated_area_ramp_samples_mid_flight() {
        let area = track(1, &[(0, 0.1944), (667, 13.1967), (867, 13.1967)], &[]);
        let life = track(1, &[(0, 0.472), (467, 0.8008), (667, 0.7), (867, 0.7)], &[]);
        let zero = M2ScalarTrack::default();
        let p = EmitParams::bake(
            [
                &zero, &zero, &zero, &zero, &zero, &life, &area, &area, &zero,
            ],
            &slots(&[((0, 867), false)]),
            &[],
        );
        let at = |t: f32| p.sample(None, t, 0.0);
        assert!((at(0.0).area_length - 0.1944).abs() < 1e-3, "opens tight");
        let mid = at(0.3335).area_length;
        assert!(
            (mid - (0.1944 + (13.1967 - 0.1944) * 0.5)).abs() < 0.05,
            "mid-ramp radius ≈ 6.7 yd, got {mid}"
        );
        assert!((at(0.8).area_width - 13.1967).abs() < 1e-3, "holds wide");
        assert!((at(0.2).lifespan - 0.6127).abs() < 5e-3, "lifespan rides");
        assert_eq!(at(0.5).emission_speed, 0.0, "keyless channel: default");
        assert!((p.peak_lifespan() - 0.8008).abs() < 1e-4);
        // The clamped clip holds its tail.
        assert!((at(5.0).area_length - 13.1967).abs() < 1e-3);
    }

    /// A windowed gate re-fires every pass, as the precast hold's pulsing hand flash does.
    #[test]
    fn a_looping_band_wraps_its_gate_window() {
        let gate = track(0, &[(0, 1.0), (200, 0.0)], &[]);
        let t = EmitTiming::bake(
            &track(0, &[(0, 40.0)], &[]),
            &gate,
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert!(t.emitting(None, 0.1, 0.0), "first pass: inside the window");
        assert!(!t.emitting(None, 0.8, 0.0), "first pass: past it");
        assert!(
            t.emitting(None, 1.1, 0.0),
            "second pass: the window re-fires"
        );
    }

    /// `BlastedLandsLightningbolt01.m2`'s emitter 2: two variations of anim 0, the strike keyed
    /// only in slot 1, which the arm's weighted roll picks about 5% of the time. A consumer pinned
    /// to slot 0 never emits, yet `peak_rate` keeps the emitter past the spawn cull.
    #[test]
    fn a_burst_keyed_only_in_a_later_variation_is_silent_in_slot_0() {
        // Absolute timeline: slot 0's band 0..1333, slot 1's band 1367..2667 (the real model's).
        let rate = track(
            1,
            &[
                (0, 0.0),
                (1633, 0.0),
                (1667, 30.0),
                (1800, 30.0),
                (1833, 0.0),
            ],
            &[],
        );
        let t = EmitTiming::bake(
            &rate,
            &M2ScalarTrack::default(),
            &slots(&[((0, 1333), true), ((1367, 2667), true)]),
            &[],
        );
        // Slot 0 is silent across its whole band.
        for s in [0.0, 0.3, 0.6, 0.9, 1.2] {
            assert_eq!(
                t.rate(Some(0), s, 0.0),
                0.0,
                "slot 0 is a flat zero at {s}s"
            );
        }
        assert_eq!(
            t.rate(None, 0.5, 0.0),
            0.0,
            "`None` degrades to slot 0 — also silent"
        );
        // Slot 1 carries the strike.
        assert_eq!(t.rate(Some(1), 0.0, 0.0), 0.0, "slot 1 opens closed");
        assert_eq!(
            t.rate(Some(1), 0.316, 0.0),
            30.0,
            "the burst fires mid-band"
        );
        assert_eq!(t.rate(Some(1), 0.5, 0.0), 0.0, "and self-closes");
        // The build-time cull still sees a live emitter.
        assert_eq!(
            t.peak_rate(),
            30.0,
            "peak folds ACROSS slots — never culled"
        );
        assert_eq!(
            t.constant_rate(),
            None,
            "and it is not a constant-rate emitter"
        );
    }
}
