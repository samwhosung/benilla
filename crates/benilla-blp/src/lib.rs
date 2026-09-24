//! A BLP2 texture decoder for **WoW 1.12.1 (build 5875)** — in-repo, replacing `wow-blp` (decision
//! 0021). Decodes to `Rgba8Unorm` (the gamma-byte form the renderer uploads verbatim).
//!
//! 1.12 ships **BLP2 / Direct** content in three shapes (no JPEG in practice):
//! - **Raw1** — a 256-entry **BGRA** palette + 1-byte indices, with a separate 0/1/4/8-bit alpha block.
//! - **Raw3** — uncompressed BGRA8.
//! - **DXTC** — S3TC blocks; `alpha_type` selects DXT1 (0) / DXT3 (1) / DXT5 (7), decoded via the same
//!   `texpresso` codec `wow-blp` uses, so the output is byte-identical.
//!
//! **Two entry points, because the GPU wants the blocks and the CPU wants the pixels.**
//! [`decode`] always yields `Rgba8Unorm`; [`decode_native`] hands back the DXTC blocks *verbatim*
//! and only decodes the shapes that have no GPU-native form (Raw1/Raw3). The reference client
//! uploads the stored blocks untouched (`glCompressedTexImage2DARB` in `0x59f270`; a raw
//! passthrough — the device eats DXT), so the native path is both the faithful one and 4x-8x
//! cheaper in VRAM and texture bandwidth. Decoding is for consumers that read texels on the CPU.
//!
//! Two fidelity points that cost the old `wow-blp` two forks, folded in here natively:
//! 1. the palette is **BGRA**, not RGBA (reading it as RGBA swaps R↔B on every palettized Blizzard
//!    atlas — the "Westfall clutter is blue" bug);
//! 2. a stale/unknown `alpha_type` byte (e.g. `2` on alpha-less particle atlases) is **not fatal** —
//!    alpha presence is governed by `alpha_bits`; `alpha_type` only picks the DXT variant.
//!
//! Proven byte-for-byte against `wow-blp` over the real corpus during the decision-0021 migration
//! (oracle test in git history); the `benilla-formats` texture loaders exercise it on every run.
//!
//! Byte access goes through `benilla-bytes`: every header read is bounds-checked,
//! and `width`/`height` are capped at [`MAX_DIM`] so a corrupt header can't turn into a
//! multi-gigabyte allocation in [`decode_dxt`].

use benilla_bytes::ByteExt;

/// Decode error. Anything outside the 1.12 BLP2 envelope surfaces here rather than guessing.
#[derive(Debug)]
pub enum Error {
    NotBlp2,
    Truncated(&'static str),
    UnknownCompression(u8),
    BadColorMap,
    OutOfBounds {
        level: usize,
    },
    /// `width`/`height` from the header exceed [`MAX_DIM`] — most likely a corrupt or hostile
    /// header, since no real 1.12 BLP is anywhere near this large.
    DimensionsTooLarge {
        width: u32,
        height: u32,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotBlp2 => write!(f, "not a BLP2 file (bad magic)"),
            Error::Truncated(what) => write!(f, "truncated BLP: {what}"),
            Error::UnknownCompression(c) => write!(f, "unknown BLP compression {c}"),
            Error::BadColorMap => write!(f, "BLP color map shorter than 256 entries"),
            Error::OutOfBounds { level } => write!(f, "BLP mip level {level} out of bounds"),
            Error::DimensionsTooLarge { width, height } => write!(
                f,
                "BLP dimensions {width}x{height} exceed the {MAX_DIM} sanity cap"
            ),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// A decoded BLP: mip-major `Rgba8Unorm`. `mips[0]` is the full image and is always present; deeper
/// levels follow while the file carries them. [`mip_chain_count`](DecodedBlp::mip_chain_count) is the
/// count `wow-blp` reports for the dimensions (what the renderer's authored-mip upload iterates).
pub struct DecodedBlp {
    pub width: u32,
    pub height: u32,
    /// One RGBA8 buffer per decoded mip level, level 0 first.
    pub mips: Vec<MipLevel>,
    mip_chain_count: usize,
}

/// One decoded mip level.
pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl DecodedBlp {
    /// The authored mip-chain length for these dimensions, matching `wow-blp`'s `mipmaps_count()`
    /// (`max(log2 w, log2 h)` when the file has mipmaps, else 0). The renderer's mip-chain upload
    /// iterates exactly this many levels; `mips` may hold one more (level 0) when this is 0.
    pub fn mip_chain_count(&self) -> usize {
        self.mip_chain_count
    }
}

const HEADER_SIZE: usize = 148; // magic..=mip_sizes[16]
const PALETTE_SIZE: usize = 256 * 4;

/// Sanity cap on header `width`/`height`. Real 1.12 art tops out at 1024×1024; this is pure
/// headroom above any real asset, and its only job is bounding allocation: [`decode_dxt`] sizes
/// both its block-padding buffer and its `w*h*4` RGBA output straight from these fields, so a
/// corrupt header claiming e.g. 65535×65535 would otherwise reserve gigabytes before a single
/// byte of pixel data is checked.
const MAX_DIM: u32 = 8192;

/// `wow-blp`'s mip count formula (kept bit-identical, float `log2` and all, so the chain length we
/// report matches the oracle exactly).
fn mip_chain_count(width: u32, height: u32, has_mipmaps: bool) -> usize {
    if has_mipmaps {
        let w = (width as f32).log2() as usize;
        let h = (height as f32).log2() as usize;
        w.max(h)
    } else {
        0
    }
}

fn level_size(width: u32, height: u32, level: usize) -> (u32, u32) {
    if level == 0 {
        (width, height)
    } else {
        ((width >> level).max(1), (height >> level).max(1))
    }
}

/// The texel form a [`NativeBlp`] level carries.
///
/// A BLP is one of three shapes on disk (module header); only DXTC has a GPU-native form, so the
/// other two report [`Self::Rgba8Unorm`] and carry decoded pixels. Callers switch on this to pick a
/// `TextureFormat` and to size a level.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlpTexels {
    /// Decoded pixels — the Raw1 (palettized) and Raw3 (BGRA8) shapes, which have no block form.
    Rgba8Unorm,
    /// DXT1 blocks (8 bytes per 4x4), `alpha_type` 0. 1-bit alpha at most.
    Bc1,
    /// DXT3 blocks (16 bytes per 4x4), `alpha_type` 1. Explicit 4-bit alpha.
    Bc2,
    /// DXT5 blocks (16 bytes per 4x4), `alpha_type` 7. Interpolated alpha.
    Bc3,
}

impl BlpTexels {
    /// Is this a block-compressed form (i.e. can it be uploaded to the GPU verbatim)?
    pub fn is_block_compressed(self) -> bool {
        !matches!(self, Self::Rgba8Unorm)
    }

    /// How many bytes a `width x height` level occupies in this form.
    ///
    /// Block forms round **up** to whole 4x4 blocks, which is why a 2x2 or 1x1 tail mip still costs
    /// one full block — exactly what the BLP stores for it, and what wgpu expects to receive.
    pub fn level_bytes(self, width: u32, height: u32) -> usize {
        match self {
            Self::Rgba8Unorm => (width as usize) * (height as usize) * 4,
            Self::Bc1 | Self::Bc2 | Self::Bc3 => {
                let blocks = width.div_ceil(4) as usize * height.div_ceil(4) as usize;
                blocks * if self == Self::Bc1 { 8 } else { 16 }
            }
        }
    }

    /// The `texpresso` codec for a block form, or `None` for [`Self::Rgba8Unorm`].
    fn codec(self) -> Option<texpresso::Format> {
        match self {
            Self::Rgba8Unorm => None,
            Self::Bc1 => Some(texpresso::Format::Bc1),
            Self::Bc2 => Some(texpresso::Format::Bc2),
            Self::Bc3 => Some(texpresso::Format::Bc3),
        }
    }
}

/// A BLP with its levels in whatever form the file stores them — see [`decode_native`].
pub struct NativeBlp {
    pub width: u32,
    pub height: u32,
    /// What every entry of `mips` holds.
    pub texels: BlpTexels,
    /// One buffer per level, level 0 first. Never empty (same guarantee as [`DecodedBlp::mips`]).
    pub mips: Vec<NativeMip>,
    mip_chain_count: usize,
}

/// One level of a [`NativeBlp`].
pub struct NativeMip {
    pub width: u32,
    pub height: u32,
    /// Block bytes (DXTC) or RGBA8 pixels — per [`NativeBlp::texels`]. Always exactly
    /// [`BlpTexels::level_bytes`] long: a sub-block tail level the header under-stores is
    /// completed from the bytes that follow it in the file, as the reference copies it
    /// (`Header::dxt_level_span`), and only a level the file ends inside is zero-padded —
    /// matching what [`decode`] does before handing blocks to the codec.
    pub bytes: Vec<u8>,
}

impl NativeBlp {
    /// See [`DecodedBlp::mip_chain_count`].
    pub fn mip_chain_count(&self) -> usize {
        self.mip_chain_count
    }
}

/// The header fields both entry points read, parsed once.
struct Header<'a> {
    compression: u8,
    alpha_bits: u32,
    alpha_type: u8,
    width: u32,
    height: u32,
    offsets: Vec<u32>,
    sizes: Vec<u32>,
    palette: &'a [u8],
    /// The authored chain length, `mip_chain_count(width, height, has_mipmaps)`.
    chain: usize,
}

/// Parse and bounds-check the BLP2 header, palette and level table. Shared by [`decode`] and
/// [`decode_native`] so the two can never disagree about what a file says.
fn parse_header(bytes: &[u8]) -> Result<Header<'_>> {
    if bytes.len() < HEADER_SIZE || &bytes[0..4] != b"BLP2" {
        return Err(Error::NotBlp2);
    }
    // content @4 (u32; 1 = Direct — we only handle direct, the only kind 1.12 ships).
    // These reads are already covered by the `HEADER_SIZE` length check above; routed through
    // `ByteExt` anyway for uniformity — no future reader should have to ask
    // "is this offset guarded?".
    let compression = bytes.u8_at(8).ok_or(Error::Truncated("header"))?;
    let alpha_bits = bytes.u8_at(9).ok_or(Error::Truncated("header"))? as u32;
    let alpha_type = bytes.u8_at(10).ok_or(Error::Truncated("header"))?;
    let has_mipmaps = bytes.u8_at(11).ok_or(Error::Truncated("header"))? != 0;
    let width = bytes.u32_at(12).ok_or(Error::Truncated("header"))?;
    let height = bytes.u32_at(16).ok_or(Error::Truncated("header"))?;
    let offsets: Vec<u32> = (0..16)
        .map(|i| bytes.u32_at(20 + i * 4).ok_or(Error::Truncated("header")))
        .collect::<Result<_>>()?;
    let sizes: Vec<u32> = (0..16)
        .map(|i| bytes.u32_at(84 + i * 4).ok_or(Error::Truncated("header")))
        .collect::<Result<_>>()?;

    // The corrupt-header guard (see `MAX_DIM`): bound before anything downstream sizes an
    // allocation from these fields. Zero is left alone — see the decode_dxt/decode_raw1/
    // decode_raw3 zero-pixel behavior, unchanged from before this migration.
    if width > MAX_DIM || height > MAX_DIM {
        return Err(Error::DimensionsTooLarge { width, height });
    }

    // BLP2 direct content always carries a 256-entry palette right after the header (used only by
    // Raw1; present-but-ignored for Raw3/DXT). Read as BGRA bytes.
    let palette = bytes
        .get(HEADER_SIZE..HEADER_SIZE + PALETTE_SIZE)
        .ok_or(Error::BadColorMap)?;

    let chain = mip_chain_count(width, height, has_mipmaps);
    Ok(Header {
        compression,
        alpha_bits,
        alpha_type,
        width,
        height,
        offsets,
        sizes,
        palette,
        chain,
    })
}

impl Header<'_> {
    /// The raw stored bytes of level `level`, or `None` when the chain ends early (some textures
    /// stop before the formula's count).
    fn level_data<'b>(&self, bytes: &'b [u8], level: usize) -> Result<Option<&'b [u8]>> {
        let off = self.offsets[level] as usize;
        let sz = self.sizes[level] as usize;
        if level > 0 && (sz == 0 || off == 0) {
            return Ok(None);
        }
        bytes
            .get(off..off + sz)
            .map(Some)
            .ok_or(Error::OutOfBounds { level })
    }

    /// The bytes of DXTC level `level` **as the reference copies them** — `need` bytes from the
    /// level's file offset, not the `sizes[level]` the header records — or `None` when the chain
    /// ends early (same rule as [`Self::level_data`]).
    ///
    /// The two disagree exactly on the **sub-block tail**. The BLP encoder stores a level as
    /// `max(1, (w/4)·(h/4))` blocks — ONE block for a 2×16 or a 1×8 level, where the block grid the
    /// GPU decodes needs four and two. The reference never reads the header's size for a DXT level.
    /// On its shipped Direct3D default the fill copies each level through the block copier
    /// `0x5a5780`, `max(4,w)·max(4,h)` bytes flat from `fileImage + mipOffsets[level]`; on the
    /// OpenGL arm it hands the upload that same pointer (`0x5a8555`, no copy) and `0x59f270` sizes
    /// the read from the format alone, `bpp · max(4, wL) · max(4, hL) / 8` (`0x59f4e6`–`0x59f515`).
    /// Either way the GPU gets the full block grid from the level's offset, so a short level is
    /// completed with the bytes that FOLLOW it in the file: the next levels' own authored blocks.
    /// Padding the difference with
    /// zeros instead (an all-zero BC block = colour black, alpha 0) is what turned the far rain
    /// black: `RainDrop01.blp`'s levels 3 and 4 came out three-quarters and half black, and its
    /// Mod2x lane reads no alpha. Only a level the FILE itself ends
    /// inside is left short here, for [`pad_to`] to zero-fill — the reference reads past the file
    /// into whatever its shared read buffer held before, which is nothing to be faithful to.
    fn dxt_level_span<'b>(
        &self,
        bytes: &'b [u8],
        level: usize,
        need: usize,
    ) -> Result<Option<&'b [u8]>> {
        let Some(stored) = self.level_data(bytes, level)? else {
            return Ok(None);
        };
        if stored.len() >= need {
            return Ok(Some(stored));
        }
        let off = self.offsets[level] as usize;
        let end = (off + need).min(bytes.len());
        Ok(Some(&bytes[off..end]))
    }

    /// Which block form this file's DXTC levels are in. `alpha_type` 0→DXT1, 1→DXT3, 7→DXT5;
    /// anything else (a stale byte — e.g. `2` on alpha-less particle atlases) → DXT1, since
    /// `alpha_bits` already governs whether alpha is meaningful.
    fn dxt_texels(&self) -> BlpTexels {
        match self.alpha_type {
            1 => BlpTexels::Bc2,
            7 => BlpTexels::Bc3,
            _ => BlpTexels::Bc1,
        }
    }
}

/// Decode a BLP2 texture (raw archive bytes) to RGBA8 mip levels.
pub fn decode(bytes: &[u8]) -> Result<DecodedBlp> {
    let h = parse_header(bytes)?;
    // Always decode level 0 (blp_to_rgba needs it even when the chain count is 0); then deeper levels
    // up to the chain count, stopping if the file doesn't carry one.
    let n = h.chain.clamp(1, 16);

    let mut mips = Vec::with_capacity(n);
    for level in 0..n {
        let (lw, lh) = level_size(h.width, h.height, level);
        // DXTC levels span what the reference copies (`dxt_level_span`), not what the header
        // records — the two differ on the sub-block tail, and the difference was black.
        let data = if h.compression == 2 {
            let texels = h.dxt_texels();
            h.dxt_level_span(bytes, level, texels.level_bytes(lw, lh))?
        } else {
            h.level_data(bytes, level)?
        };
        let Some(data) = data else {
            break;
        };
        let rgba = match h.compression {
            1 => decode_raw1(h.palette, data, lw, lh, h.alpha_bits),
            2 => decode_dxt(data, lw, lh, h.dxt_texels()),
            3 => decode_raw3(data, lw, lh),
            other => return Err(Error::UnknownCompression(other)),
        }?;
        mips.push(MipLevel {
            width: lw,
            height: lh,
            rgba,
        });
    }

    Ok(DecodedBlp {
        width: h.width,
        height: h.height,
        mips,
        mip_chain_count: h.chain,
    })
}

/// Decode a BLP2 texture **keeping its DXTC blocks verbatim** — the form the reference uploads.
///
/// DXTC levels come back as block bytes with [`NativeBlp::texels`] naming the variant; Raw1/Raw3
/// files, which have no block form, are decoded exactly as [`decode`] would and report
/// [`BlpTexels::Rgba8Unorm`]. Either way every level is padded to its full
/// [`BlpTexels::level_bytes`], so a caller can concatenate the chain and hand it to the GPU without
/// re-deriving sizes.
pub fn decode_native(bytes: &[u8]) -> Result<NativeBlp> {
    let h = parse_header(bytes)?;
    let texels = if h.compression == 2 {
        h.dxt_texels()
    } else {
        BlpTexels::Rgba8Unorm
    };
    let n = h.chain.clamp(1, 16);

    let mut mips = Vec::with_capacity(n);
    for level in 0..n {
        let (lw, lh) = level_size(h.width, h.height, level);
        let need = texels.level_bytes(lw, lh);
        let data = if h.compression == 2 {
            h.dxt_level_span(bytes, level, need)?
        } else {
            h.level_data(bytes, level)?
        };
        let Some(data) = data else {
            break;
        };
        let bytes = match h.compression {
            1 => decode_raw1(h.palette, data, lw, lh, h.alpha_bits)?,
            2 => pad_to(data, need),
            3 => decode_raw3(data, lw, lh)?,
            other => return Err(Error::UnknownCompression(other)),
        };
        mips.push(NativeMip {
            width: lw,
            height: lh,
            bytes,
        });
    }

    Ok(NativeBlp {
        width: h.width,
        height: h.height,
        texels,
        mips,
        mip_chain_count: h.chain,
    })
}

/// Decode one block-compressed level to RGBA8 — the inverse of keeping the blocks.
///
/// For a caller that took [`decode_native`]'s passthrough and then found it could not upload it
/// after all (no BC on the device): it can get the pixels without re-reading and re-parsing the
/// file. `texels` must be a block form; [`BlpTexels::Rgba8Unorm`] returns `bytes` unchanged, since
/// that is already the answer.
pub fn decode_level(texels: BlpTexels, width: u32, height: u32, bytes: &[u8]) -> Vec<u8> {
    let Some(fmt) = texels.codec() else {
        return bytes.to_vec();
    };
    let blocks = pad_to(bytes, fmt.compressed_size(width as usize, height as usize));
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    fmt.decompress(&blocks, width as usize, height as usize, &mut out);
    out
}

/// `data` grown to exactly `need` bytes with zeros (or truncated if the file over-stores).
///
/// After `Header::dxt_level_span` this only ever pads a level the FILE ends inside (a truncated
/// archive) — the sub-block tail the encoder under-stores is completed from the following levels
/// first, the way the reference does. Zero here is a BC block that decodes to black at alpha 0,
/// which is why the span comes first.
fn pad_to(data: &[u8], need: usize) -> Vec<u8> {
    let mut out = vec![0u8; need];
    let n = data.len().min(need);
    out[..n].copy_from_slice(&data[..n]);
    out
}

/// Palettized: 1-byte indices into a 256-entry BGRA palette, then a packed alpha block (`alpha_bits`).
fn decode_raw1(palette: &[u8], data: &[u8], w: u32, h: u32, alpha_bits: u32) -> Result<Vec<u8>> {
    let px = (w as usize) * (h as usize);
    let indices = data.get(..px).ok_or(Error::Truncated("raw1 indices"))?;
    let alpha = &data[px..];
    let mut out = vec![0u8; px * 4];
    for i in 0..px {
        let ci = indices[i] as usize;
        let p = ci * 4;
        // Palette is BGRA: byte0=B, byte1=G, byte2=R. (The fork fix — RGBA reading swaps R↔B.)
        out[i * 4] = palette[p + 2]; // R
        out[i * 4 + 1] = palette[p + 1]; // G
        out[i * 4 + 2] = palette[p]; // B
        out[i * 4 + 3] = alpha_at(alpha, i, alpha_bits);
    }
    Ok(out)
}

/// Per-pixel alpha from the packed block. `0` ⇒ opaque (no alpha channel); `1/4/8` ⇒ packed bits.
fn alpha_at(alpha: &[u8], i: usize, alpha_bits: u32) -> u8 {
    match alpha_bits {
        1 => {
            let bit = (alpha.get(i / 8).copied().unwrap_or(0) >> (i % 8)) & 1;
            if bit == 1 {
                255
            } else {
                0
            }
        }
        4 => {
            let block = alpha.get(i / 2).copied().unwrap_or(0);
            let nib = if i.is_multiple_of(2) {
                block & 0x0F
            } else {
                block >> 4
            };
            (nib << 4) | nib
        }
        8 => alpha.get(i).copied().unwrap_or(255),
        _ => 255, // alpha_bits == 0: fully opaque
    }
}

/// Uncompressed BGRA8 → RGBA8.
fn decode_raw3(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
    let px = (w as usize) * (h as usize);
    let src = data.get(..px * 4).ok_or(Error::Truncated("raw3 pixels"))?;
    let mut out = vec![0u8; px * 4];
    for i in 0..px {
        out[i * 4] = src[i * 4 + 2]; // R
        out[i * 4 + 1] = src[i * 4 + 1]; // G
        out[i * 4 + 2] = src[i * 4]; // B
        out[i * 4 + 3] = src[i * 4 + 3]; // A
    }
    Ok(out)
}

/// DXTC → RGBA8. The variant comes from [`Header::dxt_texels`]. `data` is the level's span as
/// the reference copies it (`Header::dxt_level_span`); anything still short of the block size —
/// a file that ends inside the level — is zero-padded so the level decodes.
fn decode_dxt(data: &[u8], w: u32, h: u32, texels: BlpTexels) -> Result<Vec<u8>> {
    let fmt = texels.codec().expect("dxt_texels never returns Rgba8Unorm");
    let blocks = pad_to(data, fmt.compressed_size(w as usize, h as usize));
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    fmt.decompress(&blocks, w as usize, h as usize, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal-but-complete BLP2 header (magic..=mip_sizes[16], `HEADER_SIZE` bytes).
    fn header(
        compression: u8,
        alpha_bits: u8,
        alpha_type: u8,
        has_mipmaps: u8,
        width: u32,
        height: u32,
        offsets: [u32; 16],
        sizes: [u32; 16],
    ) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(b"BLP2");
        b[4..8].copy_from_slice(&1u32.to_le_bytes()); // content: 1 = Direct
        b[8] = compression;
        b[9] = alpha_bits;
        b[10] = alpha_type;
        b[11] = has_mipmaps;
        b[12..16].copy_from_slice(&width.to_le_bytes());
        b[16..20].copy_from_slice(&height.to_le_bytes());
        for (i, o) in offsets.iter().enumerate() {
            b[20 + i * 4..24 + i * 4].copy_from_slice(&o.to_le_bytes());
        }
        for (i, s) in sizes.iter().enumerate() {
            b[84 + i * 4..88 + i * 4].copy_from_slice(&s.to_le_bytes());
        }
        b
    }

    #[test]
    fn corrupt_header_huge_dims_errors_cleanly() {
        // The fix under test: a header claiming an absurd width/height must fail fast, not size a
        // multi-gigabyte allocation in decode_dxt. No palette/pixel bytes needed — the dimension
        // check runs before either is touched.
        let b = header(3, 0, 0, 0, 65535, 65535, [0; 16], [0; 16]);
        assert!(matches!(
            decode(&b),
            Err(Error::DimensionsTooLarge {
                width: 65535,
                height: 65535
            })
        ));
        // Right at the cap is fine (still fails later for lack of palette/pixel bytes, but not on
        // the dimension check); just over the cap on either axis alone is rejected.
        let b = header(3, 0, 0, 0, MAX_DIM + 1, 4, [0; 16], [0; 16]);
        assert!(matches!(decode(&b), Err(Error::DimensionsTooLarge { .. })));
        let b = header(3, 0, 0, 0, 4, MAX_DIM + 1, [0; 16], [0; 16]);
        assert!(matches!(decode(&b), Err(Error::DimensionsTooLarge { .. })));
    }

    #[test]
    fn truncated_header_errors_cleanly() {
        // Shorter than HEADER_SIZE, even with a valid magic, is rejected before any field read.
        let mut b = vec![0u8; 10];
        b[0..4].copy_from_slice(b"BLP2");
        assert!(matches!(decode(&b), Err(Error::NotBlp2)));
        assert!(matches!(decode(&[]), Err(Error::NotBlp2)));
        assert!(matches!(decode(b"not a blp at all"), Err(Error::NotBlp2)));
    }

    /// Build a DXT-compressed (compression 2) BLP with a real mip chain: 8x8 down to 1x1, so the
    /// tail levels are the sub-block ones that make block handling interesting.
    fn dxt_blp(alpha_type: u8, block_bytes: usize) -> Vec<u8> {
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        let mut payload = Vec::new();
        let base = (HEADER_SIZE + PALETTE_SIZE) as u32;
        // 8x8, 4x4, 2x2, 1x1 — mip_chain_count(8, 8, true) == 3, so both entry points read the
        // first THREE levels (8x8, 4x4, 2x2); the 1x1 is stored but past the authored count.
        for (i, (w, h)) in [(8u32, 8u32), (4, 4), (2, 2), (1, 1)]
            .into_iter()
            .enumerate()
        {
            let blocks = w.div_ceil(4) as usize * h.div_ceil(4) as usize;
            let n = blocks * block_bytes;
            offsets[i] = base + payload.len() as u32;
            sizes[i] = n as u32;
            // Deterministic non-trivial block payload so a byte swap would show.
            payload.extend((0..n).map(|k| (k as u32 * 37 + i as u32 * 11) as u8));
        }
        let mut b = header(2, 8, alpha_type, 1, 8, 8, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend_from_slice(&payload);
        b
    }

    /// The load-bearing equivalence for the native upload path: `decode_native` must hand back the
    /// file's blocks *verbatim*, and running those blocks through the codec must reproduce exactly
    /// what `decode` produces. If these ever diverge, a GPU-uploaded texture stops matching what
    /// every CPU-side consumer sees.
    #[test]
    fn native_blocks_are_verbatim_and_decode_to_the_same_pixels() {
        for (alpha_type, texels, block_bytes) in [
            (0u8, BlpTexels::Bc1, 8usize),
            (1, BlpTexels::Bc2, 16),
            (7, BlpTexels::Bc3, 16),
            (2, BlpTexels::Bc1, 8), // stale alpha_type byte falls back to DXT1
        ] {
            let b = dxt_blp(alpha_type, block_bytes);
            let native = decode_native(&b).expect("valid DXT BLP decodes natively");
            let decoded = decode(&b).expect("valid DXT BLP decodes to pixels");

            assert_eq!(native.texels, texels, "alpha_type {alpha_type}");
            assert!(native.texels.is_block_compressed());
            assert_eq!(native.mips.len(), decoded.mips.len());
            assert_eq!(native.mip_chain_count(), decoded.mip_chain_count());

            for (level, (n, d)) in native.mips.iter().zip(&decoded.mips).enumerate() {
                assert_eq!((n.width, n.height), (d.width, d.height));
                // Every level is exactly its full block size — including the 2x2 and 1x1 tails,
                // which occupy one whole block each.
                assert_eq!(
                    n.bytes.len(),
                    texels.level_bytes(n.width, n.height),
                    "level {level} block size"
                );
                // Verbatim: the bytes are the file's, not re-encoded.
                let off = native_level_offset(&b, level);
                assert_eq!(
                    &n.bytes[..],
                    &b[off..off + n.bytes.len()],
                    "level {level} must be the file's own blocks"
                );
                // And they decode to exactly what the RGBA path produced.
                let fmt = texels.codec().unwrap();
                let mut out = vec![0u8; (n.width as usize) * (n.height as usize) * 4];
                fmt.decompress(&n.bytes, n.width as usize, n.height as usize, &mut out);
                assert_eq!(out, d.rgba, "level {level} pixels must match decode()");
            }
        }
    }

    /// Where level `i`'s payload starts, read back out of the header we built.
    fn native_level_offset(blp: &[u8], level: usize) -> usize {
        u32::from_le_bytes(blp[20 + level * 4..24 + level * 4].try_into().unwrap()) as usize
    }

    /// Raw1/Raw3 have no block form, so the native path decodes them and says so — a caller that
    /// switches on `texels` gets a correct `Rgba8Unorm` upload rather than garbage.
    #[test]
    fn native_reports_rgba_for_the_shapes_with_no_block_form() {
        let pixel_offset = (HEADER_SIZE + PALETTE_SIZE) as u32;
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = pixel_offset;
        sizes[0] = 2 * 2 * 4;
        let mut b = header(3, 8, 0, 0, 2, 2, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend_from_slice(&[
            10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
        ]);

        let native = decode_native(&b).expect("raw3 decodes natively");
        assert_eq!(native.texels, BlpTexels::Rgba8Unorm);
        assert!(!native.texels.is_block_compressed());
        assert_eq!(native.mips[0].bytes, decode(&b).unwrap().mips[0].rgba);
        assert_eq!(
            native.mips[0].bytes.len(),
            BlpTexels::Rgba8Unorm.level_bytes(2, 2)
        );
    }

    /// A level the header under-stores is completed from the bytes that FOLLOW it in the file —
    /// the reference's upload (`0x59f4e6`–`0x59f515`: `bpp·max(4,w)·max(4,h)/8` bytes read from
    /// the level's file offset) — so the concatenated chain a caller uploads is always exactly
    /// the size wgpu expects, and it is the file's bytes, not zeros. Zeros were the bug: an
    /// all-zero BC block is black.
    #[test]
    fn a_short_level_is_completed_from_the_bytes_that_follow_it() {
        let mut b = dxt_blp(0, 8);
        // Level 2 is the 2x2 tail — one whole BC1 block. Shrink its recorded size to 3 bytes; the
        // 1x1 level's 8 bytes follow it in the file.
        let short = 3u32;
        b[84 + 2 * 4..88 + 2 * 4].copy_from_slice(&short.to_le_bytes());
        let off = native_level_offset(&b, 2);
        let native = decode_native(&b).expect("short tail still decodes");
        let tail = native.mips.last().unwrap();
        assert_eq!((tail.width, tail.height), (2, 2));
        assert_eq!(tail.bytes.len(), 8, "one whole BC1 block");
        assert_eq!(
            &tail.bytes[..],
            &b[off..off + 8],
            "the level is the file's next 8 bytes, the header's 3 and the 1x1's first 5"
        );
        assert!(
            tail.bytes[short as usize..].iter().any(|&x| x != 0),
            "nothing was zero-filled — that was B358's black"
        );
        // And decode() reads the same span — the two paths complete identically.
        let decoded = decode(&b).unwrap();
        assert_eq!(decoded.mips.len(), native.mips.len());
        let mut out = vec![0u8; 2 * 2 * 4];
        texpresso::Format::Bc1.decompress(&tail.bytes, 2, 2, &mut out);
        assert_eq!(out, decoded.mips.last().unwrap().rgba);
    }

    /// **The shipped shape of the bug**, in miniature: a DXT3 texture taller than it is wide,
    /// whose 2×8 level the encoder stores as ONE block (`max(1, (2/4)·(8/4))`) where the block
    /// grid needs two. The reference uploads `8·max(4,2)·max(4,8)/8 = 32` bytes from that level's
    /// offset — the stored block plus the 1×4 level's block after it. `RainDrop01.blp` is this at
    /// 16×128: its 2×16 and 1×8 levels store 16 bytes each where the grid needs 64 and 32.
    #[test]
    fn a_sub_block_dxt3_level_spans_into_its_successors_as_the_reference_copies_it() {
        // 4x16 DXT3: levels 4x16 (4 blocks), 2x8 (grid: 2 blocks; STORED: 1), 1x4 (1 block),
        // 1x2 (1 block). mip_chain_count(4, 16, true) = 4, so all four are read.
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        let mut payload = Vec::new();
        let base = (HEADER_SIZE + PALETTE_SIZE) as u32;
        for (i, (blocks_stored, fill)) in [(4usize, 0x10u8), (1, 0x20), (1, 0x30), (1, 0x40)]
            .into_iter()
            .enumerate()
        {
            offsets[i] = base + payload.len() as u32;
            sizes[i] = (blocks_stored * 16) as u32;
            payload.extend(std::iter::repeat_n(fill, blocks_stored * 16));
        }
        let mut b = header(2, 8, 1, 1, 4, 16, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend_from_slice(&payload);

        let native = decode_native(&b).expect("decodes");
        assert_eq!(native.texels, BlpTexels::Bc2);
        assert_eq!(native.mips.len(), 4);
        let l1 = &native.mips[1];
        assert_eq!((l1.width, l1.height), (2, 8));
        assert_eq!(l1.bytes.len(), 32, "two BC2 blocks for a 1x2 block grid");
        assert!(
            l1.bytes[..16].iter().all(|&x| x == 0x20),
            "first block: the level's own"
        );
        assert!(
            l1.bytes[16..].iter().all(|&x| x == 0x30),
            "second block: the 1x4 level's, which follows it in the file — never zeros"
        );
        // The levels that ARE a whole block are untouched by the rule.
        assert!(native.mips[2].bytes.iter().all(|&x| x == 0x30));
        assert!(native.mips[3].bytes.iter().all(|&x| x == 0x40));
        // A short level the FILE ends inside is the one case left for the zero pad: the same
        // texture authored with only its first two levels, the file ending right after the 2x8
        // level's single stored block — the completion has nothing to read.
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = base;
        sizes[0] = 64;
        offsets[1] = base + 64;
        sizes[1] = 16;
        let mut b = header(2, 8, 1, 1, 4, 16, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend(std::iter::repeat_n(0x10u8, 64));
        b.extend(std::iter::repeat_n(0x20u8, 16));
        let native = decode_native(&b).expect("still decodes what it has");
        assert_eq!(native.mips.len(), 2, "the chain ends where the file does");
        let l1 = &native.mips[1];
        assert_eq!(l1.bytes.len(), 32);
        assert!(l1.bytes[..16].iter().all(|&x| x == 0x20));
        assert!(
            l1.bytes[16..].iter().all(|&x| x == 0),
            "past EOF there is only the pad"
        );
    }

    #[test]
    fn tiny_raw3_decodes_expected_pixels() {
        // A 2x2 uncompressed BGRA8 (Raw3) image, no mipmaps: the smallest concrete shape decode()
        // handles. Pixel data sits right after the header + 256-entry BGRA palette.
        let pixel_offset = (HEADER_SIZE + PALETTE_SIZE) as u32;
        let pixel_size = 2 * 2 * 4;
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = pixel_offset;
        sizes[0] = pixel_size;
        let mut b = header(3, 8, 0, 0, 2, 2, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0); // palette bytes: unused by Raw3, present anyway
                                                 // Four BGRA8 pixels.
        let bgra: &[u8] = &[
            10, 20, 30, 40, // B G R A
            50, 60, 70, 80, //
            90, 100, 110, 120, //
            130, 140, 150, 160,
        ];
        b.extend_from_slice(bgra);

        let decoded = decode(&b).expect("valid tiny BLP decodes");
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.mip_chain_count(), 0); // no mipmaps: chain formula reports 0
        assert_eq!(decoded.mips.len(), 1); // level 0 is still decoded regardless

        let mip = &decoded.mips[0];
        assert_eq!(mip.width, 2);
        assert_eq!(mip.height, 2);
        // BGRA -> RGBA per pixel.
        assert_eq!(
            mip.rgba,
            vec![
                30, 20, 10, 40, // R G B A
                70, 60, 50, 80, //
                110, 100, 90, 120, //
                150, 140, 130, 160,
            ]
        );
    }
}
