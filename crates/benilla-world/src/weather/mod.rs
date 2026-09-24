//! Weather's visual state, driven by `SMSG_WEATHER`: the two channels of the reference's ramp
//! (`0x67bc70`), each a clamped lerp over `(|Δ|·scale + 0.001)·10` seconds.
//!
//! - Channel A, the effect intensity (scale 1), swings fully in about 10 s. The effect density is
//!   `max((A − 0.25)·4/3, 0)`, so nothing falls below 0.25.
//! - Channel B, the sky density (scale 4), has its endpoints clamped to [0, 0.25] by `SetWeather`
//!   (`0x67baf0`), so it also swings in about 10 s. Lighting turns it into the storm blend
//!   (`0x6d4500`): the overcast leads the rain up and starts clearing at once on the way down.
//!
//! The wire's last byte is 0 for smooth, nonzero for instant: the net handler (`0x48fa5f`) inverts
//! it for `SetWeather`, whose flag is 1 for smooth; vmangos always sends 0. The channels run on
//! real time, like the reference's ms tick (`0x42c010`), so a ramp continues through loading.

use bevy::prelude::*;

use crate::dev_state::DebugState;

mod precip;

/// Rain's forced-fog window, read by the effect lane's `EffectFog::Rain` row.
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

/// One ramped channel of the reference's ramp (`0x67bc70`).
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
        // The `+ 0.001` sits inside the abs, as in `0x67bc70`.
        let den = ((self.to - self.from) * self.span_scale + 0.001).abs() * 10.0;
        let t = (((now - self.start) as f32) / den).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * t
    }

    /// Ramps toward `target` from the old target, not the current value, as `SetWeather`
    /// (`0x67baf0`) does: a mid-swing retarget jumps back to the previous endpoint.
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

/// The zone weather, the reference's `CMapWeather` manager.
#[derive(Resource)]
pub struct WeatherState {
    /// The latest wire type (`Fine` included).
    pub(crate) kind: WeatherKind,
    /// The type whose effect spawns; it follows every type change at once.
    pub effect_kind: WeatherKind,
    intensity: Channel,
    sky: Channel,
    /// Channel A this frame, for instruments. Consumers take `effect_density`, which also feeds
    /// the mist rate, so mist starts past A = 0.625.
    pub(crate) intensity_a: f32,
    /// The density the active effect spawns at, resolved per frame.
    pub effect_density: f32,
    /// Channel B this frame, the sky density in [0, 0.25].
    pub sky_density: f32,
    /// The `weatherDensity` CVar, 0 to 3, which scales only the rain, snow and mist spawn rates.
    /// Deviation: it defaults to 3 where the reference registers 2, because the precipitation
    /// rates were graded against a reference install running 3.
    pub weather_density: u8,
    /// Bumped on every wire type change, the cut: emission stops at once (`0x67585d`), the open
    /// packet retires and every unreplayed packet is discarded (`0x67575a`). A same-type grade
    /// change does not cut; the rain thins on the ramp.
    pub(crate) cut_seq: u32,
}

/// The `weatherDensity` quality table (`0x67b870`): the spawn-rate gain `K` in `rate = K·P·grade`.
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
    /// Applies one wire update as `SetWeather` (`0x67baf0`); `Fine` targets 0 whatever its grade.
    fn apply(&mut self, kind: WeatherKind, grade: f32, instant: bool, now: f64) {
        let target = if kind == WeatherKind::Fine {
            0.0
        } else {
            grade.clamp(0.0, 1.0)
        };
        // A type change cuts (`0x67585d`): `effect_kind` flips at once, so a rain-to-snow swap
        // starts snow while the old drops fall out.
        if kind != self.kind {
            self.cut_seq = self.cut_seq.wrapping_add(1);
            self.effect_kind = kind;
        }
        self.kind = kind;
        // `SetWeather` clamps both sky endpoints to 0.25 (`0x8029b0`); the full grade here would
        // hold the fog at 100% for most of every downswing.
        let sky_target = target.min(0.25);
        if instant {
            self.intensity.snap(target);
            self.sky.snap(sky_target);
        } else {
            self.intensity.retarget(target, now);
            self.sky.retarget(sky_target, now);
        }
    }

    /// Resolves the frame's published values, as the reference's update driver (`0x67be40`) does.
    fn resolve(&mut self, now: f64) {
        let a = self.intensity.value(now);
        self.intensity_a = a;
        self.effect_density = ((a - 0.25) * (4.0 / 3.0)).max(0.0);
        self.sky_density = self.sky.value(now);
    }

    /// The spawn-rate gain `K` for the current `weather_density`.
    pub(crate) fn density_gain(&self) -> f32 {
        DENSITY_GAIN[usize::from(self.weather_density.min(3))]
    }

    /// The spawn density for one type: the active type's, else 0, so its pool drains by lifetime.
    pub(crate) fn density_for(&self, kind: WeatherKind) -> f32 {
        if self.effect_kind == kind {
            self.effect_density
        } else {
            0.0
        }
    }
}

/// The storm light-blend weight (`0x6d4500`), from the sky density alone with no sun term;
/// lighting lerps the storm `LightParams` over the clear one by it.
pub fn storm_blend(sky_density: f32) -> f32 {
    (sky_density * 4.0).min(1.0)
}

/// The weather tick's set; lighting's resolve runs after it to see this frame's densities.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct WeatherTick;

/// One step of the `WOW_WEATHER` script: (kind, grade, instant) applied at `at` seconds.
struct EnvStep {
    at: f64,
    kind: u32,
    grade: f32,
    instant: bool,
}

/// Parses `WOW_WEATHER`: `;`-separated steps of `<type>,<grade>[,smooth][@<secs>]`, so
/// `1,0.9999,smooth;0,0,smooth@20` rains at boot and clears at 20 s.
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
    // The `WOW_WEATHER` script drives the panel override, so captures and headless runs can
    // force weather without a GM `.wchange`.
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
    // An armed panel override wins and consumes the wire messages, so a zone re-send cannot
    // fight it.
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

    // `WOW_WEATHER_DUMP=1` prints the live ramp at 1 Hz while a transition is in flight.
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

/// Weather visuals: the state machine and the precipitation pools. The storm light blend is
/// lighting's, read after [`WeatherTick`].
pub(crate) struct WeatherPlugin;

/// The zone's weather command: type, grade and `instant` drive the state machine, and `sound_id`
/// names the sound loop kit. Owned here, so the world runs without a network stack.
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
            // Registered where the command is owned.
            .add_message::<WeatherMessage>()
            .add_systems(Update, weather_tick.in_set(WeatherTick));
        precip::register(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_script_parses_steps() {
        let s = parse_env_script("1,0.9999,smooth;0,0,smooth@20");
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].kind, s[0].instant, s[0].at), (1, false, 0.0));
        assert!((s[0].grade - 0.9999).abs() < 1e-6);
        assert_eq!((s[1].kind, s[1].instant), (0, false));
        assert!((s[1].at - 20.0).abs() < 1e-9);
        // A bare step is instant at t = 0.
        let s = parse_env_script("2,0.5");
        assert_eq!(s.len(), 1);
        assert!(s[0].instant && s[0].at == 0.0);
    }

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

    /// A full sky swing, 0 to 0.25, also takes about 10 s: the ×4 cancels the quarter span.
    #[test]
    fn sky_swing_is_ten_seconds_and_clears_immediately() {
        let mut s = WeatherState::default();
        s.apply(WeatherKind::Rain, 1.0, true, 0.0);
        s.resolve(0.0);
        assert!((s.sky_density - 0.25).abs() < 1e-6, "knee-clamped target");
        assert!((storm_blend(s.sky_density) - 1.0).abs() < 1e-6);
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
        // A downswing of about 1 takes about 10 s from the retarget.
        assert!(c.value(15.2) < 0.01);
    }

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

    #[test]
    fn type_change_cuts_same_type_drains() {
        let mut s = WeatherState::default();
        s.apply(WeatherKind::Rain, 1.0, true, 0.0);
        let seq = s.cut_seq;
        // Same type, grade to 0: no cut; the effect drains on the ramp.
        s.apply(WeatherKind::Rain, 0.0, false, 100.0);
        assert_eq!(s.cut_seq, seq);
        assert_eq!(s.effect_kind, WeatherKind::Rain);
        s.resolve(102.0);
        assert!(s.effect_density > 0.5, "mid-drain, still raining");
        // A type change to Fine cuts, and the effect kind flips at once.
        s.apply(WeatherKind::Fine, 0.0, false, 105.0);
        assert_eq!(s.cut_seq, seq + 1);
        assert_eq!(s.effect_kind, WeatherKind::Fine);
        assert_eq!(s.density_for(WeatherKind::Rain), 0.0);
    }

    #[test]
    fn storm_blend_clamp() {
        assert_eq!(storm_blend(0.0), 0.0);
        assert!((storm_blend(0.125) - 0.5).abs() < 1e-6);
        assert_eq!(storm_blend(0.25), 1.0);
        assert_eq!(storm_blend(1.0), 1.0);
    }
}
