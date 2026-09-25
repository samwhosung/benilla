//! A BLP2 texture decoder for WoW 1.12.1, which ships Direct content in three shapes:
//! - Raw1: a 256-entry BGRA palette and 1-byte indices, then a 0, 1, 4 or 8-bit alpha block.
//! - Raw3: uncompressed BGRA8.
//! - DXTC: S3TC blocks, `alpha_type` selecting DXT1 (0), DXT3 (1) or DXT5 (7).
//!
//! [`decode`] yields gamma-encoded `Rgba8Unorm`, never linearized; [`decode_native`] keeps the
//! DXTC blocks, which the reference uploads untouched (`glCompressedTexImage2DARB` in `0x59f270`).

use benilla_bytes::ByteExt;

/// A BLP decode error: anything outside the 1.12 BLP2 envelope.
#[derive(Debug)]
pub enum Error {
    NotBlp2,
    Truncated(&'static str),
    UnknownCompression(u8),
    BadColorMap,
    OutOfBounds {
        level: usize,
    },
    /// The header's dimensions exceed [`MAX_DIM`], which no real 1.12 BLP comes near.
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

/// A decoded BLP, `Rgba8Unorm` per mip; `mips[0]` is always present.
pub struct DecodedBlp {
    pub width: u32,
    pub height: u32,
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
    /// The authored chain length: `max(log2 w, log2 h)` with mipmaps, else 0 (`mips` still holds
    /// level 0).
    pub fn mip_chain_count(&self) -> usize {
        self.mip_chain_count
    }
}

const HEADER_SIZE: usize = 148; // magic..=mip_sizes[16]
const PALETTE_SIZE: usize = 256 * 4;

/// A cap on the header's dimensions, which size the buffers (1.12 art stops at 1024×1024).
const MAX_DIM: u32 = 8192;

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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlpTexels {
    /// Decoded pixels, from Raw1 or Raw3, which have no block form.
    Rgba8Unorm,
    /// DXT1 blocks (8 bytes per 4x4), `alpha_type` 0. 1-bit alpha at most.
    Bc1,
    /// DXT3 blocks (16 bytes per 4x4), `alpha_type` 1. Explicit 4-bit alpha.
    Bc2,
    /// DXT5 blocks (16 bytes per 4x4), `alpha_type` 7. Interpolated alpha.
    Bc3,
}

impl BlpTexels {
    /// Whether this is a block form, uploadable verbatim.
    pub fn is_block_compressed(self) -> bool {
        !matches!(self, Self::Rgba8Unorm)
    }

    /// The bytes a `width x height` level takes; block forms round up to whole 4x4 blocks.
    pub fn level_bytes(self, width: u32, height: u32) -> usize {
        match self {
            Self::Rgba8Unorm => (width as usize) * (height as usize) * 4,
            Self::Bc1 | Self::Bc2 | Self::Bc3 => {
                let blocks = width.div_ceil(4) as usize * height.div_ceil(4) as usize;
                blocks * if self == Self::Bc1 { 8 } else { 16 }
            }
        }
    }

    fn codec(self) -> Option<texpresso::Format> {
        match self {
            Self::Rgba8Unorm => None,
            Self::Bc1 => Some(texpresso::Format::Bc1),
            Self::Bc2 => Some(texpresso::Format::Bc2),
            Self::Bc3 => Some(texpresso::Format::Bc3),
        }
    }
}

/// A BLP with its levels in the form the file stores them.
pub struct NativeBlp {
    pub width: u32,
    pub height: u32,
    /// What every entry of `mips` holds.
    pub texels: BlpTexels,
    /// One buffer per level, level 0 first; never empty.
    pub mips: Vec<NativeMip>,
    mip_chain_count: usize,
}

/// One level of a [`NativeBlp`].
pub struct NativeMip {
    pub width: u32,
    pub height: u32,
    /// Block bytes or RGBA8 pixels, per [`NativeBlp::texels`], exactly [`BlpTexels::level_bytes`]
    /// long: a short level is completed from the bytes after it, as the reference reads it.
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
    chain: usize,
}

/// Parse and bounds-check the BLP2 header, palette and level table.
fn parse_header(bytes: &[u8]) -> Result<Header<'_>> {
    if bytes.len() < HEADER_SIZE || &bytes[0..4] != b"BLP2" {
        return Err(Error::NotBlp2);
    }
    // `content` @4 is unread: 1.12 ships only Direct (1).
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

    // Checked before anything sizes a buffer from them; zero passes.
    if width > MAX_DIM || height > MAX_DIM {
        return Err(Error::DimensionsTooLarge { width, height });
    }

    // A 256-entry BGRA palette always follows the header; only Raw1 reads it.
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
    /// Level `level`'s stored bytes, or `None` where the chain ends before the formula's count.
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

    /// DXTC level `level` as the reference reads it: `bpp·max(4,w)·max(4,h)/8` bytes from the
    /// level's offset, never the header's size (`0x5a8555` points each level into the file;
    /// OpenGL sizes the read at `0x59f4e6`–`0x59f515`, Direct3D copies `max(4,w)·max(4,h)` bytes
    /// via `0x5a5780`). The encoder stores `max(1, (w/4)·(h/4))` blocks, one for a 2×16 level
    /// whose grid needs four, so a short level takes the next levels' bytes. Deviation: past the
    /// file's end [`pad_to`] zero-fills, because the reference reads stale buffer bytes there.
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

    /// The DXTC form by `alpha_type`. Any other value (a stale `2` on alpha-less particle atlases)
    /// is DXT1, not an error: `alpha_bits` governs alpha.
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
    // Level 0 always, then deeper levels up to the chain count while the file carries them.
    let n = h.chain.clamp(1, 16);

    let mut mips = Vec::with_capacity(n);
    for level in 0..n {
        let (lw, lh) = level_size(h.width, h.height, level);
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

/// Decode a BLP2 texture keeping its DXTC blocks verbatim, the form the reference uploads; Raw1
/// and Raw3 decode as in [`decode`].
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

/// Decode one block-compressed level to RGBA8, for a device without BC support;
/// [`BlpTexels::Rgba8Unorm`] returns `bytes` unchanged.
pub fn decode_level(texels: BlpTexels, width: u32, height: u32, bytes: &[u8]) -> Vec<u8> {
    let Some(fmt) = texels.codec() else {
        return bytes.to_vec();
    };
    let blocks = pad_to(bytes, fmt.compressed_size(width as usize, height as usize));
    let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
    fmt.decompress(&blocks, width as usize, height as usize, &mut out);
    out
}

/// `data` zero-padded or truncated to exactly `need` bytes. A zero BC block is black at alpha 0,
/// so a DXTC level is completed from the file first (`Header::dxt_level_span`).
fn pad_to(data: &[u8], need: usize) -> Vec<u8> {
    let mut out = vec![0u8; need];
    let n = data.len().min(need);
    out[..n].copy_from_slice(&data[..n]);
    out
}

/// Palettized: 1-byte indices into the BGRA palette, then a packed `alpha_bits` alpha block.
fn decode_raw1(palette: &[u8], data: &[u8], w: u32, h: u32, alpha_bits: u32) -> Result<Vec<u8>> {
    let px = (w as usize) * (h as usize);
    let indices = data.get(..px).ok_or(Error::Truncated("raw1 indices"))?;
    let alpha = &data[px..];
    let mut out = vec![0u8; px * 4];
    for i in 0..px {
        let ci = indices[i] as usize;
        let p = ci * 4;
        // The palette is BGRA.
        out[i * 4] = palette[p + 2]; // R
        out[i * 4 + 1] = palette[p + 1]; // G
        out[i * 4 + 2] = palette[p]; // B
        out[i * 4 + 3] = alpha_at(alpha, i, alpha_bits);
    }
    Ok(out)
}

/// Pixel `i`'s alpha from a block of 1, 4 or 8 bits per pixel; 0 bits is opaque.
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

/// DXTC to RGBA8; a level the file ends inside is zero-padded first.
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

    /// A BLP2 header, magic to `mip_sizes[16]`.
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
        // No palette or pixels: the dimension check runs first.
        let b = header(3, 0, 0, 0, 65535, 65535, [0; 16], [0; 16]);
        assert!(matches!(
            decode(&b),
            Err(Error::DimensionsTooLarge {
                width: 65535,
                height: 65535
            })
        ));
        let b = header(3, 0, 0, 0, MAX_DIM + 1, 4, [0; 16], [0; 16]);
        assert!(matches!(decode(&b), Err(Error::DimensionsTooLarge { .. })));
        let b = header(3, 0, 0, 0, 4, MAX_DIM + 1, [0; 16], [0; 16]);
        assert!(matches!(decode(&b), Err(Error::DimensionsTooLarge { .. })));
    }

    #[test]
    fn truncated_header_errors_cleanly() {
        let mut b = vec![0u8; 10];
        b[0..4].copy_from_slice(b"BLP2");
        assert!(matches!(decode(&b), Err(Error::NotBlp2)));
        assert!(matches!(decode(&[]), Err(Error::NotBlp2)));
        assert!(matches!(decode(b"not a blp at all"), Err(Error::NotBlp2)));
    }

    /// A DXTC BLP (compression 2) with an 8x8 to 1x1 chain, so the tail levels are sub-block.
    fn dxt_blp(alpha_type: u8, block_bytes: usize) -> Vec<u8> {
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        let mut payload = Vec::new();
        let base = (HEADER_SIZE + PALETTE_SIZE) as u32;
        // The chain count for 8x8 is 3, so the stored 1x1 is past it and unread.
        for (i, (w, h)) in [(8u32, 8u32), (4, 4), (2, 2), (1, 1)]
            .into_iter()
            .enumerate()
        {
            let blocks = w.div_ceil(4) as usize * h.div_ceil(4) as usize;
            let n = blocks * block_bytes;
            offsets[i] = base + payload.len() as u32;
            sizes[i] = n as u32;
            // A non-trivial payload, so a byte swap would show.
            payload.extend((0..n).map(|k| (k as u32 * 37 + i as u32 * 11) as u8));
        }
        let mut b = header(2, 8, alpha_type, 1, 8, 8, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend_from_slice(&payload);
        b
    }

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
                assert_eq!(
                    n.bytes.len(),
                    texels.level_bytes(n.width, n.height),
                    "level {level} block size"
                );
                let off = native_level_offset(&b, level);
                assert_eq!(
                    &n.bytes[..],
                    &b[off..off + n.bytes.len()],
                    "level {level} must be the file's own blocks"
                );
                let fmt = texels.codec().unwrap();
                let mut out = vec![0u8; (n.width as usize) * (n.height as usize) * 4];
                fmt.decompress(&n.bytes, n.width as usize, n.height as usize, &mut out);
                assert_eq!(out, d.rgba, "level {level} pixels must match decode()");
            }
        }
    }

    /// Where `level`'s payload starts, read back from the header.
    fn native_level_offset(blp: &[u8], level: usize) -> usize {
        u32::from_le_bytes(blp[20 + level * 4..24 + level * 4].try_into().unwrap()) as usize
    }

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

    /// The reference reads `bpp·max(4,w)·max(4,h)/8` bytes from the level's offset
    /// (`0x59f4e6`–`0x59f515`), whatever the header records.
    #[test]
    fn a_short_level_is_completed_from_the_bytes_that_follow_it() {
        let mut b = dxt_blp(0, 8);
        // Level 2, the 2x2 tail, is one BC1 block: record it as 3 bytes, the 1x1's 8 following.
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
            "nothing is zero-filled, which would draw black"
        );
        // `decode` reads the same span.
        let decoded = decode(&b).unwrap();
        assert_eq!(decoded.mips.len(), native.mips.len());
        let mut out = vec![0u8; 2 * 2 * 4];
        texpresso::Format::Bc1.decompress(&tail.bytes, 2, 2, &mut out);
        assert_eq!(out, decoded.mips.last().unwrap().rgba);
    }

    /// A 2×8 DXT3 level is stored as one block (`max(1, (2/4)·(8/4))`) where the grid needs two;
    /// the reference reads `8·max(4,2)·max(4,8)/8 = 32` bytes, taking the 1×4 level's block too.
    /// `RainDrop01.blp` ships this shape at 16×128.
    #[test]
    fn a_sub_block_dxt3_level_spans_into_its_successors_as_the_reference_copies_it() {
        // 4x16 DXT3: 4x16 (4 blocks), 2x8 (grid 2, stored 1), 1x4 (1), 1x2 (1); a chain of 4.
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
        // Whole-block levels are untouched.
        assert!(native.mips[2].bytes.iter().all(|&x| x == 0x30));
        assert!(native.mips[3].bytes.iter().all(|&x| x == 0x40));
        // A level the file ends inside is zero-padded: the same texture, two levels, ending after
        // the 2x8's one stored block.
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
        // A 2x2 Raw3 image, no mipmaps; the pixels follow the header and palette.
        let pixel_offset = (HEADER_SIZE + PALETTE_SIZE) as u32;
        let pixel_size = 2 * 2 * 4;
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = pixel_offset;
        sizes[0] = pixel_size;
        let mut b = header(3, 8, 0, 0, 2, 2, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0); // the palette, unused by Raw3
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
        assert_eq!(decoded.mip_chain_count(), 0); // no mipmaps
        assert_eq!(decoded.mips.len(), 1); // level 0 is still decoded

        let mip = &decoded.mips[0];
        assert_eq!(mip.width, 2);
        assert_eq!(mip.height, 2);
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
