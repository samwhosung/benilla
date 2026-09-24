//! The per-frame time-of-day RESOLVE: samples `Light.dbc` against the effective game clock (server
//! clock, manual debug-panel scrub, or the pre-connect noon fallback) into a fresh [`super::WowLighting`]
//! — the scene-fog stage, the camera-in-WMO interior-fog crossfade ([`WmoCrossfade`]), and the startup
//! seed ([`setup_lighting`]) all live here. Split from the resource type + plugin registration in
//! `super`.

use bevy::prelude::*;

use crate::dev_state::DebugState;
use crate::terrain_stream::SPAWN_XY;
use crate::view::WorldCamera;
use crate::world_map::CurrentMap;
use benilla_assets::coords::bevy_to_wow;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_formats::{Atmosphere, LightCatalog};

use super::{daynight, quantize_glow, ClockSource, GameClock, LightSampler, WowLighting};

/// The scene-fog stage (`dn_scene_fog 0x6cee30`): resolve the
/// **pushed** `(start, end)` fog pair from the blended bands. `end = min(zone_end, farclip)` — the
/// zone ends in yards (Elwynn 500 clear / 278 storm, decision 0324) exceed only a lowered farclip,
/// so the wall sits at the zone value and pulls in when the view-distance slider drops below it.
/// `start = frac × end`, **unclamped** — the storm fraction is negative (Elwynn −0.5), flooring the
/// whole near field at `1 − 1/(1−frac)` ≈ 33% fog *at the eye*: the constant storm veil.
fn scene_fog(fog_end_raw: f32, fog_start_frac: f32, farclip: f32) -> (f32, f32) {
    let end = fog_end_raw.min(farclip);
    (fog_start_frac * end, end)
}

/// The camera-in-WMO fog crossfade rate — `[0x8115b0] = 0.25`/s, a **4-second** fade both in and
/// out (`0x6cefcb–0x6cf051`).
const WMO_FOG_RAMP_PER_SEC: f32 = 0.25;

/// The camera-in-WMO **interior crossfade** — the reference's `[0xce9bdc]` (+ the staging
/// `0x6cef43–62`), ONE number with exactly two consumers: while the camera stands in a WMO
/// interior, the scene fog — storm veil included — crossfades toward the building's own MFOG fog
/// over 4 s and back out over 4 s on leaving, **and the same `t` is the WMO skybox's slot alpha**
/// ("the skybox alpha and the interior fog blend are the same number"; `crate::skybox` reads it
/// through [`Self::t`], never a ramp of its own). This is why
/// the reference's Goldshire inn keeps its warm authored haze while a storm rages outside (the
/// storm's negative-start veil never reaches the room), and why Stratholme's painted sky fades in
/// at exactly the rate its streets shed the scene fog. The staged triple LATCHES while the camera
/// exits so the fade-out lerps from the room's fog instead of popping. A mid-interior fog-record
/// switch (tavern → corridor) snaps the staged target — the client's source-identity guard
/// (`0x6cef92–b5`) re-arms a lerp there whose exact semantics are un-carved (flagged in 0338); at
/// room scales both records' fog is ≈0 so the step is invisible.
#[derive(Resource, Default)]
pub struct WmoCrossfade {
    t: f32,
    staged: Option<crate::wmo_portal::WmoFogTarget>,
}

impl WmoCrossfade {
    /// The crossfade weight this frame, `[0, 1]` — advanced only by [`update_time_lighting`]'s
    /// blend, read by the skybox weight resolve.
    pub fn t(&self) -> f32 {
        self.t
    }
    /// Advance the ramp by `dt` toward `target` and return the committed `(color, start, end)`.
    /// The staging transform is the record's own law: `end = min(end, farclip)`,
    /// `start = end × start_scalar` (`fmul 0x6cef56` — MFOG start IS a fraction of end in 1.12.1).
    fn blend(
        &mut self,
        target: Option<crate::wmo_portal::WmoFogTarget>,
        scene_color: [f32; 3],
        scene_start: f32,
        scene_end: f32,
        farclip: f32,
        dt: f32,
    ) -> ([f32; 3], f32, f32) {
        if target.is_some() {
            self.staged = target;
        }
        let dir = if target.is_some() { 1.0 } else { -1.0 };
        self.t = (self.t + dir * WMO_FOG_RAMP_PER_SEC * dt).clamp(0.0, 1.0);
        let Some(s) = self.staged else {
            return (scene_color, scene_start, scene_end);
        };
        if self.t <= 0.0 {
            self.staged = None;
            return (scene_color, scene_start, scene_end);
        }
        let end = s.end.min(farclip);
        let start = end * s.start_scalar;
        let k = self.t;
        let mut color = scene_color;
        for (c, w) in color.iter_mut().zip(s.color) {
            *c += (w - *c) * k;
        }
        (
            color,
            scene_start + (start - scene_start) * k,
            scene_end + (end - scene_end) * k,
        )
    }
}

/// Startup: load `Light.dbc` and keep it resident as the [`LightSampler`], then seed [`WowLighting`]
/// with the spawn-zone noon values (colors + the vanilla sun direction) so frame 0 has sane values;
/// `update_time_lighting` takes over from frame 1. **Phase 0:** no PBR sun / shadows / fog are spawned
/// — the faithful lighting is rebuilt in-shader. Absent client data ⇒ nothing seeded; `WowLighting`
/// stays its default.
pub(super) fn setup_lighting(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    view: Res<crate::view::ViewDistance>,
) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    // Keep the catalog resident: `update_time_lighting` re-samples it each frame against the server
    // clock. The noon seed below only fills `WowLighting` until the first frame runs.
    let catalog = match LightCatalog::load(&mut chain) {
        Ok(cat) => Some(cat),
        Err(e) => {
            warn!("Light.dbc unavailable, using neutral daytime lighting: {e:#}");
            None
        }
    };
    // The faithful path is `sample_blended` (q2 RE: "rebuild must default to it, NOT `pick_light`").
    // `sample_noon` falls through to `sample` which uses `pick_light` — single-sphere approximation.
    // Replicate `sample_noon`'s "DAY/2, false" (noon, clear) but on the blended path. Moot at
    // SPAWN_XY (no local sphere covers it, α=0 → blended ≡ global) but right anywhere spheres overlap.
    let atmo = catalog
        .as_ref()
        .map(|c| {
            c.sample_blended(
                0,
                [SPAWN_XY.0, SPAWN_XY.1, 83.5],
                1440,
                false,
                benilla_formats::Submersion::Dry,
                false,
            )
        })
        .unwrap_or(Atmosphere::DEFAULT);
    if let Some(cat) = catalog {
        commands.insert_resource(LightSampler(cat));
    }
    info!(
        "Elwynn noon lighting from Light.dbc: sun diffuse {:?}, ambient {:?}",
        atmo.sun_diffuse, atmo.ambient
    );
    let (fog_start, fog_end) = scene_fog(atmo.fog_end, atmo.fog_start_frac, view.farclip);
    // The shaders do the WoW lighting themselves, so they need the DBC colors + the sun's world-space
    // travel direction (the faithful `DayNight::SetDirection` port; noon here, re-derived each frame).
    commands.insert_resource(WowLighting {
        ambient: atmo.ambient,
        diffuse: atmo.sun_diffuse,
        spec: atmo.sun_color,
        sun_dir: daynight::sun_direction(720.0),
        celestial_dir: daynight::celestial_sun_direction(720.0),
        fog_color: atmo.fog_color,
        fog_start,
        fog_end,
        // Interior fog seeds equal to the scene (camera starts outdoors, ramp t = 0).
        wmo_fog_color: atmo.fog_color,
        wmo_fog_start: fog_start,
        wmo_fog_end: fog_end,
        sky: atmo.sky,
        water_river: atmo.water_river,
        water_ocean: atmo.water_ocean,
        water_river_alpha: atmo.water_river_alpha,
        water_ocean_alpha: atmo.water_ocean_alpha,
        glow: quantize_glow(atmo.glow),
        // Noon → the dawn/dusk curve is 0, so no sky warp at startup regardless of zone.
        sky_warp: daynight::sky_warp(720.0, atmo.highlight_sky),
        sun_disc_scale: daynight::sun_disc_scale(720.0), // noon → 1.0 (the sun grows toward the horizon)
        sun_flare_dn: daynight::sun_flare_dn(720.0),     // noon → 1.0 (full day flare)
        moon_dir_white: daynight::moon_direction(720.0),
        moon_disc_scale: daynight::moon_disc_scale(720.0),
        moon_flare_dn: daynight::moon_flare_dn(720.0), // noon → 0.0 (night-only halo)
        moon_dir_02: daynight::moon02_state(0.5).0, // day 0 noon (below the horizon, like the white moon)
        moon02_disc_scale: daynight::moon02_state(0.5).1,
        star_alpha: daynight::star_alpha(720.0), // noon → 0 (stars off by day)
        sidn_night: daynight::sidn_night_fraction(720.0), // noon → 0 (no window glow by day)
        celestial_tint: atmo.sun_color,
        cloud_density: atmo.cloud_density,
        cloud_colors: atmo.cloud_colors,
        storm_bcc: 0.0, // no weather at startup
        cloud_glow_dir: daynight::celestial_sun_direction(720.0), // noon → the sun
        cloud_glow_track: daynight::cloud_glow_track(720.0),
    });
}

/// Drive **all** scene lighting from the time of day. The effective minute comes from the **server**
/// game-clock (`SMSG_LOGIN_SETTIMESPEED`, advanced by its timescale) — or, via the debug panel, a
/// manual scrub or the pre-connect noon fallback. We sample
/// `Light.dbc` at that time for the ambient/diffuse/specular colors and set the sun *direction* from
/// vanilla's `DayNight::SetDirection` (fixed azimuth, slight elevation wobble, **always above the
/// horizon** — see [`daynight::sun_direction`]). Night is dark because the DBC colors go dark, NOT because the
/// sun sets (that's how the real client does it). **Phase 0:** this only writes the resolved values
/// into [`WowLighting`] (consumed by `apply_wow_lighting` → the materials). No PBR sun / ambient /
/// fog / sky are touched — those are rebuilt in-shader step by step.
/// The reference's `0x6d1620` — an HSV **VALUE-only** scale of a colour, as used by the ocean depth
/// ramp: unpack to RGB floats, `RGB → HSV` (`0x7bbc80`), multiply the third member (V) alone,
/// `HSV → RGB` (`0x7bbd60`), repack.
///
/// **It is a uniform RGB multiply, and that is a fact rather than an approximation.** HSV keeps
/// `V = max(r,g,b)` and `S = (max − min)/max`; scaling V while H and S are untouched scales the max
/// by `f`, and the min by `new_V·(1 − S) = f·min`, and the middle channel by the same ratio — so
/// every channel comes back multiplied by `f`. The detour is an identity for a V-only scale, so we
/// transcribe the identity. `the_hsv_detour_is_a_plain_multiply` checks that against an actual
/// round trip rather than leaving it as an argument in a comment.
///
/// The one thing we do NOT carry over is the reference's byte repack (`×255 + 512 >> 14`): its
/// light lane is 8-bit and ours is float end to end, and re-quantizing here would be inventing a
/// step this pipeline does not otherwise have.
fn hsv_value_scale(c: [f32; 3], f: f32) -> [f32; 3] {
    [c[0] * f, c[1] * f, c[2] * f]
}

pub(super) fn update_time_lighting(
    world_time: Res<super::WorldTime>,
    sampler: Option<Res<LightSampler>>,
    debug: Res<DebugState>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    current_map: Option<Res<CurrentMap>>,
    eye_liquid: crate::liquid::EyeLiquid,
    viewer: Res<crate::view::Viewer>,
    weather: Option<Res<crate::weather::WeatherState>>,
    view: Res<crate::view::ViewDistance>,
    time: Res<Time>,
    wmo_fog: Res<crate::wmo_portal::CameraWmoFog>,
    mut wmo_ramp: ResMut<WmoCrossfade>,
    mut clock: ResMut<GameClock>,
    mut lighting: ResMut<WowLighting>,
    mut last_dump: Local<Option<u32>>,
    mut last_fog_dump: Local<Option<u32>>,
) {
    // Effective time-of-day and where it came from. `minute` (whole) drives the Light.dbc colour sample,
    // the clock display, and the log; `minute_f` (FRACTIONAL) drives the celestial body *positions* +
    // size/star curves so they move continuously — sampling at the truncated whole minute steps them once
    // per game-minute, a visible jump at a fast timescale (the moon "jumping" on each update).
    let (minute, minute_f, source) = if debug.lighting.follow_server_time {
        if world_time.live {
            (world_time.minute, world_time.minute_f, ClockSource::Server)
        } else {
            (
                world_time.minute,
                world_time.minute_f,
                ClockSource::Fallback,
            )
        }
    } else {
        let m = debug.lighting.manual_minute.min(1439);
        (m, m as f32, ClockSource::Manual)
    };
    // The continuous day count for moon02's phase clock (days + day-fraction, unwrapped).
    let day_f: f64 = match debug.lighting.follow_server_time {
        true => world_time.day,
        false => debug.lighting.manual_minute.min(1439) as f64 / 1440.0,
    };
    clock.minute = minute;
    clock.source = source;

    // Sample the area-light blend at the CAMERA EYE, resampled every frame — VERIFIED: the real
    // client feeds the render camera's world eye into the `Light.dbc` sphere-containment blend
    // (`dn_light_select 0x6d2d00` samples `DNState+0x18`, stamped each frame from the active
    // `CCamera`'s eye in `WorldFrame::Render 0x482ea0`).
    // It is the camera, NOT the character: in third-person the eye is offset from the avatar by the
    // orbit/zoom/pitch, and that offset is intended (sampling `player.pos`, as we used to, was wrong).
    // No bucketing and no temporal lerp — smoothness is purely spatial (the `blendAlpha` falloff
    // ramps); the per-material GPU push stays cheap because `apply_wow_lighting` only re-pushes when
    // the resolved values change. The spawn swatch is the fallback only before the camera exists.
    let wow_pos = match cam.single() {
        Ok(t) => bevy_to_wow(t.translation()),
        Err(_) => [SPAWN_XY.0, SPAWN_XY.1, 83.5],
    };
    // The faithful area-light blend: seed from the continent global light and α-lerp toward every
    // overlapping local sphere (`calculateLightParamBlends`/`getLightResultsFromDB`, q2) — NOT the
    // single-sphere `pick_light` approximation. Sampled at the true `wow_pos` every frame; neutral
    // daytime fallback if the DBCs are missing. The map id MUST track the current continent — Light.dbc
    // spheres are filtered by `continent`, so resolving on the wrong map drops every local sphere and
    // falls back to the *other* continent's global default (e.g. Kalimdor → Eastern-Kingdoms warm fog).
    let map = current_map.as_ref().map_or(0, |m| m.0);
    // When the camera is submerged, sample the UNDERWATER Light param (slot 1) instead — dense teal
    // fog + cool light. Feeding it through the normal atmosphere → `WowLighting` path means every
    // existing consumer (terrain/model/water/WDL fog + the clear colour) becomes underwater for free
    // (VERIFIED apitrace WoW.18: the reference just switches the active param; no overlay quad).
    let submerged = eye_liquid.submersion();
    // The ghost-world atmosphere (decision 0308 §7): while PLAYER_FLAGS_GHOST is up the active
    // LightParams slot is 4 — the death profile (`[0xce9bb0]`) — applied instantly (the client
    // rebuilds its color tables per frame off the single active slot).
    let ghost = viewer.ghost;
    // The storm light-blend (the weather arc): weather's slow sky-density channel becomes
    // `bcc = min(1, density·4)` (`0x6d4500`), and the STORM LightParams
    // record is lerped over the clear one by that weight — every band at once (ambient, diffuse,
    // sky stops, fog color *and distances*). That one blend is the reference's overcast
    // darkening + fog draw-in. Zones without a storm param fall back to their clear param
    // inside `sample_blended`, making the lerp an identity there.
    let storm = weather
        .as_ref()
        .map_or(0.0, |w| crate::weather::storm_blend(w.sky_density));
    let atmo = sampler
        .as_ref()
        .map(|s| {
            let clear =
                s.0.sample_blended(map, wow_pos, minute * 2, false, submerged, ghost);
            if storm > 0.0 {
                let stormy =
                    s.0.sample_blended(map, wow_pos, minute * 2, true, submerged, ghost);
                clear.lerp(&stormy, storm)
            } else {
                clear
            }
        })
        .unwrap_or(Atmosphere::DEFAULT);
    // Sun direction from vanilla DayNight (fixed azimuth, slight elevation wobble, always up). Sampled
    // continuously (`minute_f`) — the shared global-light buffer makes per-frame light changes O(1)
    // (terrain + models read it directly; no per-material re-push), so the interim whole-minute quantize
    // (the `apply_wow_lighting` FPS stopgap) is no longer needed.
    let sun_dir = daynight::sun_direction(minute_f);
    // The *visible* celestial sun — a separate body that rises and sets in elevation (for the sprite).
    let celestial_dir = daynight::celestial_sun_direction(minute_f);

    // Step 5 fog from the same DBC sample (q6), through the scene-fog stage: `min(zone end, farclip)`
    // + `frac × end` unclamped (`dn_scene_fog 0x6cee30`, rf-weather-fog-veil + decision 0324). Elwynn
    // commits 125/500 clear and −139/278 at full storm — the storm's −0.5 fraction yields the
    // negative start = the near veil; the short storm end whites out the middle distance.
    let (fog_start, fog_end) = scene_fog(atmo.fog_end, atmo.fog_start_frac, view.farclip);
    // The camera-in-WMO interior-fog crossfade (see [`WmoCrossfade`]) — computed as its OWN triple;
    // the scene fog above stays untouched (an earlier global-overwrite washed the outside view
    // golden through the door — director-caught, corrected).
    // Submerged, the submerged Light param owns everything — the MFOG record's own underwater
    // block (`+0x24`, gated record flags 0x100/0x10) is a recorded, deferred lane. Bypassing the ramp
    // rather than blending through it is also what the reference does for the fullbright liquids
    // specifically: submerged in magma or slime *inside* an MFOG interior it **snaps** the fog block
    // instantly (VERIFIED `0x6cef6f-85`) instead of taking the 4 s crossfade — which is the case that
    // matters here, since Undercity's slime is WMO liquid.
    let (wmo_fog_color, wmo_fog_start, wmo_fog_end) = if submerged.any() {
        (atmo.fog_color, fog_start, fog_end)
    } else {
        wmo_ramp.blend(
            wmo_fog.0,
            atmo.fog_color,
            fog_start,
            fog_end,
            view.farclip,
            time.delta_secs(),
        )
    };
    let moon02 = daynight::moon02_state(day_f);
    // **The ocean depth ramp** (decision 1829). Ocean alone runs it, and it darkens the two
    // committed light triples — the first (`DNState+0x178`) by `fac1` down to 0.5 (`0x6d28e0`),
    // the second (`+0x174`) by `fac2` down to 0.75 (`0x6d28fe`), over the first 30 yards.
    // Every other submersion state, ocean's own fog colour included, is untouched: the fog-colour
    // commit in the same block is dead code, overwritten from the bands immediately after.
    let (fac1, fac2) = submerged
        .ocean_depth_factors(eye_liquid.eye_z())
        .unwrap_or((1.0, 1.0));
    let resolved = WowLighting {
        ambient: hsv_value_scale(atmo.ambient, fac1),
        diffuse: hsv_value_scale(atmo.sun_diffuse, fac2),
        spec: atmo.sun_color, // row 9 (warm-white) — WoW's specular color
        sun_dir,
        celestial_dir,
        fog_color: atmo.fog_color,
        fog_start,
        fog_end,
        wmo_fog_color,
        wmo_fog_start,
        wmo_fog_end,
        sky: atmo.sky,
        // The water swatch rides the SAME area blend as every other band — byte-VERIFIED, decision
        // 1104: the gather record's 18 colour slots (rows 14–17 at `+0x34..+0x40`) are all merged by
        // `0x6d30e0`, so there is no single-sphere pick anywhere in the client's
        // water path. Resolving it off `pick_light` made the tint DISCONTINUOUS — it snapped whenever
        // a tighter sphere's outer radius was crossed, i.e. at exactly the point where that sphere's
        // own weight is zero (Tirisfal → Silverpine: green water to near-black brown in one step).
        water_river: atmo.water_river,
        water_ocean: atmo.water_ocean,
        water_river_alpha: atmo.water_river_alpha,
        water_ocean_alpha: atmo.water_ocean_alpha,
        glow: quantize_glow(atmo.glow), // per-zone bloom weight (LightParams.glow)
        // Dawn/dusk sky-dome warp: `S = dawn_dusk_curve(dayfrac) × highlightSky`. 0 across midday and
        // in highlightSky=0 zones, so it only warps the sky at ~06:30/21:30 in highlightSky=1 zones
        // (Elwynn/Westfall, Thousand Needles, …). S-curve binary-VERIFIED faithful. (Open fidelity Q,
        // not a panel knob: our shader may over-apply the warp to the
        // apex/rim vs the binary's 4 mid rings, and the dusk visible result is unconfirmed.)
        sky_warp: daynight::sky_warp(minute_f, atmo.highlight_sky),
        // Sun disc grows to 2× at the dawn/dusk horizon (size table 0xce8cac); 1× midday. Not gated by
        // a toggle — it's a faithful, binary-verified curve (the trace cross-checks the 2× ratio).
        sun_disc_scale: daynight::sun_disc_scale(minute_f),
        // The per-body lens-flare day/night envelopes (dnCurve tables, `0x6cf6c0`, 0508): the
        // sun's flare is a day thing, the moon's halo a deep-night thing; both are 0 at dusk/dawn.
        sun_flare_dn: daynight::sun_flare_dn(minute_f),
        // The white moon (az 45°, the sun's bearing) + the shared moon size curve.
        moon_dir_white: daynight::moon_direction(minute_f),
        moon_disc_scale: daynight::moon_disc_scale(minute_f),
        moon_flare_dn: daynight::moon_flare_dn(minute_f),
        // moon02 rides the server-synced continuous day clock (phase ÷1.7); under the manual
        // scrub there is no day counter, so the scrubbed day-fraction alone feeds the phase.
        moon_dir_02: moon02.0,
        moon02_disc_scale: moon02.1,
        star_alpha: daynight::star_alpha(minute_f),
        sidn_night: daynight::sidn_night_fraction(minute_f),
        // The celestial-body tint (discs + glares; decision 0485) — band sub-9, the same row as spec.
        celestial_tint: atmo.sun_color,
        cloud_density: atmo.cloud_density,
        cloud_colors: atmo.cloud_colors,
        // The same weight that lerped the storm LightParams above — the reference computes bcc
        // once (`0x6d4500`) and every consumer reads the one global.
        storm_bcc: storm,
        // The cloud glow rides the sun by day, the white moon by night (`0x6cfb00` setup).
        cloud_glow_dir: if daynight::cloud_glow_is_sun(minute_f) {
            celestial_dir
        } else {
            daynight::moon_direction(minute_f)
        },
        cloud_glow_track: daynight::cloud_glow_track(minute_f),
    };
    // Write only on a real change so downstream change-detection (sky/glow + the per-material push in
    // `apply_wow_lighting`) still fires only on actual transitions, not every frame — even though we
    // now resample continuously. `ResMut` marks the resource changed only on the DerefMut below.
    if *lighting != resolved {
        *lighting = resolved;
    }
    // `WOW_FOG_DUMP=1`: the interior-fog crossfade live probe, 1 Hz — the committed pair + the
    // ramp t (the 4 s in/out is too fast for the per-game-minute light dump to catch).
    //
    // `WOW_FOG_DUMP=frame` drops the throttle. 1 Hz is fast enough to watch a 4 s ramp travel and far
    // too slow to see the **target** flip: `blend` switches hard between the scene triple and the
    // staged interior one on `staged.is_some()`, so a target that alternates frame to frame alternates
    // the committed fog frame to frame, and a once-a-second sample simply lands on whichever side it
    // lands on. Rows 18/19 are read by WMO materials *alone*, which is exactly the footprint B38 shows
    // (decision 0670: one WMO re-lit, terrain and sky untouched).
    static FOG_DUMP: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    if let Some(mode) = FOG_DUMP.get_or_init(|| std::env::var_os("WOW_FOG_DUMP")) {
        let sec = (mode.as_os_str() != "frame").then(|| time.elapsed_secs() as u32);
        if sec.is_none() || *last_fog_dump != sec {
            *last_fog_dump = sec;
            let b = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as i32);
            eprintln!(
                "[fog] t {:.2} target {:?} scene {:?} {:.0}/{:.0} interior {:?} {:.0}/{:.0}",
                wmo_ramp.t,
                wmo_fog.0,
                b(resolved.fog_color),
                resolved.fog_start,
                resolved.fog_end,
                b(resolved.wmo_fog_color),
                resolved.wmo_fog_start,
                resolved.wmo_fog_end,
            );
        }
    }
    // `WOW_LIGHT_DUMP=1`: print the committed global light once per game minute — the live-probe
    // instrument for comparing our resolved pair against a reference-trace decode (which reports the
    // committed GL pair in 0..255 bytes). Throttled on the whole minute so a 60 s probe logs a handful
    // of lines, not one per frame (`sun_dir` wobbles continuously).
    //
    // It leads with the **`map` + eye position it sampled at**, because those are the two inputs that
    // decide everything after them and neither is visible from the chair. Every colour below is
    // `sample_blended(map, wow_pos, …)`, and `benilla-extract lightblend <map> <x> <y> <z> <minute>`
    // recomputes the same call offline — so a line from a live run can be replayed against the DBC
    // instead of argued about. Without the map printed, "is this the atmosphere the zone actually
    // authored, or the one next door?" costs a session (Stratholme, 2026-07-29).
    // `WOW_LIGHT_DUMP=frame` drops the minute throttle, exactly as `WOW_FOG_DUMP=frame` does. The
    // minute gate is right for comparing colours against a DBC replay, and useless for anything
    // POSITIONAL: game minutes advance slowly enough that a whole probe logs two lines, and both of
    // them land in the first moments after world entry — before the submersion verdict has settled,
    // so the line reports `Dry` at a position that is plainly underwater (1829, chasing exactly
    // that contradiction).
    static LIGHT_DUMP: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    if let Some(mode) = LIGHT_DUMP.get_or_init(|| std::env::var_os("WOW_LIGHT_DUMP")) {
        let key = (mode.as_os_str() != "frame").then_some(minute);
        if key.is_some() && *last_dump == key {
            return;
        }
        *last_dump = key;
        let b = |c: [f32; 3]| [c[0] * 255.0, c[1] * 255.0, c[2] * 255.0].map(|v| v.round() as i32);
        eprintln!(
            // The submersion tail (1829) reports the RAMP ITSELF, not just its result. Comparing
            // two dives at different depths cannot prove the ramp fired — the light row moves with
            // the clock between runs, and the dump only fires on a game-minute change, so the two
            // samples are never the same row. Printing the factors and their input settles it in
            // one line instead.
            "[light] map {map} at ({:.1}, {:.1}, {:.1}) minute {minute} ({source:?}) ambient {:?} diffuse {:?} spec {:?} sun_dir {:.3} fog {:?} start/end {:.0}/{:.0} submersion {submerged:?} eye_z {:.1} ocean_fac {:.4}/{:.4}",
            wow_pos[0],
            wow_pos[1],
            wow_pos[2],
            b(resolved.ambient),
            b(resolved.diffuse),
            b(resolved.spec),
            resolved.sun_dir,
            b(resolved.fog_color),
            resolved.fog_start,
            resolved.fog_end,
            eye_liquid.eye_z(),
            fac1,
            fac2,
        );
    }
}

/// **Step 6** — drive the camera's `ClearColor` from the same Light.dbc row-7 fog colour the
/// terrain and model shaders blend in (Step 5). Sky / skybox proper is deferred; for now the
/// backdrop is just the fog colour, so a fully-fogged texel at the far plane lands on the same
/// gamma byte as the void behind it (no horizon seam). q6 RE: the reference client's sky pass
/// actually sets its own tiny 70→75 fog and draws in the fog colour, so the horizon converges on
/// row 7 the same way; we approximate that with `glClearColor` until the sky dome is built
/// (out-of-scope per the plan).
///
/// `Color::srgb` takes gamma-space 0..1 — that's exactly the form `WowLighting.fog_color` carries
/// (the DBC byte triple divided by 255). Bevy's clear pass writes the bytes to the sRGB surface
/// target directly, so the on-screen byte equals the DBC byte (same invariant as the shader output
/// decode). Only writes when the colour actually changes, so frame-to-frame churn stays at zero.
pub(super) fn apply_sky_backdrop(
    lighting: Option<Res<WowLighting>>,
    mut clear: ResMut<ClearColor>,
    mut last: Local<Option<[f32; 3]>>,
) {
    let Some(l) = lighting else {
        return;
    };
    if *last == Some(l.fog_color) {
        return;
    }
    *last = Some(l.fog_color);
    // GAMMA LANE (0161): the buffer holds gamma bytes — the clear writes the authored DBC
    // value RAW (`linear_rgb` = no conversion); the frame's one decode is the FFXGlow combine.
    clear.0 = Color::linear_rgb(l.fog_color[0], l.fog_color[1], l.fog_color[2]);
}

#[cfg(test)]
mod ocean_ramp_tests {
    use super::hsv_value_scale;

    /// A textbook `RGB → HSV → RGB` round trip, written out here so the identity claim in
    /// [`hsv_value_scale`]'s doc is *checked* rather than argued. If it were ever false, our one
    /// multiply would be silently wrong in colour rather than obviously wrong in code.
    fn round_trip_v_scale(c: [f32; 3], f: f32) -> [f32; 3] {
        let (r, g, b) = (c[0], c[1], c[2]);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let v = max;
        let sat = if max == 0.0 { 0.0 } else { (max - min) / max };
        let hue = if max == min {
            0.0
        } else if max == r {
            60.0 * (((g - b) / (max - min)) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / (max - min) + 2.0)
        } else {
            60.0 * ((r - g) / (max - min) + 4.0)
        };
        // V *= f, H and S untouched — the reference's `0x6d1620`.
        let v = v * f;
        // HSV -> RGB
        let cc = v * sat;
        let x = cc * (1.0 - (((hue / 60.0) % 2.0) - 1.0).abs());
        let m = v - cc;
        let (r1, g1, b1) = match (hue / 60.0).floor() as i32 {
            0 => (cc, x, 0.0),
            1 => (x, cc, 0.0),
            2 => (0.0, cc, x),
            3 => (0.0, x, cc),
            4 => (x, 0.0, cc),
            _ => (cc, 0.0, x),
        };
        [r1 + m, g1 + m, b1 + m]
    }

    /// The reference scales HSV **V** alone; we transcribe that as a plain RGB multiply. This is an
    /// identity, not an approximation — and the round trip above says so for real colours,
    /// including the degenerate greys and blacks where the hue is undefined.
    #[test]
    fn the_hsv_detour_is_a_plain_multiply() {
        let colours = [
            [0.8, 0.4, 0.1],
            [0.1, 0.1, 0.1],
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.2, 0.9, 0.55],
            [0.0, 0.3, 0.7],
        ];
        for c in colours {
            for f in [1.0f32, 0.875, 0.75, 0.5] {
                let want = round_trip_v_scale(c, f);
                let got = hsv_value_scale(c, f);
                for i in 0..3 {
                    assert!(
                        (want[i] - got[i]).abs() < 1e-6,
                        "{c:?} × {f}: HSV round trip {want:?} vs multiply {got:?}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wmo_portal::WmoFogTarget;

    /// The 4 s in/out crossfade (`0.25/s` — `[0x8115b0]`): entering ramps to full in 4 s, the
    /// staged fog LATCHES on exit so the fade-out lerps from the room's fog, and the ramp
    /// releases the latch at zero.
    #[test]
    fn wmo_fog_ramp_fades_in_and_out_over_four_seconds() {
        let mut ramp = WmoCrossfade::default();
        let target = WmoFogTarget {
            color: [1.0, 0.5, 0.0],
            end: 194.4,
            start_scalar: 0.25,
        };
        let scene = ([0.2, 0.2, 0.2], -139.0, 278.0);
        // 2 s in → half-way: end midway between the storm's 278 and the room's 194.4.
        let (_, _, end) = ramp.blend(Some(target), scene.0, scene.1, scene.2, 1100.0, 2.0);
        assert!((ramp.t - 0.5).abs() < 1e-6);
        assert!((end - (278.0 + (194.4 - 278.0) * 0.5)).abs() < 1e-3);
        // 2 s more → fully the room's fog: start = end × scalar, storm veil gone.
        let (color, start, end) = ramp.blend(Some(target), scene.0, scene.1, scene.2, 1100.0, 2.0);
        assert!((end - 194.4).abs() < 1e-3);
        assert!((start - 194.4 * 0.25).abs() < 1e-3);
        assert!((color[0] - 1.0).abs() < 1e-6);
        // Leave: 2 s out with NO target — the latched room fog still blends at half weight.
        let (_, _, end) = ramp.blend(None, scene.0, scene.1, scene.2, 1100.0, 2.0);
        assert!((ramp.t - 0.5).abs() < 1e-6);
        assert!((end - (278.0 + (194.4 - 278.0) * 0.5)).abs() < 1e-3);
        // 2 s more → scene fog verbatim and the latch releases.
        let (color, start, end) = ramp.blend(None, scene.0, scene.1, scene.2, 1100.0, 2.0);
        assert_eq!((color, start, end), scene);
        let (c2, s2, e2) = ramp.blend(None, scene.0, scene.1, scene.2, 1100.0, 2.0);
        assert_eq!((c2, s2, e2), scene);
        assert!(ramp.staged.is_none(), "latch releases at t = 0");
    }

    /// The staging clamp: a record end beyond the farclip commits the farclip (`0x6cef43` clamps
    /// vs `[0xce9b94]`), and start scales off the CLAMPED end.
    #[test]
    fn wmo_fog_staging_clamps_end_to_farclip() {
        let mut ramp = WmoCrossfade::default();
        let target = WmoFogTarget {
            color: [1.0; 3],
            end: 444.4,
            start_scalar: 0.25,
        };
        let (_, start, end) = ramp.blend(Some(target), [0.0; 3], 0.0, 200.0, 300.0, 4.0);
        assert!((end - 300.0).abs() < 1e-4);
        assert!((start - 75.0).abs() < 1e-4);
    }
}
