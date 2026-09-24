//! Texture/image helpers: turn decoded BLP mip chains + raw RGBA into Bevy `Image`s (correct format,
//! sampler, mip layout) plus the small solid-colour / cursor / liquid-frame textures. Pure helpers the
//! asset foundation (`super`) and other subsystems call. Split out of `assets/mod.rs`.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};

use benilla_formats::BlpMipChain;

/// Build a full mip chain for an RGBA texture (mip 0 = `mip0`), halving each level to 1×1 with a
/// triangle (box-ish) downsample. Returns the concatenated mip data + the level count. For a single
/// layer this is the `LayerMajor`/`MipMajor`-agnostic order (mip0, mip1, …).
/// Color (albedo) texture format: **non-sRGB `Rgba8Unorm`**, so the GPU does NOT linearize albedo on
/// sample. Vanilla's fixed-function pipeline multiplied gamma (sRGB) texture bytes by gamma light
/// bytes in **gamma/byte space**; loading albedo as `…Unorm` keeps our shader math in that same space.
/// This is the RE'd faithful **invariant** (no sRGB↔linear round-trip anywhere). Used for model +
/// terrain albedo.
pub fn color_texture_format() -> TextureFormat {
    TextureFormat::Rgba8Unorm
}

/// A repeating, **mipmapped + anisotropically-filtered** texture built from the BLP's **authored**
/// mip pyramid — the levels the real 1.12 client uploads verbatim (`WoW.exe` has no GLU /
/// `glGenerateMipmap`; it cannot regenerate mips, only upload the stored ones). Using the authored
/// levels instead of CPU-resampling mip 0 with a box/Triangle filter in gamma byte space is the
/// model-side of the C2 fix: gamma-byte averaging darkens mid-tones and, on alpha-cutout foliage,
/// skews the leaf colour cool (it averages the dark *transparent* texels' RGB into the leaves, 5–18
/// LSB darker in R/G) and drifts coverage with distance. The authored mips already encode WoW's
/// coverage-correct, leaf-coloured downsample, so we lay them in as-is under the alpha test — no
/// re-filter, no alpha-to-coverage (1.12 does neither). Mips + anisotropy give the no-shimmer look
/// at distance without softening the up-close art.
pub fn repeat_texture_authored(upload: crate::gpu_blp::UploadChain, wrap: (bool, bool)) -> Image {
    let crate::gpu_blp::UploadChain { chain, format } = upload;
    let levels = chain.mips.len() as u32;
    let mut data = Vec::with_capacity(chain.mips.iter().map(Vec::len).sum());
    for mip in &chain.mips {
        data.extend_from_slice(mip);
    }
    // Taking an `UploadChain` rather than a bare chain is deliberate: the format and the bytes are
    // decided together by `gpu_blp::for_upload`, so this function cannot be handed blocks under an
    // uncompressed descriptor. It was, once — see `UploadChain`'s doc (decision 1626).
    let mut image = Image::new_uninit(
        Extent3d {
            width: chain.width,
            height: chain.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        // `RENDER_WORLD` — see [`image_gpu_bytes`] and the `blp.rs` `WorldArt` variant this is the
        // synchronous twin of: the render world takes the chain rather than cloning it and keeping
        // a main-world copy. Its callers are `WorldAssets::texture` and the character-skin
        // composite, and neither reads the result back; a composite is built CPU-side and handed
        // over whole.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    // The address mode is the M2 texture record's own (`flags & 0x1/0x2`), not a blanket Repeat.
    // On a cutout sheet it decides the SILHOUETTE: content authors a card's UVs past `0..1` so the
    // margin clamps to the transparent border and the card ends there — repeat instead folds that
    // margin into the opposite edge, drawing the sheet's opaque middle as solid geometry with a hard
    // seam at the crossing. 1250 batches across 775 models author clamp and sample outside the unit
    // square (`benilla-extract uvwrapscan`); 802 are cutout/blend, where it changes shape and not
    // merely colour. Decision 0763 (bugs B52/B96 — Dun Morogh's snow-firs, the Plaguelands bush).
    let mode = |repeat: bool| {
        if repeat {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        }
    };
    // The reference forces the mip filter and the anisotropy from two process globals at every
    // `TextureCreate`, and a virgin install lands on trilinear with anisotropy OFF
    // ([`crate::tex_filter`]). This read `Linear` / `8` — mode 5 — under the comment "the vanilla
    // look", which was the look of an install with `SET anisotropic "16"` in its `Config.wtf`.
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

/// `WOW_LIQUID_DC=raw` restores the shipped frames' own per-frame DC — the A/B lever for
/// [`flatten_frame_dc`] while the fidelity question behind it is open (see that function).
fn dc_normalization_enabled() -> bool {
    std::env::var("WOW_LIQUID_DC").as_deref() != Ok("raw")
}

/// Flatten each frame's **DC** (per level, per channel) onto the loop's mean — the fix for the
/// water sheet breathing once per animation loop.
///
/// The shipped ocean frames do not share a mean. Across `ocean_h`'s 30-frame loop the full-texture
/// mean alpha runs 55.985..57.628, and DXT3's 4-bit alpha quantisation makes the small levels drift
/// harder still: measured swing is 1.64 at mip 0, 3.72 at mip 5 and 6.38 at mip 6. Near the camera
/// that is invisible, because each pixel lands on a different texel and the variation scatters as
/// ripple shimmer. At distance mipping averages the whole level into one value, so every pixel of
/// the far sheet moves *together* and the surface visibly brightens and dims once per loop —
/// 1.25 s at the 24 fps flip. Director-reported at Ratchet (2026-09-06); measured off their capture
/// at 0.8..1.84/255 with an autocorrelation period of exactly 1.250 s, and confirmed coherent
/// rather than noise (a 0.82/255 swing in a 104k-pixel mean cannot come from per-pixel scatter).
/// It reads as a flicker only at a high `farclip` — at 300 that band is fogged out or past the
/// wall, which is why the report came with "not visible at 300".
///
/// The correction is a per-level **additive** offset, not a gain: the drift is a DC shift, and an
/// offset removes it without touching the ripple's contrast — where a multiplicative fit would
/// scale the crests (alpha reaches 255) and clip them. Applied per channel because both terms of
/// the ADT combine carry the drift (`detail.rgb`, and `detail.a` through the sheen factor).
///
/// **Water/ocean only.** Magma and slime take the animated sheet as their opaque BODY colour, where
/// a per-frame brightness swing is the intended pulse, so the caller passes `false` for the
/// fullbright kinds and their DC is left alone.
///
/// **This is a deliberate divergence from the shipped art**, pending the RE round on whether the
/// reference shows the same breathing (it uploads the same authored, same-quantised mips, so it
/// plausibly does). `WOW_LIQUID_DC=raw` restores the frames verbatim for the A/B.
fn flatten_frame_dc(data: &mut [u8], spans: &[Vec<(usize, usize)>], levels: usize) {
    for level in 0..levels {
        // The loop's target sum-per-texel for this level, over every frame.
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
                // Spend the delta in ±1 steps, walking the level on a stride coprime with its
                // texel count so the touched texels scatter instead of forming a patch at the
                // start. Repeated passes each add at most 1 LSB per texel, so a large correction
                // stays a faint uniform lift rather than a few blown texels; texels already at the
                // 0/255 rail are skipped, and a pass that moves nothing ends the walk.
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

/// Stack the animated liquid frames into one **mipmapped + anisotropic** `texture_2d_array` from each
/// frame's BLP **authored** mip pyramid — same fidelity rule as [`repeat_texture_authored`]: the real
/// 1.12 client uploads the BLP's stored mips verbatim (no client-side regeneration). Without mips, the
/// 256² ripple tiled over ~16 yd aliases into dense sparkle "noise" at distance; the authored mips +
/// anisotropy give the reference's smooth, streaky wave highlights instead. Data is laid out
/// **LayerMajor** (`frame0[mip0..mipN], frame1[mip0..], …`, the wgpu/Bevy default) with a full
/// `size→1` chain; a frame whose authored chain is shorter/odd is filled by `NEAREST` from its nearest
/// authored level (gamma-safe, no averaging — mirrors [`chain_to_layer`]). All frames must share mip-0
/// `size` (the caller enforces this). Returns a `D2Array`-viewed image with `mip_level_count` set.
pub fn liquid_frame_array(frames: Vec<BlpMipChain>, normalize_dc: bool) -> Image {
    let size = frames[0].width;
    // Full mip chain down to 1×1 (square 1.12 frames): levels = log2(size) + 1.
    let mip_level_count = {
        let (mut n, mut s) = (1u32, size);
        while s > 1 {
            s >>= 1;
            n += 1;
        }
        n
    };
    let mut data = Vec::with_capacity(frames.len() * mip_chain_byte_size(size, mip_level_count));
    // Where each (frame, level) region landed, so the DC pass below can address one level of one
    // frame without re-deriving the LayerMajor arithmetic.
    let mut spans: Vec<Vec<(usize, usize)>> = Vec::with_capacity(frames.len());
    for blp in &frames {
        let mut per_level = Vec::with_capacity(mip_level_count as usize);
        for level in 0..mip_level_count {
            let lw = (size >> level).max(1);
            // Match the array level to the authored mip of the same size (normally identity — water
            // frames carry a full chain); fall back to the nearest authored level via NEAREST.
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
        // `RENDER_WORLD`: 136 frames of a 256² nine-level RGBA8 pyramid is 47,535,264 B (the figure
        // `gpu_bytes_count_every_array_layer` pins), built at `Startup` and resident for the whole
        // process. The default usage held that twice — once on the GPU and once in system RAM,
        // which nothing ever read.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mip_level_count;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    // **The apitrace that pinned this was read one step too far.** WoW.20 showed the ripple
    // sampler at LINEAR_MIPMAP_LINEAR + MAX_ANISOTROPY 16 and it was recorded here as "VERIFIED
    // the reference ripple sampler" — but that capture came off the repo's reference install,
    // whose `Config.wtf` carries `SET anisotropic "16"`. It proves the *mechanism* (the global
    // reaches the liquid sampler like every other), not the *default*, which is aniso off. There
    // is no per-texture filter override anywhere in the reference, so water takes the same policy
    // as ground ([`crate::tex_filter`]) — and at `anisotropic 16` this lane is byte-identical to
    // what it was.
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

/// A single-mip RGBA8 image for winit's custom OS cursor (`CursorIcon::Custom`). Vanilla cursors are
/// tiny (`Point.blp` is 32×32), so unlike [`repeat_texture`] this needs neither a mip chain nor a
/// repeat sampler. macOS doesn't use this path (it builds an `NSCursor` directly — see `crate::cursor`).
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

/// A single-mip, **sRGB**, clamp-sampled image for an emissive celestial billboard (the sun/moon
/// discs — see [`WorldAssets::sprite_texture`]). sRGB so an unlit pass round-trips the authored gamma
/// bytes to screen; clamp-to-edge + linear (no repeat, no mips) since the whole texture maps to one
/// quad and the disc never tiles or minifies far.
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

/// A coverage **mask** image (the minimap's `MinimapMask.blp`, decision 0203): identical sampling
/// to [`sprite_image`] (clamp, linear filtering, no mips) but uploaded **`Rgba8Unorm`** (not
/// sRGB) — the shader reads coverage, not color, so no gamma decode belongs anywhere in the path.
/// The coverage itself rides the ALPHA channel (`MinimapMask.blp` is DXT3: white color plane, the
/// circle ramp authored in its 8-bit alpha; alpha is gamma-free in either format, but the linear
/// upload keeps the color plane byte-exact too, and documents the intent).
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

/// A portrait sprite: [`sprite_image`] with the RGBA pre-masked to its **inscribed circle** — the
/// alpha of every texel outside the largest centred circle is zeroed (with a 1px anti-aliased edge),
/// so a square icon or model reads as the round portrait the unit-frame ring expects. The ring in
/// `UI-TargetingFrame` is a thin band with transparent corners; without the mask the square texture's
/// corners poke past it (the placeholder `INV_Misc_QuestionMark` even bakes a grey border, so its
/// corners read as metal brackets). Same sRGB/clamp/linear/no-mip setup as [`sprite_image`].
pub fn portrait_image(width: u32, height: u32, mut rgba: Vec<u8>) -> Image {
    if width > 0 && height > 0 && rgba.len() == (width * height * 4) as usize {
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let radius = cx.min(cy);
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Coverage: 1 well inside, 0 well outside, a 1px linear ramp across the edge.
                let coverage = (radius - dist).clamp(0.0, 1.0);
                let a = ((y * width + x) * 4 + 3) as usize;
                rgba[a] = (f32::from(rgba[a]) * coverage).round() as u8;
            }
        }
    }
    sprite_image(width, height, rgba)
}

/// A single-mip, **sRGB**, **repeat**-sampled image for a frame `Backdrop`'s tiled pieces (the
/// tooltip/panel border edges + a tiling background — `0x77e8d0`/`0x77f0c0`). Identical to
/// [`sprite_image`] except the address mode is `Repeat`: a backdrop edge strip runs UVs `[0..N]`
/// (N = the edge count = frame-side / edgeSize − 2) and a tiled bg runs `[0..w/period]`, so the
/// slice must *wrap*, not clamp-stretch. The real client flags exactly this on the backdrop's
/// `SetTexture` (arg2 pushed twice into the load descriptor — `0x770200`, inferred U+V wrap, not
/// traced to the sampler). sRGB + no mips, same as the clamp sprite (UI art, one authored gamma
/// round-trip).
///
/// Both axes wrap — the backdrop's bg tiles both ways, and its edge strips are atlas crops on
/// their bounded axis, kept off the image edge by `inset_atlas_bleed`. A texture that tiles
/// along ONE axis and spans the whole image on the other takes [`sprite_image_wrapped`] instead
/// (decision 2000).
pub fn sprite_image_tiled(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    sprite_image_wrapped(width, height, rgba, (true, true))
}

/// [`sprite_image`]'s decode with the address mode chosen **per axis**: `Repeat` where `wrap`
/// says so, `ClampToEdge` elsewhere.
///
/// The reference's tiling idiom, `SetTexCoord(0, n, 0, 1)` on an n-slot strip, runs past the
/// texture along one axis only; the other spans exactly `[0, 1]`. Sampling that bounded axis
/// with `Repeat` is a bleed: bilinear filtering at `v = 0` weighs in the texture's LAST row, so a
/// strip whose bottom row is opaque draws that row as a faint hairline along its own top edge —
/// the stance shelf's middle piece (`ShapeshiftBarMiddle.blp`: rows 0-7 transparent, row 31
/// opaque grey) wore a one-device-px grey line across the top of the strip at every four-form
/// bar, over the world. Wrapping only the axis that actually tiles is the fix at the root: the
/// bounded axis clamps at its edge, exactly as a stand-alone clamped sprite would (decision 2000).
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

/// The bytes an [`Image`] occupies **on the GPU** — every mip level of every array layer, derived
/// from the `texture_descriptor` alone.
///
/// Read from the descriptor, never from `image.data.len()`: an asset declared
/// [`RenderAssetUsages::RENDER_WORLD`] has its bytes *moved* into the render world on extract
/// (`bevy_render::render_asset`), so main-side `data` is `None` for its whole life. A meter that
/// measures `data` therefore reads 0 for exactly the textures that cost the most — the terrain
/// arrays, the sprite sheets, the world albedo. This is the arithmetic bevy_image itself uses, and
/// it is block-aware, so it stays right if a format is ever uploaded compressed.
pub fn image_gpu_bytes(image: &Image) -> usize {
    let d = &image.texture_descriptor;
    let format = d.format;
    let (bw, bh) = format.block_dimensions();
    // A copy is defined per block, and a block is only whole: a 1×1 mip of a 4×4-block format still
    // costs one full block.
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

/// Append `src` (RGBA8 `sw × sh`) resized to `dw × dh` to `out`, using `NEAREST` filtering in
/// gamma byte space. NEAREST = pure texel-replication (or decimation), so no averaging happens —
/// preserves gamma-encoded bytes verbatim (no darkening). Identity at `(sw, sh) == (dw, dh)`.
pub fn extend_nearest(out: &mut Vec<u8>, src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) {
    if sw == dw && sh == dh {
        out.extend_from_slice(src);
        return;
    }
    let Some(buf) = image::RgbaImage::from_raw(sw, sh, src.to_vec()) else {
        // Shape mismatch — fall back to a black layer rather than panicking.
        out.extend(std::iter::repeat_n(0u8, (dw * dh * 4) as usize));
        return;
    };
    let resized = image::imageops::resize(&buf, dw, dh, image::imageops::FilterType::Nearest);
    out.extend_from_slice(resized.as_raw());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two 2x2 frames whose means differ by a lot, with a full 2-level chain each — the smallest
    /// shape that exercises the per-level DC pass.
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

    /// The whole point of [`flatten_frame_dc`]: after it, no frame's level mean differs from any
    /// other's, so a mip-averaged sheet cannot brighten and dim as the loop plays. This is the
    /// property the Ratchet report turned on — a 1.25 s whole-sheet breath measured at up to
    /// 1.84/255 — and a still frame can never show it, so it is asserted here instead.
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

    /// The lever back to the shipped art, and the control for the test above: with
    /// `WOW_LIQUID_DC=raw` the frames keep their own DC, so the drift the fix removes is still
    /// there. Without this, a no-op `flatten_frame_dc` would pass the assertion above trivially.
    #[test]
    fn raw_lever_keeps_the_authored_per_frame_dc() {
        // The pass is skipped when the caller opts out, which is the fullbright (magma/slime) lane.
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

    /// The arithmetic must agree with [`mip_chain_byte_size`], which the liquid array already
    /// budgets with — two independent expressions of the same pyramid.
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

    /// The number the liquid lane costs, stated once so it cannot drift silently: 136 frames of a
    /// 256² RGBA8 nine-level pyramid, stacked as array layers.
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

    /// Block-compressed formats are the point of reading the descriptor rather than a pixel size:
    /// a 4×4-block format still owes a WHOLE block for its 2×2 and 1×1 mips.
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

    /// The stance shelf's case: tiles along its length, spans the whole texture in height — the
    /// height axis must CLAMP, or the strip's opaque bottom row bleeds into its top edge.
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

    /// The backdrop's case is unchanged: both axes wrap.
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
