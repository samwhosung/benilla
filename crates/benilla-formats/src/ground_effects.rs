//! Ground-effect doodads — the little grass tufts, weeds, flowers and pebbles scattered over the
//! terrain. This is the dense ground clutter that makes the real client's outdoors look lush; without
//! it our ground is bare texture.
//!
//! Mechanism (verified against build 5875 + wowdev.wiki ADT/v18, 2026-05-26):
//! - Each terrain texture layer in an MCNK carries an `effectId` ([`benilla_adt`] `TextureLayer.effect_id`;
//!   `0xFFFFFFFF` = none) that indexes [`GroundEffectTexture.dbc`].
//! - [`GroundEffectTexture.dbc`] (5875: **7 fields / 28 B** — `ID, doodadId[4], density, sound`; no
//!   per-doodad weights, those arrive in 6.0) gives up to four [`GroundEffectDoodad.dbc`] refs + a
//!   `density` (0→8, the client caps ~24: amount, then coverage/grouping).
//! - [`GroundEffectDoodad.dbc`] (5875: **3 fields / 12 B** — `ID, internalId, modelPath`) names the
//!   model as a bare `*.mdl` filename; on disk in this client the real model is
//!   `World\NoDXT\Detail\<stem>.m2` (an `MD20` M2 — verified `ElwGra01.m2` extracts and loads). These
//!   models are **not in the MPQ listfile** but extract fine by exact path.
//!
//! Which cell gets which doodad set is **authored**, in two per-cell 8×8 grids in the MCNK header
//! (decoded onto [`ChunkMesh::pred_tex`] / [`ChunkMesh::no_effect_doodad`]): `predominantTexture`
//! (0x40) names the layer whose `effectId` applies, `noEffectDoodad` (0x50) hard-disables the cell.
//! The exact intra-cell scatter positions are client-internal, so the renderer scatters
//! deterministically at the authored density. This module owns the **data** (`effectId → (models,
//! density)`) and the faithful per-cell [`scatter_ground_doodads`].

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::terrain::ChunkMesh;

const GROUND_EFFECT_TEXTURE: &str = "DBFilesClient\\GroundEffectTexture.dbc";
const GROUND_EFFECT_DOODAD: &str = "DBFilesClient\\GroundEffectDoodad.dbc";
/// Detail-doodad models live here; the DBC stores a bare `*.mdl` name, the on-disk file is `.m2`.
const DETAIL_DIR: &str = "World\\NoDXT\\Detail\\";

/// A "none" doodad slot. Vanilla 5875 uses **only `0xFFFFFFFF` (the wiki's "-1")** as the empty
/// sentinel — `0` is a valid `internalId`, owned by `ElwFlo01.mdl` (the dark-red branched flower
/// in Elwynn). Empirically: across `GroundEffectTexture.dbc` (12 742 rows × 4 slots = 50 968),
/// 48 083 slots are `0xFFFFFFFF`, only 16 are `0`, and all 16 resolve to `ElwFlo01` — so `0` is
/// definitely an id, not a sentinel. (Treating it as empty silently dropped every `ElwFlo01`
/// placement, e.g. the slot-0 entry on the 12 Elwynn effects 507/720/734/741/747/753/761/765/
/// 777/801/805/868.)
fn is_no_doodad(id: u32) -> bool {
    id == 0xFFFF_FFFF
}

/// The detail doodads + density for one `effectId` (a [`GroundEffectTexture.dbc`] row).
#[derive(Clone, Debug)]
pub struct GroundEffect {
    /// The 4 `doodadId` slots in order, resolved to `World\NoDXT\Detail\*.m2` paths. `None` = an
    /// empty slot (`0`/`-1`); the client treats those as "place nothing" so the slot pattern (incl.
    /// duplicates) acts as the doodad weighting. Kept in slot order (not deduped) for that reason.
    pub doodads: [Option<String>; 4],
    /// Authored density (`0`..~`24`): per-cell doodad count; higher = denser, then grouped.
    pub density: u32,
}

impl GroundEffect {
    /// The non-empty resolved model paths (diagnostics; the placement uses [`Self::doodads`] directly).
    pub fn models(&self) -> Vec<&str> {
        self.doodads.iter().filter_map(|d| d.as_deref()).collect()
    }
}

/// `effectId → GroundEffect` from the GroundEffect* DBCs. Query with [`GroundEffectCatalog::effect`].
pub struct GroundEffectCatalog {
    effects: HashMap<u32, GroundEffect>,
}

impl GroundEffectCatalog {
    /// The doodads + density for a terrain layer's `effectId`, or `None` (no effect / no models).
    pub fn effect(&self, effect_id: u32) -> Option<&GroundEffect> {
        self.effects.get(&effect_id)
    }

    /// Number of effect rows that resolve to at least one model (diagnostics).
    pub fn len(&self) -> usize {
        self.effects.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
}

/// One placed ground-clutter doodad: a small upright `World\NoDXT\Detail\*.m2` with a random heading.
/// Unlike [`crate::Doodad`] (MDDF placement matrix) these stand straight with just a yaw, like a
/// creature — so they get their own type to avoid the MDDF rotation convention.
#[derive(Clone, Debug)]
pub struct GroundDoodadPlacement {
    pub model: String,
    /// Position in WoW world coords (X north, Y west, Z up), on the terrain surface.
    pub position: [f32; 3],
    /// Heading in radians about the up axis (random — clutter has no authored facing).
    pub yaw: f32,
    /// Per-instance uniform scale, the client's random `[0.9, 1.1]` (`signed·0.1 + 1.0`).
    pub scale: f32,
}

/// The vanilla client's 256-byte detail-doodad noise table (a permutation of 0..255), recovered from
/// `Marlamin/noggit-red`'s `ChunkAddDetailDoodads.cpp` (where it sits commented out beside an `'a'`
/// placeholder). Drives [`BlizzardRandomizer`].
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

/// The vanilla client's detail-doodad PRNG (`FUN_004531e0` + the `FUN_0044ea80` seed-expand, a.k.a. the
/// Noggit `BlizzardRandomizer`). Seeded **once per chunk** from the chunk's global coords; the same
/// stream drives PASS-1 cell picks and PASS-2 positions/scale/yaw, so the layout is bit-exact to the
/// client and reload-stable. The lane math (`shuffle`) reproduces the binary exactly — see
/// [`scatter_ground_doodads`].
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

    /// Little-endian `u32` read at byte offset `i` of the noise table. The 4-byte read wraps past 255
    /// (circular); the client's exact boundary behaviour at `i ≥ 253` is unconfirmed (rare, ~1%).
    fn noise32(i: u8) -> u32 {
        let b = |k: u8| GROUND_EFFECT_NOISE[i.wrapping_add(k) as usize] as u32;
        b(0) | (b(1) << 8) | (b(2) << 16) | (b(3) << 24)
    }

    /// Advance and return the next `source`. (`rol(v, 0) == v`.)
    fn shuffle(&mut self) -> u32 {
        // Per-lane index = byte − sub, and **when that goes negative add the lane's specific constant**
        // (`0xf4/0xec/0xd4/0xbc`) — NOT a mod-256 wrap. This is the client's exact `FUN_004531e0` math
        // (verified to the instruction); the result stays in `[0, 251]` so the 4-byte noise read is
        // always in-bounds. (Using `wrapping_sub` diverges on ~11–16% of draws.)
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

    /// A signed uniform float in `(-1, 1)` — the client's `genCoord` output used for in-cell position,
    /// scale, and yaw: take the mantissa float in `[1, 2)`, then map by the roll's sign bit.
    fn signed_unit(&mut self) -> f32 {
        let u = self.shuffle();
        let f = f32::from_bits((u & 0x007F_FFFF) | 0x3F80_0000); // [1, 2)
        if (u as i32) < 0 {
            2.0 - f // (0, 1] → mapped to (−1, 0]
        } else {
            f - 2.0 // [1, 2) → [−1, 0)  … together: (−1, 1)
        }
    }
}

/// The client's `frillDensity` CVar default — the number of (randomly-chosen, with-repeats) cells the
/// client visits per chunk. It is **not** all 64 cells — that's what makes the real clutter patchy
/// (some cells hit 2–3×, ~50 left bare) rather than an even carpet.
///
/// **Byte-read, not chosen**: `CVar::Register 0x63db90` at `0x68862e` passes name `0x8423d8`
/// `"frillDensity"`, default string `0x864644` `"16"`, help "Terrain frill density", record
/// `[0xc7f2f4]`. `SetWorldDetail 0x488dd0`
/// writes 16/32/48 over it per slider stop, so a stop IS a multiple of this constant — which is
/// why [`scatter_ground_doodads`] takes a multiplier and the CVar host converts at its edge
/// ([`crate::ground_effects::FRILL_DENSITY_MAX`] is that conversion's other bound).
pub const FRILL_DENSITY: u32 = 16;

/// The top of the reference's own `frillDensity` range — its change callback `0x688de0` clamps the
/// written value to `[1, 256]`, so this is the densest ground cover the real client will scatter
/// (its renderer saturates first: `0x6b1d4b` computes the detail-doodad instance count as
/// `frillDensity << 6` capped at `0x2000`, reached at 128). Named here because both the scatter's
/// own clamp below and the `frillDensity` CVar arm in `benilla-app` have to agree on it.
pub const FRILL_DENSITY_MAX: u32 = 256;

/// Scatter ground-clutter doodads over one terrain chunk — a **bit-exact port of the client's
/// `CMapChunk::CreateDetailDoodads`** (RE'd from `WoW.exe` @ `0x6bfc10`). One RNG stream per chunk,
/// seeded by **global chunk coords**
/// `(globalChunkY << 16) | globalChunkX`, then two passes:
///
/// - **PASS 1** picks `frillDensity` (×`density_scale`) random cells `(rng&7, rng&7)` — **with repeats,
///   no dedup** (2 RNG draws each). This is the source of the reference's patchiness: cells drawn more
///   than once get extra doodads, the ~50 unpicked cells stay bare.
/// - **PASS 2** visits exactly those cells in order. Per cell: skip if [`ChunkMesh::no_effect_doodad`];
///   the layer named by [`ChunkMesh::pred_tex`] (`predominantTexture`, **never** MCAL alpha) gives the
///   MCLY `effectId` → its [`GroundEffect`]. Then spawn `density` doodads; per doodad the exact draw
///   order is **rx, ry (always), then — only if the slot isn't empty — scale, yaw**. Slot is
///   `(n + listIndex) & 3` of the effect's 4 `doodadId`s; an **empty (−1/None) slot places nothing**
///   but still consumes the rx/ry draws (matching the client, so the stream stays aligned).
///
/// Empty-slot effects are *why bare-earth cells stay clear*: a layer whose effect is all-−1 resolves to
/// `None` (we drop those from the catalog). `density_scale` (debug slider) scales the per-chunk cell
/// count = the client's `frillDensity` lever.
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
    // The chunk's 145 verts are MCVT's stride-17 interleave (B1):
    // outer (r,c) @ r·17+c, inner (r,c) @ r·17+9+c. Outer = the corner grid; inner = each cell's
    // CENTER vertex, carrying its own authored MCVT height (the bump that gives hilltops their
    // rise). Clutter heights interpolate via the 4-triangle center-fan, NOT a 4-corner bilinear:
    // matching the terrain mesh's subdivision so doodads sit flush on the surface (esp. over
    // hilltops, where a bilinear interp would float them under the rising inner vertex).
    let outer = |row: usize, col: usize| chunk.positions[row * 17 + col];
    let inner = |row: usize, col: usize| chunk.positions[row * 17 + 9 + col];
    let n_layers = chunk.layer_textures.len();

    // ONE RNG stream for the whole chunk, seeded by GLOBAL chunk coords (Y high, X low) — matches the
    // client, so the patch layout is globally unique (never repeats per tile) and reload-stable.
    let global_x = tile_x * 16 + chunk.index_x;
    let global_y = tile_y * 16 + chunk.index_y;
    let mut rng = BlizzardRandomizer::new((global_y << 16) | (global_x & 0xFFFF));

    // PASS 1: pick `frill` random cells (with repeats — do NOT dedup), 2 draws each.
    let cells: Vec<(usize, usize)> = (0..frill)
        .map(|_| {
            let col = (rng.shuffle() & 7) as usize; // cellX
            let row = (rng.shuffle() & 7) as usize; // cellY
            (row, col)
        })
        .collect();

    // PASS 2: visit exactly those cells in order; gate + spawn.
    let mut out = Vec::new();
    for (list_index, &(row, col)) in cells.iter().enumerate() {
        let k = row * 8 + col;
        if chunk.no_effect_doodad[k] {
            continue; // client's explicit "no clutter" mask (roads/clearings)
        }
        // Holes (B2): skip clutter over holed 2×2 blocks — the surface itself isn't drawn there.
        if chunk.holes & (1u16 << ((row >> 1) * 4 + (col >> 1))) != 0 {
            continue;
        }
        let layer = chunk.pred_tex[k] as usize;
        if layer >= n_layers {
            continue; // names a non-existent layer
        }
        let Some(eff) = chunk
            .layer_effect_ids
            .get(layer)
            .and_then(|&eid| catalog.effect(eid))
        else {
            continue; // effect is none / all-(−1)-slots → the cell stays bare
        };
        let density = if eff.density == 0 { 8 } else { eff.density };
        // Cell verts in our (row, col) frame: row steps south (-X), col steps east (-Y).
        let tl = outer(row, col);
        let tr = outer(row, col + 1);
        let bl = outer(row + 1, col);
        let br = outer(row + 1, col + 1);
        let ctr = inner(row, col);
        for n in 0..density as usize {
            // Exact client draw order: rx, ry ALWAYS consumed; scale/yaw only when a model is placed.
            let rx = rng.signed_unit();
            let ry = rng.signed_unit();
            let Some(model) = eff.doodads[(n + list_index) & 3].as_deref() else {
                continue; // empty (−1) slot: places nothing, but rx/ry are already consumed
            };
            let scale = rng.signed_unit() * 0.1 + 1.0; // [0.9, 1.1]
            let yaw = rng.signed_unit() * std::f32::consts::PI; // (−π, π)
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

/// Interpolate a point inside one MCNK cell at fractional `(fx, fy) ∈ [0, 1]²` (fx=east, fy=south)
/// across the **4-triangle center-fan** the terrain mesh itself draws — barycentric over the wedge
/// containing the point (CTR + two adjacent corners). The 4 wedges are bounded by the cell's two
/// diagonals (`fy=fx` and `fy=1-fx`). Matches the visible terrain so doodads sit flush on hilltops
/// (a 4-corner bilinear would sink them below the rising inner-center vertex).
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
            // North wedge: (CTR, TL, TR) — u_ctr=2·fy, v_tl=1−fx−fy, w_tr=fx−fy
            (2.0 * fy, 1.0 - fx - fy, fx - fy, tl, tr)
        } else {
            // West wedge: (CTR, BL, TL) — u_ctr=2·fx, v_bl=fy−fx, w_tl=1−fx−fy
            (2.0 * fx, fy - fx, 1.0 - fx - fy, bl, tl)
        }
    } else if fy >= fx {
        // South wedge: (CTR, BR, BL) — u_ctr=2·(1−fy), v_br=fx+fy−1, w_bl=fy−fx
        (2.0 * (1.0 - fy), fx + fy - 1.0, fy - fx, br, bl)
    } else {
        // East wedge: (CTR, TR, BR) — u_ctr=2·(1−fx), v_tr=fx−fy, w_br=fx+fy−1
        (2.0 * (1.0 - fx), fx - fy, fx + fy - 1.0, tr, br)
    };
    [
        u * ctr[0] + v * va[0] + w * vb[0],
        u * ctr[1] + v * va[1] + w * vb[1],
        u * ctr[2] + v * va[2] + w * vb[2],
    ]
}

/// GroundEffectTexture.dbc — 5875: `ID, doodadId[4], density, sound` (7 fields).
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

/// GroundEffectDoodad.dbc — 5875: `ID, internalId, modelPath` (3 fields).
fn doodad_schema() -> Schema {
    let mut s = Schema::new("GroundEffectDoodad");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("InternalId", FieldType::UInt32));
    s.add_field(SchemaField::new("ModelPath", FieldType::String));
    s
}

/// Resolve a DBC doodad string (a bare `ElwGra01.mdl`) to its on-disk model `World\NoDXT\Detail\
/// ElwGra01.m2`. Keeps only the file stem (defensive against any stray path) and forces `.m2`.
fn resolve_model(dbc_name: &str) -> String {
    let file = dbc_name.rsplit(['\\', '/']).next().unwrap_or(dbc_name);
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    format!("{DETAIL_DIR}{stem}.m2")
}

/// Load the GroundEffect* DBCs from the patch chain into a [`GroundEffectCatalog`].
///
/// `key_by_internal_id` selects how `GroundEffectTexture.doodadId[k]` is matched to a
/// `GroundEffectDoodad` row. **The faithful value is `true`:** the client (verified in `WoW.exe`)
/// treats `doodadId[k]` as a **`GroundEffectDoodad.internalId`**
/// (field 1) and indexes its model cache by it — NOT the DBC row `ID` (field 0). In 5875 the two
/// differ by exactly one (`rowID = internalId + 1`), so keying by row ID is an off-by-one that picks
/// the neighbouring species (e.g. the main Elwynn grass effect resolves a *flower* into slot 0 and
/// shifts the grass). `false` reproduces that old bug for A/B comparison only.
pub fn load_ground_effect_catalog(
    chain: &mut Chain,
    key_by_internal_id: bool,
) -> Result<GroundEffectCatalog> {
    // doodadId (an internalId) → resolved model path. Keyed by GroundEffectDoodad.internalId (field 1)
    // when faithful; field 0 (row ID) is the legacy off-by-one path.
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
        // Keep the 4 slots in order (not deduped): the client uses slot multiplicity as the weighting.
        let mut slots: [Option<String>; 4] = Default::default();
        for (i, slot) in slots.iter_mut().enumerate() {
            let d = u32_at(r, 1 + i).unwrap_or(0);
            if !is_no_doodad(d) {
                *slot = doodads.get(&d).cloned();
            }
        }
        if slots.iter().all(Option::is_none) {
            continue; // a "terrain type only" row (footsteps/sound, no doodads) — nothing to draw
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
        // 0xFFFFFFFF is the only empty marker; `0` is the valid internalId of ElwFlo01 (see the
        // function's doc-comment for the empirical tally that settles this).
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
        // Same seed → identical stream (clutter is stable every run).
        let (mut a, mut b) = (
            BlizzardRandomizer::new(0x0030_0020),
            BlizzardRandomizer::new(0x0030_0020),
        );
        for _ in 0..1000 {
            assert_eq!(a.shuffle(), b.shuffle());
        }
        // Different seeds diverge (different chunks ⇒ different layouts).
        assert_ne!(
            BlizzardRandomizer::new(1).shuffle(),
            BlizzardRandomizer::new(2).shuffle()
        );
        // signed_unit stays in (-1, 1) — the client's genCoord range.
        let mut r = BlizzardRandomizer::new(12345);
        for _ in 0..10_000 {
            let c = r.signed_unit();
            assert!((-1.0..1.0).contains(&c), "signed_unit out of range: {c}");
        }
    }
}
