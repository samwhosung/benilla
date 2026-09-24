//! Per-sequence **particle emission timing** — the key-window bake (`0x713d50`) of the emitter's
//! per-frame-sampled M2Tracks: spawn rate `+0xdc` + enabled gate `+0x1dc` ([`EmitTiming`]) and the
//! other nine emission parameters ([`EmitParams`]), one loop per FILE sequence slot. Split from
//! [`crate::particles`] (the raw record parse) because it is the *runtime sampling* face:
//! decision 0641's material-alpha structure, one channel over.

use benilla_m2::M2ScalarTrack;

use crate::models::{bake_track, ScalarAnim, SeqSlot};

/// Per-sequence **emission timing**: the emitter's two per-frame-sampled M2Tracks — spawn rate
/// (`+0xdc`) and the enabled gate (`+0x1dc`) — baked one loop per FILE sequence slot through the
/// key-window kernel `0x713d50` ([`crate::models::bake_track`]), exactly the material-alpha
/// structure of decision 0641 one channel over. The emitter phase of the reference's animate
/// kernel `0x714260` samples both through the **playing** sequence's key window every frame and
/// forces the spawn rate to 0 while the gate is off (`0x717d90`/`0x718f32`).
///
/// The clock law rides the **baked loop** ([`crate::models::KeyAnim::wrap`]), decided from the
/// slot at bake time: a **looping** sequence wraps its band (`t mod period` — a windowed gate
/// re-fires every pass), a **clamped** one parks at the band end and holds the tail value (never
/// aliasing back to the band start), and a gseq-tagged track always wraps on its own free clock
/// whatever the playing sequence does. Consumers resolve `seq` from whatever clock they run: a
/// placed doodad passes `None` (slot 0 — its one-time arm), an effect its armed slot, a
/// unit/GameObject its live playing sequence.
#[derive(Debug, Clone, Default)]
pub struct EmitTiming {
    /// Baked rate loop per file slot; `None` = the track keys nothing there (spawn rate 0).
    rate: Vec<Option<ScalarAnim>>,
    /// Baked gate per file slot (step, 0/1); `None` = no gate authored — the loader default is
    /// ON (`0x710092`: `block+0x14c = 1` for every emitter).
    enabled: Vec<Option<ScalarAnim>>,
    /// Per file slot: sequence flags bit 0 CLEAR = the band loops. Carried for [`Self::idx`]'s
    /// slot count and the dump instruments' view — the *sampling* clock is the baked loop's own.
    looping: Vec<bool>,
}

impl EmitTiming {
    /// Bake both tracks against every file sequence slot; `gseq` is the global-sequence duration
    /// table. Each slot carries its own loop flag, which becomes the baked loop's clock.
    pub(crate) fn bake(
        rate: &M2ScalarTrack,
        enabled: &M2ScalarTrack,
        slots: &[SeqSlot],
        gseq: &[u32],
    ) -> Self {
        // Keep every baked shape, constants included — this channel has no static fallback path,
        // so a held value must survive the bake (`|_| false` on both predicates).
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

    /// Resolve a consumer's sequence to a file slot: out-of-range / unknown degrades to slot 0
    /// (the doodad lane's one-time arm, and the old single-band behaviour).
    fn idx(&self, seq: Option<usize>) -> usize {
        match seq {
            Some(i) if i < self.looping.len() => i,
            _ => 0,
        }
    }

    /// Is the gate ON, `elapsed` seconds into sequence slot `seq`? A slot with no baked gate is
    /// ON (the loader default). `shared_now` is the world's shared elapsed seconds — a
    /// gseq-tagged gate routes to it ([`crate::models::KeyAnim::clock`]).
    pub fn emitting(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> bool {
        self.enabled
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .is_none_or(|a| a.sample_or(a.clock(elapsed, shared_now), 1.0) > 0.5)
    }

    /// The spawn rate (particles/sec), `elapsed` seconds into sequence slot `seq`. A slot with no
    /// baked rate spawns nothing. Floored at 0 (a track tail may legitimately go negative).
    /// `shared_now` routes a gseq-tagged rate loop to the shared world clock.
    pub fn rate(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> f32 {
        self.rate
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .map_or(0.0, |a| a.sample_or(a.clock(elapsed, shared_now), 0.0))
            .max(0.0)
    }

    /// The rate track's peak over every slot — the "can this emitter ever contribute" spawn gate
    /// (a burst emitter keys `0 → peak → 0`, so its first key is 0 but it absolutely emits).
    pub fn peak_rate(&self) -> f32 {
        self.rate
            .iter()
            .flatten()
            .flat_map(|a| a.keys.iter().map(|&(_, v)| v))
            .fold(0.0, f32::max)
    }

    /// The **burst this emitter actually fires** on sequence slot `seq`, as
    /// `(seconds into the slot, particle count)` — or `None` when it never fires one.
    ///
    /// The reference's burst gate is `enabled != 0 && sampledRate > 0`, both sampled from the
    /// same clock in the same frame, and it triggers on that predicate's **rising edge**, emitting
    /// `ftol(rate)` particles (`0x718ed2`–`0x718ef6`;
    /// benilla's `particles::accumulate_emission` runs the same rule). Both tracks are STEP, so
    /// this walks a 60 Hz grid over the slot's keyed span — the same resolution a running frame
    /// gives it — and reports the first instant the predicate holds.
    ///
    /// **`None` is a real shipped shape, not a parse failure.** An emitter whose enabled track
    /// falls to 0 on the very keyframe its rate track rises off 0 never emits anything: the two
    /// conditions are never true together. `Spells\\Strike_Impact_Chest.m2`'s gold flare emitter
    /// is exactly that, and reading its [`Self::peak_rate`] as a particle count overstates the
    /// effect by 50 particles that neither client draws.
    ///
    /// A gseq-tagged track is sampled against `shared_now = 0` — this is a static dump/inspection
    /// face, not a running clock.
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

    /// `Some(rate)` when every slot bakes the same single-key rate — the overwhelmingly common
    /// shape, and the dump instruments' quiet case.
    pub fn constant_rate(&self) -> Option<f32> {
        let mut it = self.rate.iter();
        let first = it.next()?.as_ref()?;
        let &(_, v) = (first.keys.len() == 1).then(|| first.keys.first())??;
        it.all(|a| a.as_ref().is_some_and(|a| a.keys == first.keys))
            .then_some(v)
    }

    /// Per-slot read view for the dump instruments: `(looping, rate keys, enabled keys)`, one per
    /// file sequence slot (`None` = the track keys nothing in that slot). Times are seconds from
    /// the slot's band start — the values the runtime actually samples, not the raw file keys.
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

/// One frame's sampled emitter **parameters** — the nine per-frame-sampled scalar M2Tracks of the
/// emitter record (bases `+0x34..+0x130`, every one except the rate/enabled pair). The reference's
/// animate kernel `0x714260` samples ALL ten scalar tracks into the per-emitter animation block
/// (`[model+0x3d0]`, stride 0x16c — the rate channel, sampled at `0x71850d`, is the template) and
/// pushes them onto the live emitter through its setters each frame. **These are NOT constants**:
/// `Frost_Nova_area` ramps its emission-sphere radius
/// 0.19 → 13.2 yd with the expanding ring, `ArcaneExplosion_Base` 0 → 7.2 yd with the growing
/// dome — flatten either to `value[0]` and every birth lands at the centre (the "born way too
/// close" bug this type exists to fix).
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
    /// Downward acceleration (yards/sec²) — a live per-frame emitter field (the integrator reads
    /// it every frame, `0x7b2680`), so it samples here rather than baking per particle.
    pub gravity: f32,
    /// Particle lifetime (seconds). The reference passes the CURRENT value into each spawn
    /// (`life_param`, the kernels' `ebp+0xc`), so a birth captures it for life.
    pub lifespan: f32,
    /// Plane: full x-extent (±½ rect); sphere: MIN radius; spline: tMin.
    pub area_length: f32,
    /// Plane: full y-extent; sphere: MAX radius; spline: tMax.
    pub area_width: f32,
    /// zSource — velocity pivot at `(0, 0, z)` (0 = unused).
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

/// The nine emitter parameter tracks, baked one loop per FILE sequence slot — the exact
/// bake/clock/sampling law of [`EmitTiming`]'s rate channel (byte-verified there), applied to its
/// nine sibling tracks. Sample once per sim frame on the emitter's clock and feed births/kill/
/// integration the result ([`ParamsNow`]).
#[derive(Debug, Clone, Default)]
pub struct EmitParams {
    /// Per-channel, per-slot baked loops, in [`ParamsNow`] field order; `None` = keyless there
    /// (the channel default stands).
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

    /// Sample every channel `elapsed` seconds into sequence slot `seq` (same slot resolution as
    /// [`EmitTiming`]: out-of-range degrades to slot 0). `shared_now` routes gseq-tagged
    /// channels to the shared world clock.
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

    /// The lifespan channel's peak over every slot — the spawn-cull gate (an animated lifespan
    /// may open at 0; an emitter is dead only if it NEVER exceeds 0). A keyless channel reads
    /// the loader default; a keyed one folds its own keys ONLY (folding from the default would
    /// mask an authored sub-1.0 peak).
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

    /// Constant channels from one [`ParamsNow`] — the test/tool constructor, and the shape every
    /// pre-track consumer had.
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

    /// Per-channel dump view: `(name, per-slot keys)` — `None` = keyless in that slot. The dump
    /// instruments print any channel whose keys actually move (the view that would have shown
    /// Frost Nova's 0.19 → 13.2 yd radius ramp instead of hiding it behind `value[0]`).
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

    /// File sequence slots as `(band, loops)` — the loop flag is the slot's CLOCK, so these tests
    /// spell it per slot rather than carry a parallel array beside the bands.
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

    /// The STEP rate law: a `{0:0, 67:30}` track is silent before its 67 ms key — the burst
    /// fires AT the key, full-count — and holds 30 after (moved here from the runtime's
    /// `accumulate_emission` tests when the sampling moved into the bake).
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

    /// The LINEAR ramp law (the BloodSpurt shape `0 → 100 → 0`, interp 1): mid-ramp pours the
    /// interpolated rate, and the self-closing tail really falls back to 0.
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

    /// The per-sequence window law — the B27 shape, synthesized: an enabled gate authored ON
    /// inside a clamped one-shot clip (slot 0) whose window in the idle loop (slot 1) resolves
    /// to OFF. The old seq-0-only rebase parked the clamped clock at its end value; the bake
    /// must read OFF at idle, run slot 0's window, and HOLD (not wrap) slot 0's tail.
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
        // Idle (slot 1): the collapsed window (1,1) resolves to keys[1] = OFF — at every time,
        // wrap included.
        assert!(!t.emitting(Some(1), 0.0, 0.0));
        assert!(!t.emitting(Some(1), 0.25, 0.0));
        assert!(!t.emitting(Some(1), 400.0, 0.0));
        // The one-shot clip (slot 0): ON at its start, OFF from 333 ms — and the clamped clock
        // HOLDS that tail at/after the band end instead of aliasing back to the ON start.
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
        // The later clip (slot 2): its degenerate window is the ON key.
        assert!(t.emitting(Some(2), 0.05, 0.0));
        // Unknown/out-of-range degrades to slot 0 (the doodad law).
        assert!(t.emitting(None, 0.1, 0.0));
        assert!(!t.emitting(Some(9), 0.5, 0.0));
        // The rate is a whole-track constant: same in every slot.
        assert_eq!(t.constant_rate(), Some(20.0));
        assert_eq!(t.peak_rate(), 20.0);
    }

    /// The ANIMATED-parameter law — Frost Nova's authored shape: the emission-sphere radius
    /// (areaLength = areaWidth) lerps 0.19 → 13.2 yd over 667 ms and holds, riding the ring
    /// outward; lifespan ramps beside it. `value[0]` flattening (the bug this bakes away) reads
    /// 0.19 for ever and births the whole mist at the caster's feet.
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
        // The clamped one-shot parks at its tail, never aliasing back to the tight opening.
        assert!((at(5.0).area_length - 13.1967).abs() < 1e-3);
    }

    /// A LOOPING slot wraps its band — a windowed gate re-fires every pass (the precast hold's
    /// pulsing hand flash), where a clamped slot would have parked.
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

    /// **The dead-slot-0 shape** — `BlastedLandsLightningbolt01.m2`'s emitter 2, synthesized
    /// (bug B63). Two sequences, both anim id 0: a variation chain. Slot 0 keys a
    /// single 0 — a flat silence for its whole band — while the strike itself, `0 → 30 → 0`, is
    /// keyed only in slot 1, which the arm's frequency-weighted roll reaches ~5 % of the time.
    ///
    /// A consumer that PINS slot 0 therefore emits **nothing at all, for ever**, on every
    /// placement: the emitter builds, pools and ticks, and never births a particle. That is what
    /// the placed-doodad lane did until it started passing the slot its own arm actually rolled.
    /// `peak_rate()` folding across all slots is why such an emitter survives the spawn cull — it
    /// looks alive to the build and is dead to the clock. `partslotscan` counts 947 of these.
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
        // Slot 0 — what the old pinned consumer sampled. Silent across its whole band.
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
        // Slot 1 — what the arm actually rolled. The strike is here, and only here.
        assert_eq!(t.rate(Some(1), 0.0, 0.0), 0.0, "slot 1 opens closed");
        assert_eq!(
            t.rate(Some(1), 0.316, 0.0),
            30.0,
            "the burst fires mid-band"
        );
        assert_eq!(t.rate(Some(1), 0.5, 0.0), 0.0, "and self-closes");
        // The trap that hid it: the build-time cull sees a live emitter either way.
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
