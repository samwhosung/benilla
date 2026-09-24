//! An ADT terrain-tile reader for **WoW 1.12.1 (build 5875)** — in-repo, replacing `wow-adt`
//! (decision 0021).
//!
//! Vanilla ADTs are monolithic, chunked (`IFF`-style: a 4-char magic — stored **reversed** on disk —
//! plus a `u32` size, then payload). The root file holds `MTEX` (textures), `MMDX`/`MMID` (M2 paths),
//! `MWMO`/`MWID` (WMO paths), `MDDF`/`MODF` (placements), and 256 `MCNK` terrain chunks. Each `MCNK` is
//! a 128-byte header followed by sub-chunks (`MCVT` heights, `MCNR` normals, `MCLY` layers, `MCAL`
//! alpha, `MCSH` shadow, `MCLQ` liquid), which we locate by **scanning** the chunk payload for their
//! magics (robust, and identical to the header-offset data for vanilla).
//!
//! The shape mirrors what the renderer's terrain/liquid mesher consumes; quirks the consumer
//! compensates for (the MCNR `[x,z,y]` byte order, the split predominant-texture/noEffectDoodad
//! grids) are preserved deliberately. Proven against `wow-adt` over real ADTs during the decision-0021
//! migration (oracle test in git history); the `benilla-formats` terrain golden tests pin the meshing
//! end-to-end on every run.
//!
//! Byte access goes through `benilla-bytes` (decision 0064): every read is bounds-checked and a
//! truncated chunk/record is a typed [`Error::Truncated`], never a panic.

use std::io::Cursor;

use benilla_bytes::{chunks, ByteExt};

/// A terrain vertex normal — stored on disk as `x, z, y` signed bytes.
#[derive(Debug, Clone, Copy)]
pub struct VertexNormal {
    pub x: i8,
    pub z: i8,
    pub y: i8,
}

impl VertexNormal {
    /// `[x/127, y/127, z/127]` — i.e. on-disk `[b0, b2, b1]` / 127 (kept bit-identical to wow-adt; the
    /// consumer cancels the z/y swap).
    pub fn to_normalized(&self) -> [f32; 3] {
        [
            f32::from(self.x) / 127.0,
            f32::from(self.y) / 127.0,
            f32::from(self.z) / 127.0,
        ]
    }
}

/// MCVT — 145 heights (9×9 outer + 8×8 inner, stride-17 interleave).
#[derive(Debug, Clone)]
pub struct McvtChunk {
    pub heights: Vec<f32>,
}

/// MCNR — one normal per MCVT vertex.
#[derive(Debug, Clone)]
pub struct McnrChunk {
    pub normals: Vec<VertexNormal>,
}

/// MCLY per-layer flags (we only need the alpha-compression bit).
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

/// MCLY — one texture layer (16 bytes on disk).
#[derive(Debug, Clone)]
pub struct MclyLayer {
    pub texture_id: u32,
    pub flags: MclyFlags,
    pub offset_in_mcal: u32,
    pub effect_id: u32,
}

/// MCLY — the chunk's texture layers (0–4; layer 0 is the opaque base).
#[derive(Debug, Clone)]
pub struct MclyChunk {
    pub layers: Vec<MclyLayer>,
}

/// MCAL — raw alpha-map bytes (indexed per layer via [`MclyLayer::offset_in_mcal`]).
#[derive(Debug, Clone)]
pub struct McalChunk {
    pub data: Vec<u8>,
}

/// MCSH — a 64×64 1-bit baked shadow map (512 bytes).
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

/// One MCLQ liquid vertex (8 bytes: 4 union bytes + an absolute height).
///
/// The leading 4 bytes are a **union whose meaning is the block's liquid type** — water/ocean spend
/// them on a depth byte + flow/foam, magma/slime on a `u16` texture-coordinate pair. Both readers are
/// below; which one applies is the caller's decision, from the cell nibble.
#[derive(Debug, Clone, Copy)]
pub struct LiquidVertex {
    pub union_data: [u8; 4],
    pub height: f32,
}

impl LiquidVertex {
    /// The depth byte (union byte 0) — drives the water depth-swatch on the render side. Water and
    /// ocean blocks only; on a magma block these bytes are [`Self::texcoords`] instead.
    pub fn depth_byte(&self) -> u8 {
        self.union_data[0]
    }

    /// The authored `(s, t)` texture-coordinate pair — two little-endian `u16`s over the same 4
    /// union bytes. **Magma blocks only**: the reference's lava vert-fill is single-stage and reads
    /// its `tc0` straight from here, where water/ocean instead write a UV into the depth-ramp
    /// texture (bit-exact `WoW.exe 0x68d890`: `u = (s as i32 as f64 · 3/256) as f32`, same for
    /// `t`). The field is authored world-continuous — a chunk's east edge repeats its neighbour's
    /// west edge exactly — so lava tiles seamlessly across MCNK borders.
    pub fn texcoords(&self) -> [u16; 2] {
        [
            u16::from_le_bytes([self.union_data[0], self.union_data[1]]),
            u16::from_le_bytes([self.union_data[2], self.union_data[3]]),
        ]
    }
}

/// One MCLQ liquid **block** — a 9×9 absolute-height grid + an 8×8 cell-flag grid, `0x324` bytes.
///
/// An MCNK carries **one block per set liquid header bit** (bits 2–5), packed back to back and with
/// absent bits consuming nothing (the cursor walk at `0x6af7a3`–`0x6af7cb`). Most liquid chunks set
/// exactly one bit; **28 shipped Azeroth chunks set two** — a river *and* the sea, at a river mouth
/// — and carry two blocks at different heights (measured: `sizeMCLQ` 1616 = 8 + 2·`0x324`, block 0
/// the stream at z≈5.0, block 1 the ocean at z=0). Reading only the first is how the sea goes
/// missing there.
///
/// **No liquid type here on purpose.** The header-bit→type ordering (bit2 river / bit3 ocean /
/// bit4 magma / bit5 slime) is *inferred* — never byte-proven — while the per-cell flag low nibble
/// IS the type, as both consumers read it (`0x6ba970` `and al,0xf`, `0x68d9b0`). So this reader
/// takes only the block *count* from the flags and leaves the type to whoever reads
/// [`Self::tile_flags`]. (The old `LiquidType::from_mcnk_flags` priority guess labelled all 28
/// two-bit chunks "ocean" while handing back the river block's bytes.)
#[derive(Debug, Clone)]
pub struct MclqChunk {
    pub min_height: f32,
    pub max_height: f32,
    pub vertices: Vec<LiquidVertex>,
    /// Per-cell flags, row-major 8×8. Low nibble = liquid type (`0xf` = dry/hole); `0x80` shared.
    /// `0x40` ("fishable") is carried but deliberately unread: its only compiled reader, the
    /// reference's fishable query `0x69b5d0`, is a zero-caller dead island — fishability is the
    /// server's verdict (`SPELL_FAILED_NOT_FISHABLE` passthru). Decision 1825.
    pub tile_flags: [u8; 64],
}

/// MCNK header flag **bit 1** — the chunk a mover may not enter (the "impassable" band that walls
/// Searing Gorge and the other mountain rims off from their neighbours).
///
/// The *producer* is the MCNK header walk (`0x6af5f0`: `hdr+0x00` bit1 → `chunk+0xc |= 0x40`);
/// the *name* is an inferred label for that bit. Consuming it is decision 1266 / report B129.
pub const MCNK_IMPASSABLE: u32 = 0x2;

/// The fields of the 128-byte MCNK header the renderer reads. `pred_tex`/`no_effect_doodad`/
/// `unknown_8bytes` mirror wow-adt's (mis)split of the predominant-texture + noEffectDoodad region,
/// which the consumer reconstructs — kept identical on purpose.
#[derive(Debug, Clone)]
pub struct McnkHeader {
    pub flags: u32,
    pub index_x: u32,
    pub index_y: u32,
    /// `AreaTable.dbc` id of this chunk (header +0x34) — the zone/subzone the player is standing
    /// in; drives zone music/ambience/reverb selection (decision 0070).
    pub area_id: u32,
    pub holes_low_res: u16,
    pub pred_tex: [u8; 8],
    pub no_effect_doodad: [u8; 8],
    pub unknown_8bytes: [u8; 8],
    pub position: [f32; 3],
}

impl McnkHeader {
    /// Is this chunk flagged [`MCNK_IMPASSABLE`]? The flag is a header bit, so its granularity is
    /// the whole 33.333 yd chunk; in the shipped data the set chunks form contiguous ribbons along
    /// zone rims (Searing Gorge's is 48 chunks of `Azeroth_33_44` alone).
    pub fn impassable(&self) -> bool {
        self.flags & MCNK_IMPASSABLE != 0
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
    /// The chunk's MCLQ liquid blocks — **one per set liquid header bit**, in on-disk order (see
    /// [`MclqChunk`]). Empty on a dry chunk; two at a river mouth.
    pub liquids: Vec<MclqChunk>,
}

/// MDDF — a placed M2 doodad (36 bytes).
#[derive(Debug, Clone)]
pub struct DoodadPlacement {
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: u16,
    pub flags: u16,
}

/// MODF — a placed WMO (64 bytes).
#[derive(Debug, Clone)]
pub struct WmoPlacement {
    pub name_id: u32,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub flags: u16,
    pub doodad_set: u16,
    /// `WMOAreaTable.NameSetID` selector — which naming/audio variant of the building this
    /// placement is (Northshire Abbey vs Tyr's Hand Abbey are one WMO, different name sets).
    pub name_set: u16,
}

/// A parsed vanilla (monolithic) root ADT.
#[derive(Debug, Clone)]
pub struct RootAdt {
    pub textures: Vec<String>,
    pub models: Vec<String>,
    pub wmos: Vec<String>,
    pub doodad_placements: Vec<DoodadPlacement>,
    pub wmo_placements: Vec<WmoPlacement>,
    pub mcnk_chunks: Vec<McnkChunk>,
}

/// Either a vanilla monolithic root ADT (all we produce) — kept as an enum to mirror the consumer's
/// `ParsedAdt::Root(..)` match.
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

/// Split a NUL-separated string blob into a `Vec<u32>` of start offsets paired with strings (used to
/// resolve MMDX/MWMO via MMID/MWID offsets).
fn cstring_at(blob: &[u8], offset: usize) -> String {
    let end = blob[offset.min(blob.len())..]
        .iter()
        .position(|&c| c == 0)
        .map(|p| offset + p)
        .unwrap_or(blob.len());
    String::from_utf8_lossy(&blob[offset.min(blob.len())..end]).into_owned()
}

/// Parse a vanilla monolithic ADT. (`cursor` position is ignored; the file starts at the first chunk.)
pub fn parse_adt(cursor: &mut Cursor<&[u8]>) -> Result<ParsedAdt> {
    let b: &[u8] = cursor.get_ref();

    let mut textures = Vec::new();
    let mut mmdx = Vec::new();
    let mut mmid: Vec<u32> = Vec::new();
    let mut mwmo = Vec::new();
    let mut mwid: Vec<u32> = Vec::new();
    let mut doodad_placements = Vec::new();
    let mut wmo_placements = Vec::new();
    let mut mcnk_chunks = Vec::new();

    for (magic, data) in chunks(b) {
        match &magic {
            b"XETM" => textures = split_strings(data),       // MTEX
            b"XDMM" => mmdx = data.to_vec(),                 // MMDX (blob)
            b"DIMM" => mmid = read_u32_list(data, "MMID")?,  // MMID (offsets)
            b"OMWM" => mwmo = data.to_vec(),                 // MWMO (blob)
            b"DIWM" => mwid = read_u32_list(data, "MWID")?,  // MWID (offsets)
            b"FDDM" => doodad_placements = read_mddf(data)?, // MDDF
            b"FDOM" => wmo_placements = read_modf(data)?,    // MODF
            b"KNCM" => mcnk_chunks.push(read_mcnk(data)?),   // MCNK
            _ => {}
        }
    }

    // Resolve model/WMO paths via the offset tables (the order MDDF/MODF `name_id` indexes).
    let models = mmid
        .iter()
        .map(|&o| cstring_at(&mmdx, o as usize))
        .collect();
    let wmos = mwid
        .iter()
        .map(|&o| cstring_at(&mwmo, o as usize))
        .collect();

    Ok(ParsedAdt::Root(Box::new(RootAdt {
        textures,
        models,
        wmos,
        doodad_placements,
        wmo_placements,
        mcnk_chunks,
    })))
}

/// Split a NUL-separated, NUL-terminated string blob, dropping empties.
fn split_strings(blob: &[u8]) -> Vec<String> {
    blob.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// `name` tags the truncation error with the chunk this record list came from (MMID/MWID).
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
            // 0x20..0x38 = bounding box (6 f32); then:
            flags: r.u16_at(56).ok_or(Error::Truncated("MODF"))?,
            doodad_set: r.u16_at(58).ok_or(Error::Truncated("MODF"))?,
            name_set: r.u16_at(60).ok_or(Error::Truncated("MODF"))?,
            // 0x3E scale u16 — unused.
        });
    }
    Ok(out)
}

/// Parse one MCNK: a 128-byte header, then sub-chunks located by scanning the payload for their
/// (reversed) magics.
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
    };

    // Locate sub-chunks by the MCNK header offsets (the authoritative method — sequential scanning
    // breaks when sub-chunks aren't contiguous, e.g. AhnQiraj). MCVT/MCNR offsets live in the
    // `multipurpose_field` (0x14/0x18); the rest are explicit header fields. The offset base differs
    // across sources (relative to the MCNK magic vs the MCNK data), so we auto-detect per sub-chunk by
    // validating the (reversed) magic at both candidate positions. Returns the payload `(start, size)`.
    // `ofs` comes straight off a (possibly corrupt) file's header, so both the "minus 8" candidate
    // and the 8-byte header read use checked/bounds-checked arithmetic (`checked_sub`/`bytes_at`/
    // `u32_at`) — a wrapped or out-of-range offset yields `None`, never a panic.
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
    // `start` always comes from a `locate` hit (bounds-proven above); `len` is attacker-controlled,
    // so the end is saturating before clamping to the buffer.
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
                z: sub.u8_at(i * 3 + 1).ok_or(Error::Truncated("MCNR"))? as i8,
                y: sub.u8_at(i * 3 + 2).ok_or(Error::Truncated("MCNR"))? as i8,
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
    // MCAL's data length is the header's `size_alpha`, NOT the sub-chunk's declared size (they differ
    // on some files, e.g. AhnQiraj — wow-adt reads `size_alpha`).
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
    // MCLQ, like MCAL, takes its length from the MCNK header (`size_liquid`): the MCLQ sub-chunk's
    // own declared size is always 0.
    if let Some((p, _)) = locate(
        h.u32_at(0x60).ok_or(Error::Truncated("MCNK header"))?,
        b"QLCM",
    ) {
        let size_liquid = h.u32_at(0x64).ok_or(Error::Truncated("MCNK header"))? as usize;
        chunk.liquids = read_mclq_blocks(slice(p, size_liquid), chunk.header.flags);
    }

    Ok(chunk)
}

/// Bytes per MCLQ block: `{f32 min, f32 max, 81×8B verts, 64B cell flags, u32 nFlow, 2×40B flow}`.
const MCLQ_BLOCK: usize = 0x324;
/// Liquid header bits (2–5) — their **count** is the number of packed MCLQ blocks. Only the count is
/// read: the bit→type ordering is INFERRED, the cell nibble is VERIFIED (see [`MclqChunk`]).
const MCNK_LIQUID_BITS: u32 = 0x3c;

/// Read the packed MCLQ blocks — one per set liquid header bit, back to back (the cursor walk
/// `0x6af7a3`–`0x6af7cb`). Stops early on a block the payload can't cover, so a truncated tail
/// degrades to the blocks that *are* whole rather than to nothing. A chunk with an MCLQ sub-chunk
/// but no liquid bit set still yields one block, matching the pre-multi-block reader.
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

/// `None` (not an error) below the minimum size: too-short MCLQ data is treated as "no liquid",
/// matching the pre-0064 reader — a real-data tolerance, not a hardening carve-out. (The minimum is
/// the *read* span, short of the full [`MCLQ_BLOCK`]: the trailing flow vectors are unread.)
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

    /// A 128-byte MCNK header with a single MCVT (145 heights) sub-chunk placed right after it,
    /// referenced via the "offset relative to the MCNK magic" convention (multipurpose field @0x14
    /// pointing at the MCVT chunk's own magic, 128 bytes in).
    fn mcnk_with_mcvt() -> Vec<u8> {
        let mut data = vec![0u8; 128];
        data[0x14..0x18].copy_from_slice(&128u32.to_le_bytes());
        let heights: Vec<u8> = (0..145u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
        data.extend(chunk(b"TVCM", &heights));
        data
    }

    fn parse(bytes: &[u8]) -> Result<ParsedAdt> {
        parse_adt(&mut Cursor::new(bytes))
    }

    #[test]
    fn root_parses_a_minimal_well_formed_chunk_walk() {
        let mtex = b"tex1.blp\0tex2.blp\0";
        let mmdx = b"model.m2\0";
        let mmid = 0u32.to_le_bytes();
        let mwmo = b"world.wmo\0";
        let mwid = 0u32.to_le_bytes();

        let mut mddf = vec![0u8; 36];
        mddf[32..34].copy_from_slice(&7u16.to_le_bytes()); // scale

        let mut modf = vec![0u8; 64];
        modf[58..60].copy_from_slice(&3u16.to_le_bytes()); // doodad_set

        let mut b = chunk(b"XETM", mtex);
        b.extend(chunk(b"XDMM", mmdx));
        b.extend(chunk(b"DIMM", &mmid));
        b.extend(chunk(b"OMWM", mwmo));
        b.extend(chunk(b"DIWM", &mwid));
        b.extend(chunk(b"FDDM", &mddf));
        b.extend(chunk(b"FDOM", &modf));
        b.extend(chunk(b"KNCM", &mcnk_with_mcvt()));

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

    #[test]
    fn partial_trailing_mddf_record_is_dropped_not_a_panic() {
        // A record shorter than the 36-byte MDDF stride: `chunks_exact` drops the short remainder
        // (a pre-existing, unchanged leniency — every *complete* 36-byte record is still
        // bounds-checked via `ByteExt`, so this can never panic or misread).
        let b = chunk(b"FDDM", &[0u8; 10]);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert!(root.doodad_placements.is_empty());
    }

    #[test]
    fn truncated_mcnk_header_is_an_error_not_a_panic() {
        let b = chunk(b"KNCM", &[0u8; 40]);
        assert!(matches!(parse(&b), Err(Error::Truncated("MCNK header"))));
    }

    #[test]
    fn corrupt_mcnk_subchunk_offset_skips_cleanly_never_panics() {
        // ofs = 3: `checked_sub(8)` on the second candidate is `None`, and the first candidate (3)
        // doesn't land on a valid magic — must yield `None`, not wrap/panic (the verified
        // `ofs.wrapping_sub(8)` trap this replaces).
        let mut data = vec![0u8; 128];
        data[0x14..0x18].copy_from_slice(&3u32.to_le_bytes());
        let b = chunk(b"KNCM", &data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses despite corrupt MCVT offset");
        assert!(root.mcnk_chunks[0].heights.is_none());

        // ofs points past the end of the buffer entirely.
        let mut data = vec![0u8; 128];
        data[0x14..0x18].copy_from_slice(&u32::MAX.to_le_bytes());
        let b = chunk(b"KNCM", &data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses despite out-of-range MCVT offset");
        assert!(root.mcnk_chunks[0].heights.is_none());
    }

    #[test]
    fn short_mclq_is_treated_as_no_liquid_not_an_error() {
        let mut data = vec![0u8; 128];
        data[0x60..0x64].copy_from_slice(&128u32.to_le_bytes()); // MCLQ offset
        data[0x64..0x68].copy_from_slice(&4u32.to_le_bytes()); // size_liquid — far short of NEED
        data.extend(chunk(b"QLCM", &[0u8; 4]));
        let b = chunk(b"KNCM", &data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");
        assert!(root.mcnk_chunks[0].liquids.is_empty());
    }

    /// **One MCLQ block per set liquid header bit**, packed back to back — the shape that made the
    /// sea vanish at 28 shipped river mouths when only the first was read (see [`MclqChunk`]).
    /// Two bits set ⇒ two blocks, each keeping its OWN cell flags, so the consumer can tell the
    /// river block from the ocean block by nibble.
    #[test]
    fn a_two_bit_mcnk_yields_two_mclq_blocks_in_disk_order() {
        // One block: min/max, 81×8B verts, 64B cell flags, then the flow tail — 0x324 total.
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
        let mut payload = block(5.0, 4); // bit 2 (river) — first on disk
        payload.extend(block(0.0, 1)); // bit 3 (ocean) — second

        let mut data = vec![0u8; 128];
        data[0x00..0x04].copy_from_slice(&0x0cu32.to_le_bytes()); // liquid bits 2 AND 3
        data[0x60..0x64].copy_from_slice(&128u32.to_le_bytes());
        data[0x64..0x68].copy_from_slice(&((payload.len() + 8) as u32).to_le_bytes());
        data.extend(chunk(b"QLCM", &payload));
        let b = chunk(b"KNCM", &data);
        let ParsedAdt::Root(root) = parse(&b).expect("parses");

        let liquids = &root.mcnk_chunks[0].liquids;
        assert_eq!(liquids.len(), 2, "one block per set liquid bit");
        assert_eq!(liquids[0].tile_flags[0] & 0xf, 4, "block 0 is the river");
        assert_eq!(liquids[0].min_height, 5.0);
        assert_eq!(liquids[1].tile_flags[0] & 0xf, 1, "block 1 is the ocean");
        assert_eq!(liquids[1].min_height, 0.0);
    }

    /// The magma union bytes are an authored `(s, t)` u16 pair, little-endian over the same 4 bytes
    /// the water blocks spend on a depth byte — VERIFIED `0x68d890` reads them as `u16 → i32`, so
    /// `0xffff` is 65535 and never −1.
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
