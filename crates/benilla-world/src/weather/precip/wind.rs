//! The weather wind (`0x67c150`) and the rotations it drives: the streak apex tilt, the
//! spawn-slab tilt and the mist spawn yaw.

use bevy::prelude::*;

/// Streak tilt `lerp(0°, 45°, sat(|wind|/30))` (`0x674a70`; 30 at `0x8680f0`, 45 at `0x8680f4`),
/// applied to the streak's apex vertex only.
const WIND_TILT_DIV: f32 = 30.0;
const WIND_TILT_MAX_DEG: f32 = 45.0;

/// Spawn-slab tilt `lerp(0°, 65°, sat(speed/18))` (`0x677965`–`0x677a55`): the streak tilt's
/// axis, but a separate rotation with its own ramp.
const SLAB_TILT_DIV: f32 = 18.0;
const SLAB_TILT_MAX_DEG: f32 = 65.0;

/// The player's trailing-average velocity over ~149 ms in yd/s (`0x67c150`), from its own
/// position deltas, so orbiting the camera does not stir it. The slab tilt keys on `mgr+0x7c`
/// instead (`0x67bf91`): a ridden transport's averaged speed when `|mgr+0x5c|² > 2`, else the
/// player's live speed, the only source here.
#[derive(Resource)]
pub(crate) struct WeatherWind {
    pub(super) last_pos: Option<Vec3>,
    /// Per-frame (Δseconds, Δpos), trimmed from the head to 149 ms.
    window: Vec<(f32, Vec3)>,
    /// The windowed planar velocity (Bevy x/z, y forced 0), yd/s.
    pub(super) vel: Vec3,
    /// The wind heading (`mgr+0x78`, WoW frame), rewritten every frame.
    heading: f32,
    /// The heading as a Bevy planar direction: `heading` is `atan2(y, x)` but
    /// [`wow_azimuth_to_bevy`] reads `atan2(x, y)`, a quarter turn apart.
    heading_dir: Vec3,
    /// The streak apex tilt, leaning the fall axis toward the motion (`0x674a70`).
    pub(super) tilt: Quat,
    /// The spawn-slab tilt: linear from zero with no dead zone (`0x674ba0`,
    /// `0x677965`–`0x677a41`), identity at rest on the same frame.
    pub(super) slab: Quat,
}

impl Default for WeatherWind {
    fn default() -> Self {
        Self {
            last_pos: None,
            window: Vec::new(),
            vel: Vec3::ZERO,
            heading: 0.0,
            // The direction `heading = 0` names: `atan2(−x, −z) = 0` ⇒ `−z = 1`.
            heading_dir: Vec3::NEG_Z,
            tilt: Quat::IDENTITY,
            slab: Quat::IDENTITY,
        }
    }
}

/// The wind window: the delta list is trimmed to `Σ(Δtick) ≤ 0x95` = 149 ms.
const WIND_WINDOW_S: f32 = 0.149;

impl WeatherWind {
    /// `pos` is the player's feet, `facing` their aim (the heading below 1 yd/s), `live_speed`
    /// their commanded planar speed ([`crate::view::Viewer::planar_speed`], `mgr+0x7c`).
    pub(super) fn update(&mut self, pos: Vec3, facing: Vec3, live_speed: f32, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let delta = match self.last_pos {
            Some(prev) => {
                let mut d = pos - prev;
                d.y = 0.0;
                // A teleport delta is not wind: restart the window.
                if d.length_squared() > (100.0 * dt).powi(2).max(100.0) {
                    self.window.clear();
                    d = Vec3::ZERO;
                }
                d
            }
            None => Vec3::ZERO,
        };
        self.last_pos = Some(pos);
        self.window.push((dt, delta));
        let mut total: f32 = self.window.iter().map(|(d, _)| d).sum();
        while self.window.len() > 1 && total - self.window[0].0 >= WIND_WINDOW_S {
            total -= self.window[0].0;
            self.window.remove(0);
        }
        let sum_dpos: Vec3 = self.window.iter().map(|(_, d)| *d).sum();
        // `Σ(Δpos)/((Σ(Δms) + 1)·0.001)`, the reference's formula, its +1 ms included.
        self.vel = sum_dpos / (total.mul_add(1000.0, 1.0) * 0.001);

        let mag2 = self.vel.length_squared();
        // `mgr+0x78` (`0x67be40`): `|W| ≥ 1` picks the heading's source and every leg stores it
        // (`0x67bee0`, `0x67bef6`, `0x67bf02`); below 1 yd/s it is the unit's facing (`0x67beff`).
        self.heading_dir = if mag2 >= 1.0 {
            self.vel / mag2.sqrt()
        } else {
            facing
                .with_y(0.0)
                .try_normalize()
                .unwrap_or(self.heading_dir)
        };
        // wind_wow = (−dir.z, −dir.x) per the bevy→wow basis; heading = atan2(y, x).
        self.heading = (-self.heading_dir.x).atan2(-self.heading_dir.z);

        // Both turn about `ŷ × ĥ = (h.z, 0, −h.x)`, carrying `ŷ` to `ĥ`: the apex tips downwind.
        let lean = |dir: Vec3, speed: f32, div: f32, max_deg: f32| {
            Vec3::new(dir.z, 0.0, -dir.x)
                .try_normalize()
                .map_or(Quat::IDENTITY, |axis| {
                    Quat::from_axis_angle(
                        axis,
                        ((speed / div).clamp(0.0, 1.0) * max_deg).to_radians(),
                    )
                })
        };
        // The streak apex keys on the averaged wind and dies below `|wind|² < 0.001` (`0x674a70`).
        self.tilt = if mag2 < 0.001 {
            Quat::IDENTITY
        } else {
            lean(self.vel, mag2.sqrt(), WIND_TILT_DIV, WIND_TILT_MAX_DEG)
        };
        // The slab keys on the live speed, exactly 0 with no direction key held: no epsilon needed.
        self.slab = lean(
            self.heading_dir,
            live_speed,
            SLAB_TILT_DIV,
            SLAB_TILT_MAX_DEG,
        );
    }

    /// The wind heading (`mgr+0x78`, WoW frame); 0, world north, at start.
    pub(crate) fn heading_wow(&self) -> f32 {
        self.heading
    }

    /// The mist spawn yaw, `+heading`: `0x67a990` passes `−heading` to `0x7bdd60`, whose core
    /// `0x7bdb00` negates it, so the −1.57 stream blows into a moving player's face.
    pub(crate) fn mist_yaw(&self) -> Quat {
        Quat::from_rotation_y(self.heading_wow())
    }
}

/// A WoW-frame azimuth, the spawn kernels' `(vx, vy) = (sin a, cos a)`, as a Bevy unit direction:
/// `wow (x, y, z) → bevy (−y, z, −x)` maps `(sin a, cos a, 0)` to `(−cos a, 0, −sin a)`.
pub(crate) fn wow_azimuth_to_bevy(a: f32) -> Vec3 {
    Vec3::new(-a.cos(), 0.0, -a.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wow_azimuth_matches_coords() {
        for i in 0..16 {
            let a = i as f32 / 16.0 * std::f32::consts::TAU - std::f32::consts::PI;
            let via_coords = benilla_assets::coords::wow_to_bevy([a.sin(), a.cos(), 0.0]);
            assert!(
                (wow_azimuth_to_bevy(a) - via_coords).length() < 1e-6,
                "azimuth {a} maps to {:?}, coords say {via_coords:?}",
                wow_azimuth_to_bevy(a),
            );
        }
    }

    #[test]
    fn heading_takes_the_facing_below_a_yard_per_second() {
        let dt = 1.0 / 60.0;
        let mut wind = WeatherWind::default();
        assert_eq!(wind.heading_wow(), 0.0);
        assert!(wind.mist_yaw().angle_between(Quat::IDENTITY) < 1e-6);
        // Walk Bevy +X at 8 yd/s for 0.3 s.
        for i in 0..18 {
            wind.update(Vec3::X * (8.0 * dt * i as f32), Vec3::X, 8.0, dt);
        }
        assert!(wind.vel.length() > 1.0, "window should read ~8 yd/s");
        // Bevy +X = WoW −Y ⇒ wind_wow = (0, −8) ⇒ heading −π/2.
        let moving = wind.heading_wow();
        assert!((moving + std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        // Stop facing Bevy −Z: the wind decays and the heading follows the facing to 0.
        let last = wind.last_pos.unwrap();
        for _ in 0..60 {
            wind.update(last, Vec3::NEG_Z, 0.0, dt);
        }
        assert!(wind.vel.length() < 0.01);
        assert!(
            wind.heading_wow().abs() < 1e-3,
            "stopped and facing −Z, the heading should read 0, got {}",
            wind.heading_wow()
        );
        // …and the slab levels the moment the commanded speed is 0, with no 149 ms tail.
        assert_eq!(wind.slab, Quat::IDENTITY);
    }

    #[test]
    fn the_slab_tracks_the_commanded_speed_not_the_averaged_wind() {
        let dt = 1.0 / 60.0;
        let mut wind = WeatherWind::default();
        // One frame of running: the average has barely moved, the slab is already fully leaned.
        wind.update(Vec3::X * (7.0 * dt), Vec3::X, 7.0, dt);
        let deg = |q: Quat| q.angle_between(Quat::IDENTITY).to_degrees();
        assert!(
            wind.vel.length() < 7.0,
            "the 149 ms average should still be catching up, got {}",
            wind.vel.length()
        );
        assert!(
            (deg(wind.slab) - 25.28).abs() < 0.1,
            "slab {} — 65°·(7/18) is 25.28°",
            deg(wind.slab)
        );
        // Keep running so the average converges, then stop dead.
        for i in 2..40 {
            wind.update(Vec3::X * (7.0 * dt * i as f32), Vec3::X, 7.0, dt);
        }
        assert!(wind.vel.length() > 6.0, "the average has caught up");
        wind.update(Vec3::X * (7.0 * dt * 39.0), Vec3::X, 0.0, dt);
        assert!(wind.vel.length() > 1.0, "the average still carries the run");
        assert_eq!(wind.slab, Quat::IDENTITY, "but the slab is flat that frame");
    }

    /// The stream opposes the player's motion; a sign regression reads as lateral drift.
    #[test]
    fn mist_stream_blows_anti_motion() {
        let mut wind = WeatherWind::default();
        let dt = 1.0 / 60.0;
        // Walk Bevy +X (east) at 8 yd/s.
        for i in 0..18 {
            wind.update(Vec3::X * (8.0 * dt * i as f32), Vec3::X, 8.0, dt);
        }
        let motion = Vec3::X;
        // The base azimuth −1.57 is the stream's centre; spin it by the mist frame.
        let stream = wind.mist_yaw() * wow_azimuth_to_bevy(-1.57);
        assert!(
            stream.dot(motion) < -0.99,
            "stream {stream:?} must oppose motion {motion:?}"
        );
    }
}
