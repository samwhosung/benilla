//! BLP → Bevy [`Image`] loader.
//!
//! Decodes a WoW BLP texture into an `Image` in one of several variants (selected by loader
//! settings):
//! - [`BlpVariant::WorldArt`] — tiling world/model albedo: `Rgba8Unorm` (the RE'd gamma-space
//!   invariant — the GPU does *not* linearize albedo on sample, so shader math stays in WoW's
//!   gamma/byte space), the BLP's **authored** mip pyramid used verbatim (the real 1.12 client cannot
//!   regenerate mips — it uploads the stored ones), repeat + the process filter policy
//!   ([`crate::tex_filter`]).
//! - [`BlpVariant::Sprite`] — emissive billboards (sun/moon discs): `Rgba8UnormSrgb`, clamp, mip 0.
//! - [`BlpVariant::Cursor`] — the OS cursor image: `Rgba8UnormSrgb`, single mip.
//!
//! Fidelity rationale (authored mips, the gamma-space `Unorm` invariant) is anchored in the
//! reference client; here we only build the GPU resource.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages};
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use serde::{Deserialize, Serialize};

use benilla_formats::{blp_bytes_to_native_chain, blp_to_rgba};

use crate::gpu_blp::{for_upload, UploadChain};

/// Which on-GPU form a BLP decodes to. See the module docs.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BlpVariant {
    /// Tiling world/model albedo — `Rgba8Unorm`, authored mips, repeat + the filter policy. The default.
    #[default]
    WorldArt,
    /// Emissive billboard (celestial disc) — `Rgba8UnormSrgb`, clamp, mip 0 only.
    Sprite,
    /// Gamma-lane effect sprite (rain splashes, weather mist) — `Rgba8Unorm` like
    /// [`Self::WorldArt`] but **mip 0 only** + clamp: these draw as world-space quads that are
    /// magnified far more often than minified, and their thin cut-out arms collapse to near-zero
    /// alpha under the authored mip chain.
    Effect,
    /// A **point sprite** — [`Self::Effect`]'s clamp and gamma lane, but with the BLP's
    /// **authored mip pyramid** kept (trilinear, no anisotropy). The snow flake's lane: the
    /// reference draws it through `glDrawArrays(GL_POINTS)` with `GL_COORD_REPLACE`, so the whole
    /// texture maps across a sprite that is **14 px at the eye and 1 px past 46 yd** — a 4.5×–64×
    /// *minification* of a 64×64 texture, which is why the asset ships 7 mip levels. Mip-0-only
    /// here is not "crisp", it is aliased: a 1-px flake samples one arbitrary texel of a dendrite
    /// and the field becomes flickering speckle.
    /// (The point-sprite leg `0x678610`; `GL_COORD_REPLACE` at device init, `0x59cf30`–`0x59cf58`.)
    PointSprite,
    /// A **minimap tile** (ADT `map<X>_<Y>` or a WMO group's interior tile) — clamp, mip 0, linear,
    /// and filtered in **gamma space**, the reference's `GL_SKIP_DECODE_EXT`. See [`map_tile_image`].
    MapTile,
    /// OS cursor image — `Rgba8UnormSrgb`, single mip.
    Cursor,
}

/// Per-load settings for [`BlpImageLoader`]: choose the [`BlpVariant`] via `load_with_settings`.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct BlpLoaderSettings {
    pub variant: BlpVariant,
}

/// Bevy [`AssetLoader`] decoding `*.blp` → [`Image`].
#[derive(Default, TypePath)]
pub struct BlpImageLoader;

impl AssetLoader for BlpImageLoader {
    type Asset = Image;
    type Settings = BlpLoaderSettings;
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &Self::Settings,
        ctx: &mut LoadContext<'_>,
    ) -> Result<Image, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let to_io = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));
        Ok(match settings.variant {
            BlpVariant::WorldArt => world_art_image(
                // The blocks go straight to the GPU where it can take them (decision 1626);
                // `for_upload` decodes only when it cannot, and hands back the matching format.
                for_upload(blp_bytes_to_native_chain(&bytes).map_err(to_io)?),
                // The address mode rides the asset path (`crate::texture_url`, decision 0763): the
                // sampler is a property of this upload, and one `.blp` legitimately has two.
                crate::sampler_mode_of(&ctx.path().to_string()),
            ),
            BlpVariant::Sprite => {
                let (w, h, rgba) = blp_to_rgba(&bytes).map_err(to_io)?;
                sprite_image(w, h, rgba)
            }
            BlpVariant::MapTile => {
                let (w, h, rgba) = blp_to_rgba(&bytes).map_err(to_io)?;
                map_tile_image(w, h, rgba)
            }
            BlpVariant::Effect => {
                let (w, h, rgba) = blp_to_rgba(&bytes).map_err(to_io)?;
                effect_image(w, h, rgba)
            }
            BlpVariant::PointSprite => point_sprite_image(for_upload(
                blp_bytes_to_native_chain(&bytes).map_err(to_io)?,
            )),
            BlpVariant::Cursor => {
                let (w, h, rgba) = blp_to_rgba(&bytes).map_err(to_io)?;
                cursor_image(w, h, rgba)
            }
        })
    }

    fn extensions(&self) -> &[&str] {
        &["blp"]
    }
}

/// The gamma-byte lane's uncompressed format — non-sRGB, so the GPU does not linearize on sample
/// and shader math stays in WoW's byte space (the RE'd faithful invariant). Every decoded lane
/// below uploads as this.
const GAMMA_BYTES: TextureFormat = TextureFormat::Rgba8Unorm;

/// World/model albedo: the BLP's authored mip pyramid laid in verbatim, repeat + the process
/// filter policy ([`crate::tex_filter`]). (Port of the old `repeat_texture_authored`.)
fn world_art_image(upload: UploadChain, wrap: (bool, bool)) -> Image {
    let UploadChain { chain, format } = upload;
    let levels = chain.mips.len() as u32;
    let mut data = Vec::with_capacity(chain.mips.iter().map(Vec::len).sum());
    for mip in &chain.mips {
        data.extend_from_slice(mip);
    }
    // `new_uninit` rather than `new`: `Image::new`'s length assert is written for uncompressed
    // formats (it skips itself when `pixel_size()` errors, which is every BC format), and feeding
    // it mip 0 just to satisfy that assert cost a clone of the largest level on every load.
    let mut image = Image::new_uninit(
        Extent3d {
            width: chain.width,
            height: chain.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        // `RENDER_WORLD`: the render world TAKES the mip chain on extract instead of cloning it and
        // leaving a main-world copy resident for the asset's life (bevy_render `render_asset.rs` —
        // the `asset_usage == RENDER_WORLD` branch moves, every other usage clones). This is the
        // variant carrying every terrain layer, model albedo and WMO albedo in the game, so the
        // default usage was paying a full mip-chain memcpy on every landing frame and holding a
        // second copy of the whole world's art in system RAM. Nothing reads a world texture
        // main-side (the minimap, colliders and clutter all derive from other sources) — same
        // reasoning, and now the same usage, as the terrain tile arrays (`terrain.rs`) and the
        // Sprite/Effect/PointSprite variants below.
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    let mode = |repeat: bool| {
        if repeat {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        }
    };
    // The mip filter and the anisotropy are NOT this lane's to choose: the reference forces both
    // from two process globals at every `TextureCreate` (`0x449ae0`), and what a virgin install
    // gets from `hwDetect` is trilinear with anisotropy OFF. See [`crate::tex_filter`] — this used
    // to read `Linear` / `8`, which is mode 5 shipped as if it were the default.
    let filter = crate::tex_filter::tex_filter();
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode(wrap.0),
        address_mode_v: mode(wrap.1),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: filter.mipmap_filter(),
        anisotropy_clamp: filter.anisotropy_clamp(),
        ..Default::default()
    });
    image
}

/// Emissive billboard (celestial disc): sRGB so an unlit pass round-trips the authored gamma bytes to
/// screen; clamp + linear, mip 0 only (the disc maps to one quad and never tiles/minifies far).
fn sprite_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
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
        ..Default::default()
    });
    image
}

/// A **minimap tile** — clamp, mip 0, **LINEAR**, and filtered in **GAMMA** space.
///
/// The reference binds one sampler object for every minimap tile it draws, interior and outdoor
/// alike, and the trace reads it whole: `CLAMP_TO_EDGE` both axes, `MAG_FILTER = MIN_FILTER =
/// GL_LINEAR` with no mip term, LOD bias 0, anisotropy 1, and — the part that is not the default —
/// **`GL_TEXTURE_SRGB_DECODE_EXT = GL_SKIP_DECODE_EXT`**, plus `GL_TEXTURE_MAX_LEVEL = 0` per tile.
/// (A reference capture, matching the binary derivation at the tile loader `0x6d9ed0`; a positive
/// control in the same trace emits `GL_NEAREST` for a different sampler, so the layer would have
/// shown point sampling had the client asked for it.)
///
/// SKIP_DECODE is the whole reason this is not [`sprite_image`]: it means the hardware **filters the
/// authored bytes**, not their linearisation. An `Rgba8UnormSrgb` upload decodes each texel to
/// linear before the filter weights it, so every blend across an edge lands on a different colour
/// than the reference's — darker, because averaging in linear space and re-encoding is darker than
/// averaging the gamma bytes. On a 1-bit-alpha bake whose edges sit against black that is a visible
/// dark fringe at every alpha boundary. So the tile arrives as gamma bytes ([`GAMMA_BYTES`], no
/// decode) and the shader converts AFTER the filter.
fn map_tile_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    effect_image(width, height, rgba)
}

/// Gamma-lane effect sprite: `Rgba8Unorm` (the shader does WoW's byte-space combine), clamp,
/// **mip 0 only** — see [`BlpVariant::Effect`].
fn effect_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        GAMMA_BYTES,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..Default::default()
    });
    image
}

/// Point sprite: [`effect_image`]'s gamma lane and clamp, with the **authored mip pyramid** —
/// see [`BlpVariant::PointSprite`]. No anisotropy: a point sprite is always screen-axis-aligned
/// and square, so there is no anisotropic footprint to correct for.
fn point_sprite_image(upload: UploadChain) -> Image {
    let UploadChain { chain, format } = upload;
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
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        // A mip chain means the policy owns the mip filter here too — the reference's override is
        // unconditional across every texture it creates, with no per-class exemption. The aniso
        // stays absent by the argument above, and the policy agrees in modes 3 and 4.
        mipmap_filter: crate::tex_filter::tex_filter().mipmap_filter(),
        ..Default::default()
    });
    image
}

/// OS cursor image: sRGB, single mip (cursors are tiny and need neither a mip chain nor repeat).
fn cursor_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
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
