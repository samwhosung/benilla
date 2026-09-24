//! Time-of-day lighting: `Light.dbc` resolved against the game clock into [`WowLighting`] and the
//! shared light buffer every world shader reads; the shaders do the lighting themselves.

use bevy::prelude::*;

use benilla_assets::AssetSet;
use benilla_formats::{LightCatalog, LiquidKind};

mod blob; // off-world light blobs (portrait booths, body panes, glue scene)
mod daynight; // the sun, moon and day/night curves
mod global_light; // the shared global-light storage buffer
mod prop_probes; // the interior-prop SH probe table
mod resolve; // the per-frame time-of-day resolve and the interior-fog crossfade
mod sh; // the model SH probe fold
pub use blob::LightBlob;
pub use global_light::{new_shared_light_buffer, LightRooms, SharedLightBuffer, WorldPointLight};
pub use prop_probes::{PropProbeSlot, PropProbes, MAX_PROP_PROBES};
// The std430 layout stays in the crate: off-world producers state values through `LightBlob`,
// never a row index.
pub(crate) use prop_probes::prop_probe_region_offset;
pub use resolve::WmoCrossfade;
use resolve::{apply_sky_backdrop, setup_lighting, update_time_lighting};
pub(crate) use sh::prop_probe_coeffs;

/// Scene lighting sampled from `Light.dbc` for the time of day. Colours are sRGB 0..1; `sun_dir` is
/// the Bevy-space direction the sun's light travels.
#[derive(Resource, Clone, Copy, Default, PartialEq)]
pub struct WowLighting {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    /// The specular colour, `Light.dbc` IntBand row 9: the terrain and water sun sheen.
    pub spec: [f32; 3],
    pub sun_dir: Vec3,
    /// The visible sun, camera to sun in Bevy space; unlike the lighting sun it rises and sets.
    pub(crate) celestial_dir: Vec3,
    /// Distance fog colour, `Light.dbc` IntBand row 7 raw, applied `GL_LINEAR` in-shader in gamma
    /// space; Bevy's `DistanceFog` blends in linear and would break the gamma invariant.
    pub fog_color: [f32; 3],
    /// The pushed fog pair from [`resolve::scene_fog`]: `end = min(band end, farclip)` and
    /// `start = frac × end` unclamped (`0x6cee61`), negative under storm: the near veil in rain.
    pub(crate) fog_start: f32,
    pub(crate) fog_end: f32,
    /// The interior fog triple (`DNState+0x80/84/88`): the scene fog lerped toward the claimed
    /// WMO's MFOG by the 4 s [`WmoCrossfade`]. Only content tagged as in a room on the camera's
    /// portal chain reads it: the group's surfaces (`0x6b5190`) and liquid (`0x6b62e0`) and the M2s
    /// in it (`0x71c110`); terrain, ADT liquid, the sky and exterior groups keep the scene fog.
    pub(crate) wmo_fog_color: [f32; 3],
    pub(crate) wmo_fog_start: f32,
    pub(crate) wmo_fog_end: f32,
    /// The five sky-dome stops, zenith to horizon (`Atmosphere.sky`, IntBand rows 2-6).
    pub(crate) sky: [[f32; 3]; 5],
    /// Per-kind water tint `[shallow, deep]`: IntBand rows 16/17 (river, lake) and 14/15 (ocean),
    /// raw and area-blended like every band (`0x6d30e0` merges all 18 colour rows per light).
    pub(crate) water_river: [[f32; 3]; 2],
    pub(crate) water_ocean: [[f32; 3]; 2],
    /// Per-kind water alphas `[shallow, deep]`, `LightParams` fields 5-8, blended like the tint.
    pub(crate) water_river_alpha: [f32; 2],
    pub(crate) water_ocean_alpha: [f32; 2],
    /// The FFXGlow composite weight, `LightParams.glow` quantised as the reference packs it.
    pub(crate) glow: f32,
    /// The dawn/dusk sky warp strength `curve(dayfrac) × highlightSky` for `sky.wgsl`'s warp
    /// (`0x6d0f50`): 0, the identity, except at dawn and dusk in flagged zones.
    pub(crate) sky_warp: f32,
    /// The sun disc size multiplier (`0xce8cac`): 1 across midday, 2 at the dawn and dusk horizon.
    pub(crate) sun_disc_scale: f32,
    /// The sun lens-flare day envelope (`0xce9818`): 1 from 07:30 to 19:30, 0 at night.
    pub(crate) sun_flare_dn: f32,
    /// The white moon, camera to moon in Bevy space: up at night, below the horizon by day.
    pub(crate) moon_dir_white: Vec3,
    /// The moon disc size multiplier (`0xce8c8c`): 1 overhead, 1.5 at moonrise and moonset.
    pub(crate) moon_disc_scale: f32,
    /// The moon lens-flare night envelope (`0xce9768`): 0 from 03:15 to 22:45, full after midnight.
    pub(crate) moon_flare_dn: f32,
    /// `moon02`, camera to body in Bevy space: drawn vertex-black, it only occludes the stars.
    pub(crate) moon_dir_02: Vec3,
    /// `moon02`'s size multiplier, the `0xce8c8c` curve on its own phase clock (base ×1.0).
    pub(crate) moon02_disc_scale: f32,
    /// The star field's global alpha (`0xce9a98`): 1 in deep night, 0 all day.
    pub(crate) star_alpha: f32,
    /// The SIDN night fraction (`DNState+0x1ac`, `0xce9a34`) scaling the WMO windows' night glow.
    pub(crate) sidn_night: f32,
    /// The celestial tint (sRGB) the reference broadcasts into the sun and white-moon discs and
    /// glares each frame (`[0xce9c2c]`, `0x6d2260`): IntBand sub-9, the row [`Self::spec`] samples
    /// (the gather `0x6d64d0` swaps slots 8/9; alpha forced 0xFF at `0x6d62e0`). The moon's teal
    /// rim is the dome's night bands through the disc's feathered edge, not this tint.
    pub(crate) celestial_tint: [f32; 3],
    /// Authored cloud density `C`, FloatBand sub-3 with the weather and area blends (`[0xce9c64]`):
    /// the coverage threshold, 0 cloudless to 1 full overcast.
    pub(crate) cloud_density: f32,
    /// The cloud palette `[sun-glow, slope, gbase]`, IntBand sub-10/11/12 (`0x6d64d0`).
    pub(crate) cloud_colors: [[f32; 3]; 3],
    /// The storm blend `bcc = min(1, density·4)` (`0x6d4500`) of the weather global `[0xce9ba0]`,
    /// never `C`: the storm `LightParams` weight, the seed `floor(255·(1−bcc))` of the five body
    /// alphas (`0x6d2c74`) and the cloud glow dim `1 − 0.75·bcc`.
    pub(crate) storm_bcc: f32,
    /// The cloud glow's body (`0x6cfb00`): the sun from about 04:50 to 22:10, else the moon.
    pub(crate) cloud_glow_dir: Vec3,
    /// The cloud glow's day envelope (`0xce9ab8`), times `1 − 0.75·bcc` for the glow intensity.
    pub(crate) cloud_glow_track: f32,
}

impl WowLighting {
    /// Per-kind water swatch endpoints `(shallow_rgb, deep_rgb, shallow_alpha, deep_alpha)`, the
    /// IntBand rows raw: the × 0.711 dim belongs to the sky fill `0x68c250`, not to water. The ADT
    /// swatch is the reference's 64-row byte ramp between them (`0x68a830`, in `liquid.wgsl`);
    /// WMO liquid's opacity is the 256-entry `shallow + d·(deep − shallow)/256` (`0x6b6b60`). One
    /// depth `V` indexes colour and alpha: `clamp(byte/42)` for river and lake (`0x68d790`, opaque
    /// by about 5 yd), `clamp(byte/255)` for ocean (`0x68d690`), both built in `0x68c4c0`.
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

/// The water material's Phong power for the sun sheen: 6 at the reference's water draw, where
/// terrain uses 20.
pub(crate) const WATER_SHININESS: f32 = 6.0;

/// `LightParams.glow` quantised to the byte the reference packs into the composite quad's colour,
/// `floor(g·255)/255` (Elwynn 0.65 → 0.647).
pub(crate) fn quantize_glow(glow: f32) -> f32 {
    (glow * 255.0).floor() / 255.0
}

/// The parsed `Light.dbc` family, resident for the per-frame resample; absent without client data.
#[derive(Resource)]
pub(crate) struct LightSampler(pub(crate) LightCatalog);

/// Where the lighting's time of day comes from, for the debug panel.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
pub enum ClockSource {
    /// Before connect, or without client data: noon.
    #[default]
    Fallback,
    /// Live server game-clock (`SMSG_LOGIN_SETTIMESPEED`, advanced by its timescale).
    Server,
    /// Manually scrubbed in the debug panel.
    Manual,
}

/// The world's time of day, the engine's clock input, written by whatever owns a session clock; a
/// program with no server gets noon.
#[derive(Resource, Debug, Clone, Copy)]
pub struct WorldTime {
    /// Minute of the game day (`0..1440`), for the `Light.dbc` sample, the clock and the log.
    pub minute: u32,
    /// The same, fractional, so the sun and moons glide rather than step each game minute.
    pub minute_f: f32,
    /// The continuous day count for moon02's phase clock (days plus day fraction, unwrapped).
    pub day: f64,
    /// Whether a real clock has landed; `false` means the noon fallback (`ClockSource::Fallback`).
    pub live: bool,
}

impl Default for WorldTime {
    /// Noon, half a day in, not live.
    fn default() -> Self {
        Self {
            minute: 720,
            minute_f: 720.0,
            day: 0.5,
            live: false,
        }
    }
}

/// The game clock the lighting used this frame, for the debug panel's readout.
#[derive(Resource, Default, Clone, Copy)]
pub struct GameClock {
    /// Minute of the game day (`0..1440`) being rendered.
    pub minute: u32,
    pub source: ClockSource,
}

/// [`WowLighting`] and [`WmoCrossfade`] are resolved for the frame after this set; the skybox
/// weight reads the crossfade after it, or the painted sky and the fog disagree for a frame.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LightingResolveSet;

/// Every `Update` reader of the resolved [`WowLighting`] joins this set, ordered after
/// [`LightingResolveSet`]. The resolve runs late (after the wire drain, the weather tick and the
/// submersion verdict), so an unordered reader sees last frame's atmosphere, which shows when it
/// swaps whole at a submersion crossing. A `PostUpdate` reader is already after the resolve.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LightingConsumeSet;

/// Orders [`LightingConsumeSet`] after [`LightingResolveSet`]; the test below calls this same
/// function, so it checks the edge the plugin installs.
pub(crate) fn configure_lighting_sets(app: &mut App) {
    app.configure_sets(Update, LightingConsumeSet.after(LightingResolveSet));
}

/// The lighting subsystem: the light and clock resources and the per-frame resolve.
pub(crate) struct LightingPlugin;

impl Plugin for LightingPlugin {
    fn build(&self, app: &mut App) {
        // Readers of the resolve run after it.
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
                    // The storm blend reads this frame's weather densities.
                    .after(crate::weather::WeatherTick)
                    // This frame's game clock, published in the wire-drain stage (`WorldTime`).
                    .after(crate::schedule::WorldStage::Net)
                    // This frame's submersion verdict, which the sky-pass suppression also reads.
                    .after(crate::liquid::SubmersionVerdict),
            );
        // The shared light buffer, packed after the resolve and uploaded in the render world.
        global_light::register(app);
    }
}

#[cfg(test)]
mod ordering_tests {
    use super::*;

    /// The edge is asserted as an ambiguity, not an observed order: an unordered pair still runs
    /// in some order, so only Bevy's ambiguity check fails when [`configure_lighting_sets`] is
    /// removed or inverted.
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

    /// Every `Update` reader of [`WowLighting`] in this crate joins [`LightingConsumeSet`], checked
    /// by reading the crate's source; `ALLOWED` lists the readers already after the resolve by
    /// schedule, each with its reason. `benilla-app`'s readers are outside this check.
    #[test]
    fn every_update_reader_joins_the_consume_set() {
        /// Readers already after the resolve by schedule: file, reason.
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
                // Membership is a real `.in_set(..)` call, not a comment naming the set.
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
