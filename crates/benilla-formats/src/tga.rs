//! TGA decoding, the other half of the reference's UI texture lookup: `TextureCreate` tries
//! `{".tga", ".blp"}` (`0x835248`), and while 1.12 ships its UI art as BLP, addons ship TGAs. The
//! reference's `0x5a3a30` dispatches on `imageType`: 1 and 9 (colour-mapped, raw or RLE) to
//! `0x5a39d0` through the palette at `0x5a3820`, 2 to `0x5a3c80`, 10 to `0x5a3d70`.
//!
//! Deviation: grayscale types 3 and 11 decode where the reference refuses them, because no shipped
//! or addon file is one and removing working code would buy nothing.
//!
//! [`tga_to_rgba`] checks the header before the pixels, so it can serve as the fallback after a
//! failed `BLP2` sniff: TGA has no magic.

use anyhow::{bail, Result};

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
    // A colour-mapped pixel is an index, its palette entry a width of its own (`0x5a3b40`).
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

    // BGR(A) or gray → RGBA. Descriptor bit 5 is a top origin, bit 4 a right one; TGA's default
    // is bottom-left.
    let top_down = descriptor & 0x20 != 0;
    let right_first = descriptor & 0x10 != 0;
    let mut rgba = vec![0u8; pixel_count * 4];
    for y in 0..height as usize {
        let src_y = if top_down { y } else { height as usize - 1 - y };
        for x in 0..width as usize {
            let src_x = if right_first {
                width as usize - 1 - x
            } else {
                x
            };
            let s = (src_y * width as usize + src_x) * bytes_per_pixel;
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
        out[5..7]
            .copy_from_slice(&((palette.len() / (entry_bits as usize / 8)) as u16).to_le_bytes());
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
        let (w, h, rgba) = tga_to_rgba(&tga(2, 2, 2, 32, 0, &px)).unwrap();
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

    #[test]
    fn garbage_and_unsupported_types_are_loud_misses() {
        assert!(tga_to_rgba(b"BLP2xxxxxxxxxxxxxxxxxx").is_err()); // a BLP is not a TGA
        assert!(tga_to_rgba(&[0u8; 4]).is_err()); // truncated header
        assert!(tga_to_rgba(&tga(1, 2, 2, 8, 0, &[0; 32])).is_err()); // colour-mapped, no map
        assert!(tga_to_rgba(&tga(2, 2, 2, 16, 0, &[0; 32])).is_err()); // 16-bit
        assert!(tga_to_rgba(&tga(2, 2, 2, 32, 0, &[0; 4])).is_err()); // truncated pixels
    }
}
