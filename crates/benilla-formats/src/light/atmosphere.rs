//! The resolved lighting type, [`Atmosphere`], and the names of the `LightIntBand` and
//! `LightFloatBand` rows and `LightParams` fields it is read from, in the vanilla layout.

/// `LightIntBand` row indices, 0-based within a param's 18 rows.
pub(super) const IB_DIFFUSE: u32 = 0;
pub(super) const IB_AMBIENT: u32 = 1;
/// The five sky-gradient stops, rows 2..6, zenith to horizon.
pub(super) const IB_SKY0: u32 = 2;
pub(super) const IB_FOG_COLOR: u32 = 7;
pub(super) const IB_SUN_COLOR: u32 = 9;
/// The cloud palette rows, sun-glow tint, gradient slope and gradient base: gathered by `0x6d64d0`
/// into `0xce9c30/34/38` for the cloud dome builder `0x6cfb00`.
pub(super) const IB_CLOUD_SUN: u32 = 10;
pub(super) const IB_CLOUD_SLOPE: u32 = 11;
pub(super) const IB_CLOUD_GBASE: u32 = 12;
/// The water-surface tint rows, ocean 14/15 and river/lake 16/17 (shallow, deep): the reference's
/// swatch builder `0x68a830` lerps them raw (`gWorldLight+0xe0..+0xec`), not the sky gradient.
pub(super) const IB_OCEAN_SHALLOW: u32 = 14;
pub(super) const IB_OCEAN_DEEP: u32 = 15;
pub(super) const IB_RIVER_SHALLOW: u32 = 16;
pub(super) const IB_RIVER_DEEP: u32 = 17;
/// `LightFloatBand` row indices, within a param's 6 rows.
pub(super) const FB_FOG_END: u32 = 0;
pub(super) const FB_FOG_START_MULT: u32 = 1;
/// Cloud density `C`, gathered by `0x6d64d0` into `[0xce9c64]`: the coverage threshold is
/// `T = trunc((1 − C)·255)`.
pub(super) const FB_CLOUD_DENSITY: u32 = 3;

/// `LightParams.dbc` field of the glow, the bloom composite weight: the reference reads record byte
/// `+0x10`, field 4, not the `+0x0C` the wiki labels `m_glow`, which is 0 in every vanilla row.
pub(super) const LP_GLOW: usize = 4;

/// `LightParams.dbc` field of the highlightSky flag, record byte `+0x04`: an int 0/1 the sky-dome
/// fold `0x6d0f50` reads to gate the dawn/dusk azimuthal warp. Stored as 0.0/1.0.
pub(super) const LP_HIGHLIGHT: usize = 1;

/// `LightParams.dbc` field of lightSkyboxID, record byte `+0x08`: the `LightSkybox.dbc` model that
/// replaces the celestial pass. The reference fills `[0xce9bb4]` (`0x6d2260` at `0x6d26cb`) only
/// while `0x6d4620` has set the ghost override `[0xce9bb0]`; the 5 non-zero rows are all
/// `DeathClouds.mdx`.
pub(super) const LP_SKYBOX: usize = 2;

/// `LightParams.dbc` fields 5 to 8, after the glow: the water-blend alphas, the depth-alpha
/// endpoints of the swatch (`swatch.a = lerp(shallow, deep, V)`, read by `0x6b6b60`).
pub(super) const LP_WATER_SHALLOW_ALPHA: usize = 5;
pub(super) const LP_WATER_DEEP_ALPHA: usize = 6;
pub(super) const LP_OCEAN_SHALLOW_ALPHA: usize = 7;
pub(super) const LP_OCEAN_DEEP_ALPHA: usize = 8;

/// A band row with no keyframes (973 of the 7668 `LightIntBand` rows) commits opaque black: the
/// reference's colour evaluator `0x6d62e0` stores the immediate `0xff000000` at `0x6d62f6` before
/// reading any key, and the copy into the colour table (`0x6d6643`) is unconditional.
pub const ZERO_KEY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// A keyless `LightFloatBand` row commits `+0.0f` (`0x6d6489`, `fld [0x7ffd74]`).
pub const ZERO_KEY_SCALAR: f32 = 0.0;

/// The sampled atmosphere at a position and time of day: distances in yards, colours sRGB 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    /// Fog end in yards, FloatBand sub 0 divided by 36 once at load (`0x6d6090`). The consumer
    /// pushes `min(fog_end, farclip)` (`0x6cee30`); blends lerp it raw (`0x6d69d8`).
    pub fog_end: f32,
    /// Fog start as a fraction of the pushed end, FloatBand sub 1, unclamped (`0x6cee6d`), so a
    /// storm's negative value puts fog at the eye; lerped raw (`0x6d69ed`).
    pub fog_start_frac: f32,
    pub fog_color: [f32; 3],
    /// Global diffuse, the sun's direct tint.
    pub sun_diffuse: [f32; 3],
    /// IntBand row 9: the sun disc and halo tint, also the specular colour.
    pub sun_color: [f32; 3],
    pub ambient: [f32; 3],
    /// The five sky-dome gradient stops, zenith to horizon (`SkyColor0..4`); all five are used.
    pub sky: [[f32; 3]; 5],
    /// River/lake tint `[shallow, deep]`, IntBand rows 16/17 raw, lerped by depth (`0x68a830`).
    pub water_river: [[f32; 3]; 2],
    /// Ocean tint `[shallow, deep]`, IntBand rows 14/15 raw, from the same builder.
    pub water_ocean: [[f32; 3]; 2],
    /// River/lake blend alphas `[shallow, deep]`, `LightParams.waterShallowAlpha`/`waterDeepAlpha`.
    pub water_river_alpha: [f32; 2],
    /// Ocean blend alphas `[shallow, deep]`, `LightParams.oceanShallowAlpha`/`oceanDeepAlpha`.
    pub water_ocean_alpha: [f32; 2],
    /// The FFXGlow composite weight, `LightParams.glow`, raw 0..1: the composite is
    /// `scene + glow·bloom²`, and the reference quantises glow to `floor(glow·255)/255`.
    pub glow: f32,
    /// `LightParams.highlightSky` as 0.0/1.0, scaling the dawn/dusk dome warp (`0x6d0f50`).
    pub highlight_sky: f32,
    /// Cloud density `C`, FloatBand sub 3: the coverage threshold is `T = trunc((1 − C)·255)`.
    pub cloud_density: f32,
    /// Cloud palette `[sun-glow, slope, gbase]`, IntBand sub 10/11/12: per texel
    /// `RGB = slope·p + gbase` plus the sun-aligned glow (`0x6cfb00`).
    pub cloud_colors: [[f32; 3]; 3],
}

impl Atmosphere {
    /// A daytime fallback for no lighting data, unreachable from a shipped install (a keyless row
    /// takes [`ZERO_KEY_COLOR`]); the reference's pre-load table is white (`0x6d22ce`).
    pub const DEFAULT: Atmosphere = Atmosphere {
        fog_end: 1000.0,
        fog_start_frac: 0.4,
        fog_color: [0.55, 0.72, 0.92],
        sun_diffuse: [1.0, 0.96, 0.86],
        sun_color: [1.0, 1.0, 0.9],
        ambient: [0.45, 0.52, 0.65],
        sky: [
            [0.30, 0.50, 0.85],
            [0.35, 0.58, 0.88],
            [0.55, 0.72, 0.92],
            [0.68, 0.80, 0.93],
            [0.78, 0.86, 0.95],
        ],
        water_river: [[0.27, 0.33, 0.14], [0.19, 0.31, 0.32]],
        water_ocean: [[0.10, 0.29, 0.34], [0.04, 0.16, 0.28]],
        water_river_alpha: [0.5, 1.0],
        water_ocean_alpha: [0.75, 1.0],
        glow: 0.5,          // the binary's "no active world-light" fallback
        highlight_sky: 0.0, // no dawn/dusk dome warp unless a zone sets the flag
        // The reference's no-light-record fallback zeroes `C` (`0x6d22ec`).
        cloud_density: 0.0,
        cloud_colors: [[1.0, 0.98, 0.9], [0.35, 0.38, 0.42], [0.75, 0.78, 0.82]],
    };

    /// Linear blend: `t = 0` is `self`, `t = 1` is `other`.
    pub fn lerp(&self, other: &Atmosphere, t: f32) -> Atmosphere {
        let f = |a: f32, b: f32| a + (b - a) * t;
        let c = |a: [f32; 3], b: [f32; 3]| [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])];
        Atmosphere {
            fog_end: f(self.fog_end, other.fog_end),
            fog_start_frac: f(self.fog_start_frac, other.fog_start_frac),
            fog_color: c(self.fog_color, other.fog_color),
            sun_diffuse: c(self.sun_diffuse, other.sun_diffuse),
            sun_color: c(self.sun_color, other.sun_color),
            ambient: c(self.ambient, other.ambient),
            sky: std::array::from_fn(|i| c(self.sky[i], other.sky[i])),
            water_river: std::array::from_fn(|i| c(self.water_river[i], other.water_river[i])),
            water_ocean: std::array::from_fn(|i| c(self.water_ocean[i], other.water_ocean[i])),
            water_river_alpha: std::array::from_fn(|i| {
                f(self.water_river_alpha[i], other.water_river_alpha[i])
            }),
            water_ocean_alpha: std::array::from_fn(|i| {
                f(self.water_ocean_alpha[i], other.water_ocean_alpha[i])
            }),
            glow: f(self.glow, other.glow),
            highlight_sky: f(self.highlight_sky, other.highlight_sky),
            cloud_density: f(self.cloud_density, other.cloud_density),
            cloud_colors: std::array::from_fn(|i| c(self.cloud_colors[i], other.cloud_colors[i])),
        }
    }
}
