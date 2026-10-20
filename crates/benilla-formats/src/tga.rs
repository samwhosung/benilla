//! TGA decoding, the other half of the reference's UI texture lookup: `TextureCreate` tries
//! `{".tga", ".blp"}` (`0x835248`), and while 1.12 ships its UI art as BLP, addons ship TGAs. The
//! reference's `0x5a3a30` dispatches on `imageType`: 1 and 9 (colour-mapped, raw or RLE) to
//! `0x5a39d0` through the palette at `0x5a3820`, 2 to `0x5a3c80`, 10 to `0x5a3d70`.
//!
//! Deviation: grayscale types 3 and 11 decode where the reference refuses them, because no shipped
//! or addon file is one and removing working code would buy nothing.
//!
//! Deviation: colour maps of 15, 16 and 32 bits decode at the entry width the file declares, where
//! the reference has no defined result to copy for 15 and 16 bits and only a garble for 32. It
//! sizes an entry as `(min(bits / 3, 8) · 3) >> 3` bytes (`0x5a3b40`): 3 for 24 and 32 bits, 1 for
//! 15 and 16. A 16-bit map is read short and copied one byte a pixel (`0x5a3890`-`0x5a38fa`) into
//! the three-byte-a-pixel buffer `0x5a3820` allocates and never zeroes, so the rest of the colours
//! are heap garbage; a 32-bit map is read a byte an entry short and the pixels start inside it.
//!
//! Deviation: a colour-mapped image whose descriptor has alpha bits decodes as its picture, where
//! the reference has no defined result to copy. `0x5a3820` sizes each pixel as 24 plus the alpha
//! bits (`0x5a382b`-`0x5a383d`, stored at `0x5a3973`) but copies 3-byte entries (`0x5a38f3`), then
//! `0x5a3b80` moves `(bpp + 7) >> 3` bytes a pixel (`0x5a3c00`-`0x5a3c13`): each pixel after the
//! first takes its bytes from its neighbours, and the last read heap that was never written.
//!
//! Deviation: a truecolour image whose depth less its alpha bits is 24, but whose depth is not 24
//! or 32, is refused where the reference crashes. The check `0x5a37e0` admits it (25 to 31 bits
//! with 1 to 7 alpha bits, 33 to 39 with 9 to 15), then `0x5a41f0` returns 0 for a depth that is
//! not 32, and the copy at `0x4492c4` reads through that null pointer: there is no result to match.
//!
//! [`tga_to_rgba`] checks the header before the pixels, so it can serve as the fallback after a
//! failed `BLP2` sniff: TGA has no magic.

use anyhow::{bail, Result};

/// A colour-map entry's bytes as the reference sizes it (`0x5a3b40`, `0x5a36e1`-`0x5a3760`).
fn reference_map_entry_bytes(bits: u8) -> usize {
    (usize::from(bits) / 3).min(8) * 3 / 8
}

/// How much of a file [`tga_texture_size`] needs: it answers the same for this many leading bytes
/// as for the whole, since it asks only that the 18-byte header, an id of at most 255 bytes and one
/// byte after it are there.
pub const TGA_GATE_PREFIX: usize = 18 + 255 + 1;

/// The size `TextureCreate`'s TGA loader (`0x44a140`) makes a texture at, or `None` when that load
/// fails and the next candidate is tried. The loader returns the texture once the header opens
/// (`0x5a3660`: the 18 bytes, then the id and the colour map the header declares, each read
/// succeeding on any byte it gets, `0x64853b`-`0x64854a`) and `0x448450` makes the device texture
/// at the header's size, refusing a width or height that is not a power of two below 1024
/// (`0x4484ae`-`0x4484d0`). The pixels decode later, in `0x44a260`: a file [`tga_to_rgba`] refuses
/// still has this texture, filled opaque white (`0x44a2eb`). A zero size passes the gate and draws
/// nothing.
pub fn tga_texture_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = bytes.get(..18)?;
    let id_length = usize::from(header[0]);
    let map_bytes = if header[1] != 0 {
        reference_map_entry_bytes(header[7])
            * usize::from(u16::from_le_bytes([header[5], header[6]]))
    } else {
        0
    };
    let after_header = bytes.len() - 18;
    if id_length > 0 && after_header == 0 {
        return None;
    }
    if map_bytes > 0 && after_header.saturating_sub(id_length) == 0 {
        return None;
    }
    let width = u32::from(u16::from_le_bytes([header[12], header[13]]));
    let height = u32::from(u16::from_le_bytes([header[14], header[15]]));
    // `test ebx, ebx-1` lets 0 through (`0x44849d`), so a zero size passes.
    let fits = |n: u32| n & n.wrapping_sub(1) == 0 && n < 1024;
    (fits(width) && fits(height)).then_some((width, height))
}

/// Decode a TGA to RGBA8 `(width, height, pixels)`, rows top-down like [`crate::blp_to_rgba`].
pub fn tga_to_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let Some(header) = bytes.get(..18) else {
        bail!("TGA: truncated header ({} bytes)", bytes.len());
    };
    let id_length = header[0] as usize;
    let color_map_type = header[1];
    let image_type = header[2];
    let color_map_origin = u16::from_le_bytes([header[3], header[4]]) as usize;
    let color_map_len = u16::from_le_bytes([header[5], header[6]]) as usize;
    let color_map_entry_bits = header[7] as usize;
    let width = u16::from_le_bytes([header[12], header[13]]) as u32;
    let height = u16::from_le_bytes([header[14], header[15]]) as u32;
    let bpp = header[16];
    let descriptor = header[17];

    let rle = match image_type {
        1..=3 => false,
        9..=11 => true,
        t => bail!("TGA: unknown image type {t}"),
    };
    let paletted = matches!(image_type, 1 | 9);
    let grayscale = matches!(image_type, 3 | 11);
    // The descriptor's low nibble is the alpha bit count. A 24-bit image declaring 8 has them
    // cleared on open (`0x5a37c5`), and a truecolour image (types 2 and 10) is read only when its
    // depth less its alpha bits is 24 (`0x5a37e0`, called at `0x5a3ace` and `0x5a3ae9`): a 32-bit
    // image with none fails there (`0x5a3809`), and its texture draws opaque white.
    let alpha_bits = match descriptor & 0xf {
        8 if bpp == 24 => 0,
        bits => bits,
    };
    if matches!(image_type, 2 | 10) && u32::from(bpp) != 24 + u32::from(alpha_bits) {
        bail!(
            "TGA: {bpp}-bit truecolour image with {alpha_bits} alpha bits \
             (depth less alpha bits must be 24)"
        );
    }
    // A colour-mapped pixel is an index, its palette entry a width of its own (`0x5a3b40`). The
    // truecolour depths other than 24 and 32 are refused, the crash pairs by the fourth Deviation.
    let bytes_per_pixel = match (bpp, paletted, grayscale) {
        (8, true, _) => 1,
        (24, false, false) => 3,
        (32, false, false) => 4,
        (8, false, true) => 1,
        _ => bail!("TGA: unsupported depth {bpp} for image type {image_type}"),
    };
    if paletted && color_map_type != 1 {
        bail!("TGA: image type {image_type} is colour-mapped but declares no colour map");
    }
    let map_entry_bytes = color_map_entry_bits.div_ceil(8);
    if paletted && !matches!(color_map_entry_bits, 15 | 16 | 24 | 32) {
        bail!("TGA: unsupported colour-map entry width {color_map_entry_bits}");
    }
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        bail!("TGA: implausible dimensions {width}x{height}");
    }

    // The pixels start after the id field and any colour map, which a colour-mapped image reads
    // on the way (`0x5a3b40`: `entryBytes * colorMapLength + idLength + 0x12`).
    let map_bytes = if color_map_type == 1 {
        color_map_len * map_entry_bytes
    } else {
        0
    };
    let palette = if paletted {
        let Some(p) = bytes.get(18 + id_length..18 + id_length + map_bytes) else {
            bail!("TGA: truncated colour map");
        };
        p
    } else {
        &[][..]
    };
    let data_start = 18 + id_length + map_bytes;
    let Some(data) = bytes.get(data_start..) else {
        bail!("TGA: truncated before pixel data");
    };

    let pixel_count = (width as usize) * (height as usize);
    let mut pixels = Vec::with_capacity(pixel_count * bytes_per_pixel);
    if rle {
        // An RLE packet's bit 7 marks a run of one pixel, else `count` raw pixels follow; `count`
        // is the low 7 bits plus 1.
        let mut at = 0usize;
        while pixels.len() < pixel_count * bytes_per_pixel {
            let Some(&packet) = data.get(at) else {
                bail!("TGA: RLE stream truncated");
            };
            at += 1;
            let count = ((packet & 0x7f) as usize) + 1;
            if packet & 0x80 != 0 {
                let Some(px) = data.get(at..at + bytes_per_pixel) else {
                    bail!("TGA: RLE run truncated");
                };
                at += bytes_per_pixel;
                for _ in 0..count {
                    pixels.extend_from_slice(px);
                }
            } else {
                let n = count * bytes_per_pixel;
                let Some(px) = data.get(at..at + n) else {
                    bail!("TGA: RLE raw packet truncated");
                };
                at += n;
                pixels.extend_from_slice(px);
            }
        }
        pixels.truncate(pixel_count * bytes_per_pixel);
    } else {
        let n = pixel_count * bytes_per_pixel;
        let Some(px) = data.get(..n) else {
            bail!("TGA: pixel data truncated");
        };
        pixels.extend_from_slice(px);
    }

    // BGR(A) or gray → RGBA. Descriptor bit 5 is a top origin, which `0x5a4070` (called with 1
    // from `0x4491f0`) honours by reversing the rows when it is clear; no read-path routine
    // tests bit 4, so the pixels keep their stored order left to right.
    let top_down = descriptor & 0x20 != 0;
    let mut rgba = vec![0u8; pixel_count * 4];
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        for x in 0..width as usize {
            let s = (src_y * width as usize + x) * bytes_per_pixel;
            let d = (y * width as usize + x) * 4;
            if paletted {
                // `0x5a3820`: `palette + (index - firstIndex) * entryBytes`, `firstIndex` being
                // the header's colour-map origin (almost always 0).
                let idx = (pixels[s] as usize).wrapping_sub(color_map_origin);
                let e = idx * map_entry_bytes;
                let Some(entry) = palette.get(e..e + map_entry_bytes) else {
                    bail!("TGA: colour index {} is outside the map", pixels[s]);
                };
                rgba[d..d + 4].copy_from_slice(&match map_entry_bytes {
                    // 15/16-bit entries are `ARRRRRGG GGGBBBBB` little-endian, the top bit alpha
                    // only at 16; the high bits replicate down so a full channel reads 0xff.
                    2 => {
                        let v = u16::from_le_bytes([entry[0], entry[1]]);
                        let c = |shift: u32| {
                            let f = ((v >> shift) & 0x1f) as u8;
                            (f << 3) | (f >> 2)
                        };
                        let a = if color_map_entry_bits == 16 && v & 0x8000 == 0 {
                            0
                        } else {
                            0xff
                        };
                        [c(10), c(5), c(0), a]
                    }
                    4 => [entry[2], entry[1], entry[0], entry[3]],
                    _ => [entry[2], entry[1], entry[0], 0xff],
                });
                continue;
            }
            match bytes_per_pixel {
                1 => {
                    let g = pixels[s];
                    rgba[d..d + 4].copy_from_slice(&[g, g, g, 0xff]);
                }
                3 => {
                    rgba[d..d + 4].copy_from_slice(&[pixels[s + 2], pixels[s + 1], pixels[s], 0xff])
                }
                _ => {
                    rgba[d..d + 4].copy_from_slice(&[
                        pixels[s + 2],
                        pixels[s + 1],
                        pixels[s],
                        pixels[s + 3],
                    ]);
                }
            }
        }
    }
    Ok((width, height, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal colour-mapped TGA: header, palette, index stream.
    fn tga_paletted(
        image_type: u8,
        w: u16,
        h: u16,
        palette: &[u8],
        entry_bits: u8,
        idx: &[u8],
    ) -> Vec<u8> {
        let mut out = vec![0u8; 18];
        out[1] = 1; // colour-map type: present
        out[2] = image_type;
        out[5..7].copy_from_slice(
            &((palette.len() / (entry_bits as usize).div_ceil(8)) as u16).to_le_bytes(),
        );
        out[7] = entry_bits;
        out[12..14].copy_from_slice(&w.to_le_bytes());
        out[14..16].copy_from_slice(&h.to_le_bytes());
        out[16] = 8; // 8-bit indices
        out[17] = 0x20; // top-origin, so the test reads in file order
        out.extend_from_slice(palette);
        out.extend_from_slice(idx);
        out
    }

    /// Types 1 and 9, which the reference sends to `0x5a39d0`, in the addons' shape: 8-bit
    /// indices into a 24-bit palette.
    #[test]
    fn colour_mapped_images_decode_through_the_palette() {
        // Palette as BGR: entry 0 red, entry 1 green, entry 2 blue.
        let pal = [
            0, 0, 255, /* red */ 0, 255, 0, /* green */ 255, 0, 0, /* blue */
        ];
        let raw = tga_paletted(1, 2, 2, &pal, 24, &[0, 1, 2, 0]);
        let (w, h, rgba) = tga_to_rgba(&raw).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(
            rgba,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 0, 0, 255,
            ]
        );

        // Type 9 is the same image RLE'd; the runs are over index bytes, not expanded pixels.
        let rle = tga_paletted(9, 2, 2, &pal, 24, &[0x02, 0, 1, 2, 0x80, 0]);
        assert_eq!(tga_to_rgba(&rle).unwrap().2, rgba);

        // An index outside the map is an error, not a black pixel.
        let bad = tga_paletted(1, 1, 1, &pal, 24, &[9]);
        assert!(tga_to_rgba(&bad)
            .unwrap_err()
            .to_string()
            .contains("outside the map"));
    }

    /// The module's second Deviation: maps of 15, 16 and 32 bits decode at the width the file
    /// declares. The reference sizes their entries 1, 1 and 3 bytes (`0x5a3b40`).
    #[test]
    fn colour_maps_of_15_16_and_32_bits_decode_at_their_own_width() {
        // 16 bits, `ARRRRRGG GGGBBBBB` little-endian: opaque red, opaque green, clear blue.
        let pal16 = [0x00, 0xfc, 0xe0, 0x83, 0x1f, 0x00];
        let raw = tga_paletted(1, 3, 1, &pal16, 16, &[0, 1, 2]);
        assert_eq!(
            tga_to_rgba(&raw).unwrap().2,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 0]
        );
        // 15 bits: the top bit is no alpha, so the blue is opaque.
        let raw = tga_paletted(1, 3, 1, &pal16, 15, &[0, 1, 2]);
        assert_eq!(
            tga_to_rgba(&raw).unwrap().2,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255]
        );
        // 32 bits, BGRA: byte 3 is alpha.
        let pal32 = [0, 0, 255, 255, 0, 255, 0, 128];
        let raw = tga_paletted(9, 2, 1, &pal32, 32, &[0x01, 0, 1]);
        assert_eq!(
            tga_to_rgba(&raw).unwrap().2,
            vec![255, 0, 0, 255, 0, 255, 0, 128]
        );
    }

    /// The module's third Deviation: alpha bits in a colour-mapped image's descriptor change
    /// nothing, where the reference garbles it (`0x5a3820`, `0x5a3b80`).
    #[test]
    fn colour_mapped_images_with_alpha_bits_decode_as_the_picture() {
        let pal = [0, 0, 255, 0, 255, 0, 255, 0, 0];
        let plain = tga_paletted(1, 3, 1, &pal, 24, &[0, 1, 2]);
        let want = tga_to_rgba(&plain).unwrap().2;
        assert_eq!(want, vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255]);
        for descriptor in [0x28, 0x08, 0x21, 0x2f] {
            let mut raw = plain.clone();
            raw[17] = descriptor;
            assert_eq!(
                tga_to_rgba(&raw).unwrap().2,
                want,
                "descriptor {descriptor:#04x}"
            );
        }
        // The run-length type is expanded the same way.
        let mut rle = tga_paletted(9, 3, 1, &pal, 24, &[0x02, 0, 1, 2]);
        rle[17] = 0x28;
        assert_eq!(tga_to_rgba(&rle).unwrap().2, want);
    }

    /// A minimal TGA: an 18-byte header and pixels.
    fn tga(image_type: u8, w: u16, h: u16, bpp: u8, descriptor: u8, data: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 18];
        out[2] = image_type;
        out[12..14].copy_from_slice(&w.to_le_bytes());
        out[14..16].copy_from_slice(&h.to_le_bytes());
        out[16] = bpp;
        out[17] = descriptor;
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn uncompressed_32bit_bottom_origin_flips_and_swizzles() {
        // Bottom-up rows, as BGRA: blue, green at the bottom; red, half-alpha white on top.
        let px = [
            255, 0, 0, 255, /* blue */ 0, 255, 0, 255, /* green */
            0, 0, 255, 255, /* red */ 255, 255, 255, 128, /* white a=128 */
        ];
        let (w, h, rgba) = tga_to_rgba(&tga(2, 2, 2, 32, 8, &px)).unwrap();
        assert_eq!((w, h), (2, 2));
        // Top-down output, as RGBA: red, white, then blue, green.
        assert_eq!(
            rgba,
            vec![
                255, 0, 0, 255, 255, 255, 255, 128, //
                0, 0, 255, 255, 0, 255, 0, 255,
            ]
        );
    }

    #[test]
    fn uncompressed_24bit_top_origin_gets_opaque_alpha() {
        let px = [255, 0, 0, /* blue */ 0, 0, 255 /* red */];
        let (w, h, rgba) = tga_to_rgba(&tga(2, 2, 1, 24, 0x20, &px)).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgba, vec![0, 0, 255, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn rle_run_and_raw_packets_expand() {
        // 4×1 top-origin: a run of 3 red and a raw packet of 1 green (BGR, 24-bit).
        let data = [0x82, 0, 0, 255, 0x00, 0, 255, 0];
        let (w, _, rgba) = tga_to_rgba(&tga(10, 4, 1, 24, 0x20, &data)).unwrap();
        assert_eq!(w, 4);
        assert_eq!(
            rgba,
            vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, //
                0, 255, 0, 255,
            ]
        );
    }

    #[test]
    fn grayscale_replicates_channels() {
        let (_, _, rgba) = tga_to_rgba(&tga(3, 1, 1, 8, 0x20, &[7])).unwrap();
        assert_eq!(rgba, vec![7, 7, 7, 255]);
    }

    /// The reference orients a TGA through descriptor bit 5 alone (`0x5a4070`); no read-path
    /// routine tests bit 4 (`0x5a3660`, `0x5a3a30`, `0x5a3c80`, `0x5a3d70`, `0x5a4070`): only the
    /// writer's builder sets it (`0x5a43c0`).
    #[test]
    fn descriptor_bit_4_does_not_mirror_the_rows() {
        // 2×2, 24-bit, blue and red over green and white.
        let px = [
            255, 0, 0, /* blue */ 0, 0, 255, /* red */
            0, 255, 0, /* green */ 255, 255, 255, /* white */
        ];
        for origin in [0x00, 0x20] {
            let plain = tga_to_rgba(&tga(2, 2, 2, 24, origin, &px)).unwrap();
            let bit4 = tga_to_rgba(&tga(2, 2, 2, 24, origin | 0x10, &px)).unwrap();
            assert_eq!(bit4, plain, "origin bits {origin:#04x}");
        }
        // Stored left to right, top row first, as a top-origin file lists it.
        let (_, _, rgba) = tga_to_rgba(&tga(2, 2, 2, 24, 0x30, &px)).unwrap();
        assert_eq!(
            rgba,
            vec![
                0, 0, 255, 255, 255, 0, 0, 255, //
                0, 255, 0, 255, 255, 255, 255, 255,
            ]
        );
        // The run-length reader orients the same way.
        let rle = [0x80, 255, 0, 0, 0x80, 0, 0, 255];
        let plain = tga_to_rgba(&tga(10, 2, 1, 24, 0x20, &rle)).unwrap();
        assert_eq!(tga_to_rgba(&tga(10, 2, 1, 24, 0x30, &rle)).unwrap(), plain);
    }

    /// `0x5a37e0` admits a truecolour image (types 2 and 10, from `0x5a3ace` and `0x5a3ae9`) only
    /// when its depth less its alpha bits is 24; `0x5a37c5` first clears the 8 alpha bits a 24-bit
    /// image declares. A 32-bit image with no alpha bits fails at `0x5a3809`.
    #[test]
    fn a_truecolour_depth_less_alpha_bits_must_be_24() {
        let bgr = [255, 0, 0];
        let bgra = [255, 0, 0, 77];
        // 24 bits with no alpha bits, or with 8 (cleared): opaque.
        for alpha_bits in [0, 8] {
            let (_, _, rgba) = tga_to_rgba(&tga(2, 1, 1, 24, 0x20 | alpha_bits, &bgr)).unwrap();
            assert_eq!(
                rgba,
                vec![0, 0, 255, 255],
                "24-bit, alpha bits {alpha_bits}"
            );
        }
        // 32 bits with 8 alpha bits: byte 3 is alpha.
        let (_, _, rgba) = tga_to_rgba(&tga(2, 1, 1, 32, 0x28, &bgra)).unwrap();
        assert_eq!(rgba, vec![0, 0, 255, 77]);
        let rle = [0x80, 255, 0, 0, 77];
        let (_, _, rgba) = tga_to_rgba(&tga(10, 1, 1, 32, 0x28, &rle)).unwrap();
        assert_eq!(rgba, vec![0, 0, 255, 77]);

        // 32 bits with no alpha bits: 32 - 0 is not 24.
        for (image_type, data) in [(2, &bgra[..]), (10, &rle[..])] {
            let err = tga_to_rgba(&tga(image_type, 1, 1, 32, 0x20, data)).unwrap_err();
            assert!(
                err.to_string().contains("alpha bits"),
                "type {image_type}: {err}"
            );
        }
        // Any other pair that does not leave 24 is refused, whatever the bytes could mean. So are
        // the pairs that do, bar 24 and 32 bits: `0x5a37e0` admits them and the reference crashes
        // at `0x4492c4` (the module's fourth Deviation).
        for (bpp, alpha_bits) in [
            (32, 4),
            (32, 1),
            (24, 4),
            (24, 1),
            (16, 0),
            (16, 1),
            (25, 1),
            (28, 4),
            (31, 7),
            (33, 9),
            (39, 15),
        ] {
            assert!(
                tga_to_rgba(&tga(2, 1, 1, bpp, 0x20 | alpha_bits, &bgra)).is_err(),
                "{bpp} bits, {alpha_bits} alpha bits"
            );
        }
    }

    /// The addon corpus: every TGA that a descriptor bit 4 would change decodes the same without
    /// it, and every truecolour TGA whose depth less its alpha bits is not 24 (with `0x5a37c5`'s
    /// 24-bit case first cleared) is refused, as `0x5a37e0` refuses it.
    #[test]
    fn addon_tgas_follow_the_references_descriptor_rules() {
        let corpus = crate::addon_corpus_or_skip!();
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("tga"))
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&corpus, &mut files);
        assert!(
            files.len() > 100,
            "only {} TGAs under {corpus:?}",
            files.len()
        );
        let (mut decoded, mut refused) = (0usize, 0usize);
        for path in files {
            let Ok(mut bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes.len() < 18 {
                continue;
            }
            let (image_type, bpp, mut alpha_bits) = (bytes[2], bytes[16], bytes[17] & 0xf);
            if bpp == 24 && alpha_bits == 8 {
                alpha_bits = 0;
            }
            let result = tga_to_rgba(&bytes);
            if matches!(image_type, 2 | 10) && usize::from(bpp) != 24 + usize::from(alpha_bits) {
                assert!(
                    result.is_err(),
                    "{path:?}: {bpp} bits, {alpha_bits} alpha bits"
                );
                refused += 1;
            }
            if let Ok(decoded_plain) = result {
                bytes[17] |= 0x10;
                let with_bit_4 = tga_to_rgba(&bytes).unwrap();
                assert_eq!(with_bit_4, decoded_plain, "{path:?}");
                decoded += 1;
            }
        }
        assert!(decoded > 100, "only {decoded} TGAs decoded");
        eprintln!("{decoded} corpus TGAs decode, {refused} are refused by depth");
    }

    /// `0x44a140` has a texture once the header opens (`0x5a3660`), at the header's size
    /// (`0x448450`): whether the pixels decode is `0x44a260`'s question, later.
    #[test]
    fn a_header_that_opens_makes_a_texture_at_its_size() {
        // A 32-bit image with no alpha bits is refused by the decoder, and still a 4 by 2 texture.
        let refused = tga(2, 4, 2, 32, 0x20, &[0; 32]);
        assert!(tga_to_rgba(&refused).is_err());
        assert_eq!(tga_texture_size(&refused), Some((4, 2)));
        assert_eq!(tga_texture_size(&tga(2, 4, 2, 32, 0x20, &[])), Some((4, 2)));
        assert_eq!(
            tga_texture_size(&tga(99, 512, 1, 7, 0xff, &[])),
            Some((512, 1))
        );

        // A zero size passes `0x44849d`'s test; a size that is not a power of two below 1024
        // makes no texture.
        assert_eq!(tga_texture_size(&tga(2, 0, 4, 32, 0x28, &[])), Some((0, 4)));
        assert_eq!(tga_texture_size(&tga(2, 4, 0, 32, 0x28, &[])), Some((4, 0)));
        for (w, h) in [(3, 2), (4, 1024), (1024, 4), (65535, 2), (6, 6)] {
            assert_eq!(
                tga_texture_size(&tga(2, w, h, 32, 0x28, &[0; 8])),
                None,
                "{w}x{h}"
            );
        }
        assert_eq!(tga_texture_size(&[0; 17]), None);
        assert_eq!(tga_texture_size(b"BLP2"), None);
    }

    /// The gate's prefix: nothing past `TGA_GATE_PREFIX` bytes changes the answer.
    #[test]
    fn the_gate_answers_the_same_for_its_prefix_as_for_the_file() {
        for id_length in [0usize, 1, 17, 254, 255] {
            for map_len in [0u16, 3] {
                for tail in [0usize, 1, 2, 300, 2000] {
                    for map_type in [0u8, 1] {
                        let mut file = tga(1, 8, 4, 8, 0x20, &[]);
                        file[0] = id_length as u8;
                        file[1] = map_type;
                        file[5..7].copy_from_slice(&map_len.to_le_bytes());
                        file[7] = 24;
                        file.extend(vec![0; id_length + usize::from(map_len) * 3 + tail]);
                        for cut in [0, 1, id_length, id_length + 1, 300, file.len()] {
                            let file = &file[..file.len().saturating_sub(cut)];
                            let prefix = &file[..file.len().min(TGA_GATE_PREFIX)];
                            assert_eq!(
                                tga_texture_size(prefix),
                                tga_texture_size(file),
                                "id {id_length}, map {map_type}/{map_len}, {} bytes",
                                file.len()
                            );
                        }
                    }
                }
            }
        }
    }

    /// The id and the colour map are read after the header, and a read that gets any byte
    /// succeeds (`0x64853b`-`0x64854a`); one that gets none fails the open. The map is sized as
    /// the reference sizes it (`0x5a3b40`): 32-bit entries count 3 bytes.
    #[test]
    fn the_id_and_map_reads_fail_the_open_only_with_no_bytes() {
        let mut with_id = tga(2, 4, 2, 24, 0x20, &[]);
        with_id[0] = 5;
        assert_eq!(tga_texture_size(&with_id), None, "no id byte");
        with_id.push(0);
        assert_eq!(tga_texture_size(&with_id), Some((4, 2)), "a short id");

        let mut with_map = tga(1, 4, 2, 8, 0x20, &[]);
        with_map[1] = 1; // colour-map type: present
        with_map[5..7].copy_from_slice(&4u16.to_le_bytes());
        with_map[7] = 32;
        assert_eq!(tga_texture_size(&with_map), None, "no map byte");
        with_map.push(0);
        assert_eq!(tga_texture_size(&with_map), Some((4, 2)), "a short map");
        // Past an id of 2, one byte reaches the map.
        with_map[0] = 2;
        assert_eq!(tga_texture_size(&with_map), None, "the id took the byte");
        with_map.extend([0, 0]);
        assert_eq!(tga_texture_size(&with_map), Some((4, 2)));
        // No map type, no read, whatever the length field says.
        with_map[1] = 0;
        assert_eq!(tga_texture_size(&tga(2, 4, 2, 24, 0x20, &[])), Some((4, 2)));
        assert_eq!(tga_texture_size(&with_map), Some((4, 2)));
    }

    #[test]
    fn garbage_and_unsupported_types_are_loud_misses() {
        assert!(tga_to_rgba(b"BLP2xxxxxxxxxxxxxxxxxx").is_err()); // a BLP is not a TGA
        assert!(tga_to_rgba(&[0u8; 4]).is_err()); // truncated header
        assert!(tga_to_rgba(&tga(1, 2, 2, 8, 0, &[0; 32])).is_err()); // colour-mapped, no map
        assert!(tga_to_rgba(&tga(2, 2, 2, 16, 0, &[0; 32])).is_err()); // 16-bit
        assert!(tga_to_rgba(&tga(2, 2, 2, 32, 8, &[0; 4])).is_err()); // truncated pixels
    }
}
