//! ADT terrain → per-chunk triangle meshes (Phase 2 + Phase 6 splat).
//!
//! Output is in **raw WoW world coords** (+X north, +Y west, +Z up, yards); the renderer
//! (`benilla`) applies the WoW→Bevy transform. Each MCNK contributes one [`ChunkMesh`]
//! built as the binary's **145-vertex center-fan** (B1): 81 outer verts (9×9 corner grid) + 64 inner
//! center verts (8×8 at +½-cell horizontal in both planar axes), each cell rendered as 4
//! triangles fanning from its center to its 4 corners. Each inner vertex carries its **own**
//! authored MCVT center height (independent sample, NOT a corner average — dropping it was
//! the "flattened hilltops" defect). Holes (B2, `MCNK+0x3C`) omit a 2×2-cell block from the
//! index buffer; the underlying verts stay (unreferenced).
//!
//! Vertex layout mirrors MCVT's on-disk **stride-17 interleave** (row r: 9 outer then 8 inner;
//! final row 9 outer only): outer `(r,c)` for r,c ∈ 0..=8 is at `r·17+c`; inner `(r,c)` for
//! r,c ∈ 0..=7 is at `r·17+9+c`. Positions, normals (MCNR `[X,Y,Z]` raw, no swap), and
//! 0..1 UVs are parallel arrays in this order — the renderer splats up to 4 layers using a
//! per-chunk RGBA alpha map (R/G/B = layers 1/2/3 opacity).

use std::io::Cursor;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_adt::{parse_adt, CombinedAlphaMap, ParsedAdt};
use benilla_wdt::{version::WowVersion, WdtFile, WdtReader};

/// Edge length (texels) of a chunk's combined alpha map (always 64×64 in vanilla).
pub const ALPHA_MAP_SIZE: u32 = 64;

/// Edge length (texels) of a chunk's MCSH shadow map (always 64×64 in vanilla).
pub const SHADOW_MAP_SIZE: u32 = 64;

/// Yards per ADT tile (1/64 of a map edge).
pub const TILE_SIZE: f32 = 533.333_3;
/// Yards per MCNK chunk (16×16 chunks per tile) — also the grid the impassable-chunk wall runs on
/// (decision 1266), which is why it is public.
pub const CHUNK_SIZE: f32 = TILE_SIZE / 16.0;
/// Yards between adjacent outer-grid vertices (9×9 grid spans one chunk).
/// `pub(crate)` so the liquid mesher shares the exact same lattice spacing.
pub(crate) const UNIT_SIZE: f32 = CHUNK_SIZE / 8.0;

/// World (x, y) of Stormwind, from `TaxiNodes.dbc` — a handy anchor for "Elwynn".
pub const STORMWIND_XY: (f32, f32) = (-8840.56, 489.7);

/// Ground-texture repeats per chunk — how many times a layer texture tiles across one MCNK. The
/// binary's own value is 8 (each of the chunk's 8×8 cells carries one repeat), and it is not a
/// setting: a `$WOW_TEX_TILES` knob once let a capture try other values and was retired (2335)
/// because nothing in the reference tunes it.
pub const TERRAIN_LAYER_TILES: f32 = 8.0;

/// One MCNK chunk as an indexed triangle mesh in **raw WoW coords**, plus texturing data.
///
/// Vertex arrays (`positions`, `normals`, `uvs`) hold **145 entries** in MCVT's stride-17 order:
/// outer `(r,c)` at `r·17+c` (r,c ∈ 0..=8), inner `(r,c)` at `r·17+9+c` (r,c ∈ 0..=7). The
/// index buffer is the 4-triangle center-fan per cell (`(CTR,TL,BL),(CTR,BL,BR),(CTR,BR,TR),
/// (CTR,TR,TL)`), 256 tris / 768 indices for a chunk with no holes.
#[derive(Debug, Clone)]
pub struct ChunkMesh {
    /// Vertex positions `[x, y, z]` in WoW yards — 145 entries (81 outer + 64 inner). See the
    /// type-level doc for the stride-17 layout.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals (unit, WoW coords) decoded from MCNR, one per [`Self::positions`] entry.
    /// Authored continuously across chunk borders, so using these instead of recomputing per-chunk
    /// removes the lighting seams at MCNK boundaries. Empty if the chunk had no MCNR data.
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex texture coordinates (0..1 across the chunk; outer at `(c/8, r/8)`, inner at
    /// `((c+½)/8, (r+½)/8)`). Renderers scale `× `[`TERRAIN_LAYER_TILES`] for layer sampling and
    /// use the raw 0..1 for alpha/shadow.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle-list indices into `positions` (4-tri center-fan/cell, holed blocks omitted).
    pub indices: Vec<u32>,
    /// Low-res hole bitmap (`u16 @ MCNK+0x3C`). Bit `n = (cellY>>1)·4 + (cellX>>1)` set ⇒ that
    /// 2×2 block of the 8×8 cells is missing (caves/doorways). The mesher already omits the
    /// matching index ranges; downstream consumers (e.g. clutter) re-test it to skip placing
    /// surface decoration over a hole.
    pub holes: u16,
    /// MTEX filename of the chunk's base (layer 0) texture, if the chunk has layers.
    pub base_texture: Option<String>,
    /// MTEX filenames of all texture layers (1–4), layer 0 first. The renderer blends layers
    /// 1..n over layer 0 using [`Self::alpha_map`].
    pub layer_textures: Vec<String>,
    /// Per-layer `effectId` (MCLY), aligned with [`Self::layer_textures`]: a `GroundEffectTexture.dbc`
    /// reference selecting the ground-clutter doodads for that layer (`0xFFFFFFFF` = none).
    pub layer_effect_ids: Vec<u32>,
    /// Packed RGBA alpha map (`ALPHA_MAP_SIZE`²), R/G/B = opacity of layers 1/2/3 across the
    /// chunk (0..1 UV). `None` when the chunk has ≤1 layer (nothing to blend).
    pub alpha_map: Option<Vec<u8>>,
    /// Pre-baked terrain shadow map from MCSH: one byte per texel over a `SHADOW_MAP_SIZE`² grid
    /// (`255` = in shadow, `0` = lit), row-major over the chunk's 0..1 UV. This is WoW's **static**
    /// terrain shadowing (baked at map-compile time); `None` when the chunk has no MCSH.
    pub shadow: Option<Vec<u8>>,
    /// `predominantTexture` (MCNK 0x40, `uint2[8][8]`): for each 8×8 cell, the texture **layer
    /// index** (0..3) whose MCLY `effectId` selects that cell's ground-clutter doodads. Row-major,
    /// `k = row*8 + col` — same frame as the vertex grid (row steps south, col east). This is the
    /// client's *authoritative* per-cell doodad-layer map; verified to match the alpha layout
    /// cell-for-cell (`examples/clutter_probe`).
    pub pred_tex: [u8; 64],
    /// `noEffectDoodad` (MCNK 0x50, `uint1[8][8]`): `true` ⇒ the client draws **no** clutter on that
    /// cell (e.g. paved roads/clearings). Same `k = row*8 + col` indexing as [`Self::pred_tex`].
    ///
    /// ⚠️ wow-adt 0.6.4 mislabels both grids for vanilla: it splits the 16-byte predominantTexture
    /// into `pred_tex[8]` + `no_effect_doodad[8]` and calls the *real* noEffectDoodad `unknown_8bytes`.
    /// We reconstruct the correct grids from those preserved bytes (see `adt_to_tile_mesh`).
    pub no_effect_doodad: [bool; 64],
    /// This chunk's `(IndexX, IndexY)` within the 16×16 tile grid (MCNK header). Needed to seed the
    /// ground-clutter PRNG by global chunk position (`scatter_ground_doodads`), matching the client.
    pub index_x: u32,
    pub index_y: u32,
    /// `AreaTable.dbc` id of this chunk (MCNK +0x34) — the zone/subzone under the player's feet;
    /// drives zone music/ambience/reverb selection (decision 0070).
    pub area_id: u32,
    /// MCNK header flag bit 1 ([`benilla_adt::MCNK_IMPASSABLE`]): this chunk is authored
    /// **impassable** — the ADT-level invisible wall the reference stops a mover at (report B129,
    /// decision 1266). A header bit, so it covers the whole 33.333 yd chunk: the wall runs the
    /// chunk grid, not the terrain relief.
    pub impassable: bool,
    /// The chunk's MCLQ liquid surfaces — flat meshes built alongside the terrain (see
    /// [`crate::liquid`]). Empty on a dry chunk; **more than one** where the MCNK carries more than
    /// one liquid block, as the 28 shipped river-mouth chunks do (a stream over the sea).
    pub liquids: Vec<crate::liquid::LiquidMesh>,
}

impl ChunkMesh {
    /// Sample the baked MCSH terrain shadow at a **raw WoW-space** position (X north, Y west): the same
    /// per-texel lookup the terrain shader and clutter use, so a doodad/unit standing here inherits the
    /// ground's static shade. Returns `Some(true)` = shadowed, `Some(false)` = lit, and `None` when the
    /// position is outside this chunk's footprint (caller tries the next chunk). A chunk with no MCSH map
    /// is fully lit. Coord math mirrors the vertex grid: NW corner is `positions[0]`, +X is north (south
    /// distance = `nw_x − x`), +Y is west (east distance = `nw_y − y`), both `0..1` across the chunk.
    pub fn mcsh_shadowed_at(&self, wow: [f32; 3]) -> Option<bool> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - wow[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - wow[1]) / TILE_SIZE * 16.0;
        if !(0.0..=1.0).contains(&south) || !(0.0..=1.0).contains(&east) {
            return None;
        }
        let Some(shadow) = self.shadow.as_ref() else {
            return Some(false); // inside this chunk but no MCSH → lit
        };
        let n = SHADOW_MAP_SIZE as usize;
        let row = ((south * n as f32) as usize).min(n - 1);
        let col = ((east * n as f32) as usize).min(n - 1);
        Some(shadow.get(row * n + col).copied().unwrap_or(0) >= 128)
    }

    /// The `GroundEffectTexture` id under a **raw WoW-space** position, `None` when the position
    /// is outside this chunk (or the cell's layer carries no effect). Uses the chunk's own
    /// `predominantTexture` per-cell layer map — the client's authoritative per-cell selector
    /// (the same grid the ground clutter scatters by) — then that layer's MCLY `effectId`.
    /// Feeds the footstep terrain-type chain (decision 0070 slice 3).
    pub fn ground_effect_at(&self, wow: [f32; 3]) -> Option<u32> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - wow[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - wow[1]) / TILE_SIZE * 16.0;
        if !(0.0..=1.0).contains(&south) || !(0.0..=1.0).contains(&east) {
            return None;
        }
        let row = ((south * 8.0) as usize).min(7);
        let col = ((east * 8.0) as usize).min(7);
        let layer = self.pred_tex[row * 8 + col] as usize;
        let effect = self.layer_effect_ids.get(layer).copied()?;
        (effect != u32::MAX).then_some(effect)
    }

    /// The chunk's `AreaTable` id if the **raw WoW-space** position lies inside its footprint,
    /// else `None` (caller tries the next chunk). Same containment math as
    /// [`Self::mcsh_shadowed_at`].
    pub fn area_at(&self, wow: [f32; 3]) -> Option<u32> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - wow[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - wow[1]) / TILE_SIZE * 16.0;
        if (0.0..=1.0).contains(&south) && (0.0..=1.0).contains(&east) {
            Some(self.area_id)
        } else {
            None
        }
    }

    /// Is the **raw WoW-space** position inside this chunk's footprint, and is the chunk flagged
    /// [`benilla_adt::MCNK_IMPASSABLE`]? `None` = outside (caller tries the next chunk). Same
    /// containment math as [`Self::area_at`] — and the same reason it is a *footprint* test and not
    /// a height test: the flag has no z extent, so the answer is the same at every altitude.
    pub fn impassable_at(&self, wow: [f32; 3]) -> Option<bool> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - wow[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - wow[1]) / TILE_SIZE * 16.0;
        ((0.0..=1.0).contains(&south) && (0.0..=1.0).contains(&east)).then_some(self.impassable)
    }

    /// The terrain surface height (WoW `z`) under a **raw WoW-space** column, by exact interpolation
    /// of the covering center-fan triangle. `None` when the column lies outside this chunk's footprint
    /// (caller tries the next chunk) **or falls in an MCNK hole**.
    ///
    /// The hole case is load-bearing, not an edge case: a hole is how a mine/cave entrance is cut out
    /// of the ground, and it carries no triangle in [`Self::indices`] — so the query misses exactly
    /// where the client's terrain probe misses, handing that column to the WMO interior below it. Any
    /// implementation that interpolated a height across a hole would seal the entrance shut.
    pub fn height_at(&self, wow: [f32; 3]) -> Option<f32> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - wow[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - wow[1]) / TILE_SIZE * 16.0;
        if !(0.0..=1.0).contains(&south) || !(0.0..=1.0).contains(&east) {
            return None;
        }
        // The covering cell, then only its four fan triangles — the same corners, centre,
        // order and hole rule the index buffer was built from (`adt_to_tile_mesh`), so the
        // answer is the walk's answer without the walk: a column used to test up to 256
        // triangles, and the sun-flare march asks ~96 columns a frame (decision 1979).
        if self.positions.len() == 145 {
            let row = ((south * 8.0) as u32).min(7);
            let col = ((east * 8.0) as u32).min(7);
            if self.holes & (1u16 << ((row >> 1) * 4 + (col >> 1))) != 0 {
                return None;
            }
            let tl = (row * 17 + col) as usize;
            let (tr, bl, br, ctr) = (tl + 1, tl + 17, tl + 18, (row * 17 + 9 + col) as usize);
            let p = |i: usize| self.positions[i];
            return [
                [p(ctr), p(tl), p(bl)],
                [p(ctr), p(bl), p(br)],
                [p(ctr), p(br), p(tr)],
                [p(ctr), p(tr), p(tl)],
            ]
            .iter()
            .find_map(|tri| triangle_z_at(tri, wow[0], wow[1]));
        }
        self.indices.as_chunks::<3>().0.iter().find_map(|t| {
            let tri = [
                *self.positions.get(t[0] as usize)?,
                *self.positions.get(t[1] as usize)?,
                *self.positions.get(t[2] as usize)?,
            ];
            triangle_z_at(&tri, wow[0], wow[1])
        })
    }
}

/// The `z` of triangle `tri` (WoW coords, Z up) at column `(x, y)`, or `None` when `(x, y)` falls
/// outside the triangle's XY projection (or the triangle is vertical / degenerate in XY).
///
/// Barycentric interpolation — for a **vertical** ray this IS the exact ray-vs-triangle test: a
/// vertical face projects to a degenerate XY sliver, and a vertical ray is parallel to it anyway.
/// Shared by the WMO current-group down-ray (its Leg A walking-collision faces) and the ADT terrain
/// probe that races it, so the two legs of the client's arbitration can never drift apart.
pub fn triangle_z_at(tri: &[[f32; 3]; 3], x: f32, y: f32) -> Option<f32> {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if det.abs() < 1.0e-9 {
        return None; // vertical / degenerate in XY
    }
    let l1 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / det;
    let l2 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / det;
    let l3 = 1.0 - l1 - l2;
    if l1 < 0.0 || l2 < 0.0 || l3 < 0.0 {
        return None;
    }
    Some(l1 * a[2] + l2 * b[2] + l3 * c[2])
}

/// The `AreaTable` id under a raw WoW-space position, given **one tile's** chunks; `None` when the
/// position lies outside every chunk (a different tile — same containment contract as
/// [`mcsh_shadowed_at`]). The zone/subzone driving music/ambience/reverb (decision 0070).
pub fn area_id_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<u32> {
    chunks.iter().find_map(|c| c.area_at(wow))
}

/// Is a raw WoW-space position over an **impassable** MCNK, given **one tile's** chunks? `None`
/// when the position lies outside every chunk (a different tile — the same containment contract as
/// [`area_id_at`]), which a caller must read as "unknown", never as "passable": the chunk on the
/// far side of a tile seam is exactly where B129's wall is authored.
pub fn impassable_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<bool> {
    chunks.iter().find_map(|c| c.impassable_at(wow))
}

/// The `GroundEffectTexture` id under a raw WoW-space position, given **one tile's** chunks;
/// `None` when outside every chunk or when the containing cell's layer has no effect (footstep
/// consumers fall back to the Dirt default either way).
pub fn ground_effect_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<u32> {
    chunks.iter().find_map(|c| c.ground_effect_at(wow))
}

/// The terrain surface height (WoW `z`) under a raw WoW-space column, given **one tile's** chunks;
/// `None` when the column lies outside every chunk (a different tile) or falls in an MCNK hole.
/// The terrain leg of the client's down-ray arbitration (see [`ChunkMesh::height_at`]).
pub fn terrain_height_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<f32> {
    match chunk_at(chunks, wow) {
        Some(c) => c.height_at(wow),
        None => chunks.iter().find_map(|c| c.height_at(wow)),
    }
}

/// The one MCNK of a full 16×16 tile that covers `wow`'s column — arithmetic on the tile's
/// north-west corner, not a walk. A tile's chunks are in MCIN order (row-major: row = the
/// south step along WoW x, column = the east step along y), and every chunk's first vertex is
/// its own north-west corner, so chunk 0's is the tile's. `None` when the slice is not a full
/// tile or its first chunk carries no geometry — the callers fall back to the linear walk,
/// which is what every column lookup used to be: 256 bounds tests per call, and the flare
/// oracle alone asks ~96 times a frame (decision 1979's floor).
fn chunk_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<&ChunkMesh> {
    if chunks.len() != 256 {
        return None;
    }
    let nw = *chunks[0].positions.first()?;
    let row = (nw[0] - wow[0]) / CHUNK_SIZE;
    let col = (nw[1] - wow[1]) / CHUNK_SIZE;
    if !(0.0..16.0).contains(&row) || !(0.0..16.0).contains(&col) {
        return None;
    }
    chunks.get(row as usize * 16 + col as usize)
}

/// Is a raw WoW-space position in MCSH terrain shadow, given **one tile's** chunks? `Some(true)` when the
/// containing chunk's MCSH bit is set, `Some(false)` when the containing chunk is lit (or has no shadow
/// map), and **`None` when the position lies outside every chunk in `chunks`** — i.e. it belongs to a
/// different tile. Returning `None` rather than defaulting to lit is load-bearing: an MDDF doodad that
/// straddles a tile edge is listed in *every* tile it touches, but only the tile that CONTAINS its origin
/// can answer; the caller must take the `Some` from that tile and never let a straddle-tile miss
/// masquerade as "lit". (The real client sidesteps this with a global world→chunk MCSH lookup,
/// `0x69b350`.) Feeds the model lobe's `sun_scale` (the faithful per-instance terrain-shade, 2.5
/// sunlit / 0.5 shaded, per the reference).
pub fn mcsh_shadowed_at(chunks: &[ChunkMesh], wow: [f32; 3]) -> Option<bool> {
    match chunk_at(chunks, wow) {
        Some(c) => c.mcsh_shadowed_at(wow),
        None => chunks.iter().find_map(|c| c.mcsh_shadowed_at(wow)),
    }
}

/// A placed M2 doodad (tree/bush/prop): which model, and where, in **raw WoW world coords**.
#[derive(Debug, Clone)]
pub struct Doodad {
    /// MMDX model path (`.mdx`); `load_m2_mesh` normalizes it to `.m2`.
    pub model: String,
    /// Position in WoW world coords (X north, Y west, Z up), converted from MDDF.
    pub position: [f32; 3],
    /// Euler rotation in **degrees** (X, Y, Z); Y is the heading about the up axis.
    pub rotation: [f32; 3],
    /// Uniform scale (MDDF stores it ×1024).
    pub scale: f32,
    /// MDDF uniqueId — stable per placement, identical across the ADT tiles a doodad straddles, so
    /// the streamer can spawn it once and refcount it (the client's own dedup, `FUN_00694bc0`).
    pub unique_id: u32,
}

/// A placed WMO (building) — which model, and where, in **raw WoW world coords**.
#[derive(Debug, Clone)]
pub struct WmoInstance {
    /// MWMO root path (`.wmo`); `load_wmo` loads its group files too.
    pub model: String,
    /// Position in WoW world coords (X north, Y west, Z up), converted from MODF.
    pub position: [f32; 3],
    /// Euler rotation in **degrees** (X, Y, Z); Y is the heading about the up axis.
    pub rotation: [f32; 3],
    /// MODF uniqueId — stable per placement, identical across the ADT tiles a building straddles
    /// (Stormwind spans 6 tiles), so the streamer dedups it instead of spawning 6× (`FUN_00694bc0`).
    pub unique_id: u32,
    /// MODF doodad-set index: which *additional* WMO doodad set to show beyond the always-on global
    /// set 0 (selects interior-prop variants — e.g. an inn's furnished vs empty rooms).
    pub doodad_set: u16,
    /// MODF name-set index — the placement's `WMOAreaTable.NameSetID` naming/audio variant.
    pub name_set: u16,
}

/// One ADT tile's terrain: its per-MCNK chunk meshes + placed doodads + buildings.
#[derive(Debug, Clone)]
pub struct TileMesh {
    pub chunks: Vec<ChunkMesh>,
    pub doodads: Vec<Doodad>,
    pub wmos: Vec<WmoInstance>,
}

impl TileMesh {
    /// Total vertices across all chunks.
    pub fn vertex_count(&self) -> usize {
        self.chunks.iter().map(|c| c.positions.len()).sum()
    }
}

/// Snap an X/Y world coordinate to the global terrain vertex lattice (`MAP_CENTER − n·UNIT_SIZE`).
///
/// Each MCNK stores its own corner position as `f32`, which carries ~1–2 mm of per-chunk rounding.
/// So a vertex shared by two adjacent chunks (or tiles) — computed from each one's header + offset —
/// doesn't come out bit-identical, leaving the edges fractionally misaligned: hairline cracks that
/// show the sky through the terrain, and z-fighting where the coplanar edge triangles overlap (a
/// shimmering seam visible at any zoom). Real vertices lie on the lattice to <0.0005 unit, so
/// snapping (computed in `f64`) collapses each shared vertex to the *same* coordinate → watertight.
/// `pub(crate)` so the liquid mesher snaps its grid to the same lattice as the terrain it sits in.
pub(crate) fn snap_to_lattice(coord: f32) -> f32 {
    let map_center = 32.0_f64 * 1600.0 / 3.0; // 64-tile map; TILE_SIZE = 1600/3 yd
    let unit = (1600.0_f64 / 3.0) / 128.0; // 128 vertex-units per tile edge
    let idx = ((map_center - f64::from(coord)) / unit).round();
    (map_center - idx * unit) as f32
}

/// Build per-chunk meshes from one vanilla (monolithic) ADT file's bytes.
pub fn adt_to_tile_mesh(adt_bytes: &[u8]) -> Result<TileMesh> {
    let mut cursor = Cursor::new(adt_bytes);
    let parsed = parse_adt(&mut cursor).map_err(|e| anyhow::anyhow!("parsing ADT: {e}"))?;
    // benilla-adt only produces vanilla monolithic root ADTs (no Cata+ split files).
    let ParsedAdt::Root(root) = parsed;

    let mut chunks = Vec::new();
    for mcnk in &root.mcnk_chunks {
        let Some(mcvt) = &mcnk.heights else {
            continue; // no MCVT height data for this chunk
        };
        if mcvt.heights.len() < 145 {
            continue;
        }
        // wow-adt 0.6.4's world_position() mis-orders vanilla axes; the raw `position` field
        // is already WoW [X(north), Y(west), Z(up)].
        let [wx, wy, wz] = mcnk.header.position;

        // MCNR normals parallel MCVT 1:1 (same 145-entry stride-17 grid). The disk byte order is
        // already WoW `[X, Y, Z]` with no swap (qa3-adt: geometric dot 0.999 vs ~0.4 for the
        // `[X, Z, Y]` interpretation). The `wow-adt` crate's `to_normalized()` is buggy and
        // returns `[b0, b2, b1]`; our `[x, z, y] = …; [x, y, z]` swap below cancels the crate's
        // swap, recovering the true raw `[X, Y, Z]` order. Output verified vs the mesh's own
        // geometric normals (avg dot 0.99). Skipped if the chunk has no MCNR.
        let mcnr = mcnk.normals.as_ref().filter(|n| n.normals.len() >= 145);

        // 145 verts in MCVT's stride-17 interleave: row r (0..8) emits 9 outer at indices
        // `r·17+(0..9)` then 8 inner at `r·17+9+(0..8)`; the final outer row 8 has no inner
        // companion. Outer (r,c) sits at chunk-local `(r·UNIT, c·UNIT, h)`, inner (r,c) at
        // `((r+½)·UNIT, (c+½)·UNIT, h_inner)` — the **+½-cell offset is horizontal only**, and
        // each inner vertex carries its OWN authored MCVT center height (the binary's B1).
        let mut positions = Vec::with_capacity(145);
        let mut normals = Vec::with_capacity(if mcnr.is_some() { 145 } else { 0 });
        let mut uvs = Vec::with_capacity(145);
        for row in 0..9u32 {
            // Outer row (9 verts at the corner grid).
            for col in 0..9u32 {
                let idx = (row * 17 + col) as usize;
                let h = mcvt.heights[idx];
                positions.push([
                    // Snap X/Y to the global lattice so shared chunk/tile edge vertices coincide
                    // exactly (no hairline cracks / z-fight seams); Z stays the MCVT height.
                    snap_to_lattice(wx - row as f32 * UNIT_SIZE), // WoW X (north): rows step south
                    snap_to_lattice(wy - col as f32 * UNIT_SIZE), // WoW Y (west): columns step east
                    wz + h,                                       // WoW Z (up): height over base
                ]);
                if let Some(mcnr) = mcnr {
                    let [x, z, y] = mcnr.normals[idx].to_normalized();
                    normals.push([x, y, z]);
                }
                // 0..1 per chunk; outer at integer/8.
                uvs.push([col as f32 / 8.0, row as f32 / 8.0]);
            }
            // Inner row (8 verts at the cell centers) — skipped after the final outer row.
            if row < 8 {
                for col in 0..8u32 {
                    let idx = (row * 17 + 9 + col) as usize;
                    let h = mcvt.heights[idx];
                    // No lattice-snap on inner: each inner vertex is per-chunk-private (the
                    // adjacent chunk has no twin at the same (x, y)), so the seam concern doesn't
                    // apply and we'd just move it off its authored position by the rounding error.
                    positions.push([
                        wx - (row as f32 * UNIT_SIZE + UNIT_SIZE / 2.0),
                        wy - (col as f32 * UNIT_SIZE + UNIT_SIZE / 2.0),
                        wz + h,
                    ]);
                    if let Some(mcnr) = mcnr {
                        let [x, z, y] = mcnr.normals[idx].to_normalized();
                        normals.push([x, y, z]);
                    }
                    // Inner at +½ in both axes (qb4): `((c+½)/8, (r+½)/8)`.
                    uvs.push([(col as f32 + 0.5) / 8.0, (row as f32 + 0.5) / 8.0]);
                }
            }
        }

        // 4-triangle center-fan per cell, hole-aware (B1 + B2). For cell (r, c) ∈ 0..8²:
        //   TL = outer(r,c) @ r·17+c           TR = outer(r,c+1) @ r·17+c+1
        //   BL = outer(r+1,c) @ (r+1)·17+c     BR = outer(r+1,c+1) @ (r+1)·17+c+1
        //   CTR = inner(r,c) @ r·17+9+c
        // Fan winding (CCW from above, +Z normal in WoW = front-face up in Bevy under the det-+1
        // transform; matches the existing 2-tri winding the engine already accepts).
        // Holes: u16 @ MCNK+0x3C, bit `n=(r>>1)·4+(c>>1)` set ⇒ omit the 2×2 block of fans (verts
        // remain — unreferenced is fine). 1.12 has no high-res holes.
        let holes = mcnk.header.holes_low_res;
        let mut indices = Vec::with_capacity(8 * 8 * 12);
        for row in 0..8u32 {
            for col in 0..8u32 {
                if holes & (1u16 << ((row >> 1) * 4 + (col >> 1))) != 0 {
                    continue;
                }
                let tl = row * 17 + col;
                let tr = tl + 1;
                let bl = tl + 17;
                let br = bl + 1;
                let ctr = row * 17 + 9 + col;
                indices.extend_from_slice(&[
                    ctr, tl, bl, // west fan
                    ctr, bl, br, // south fan
                    ctr, br, tr, // east fan
                    ctr, tr, tl, // north fan
                ]);
            }
        }

        // All layers (layer 0 first): texture name (resolved through MTEX) + its ground-effect id,
        // kept together so the two parallel vecs stay index-aligned even if a layer is dropped.
        let layers: Vec<(String, u32)> = mcnk
            .layers
            .as_ref()
            .map(|mcly| {
                mcly.layers
                    .iter()
                    .filter_map(|layer| {
                        root.textures
                            .get(layer.texture_id as usize)
                            .cloned()
                            .map(|t| (t, layer.effect_id))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let layer_textures: Vec<String> = layers.iter().map(|(t, _)| t.clone()).collect();
        let layer_effect_ids: Vec<u32> = layers.iter().map(|(_, e)| *e).collect();
        let base_texture = layer_textures.first().cloned();

        // Pack layers 1..3 alpha into one RGBA map for the splat shader. Vanilla uses 4-bit
        // (small) alpha maps and the renderer convention of duplicating the last row/col
        // (`fix_alpha`); big-alpha (8-bit) was introduced post-1.12.
        let alpha_map = (layer_textures.len() > 1).then(|| {
            CombinedAlphaMap::new(
                mcnk, /* has_big_alpha */ false, /* fix_alpha */ true,
            )
            .as_slice()
            .to_vec()
        });

        // MCSH baked shadow → one byte per texel (255 = shadowed, 0 = lit), row-major over the
        // chunk's 0..1 UV. `is_shadowed(x, y)` unpacks the 1-bit/texel, LSB-first map for us.
        let shadow = mcnk.shadow.as_ref().map(|sh| {
            let n = SHADOW_MAP_SIZE as usize;
            let mut out = vec![0u8; n * n];
            for y in 0..n {
                for x in 0..n {
                    if sh.is_shadowed(x, y) {
                        out[y * n + x] = 255;
                    }
                }
            }
            out
        });

        // Authored ground-effect placement grids (see `ChunkMesh::pred_tex`). wow-adt mislabels
        // these for vanilla, but the raw bytes survive: predominantTexture (0x40-0x4F) =
        // `pred_tex` ++ `no_effect_doodad`; the real noEffectDoodad (0x50-0x57) = `unknown_8bytes`.
        // Decode per wowdev ADT/v18: predTex = 2 bits/cell, noEffect = 1 bit/cell, both LSB-first
        // row-major (`k = row*8 + col`), confirmed against the alpha map by `examples/clutter_probe`.
        let h = &mcnk.header;
        let mut pt16 = [0u8; 16];
        pt16[..8].copy_from_slice(&h.pred_tex);
        pt16[8..].copy_from_slice(&h.no_effect_doodad);
        let ned8 = h.unknown_8bytes;
        let mut pred_tex = [0u8; 64];
        let mut no_effect_doodad = [false; 64];
        for k in 0..64 {
            pred_tex[k] = (pt16[k / 4] >> (2 * (k % 4))) & 0x3;
            no_effect_doodad[k] = (ned8[k / 8] >> (k % 8)) & 0x1 == 1;
        }

        // MCLQ liquid surfaces (lake/river/ocean/lava), built on the same lattice as this chunk's
        // terrain — one mesh per liquid block the MCNK carries. See `crate::liquid`.
        let liquids: Vec<_> = mcnk
            .liquids
            .iter()
            .filter_map(|mclq| crate::liquid::build_liquid_mesh(mclq, h.position))
            .collect();

        chunks.push(ChunkMesh {
            positions,
            normals,
            uvs,
            indices,
            holes,
            base_texture,
            layer_textures,
            layer_effect_ids,
            alpha_map,
            shadow,
            pred_tex,
            no_effect_doodad,
            index_x: h.index_x,
            index_y: h.index_y,
            area_id: h.area_id,
            impassable: h.impassable(),
            liquids,
        });
    }

    if chunks.is_empty() {
        anyhow::bail!("ADT produced no terrain chunks (no MCVT height data?)");
    }

    // Doodad placements. MDDF positions are in a corner-origin space; convert to WoW world coords
    // (verified against the tile's MCNK position range): X = 32·TILE − mddf.z, Y = 32·TILE −
    // mddf.x, Z = mddf.y. Scale is stored ×1024. `name_id` indexes the MMDX list directly.
    const MAP_CENTER: f32 = 32.0 * TILE_SIZE;
    let doodads = root
        .doodad_placements
        .iter()
        .filter_map(|d| {
            let model = root.models.get(d.name_id as usize)?.clone();
            Some(Doodad {
                model,
                position: [
                    MAP_CENTER - d.position[2],
                    MAP_CENTER - d.position[0],
                    d.position[1],
                ],
                rotation: d.rotation,
                scale: d.scale as f32 / 1024.0,
                unique_id: d.unique_id,
            })
        })
        .collect();

    // WMO (building) placements — same MODF→world transform as doodads; scale is unused in vanilla.
    let wmos = root
        .wmo_placements
        .iter()
        .filter_map(|w| {
            let model = root.wmos.get(w.name_id as usize)?.clone();
            Some(WmoInstance {
                model,
                position: [
                    MAP_CENTER - w.position[2],
                    MAP_CENTER - w.position[0],
                    w.position[1],
                ],
                rotation: w.rotation,
                unique_id: w.unique_id,
                doodad_set: w.doodad_set,
                name_set: w.name_set,
            })
        })
        .collect();

    Ok(TileMesh {
        chunks,
        doodads,
        wmos,
    })
}

/// Read a map's WDT from the chain and parse it (vanilla/Classic layout).
fn read_wdt(chain: &mut Chain, map: &str) -> Result<WdtFile> {
    let path = format!("World\\Maps\\{map}\\{map}.wdt");
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading {path}"))?;
    WdtReader::new(Cursor::new(bytes), WowVersion::Classic)
        .read()
        .map_err(|e| anyhow::anyhow!("parsing WDT {path}: {e}"))
}

fn tile_exists(wdt: &WdtFile, x: u32, y: u32) -> bool {
    wdt.get_tile(x as usize, y as usize)
        .is_some_and(|t| t.has_adt)
}

/// Find an existing ADT tile near a world (x, y) on `map`, spiraling out if the exact tile
/// is missing. Returns `(tile_x, tile_y)` matching the `Map_<x>_<y>.adt` naming.
pub fn find_tile_near(
    chain: &mut Chain,
    map: &str,
    world_x: f32,
    world_y: f32,
) -> Result<(u32, u32)> {
    let wdt = read_wdt(chain, map)?;
    let (tx, ty) = benilla_wdt::world_to_tile(world_x, world_y);
    if tile_exists(&wdt, tx, ty) {
        return Ok((tx, ty));
    }
    for r in 1..=4i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                let (nx, ny) = (tx as i32 + dx, ty as i32 + dy);
                if (0..64).contains(&nx)
                    && (0..64).contains(&ny)
                    && tile_exists(&wdt, nx as u32, ny as u32)
                {
                    return Ok((nx as u32, ny as u32));
                }
            }
        }
    }
    anyhow::bail!("no existing ADT tile near world ({world_x}, {world_y}) on map {map}")
}

/// A map's tile index (its WDT), parsed once and kept so a renderer can stream tiles in and out as
/// the player moves — without re-reading the WDT every frame.
pub struct MapTiles {
    map: String,
    wdt: WdtFile,
}

impl MapTiles {
    /// Parse the map's WDT from the chain.
    pub fn load(chain: &mut Chain, map: &str) -> Result<Self> {
        let wdt = read_wdt(chain, map)?;
        Ok(Self {
            map: map.to_string(),
            wdt,
        })
    }

    /// The map name (for `load_tile_mesh`).
    pub fn map(&self) -> &str {
        &self.map
    }

    /// The tile `(x, y)` containing a world `(x, y)`.
    pub fn tile_at(&self, world_x: f32, world_y: f32) -> (u32, u32) {
        benilla_wdt::world_to_tile(world_x, world_y)
    }

    /// Coords of every **existing** ADT tile within `radius` tiles of a world `(x, y)`, **sorted
    /// nearest-first** (by squared tile-grid distance to the centre tile). The streamer loads the
    /// first not-yet-loaded entry, so nearest-first ⇒ the tile you're about to see in loads before the
    /// far ones — no "drove past the boundary into an unloaded tile" pop-in.
    pub fn existing_in_radius(&self, world_x: f32, world_y: f32, radius: u32) -> Vec<(u32, u32)> {
        let (cx, cy) = benilla_wdt::world_to_tile(world_x, world_y);
        let r = radius as i32;
        let mut out: Vec<(i32, (u32, u32))> = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
                if (0..64).contains(&tx)
                    && (0..64).contains(&ty)
                    && tile_exists(&self.wdt, tx as u32, ty as u32)
                {
                    out.push((dx * dx + dy * dy, (tx as u32, ty as u32)));
                }
            }
        }
        out.sort_by_key(|(d, _)| *d);
        out.into_iter().map(|(_, t)| t).collect()
    }
}

/// Read and mesh one ADT tile from the chain.
pub fn load_tile_mesh(chain: &mut Chain, map: &str, tile_x: u32, tile_y: u32) -> Result<TileMesh> {
    let path = format!("World\\Maps\\{map}\\{map}_{tile_x}_{tile_y}.adt");
    let bytes = chain
        .read_file(&path)
        .with_context(|| format!("reading {path}"))?;
    adt_to_tile_mesh(&bytes)
}

/// Load every existing ADT tile within `radius` tiles of the world (x, y) on `map`.
///
/// Returns `((tile_x, tile_y), tile)` for each tile that exists and parses. Meshes use
/// absolute world coords, so neighboring tiles abut seamlessly.
pub fn load_tiles_around(
    chain: &mut Chain,
    map: &str,
    world_x: f32,
    world_y: f32,
    radius: u32,
) -> Result<Vec<((u32, u32), TileMesh)>> {
    let wdt = read_wdt(chain, map)?;
    let (cx, cy) = benilla_wdt::world_to_tile(world_x, world_y);
    let r = radius as i32;

    let mut out = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
            if !(0..64).contains(&tx) || !(0..64).contains(&ty) {
                continue;
            }
            let (tx, ty) = (tx as u32, ty as u32);
            if !tile_exists(&wdt, tx, ty) {
                continue;
            }
            // Skip (rather than fail the whole load on) the occasional unparseable tile.
            if let Ok(mesh) = load_tile_mesh(chain, map, tx, ty) {
                out.push(((tx, ty), mesh));
            }
        }
    }

    if out.is_empty() {
        anyhow::bail!("no loadable tiles within {radius} of world ({world_x}, {world_y}) on {map}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-chunk mesh with `positions`/`indices` and every other field inert — enough to exercise
    /// the footprint + hole logic of [`ChunkMesh::height_at`].
    fn chunk(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> ChunkMesh {
        ChunkMesh {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            holes: 0,
            base_texture: None,
            layer_textures: Vec::new(),
            layer_effect_ids: Vec::new(),
            alpha_map: None,
            shadow: None,
            pred_tex: [0; 64],
            no_effect_doodad: [false; 64],
            index_x: 0,
            index_y: 0,
            area_id: 0,
            impassable: false,
            liquids: Vec::new(),
        }
    }

    #[test]
    fn height_at_interpolates_inside_and_misses_holes_and_off_footprint() {
        // NW corner at positions[0]; +X is south-distance, +Y is east-distance across the chunk. Two
        // quads (four tris) tented to z=10 at the shared middle edge, with the SOUTH half of the chunk
        // left UN-indexed — an MCNK hole (a cave mouth), verts present but no triangle references them.
        let nw = [0.0, 0.0, 0.0];
        let mid_x = nw[0] - CHUNK_SIZE * 0.5; // half a chunk south
        let far_x = nw[0] - CHUNK_SIZE; // the chunk's south edge
        let far_y = nw[1] - CHUNK_SIZE; // the chunk's east edge
        let positions = vec![
            nw,                   // 0: NW
            [nw[0], far_y, 0.0],  // 1: NE
            [mid_x, nw[1], 10.0], // 2: mid-W (ridge)
            [mid_x, far_y, 10.0], // 3: mid-E (ridge)
            [far_x, nw[1], 0.0],  // 4: SW
            [far_x, far_y, 0.0],  // 5: SE
        ];
        // North half only (verts 0,1,2,3) — the south half (4,5) is the hole.
        let indices = vec![0, 1, 2, 1, 3, 2];
        let c = chunk(positions, indices);

        // A column in the meshed north half interpolates up the ridge.
        let quarter_x = nw[0] - CHUNK_SIZE * 0.25;
        let z = c
            .height_at([quarter_x, nw[1] - CHUNK_SIZE * 0.5, 0.0])
            .unwrap();
        assert!((z - 5.0).abs() < 0.01, "ridge midpoint ≈ 5, got {z}");
        // A column over the un-indexed south half — the hole — has no surface: the mine mouth.
        assert_eq!(
            c.height_at([far_x + 1.0, nw[1] - CHUNK_SIZE * 0.5, 0.0]),
            None
        );
        // A column past the chunk's east edge is off the footprint entirely.
        assert_eq!(c.height_at([quarter_x, far_y - 5.0, 0.0]), None);
    }

    /// The real 5875 map set: which maps have terrain and which are a single global WMO
    /// (decision 0688). 20 of the 43 shipped WDTs author **zero** ADT tiles — on those maps the
    /// global WMO is the entire world, and a client that ignores it renders a void you fall
    /// through. Skips without client data.
    #[test]
    fn real_wdts_split_into_adt_maps_and_wmo_only_maps() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // A WMO-only map, an ADT map, and the one map whose placement is NOT identity — the three
        // cases the streamer has to tell apart.
        let jail = read_wdt(&mut chain, "StormwindJail").expect("StormwindJail.wdt");
        let g = jail
            .global_wmo()
            .expect("the Stockade is one WMO, no terrain");
        assert_eq!(
            g.model.to_ascii_lowercase(),
            "world\\wmo\\dungeon\\az_stormwindprisons\\stormwindjail.wmo"
        );
        // Authored at the world origin, unrotated: the placement the whole dungeon hangs off.
        assert_eq!(g.position, [0.0, 0.0, 0.0]);
        assert_eq!(g.rotation, [0.0, 0.0, 0.0]);
        assert!(
            (0..64).all(|y| (0..64).all(|x| !tile_exists(&jail, x, y))),
            "a WMO-only map authors no ADT tiles at all"
        );

        // Deadmines is the control: the reports that opened 0688 said it alone kept you standing,
        // and this is why — it ships real terrain, so the missing global-WMO branch never bit.
        let deadmines = read_wdt(&mut chain, "DeadminesInstance").expect("DeadminesInstance.wdt");
        assert!(deadmines.global_wmo().is_none());
        let tiles = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .filter(|&(x, y)| tile_exists(&deadmines, x, y))
            .count();
        assert_eq!(tiles, 36, "Deadmines ships 36 ADT tiles");

        // Dire Maul is the reason the rotation column is read rather than assumed: it is the only
        // shipped map placed at a heading, and dropping it would spin the dungeon 180° about the
        // origin — every room mirrored onto the wrong side of the map.
        let dm = read_wdt(&mut chain, "DireMaul").expect("DireMaul.wdt");
        assert_eq!(
            dm.global_wmo().expect("WMO-only").rotation,
            [0.0, 180.0, 0.0]
        );
    }

    /// B129's pin, from the shipped bytes: the reference stops you 1.46 yd east of
    /// `.go xyz -6601.98 -531.87 335.60 0`, and this is the wall — the MCNK immediately east is
    /// flagged [`benilla_adt::MCNK_IMPASSABLE`] while the one under the pin is not.
    ///
    /// The numbers are the point. A regression that dropped the flag, or read the neighbouring bit,
    /// would leave the pin's own chunk reading the same and only the *other* side of the seam wrong
    /// — so the test pins both sides, the seam they straddle, and the band's size. Skips without
    /// client data.
    #[test]
    fn the_b129_pin_stands_one_chunk_west_of_an_impassable_band() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // The pin, and a step east across the chunk boundary at y = -533.333 (one tile seam too:
        // the flagged chunk is column 0 of Azeroth_33_44, which is why a single-tile view misses it).
        let pin = [-6601.98, -531.87, 335.60];
        let east = [-6601.98, -535.0, 335.60];
        assert_eq!(crate::world_to_tile(pin[0], pin[1]), (32, 44));
        assert_eq!(crate::world_to_tile(east[0], east[1]), (33, 44));

        let here = load_tile_mesh(&mut chain, "Azeroth", 32, 44).expect("Azeroth_32_44.adt");
        let next = load_tile_mesh(&mut chain, "Azeroth", 33, 44).expect("Azeroth_33_44.adt");

        assert_eq!(
            impassable_at(&here.chunks, pin),
            Some(false),
            "the pin's own chunk is walkable — the reporter was standing on it"
        );
        assert_eq!(
            impassable_at(&next.chunks, east),
            Some(true),
            "the chunk 1.46 yd east is the wall"
        );
        // The containment contract: a tile answers only for its own footprint. A caller that read
        // `None` as "passable" would walk straight through every wall authored across a seam.
        assert_eq!(impassable_at(&here.chunks, east), None);
        assert_eq!(impassable_at(&next.chunks, pin), None);

        // Not one stray chunk but an authored ribbon: Searing Gorge's rim (area 51) crosses this
        // tile, and the far side of the map's tiles carry none of it.
        assert_eq!(
            next.chunks.iter().filter(|c| c.impassable).count(),
            48,
            "Azeroth_33_44 authors 48 impassable chunks"
        );
        assert_eq!(here.chunks.iter().filter(|c| c.impassable).count(), 0);
    }
    /// The cell lookup must answer exactly like the raw triangle walk it replaced, height and
    /// MCSH alike — sampled off-border across four real tiles (Elwynn and Stormwind), so a
    /// wrong row/column convention, an off-by-one at an edge or a mishandled hole fails here,
    /// not under a foot.
    #[test]
    fn the_chunk_index_answers_exactly_like_the_walk() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        // The raw walk the cell lookup replaced: every triangle of every chunk, in index order.
        let walk = |chunks: &[ChunkMesh], wow: [f32; 3]| {
            chunks.iter().find_map(|c| {
                c.indices.as_chunks::<3>().0.iter().find_map(|t| {
                    let tri = [
                        c.positions[t[0] as usize],
                        c.positions[t[1] as usize],
                        c.positions[t[2] as usize],
                    ];
                    triangle_z_at(&tri, wow[0], wow[1])
                })
            })
        };
        let mut hits = 0;
        for (x, y) in [(32u32, 48u32), (31, 48), (30, 48), (31, 47)] {
            let tile = load_tile_mesh(&mut chain, "Azeroth", x, y).expect("Elwynn/Stormwind tile");
            assert_eq!(tile.chunks.len(), 256);
            let nw = tile.chunks[0].positions[0];
            let step = TILE_SIZE / 64.0;
            for i in 0..64 {
                for j in 0..64 {
                    let wow = [
                        nw[0] - (i as f32 + 0.37) * step,
                        nw[1] - (j as f32 + 0.61) * step,
                        0.0,
                    ];
                    let walk_h = walk(&tile.chunks, wow);
                    assert_eq!(
                        terrain_height_at(&tile.chunks, wow),
                        walk_h,
                        "height at {wow:?}"
                    );
                    let walk_s = tile.chunks.iter().find_map(|c| c.mcsh_shadowed_at(wow));
                    assert_eq!(
                        mcsh_shadowed_at(&tile.chunks, wow),
                        walk_s,
                        "shadow at {wow:?}"
                    );
                    hits += usize::from(walk_h.is_some());
                }
            }
        }
        assert!(
            hits > 12000,
            "the sample grid should land on terrain almost everywhere: {hits}"
        );
    }
}
