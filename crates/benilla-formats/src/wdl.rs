//! WDL (low-detail distant terrain) loader + coarse mesher.
//!
//! One `.wdl` per map holds the **whole 64×64-tile world** at a single height sample per MCNK — the
//! horizon geometry the reference draws BEYOND the streamed high-detail tiles (out to `horizonfarclip`),
//! unlit + untextured + vertex-white + fogged into the haze. RE'd from apitrace WoW.8 + the real file.
//!
//! Layout — VERIFIED against `World\Maps\Azeroth\Azeroth.wdl` (770730 B, 2026-05-30):
//!   `MVER` (version **18**) · `MAOF` (4096 `u32` tile offsets, `[tile_y][tile_x]`) · per present tile
//!   `MARE` (1090 B = 17×17 outer + 16×16 inner `int16` absolute heights, yards). No `MWMO`/`MWID`/
//!   `MODF`, no `MAHO` in vanilla Azeroth (low-detail WMOs + holes are out of scope for v1).
//!
//! The mesh is in **raw WoW world coords** (X north, Y west, Z up, yards) like [`crate::terrain`], with
//! the same per-cell 4-tri **center-fan** — but at `CHUNK_SIZE` (one MCNK) spacing instead of
//! `UNIT_SIZE`, so one WDL tile = a single MCNK's topology scaled ×16 (545 verts / 3072 indices, the
//! exact draw the apitrace shows). The renderer applies the WoW→Bevy transform.

use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::Chain;
use anyhow::{bail, Context, Result};

use crate::terrain::TILE_SIZE;

/// One MCNK's worth of horizon detail: 16 cells per tile edge, so the outer grid is 17×17 corners and
/// the inner grid is 16×16 cell centers — identical to an MCNK but at tile scale.
const OUTER_EDGE: usize = 17;
const INNER_EDGE: usize = 16;
const OUTER_N: usize = OUTER_EDGE * OUTER_EDGE; // 289
const INNER_N: usize = INNER_EDGE * INNER_EDGE; // 256
/// MARE body = (289 + 256) `int16` = 1090 bytes (VERIFIED).
const MARE_BYTES: usize = (OUTER_N + INNER_N) * 2;
/// One MCNK edge in yards (the WDL outer-grid spacing). `TILE_SIZE / 16`.
const CHUNK_SIZE: f32 = TILE_SIZE / 16.0;
/// World coord of tile (0,0)'s origin (max-X, max-Y) corner — the 64-tile map is centred on the origin.
const MAP_OFFSET: f64 = 32.0 * (TILE_SIZE as f64);

/// A map's parsed WDL: the present tiles' low-detail heightmaps, indexed by MAOF position.
pub struct WdlFile {
    /// 64×64, indexed `[tile_y * 64 + tile_x]` (the on-disk MAOF order); `None` where the map has no
    /// tile. `tile_x`/`tile_y` match `Map_<tile_x>_<tile_y>.adt` and `benilla_wdt::world_to_tile`.
    tiles: Vec<Option<MareTile>>,
}

/// One tile's low-detail heightmap: 17×17 outer (corner) then 16×16 inner (cell-center) `int16`
/// heights, in yards (absolute world Z). Both grids are row-major.
struct MareTile {
    outer: [i16; OUTER_N],
    inner: [i16; INNER_N],
}

/// A coarse WDL tile mesh in raw WoW world coords: positions + a 4-tri-per-cell center-fan index
/// buffer. No normals (unlit) and no UVs (untextured) — the reference draws this vertex-white + fogged.
pub struct WdlTileMesh {
    /// 545 verts: 289 outer (`[r*17+c]`) then 256 inner (`[289 + r*16+c]`).
    pub positions: Vec<[f32; 3]>,
    /// 3072 indices = 16×16 cells × 4 triangles.
    pub indices: Vec<u32>,
}

/// MAOF/tiles index for tile `(tile_x, tile_y)`. Row-major over `tile_y` (the on-disk `[y][x]` order),
/// matching `benilla_wdt`'s tile addressing so the WDL present-set lines up with the WDT's.
fn tile_index(tile_x: u32, tile_y: u32) -> usize {
    tile_y as usize * 64 + tile_x as usize
}

impl WdlFile {
    /// Read and parse the map's `.wdl` off the patch chain (`World\Maps\<map>\<map>.wdl`).
    pub fn load(chain: &mut Chain, map: &str) -> Result<Self> {
        let path = format!("World\\Maps\\{map}\\{map}.wdl");
        let bytes = chain
            .read_file(&path)
            .with_context(|| format!("reading {path}"))?;
        parse_wdl(&bytes).with_context(|| format!("parsing {path}"))
    }

    /// Number of present (non-empty) tiles.
    pub fn present_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count()
    }

    /// Is tile `(tile_x, tile_y)` present in this WDL?
    pub fn is_present(&self, tile_x: u32, tile_y: u32) -> bool {
        tile_x < 64 && tile_y < 64 && self.tiles[tile_index(tile_x, tile_y)].is_some()
    }

    /// Iterate every present tile's `(tile_x, tile_y)`.
    pub fn present_tiles(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..4096u32).filter_map(move |i| self.tiles[i as usize].as_ref().map(|_| (i % 64, i / 64)))
    }

    /// Present WDL tiles forming a Chebyshev **window** around a world `(x, y)`: every present tile
    /// within `radius` tiles, **the centre one included** — the reference's far walk (`0x683040`)
    /// visits a camera-centred ±3-tile window of the distant grid with no exclusion, and so must ours.
    ///
    /// There used to be a `skip_inner` parameter, and the renderer passed 0 — dropping the camera's
    /// OWN tile, on the reasoning that a 533 yd tile sits entirely inside the far-clip wall and would
    /// be discarded anyway. That holds only near the default `farclip` of 777. **Drop the view
    /// distance below a tile — the vanilla range reaches down to 177 — and most of the camera's own
    /// tile lies BEYOND the wall, where it is the only thing that can draw the near horizon.** Its
    /// absence is then a gap between the detailed terrain and the distant hills, right above the
    /// horizon line, through which the sky pours (the director's Weazel's Crater
    /// report at view distance 320). The parameter is gone rather than defaulted so the hole cannot
    /// come back: what bounds the band on the near side is the far band's near plane in `wdl.wgsl`,
    /// and nothing else.
    pub fn tiles_in_ring(&self, world_x: f32, world_y: f32, radius: u32) -> Vec<(u32, u32)> {
        let (cx, cy) = benilla_wdt::world_to_tile(world_x, world_y);
        let r = radius as i32;
        let mut out = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
                if (0..64).contains(&tx)
                    && (0..64).contains(&ty)
                    && self.is_present(tx as u32, ty as u32)
                {
                    out.push((tx as u32, ty as u32));
                }
            }
        }
        out
    }

    /// Build the coarse center-fan mesh for tile `(tile_x, tile_y)`, or `None` if it's absent.
    ///
    /// Vertex X/Y are derived from the **global** lattice index (`tile·16 + local`) computed in `f64`,
    /// so a shared edge between adjacent WDL tiles is bit-identical → watertight, no hairline seam.
    pub fn tile_mesh(&self, tile_x: u32, tile_y: u32) -> Option<WdlTileMesh> {
        if tile_x >= 64 || tile_y >= 64 {
            return None;
        }
        let tile = self.tiles[tile_index(tile_x, tile_y)].as_ref()?;
        let cs = CHUNK_SIZE as f64;
        // Lattice position for a global row/col index (X north decreases with row, Y west with col).
        let lat = |global: f64| (MAP_OFFSET - global * cs) as f32;

        let mut positions = Vec::with_capacity(OUTER_N + INNER_N);
        // Outer 17×17 corners.
        for r in 0..OUTER_EDGE {
            for c in 0..OUTER_EDGE {
                positions.push([
                    lat((tile_y as usize * 16 + r) as f64),
                    lat((tile_x as usize * 16 + c) as f64),
                    f32::from(tile.outer[r * OUTER_EDGE + c]),
                ]);
            }
        }
        // Inner 16×16 cell centers (+½ cell in both planar axes).
        for r in 0..INNER_EDGE {
            for c in 0..INNER_EDGE {
                positions.push([
                    lat(tile_y as usize as f64 * 16.0 + r as f64 + 0.5),
                    lat(tile_x as usize as f64 * 16.0 + c as f64 + 0.5),
                    f32::from(tile.inner[r * INNER_EDGE + c]),
                ]);
            }
        }

        // 4-triangle center-fan per cell, mirroring terrain.rs's winding (CCW from above → front-face
        // up under the det-+1 WoW→Bevy transform). For cell (r, c) ∈ 0..16²:
        //   TL=outer(r,c)  TR=outer(r,c+1)  BL=outer(r+1,c)  BR=outer(r+1,c+1)  CTR=inner(r,c)
        let inner_base = OUTER_N as u32;
        let mut indices = Vec::with_capacity(INNER_EDGE * INNER_EDGE * 12);
        for r in 0..INNER_EDGE as u32 {
            for c in 0..INNER_EDGE as u32 {
                let tl = r * OUTER_EDGE as u32 + c;
                let tr = tl + 1;
                let bl = tl + OUTER_EDGE as u32;
                let br = bl + 1;
                let ctr = inner_base + r * INNER_EDGE as u32 + c;
                indices.extend_from_slice(&[
                    ctr, tl, bl, // west fan
                    ctr, bl, br, // south fan
                    ctr, br, tr, // east fan
                    ctr, tr, tl, // north fan
                ]);
            }
        }
        Some(WdlTileMesh { positions, indices })
    }

    /// The drawn WDL surface height (absolute WoW `z`) under world `(x, y)` — interpolated over the
    /// SAME 4-triangle center-fan [`Self::tile_mesh`] builds, so a query agrees exactly with the
    /// rendered horizon silhouette (`height_at` at a mesh vertex's `x, y` returns that vertex's
    /// `z`). `None` where the map has no WDL tile. The far leg of the lens-flare occlusion march
    /// (benilla `sun::follow`), beyond the detailed-terrain ring.
    pub fn height_at(&self, world_x: f32, world_y: f32) -> Option<f32> {
        let cs = CHUNK_SIZE as f64;
        // Global lattice coords — the inverse of `tile_mesh`'s `lat`: row grows along −X (north →
        // south), col along −Y. 64 tiles × 16 cells = 1024 cells per axis.
        let gr = (MAP_OFFSET - f64::from(world_x)) / cs;
        let gc = (MAP_OFFSET - f64::from(world_y)) / cs;
        if !(0.0..1024.0).contains(&gr) || !(0.0..1024.0).contains(&gc) {
            return None;
        }
        let (cell_r, cell_c) = (gr as usize, gc as usize);
        let tile = self.tiles[tile_index((cell_c / 16) as u32, (cell_r / 16) as u32)].as_ref()?;
        let (r, c) = (cell_r % 16, cell_c % 16);
        // The cell's corner + center heights (outer 17×17, inner 16×16 — same addressing as the mesh).
        let h = |rr: usize, cc: usize| f64::from(tile.outer[rr * OUTER_EDGE + cc]);
        let (tl, tr) = (h(r, c), h(r, c + 1));
        let (bl, br) = (h(r + 1, c), h(r + 1, c + 1));
        let ctr = f64::from(tile.inner[r * INNER_EDGE + c]);
        // Fractional position inside the cell: `v` down the rows (toward BL), `u` across the cols
        // (toward TR). The center-fan splits the cell into 4 triangles meeting at (0.5, 0.5); pick
        // the fan by which quadrant-diagonal region holds (u, v), then interpolate linearly along
        // the outer edge and blend toward the center — the exact plane of that fan triangle.
        let (v, u) = (gr - cell_r as f64, gc - cell_c as f64);
        let (a, b, t, s) = if v <= u && v <= 1.0 - u {
            (tl, tr, u, v) // north fan (CTR, TR, TL): edge TL→TR at v = 0
        } else if v >= u && v >= 1.0 - u {
            (bl, br, u, 1.0 - v) // south fan: edge BL→BR at v = 1
        } else if u < v {
            (tl, bl, v, u) // west fan: edge TL→BL at u = 0
        } else {
            (tr, br, v, 1.0 - u) // east fan: edge TR→BR at u = 1
        };
        // On the fan's plane: at edge distance `s` ∈ [0, 0.5] between the outer edge (s = 0) and the
        // center (s = 0.5), the edge lerp `a→b` at parameter `t` blends toward CTR. The edge-parallel
        // coordinate compresses toward the apex: lerp param `(t − s) / (1 − 2s)` spans the shrinking
        // cross-section (degenerate only exactly at the apex, where the height is CTR).
        let height = if s >= 0.5 {
            ctr
        } else {
            let edge = a + (b - a) * ((t - s) / (1.0 - 2.0 * s));
            edge + (ctr - edge) * (s * 2.0)
        };
        Some(height as f32)
    }
}

/// Read one IFF chunk header: 4-byte magic (stored reversed on disk) + `u32` LE size. Returns the
/// de-reversed magic (e.g. `b"MARE"`) and the body size.
fn read_chunk_header(cur: &mut Cursor<&[u8]>) -> Result<([u8; 4], u32)> {
    let mut magic = [0u8; 4];
    cur.read_exact(&mut magic)?;
    magic.reverse(); // on-disk is little-endian fourCC, i.e. reversed
    let mut size = [0u8; 4];
    cur.read_exact(&mut size)?;
    Ok((magic, u32::from_le_bytes(size)))
}

fn parse_wdl(bytes: &[u8]) -> Result<WdlFile> {
    let mut cur = Cursor::new(bytes);

    // MVER (version 18 in vanilla).
    let (magic, size) = read_chunk_header(&mut cur)?;
    if &magic != b"MVER" {
        bail!("expected MVER first, got {:?}", magic);
    }
    let mut ver = [0u8; 4];
    cur.read_exact(&mut ver)?;
    let version = u32::from_le_bytes(ver);
    if version != 18 {
        bail!("unexpected WDL version {version} (expected 18)");
    }
    cur.seek(SeekFrom::Current(size as i64 - 4))?; // skip any MVER tail (none in vanilla)

    // Skip any optional low-detail-WMO chunks (MWMO/MWID/MODF — absent in vanilla Azeroth) until MAOF.
    let offsets = loop {
        let (magic, size) = read_chunk_header(&mut cur)?;
        if &magic == b"MAOF" {
            if size as usize != 4096 * 4 {
                bail!("MAOF is {size} B, expected {} (4096 u32)", 4096 * 4);
            }
            let mut raw = vec![0u8; size as usize];
            cur.read_exact(&mut raw)?;
            let mut offs = vec![0u32; 4096];
            for (i, o) in offs.iter_mut().enumerate() {
                *o = u32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap());
            }
            break offs;
        }
        cur.seek(SeekFrom::Current(size as i64))?;
    };

    // Each non-zero MAOF entry points at that tile's MARE chunk header.
    let mut tiles: Vec<Option<MareTile>> = (0..4096).map(|_| None).collect();
    for (i, &off) in offsets.iter().enumerate() {
        if off == 0 {
            continue;
        }
        cur.seek(SeekFrom::Start(off as u64))?;
        let (magic, size) = read_chunk_header(&mut cur)?;
        if &magic != b"MARE" {
            bail!("MAOF[{i}] offset {off} points at {magic:?}, expected MARE");
        }
        if (size as usize) < MARE_BYTES {
            bail!("MARE[{i}] is {size} B, expected ≥ {MARE_BYTES}");
        }
        let mut body = vec![0u8; MARE_BYTES];
        cur.read_exact(&mut body)?;
        let mut outer = [0i16; OUTER_N];
        let mut inner = [0i16; INNER_N];
        for (j, h) in outer.iter_mut().enumerate() {
            *h = i16::from_le_bytes([body[j * 2], body[j * 2 + 1]]);
        }
        let base = OUTER_N * 2;
        for (j, h) in inner.iter_mut().enumerate() {
            *h = i16::from_le_bytes([body[base + j * 2], body[base + j * 2 + 1]]);
        }
        tiles[i] = Some(MareTile { outer, inner });
    }

    Ok(WdlFile { tiles })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_index_is_row_major_over_y() {
        // The apitrace WDL tile was (tile_x=34, tile_y=48) -> 48*64+34.
        assert_eq!(tile_index(34, 48), 48 * 64 + 34);
        assert_eq!(tile_index(0, 0), 0);
        assert_eq!(tile_index(63, 63), 4095);
    }

    #[test]
    fn mare_geometry_constants() {
        assert_eq!(OUTER_N, 289);
        assert_eq!(INNER_N, 256);
        assert_eq!(MARE_BYTES, 1090);
        assert_eq!(OUTER_N + INNER_N, 545); // verts per tile
                                            // 16×16 cells × 4 tris × 3 indices = 3072 (the apitrace draw count).
        assert_eq!(INNER_EDGE * INNER_EDGE * 12, 3072);
        // One WDL cell spans one MCNK.
        assert!((CHUNK_SIZE - TILE_SIZE / 16.0).abs() < 1e-3);
    }

    /// `height_at` must agree with the RENDERED surface — sample it at every `tile_mesh` vertex
    /// position (outer corners + inner fan centers) of a synthetic bumpy tile and require the
    /// vertex's own height back. Skips the tile's south/east boundary lattice lines (those verts
    /// belong to the absent neighbour tile in this synthetic file; real MARE data duplicates
    /// shared edges).
    #[test]
    fn height_at_matches_the_drawn_mesh_exactly() {
        let mut tiles: Vec<Option<MareTile>> = (0..4096).map(|_| None).collect();
        let mut outer = [0i16; OUTER_N];
        let mut inner = [0i16; INNER_N];
        for (i, o) in outer.iter_mut().enumerate() {
            *o = ((i * 37) % 251) as i16 - 100;
        }
        for (i, o) in inner.iter_mut().enumerate() {
            *o = ((i * 53) % 211) as i16 - 60;
        }
        let (tx, ty) = (30u32, 41u32);
        tiles[tile_index(tx, ty)] = Some(MareTile { outer, inner });
        let wdl = WdlFile { tiles };
        let mesh = wdl.tile_mesh(tx, ty).unwrap();
        for (i, p) in mesh.positions.iter().enumerate() {
            if i < OUTER_N {
                let (r, c) = (i / OUTER_EDGE, i % OUTER_EDGE);
                if r == OUTER_EDGE - 1 || c == OUTER_EDGE - 1 {
                    continue; // neighbour-tile lattice line
                }
            }
            let h = wdl.height_at(p[0], p[1]).unwrap();
            assert!(
                (h - p[2]).abs() < 0.01,
                "vertex {i} at ({}, {}): height_at {h} vs mesh {}",
                p[0],
                p[1],
                p[2]
            );
        }
        // Unauthored tile → None (never "ground at 0").
        assert_eq!(wdl.height_at(0.0, 0.0), None);
    }
}
