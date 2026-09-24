//! Day/night cycle math — pure functions, no Bevy systems. Owns the **two sun directions** (the
//! near-fixed lighting/shadow sun and the visible celestial sun that rises/sets), the wrap-around
//! `DayNight::InterpTable` interpolator, and the dawn/dusk sky-warp strength curve. All ported from
//! `WoW.exe` (`DayNight::SetDirection`, the sky-bodies builder `FUN_006d3b80`, the warp curve
//! `0xce9b2c`) and pinned by golden tests. The parent [`super`] consumes these per frame.

use bevy::math::Vec3;

use benilla_assets::coords::wow_to_bevy;

/// Vanilla day/night sun light direction, ported from `DayNight::SetDirection`
/// (wowdev.wiki Rendering/DayNight). The azimuth (`theta`) is **fixed** at 225° all day and the
/// elevation (`phi`) only wobbles between ~110° and ~127°, so `dir.z = cos(phi)` stays negative —
/// the sun is **always above the horizon**. Night is handled by the colors going dark, not by the
/// sun setting. Returns the **Bevy-space** light *travel* direction (the shader's `-light_sun` is the
/// to-light vector). `minute` is the game minute-of-day (`0..1440`).
pub(super) fn sun_direction(minute: f32) -> Vec3 {
    // (dayProgression, value) tables straight from the client. theta's table is constant, so we use
    // the value directly. dayProgression = minute / 1440 (the client uses half-min/2880, identical).
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
    // Spherical → cartesian in WoW world coords (Z up), then into Bevy (pure rotation).
    let wow_dir = [sp * THETA.cos(), sp * THETA.sin(), cp];
    wow_to_bevy(wow_dir).normalize()
}

/// Vanilla **visible celestial sun** direction — the sky disc/glare the player sees — ported from the
/// sky-bodies builder `FUN_006d3b80` (body 0). This is a **separate system** from the lighting sun
/// ([`sun_direction`]): the engine computes two suns. The lighting sun barely moves (elevation only
/// +20°..+37°, so MCSH shadows can be baked static), but the **visible** sun genuinely RISES AND SETS
/// in elevation along the same fixed compass bearing — its polar angle `φ` sweeps 5° (near-zenith at
/// solar noon) ↔ 100° (−10°, below the horizon at night), azimuth fixed at 45° to-sun (matching the
/// lighting sun's bearing, so the bright disc sits in the lit direction). Below the horizon (night)
/// the disc is parked at −10° and the dark sky takes over. Returns the **Bevy-space to-sun** direction
/// (camera→sun); `minute` = game minute-of-day (`0..1440`).
///
/// (VERIFIED off the binary; the disc/lighting suns measure 25–32° apart in the WoW.8 apitrace. The
/// once-flagged "builder-local→world rotation toward ~61°" is REFUTED — `0x6d3b80` adds the
/// spherical offset to the camera directly in world space, no rotation; decision 0485.)
pub(super) fn celestial_sun_direction(minute: f32) -> Vec3 {
    // Body-0 elevation LUT (dayProgression, polar φ in rad) — `FUN_006d3b80` table @0xce8d64, count 5
    // (decoded from the PE immediates: π·0.5555556 = 100°, π·0.0277778 = 5°). 100° = −10° (below the
    // horizon, night); 5° = +85° (near zenith, solar noon). The azimuth LUT @0xce8d4c is a constant
    // π·0.25 = 45°. The tightly-spaced keys around 0.5 hold the disc at the zenith across solar noon.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.2291667, 1.7453293), // 100°  (near horizon — dawn / sunrise)
        (0.4965278, 0.0872665), //   5°  (near zenith — rising into noon)
        (0.5, 0.0872665),       //   5°  (solar noon)
        (0.5034722, 0.0872665), //   5°  (near zenith — falling out of noon)
        (0.8958333, 1.7453293), // 100°  (near horizon — dusk / sunset)
    ];
    const THETA: f32 = std::f32::consts::FRAC_PI_4; // 45°, constant — the to-sun azimuth (= the light's)
    let dp = minute / 1440.0;
    let phi = interp_daynight(&ELEV_TABLE, dp);
    let (sp, cp) = (phi.sin(), phi.cos());
    // to-sun = (cosθ·sinφ, sinθ·sinφ, cosφ) in WoW world (Z up) — used directly (not negated: the
    // builder offsets the disc *toward* the sun), then into Bevy (pure rotation).
    let wow_to_sun = [sp * THETA.cos(), sp * THETA.sin(), cp];
    wow_to_bevy(wow_to_sun).normalize()
}

/// Visible **moon** direction (camera→moon, Bevy space) — the WHITE moon (`moon.blp`). Elevation
/// table `0xce8d24`: polar φ **35° (overhead at midnight, elev +55°) ↔ 100° (below the horizon by day, elev
/// −10°)** — up at night, parked below the horizon 04:00→22:00; azimuth a constant **45°** (the sun's
/// bearing, table `0xce8d0c`). `minute` = game minute-of-day. (VERIFIED off `0x6d3b80`; decision 0485.)
///
/// The engine's second disc (`moon02.blp`, azimuth 135°→165°, table `0xce8ccc`) IS drawn every frame —
/// the old "NULL-registered, never drawn" reading misattributed a glare-slot null in `0x6d4810` — but
/// its colour field (`[0xce98a4]`) has NO writer in the binary, so it renders vertex-black on a
/// phase-precessed schedule (`÷1.7` of the day counter) and never reads as a second moon (decision
/// 0485). Its body lands with the follow-up-pinned tables.
pub(super) fn moon_direction(minute: f32) -> Vec3 {
    // Elevation LUT (dayProgression, polar φ rad): 35° = π·0.19444, 100° = π·0.55556.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.000000, 0.6108652), // 35°  — midnight (overhead, +55°)
        (0.003472, 0.6108652), // 35°
        (0.166667, 1.7453293), // 100° — 04:00 (sets below the horizon, −10°)
        (0.916667, 1.7453293), // 100° — 22:00 (still below the horizon)
        (0.996528, 0.6108652), // 35°  — risen again before midnight
    ];
    let dp = minute / 1440.0;
    let phi = interp_daynight(&ELEV_TABLE, dp);
    let theta = std::f32::consts::FRAC_PI_4; // 45°, constant — the white moon shares the sun's bearing
    let (sp, cp) = (phi.sin(), phi.cos());
    // to-moon = (cosθ·sinφ, sinθ·sinφ, cosφ) in WoW world (Z up), then into Bevy (pure rotation).
    let wow_to_moon = [sp * theta.cos(), sp * theta.sin(), cp];
    wow_to_bevy(wow_to_moon).normalize()
}

/// Visible **moon02** direction + size scale (camera→body Bevy direction, size-curve multiplier) —
/// the engine's third disc (`moon02.blp`), drawn every frame but **vertex-black** (its colour field
/// `[0xce98a4]` has no writer in the binary), so it never reads as a second moon (decision 0485).
/// Its tracks run on the **phase-precessed clock**: `phase = fmod(dayCounter + todPhase, 1.7)`
/// (`0x6d41b9`, 65536 fixed-point; `day_continuous` = that sum, server-synced), which the track
/// kernel `0x6cf6c0` **clamps to [0,1]** — so for the whole `r∈[1.0,1.7)` leg both tracks park on
/// their wrap value (az 135°, φ 35° = elevation +55°, frozen high). Azimuth table `0xce8ccc`
/// sweeps 135°→150°→165°; elevation table `0xce8ce4` is the white moon's curve shape (φ 35°↔100°);
/// the size samples the shared `0xce8c8c` curve on the same phase, base ×1.0.
pub(super) fn moon02_state(day_continuous: f64) -> (Vec3, f32) {
    // (dayFraction, azimuth rad): 135° = π·0.75 [0x8012cc], 150° = π·0.8333 [0x810018],
    // 165° = π·0.9167 [0x811644]; key times 04:00 / 22:00 like the elevation track.
    const AZ_TABLE: [(f32, f32); 3] = [
        (0.000000, 2.3561945), // 135°
        (0.166667, 2.6179938), // 150°
        (0.916667, 2.8797932), // 165°
    ];
    // (dayFraction, polar φ rad) — the white-moon curve shape on moon02's own clock.
    const ELEV_TABLE: [(f32, f32); 5] = [
        (0.000000, 0.6108652), // 35°  — phase 0 (overhead, +55°)
        (0.003472, 0.6108652), // 35°
        (0.166667, 1.7453293), // 100° — below the horizon (clipped away)
        (0.916667, 1.7453293), // 100°
        (0.996528, 0.6108652), // 35°
    ];
    // The kernel clamp: fmod into [0,1.7), then anything past 1.0 evaluates AT 1.0 (0x6cf6c0).
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

/// Dawn/dusk sky-dome warp strength curve `S(dayfrac)` — the day/night intensity that gates the
/// azimuthal sky warp (`FUN_006d0f50`, table `0xce9b2c`, decoded off writer `FUN_006ce120`; VERIFIED).
/// Two triangular spikes — sunrise (~06:29) and sunset (~21:29) — and **0 everywhere else** (all of
/// midday AND deep night). Multiplied by the per-zone `highlightSky` flag to get `WowLighting.sky_warp`.
const SKY_WARP_CURVE: [(f32, f32); 6] = [
    (0.1250, 0.0), // 03:00
    (0.2708, 1.0), // 06:29 — dawn spike
    (0.2917, 0.0), // 07:00
    (0.8542, 0.0), // 20:30
    (0.8958, 1.0), // 21:29 — dusk spike
    (0.9993, 0.0), // 23:59
];

/// Dawn/dusk sky-dome warp strength `S` = `dawn_dusk_curve(dayfrac) × highlight_sky` (raw — the debug
/// toggle gating lives in the caller). 0 across all of midday/night and in `highlightSky=0` zones, so
/// the sky warp is identity except at ~06:30/21:30 in flagged zones. See [`SKY_WARP_CURVE`] + `sky.wgsl`.
pub(super) fn sky_warp(minute: f32, highlight_sky: f32) -> f32 {
    interp_daynight(&SKY_WARP_CURVE, minute / 1440.0) * highlight_sky
}

/// Sun-disc **size-multiplier** curve — vanilla size table `0xce8cac` (VERIFIED off `WoW.exe`),
/// sampled by the same wrap-around interpolator as the other DayNight tables.
/// The disc grows to **2× at the dawn/dusk horizon** (06:00 / 21:00) and is **1× across midday** — the
/// baked "huge sun at the horizon". The sun's per-body base multiplier is ×1.0, so this curve *is* its
/// disc scale. (The night-side 2× plateau is never seen — the sun is parked −10° below the horizon then.)
const SUN_SIZE_CURVE: [(f32, f32); 4] = [
    (0.25000, 2.0), // 06:00 — sunrise at the horizon (largest)
    (0.28125, 1.0), // 06:45 — risen → the midday plateau
    (0.84375, 1.0), // 20:15 — still small
    (0.87500, 2.0), // 21:00 — sunset at the horizon (largest)
];

/// The sun disc's size multiplier at `minute` (game minute-of-day): `1.0` midday → `2.0` at the dawn/dusk
/// horizon. `sun.rs` multiplies the disc's base angular size (`sun_size`) by this. See [`SUN_SIZE_CURVE`].
pub(super) fn sun_disc_scale(minute: f32) -> f32 {
    interp_daynight(&SUN_SIZE_CURVE, minute / 1440.0)
}

/// Moon-disc **size-multiplier** curve — shared size table `0xce8c8c` for BOTH moon discs (VERIFIED,
/// decision 0485). `1.5×` at moonrise/moonset (the horizon, ~22:00 / ~04:00) shrinking to `1.0×` overhead
/// (~midnight) — the same horizon-enlargement as the sun. Multiply by the per-disc base (white ×1.75,
/// moon02 ×1.0); moon02's phase precession shifts *when* it samples this, not the range.
const MOON_SIZE_CURVE: [(f32, f32); 4] = [
    (0.041667, 1.0), // 01:00 — overhead (smallest)
    (0.166667, 1.5), // 04:00 — moonset horizon
    (0.916667, 1.5), // 22:00 — moonrise horizon
    (0.999306, 1.0), // 23:59 — wraps toward 01:00 (overhead)
];

/// The moon disc's size multiplier at `minute` — `1.0` overhead (~midnight) → `1.5` at moonrise/set. Both
/// moon discs share the curve; multiply by the per-disc base (white ×1.75, moon02 ×1.0). See [`MOON_SIZE_CURVE`].
pub(super) fn moon_disc_scale(minute: f32) -> f32 {
    interp_daynight(&MOON_SIZE_CURVE, minute / 1440.0)
}

/// Star-field **global-alpha** curve — vanilla star curve `0xce9a98` (VERIFIED off `WoW.exe`):
/// the `Stars.m2` model-global alpha (`[ce96d8+0xb]/255`) the reference fades on
/// this exact schedule. Keyframes (game-day fraction → 0..1): `00:00→1.0, 03:00→1.0, 04:30→0.0, 22:30→0.0`.
/// The wrap-around interpolator gives the rest: stars **fade IN 22:30→00:00**, full all deep night, **fade
/// OUT 03:00→04:30**, and **0 all day** (the reference skips the draw when the byte < 2). The day/night
/// colours already darken the sky; this is the separate star layer on top.
const STAR_CURVE: [(f32, f32); 4] = [
    (0.0, 1.0),    // 00:00 — full (deep night)
    (0.125, 1.0),  // 03:00 — still full
    (0.1875, 0.0), // 04:30 — fully out
    (0.9375, 0.0), // 22:30 — start fading in (wrap → 00:00)
];

/// Star-field global alpha at `minute` (game minute-of-day): `1.0` deep night → `0.0` all day, fading in
/// 22:30→00:00 and out 03:00→04:30. `sun.rs` multiplies the star dome's base-colour alpha by this (the
/// model-global fade the reference applies). See [`STAR_CURVE`].
pub(super) fn star_alpha(minute: f32) -> f32 {
    interp_daynight(&STAR_CURVE, minute / 1440.0)
}

/// Sun lens-flare **day/night envelope** curve — the per-body inline 4-key dnCurve table at
/// `[glare+0x70]` (sun `0xce9818`, built by `0x6d1e30`; decision 0508). A factor of the
/// flare-intensity slew target in `sun::follow`: the sun's
/// flare exists only by day — 0 until 06:30, full 07:30→19:30, gone by 21:00. Dusk (21:00→22:45)
/// and dawn (03:15→06:30) are flare dead-bands for BOTH bodies.
const SUN_FLARE_DN_CURVE: [(f32, f32); 4] = [
    (0.2708333, 0.0), // 06:30 — still off
    (0.3125, 1.0),    // 07:30 — full day flare
    (0.8125, 1.0),    // 19:30 — still full
    (0.875, 0.0),     // 21:00 — off before the sun sets
];

/// The sun's flare dnCurve at `minute` (game minute-of-day). See [`SUN_FLARE_DN_CURVE`].
pub(super) fn sun_flare_dn(minute: f32) -> f32 {
    interp_daynight(&SUN_FLARE_DN_CURVE, minute / 1440.0)
}

/// Moon lens-flare **night envelope** curve (moon dnCurve table `0xce9768`, built by `0x6d1f30`;
/// decision 0508): flat **zero from 03:15 to 22:45** — the whole day *and*
/// early evening — ramping in 22:45→24:00 (23:00 ≈ 0.20, 23:30 ≈ 0.61), full 00:00→02:00, out by
/// 03:15. A 22:30 moonrise has NO halo in the reference; it first lights at 22:45 and peaks near
/// midnight. This was the missing gate behind the director's "moon halo far too big" report.
const MOON_FLARE_DN_CURVE: [(f32, f32); 4] = [
    (0.0833333, 1.0), // 02:00 — still full
    (0.1354167, 0.0), // 03:15 — out
    (0.9479167, 0.0), // 22:45 — first light
    (0.999306, 1.0),  // ~23:59 — full (the wrap to 02:00 holds 1.0 across midnight)
];

/// The moon's flare dnCurve at `minute` (game minute-of-day). See [`MOON_FLARE_DN_CURVE`].
pub(super) fn moon_flare_dn(minute: f32) -> f32 {
    interp_daynight(&MOON_FLARE_DN_CURVE, minute / 1440.0)
}

/// The **SIDN night fraction** curve — the runtime-built 4-key track at `0xce9a34` (keys + the
/// constants `0x811518`/`0x8115bc`, builder `0x6ce670`). Sampled per frame into `DNState+0x1ac`;
/// the WMO material updater `0x6b4090` multiplies every
/// SIDN material's authored emissive colour by it — windows glow **1.0 all night (21:30→06:00)**,
/// ramp OFF over 06:00→07:00, stay **0 through the day**, and ramp ON over 20:30→21:30.
const SIDN_NIGHT_CURVE: [(f32, f32); 4] = [
    (0.25, 1.0),      // 06:00 — still full night glow
    (0.2916667, 0.0), // 07:00 — faded out for the day
    (0.8541667, 0.0), // 20:30 — starts ramping in
    (0.8958333, 1.0), // 21:30 — full glow (wrap → 06:00 holds 1.0)
];

/// The SIDN self-illum night fraction at `minute` (game minute-of-day): `1.0` overnight, `0.0` all
/// day, linear ramps 20:30→21:30 and 06:00→07:00. Multiplied into every WMO SIDN material's authored
/// emissive colour (`wow_model.wgsl`, via the shared light buffer). See [`SIDN_NIGHT_CURVE`].
pub(super) fn sidn_night_fraction(minute: f32) -> f32 {
    interp_daynight(&SIDN_NIGHT_CURVE, minute / 1440.0)
}

/// The **cloud sun-glow envelope** — the INTERNAL static 8-key track at `0xce9ab8` (built once by
/// `0x6ce390` from constants; NOT a Light.dbc band), evaluated by the shared track kernel at the
/// day fraction. ≈1.0 across the full day, notching to 0 at the twilight boundaries. The key
/// array is stored NON-MONOTONIC (keys 6/7 sit before key 5 in time) — byte-verified as-is; the
/// array-order scan makes them structurally unreachable and wraps `t > 0.9236` back to 1.0. We
/// keep the keys in the stored order so [`interp_daynight`] (the same kernel) reproduces exactly
/// that behavior (`0x6cf6c0`) — whether the non-monotonic layout is intentional or a data quirk
/// is inferred, not settled from the binary.
pub(super) fn cloud_glow_track(minute: f32) -> f32 {
    const CLOUD_GLOW_CURVE: [(f32, f32); 8] = [
        (0.16667, 1.0), // 04:00 — full
        (0.19444, 0.0), // 04:40 — dawn notch
        (0.20139, 0.0), // 04:50
        (0.22917, 1.0), // 05:30 — full through the day
        (0.89583, 1.0), // 21:30
        (0.92361, 0.0), // 22:10 — dusk notch
        (0.88889, 0.0), // (structurally unreachable — stored order kept, see above)
        (0.91667, 1.0), // (ditto; the seam wraps >22:10 back toward 1.0)
    ];
    interp_daynight(&CLOUD_GLOW_CURVE, minute / 1440.0)
}

/// The cloud glow's **body pick** (`0x6cfb00` setup): the SUN drives the glow while the day
/// fraction sits in `[0.2013889, 0.9236111]` (≈ 04:50–22:10), the MOON otherwise.
pub(super) fn cloud_glow_is_sun(minute: f32) -> bool {
    let dp = minute / 1440.0;
    (0.201_388_9..=0.923_611_1).contains(&dp)
}

/// Vanilla `DayNight::InterpTable`: wrap-around linear interpolation of a `(dayProgression, value)`
/// table over `dp ∈ [0,1)`. (The client's two return branches both reduce to a plain lerp.)
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

    /// The cloud glow envelope (`0x6cf6c0`): full across the day, zero at the twilight
    /// notches, and the seam wrap past 22:10 back to 1.0 (the non-monotonic stored
    /// keys are unreachable under the array-order scan).
    #[test]
    fn cloud_glow_track_matches_the_client_envelope() {
        assert_eq!(cloud_glow_track(720.0), 1.0); // noon
        assert_eq!(cloud_glow_track(0.20139 * 1440.0), 0.0); // the 04:50 notch
        let notch = cloud_glow_track(0.9236 * 1440.0); // approaching the 22:10 notch from below
        assert!(notch < 0.01, "dusk notch {notch}");
        // The cliff: one step past the notch the array-order scan runs off the non-monotonic
        // tail and the seam wrap snaps the envelope back to 1.0 (the verified mechanism).
        assert_eq!(cloud_glow_track(0.9237 * 1440.0), 1.0);
        assert_eq!(cloud_glow_track(0.95 * 1440.0), 1.0); // deep night stays wrapped
        let mid_dusk = cloud_glow_track(0.9097 * 1440.0); // halfway 21:30→22:10
        assert!((mid_dusk - 0.5).abs() < 0.01, "mid-dusk {mid_dusk}");
        // The body pick: sun by day, moon outside the window.
        assert!(cloud_glow_is_sun(720.0));
        assert!(!cloud_glow_is_sun(0.95 * 1440.0));
    }

    // Vanilla DayNight phi keyframes (wowdev.wiki Rendering/DayNight) — pinned so a refactor can't
    // silently change the sun's elevation curve.
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
                                                                        // Halfway between keyframes is the midpoint value.
        assert!((interp_daynight(&PHI, 0.125) - 2.0682153).abs() < 1e-4);
    }

    // The SIDN night-glow schedule (track `0xce9a34`):
    // 1.0 overnight, 0.0 all day, linear ramps 20:30→21:30 and 06:00→07:00 — pinned so a curve
    // edit can't silently shift when windows light up.
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
        // Bevy +Y is up; the sun's *travel* direction must point down (y < 0) at every time of day,
        // i.e. the sun never sets (night comes from the colors, not the geometry).
        for minute in (0..1440).step_by(30) {
            assert!(
                sun_direction(minute as f32).y < 0.0,
                "sun below horizon at minute {minute}"
            );
        }
    }

    #[test]
    fn sun_azimuth_is_fixed_all_day() {
        // theta is constant in vanilla, so the horizontal heading must not change over the day.
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
        // Unlike the lighting sun, the VISIBLE sun sweeps a big elevation arc (`FUN_006d3b80`):
        // below the horizon at night, near zenith at noon. Keyframes from agent A's binary tables.
        let noon = elev_deg(celestial_sun_direction(720.0)); // φ 5°  → +85°
        let dawn = elev_deg(celestial_sun_direction(0.0)); // φ 100° → −10°
        let morning = elev_deg(celestial_sun_direction(480.0)); // 08:00, φ 63° → +27°
        assert!((noon - 85.0).abs() < 1.0, "solar noon ~+85°, got {noon}");
        assert!((dawn + 10.0).abs() < 1.0, "midnight ~−10°, got {dawn}");
        assert!((morning - 27.0).abs() < 2.0, "08:00 ~+27°, got {morning}");
        // It genuinely climbs from morning to noon (the lighting sun does NOT — it only bobs ±8°).
        assert!(noon > morning + 30.0, "should climb steeply toward noon");
        assert!(noon > dawn + 80.0, "midnight far below noon");
    }

    #[test]
    fn celestial_sun_dips_below_horizon_at_night() {
        // Below the horizon (y < 0) before dawn and after dusk; above it through the day.
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
        // Both suns sit at to-sun azimuth 45° — the disc tracks the lit direction, differing only in
        // elevation. Compare horizontal headings (atan2(x,z)) of celestial to-sun vs lighting to-sun.
        let heading = |d: Vec3| d.x.atan2(d.z);
        let light_to_sun = -sun_direction(600.0); // lighting to-sun
        let celes = celestial_sun_direction(600.0);
        let dh = (heading(celes) - heading(light_to_sun)).abs();
        assert!(
            dh < 1e-3,
            "celestial & lighting suns share a bearing, Δ={dh}"
        );
        // And the celestial sun is HIGHER than the lighting sun at 10:00 (56° vs 31°).
        assert!(
            elev_deg(celes) > elev_deg(light_to_sun) + 15.0,
            "visible sun should ride higher than the lighting sun"
        );
    }

    #[test]
    fn sky_warp_curve_spikes_at_dawn_dusk_and_is_zero_midday() {
        let s = |min: u32| interp_daynight(&SKY_WARP_CURVE, min as f32 / 1440.0);
        // 0 across all of midday AND deep night — the warp must not touch the daytime/night sky.
        assert_eq!(s(720), 0.0, "noon"); // 12:00
        assert_eq!(s(0), 0.0, "midnight"); // 00:00 (wrap between two 0.0 keyframes)
        assert_eq!(s(1080), 0.0, "18:00");
        // Triangular spikes to ~1.0 at sunrise (06:30) and sunset (21:30).
        assert!(s(390) > 0.99, "dawn spike ~1.0, got {}", s(390));
        assert!(s(1290) > 0.99, "dusk spike ~1.0, got {}", s(1290));
        // Partway up the dawn ramp (06:00, between the 03:00=0 and 06:30=1 keyframes) is in (0,1).
        let mid = s(360);
        assert!(mid > 0.0 && mid < 1.0, "dawn ramp partial, got {mid}");
    }

    #[test]
    fn sun_disc_doubles_at_the_horizon_and_is_unit_at_midday() {
        // VERIFIED size table 0xce8cac: 2× at sunrise (06:00) / sunset (21:00), 1× across midday.
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
        // Mid-ramp just after sunrise (06:20, between 06:00→2.0 and 06:45→1.0) is in (1,2).
        let r = sun_disc_scale((6 * 60 + 20) as f32);
        assert!(r > 1.0 && r < 2.0, "post-sunrise shrink ramp, got {r}");
    }

    #[test]
    fn stars_fade_in_at_night_and_are_off_by_day() {
        // VERIFIED star curve 0xce9a98: full deep night, off all day, fade in 22:30→00:00 / out 03:00→04:30.
        assert!((star_alpha((0) as f32) - 1.0).abs() < 1e-3, "00:00 full"); // midnight
        assert!(
            (star_alpha((2 * 60) as f32) - 1.0).abs() < 1e-3,
            "02:00 full"
        );
        assert_eq!(star_alpha((12 * 60) as f32), 0.0, "noon off"); // 12:00
        assert_eq!(star_alpha((18 * 60) as f32), 0.0, "18:00 off");
        // Fading in across 22:30→00:00: 23:15 is partway up (0,1).
        let dusk = star_alpha((23 * 60 + 15) as f32);
        assert!(dusk > 0.0 && dusk < 1.0, "fading in at 23:15, got {dusk}");
        // Fading out across 03:00→04:30: 03:45 is partway down (0,1).
        let dawn = star_alpha((3 * 60 + 45) as f32);
        assert!(dawn > 0.0 && dawn < 1.0, "fading out at 03:45, got {dawn}");
    }

    #[test]
    fn flare_dn_curves_gate_the_halos_by_time_of_day() {
        // VERIFIED per-body dnCurve tables (sun 0xce9818 / moon 0xce9768, 0508). The
        // load-bearing value: a 22:30 moonrise has NO halo — the moon's curve is flat zero from
        // 03:15 all the way to 22:45, then ramps in toward midnight (23:00 ≈ 0.20, 23:30 ≈ 0.61).
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
        // The sun's is the day mirror: full across midday, gone by 21:00, off all night.
        assert!((sun_flare_dn((12 * 60) as f32) - 1.0).abs() < 1e-3, "noon");
        assert_eq!(sun_flare_dn((21 * 60) as f32), 0.0, "21:00 off");
        assert_eq!(sun_flare_dn((23 * 60) as f32), 0.0, "night off");
        // 390/1440 isn't f32-representable, so the key lookup sits an epsilon up the ramp.
        assert!(sun_flare_dn((6 * 60 + 30) as f32) < 1e-3, "06:30 still off");
    }

    #[test]
    fn moon_disc_enlarges_at_moonrise_and_set() {
        // VERIFIED shared size table 0xce8c8c: 1.5× at the horizon (~22:00 rise / ~04:00 set), 1× overhead.
        let m = |min: u32| interp_daynight(&MOON_SIZE_CURVE, min as f32 / 1440.0);
        assert!((m(22 * 60) - 1.5).abs() < 1e-3, "22:00 moonrise = 1.5×");
        assert!((m(4 * 60) - 1.5).abs() < 1e-3, "04:00 moonset = 1.5×");
        assert!((m(60) - 1.0).abs() < 1e-3, "01:00 overhead = 1×");
    }

    // moon02's phase-precessed clock (0x6d41b9 fmod ÷1.7 + the 0x6cf6c0 kernel clamp; decision
    // 0485): a normal-leg "day" behaves like the white moon on its own bearing, and the whole
    // r∈[1.0,1.7) leg parks on the clamp value (az 135°, elevation +55°).
    #[test]
    fn moon02_precesses_and_parks_on_the_clamp_leg() {
        // Phase 0 (day 0 midnight): up (+55°), azimuth at the 135° key.
        let (dir, scale) = moon02_state(0.0);
        assert!(dir.y > 0.5, "phase 0: up");
        assert!((scale - 1.0).abs() < 1e-3, "phase 0: overhead size 1×");
        // Phase 0.5 (day 0 noon): below the horizon (clipped away by the shader).
        assert!(moon02_state(0.5).0.y < 0.0, "phase 0.5: below horizon");
        // The clamp leg: any remainder in [1.0, 1.7) evaluates AT 1.0 — parked up, identical state.
        let (park_a, sa) = moon02_state(1.05);
        let (park_b, sb) = moon02_state(1.65);
        assert!(park_a.y > 0.5, "clamp leg: parked above the horizon");
        assert!(
            (park_a - park_b).length() < 1e-6 && (sa - sb).abs() < 1e-6,
            "clamp leg: frozen"
        );
        // Precession: the same time-of-day on consecutive days lands on different phases —
        // day 10 midnight (phase fmod(10,1.7)≈1.5 → parked up) vs day 11 (fmod(11,1.7)≈0.8 → the
        // below-horizon stretch of the normal leg).
        assert!(moon02_state(10.0).0.y > 0.5, "day 10 midnight: parked leg");
        assert!(
            moon02_state(11.0).0.y < 0.0,
            "day 11 midnight: below horizon"
        );
        // And moon02's bearing is its own (az 135–165°), never the white moon's 45°.
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
        // The single (white) moon: overhead at midnight (elev +55° → Bevy y>0), below the horizon at noon
        // (−10° → y<0).
        assert!(moon_direction(0.0).y > 0.5, "moon up at midnight");
        assert!(
            moon_direction((12 * 60) as f32).y < 0.0,
            "moon below horizon at noon"
        );
    }
}
