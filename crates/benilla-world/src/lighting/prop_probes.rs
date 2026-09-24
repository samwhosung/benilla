//! The interior-prop light-probe table: one 7-row SH probe per lit interior MODD prop, folded at
//! spawn ([`super::sh::prop_probe_coeffs`] over the MODD colour, the fixed-axis lobe and the
//! group's MOLR lights) and uploaded behind the shared light blob. The prop's `MeshTag` payload
//! carries its slot, which the [`PropProbeSlot`] hook frees on despawn.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::math::Vec4;
use bevy::prelude::*;
use bevy::render::extract_resource::ExtractResource;

/// Capacity of the probe table, mirrored by `wow_model.wgsl`'s `prop_probes` array (keep in sync):
/// a city WMO streams in thousands of lit interior props at once.
pub const MAX_PROP_PROBES: usize = 8192;

/// A probe's rows, bit for bit: identical props fold to identical bits and share a slot.
#[derive(Clone, PartialEq, Eq, Hash)]
struct ProbeKey([[u32; 4]; 7]);

impl ProbeKey {
    fn of(coeffs: &[Vec4; 7]) -> Self {
        Self(coeffs.map(|v| v.to_array().map(f32::to_bits)))
    }
}

/// The main-world slot table: a slab whose freed slots recycle first, with identical probes sharing
/// a refcounted slot (without it a login scene's prop WMOs overflow the table). The rows sit behind
/// an `Arc`: the render extract is a pointer bump, and `make_mut` copies when a change races it.
#[derive(Resource)]
pub struct PropProbes {
    rows: Arc<Vec<[[f32; 4]; 7]>>,
    free: Vec<u16>,
    high: usize,
    /// Probe bits to live slot, for as long as the slot's refcount lasts.
    by_key: HashMap<ProbeKey, u16>,
    /// Per slot `(refcount, key)`, `None` when free; a `None` key marks an owned, unshared slot.
    slots: Vec<Option<(u32, Option<ProbeKey>)>>,
    /// Bumped on every write; the render-world upload watches it.
    generation: u64,
    /// Peak distinct occupancy, logged by the spawner on overflow.
    peak: usize,
    /// Slots written since the last publish, `[lo, hi)`: the upload writes only this span.
    dirty: Option<(usize, usize)>,
}

impl Default for PropProbes {
    fn default() -> Self {
        Self {
            rows: Arc::new(vec![[[0.0; 4]; 7]; MAX_PROP_PROBES]),
            free: Vec::new(),
            high: 0,
            by_key: HashMap::new(),
            slots: vec![None; MAX_PROP_PROBES],
            generation: 0,
            peak: 0,
            dirty: None,
        }
    }
}

impl PropProbes {
    /// Claims a slot, sharing a live identical probe's; `None` when the table is full of distinct
    /// probes, and the caller then lights the prop as exterior.
    pub fn alloc(&mut self, coeffs: [Vec4; 7]) -> Option<u16> {
        let key = ProbeKey::of(&coeffs);
        if let Some(&slot) = self.by_key.get(&key) {
            if let Some(Some((refs, _))) = self.slots.get_mut(slot as usize) {
                *refs += 1;
                return Some(slot);
            }
        }
        let slot = self.take_free_slot()?;
        Arc::make_mut(&mut self.rows)[slot as usize] = coeffs.map(|v| v.to_array());
        self.by_key.insert(key.clone(), slot);
        self.slots[slot as usize] = Some((1, Some(key)));
        self.touch(slot);
        self.peak = self.peak.max(self.high - self.free.len());
        Some(slot)
    }

    /// Claims an owned slot for a moving entity's probe, never shared, since two units in one room
    /// ramp independently; the holder rewrites it with [`Self::update_owned`].
    pub(crate) fn alloc_owned(&mut self, coeffs: [Vec4; 7]) -> Option<u16> {
        let slot = self.take_free_slot()?;
        Arc::make_mut(&mut self.rows)[slot as usize] = coeffs.map(|v| v.to_array());
        self.slots[slot as usize] = Some((1, None));
        self.touch(slot);
        self.peak = self.peak.max(self.high - self.free.len());
        Some(slot)
    }

    /// Rewrites an owned slot in place. A shared or free slot is refused with a warning: a holder
    /// pointing at one is stale, and a silent no-op would leave it black.
    pub(crate) fn update_owned(&mut self, slot: u16, coeffs: [Vec4; 7]) {
        if !matches!(self.slots.get(slot as usize), Some(Some((_, None)))) {
            warn_once!("update_owned({slot}) on a non-owned slot — the holder's slot is stale");
            return;
        }
        Arc::make_mut(&mut self.rows)[slot as usize] = coeffs.map(|v| v.to_array());
        self.touch(slot);
    }

    fn touch(&mut self, slot: u16) {
        let s = slot as usize;
        self.dirty = Some(match self.dirty {
            Some((lo, hi)) => (lo.min(s), hi.max(s + 1)),
            None => (s, s + 1),
        });
        self.generation += 1;
    }

    fn take_free_slot(&mut self) -> Option<u16> {
        match self.free.pop() {
            Some(s) => Some(s),
            None if self.high < MAX_PROP_PROBES => {
                self.high += 1;
                Some((self.high - 1) as u16)
            }
            None => None,
        }
    }

    /// Live and peak distinct occupancy.
    pub fn occupancy(&self) -> (usize, usize) {
        (self.high - self.free.len(), self.peak)
    }

    /// Drops one reference, freeing at zero; besides the [`PropProbeSlot`] hook, the interior
    /// classifier calls it for a slot whose anchor was despawned before its component landed.
    pub(crate) fn release(&mut self, slot: u16) {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return;
        };
        let Some((refs, _)) = entry else {
            return;
        };
        *refs -= 1;
        if *refs > 0 {
            return;
        }
        let Some((_, key)) = entry.take() else {
            return;
        };
        if let Some(key) = key {
            self.by_key.remove(&key);
        }
        // Zeroed, so a stale `MeshTag` in a frame of despawn skew reads black.
        Arc::make_mut(&mut self.rows)[slot as usize] = [[0.0; 4]; 7];
        self.touch(slot);
        self.free.push(slot);
    }
}

/// The render-world mirror of the probe table, an `Arc` bump per frame.
#[derive(Resource, Clone, ExtractResource)]
pub(crate) struct PropProbeExtract {
    rows: Arc<Vec<[[f32; 4]; 7]>>,
    high: usize,
    generation: u64,
    /// The rows this generation changed, `[lo, hi)`; `None` means everything up to `high`.
    dirty: Option<(usize, usize)>,
}

impl Default for PropProbeExtract {
    fn default() -> Self {
        Self {
            rows: Arc::new(Vec::new()),
            high: 0,
            // Not `PropProbes`' initial 0, so the first publish sends even an empty table.
            generation: u64::MAX,
            dirty: None,
        }
    }
}

/// Main world, after the spawners: publishes the table for extraction when it changed.
pub(super) fn publish_prop_probes(
    mut probes: ResMut<PropProbes>,
    mut out: ResMut<PropProbeExtract>,
) {
    if out.generation != probes.generation {
        // After a skipped generation the whole allocated span goes, not just the last change.
        let consecutive = out.generation.wrapping_add(1) == probes.generation
            || out
                .dirty
                .is_some_and(|_| out.generation != u64::MAX && probes.generation > out.generation);
        let dirty = probes.dirty.take();
        out.dirty = if consecutive { dirty } else { None };
        out.rows = Arc::clone(&probes.rows);
        out.high = probes.high;
        out.generation = probes.generation;
    }
}

/// Byte offset of the probe region in the shared light buffer, right after the per-frame blob.
pub fn prop_probe_region_offset() -> u64 {
    super::global_light::per_frame_blob_bytes()
}

/// Render world, in `PrepareResources`: writes the probe region when the table changed; the
/// per-frame upload writes only the prefix, so the region persists between changes.
pub(super) fn upload_prop_probes(
    queue: Res<bevy::render::renderer::RenderQueue>,
    buffer: Option<Res<super::SharedLightBuffer>>,
    data: Option<Res<PropProbeExtract>>,
    mut last: Local<Option<u64>>,
) {
    let (Some(buffer), Some(data)) = (buffer, data) else {
        return;
    };
    if *last == Some(data.generation) {
        return;
    }
    // Only the changed span when this follows the last upload; the whole allocated span on the
    // first upload and after an unseen generation, so the GPU never keeps a rewritten row.
    let high = data.high.min(data.rows.len());
    let (lo, hi) = match (data.dirty, *last) {
        (Some((lo, hi)), Some(seen)) if seen.wrapping_add(1) == data.generation => {
            (lo.min(high), hi.min(high))
        }
        _ => (0, high),
    };
    *last = Some(data.generation);
    let rows = &data.rows[lo..hi];
    if !rows.is_empty() {
        queue.write_buffer(
            &buffer.0,
            prop_probe_region_offset() + (lo * std::mem::size_of::<[[f32; 4]; 7]>()) as u64,
            bytemuck::cast_slice(rows),
        );
    }
}

/// On one entity per lit interior prop; its hook returns the slot to the table. `on_replace`, not
/// `on_remove`: an insert over a live slot must free the old one, and `on_remove` never fires on
/// an overwrite.
#[derive(Component)]
#[component(on_replace = free_prop_probe_slot)]
pub struct PropProbeSlot(pub u16);

fn free_prop_probe_slot(mut world: DeferredWorld, ctx: HookContext) {
    let slot = world
        .get::<PropProbeSlot>(ctx.entity)
        .map(|s| s.0)
        .expect("on_replace runs with the outgoing component still present");
    world.resource_mut::<PropProbes>().release(slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_probes_share_one_slot_and_refcount() {
        let mut t = PropProbes::default();
        let c = [Vec4::splat(0.5); 7];
        let a = t.alloc(c).unwrap();
        let b = t.alloc(c).unwrap();
        assert_eq!(a, b); // identical content shares one slot
        let other = t.alloc([Vec4::splat(0.25); 7]).unwrap();
        assert_ne!(a, other);
        // The first release keeps the shared slot alive; the second frees it.
        t.release(a);
        assert_ne!(t.rows[a as usize], [[0.0; 4]; 7]);
        t.release(b);
        assert_eq!(t.rows[a as usize], [[0.0; 4]; 7]); // freed reads black, not stale light
        let c2 = t.alloc(c).unwrap(); // the freed slot recycles
        assert_eq!(c2, a);
        assert_eq!(t.high, 2);
    }

    #[test]
    fn overwriting_a_probe_slot_frees_the_old_one() {
        let mut w = World::new();
        let mut t = PropProbes::default();
        let a = t.alloc([Vec4::splat(0.5); 7]).unwrap();
        let b = t.alloc([Vec4::splat(0.25); 7]).unwrap();
        w.insert_resource(t);
        let e = w.spawn(PropProbeSlot(a)).id();
        w.entity_mut(e).insert(PropProbeSlot(b));
        let t = w.resource::<PropProbes>();
        assert_eq!(
            t.rows[a as usize], [[0.0; 4]; 7],
            "the overwritten slot is freed"
        );
        assert_ne!(t.rows[b as usize], [[0.0; 4]; 7], "the new slot is live");
        w.entity_mut(e).despawn();
        let t = w.resource::<PropProbes>();
        assert_eq!(t.rows[b as usize], [[0.0; 4]; 7], "despawn still frees");
    }

    #[test]
    fn owned_slots_never_dedup_and_update_in_place() {
        let mut t = PropProbes::default();
        let c = [Vec4::splat(0.5); 7];
        let a = t.alloc_owned(c).unwrap();
        let b = t.alloc_owned(c).unwrap();
        assert_ne!(a, b); // identical content still gets its own slot
        let g0 = t.generation;
        t.update_owned(a, [Vec4::splat(0.7); 7]);
        assert_eq!(t.rows[a as usize], [[0.7; 4]; 7]);
        assert!(t.generation > g0, "in-place update must republish");
        // A shared slot ignores `update_owned`.
        let d = t.alloc(c).unwrap();
        let before = t.rows[d as usize];
        t.update_owned(d, [Vec4::splat(0.9); 7]);
        assert_eq!(t.rows[d as usize], before);
        // An owned slot frees through the same release, zeroes, and recycles.
        t.release(a);
        assert_eq!(t.rows[a as usize], [[0.0; 4]; 7]);
        assert_eq!(t.alloc_owned(c).unwrap(), a);
    }

    #[test]
    fn alloc_fails_gracefully_at_capacity() {
        let mut t = PropProbes::default();
        for i in 0..MAX_PROP_PROBES {
            let unique = [Vec4::splat(i as f32 + 1.0); 7];
            assert!(t.alloc(unique).is_some());
        }
        assert!(t.alloc([Vec4::splat(-1.0); 7]).is_none());
        // A duplicate of a live probe still succeeds at capacity, costing no slot.
        assert!(t.alloc([Vec4::splat(1.0); 7]).is_some());
    }

    #[test]
    fn extracted_rows_are_copy_on_write() {
        let mut t = PropProbes::default();
        let a = t.alloc([Vec4::splat(0.5); 7]).unwrap();
        let extracted = Arc::clone(&t.rows);
        t.release(a);
        assert_eq!(extracted[a as usize], [[0.5; 4]; 7]); // the render copy is untouched
        assert_eq!(t.rows[a as usize], [[0.0; 4]; 7]);
    }
}
