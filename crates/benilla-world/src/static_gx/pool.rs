//! The retained pass's shared texture-array pool. A texture keeps one (class, layer) slot, deduped
//! across cells and regions, and its layer copy is encoded once. A full class opens a sibling
//! rather than growing, so no bind group goes stale. The pool resets only with the map.

use bevy::asset::AssetId;
use bevy::image::Image;
use bevy::platform::collections::HashMap;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;

/// Pooled textures of one (size, format, mips) key in one fixed-capacity `texture_2d_array`.
struct GxPoolClass {
    size: Extent3d,
    format: TextureFormat,
    mips: u32,
    array: Texture,
    view: TextureView,
    capacity: u32,
    /// Layers assigned so far (≤ capacity).
    members: u32,
    /// Queued (source, layer) copies, encoded once by [`GxTexturePool::drain_pending`].
    pending: Vec<(Texture, u32)>,
}

/// A key's first capacity; each sibling opens at four times the largest, up to the layer limit.
const POOL_BASE_CAPACITY: u32 = 8;

/// The render-world pool: a texture id keeps its (class, layer) until the map clears.
#[derive(bevy::prelude::Resource, Default)]
pub(super) struct GxTexturePool {
    classes: Vec<GxPoolClass>,
    assigned: HashMap<AssetId<Image>, (u16, u16)>,
}

impl GxTexturePool {
    /// The pool slot for `id`, assigning one (and queueing its layer copy) on first sight.
    pub(super) fn assign(
        &mut self,
        id: AssetId<Image>,
        g: &GpuImage,
        render_device: &RenderDevice,
    ) -> (u16, u16) {
        if let Some(&slot) = self.assigned.get(&id) {
            return slot;
        }
        let sz = g.texture.size();
        let size = Extent3d {
            width: sz.width,
            height: sz.height,
            depth_or_array_layers: 1,
        };
        let key = (size, g.texture.format(), g.texture.mip_level_count());
        let ci = self.class_with_room(key, render_device);
        let class = &mut self.classes[ci];
        let layer = class.members;
        class.members += 1;
        class.pending.push((g.texture.clone(), layer));
        let slot = (
            u16::try_from(ci).expect("gx pool under u16 classes"),
            u16::try_from(layer).expect("layer under u16 (device limit is 2048)"),
        );
        self.assigned.insert(id, slot);
        slot
    }

    /// The 1×1 white class untextured items bind: never sampled, but the bind group needs a view.
    pub(super) fn white(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) -> u16 {
        let key = (
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureFormat::Rgba8UnormSrgb,
            1,
        );
        if let Some(ci) = self
            .classes
            .iter()
            .position(|c| (c.size, c.format, c.mips) == key)
        {
            return u16::try_from(ci).unwrap();
        }
        let ci = self.class_with_room(key, render_device);
        let class = &mut self.classes[ci];
        class.members = 1; // layer 0: white, written directly (no GpuImage source)
        render_queue.write_texture(
            class.array.as_image_copy(),
            &[255, 255, 255, 255],
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            key.0,
        );
        u16::try_from(ci).unwrap()
    }

    /// The array view a bind group binds for `class`.
    pub(super) fn view(&self, class: u16) -> &TextureView {
        &self.classes[usize::from(class)].view
    }

    pub(super) fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }

    /// A key's class with a free layer, opening a new sibling when every one is full.
    fn class_with_room(
        &mut self,
        key: (Extent3d, TextureFormat, u32),
        render_device: &RenderDevice,
    ) -> usize {
        let max_layers = render_device.limits().max_texture_array_layers;
        if let Some(ci) = self
            .classes
            .iter()
            .position(|c| (c.size, c.format, c.mips) == key && c.members < c.capacity)
        {
            return ci;
        }
        let capacity = self
            .classes
            .iter()
            .filter(|c| (c.size, c.format, c.mips) == key)
            .map(|c| c.capacity)
            .max()
            .map_or(POOL_BASE_CAPACITY, |c| c.saturating_mul(4))
            .min(max_layers);
        // `GX_VRAM`: the array at capacity, BC at block rate else 4 B/texel, ×4/3 with mips.
        if super::gx_perf_enabled() {
            let per_layer = match key.1 {
                TextureFormat::Bc1RgbaUnorm | TextureFormat::Bc1RgbaUnormSrgb => {
                    u64::from(key.0.width) * u64::from(key.0.height) / 2
                }
                TextureFormat::Bc2RgbaUnorm
                | TextureFormat::Bc2RgbaUnormSrgb
                | TextureFormat::Bc3RgbaUnorm
                | TextureFormat::Bc3RgbaUnormSrgb => {
                    u64::from(key.0.width) * u64::from(key.0.height)
                }
                _ => u64::from(key.0.width) * u64::from(key.0.height) * 4,
            };
            let bytes = per_layer * u64::from(capacity) * if key.2 > 1 { 4 } else { 3 } / 3;
            super::GX_VRAM.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
        }
        let array = render_device.create_texture(&TextureDescriptor {
            label: Some("static_gx_pool_array"),
            size: Extent3d {
                width: key.0.width,
                height: key.0.height,
                depth_or_array_layers: capacity,
            },
            mip_level_count: key.2,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: key.1,
            usage: TextureUsages::COPY_DST | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = array.create_view(&TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.classes.push(GxPoolClass {
            size: key.0,
            format: key.1,
            mips: key.2,
            array,
            view,
            capacity,
            members: 0,
            pending: Vec::new(),
        });
        self.classes.len() - 1
    }

    /// Encode and submit every queued layer copy, emptying the queue; submissions are ordered, so
    /// the copies land before the frame's render-graph submit.
    pub(super) fn drain_pending(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        if self.classes.iter().all(|c| c.pending.is_empty()) {
            return;
        }
        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("static_gx_pool_copies"),
        });
        for class in &mut self.classes {
            for (src, layer) in class.pending.drain(..) {
                let (block_w, block_h) = class.format.block_dimensions();
                for mip in 0..class.mips.min(src.mip_level_count()) {
                    let mut dst = class.array.as_image_copy();
                    dst.mip_level = mip;
                    dst.origin.z = layer;
                    let mut s = src.as_image_copy();
                    s.mip_level = mip;
                    encoder.copy_texture_to_texture(
                        s,
                        dst,
                        // Rounded up to whole blocks: wgpu-core's `validate_texture_copy_range`
                        // rejects a partial block even for a whole mip, and a BLP chain ends at
                        // 2x2, under one BC block. Uncompressed blocks are 1x1.
                        Extent3d {
                            width: (class.size.width >> mip).max(1).div_ceil(block_w) * block_w,
                            height: (class.size.height >> mip).max(1).div_ceil(block_h) * block_h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        }
        render_queue.submit([encoder.finish()]);
    }
}
