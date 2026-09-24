//! The shared texture-array pool of the retained pass (B3, decision 1432; split from
//! `render.rs` at the line budget — one concern: texture residency for the whole lane).
//!
//! B2's legs caught the per-cell array design's two driver taxes red-handed in a `sample` of
//! the armed process: every re-bake recreated a cell's whole array set (`AGX::TextureGen4`
//! hot — +154 MB of arrays in a 20-s walk), and the per-cell `pending` copy list was never
//! drained, so the node re-encoded every visible cell's full layer-copy set EVERY FRAME
//! (`checkDependentBlits` hot). The pool fixes both structurally: a texture is assigned one
//! (class, layer) slot for the pool's lifetime — deduped across every cell and region — its
//! layer copy is encoded ONCE (`drain_pending`, the same frame it is queued), and a re-bake
//! touches no texture at all. Classes grow by SIBLING (a full class opens a bigger one
//! beside it, capacity ×4), so an existing array is never replaced and no bind group ever
//! goes stale; the cost is that runs cannot fuse across siblings, which leaves draw counts
//! trivial either way. The pool resets only with the map (`prepare_static_gx` sees both
//! published maps empty).

use bevy::asset::AssetId;
use bevy::image::Image;
use bevy::platform::collections::HashMap;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;

/// One class of the pool: every pooled texture of one (size, format, mips) key, as one
/// `texture_2d_array` with fixed capacity.
struct GxPoolClass {
    size: Extent3d,
    format: TextureFormat,
    mips: u32,
    array: Texture,
    view: TextureView,
    capacity: u32,
    /// Layers assigned so far (≤ capacity).
    members: u32,
    /// Layer copies queued but not yet encoded (source texture, destination layer) — drained
    /// by [`GxTexturePool::drain_pending`] the SAME frame they are queued, exactly once.
    pending: Vec<(Texture, u32)>,
}

/// A new class's starting capacity; siblings of a full key open at ×4 (8 → 32 → 128 → …,
/// clamped to the device layer limit) — geometric growth without ever migrating a layer.
const POOL_BASE_CAPACITY: u32 = 8;

/// The pool resource (render world). Assignment is stable for the pool's lifetime: a texture
/// id maps to one (class, layer) until the map clears.
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

    /// The slot untextured items bind (the shader never samples them — the TEXTURED bit is
    /// clear — but the bind group needs a D2-array view): a 1×1 white class, created once.
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

    /// Find a key's class with a free layer, or open one (a sibling at ×4 when the key's
    /// classes are all full).
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
        // The VRAM ledger (1431's regression hunt): bytes for the array about to be created —
        // block-compressed at their block rate, else 4 B/texel — ×4/3 for mips, over the full
        // CAPACITY (what the driver allocates). Pooled, this should go near-flat after the
        // first minutes; a climbing ledger is the churn coming back.
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

    /// Encode + submit all queued layer copies (one encoder, one submit, then EMPTY — the
    /// whole point; queue submissions are ordered, so the copies land before the frame's
    /// render-graph submit).
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
                        // The **physical** mip extent — rounded up to whole blocks. wgpu-core's
                        // `validate_texture_copy_range` checks `copy_size % block_dimensions`
                        // unconditionally (there is no "it's the whole mip" exemption), and a
                        // BLP's authored chain bottoms out at 2x2 — below a BC block on EVERY
                        // texture. Uncompressed formats have 1x1 blocks, so this is the identity
                        // there and the line reads the same for both. Without the round-up, the
                        // first BC texture pooled here is a validation error, and wgpu's default
                        // handler makes that a process panic.
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
