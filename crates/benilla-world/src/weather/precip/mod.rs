//! Precipitation: the pooled rain and snow simulation, its ground layers and the mist companion,
//! drawn on the shared effect stream. The laws (constants, rates, gates, blend states) are the
//! reference's, in f32 and Bevy space. Drops pass through a packet delay line, so an upswing's
//! first rain shows at ~5 s or 8.5–14 s, after the fog and mist, and outlasts a same-type
//! clear-down.

use bevy::prelude::*;

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};

use crate::lighting::WowLighting;
use crate::particles::buffer::{
    begin_effect_frame, EffectBlend, EffectDrawSpec, EffectFog, EffectQuads,
};
use crate::view::WorldCamera;
use crate::wmo_portal::{CameraInteriorClaim, WmoPvsSet};

use super::{WeatherKind, WeatherState, WeatherTick};

mod census;
mod mist;
mod pool;
mod render;
mod wind;
use census::{census, profile};
use mist::{push_mist, run_mist, Mist};
use pool::{run_kind, HeightCache, Pool};
use render::{push_flakes, push_patters, push_streaks, FlakeView};
pub(crate) use wind::{wow_azimuth_to_bevy, WeatherWind};

// ===== The reference's constants =====

/// The weather leg: the reference runs the shader leg unless `useWeatherShaders` (default 1,
/// `0x67b81d`) is 0 or its ARB programs fail to validate (`0x58b360`); this always runs it.
const SHADER_LEG: bool = true;
/// Per-packet record capacity, 6144 (`0x80ffc8` as a float, which the open stamp divides by the
/// rate). Not a population cap: packets form a growing list, so demand bounds the population.
const PACKET_CAP: usize = 0x1800;
/// The anchor cull, retirement condition 3 of the active-list walk (`0x677ff0`; `0x6780d3`–
/// `0x678110` against `0x810008` = 200): a packet whose open-time anchor is farther than this
/// from the eye, in 3-D (`0x4549f0`), is discarded outright. Only a teleport can trip it.
const RETIRE_DIST: f32 = 200.0;
/// Slack on [`RETIRE_DIST`] for active drops, which keep no packet here and go by their own
/// distance. The reference draws no flake past `200 + box corner + 1.75·W` (≤ ~324 yd), so this
/// cull is strictly weaker than its own and never retires a flake it would still draw.
const RETIRE_DROP_SLACK: f32 = 50.0;
/// Deviation: a spawn ceiling on live plus pending drops, which the reference lacks, because each
/// flake here costs a 4-vertex quad. Demand is `rate · fall_time` plus a packet, and the fall
/// depends on the ground under the spawn plane, so this sits above the worst case measured at
/// the ungained rate; nothing is preallocated. It bites when the state log's pipe count falls
/// below [`PACKET_CAP`].
const fn pool_bound(kind: WeatherKind) -> usize {
    match kind {
        WeatherKind::Snow => {
            if SHADER_LEG {
                0x20000
            } else {
                0x4000
            }
        }
        _ => {
            if SHADER_LEG {
                0xC000
            } else {
                0x4000
            }
        }
    }
}
/// The largest pool bound, the `take(…)` guard on a push.
const POOL: usize = pool_bound(WeatherKind::Snow);
// The density gain `K` in `rate = K·P·grade` is `WeatherState::density_gain()`, the
// `weatherDensity` table (`0x67b870`: 0.1, 0.33, 0.66, 1.0 for settings 0 to 3).
/// Rain population `P`: fixed-function `0x80ffa8` = 6500, shader `0x80ffac` = 35000.
const RAIN_P: f32 = if SHADER_LEG { 35000.0 } else { 6500.0 };
/// Snow population `P`: fixed-function `0x80ffdc` = 1300, shader `0x80ffe0` = 14000.
const SNOW_P: f32 = if SHADER_LEG { 14000.0 } else { 1300.0 };
/// Deviation: the drop rate is scaled to a 30 fps client, the frame rate the reference's look
/// was matched at, because the reference budgets `min(dt, 1/60)·rate` and drops the remainder
/// (`0x67846f`–`0x678480`), so it emits half its nominal rate at 30 fps. 60 is byte-faithful.
const REF_MAXFPS: f32 = 30.0;
/// [`REF_MAXFPS`] as a gain on the drop rate. Never on the mist: its accumulator carries its
/// remainder (`0x67b172`), so it is already frame-rate independent in the reference.
const REF_FPS_GAIN: f32 = if REF_MAXFPS < 60.0 {
    REF_MAXFPS / 60.0
} else {
    1.0
};
/// A patter lives 0.25 s (`0x675280`: `lifetime = now + 0.25`).
const PATTER_LIFE: f32 = 0.25;
/// Per-frame spawn budget uses `min(dt, 1/60)` (`0x80ffcc`).
const DT_CAP: f32 = 1.0 / 60.0;
/// The spawn box leads the camera by `1.75·wind` (`0x8680f8`).
const WIND_LEAD: f32 = 1.75;
/// The drift heading's centre, −1.57 rad (`0x80ffbc`): a fixed world azimuth in the WoW frame
/// for rain and snow, never the wind's heading.
const DRIFT_AZ_CENTER: f32 = -1.57;
/// Rain fall speed `vz = −28 − 4w − 2w·r` (`0x80306c`, `0x80ffb4`).
const RAIN_VZ_BASE: f32 = 28.0;
const RAIN_VZ_W: f32 = 4.0;
const RAIN_VZ_RNG: f32 = 2.0;
/// Rain drift speed `((2r − 1) + 9.49)·w + 0.01`, heading spread `0.209·w + 0.052` rad.
const RAIN_DRIFT_BASE: f32 = 9.49;
const RAIN_DRIFT_EPS: f32 = 0.01;
const RAIN_SPREAD_W: f32 = f32::from_bits(0x3e56_7750); // 0x80ffc4 ≈ 0.20944 (12°)
const RAIN_SPREAD_BIAS: f32 = f32::from_bits(0x3d56_7750); // 0x80ffc0 ≈ 0.05236 (3°)
/// The streak triangle: base `head ∓ 0.05·right` (`0x80ff78`), apex `head + tilt·(2·antiVel̂)`
/// (`0x80ff74`), fixed world sizes, not scaled by speed; no vertex colour.
const STREAK_HALF_W: f32 = 0.05;
const STREAK_TAIL: f32 = 2.0;
/// Rain's forced fog window (render states 0x0a/0x0b): under Mod2x its grey-0.5 colour is
/// neutral, so this is the streak distance fade. `EffectFog::Rain`'s row is written from these.
pub(crate) const RAIN_FOG_START: f32 = 70.0;
pub(crate) const RAIN_FOG_END: f32 = 75.0;
/// Patter triangle half-edges: `view_right/12` and `view_up/6` (`0x80e004`, `0x803568`).
const PATTER_RIGHT: f32 = 1.0 / 12.0;
const PATTER_UP: f32 = 1.0 / 6.0;
/// Spawn-box horizontal half-extents, from the ctor args (`0x67be40`): rain 130×130
/// (`0x43020000`), snow 90×90 (`0x42b40000`).
const RAIN_HALF_XY: f32 = 65.0;
const SNOW_HALF_XY: f32 = 45.0;
/// The slab's lift in slab-local space, half the ctor's vertical extent (rain 75, snow 60): the
/// box's vertical random is dead but this lift is not (`0x674df6`, `T = −37.5/Vz`). Not a world
/// height: the tilt fans spawn heights to `z_off·cos α ∓ half_xy·sin α`.
const RAIN_Z_OFF: f32 = 37.5;
const SNOW_Z_OFF: f32 = 30.0;
/// Snow fall speed `vz = −2 − 3.5m − m·r` (`0x6778bc`–`0x6778ec`), 2 to 6.5 yd/s.
const SNOW_VZ_BASE: f32 = 2.0;
const SNOW_VZ_W: f32 = 3.5;
/// Snow horizontal drift `((r − 0.5) + 5.985)·m + 0.015`.
const SNOW_DRIFT_OFF: f32 = 5.985;
const SNOW_DRIFT_EPS: f32 = 0.015;
/// Snow heading spread `2π − 5.934·m` (`0x80fff4`): any direction calm, ±10° in a blizzard.
const SNOW_SPREAD_W: f32 = f32::from_bits(0x40bd_e44f); // ≈ 5.9341197
/// Snow flake size in pixels, `snowpoint.bls`'s `max(1, 14·clamp01(1 − 0.02·d))`, `d` in yards:
/// not a world size, not `1/d`. The point-sprite pass `0x678610` runs; `0x678960`'s 1/12
/// triangle is the fallback, taken only when `snowpoint.bls` fails, `GL_ARB_point_sprite` is
/// missing or `useWeatherShaders` is 0 (`0x677420`).
const SNOW_PX_AT_EYE: f32 = 14.0;
/// Deviation: flake pixels are pixels of an 800-px-tall screen, the height the look was matched
/// at, converted to an angle, because the reference's literal framebuffer pixels shrink with
/// resolution (2.7× smaller at 4K scale 2). At 800 px tall the reference's pixels are exact.
const SNOW_PX_REF_HEIGHT: f32 = 800.0;
/// The falloff slope: `1 − 0.02·d` reaches the 1 px floor at `d = 13/0.28 ≈ 46.4` yd.
const SNOW_PX_FALLOFF: f32 = 0.02;
/// The 1 px floor the ARB program applies last.
const SNOW_PX_MIN: f32 = 1.0;
/// A settled flake fades over 0.25 s (`0x678dfa`, constant `0x8029b0`: `max(t,0) + 0.25`).
const SNOW_SETTLE_LIFE: f32 = 0.25;
/// A falling flake fades in over its first second, the vertex program's `clamp01(t − f1)`.
const SNOW_FADE_IN: f32 = 1.0;

/// The precipitation state: the per-kind pools, the mist and the shared RNG.
#[derive(Resource)]
pub(super) struct Precip {
    rain: Pool,
    snow: Pool,
    mist: Mist,
    rng: u32,
}

/// Is weather hidden by the camera's room: the reference's weather-visible flag `[0xca80c4]`
/// inverted, cleared per frame (`0x6b38c1`), set with the camera outdoors (`0x6811d4`) or an
/// exterior group seen through a portal (`0x6b42d9`). The reference's dispatchers update the
/// drops first and gate only the draw and the mist on it (`0x677380`, `0x6790b0`, `0x67a520`).
#[derive(Resource, Default)]
pub(super) struct WeatherIndoors(bool);

/// Resolves [`WeatherIndoors`] from the portal pass's [`CameraInteriorClaim`] this frame.
fn gate_weather_indoors(claim: Res<CameraInteriorClaim>, mut indoors: ResMut<WeatherIndoors>) {
    indoors.0 = claim.0.is_some_and(|c| !c.exterior_visible);
}

/// Deviation: xorshift32 to [0, 1) instead of the reference's lagged-table generator, because
/// the laws need only a uniform distribution, not its exact stream.
fn rand01(rng: &mut u32) -> f32 {
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *rng = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Ground-layer capacity: the reference's patter packet holds 0x1800 (`Packet<Patter,Rain>`).
const GROUND_CAP: usize = 0x1800;

/// The four precip textures; the stream withholds their draws until they load.
#[derive(Resource)]
struct PrecipAssets {
    streak: Handle<Image>,
    splash: Handle<Image>,
    flake: Handle<Image>,
    mist: Handle<Image>,
}

fn setup_precip(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    existing: Option<Res<Precip>>,
) {
    // Spawned on the first frame with a live camera.
    if existing.is_some() || cam.single().is_err() {
        return;
    }
    // Streaks keep their mips; the splash and mist cut-outs load mip 0 only, as their thin arms
    // collapse under the chain; the flake keeps its 7 mips, drawn 4.5× to 64× minified.
    let effect = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::Effect;
    };
    let point_sprite = |s: &mut benilla_assets::BlpLoaderSettings| {
        s.variant = benilla_assets::BlpVariant::PointSprite;
    };
    commands.insert_resource(PrecipAssets {
        streak: asset_server.load("mpq://textures/weather/raindrop01.blp"),
        splash: asset_server
            .load_with_settings("mpq://textures/weather/raindropsplash01.blp", effect),
        flake: asset_server
            .load_with_settings("mpq://textures/weather/snowflake01.blp", point_sprite),
        mist: asset_server.load_with_settings("mpq://textures/weather/snowmist01.blp", effect),
    });
    commands.insert_resource(Precip {
        rain: Pool::default(),
        snow: Pool::default(),
        mist: Mist::default(),
        rng: 0x9E37_79B9,
    });
}

/// Per frame: wind, spawn budgets, integrate, land; [`push_precip`] draws in `PostUpdate`.
fn simulate_precip(
    time: Res<Time>,
    weather: Res<WeatherState>,
    cam: Query<&GlobalTransform, With<WorldCamera>>,
    player: Query<&Transform, (With<crate::world_unit::ViewerUnit>, Without<WorldCamera>)>,
    // The live commanded speed the slab tilt keys on (`mgr+0x7c`), not the averaged wind.
    viewer: Res<crate::view::Viewer>,
    spatial: SpatialQuery,
    indoors: Res<WeatherIndoors>,
    mut wind: ResMut<WeatherWind>,
    mut heights: ResMut<HeightCache>,
    mut precip: Option<ResMut<Precip>>,
    mut last_cut: Local<u32>,
    // Frames since the previous census line.
    mut census_frames: Local<u32>,
) {
    let Some(precip) = precip.as_deref_mut() else {
        return;
    };
    // Indoors this freezes the whole effect, where the reference keeps simulating drops unseen
    // and freezes only the mist.
    if indoors.0 {
        return;
    }
    let Ok(cam_tf) = cam.single() else { return };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let now = time.elapsed_secs();
    let cam_pos = cam_tf.translation();
    // The wind tracks the player, not the camera; the camera stands in only when there is none.
    let (wind_pos, facing) = player
        .single()
        .map_or((cam_pos, cam_tf.forward().as_vec3()), |t| {
            (t.translation, t.forward().as_vec3())
        });
    wind.update(wind_pos, facing, viewer.planar_speed, dt);

    let filter = crate::collision::WorldCollision::body_filter();
    let Precip {
        rain,
        snow,
        mist,
        rng,
        ..
    } = precip;

    // A wire type change (fine included) stops emission (`0x67585d`) and cuts the pipeline.
    if weather.cut_seq != *last_cut {
        *last_cut = weather.cut_seq;
        rain.cut(now);
        snow.cut(now);
        mist.cut();
    }

    run_kind(
        rain,
        WeatherKind::Rain,
        &weather,
        &wind,
        &mut heights,
        &spatial,
        &filter,
        rng,
        now,
        dt,
        cam_pos,
    );
    run_kind(
        snow,
        WeatherKind::Snow,
        &weather,
        &wind,
        &mut heights,
        &spatial,
        &filter,
        rng,
        now,
        dt,
        cam_pos,
    );

    // The 1 Hz state log, while there is weather or anything left in the pools.
    *census_frames += 1;
    if (weather.effect_density > 0.0
        || !rain.drops.is_empty()
        || !snow.drops.is_empty()
        || rain.pending_len() > 0
        || snow.pending_len() > 0)
        && time.elapsed_secs().fract() < dt
    {
        debug!(
            "weather pools: rain {} (+{} pipe, +{} ground) snow {} (+{} pipe, +{} ground), density {:.2}",
            rain.drops.len(),
            rain.pending_len(),
            rain.patters.len(),
            snow.drops.len(),
            snow.pending_len(),
            snow.patters.len(),
            weather.effect_density,
        );
        // Where the drops are: split along the motion heading, or the view heading when standing.
        let motion = wind.vel.with_y(0.0);
        let axis = motion
            .try_normalize()
            .or_else(|| cam_tf.forward().as_vec3().with_y(0.0).try_normalize())
            .unwrap_or(Vec3::Z);
        // And where the field stops overhead; rain's streaks carry no vertex alpha, so fade-in 0.
        for (name, pool, fade_in) in [("rain", &*rain, 0.0), ("snow", &*snow, SNOW_FADE_IN)] {
            if let Some(c) = census(&pool.drops, cam_pos, axis, motion.length(), *census_frames) {
                debug!("weather field ({name}): {c}");
            }
            if let Some(p) = profile(&pool.drops, cam_pos, fade_in) {
                debug!("weather column ({name}): {p}");
            }
        }
        *census_frames = 0;
    }

    // ===== the mist companion =====
    {
        let Precip { mist, rng, .. } = precip;
        run_mist(
            mist,
            &weather,
            &wind,
            &mut heights,
            &spatial,
            &filter,
            rng,
            dt,
            cam_pos,
        );
    }
}

/// Pushes the frame's precip onto the effect stream after its clear: rain as Mod2x triangles
/// under the forced grey fog (both textures' grounds are exactly RGB 128), snow and mist as
/// alpha quads with fog off (`0x678610`, `0x67ae20`). Anchored at the camera, they sort after
/// every world transparent and before the biased glare and nameplate rungs.
fn push_precip(
    precip: Option<Res<Precip>>,
    assets: Option<Res<PrecipAssets>>,
    indoors: Res<WeatherIndoors>,
    lighting: Res<WowLighting>,
    wind: Res<WeatherWind>,
    cam: Query<(Entity, &GlobalTransform, &Camera, &Projection), With<WorldCamera>>,
    mut quads: ResMut<EffectQuads>,
) {
    let (Some(precip), Some(assets)) = (precip, assets) else {
        return;
    };
    if indoors.0 {
        return;
    }
    let Ok((cam_entity, cam_tf, camera, proj)) = cam.single() else {
        return;
    };
    let cam_pos = cam_tf.translation();
    let cam_right = cam_tf.right().as_vec3();
    let cam_up = cam_tf.up().as_vec3();
    // The live viewport only gates whether this camera draws: flakes are sized in era pixels.
    let flake_view = camera
        .physical_viewport_size()
        .filter(|vp| vp.y > 0)
        .map(|_| FlakeView {
            eye: cam_pos,
            forward: cam_tf.forward().as_vec3(),
            right: cam_right,
            up: cam_up,
            world_per_px: match proj {
                Projection::Perspective(p) => (p.fov * 0.5).tan(),
                // The reference has no orthographic mode; this keeps the sprites sane.
                _ => (crate::view::CAM_FOVY * 0.5).tan(),
            } / SNOW_PX_REF_HEIGHT,
        });
    let spec = |texture: &Handle<Image>, blend: EffectBlend, fog: EffectFog| EffectDrawSpec {
        cam: cam_entity,
        texture: texture.id(),
        blend,
        fog,
        // The weather's own render state, not the M2 batch state: unlit.
        lighting: crate::particles::buffer::EffectLighting::None,
        anchor: cam_pos,
        bias: 0.0,
        raster_bias: 0,
        raster_slope: 0.0,
        // Absolute verts suit centimetre-scale geometry; the flakes override this.
        cam_relative: false,
        no_depth_test: false,
        main_entity: Entity::PLACEHOLDER,
        light: None,
        clip: None,
    };
    let start = quads.begin();
    push_streaks(&mut quads.verts, &precip.rain.drops, wind.tilt, cam_pos);
    quads.commit_tris(
        start,
        spec(&assets.streak, EffectBlend::Mod2x, EffectFog::Rain),
    );
    let start = quads.begin();
    push_patters(&mut quads.verts, &precip.rain.patters, cam_right, cam_up);
    quads.commit_tris(
        start,
        spec(&assets.splash, EffectBlend::Mod2x, EffectFog::Rain),
    );
    if let Some(view) = &flake_view {
        let start = quads.begin();
        push_flakes(
            &mut quads.verts,
            &precip.snow.drops,
            &precip.snow.patters,
            view,
        );
        quads.commit_quads(
            start,
            EffectDrawSpec {
                // `push_flakes` wrote camera-relative offsets: its millimetre quads need them.
                cam_relative: true,
                ..spec(&assets.flake, EffectBlend::Alpha, EffectFog::Off)
            },
        );
    }
    let start = quads.begin();
    push_mist(
        &mut quads.verts,
        &precip.mist,
        cam_right,
        cam_up,
        cam_pos,
        lighting.fog_color,
    );
    quads.commit_quads(
        start,
        spec(&assets.mist, EffectBlend::Alpha, EffectFog::Off),
    );
}

pub(super) fn register(app: &mut App) {
    app.init_resource::<WeatherWind>()
        .init_resource::<HeightCache>()
        .init_resource::<WeatherIndoors>()
        .add_systems(
            Update,
            (
                setup_precip,
                gate_weather_indoors.after(setup_precip).after(WmoPvsSet),
                simulate_precip
                    .after(WeatherTick)
                    .after(gate_weather_indoors),
            ),
        )
        // The stream push: PostUpdate after the frame's clear (the sim ran in Update).
        .add_systems(PostUpdate, push_precip.after(begin_effect_frame));
}
