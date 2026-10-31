//! BLP to Bevy [`Image`] loader, in the [`BlpVariant`] the load settings choose.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages};
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use serde::{Deserialize, Serialize};

use benilla_formats::{
    blp_bytes_to_native_chain, blp_to_rgba, tga_texture_size, tga_to_rgba, BlpMipChain, BlpTexels,
};

use crate::gpu_blp::{for_upload, UploadChain};

/// Which on-GPU form a BLP decodes to.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BlpVariant {
    /// World and model albedo, the default: non-sRGB, so shader math stays in gamma bytes; the
    /// authored mips as stored (the 1.12 client never regenerates them); the process filter policy.
    #[default]
    WorldArt,
    /// Emissive billboard (the sun and moon discs): `Rgba8UnormSrgb`, clamp, mip 0 only.
    Sprite,
    /// Gamma-lane effect sprite (rain splashes, weather mist): `Rgba8Unorm`, clamp, mip 0 only, as
    /// these quads are mostly magnified and their thin cut-out arms fade out down the mips.
    Effect,
    /// The snow flake: [`Self::Effect`] with the authored mips kept. The reference draws it as
    /// `GL_POINTS` with `GL_COORD_REPLACE` (`0x678610`; device init `0x59cf30`-`0x59cf58`), 14 px
    /// at the eye and 1 px past 46 yd, where mip 0 alone aliases into speckle.
    PointSprite,
    /// A minimap tile (ADT `map<X>_<Y>` or a WMO group's interior tile): [`map_tile_image`].
    MapTile,
    /// OS cursor image: `Rgba8UnormSrgb`, single mip.
    Cursor,
}

/// Per-load settings for [`BlpImageLoader`]: choose the [`BlpVariant`] via `load_with_settings`.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct BlpLoaderSettings {
    pub variant: BlpVariant,
}

/// Bevy [`AssetLoader`] decoding `*.blp` to [`Image`].
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
        // The reader hands over the first candidate of `TextureCreate`'s chain that opens
        // (`0x449d90`), which is a `.tga` where no `.blp` is there or the name asked for it.
        if !bytes.starts_with(b"BLP") && tga_texture_size(&bytes).is_some() {
            return tga_image(
                &bytes,
                settings.variant,
                crate::sampler_mode_of(&ctx.path().to_string()),
            )
            .map_err(to_io);
        }
        Ok(match settings.variant {
            BlpVariant::WorldArt => world_art_image(
                for_upload(blp_bytes_to_native_chain(&bytes).map_err(to_io)?),
                // The address mode rides the asset path (`crate::texture_url`): one `.blp` can be
                // uploaded under two samplers.
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

/// A `.tga` candidate of `TextureCreate`'s chain (`0x449d90`) as the `variant` lane draws it: one
/// level of decoded pixels, as the interface's TGAs are.
fn tga_image(bytes: &[u8], variant: BlpVariant, wrap: (bool, bool)) -> anyhow::Result<Image> {
    let (w, h, rgba) = tga_to_rgba(bytes)?;
    let one_level = |rgba| {
        for_upload(BlpMipChain {
            width: w,
            height: h,
            texels: BlpTexels::Rgba8Unorm,
            mips: vec![rgba],
        })
    };
    Ok(match variant {
        BlpVariant::WorldArt => world_art_image(one_level(rgba), wrap),
        BlpVariant::Sprite => sprite_image(w, h, rgba),
        BlpVariant::MapTile => map_tile_image(w, h, rgba),
        BlpVariant::Effect => effect_image(w, h, rgba),
        BlpVariant::PointSprite => point_sprite_image(one_level(rgba)),
        BlpVariant::Cursor => cursor_image(w, h, rgba),
    })
}

/// The gamma lanes' uncompressed format: non-sRGB, so the GPU does not linearize on sample.
const GAMMA_BYTES: TextureFormat = TextureFormat::Rgba8Unorm;

fn world_art_image(upload: UploadChain, wrap: (bool, bool)) -> Image {
    let UploadChain { chain, format } = upload;
    let levels = chain.mips.len() as u32;
    let mut data = Vec::with_capacity(chain.mips.iter().map(Vec::len).sum());
    for mip in &chain.mips {
        data.extend_from_slice(mip);
    }
    // `new_uninit`: `Image::new` would clone mip 0 only to feed a length assert meant for
    // uncompressed formats.
    let mut image = Image::new_uninit(
        Extent3d {
            width: chain.width,
            height: chain.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        // The render world takes the chain on extract rather than cloning it and keeping a
        // main-world copy (bevy_render `render_asset.rs`); nothing reads world art main-side.
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
    // The reference's terrain, WMO and M2 textures take the mip filter and anisotropy from the
    // process policy (`0x449ae0`, `crate::tex_filter`).
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

/// Emissive billboard: sRGB, so an unlit pass returns the authored gamma bytes to screen.
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

/// A minimap tile, sampled as the reference samples every tile (the tile loader `0x6d9ed0`): clamp,
/// `GL_LINEAR` with no mips, anisotropy 1 and `GL_SKIP_DECODE_EXT`, so the hardware filters the
/// gamma bytes. An sRGB upload would darken every alpha edge against black, so the tile uploads as
/// [`GAMMA_BYTES`] and the shader converts after the filter.
fn map_tile_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    effect_image(width, height, rgba)
}

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

/// No anisotropy: a point sprite is square and screen-aligned, with no anisotropic footprint.
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
        mipmap_filter: crate::tex_filter::tex_filter().mipmap_filter(),
        ..Default::default()
    });
    image
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 uncompressed 32-bit TGA: B G R A per texel, bottom-up.
    fn tga_2x1() -> Vec<u8> {
        let mut b = vec![0u8; 18];
        b[2] = 2;
        b[12] = 2;
        b[14] = 1;
        b[16] = 32;
        b[17] = 8;
        b.extend_from_slice(&[10, 20, 30, 255, 40, 50, 60, 128]);
        b
    }

    /// A `.tga` candidate of the texture chain decodes in every lane, where a BLP parse of it
    /// refuses the file.
    #[test]
    fn a_tga_candidate_decodes_in_every_lane() {
        let tga = tga_2x1();
        assert!(benilla_formats::blp_to_rgba(&tga).is_err());
        for variant in [
            BlpVariant::WorldArt,
            BlpVariant::Sprite,
            BlpVariant::MapTile,
            BlpVariant::Effect,
            BlpVariant::PointSprite,
            BlpVariant::Cursor,
        ] {
            let image = tga_image(&tga, variant, (true, true)).expect("a TGA decodes");
            assert_eq!(
                (image.width(), image.height()),
                (2, 1),
                "{variant:?} keeps the TGA's size"
            );
        }
    }
}
