//! Where a BLP's stored form meets the GPU: **can we hand the blocks over untouched?**
//!
//! WoW ships almost all world art as S3TC (DXT1/DXT3/DXT5) blocks, and the reference client uploads
//! those blocks verbatim — `glCompressedTexImage2DARB`, with the software DXT decoder existing only
//! as a 16-bit-device fallback (the device-format query `0x58a230`, where DXT is a raw passthrough
//! the device eats; the per-level upload formats at `0x59f270`).
//! benilla used to decode every one of them to `Rgba8Unorm` on the CPU and upload that: **8x the
//! bytes of DXT1, 4x of DXT3/DXT5**, on the whole resident working set and on every texture fetch
//! that misses the cache. On a bandwidth-starved integrated GPU that is the frame's dominant cost,
//! and a Steam Deck is one (decision 1626).
//!
//! Two things have to be true before the blocks can go straight over, and this module owns both:
//!
//! 1. **The device must accept BC.** [`bc_supported`] is a process-wide fact published once from
//!    the render device (see [`publish_bc_support`]). Not a guess: `wgpu-hal`'s Metal backend gates
//!    `TEXTURE_COMPRESSION_BC` on `format_bc: os_is_mac` and the Vulkan backend on the device's
//!    `textureCompressionBC`, and bevy's `WgpuSettingsPriority::Functionality` (the default) requests
//!    every feature the adapter reports. Every desktop target we ship has it — but "every target we
//!    ship" is not "every device", so the fallback stays.
//! 2. **The dimensions must be whole 4x4 blocks.** A BC texture is addressed in blocks; a BLP whose
//!    mip 0 is not a multiple of 4 on both axes cannot be uploaded as one. Real 1.12 world art is
//!    power-of-two, so this practically never fires — it exists so that when it does, the texture
//!    renders decoded instead of failing.
//!
//! When either is false the caller decodes, exactly as before. That is why the fallback lives here
//! as one function and not as a flag threaded through two loaders: there is one rule, and both the
//! async `AssetLoader` lane ([`crate::blp`]) and the synchronous `WorldAssets` lane
//! ([`crate::world_assets`]) ask it the same question.

use std::sync::OnceLock;

use benilla_formats::{BlpMipChain, BlpTexels};
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

static BC_SUPPORTED: OnceLock<bool> = OnceLock::new();

/// Publish whether this run's GPU accepts BC textures. Called once, from the app's plugin
/// `finish()`, after `RenderPlugin` has inserted `CompressedImageFormatSupport` into the main world.
///
/// **Ordering is why this is a `OnceLock` and not a resource.** An `AssetLoader` runs on the async
/// task pool with no world access, and the synchronous `WorldAssets` lane is called from ordinary
/// systems — neither can read a resource the other can. But every plugin's `finish()` completes
/// before the runner's first update, and no BLP is loaded before then, so a single process-wide
/// publish is both sufficient and race-free. Absent a render device (headless tests), nothing
/// publishes and [`bc_supported`] reports `false` — the decoded path, which is what a test that
/// inspects pixels wants anyway.
pub fn publish_bc_support(supported: bool) {
    let _ = BC_SUPPORTED.set(supported);
}

/// Does this run's GPU accept BC? `false` until [`publish_bc_support`] says otherwise.
pub fn bc_supported() -> bool {
    *BC_SUPPORTED.get().unwrap_or(&false)
}

/// Publishes [`bc_supported`] from the live render device. Added by
/// [`crate::register_asset_loaders`], so both the client and the world viewer get it.
///
/// **All the work is in `finish`, deliberately.** `RenderPlugin::finish` is what inserts
/// `CompressedImageFormatSupport` into the main world (bevy_render `lib.rs`, the
/// `FutureRenderResources` block) — it does not exist at `build` time, and it never exists at all
/// in a headless app. Reading it here, one plugin later, is the earliest honest moment.
pub struct BlpGpuSupportPlugin;

impl Plugin for BlpGpuSupportPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        // `WOW_NO_BC=1` — force the decoded path on a device that could take blocks. The A/B lever
        // for this whole change: it is the only way to price the passthrough against what it
        // replaced on one machine in one session, and the first thing to try if a texture ever
        // looks wrong (hardware block decode is not bit-identical to `texpresso`'s — see 1626).
        let forced_off = std::env::var("WOW_NO_BC").as_deref() == Ok("1");
        let device_can = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .is_some_and(|s| s.0.contains(CompressedImageFormats::BC));
        let supported = device_can && !forced_off;
        publish_bc_support(supported);
        // At `info`, not `debug`: "which texture lane am I on?" is the first question any perf or
        // look report about this change has to answer, and a line that only exists in a debug build
        // cannot answer it from a player's log.
        if supported {
            info!("blp: DXT blocks upload natively (BC)");
        } else if forced_off {
            info!("blp: WOW_NO_BC=1 — decoding textures to RGBA8");
        } else {
            info!("blp: device reports no BC support — decoding textures to RGBA8");
        }
    }
}

/// Can `chain` be uploaded in the form it is already in?
fn uploadable_as_blocks(chain: &BlpMipChain) -> bool {
    chain.texels.is_block_compressed()
        && bc_supported()
        && chain.width.is_multiple_of(4)
        && chain.height.is_multiple_of(4)
}

/// A chain and the `TextureFormat` it uploads as — **decided together, returned together.**
///
/// This pairing is the whole point of the type. The first cut of this module had two functions, one
/// that picked the chain and one that picked the format, and a caller reached the second without
/// going through the first: it uploaded DXT3 blocks under an `Rgba8Unorm` descriptor and wgpu
/// panicked slicing 262144 bytes out of an 87392-byte chain. There is no way to hold one of these
/// without the other, so that class of mistake cannot recur.
pub struct UploadChain {
    pub chain: BlpMipChain,
    pub format: TextureFormat,
}

/// A BLP's authored mip chain in the best form this GPU can take, with the format that matches it:
/// **blocks if it can, pixels if it cannot.**
///
/// The format is always in the **gamma-byte (non-sRGB) lane** every world/model albedo lives in —
/// the RE'd invariant that the GPU must not linearize albedo on sample. `Bc1RgbaUnorm` and friends
/// carry that rule exactly as `Rgba8Unorm` does, which is what makes the passthrough look-neutral.
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
    // Either it was never blocks, or this device/texture cannot take them. `into_rgba8` is a no-op
    // in the first case and the decode we were trying to avoid in the second — and it cannot fail,
    // which is why the format below is unconditionally right.
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

    /// Without a published device fact we decode. A headless test that reads pixels depends on
    /// this, and so does any run whose GPU genuinely lacks BC.
    #[test]
    fn no_published_support_means_the_decoded_path() {
        // `publish_bc_support` is never called in this test binary, so the lock stays empty.
        assert!(!bc_supported());
        let up = for_upload(chain(BlpTexels::Bc1, 16, 16));
        assert_eq!(up.format, TextureFormat::Rgba8Unorm);
        assert!(up.chain.is_rgba8(), "blocks must have been decoded");
    }

    /// **The invariant this module exists to hold**: whatever comes back, the bytes and the format
    /// agree about how big mip 0 is. This is the assertion whose absence let block bytes be
    /// uploaded under an `Rgba8Unorm` descriptor.
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

    /// The block-alignment rule is on mip 0's dimensions, both axes. wgpu-core rejects a compressed
    /// texture whose size is not a whole number of blocks (`NotMultipleOfBlockWidth/Height`), and
    /// its default error handler makes that a process panic — so this is a hard gate, not a
    /// preference.
    #[test]
    fn non_block_aligned_dimensions_are_refused() {
        for (w, h) in [(258u32, 256u32), (256, 258), (6, 6), (1, 1)] {
            assert!(
                !uploadable_as_blocks(&chain(BlpTexels::Bc1, w, h)),
                "{w}x{h} is not whole blocks"
            );
        }
    }

    /// An already-decoded chain is never re-encoded, and never copied through a decode path.
    #[test]
    fn an_rgba_chain_passes_through_untouched() {
        let c = chain(BlpTexels::Rgba8Unorm, 8, 8);
        let before = c.mips[0].len();
        let up = for_upload(c);
        assert_eq!(up.format, TextureFormat::Rgba8Unorm);
        assert_eq!(up.chain.mips[0].len(), before);
    }
}
