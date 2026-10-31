//! The mip chain of a `.tga` texture, as `TextureDecodeTga` (`0x4491f0`) builds it: level 0 from
//! the file, the next levels from `<name>_mip<n>.tga` where those exist (`0x4493a0`), and every
//! level below from the last of those by an alpha-weighted box filter (`0x44aec0`, `0x5a4c30`).

use benilla_formats::{full_mip_count, tga_texture_size, tga_to_rgba, BlpMipChain, BlpTexels};

/// The file of level `n` for the TGA `stem` names (`0x449550`): the name cut at its last dot.
pub(crate) fn mip_file_name(stem: &str, n: u32) -> String {
    format!("{stem}_mip{n}.tga")
}

/// The size a mip file of level `n` must have to be taken (`0x449457`, `0x449463`): the base's
/// sides halved for level 1, so a side of 1 gives 0 and no file matches, then each side above 1
/// halved per level.
fn expected_size(width: u32, height: u32, n: u32) -> (u32, u32) {
    let (mut w, mut h) = (width >> 1, height >> 1);
    for _ in 1..n {
        w = if w > 1 { w >> 1 } else { w };
        h = if h > 1 { h >> 1 } else { h };
    }
    (w, h)
}

/// A mip file's pixels when it decodes and has the size level `n` expects.
fn accept_mip(bytes: &[u8], base: (u32, u32), n: u32) -> Option<Vec<u8>> {
    let (w, h, rgba) = tga_to_rgba(bytes).ok()?;
    ((w, h) == expected_size(base.0, base.1, n)).then_some(rgba)
}

/// Each level's side: the base's halved, held at 1.
fn level_size(width: u32, height: u32, n: u32) -> (u32, u32) {
    ((width >> n).max(1), (height >> n).max(1))
}

/// The chain of a TGA texture. `read_mip(n)` returns the bytes of level `n`'s file, or `None`
/// where none resolves; the first level without an acceptable file ends them.
pub(crate) fn tga_chain(
    base: (u32, u32, Vec<u8>),
    mut read_mip: impl FnMut(u32) -> Option<Vec<u8>>,
) -> BlpMipChain {
    let (width, height, rgba) = base;
    let levels = full_mip_count(width, height) as u32;
    let mut mips = vec![rgba];
    for n in 1..levels {
        let Some(pixels) = read_mip(n).and_then(|b| accept_mip(&b, (width, height), n)) else {
            break;
        };
        mips.push(pixels);
    }
    // `0x44aec0` filters every level from the last one read straight from it, never level by level.
    let from = mips.len() - 1;
    let source = level_size(width, height, from as u32);
    for n in from as u32 + 1..levels {
        let level = box_downsample(&mips[from], source, level_size(width, height, n));
        mips.push(level);
    }
    BlpMipChain {
        width,
        height,
        texels: BlpTexels::Rgba8Unorm,
        mips,
    }
}

/// The chain a TGA texture takes from its bytes: the gate's size (`0x448450`) or no texture
/// (`None`); a file the decoder refuses is opaque white at that size (`0x44a2eb` into `0x448920`).
pub(crate) fn tga_texture_chain(
    bytes: &[u8],
    read_mip: impl FnMut(u32) -> Option<Vec<u8>>,
) -> Option<BlpMipChain> {
    let (gw, gh) = tga_texture_size(bytes)?;
    Some(match tga_to_rgba(bytes) {
        Ok(base) => tga_chain(base, read_mip),
        Err(_) => white_chain(gw, gh),
    })
}

/// A full chain of opaque white at `width` by `height`.
pub(crate) fn white_chain(width: u32, height: u32) -> BlpMipChain {
    let levels = full_mip_count(width, height) as u32;
    BlpMipChain {
        width,
        height,
        texels: BlpTexels::Rgba8Unorm,
        mips: (0..levels)
            .map(|n| {
                let (w, h) = level_size(width, height, n);
                vec![0xff; w as usize * h as usize * 4]
            })
            .collect(),
    }
}

/// `0x5a4c30`: each destination texel averages a `(sw / dw) x (sh / dh)` block of the source, the
/// colour channels weighted by alpha. A block with no alpha averages its colours plainly and has
/// alpha 0; otherwise alpha is its mean.
fn box_downsample(src: &[u8], (sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> Vec<u8> {
    let (bw, bh) = ((sw / dw) as usize, (sh / dh) as usize);
    let count = (bw * bh) as u64;
    let mut out = Vec::with_capacity(dw as usize * dh as usize * 4);
    for y in 0..dh as usize {
        for x in 0..dw as usize {
            let (mut colour, mut plain, mut alpha) = ([0u64; 3], [0u64; 3], 0u64);
            for by in 0..bh {
                for bx in 0..bw {
                    let i = ((y * bh + by) * sw as usize + x * bw + bx) * 4;
                    let a = u64::from(src[i + 3]);
                    alpha += a;
                    for c in 0..3 {
                        colour[c] += u64::from(src[i + c]) * a;
                        plain[c] += u64::from(src[i + c]);
                    }
                }
            }
            if alpha != 0 {
                out.extend(colour.map(|c| (c / alpha) as u8));
                out.push((alpha / count) as u8);
            } else {
                out.extend(plain.map(|c| (c / count) as u8));
                out.push(0);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tga(width: u16, height: u16, bgra: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 18];
        b[2] = 2;
        b[12..14].copy_from_slice(&width.to_le_bytes());
        b[14..16].copy_from_slice(&height.to_le_bytes());
        b[16] = 32;
        b[17] = 8 | 0x20;
        b.extend_from_slice(bgra);
        b
    }

    #[test]
    fn the_chain_runs_to_one_by_one_and_filters_by_alpha() {
        // 2x2: two opaque red, one opaque blue, one clear green. Alpha-weighted, green is lost.
        let px = [
            [255, 0, 0, 255],
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            [0, 255, 0, 0],
        ];
        let base = tga_to_rgba(&tga(2, 2, &px.concat())).unwrap();
        let chain = tga_chain(base, |_| None);
        assert_eq!(chain.mips.len(), 2);
        assert_eq!(chain.mips[1].len(), 4);
        // Σ(c·A)/ΣA over three opaque texels; alpha ΣA / 4.
        assert_eq!(chain.mips[1][3], 191);
        assert_eq!(
            chain.mips[1][1], 0,
            "the clear texel's green carries no weight"
        );
    }

    #[test]
    fn a_mip_file_of_the_expected_size_replaces_the_filtered_level() {
        let base = tga_to_rgba(&tga(4, 2, &[7u8; 32])).unwrap();
        let level1 = tga(2, 1, &[200u8; 8]);
        let mut asked = Vec::new();
        let chain = tga_chain(base, |n| {
            asked.push(n);
            (n == 1).then(|| level1.clone())
        });
        assert_eq!(chain.mips.len(), 3);
        assert!(chain.mips[1].iter().all(|&b| b == 200));
        assert_eq!(asked, [1, 2], "the first miss ends the files");
        // A file of the wrong size is a miss.
        let base = tga_to_rgba(&tga(4, 2, &[7u8; 32])).unwrap();
        let wrong = tga(4, 2, &[200u8; 32]);
        let chain = tga_chain(base, |_| Some(wrong.clone()));
        assert!(chain.mips[1].iter().all(|&b| b == 7));
    }

    #[test]
    fn a_gated_tga_that_will_not_decode_is_white_at_the_gated_size() {
        let bytes = tga(8, 8, &[]);
        let chain = tga_texture_chain(&bytes, |_| None).expect("the header makes a texture");
        assert_eq!((chain.width, chain.height), (8, 8));
        assert_eq!(chain.mips.len(), 4);
        assert!(chain.mips.iter().flatten().all(|&b| b == 0xff));
        assert!(tga_texture_chain(&bytes[..10], |_| None).is_none());
    }

    #[test]
    fn mip_file_levels_expect_halved_sides() {
        assert_eq!(expected_size(8, 2, 1), (4, 1));
        assert_eq!(expected_size(8, 2, 2), (2, 1));
        assert_eq!(expected_size(1, 4, 1), (0, 2));
        assert_eq!(mip_file_name("a/b", 3), "a/b_mip3.tga");
    }
}
