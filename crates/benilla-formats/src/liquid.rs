//! MCLQ liquid surfaces to per-chunk liquid meshes, in raw WoW coords (+X north, +Y west, +Z up).
//!
//! An MCNK carries one 804-byte MCLQ block per set liquid header bit, each a 9×9 absolute-height
//! grid and an 8×8 cell-flag grid. Each wet cell is two triangles; a dry cell (nibble `0xf`,
//! corners at the FLT_MAX sentinel) is skipped. The cell nibble is the liquid type (`0x6ba970`,
//! `0x68d9b0`); the header bits only count the blocks.

use benilla_adt::MclqChunk;

use crate::terrain::{snap_to_lattice, UNIT_SIZE};

/// A liquid's animated texture set and render path; ADT liquid is only Still, Ocean or Magma.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    /// Still water, `XTextures\river\lake_a.*` (30 frames): lakes, ponds, slow rivers, WMO canals.
    Still,
    /// Rapids, `XTextures\river\fast_a.*` (16 frames), nibble 8 in the name table `0x86a000`. No
    /// shipped MCLQ or MLIQ cell carries 8, and the ADT path cannot reach it.
    Rapids,
    /// Ocean, `XTextures\ocean\ocean_h.*` (30 frames).
    Ocean,
    /// Magma, `XTextures\lava\lava.*`: opaque and unlit, but fogged like every liquid batch, since
    /// no liquid setup overrides the device's fog-on default (`0x593d18`). Both paths reach it; its
    /// ADT UVs are authored per vertex ([`benilla_adt::LiquidVertex::texcoords`]).
    Magma,
    /// Slime, `XTextures\slime\slime.*`: magma's render path with its own texture (one handler,
    /// `0x6b68f0`). WMO only (nibbles 3 and 7): the ADT dispatch `0x68de40` has no slime queue.
    Slime,
}

impl LiquidKind {
    /// The kind a WMO tile nibble selects: it indexes the animated-texture name table `0x86a000`
    /// directly, with 4, 6 and 7 as variants of 0, 2 and 3.
    pub fn from_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 0xf {
            0 | 4 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 | 6 => Some(LiquidKind::Magma),
            3 | 7 => Some(LiquidKind::Slime),
            8 => Some(LiquidKind::Rapids),
            _ => None, // 5, 9..=0xf: empty name-table slots and the 0xf hole
        }
    }

    /// The kind an ADT cell nibble selects, by its class `nibble & 3`: the reference draws ADT
    /// liquid in three queues with fixed textures, `lake_a` (`0x6851b0`, `0x68db9d`), `ocean_h`
    /// (`0x685010`, `0x68dabb`) and `lava` (`0x6855a0`, `0x68dcab`), and none for slime. The WMO
    /// path binds the raw nibble instead (`0x68aac0`), as [`Self::from_nibble`] does.
    pub fn from_adt_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 3 {
            0 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 => Some(LiquidKind::Magma),
            _ => None, // class 3, slime: no ADT queue
        }
    }

    /// Whether this kind is opaque and unlit, its texture the body colour (magma, slime); water and
    /// ocean take the `ocean0_s.bls` depth-swatch path. Unlit is not unfogged (`0x593d18`).
    pub fn is_fullbright(self) -> bool {
        matches!(self, LiquidKind::Magma | LiquidKind::Slime)
    }
}

/// One liquid surface, an MCNK's MCLQ or a WMO group's MLIQ: a regular vertex grid in raw WoW
/// coords and the triangles over its wet cells. Queries read the grid, as the reference samples
/// liquid height bilinearly over the containing cell's corners (`0x6b7500`). Drawn two-sided, as
/// every reference liquid pass turns culling off (`0x6851da`, `0x6b6302`).
#[derive(Debug, Clone)]
pub struct LiquidMesh {
    /// Vertex counts `[cols, rows]`, cols fastest: `[9, 9]` for MCLQ, the header's
    /// `xverts`/`yverts` for MLIQ. Every per-vertex array is this grid, row-major.
    pub grid: [u32; 2],
    /// Per-cell coverage over `(cols−1)·(rows−1)`: the nibble says liquid and, for MCLQ, the
    /// corners are real. The containment truth, since the grid's bounds span dry cells.
    pub wet: Vec<bool>,
    /// Per cell, the MLIQ tile flag's `0x80`: another group claims the cell too. The reference
    /// draws it only for the group winning a per-frame 2-colouring of the portal graph, so the
    /// translucent overlap draws once; applied in `benilla_assets::wmo`, `false` on the ADT path.
    pub shared: Vec<bool>,
    /// Vertex positions in WoW yards, `cols·rows` of them.
    pub positions: Vec<[f32; 3]>,
    /// Texture UVs: generated at ¼ per cell for water and ocean (WMO liquid in model space, same
    /// period); ADT magma's are authored per vertex (`u = s · 3/256`), since the reference's lava
    /// batch is single-stage with a reset texture matrix.
    pub uvs: Vec<[f32; 2]>,
    /// Per-vertex swatch coord V (0..1) from the MCLQ depth byte: `clamp(byte/42)` for river and
    /// lake, `clamp(byte/255)` for ocean. The reference indexes the depth swatch with this one V
    /// for both the body colour and the opacity ramp (`colorTex.a = 127 + 2·row`). 0 on ADT magma,
    /// whose union bytes are texture coords and whose vert-fill (`0x68d890`) reads no ramp.
    pub depths: Vec<f32>,
    /// Triangle indices, 6 per wet cell in row-major order, derived from [`Self::wet`].
    pub indices: Vec<u32>,
    /// The sound nibble, class `& 3` and FluidSpeed `& 0xc`: the majority wet cell's (ADT) or the
    /// `0x6ba970` one (WMO), keyed through `SoundWaterType.dbc` (`0x462a40`). Kept beside `kind`,
    /// which merges the river speeds (nibbles 0 and 4) that the sound table splits.
    pub sound_nibble: u8,
    /// WMO only: the MLIQ header's `materialId`, an index into the root's MOMT, whose `diffColor`
    /// is the reference's interior water body colour. `None` on the ADT path.
    pub material_id: Option<u16>,
    pub kind: LiquidKind,
}

/// 9×9 vertex grid per MCNK.
const GRID: usize = 9;
/// 8×8 cell grid per MCNK.
const CELLS: usize = 8;
/// Cell-flag low nibble meaning "dry / do not render".
const DRY_NIBBLE: u8 = 0x0f;
/// The reference's scale on a magma vertex's authored `u16` texcoords, `0x3c400000` (3/256):
/// `u = (s as i32 as f64 · scale) as f32`, bit-exact to `0x68d890`.
const MAGMA_TEXCOORD_SCALE: f32 = f32::from_bits(0x3c40_0000);
/// Verts under no liquid carry FLT_MAX (`0x7F7FFFFF`), which is finite, so gate on magnitude.
const HEIGHT_SENTINEL: f32 = 1.0e9;
/// The river and lake depth byte at which V saturates, `clamp(byte/42)`: the LUT `0xc81768`, built
/// by `0x68c4c0` as `clamp((d/9)/4.6667)` (`0x81028c`, `0x810380`) and read by the river fill
/// `0x68d790`. About 5 yd of water.
const RIVER_DEPTH_V_SATURATION: f32 = 42.0;
/// The ocean depth byte at which V saturates, `clamp(byte/255)`: the ocean's own LUT `0xc7fcd8`,
/// built beside the river's (`0x68c57c`) and read by the ocean fill `0x68d690` at `0x68d718`, as
/// the river fill reads its own at `0x68d818`. The divisors differ because the bytes do: over every
/// Azeroth and Kalimdor MCLQ block (`examples/liquid_depth_census`) the sea is authored at 1.72
/// byte/yd against the river's 8.96, and 83.4 % of ocean vertices sit at 255.
const OCEAN_DEPTH_V_SATURATION: f32 = 255.0;

/// `WOW_OCEAN_DEPTH_DIV=<divisor>` overrides [`OCEAN_DEPTH_V_SATURATION`] for one run, read once;
/// 42 puts the river's ramp on the sea for a side-by-side at a shoreline.
fn ocean_depth_v_divisor() -> f32 {
    static DIV: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *DIV.get_or_init(|| {
        std::env::var("WOW_OCEAN_DEPTH_DIV")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|d| *d > 0.0)
            .unwrap_or(OCEAN_DEPTH_V_SATURATION)
    })
}

/// Mesh one MCLQ block at the MCNK header's raw WoW `position`; `None` when no cell is wet or the
/// cells are slime, which has no ADT queue.
pub(crate) fn build_liquid_mesh(mclq: &MclqChunk, position: [f32; 3]) -> Option<LiquidMesh> {
    if mclq.vertices.len() < GRID * GRID || mclq.tile_flags.len() < CELLS * CELLS {
        return None;
    }

    // ── Cells first: they decide the kind. ──
    let mut indices = Vec::with_capacity(CELLS * CELLS * 6);
    let mut wet = vec![false; CELLS * CELLS];
    // The majority nibble over drawn cells is both the render kind and the sound class.
    let mut nibble_counts = [0u32; 16];
    for row in 0..CELLS {
        for col in 0..CELLS {
            let flag = mclq.tile_flags[row * CELLS + col] & 0x0f;
            if flag == DRY_NIBBLE {
                continue;
            }
            let tl = (row * GRID + col) as u32;
            let tr = tl + 1;
            let bl = ((row + 1) * GRID + col) as u32;
            let br = bl + 1;
            // A wet cell with a sentinel or NaN corner is a data anomaly; skip it.
            if [tl, tr, bl, br].iter().any(|&i| {
                let raw = mclq.vertices[i as usize].height;
                !raw.is_finite() || raw.abs() >= HEIGHT_SENTINEL
            }) {
                continue;
            }
            nibble_counts[flag as usize] += 1;
            // One gate for `wet` and the triangles, so the queries and the drawing agree.
            wet[row * CELLS + col] = true;
            indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
        }
    }
    if indices.is_empty() {
        return None;
    }

    // The kind is the cells' nibble, not the header bits; a shipped block is one class throughout.
    let sound_nibble = nibble_counts
        .iter()
        .enumerate()
        .max_by_key(|(_, &c)| c)
        .map(|(n, _)| n as u8)
        .unwrap_or(0);
    let kind = LiquidKind::from_adt_nibble(sound_nibble)?;

    let depth_v_div = match kind {
        LiquidKind::Ocean => ocean_depth_v_divisor(),
        _ => RIVER_DEPTH_V_SATURATION,
    };
    // Magma's UVs are authored and its depth unread; the rest generate UVs and ramp the depth byte.
    let authored_uvs = kind == LiquidKind::Magma;
    let [wx, wy, _wz] = position;

    // Rows step south (−X) and columns east (−Y) like the MCVT outer grid, on the terrain's
    // lattice; Z is the MCLQ absolute height.
    let mut positions = Vec::with_capacity(GRID * GRID);
    let mut uvs = Vec::with_capacity(GRID * GRID);
    let mut depths = Vec::with_capacity(GRID * GRID);
    for n in 0..GRID * GRID {
        let row = (n / GRID) as f32;
        let col = (n % GRID) as f32;
        // A dry vert's sentinel would stretch the AABB to ~1e38 and get a partly wet chunk culled;
        // it takes `min_height`, unreferenced.
        let raw = mclq.vertices[n].height;
        let h = if raw.is_finite() && raw.abs() < HEIGHT_SENTINEL {
            raw
        } else {
            mclq.min_height
        };
        positions.push([
            snap_to_lattice(wx - row * UNIT_SIZE),
            snap_to_lattice(wy - col * UNIT_SIZE),
            h,
        ]);
        if authored_uvs {
            // Unsigned: the reference widens the `u16` to `i32`, so 0xffff is 65535, never −1.
            let [s, t] = mclq.vertices[n].texcoords();
            let scale = f64::from(MAGMA_TEXCOORD_SCALE);
            uvs.push([
                ((f64::from(s)) * scale) as f32,
                ((f64::from(t)) * scale) as f32,
            ]);
            depths.push(0.0);
        } else {
            uvs.push([col * 0.25, row * 0.25]);
            depths.push((mclq.vertices[n].depth_byte() as f32 / depth_v_div).clamp(0.0, 1.0));
        }
    }

    Some(LiquidMesh {
        grid: [GRID as u32, GRID as u32],
        shared: vec![false; wet.len()],
        wet,
        positions,
        uvs,
        depths,
        indices,
        sound_nibble,
        material_id: None, // ADT liquid has no MOMT to index
        kind,
    })
}

impl LiquidMesh {
    /// Drop the cells `keep` rejects (indexed like [`Self::wet`]) and rebuild [`Self::indices`];
    /// the vertex grid stays whole. Applies [`Self::shared`] once the model has every group.
    pub fn retain_cells(&mut self, keep: impl Fn(usize) -> bool) {
        let (cols, rows) = (self.grid[0] as usize, self.grid[1] as usize);
        let (xt, yt) = (cols.saturating_sub(1), rows.saturating_sub(1));
        let mut dropped = false;
        for c in 0..self.wet.len() {
            if self.wet[c] && !keep(c) {
                self.wet[c] = false;
                dropped = true;
            }
        }
        if !dropped {
            return;
        }
        self.indices.clear();
        for ty in 0..yt {
            for tx in 0..xt {
                if !self.wet[ty * xt + tx] {
                    continue;
                }
                let tl = (ty * cols + tx) as u32;
                let (tr, bl) = (tl + 1, ((ty + 1) * cols + tx) as u32);
                let br = bl + 1;
                self.indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use benilla_adt::{parse_adt, ParsedAdt};

    use super::*;

    /// Every liquid mesh a real tile builds.
    fn tile_liquids(chain: &mut crate::Chain, map: &str, tx: u32, ty: u32) -> Vec<LiquidMesh> {
        let bytes = chain
            .read_file(&format!("World\\Maps\\{map}\\{map}_{tx}_{ty}.adt"))
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");
        root.mcnk_chunks
            .iter()
            .flat_map(|m| {
                m.liquids
                    .iter()
                    .filter_map(|lq| build_liquid_mesh(lq, m.header.position))
            })
            .collect()
    }

    /// The surface height where a mesh's wet cells cover a raw WoW `(x, y)`.
    fn wet_at(m: &LiquidMesh, x: f32, y: f32) -> Option<f32> {
        let cols = m.grid[0] as usize;
        for (c, _) in m.wet.iter().enumerate().filter(|(_, &w)| w) {
            let (row, col) = (c / (cols - 1), c % (cols - 1));
            let corners = [
                row * cols + col,
                row * cols + col + 1,
                (row + 1) * cols + col,
                (row + 1) * cols + col + 1,
            ];
            let xs: Vec<f32> = corners.iter().map(|&i| m.positions[i][0]).collect();
            let ys: Vec<f32> = corners.iter().map(|&i| m.positions[i][1]).collect();
            let (x0, x1) = (
                xs.iter().cloned().fold(f32::MAX, f32::min),
                xs.iter().cloned().fold(f32::MIN, f32::max),
            );
            let (y0, y1) = (
                ys.iter().cloned().fold(f32::MAX, f32::min),
                ys.iter().cloned().fold(f32::MIN, f32::max),
            );
            if (x0..=x1).contains(&x) && (y0..=y1).contains(&y) {
                return Some(corners.iter().map(|&i| m.positions[i][2]).sum::<f32>() / 4.0);
            }
        }
        None
    }

    /// Open-world lava is MCLQ magma: the pin, `.go xyz -7845.99 -1065.50 123.60 0` in Burning
    /// Steppes (tile 33_46), stands on it.
    #[test]
    fn burning_steppes_lava_builds_a_magma_surface_at_the_reported_pin() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let meshes = tile_liquids(&mut chain, "Azeroth", 33, 46);

        let magma: Vec<&LiquidMesh> = meshes
            .iter()
            .filter(|m| m.kind == LiquidKind::Magma)
            .collect();
        assert_eq!(magma.len(), 64, "Burning Steppes tile 33_46 magma chunks");

        // The pin itself is on lava, not merely the tile.
        let (px, py, pz) = (-7845.99f32, -1065.50, 123.60);
        let hit = magma
            .iter()
            .find_map(|m| wet_at(m, px, py))
            .expect("the reported pin stands on a magma surface");
        assert!(
            (hit - pz).abs() < 6.0,
            "lava surface {hit} sits at the reported eye height {pz}"
        );

        // Nibble 6 is class 2 (magma) at FluidSpeed 4, the lava-flow loop.
        assert!(magma.iter().all(|m| m.kind.is_fullbright()));
        assert!(
            magma.iter().all(|m| m.sound_nibble == 6),
            "shipped ADT magma is nibble 6 throughout"
        );
        // No ADT slime ships, and the reference has no ADT queue for it.
        assert!(
            !meshes.iter().any(|m| m.kind == LiquidKind::Slime),
            "slime never renders from an ADT"
        );
    }

    /// Lava UVs are authored world-continuously, so a chunk's east edge repeats its neighbour's
    /// west edge; reading the wrong bytes shows as a seam.
    #[test]
    fn magma_uvs_come_from_the_vertex_and_run_continuous_across_chunks() {
        let data = crate::wow_data_or_skip!();
        if !data.is_dir() {
            return;
        }
        let mut chain = crate::open_chain(&data).expect("open chain");
        let magma: Vec<LiquidMesh> = tile_liquids(&mut chain, "Azeroth", 33, 46)
            .into_iter()
            .filter(|m| m.kind == LiquidKind::Magma)
            .collect();

        // Not the generated ¼-per-cell field: authored steps are ≈42 units ≈ 0.49 UV per cell.
        let stepped = magma.iter().any(|m| {
            let du = (m.uvs[1][0] - m.uvs[0][0]).abs();
            du > 0.001 && (du - 0.25).abs() > 0.05
        });
        assert!(
            stepped,
            "magma UVs are authored, not the water cell-index UVs"
        );

        // Two chunks adjacent along −Y: the west one's column 8 is the east one's column 0.
        let mut shared = 0;
        for a in &magma {
            for b in &magma {
                let (pa, pb) = (a.positions[8], b.positions[0]);
                if (pa[0] - pb[0]).abs() < 0.01 && (pa[1] - pb[1]).abs() < 0.01 {
                    for row in 0..9 {
                        let (ua, ub) = (a.uvs[row * 9 + 8], b.uvs[row * 9]);
                        // Dry verts carry no authored coord.
                        if ua == [0.0, 0.0] || ub == [0.0, 0.0] {
                            continue;
                        }
                        assert!(
                            (ua[0] - ub[0]).abs() < 1e-4 && (ua[1] - ub[1]).abs() < 1e-4,
                            "shared lava edge vertex must carry one UV: {ua:?} vs {ub:?}"
                        );
                        shared += 1;
                    }
                }
            }
        }
        assert!(shared > 0, "no shared lava chunk edge found to check");
    }

    /// An MCNK carries one MCLQ block per set liquid header bit (`0x6af7a3`); a river mouth, as on
    /// Hillsbrad's tile 33_33, carries a stream over the sea.
    #[test]
    fn a_river_mouth_chunk_builds_both_its_river_and_its_sea() {
        let data = crate::wow_data_or_skip!();
        if !data.is_dir() {
            return;
        }
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Maps\\Azeroth\\Azeroth_33_33.adt")
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");

        let two_block: Vec<&benilla_adt::McnkChunk> = root
            .mcnk_chunks
            .iter()
            .filter(|m| m.liquids.len() > 1)
            .collect();
        assert!(
            !two_block.is_empty(),
            "Azeroth_33_33 carries river-mouth chunks with two liquid blocks"
        );

        for mcnk in two_block {
            assert_eq!(mcnk.liquids.len(), 2, "two set liquid bits ⇒ two blocks");
            let built: Vec<LiquidMesh> = mcnk
                .liquids
                .iter()
                .filter_map(|lq| build_liquid_mesh(lq, mcnk.header.position))
                .collect();
            assert_eq!(built.len(), 2, "both blocks mesh");
            // Disk order is header-bit order, bit 2 (river) then bit 3 (ocean).
            assert_eq!(built[0].kind, LiquidKind::Still, "block 0 is the stream");
            assert_eq!(built[1].kind, LiquidKind::Ocean, "block 1 is the sea");
            let sea = built[1].positions[built[1].indices[0] as usize][2];
            let river = built[0].positions[built[0].indices[0] as usize][2];
            assert!(
                river > sea + 1.0,
                "the stream ({river}) sits above sea level ({sea})"
            );
            // `benilla_world`'s `liquid_at` takes the lowest of stacked surfaces, so a cell wet in
            // both would read the stream as dry; the blocks are authored disjoint. Only Kalimdor
            // tile 26_54 has overlapping blocks, a sheet at −23.47 under open sea.
            for (cell, (&r, &o)) in built[0].wet.iter().zip(&built[1].wet).enumerate() {
                assert!(
                    !(r && o),
                    "cell {cell} is wet in BOTH blocks — the stacked-surface case liquid_at \
                     resolves lowest-wins, which would read this stream as dry"
                );
            }
        }
    }

    /// Every liquid mesh a real tile builds, paired with the raw MCLQ block it came from.
    fn tile_liquids_with_source(
        chain: &mut crate::Chain,
        map: &str,
        tx: u32,
        ty: u32,
    ) -> Vec<(benilla_adt::MclqChunk, LiquidMesh)> {
        let bytes = chain
            .read_file(&format!("World\\Maps\\{map}\\{map}_{tx}_{ty}.adt"))
            .expect("read adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");
        root.mcnk_chunks
            .iter()
            .flat_map(|m| {
                m.liquids.iter().filter_map(|lq| {
                    build_liquid_mesh(lq, m.header.position).map(|mesh| (lq.clone(), mesh))
                })
            })
            .collect()
    }

    /// The V assertions read the raw depth byte, never one recovered from the mesh's V, which would
    /// pass under any divisor.
    #[test]
    fn each_liquid_kind_rides_its_own_verified_depth_divisor() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");

        // Baradin Bay: shelf water whose bytes span the ocean ramp rather than pinning at 255.
        let ocean: Vec<(benilla_adt::MclqChunk, LiquidMesh)> =
            tile_liquids_with_source(&mut chain, "Azeroth", 41, 41)
                .into_iter()
                .filter(|(_, m)| m.kind == LiquidKind::Ocean)
                .collect();
        assert!(!ocean.is_empty(), "Azeroth_41_41 carries ocean");

        let mut raw: Vec<u8> = Vec::new();
        for (src, mesh) in &ocean {
            for (n, v) in mesh.depths.iter().enumerate() {
                let byte = src.vertices[n].depth_byte();
                raw.push(byte);
                assert!(
                    (*v - f32::from(byte) / OCEAN_DEPTH_V_SATURATION).abs() < 1e-4,
                    "ocean V is the raw byte over 255 (VERIFIED `c7fcd8`); byte {byte} gave V={v}"
                );
            }
        }

        // Under `/42` every one of these mid-band verts would already read fully deep.
        let mid = raw.iter().filter(|&&b| (42..=200).contains(&b)).count();
        let pinned = raw.iter().filter(|&&b| b == u8::MAX).count();
        assert!(
            mid > 100,
            "this tile must carry a real shore ramp to test, saw {mid} mid-band verts"
        );
        assert!(
            mid * 4 > pinned,
            "the `/42` divisor would flatten a shore band this tile spends {mid} verts ramping \
             through (against {pinned} genuinely pinned-deep) — the look change the ocean ramp is not"
        );

        // Control: Elwynn's rivers ride `/42`, and their channel middles saturate.
        let river: Vec<(benilla_adt::MclqChunk, LiquidMesh)> =
            tile_liquids_with_source(&mut chain, "Azeroth", 32, 48)
                .into_iter()
                .filter(|(_, m)| m.kind == LiquidKind::Still)
                .collect();
        assert!(!river.is_empty(), "Azeroth_32_48 carries river/lake water");
        let mut saturated_below_255 = false;
        for (src, mesh) in &river {
            for (n, v) in mesh.depths.iter().enumerate() {
                let byte = src.vertices[n].depth_byte();
                let want = (f32::from(byte) / RIVER_DEPTH_V_SATURATION).clamp(0.0, 1.0);
                assert!(
                    (*v - want).abs() < 1e-4,
                    "river V is clamp(byte/42) (VERIFIED `c81768`); byte {byte} gave V={v}"
                );
                // A byte well under 255 reading fully deep, which `/255` could never produce.
                saturated_below_255 |= (42..200).contains(&byte) && *v >= 0.999;
            }
        }
        assert!(
            saturated_below_255,
            "a river channel middle saturates to the deep row by byte 42, not byte 255"
        );
    }

    #[test]
    fn parses_elwynn_lake_tile() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Maps\\Azeroth\\Azeroth_32_48.adt")
            .expect("read Azeroth_32_48.adt");
        let ParsedAdt::Root(root) =
            parse_adt(&mut std::io::Cursor::new(&bytes[..])).expect("parse adt");

        let mut meshes = Vec::new();
        for mcnk in &root.mcnk_chunks {
            for mclq in &mcnk.liquids {
                if let Some(m) = build_liquid_mesh(mclq, mcnk.header.position) {
                    meshes.push(m);
                }
            }
        }

        assert_eq!(
            meshes.len(),
            42,
            "expected 42 liquid chunks on Azeroth_32_48"
        );
        assert!(
            meshes.iter().all(|m| m.kind == LiquidKind::Still),
            "Crystal Lake is all still water"
        );

        for m in &meshes {
            assert_eq!(m.positions.len(), 81, "9×9 grid");
            assert_eq!(m.uvs.len(), 81, "uv per vertex");
            assert_eq!(m.depths.len(), 81, "depth per vertex");
            // Even unreferenced verts stay finite, or the AABB blows up and the chunk is culled.
            for p in &m.positions {
                assert!(
                    p[2].is_finite() && p[2].abs() < 10_000.0,
                    "all liquid verts sane for a clean AABB, got z={}",
                    p[2]
                );
            }
            assert!(!m.indices.is_empty(), "at least one wet cell");
            assert_eq!(m.indices.len() % 6, 0, "2 tris per wet cell");
            // Elwynn's water sits between 50 and 300 yd (Crystal Lake ≈ 144).
            assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
            for &i in &m.indices {
                let z = m.positions[i as usize][2];
                assert!(
                    z.is_finite() && z.abs() < 10_000.0,
                    "referenced height {z} sane"
                );
                assert!(
                    (50.0..300.0).contains(&z),
                    "Elwynn water elevation {z} plausible"
                );
            }
            // A chunk's surface is near-planar; a stream slopes a little across 33 yd.
            let zs: Vec<f32> = m
                .indices
                .iter()
                .map(|&i| m.positions[i as usize][2])
                .collect();
            let (zmin, zmax) = zs
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &z| (lo.min(z), hi.max(z)));
            assert!(
                zmax - zmin < 30.0,
                "per-chunk surface roughly flat ({zmin}..{zmax})"
            );
        }
    }
}
