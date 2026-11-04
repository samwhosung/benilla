//! A reader for 1.12.1's monolithic root ADTs: IFF records (a reversed magic, a `u32` size, the
//! payload) holding the asset names, the placements and 256 `MCNK` terrain chunks.
//!
//! The tile is read the way the reference reads it (`0x6c2010`): never by walking record sizes,
//! but through `MHDR`'s offsets to the top-level records and `MCIN`'s entries to the `MCNK`s.

use std::collections::HashMap;
use std::io::Cursor;

use benilla_bytes::ByteExt;

/// A terrain vertex normal: three signed bytes in WoW's X, Y, Z, the order of the file
/// (`CMapChunk::LoadNormals`, `0x6aff10`).
#[derive(Debug, Clone, Copy)]
pub struct VertexNormal {
    pub x: i8,
    pub y: i8,
    pub z: i8,
}

impl VertexNormal {
    /// `[x, y, z] / 127`, the bytes in file order (`0x810c08` is the 1/127).
    pub fn to_normalized(&self) -> [f32; 3] {
        [
            f32::from(self.x) / 127.0,
            f32::from(self.y) / 127.0,
            f32::from(self.z) / 127.0,
        ]
    }
}

/// MCVT: 145 heights (9×9 outer + 8×8 inner, stride-17 interleave).
#[derive(Debug, Clone)]
pub struct McvtChunk {
    pub heights: Vec<f32>,
}

/// MCNR: one normal per MCVT vertex.
#[derive(Debug, Clone)]
pub struct McnrChunk {
    pub normals: Vec<VertexNormal>,
}

/// MCLY per-layer flags.
#[derive(Debug, Clone, Copy)]
pub struct MclyFlags {
    pub value: u32,
}

impl MclyFlags {
    /// Bit 9 (0x200): the layer's MCAL alpha map is RLE-compressed.
    pub fn alpha_map_compressed(&self) -> bool {
        self.value & 0x200 != 0
    }
}

/// MCLY: one texture layer (16 bytes on disk).
#[derive(Debug, Clone)]
pub struct MclyLayer {
    pub texture_id: u32,
    pub flags: MclyFlags,
    pub offset_in_mcal: u32,
    pub effect_id: u32,
}

/// MCLY: the chunk's texture layers (0–4; layer 0 is the opaque base).
#[derive(Debug, Clone)]
pub struct MclyChunk {
    pub layers: Vec<MclyLayer>,
}

/// MCAL: raw alpha-map bytes (indexed per layer via [`MclyLayer::offset_in_mcal`]).
#[derive(Debug, Clone)]
pub struct McalChunk {
    pub data: Vec<u8>,
}

/// MCSH: a 64×64 1-bit baked shadow map (512 bytes).
#[derive(Debug, Clone)]
pub struct McshChunk {
    pub shadow_map: Vec<u8>,
}

impl McshChunk {
    /// Whether texel `(x, y)` (column, row; both 0..63) is shadowed.
    pub fn is_shadowed(&self, x: usize, y: usize) -> bool {
        if x >= 64 || y >= 64 {
            return false;
        }
        let byte = self.shadow_map.get(y * 8 + x / 8).copied().unwrap_or(0);
        (byte >> (x % 8)) & 1 != 0
    }
}

/// One MCLQ liquid vertex, 8 bytes: 4 union bytes and an absolute height. The union follows the
/// cell's liquid type: a depth byte and flow for water and ocean, a UV pair for magma and slime.
#[derive(Debug, Clone, Copy)]
pub struct LiquidVertex {
    pub union_data: [u8; 4],
    pub height: f32,
}

impl LiquidVertex {
    /// The water or ocean depth byte (union byte 0).
    pub fn depth_byte(&self) -> u8 {
        self.union_data[0]
    }

    /// A magma block's authored `(s, t)`, two little-endian `u16`s the reference's lava fill takes
    /// as its UV (`0x68d890`: `u = (s as i32 as f64 · 3/256) as f32`, same for `t`). The field is
    /// world-continuous, so lava tiles seamlessly across MCNK borders.
    pub fn texcoords(&self) -> [u16; 2] {
        [
            u16::from_le_bytes([self.union_data[0], self.union_data[1]]),
            u16::from_le_bytes([self.union_data[2], self.union_data[3]]),
        ]
    }
}

/// One MCLQ liquid block: a 9×9 absolute-height grid and an 8×8 cell-flag grid. An MCNK carries
/// one per set liquid header bit, back to back (`0x6af7a3`–`0x6af7cb`): two at the 28 shipped river
/// mouths. The liquid type is each cell's flag nibble (`0x68b21f`, `0x68d9b0`), not a header bit.
#[derive(Debug, Clone)]
pub struct MclqChunk {
    pub min_height: f32,
    pub max_height: f32,
    pub vertices: Vec<LiquidVertex>,
    /// Per-cell flags, row-major 8×8: the low nibble is the liquid type (`0xf` dry), `0x80`
    /// shared. `0x40`, fishable, is unread: the reference's only reader (`0x69b5d0`) has no
    /// caller, and the server decides fishability.
    pub tile_flags: [u8; 64],
}

/// MCNK header flag bit 1: a chunk a mover may not enter, the bands along mountain rims such as
/// Searing Gorge's. The reference's header walk copies it to its chunk flag `0x40` (`0x6af5f0`).
pub const MCNK_IMPASSABLE: u32 = 0x2;

/// MCNK header flag bit 15: the chunk's alpha maps are stored whole, 64 × 64. Clear, a map holds 63
/// useful values per 64-value row and the decoder copies column 62 into 63 and row 62 into 63
/// (`0x6b0a60` picks `0x6b0ac0` on the bit, else `0x6b0bf0`). That holds at `shadowLevel` 0; with
/// the global `0xc63284` non-zero the dispatcher takes the 32 × 32 decoders `0x6b0b30` and
/// `0x6b0c80` instead (`cmp eax,2` at `0x6b0a72` is always 2, from its one caller `0x6b02d9`).
pub const MCNK_ALPHA_64: u32 = 0x8000;

/// The MCNK header fields the renderer reads. The names mislead: `pred_tex` and `no_effect_doodad`
/// are the two halves of the predominant-texture map at `+0x40`, and `unknown_8bytes` is the real
/// noEffectDoodad at `+0x50`; the consumer reassembles them.
#[derive(Debug, Clone)]
pub struct McnkHeader {
    pub flags: u32,
    pub index_x: u32,
    pub index_y: u32,
    /// `AreaTable.dbc` id of the chunk's zone or subzone (`+0x34`).
    pub area_id: u32,
    pub holes_low_res: u16,
    pub pred_tex: [u8; 8],
    pub no_effect_doodad: [u8; 8],
    pub unknown_8bytes: [u8; 8],
    pub position: [f32; 3],
}

impl McnkHeader {
    /// Whether the raw header sets [`MCNK_IMPASSABLE`], which covers the whole 33.333 yd chunk.
    pub fn impassable(&self) -> bool {
        self.flags & MCNK_IMPASSABLE != 0
    }

    /// Whether the raw header sets [`MCNK_ALPHA_64`]: the alpha maps need no edge copy.
    pub fn alpha_maps_whole(&self) -> bool {
        self.flags & MCNK_ALPHA_64 != 0
    }
}

/// One MCNK terrain chunk.
#[derive(Debug, Clone)]
pub struct McnkChunk {
    pub header: McnkHeader,
    pub heights: Option<McvtChunk>,
    pub normals: Option<McnrChunk>,
    pub layers: Option<MclyChunk>,
    pub alpha: Option<McalChunk>,
    pub shadow: Option<McshChunk>,
    /// The MCLQ liquid blocks in disk order: none on a dry chunk, two at a river mouth.
    pub liquids: Vec<MclqChunk>,
    /// MCRF's first part: indices into the tile's MDDF, the doodads that overlap this chunk. The
    /// count is the header's `+0x10`, not the record's size (`0x6af68e`).
    pub doodad_refs: Vec<u32>,
    /// MCRF's second part, after the doodad refs: indices into MODF, the count the header's
    /// `+0x38`.
    pub object_refs: Vec<u32>,
}

/// MDDF: a placed M2 doodad (36 bytes).
#[derive(Debug, Clone)]
pub struct DoodadPlacement {
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: u16,
    pub flags: u16,
}

/// MODF: a placed WMO (64 bytes).
#[derive(Debug, Clone)]
pub struct WmoPlacement {
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub flags: u16,
    pub doodad_set: u16,
    /// `WMOAreaTable.NameSetID`: which name and audio variant of the WMO this placement is
    /// (Northshire Abbey and Tyr's Hand Abbey are one WMO).
    pub name_set: u16,
}

/// A parsed vanilla (monolithic) root ADT.
#[derive(Debug, Clone)]
pub struct RootAdt {
    pub textures: Vec<String>,
    pub models: Vec<String>,
    pub wmos: Vec<String>,
    /// The MDDF entries some chunk's MCRF references, in MDDF order, one per unique id. The
    /// reference makes a doodad only from a chunk's ref (`0x6b00f0` → `0x694bc0`), and nothing else
    /// reads MDDF, so an unreferenced entry never exists. Its dedup is global, in a hash by unique
    /// id (`0xca7dc0`, `0x694c08`): the first def made wins, in chunk load order. Here the dedup is
    /// per tile, first reference in MCIN order; the streamer's by-uid refcount does the rest.
    pub doodad_placements: Vec<DoodadPlacement>,
    /// The MODF entries some chunk's MCRF references, by the same rule (`0x6b00f0` → `0x695650`,
    /// hash `0xca7d98`, `0x695698`).
    pub wmo_placements: Vec<WmoPlacement>,
    /// The tile's chunks in MCIN order (row · 16 + column). An entry the file cannot hold an MCNK
    /// at, offset 0 included, is skipped: the reference does not check and would read garbage.
    pub mcnk_chunks: Vec<McnkChunk>,
}

/// A parsed ADT; vanilla has only the monolithic root.
#[derive(Debug, Clone)]
pub enum ParsedAdt {
    Root(Box<RootAdt>),
}

/// ADT parse error.
#[derive(Debug)]
pub enum Error {
    Truncated(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Truncated(w) => write!(f, "truncated ADT: {w}"),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// The NUL-terminated string at `offset`: an MMDX or MWMO name by its MMID or MWID offset.
fn cstring_at(blob: &[u8], offset: usize) -> String {
    let end = blob[offset.min(blob.len())..]
        .iter()
        .position(|&c| c == 0)
        .map(|p| offset + p)
        .unwrap_or(blob.len());
    String::from_utf8_lossy(&blob[offset.min(blob.len())..end]).into_owned()
}

/// The data of the top-level record `offset` bytes from MHDR's data (`0x6c2010` forms each table
/// as `lea [offset + mhdr + 8]`), cut to the record's own size and the file's end: the reference
/// reads MTEX's, MDDF's and MODF's size and indexes the rest unbounded. A record that does not fit
/// the file, or an offset of 0, is empty.
fn mhdr_record(b: &[u8], mhdr: usize, offset: u32) -> &[u8] {
    if offset == 0 {
        return &[];
    }
    let record = mhdr + offset as usize;
    let (Some(size), Some(rest)) = (b.u32_at(record + 4), b.get(record + 8..)) else {
        return &[];
    };
    &rest[..(size as usize).min(rest.len())]
}

/// Parse a vanilla monolithic ADT; the cursor's position is ignored.
///
/// The reference finds the records without a size walk (`0x6c2010`): MHDR's data starts at
/// `16 + the version record's size` (`0x6c2021`), its offsets from there name MCIN, MTEX, MMDX,
/// MMID, MWMO, MWID, MDDF and MODF (`+0x04` to `+0x20`), and no tag is read. Chunk row · 16 +
/// column is the MCNK at its MCIN entry's file offset (`0x6c2880`, `0x6c299d`); the entry's size is
/// never read and its third dword is the client's own.
pub fn parse_adt(cursor: &mut Cursor<&[u8]>) -> Result<ParsedAdt> {
    let b: &[u8] = cursor.get_ref();

    let version_size = b.u32_at(4).ok_or(Error::Truncated("MVER"))? as usize;
    let mhdr = 16 + version_size;
    let field = |at: usize| b.u32_at(mhdr + at).ok_or(Error::Truncated("MHDR"));
    let (mcin, mtex, mmdx, mmid) = (field(0x04)?, field(0x08)?, field(0x0c)?, field(0x10)?);
    let (mwmo, mwid, mddf, modf) = (field(0x14)?, field(0x18)?, field(0x1c)?, field(0x20)?);
    let table = |offset| mhdr_record(b, mhdr, offset);

    let textures = split_strings(table(mtex));
    let mmdx = table(mmdx);
    let mmid = read_u32_list(table(mmid), "MMID")?;
    let mwmo = table(mwmo);
    let mwid = read_u32_list(table(mwid), "MWID")?;
    let doodad_entries = read_mddf(table(mddf))?;
    let wmo_entries = read_modf(table(modf))?;

    // MCIN's data, 256 sixteen-byte entries; its own size is not read.
    let mcin_data = (mcin != 0)
        .then(|| b.get(mhdr + mcin as usize + 8..))
        .flatten()
        .unwrap_or(&[]);
    let mut mcnk_chunks = Vec::new();
    for entry in mcin_data.as_chunks::<16>().0.iter().take(256) {
        // The entry's first dword is the MCNK record's file offset (`0x6c299a`). The reference
        // does not check it (an offset of 0 would read the file's start as an MCNK); a malformed
        // entry that no MCNK header fits at is skipped here, the rest of the tile kept.
        let record = entry.u32_at(0).ok_or(Error::Truncated("MCIN"))? as usize;
        if record == 0 {
            continue;
        }
        if let Some(chunk) = b.get(record + 8..).and_then(|d| read_mcnk(d).ok()) {
            mcnk_chunks.push(chunk);
        }
    }

    // MDDF/MODF `name_id` indexes the MMID/MWID offset tables.
    let models = mmid.iter().map(|&o| cstring_at(mmdx, o as usize)).collect();
    let wmos = mwid.iter().map(|&o| cstring_at(mwmo, o as usize)).collect();

    // The placements are the ones a chunk refers to (`0x6b00f0`), one def per unique id.
    let doodad_placements = referenced(
        &doodad_entries,
        |p| p.unique_id,
        &mcnk_chunks,
        |c| &c.doodad_refs,
    );
    let wmo_placements = referenced(
        &wmo_entries,
        |p| p.unique_id,
        &mcnk_chunks,
        |c| &c.object_refs,
    );

    Ok(ParsedAdt::Root(Box::new(RootAdt {
        textures,
        models,
        wmos,
        doodad_placements,
        wmo_placements,
        mcnk_chunks,
    })))
}

/// The `entries` some chunk's `refs` name (an index out of range names none), in entry order, one
/// per unique id: the first reference in MCIN order wins. The client's table is global and its
/// first def made wins, in chunk load order (`0x694bc0`, `0x695650`); with this per-tile pick, the
/// streamer's by-uid refcount gives the same.
fn referenced<T: Clone>(
    entries: &[T],
    unique_id: impl Fn(&T) -> u32,
    chunks: &[McnkChunk],
    refs: impl Fn(&McnkChunk) -> &[u32],
) -> Vec<T> {
    let mut first: HashMap<u32, usize> = HashMap::new();
    for &i in chunks.iter().flat_map(&refs) {
        if let Some(e) = entries.get(i as usize) {
            first.entry(unique_id(e)).or_insert(i as usize);
        }
    }
    let mut picked: Vec<usize> = first.into_values().collect();
    picked.sort_unstable();
    picked.into_iter().map(|i| entries[i].clone()).collect()
}

/// Split a NUL-separated, NUL-terminated string blob, dropping empties.
fn split_strings(blob: &[u8]) -> Vec<String> {
    blob.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

fn read_u32_list(data: &[u8], name: &'static str) -> Result<Vec<u32>> {
    data.as_chunks::<4>()
        .0
        .iter()
        .map(|c| c.u32_at(0).ok_or(Error::Truncated(name)))
        .collect()
}

fn read_mddf(data: &[u8]) -> Result<Vec<DoodadPlacement>> {
    let mut out = Vec::new();
    for r in data.as_chunks::<36>().0 {
        out.push(DoodadPlacement {
            name_id: r.u32_at(0).ok_or(Error::Truncated("MDDF"))?,
            unique_id: r.u32_at(4).ok_or(Error::Truncated("MDDF"))?,
            position: [
                r.f32_at(8).ok_or(Error::Truncated("MDDF"))?,
                r.f32_at(12).ok_or(Error::Truncated("MDDF"))?,
                r.f32_at(16).ok_or(Error::Truncated("MDDF"))?,
            ],
            rotation: [
                r.f32_at(20).ok_or(Error::Truncated("MDDF"))?,
                r.f32_at(24).ok_or(Error::Truncated("MDDF"))?,
                r.f32_at(28).ok_or(Error::Truncated("MDDF"))?,
            ],
            scale: r.u16_at(32).ok_or(Error::Truncated("MDDF"))?,
            flags: r.u16_at(34).ok_or(Error::Truncated("MDDF"))?,
        });
    }
    Ok(out)
}

fn read_modf(data: &[u8]) -> Result<Vec<WmoPlacement>> {
    let mut out = Vec::new();
    for r in data.as_chunks::<64>().0 {
        out.push(WmoPlacement {
            name_id: r.u32_at(0).ok_or(Error::Truncated("MODF"))?,
            unique_id: r.u32_at(4).ok_or(Error::Truncated("MODF"))?,
            position: [
                r.f32_at(8).ok_or(Error::Truncated("MODF"))?,
                r.f32_at(12).ok_or(Error::Truncated("MODF"))?,
                r.f32_at(16).ok_or(Error::Truncated("MODF"))?,
            ],
            rotation: [
                r.f32_at(20).ok_or(Error::Truncated("MODF"))?,
                r.f32_at(24).ok_or(Error::Truncated("MODF"))?,
                r.f32_at(28).ok_or(Error::Truncated("MODF"))?,
            ],
            // 0x20..0x38: the bounding box (6 f32), unread.
            flags: r.u16_at(56).ok_or(Error::Truncated("MODF"))?,
            doodad_set: r.u16_at(58).ok_or(Error::Truncated("MODF"))?,
            name_set: r.u16_at(60).ok_or(Error::Truncated("MODF"))?,
            // 0x3E: a u16, unread.
        });
    }
    Ok(out)
}

/// Parse one MCNK: the 128-byte header, then the sub-chunks at the header's offsets.
fn read_mcnk(data: &[u8]) -> Result<McnkChunk> {
    if data.len() < 128 {
        return Err(Error::Truncated("MCNK header"));
    }
    let h = &data[..128];
    let header = McnkHeader {
        flags: h.u32_at(0x00).ok_or(Error::Truncated("MCNK header"))?,
        index_x: h.u32_at(0x04).ok_or(Error::Truncated("MCNK header"))?,
        index_y: h.u32_at(0x08).ok_or(Error::Truncated("MCNK header"))?,
        area_id: h.u32_at(0x34).ok_or(Error::Truncated("MCNK header"))?,
        holes_low_res: h.u16_at(0x3C).ok_or(Error::Truncated("MCNK header"))?,
        pred_tex: h
            .bytes_at(0x40, 8)
            .ok_or(Error::Truncated("MCNK header"))?
            .try_into()
            .unwrap(),
        no_effect_doodad: h
            .bytes_at(0x48, 8)
            .ok_or(Error::Truncated("MCNK header"))?
            .try_into()
            .unwrap(),
        unknown_8bytes: h
            .bytes_at(0x50, 8)
            .ok_or(Error::Truncated("MCNK header"))?
            .try_into()
            .unwrap(),
        position: [
            h.f32_at(0x68).ok_or(Error::Truncated("MCNK header"))?,
            h.f32_at(0x6C).ok_or(Error::Truncated("MCNK header"))?,
            h.f32_at(0x70).ok_or(Error::Truncated("MCNK header"))?,
        ],
    };

    let mut chunk = McnkChunk {
        header,
        heights: None,
        normals: None,
        layers: None,
        alpha: None,
        shadow: None,
        liquids: Vec::new(),
        doodad_refs: Vec::new(),
        object_refs: Vec::new(),
    };

    // Sub-chunks sit at the header's offsets, not in sequence (AhnQiraj's are not contiguous).
    // An offset may count from the MCNK magic or from its data, so the reversed magic is checked
    // at both; yields the payload's `(start, size)`.
    let locate = |ofs: u32, magic: &[u8; 4]| -> Option<(usize, usize)> {
        if ofs == 0 {
            return None;
        }
        let ofs = ofs as usize;
        for hdr in [Some(ofs), ofs.checked_sub(8)].into_iter().flatten() {
            if data.bytes_at(hdr, 4) == Some(magic.as_slice()) {
                let size = data.u32_at(hdr + 4)?;
                return Some((hdr + 8, size as usize));
            }
        }
        None
    };
    // `locate` proves `start` in bounds; `len` is unvalidated, so the end saturates, then clamps.
    let slice = |start: usize, len: usize| &data[start..start.saturating_add(len).min(data.len())];

    if let Some((p, n)) = locate(
        h.u32_at(0x14).ok_or(Error::Truncated("MCNK header"))?,
        b"TVCM",
    )
    .filter(|&(_, n)| n > 0)
    {
        let sub = slice(p, n);
        let mut heights = Vec::new();
        for i in 0..145.min(sub.len() / 4) {
            heights.push(sub.f32_at(i * 4).ok_or(Error::Truncated("MCVT"))?);
        }
        chunk.heights = Some(McvtChunk { heights });
    }
    if let Some((p, n)) = locate(
        h.u32_at(0x18).ok_or(Error::Truncated("MCNK header"))?,
        b"RNCM",
    )
    .filter(|&(_, n)| n > 0)
    {
        let sub = slice(p, n);
        let mut normals = Vec::new();
        for i in 0..145.min(sub.len() / 3) {
            normals.push(VertexNormal {
                x: sub.u8_at(i * 3).ok_or(Error::Truncated("MCNR"))? as i8,
                y: sub.u8_at(i * 3 + 1).ok_or(Error::Truncated("MCNR"))? as i8,
                z: sub.u8_at(i * 3 + 2).ok_or(Error::Truncated("MCNR"))? as i8,
            });
        }
        chunk.normals = Some(McnrChunk { normals });
    }
    if let Some((p, n)) = locate(
        h.u32_at(0x1C).ok_or(Error::Truncated("MCNK header"))?,
        b"YLCM",
    )
    .filter(|&(_, n)| n > 0)
    {
        let mut layers = Vec::new();
        for l in slice(p, n).as_chunks::<16>().0 {
            layers.push(MclyLayer {
                texture_id: l.u32_at(0).ok_or(Error::Truncated("MCLY"))?,
                flags: MclyFlags {
                    value: l.u32_at(4).ok_or(Error::Truncated("MCLY"))?,
                },
                offset_in_mcal: l.u32_at(8).ok_or(Error::Truncated("MCLY"))?,
                effect_id: l.u32_at(12).ok_or(Error::Truncated("MCLY"))?,
            });
        }
        chunk.layers = Some(MclyChunk { layers });
    }
    // MCAL's length is the header's `size_alpha`, not the sub-chunk's own size: they differ on some
    // files (AhnQiraj).
    if let Some((p, _)) = locate(
        h.u32_at(0x24).ok_or(Error::Truncated("MCNK header"))?,
        b"LACM",
    ) {
        let size_alpha = h.u32_at(0x28).ok_or(Error::Truncated("MCNK header"))? as usize;
        chunk.alpha = Some(McalChunk {
            data: slice(p, size_alpha).to_vec(),
        });
    }
    if let Some((p, n)) = locate(
        h.u32_at(0x2C).ok_or(Error::Truncated("MCNK header"))?,
        b"HSCM",
    )
    .filter(|&(_, n)| n > 0)
    {
        let sub = slice(p, n);
        let mut shadow_map = vec![0u8; 512];
        let k = sub.len().min(512);
        shadow_map[..k].copy_from_slice(&sub[..k]);
        chunk.shadow = Some(McshChunk { shadow_map });
    }
    // MCLQ's length is the header's `size_liquid` too: its own declared size is always 0.
    if let Some((p, _)) = locate(
        h.u32_at(0x60).ok_or(Error::Truncated("MCNK header"))?,
        b"QLCM",
    ) {
        let size_liquid = h.u32_at(0x64).ok_or(Error::Truncated("MCNK header"))? as usize;
        chunk.liquids = read_mclq_blocks(slice(p, size_liquid), chunk.header.flags);
    }

    // MCRF: `+0x10` doodad refs then `+0x38` object refs, one dword each, from the offset at `+0x20`
    // (`0x6af9a7` reads `+0x20` into `+0xf28`; `0x6af68b`, `0x6af68e`). The counts are the header's; the record's own size is not read.
    let doodad_count = h.u32_at(0x10).ok_or(Error::Truncated("MCNK header"))? as usize;
    let object_count = h.u32_at(0x38).ok_or(Error::Truncated("MCNK header"))? as usize;
    if doodad_count + object_count > 0 {
        if let Some((p, _)) = locate(
            h.u32_at(0x20).ok_or(Error::Truncated("MCNK header"))?,
            b"FRCM",
        ) {
            let refs = data[p..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|r| u32::from_le_bytes(*r));
            let mut refs = refs.take(doodad_count + object_count);
            chunk.doodad_refs = refs.by_ref().take(doodad_count).collect();
            chunk.object_refs = refs.collect();
        }
    }

    Ok(chunk)
}

/// Bytes per MCLQ block: `{f32 min, f32 max, 81×8B verts, 64B cell flags, u32 nFlow, 2×40B flow}`.
const MCLQ_BLOCK: usize = 0x324;
/// Liquid header bits 2–5: how many are set is how many MCLQ blocks there are.
const MCNK_LIQUID_BITS: u32 = 0x3c;

/// Read the packed MCLQ blocks, stopping at the first one the payload cannot hold. A chunk with an
/// MCLQ sub-chunk but no liquid bit set still yields one block; the reference creates one per set
/// bit only (`0x6af760`).
fn read_mclq_blocks(sub: &[u8], mcnk_flags: u32) -> Vec<MclqChunk> {
    let want = (mcnk_flags & MCNK_LIQUID_BITS).count_ones().max(1) as usize;
    let mut out = Vec::with_capacity(want);
    for i in 0..want {
        let Some(rest) = sub.get(i * MCLQ_BLOCK..) else {
            break;
        };
        match read_mclq_block(rest) {
            Some(b) => out.push(b),
            None => break,
        }
    }
    out
}

/// Too-short MCLQ data is no liquid, not an error. The minimum stops short of [`MCLQ_BLOCK`]: the
/// trailing flow data is unread.
fn read_mclq_block(sub: &[u8]) -> Option<MclqChunk> {
    const NEED: usize = 8 + 81 * 8 + 64;
    if sub.len() < NEED {
        return None;
    }
    let min_height = sub.f32_at(0)?;
    let max_height = sub.f32_at(4)?;
    let mut vertices = Vec::with_capacity(81);
    for i in 0..81 {
        let o = 8 + i * 8;
        vertices.push(LiquidVertex {
            union_data: sub.bytes_at(o, 4)?.try_into().ok()?,
            height: sub.f32_at(o + 4)?,
        });
    }
    let tile_flags = sub.bytes_at(8 + 81 * 8, 64)?.try_into().ok()?;
    Some(MclqChunk {
        min_height,
        max_height,
        vertices,
        tile_flags,
    })
}

mod combined_alpha;
pub use combined_alpha::CombinedAlphaMap;

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one IFF chunk: reversed magic + `u32` size + payload.
    fn chunk(magic: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(magic);
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// An MCNK payload: the 128-byte header with `flags`, MCVT (heights 0, 1, 2, …) and MCRF, the
    /// sub-record offsets counted from the MCNK's magic as the files do, so 8 more than in the payload.
    fn mcnk(flags: u32, doodad_refs: &[u32], object_refs: &[u32]) -> Vec<u8> {
        let heights: Vec<u8> = (0..145u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
        let mcvt = chunk(b"TVCM", &heights);
        let refs: Vec<u8> = doodad_refs
            .iter()
            .chain(object_refs)
            .flat_map(|r| r.to_le_bytes())
            .collect();
        let mcrf = chunk(b"FRCM", &refs);
        let mut h = vec![0u8; 128];
        let mut put = |at: usize, v: u32| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(0x00, flags);
        put(0x10, doodad_refs.len() as u32);
        put(0x14, 8 + 128);
        put(0x20, 8 + 128 + mcvt.len() as u32);
        put(0x38, object_refs.len() as u32);
        [h, mcvt, mcrf].concat()
    }

    /// A tile as the reference lays it out: MVER, MHDR (64 bytes), MCIN, the tables, then the MCNK
    /// records, MHDR's offsets and MCIN's entries filled in. An MCIN entry's size and third dword
    /// are junk, as the reference reads neither.
    #[derive(Default)]
    struct Tile {
        mtex: Vec<u8>,
        mmdx: Vec<u8>,
        mmid: Vec<u8>,
        mwmo: Vec<u8>,
        mwid: Vec<u8>,
        mddf: Vec<u8>,
        modf: Vec<u8>,
        /// `(MCIN index, payload)` in file order.
        mcnks: Vec<(usize, Vec<u8>)>,
        /// Noggit's convention: an MCNK's size counts its own 8-byte header.
        mcnk_size_counts_header: bool,
    }

    impl Tile {
        fn bytes(&self) -> Vec<u8> {
            let mut out = chunk(b"REVM", &18u32.to_le_bytes());
            out.extend(chunk(b"RDHM", &[0u8; 64]));
            let mhdr_data = out.len() - 64;
            let mut offsets = [0u32; 8]; // MCIN, MTEX, MMDX, MMID, MWMO, MWID, MDDF, MODF
            let mcin_at = out.len() + 8;
            let tables = [
                (b"NICM", vec![0u8; 4096]),
                (b"XETM", self.mtex.clone()),
                (b"XDMM", self.mmdx.clone()),
                (b"DIMM", self.mmid.clone()),
                (b"OMWM", self.mwmo.clone()),
                (b"DIWM", self.mwid.clone()),
                (b"FDDM", self.mddf.clone()),
                (b"FDOM", self.modf.clone()),
            ];
            for (n, (magic, payload)) in tables.iter().enumerate() {
                offsets[n] = (out.len() - mhdr_data) as u32;
                out.extend(chunk(magic, payload));
            }
            for (n, o) in offsets.iter().enumerate() {
                let at = mhdr_data + 4 + n * 4;
                out[at..at + 4].copy_from_slice(&o.to_le_bytes());
            }
            for (index, payload) in &self.mcnks {
                let at = mcin_at + index * 16;
                let record_at = out.len() as u32;
                out[at..at + 4].copy_from_slice(&record_at.to_le_bytes());
                out[at + 4..at + 16].fill(0xff);
                let mut record = chunk(b"KNCM", payload);
                if self.mcnk_size_counts_header {
                    record[4..8].copy_from_slice(&(payload.len() as u32 + 8).to_le_bytes());
                }
                out.extend(record);
            }
            out
        }
    }

    /// A tile of one MCNK, `payload`, at MCIN index 0.
    fn one_mcnk(payload: Vec<u8>) -> Vec<u8> {
        Tile {
            mcnks: vec![(0, payload)],
            ..Tile::default()
        }
        .bytes()
    }

    fn parse(bytes: &[u8]) -> Result<ParsedAdt> {
        parse_adt(&mut Cursor::new(bytes))
    }

    #[test]
    fn root_parses_a_minimal_well_formed_tile() {
        let mut mddf = vec![0u8; 36];
        mddf[32..34].copy_from_slice(&7u16.to_le_bytes()); // scale
        let mut modf = vec![0u8; 64];
        modf[58..60].copy_from_slice(&3u16.to_le_bytes()); // doodad_set

        let b = Tile {
            mtex: b"tex1.blp\0tex2.blp\0".to_vec(),
            mmdx: b"model.m2\0".to_vec(),
            mmid: 0u32.to_le_bytes().to_vec(),
            mwmo: b"world.wmo\0".to_vec(),
            mwid: 0u32.to_le_bytes().to_vec(),
            mddf,
            modf,
            mcnks: vec![(0, mcnk(0, &[0], &[0]))],
            ..Tile::default()
        }
        .bytes();

        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert_eq!(root.textures, vec!["tex1.blp", "tex2.blp"]);
        assert_eq!(root.models, vec!["model.m2"]);
        assert_eq!(root.wmos, vec!["world.wmo"]);
        assert_eq!(root.doodad_placements.len(), 1);
        assert_eq!(root.doodad_placements[0].scale, 7);
        assert_eq!(root.wmo_placements.len(), 1);
        assert_eq!(root.wmo_placements[0].doodad_set, 3);
        assert_eq!(root.mcnk_chunks.len(), 1);
        let heights = &root.mcnk_chunks[0]
            .heights
            .as_ref()
            .expect("MCVT parsed")
            .heights;
        assert_eq!(heights.len(), 145);
        assert_eq!(heights[1], 1.0);
    }

    /// The reference never reads an MCNK's size (`0x6c2880` forms the record from its MCIN entry's
    /// offset alone, `0x6c299d`), so Noggit's sizes, which count the record's header, lose nothing.
    #[test]
    fn an_mcnk_size_that_counts_its_header_loses_no_chunk() {
        let b = Tile {
            mcnks: vec![(0, mcnk(0, &[], &[])), (1, mcnk(0, &[], &[]))],
            mcnk_size_counts_header: true,
            ..Tile::default()
        }
        .bytes();
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert_eq!(root.mcnk_chunks.len(), 2);
        assert_eq!(
            root.mcnk_chunks[1].heights.as_ref().unwrap().heights.len(),
            145
        );
    }

    /// Chunks come from MCIN's entries, row · 16 + column, wherever their records lie: out of order,
    /// or with bytes between that no size accounts for.
    #[test]
    fn mcnks_are_found_by_their_mcin_entries_not_by_walking_sizes() {
        let at = |x: u32| {
            let mut m = mcnk(0, &[], &[]);
            m[4..8].copy_from_slice(&x.to_le_bytes()); // the header's own index_x
            m
        };
        let tile = Tile {
            mcnks: vec![(255, at(255)), (0, at(0)), (17, at(17))],
            ..Tile::default()
        };
        let ParsedAdt::Root(root) = parse(&tile.bytes()).expect("parses");
        let order: Vec<u32> = root.mcnk_chunks.iter().map(|c| c.header.index_x).collect();
        assert_eq!(order, [0, 17, 255], "MCIN order");

        // Five bytes before the last record; its MCIN entry moves with it.
        let mut b = tile.bytes();
        let last = b.windows(4).rposition(|w| w == b"KNCM").unwrap();
        b.splice(last..last, [0xaa; 5]);
        let entry = 20 + 64 + 8 + 17 * 16;
        b[entry..entry + 4].copy_from_slice(&(last as u32 + 5).to_le_bytes());
        let ParsedAdt::Root(root) = parse(&b).expect("parses with stray bytes");
        assert_eq!(root.mcnk_chunks.len(), 3);
        assert_eq!(root.mcnk_chunks[1].header.index_x, 17);
    }

    /// The reference creates a doodad or an object only from a chunk's MCRF (`0x6b00f0`), reading
    /// the counts from the MCNK header (`0x6af68b`) and making one def per unique id (`0x694bc0`,
    /// `0x695650`); nothing else reads MDDF or MODF.
    #[test]
    fn placements_are_the_ones_the_chunks_refer_to() {
        let entry = |size: usize, uid: u32| {
            let mut e = vec![0u8; size];
            e[4..8].copy_from_slice(&uid.to_le_bytes());
            e
        };
        // Unique ids 7, 8, 9 and 7 again.
        let mddf: Vec<u8> = [7, 8, 9, 7].iter().flat_map(|&u| entry(36, u)).collect();
        let modf: Vec<u8> = [7, 8, 9, 7].iter().flat_map(|&u| entry(64, u)).collect();

        // Chunk 0 names doodad 0 and object 1; chunk 1 names 3 (a second 7), 2, and one out of
        // range. Doodad 1 (id 8) is named by no chunk: chunk 0's second ref is past its header count.
        let mut c0 = mcnk(0, &[0, 1], &[0, 1]);
        c0[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
        c0[0x38..0x3c].copy_from_slice(&1u32.to_le_bytes());
        // With a count of 1 each, the object ref is the second dword, which held 1.
        let b = Tile {
            mddf,
            modf,
            mcnks: vec![(0, c0), (1, mcnk(0, &[3, 2, 99], &[3, 2, 99]))],
            ..Tile::default()
        }
        .bytes();
        let ParsedAdt::Root(root) = parse(&b).expect("parses");

        assert_eq!(root.mcnk_chunks[0].doodad_refs, [0]);
        assert_eq!(
            root.mcnk_chunks[0].object_refs,
            [1],
            "after the doodad refs"
        );
        let doodads: Vec<u32> = root.doodad_placements.iter().map(|d| d.unique_id).collect();
        assert_eq!(
            doodads,
            [7, 9],
            "8 is unreferenced and the second 7 is the same def"
        );
        // Chunk 0 names MODF entry 1 (id 8); chunk 1 names 3 (id 7, the first ref to it) and 2.
        let wmos: Vec<u32> = root.wmo_placements.iter().map(|w| w.unique_id).collect();
        assert_eq!(wmos, [8, 9, 7], "in MODF order");
    }

    /// Flag bit 15 is read off the header; `0x6b0a60` picks the 64 × 64 decoder on it.
    #[test]
    fn mcnk_flag_bit_15_marks_whole_alpha_maps() {
        let h = |flags| {
            let ParsedAdt::Root(root) = parse(&one_mcnk(mcnk(flags, &[], &[]))).unwrap();
            root.mcnk_chunks[0].header.clone()
        };
        assert!(h(0x8000).alpha_maps_whole());
        assert!(!h(0x7fff).alpha_maps_whole());
    }

    #[test]
    fn partial_trailing_mddf_record_is_dropped_not_a_panic() {
        // Shorter than the 36-byte stride: `as_chunks` drops the remainder.
        let b = Tile {
            mddf: vec![0u8; 10],
            ..Tile::default()
        }
        .bytes();
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert!(root.doodad_placements.is_empty());
    }

    #[test]
    fn an_mcin_entry_no_mcnk_header_fits_at_is_skipped_not_fatal() {
        // The only entry points at 40 bytes; the tile parses and holds no chunk.
        let b = one_mcnk(vec![0u8; 40]);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert!(root.mcnk_chunks.is_empty());

        // An entry past the file's end, beside a good chunk, loses only itself.
        let mut b = one_mcnk(mcnk(0, &[], &[]));
        let entry = 20 + 64 + 8 + 16;
        b[entry..entry + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert_eq!(root.mcnk_chunks.len(), 1);
    }

    #[test]
    fn corrupt_mcnk_subchunk_offset_skips_cleanly_never_panics() {
        // ofs = 3: the minus-8 candidate underflows and 3 holds no magic.
        let mut data = vec![0u8; 128];
        data[0x14..0x18].copy_from_slice(&3u32.to_le_bytes());
        let b = one_mcnk(data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses despite corrupt MCVT offset");
        assert!(root.mcnk_chunks[0].heights.is_none());

        // ofs points past the end of the buffer entirely.
        let mut data = vec![0u8; 128];
        data[0x14..0x18].copy_from_slice(&u32::MAX.to_le_bytes());
        let b = one_mcnk(data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses despite out-of-range MCVT offset");
        assert!(root.mcnk_chunks[0].heights.is_none());
    }

    #[test]
    fn short_mclq_is_treated_as_no_liquid_not_an_error() {
        let mut data = vec![0u8; 128];
        data[0x60..0x64].copy_from_slice(&128u32.to_le_bytes()); // MCLQ offset
        data[0x64..0x68].copy_from_slice(&4u32.to_le_bytes()); // size_liquid, far short of NEED
        data.extend(chunk(b"QLCM", &[0u8; 4]));
        let b = one_mcnk(data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert!(root.mcnk_chunks[0].liquids.is_empty());
    }

    /// `CMapChunk::LoadNormals` (`0x6aff10`) stores each MCNR triple's three signed bytes as the
    /// vector's components in file order, each times 1/127 (`0x810c08`): the bytes are X, Y, Z.
    #[test]
    fn mcnr_normals_are_x_y_z_in_file_order() {
        let triple = |i: usize| [i as u8, (200 + i) as u8, 127];
        let mcnr: Vec<u8> = (0..145).flat_map(triple).collect();
        let mut data = vec![0u8; 128];
        // The MCNR offset counts from the MCNK's magic, so the 8-byte chunk header is in it.
        data[0x18..0x1c].copy_from_slice(&(8u32 + 128).to_le_bytes());
        data.extend(chunk(b"RNCM", &mcnr));
        let b = one_mcnk(data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");

        let normals = &root.mcnk_chunks[0].normals.as_ref().expect("MCNR").normals;
        assert_eq!(normals.len(), 145);
        for (i, n) in normals.iter().enumerate() {
            let [x, y, z] = triple(i).map(|b| b as i8);
            assert_eq!((n.x, n.y, n.z), (x, y, z), "normal {i}");
            assert_eq!(
                n.to_normalized(),
                [x, y, z].map(|c| f32::from(c) / 127.0),
                "normal {i}"
            );
        }
        // Flat ground: the bytes 0, 0, 127 are straight up in WoW's Z.
        let up = VertexNormal { x: 0, y: 0, z: 127 };
        assert_eq!(up.to_normalized(), [0.0, 0.0, 1.0]);
    }

    /// Each block keeps its own cell flags, so the river and the ocean tell apart by nibble.
    #[test]
    fn a_two_bit_mcnk_yields_two_mclq_blocks_in_disk_order() {
        let block = |height: f32, nibble: u8| {
            let mut b = vec![0u8; MCLQ_BLOCK];
            b[0..4].copy_from_slice(&height.to_le_bytes());
            b[4..8].copy_from_slice(&height.to_le_bytes());
            for i in 0..81 {
                b[8 + i * 8 + 4..8 + i * 8 + 8].copy_from_slice(&height.to_le_bytes());
            }
            b[8 + 81 * 8..8 + 81 * 8 + 64].fill(nibble);
            b
        };
        let mut payload = block(5.0, 4); // the river, first on disk
        payload.extend(block(0.0, 1)); // the ocean, second

        let mut data = vec![0u8; 128];
        data[0x00..0x04].copy_from_slice(&0x0cu32.to_le_bytes()); // liquid bits 2 and 3
        data[0x60..0x64].copy_from_slice(&128u32.to_le_bytes());
        data[0x64..0x68].copy_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
        data.extend(chunk(b"QLCM", &payload));
        let b = one_mcnk(data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");

        let liquids = &root.mcnk_chunks[0].liquids;
        assert_eq!(liquids.len(), 2, "one block per set liquid bit");
        assert_eq!(liquids[0].tile_flags[0] & 0xf, 4, "block 0 is the river");
        assert_eq!(liquids[0].min_height, 5.0);
        assert_eq!(liquids[1].tile_flags[0] & 0xf, 1, "block 1 is the ocean");
        assert_eq!(liquids[1].min_height, 0.0);
    }

    /// `0x68d890` reads the magma pair as `u16 → i32`, so `0xffff` is 65535, never −1.
    #[test]
    fn magma_union_bytes_read_as_two_unsigned_u16_texcoords() {
        let v = LiquidVertex {
            union_data: [0xbd, 0x00, 0xe8, 0x00],
            height: 1.0,
        };
        assert_eq!(v.texcoords(), [189, 232]); // a real Burning Steppes vertex
        assert_eq!(v.depth_byte(), 0xbd, "the same bytes, read the water way");
        let max = LiquidVertex {
            union_data: [0xff, 0xff, 0xff, 0xff],
            height: 1.0,
        };
        assert_eq!(max.texcoords(), [65535, 65535], "unsigned, not −1");
    }
}
