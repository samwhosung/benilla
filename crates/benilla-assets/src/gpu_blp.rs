//! Whether a BLP's stored S3TC blocks can go to the GPU untouched, as the reference uploads them
//! (`glCompressedTexImage2DARB`; its software DXT decoder is a 16-bit-device fallback, device
//! formats `0x58a230`, per-level upload formats `0x59f270`). They can when the device accepts BC
//! ([`bc_supported`]) and mip 0 is whole 4x4 blocks; otherwise the chain decodes to `Rgba8Unorm`.
//! Both the async loader ([`crate::blp`]) and `WorldAssets` ([`crate::world_assets`]) ask here.

use std::sync::OnceLock;

use benilla_formats::{BlpMipChain, BlpTexels};
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

static BC_SUPPORTED: OnceLock<bool> = OnceLock::new();

/// Publish whether this run's GPU accepts BC, once, from a plugin's `finish()`. A `OnceLock`, not
/// a resource, because the async loader has no world access; every `finish()` runs before the
/// first update, so before any BLP loads. Headless, nothing publishes and the decoded path runs.
pub fn publish_bc_support(supported: bool) {
    let _ = BC_SUPPORTED.set(supported);
}

/// Whether this run's GPU accepts BC; `false` until [`publish_bc_support`].
pub fn bc_supported() -> bool {
    *BC_SUPPORTED.get().unwrap_or(&false)
}

/// Publishes [`bc_supported`] from the render device; added by [`crate::register_asset_loaders`].
/// The work is in `finish`: `RenderPlugin::finish` inserts `CompressedImageFormatSupport`, which
/// does not exist at `build` time or in a headless app.
pub struct BlpGpuSupportPlugin;

impl Plugin for BlpGpuSupportPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        // `WOW_NO_BC=1` forces the decoded path: the A/B lever, and the first thing to try when a
        // texture looks wrong (hardware block decode is not bit-identical to `texpresso`'s).
        let forced_off = std::env::var("WOW_NO_BC").as_deref() == Ok("1");
        let device_can = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .is_some_and(|s| s.0.contains(CompressedImageFormats::BC));
        let supported = device_can && !forced_off;
        publish_bc_support(supported);
        // At `info`, so a player's log says which texture lane the run is on.
        if supported {
            info!("blp: DXT blocks upload natively (BC)");
        } else if forced_off {
            info!("blp: WOW_NO_BC=1 — decoding textures to RGBA8");
        } else {
            info!("blp: device reports no BC support — decoding textures to RGBA8");
        }
    }
}

fn uploadable_as_blocks(chain: &BlpMipChain) -> bool {
    chain.texels.is_block_compressed()
        && bc_supported()
        && chain.width.is_multiple_of(4)
        && chain.height.is_multiple_of(4)
}

/// A chain and the `TextureFormat` it uploads as, decided together so block bytes can never meet
/// an uncompressed descriptor.
pub struct UploadChain {
    pub chain: BlpMipChain,
    pub format: TextureFormat,
}

/// A BLP's authored mip chain in the best form this GPU takes, with its format: blocks if it can,
/// pixels if not. Always non-sRGB, so the GPU never linearizes albedo on sample.
pub fn for_upload(chain: BlpMipChain) -> UploadChain {
    if uploadable_as_blocks(&chain) {
        let format = match chain.texels {
            BlpTexels::Bc1 => TextureFormat::Bc1RgbaUnorm,
            BlpTexels::Bc2 => TextureFormat::Bc2RgbaUnorm,
            BlpTexels::Bc3 => TextureFormat::Bc3RgbaUnorm,
            // `uploadable_as_blocks` already excluded this arm.
            BlpTexels::Rgba8Unorm => TextureFormat::Rgba8Unorm,
        };
        return UploadChain { chain, format };
    }
    // `into_rgba8` is a no-op on pixels and an infallible decode of blocks.
    UploadChain {
        chain: chain.into_rgba8(),
        format: TextureFormat::Rgba8Unorm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(texels: BlpTexels, w: u32, h: u32) -> BlpMipChain {
        let mips = vec![vec![0u8; texels.level_bytes(w, h)]];
        BlpMipChain {
            width: w,
            height: h,
            texels,
            mips,
        }
    }

    #[test]
    fn no_published_support_means_the_decoded_path() {
        // `publish_bc_support` is never called in this test binary, so the lock stays empty.
        assert!(!bc_supported());
        let up = for_upload(chain(BlpTexels::Bc1, 16, 16));
        assert_eq!(up.format, TextureFormat::Rgba8Unorm);
        assert!(up.chain.is_rgba8(), "blocks must have been decoded");
    }

    #[test]
    fn the_returned_format_always_matches_the_returned_bytes() {
        for texels in [
            BlpTexels::Rgba8Unorm,
            BlpTexels::Bc1,
            BlpTexels::Bc2,
            BlpTexels::Bc3,
        ] {
            for (w, h) in [(16u32, 16u32), (8, 32), (258, 256), (6, 6)] {
                let up = for_upload(chain(texels, w, h));
                let expected = match up.format {
                    TextureFormat::Rgba8Unorm => BlpTexels::Rgba8Unorm,
                    TextureFormat::Bc1RgbaUnorm => BlpTexels::Bc1,
                    TextureFormat::Bc2RgbaUnorm => BlpTexels::Bc2,
                    TextureFormat::Bc3RgbaUnorm => BlpTexels::Bc3,
                    other => panic!("unexpected albedo format {other:?}"),
                };
                assert_eq!(up.chain.texels, expected, "{texels:?} at {w}x{h}");
                assert_eq!(
                    up.chain.mips[0].len(),
                    expected.level_bytes(w, h),
                    "mip 0 bytes must match what {:?} implies at {w}x{h}",
                    up.format
                );
            }
        }
    }

    /// wgpu-core rejects a compressed texture that is not whole blocks
    /// (`NotMultipleOfBlockWidth/Height`), and its default error handler panics.
    #[test]
    fn non_block_aligned_dimensions_are_refused() {
        for (w, h) in [(258u32, 256u32), (256, 258), (6, 6), (1, 1)] {
            assert!(
                !uploadable_as_blocks(&chain(BlpTexels::Bc1, w, h)),
                "{w}x{h} is not whole blocks"
            );
        }
    }

    #[test]
    fn an_rgba_chain_passes_through_untouched() {
        let c = chain(BlpTexels::Rgba8Unorm, 8, 8);
        let before = c.mips[0].len();
        let up = for_upload(c);
        assert_eq!(up.format, TextureFormat::Rgba8Unorm);
        assert_eq!(up.chain.mips[0].len(), before);
    }
}
