//! The atmosphere from the lighting DBCs: fog, sun, ambient and sky colours from authored data.
//! `Light.dbc` gives a sphere per area and a `LightParams` id per weather slot; param `P` keys 18
//! `LightIntBand` and 6 `LightFloatBand` rows, row `b` at id `(P − 1)·per + b + 1`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

mod atmosphere;
mod tables;

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

/// `Light.dbc` positions and radii are inches: yards × 36.
const POS_SCALE: f32 = 36.0;

/// `Light.dbc` param slots: 0 clear, 1 clear underwater, 2 storm, 3 storm underwater, 4 death.
const SLOT_CLEAR: usize = 0;
/// Denser fog and cooler light while the eye is under water; only the water kinds reach it,
/// magma and slime take fixed rows ([`PARAM_MAGMA`]).
const SLOT_CLEAR_UNDERWATER: usize = 1;
const SLOT_STORM: usize = 2;
const SLOT_STORM_UNDERWATER: usize = 3;
/// The ghost profile: the ghost watcher `0x5de9c0` writes slot 4 into the active-slot global
/// `[0xce9bb0]`, and the colour-table rebuild takes it at once, with no blend, over all else.
const SLOT_DEATH: usize = 4;

/// The fixed global rows for submersion in slime (6) and magma (7): `0x6d2371` branches before
/// the slot selection, to row 7 at `0x6d23e1` and row 6 at `0x6d239e`, each guarded on `maxId`.
/// No `Light.dbc` row names them, and the binary has no other magma or slime fog.
const PARAM_SLIME: u32 = 6;
const PARAM_MAGMA: u32 = 7;

/// The ocean depth ramp's floor, −30.0 at `0x81162c`; the factors hold flat below it.
const OCEAN_RAMP_FLOOR: f32 = -30.0;
/// The ramp's reciprocal as the binary stores it, the f32 at `0x811628`, written from its bits
/// (`0xbd088889`) because the decimal trips clippy's `excessive_precision`.
const OCEAN_RAMP_RECIP: f32 = f32::from_bits(0xbd08_8889);

/// What the camera eye is submerged in. Water and ocean take the zone's underwater slot, magma
/// and slime a fixed row. The reference's five states, `[0xc7f288]` ∈ {0xf dry, 0 water, 1 ocean,
/// 2 magma, 3 slime}: only ocean runs the depth ramp, gated on the whole dword `== 1` at
/// `0x6d2821` (a nibble test would pass 0x11); everything else treats ocean as water.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Submersion {
    /// Dry: the ordinary clear or storm slot.
    #[default]
    Dry,
    /// Under still water or rapids: the zone's underwater `LightParams`, area-blended as usual.
    Water,
    /// Under ocean: the water slot plus the depth ramp. Only the ADT MCLQ per-subtile flags
    /// author it (`(b & 0x0f) & 3 == 1`); a WMO cannot.
    Ocean,
    /// Under magma: the fixed global row, verbatim.
    Magma,
    /// Under slime: the fixed global row, verbatim.
    Slime,
}

impl Submersion {
    /// Submerged in any liquid.
    pub fn any(self) -> bool {
        self != Submersion::Dry
    }

    /// A water kind (water, ocean, rapids), for the `UnderWaterLoop` bed and the underwater reverb;
    /// whether the reference swaps those for magma and slime too is untraced.
    pub fn is_water(self) -> bool {
        matches!(self, Submersion::Water | Submersion::Ocean)
    }

    /// The ocean depth ramp (`0x6d2823`, behind the gate `0x6d2821`): `(fac1, fac2)` multiply the
    /// light record's colour triples `DNState+0x178` and `DNState+0x174`, running 1.0 → 0.5 and
    /// 1.0 → 0.75 over the first 30 yd of depth. Keep the multiply by the stored reciprocal:
    /// `t / 30.0` rounds differently inside the range (at −29 yd, by 6e−8). The fog colour it also
    /// writes (`0x6d28b0` → `DNState+0x70`) is overwritten from the bands: `0x66ff60` calls
    /// `0x6cee30` after the tint, whose `0x6cee76` rewrites that slot.
    pub fn ocean_depth_factors(self, eye_z: f32) -> Option<(f32, f32)> {
        if self != Submersion::Ocean {
            return None;
        }
        let t = eye_z.clamp(OCEAN_RAMP_FLOOR, 0.0);
        let k = 1.0 - t * OCEAN_RAMP_RECIP;
        Some(((k + 1.0) * 0.5, k * 0.25 + 0.75))
    }

    /// The fixed global `LightParams` row that replaces the whole blend, for magma and slime.
    fn fixed_param(self) -> Option<u32> {
        match self {
            Submersion::Magma => Some(PARAM_MAGMA),
            Submersion::Slime => Some(PARAM_SLIME),
            Submersion::Dry | Submersion::Water | Submersion::Ocean => None,
        }
    }
}

/// The `LightParams` slot for the ghost, weather and submersion state: ghost wins outright, as
/// the client keeps one active slot, and only a water kind reaches the underwater slots.
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

/// The zero-match fallback record, by `Light.dbc` ID. When no row matches the map, the light
/// array build (`0x6d6170`, matching at `0x6d61a9`) writes record 1, the Azeroth global, into
/// slot 0 (`0x6d62b2`..`0x6d62c9`) with the count seeded at 1 (`0x6d6188`), so `0x6d2d00` has
/// nothing to blend. Deeprun Tram (369) is the one shipped map reachable in play that needs it.
const FALLBACK_LIGHT_ID: u32 = 1;

struct Light {
    /// The `Light.dbc` ID column, not the row index: the zero-match fallback names a record by id.
    id: u32,
    map: u32,
    /// World coordinates in yards (see [`dbc_to_world`]); meaningless when `global`.
    pos: [f32; 3],
    falloff_start: f32,
    falloff_end: f32,
    /// The continent's `(0,0,0)` light: matches everywhere, lowest priority.
    global: bool,
    /// `LightParams` id per slot (`SLOT_*`); 0 = unset.
    params: [u32; 5],
}

/// A `Light.dbc` position to world yards: the DBC stores inches in a mirrored, axis-swapped frame
/// (`LightRec::ConvertDBToGameCoords`), and 17066.666 is half the 34133⅓-yard map.
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
    /// `LightParams` id → glow ([`LP_GLOW`]), the per-zone bloom composite weight.
    light_params_glow: HashMap<u32, f32>,
    /// `LightParams` id → `highlightSky` as 0.0/1.0, gating the dawn and dusk sky-dome warp.
    light_params_highlight: HashMap<u32, f32>,
    /// `LightParams` id → the four water-blend alphas, the swatch's depth-alpha endpoints.
    light_params_water_alpha: HashMap<u32, [f32; 4]>,
    /// `LightParams` id → `lightSkyboxID` ([`LP_SKYBOX`]), for its non-zero rows only.
    light_params_skybox: HashMap<u32, u32>,
    /// `LightSkybox.dbc` id → model chain path, spelled as a WMO MOSB skybox is so one model never
    /// builds twice. Only id 3 (`DeathClouds.mdx`) is reachable from `LightParams`; the other five
    /// rows are MOSB skyboxes.
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

/// `LightParams.dbc`: 9 fields, 36-byte records, types per the client's reader `0x589030`.
/// `cloudTypeID` (+0x0C) is 0 in every 5875 record; the glow is +0x10.
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

/// An area light's strength: 1 within `start`, linear to 0 at `end` (`0x6d2d00`, which admits a
/// sphere only at `dist ≤ end`), so a sphere enters at 0 and the blend stays continuous.
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
    /// Read the lighting DBCs off the patch chain.
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
                    let mut params = [0u32; 5];
                    for (i, p) in params.iter_mut().enumerate() {
                        *p = u32_at(r, 7 + i).unwrap_or(0);
                    }
                    // `(0,0,0)` marks the global; test it raw, since the transform moves it.
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
                if let Some(h) = u32_at(r, LP_HIGHLIGHT) {
                    highlight.insert(id, if h != 0 { 1.0 } else { 0.0 });
                }
                if let (Some(ws), Some(wd), Some(os), Some(od)) = (
                    f32_at(r, LP_WATER_SHALLOW_ALPHA),
                    f32_at(r, LP_WATER_DEEP_ALPHA),
                    f32_at(r, LP_OCEAN_SHALLOW_ALPHA),
                    f32_at(r, LP_OCEAN_DEEP_ALPHA),
                ) {
                    water_alpha.insert(id, [ws, wd, os, od]);
                }
                // 0 is no skybox; storing only the others makes a miss and a 0 the same answer.
                match u32_at(r, LP_SKYBOX) {
                    Some(0) | None => {}
                    Some(sky) => {
                        skybox.insert(id, sky);
                    }
                }
            }
            (glow, highlight, water_alpha, skybox)
        };
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

    /// The atmosphere at `pos` (world yards) on `map` at `time` (half-minutes, 1440 = noon): the
    /// smallest sphere containing `pos`, else the map's global light, else record 1 on a map with
    /// no rows, else [`Atmosphere::DEFAULT`]. `stormy` picks the storm slot.
    pub fn sample(&self, map: u32, pos: [f32; 3], time: u32, stormy: bool) -> Atmosphere {
        let Some(light) = self.pick_light(map, pos) else {
            return Atmosphere::DEFAULT;
        };
        // An unset slot falls back to clear.
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

    /// Debug: print the band rows of the clear `LightParams` covering `pos`, at `time`.
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

    /// One `LightParams` id at a time of day. Band rows key by id, not ordinal: the ids run 1..499
    /// with 73 gaps.
    pub fn sample_params_id(&self, p: u32, time: u32) -> Option<Atmosphere> {
        (p >= 1 && self.has_param(p)).then(|| self.sample_param(p, time))
    }

    /// Debug: the band rows of a named `LightParams` id, such as the magma and slime rows 7 and 6
    /// that only `0x6d2371` reads.
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

    /// The ghost sky here, from slot 4 with no clear-slot fallback, as `0x6d6ab0(row, 4)` reads
    /// `[row+0x2c]`. The ghost bit is the whole condition: the colour-table build `0x6d2260` fills
    /// `[0xce9bb4]` at `0x6d26cb` only while `[0xce9bb0] != -1`, which only `0x6d4620` writes, for
    /// the ghost selector `0x5de9c0` (`mov ecx,4`). The row is [`pick_light`](Self::pick_light)'s;
    /// every shipped row's slot 4 names the same sky, `DeathClouds.mdx`.
    pub fn ghost_skybox(&self, map: u32, pos: [f32; 3]) -> Option<&str> {
        let light = self.pick_light(map, pos)?;
        let id = self.light_params_skybox.get(&light.params[SLOT_DEATH])?;
        self.skyboxes.get(id).map(String::as_str)
    }

    /// The reference's area-light blend: from the map's global light, lerp toward each local
    /// sphere containing `pos` by its [`blend_alpha`], farthest first.
    pub fn sample_blended(
        &self,
        map: u32,
        pos: [f32; 3],
        time: u32,
        stormy: bool,
        submersion: Submersion,
        ghost: bool,
    ) -> Atmosphere {
        // Magma and slime replace the whole blend with one fixed row (`0x6d2371`, before the slot
        // selection); without the row the chain falls through, as the `maxId` guard does.
        if let Some(p) = submersion.fixed_param() {
            if self.has_param(p) {
                return self.sample_param(p, time);
            }
        }
        let slot = weather_slot(ghost, stormy, submersion.is_water());
        let atmo_of = |l: &Light| -> Option<Atmosphere> {
            // An unset slot falls back to clear.
            let param = match l.params[slot] {
                0 => l.params[SLOT_CLEAR],
                p => p,
            };
            (param >= 1).then(|| self.sample_param(param, time))
        };

        // The base: the map's global, else the zero-match record for a rowless map (`0x6d62b2`).
        // The seven maps with rows but no global (33/37/129/169/209/489/531) start from `DEFAULT`:
        // the reference leaves slot 0 unwritten there, and what it blends against is untraced.
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

        // Farthest first, so the nearest lands last: `0x6d2d00` drains a max-heap keyed on
        // distance, not weight, calling `0x6d30e0(dst, row, w)` for every entry, a lone one too.
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

    /// Debug: the spheres near `pos`, nearest first (the blend applies them farthest first), then
    /// the blended result against the `pick_light` sample and the water swatch endpoints.
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

    /// Debug: per slot, the `LightParams` ids the picked sphere and the global name, and the blend.
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
        // The same zero-match tail as `sample_blended` ([`FALLBACK_LIGHT_ID`]), so a map with no
        // rows resolves the same record on both paths.
        local.or(global).or_else(|| {
            (!any)
                .then(|| self.lights.iter().find(|l| l.id == FALLBACK_LIGHT_ID))
                .flatten()
        })
    }

    /// Whether `LightParams` `p` has band rows in this chain, the client's `maxId` guard on the
    /// fixed magma and slime rows; checks the fog-colour row, which every real record carries.
    fn has_param(&self, p: u32) -> bool {
        p >= 1
            && self
                .int_bands
                .contains_key(&((p - 1) * 18 + IB_FOG_COLOR + 1))
    }

    fn sample_param(&self, p: u32, t: u32) -> Atmosphere {
        let ib = |b: u32| self.int_bands.get(&((p - 1) * 18 + b + 1));
        let fb = |b: u32| self.float_bands.get(&((p - 1) * 6 + b + 1));
        // `d` serves only the record fields; a keyless band row takes the zero-key constant.
        let d = Atmosphere::DEFAULT;
        // Only the fog end is stored × 36. The reference scales it once at load (`0x53f504` →
        // `0x6d6160` → `0x6d6100` runs `0x6d6090`, × 1/36 at `0x7ff9d0`, on rows with
        // `rowIndex % 6 == 0`); scaling the interpolated value here is the same. A 0 end (params 9
        // and 93) stays 0, kept finite by the shaders' `max(end − start, 0.001)`.
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
            // Unclamped: negative under storm (Elwynn −0.5), the reference's near veil in rain.
            fog_start_frac,
            fog_color: col(IB_FOG_COLOR),
            sun_diffuse: col(IB_DIFFUSE),
            sun_color: col(IB_SUN_COLOR),
            ambient: col(IB_AMBIENT),
            sky: std::array::from_fn(|i| col(IB_SKY0 + i as u32)),
            // Water-tint rows, the swatch endpoints (`0x68a830`). An unkeyed row is black, as in
            // the reference; 53 clear-slot spheres on maps 0 and 1 leave one unkeyed.
            water_river: [col(IB_RIVER_SHALLOW), col(IB_RIVER_DEEP)],
            water_ocean: [col(IB_OCEAN_SHALLOW), col(IB_OCEAN_DEEP)],
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
            glow: self.light_params_glow.get(&p).copied().unwrap_or(d.glow),
            // Cloud density and the three cloud palette rows, gathered by `0x6d64d0`.
            cloud_density: fb(FB_CLOUD_DENSITY)
                .and_then(|b| sample_float(b, t))
                .unwrap_or(ZERO_KEY_SCALAR),
            cloud_colors: [col(IB_CLOUD_SUN), col(IB_CLOUD_SLOPE), col(IB_CLOUD_GBASE)],
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

    #[test]
    fn the_ocean_ramp_runs_over_thirty_yards_and_then_holds() {
        let f = |z: f32| Submersion::Ocean.ocean_depth_factors(z).unwrap();
        let (a, b) = f(0.0);
        assert_eq!(
            (a, b),
            (1.0, 1.0),
            "at the surface the ramp is the identity"
        );
        // Above the surface it clamps to the identity: the ramp never brightens.
        assert_eq!(f(12.0), (1.0, 1.0));
        let (a30, b30) = f(-30.0);
        assert!((a30 - 0.5).abs() < 1e-6 && (b30 - 0.75).abs() < 1e-6);
        assert_eq!(f(-100.0), (a30, b30), "below 30 yd it holds flat");
        let (a15, b15) = f(-15.0);
        assert!((a15 - 0.75).abs() < 1e-6 && (b15 - 0.875).abs() < 1e-6);
        assert!(((1.0 - a15) - 2.0 * (1.0 - b15)).abs() < 1e-6);
    }

    #[test]
    fn the_ramp_multiplies_by_the_binarys_own_reciprocal() {
        assert_eq!(OCEAN_RAMP_RECIP.to_bits(), 0xbd088889);
        assert_eq!(
            OCEAN_RAMP_RECIP,
            -1.0f32 / 30.0,
            "the constant itself is just the nearest f32 to -1/30"
        );
        // −29 yd is a depth where the two forms differ.
        let t = -29.0f32;
        assert_ne!(
            1.0 - t * OCEAN_RAMP_RECIP,
            1.0 + t / 30.0,
            "multiply and divide must be observably different, or the warning is empty"
        );
        assert_eq!(
            Submersion::Ocean.ocean_depth_factors(-30.0).unwrap(),
            (0.5, 0.75)
        );
    }

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
        // The `(0,0,0)` global sentinel maps to the map centre (the loader special-cases it).
        assert_eq!(dbc_to_world(0.0, 0.0, 0.0), [17066.666, 17066.666, 0.0]);

        // Light #51 at (-8480, 548, 81), through the inverse transform and back.
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
        // The binary's "no active world light" glow.
        assert_eq!(Atmosphere::DEFAULT.glow, 0.5);
        // Duskwood's 0.50 against a 0.85 area.
        let a = Atmosphere {
            glow: 0.50,
            ..Atmosphere::DEFAULT
        };
        let b = Atmosphere {
            glow: 0.85,
            ..Atmosphere::DEFAULT
        };
        assert!((a.lerp(&b, 0.5).glow - 0.675).abs() < 1e-6);
        // floor(glow·255)/255: Elwynn's 0.65 gives the reference's 165/255 composite weight.
        let q = |g: f32| (g * 255.0).floor() / 255.0;
        assert!((q(0.65) - 165.0 / 255.0).abs() < 1e-6);
        assert!((q(0.50) - 127.0 / 255.0).abs() < 1e-6); // Duskwood → 0.498
    }
}
