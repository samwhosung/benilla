//! Ground-effect doodads: the grass tufts, weeds, flowers and pebbles scattered over the terrain.
//!
//! An MCNK texture layer's `effectId` (`0xFFFFFFFF` none) indexes `GroundEffectTexture.dbc`, which
//! gives four `GroundEffectDoodad.dbc` slots and a density. Which layer a cell uses is authored in
//! two per-cell 8×8 grids in the MCNK header: `predominantTexture` (`0x40`) names the layer, and
//! `noEffectDoodad` (`0x50`) disables the cell.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::terrain::ChunkMesh;

const GROUND_EFFECT_TEXTURE: &str = "DBFilesClient\\GroundEffectTexture.dbc";
const GROUND_EFFECT_DOODAD: &str = "DBFilesClient\\GroundEffectDoodad.dbc";
/// Detail-doodad models: the DBC names a bare `*.mdl`, the file is `<stem>.m2` here, absent from
/// the MPQ listfile but readable by exact path.
const DETAIL_DIR: &str = "World\\NoDXT\\Detail\\";

/// The empty slot is `0xFFFFFFFF` alone: 0 is a real `internalId`, `ElwFlo01.mdl`'s, which 16
/// shipped slots name.
fn is_no_doodad(id: u32) -> bool {
    id == 0xFFFF_FFFF
}

/// One `GroundEffectTexture.dbc` row: the detail doodads and density for an `effectId`.
#[derive(Clone, Debug)]
pub struct GroundEffect {
    /// The four `doodadId` slots in order, resolved to model paths; `None` places nothing. Slot
    /// order and duplicates are kept: the slot pattern is the weighting.
    pub doodads: [Option<String>; 4],
    /// Doodads per visited cell, up to about 24 as shipped.
    pub density: u32,
}

impl GroundEffect {
    /// The non-empty model paths, for diagnostics.
    pub fn models(&self) -> Vec<&str> {
        self.doodads.iter().filter_map(|d| d.as_deref()).collect()
    }
}

/// `effectId` to [`GroundEffect`], from the two ground-effect DBCs.
pub struct GroundEffectCatalog {
    effects: HashMap<u32, GroundEffect>,
}

impl GroundEffectCatalog {
    /// A terrain layer's effect; a row that resolves no model is absent.
    pub fn effect(&self, effect_id: u32) -> Option<&GroundEffect> {
        self.effects.get(&effect_id)
    }

    /// Effect rows that resolve at least one model.
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// One placed ground-clutter doodad. It stands upright with only a yaw, unlike an MDDF
/// [`crate::Doodad`] and its rotation convention.
#[derive(Clone, Debug)]
pub struct GroundDoodadPlacement {
    pub model: String,
    /// WoW world coords (X north, Y west, Z up), on the terrain surface.
    pub position: [f32; 3],
    /// Radians about the up axis, random: clutter has no authored facing.
    pub yaw: f32,
    /// Uniform scale, the reference's random `[0.9, 1.1]`.
    pub scale: f32,
}

/// The reference's detail-doodad noise table, a permutation of 0..255, as Noggit Red's
/// `ChunkAddDetailDoodads.cpp` recovers it.
#[rustfmt::skip]
pub(crate) const GROUND_EFFECT_NOISE: [u8; 256] = [
    0x8e, 0x14, 0x27, 0x99, 0xfd, 0xaa, 0xc7, 0x08, 0xd5, 0xe6, 0x3e, 0x1f, 0xf6, 0xbb, 0x55, 0xda,
    0x75, 0xa0, 0x4a, 0x6a, 0xe8, 0xbd, 0x97, 0xff, 0xde, 0x9b, 0xbc, 0x9f, 0x81, 0x8a, 0xa1, 0x46,
    0x6e, 0x0b, 0xe3, 0x63, 0x76, 0x7a, 0x6c, 0x5d, 0x88, 0xd3, 0x69, 0xca, 0xc3, 0x47, 0xb9, 0x25,
    0x83, 0xab, 0xa2, 0x3f, 0xa6, 0x41, 0x7c, 0xba, 0xe5, 0xac, 0x95, 0x01, 0x7e, 0xcf, 0x09, 0xc1,
    0xd9, 0x62, 0x70, 0x71, 0x8d, 0xdb, 0x05, 0x02, 0x24, 0x87, 0xef, 0x54, 0xc6, 0xd4, 0x37, 0x30,
    0xd0, 0x1b, 0xcb, 0x7b, 0xb8, 0xe4, 0xd8, 0xec, 0x49, 0xce, 0xad, 0xdc, 0x13, 0xa9, 0x94, 0xc4,
    0x8f, 0x39, 0xae, 0x0d, 0x18, 0x52, 0xdd, 0x0e, 0x78, 0xfa, 0xf5, 0x85, 0x58, 0xd2, 0xaf, 0x6d,
    0xa4, 0xb2, 0x53, 0x3b, 0x51, 0xa5, 0x50, 0xbe, 0xfc, 0x2d, 0xf4, 0x11, 0x48, 0x98, 0x16, 0xf1,
    0x86, 0xdf, 0x3d, 0x66, 0x5e, 0x44, 0x2e, 0x2f, 0x36, 0x07, 0x6b, 0x17, 0x8b, 0x29, 0x4c, 0xb6,
    0xe2, 0x89, 0x5f, 0xe7, 0xcd, 0xa7, 0x21, 0xe1, 0x4d, 0xc9, 0x65, 0xed, 0xfe, 0xee, 0x9c, 0x23,
    0x33, 0x7d, 0xb7, 0x04, 0x9e, 0x9a, 0x2a, 0x40, 0xb3, 0x10, 0x5b, 0xf3, 0x82, 0x77, 0x1c, 0x92,
    0x20, 0x4e, 0x1e, 0x57, 0x22, 0x72, 0x06, 0x8c, 0x67, 0x2c, 0x73, 0xfb, 0x59, 0xc2, 0x0a, 0xbf,
    0x79, 0x5c, 0xf9, 0x0c, 0x28, 0x1a, 0x12, 0x68, 0x74, 0x34, 0x19, 0x42, 0xb1, 0xc0, 0x84, 0xf8,
    0x38, 0xf0, 0x15, 0x9d, 0x60, 0xf2, 0x3a, 0x6f, 0xb4, 0x90, 0xeb, 0x91, 0x1d, 0x7f, 0x35, 0x61,
    0x5a, 0x32, 0x03, 0x56, 0xa3, 0xc5, 0x2b, 0x93, 0x80, 0x0f, 0x4b, 0x43, 0xf7, 0xa8, 0xe0, 0x3c,
    0x96, 0xd1, 0x64, 0x26, 0xd7, 0x45, 0xcc, 0x4f, 0xc8, 0xb0, 0xe9, 0xb5, 0x00, 0xd6, 0x31, 0xea,
];

/// The reference's detail-doodad PRNG (`0x4531e0`, seed expanded by `0x44ea80`), seeded once per
/// chunk; one stream drives both scatter passes.
struct BlizzardRandomizer {
    source: u32,
    seed: u32,
}

impl BlizzardRandomizer {
    fn new(source: u32) -> Self {
        let seed = ((source % 0x2F) << 26)
            | ((source % 0x35) << 18)
            | ((source % 0x3B) << 10)
            | (4 * (source % 0x3D));
        Self { source, seed }
    }

    /// Little-endian `u32` at byte `i` of the noise table; the lanes keep `i` at most 251.
    fn noise32(i: u8) -> u32 {
        let b = |k: u8| GROUND_EFFECT_NOISE[i.wrapping_add(k) as usize] as u32;
        b(0) | (b(1) << 8) | (b(2) << 16) | (b(3) << 24)
    }

    /// Advance and return the next `source`.
    fn shuffle(&mut self) -> u32 {
        // A negative lane index adds the lane's own constant, not a mod-256 wrap (`0x4531e0`); the
        // result stays in `[0, 251]`.
        let lane = |byte: u32, sub: i32, wrap: i32| -> u8 {
            let idx = (byte & 0xFF) as i32 - sub;
            (if idx < 0 { idx + wrap } else { idx }) as u8
        };
        let a = lane(self.seed, 0x1C, 0xF4);
        let b = lane(self.seed >> 8, 0x18, 0xEC);
        let c = lane(self.seed >> 16, 0x0C, 0xD4);
        let d = lane(self.seed >> 24, 0x04, 0xBC);
        self.seed = (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24);
        self.source = self.source.wrapping_add(
            Self::noise32(a)
                ^ Self::noise32(d).rotate_left(1)
                ^ Self::noise32(c).rotate_left(2)
                ^ Self::noise32(b).rotate_left(3),
        );
        self.source
    }

    /// The reference's `genCoord`, for in-cell position, scale and yaw: a mantissa float in
    /// `[1, 2)`, mapped by the roll's sign bit to `(0, 1]` or `[-1, 0)`.
    fn signed_unit(&mut self) -> f32 {
        let u = self.shuffle();
        let f = f32::from_bits((u & 0x007F_FFFF) | 0x3F80_0000); // [1, 2)
        if (u as i32) < 0 {
            2.0 - f // (0, 1]
        } else {
            f - 2.0 // [-1, 0)
        }
    }
}

/// The reference's `frillDensity` default (`CVar::Register` at `0x68862e`, record `[0xc7f2f4]`):
/// cells visited per chunk, drawn with repeats, so clutter is patchy. `SetWorldDetail`
/// (`0x488dd0`) writes 16, 32 or 48 per slider stop, hence the scatter's multiplier.
pub const FRILL_DENSITY: u32 = 16;

/// The top of the reference's `frillDensity` range: its change callback (`0x688de0`) clamps to
/// `[1, 256]`, though the renderer saturates at 128 (`0x6b1d4b` caps `frillDensity << 6` at
/// `0x2000` instances). The scatter's clamp and `benilla-world`'s `set_frill_density` share it.
pub const FRILL_DENSITY_MAX: u32 = 256;

/// Scatter ground clutter over one terrain chunk, a port of the reference's
/// `CMapChunk::CreateDetailDoodads` (`0x6bfc10`): one RNG stream per chunk, seeded by the global
/// chunk coords `(globalChunkY << 16) | globalChunkX`; pass 1 picks `frillDensity` cells
/// `(rng & 7, rng & 7)` with repeats, and pass 2 spawns `density` doodads in each, slot
/// `(n + listIndex) & 3`, from the layer [`ChunkMesh::pred_tex`] names, never MCAL alpha.
/// `density_scale` multiplies [`FRILL_DENSITY`].
pub fn scatter_ground_doodads(
    chunk: &ChunkMesh,
    catalog: &GroundEffectCatalog,
    tile_x: u32,
    tile_y: u32,
    density_scale: f32,
) -> Vec<GroundDoodadPlacement> {
    if chunk.positions.len() < 145 || density_scale <= 0.0 {
        return Vec::new();
    }
    let frill = ((FRILL_DENSITY as f32 * density_scale).round() as i64)
        .clamp(0, i64::from(FRILL_DENSITY_MAX)) as u32;
    if frill == 0 {
        return Vec::new();
    }
    // MCVT's stride-17 interleave: corner `(r, c)` at `r * 17 + c`, the cell's centre vertex at
    // `r * 17 + 9 + c`.
    let outer = |row: usize, col: usize| chunk.positions[row * 17 + col];
    let inner = |row: usize, col: usize| chunk.positions[row * 17 + 9 + col];
    let n_layers = chunk.layer_textures.len();

    let global_x = tile_x * 16 + chunk.index_x;
    let global_y = tile_y * 16 + chunk.index_y;
    let mut rng = BlizzardRandomizer::new((global_y << 16) | (global_x & 0xFFFF));

    // Pass 1: `frill` cells, two draws each, with repeats; never dedup.
    let cells: Vec<(usize, usize)> = (0..frill)
        .map(|_| {
            let col = (rng.shuffle() & 7) as usize; // cellX
            let row = (rng.shuffle() & 7) as usize; // cellY
            (row, col)
        })
        .collect();

    // Pass 2: those cells, in order.
    let mut out = Vec::new();
    for (list_index, &(row, col)) in cells.iter().enumerate() {
        let k = row * 8 + col;
        if chunk.no_effect_doodad[k] {
            continue; // the authored no-clutter mask (roads, clearings)
        }
        // No clutter over a holed 2×2 block, where no surface is drawn.
        if chunk.holes & (1u16 << ((row >> 1) * 4 + (col >> 1))) != 0 {
            continue;
        }
        let layer = chunk.pred_tex[k] as usize;
        if layer >= n_layers {
            continue;
        }
        let Some(eff) = chunk
            .layer_effect_ids
            .get(layer)
            .and_then(|&eid| catalog.effect(eid))
        else {
            continue; // no effect, or none of its slots resolves: the cell stays bare
        };
        let density = if eff.density == 0 { 8 } else { eff.density };
        // Cell verts in our (row, col) frame: row steps south (-X), col steps east (-Y).
        let tl = outer(row, col);
        let tr = outer(row, col + 1);
        let bl = outer(row + 1, col);
        let br = outer(row + 1, col + 1);
        let ctr = inner(row, col);
        for n in 0..density as usize {
            // The reference's draw order: rx and ry always, scale and yaw only for a placed model.
            let rx = rng.signed_unit();
            let ry = rng.signed_unit();
            let Some(model) = eff.doodads[(n + list_index) & 3].as_deref() else {
                continue;
            };
            let scale = rng.signed_unit() * 0.1 + 1.0; // [0.9, 1.1]
            let yaw = rng.signed_unit() * std::f32::consts::PI; // [-π, π]
            let (fx, fy) = ((rx + 1.0) * 0.5, (ry + 1.0) * 0.5);
            let position = fan_point(fx, fy, tl, tr, bl, br, ctr);
            out.push(GroundDoodadPlacement {
                model: model.to_string(),
                position,
                yaw,
                scale,
            });
        }
    }
    out
}

/// The point at `(fx, fy)` in `[0, 1]²` (east, south) of an MCNK cell, on the terrain's drawn
/// 4-triangle centre fan; a corner bilinear would sink clutter under a raised centre vertex.
fn fan_point(
    fx: f32,
    fy: f32,
    tl: [f32; 3],
    tr: [f32; 3],
    bl: [f32; 3],
    br: [f32; 3],
    ctr: [f32; 3],
) -> [f32; 3] {
    // (u, v, w) weights and the two corner vertices for the wedge.
    let (u, v, w, va, vb) = if fx + fy <= 1.0 {
        if fy <= fx {
            (2.0 * fy, 1.0 - fx - fy, fx - fy, tl, tr)
        } else {
            (2.0 * fx, fy - fx, 1.0 - fx - fy, bl, tl)
        }
    } else if fy >= fx {
        (2.0 * (1.0 - fy), fx + fy - 1.0, fy - fx, br, bl)
    } else {
        (2.0 * (1.0 - fx), fx - fy, fx + fy - 1.0, tr, br)
    };
    [
        u * ctr[0] + v * va[0] + w * vb[0],
        u * ctr[1] + v * va[1] + w * vb[1],
        u * ctr[2] + v * va[2] + w * vb[2],
    ]
}

fn texture_schema() -> Schema {
    let mut s = Schema::new("GroundEffectTexture");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    for i in 0..4 {
        s.add_field(SchemaField::new(format!("Doodad{i}"), FieldType::UInt32));
    }
    s.add_field(SchemaField::new("Density", FieldType::UInt32));
    s.add_field(SchemaField::new("Sound", FieldType::UInt32));
    s
}

fn doodad_schema() -> Schema {
    let mut s = Schema::new("GroundEffectDoodad");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("InternalId", FieldType::UInt32));
    s.add_field(SchemaField::new("ModelPath", FieldType::String));
    s
}

/// A DBC doodad name, a bare `ElwGra01.mdl`, to its model `World\NoDXT\Detail\ElwGra01.m2`.
fn resolve_model(dbc_name: &str) -> String {
    let file = dbc_name.rsplit(['\\', '/']).next().unwrap_or(dbc_name);
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    format!("{DETAIL_DIR}{stem}.m2")
}

/// Load both ground-effect DBCs off the patch chain. The reference matches `doodadId` against
/// `GroundEffectDoodad.internalId` (field 1), one less than the row `ID` in 5875: `true` is
/// faithful, `false` keys by row `ID`, an off-by-one kept only for A/B comparison.
pub fn load_ground_effect_catalog(
    chain: &mut Chain,
    key_by_internal_id: bool,
) -> Result<GroundEffectCatalog> {
    let key_field = if key_by_internal_id { 1 } else { 0 };
    let doodads = {
        let bytes = chain
            .read_file(GROUND_EFFECT_DOODAD)
            .with_context(|| format!("reading {GROUND_EFFECT_DOODAD}"))?;
        let rs = parse(&bytes, doodad_schema(), "GroundEffectDoodad")?;
        let mut m = HashMap::with_capacity(rs.records().len());
        for r in rs.records() {
            if let (Some(id), Some(name)) = (u32_at(r, key_field), str_at(&rs, r, 2)) {
                m.insert(id, resolve_model(&name));
            }
        }
        m
    };

    let bytes = chain
        .read_file(GROUND_EFFECT_TEXTURE)
        .with_context(|| format!("reading {GROUND_EFFECT_TEXTURE}"))?;
    let rs = parse(&bytes, texture_schema(), "GroundEffectTexture")?;
    let mut effects = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut slots: [Option<String>; 4] = Default::default();
        for (i, slot) in slots.iter_mut().enumerate() {
            let d = u32_at(r, 1 + i).unwrap_or(0);
            if !is_no_doodad(d) {
                *slot = doodads.get(&d).cloned();
            }
        }
        if slots.iter().all(Option::is_none) {
            continue; // a terrain-type row: footsteps and sound, no doodads
        }
        let density = u32_at(r, 5).unwrap_or(0);
        effects.insert(
            id,
            GroundEffect {
                doodads: slots,
                density,
            },
        );
    }
    Ok(GroundEffectCatalog { effects })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_forces_detail_m2_path() {
        assert_eq!(
            resolve_model("ElwGra01.mdl"),
            "World\\NoDXT\\Detail\\ElwGra01.m2"
        );
        assert_eq!(
            resolve_model("ElwFlo01.MDX"),
            "World\\NoDXT\\Detail\\ElwFlo01.m2"
        );
    }

    #[test]
    fn no_doodad_sentinels() {
        assert!(is_no_doodad(0xFFFF_FFFF));
        assert!(!is_no_doodad(0));
        assert!(!is_no_doodad(42));
    }

    #[test]
    fn noise_table_is_a_permutation() {
        let mut seen = [false; 256];
        for &b in &GROUND_EFFECT_NOISE {
            seen[b as usize] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "noise table must be a 0..255 permutation"
        );
    }

    #[test]
    fn randomizer_is_deterministic_and_bounded() {
        let (mut a, mut b) = (
            BlizzardRandomizer::new(0x0030_0020),
            BlizzardRandomizer::new(0x0030_0020),
        );
        for _ in 0..1000 {
            assert_eq!(a.shuffle(), b.shuffle());
        }
        assert_ne!(
            BlizzardRandomizer::new(1).shuffle(),
            BlizzardRandomizer::new(2).shuffle()
        );
        let mut r = BlizzardRandomizer::new(12345);
        for _ in 0..10_000 {
            let c = r.signed_unit();
            assert!((-1.0..1.0).contains(&c), "signed_unit out of range: {c}");
        }
    }
}
