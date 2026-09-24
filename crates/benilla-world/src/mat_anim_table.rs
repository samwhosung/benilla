//! The mat-anim table: per-frame samples of every UV-scroll and tint-animated batch material,
//! delivered through the `wow_light` buffer rather than by mutating the material asset.
//!
//! Rows are deltas from the material's build-time seed (`sun_scale.zw = sample(0.0)`, the `tint`
//! first key), and row 0 is pinned to zero, so a static material (`anim_slots = 0`), a zeroed
//! studio buffer, a skipped tick and an exhausted table all read the seed. The region sits between
//! the rig-origin table and the straddle clip table, the palette last; `wow_model.wgsl`'s struct
//! mirrors this order.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

/// Slots in the table, row 0 included. A batch whose loop differs by sequence takes a row per
/// placement (about 145 within half a second of entering Upper Blackrock Spire), and exhaustion
/// is silent: the batch freezes at its seed. 2048 costs 32 KB per `wow_light`-layout buffer.
pub(crate) const MAX_MAT_ANIM_SLOTS: usize = 2048;

/// Byte offset of the region in a `wow_light`-layout buffer, in `wow_model.wgsl`'s struct order.
pub(crate) fn region_offset() -> u64 {
    crate::rig_palette::rig_origin_region_offset() + crate::rig_palette::rig_origin_region_bytes()
}

/// Bytes this region adds to every `wow_light`-layout buffer (32 KB at 2048 slots).
pub(crate) fn region_bytes() -> u64 {
    (MAX_MAT_ANIM_SLOTS * 16) as u64
}

/// Off-world `wow_light`-layout buffers that also carry the region, by key. A lane whose materials
/// bind their own light buffer (the UI model tiles) reads `matanim[slot]` from it, so the table is
/// uploaded there too. The portrait booths are not on it: their zeroed region is the seed pose.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct MatAnimMirrors(
    pub std::collections::HashMap<&'static str, bevy::render::render_resource::Buffer>,
);

/// The live delta table, `Arc`-shared for a cheap extract and generation-stamped so an unchanged
/// table uploads nothing.
#[derive(Resource, Clone, ExtractResource)]
pub struct MatAnimTable {
    rows: Arc<Vec<[f32; 4]>>,
    generation: u64,
    /// The next never-used slot and the freed ones; slot 0 is never handed out.
    next: u16,
    free: Vec<u16>,
}

impl Default for MatAnimTable {
    fn default() -> Self {
        Self {
            rows: Arc::new(vec![[0.0; 4]; MAX_MAT_ANIM_SLOTS]),
            generation: 0,
            next: 1,
            free: Vec::new(),
        }
    }
}

impl MatAnimTable {
    /// Allocate a slot, never 0; on `None` the caller does not register and the batch stays at its
    /// seed.
    pub fn alloc(&mut self) -> Option<u16> {
        if let Some(slot) = self.free.pop() {
            return Some(slot);
        }
        if (self.next as usize) < MAX_MAT_ANIM_SLOTS {
            let slot = self.next;
            self.next += 1;
            Some(slot)
        } else {
            None
        }
    }

    /// Slot `slot`'s row as last written, identity when out of range.
    pub fn row(&self, slot: u16) -> [f32; 4] {
        self.rows.get(slot as usize).copied().unwrap_or([0.0; 4])
    }

    /// Free a slot, zeroing its row so the next owner never inherits a dead batch's delta.
    pub fn free(&mut self, slot: u16) {
        self.set(slot, [0.0; 4]);
        self.free.push(slot);
    }

    /// Write slot `slot`'s delta row; a same-value write costs nothing, and slot 0, which every
    /// static batch reads, is refused.
    pub fn set(&mut self, slot: u16, row: [f32; 4]) {
        let i = slot as usize;
        if i == 0 || i >= MAX_MAT_ANIM_SLOTS || self.rows[i] == row {
            return;
        }
        Arc::make_mut(&mut self.rows)[i] = row;
        self.generation += 1;
    }

    /// This slot's current row, for tests.
    #[cfg(test)]
    pub(crate) fn get(&self, slot: u16) -> [f32; 4] {
        self.rows.get(slot as usize).copied().unwrap_or([0.0; 4])
    }
}

/// A texture transform's rotation and scale as deltas from identity, `[cos − 1, sin, sx − 1,
/// sy − 1]`, with `cos` and `sin` the quaternion's `1 − 2z²` and `2zw`; identity is the zero row.
pub fn affine_row(q: [f32; 4], scale: [f32; 2]) -> [f32; 4] {
    let (c, s) = benilla_formats::rotation_2x2(q);
    [c - 1.0, s, scale[0] - 1.0, scale[1] - 1.0]
}

/// Render world (`PrepareResources`): write the whole region to every carrying buffer when the
/// generation moved.
fn upload_mat_anim(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    mirrors: Option<Res<MatAnimMirrors>>,
    table: Option<Res<MatAnimTable>>,
    mut last: Local<Option<u64>>,
) {
    let Some(table) = table else { return };
    // The gate folds in the mirror count, so a mirror registered after the last write gets rows.
    let mirrors = mirrors
        .map(|m| m.0.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let gate = table.generation ^ ((mirrors.len() as u64) << 40);
    if *last == Some(gate) {
        return;
    }
    *last = Some(gate);
    let rows = bytemuck::cast_slice(table.rows.as_slice());
    for buffer in shared.iter().map(|s| &s.0).chain(mirrors.iter()) {
        queue.write_buffer(buffer, region_offset(), rows);
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<MatAnimTable>()
        .init_resource::<MatAnimMirrors>()
        .add_plugins(ExtractResourcePlugin::<MatAnimTable>::default())
        .add_plugins(ExtractResourcePlugin::<MatAnimMirrors>::default());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            upload_mat_anim.in_set(RenderSystems::PrepareResources),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_affine_identity_is_the_zero_row() {
        assert_eq!(affine_row([0.0, 0.0, 0.0, 1.0], [1.0, 1.0]), [0.0; 4]);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let row = affine_row([0.0, 0.0, r, r], [2.0, 0.5]);
        assert!(
            (row[0] + 1.0).abs() < 1e-6 && (row[1] - 1.0).abs() < 1e-6,
            "{row:?}"
        );
        assert!(
            (row[2] - 1.0).abs() < 1e-6 && (row[3] + 0.5).abs() < 1e-6,
            "{row:?}"
        );
    }

    #[test]
    fn slot_zero_is_never_written() {
        let mut t = MatAnimTable::default();
        t.set(0, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(t.get(0), [0.0; 4]);
        assert_eq!(t.generation, 0, "and it costs no upload");
    }

    #[test]
    fn alloc_free_recycles_and_zeroes() {
        let mut t = MatAnimTable::default();
        let a = t.alloc().unwrap();
        assert_eq!(a, 1);
        t.set(a, [0.25, -0.5, 0.0, 0.0]);
        assert_eq!(t.get(a), [0.25, -0.5, 0.0, 0.0]);
        t.free(a);
        assert_eq!(t.get(a), [0.0; 4], "freed row is identity");
        assert_eq!(t.alloc().unwrap(), a, "freed slot recycles first");
    }

    #[test]
    fn the_generation_tracks_real_changes_only() {
        let mut t = MatAnimTable::default();
        let s = t.alloc().unwrap();
        assert_eq!(t.generation, 0, "allocation alone uploads nothing");
        t.set(s, [0.1, 0.2, 0.0, 0.0]);
        assert_eq!(t.generation, 1);
        t.set(s, [0.1, 0.2, 0.0, 0.0]);
        assert_eq!(t.generation, 1, "same row, no upload");
    }

    #[test]
    fn exhaustion_is_none_and_recoverable() {
        let mut t = MatAnimTable::default();
        let slots: Vec<u16> = std::iter::from_fn(|| t.alloc()).collect();
        assert_eq!(slots.len(), MAX_MAT_ANIM_SLOTS - 1, "slot 0 is reserved");
        assert!(t.alloc().is_none());
        t.free(slots[7]);
        assert_eq!(t.alloc(), Some(slots[7]));
    }
}
