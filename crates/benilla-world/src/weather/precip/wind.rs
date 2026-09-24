//! The weather **wind** — the local player's trailing-average velocity (`0x67c150`) and the
//! three frames derived from it: the **spawn-slab tilt**, the streak apex tilt, and the mist
//! spawn yaw. Split from `precip`'s root; the sim/spawn economics stay there.

use bevy::prelude::*;

/// Streak tilt: `lerp(0°, 45°, sat(|wind.xy|/30))` (`0x674a70`; 30 @`0x8680f0`,
/// 45 @`0x8680f4`). Applied to the streak's APEX vertex only (the `M·(2·antiVel)` term).
const WIND_TILT_DIV: f32 = 30.0;
const WIND_TILT_MAX_DEG: f32 = 45.0;

/// **Spawn-slab tilt**: `α = lerp(0°, 65°, sat(|wind|/18))` (`0x677965`–`0x677a55`). Shares the
/// ⊥-heading axis with the streak tilt but is a *separate* rotation with its own ramp — steeper
/// (65° vs 45°) and reaching full tilt at a lower speed (18 vs 30 yd/s). See
/// [`super::pool::spawn_particle`] for what it does and why its absence was B233.
const SLAB_TILT_DIV: f32 = 18.0;
const SLAB_TILT_MAX_DEG: f32 = 65.0;

/// Wind = the **local player's** trailing-average velocity (`0x67c150`, verified: the list is
/// per-frame position deltas of object 0x8fe — the player, not the camera — trimmed to a
/// **~149 ms** window; `wind = Σ(Δpos)/((Σ(Δms)+1)·0.001)`, a true yd/s velocity). Orbiting
/// the camera therefore does NOT stir the wind. The streak apex tilt and the spawn-box lead
/// derive from it here.
///
/// **The spawn-slab tilt does not.** It keys on `mgr+0x7c`, which merges two sources at one store
/// (`0x67bf91`): the ridden transport's averaged planar speed when `|mgr+0x5c|² > 2`, else the
/// local player's own **live** CMovement speed `[[player+0x118]+0x84]`. benilla has no ridden
/// transports yet, so it is always the second — [`crate::player::Player::planar_speed`], passed
/// into [`Self::update`]. Averaging it would be wrong in both directions: late to lean in, and
/// still leaning 150 ms after the player stops.
#[derive(Resource)]
pub(crate) struct WeatherWind {
    pub(super) last_pos: Option<Vec3>,
    /// The trailing window: per-frame (Δseconds, Δpos), trimmed from the head to ≤ 149 ms.
    window: Vec<(f32, Vec3)>,
    /// The windowed planar velocity (Bevy x/z, y forced 0), yd/s.
    pub(super) vel: Vec3,
    /// The wind heading (WoW frame, `manager+0x78`) — the wind's own azimuth at |wind| ≥ 1, the
    /// unit's facing below it. Rewritten every frame; it does **not** hold through calm.
    heading: f32,
    /// The same heading as a **Bevy planar unit direction**. Kept alongside the angle rather than
    /// re-derived from it, because the two angle conventions in this file are 90° apart and only
    /// compose correctly as a *rotation*: [`heading`] is `atan2(y, x)` in the WoW frame, while
    /// [`wow_azimuth_to_bevy`] reads its argument as the kernels' `(vx, vy) = (sin a, cos a)`
    /// pair — i.e. `atan2(x, y)`. [`Self::mist_yaw`] composes them as a rotation and is fine;
    /// feeding `heading` straight into `wow_azimuth_to_bevy` to get a *direction* would silently
    /// yield an axis a quarter turn off.
    heading_dir: Vec3,
    /// Streak-field tilt (rotates the fall axis toward the motion), from `0x674a70`.
    pub(super) tilt: Quat,
    /// The **spawn-slab** tilt — rotates each particle's slab-local offset into the direction of
    /// travel, so the volume's leading edge is born low and close instead of a flat `z_off` up.
    /// `α = 65°·sat(live_speed/18)`, linear from zero with **no dead zone** (`0x674ba0` is a plain
    /// clamped lerp and `0x677965`–`0x677a41` has no branch at all): 9° at a 2.5 yd/s walk, 25.3°
    /// at a 7 yd/s run, saturated from 18 up. Exactly identity at rest, on the same frame.
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

/// The wind window length: `Σ(Δtick) ≤ 0x95 = 149 ms` (the pre-pass trims the delta list to a
/// trailing ~149 ms).
const WIND_WINDOW_S: f32 = 0.149;

impl WeatherWind {
    /// `pos` — the local player's feet; `facing` — their aim as a Bevy direction (the heading's
    /// source below 1 yd/s); `live_speed` — their commanded planar speed in yd/s
    /// ([`crate::player::Player::planar_speed`], the reference's `mgr+0x7c`).
    pub(super) fn update(&mut self, pos: Vec3, facing: Vec3, live_speed: f32, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let delta = match self.last_pos {
            Some(prev) => {
                let mut d = pos - prev;
                d.y = 0.0;
                // A teleport/worldport delta is not wind — restart the window.
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
        // Trim from the head to the trailing window (the reference's Σ(Δtick) ≤ 149 ms).
        let mut total: f32 = self.window.iter().map(|(d, _)| d).sum();
        while self.window.len() > 1 && total - self.window[0].0 >= WIND_WINDOW_S {
            total -= self.window[0].0;
            self.window.remove(0);
        }
        let sum_dpos: Vec3 = self.window.iter().map(|(_, d)| *d).sum();
        // wind = Σ(Δpos) / ((Σ(Δms)+1)·0.001) — displacement over seconds, true yd/s.
        self.vel = sum_dpos / (total.mul_add(1000.0, 1.0) * 0.001);

        let mag2 = self.vel.length_squared();
        // `mgr+0x78` (`0x67be40`): the `|W_xy| ≥ 1` test selects the heading's SOURCE — it does
        // NOT suppress the write. All three of its legs store `[esi+0x78]` (`0x67bee0`/`0x67bef6`/
        // `0x67bf02`), and below 1 yd/s the heading is overwritten with the **unit's own facing**
        // (`0x67beff call [vtbl+0x18]`). The earlier "it HOLDS through calm" reading was wrong.
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

        // Both rotations turn about `ŷ × ĥ` = `(h.z, 0, −h.x)`: a positive angle about it carries
        // `ŷ` toward `ĥ` (`R·ŷ = ŷcos + ĥsin`), i.e. leans into the heading. The handedness is
        // verified for the streak — the apex tips DOWNWIND, rain rushing into a moving player —
        // and holds equally for the slab.
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
        // The streak apex (`0x674a70`) keys on the AVERAGED wind, and dies below
        // |wind|² < 0.001.
        self.tilt = if mag2 < 0.001 {
            Quat::IDENTITY
        } else {
            lean(self.vel, mag2.sqrt(), WIND_TILT_DIV, WIND_TILT_MAX_DEG)
        };
        // The slab keys on `mgr+0x7c` — the LIVE commanded speed, not the averaged wind, and not
        // lagged by the 149 ms window. It is exactly 0 with no direction bit held, so this is
        // identity at rest with no epsilon needed.
        self.slab = lean(
            self.heading_dir,
            live_speed,
            SLAB_TILT_DIV,
            SLAB_TILT_MAX_DEG,
        );
    }

    /// The held wind heading (`manager+0x78`, WoW frame). Starts 0 = world north.
    pub(crate) fn heading_wow(&self) -> f32 {
        self.heading
    }

    /// The mist spawn frame: a yaw about the vertical by **+heading**. `0x67a990`
    /// feeds `−mgr+0x78` into the Z-rotation builder `0x7bdd60` — a wrapper around `0x7bdb00`,
    /// which is the NEGATED-handedness family (`M(axis, θ) = R_standard(axis, −θ)`) — so the true
    /// rotation is `R_standard(+Z_wow, +heading)`, and WoW +Z ↔ Bevy +Y carries
    /// the angle sign-intact. Net effect: the −1.57 base azimuth (south = anti-north) spins to
    /// **anti-player-motion** — the stream always blows into a moving player's face, and holds
    /// that axis when they stop. (The earlier `−heading` transcribed the builder's literal minus
    /// into Bevy's standard API — a mirror; the director saw it as lateral drift.)
    pub(crate) fn mist_yaw(&self) -> Quat {
        Quat::from_rotation_y(self.heading_wow())
    }
}

/// A WoW-frame horizontal azimuth (the spawn kernels' `(vx, vy) = (mag·sin a, mag·cos a)` pair)
/// as a Bevy unit direction: `wow (x,y,z) → bevy (−y, z, −x)` ⇒ `(sin a, cos a, 0) →
/// (−cos a, 0, −sin a)`.
pub(crate) fn wow_azimuth_to_bevy(a: f32) -> Vec3 {
    Vec3::new(-a.cos(), 0.0, -a.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The inline drift-azimuth mapping must agree with the canonical WoW→Bevy basis —
    /// `(sin a, cos a, 0)_wow` is the spawn kernels' `(vx, vy)` pair, and the world-fixed
    /// −1.57 centre only means anything if the frame conversion is the terrain's.
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

    /// `mgr+0x78`'s `|W| ≥ 1` test picks the heading's **source**, it does not gate the store
    /// (`0x67be40`: all three legs write `[esi+0x78]`). Moving, the heading is the wind's own
    /// azimuth; on stopping it is **overwritten with the unit's facing** rather than held.
    ///
    /// This test previously asserted the opposite ("standing still again HOLDS it"). That reading
    /// was wrong. The visible consequence is the mist frame: it re-aims to where the player is
    /// looking when they stop, instead of staying pinned to the direction they last ran.
    #[test]
    fn heading_takes_the_facing_below_a_yard_per_second() {
        let dt = 1.0 / 60.0;
        let mut wind = WeatherWind::default();
        assert_eq!(wind.heading_wow(), 0.0);
        assert!(wind.mist_yaw().angle_between(Quat::IDENTITY) < 1e-6);
        // Walk Bevy +X at 8 yd/s for 0.3 s — the ~149 ms window reads ~8 yd/s.
        for i in 0..18 {
            wind.update(Vec3::X * (8.0 * dt * i as f32), Vec3::X, 8.0, dt);
        }
        assert!(wind.vel.length() > 1.0, "window should read ~8 yd/s");
        // Bevy +X = WoW −Y ⇒ wind_wow = (0, −8) ⇒ heading −π/2.
        let moving = wind.heading_wow();
        assert!((moving + std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        // Stop, and turn to face Bevy −Z. The wind decays out of the window and the heading
        // follows the FACING to 0 — it does not stay at the direction of travel.
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

    /// The slab's tilt keys on the LIVE commanded speed (`mgr+0x7c`), not the averaged wind: on
    /// the very first frame of a run it is already at its full angle, and on the first frame of a
    /// stop it is already flat — while `vel`, the 149 ms average, is still catching up in both
    /// directions. Keying the tilt off `vel` would lean the slab in late and hold it late.
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

    /// The mist stream is ANTI-player-motion (`0x67a990`: `−heading` through the negated
    /// builder `0x7bdd60`→`0x7bdb00` = `R_std(+Z, +heading)`): a player moving along +v gets
    /// the −1.57-base stream spun to blow along −v — into their face, passing the camera on
    /// both sides. A sign regression here reads as lateral side-to-side drift.
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
