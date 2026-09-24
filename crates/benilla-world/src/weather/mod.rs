//! Weather — the `SMSG_WEATHER`-driven state machine and its render subsystems.
//!
//! The wire (`WeatherMessage`, bridged from the Net drain) carries `type/grade/sound/instant`;
//! sound is already handled (`sound/weather.rs`). This module owns the **visual** state: the
//! two ramped intensity channels of the real client's weather ramp (`0x67bc70`):
//!
//! - **channel A — effect intensity**: `t = elapsed_s / ((|to−from| + 0.001)·10)`, clamped lerp —
//!   a full 0→1 swing takes ~10 s. The *effect density* handed to the precipitation pools is
//!   `max((A − 0.25)·4/3, 0)`: below grade 0.25 nothing falls, 0.25..1 maps to 0..1.
//! - **channel B — sky density**: the same law with `den = (|Δ|·4 + 0.001)·10`, but its
//!   endpoints live in the **[0, 0.25] knee domain** — `SetWeather 0x67baf0` writes
//!   `clamp(grade, 0, 0.25)` into them, so the ×4 cancels the quarter-span and B ALSO swings in
//!   ~10 s (the 0310 fold misread the ×4 as "4× slower"; corrected in 0326). Published as
//!   [`WeatherState::sky_density`] (0..0.25); lighting turns it into the **storm blend**
//!   `bcc = min(1, B·4)` (`0x6d4500` — no sun term) and lerps the storm
//!   `LightParams` record over the clear one — so the overcast/fog ramps linearly across the
//!   whole swing: it leads the rain on the way up and clears immediately on the way down.
//!
//! The wire's last byte is **0 = smooth, nonzero = instant** — the net handler `0x48fa5f`
//! INVERTS it (`test dl,dl; sete al`) before `SetWeather`, whose internal flag is 1 = smooth.
//! vmangos always sends 0 (smooth); our `instant = u8 ≠ 0` decode is byte-correct end-to-end.
//!
//! The channels run on real elapsed time (the reference uses its ms tick `0x42c010`), so a
//! transition keeps ramping through loading screens exactly like the real client.

use bevy::prelude::*;

use crate::dev_state::DebugState;

mod precip;

/// Rain's forced-fog window — the effect lane's `EffectFog::Rain` params row reads the law
/// from its owner.
pub(crate) use precip::{RAIN_FOG_END, RAIN_FOG_START};

/// Wire weather types (`SMSG_WEATHER` / vmangos `WeatherType`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WeatherKind {
    #[default]
    Fine,
    Rain,
    Snow,
    Sand,
}

impl WeatherKind {
    pub(crate) fn from_wire(v: u32) -> Self {
        match v {
            1 => WeatherKind::Rain,
            2 => WeatherKind::Snow,
            3 => WeatherKind::Sand,
            _ => WeatherKind::Fine,
        }
    }
}

/// One ramped intensity channel — the byte law of the reference's ramp function (`0x67bc70`):
/// `value = clamped_lerp(from → to, elapsed / ((|to−from|·span_scale + 0.001)·10))`. Channel A
/// uses `span_scale = 1` over the grade domain (~10 s per full swing); channel B `span_scale = 4`
/// over the **[0, 0.25] knee domain** — also ~10 s per full swing (the ×4 cancels the quarter-span).
#[derive(Clone, Copy, Debug)]
struct Channel {
    from: f32,
    to: f32,
    /// Ramp epoch, seconds (real time).
    start: f64,
    span_scale: f32,
}

impl Channel {
    fn new(span_scale: f32) -> Self {
        Self {
            from: 0.0,
            to: 0.0,
            start: 0.0,
            span_scale,
        }
    }

    fn value(&self, now: f64) -> f32 {
        // The `+0.001` sits INSIDE the abs (`0x67bc70`: `|Δ·span + 0.001|·10`) — sub-10 ms
        // from the outside form, transcribed exactly anyway (0328 C1 nit).
        let den = ((self.to - self.from) * self.span_scale + 0.001).abs() * 10.0;
        let t = (((now - self.start) as f32) / den).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * t
    }

    /// Start ramping toward `target` from the **old target** — `SetWeather 0x67baf0` re-stamps
    /// the epoch and writes `from ← old *target*` (`param_1[1] = *param_1`), NOT the current
    /// ramped value. A mid-swing retarget therefore restarts the ramp from the previous
    /// endpoint (a small jump the real client accepts); resuming from the current value was a
    /// benilla-invented smoothing (corrected in 0326).
    fn retarget(&mut self, target: f32, now: f64) {
        self.from = self.to;
        self.to = target;
        self.start = now;
    }

    /// The wire's `instant` flag: jump straight to `target`.
    fn snap(&mut self, target: f32) {
        self.from = target;
        self.to = target;
    }
}

/// The zone weather state — the client-side mirror of the reference's `CMapWeather` manager.
#[derive(Resource)]
pub struct WeatherState {
    /// The latest wire type (`Fine` included).
    pub(crate) kind: WeatherKind,
    /// The latest *non-fine* type — the effect the density applies to while a fine transition
    /// drains the sky/particles. (The reference keeps per-type effect slots alive until their
    /// density reaches 0; a direct type→type switch retargets the same channel, and the old
    /// type's particles simply live out their few-second lifetimes.)
    pub effect_kind: WeatherKind,
    intensity: Channel,
    sky: Channel,
    /// Channel A's raw value this frame (the wire grade, ramped) — published for instruments
    /// (the `WOW_WEATHER_DUMP` pacing log). Consumers take the knee-mapped
    /// [`WeatherState::effect_density`]: C3-VERIFIED (rf-weather-emission-timeline), the mist
    /// rate's input is the knee density too — `2·max(intensity − 0.5, 0)·K·Q` where intensity
    /// is `(A − 0.25)·4/3`, so mist starts at A > 0.625 (~6.25 s into a full upswing).
    pub(crate) intensity_a: f32,
    /// `max((A − 0.25)·4/3, 0)` — the density the active effect spawns at, resolved per frame.
    pub effect_density: f32,
    /// Channel B's current value — the slow sky/cloud density, resolved per frame.
    pub sky_density: f32,
    /// The `weatherDensity` CVar (the video-options **Weather Intensity** setting, 0–3) — the
    /// client-side particle-density step. Rendering only: it scales the spawn-rate gain of
    /// rain/snow/mist through the quality table (see [`WeatherState::density_gain`]); it never
    /// touches the wire grade, the ramps, or the storm/fog blend. Default 3 = the reference
    /// install's live `Config.wtf` (`SET weatherDensity "3"`) — the look target.
    pub weather_density: u8,
    /// Bumped on every wire TYPE change (fine included) — the cut signal: the
    /// driver's cross-fade path stops emission at once (`0x67585d`), retires the open packet,
    /// and discards every packet whose replay hasn't started (`0x67575a`). Same-type grade
    /// changes (e.g. rain 1 → rain 0) do NOT cut — they drain via the ramp, rain thinning and
    /// persisting, exactly as the director observed on `1 1 → 1 0` vs `1 1 → 0 0`.
    pub(crate) cut_seq: u32,
}

/// The `weatherDensity` quality table (`0x67b870`): setting 0–3 → spawn-rate gain `K` in
/// `rate = K·P·grade`. The wx transcriptions carried quality 2's `0.66` as a constant, which
/// under-spawned every effect ×1.52 vs the reference running at 3.
const DENSITY_GAIN: [f32; 4] = [0.1, 0.33, 0.66, 1.0];

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            kind: WeatherKind::Fine,
            effect_kind: WeatherKind::Fine,
            intensity: Channel::new(1.0),
            sky: Channel::new(4.0),
            intensity_a: 0.0,
            effect_density: 0.0,
            sky_density: 0.0,
            weather_density: 3,
            cut_seq: 0,
        }
    }
}

impl WeatherState {
    /// Apply one wire update (`SetWeather 0x67baf0` semantics: retarget both channels; `instant`
    /// snaps them). A `Fine` packet targets 0 regardless of its grade.
    fn apply(&mut self, kind: WeatherKind, grade: f32, instant: bool, now: f64) {
        let target = if kind == WeatherKind::Fine {
            0.0
        } else {
            grade.clamp(0.0, 1.0)
        };
        // A TYPE change cuts (`0x67585d`): emission stops instantly and the unreplayed
        // pipeline is discarded — `effect_kind` flips to the NEW type at once (Fine ⇒ no
        // effect spawns; a rain→snow swap starts snow while old rain drops fall out). Only a
        // same-type grade change keeps the effect and drains it via the ramp.
        if kind != self.kind {
            self.cut_seq = self.cut_seq.wrapping_add(1);
            self.effect_kind = kind;
        }
        self.kind = kind;
        // Channel B (sky) lives in the **[0, 0.25] knee domain** — `SetWeather 0x67baf0` writes
        // `clamp(grade, 0, 0.25)` into both B endpoints (the 0.25 @`0x8029b0`). With the ×4 in
        // B's denominator this makes both channels swing in ~10 s, and `bcc = min(1, B·4)` ramps
        // linearly over the WHOLE swing — fog visibly leads the rain up (rain needs A > 0.25)
        // and starts clearing IMMEDIATELY on the way down. Feeding the full grade here (the
        // 0310-era misread of the ×4 as "4× slower") pinned the fog at 100% for ~30 s of every
        // downswing — the director's "takes a long time until it gets sunny".
        let sky_target = target.min(0.25);
        if instant {
            self.intensity.snap(target);
            self.sky.snap(sky_target);
        } else {
            self.intensity.retarget(target, now);
            self.sky.retarget(sky_target, now);
        }
    }

    /// Resolve the per-frame published values (the reference does this in the update driver
    /// `0x67be40` → the ramp → the effect setters).
    fn resolve(&mut self, now: f64) {
        let a = self.intensity.value(now);
        self.intensity_a = a;
        self.effect_density = ((a - 0.25) * (4.0 / 3.0)).max(0.0);
        self.sky_density = self.sky.value(now);
    }

    /// The spawn-rate gain `K` for the current [`WeatherState::weather_density`] setting
    /// (`0x67b870` quality table).
    pub(crate) fn density_gain(&self) -> f32 {
        DENSITY_GAIN[usize::from(self.weather_density.min(3))]
    }

    /// The spawn density for one effect type: the active effect gets the ramped density, every
    /// other type 0 (its pool drains by particle lifetime).
    pub(crate) fn density_for(&self, kind: WeatherKind) -> f32 {
        if self.effect_kind == kind {
            self.effect_density
        } else {
            0.0
        }
    }
}

/// The storm light-blend weight — `0x6d4500`: `bcc = min(1, density·4)`,
/// purely the clamped sky density (no sun term). Lighting lerps the storm `LightParams` record
/// over the clear one by this weight.
pub fn storm_blend(sky_density: f32) -> f32 {
    (sky_density * 4.0).min(1.0)
}

/// Label set for the weather tick — lighting's resolve runs `.after` this so the storm blend
/// sees this frame's densities.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct WeatherTick;

/// Drain wire updates into the channels and resolve the frame's densities. The debug panel's
/// override (when armed) substitutes a synthetic wire update — the same path end-to-end.
/// One step of the `WOW_WEATHER` script: apply (kind, grade, instant) at `at` seconds.
struct EnvStep {
    at: f64,
    kind: u32,
    grade: f32,
    instant: bool,
}

/// Parse the `WOW_WEATHER` value: `;`-separated steps of `<type>,<grade>[,smooth][@<secs>]`
/// (no `@` → t = 0). One step reproduces the old syntax; several script a transition — e.g.
/// `1,0.9999,smooth;0,0,smooth@20` rains at boot and clears at t = 20 s, so downswing pacing
/// is measurable headless.
fn parse_env_script(v: &str) -> Vec<EnvStep> {
    v.split(';')
        .filter_map(|seg| {
            let (seg, at) = match seg.rsplit_once('@') {
                Some((s, at)) => (s, at.trim().parse::<f64>().ok()?),
                None => (seg, 0.0),
            };
            let mut parts = seg.split(',');
            Some(EnvStep {
                at,
                kind: parts.next()?.trim().parse().ok()?,
                grade: parts.next()?.trim().parse().ok()?,
                instant: parts.next().map(str::trim) != Some("smooth"),
            })
        })
        .collect()
}

fn weather_tick(
    time: Res<Time<Real>>,
    mut msgs: MessageReader<WeatherMessage>,
    mut state: ResMut<WeatherState>,
    mut debug: ResMut<DebugState>,
    mut env_script: Local<Option<Vec<EnvStep>>>,
    mut dump: Local<Option<bool>>,
    mut next_dump: Local<f64>,
) {
    let now = time.elapsed_secs_f64();
    // Dev/capture hook: the `WOW_WEATHER` script (see [`parse_env_script`]) drives the panel
    // override, so captures and headless runs can force weather — or exercise live ramps in
    // both directions — without a GM `.wchange`.
    let script = env_script.get_or_insert_with(|| {
        std::env::var("WOW_WEATHER")
            .map(|v| parse_env_script(&v))
            .unwrap_or_default()
    });
    while script.first().is_some_and(|s| s.at <= now) {
        let step = script.remove(0);
        debug.weather.force = true;
        debug.weather.dirty = true;
        debug.weather.kind = step.kind;
        debug.weather.grade = step.grade;
        debug.weather.instant = step.instant;
    }
    // A panel-armed override wins over the wire (and consumes the wire messages so a zone
    // re-send doesn't fight the scrub); otherwise the wire drives.
    if debug.weather.force {
        if debug.weather.dirty {
            debug.weather.dirty = false;
            state.apply(
                WeatherKind::from_wire(debug.weather.kind),
                debug.weather.grade,
                debug.weather.instant,
                now,
            );
        }
        msgs.clear();
    } else {
        for m in msgs.read() {
            state.apply(
                WeatherKind::from_wire(m.weather_type),
                m.grade,
                m.instant,
                now,
            );
        }
    }
    state.resolve(now);

    // `WOW_WEATHER_DUMP=1`: print the live ramp at 1 Hz while a transition is in flight — the
    // pacing instrument (the director's stopwatch vs the code, on real numbers, live pipeline).
    if dump.is_none() {
        *dump = Some(std::env::var("WOW_WEATHER_DUMP").is_ok());
    }
    if *dump == Some(true)
        && now >= *next_dump
        && ((state.intensity_a - state.intensity.to).abs() > 1e-4
            || (state.sky_density - state.sky.to).abs() > 1e-4
            || state.effect_density > 0.0)
    {
        *next_dump = now + 1.0;
        println!(
            "[weather] t={now:7.2}s  A={:.3}  density={:.3}  B={:.3}  bcc={:.3}",
            state.intensity_a,
            state.effect_density,
            state.sky_density,
            storm_blend(state.sky_density),
        );
    }
}

/// Weather visuals: the state machine (this file) + the precipitation pools (`precip`).
/// The storm light-blend itself lives in `lighting::update_time_lighting` (it owns the
/// atmosphere resolve); it reads [`WeatherState`] `.after(WeatherTick)`.
pub(crate) struct WeatherPlugin;

/// **The weather command.** What the zone's weather should now be: `weather_type`/`grade`/
/// `instant` drive the visuals' state machine ([`weather_tick`]) and `sound_id`
/// names the loop kit for the sound subsystem.
///
/// Owned here, written by whoever knows. It lived in `net` — where `SMSG_WEATHER`
/// is decoded — and `NetPlugin` registered it, so the weather system could not run at all without
/// a network stack: the world viewer's survey caught it as "Message not initialized" every frame.
/// The engine owning the command and the game writing it is the same relationship the other way
/// round, and it is the one that composes: an editor sets the weather with no server in sight.
#[derive(Message, Clone, Copy)]
pub struct WeatherMessage {
    pub weather_type: u32,
    pub grade: f32,
    /// A SoundEntries loop kit (8533..8558), 0 = clear skies.
    pub sound_id: u32,
    pub instant: bool,
}

impl Plugin for WeatherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeatherState>()
            // The command is ours, so its registration is ours — see [`WeatherMessage`].
            .add_message::<WeatherMessage>()
            .add_systems(Update, weather_tick.in_set(WeatherTick));
        precip::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WOW_WEATHER script syntax: bare (old form), `,smooth`, and `@secs` steps.
    #[test]
    fn env_script_parses_steps() {
        let s = parse_env_script("1,0.9999,smooth;0,0,smooth@20");
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].kind, s[0].instant, s[0].at), (1, false, 0.0));
        assert!((s[0].grade - 0.9999).abs() < 1e-6);
        assert_eq!((s[1].kind, s[1].instant), (0, false));
        assert!((s[1].at - 20.0).abs() < 1e-9);
        // The old one-shot instant form still parses.
        let s = parse_env_script("2,0.5");
        assert_eq!(s.len(), 1);
        assert!(s[0].instant && s[0].at == 0.0);
    }

    /// Channel A: a 0→1 retarget crosses in ~10 s ((1·1+0.001)·10), the byte law.
    #[test]
    fn channel_a_full_swing_is_ten_seconds() {
        let mut c = Channel::new(1.0);
        c.retarget(1.0, 0.0);
        assert_eq!(c.value(0.0), 0.0);
        let mid = c.value(5.0);
        assert!((mid - 0.5).abs() < 0.01, "midpoint {mid}");
        assert!(c.value(10.02) > 0.999);
        assert_eq!(c.value(20.0), 1.0); // clamped
    }

    /// Channel B runs the [0, 0.25] knee domain: a full sky swing (0→0.25) ALSO takes ~10 s
    /// ((0.25·4+0.001)·10) — the ×4 cancels the quarter-span. The storm blend `min(1, B·4)`
    /// therefore ramps linearly over the whole swing, both directions (the "40 s sky"
    /// was the 0310 misread, and it pinned the fog at 100% for ~30 s of every downswing).
    #[test]
    fn sky_swing_is_ten_seconds_and_clears_immediately() {
        let mut s = WeatherState::default();
        s.apply(WeatherKind::Rain, 1.0, true, 0.0);
        s.resolve(0.0);
        assert!((s.sky_density - 0.25).abs() < 1e-6, "knee-clamped target");
        assert!((storm_blend(s.sky_density) - 1.0).abs() < 1e-6);
        // Downswing: fog starts clearing at once, gone in ~10 s.
        s.apply(WeatherKind::Fine, 0.0, false, 100.0);
        s.resolve(105.0);
        assert!(
            (storm_blend(s.sky_density) - 0.5).abs() < 0.02,
            "half-clear at 5 s, got {}",
            storm_blend(s.sky_density)
        );
        s.resolve(110.1);
        assert!(storm_blend(s.sky_density) < 0.01, "clear by ~10 s");
    }

    /// A retarget mid-ramp restarts from the old *target* (`SetWeather`: `from ← old target`),
    /// not the current ramped value — the byte behaviour, replacing the invented smoothing.
    #[test]
    fn retarget_restarts_from_old_target() {
        let mut c = Channel::new(1.0);
        c.retarget(1.0, 0.0);
        c.retarget(0.0, 5.0); // half-way up, turn around
        let v = c.value(5.0);
        assert!(
            (v - 1.0).abs() < 0.01,
            "restarts at the OLD target, got {v}"
        );
        // Down-swing of Δ≈1 takes ≈10 s from the retarget epoch.
        assert!(c.value(15.2) < 0.01);
    }

    /// The effect-density knee: nothing below grade 0.25; 0.25..1 maps to 0..1.
    #[test]
    fn effect_density_knee() {
        let mut s = WeatherState::default();
        s.apply(WeatherKind::Rain, 1.0, true, 0.0);
        s.resolve(0.0);
        assert!((s.effect_density - 1.0).abs() < 1e-5);
        s.apply(WeatherKind::Rain, 0.25, true, 0.0);
        s.resolve(0.0);
        assert_eq!(s.effect_density, 0.0);
        s.apply(WeatherKind::Rain, 0.625, true, 0.0);
        s.resolve(0.0);
        assert!((s.effect_density - 0.5).abs() < 1e-5);
    }

    /// The `weatherDensity` quality table (`0x67b870`) and its default: the reference install
    /// runs `SET weatherDensity "3"` (K = 1.0) — the look target. The transcriptions' 0.66 is
    /// setting 2, kept reachable via the panel.
    #[test]
    fn weather_density_gain_table() {
        let mut s = WeatherState::default();
        assert_eq!(s.weather_density, 3);
        assert!((s.density_gain() - 1.0).abs() < 1e-6);
        for (wd, k) in [(0u8, 0.1f32), (1, 0.33), (2, 0.66), (3, 1.0)] {
            s.weather_density = wd;
            assert!((s.density_gain() - k).abs() < 1e-6);
        }
        s.weather_density = 200; // out-of-range setting clamps to the top entry
        assert!((s.density_gain() - 1.0).abs() < 1e-6);
    }

    /// A wire TYPE change cuts (`0x67585d`): Fine flips the effect kind at once (emission stops
    /// that frame — the pipeline discard rides `cut_seq`), while a same-type grade change keeps the
    /// effect and drains it via the ramp — the director's `1 1 → 1 0` vs `1 1 → 0 0` split.
    #[test]
    fn type_change_cuts_same_type_drains() {
        let mut s = WeatherState::default();
        s.apply(WeatherKind::Rain, 1.0, true, 0.0);
        let seq = s.cut_seq;
        // Same type, grade → 0: no cut; the effect drains on the ramp.
        s.apply(WeatherKind::Rain, 0.0, false, 100.0);
        assert_eq!(s.cut_seq, seq);
        assert_eq!(s.effect_kind, WeatherKind::Rain);
        s.resolve(102.0);
        assert!(s.effect_density > 0.5, "mid-drain, still raining");
        // Type change to Fine: cut — the effect kind flips immediately.
        s.apply(WeatherKind::Fine, 0.0, false, 105.0);
        assert_eq!(s.cut_seq, seq + 1);
        assert_eq!(s.effect_kind, WeatherKind::Fine);
        assert_eq!(s.density_for(WeatherKind::Rain), 0.0);
    }

    /// The storm blend: bcc = min(1, 4·density) — full overcast by density 0.25.
    #[test]
    fn storm_blend_clamp() {
        assert_eq!(storm_blend(0.0), 0.0);
        assert!((storm_blend(0.125) - 0.5).abs() < 1e-6);
        assert_eq!(storm_blend(0.25), 1.0);
        assert_eq!(storm_blend(1.0), 1.0);
    }
}
