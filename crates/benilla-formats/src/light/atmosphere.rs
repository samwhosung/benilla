//! The resolved lighting **output type** ([`Atmosphere`]) + the DBC row/field → meaning map.
//!
//! `LightIntBand`/`LightFloatBand` rows and `LightParams` fields are just numbered slots; the
//! constants here name which slot is which (the **vanilla** layout, VERIFIED — see the `light`
//! module header). [`super::LightCatalog::sample`] reads these rows/fields into an [`Atmosphere`].

/// LightIntBand row indices (0-based within a param's 18 rows).
pub(super) const IB_DIFFUSE: u32 = 0;
pub(super) const IB_AMBIENT: u32 = 1;
/// The five sky-gradient stops are rows 2..6: SkyColor0 (zenith) → SkyColor4 (horizon).
pub(super) const IB_SKY0: u32 = 2;
pub(super) const IB_FOG_COLOR: u32 = 7;
pub(super) const IB_SUN_COLOR: u32 = 9;
/// **Cloud layer palette rows** (the band gather `0x6d64d0` stores sub-10/11/12 at color-table
/// slots 9/10/11 = `0xce9c30/34/38`, the three palettes the cloud dome builder `0x6cfb00` reads):
/// sun-glow tint, gradient slope, gradient base.
pub(super) const IB_CLOUD_SUN: u32 = 10;
pub(super) const IB_CLOUD_SLOPE: u32 = 11;
pub(super) const IB_CLOUD_GBASE: u32 = 12;
/// **Dedicated water-surface tint rows.** The from-above depth swatch is a plain 2-endpoint linear
/// lerp of these IntBand colours, RAW (no scale), VERIFIED from WoW.exe `FUN_0068a830` (the body-colour
/// swatch builder reads `gWorldLight+0xe0..+0xec` = these rows; golden-vector-matched to the apitrace
/// swatch ≤1/255 over all 64 rows). Ocean = rows 14/15 (shallow/deep), river/lake = rows 16/17.
/// NOT the sky gradient — the earlier "reflected sky × 0.711 via `FUN_0068c250`" conclusion fingered
/// the WRONG builder (that fills a *separate* symmetric grey edge texture not bound on the water unit).
pub(super) const IB_OCEAN_SHALLOW: u32 = 14;
pub(super) const IB_OCEAN_DEEP: u32 = 15;
pub(super) const IB_RIVER_SHALLOW: u32 = 16;
pub(super) const IB_RIVER_DEEP: u32 = 17;
/// LightFloatBand row indices (within a param's 6 rows).
pub(super) const FB_FOG_END: u32 = 0;
pub(super) const FB_FOG_START_MULT: u32 = 1;
/// **Cloud density `C`** (the scalar gather `0x6d64d0` stores float sub-3 at `0xce9c0c+0x58` =
/// `[0xce9c64]`, the coverage threshold input `T = trunc((1−C)·255)`). Zeroed only in the
/// degenerate no-light-record fallback — hence the 0.0 default.
pub(super) const FB_CLOUD_DENSITY: u32 = 3;

/// LightParams.dbc field index of the **glow** scalar (the FFXGlow/bloom composite weight). VERIFIED
/// from `WoW.exe`: the engine reads glow from record byte
/// **+0x10 = field index 4**, NOT the wiki's labelled +0x0C `m_glow` (which is 0.0 in all 426 vanilla
/// records). Byte-exact cross-check: LightParams 12 (EK default) field 4 = 0.65 → `floor(0.65·255)/255 =
/// 0.647`, matching the apitrace composite weight. Per-zone (13 distinct values, 0.0–1.0 across zones).
pub(super) const LP_GLOW: usize = 4;

/// `LightParams.dbc` field index of the **highlightSky** flag (record byte +0x04 = field index 1).
/// VERIFIED from `WoW.exe`: the sky-dome
/// colour fold `FUN_006d0f50` reads it as `params[0x14] = (float)*(int*)(LightSrc+4)` — an int 0/1
/// flag that **gates the dawn/dusk sky-dome azimuthal warp** (Elwynn/Westfall = 1, Duskwood = 0).
/// Stored as 0.0/1.0.
pub(super) const LP_HIGHLIGHT: usize = 1;

/// `LightParams.dbc` field index of **lightSkyboxID** (record byte +0x08 = field index 2) — the
/// `LightSkybox.dbc` row whose model replaces the whole celestial pass while it is active.
///
/// **In 5875 this field is the ghost sky and nothing else.** The DBC skybox slot `[0xce9bb4]` is
/// filled inside the colour-table builder `0x6d2260` at `0x6d26cb` **gated on `[0xce9bb0] != -1`**
/// — the ghost override cell, written only by `0x6d4620` off the `PLAYER_FLAGS` ghost bit.
/// The shipped data agrees exactly: of 426 `LightParams` rows only **5** carry a non-zero id, all
/// of them **3 = `DeathClouds.mdx`**, and all 374 `Light.dbc` rows reach one through param slot
/// **4** alone (slots 0–3 are uniformly 0). So there is no live non-ghost skybox to miss here.
pub(super) const LP_SKYBOX: usize = 2;

/// `LightParams.dbc` field indices of the **water-blend alphas** — the from-above swatch's depth-alpha
/// ramp endpoints (`swatch.a = lerp(shallow, deep, V)`; the runtime reads these into `gWorldLight+0x114..`
/// via `FUN_006b6b60`). Layout follows the glow shift: glow is field 4 (+0x10), so these are fields 5–8
/// (+0x14..+0x20). Cross-check: LP 26 (Stranglethorn) `waterShallow` ≈ 0.85 matches the WoW.21 apitrace
/// swatch alpha 216/255, and the area blend resolves LP 26 whole at that river (test
/// `light_area_blend_order.rs`). If a future file disagrees, re-check the offset (cf. the glow
/// +0x0C-vs-+0x10 trap).
pub(super) const LP_WATER_SHALLOW_ALPHA: usize = 5;
pub(super) const LP_WATER_DEEP_ALPHA: usize = 6;
pub(super) const LP_OCEAN_SHALLOW_ALPHA: usize = 7;
pub(super) const LP_OCEAN_DEEP_ALPHA: usize = 8;

/// The reference's answer for a band row that exists but carries **no keyframes**
/// (`numKeys == 0`) — the case 973 of the 7668 shipped `LightIntBand` rows and 132 of the 2556
/// `LightFloatBand` rows are in, and the one this crate used to answer with an invented
/// "neutral daytime" constant.
///
/// In the reference:
/// the colour evaluator `0x6d62e0` early-outs *before* touching the key/value arrays — key-count
/// load `0x6d62ec`, guard `0x6d62ef`/`0x6d62f4`, store `0x6d62f6 mov [edi], 0xff000000` — an
/// immediate, so the slot is opaque **black**: not zero-alpha, not stale, not skipped, and the
/// driver's copy into the colour table (`0x6d6643`/`0x6d664c` for sub-12) is unconditional.
/// The scalar evaluator does the same at `0x6d6489`/`0x6d648e` with `fld [0x7ffd74]` = `+0.0f`.
/// We keep only the RGB; the forced 0xff alpha has no consumer on this side.
///
/// This is NOT [`Atmosphere::DEFAULT`]'s job. That constant answers "there is no light record at
/// all", which decision 0987 showed is unreachable in world (a rowless map takes `Light.dbc`
/// record 1); the reference's own whole-record default is the pre-first-map-load white table at
/// `0x6d22ce`. Using a *daytime palette* to fill a *single missing row* was the actual defect
/// behind report B90: `LightParams` 36 (Blasted Lands) leaves the cloud gradient base
/// (`IB_CLOUD_GBASE`) unkeyed, so the sky took a pale grey where the data says black — under a
/// zone that authors cloud density 0.85, that is the whole sky.
pub const ZERO_KEY_COLOR: [f32; 3] = [0.0, 0.0, 0.0];

/// The scalar half of [`ZERO_KEY_COLOR`] — a keyless `LightFloatBand` row commits `+0.0f`.
pub const ZERO_KEY_SCALAR: f32 = 0.0;

/// Sampled atmosphere for a position + time of day. Distances are yards; colors are sRGB 0..1
/// (feed straight into Bevy `Color::srgb`).
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    /// Fog-end band value (FloatBand sub0) in **yards** (`raw/36` — Elwynn clear 18000→500, storm
    /// 10000→278). Byte-VERIFIED: the client applies the ×1/36 ONCE at DBC load
    /// (`0x6d6090`, selector `rowIndex % 6 == 0` — the sub-0 distance band only).
    /// The consumer pushes `min(fog_end, farclip)` (`dn_scene_fog 0x6cee30`), so the wall tracks
    /// the view-distance slider only below the zone's value. Lerped verbatim across weather/area
    /// blends, like the binary's `0x6d69d8`.
    pub fog_end: f32,
    /// Fog-start **fraction** of the pushed fog end (FloatBand sub1), **raw and unclamped** — storm
    /// params go negative (Elwynn storm −0.5), which is the mechanism behind the constant near veil:
    /// `GL_FOG_START = frac × GL_FOG_END` with no clamp (`0x6cee6d`), so a negative start puts
    /// `1 − 1/(1−frac)` fog at the eye. Lerped verbatim, like the binary's `0x6d69ed`.
    pub fog_start_frac: f32,
    pub fog_color: [f32; 3],
    /// Global diffuse (the sun's warm direct tint; orange in daylight).
    pub sun_diffuse: [f32; 3],
    /// Sun disc / halo color (the cleaner near-white we drive the directional light with).
    pub sun_color: [f32; 3],
    pub ambient: [f32; 3],
    /// The five sky-dome gradient stops, zenith → horizon (`SkyColor0..4`): navy zenith, the strong
    /// midday blue, light blue, pale cyan, pale horizon. All five matter — using only every other one
    /// drops the saturated blue and washes the sky out.
    pub sky: [[f32; 3]; 5],
    /// **River/lake water-surface tint** `[shallow, deep]` — IntBand rows 16/17, RAW. The from-above
    /// depth swatch is a 2-endpoint linear lerp of these by the per-vertex depth `V` (built in
    /// `benilla-formats::liquid` — river/lake `V = clamp(byte/42)`) (VERIFIED `WoW.exe FUN_0068a830`; see
    /// [`IB_RIVER_SHALLOW`]). No `× 0.711` — these are the raw water rows.
    pub water_river: [[f32; 3]; 2],
    /// **Ocean water-surface tint** `[shallow, deep]` — IntBand rows 14/15, RAW (same builder as
    /// [`Atmosphere::water_river`], `param_6 = 0`). Ocean uses a gentler depth-byte ramp than rivers
    /// (≈1.7 vs ≈8.5 byte/yd — the data's own slope), so its deep colour appears far from shore.
    pub water_ocean: [[f32; 3]; 2],
    /// **River/lake water-blend alphas** `[shallow, deep]` — `LightParams.waterShallowAlpha`/
    /// `waterDeepAlpha` (fields [`LP_WATER_SHALLOW_ALPHA`]/[`LP_WATER_DEEP_ALPHA`]). The depth swatch's
    /// alpha ramp endpoints (`swatch.a = lerp(shallow, deep, V)`). Per-zone: Elwynn/Loch Modan shallow
    /// ≈0.5, STV ≈0.85 (its shallows read pale from the colour, not transparency). Deep ≈1.0 everywhere.
    pub water_river_alpha: [f32; 2],
    /// **Ocean water-blend alphas** `[shallow, deep]` — `LightParams.oceanShallowAlpha`/`oceanDeepAlpha`
    /// (fields [`LP_OCEAN_SHALLOW_ALPHA`]/[`LP_OCEAN_DEEP_ALPHA`]). Shallow ≈0.75, deep ≈1.0.
    pub water_ocean_alpha: [f32; 2],
    /// **FFXGlow / bloom composite weight** for this zone — `LightParams.glow` (field [`LP_GLOW`]),
    /// raw `0.0..1.0`. The composite is `scene + glow·bloom²`; the real client quantises it to a byte
    /// (`floor(glow·255)/255`) when it packs the composite-quad colour. Per-zone (Elwynn 0.65, Duskwood
    /// 0.50, …); fallback 0.5.
    pub glow: f32,
    /// **highlightSky** flag for this zone (`LightParams.highlightSky`, field [`LP_HIGHLIGHT`]), as
    /// 0.0/1.0. Gates the dawn/dusk sky-dome azimuthal warp (`FUN_006d0f50`): the warp strength is
    /// `dawn_dusk_curve(dayfrac) × highlight_sky`, so highlightSky=0 zones (e.g. Duskwood) get no
    /// warp at all. Per-zone, static (a LightParams record field, not a time band); fallback 0.0.
    pub highlight_sky: f32,
    /// **Cloud density `C`** (FloatBand sub-3, [`FB_CLOUD_DENSITY`]) — the coverage threshold
    /// input `T = trunc((1−C)·255)`: 0 = a cloudless sky (the field quantizes to zero
    /// everywhere), 1 = full overcast potential. Per-zone, per-time, and lerped across the
    /// weather/area blends like every band.
    pub cloud_density: f32,
    /// **Cloud palette** `[sun-glow, slope, gbase]` (IntBand sub-10/11/12) — the visible cloud
    /// dome's colors: per-texel `RGB = slope·p + gbase` plus a sun-aligned glow tinted by the
    /// sun row (the cloud dome builder `0x6cfb00`).
    pub cloud_colors: [[f32; 3]; 3],
}

impl Atmosphere {
    /// Neutral daytime fallback for **no lighting data at all** — a chain with no `Light.dbc`, or a
    /// catalog that failed to load. It is not reachable from a shipped install: every map resolves a
    /// record and every record's 24 band rows exist, so a *row* with no keys takes
    /// [`ZERO_KEY_COLOR`]/[`ZERO_KEY_SCALAR`] instead — this constant is never spliced into one.
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
        // Daytime fallback water tints (used only when the DBC rows are absent): green shore → teal
        // deep (river/lake), teal → deep-blue (ocean). Real values come from IntBand rows 16/17 / 14/15.
        water_river: [[0.27, 0.33, 0.14], [0.19, 0.31, 0.32]],
        water_ocean: [[0.10, 0.29, 0.34], [0.04, 0.16, 0.28]],
        water_river_alpha: [0.5, 1.0],
        water_ocean_alpha: [0.75, 1.0],
        glow: 0.5,          // the binary's "no active world-light" fallback
        highlight_sky: 0.0, // no dawn/dusk dome warp unless a zone sets the flag
        // The degenerate no-light-record fallback force-zeroes C (`0x6d22ec`) — no clouds.
        cloud_density: 0.0,
        cloud_colors: [[1.0, 0.98, 0.9], [0.35, 0.38, 0.42], [0.75, 0.78, 0.82]],
    };

    /// Linearly blend two atmospheres (`t=0`→`self`, `t=1`→`other`) — for smooth weather/time fades.
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
