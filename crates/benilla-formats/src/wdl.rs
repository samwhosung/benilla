//! WDL, the low-detail distant terrain: loader and coarse mesher. One `.wdl` per map holds the
//! whole 64×64-tile world at one height per MCNK, the horizon the reference draws beyond the
//! streamed tiles out to `horizonfarclip`, unlit, untextured, vertex-white and fogged.
//!
//! Layout: `MVER` (version 18), `MAOF` (4096 `u32` tile offsets, `[tile_y][tile_x]`), then per
//! present tile a `MARE` of 17×17 outer and 16×16 inner `int16` heights; vanilla Azeroth has no
//! `MWMO`/`MWID`/`MODF` or `MAHO`. The mesh is raw WoW world coordinates (X north, Y west, Z up).

use std::io::{Cursor, Read, Seek, SeekFrom};

use crate::Chain;
use anyhow::{bail, Context, Result};

use crate::terrain::TILE_SIZE;

/// 16 cells a tile edge: 17×17 corners and 16×16 cell centres, an MCNK's grid at tile scale.
const OUTER_EDGE: usize = 17;
const INNER_EDGE: usize = 16;
const OUTER_N: usize = OUTER_EDGE * OUTER_EDGE;
const INNER_N: usize = INNER_EDGE * INNER_EDGE;
const MARE_BYTES: usize = (OUTER_N + INNER_N) * 2;
const CHUNK_SIZE: f32 = TILE_SIZE / 16.0;
/// World coordinate of tile (0, 0)'s max-X, max-Y corner; the 64-tile map is centred on the origin.
const MAP_OFFSET: f64 = 32.0 * (TILE_SIZE as f64);

/// A map's parsed WDL: the present tiles' low-detail heightmaps, indexed by MAOF position.
pub struct WdlFile {
    /// 64×64 in MAOF order, `[tile_y * 64 + tile_x]`, with `tile_x`/`tile_y` as in
    /// `Map_<tile_x>_<tile_y>.adt` and `benilla_wdt::world_to_tile`; `None` where the map has none.
    tiles: Vec<Option<MareTile>>,
}

/// One tile's absolute heights in yards: 17×17 corners, then 16×16 cell centres, row-major.
struct MareTile {
    outer: [i16; OUTER_N],
    inner: [i16; INNER_N],
}

/// A coarse WDL tile mesh in raw WoW world coordinates, with no normals or UVs: the reference
/// draws it unlit, vertex-white and fogged.
pub struct WdlTileMesh {
    /// 545 verts: 289 outer (`[r*17+c]`) then 256 inner (`[289 + r*16+c]`).
    pub positions: Vec<[f32; 3]>,
    /// 3072 indices = 16×16 cells × 4 triangles.
    pub indices: Vec<u32>,
}

/// The MAOF index of a tile, row-major over `tile_y` as on disk and in `benilla_wdt`.
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

    /// Present tiles within a Chebyshev `radius` of world `(x, y)`, the centre tile included, as
    /// the reference's far walk (`0x683040`) visits a ±3-tile window. Below a tile of view distance
    /// (vanilla goes down to 177) the camera's own tile draws the near horizon, and without it sky
    /// shows through; the far band's near plane in `wdl.wgsl` alone bounds that side.
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

    /// The coarse centre-fan mesh of tile `(tile_x, tile_y)`. Vertex X/Y come from the global
    /// lattice index in `f64`, so neighbouring tiles share bit-identical edges.
    pub fn tile_mesh(&self, tile_x: u32, tile_y: u32) -> Option<WdlTileMesh> {
        if tile_x >= 64 || tile_y >= 64 {
            return None;
        }
        let tile = self.tiles[tile_index(tile_x, tile_y)].as_ref()?;
        let cs = CHUNK_SIZE as f64;
        // Lattice position of a global row/col index (X north falls with row, Y west with col).
        let lat = |global: f64| (MAP_OFFSET - global * cs) as f32;

        let mut positions = Vec::with_capacity(OUTER_N + INNER_N);
        for r in 0..OUTER_EDGE {
            for c in 0..OUTER_EDGE {
                positions.push([
                    lat((tile_y as usize * 16 + r) as f64),
                    lat((tile_x as usize * 16 + c) as f64),
                    f32::from(tile.outer[r * OUTER_EDGE + c]),
                ]);
            }
        }
        for r in 0..INNER_EDGE {
            for c in 0..INNER_EDGE {
                positions.push([
                    lat(tile_y as usize as f64 * 16.0 + r as f64 + 0.5),
                    lat(tile_x as usize as f64 * 16.0 + c as f64 + 0.5),
                    f32::from(tile.inner[r * INNER_EDGE + c]),
                ]);
            }
        }

        // Four-triangle centre fan per cell, wound CCW from above like the terrain so it faces up
        // after the WoW-to-Bevy transform.
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

    /// The drawn WDL height (absolute WoW `z`) under world `(x, y)`, interpolated over the same
    /// centre fan [`Self::tile_mesh`] builds, so it matches the rendered horizon exactly.
    pub fn height_at(&self, world_x: f32, world_y: f32) -> Option<f32> {
        let cs = CHUNK_SIZE as f64;
        // Global lattice coordinates, the inverse of `tile_mesh`'s `lat`: 1024 cells per axis.
        let gr = (MAP_OFFSET - f64::from(world_x)) / cs;
        let gc = (MAP_OFFSET - f64::from(world_y)) / cs;
        if !(0.0..1024.0).contains(&gr) || !(0.0..1024.0).contains(&gc) {
            return None;
        }
        let (cell_r, cell_c) = (gr as usize, gc as usize);
        let tile = self.tiles[tile_index((cell_c / 16) as u32, (cell_r / 16) as u32)].as_ref()?;
        let (r, c) = (cell_r % 16, cell_c % 16);
        let h = |rr: usize, cc: usize| f64::from(tile.outer[rr * OUTER_EDGE + cc]);
        let (tl, tr) = (h(r, c), h(r, c + 1));
        let (bl, br) = (h(r + 1, c), h(r + 1, c + 1));
        let ctr = f64::from(tile.inner[r * INNER_EDGE + c]);
        // `v` runs down the rows, `u` across; the cell's diagonals pick the fan triangle.
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
        // `s` runs 0 at the outer edge to 0.5 at the centre, where the edge parameter degenerates.
        let height = if s >= 0.5 {
            ctr
        } else {
            let edge = a + (b - a) * ((t - s) / (1.0 - 2.0 * s));
            edge + (ctr - edge) * (s * 2.0)
        };
        Some(height as f32)
    }
}

/// Read one IFF chunk header: the magic, stored reversed on disk, and the `u32` LE body size.
fn read_chunk_header(cur: &mut Cursor<&[u8]>) -> Result<([u8; 4], u32)> {
    let mut magic = [0u8; 4];
    cur.read_exact(&mut magic)?;
    magic.reverse();
    let mut size = [0u8; 4];
    cur.read_exact(&mut size)?;
    Ok((magic, u32::from_le_bytes(size)))
}

fn parse_wdl(bytes: &[u8]) -> Result<WdlFile> {
    let mut cur = Cursor::new(bytes);

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

    // Skip to MAOF past any low-detail WMO chunks.
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
        // A tile from a trace of the reference: (tile_x=34, tile_y=48) -> 48*64+34.
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
                                            // 16×16 cells × 4 tris × 3: the reference's draw count.
        assert_eq!(INNER_EDGE * INNER_EDGE * 12, 3072);
        assert!((CHUNK_SIZE - TILE_SIZE / 16.0).abs() < 1e-3);
    }

    /// The tile's last corner row and column belong to the absent neighbour tile in this synthetic
    /// file, so they are skipped.
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
        assert_eq!(wdl.height_at(0.0, 0.0), None);
    }
}
