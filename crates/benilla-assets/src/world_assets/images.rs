//! Image helpers: BLP mip chains and raw RGBA into Bevy `Image`s (format, sampler, mip layout),
//! for world art, liquid frames, sprites, masks and the cursor.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

use benilla_formats::BlpMipChain;

/// Albedo format, non-sRGB: the reference multiplies gamma texture bytes by gamma light bytes.
pub fn color_texture_format() -> TextureFormat {
    TextureFormat::Rgba8Unorm
}

/// World or model albedo from the BLP's authored mips as stored: the 1.12 client has no GLU or
/// `glGenerateMipmap`, and does no re-filter or alpha-to-coverage.
pub fn repeat_texture_authored(upload: crate::gpu_blp::UploadChain, wrap: (bool, bool)) -> Image {
    let crate::gpu_blp::UploadChain { chain, format } = upload;
    let levels = chain.mips.len() as u32;
    let mut data = Vec::with_capacity(chain.mips.iter().map(Vec::len).sum());
    for mip in &chain.mips {
        data.extend_from_slice(mip);
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: chain.width,
            height: chain.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        // The synchronous twin of `blp.rs`'s `WorldArt` lane: the render world takes the chain, and
        // neither caller (`WorldAssets::texture`, the character-skin composite) reads it back.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    // The M2 texture record's own address mode (`flags & 0x1/0x2`): a cutout card authors UVs
    // past `0..1` to clamp onto its transparent border, where repeat folds its opaque middle in.
    let mode = |repeat: bool| {
        if repeat {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        }
    };
    // M2, WMO and character textures take the process filter policy (`0x449ae0`, applyGlobal 1).
    let filter = crate::tex_filter::tex_filter();
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode(wrap.0),
        address_mode_v: mode(wrap.1),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: filter.mipmap_filter(),
        anisotropy_clamp: filter.anisotropy_clamp(),
        ..default()
    });
    image
}

/// `WOW_LIQUID_DC=raw` keeps the shipped frames' own DC, the A/B lever for [`flatten_frame_dc`].
fn dc_normalization_enabled() -> bool {
    std::env::var("WOW_LIQUID_DC").as_deref() != Ok("raw")
}

/// Shift each frame's DC (per level, per channel) onto the loop's mean.
///
/// Deviation: the reference uploads the shipped frames as they are; this evens them because their
/// means differ (`ocean_h` alpha 55.985..57.628 over its 30 frames, more down the DXT3 mips), and
/// at distance, where mipping averages a level to one value, the far sheet brightens and dims once
/// per 1.25 s loop. An additive offset leaves the ripple's crests unscaled; per channel, since
/// `detail.rgb` and `detail.a` both drift. Water only: magma and slime keep their pulse.
fn flatten_frame_dc(data: &mut [u8], spans: &[Vec<(usize, usize)>], levels: usize) {
    for level in 0..levels {
        // The loop's target sum per texel for this level, over every frame.
        let mut sums = [0i64; 4];
        let mut texels = 0usize;
        for frame in spans {
            let (start, len) = frame[level];
            for px in data[start..start + len].as_chunks::<4>().0 {
                for c in 0..4 {
                    sums[c] += i64::from(px[c]);
                }
            }
            texels += len / 4;
        }
        if texels == 0 {
            continue;
        }
        for frame in spans {
            let (start, len) = frame[level];
            let n = len / 4;
            if n == 0 {
                continue;
            }
            for c in 0..4 {
                let have: i64 = data[start..start + len]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|px| i64::from(px[c]))
                    .sum();
                // The exact integer total this channel must move by to sit on the loop mean.
                let want = (sums[c] * n as i64).div_euclid(texels as i64);
                let mut delta = want - have;
                if delta == 0 {
                    continue;
                }
                // Spend the delta in ±1 steps on a stride coprime with the texel count, so the
                // touched texels scatter and each pass adds at most 1 LSB per texel; railed texels
                // are skipped, and a pass that moves nothing ends the walk.
                let stride = if n % 7 == 0 { 1 } else { 7 };
                let mut progress = true;
                while delta != 0 && progress {
                    progress = false;
                    for k in 0..n {
                        if delta == 0 {
                            break;
                        }
                        let b = &mut data[start + ((k * stride) % n) * 4 + c];
                        if delta > 0 && *b < 255 {
                            *b += 1;
                            delta -= 1;
                            progress = true;
                        } else if delta < 0 && *b > 0 {
                            *b -= 1;
                            delta += 1;
                            progress = true;
                        }
                    }
                }
            }
        }
    }
}

/// The animated liquid frames stacked layer-major into one `texture_2d_array` from their authored
/// mips, a full chain to 1×1; a level a frame lacks is filled `NEAREST` from its nearest authored
/// level, so no gamma averaging. Every frame shares mip 0's `size` (the caller enforces it).
pub fn liquid_frame_array(frames: Vec<BlpMipChain>, normalize_dc: bool) -> Image {
    let size = frames[0].width;
    // Square 1.12 frames: levels = log2(size) + 1.
    let mip_level_count = {
        let (mut n, mut s) = (1u32, size);
        while s > 1 {
            s >>= 1;
            n += 1;
        }
        n
    };
    let mut data = Vec::with_capacity(frames.len() * mip_chain_byte_size(size, mip_level_count));
    // Where each (frame, level) region landed, for the DC pass.
    let mut spans: Vec<Vec<(usize, usize)>> = Vec::with_capacity(frames.len());
    for blp in &frames {
        let mut per_level = Vec::with_capacity(mip_level_count as usize);
        for level in 0..mip_level_count {
            let lw = (size >> level).max(1);
            // The authored mip of this size (water frames carry a full chain), else the nearest.
            let src_level = if blp.width >= lw {
                let ratio = (blp.width / lw).max(1);
                (ratio.trailing_zeros() as usize).min(blp.mips.len() - 1)
            } else {
                0
            };
            let (sw, sh) = blp.mip_size(src_level as u32);
            let start = data.len();
            extend_nearest(&mut data, &blp.mips[src_level], sw, sh, lw, lw);
            per_level.push((start, data.len() - start));
        }
        spans.push(per_level);
    }
    if normalize_dc && dc_normalization_enabled() {
        flatten_frame_dc(&mut data, &spans, mip_level_count as usize);
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: frames.len() as u32,
        },
        TextureDimension::D2,
        color_texture_format(),
        // 47,535,264 B resident for the whole process, never read main-side.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_level_count;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    // The liquid sheets take the process filter policy (`0x68abab`, applyGlobal 1), as ground does.
    let filter = crate::tex_filter::tex_filter();
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: filter.mipmap_filter(),
        anisotropy_clamp: filter.anisotropy_clamp(),
        ..default()
    });
    image
}

/// A single-mip sRGB image for winit's custom OS cursor; macOS builds an `NSCursor` instead.
#[cfg(not(target_os = "macos"))]
pub fn cursor_texture(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

/// A single-mip sRGB sprite, clamped and linear, for the celestial discs and the UI: sRGB so an
/// unlit pass returns the authored gamma bytes to screen.
pub fn sprite_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// A coverage mask (the minimap's `MinimapMask.blp`, its circle ramp in the alpha): sampled like
/// [`sprite_image`] but uploaded `Rgba8Unorm`, since the shader reads coverage, not colour.
pub fn mask_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// A portrait: [`sprite_image`] with alpha zeroed outside the inscribed circle (1 px anti-aliased),
/// so square art does not poke past the unit frame's round ring (`UI-TargetingFrame`).
pub fn portrait_image(width: u32, height: u32, mut rgba: Vec<u8>) -> Image {
    if width > 0 && height > 0 && rgba.len() == (width * height * 4) as usize {
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let radius = cx.min(cy);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let coverage = (radius - dist).clamp(0.0, 1.0);
                let a = ((y * width + x) * 4 + 3) as usize;
                rgba[a] = (f32::from(rgba[a]) * coverage).round() as u8;
            }
        }
    }
    sprite_image(width, height, rgba)
}

/// [`sprite_image`] wrapping on both axes, for a frame `Backdrop`, whose `SetTexture` pushes one
/// argument twice into the load descriptor (`0x770200`), read as U and V wrap but not traced to
/// the sampler: an edge strip runs UVs `[0..N]` (N = frame side / edgeSize − 2) and a tiled
/// background `[0..w/period]` (`0x77e8d0`, `0x77f0c0`). The edge strips are atlas crops kept off
/// the image edge (`inset_atlas_bleed`), so their bounded axis does not bleed.
pub fn sprite_image_tiled(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    sprite_image_wrapped(width, height, rgba, (true, true))
}

/// [`sprite_image`] with `Repeat` on the axes `wrap` names and clamp elsewhere. The reference's
/// `SetTexCoord(0, n, 0, 1)` strips tile along one axis only; repeat on the bounded axis would
/// filter the last row into the first as a hairline along the strip's edge.
pub fn sprite_image_wrapped(width: u32, height: u32, rgba: Vec<u8>, wrap: (bool, bool)) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let mode = |repeat: bool| {
        if repeat {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        }
    };
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode(wrap.0),
        address_mode_v: mode(wrap.1),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// The bytes an [`Image`] occupies on the GPU, every mip of every layer, from the
/// `texture_descriptor` alone: a `RENDER_WORLD` asset's `data` moves to the render world on
/// extract, so main-side it reads `None`. Block-aware, as bevy_image computes it.
pub fn image_gpu_bytes(image: &Image) -> usize {
    let d = &image.texture_descriptor;
    let format = d.format;
    let (bw, bh) = format.block_dimensions();
    // A block is whole: a 1×1 mip of a 4×4-block format costs a full block.
    let Some(block_bytes) = format.block_copy_size(None) else {
        return 0; // multi-planar/depth-stencil: no single block size, and we upload neither
    };
    let volume = d.dimension == TextureDimension::D3;
    (0..d.mip_level_count.max(1))
        .map(|i| {
            let w = (d.size.width >> i).max(1);
            let h = (d.size.height >> i).max(1);
            // Array layers do not halve with the mip level; a 3-D texture's depth does.
            let layers = if volume {
                (d.size.depth_or_array_layers >> i).max(1)
            } else {
                d.size.depth_or_array_layers.max(1)
            };
            let blocks = w.div_ceil(bw) * h.div_ceil(bh);
            (blocks * block_bytes * layers) as usize
        })
        .sum()
}

/// Total bytes for a mip pyramid of `mip_count` levels starting at `top²` RGBA8.
pub fn mip_chain_byte_size(top: u32, mip_count: u32) -> usize {
    (0..mip_count)
        .map(|i| {
            let w = (top >> i).max(1);
            (w * w * 4) as usize
        })
        .sum()
}

/// Append `src` (RGBA8 `sw × sh`) resized to `dw × dh` by `NEAREST`, which only replicates or
/// decimates texels and so keeps the gamma bytes verbatim.
pub fn extend_nearest(out: &mut Vec<u8>, src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) {
    if sw == dw && sh == dh {
        out.extend_from_slice(src);
        return;
    }
    let Some(buf) = image::RgbaImage::from_raw(sw, sh, src.to_vec()) else {
        // A shape mismatch gives a black layer rather than a panic.
        out.extend(std::iter::repeat_n(0u8, (dw * dh * 4) as usize));
        return;
    };
    let resized = image::imageops::resize(&buf, dw, dh, image::imageops::FilterType::Nearest);
    out.extend_from_slice(resized.as_raw());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two 2x2 frames with far-apart means and a full 2-level chain each.
    fn dc_frames() -> Vec<BlpMipChain> {
        let frame = |base: u8| BlpMipChain {
            width: 2,
            height: 2,
            texels: benilla_formats::BlpTexels::Rgba8Unorm,
            mips: vec![
                // 2x2: four texels around `base`.
                vec![
                    base,
                    base,
                    base,
                    base,
                    base + 10,
                    base + 10,
                    base + 10,
                    base + 10,
                    base,
                    base,
                    base,
                    base,
                    base + 10,
                    base + 10,
                    base + 10,
                    base + 10,
                ],
                // 1x1
                vec![base + 5, base + 5, base + 5, base + 5],
            ],
        };
        vec![frame(40), frame(80)]
    }

    #[test]
    fn flatten_frame_dc_equalizes_every_frame_at_every_level() {
        let frames = dc_frames();
        let n = frames.len();
        let img = liquid_frame_array(frames, true);
        let data = img.data.as_ref().expect("array has data");

        // LayerMajor: frame0[2x2, 1x1], frame1[2x2, 1x1].
        let level_bytes = [2 * 2 * 4usize, 4];
        let stride: usize = level_bytes.iter().sum();
        for (level, &len) in level_bytes.iter().enumerate() {
            let offset: usize = level_bytes[..level].iter().sum();
            let means: Vec<f64> = (0..n)
                .map(|f| {
                    let start = f * stride + offset;
                    let px = &data[start..start + len];
                    px.iter().map(|&b| f64::from(b)).sum::<f64>() / px.len() as f64
                })
                .collect();
            let spread = means.iter().cloned().fold(f64::MIN, f64::max)
                - means.iter().cloned().fold(f64::MAX, f64::min);
            assert!(
                spread <= 1.0,
                "level {level} still drifts across frames: {means:?} (spread {spread})"
            );
        }
    }

    /// The control for the test above: opted out (the magma and slime lane), the frames keep their
    /// own DC, so a no-op pass cannot pass it.
    #[test]
    fn raw_lever_keeps_the_authored_per_frame_dc() {
        let frames = dc_frames();
        let img = liquid_frame_array(frames, false);
        let data = img.data.as_ref().expect("array has data");
        let mean = |start: usize, len: usize| {
            data[start..start + len]
                .iter()
                .map(|&b| f64::from(b))
                .sum::<f64>()
                / len as f64
        };
        let stride = 2 * 2 * 4 + 4;
        let spread = (mean(stride, 16) - mean(0, 16)).abs();
        assert!(
            spread > 30.0,
            "opting out must leave the frames' own DC alone, saw spread {spread}"
        );
    }

    fn descriptor_only(size: Extent3d, mips: u32, format: TextureFormat) -> Image {
        let mut image = Image::new_uninit(
            size,
            TextureDimension::D2,
            format,
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.mip_level_count = mips;
        image
    }

    #[test]
    fn gpu_bytes_agree_with_the_rgba8_chain_arithmetic() {
        for (top, mips) in [(256, 9), (128, 8), (64, 7), (1, 1)] {
            let image = descriptor_only(
                Extent3d {
                    width: top,
                    height: top,
                    depth_or_array_layers: 1,
                },
                mips,
                TextureFormat::Rgba8Unorm,
            );
            assert_eq!(
                image_gpu_bytes(&image),
                mip_chain_byte_size(top, mips),
                "{top}²/{mips}"
            );
        }
    }

    /// The liquid lane: 136 frames of a 256² RGBA8 nine-level pyramid, stacked as array layers.
    #[test]
    fn gpu_bytes_count_every_array_layer() {
        let frames = 136;
        let image = descriptor_only(
            Extent3d {
                width: 256,
                height: 256,
                depth_or_array_layers: frames,
            },
            9,
            TextureFormat::Rgba8Unorm,
        );
        assert_eq!(image_gpu_bytes(&image), 47_535_264); // 45.33 MiB
        assert_eq!(
            image_gpu_bytes(&image),
            mip_chain_byte_size(256, 9) * frames as usize
        );
    }

    #[test]
    fn gpu_bytes_are_block_aware_and_round_up() {
        let bc1 = descriptor_only(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            3,
            TextureFormat::Bc1RgbaUnorm,
        );
        // 4×4 → 1 block, 2×2 → 1 block, 1×1 → 1 block; BC1 is 8 bytes a block.
        assert_eq!(image_gpu_bytes(&bc1), 24);
        // The same pyramid uncompressed is 4 B/texel with no rounding: 64 + 16 + 4.
        let rgba = descriptor_only(
            Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            3,
            TextureFormat::Rgba8Unorm,
        );
        assert_eq!(image_gpu_bytes(&rgba), 84);
    }
}

#[cfg(test)]
mod wrap_tests {
    use super::*;

    fn modes(image: &Image) -> (ImageAddressMode, ImageAddressMode) {
        match &image.sampler {
            ImageSampler::Descriptor(d) => (d.address_mode_u, d.address_mode_v),
            other => panic!("a sprite carries its own sampler, got {other:?}"),
        }
    }

    #[test]
    fn a_one_axis_tile_wraps_that_axis_and_clamps_the_other() {
        let img = sprite_image_wrapped(2, 2, vec![0; 16], (true, false));
        assert_eq!(
            modes(&img),
            (ImageAddressMode::Repeat, ImageAddressMode::ClampToEdge)
        );
        let img = sprite_image_wrapped(2, 2, vec![0; 16], (false, true));
        assert_eq!(
            modes(&img),
            (ImageAddressMode::ClampToEdge, ImageAddressMode::Repeat)
        );
    }

    #[test]
    fn the_tiled_sprite_still_wraps_both_axes() {
        let img = sprite_image_tiled(2, 2, vec![0; 16]);
        assert_eq!(
            modes(&img),
            (ImageAddressMode::Repeat, ImageAddressMode::Repeat)
        );
        assert_eq!(img.texture_descriptor.format, TextureFormat::Rgba8UnormSrgb);
    }
}
