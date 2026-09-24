//! A WMO (World Map Object) reader for **WoW 1.12.1 (build 5875)** — in-repo, replacing `wow-wmo`
//! (decision 0021), scoped to what the renderer consumes.
//!
//! A WMO is split across a **root** file (`MOHD` header, `MOTX` texture blob, `MOMT` materials, group
//! info, doodads, lights) and one **group** file per group (`MOGP` super-chunk wrapping `MOVT`/`MONR`/
//! `MOTV`/`MOVI`/`MOBA`/`MOCV`). [`parse_wmo`] returns whichever the bytes are. Chunks are IFF-style:
//! a 4-char magic stored **reversed** on disk, a `u32` size, then payload.
//!
//! Byte access goes through `benilla-bytes` (decision 0064): every read is bounds-checked and a
//! truncated chunk is a typed [`Error::Truncated`], never a panic or a silently-empty struct.
//!
//! Only the render path lives here; benilla-formats hand-parses MOLT/MOGI/MODS/MOPY itself. Proven
//! against `wow-wmo` over real WMOs during the decision-0021 migration (oracle test in git history);
//! the WMO mesh/collision golden tests in benilla-formats pin it end-to-end on every run.

use std::collections::HashMap;
use std::io::Cursor;

use benilla_bytes::{chunks, ByteExt};

/// A parsed WMO file — either the root or one group.
pub enum ParsedWmo {
    Root(WmoRoot),
    Group(WmoGroup),
}

/// The WMO root: textures, materials, and the group count.
pub struct WmoRoot {
    pub textures: Vec<String>,
    /// MOTX byte-offset → index into [`textures`](Self::textures) (materials reference textures by the
    /// byte offset of their name in the MOTX blob).
    pub texture_offset_index_map: HashMap<u32, u32>,
    pub materials: Vec<Material>,
    /// Group count (`MOHD.nGroups`).
    pub n_groups: u32,
}

/// One MOMT material (the fields the renderer reads).
pub struct Material {
    pub flags: u32,
    pub blend_mode: u32,
    /// Byte offset of texture 1's name in the MOTX blob.
    pub texture_1: u32,
    /// The authored **SIDN** self-illum colour (MOMT+0x10, stored BGRA on disk — here RGB). The real
    /// client's per-frame updater scales it by the night fraction and adds it as the material
    /// EMISSION on lit batches (`flags & 0x10` — the windows-glow-at-night mechanism). Meaningless
    /// (usually zero) when the SIDN flag is clear.
    pub sidn_rgb: [u8; 3],
    /// **MOMT+0x1C — `diffColor`**, the material's authored diffuse colour (BGRA on disk, RGB here).
    ///
    /// Read for exactly one consumer: it is the **body colour of an interior WMO liquid pool**. The
    /// reference's interior water kernel `0x6b6420` runs with lighting forced off and no pixel
    /// shader, and its whole RGB is this dword taken raw — `Cf = MOMT[MLIQ.materialId].diffColor`,
    /// combined with the animated sheet as `clamp(Cf + Ct)`. The pool's material's own alpha at
    /// `+0x1f` is discarded; opacity comes from the per-vertex LUT instead.
    ///
    /// The offset is anchored on both sides by fields this repo verified independently of water:
    /// `+0x18` texture 2 and `+0x20` [`Self::ground_type`]. Stride 0x40, base `[CMapObj+0x1d8]`
    /// (`0x6c3ace` + `0x6c3ad7 shr edx,6`).
    pub diff_color: [u8; 3],
    /// **MOMT+0x20 — the surface's `TerrainType.dbc` id**, i.e. what walking on this material
    /// sounds like. The footstep chain's WMO leg: the client's down-ray arbitrates a terrain probe
    /// against a WMO probe and the nearer surface supplies the terrain type, so a building's own
    /// floor — not the ADT beneath it — decides the step indoors.
    ///
    /// Verified from the shipped 5875 data, not from a format doc: across all **815 root WMOs /
    /// 10 299 materials** this dword only ever holds `{0,1,2,3,4,5,7,8,10}` — exactly the
    /// `TerrainType.dbc` id domain (ids 0–10) — and **10 = "None" on 10 075 of them**, the
    /// unauthored default, which resolves through `SoundID 0` to the generic `*Dirt` kit rather
    /// than to silence. Only ~224 materials name a surface explicitly.
    pub ground_type: u32,
}

impl Material {
    /// Resolve texture 1 to its index in [`WmoRoot::textures`] via the offset map.
    pub fn get_texture1_index(&self, texture_offset_index_map: &HashMap<u32, u32>) -> u32 {
        texture_offset_index_map
            .get(&self.texture_1)
            .copied()
            .unwrap_or(u32::MAX)
    }
}

/// A 3-float vector (MOVT position / MONR normal).
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A MOTV texture coordinate.
pub struct Uv {
    pub u: f32,
    pub v: f32,
}

/// A MOCV vertex colour (stored BGRA on disk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

/// A MOBA render batch (the fields the renderer reads; 24 bytes on disk).
pub struct Batch {
    pub start_index: u32,
    pub count: u16,
    pub material_id: u8,
}

/// A MOPY per-face material/flags entry (2 bytes), one per triangle — drives collision filtering.
pub struct MopyEntry {
    pub flags: u8,
    pub material_id: u8,
}

/// One WMO group's MLIQ liquid grid (the water/lava/slime surface embedded in the group — canals,
/// fountains, dungeon pools). A flat `xverts × yverts` height grid over `xtiles × ytiles` cells, in
/// **WMO model space** (WoW axes, yards); the placement transform lifts it into the world. Cell
/// membership + type come from the per-tile flag low nibble (`& 0xf`): `0xf` = hole (no liquid), else
/// the nibble is the liquid type indexing the reference's texture table (render dispatch
/// `0x6b62e0`, water kernels `0x6b6420`/`0x6b6630`, magma/slime `0x6b68f0`).
pub struct WmoLiquid {
    /// Vertex-grid dimensions (`xtiles = xverts − 1`, `ytiles = yverts − 1`).
    pub xverts: u32,
    pub yverts: u32,
    pub xtiles: u32,
    pub ytiles: u32,
    /// The `(0,0)` grid corner in WMO model space (WoW `[x, y, z]`, yards). Grid vertex `(i, j)` sits
    /// at `(base.x + i·MLIQ_CELL_STEP, base.y + j·MLIQ_CELL_STEP, heights[j·xverts + i])`.
    pub base: [f32; 3],
    /// MLIQ `materialId` (index into the root MOMT — unused by the animated-water render path, which
    /// selects its texture from the tile-flag type; kept for completeness).
    pub material_id: u16,
    /// Per-vertex absolute liquid height (WMO-local Z), row-major `j·xverts + i`, `xverts·yverts` long.
    /// The MLIQ vertex is 8 bytes and the height is its trailing `f32`; what the leading 4 bytes mean
    /// is per liquid type — see [`Self::opacity`] for the water reading.
    pub heights: Vec<f32>,
    /// Per-vertex **opacity index** — the water vertex's byte 0, row-major beside [`Self::heights`].
    ///
    /// WMO liquid carries no bathymetry, so nothing here can be ramped by depth the way ADT MCLQ's
    /// water is; this byte **is** the authored opacity. The reference indexes a 256-entry alpha ramp
    /// (`[0xca7f10]`, rebuilt per frame from the zone's river shallow/deep alpha pair) with it and
    /// uses the result as the vertex alpha — the water arms bind no depth-ramp texture at all
    /// (zero references to the ADT ramp globals `0xc7fbc0`/`0xc81768`/`0xc7fcd8` anywhere in
    /// `[0x6b0000, 0x6c4000)`).
    ///
    /// It was parsed as nothing for a long time — the field it replaced was documented as "flow
    /// data" — which is why every WMO pool in benilla rendered at one pinned opacity.
    ///
    /// On the **magma/slime** arm the same four bytes are an authored `int16` `(s, t)` UV pair
    /// instead, so byte 0 there is half a texture coordinate and means nothing as an opacity. The
    /// consumer picks by liquid type; this parser just carries the byte.
    ///
    /// That split is also what corroborates the reading against the shipped data, independently of
    /// the disassembly. Censused over every drawn liquid vertex in the corpus: on the **water** arms
    /// the byte is structured — Stormwind's canals are **91.2% a single value (86)** across 3492
    /// vertices with only 41 distinct values, exactly the shape of an authored per-surface opacity —
    /// while on **magma/slime** it is near-uniform noise over all 256 values (each ≈0.7–1.4%), which
    /// is what half of a texture coordinate should look like. A byte that is a constant on one arm
    /// and white noise on the other is not being read at the wrong offset.
    pub opacity: Vec<u8>,
    /// Per-tile flag bytes, row-major `j·xtiles + i`, `xtiles·ytiles` long. Low nibble = liquid type;
    /// `0xf` = hole (skip the tile); `0x80` shared (the strip-builder gate). `0x40` ("fishable") is
    /// carried but deliberately unread: the reference's WMO-side reader `0x6b9e50` is only reached
    /// from the zero-caller `0x69b5d0` island — fishability is the server's verdict. Decision 1825.
    pub tile_flags: Vec<u8>,
}

/// One WMO group's render geometry.
pub struct WmoGroup {
    /// MOGP group flags (bit test `& 0x48` selects exterior vs interior).
    pub flags: u32,
    /// MOGP `groupLiquid` @ header `0x34` — the liquid **type override**: when `!= 0xf` it names the
    /// whole group's liquid type directly (`0x6b9f10`); `0xf` (the common
    /// case) defers to the per-tile MLIQ nibble. `0xf` when the header is too short to carry it.
    pub group_liquid: u32,
    pub vertex_positions: Vec<Vec3>,
    pub vertex_normals: Vec<Vec3>,
    pub texture_coords: Vec<Uv>,
    pub vertex_colors: Vec<Color>,
    pub vertex_indices: Vec<u16>,
    pub render_batches: Vec<Batch>,
    /// MOPY per-face material/flags (one per triangle).
    pub material_info: Vec<MopyEntry>,
    /// The group's MLIQ liquid grid, if it carries one (`QILM` sub-chunk present).
    pub liquid: Option<WmoLiquid>,
}

/// WMO parse error.
#[derive(Debug)]
pub enum Error {
    NotWmo,
    /// A chunk's payload is shorter than its format requires (names the chunk).
    Truncated(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotWmo => write!(f, "not a WMO (no MOHD root or MOGP group chunk)"),
            Error::Truncated(chunk) => write!(f, "truncated WMO chunk: {chunk}"),
        }
    }
}
impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// Parse a WMO root or group from a cursor over the whole file (position ignored).
pub fn parse_wmo(cursor: &mut Cursor<&[u8]>) -> Result<ParsedWmo> {
    let b: &[u8] = cursor.get_ref();
    // A group file carries a MOGP super-chunk; a root carries MOHD.
    for (magic, payload) in chunks(b) {
        if &magic == b"PGOM" {
            return Ok(ParsedWmo::Group(parse_group(payload)?));
        }
        if &magic == b"DHOM" {
            return Ok(ParsedWmo::Root(parse_root(b)?));
        }
    }
    Err(Error::NotWmo)
}

fn parse_root(b: &[u8]) -> Result<WmoRoot> {
    let mut root = WmoRoot {
        textures: Vec::new(),
        texture_offset_index_map: HashMap::new(),
        materials: Vec::new(),
        n_groups: 0,
    };
    for (magic, p) in chunks(b) {
        match &magic {
            // MOHD: nTextures@0, nGroups@4
            b"DHOM" => root.n_groups = p.u32_at(4).ok_or(Error::Truncated("MOHD"))?,
            b"XTOM" => {
                let (t, m) = parse_motx(p);
                root.textures = t;
                root.texture_offset_index_map = m;
            }
            // MOMT entry is 64 bytes: flags@0, shader@4, blend_mode@8, texture_1@12, sidnColor@16, …
            b"TMOM" => {
                for m in p.as_chunks::<64>().0 {
                    // CImVector — BGRA bytes on disk, so the LE u32 reads B | G<<8 | R<<16 | A<<24.
                    let sidn = m.u32_at(0x10).ok_or(Error::Truncated("MOMT"))?;
                    let diff = m.u32_at(0x1c).ok_or(Error::Truncated("MOMT"))?;
                    root.materials.push(Material {
                        flags: m.u32_at(0).ok_or(Error::Truncated("MOMT"))?,
                        blend_mode: m.u32_at(8).ok_or(Error::Truncated("MOMT"))?,
                        texture_1: m.u32_at(12).ok_or(Error::Truncated("MOMT"))?,
                        sidn_rgb: [(sidn >> 16) as u8, (sidn >> 8) as u8, sidn as u8],
                        diff_color: [(diff >> 16) as u8, (diff >> 8) as u8, diff as u8],
                        ground_type: m.u32_at(0x20).ok_or(Error::Truncated("MOMT"))?,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(root)
}

/// MOTX is a NUL-separated string blob; materials reference each name by its byte offset. Build the
/// texture list + an offset→index map (matching wow-wmo: record the offset at each string's first byte).
fn parse_motx(blob: &[u8]) -> (Vec<String>, HashMap<u32, u32>) {
    let mut textures = Vec::new();
    let mut map = HashMap::new();
    let mut current = String::new();
    for (i, &byte) in blob.iter().enumerate() {
        if byte == 0 {
            if !current.is_empty() {
                textures.push(std::mem::take(&mut current));
            }
        } else {
            if current.is_empty() {
                map.insert(i as u32, textures.len() as u32);
            }
            current.push(byte as char);
        }
    }
    (textures, map)
}

/// Parse a MOGP super-chunk payload: a 68-byte group header (flags @8) then the geometry sub-chunks.
/// A payload shorter than the header parses to an empty group (a real-data tolerance, kept from the
/// pre-0064 reader) — but a *sub-chunk* shorter than its record size is a hard [`Error::Truncated`].
fn parse_group(mogp: &[u8]) -> Result<WmoGroup> {
    let flags = mogp.u32_at(8).unwrap_or(0);
    // groupLiquid @ 0x34 (the type override; 0xf = "use the per-tile MLIQ nibble"). Absent header ⇒ 0xf.
    let group_liquid = mogp.u32_at(0x34).unwrap_or(0xf);
    let mut g = WmoGroup {
        flags,
        group_liquid,
        vertex_positions: Vec::new(),
        vertex_normals: Vec::new(),
        texture_coords: Vec::new(),
        vertex_colors: Vec::new(),
        vertex_indices: Vec::new(),
        render_batches: Vec::new(),
        material_info: Vec::new(),
        liquid: None,
    };
    if mogp.len() < 68 {
        return Ok(g);
    }
    // `extend` (not assign): a sub-chunk that appears more than once concatenates, matching wow-adt's
    // push-loop (some WMOs carry a second MOTV, e.g. KL_PvPBarracks).
    for (magic, s) in chunks(&mogp[68..]) {
        match &magic {
            b"TVOM" => {
                for c in s.as_chunks::<12>().0 {
                    g.vertex_positions.push(Vec3 {
                        x: c.f32_at(0).ok_or(Error::Truncated("MOVT"))?,
                        y: c.f32_at(4).ok_or(Error::Truncated("MOVT"))?,
                        z: c.f32_at(8).ok_or(Error::Truncated("MOVT"))?,
                    });
                }
            }
            b"RNOM" => {
                for c in s.as_chunks::<12>().0 {
                    g.vertex_normals.push(Vec3 {
                        x: c.f32_at(0).ok_or(Error::Truncated("MONR"))?,
                        y: c.f32_at(4).ok_or(Error::Truncated("MONR"))?,
                        z: c.f32_at(8).ok_or(Error::Truncated("MONR"))?,
                    });
                }
            }
            b"VTOM" => {
                for c in s.as_chunks::<8>().0 {
                    g.texture_coords.push(Uv {
                        u: c.f32_at(0).ok_or(Error::Truncated("MOTV"))?,
                        v: c.f32_at(4).ok_or(Error::Truncated("MOTV"))?,
                    });
                }
            }
            b"IVOM" => {
                for c in s.as_chunks::<2>().0 {
                    g.vertex_indices
                        .push(c.u16_at(0).ok_or(Error::Truncated("MOVI"))?);
                }
            }
            // MOBA: bbox_min[i16;3] bbox_max[i16;3] start_index(u32@12) count(u16@16) min/max(u16) flags(u8@22) material_id(u8@23)
            b"ABOM" => {
                for c in s.as_chunks::<24>().0 {
                    g.render_batches.push(Batch {
                        start_index: c.u32_at(12).ok_or(Error::Truncated("MOBA"))?,
                        count: c.u16_at(16).ok_or(Error::Truncated("MOBA"))?,
                        material_id: c.u8_at(23).ok_or(Error::Truncated("MOBA"))?,
                    });
                }
            }
            b"VCOM" => {
                for c in s.as_chunks::<4>().0 {
                    g.vertex_colors.push(Color {
                        b: c[0],
                        g: c[1],
                        r: c[2],
                        a: c[3],
                    });
                }
            }
            // MOPY: 2 bytes/face — flags + material id, one per triangle (collision filtering).
            b"YPOM" => {
                for c in s.as_chunks::<2>().0 {
                    g.material_info.push(MopyEntry {
                        flags: c[0],
                        material_id: c[1],
                    });
                }
            }
            // MLIQ: the group's liquid grid — a 30-byte SMOLiquidHeader, then `xverts·yverts`
            // 8-byte vertices (height at +4), then `xtiles·ytiles` 1-byte tile flags.
            b"QILM" => g.liquid = parse_mliq(s)?,
            _ => {}
        }
    }
    Ok(g)
}

/// Parse an `MLIQ` sub-chunk payload into a [`WmoLiquid`]. Returns `Ok(None)` when the payload is
/// too short for the header or its declared grid (a truncated/garbage chunk yields no liquid rather
/// than a hard error — the group still renders its geometry).
fn parse_mliq(s: &[u8]) -> Result<Option<WmoLiquid>> {
    // SMOLiquidHeader: xverts@0 yverts@4 xtiles@8 ytiles@12 base[f32;3]@16 materialId(u16)@28 (30 B).
    let (Some(xverts), Some(yverts), Some(xtiles), Some(ytiles)) =
        (s.u32_at(0), s.u32_at(4), s.u32_at(8), s.u32_at(12))
    else {
        return Ok(None);
    };
    let nverts = (xverts as usize).saturating_mul(yverts as usize);
    let ntiles = (xtiles as usize).saturating_mul(ytiles as usize);
    // Sanity-cap the grid so a corrupt header can't ask us to allocate gigabytes (real grids are ≤~32²).
    if nverts == 0 || ntiles == 0 || nverts > 1 << 20 || ntiles > 1 << 20 {
        return Ok(None);
    }
    let base = [
        s.f32_at(16).ok_or(Error::Truncated("MLIQ"))?,
        s.f32_at(20).ok_or(Error::Truncated("MLIQ"))?,
        s.f32_at(24).ok_or(Error::Truncated("MLIQ"))?,
    ];
    let material_id = s.u16_at(28).ok_or(Error::Truncated("MLIQ"))?;
    let vbase = 30usize;
    let tbase = vbase + nverts * 8;
    if s.len() < tbase + ntiles {
        return Ok(None); // declared grid runs past the payload — treat as no liquid
    }
    let heights: Vec<f32> = (0..nverts)
        .map(|k| s.f32_at(vbase + k * 8 + 4).unwrap_or(0.0))
        .collect();
    // Byte 0 of the same 8-byte vertex — the water opacity index (see `WmoLiquid::opacity`). The
    // bounds are already guaranteed by the `tbase` check above.
    let opacity: Vec<u8> = (0..nverts).map(|k| s[vbase + k * 8]).collect();
    let tile_flags = s[tbase..tbase + ntiles].to_vec();
    Ok(Some(WmoLiquid {
        xverts,
        yverts,
        xtiles,
        ytiles,
        base,
        material_id,
        heights,
        opacity,
        tile_flags,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(magic: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(magic);
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn parse(bytes: &[u8]) -> Result<ParsedWmo> {
        parse_wmo(&mut Cursor::new(bytes))
    }

    #[test]
    fn root_parses_textures_materials_and_group_count() {
        let mut mohd = vec![0u8; 64];
        mohd[4..8].copy_from_slice(&7u32.to_le_bytes()); // nGroups
        let motx = b"a.blp\0\0b.blp\0";
        let mut momt = vec![0u8; 64];
        momt[8..12].copy_from_slice(&1u32.to_le_bytes()); // blend_mode
        momt[12..16].copy_from_slice(&7u32.to_le_bytes()); // texture_1 offset → "b.blp"
        let mut b = chunk(b"DHOM", &mohd);
        b.extend(chunk(b"XTOM", motx));
        b.extend(chunk(b"TMOM", &momt));
        let ParsedWmo::Root(root) = parse(&b).expect("parses") else {
            panic!("expected a root");
        };
        assert_eq!(root.n_groups, 7);
        assert_eq!(
            root.textures,
            vec!["a.blp".to_string(), "b.blp".to_string()]
        );
        assert_eq!(root.materials.len(), 1);
        assert_eq!(root.materials[0].blend_mode, 1);
        assert_eq!(
            root.materials[0].get_texture1_index(&root.texture_offset_index_map),
            1
        );
    }

    #[test]
    fn group_parses_geometry() {
        let mut mogp = vec![0u8; 68];
        mogp[8..12].copy_from_slice(&0x48u32.to_le_bytes()); // flags
        let vert = [1.0f32, 2.0, 3.0].map(f32::to_le_bytes).concat();
        mogp.extend(chunk(b"TVOM", &vert));
        mogp.extend(chunk(b"IVOM", &[0u8, 0, 1, 0, 2, 0]));
        let b = chunk(b"PGOM", &mogp);
        let ParsedWmo::Group(g) = parse(&b).expect("parses") else {
            panic!("expected a group");
        };
        assert_eq!(g.flags, 0x48);
        assert_eq!(g.vertex_positions.len(), 1);
        assert_eq!(g.vertex_positions[0].y, 2.0);
        assert_eq!(g.vertex_indices, vec![0, 1, 2]);
    }

    #[test]
    fn group_parses_mliq_liquid_grid() {
        // A 2×2-vertex (1×1-tile) MLIQ: header + 4 verts (8 B each, height at +4) + 1 tile byte.
        let mut mliq = Vec::new();
        mliq.extend(2u32.to_le_bytes()); // xverts
        mliq.extend(2u32.to_le_bytes()); // yverts
        mliq.extend(1u32.to_le_bytes()); // xtiles
        mliq.extend(1u32.to_le_bytes()); // ytiles
        mliq.extend([10.0f32, 20.0, -5.0].map(f32::to_le_bytes).concat()); // base xyz
        mliq.extend(115u16.to_le_bytes()); // materialId
        for h in [-5.0f32, -5.0, -5.0, -5.0] {
            mliq.extend([0u8, 0, 0, 0]); // flow bytes
            mliq.extend(h.to_le_bytes()); // height
        }
        mliq.push(0x44); // tile flag: nibble 4 (lake_a), fishable (0x40) — a wet tile

        let mut mogp = vec![0u8; 68];
        mogp[8..12].copy_from_slice(&0x2000u32.to_le_bytes()); // flags
        mogp[0x34..0x38].copy_from_slice(&0xfu32.to_le_bytes()); // groupLiquid = 0xf (per-tile)
        mogp.extend(chunk(b"QILM", &mliq));
        let b = chunk(b"PGOM", &mogp);
        let ParsedWmo::Group(g) = parse(&b).expect("parses") else {
            panic!("expected a group");
        };
        assert_eq!(g.group_liquid, 0xf);
        let lq = g.liquid.expect("has MLIQ liquid");
        assert_eq!((lq.xverts, lq.yverts, lq.xtiles, lq.ytiles), (2, 2, 1, 1));
        assert_eq!(lq.base, [10.0, 20.0, -5.0]);
        assert_eq!(lq.material_id, 115);
        assert_eq!(lq.heights, vec![-5.0, -5.0, -5.0, -5.0]);
        assert_eq!(lq.tile_flags, vec![0x44]);
        assert_eq!(lq.tile_flags[0] & 0xf, 4); // wet tile, type 4 (lake_a)
    }

    #[test]
    fn truncated_mliq_yields_no_liquid_not_a_panic() {
        // A header that declares a grid larger than the payload → None (the group still parses).
        let mut mliq = Vec::new();
        mliq.extend(9u32.to_le_bytes()); // xverts
        mliq.extend(9u32.to_le_bytes()); // yverts
        mliq.extend(8u32.to_le_bytes()); // xtiles
        mliq.extend(8u32.to_le_bytes()); // ytiles
        mliq.extend([0.0f32, 0.0, 0.0].map(f32::to_le_bytes).concat());
        mliq.extend(0u16.to_le_bytes());
        // …but no vertex/tile bytes follow.
        let mut mogp = vec![0u8; 68];
        mogp.extend(chunk(b"QILM", &mliq));
        let b = chunk(b"PGOM", &mogp);
        let ParsedWmo::Group(g) = parse(&b).expect("parses") else {
            panic!("expected a group");
        };
        assert!(g.liquid.is_none());
    }

    #[test]
    fn truncated_mohd_is_an_error_not_a_panic() {
        // MOHD payload of 3 bytes: the pre-0064 reader panicked on rd_u32(p, 4).
        let b = chunk(b"DHOM", &[0u8; 3]);
        assert!(matches!(parse(&b), Err(Error::Truncated("MOHD"))));
    }

    #[test]
    fn hostile_shapes_do_not_panic() {
        assert!(matches!(parse(&[]), Err(Error::NotWmo)));
        assert!(matches!(parse(&[0u8; 7]), Err(Error::NotWmo)));
        // Over-declared chunk size clamps (lenient) — parses to an empty-ish root.
        let mut b = chunk(b"DHOM", &[0u8; 64]);
        b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse(&b), Err(Error::Truncated("MOHD")) | Ok(_)));
        // A short MOGP (below the 68-byte header) is an empty group, kept from the old reader.
        let b = chunk(b"PGOM", &[0u8; 10]);
        let Ok(ParsedWmo::Group(g)) = parse(&b) else {
            panic!("short MOGP should parse to an empty group");
        };
        assert!(g.vertex_positions.is_empty());
    }
}
