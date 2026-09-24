//! Day/night math ported from the 1.12 client's `DayNight` tables: the lighting sun, the visible
//! sun and moons (the sky-bodies builder `0x6d3b80`), the celestial curves and their interpolator.

use bevy::math::Vec3;

use benilla_assets::coords::wow_to_bevy;

/// The lighting sun, `DayNight::SetDirection` (wowdev.wiki Rendering/DayNight): azimuth fixed at
/// 225° and polar angle wobbling between ~110° and ~127°, so the sun never sets and night comes
/// from the colours. Returns the Bevy-space travel direction; `minute` is the game minute of day.
pub(super) fn sun_direction(minute: f32) -> Vec3 {
    // `(dayProgression, φ)`; the client's half-minute / 2880 equals minute / 1440.
    const PHI_TABLE: [(f32, f32); 4] = [
        (0.0, 2.2165682),
        (0.25, 1.9198623),
        (0.5, 2.2165682),
        (0.75, 1.9198623),
    ];
    const THETA: f32 = 3.926991; // 225°, constant across the day
    let dp = minute / 1440.0;
    let phi = interp_daynight(&PHI_TABLE, dp);
    let (sp, cp) = (phi.sin(), phi.cos());
    let wow_dir = [sp * THETA.cos(), sp * THETA.sin(), cp];
    wow_to_bevy(wow_dir).normalize()
}

/// The visible sun, body 0 of `0x6d3b80`: on the lighting sun's 45° bearing it rises and sets,
/// polar angle 5° at solar noon to 100° (10° below the horizon) at night, offset from the camera in
/// world space with no rotation. Returns the Bevy-space to-sun direction.
pub(super) fn celestial_sun_direction(minute: f32) -> Vec3 {
    // Elevation table `0xce8d64` (dayProgression, polar φ): 100° is 10° below the horizon, 5° near
    // the zenith, held across noon; the azimuth table `0xce8d4c` is a constant 45°.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.2291667, 1.7453293), // 100°  (near the horizon, sunrise)
        (0.4965278, 0.0872665), //   5°  (near the zenith, rising into noon)
        (0.5, 0.0872665),       //   5°  (solar noon)
        (0.5034722, 0.0872665), //   5°  (near the zenith, falling out of noon)
        (0.8958333, 1.7453293), // 100°  (near the horizon, sunset)
    ];
    const THETA: f32 = std::f32::consts::FRAC_PI_4; // 45°, the lighting sun's bearing
    let dp = minute / 1440.0;
    let phi = interp_daynight(&ELEV_TABLE, dp);
    let (sp, cp) = (phi.sin(), phi.cos());
    // to-sun = (cosθ·sinφ, sinθ·sinφ, cosφ) in WoW space, not negated: the builder offsets the
    // disc toward the sun.
    let wow_to_sun = [sp * THETA.cos(), sp * THETA.sin(), cp];
    wow_to_bevy(wow_to_sun).normalize()
}

/// The white moon (`moon.blp`), camera to moon in Bevy space (`0x6d3b80`): elevation table
/// `0xce8d24` puts it overhead at midnight (φ 35°, +55°) and parks it 10° below the horizon from
/// 04:00 to 22:00; azimuth a constant 45°, the sun's bearing (table `0xce8d0c`).
pub(super) fn moon_direction(minute: f32) -> Vec3 {
    // Elevation table (dayProgression, polar φ rad): 35° = π·0.19444, 100° = π·0.55556.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.000000, 0.6108652), // 35°, midnight (overhead, +55°)
        (0.003472, 0.6108652), // 35°
        (0.166667, 1.7453293), // 100°, 04:00 (below the horizon, −10°)
        (0.916667, 1.7453293), // 100°, 22:00 (still below the horizon)
        (0.996528, 0.6108652), // 35°, risen again before midnight
    ];
    let dp = minute / 1440.0;
    let phi = interp_daynight(&ELEV_TABLE, dp);
    let theta = std::f32::consts::FRAC_PI_4; // 45°, the sun's bearing
    let (sp, cp) = (phi.sin(), phi.cos());
    let wow_to_moon = [sp * theta.cos(), sp * theta.sin(), cp];
    wow_to_bevy(wow_to_moon).normalize()
}

/// `moon02` (`moon02.blp`), the third sky body: its Bevy direction and size multiplier. It is drawn
/// every frame but vertex-black, since its colour field `[0xce98a4]` has no writer, so it never
/// shows as a second moon. Its tracks run on `fmod(dayCounter + todPhase, 1.7)` (`0x6d41b9`;
/// `day_continuous` is that sum), which the track kernel `0x6cf6c0` clamps to [0, 1], so the whole
/// [1.0, 1.7) leg parks at azimuth 135°, elevation +55°. Azimuth `0xce8ccc` sweeps 135° to 165°,
/// elevation `0xce8ce4` has the white moon's shape, and the size samples `0xce8c8c` at base ×1.0.
pub(super) fn moon02_state(day_continuous: f64) -> (Vec3, f32) {
    // (dayFraction, azimuth rad): 135° = π·0.75 [0x8012cc], 150° = π·0.8333 [0x810018],
    // 165° = π·0.9167 [0x811644]; key times 04:00 / 22:00 like the elevation track.
    const AZ_TABLE: [(f32, f32); 3] = [
        (0.000000, 2.3561945), // 135°
        (0.166667, 2.6179938), // 150°
        (0.916667, 2.8797932), // 165°
    ];
    // (dayFraction, polar φ rad): the white moon's shape on moon02's own clock.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.000000, 0.6108652), // 35°, phase 0 (overhead, +55°)
        (0.003472, 0.6108652), // 35°
        (0.166667, 1.7453293), // 100°, below the horizon (clipped away)
        (0.916667, 1.7453293), // 100°
        (0.996528, 0.6108652), // 35°
    ];
    // The kernel clamp: past 1.0 the phase evaluates at 1.0 (`0x6cf6c0`).
    let phase = (day_continuous.rem_euclid(1.7) as f32).min(1.0);
    let theta = interp_daynight(&AZ_TABLE, phase);
    let phi = interp_daynight(&ELEV_TABLE, phase);
    let (sp, cp) = (phi.sin(), phi.cos());
    let wow_to_body = [sp * theta.cos(), sp * theta.sin(), cp];
    (
        wow_to_bevy(wow_to_body).normalize(),
        interp_daynight(&MOON_SIZE_CURVE, phase),
    )
}

/// The dawn/dusk sky-warp strength curve (table `0xce9b2c`, written by `0x6ce120`, gating the warp
/// `0x6d0f50`): spikes at about 06:29 and 21:29, 0 everywhere else.
const SKY_WARP_CURVE: [(f32, f32); 6] = [
    (0.1250, 0.0), // 03:00
    (0.2708, 1.0), // 06:29, dawn spike
    (0.2917, 0.0), // 07:00
    (0.8542, 0.0), // 20:30
    (0.8958, 1.0), // 21:29, dusk spike
    (0.9993, 0.0), // 23:59
];

/// The sky warp strength `S = curve(dayfrac) × highlight_sky`.
pub(super) fn sky_warp(minute: f32, highlight_sky: f32) -> f32 {
    interp_daynight(&SKY_WARP_CURVE, minute / 1440.0) * highlight_sky
}

/// The sun disc's size multiplier, table `0xce8cac` at base ×1.0: 2× at the horizon (06:00, 21:00)
/// and 1× across midday; its night-side 2× is never seen, the sun being below the horizon.
const SUN_SIZE_CURVE: [(f32, f32); 4] = [
    (0.25000, 2.0), // 06:00, sunrise (largest)
    (0.28125, 1.0), // 06:45, the midday plateau
    (0.84375, 1.0), // 20:15, still small
    (0.87500, 2.0), // 21:00, sunset (largest)
];

pub(super) fn sun_disc_scale(minute: f32) -> f32 {
    interp_daynight(&SUN_SIZE_CURVE, minute / 1440.0)
}

/// The moon discs' size multiplier, table `0xce8c8c`, shared by both: 1.5× at moonrise and moonset
/// (about 22:00 and 04:00), 1× overhead, times the disc's base (white ×1.75, moon02 ×1.0).
const MOON_SIZE_CURVE: [(f32, f32); 4] = [
    (0.041667, 1.0), // 01:00, overhead (smallest)
    (0.166667, 1.5), // 04:00, moonset
    (0.916667, 1.5), // 22:00, moonrise
    (0.999306, 1.0), // 23:59, wrapping toward 01:00
];

/// The moon disc's size multiplier at `minute`, before the disc's base.
pub(super) fn moon_disc_scale(minute: f32) -> f32 {
    interp_daynight(&MOON_SIZE_CURVE, minute / 1440.0)
}

/// The star field's alpha curve `0xce9a98`, the `Stars.m2` model-global alpha
/// (`[ce96d8+0xb]/255`): fades in from 22:30 to 00:00 and out from 03:00 to 04:30, 0 all day (the
/// reference skips the draw below byte 2).
const STAR_CURVE: [(f32, f32); 4] = [
    (0.0, 1.0),    // 00:00, full
    (0.125, 1.0),  // 03:00, still full
    (0.1875, 0.0), // 04:30, out
    (0.9375, 0.0), // 22:30, fading in toward 00:00
];

pub(super) fn star_alpha(minute: f32) -> f32 {
    interp_daynight(&STAR_CURVE, minute / 1440.0)
}

/// The sun lens-flare day envelope, the per-body dnCurve at `[glare+0x70]` (sun `0xce9818`, built
/// by `0x6d1e30`): off until 06:30, full from 07:30 to 19:30, off by 21:00.
const SUN_FLARE_DN_CURVE: [(f32, f32); 4] = [
    (0.2708333, 0.0), // 06:30, still off
    (0.3125, 1.0),    // 07:30, full
    (0.8125, 1.0),    // 19:30, still full
    (0.875, 0.0),     // 21:00, off before sunset
];

pub(super) fn sun_flare_dn(minute: f32) -> f32 {
    interp_daynight(&SUN_FLARE_DN_CURVE, minute / 1440.0)
}

/// The moon lens-flare night envelope (`0xce9768`, built by `0x6d1f30`): zero from 03:15 to
/// 22:45, ramping in to full at midnight and full to 02:00, so a 22:30 moonrise has no halo.
const MOON_FLARE_DN_CURVE: [(f32, f32); 4] = [
    (0.0833333, 1.0), // 02:00, still full
    (0.1354167, 0.0), // 03:15, out
    (0.9479167, 0.0), // 22:45, first light
    (0.999306, 1.0),  // 23:59, full; the wrap to 02:00 holds it across midnight
];

pub(super) fn moon_flare_dn(minute: f32) -> f32 {
    interp_daynight(&MOON_FLARE_DN_CURVE, minute / 1440.0)
}

/// The SIDN night fraction track `0xce9a34` (constants `0x811518`/`0x8115bc`, built by
/// `0x6ce670`), sampled into `DNState+0x1ac`; the WMO material updater `0x6b4090` scales every
/// SIDN material's emissive by it: 1 overnight, off from 06:00 to 07:00, on from 20:30 to 21:30.
const SIDN_NIGHT_CURVE: [(f32, f32); 4] = [
    (0.25, 1.0),      // 06:00, still full
    (0.2916667, 0.0), // 07:00, off for the day
    (0.8541667, 0.0), // 20:30, ramping in
    (0.8958333, 1.0), // 21:30, full; the wrap to 06:00 holds it
];

pub(super) fn sidn_night_fraction(minute: f32) -> f32 {
    interp_daynight(&SIDN_NIGHT_CURVE, minute / 1440.0)
}

/// The cloud sun-glow envelope, the static 8-key track `0xce9ab8` (built by `0x6ce390`, not a
/// `Light.dbc` band): about 1 all day with twilight notches. Its keys are stored non-monotonic
/// (6 and 7 fall before 5 in time), and the kernel's array-order scan (`0x6cf6c0`) never reaches
/// them and wraps `t > 0.9236` back to 1, so the stored order is kept for [`interp_daynight`].
pub(super) fn cloud_glow_track(minute: f32) -> f32 {
    const CLOUD_GLOW_CURVE: [(f32, f32); 8] = [
        (0.16667, 1.0), // 04:00, full
        (0.19444, 0.0), // 04:40, dawn notch
        (0.20139, 0.0), // 04:50
        (0.22917, 1.0), // 05:30, full through the day
        (0.89583, 1.0), // 21:30
        (0.92361, 0.0), // 22:10, dusk notch
        (0.88889, 0.0), // unreachable, kept in the stored order
        (0.91667, 1.0), // unreachable: past 22:10 the seam wraps back to 1
    ];
    interp_daynight(&CLOUD_GLOW_CURVE, minute / 1440.0)
}

/// The cloud glow's body (`0x6cfb00`): the sun from about 04:50 to 22:10, else the moon.
pub(super) fn cloud_glow_is_sun(minute: f32) -> bool {
    let dp = minute / 1440.0;
    (0.201_388_9..=0.923_611_1).contains(&dp)
}

/// The reference's `DayNight::InterpTable`: wrap-around linear interpolation of a
/// `(dayProgression, value)` table over `dp ∈ [0, 1)`; both of its return branches are this lerp.
fn interp_daynight(table: &[(f32, f32)], dp: f32) -> f32 {
    let n = table.len();
    let mut a = 0;
    while a < n && dp > table[a].0 {
        a += 1;
    }
    let (a, b) = if a == n || a == 0 {
        (if a == n { 0 } else { a }, n - 1)
    } else {
        (a, a - 1)
    };
    let mut span = table[a].0 - table[b].0;
    if span < 0.0 {
        span += 1.0;
    }
    let mut into = dp - table[b].0;
    if into < 0.0 {
        into += 1.0;
    }
    let t = if span != 0.0 { into / span } else { 0.0 };
    table[b].1 + t * (table[a].1 - table[b].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cloud glow envelope under `0x6cf6c0`'s scan: full by day, zero at the twilight notches,
    /// and back to 1 past 22:10.
    #[test]
    fn cloud_glow_track_matches_the_client_envelope() {
        assert_eq!(cloud_glow_track(720.0), 1.0); // noon
        assert_eq!(cloud_glow_track(0.20139 * 1440.0), 0.0); // the 04:50 notch
        let notch = cloud_glow_track(0.9236 * 1440.0); // approaching the 22:10 notch from below
        assert!(notch < 0.01, "dusk notch {notch}");
        // One step past the notch the scan runs off the non-monotonic tail and the seam wraps
        // the envelope back to 1.
        assert_eq!(cloud_glow_track(0.9237 * 1440.0), 1.0);
        assert_eq!(cloud_glow_track(0.95 * 1440.0), 1.0); // deep night stays wrapped
        let mid_dusk = cloud_glow_track(0.9097 * 1440.0); // halfway 21:30→22:10
        assert!((mid_dusk - 0.5).abs() < 0.01, "mid-dusk {mid_dusk}");
        assert!(cloud_glow_is_sun(720.0));
        assert!(!cloud_glow_is_sun(0.95 * 1440.0));
    }

    // The `DayNight::SetDirection` φ keyframes.
    #[test]
    fn daynight_phi_matches_client_keyframes() {
        const PHI: [(f32, f32); 4] = [
            (0.0, 2.2165682),
            (0.25, 1.9198623),
            (0.5, 2.2165682),
            (0.75, 1.9198623),
        ];
        assert!((interp_daynight(&PHI, 0.0) - 2.2165682).abs() < 1e-5); // midnight
        assert!((interp_daynight(&PHI, 0.25) - 1.9198623).abs() < 1e-5); // 06:00
        assert!((interp_daynight(&PHI, 0.5) - 2.2165682).abs() < 1e-5); // noon
        assert!((interp_daynight(&PHI, 0.125) - 2.0682153).abs() < 1e-4); // halfway: the midpoint
    }

    // The SIDN track `0xce9a34`: 1 overnight, 0 by day, ramps 20:30 to 21:30 and 06:00 to 07:00.
    #[test]
    fn sidn_night_fraction_matches_the_client_track() {
        let f = |h: f32, m: f32| sidn_night_fraction(h * 60.0 + m);
        assert!((f(0.0, 0.0) - 1.0).abs() < 1e-5, "midnight: full glow");
        assert!((f(6.0, 0.0) - 1.0).abs() < 1e-5, "06:00: still full");
        assert!((f(6.0, 30.0) - 0.5).abs() < 1e-5, "06:30: halfway out");
        assert!(f(7.0, 0.0).abs() < 1e-5, "07:00: off for the day");
        assert!(f(12.0, 0.0).abs() < 1e-5, "noon: off");
        assert!(f(20.0, 30.0).abs() < 1e-5, "20:30: about to ramp in");
        assert!((f(21.0, 0.0) - 0.5).abs() < 1e-5, "21:00: halfway in");
        assert!((f(21.0, 30.0) - 1.0).abs() < 1e-4, "21:30: full glow");
        assert!((f(23.0, 0.0) - 1.0).abs() < 1e-5, "23:00: full (wrap leg)");
    }

    #[test]
    fn sun_is_always_above_the_horizon() {
        // Bevy +Y is up, so the travel direction points down at every minute.
        for minute in (0..1440).step_by(30) {
            assert!(
                sun_direction(minute as f32).y < 0.0,
                "sun below horizon at minute {minute}"
            );
        }
    }

    #[test]
    fn sun_azimuth_is_fixed_all_day() {
        let heading = |m: f32| {
            let d = sun_direction(m);
            d.x.atan2(d.z)
        };
        let noon = heading(720.0);
        for minute in (0..1440).step_by(30) {
            assert!(
                (heading(minute as f32) - noon).abs() < 1e-4,
                "azimuth drifted at minute {minute}"
            );
        }
    }

    // Elevation above the Bevy horizon (+Y up) of a to-sun direction.
    fn elev_deg(dir: Vec3) -> f32 {
        dir.normalize().y.asin().to_degrees()
    }

    #[test]
    fn celestial_sun_rises_and_sets_in_elevation() {
        let noon = elev_deg(celestial_sun_direction(720.0)); // φ 5°  → +85°
        let dawn = elev_deg(celestial_sun_direction(0.0)); // φ 100° → −10°
        let morning = elev_deg(celestial_sun_direction(480.0)); // 08:00, φ 63° → +27°
        assert!((noon - 85.0).abs() < 1.0, "solar noon ~+85°, got {noon}");
        assert!((dawn + 10.0).abs() < 1.0, "midnight ~−10°, got {dawn}");
        assert!((morning - 27.0).abs() < 2.0, "08:00 ~+27°, got {morning}");
        assert!(noon > morning + 30.0, "should climb steeply toward noon");
        assert!(noon > dawn + 80.0, "midnight far below noon");
    }

    #[test]
    fn celestial_sun_dips_below_horizon_at_night() {
        assert!(
            celestial_sun_direction(0.0).y < 0.0,
            "midnight below horizon"
        );
        assert!(
            celestial_sun_direction(1380.0).y < 0.0,
            "23:00 below horizon"
        );
        assert!(celestial_sun_direction(720.0).y > 0.0, "noon above horizon");
    }

    #[test]
    fn celestial_sun_shares_the_lighting_bearing() {
        let heading = |d: Vec3| d.x.atan2(d.z);
        let light_to_sun = -sun_direction(600.0); // lighting to-sun
        let celes = celestial_sun_direction(600.0);
        let dh = (heading(celes) - heading(light_to_sun)).abs();
        assert!(
            dh < 1e-3,
            "celestial & lighting suns share a bearing, Δ={dh}"
        );
        // At 10:00 the visible sun rides higher, 56° against 31°.
        assert!(
            elev_deg(celes) > elev_deg(light_to_sun) + 15.0,
            "visible sun should ride higher than the lighting sun"
        );
    }

    #[test]
    fn sky_warp_curve_spikes_at_dawn_dusk_and_is_zero_midday() {
        let s = |min: u32| interp_daynight(&SKY_WARP_CURVE, min as f32 / 1440.0);
        assert_eq!(s(720), 0.0, "noon"); // 12:00
        assert_eq!(s(0), 0.0, "midnight"); // 00:00, wrapping between two 0 keys
        assert_eq!(s(1080), 0.0, "18:00");
        assert!(s(390) > 0.99, "dawn spike ~1.0, got {}", s(390));
        assert!(s(1290) > 0.99, "dusk spike ~1.0, got {}", s(1290));
        let mid = s(360);
        assert!(mid > 0.0 && mid < 1.0, "dawn ramp partial, got {mid}");
    }

    #[test]
    fn sun_disc_doubles_at_the_horizon_and_is_unit_at_midday() {
        // Size table `0xce8cac`.
        assert!(
            (sun_disc_scale((6 * 60) as f32) - 2.0).abs() < 1e-3,
            "06:00 sunrise = 2×"
        );
        assert!(
            (sun_disc_scale((21 * 60) as f32) - 2.0).abs() < 1e-3,
            "21:00 sunset = 2×"
        );
        assert!(
            (sun_disc_scale((12 * 60) as f32) - 1.0).abs() < 1e-3,
            "noon = 1×"
        );
        assert!(
            (sun_disc_scale((9 * 60) as f32) - 1.0).abs() < 1e-3,
            "09:00 plateau = 1×"
        );
        let r = sun_disc_scale((6 * 60 + 20) as f32);
        assert!(r > 1.0 && r < 2.0, "post-sunrise shrink ramp, got {r}");
    }

    #[test]
    fn stars_fade_in_at_night_and_are_off_by_day() {
        // Star curve `0xce9a98`.
        assert!((star_alpha((0) as f32) - 1.0).abs() < 1e-3, "00:00 full"); // midnight
        assert!(
            (star_alpha((2 * 60) as f32) - 1.0).abs() < 1e-3,
            "02:00 full"
        );
        assert_eq!(star_alpha((12 * 60) as f32), 0.0, "noon off"); // 12:00
        assert_eq!(star_alpha((18 * 60) as f32), 0.0, "18:00 off");
        let dusk = star_alpha((23 * 60 + 15) as f32);
        assert!(dusk > 0.0 && dusk < 1.0, "fading in at 23:15, got {dusk}");
        let dawn = star_alpha((3 * 60 + 45) as f32);
        assert!(dawn > 0.0 && dawn < 1.0, "fading out at 03:45, got {dawn}");
    }

    #[test]
    fn flare_dn_curves_gate_the_halos_by_time_of_day() {
        // The dnCurve tables, sun `0xce9818` and moon `0xce9768`: a 22:30 moonrise has no halo.
        assert_eq!(moon_flare_dn((22 * 60 + 30) as f32), 0.0, "22:30 no halo");
        assert_eq!(moon_flare_dn((12 * 60) as f32), 0.0, "noon no halo");
        let m2300 = moon_flare_dn((23 * 60) as f32);
        assert!((m2300 - 0.203).abs() < 5e-3, "23:00 ≈ 0.20, got {m2300}");
        let m2330 = moon_flare_dn((23 * 60 + 30) as f32);
        assert!((m2330 - 0.608).abs() < 5e-3, "23:30 ≈ 0.61, got {m2330}");
        assert!(
            (moon_flare_dn((60) as f32) - 1.0).abs() < 1e-3,
            "01:00 full (the wrap across midnight holds 1.0)"
        );
        assert!(moon_flare_dn((3 * 60 + 15) as f32) < 1e-3, "03:15 out");
        assert!((sun_flare_dn((12 * 60) as f32) - 1.0).abs() < 1e-3, "noon");
        assert_eq!(sun_flare_dn((21 * 60) as f32), 0.0, "21:00 off");
        assert_eq!(sun_flare_dn((23 * 60) as f32), 0.0, "night off");
        // 390/1440 isn't f32-representable, so the key lookup sits an epsilon up the ramp.
        assert!(sun_flare_dn((6 * 60 + 30) as f32) < 1e-3, "06:30 still off");
    }

    #[test]
    fn moon_disc_enlarges_at_moonrise_and_set() {
        // The shared size table `0xce8c8c`.
        let m = |min: u32| interp_daynight(&MOON_SIZE_CURVE, min as f32 / 1440.0);
        assert!((m(22 * 60) - 1.5).abs() < 1e-3, "22:00 moonrise = 1.5×");
        assert!((m(4 * 60) - 1.5).abs() < 1e-3, "04:00 moonset = 1.5×");
        assert!((m(60) - 1.0).abs() < 1e-3, "01:00 overhead = 1×");
    }

    // moon02's precessed clock (`0x6d41b9`'s fmod by 1.7, then `0x6cf6c0`'s clamp): the normal
    // leg follows the white moon's shape on its own bearing, and [1.0, 1.7) parks at the clamp.
    #[test]
    fn moon02_precesses_and_parks_on_the_clamp_leg() {
        // Phase 0, day 0 midnight: up at +55°, azimuth on the 135° key.
        let (dir, scale) = moon02_state(0.0);
        assert!(dir.y > 0.5, "phase 0: up");
        assert!((scale - 1.0).abs() < 1e-3, "phase 0: overhead size 1×");
        assert!(moon02_state(0.5).0.y < 0.0, "phase 0.5: below horizon");
        // Any remainder in [1.0, 1.7) evaluates at 1.0: parked up, one state.
        let (park_a, sa) = moon02_state(1.05);
        let (park_b, sb) = moon02_state(1.65);
        assert!(park_a.y > 0.5, "clamp leg: parked above the horizon");
        assert!(
            (park_a - park_b).length() < 1e-6 && (sa - sb).abs() < 1e-6,
            "clamp leg: frozen"
        );
        // The same time on consecutive days lands on different phases: day 10 midnight is
        // fmod(10, 1.7) ≈ 1.5 (parked up), day 11 is ≈ 0.8 (below the horizon).
        assert!(moon02_state(10.0).0.y > 0.5, "day 10 midnight: parked leg");
        assert!(
            moon02_state(11.0).0.y < 0.0,
            "day 11 midnight: below horizon"
        );
        // moon02's bearing is its own (135° to 165°), never the white moon's 45°.
        let heading = |d: Vec3| d.x.atan2(d.z).to_degrees();
        let h02 = heading(moon02_state(0.0).0);
        let hw = heading(moon_direction(0.0));
        assert!(
            (h02 - hw).abs() > 30.0,
            "moon02 sits on a different bearing"
        );
    }

    #[test]
    fn moon_rises_at_night_sets_by_day() {
        // The white moon: +55° at midnight, −10° at noon.
        assert!(moon_direction(0.0).y > 0.5, "moon up at midnight");
        assert!(
            moon_direction((12 * 60) as f32).y < 0.0,
            "moon below horizon at noon"
        );
    }
}
