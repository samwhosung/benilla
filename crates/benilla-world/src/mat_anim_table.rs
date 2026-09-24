//! The shared **mat-anim table** — the per-frame samples of every UV-scroll /
//! tint-animated batch material, delivered through the `wow_light` buffer instead of by mutating
//! the material asset.
//!
//! **Why.** `doodad_anim::tick_anim_materials` used to write each drawn animated material's
//! sampled offset into the asset every frame (`sun_scale.zw` / `tint`). Every such write is an
//! `AssetEvent::Modified`: a uniform re-upload plus, on the Metal non-bindless path, a bind-group
//! rebuild (~57 µs per material — B131's measurement), plus the whole-population `AssetChanged`
//! walks it arms (`mark_meshes_as_changed_if_their_materials_changed`, the per-material-type
//! specialization checks), plus the far-twin mirror's whole-material re-insert
//! (`model_render::classify_water_side`). 1375 bounded the *scan* and gated *unchanged* writes;
//! this module removes the mutation itself — the remaining half of 1370's finding.
//!
//! **The encoding: rows are DELTAS from the built seed, and zero is identity.** A registered
//! material keeps its build-time `sun_scale.zw = sample(0.0)` seed (and its `tint` first-key)
//! untouched for ever; the shader adds `matanim[slot]` on top, with **row 0 pinned to zero** so
//! `anim_slots = 0` — every static material — reads a no-op without a branch. Zero-as-identity is
//! [`crate::instance_tint`]'s own argument, and it buys the same three properties here: the
//! portrait booths' studio buffers (zeroed region ⇒ the seed pose, the static studio look),
//! deterministic captures (the tick is skipped ⇒ zero deltas ⇒ bit-identical to the old seed
//! frames), and slot exhaustion (no slot ⇒ frozen at seed, never garbage).
//!
//! Region layout: between the rig-origin table and the straddle clip table ([`crate::straddle`]);
//! the palette rows stay last (the palette array is runtime-sized) — `wow_model.wgsl`'s struct
//! mirrors this order.

use std::sync::Arc;

use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

/// Slots in the table, row 0 included (row 0 is the pinned-zero identity every static material
/// reads). B131 observed material-anim residency grow to ~250 entries on a long single-map
/// session, which is what sized the original 512.
///
/// **Raised to 2048 by decision 1408**, and measured rather than guessed: the per-placement lane
/// gives a batch whose UV/tint loop differs between sequences one row *per placement*, and a live
/// probe into Upper Blackrock Spire armed **145 rows within half a second of world entry** — 22
/// placements of one 5-batch prop, each batch taking a row for its cutout material and another for
/// its blend twin. The old ceiling would have exhausted in one dungeon, and exhaustion is silent by
/// design (the batch stays at its built seed), i.e. it would have re-frozen exactly the bubbles the
/// decision unfroze. The row count is inherent to the fix, not to how it is addressed — a
/// per-instance row in a shared material would need the same 220. 2048 costs 32 KB per
/// `wow_light`-layout buffer.
pub(crate) const MAX_MAT_ANIM_SLOTS: usize = 2048;

/// Byte offset of the mat-anim region inside a `wow_light`-layout buffer: after the rig-origin
/// table, before the straddle clip table — mirroring `wow_model.wgsl`'s struct order.
pub(crate) fn region_offset() -> u64 {
    crate::rig_palette::rig_origin_region_offset() + crate::rig_palette::rig_origin_region_bytes()
}

/// Bytes this region adds to every `wow_light`-layout buffer (32 KB at 2048 slots).
pub(crate) fn region_bytes() -> u64 {
    (MAX_MAT_ANIM_SLOTS * 16) as u64
}

/// Off-world `wow_light`-layout buffers that also carry the mat-anim region — registered by key,
/// the shape of [`crate::instance_tint::InstanceTintMirrors`] and, like it, deliberately its own
/// list.
///
/// A lane whose materials bind a light buffer of their own — the UI model tiles, whose twins
/// carry the widget's black light — reads `matanim[slot]` out of THAT buffer, so rows it writes
/// into the table reach it only if the table is uploaded there too (the cooldown
/// sweep's rotation rows were written every frame into a region only the world's materials ever
/// sampled, and the tile read the zero-initialised identity). The portrait booths are not on it:
/// a bake stands in for a world instance whose animated materials keep the world's rows, and the
/// studio buffers' zeroed region is the seed pose those bakes were built to show.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct MatAnimMirrors(
    pub std::collections::HashMap<&'static str, bevy::render::render_resource::Buffer>,
);

/// The live delta table. `Arc`-shared so the render-world extract is a pointer bump, and
/// generation-stamped so a scene with nothing animated in view uploads nothing at all
/// ([`crate::instance_tint`]'s pattern, verbatim).
#[derive(Resource, Clone, ExtractResource)]
pub struct MatAnimTable {
    rows: Arc<Vec<[f32; 4]>>,
    generation: u64,
    /// Slot allocator: the next never-used slot, and the freed ones. Slot 0 is never handed out.
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
    /// Allocate a slot (1-based; 0 is the identity row). `None` when the table is full — the
    /// caller then simply doesn't register, and the batch stays frozen at its built seed: a
    /// degraded look only a >2047-material session could see, never a wrong pixel.
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

    /// Read slot `slot`'s row as last written (the identity row for an out-of-range slot) — the
    /// tile probe's view of what a lane wrote.
    pub fn row(&self, slot: u16) -> [f32; 4] {
        self.rows.get(slot as usize).copied().unwrap_or([0.0; 4])
    }

    /// Free a slot when its registry entry dies (the material was unloaded): the row zeroes —
    /// back to identity — so the next allocation can never inherit a dead batch's delta.
    pub fn free(&mut self, slot: u16) {
        self.set(slot, [0.0; 4]);
        self.free.push(slot);
    }

    /// Write slot `slot`'s delta row; a same-value write costs nothing (the tick's quantized
    /// samples make equality the common case on slow loops). Slot 0 — the shared identity — is
    /// refused: writing it would scroll every static batch in the world at once.
    pub fn set(&mut self, slot: u16, row: [f32; 4]) {
        let i = slot as usize;
        if i == 0 || i >= MAX_MAT_ANIM_SLOTS || self.rows[i] == row {
            return;
        }
        Arc::make_mut(&mut self.rows)[i] = row;
        self.generation += 1;
    }

    /// This slot's current row (test support — the render path reads the uploaded region).
    #[cfg(test)]
    pub(crate) fn get(&self, slot: u16) -> [f32; 4] {
        self.rows.get(slot as usize).copied().unwrap_or([0.0; 4])
    }
}

/// The **affine row**: a texture transform's rotation and scale as the deltas
/// from the identity the shader adds back — `[cos − 1, sin, sx − 1, sy − 1]`, with `cos`/`sin`
/// the raw quaternion's `1 − 2z²` / `2zw` ([`benilla_formats::rotation_2x2`]). The identity
/// encodes as the zero row, which is what lets slot 0 serve every material with no transform.
pub fn affine_row(q: [f32; 4], scale: [f32; 2]) -> [f32; 4] {
    let (c, s) = benilla_formats::rotation_2x2(q);
    [c - 1.0, s, scale[0] - 1.0, scale[1] - 1.0]
}

/// Render world (`PrepareResources`): write the whole 8 KB region when anything changed, gated on
/// the generation — a scene with no drawn animated material (most of the world) never touches the
/// queue. Whole-region writes for the same reason `instance_tint` chose them: the table is small
/// and the write is one queue call.
fn upload_mat_anim(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    mirrors: Option<Res<MatAnimMirrors>>,
    table: Option<Res<MatAnimTable>>,
    mut last: Local<Option<u64>>,
) {
    let Some(table) = table else { return };
    // The generation gate covers the mirror list too (the tint table's rule): a mirror
    // registered after the last write would otherwise hold the zero region until the next
    // animated frame. One comparison closes the hole by construction.
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

    /// The affine row's identity is the zero row — the whole reason the encoding is deltas.
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

    /// Slot 0 is the identity row every static material in the world reads — the setter refuses
    /// it, and freeing can never zero it "again" into a generation bump.
    #[test]
    fn slot_zero_is_never_written() {
        let mut t = MatAnimTable::default();
        t.set(0, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(t.get(0), [0.0; 4]);
        assert_eq!(t.generation, 0, "and it costs no upload");
    }

    /// The allocator hands out 1.., recycles freed slots, and a freed slot's row is back to
    /// identity before anyone can inherit it.
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

    /// Generation moves only on real content changes — the upload gate's whole premise.
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

    /// Exhaustion answers `None` (the caller skips registration — frozen at seed), and freed
    /// slots make it recoverable.
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
