//! Time-of-day lighting, driven by `Light.dbc` sampled against the server game-clock. **Lighting
//! rebuild — Phase 0 (pitch black).** This module now only *resolves the faithful light values* —
//! [`WowLighting`] (ambient/diffuse/specular colors + the vanilla `DayNight::SetDirection` sun
//! direction) — and pushes them onto the terrain/model materials. It no longer spawns a PBR sun /
//! ambient / distance fog / sky; the faithful lighting is rebuilt in-shader, one verified step at a
//! time, on top of the Phase-0 black baseline.

use bevy::prelude::*;

use benilla_assets::AssetSet;
use benilla_formats::{LightCatalog, LiquidKind};

mod blob; // the off-world light-blob builder (booth studio, body pane, glue scene)
mod daynight; // the two sun directions + day/night interp + the dawn/dusk warp curve
mod global_light; // the one shared global-light storage buffer (replaces the per-material push)
mod prop_probes; // the per-instance interior-prop SH probe table (slot ↔ MeshTag payload)
mod resolve; // the per-frame time-of-day sample into WowLighting + the WMO interior-fog crossfade
mod sh; // the model SH light-probe coefficient math
pub use blob::LightBlob;
pub use global_light::{new_shared_light_buffer, LightRooms, SharedLightBuffer, WorldPointLight};
pub use prop_probes::{PropProbeSlot, PropProbes, MAX_PROP_PROBES};
// The std430 layout itself — row indices, byte sizes, region offsets and the folds that fill them
// — stays in the crate. Off-world producers state values through `LightBlob` and never a row index
// (see `blob`); `global_light` builds the world's own blob on the frame path.
pub(crate) use prop_probes::prop_probe_region_offset;
pub use resolve::WmoCrossfade;
use resolve::{apply_sky_backdrop, setup_lighting, update_time_lighting};
pub(crate) use sh::prop_probe_coeffs;

/// Scene lighting sampled from `Light.dbc` for the current time of day — fed into the terrain
/// shader's WoW lighting. Colors are sRGB 0..1; `sun_dir` is the world-space (Bevy) direction the
/// sun's light travels. Rewritten every frame by [`update_time_lighting`] as the clock advances.
#[derive(Resource, Clone, Copy, Default, PartialEq)]
pub struct WowLighting {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    /// Specular highlight color — the DBC sun-halo color (row 9, warm-white). Drives the terrain
    /// sheen (Step 3b) and, later, model highlights (Step 7).
    pub spec: [f32; 3],
    pub sun_dir: Vec3,
    /// The **visible celestial sun** direction (camera→sun, Bevy space) — a *separate* body from the
    /// lighting `sun_dir`. Drives the sun-disc/glow sprite (`sun.rs`). Unlike the near-fixed lighting
    /// sun, this one genuinely rises and sets in elevation over the day (see [`daynight::celestial_sun_direction`]).
    pub(crate) celestial_dir: Vec3,
    /// **Step 5 fog** — gamma-space distance fog, per the q6 RE. Color = `Light.dbc` IntBand
    /// **row 7** raw (no sky-blend, no sun-glow). `GL_LINEAR`, applied in-shader in gamma space
    /// BEFORE the sRGB store cancel — NOT Bevy's `DistanceFog`, which blends in linear and breaks
    /// the gamma invariant.
    pub fog_color: [f32; 3],
    /// The **pushed** GL fog pair from [`resolve::scene_fog`] — `end = min(raw band end, farclip)`,
    /// `start = frac × end` **unclamped** (`0x6cee61`): negative under storm (Elwynn frac −0.5),
    /// which is the reference's constant ~33% near veil in rain.
    pub(crate) fog_start: f32,
    pub(crate) fog_end: f32,
    /// The **interior** fog triple (DNState+0x80/84/88): `lerp(scene fog → the claimed WMO's MFOG
    /// fog, t)` on the 4 s camera-in-WMO ramp ([`WmoCrossfade`]) — equal to the scene triple while
    /// the camera is outside. Consumed ONLY by the interior lanes: the interior WMO-group surfaces
    /// and THAT group's doodads (`0x6b5190` / the group-doodad drawer `0x6b62e0`) —
    /// `wow_model.wgsl` selects it by the material interior flag. Terrain, liquid,
    /// sky, exterior groups, and world units keep the scene fog — the storm stays grey through the
    /// inn's open door (director ref-shot, 2026-07-13).
    pub(crate) wmo_fog_color: [f32; 3],
    pub(crate) wmo_fog_start: f32,
    pub(crate) wmo_fog_end: f32,
    /// The five sky-dome gradient stops, zenith→horizon (`Atmosphere.sky`, LightIntBand rows 2–6).
    /// Fed to the `SkyPlugin` dome material; interpolated across the dome by elevation.
    pub(crate) sky: [[f32; 3]; 5],
    /// **Per-kind water-surface tint** `[shallow, deep]` — `Atmosphere.water_river` (IntBand rows
    /// 16/17) and `.water_ocean` (rows 14/15), RAW, resolved from the **area-light blend**, exactly
    /// like every other band: the client's gather record carries all 18 colour rows and
    /// `0x6d30e0` merges all 18 per light (rows 14–17 are its `+0x34..+0x40`
    /// step-9 loop) — there is no single-sphere pick in the water path (decision 1104, superseding
    /// the `pick_light` split whose discontinuity snapped the tint at Tirisfal→Silverpine). The
    /// from-above depth swatch is a 2-endpoint linear lerp of these by the per-vertex depth `V`
    /// (river/lake `V = clamp(byte/42)`; VERIFIED `WoW.exe FUN_0068a830` + `c81768`). Pushed onto the
    /// per-kind liquid materials by [`apply_wow_lighting`] via [`WowLighting::water_colors`].
    pub(crate) water_river: [[f32; 3]; 2],
    pub(crate) water_ocean: [[f32; 3]; 2],
    /// **Per-kind water-blend alphas** `[shallow, deep]` — `LightParams.water/oceanShallow/DeepAlpha`,
    /// blended across the same spheres as the tint. The swatch's depth-alpha ramp endpoints. Per-zone:
    /// Elwynn/Loch Modan shallow ≈0.5, STV ≈0.85; deep ≈1.0. Replaces the hardcoded `*_SHALLOW_ALPHA`
    /// constants for the live path (they remain as the `setup_liquid` frame-0 seed + DBC-absent default).
    pub(crate) water_river_alpha: [f32; 2],
    pub(crate) water_ocean_alpha: [f32; 2],
    /// **Per-zone FFXGlow/bloom composite weight** — `LightParams.glow`, quantised `floor(g·255)/255`
    /// like the real client. Read by
    /// `glow.rs`'s `sync_glow_weight` as the faithful default for `GlowSettings.weight` (the panel
    /// slider is an override). Elwynn ≈ 0.647, Duskwood ≈ 0.498; fallback 0.5.
    pub(crate) glow: f32,
    /// **Dawn/dusk sky-dome warp strength** `S` — `dawn_dusk_curve(dayfrac) × highlightSky`, in
    /// `[0,1]`. Drives the per-zone azimuthal warp in `sky.wgsl` (`FUN_006d0f50`): the sun-facing
    /// quarter of the dome warms toward SkyColor0, the away side desaturates toward SkyColor1. The
    /// curve is **0 across all of midday and deep night** (spikes to 1 only at ~06:30/21:30), so this
    /// is 0 except at dawn/dusk — and 0 entirely in highlightSky=0 zones (Duskwood). At `S=0` the
    /// warp is identity, so daytime sky stays byte-faithful. See [`daynight::sky_warp`].
    pub(crate) sky_warp: f32,
    /// **Sun disc size multiplier** — `sun_disc_scale(dayfrac)` (vanilla size table `0xce8cac`): `1.0`
    /// across midday, up to `2.0` at the dawn/dusk horizon (the "huge sun at the horizon"). `sun.rs`
    /// multiplies the disc's base angular size (`sun_size`) by this. See [`daynight::sun_disc_scale`].
    pub(crate) sun_disc_scale: f32,
    /// **Sun lens-flare day/night envelope** — the per-body dnCurve table (`0xce9818`): `1.0` across
    /// the day (07:30→19:30), `0.0` all night with dawn/dusk dead-bands (off by 21:00, back at
    /// 06:30→07:30). A factor of the flare intensity's slew target in `sun::follow`
    /// (decision 0508). See [`daynight::sun_flare_dn`].
    pub(crate) sun_flare_dn: f32,
    /// **Visible moon direction** (Bevy camera→moon) — the white moon at azimuth 45° (the sun's
    /// bearing), up at night and below the horizon by day (`daynight::moon_direction`). The engine's
    /// second disc (`moon02.blp`) IS drawn but vertex-black (its colour field has no writer in the
    /// binary) at azimuth 135–165° on a phase-precessed schedule — never a visible second moon
    /// (decision 0485). Consumed by the moon billboards in `sun`.
    pub(crate) moon_dir_white: Vec3,
    /// **Moon disc size multiplier** — `1.0` overhead (~midnight) → `1.5` at moonrise/set (size table
    /// `0xce8c8c`); `sun` multiplies by the white moon's base ×1.75.
    pub(crate) moon_disc_scale: f32,
    /// **Moon lens-flare night envelope** — the moon's dnCurve table (`0xce9768`): flat `0.0` from
    /// 03:15 all the way to 22:45 (the whole day + early evening), ramping in 22:45→24:00, full
    /// 00:00→02:00. The moon's halo simply does not exist at a 22:30 moonrise (decision 0508).
    /// See [`daynight::moon_flare_dn`].
    pub(crate) moon_flare_dn: f32,
    /// **moon02 direction** (Bevy camera→body) — the engine's third disc, drawn vertex-BLACK on its
    /// phase-precessed 1.7-day clock ([`daynight::moon02_state`]; decision 0485). Never a visible
    /// second moon; faithfully occludes stars behind it.
    pub(crate) moon_dir_02: Vec3,
    /// **moon02 size multiplier** — the shared `0xce8c8c` curve sampled on moon02's own phase
    /// clock (base ×1.0).
    pub(crate) moon02_disc_scale: f32,
    /// **Star-field global alpha** — `star_alpha(dayfrac)` (vanilla star curve `0xce9a98`): `1.0` deep
    /// night → `0.0` all day (fade in 22:30→00:00, out 03:00→04:30). `sun.rs` multiplies the star dome's
    /// base-colour alpha by this (the reference's model-global star fade). See [`daynight::star_alpha`].
    pub(crate) star_alpha: f32,
    /// **SIDN night fraction** (`DNState+0x1ac`, track `0xce9a34`): `1.0` overnight → `0.0` all day
    /// (ramps 20:30→21:30 / 06:00→07:00). `wow_model.wgsl` multiplies every WMO SIDN material's
    /// authored emissive colour by it — the windows-glow-at-night ramp. See
    /// [`daynight::sidn_night_fraction`].
    pub(crate) sidn_night: f32,
    /// **Celestial diffuse tint** (sRGB) — the one DayNight colour the client broadcasts into the sun
    /// disc, sun glare, white-moon disc, and moon glare every frame (`[0xce9c2c]`, broadcast by
    /// `0x6d2260`; decision 0485). **LightIntBand sub-9** — byte-pinned: the band gather `0x6d64d0`
    /// swaps positions 8/9 so table[8] = sub-9, whose alpha is forced 0xFF (`0x6d62e0`); the same
    /// row [`Self::spec`] samples (warm cream at night — matching the reference trace's moon VBO
    /// (254,240,228) — orange at dawn/dusk). The `sun` follow systems rewrite the disc/glare
    /// material tints from it per frame; the moon's TEAL rim is NOT this tint but the dome's teal
    /// night bands through the disc's feathered edge.
    pub(crate) celestial_tint: [f32; 3],
    /// **Authored cloud density `C`** — Light.dbc FloatBand sub-3, weather/area blends included.
    /// Drives the coverage-field threshold (`clouds`): 0 = cloudless, 1 = full overcast potential
    /// (`[0xce9c64]`).
    pub(crate) cloud_density: f32,
    /// **Cloud palette** `[sun-glow, slope, gbase]` — IntBand sub-10/11/12, the visible cloud
    /// dome's colors (`0x6d64d0`).
    pub(crate) cloud_colors: [[f32; 3]; 3],
    /// **Storm blend `bcc`** = `min(1, weather_density·4)` (`0x6d4500`) — the
    /// weight already lerping the storm LightParams over the clear one, published for the
    /// celestial-alpha seed (`floor(255·(1−bcc))` on the five body alphas, Addendum #6) and the
    /// cloud sun-glow dimming (`1 − 0.75·bcc`). Purely weather-driven — authored `C` (`[0xce9c64]`)
    /// never feeds it; `bcc` reads the separate weather-density global `[0xce9ba0]`.
    pub(crate) storm_bcc: f32,
    /// **Cloud glow body direction** (Bevy, camera→body) — the sun while the day fraction sits in
    /// ≈04:50–22:10, the moon otherwise (`0x6cfb00` setup; [`daynight::cloud_glow_is_sun`]). The
    /// cloud color pass projects it onto the coverage tile as the glow centre.
    pub(crate) cloud_glow_dir: Vec3,
    /// **Cloud glow track factor** — the internal 8-key day envelope (`0xce9ab8`, ≈1.0 all day,
    /// twilight notches; [`daynight::cloud_glow_track`]). Multiplied by the weather dim
    /// `1 − 0.75·bcc` to form the glow intensity.
    pub(crate) cloud_glow_track: f32,
}

impl WowLighting {
    /// Per-kind **water swatch endpoints**: `(shallow_rgb, deep_rgb, shallow_alpha, deep_alpha)`. These
    /// are the ENDPOINTS; the ramp between them is a 64-row byte-space accumulator that `liquid.wgsl`
    /// reproduces (`swatch_row`), not the plain lerp this doc used to describe — it stops one row short
    /// of `deep`, and the ocean's last row is darkened (decision 2074). The rows are the zone's
    /// dedicated `Light.dbc` water rows — IntBand 16/17 (river/lake) or 14/15 (ocean), **RAW** (no
    /// ×0.711) — indexed by the per-vertex
    /// depth `V` (river/lake `V = clamp(byte/42)`, built in `benilla-formats::liquid`). VERIFIED from WoW.exe
    /// `FUN_0068a830`, golden-vector-matched to the apitrace swatch (≤1/255 over all 64 rows). The shader
    /// (`liquid.wgsl`) lerps both colour and opacity by the *same* V, so they track together.
    ///
    /// Alpha endpoints are the **per-zone `LightParams` water-blend alphas** (`water_*_alpha`, decoded
    /// from fields 5–8): Elwynn/Loch Modan shallow ≈0.5, STV ≈0.85 (its shallows read pale from the
    /// colour, not transparency) — VERIFIED vs the apitrace swatch alpha + user-confirmed in-game.
    /// (The earlier "water reflects the sky × 0.711 via `FUN_0068c250`" derivation fingered the WRONG
    /// builder; the dedicated rows 14-17 we'd originally used were right.)
    ///
    /// The opacity ramp itself is indexed by the raw MCLQ depth byte (0 = shore → 255 = deep) with
    /// **no scale** — VERIFIED `WoW.exe FUN_006b6b60` builds `ca7f10[d] = shallow + d·(deep−shallow)/256`
    /// from `gWorldLight+0x114/+0x118`, corroborated by the apitrace swatch (`α = 127 + 2·row` ⇒
    /// 0.5→1.0 for river/lake, 0.75→1.0 for ocean). The alpha and the colour ride the SAME per-vertex
    /// `V`, and **each kind rides its own verified LUT** — river/lake the steep `clamp(byte/42)`
    /// (`c81768`, `FUN_0068d790`), ocean the gentle `clamp(byte/255)` (`c7fcd8`, `FUN_0068d690`),
    /// both built side by side in `FUN_0068c4c0`. A river channel saturates to opaque deep teal by
    /// **byte 42 ≈ 5 yd** (ramp ≈8.5 byte/yd, VERIFIED `probe_water_depth`, re-measured at 8.96 over
    /// every MCLQ block in Azeroth + Kalimdor), leaving only the shore edge see-through; the sea
    /// authors its byte 5.2× gentler (1.72 byte/yd), so its `/255` saturates at ~148 yd of depth —
    /// the same ramp in yards, not a 6× slower one (decision 2069).
    /// (Earlier bugs: `×8 DEPTH_RAMP_SCALE` saturated at ~4 yd; then the gentle `byte/255` was the
    /// WRONG LUT **for a river** and the river middle never reached teal — which is what got the
    /// sea's own `/255` mislabelled a placeholder for months.)
    pub(crate) fn water_colors(&self, kind: LiquidKind) -> ([f32; 3], [f32; 3], f32, f32) {
        let (shallow_rgb, deep_rgb, alpha) = if kind == LiquidKind::Ocean {
            (
                self.water_ocean[0],
                self.water_ocean[1],
                self.water_ocean_alpha,
            )
        } else {
            (
                self.water_river[0],
                self.water_river[1],
                self.water_river_alpha,
            )
        };
        (shallow_rgb, deep_rgb, alpha[0], alpha[1])
    }
}

/// Water surface **specular shininess** — the water material's Phong power for the sun sheen
/// (`ffp_material.shininess` at the traced water draw ≈ 6; terrain uses 20). Lower ⇒ a broader,
/// softer glint that spreads at grazing (sunrise/sunset) sun.
pub(crate) const WATER_SHININESS: f32 = 6.0;

/// Quantise a raw `LightParams.glow` (0..1) to the byte the reference packs into the composite-quad
/// colour: `floor(g·255)/255`. (Elwynn 0.65 → 0.647.)
pub(crate) fn quantize_glow(glow: f32) -> f32 {
    (glow * 255.0).floor() / 255.0
}

/// The parsed `Light.dbc` family, kept resident so [`update_time_lighting`] can re-sample it each
/// frame as the server clock advances. Absent if the DBCs failed to load (we fall back to a neutral
/// day). Loaded once in `setup_lighting`.
#[derive(Resource)]
pub(crate) struct LightSampler(pub(crate) LightCatalog);

/// Where the time-of-day currently driving lighting comes from — a readout for the debug panel.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum ClockSource {
    /// Pre-connect (or DBC-less): hardcoded noon.
    #[default]
    Fallback,
    /// Live server game-clock (`SMSG_LOGIN_SETTIMESPEED`, advanced by its timescale).
    Server,
    /// Manually scrubbed in the debug panel.
    Manual,
}

/// **What time it is in the world** — the engine's clock *input*, written by whatever owns a
/// session clock.
///
/// The lighting read `net::ServerTime` directly, which made the renderer depend on a wire type
/// carrying an `Instant` and a packet timescale. 1160 put it plainly: the engine's clock is the
/// server's clock — but the engine only ever needed three scalars off it, and a program with no
/// server should get noon rather than a stub.
#[derive(Resource, Debug, Clone, Copy)]
pub struct WorldTime {
    /// Minute of the game day (`0..1440`) — what the `Light.dbc` colour sample, the clock display
    /// and the log read.
    pub minute: u32,
    /// The same, **fractional** — what the celestial body *positions* read, so the sun and moons
    /// glide instead of stepping once per game-minute (a visible jump at a fast timescale).
    pub minute_f: f32,
    /// The continuous day count for moon02's phase clock (days + day-fraction, unwrapped).
    pub day: f64,
    /// Has a real clock landed yet? `false` means the fields are the noon fallback, and the
    /// lighting reports `ClockSource::Fallback` rather than `Server`.
    pub live: bool,
}

impl Default for WorldTime {
    /// Noon, half a day in, not live — the fallback the lighting used to spell out itself when
    /// `ServerTime` held `None`.
    fn default() -> Self {
        Self {
            minute: 720,
            minute_f: 720.0,
            day: 0.5,
            live: false,
        }
    }
}

/// The effective game-clock driving lighting this frame — written by [`update_time_lighting`],
/// read by the debug panel for its time readout. The *output* side of [`WorldTime`]'s input.
#[derive(Resource, Default, Clone, Copy)]
pub struct GameClock {
    /// Minute of the game day (`0..1440`) being rendered.
    pub minute: u32,
    pub source: ClockSource,
}

/// Ordering handle: [`WowLighting`] and [`WmoCrossfade`] are resolved for the frame after this
/// set. The skybox weight resolve hangs off it — the sky reading a stale crossfade is a frame
/// where the painted sky and the fog disagree about how far into the building you are.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LightingResolveSet;

/// **The read side of [`LightingResolveSet`] — every `Update` system that reads the resolved
/// [`WowLighting`] belongs in this set.** Configured once, here, as
/// `LightingConsumeSet.after(LightingResolveSet)`; a consumer joins the contract with
/// `.in_set(..)` instead of remembering a bespoke `.after(..)`, which is precisely what five of
/// them did not remember (decision 2032).
///
/// **Why the contract needs a name of its own.** [`resolve::update_time_lighting`] is a *late*
/// system by construction — it waits on the wire drain ([`crate::schedule::WorldStage::Net`]),
/// the weather tick and the submersion verdict, each of which waits on something else. A consumer
/// that declares no ordering waits on nothing, so Bevy's executor starts it at the top of
/// `Update` and it reads the value the resolve wrote **last** frame. That is not a race: it is
/// deterministic, it happens on every frame, and it is invisible for as long as the atmosphere
/// only drifts with the clock.
///
/// It stops being invisible at a **submersion crossing**, where the atmosphere does not drift but
/// jumps — the whole underwater `LightParams` swaps in or out in one frame, on the very frame
/// [`crate::sky`]'s and [`crate::clouds`]'s own gates un-hide the domes. Report B354 is that
/// frame photographed: coming up off the Savage Coast, the first dry frame painted the sky dome
/// in Stranglethorn's UNDERWATER sky stops (`LightParams` 27) over an already-dry world, and
/// [`crate::clouds`]'s surfacing full-rebuild — the fix for the 0.4 s cloud pop-in — rebuilt the
/// coverage field at the *underwater* cloud density, which Stranglethorn authors as 0.0, so it
/// rebuilt an empty sky and the clouds crept back band by band exactly as they had before that
/// fix existed.
///
/// A consumer that runs in `PostUpdate` (the material/GPU pushes, the camera-anchored follows) is
/// already after the resolve by schedule and needs nothing from this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LightingConsumeSet;

/// The one place [`LightingConsumeSet`] is tied to [`LightingResolveSet`] — a named function so
/// the test below locks the SAME edge the plugin installs, rather than a copy of it.
pub(crate) fn configure_lighting_sets(app: &mut App) {
    app.configure_sets(Update, LightingConsumeSet.after(LightingResolveSet));
}

/// The lighting subsystem: registers the WoW-light resource + game-clock, a **black** background
/// (Phase 0 — the DBC sky comes back in a later step), and the two per-frame systems that resolve the
/// `Light.dbc` values for the time of day and push them onto the materials.
pub(crate) struct LightingPlugin;

impl Plugin for LightingPlugin {
    fn build(&self, app: &mut App) {
        // The read side of the resolve, configured once (see [`LightingConsumeSet`]).
        configure_lighting_sets(app);
        app.init_resource::<WowLighting>()
            .init_resource::<PropProbes>()
            .init_resource::<GameClock>()
            .init_resource::<WorldTime>()
            .init_resource::<WmoCrossfade>()
            .insert_resource(ClearColor(Color::BLACK))
            .add_systems(Startup, setup_lighting.after(AssetSet::Open))
            .add_systems(
                Update,
                (update_time_lighting, apply_sky_backdrop)
                    .chain()
                    .in_set(LightingResolveSet)
                    // The storm blend reads this frame's weather densities (decision 0302).
                    .after(crate::weather::WeatherTick)
                    // …and this frame's game clock, which whatever owns the session publishes in
                    // the wire-drain stage (`WorldTime`). Without this the two are unordered and
                    // the sun's position can lag a frame at a fast timescale.
                    .after(crate::schedule::WorldStage::Net)
                    // The submerged atmosphere reads THIS frame's submersion verdict — unordered,
                    // the murk could arrive a frame after the eye went under (and out of step with
                    // the sky-pass suppression, which reads the same verdict).
                    .after(crate::liquid::SubmersionVerdict),
            );
        // The shared global-light buffer (build_light_data after the resolve above; the extract +
        // render-world upload). Materials read this instead of carrying their own light copy.
        global_light::register(app);
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;

    /// The edge itself, asserted the way the defect presented: **not** "did they happen to run in
    /// this order" — B354's whole point is that an unordered pair *does* run in some order, one
    /// the executor picks and that changes with the graph around it — but "is the order
    /// DETERMINED". Two conflicting systems with no path between them are an *ambiguity*, which
    /// Bevy will name when asked; deleting or inverting [`configure_lighting_sets`] puts the
    /// ambiguity back and this fails.
    ///
    /// (An earlier cut of this test asserted the observed order and passed with the edge removed,
    /// because the executor's arbitrary choice happened to be the right one. That is exactly the
    /// reassurance the bug was hiding behind for months.)
    #[test]
    fn consumers_are_ordered_against_the_resolve() {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings, ScheduleBuildWarning};

        #[derive(Resource, Default)]
        struct Probe(u32);

        let mut app = App::new();
        app.init_resource::<Probe>();
        configure_lighting_sets(&mut app);
        app.edit_schedule(Update, |s| {
            s.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Warn,
                ..default()
            });
        });
        app.add_systems(
            Update,
            (
                (|mut p: ResMut<Probe>| p.0 += 1).in_set(LightingConsumeSet),
                (|mut p: ResMut<Probe>| p.0 += 1).in_set(LightingResolveSet),
            ),
        );
        app.update();
        let warnings = app
            .get_schedule(Update)
            .expect("Update schedule")
            .warnings()
            .iter()
            .filter(|w| matches!(w, ScheduleBuildWarning::Ambiguity(_)))
            .count();
        assert_eq!(
            warnings, 0,
            "a `LightingConsumeSet` system and a `LightingResolveSet` system are ambiguous — \
             `configure_lighting_sets` no longer orders the read side after the resolve"
        );
    }

    /// **Every `Update` reader of [`WowLighting`] in this crate joins the contract.** The defect
    /// B354 photographed was not a subtle one in any single system — it was five systems that
    /// each read the resolved atmosphere and none of which said when, so the executor started
    /// them at the top of `Update` and they got last frame's. A named set only helps if joining
    /// it is not something a sixth consumer can quietly forget, so this reads the crate's own
    /// source and insists.
    ///
    /// The allowlist is the set of readers that are after the resolve **by schedule** rather than
    /// by the set — every one of them runs in `PostUpdate` or is chained onto the resolve itself.
    /// Adding a name here is a claim about WHEN it runs; make it only with the reason.
    ///
    /// Scoped to this crate's sources — `benilla-app`'s three readers are outside what a
    /// `benilla-world` test can see (two are after the resolve by schedule; `minimap` is a
    /// deliberate exception, decision 2032).
    #[test]
    fn every_update_reader_joins_the_consume_set() {
        /// after-by-schedule, so the set would be redundant — file → why.
        const ALLOWED: &[(&str, &str)] = &[
            (
                "lighting/resolve.rs",
                "apply_sky_backdrop is .chain()ed onto the resolve",
            ),
            (
                "lighting/global_light.rs",
                "build_light_data: PostUpdate, .after(update_time_lighting)",
            ),
            (
                "sun/follow.rs",
                "the celestial follows: PostUpdate, BillboardPlace",
            ),
            ("weather/precip/mod.rs", "push_precip: PostUpdate"),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read_dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source");
                // A system param, not a doc mention: `Res<…WowLighting>` on one line.
                let reads = text.lines().any(|l| {
                    !l.trim_start().starts_with("//")
                        && l.contains("Res<")
                        && l.contains("WowLighting>")
                });
                // Membership means a real `.in_set(..)` CALL, not a doc comment naming the set —
                // an early cut of this check passed a file whose only mention was the comment
                // explaining why it joined.
                let joins = text.lines().any(|l| {
                    !l.trim_start().starts_with("//")
                        && l.contains("in_set(")
                        && l.contains("LightingConsumeSet")
                });
                if !reads || joins {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                if !ALLOWED.iter().any(|(f, _)| *f == rel) {
                    offenders.push(rel);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these read `WowLighting` without joining `LightingConsumeSet`: {offenders:?}\n\
             Order the system `.in_set(crate::lighting::LightingConsumeSet)`, or — if it already \
             runs after the resolve by schedule (PostUpdate, or chained onto it) — add it to \
             ALLOWED here with the reason."
        );
    }
}
