//! Authentic vanilla atmosphere from the client's lighting DBCs — so fog distance/color and
//! sun/ambient tint come from Blizzard's authored data, not invented constants.
//!
//! Pipeline: [`Light.dbc`] gives a sphere per area (a world
//! position via [`dbc_to_world`] + a radius in yards) and a `LightParams` id per weather condition;
//! that id indexes the band tables — [`LightIntBand.dbc`] (18 color rows/param) and
//! [`LightFloatBand.dbc`] (6 float rows/param) — each row a value over time-of-day. We sample the
//! **clear-weather** param at a given time and return the fog/sun/ambient/sky values.
//!
//! **Verified against build 5875** (raw-byte decode cross-checked with wowdev.wiki, 2026-05-25):
//! headers are Light = 12 fields/48 B (5 param refs), both bands = 34 fields/136 B
//! (`ID, num, time[16], value[16]`). LightIntBand = 7668 = 426×18 rows and LightFloatBand =
//! 2556 = 426×6 rows — i.e. exactly 18 int / 6 float rows per LightParams, confirming the band row
//! id for param `P` (1-based), band index `b`: `(P-1)*per + b + 1` (== wiki `P*18−17` / `P*6−5`).
//! The int-row→meaning map is the **vanilla** layout (rows 2–6 = the 5 sky stops, 7 = fog, 9 = sun),
//! confirmed by the data itself (row 8 is flat gray = shadow-opacity slot, not a sun). The
//! decoded values match our parser exactly (e.g. Northshire/global-light param 12 noon SkyMiddle
//! = `(58,162,207)`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

mod atmosphere; // the resolved `Atmosphere` output type + the DBC row/field → meaning constants
mod tables; // raw LightIntBand/FloatBand decode + time-of-day interpolation

pub use atmosphere::Atmosphere;
use atmosphere::{
    FB_CLOUD_DENSITY, FB_FOG_END, FB_FOG_START_MULT, IB_AMBIENT, IB_CLOUD_GBASE, IB_CLOUD_SLOPE,
    IB_CLOUD_SUN, IB_DIFFUSE, IB_FOG_COLOR, IB_OCEAN_DEEP, IB_OCEAN_SHALLOW, IB_RIVER_DEEP,
    IB_RIVER_SHALLOW, IB_SKY0, IB_SUN_COLOR, LP_GLOW, LP_HIGHLIGHT, LP_OCEAN_DEEP_ALPHA,
    LP_OCEAN_SHALLOW_ALPHA, LP_SKYBOX, LP_WATER_DEEP_ALPHA, LP_WATER_SHALLOW_ALPHA,
};
pub use atmosphere::{ZERO_KEY_COLOR, ZERO_KEY_SCALAR};
use tables::{band_schema, load_bands, load_float_bands, sample_color, sample_float, Band, DAY};

const LIGHT: &str = "DBFilesClient\\Light.dbc";
const INT_BAND: &str = "DBFilesClient\\LightIntBand.dbc";
const FLOAT_BAND: &str = "DBFilesClient\\LightFloatBand.dbc";
const LIGHT_PARAMS: &str = "DBFilesClient\\LightParams.dbc";
const LIGHT_SKYBOX: &str = "DBFilesClient\\LightSkybox.dbc";

/// World-unit scale factor on Light.dbc positions/radii (stored as yards × 36).
const POS_SCALE: f32 = 36.0;

/// Light.dbc weather param slots: [0]=clear, [1]=clear-underwater, [2]=storm, [3]=storm-underwater,
/// [4]=death.
const SLOT_CLEAR: usize = 0;
/// `[1]` clear-underwater: the denser fog + cooler light the client switches to when the eye is
/// submerged in **water** (VERIFIED apitrace WoW.18: ~38 yd teal fog + blue ambient/diffuse vs the
/// ~1284 yd surface fog). Same band-row meanings as the clear slot, just a different `LightParams` id.
/// Reached only by [`Submersion::Water`] — magma and slime never consult a zone slot at all
/// (see [`PARAM_MAGMA`]).
const SLOT_CLEAR_UNDERWATER: usize = 1;
const SLOT_STORM: usize = 2;
const SLOT_STORM_UNDERWATER: usize = 3;
/// The ghost-world profile: the client's ghost watcher `0x5de9c0`
/// writes slot index 4 into the active-slot global `[0xce9bb0]` and the day/night color-table
/// rebuild consumes it INSTANTLY (per-frame rebuild, no blend). Selected while `ghost`, taking
/// priority over weather/underwater exactly as the single global slot does.
const SLOT_DEATH: usize = 4;

/// The **fixed global `LightParams` rows** the reference uses for submersion in the two fullbright
/// liquids — byte-VERIFIED (`0x6d2371` routes magma and slime to their own leg *before* the slot
/// selection: magma at `0x6d23e1` reads row 7, slime at `0x6d239e` reads row 6, each guarded on the
/// table's `maxId`). These are **zone-independent** and nothing in `Light.dbc` references them, which
/// is why no position-keyed dump ever surfaced them: submersion in lava does not read the zone's
/// underwater slot at all.
///
/// The shipped rows are exactly the vanilla look, which is the cross-check that earned this:
/// magma fog `[200,52,0]` at **27 yd** with start fraction **−2.0** (67 % fiery orange at the eye),
/// slime fog `[0,255,0]` at **50 yd** with **−1.0** (50 % green). There is no separate magma/slime
/// fog constant anywhere in the binary (VERIFIED negative) — these two rows *are* the distinct look.
const PARAM_SLIME: u32 = 6;
const PARAM_MAGMA: u32 = 7;

/// The ocean depth ramp's floor — `0x81162c` = −30.0f. Below this the factors hold flat.
const OCEAN_RAMP_FLOOR: f32 = -30.0;
/// The ramp's reciprocal, **as the binary stores it** — the f32 at `0x811628`, bit pattern
/// `0xbd088889`.
///
/// Written from the bits rather than as a decimal on purpose. The decimal form
/// (`−0.0333333351`) carries more digits than an f32 holds, so spelling it out is both a clippy
/// `excessive_precision` error and a small lie about what is in the file; and rounding it to
/// `−0.033_333_335` would read as a value someone chose. `from_bits` says the true thing: this
/// constant is a bit pattern lifted out of the image. (It happens to equal `-1.0f32 / 30.0` — see
/// [`Submersion::ocean_depth_factors`] for why that is not the point.)
const OCEAN_RAMP_RECIP: f32 = f32::from_bits(0xbd08_8889);

/// What the camera eye is submerged in, if anything — the atmosphere selector. Water and ocean pick
/// the zone's *underwater slot*; magma and slime replace the whole area blend with a fixed global row
/// (see [`PARAM_MAGMA`]/[`PARAM_SLIME`]).
///
/// **Five states, because the reference has five.** `[0xc7f288] ∈ {0xf dry, 0 water, 1 ocean,
/// 2 magma, 3 slime}` — and ocean is not a synonym for water. It is the only kind that runs the
/// depth ramp ([`Submersion::ocean_depth_factors`]), which is gated on the **full unmasked dword
/// `== 1`** at `0x6d2821` (not the nibble — `0x11` would pass a nibble test and must not). Every
/// other consumer image-wide treats the two alike, so this enum forks in exactly one place, which
/// is the shape of the finding.
///
/// Collapsing them, as we did until decision 1829, was not a small approximation: ocean is
/// **96.8% of every wet sub-tile** in the shipped corpus (14 069 166 of 14 528 712). The state we
/// were not modelling was essentially all open water.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Submersion {
    /// Dry — the ordinary clear/storm slot.
    #[default]
    Dry,
    /// Under still water or rapids: the zone's own underwater `LightParams`, area-blended as usual.
    Water,
    /// Under **ocean**: the same underwater slot as [`Self::Water`], plus the depth ramp. Comes
    /// only from the ADT MCLQ per-subtile flags (`(b & 0x0f) & 3 == 1`); a WMO cannot author it.
    Ocean,
    /// Under magma: the fixed global row, verbatim.
    Magma,
    /// Under slime: the fixed global row, verbatim.
    Slime,
}

impl Submersion {
    /// Submerged in **any** liquid.
    pub fn any(self) -> bool {
        self != Submersion::Dry
    }

    /// Submerged specifically in a WATER kind (water / ocean / rapids) — the predicate for the
    /// consumers that are about water rather than about liquid: the `UnderWaterLoop` ambience bed and
    /// the "Underwater" reverb-preference column.
    ///
    /// Deliberately NOT [`Self::any`]: whether the reference swaps the submerged ambience bed and the
    /// reverb preset for **lava and slime** as well is unverified, and widening it silently on the way
    /// past would be inventing a behaviour. The atmosphere is verified per kind; the audio is not.
    ///
    /// [`Submersion::Ocean`] belongs here with [`Submersion::Water`]: the split exists for the depth
    /// ramp alone, and every other consumer — the underwater slot, the ambience bed, the reverb
    /// column, the drift cloud's cell set — treats a sea exactly like a lake.
    pub fn is_water(self) -> bool {
        matches!(self, Submersion::Water | Submersion::Ocean)
    }

    /// The **ocean depth ramp** — the one thing ocean does that water does not (the ramp at
    /// `0x6d2823`, behind the ocean gate `0x6d2821`).
    ///
    /// Returns `(fac1, fac2)`, the multipliers on the committed light record's first and second
    /// colour triples — `DNState+0x178 × fac1` and `DNState+0x174 × fac2`. `fac1` runs `1.0 → 0.5`
    /// and `fac2` runs `1.0 → 0.75` over the first 30 yards of depth, so the fill darkens twice as
    /// fast as the direct term and a deep sea goes flat as well as dim.
    ///
    /// **A multiply by the stored constant, not a divide by 30.** A bit-exact reimplementation must
    /// use the stored f32, not `t/30.0` — and not because of the operand: `0xbd088889` **is** the
    /// nearest f32 to −1/30, bit for bit. What differs is the *operation* — `t · recip` and
    /// `t / 30.0` round differently, and they disagree inside the ramp's own range (at `t = −29` by
    /// 6e−8 in `k`). So the thing to transcribe is the multiply, and the constant is incidental.
    ///
    /// The boundaries are exact, not near-misses: `k` reaches exactly `0.0` at −30 yd, so `fac1`
    /// bottoms at exactly 0.5 and `fac2` at exactly 0.75.
    ///
    /// **Not the fog colour.** The binary commits this ramp to three slots, but the fog-colour one
    /// (`0x6d28b0` → `DNState+0x70`) is dead: `0x66ff60` calls the tint and *then* calls `0x6cee30`,
    /// whose `0x6cee76` unconditionally overwrites that slot from the band-derived colour. The
    /// underwater murk colour reaches the screen through the LightParams slot switch we already
    /// have; this ramp darkens two light bands and nothing else.
    pub fn ocean_depth_factors(self, eye_z: f32) -> Option<(f32, f32)> {
        if self != Submersion::Ocean {
            return None;
        }
        let t = eye_z.clamp(OCEAN_RAMP_FLOOR, 0.0);
        let k = 1.0 - t * OCEAN_RAMP_RECIP;
        Some(((k + 1.0) * 0.5, k * 0.25 + 0.75))
    }

    /// The fixed global `LightParams` row this submersion replaces the whole blend with, if any.
    /// `None` for the dry and water-kind states, which go through the slot selection.
    fn fixed_param(self) -> Option<u32> {
        match self {
            Submersion::Magma => Some(PARAM_MAGMA),
            Submersion::Slime => Some(PARAM_SLIME),
            Submersion::Dry | Submersion::Water | Submersion::Ocean => None,
        }
    }
}

/// Pick the `LightParams` slot for the current ghost/weather/submersion state (see `SLOT_*`).
/// Ghost wins outright — the client keeps ONE active slot, and the ghost watcher owns it while
/// the flag is up. Only a **water kind** reaches the underwater slots (water or ocean alike — the
/// slot switch does not distinguish them); the fullbright liquids never get here (their fixed row
/// short-circuits the blend).
fn weather_slot(ghost: bool, stormy: bool, underwater: bool) -> usize {
    if ghost {
        return SLOT_DEATH;
    }
    match (stormy, underwater) {
        (false, false) => SLOT_CLEAR,
        (false, true) => SLOT_CLEAR_UNDERWATER,
        (true, false) => SLOT_STORM,
        (true, true) => SLOT_STORM_UNDERWATER,
    }
}

/// **The zero-match fallback record**, addressed by its `Light.dbc` **ID column** (not its row
/// index).
///
/// The client builds a per-map light array in `dn_light_array_build 0x6d6170` (`0x6d61a9 cmp
/// [row+4], mapId`), and when **no row matches the map at all** its tail (`0x6d62b2`–`0x6d62c9`)
/// writes `idMap[1]` — this record — into slot 0, unchecked. `dn_light_select 0x6d2d00` then
/// no-ops (count ≤ 1 ⇒ empty blend heap) and the colour-table build commits that one record whole.
/// The count is *seeded* at 1 (`0x6d6188 mov edi,1`), so the no-data white table (fog end 1e10) is
/// the before-first-load state and is unreachable in world.
///
/// Shipped row 1 is the Azeroth global: map 0, `(0,0,0)`, falloff 0/0, params
/// `(12, 13, 10, 11, 4)`. So a map with no `Light.dbc` row of its own — **Deeprun Tram (369) is the
/// one that matters, and it is the only shipped map reachable in play with none** — renders under
/// LightParams 12's ordinary six-key day curve, not under any invented constant.
const FALLBACK_LIGHT_ID: u32 = 1;

struct Light {
    /// The row's own `Light.dbc` **ID column** — not its row index. Carried because the client's
    /// zero-match fallback addresses one specific record *by id* ([`FALLBACK_LIGHT_ID`]).
    id: u32,
    map: u32,
    /// WoW net-protocol **world** coords (yards), converted from the DBC's mirrored/axis-swapped
    /// inch frame via [`dbc_to_world`]. Meaningless when `global` (the sentinel was `(0,0,0)`).
    pos: [f32; 3],
    /// Distance (yards) at which this light is at full strength; inside it `blendAlpha = 1`.
    falloff_start: f32,
    falloff_end: f32,
    /// The continent's `(0,0,0)` fallback light — matches everywhere, lowest priority.
    global: bool,
    /// LightParams id per weather slot (see `SLOT_*`); 0 = unset.
    params: [u32; 5],
}

/// Convert a `Light.dbc` position to WoW net-protocol **world** coords (yards). The DBC stores
/// continent coords as **inches** (yards × 36) in a mirrored, axis-swapped frame, so it is *not* a
/// plain `/36`. Per wowdev.wiki `LightRec::ConvertDBToGameCoords` (the world-coord correction):
/// `world.x = 17066.666 − z/36`, `world.y = 17066.666 − x/36`, `world.z = y/36`.
/// `17066.666` = half the 34133⅓-yard map (32 tiles × 533⅓). Verified against decoded entries
/// (e.g. Light #51 → `(-8480, 548, 81)`).
fn dbc_to_world(x: f32, y: f32, z: f32) -> [f32; 3] {
    const MAP_HALF: f32 = 17066.666;
    [
        MAP_HALF - z / POS_SCALE,
        MAP_HALF - x / POS_SCALE,
        y / POS_SCALE,
    ]
}

/// Parsed lighting tables; query with [`LightCatalog::sample`].
pub struct LightCatalog {
    lights: Vec<Light>,
    int_bands: HashMap<u32, Band<u32>>,
    float_bands: HashMap<u32, Band<f32>>,
    /// `LightParams.dbc` id → glow scalar (field [`LP_GLOW`]); the per-zone bloom composite weight.
    light_params_glow: HashMap<u32, f32>,
    /// `LightParams.dbc` id → `highlightSky` flag (field [`LP_HIGHLIGHT`]) as 0.0/1.0; gates the
    /// per-zone dawn/dusk sky-dome warp.
    light_params_highlight: HashMap<u32, f32>,
    /// `LightParams.dbc` id → water-blend alphas `[waterShallow, waterDeep, oceanShallow, oceanDeep]`
    /// (fields 5–8). The from-above swatch's per-zone depth-alpha ramp endpoints.
    light_params_water_alpha: HashMap<u32, [f32; 4]>,
    /// `LightParams.dbc` id → `lightSkyboxID` (field [`LP_SKYBOX`]), kept only for the non-zero rows
    /// — 5 of 426 in the shipped chain, every one of them the ghost sky.
    light_params_skybox: HashMap<u32, u32>,
    /// `LightSkybox.dbc` id → the model it names, normalised to its chain path (`crate::models`'
    /// `model_path`: lowercased, `.mdx` → the physical `.m2`) so both skybox sources hand the render
    /// lane the same spelling and one model can never build twice under two names.
    ///
    /// Six rows in 5875; only id 3 (`DeathClouds.mdx`) is reachable from `LightParams` — the other
    /// five are the WMO **MOSB** skyboxes, which this table does not drive (`crate::skybox`).
    skyboxes: HashMap<u32, String>,
}

fn light_schema() -> Schema {
    let mut s = Schema::new("Light");
    for (n, t) in [
        ("ID", FieldType::UInt32),
        ("continent", FieldType::UInt32),
        ("x", FieldType::Float32),
        ("y", FieldType::Float32),
        ("z", FieldType::Float32),
        ("falloffStart", FieldType::Float32),
        ("falloffEnd", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(n, t));
    }
    for i in 0..5 {
        s.add_field(SchemaField::new(format!("param{i}"), FieldType::UInt32));
    }
    s
}

/// `LightParams.dbc` — 9 fields, 36 B/record. We only consume `glow` (field [`LP_GLOW`]); the rest are
/// named for clarity (highlightSky flag, skybox/cloud ids, and the four water/ocean blend alphas we
/// don't yet use). The `+0x0C` `cloudTypeID`/reserved slot is 0 in all 5875 records — the engine's glow
/// is the *next* field (+0x10). Field types per wowdev + the client's reader `0x589030`.
fn light_params_schema() -> Schema {
    let mut s = Schema::new("LightParams");
    for (n, t) in [
        ("ID", FieldType::UInt32),
        ("highlightSky", FieldType::UInt32),
        ("lightSkyboxID", FieldType::UInt32),
        ("cloudTypeID", FieldType::UInt32),
        ("glow", FieldType::Float32),
        ("waterShallowAlpha", FieldType::Float32),
        ("waterDeepAlpha", FieldType::Float32),
        ("oceanShallowAlpha", FieldType::Float32),
        ("oceanDeepAlpha", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(n, t));
    }
    s
}

/// The client's distance→strength ramp for an area light: `1` within `start`, falling linearly to
/// `0` at `end` (and `0` beyond). `dn_light_select 0x6d2d00`:
/// `w = dist ≤ inner ? 1 : 1 − (dist−inner)/(outer−inner)`, with
/// the containment gate `dist ≤ outer` — so a sphere entering at its outer radius enters at **w = 0**.
/// That is what makes the area blend continuous across a sphere boundary, and it is the invariant
/// decision 1104's water fix rests on.
fn blend_alpha(dist: f32, start: f32, end: f32) -> f32 {
    if dist >= end {
        0.0
    } else if dist <= start || end <= start {
        1.0
    } else {
        (end - dist) / (end - start)
    }
}

impl LightCatalog {
    /// Read the three lighting DBCs off the patch chain.
    pub fn load(chain: &mut Chain) -> Result<Self> {
        let lights = {
            let bytes = chain
                .read_file(LIGHT)
                .with_context(|| format!("reading {LIGHT}"))?;
            let rs = parse(&bytes, light_schema(), "Light")?;
            let mut v = Vec::with_capacity(rs.records().len());
            for r in rs.records() {
                if let (Some(id), Some(map), Some(x), Some(y), Some(z), Some(start), Some(end)) = (
                    u32_at(r, 0),
                    u32_at(r, 1),
                    f32_at(r, 2),
                    f32_at(r, 3),
                    f32_at(r, 4),
                    f32_at(r, 5),
                    f32_at(r, 6),
                ) {
                    // Fields 7..12 are the 5 LightParams refs (clear, clear-water, storm, …).
                    let mut params = [0u32; 5];
                    for (i, p) in params.iter_mut().enumerate() {
                        *p = u32_at(r, 7 + i).unwrap_or(0);
                    }
                    // `(0,0,0)` is the continent-wide fallback sentinel — detect it on the raw
                    // values (it does NOT survive the world transform).
                    let global = x == 0.0 && y == 0.0 && z == 0.0;
                    v.push(Light {
                        id,
                        map,
                        pos: if global {
                            [0.0; 3]
                        } else {
                            dbc_to_world(x, y, z)
                        },
                        falloff_start: start / POS_SCALE,
                        falloff_end: end / POS_SCALE,
                        global,
                        params,
                    });
                }
            }
            v
        };
        let int_bands = load_bands(
            chain,
            INT_BAND,
            band_schema("LightIntBand", FieldType::UInt32),
        )?;
        let float_bands = load_float_bands(
            chain,
            FLOAT_BAND,
            band_schema("LightFloatBand", FieldType::Float32),
        )?;
        let (
            light_params_glow,
            light_params_highlight,
            light_params_water_alpha,
            light_params_skybox,
        ) = {
            let bytes = chain
                .read_file(LIGHT_PARAMS)
                .with_context(|| format!("reading {LIGHT_PARAMS}"))?;
            let rs = parse(&bytes, light_params_schema(), "LightParams")?;
            let mut glow = HashMap::with_capacity(rs.records().len());
            let mut highlight = HashMap::with_capacity(rs.records().len());
            let mut water_alpha = HashMap::with_capacity(rs.records().len());
            let mut skybox = HashMap::new();
            for r in rs.records() {
                let Some(id) = u32_at(r, 0) else { continue };
                if let Some(g) = f32_at(r, LP_GLOW) {
                    glow.insert(id, g);
                }
                // highlightSky is an int 0/1 flag; store 0.0/1.0 as the dawn/dusk-warp gate.
                if let Some(h) = u32_at(r, LP_HIGHLIGHT) {
                    highlight.insert(id, if h != 0 { 1.0 } else { 0.0 });
                }
                // Water-blend alphas (fields 5–8). Stored only when all four parse; consumers fall back
                // to the DEFAULT alphas otherwise. Per-zone the from-above swatch alpha ramp endpoints.
                if let (Some(ws), Some(wd), Some(os), Some(od)) = (
                    f32_at(r, LP_WATER_SHALLOW_ALPHA),
                    f32_at(r, LP_WATER_DEEP_ALPHA),
                    f32_at(r, LP_OCEAN_SHALLOW_ALPHA),
                    f32_at(r, LP_OCEAN_DEEP_ALPHA),
                ) {
                    water_alpha.insert(id, [ws, wd, os, od]);
                }
                // `lightSkyboxID`: 0 means "no skybox", which is 421 of the 426 rows — store only
                // the live ones so a lookup miss and an explicit zero are the same answer.
                match u32_at(r, LP_SKYBOX) {
                    Some(0) | None => {}
                    Some(sky) => {
                        skybox.insert(id, sky);
                    }
                }
            }
            (glow, highlight, water_alpha, skybox)
        };
        // `LightSkybox.dbc` — 6 records × 2 fields (`ID`, `Name`), the model each skybox id names.
        let skyboxes = {
            let bytes = chain
                .read_file(LIGHT_SKYBOX)
                .with_context(|| format!("reading {LIGHT_SKYBOX}"))?;
            let mut schema = Schema::new("LightSkybox");
            schema.add_field(SchemaField::new("ID", FieldType::UInt32));
            schema.add_field(SchemaField::new("Name", FieldType::String));
            let rs = parse(&bytes, schema, "LightSkybox")?;
            let mut m = HashMap::with_capacity(rs.records().len());
            for r in rs.records() {
                if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 1)) {
                    m.insert(id, crate::models::model_path(&path));
                }
            }
            m
        };
        Ok(LightCatalog {
            lights,
            int_bands,
            float_bands,
            light_params_glow,
            light_params_highlight,
            light_params_water_alpha,
            light_params_skybox,
            skyboxes,
        })
    }

    /// Sample the atmosphere for `pos` (raw WoW yards) on `map` at `time` (half-minutes, 1440=noon).
    /// `stormy` selects the rainy/snowy param slot over the clear one. Picks the most-local light
    /// sphere containing `pos`, else the continent's global `(0,0,0)` light, else
    /// [`Atmosphere::DEFAULT`].
    pub fn sample(&self, map: u32, pos: [f32; 3], time: u32, stormy: bool) -> Atmosphere {
        let Some(light) = self.pick_light(map, pos) else {
            return Atmosphere::DEFAULT;
        };
        // Fall back to the clear slot if the requested weather slot is unset.
        let slot = if stormy { SLOT_STORM } else { SLOT_CLEAR };
        let param = match light.params[slot] {
            0 => light.params[SLOT_CLEAR],
            p => p,
        };
        if param < 1 {
            return Atmosphere::DEFAULT;
        }
        self.sample_param(param, time)
    }

    /// [`LightCatalog::sample`] at noon, clear weather.
    pub fn sample_noon(&self, map: u32, pos: [f32; 3]) -> Atmosphere {
        self.sample(map, pos, DAY / 2, false)
    }

    /// Debug: print every `LightIntBand` row (0..18) and `LightFloatBand` row (0..6) for the clear
    /// LightParams covering `pos`, sampled at `time`. Used to verify which rows hold which colors
    /// (the five sky-gradient stops, sun/cloud colors, etc.) rather than guessing the layout.
    pub fn debug_bands(&self, map: u32, pos: [f32; 3], time: u32) {
        let Some(light) = self.pick_light(map, pos) else {
            println!("(no light covers this position)");
            return;
        };
        let p = match light.params[SLOT_CLEAR] {
            0 => {
                println!("(no clear LightParams)");
                return;
            }
            x => x,
        };
        self.debug_param(p, time);
    }

    /// Resolve an explicitly named `LightParams` id at a time of day, with no position and no slot
    /// selection — the machine-readable twin of [`Self::debug_param`]. The band-row id for params
    /// `p`, sub `b` is `(p-1)*per + b + 1`, an **ID** and not a row ordinal: `LightParams` ids run
    /// 1..499 with 73 gaps, so an ordinal keying reads another zone's bands for every group past the
    /// first gap. Exposed so a test can pin a record above a gap.
    pub fn sample_params_id(&self, p: u32, time: u32) -> Option<Atmosphere> {
        (p >= 1 && self.has_param(p)).then(|| self.sample_param(p, time))
    }

    /// Debug: [`Self::debug_bands`] for an explicitly named `LightParams` id, with no position and no
    /// slot selection. The instrument for the rows **nothing in `Light.dbc` references**: magma and
    /// slime submersion do not read a zone's underwater slot at all, they read the fixed global rows
    /// 7 and 6 (VERIFIED `0x6d2371`, guards `maxId >= 7`/`>= 6`), which no position-keyed dump can
    /// ever reach.
    pub fn debug_param(&self, p: u32, time: u32) {
        if p < 1 {
            println!("(LightParams ids are 1-based)");
            return;
        }
        println!("LightParams {p} @ time {time} half-min — all int rows (sRGB 0..255):");
        for b in 0..18u32 {
            let key = (p - 1) * 18 + b + 1;
            match self
                .int_bands
                .get(&key)
                .and_then(|band| sample_color(band, time))
            {
                Some(c) => println!(
                    "  int[{b:2}] = [{:3}, {:3}, {:3}]",
                    (c[0] * 255.0) as u8,
                    (c[1] * 255.0) as u8,
                    (c[2] * 255.0) as u8
                ),
                None => println!("  int[{b:2}] = (unset)"),
            }
        }
        for b in 0..6u32 {
            let key = (p - 1) * 6 + b + 1;
            match self
                .float_bands
                .get(&key)
                .and_then(|band| sample_float(band, time))
            {
                Some(v) => println!("  float[{b}] = {v:.4}"),
                None => println!("  float[{b}] = (unset)"),
            }
        }
    }

    /// The **ghost sky** for this position: the model `LightSkybox.dbc` names for the death
    /// profile, or `None` when the chain's data does not name one.
    ///
    /// The caller gates on the `PLAYER_FLAGS` ghost bit — that bit is the *whole* condition, and it
    /// is the reference's too: the DBC skybox slot `[0xce9bb4]` is filled in the colour-table build
    /// `0x6d2260` at `0x6d26cb`, **gated on the override cell `[0xce9bb0] != -1`**, which only
    /// `0x6d4620` writes and only the ghost-bit selector `0x5de9c0` calls (`mov ecx,4` — param slot
    /// [`SLOT_DEATH`]). So: ghost ⇒ this skybox; alive ⇒ none, with no other path into the table.
    ///
    /// **Read straight off slot 4, with no clear-slot fallback** — unlike the atmosphere's
    /// `atmo_of`, which degrades an unset slot to [`SLOT_CLEAR`]. The reference resolves the row
    /// with `0x6d6ab0(row, 4)` = `[row+0x2c]` and simply finds nothing if it is 0; degrading here
    /// would instead read a slot that, in shipped data, never names a skybox at all.
    ///
    /// Resolved through [`pick_light`](Self::pick_light) so the sky comes from the same zone row
    /// the water tint does. On 5875 data the choice is unobservable — all 374 `Light` rows reach
    /// skybox **3** (`DeathClouds.mdx`) through slot 4 — but a chain that ever varied it would then
    /// vary sky and zone together rather than drifting apart.
    pub fn ghost_skybox(&self, map: u32, pos: [f32; 3]) -> Option<&str> {
        let light = self.pick_light(map, pos)?;
        let id = self.light_params_skybox.get(&light.params[SLOT_DEATH])?;
        self.skyboxes.get(id).map(String::as_str)
    }

    /// Faithful **area-light blend** (step 2 isolation): replicates the real client's
    /// `calculateLightParamBlends` + the sequential lerp in `getLightResultsFromDB`. Start from the
    /// continent's global `(0,0,0)` light, then for every local light sphere covering `pos` (sorted by
    /// `blendAlpha` descending) lerp the accumulated atmosphere toward that light by its `blendAlpha`
    /// (`1` inside `falloffStart`, ramping to `0` at `falloffEnd`). Contrast with [`pick_light`], which
    /// picks just the single most-local sphere — this tells us whether a faithful blend resolves a
    /// **different** (e.g. redder) light at our spot than the single-sample approximation.
    pub fn sample_blended(
        &self,
        map: u32,
        pos: [f32; 3],
        time: u32,
        stormy: bool,
        submersion: Submersion,
        ghost: bool,
    ) -> Atmosphere {
        // Magma/slime submersion replaces the WHOLE area blend with one fixed global row — the
        // reference branches out at `0x6d2371`, before the slot selection, so there is no zone
        // sphere blend and no storm lerp on this leg (see [`PARAM_MAGMA`]). Guarded on the row
        // actually existing, matching the binary's own `maxId` guard: a chain without it falls
        // through to the ordinary atmosphere rather than inventing one.
        if let Some(p) = submersion.fixed_param() {
            if self.has_param(p) {
                return self.sample_param(p, time);
            }
        }
        let slot = weather_slot(ghost, stormy, submersion.is_water());
        let atmo_of = |l: &Light| -> Option<Atmosphere> {
            // Fall back to the clear slot if the requested slot is unset for this light (e.g. a zone
            // with no dedicated underwater param degrades to the above-water atmosphere).
            let param = match l.params[slot] {
                0 => l.params[SLOT_CLEAR],
                p => p,
            };
            (param >= 1).then(|| self.sample_param(param, time))
        };

        // Base = the continent global light (lowest priority, covers everywhere), else — when the
        // map has **no `Light.dbc` row at all** — the client's zero-match tail fallback, record
        // [`FALLBACK_LIGHT_ID`] (`0x6d62b2`; see its doc). Deeprun Tram (369) is the shipped map
        // this is for: with the old invented default it rendered as a bright noon scene (fog
        // [140,183,234] @ 1000 yd) instead of LightParams 12's own curve (fog [77,120,143] @ 500).
        //
        // Scoped to zero-match ON PURPOSE. Seven shipped maps (33/37/129/169/209/489/531) carry
        // positioned rows but no `(0,0,0)` global; there the reference seeds count = 1 and fills
        // slots 1.. with the positioned rows, leaving slot 0 unwritten — what it then blends
        // against is NOT settled by the finding this rests on, so that case keeps the behaviour it
        // has rather than inheriting a guess.
        let map_has_no_light = !self.lights.iter().any(|l| l.map == map);
        let mut acc = self
            .lights
            .iter()
            .find(|l| l.map == map && l.global)
            .or_else(|| {
                map_has_no_light
                    .then(|| self.lights.iter().find(|l| l.id == FALLBACK_LIGHT_ID))
                    .flatten()
            })
            .and_then(atmo_of)
            .unwrap_or(Atmosphere::DEFAULT);

        // Collect every local sphere CONTAINING `pos` (`dist ≤ outer`), then apply them
        // **farthest first** so the nearest lands last and dominates.
        //
        // The order is by DISTANCE, not by blend weight (`dn_light_select 0x6d2d00`): the client
        // pushes the in-radius rows into a **max-heap keyed on distance** and
        // drains it root-first, calling `0x6d30e0(dst, row, w)` for **every**
        // entry including a lone one. Sorting by weight instead (what we did before decision 1104)
        // inverts the pair whenever a wide sphere out-weighs a tight near one, and then the near
        // zone's own palette gets *diluted* by its neighbour instead of overwriting it — the
        // Stranglethorn river that read muddy yellow-green instead of LP 26's grey-green.
        let mut locals: Vec<(f32, &Light)> = self
            .lights
            .iter()
            .filter(|l| l.map == map && !l.global)
            .filter_map(|l| {
                let d = (0..3)
                    .map(|i| (l.pos[i] - pos[i]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                (d <= l.falloff_end).then_some((d, l))
            })
            .collect();
        locals.sort_by(|a, b| b.0.total_cmp(&a.0));

        for (dist, l) in locals {
            if let Some(atmo) = atmo_of(l) {
                acc = acc.lerp(&atmo, blend_alpha(dist, l.falloff_start, l.falloff_end));
            }
        }
        acc
    }

    /// Debug (step 2): dump every light sphere on `map` near `pos` — distance, falloff, `blendAlpha`,
    /// and its clear-weather ambient + sun colors — then the faithful blended result vs the single
    /// [`pick_light`] sample, so we can see whether the residual under-red lives in the **values**.
    ///
    /// Rows print **nearest first** but the blend applies them **farthest first** (see
    /// [`Self::sample_blended`]) — the last row listed is the first one merged, and the top row wins.
    /// The closing `water` lines are the river/lake and ocean swatch endpoints: a discontinuity there
    /// across two nearby positions is the Tirisfal→Silverpine class of bug, and it is
    /// invisible in the ambient/sun columns because the water rows are their own bands.
    pub fn debug_blend(&self, map: u32, pos: [f32; 3], time: u32) {
        let mut rows: Vec<(f32, f32, &Light)> = self
            .lights
            .iter()
            .filter(|l| l.map == map && !l.global)
            .map(|l| {
                let d = (0..3)
                    .map(|i| (l.pos[i] - pos[i]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                (d, blend_alpha(d, l.falloff_start, l.falloff_end), l)
            })
            .collect();
        rows.sort_by(|a, b| a.0.total_cmp(&b.0));

        let c = |x: [f32; 3]| {
            format!(
                "[{:3} {:3} {:3}]",
                (x[0] * 255.0) as u8,
                (x[1] * 255.0) as u8,
                (x[2] * 255.0) as u8
            )
        };
        println!(
            "Lights on map {map} near [{:.0} {:.0} {:.0}] (nearest 8) — dist | start->end | alpha | amb / sun:",
            pos[0], pos[1], pos[2]
        );
        for (d, alpha, l) in rows.iter().take(8) {
            let a = if l.params[SLOT_CLEAR] >= 1 {
                self.sample_param(l.params[SLOT_CLEAR], time)
            } else {
                Atmosphere::DEFAULT
            };
            println!(
                "  d={d:7.0} | {:6.0}->{:6.0} | a={alpha:.3} | amb {} sun {}",
                l.falloff_start,
                l.falloff_end,
                c(a.ambient),
                c(a.sun_diffuse)
            );
        }
        let global = self
            .lights
            .iter()
            .find(|l| l.map == map && l.global)
            .and_then(|l| {
                (l.params[SLOT_CLEAR] >= 1).then(|| self.sample_param(l.params[SLOT_CLEAR], time))
            });
        if let Some(g) = global {
            println!(
                "  GLOBAL (0,0,0) base      | amb {} sun {}",
                c(g.ambient),
                c(g.sun_diffuse)
            );
        }
        let picked = self.sample(map, pos, time, false);
        let blended = self.sample_blended(map, pos, time, false, Submersion::Dry, false);
        println!(
            "  => pick_light : amb {} sun {}",
            c(picked.ambient),
            c(picked.sun_diffuse)
        );
        println!(
            "  => blended    : amb {} sun {}",
            c(blended.ambient),
            c(blended.sun_diffuse)
        );
        // The water swatch is what the liquid materials actually read (`WowLighting.water_*`), and it
        // rides this same blend — print it so "the water popped here" is one command, not a session.
        println!(
            "  => water river: shallow {} a={:.2}  deep {} a={:.2}",
            c(blended.water_river[0]),
            blended.water_river_alpha[0],
            c(blended.water_river[1]),
            blended.water_river_alpha[1],
        );
        println!(
            "  => water ocean: shallow {} a={:.2}  deep {} a={:.2}",
            c(blended.water_ocean[0]),
            blended.water_ocean_alpha[0],
            c(blended.water_ocean[1]),
            blended.water_ocean_alpha[1],
        );
    }

    /// Debug: the **per-weather-slot** resolve at `pos` — for each of the five `Light.dbc` param
    /// slots (clear / clear-underwater / storm / storm-underwater / death), which `LightParams` id
    /// the picked sphere and the continent global each name, whether the slot is UNSET (and so falls
    /// back to clear), and the fog/ambient/diffuse the blend actually resolves.
    ///
    /// The companion to [`Self::debug_bands`], which only ever dumps the clear slot: an underwater or
    /// ghost atmosphere that reads wrong is either a slot the data leaves unset (→ we correctly show
    /// the surface atmosphere) or a slot whose values we mis-consume, and those two look identical
    /// from inside the game. This tells them apart in one command.
    pub fn debug_slots(&self, map: u32, pos: [f32; 3], time: u32) {
        const NAMES: [&str; 5] = [
            "clear",
            "clear-underwater",
            "storm",
            "storm-underwater",
            "death",
        ];
        let c = |x: [f32; 3]| {
            format!(
                "[{:3} {:3} {:3}]",
                (x[0] * 255.0) as u8,
                (x[1] * 255.0) as u8,
                (x[2] * 255.0) as u8
            )
        };
        let picked = self.pick_light(map, pos);
        let global = self.lights.iter().find(|l| l.map == map && l.global);
        println!(
            "Light slots on map {map} at [{:.1} {:.1} {:.1}], time {time} half-min:",
            pos[0], pos[1], pos[2]
        );
        match picked {
            Some(l) if l.global => {
                println!("  picked sphere : (none local — the continent global)")
            }
            Some(l) => println!(
                "  picked sphere : local, falloff {:.0}->{:.0} yd, params {:?}",
                l.falloff_start, l.falloff_end, l.params
            ),
            None => println!("  picked sphere : (no light covers this position)"),
        }
        if let Some(g) = global {
            println!("  continent glob: params {:?}", g.params);
        }
        println!(
            "  {:<17} {:>6} {:>6} {:>9} {:>6}  {:<15} {:<15} {:<15}",
            "slot", "picked", "global", "fog_end", "frac", "fog_color", "ambient", "diffuse"
        );
        for (slot, name) in NAMES.iter().enumerate() {
            // The same UNSET→clear fallback both sample paths apply, reported per source so an
            // "unset everywhere" slot is visibly distinct from one carrying real values.
            let show = |l: Option<&Light>| match l.map(|l| l.params[slot]) {
                None => "     -".to_string(),
                Some(0) => " unset".to_string(),
                Some(p) => format!("{p:>6}"),
            };
            let a = self.sample_blended(
                map,
                pos,
                time,
                slot == SLOT_STORM || slot == SLOT_STORM_UNDERWATER,
                if slot == SLOT_CLEAR_UNDERWATER || slot == SLOT_STORM_UNDERWATER {
                    Submersion::Water
                } else {
                    Submersion::Dry
                },
                slot == SLOT_DEATH,
            );
            println!(
                "  {name:<17} {} {} {:>9.1} {:>6.2}  {:<15} {:<15} {:<15}",
                show(picked),
                show(global),
                a.fog_end,
                a.fog_start_frac,
                c(a.fog_color),
                c(a.ambient),
                c(a.sun_diffuse)
            );
        }
    }

    fn pick_light(&self, map: u32, pos: [f32; 3]) -> Option<&Light> {
        let mut local: Option<&Light> = None;
        let mut global: Option<&Light> = None;
        let mut any = false;
        for l in self.lights.iter().filter(|l| l.map == map) {
            any = true;
            if l.global {
                global = Some(l);
                continue;
            }
            let d2 = (0..3).map(|i| (l.pos[i] - pos[i]).powi(2)).sum::<f32>();
            if d2 <= l.falloff_end * l.falloff_end
                && local.is_none_or(|b| l.falloff_end < b.falloff_end)
            {
                local = Some(l);
            }
        }
        // Same zero-match tail as `sample_blended` (see [`FALLBACK_LIGHT_ID`]): a map with no row
        // of its own resolves record 1. Kept in step deliberately — this is the single-sphere
        // approximation the water tint reads, and a rowless map that answered one thing to the
        // area blend and another here would tint its water off a different atmosphere than it fogs
        // with (decision 0706's split, in miniature).
        local.or(global).or_else(|| {
            (!any)
                .then(|| self.lights.iter().find(|l| l.id == FALLBACK_LIGHT_ID))
                .flatten()
        })
    }

    /// Whether `LightParams` `p` actually has band rows in this chain — the client's `maxId` guard on
    /// the fixed magma/slime rows. Checks the fog-colour row, which every real record carries.
    fn has_param(&self, p: u32) -> bool {
        p >= 1
            && self
                .int_bands
                .contains_key(&((p - 1) * 18 + IB_FOG_COLOR + 1))
    }

    fn sample_param(&self, p: u32, t: u32) -> Atmosphere {
        let ib = |b: u32| self.int_bands.get(&((p - 1) * 18 + b + 1));
        let fb = |b: u32| self.float_bands.get(&((p - 1) * 6 + b + 1));
        // `d` survives only for the four `LightParams` **record fields** below (glow, highlightSky,
        // the water alphas). Those are not band rows: their "missing" case is a record that failed
        // to parse, not a keyless row, so they keep the record-level fallback. Every BAND row on
        // this record takes the reference's zero-key constant instead.
        let d = Atmosphere::DEFAULT;
        // Fog distances carry the ×36 storage scale (yards = raw/36): Elwynn clear 18000→500 yd,
        // storm 10000→278 yd: the client scales ONCE at DBC load —
        // `0x53f504 → 0x6d6160 → 0x6d6100` runs the band scaler `0x6d6090` (×1/36 @0x7ff9d0) over
        // each float-band record with `rowIndex % 6 == 0`, i.e.
        // ONLY the sub-0 fog-END band; the sub-1 start FRACTION is never scaled. We apply the same
        // /36 at sample time (a time-interp of scaled values ≡ the scale of the interp).
        // A row with no keyframes commits the reference's own constant, never an invented one
        // (`ZERO_KEY_SCALAR`/`ZERO_KEY_COLOR`). The `.filter(|v| v > 1.0)` that used
        // to sit here substituted `d.fog_end` for an authored ZERO as well as for a keyless row —
        // it is gone, and no shipped row lands in the (0, 1] yd window it also covered (checked
        // across all 426 params: the only sub-1 values are the exact 0.0 that params 9 and 93
        // author). `min(end − start, 0.001)` in the fog shaders is what keeps end = 0 finite.
        let fog_end = fb(FB_FOG_END)
            .and_then(|b| sample_float(b, t))
            .map(|v| v / POS_SCALE)
            .unwrap_or(ZERO_KEY_SCALAR);
        let fog_start_frac = fb(FB_FOG_START_MULT)
            .and_then(|b| sample_float(b, t))
            .unwrap_or(ZERO_KEY_SCALAR);
        let col = |idx: u32| {
            ib(idx)
                .and_then(|b| sample_color(b, t))
                .unwrap_or(ZERO_KEY_COLOR)
        };
        Atmosphere {
            fog_end,
            // UNCLAMPED, negative under storm (Elwynn −0.5) — the negative start IS the constant
            // near veil the reference shows in rain; clamping it to 0 was the veil-killing bug.
            fog_start_frac,
            fog_color: col(IB_FOG_COLOR),
            sun_diffuse: col(IB_DIFFUSE),
            sun_color: col(IB_SUN_COLOR),
            ambient: col(IB_AMBIENT),
            sky: std::array::from_fn(|i| col(IB_SKY0 + i as u32)),
            // Dedicated water-tint rows (RAW, no scale): 16/17 river/lake, 14/15 ocean — the 2-endpoint
            // depth-swatch lerp (`FUN_0068a830`). These are real per-zone rows, NOT the sky gradient.
            //
            // ⚠ The zero-key arm is WIDEST here and least self-corroborating. 125/149/59/59 of the
            // 426 params leave ocean-shallow/ocean-deep/river-shallow/river-deep unkeyed, and 53
            // CLEAR-slot spheres on maps 0/1 are among them — so this is where black actually
            // reaches player water. Unlike the cloud base (209 of 308 keyed rows are exactly
            // `(0,0,0)`, not one is pale — black is that lane's modal authored value), only 3-10 of
            // ~300 keyed water rows are black: a keyless water row answering black is out of family
            // for the authored data. Both halves are byte-verified — the slot is black (1465) and
            // the swatch builder reads the slot — so this is what the reference computes;
            // what it LOOKS like in one of those 53 zones is the director's to judge, and this note
            // is the pointer back if it reads wrong.
            water_river: [col(IB_RIVER_SHALLOW), col(IB_RIVER_DEEP)],
            water_ocean: [col(IB_OCEAN_SHALLOW), col(IB_OCEAN_DEEP)],
            // Per-zone water-blend alphas (static LightParams fields 5–8); fall back to the DEFAULT
            // ramp endpoints when the record didn't carry them.
            water_river_alpha: self
                .light_params_water_alpha
                .get(&p)
                .map(|a| [a[0], a[1]])
                .unwrap_or(d.water_river_alpha),
            water_ocean_alpha: self
                .light_params_water_alpha
                .get(&p)
                .map(|a| [a[2], a[3]])
                .unwrap_or(d.water_ocean_alpha),
            // Per-zone glow (static LightParams field, not a time band); fallback 0.5.
            glow: self.light_params_glow.get(&p).copied().unwrap_or(d.glow),
            // Cloud density C (float sub-3) + the three cloud palette rows (int sub-10/11/12) —
            // the coverage threshold and the visible dome's colors (the band gather `0x6d64d0`).
            cloud_density: fb(FB_CLOUD_DENSITY)
                .and_then(|b| sample_float(b, t))
                .unwrap_or(ZERO_KEY_SCALAR),
            cloud_colors: [col(IB_CLOUD_SUN), col(IB_CLOUD_SLOPE), col(IB_CLOUD_GBASE)],
            // Per-zone highlightSky flag (static); gates the dawn/dusk dome warp. Fallback 0.0.
            highlight_sky: self
                .light_params_highlight
                .get(&p)
                .copied()
                .unwrap_or(d.highlight_sky),
        }
    }
}

#[cfg(test)]
mod ocean_tests {
    use super::*;

    /// The ramp's shape: both factors are 1.0 at the surface, bottom out at 30 yards, and hold flat
    /// below. `fac1` (the first light triple) halves; `fac2` (the second) only drops to 0.75 — the
    /// fill darkens twice as fast as the direct term, which is what makes a deep sea read flat as
    /// well as dim.
    #[test]
    fn the_ocean_ramp_runs_over_thirty_yards_and_then_holds() {
        let f = |z: f32| Submersion::Ocean.ocean_depth_factors(z).unwrap();
        let (a, b) = f(0.0);
        assert_eq!(
            (a, b),
            (1.0, 1.0),
            "at the surface the ramp is the identity"
        );
        // Above the surface clamps to the identity too — the ramp never brightens.
        assert_eq!(f(12.0), (1.0, 1.0));
        let (a30, b30) = f(-30.0);
        assert!((a30 - 0.5).abs() < 1e-6 && (b30 - 0.75).abs() < 1e-6);
        assert_eq!(f(-100.0), (a30, b30), "below 30 yd it holds flat");
        // Linear, and fac1 falls exactly twice as fast as fac2.
        let (a15, b15) = f(-15.0);
        assert!((a15 - 0.75).abs() < 1e-6 && (b15 - 0.875).abs() < 1e-6);
        assert!(((1.0 - a15) - 2.0 * (1.0 - b15)).abs() < 1e-6);
    }

    /// GOLDEN — the binary's own constant, and the reason it has to be a MULTIPLY.
    ///
    /// A bit-exact port must use the stored f32 rather than `t/30.0`, though its bit pattern IS the
    /// nearest f32 to −1/30. The hazard is the
    /// operation, not the operand — `t · recip` and `t / 30.0` round differently, and they disagree
    /// *inside* the ramp's range, so a port that "simplifies" the multiply into a divide drifts.
    /// This test pins both halves so neither can be tidied away.
    #[test]
    fn the_ramp_multiplies_by_the_binarys_own_reciprocal() {
        assert_eq!(OCEAN_RAMP_RECIP.to_bits(), 0xbd088889);
        assert_eq!(
            OCEAN_RAMP_RECIP,
            -1.0f32 / 30.0,
            "the constant itself is just the nearest f32 to -1/30"
        );
        // …and yet the two forms are not interchangeable. -29 yd is a witness inside the range.
        let t = -29.0f32;
        assert_ne!(
            1.0 - t * OCEAN_RAMP_RECIP,
            1.0 + t / 30.0,
            "multiply and divide must be observably different, or the warning is empty"
        );
        // The boundaries land exactly, with no f32 residue to explain away.
        assert_eq!(
            Submersion::Ocean.ocean_depth_factors(-30.0).unwrap(),
            (0.5, 0.75)
        );
    }

    /// The ramp is OCEAN'S ALONE — the gate is the full unmasked dword `== 1` (`0x6d2821`), and
    /// every other state, still water included, is untouched at any depth.
    #[test]
    fn only_ocean_ramps() {
        for s in [
            Submersion::Dry,
            Submersion::Water,
            Submersion::Magma,
            Submersion::Slime,
        ] {
            assert!(
                s.ocean_depth_factors(-30.0).is_none(),
                "{s:?} must not ramp"
            );
        }
    }

    /// The split exists for the ramp and nothing else: everywhere a *water kind* is the question,
    /// ocean answers with water — the underwater LightParams slot, the ambience bed, the reverb
    /// column. And neither takes a fixed global row, which is what magma and slime do.
    #[test]
    fn ocean_is_a_water_kind_everywhere_but_the_ramp() {
        assert!(Submersion::Ocean.is_water() && Submersion::Water.is_water());
        assert!(!Submersion::Magma.is_water() && !Submersion::Slime.is_water());
        assert!(Submersion::Ocean.any());
        assert_eq!(Submersion::Ocean.fixed_param(), None);
        assert_eq!(
            weather_slot(false, false, Submersion::Ocean.is_water()),
            SLOT_CLEAR_UNDERWATER,
            "ocean reaches the underwater slot exactly as water does"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbc_to_world_matches_wiki_convertdbtogamecoords() {
        // The `(0,0,0)` global sentinel maps to the map center (we special-case it before this).
        assert_eq!(dbc_to_world(0.0, 0.0, 0.0), [17066.666, 17066.666, 0.0]);

        // Round-trip a known world point through the wiki inverse
        // (DBC.x = (17066.666 − world.y)·36, DBC.y = world.z·36, DBC.z = (17066.666 − world.x)·36):
        // applying `dbc_to_world` must recover the world coords. Light #51 → (-8480, 548, 81).
        let world = [-8480.4f32, 548.3, 80.9];
        let dbc = [
            (17066.666 - world[1]) * POS_SCALE,
            world[2] * POS_SCALE,
            (17066.666 - world[0]) * POS_SCALE,
        ];
        let back = dbc_to_world(dbc[0], dbc[1], dbc[2]);
        for i in 0..3 {
            assert!(
                (back[i] - world[i]).abs() < 0.1,
                "axis {i}: {} != {}",
                back[i],
                world[i]
            );
        }
    }

    #[test]
    fn glow_defaults_blends_and_quantises() {
        // Fallback = the binary's "no active world-light" 0.5.
        assert_eq!(Atmosphere::DEFAULT.glow, 0.5);
        // Glow blends across light spheres like the colours (Duskwood 0.50 ↔ a 0.85 area).
        let a = Atmosphere {
            glow: 0.50,
            ..Atmosphere::DEFAULT
        };
        let b = Atmosphere {
            glow: 0.85,
            ..Atmosphere::DEFAULT
        };
        assert!((a.lerp(&b, 0.5).glow - 0.675).abs() < 1e-6);
        // The renderer quantises floor(glow·255)/255 — Elwynn's 0.65 → 165/255 = 0.647 (the
        // apitrace composite weight), confirming LP_GLOW (field 4) is the right field.
        let q = |g: f32| (g * 255.0).floor() / 255.0;
        assert!((q(0.65) - 165.0 / 255.0).abs() < 1e-6);
        assert!((q(0.50) - 127.0 / 255.0).abs() < 1e-6); // Duskwood → 0.498
    }
}
