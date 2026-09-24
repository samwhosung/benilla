//! The owned skin palette: every skinned rig's joint palette, computed here and uploaded to a
//! region of the shared light buffer that `wow_model.wgsl`'s vertex stage skins from, in place of
//! Bevy's `SkinnedMesh`. Rows are rig-relative (`rig_from_joint × inverse_bindpose`, the rig's
//! origin subtracted) and the vertex stage adds `origin − camera` back, so no vertex is an
//! absolute f32 world coordinate, whose ~1 mm ULP at ~9.5 k yards shimmers every frame.

use std::sync::Arc;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::math::Affine3A;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::renderer::RenderQueue;
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::mesh_tag::MAX_RIG_SLOTS;
use crate::vis_chain::VisChainOnly;

/// Bone capacity across every live rig, about twice a Lower Blackrock Spire census (~61 k).
/// Mirrored by `wow_model.wgsl`'s region declaration: keep in sync.
pub(crate) const MAX_PALETTE_BONES: usize = 131_072;

/// Bytes a bone: 3 `vec4` rows of the affine.
const BONE_BYTES: u64 = 48;

/// Byte offset of the rig slot table (a base bone index a slot), after the prop-probe region.
pub(crate) fn rig_table_region_offset() -> u64 {
    crate::lighting::prop_probe_region_offset() + (7 * crate::lighting::MAX_PROP_PROBES * 16) as u64
}

/// Byte offset of the rig origin table (a `vec4` a slot), after the tint table, as in the shader.
pub(crate) fn rig_origin_region_offset() -> u64 {
    crate::instance_tint::region_offset() + crate::instance_tint::region_bytes()
}

/// Bytes the origin table adds (32 KB at 2048 slots).
pub(crate) fn rig_origin_region_bytes() -> u64 {
    MAX_RIG_SLOTS as u64 * 16
}

/// Byte offset of the palette rows, after the slot, tint, origin, mat-anim and straddle-clip
/// tables: last, because `wow_model.wgsl` declares them as the struct's one runtime-sized array.
pub(crate) fn palette_region_offset() -> u64 {
    crate::straddle::region_offset() + crate::straddle::region_bytes()
}

/// Total bytes the slot-indexed regions add to every `wow_light`-layout buffer.
pub(crate) fn palette_regions_bytes() -> u64 {
    (MAX_RIG_SLOTS * 4) as u64
        + crate::instance_tint::region_bytes()
        + rig_origin_region_bytes()
        + crate::mat_anim_table::region_bytes()
        + crate::straddle::region_bytes()
        + MAX_PALETTE_BONES as u64 * BONE_BYTES
}

/// The main-world palette table: `Arc`-shared rows and slot table, the slot and bone-range slabs.
#[derive(Resource)]
pub struct RigPalettes {
    /// `3 × MAX_PALETTE_BONES` rows, the CPU mirror the mouseover picker reads.
    rows: Arc<Vec<[f32; 4]>>,
    /// The row buffer retired by the last clone (`rows_make_mut`), reused once unshared.
    spare: Option<Arc<Vec<[f32; 4]>>>,
    /// Slot → base bone index. Slot 0 is the tag's "no rig" sentinel and never allocated.
    table: Arc<Vec<u32>>,
    /// Slot → the world position its rows are measured from, written with the rows.
    origins: Arc<Vec<[f32; 4]>>,
    origin_generation: u64,
    /// Bumped only for a mirrored slot's origin, so world traffic skips the booth buffers.
    origin_mirror_generation: u64,
    /// Per-slot bone count; 0 = free slot.
    slot_len: Vec<u32>,
    free_slots: Vec<u16>,
    slot_high: usize,
    /// Free bone ranges `(base, len)`, kept sorted by base and coalesced on free.
    free_ranges: Vec<(u32, u32)>,
    /// Per slot: the rows also reach the booth mirror buffers ([`RigPaletteMirrors`]).
    mirrored: Vec<bool>,
    /// Bone ranges rewritten since the last publish; `.2` = the owning slot was mirrored.
    dirty: Vec<(u32, u32, bool)>,
    table_generation: u64,
    peak_slots: usize,
    peak_bones: u32,
    live_bones: u32,
    /// Allocations refused since the last `WOW_RIG_CENSUS` line; reset by [`census_rig_palettes`].
    denied: u32,
    /// The exhaustion warning's once-a-session latch, never reset: [`Self::denied`] resets with
    /// each census, so gating on it would warn once a census window.
    denied_warned: bool,
    /// `WOW_RIG_COST` meters, printed and reset by [`publish_rig_palettes`].
    cost_copies: u32,
    cost_copy_us: f32,
    cost_rows: u32,
}

impl Default for RigPalettes {
    fn default() -> Self {
        Self {
            rows: Arc::new(vec![[0.0; 4]; 3 * MAX_PALETTE_BONES]),
            spare: None,
            table: Arc::new(vec![0; MAX_RIG_SLOTS]),
            origins: Arc::new(vec![[0.0; 4]; MAX_RIG_SLOTS]),
            origin_generation: 0,
            origin_mirror_generation: 0,
            slot_len: vec![0; MAX_RIG_SLOTS],
            mirrored: vec![false; MAX_RIG_SLOTS],
            free_slots: Vec::new(),
            slot_high: 1, // slot 0 = the "no rig" tag sentinel
            free_ranges: vec![(0, MAX_PALETTE_BONES as u32)],
            dirty: Vec::new(),
            table_generation: 0,
            peak_slots: 0,
            peak_bones: 0,
            live_bones: 0,
            denied: 0,
            denied_warned: false,
            cost_copies: 0,
            cost_copy_us: 0.0,
            cost_rows: 0,
        }
    }
}

/// `WOW_RIG_COST=1`: per-frame `[rig-cost]` and `[rig-upload]` lines; one env read when off.
pub fn rig_cost_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_RIG_COST").is_some())
}

/// Copy-on-write on the shared rows: after a publish the extract holds a second reference, so the
/// frame's first write clones, copying only the live prefix ([`RigPalettes::bone_watermark`]).
fn rows_make_mut<'a>(
    rows: &'a mut Arc<Vec<[f32; 4]>>,
    spare: &mut Option<Arc<Vec<[f32; 4]>>>,
    watermark_bones: u32,
    copies: &mut u32,
    copy_us: &mut f32,
) -> &'a mut Vec<[f32; 4]> {
    if Arc::strong_count(rows) > 1 || Arc::weak_count(rows) > 0 {
        let t = std::time::Instant::now();
        let live = 3 * watermark_bones as usize;
        // Reuse the buffer retired two publishes ago once the render world lets go; its stale
        // tail is never uploaded, and `alloc` zeroes a range before use.
        let mut new = match spare.take().and_then(|a| Arc::try_unwrap(a).ok()) {
            Some(v) if v.len() == rows.len() => v,
            _ => vec![[0.0f32; 4]; rows.len()],
        };
        new[..live].copy_from_slice(&rows[..live]);
        *spare = Some(std::mem::replace(rows, Arc::new(new)));
        *copy_us += t.elapsed().as_secs_f32() * 1e6;
        *copies += 1;
    }
    Arc::make_mut(rows)
}

fn rebase(mut world: Affine3A, origin: Vec3) -> Affine3A {
    world.translation -= bevy::math::Vec3A::from(origin);
    world
}

/// [`rebase`] for a `GlobalTransform`.
pub(crate) fn rebase_global(g: GlobalTransform, origin: Vec3) -> GlobalTransform {
    GlobalTransform::from(rebase(g.affine(), origin))
}

/// The origin a rig's rows are measured from, its root's world translation: the rebase's one
/// switch. `WOW_NO_RIG_REBASE=1` makes it the map origin, an A/B that brings the shimmer back.
pub(crate) fn rebase_origin(root_world: Vec3) -> Vec3 {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    match *ON.get_or_init(|| std::env::var_os("WOW_NO_RIG_REBASE").is_none()) {
        true => root_world,
        false => Vec3::ZERO,
    }
}

impl RigPalettes {
    /// The bone offset above which no rig is allocated, the copy-on-write clone's bound; first-fit
    /// allocation over a coalescing free list packs rigs low.
    fn bone_watermark(&self) -> u32 {
        match self.free_ranges.last() {
            Some(&(base, len)) if base + len == MAX_PALETTE_BONES as u32 => base,
            _ => MAX_PALETTE_BONES as u32,
        }
    }

    /// Claim a slot and a bone range for a rig; `None` when either slab is exhausted.
    pub(crate) fn alloc(&mut self, bones: u32) -> Option<(u16, u32)> {
        if bones == 0 {
            return None;
        }
        let (i, &(base, len)) = self
            .free_ranges
            .iter()
            .enumerate()
            .find(|(_, &(_, len))| len >= bones)?;
        let slot = match self.free_slots.pop() {
            Some(s) => s,
            None if self.slot_high < MAX_RIG_SLOTS => {
                self.slot_high += 1;
                (self.slot_high - 1) as u16
            }
            None => return None,
        };
        if len == bones {
            self.free_ranges.remove(i);
        } else {
            self.free_ranges[i] = (base + bones, len - bones);
        }
        // Zero the range: a reused spare buffer may hold a previous occupant's pose, and
        // `write_rig`, `write_rider` and `computed_rigs` read a new rig's rows as zero.
        let (r0, r1) = (3 * base as usize, 3 * (base + bones) as usize);
        let wm = self.bone_watermark();
        let rows = rows_make_mut(
            &mut self.rows,
            &mut self.spare,
            wm,
            &mut self.cost_copies,
            &mut self.cost_copy_us,
        );
        let r1 = r1.min(rows.len());
        rows[r0..r1].fill([0.0; 4]);
        Arc::make_mut(&mut self.table)[slot as usize] = base;
        self.slot_len[slot as usize] = bones;
        self.mirrored[slot as usize] = false;
        self.table_generation += 1;
        self.live_bones += bones;
        self.peak_bones = self.peak_bones.max(self.live_bones);
        self.peak_slots = self
            .peak_slots
            .max(self.slot_high - 1 - self.free_slots.len());
        Some((slot, base))
    }

    /// Return a rig's slot and range. Its rows and origin are zeroed, so a part's stale tag in the
    /// one-frame despawn skew reads a matrix collapsed at the origin, never another rig's pose.
    pub fn free(&mut self, slot: u16) {
        let s = slot as usize;
        let Some(&len) = self.slot_len.get(s).filter(|&&l| l > 0) else {
            return;
        };
        let base = self.table[s];
        self.slot_len[s] = 0;
        self.free_slots.push(slot);
        self.live_bones -= len;
        self.table_generation += 1;
        // The watermark before the range re-enters the free list, so the zeroing below lands
        // inside the copied prefix.
        let wm = self.bone_watermark();
        let rows = rows_make_mut(
            &mut self.rows,
            &mut self.spare,
            wm,
            &mut self.cost_copies,
            &mut self.cost_copy_us,
        );
        rows[3 * base as usize..3 * (base + len) as usize].fill([0.0; 4]);
        self.dirty.push((base, len, self.mirrored[s]));
        self.set_origin(slot, Vec3::ZERO);
        // Insert sorted and coalesce with both neighbours.
        let i = self.free_ranges.partition_point(|&(b, _)| b < base);
        self.free_ranges.insert(i, (base, len));
        if i + 1 < self.free_ranges.len() {
            let (nb, nl) = self.free_ranges[i + 1];
            if base + len == nb {
                self.free_ranges[i].1 += nl;
                self.free_ranges.remove(i + 1);
            }
        }
        if i > 0 {
            let (pb, pl) = self.free_ranges[i - 1];
            if pb + pl == base {
                self.free_ranges[i - 1].1 += self.free_ranges[i].1;
                self.free_ranges.remove(i);
            }
        }
    }

    /// Publish a slot's origin, called by each row writer with the origin it subtracted.
    fn set_origin(&mut self, slot: u16, origin: Vec3) {
        let s = slot as usize;
        let word = [origin.x, origin.y, origin.z, 0.0];
        match self.origins.get(s) {
            Some(&cur) if cur != word => {}
            _ => return,
        }
        Arc::make_mut(&mut self.origins)[s] = word;
        self.origin_generation += 1;
        if self.mirrored[s] {
            self.origin_mirror_generation += 1;
        }
    }

    /// Rewrite one rig's rows from its joints' globals relative to `origin`, the holder's
    /// translation; propagated in absolute space, they keep the rounding propagation spent.
    fn write_rig(
        &mut self,
        rig: &RigSkin,
        ibp: &[Mat4],
        origin: Vec3,
        worlds: &Query<Ref<GlobalTransform>>,
    ) {
        let (slot, base, joints) = (rig.slot, rig.base, &rig.joints);
        let n = (joints.len().min(ibp.len()) as u32).min(rig.len);
        self.cost_rows += n;
        self.set_origin(slot, origin);
        let wm = self.bone_watermark();
        let rows = rows_make_mut(
            &mut self.rows,
            &mut self.spare,
            wm,
            &mut self.cost_copies,
            &mut self.cost_copy_us,
        );
        for b in 0..n as usize {
            let Ok(g) = worlds.get(joints[b]) else {
                continue; // a torn-down joint keeps its last rows
            };
            let m = rebase(g.affine(), origin) * Affine3A::from_mat4(ibp[b]);
            let (m3, t) = (m.matrix3, m.translation);
            let r = 3 * (base as usize + b);
            rows[r] = [m3.x_axis.x, m3.y_axis.x, m3.z_axis.x, t.x];
            rows[r + 1] = [m3.x_axis.y, m3.y_axis.y, m3.z_axis.y, t.y];
            rows[r + 2] = [m3.x_axis.z, m3.y_axis.z, m3.z_axis.z, t.z];
        }
        if n > 0 {
            let mirrored = self.mirrored.get(slot as usize).copied().unwrap_or(false);
            self.dirty.push((base, n, mirrored));
        }
    }

    /// The collapsed lane's row write, `frame × ibp` over frames the world pass composed from a
    /// zero-translation root, so the rows are exact at rig scale.
    pub(crate) fn write_rig_worlds(
        &mut self,
        rig: &RigSkin,
        worlds: &[GlobalTransform],
        ibp: &[Mat4],
        origin: Vec3,
    ) {
        let n = (worlds.len().min(ibp.len()) as u32).min(rig.len);
        let (slot, base) = (rig.slot, rig.base);
        self.cost_rows += n;
        self.set_origin(slot, origin);
        let wm = self.bone_watermark();
        let rows = rows_make_mut(
            &mut self.rows,
            &mut self.spare,
            wm,
            &mut self.cost_copies,
            &mut self.cost_copy_us,
        );
        for b in 0..n as usize {
            let m = worlds[b].affine() * Affine3A::from_mat4(ibp[b]);
            let (m3, t) = (m.matrix3, m.translation);
            let r = 3 * (base as usize + b);
            rows[r] = [m3.x_axis.x, m3.y_axis.x, m3.z_axis.x, t.x];
            rows[r + 1] = [m3.x_axis.y, m3.y_axis.y, m3.z_axis.y, t.y];
            rows[r + 2] = [m3.x_axis.z, m3.y_axis.z, m3.z_axis.z, t.z];
        }
        if n > 0 {
            let mirrored = self.mirrored.get(slot as usize).copied().unwrap_or(false);
            self.dirty.push((base, n, mirrored));
        }
    }

    /// The rider write: every row of `slot` gets one rigid frame in the host rig's space (a model
    /// at bind pose has one placement for every joint), with the host's position as origin. An
    /// unchanged frame is a no-op, by bit equality so a sub-epsilon move still lands.
    pub(crate) fn write_rider(&mut self, slot: u16, frame: Affine3A, origin: Vec3) {
        let s = slot as usize;
        let (Some(&base), Some(&len)) = (self.table.get(s), self.slot_len.get(s)) else {
            return;
        };
        if len == 0 {
            return;
        }
        let (m3, t) = (frame.matrix3, frame.translation);
        let want = [
            [m3.x_axis.x, m3.y_axis.x, m3.z_axis.x, t.x],
            [m3.x_axis.y, m3.y_axis.y, m3.z_axis.y, t.y],
            [m3.x_axis.z, m3.y_axis.z, m3.z_axis.z, t.z],
        ];
        let r0 = 3 * base as usize;
        let origin_word = self.origins.get(s).copied();
        let unchanged = self.rows[r0..r0 + 3] == want
            && origin_word == Some([origin.x, origin.y, origin.z, 0.0]);
        if unchanged {
            return;
        }
        self.cost_rows += len;
        self.set_origin(slot, origin);
        let wm = self.bone_watermark();
        let rows = rows_make_mut(
            &mut self.rows,
            &mut self.spare,
            wm,
            &mut self.cost_copies,
            &mut self.cost_copy_us,
        );
        for b in 0..len as usize {
            let r = 3 * (base as usize + b);
            rows[r..r + 3].copy_from_slice(&want);
        }
        let mirrored = self.mirrored.get(s).copied().unwrap_or(false);
        self.dirty.push((base, len, mirrored));
    }

    /// Flag a booth rig's rows to also reach the mirror buffers ([`RigPaletteMirrors`]).
    pub fn mark_mirrored(&mut self, slot: u16) {
        if let Some(m) = self.mirrored.get_mut(slot as usize) {
            *m = true;
        }
        // Push the origin table to the mirrors unconditionally: the generation only moves for an
        // already-mirrored slot, so an origin written before this call would never reach them.
        self.origin_mirror_generation += 1;
    }

    /// The mouseover picker's read: the slot's world-space palette, origin added back.
    pub fn world_palette(&self, slot: u16, bones: usize) -> Option<Vec<Mat4>> {
        let o = *self.origins.get(slot as usize)?;
        self.rows_at(slot, bones, [o[0], o[1], o[2]])
    }

    /// The rig's rows as the vertex stage blends them, rig-relative: the precision read, since
    /// adding the ~9 k-yard origin would put the f32 grid the lane avoids onto the measurement.
    pub fn rig_rows(&self, slot: u16, bones: usize) -> Option<Vec<Mat4>> {
        self.rows_at(slot, bones, [0.0; 3])
    }

    /// The rows of `slot` as `Mat4`s, `o` added to the translation column.
    fn rows_at(&self, slot: u16, bones: usize, o: [f32; 3]) -> Option<Vec<Mat4>> {
        let s = slot as usize;
        let len = (*self.slot_len.get(s)? as usize).min(bones);
        if len == 0 {
            return None;
        }
        let base = *self.table.get(s)? as usize;
        Some(
            (0..len)
                .map(|b| {
                    let r = 3 * (base + b);
                    let row = |i: usize, off: f32| {
                        let mut v = self.rows[r + i];
                        v[3] += off;
                        Vec4::from_array(v)
                    };
                    Mat4::from_cols(row(0, o[0]), row(1, o[1]), row(2, o[2]), Vec4::W).transpose()
                })
                .collect(),
        )
    }

    /// A slot's origin as the vertex stage reads it; with [`Self::rig_rows`] it reproduces the
    /// shader's `frame_from_local * v + (frame_origin - view.world_position)`.
    pub fn slot_origin(&self, slot: u16) -> Option<Vec3> {
        let o = self.origins.get(slot as usize)?;
        Some(Vec3::new(o[0], o[1], o[2]))
    }

    /// A rider slot's `(rig_origin, row-0 translation)` pair, unsummed, as `WOW_JITTER` measures
    /// it: summing would put the origin's coarse f32 grid onto the row's own motion.
    pub fn rider_placement(&self, slot: u16) -> Option<(Vec3, Vec3)> {
        self.row_placement(slot, 0)
    }

    /// The same pair for any bone: a posed rider (the flexing ranged prop) differs by row.
    pub fn row_placement(&self, slot: u16, bone: u32) -> Option<(Vec3, Vec3)> {
        let s = slot as usize;
        if bone >= *self.slot_len.get(s)? {
            return None;
        }
        let r = 3 * (*self.table.get(s)? + bone) as usize;
        let o = self.origins.get(s)?;
        Some((
            Vec3::new(o[0], o[1], o[2]),
            Vec3::new(self.rows[r][3], self.rows[r + 1][3], self.rows[r + 2][3]),
        ))
    }

    /// How many live rigs have a computed palette (a non-zero first row); the FPS probe prints it,
    /// since an allocated but never-computed rig renders collapsed at the origin.
    pub fn computed_rigs(&self) -> usize {
        (0..self.slot_high)
            .filter(|&s| self.slot_len[s] > 0 && self.rows[3 * self.table[s] as usize] != [0.0; 4])
            .count()
    }

    /// Live and peak occupancy: `(slots, bones, peak_slots, peak_bones)`.
    pub fn occupancy(&self) -> (usize, u32, usize, u32) {
        (
            self.slot_high - 1 - self.free_slots.len(),
            self.live_bones,
            self.peak_slots,
            self.peak_bones,
        )
    }

    /// Slots still allocatable: the doodad reaper reclaims parked rigs below
    /// [`crate::doodad_anim::REAP_LOW_WATER`], and the unit heal waits for room.
    pub fn slot_headroom(&self) -> usize {
        MAX_RIG_SLOTS - self.slot_high + self.free_slots.len()
    }
}

/// A unit built without a palette rig because the table was full: it renders the bind pose until
/// the app's `entities` heal rebuilds it (the `0x60abe0` display-swap teardown, `Reattached` so it
/// does not re-fade) once there is headroom.
#[derive(Component)]
pub struct RigStarved;

/// A registered rig, on the entity that owns its pose: its palette slot, joint entities and inverse
/// bindposes. The slot is freed `on_replace`, not `on_remove`, because a gear or display rebuild
/// overwrites this component on the same entity, and `on_remove` does not fire on an overwrite.
#[derive(Component)]
#[component(on_replace = free_rig_skin)]
pub struct RigSkin {
    pub slot: u16,
    base: u32,
    len: u32,
    pub(crate) joints: Vec<Entity>,
    pub(crate) ibp: Handle<SkinnedMeshInverseBindposes>,
}

impl RigSkin {
    /// The rig's allocated bone count; the collapsed lane has no joint list to measure.
    pub fn bones(&self) -> u32 {
        self.len
    }

    /// The rig's inverse bindposes, for `WOW_JITTER` to probe rows at each bone's bind position: a
    /// row's translation column lies up to a model's height away, magnifying a rotation ~10x.
    pub fn ibp(&self) -> &Handle<SkinnedMeshInverseBindposes> {
        &self.ibp
    }

    /// Allocate a rig for the collapsed lane, written by [`RigPalettes::write_rig_worlds`].
    pub fn allocate_bones(
        palettes: &mut RigPalettes,
        bones: u32,
        ibp: Handle<SkinnedMeshInverseBindposes>,
    ) -> Option<Self> {
        Self::allocate_inner(palettes, bones, Vec::new(), ibp)
    }

    /// Allocate a rig over live joints; `None` when full, and the caller renders the bind pose.
    pub fn allocate(
        palettes: &mut RigPalettes,
        joints: Vec<Entity>,
        ibp: Handle<SkinnedMeshInverseBindposes>,
    ) -> Option<Self> {
        let bones = joints.len() as u32;
        Self::allocate_inner(palettes, bones, joints, ibp)
    }

    fn allocate_inner(
        palettes: &mut RigPalettes,
        bones: u32,
        joints: Vec<Entity>,
        ibp: Handle<SkinnedMeshInverseBindposes>,
    ) -> Option<Self> {
        match palettes.alloc(bones) {
            Some((slot, base)) => Some(Self {
                slot,
                base,
                len: bones,
                joints,
                ibp,
            }),
            None => {
                palettes.denied += 1;
                // Once a session: the lazy-doodad caller retries every frame while drawn, and
                // `denied` already counts the repeats for the census.
                if !palettes.denied_warned {
                    palettes.denied_warned = true;
                    let (s, b, ps, pb) = palettes.occupancy();
                    warn!(
                        "rig palette exhausted ({bones} bones wanted; live {s} slots / {b} bones, \
                         peak {ps}/{pb}) — rig renders at bind pose. Further denials are counted \
                         in `denied`, not logged; run with WOW_RIG_CENSUS for the running tally."
                    );
                }
                None
            }
        }
    }
}

fn free_rig_skin(mut world: DeferredWorld, ctx: HookContext) {
    let slot = world
        .get::<RigSkin>(ctx.entity)
        .map(|r| r.slot)
        .expect("on_remove runs with the component still present");
    world.resource_mut::<RigPalettes>().free(slot);
    // Slots are recycled and also index the tint table: clear it on the same edge, so no dead
    // unit's colour reaches the slot's next owner.
    if let Some(mut tints) = world.get_resource_mut::<crate::instance_tint::InstanceTints>() {
        tints.clear(slot);
    }
    // And the straddle waterline, for the same reason.
    if let Some(mut clips) = world.get_resource_mut::<crate::straddle::WaterClips>() {
        clips.clear(slot);
    }
}

/// On every joint entity: the rig root whose palette it feeds, for the change sweep.
#[derive(Component)]
pub(crate) struct RigJoint(pub(crate) Entity);

/// On every skinned part: the rig root, for CPU readers such as the mouseover picker.
#[derive(Component)]
pub struct RigPart(pub Entity);

/// PostUpdate, after the billboard joint pass (the last joint-world writer): rewrite the rows of
/// every rig with a joint whose `GlobalTransform` changed.
fn compute_rig_palettes(
    changed: Query<&RigJoint, Changed<GlobalTransform>>,
    rigs: Query<&RigSkin>,
    worlds: Query<Ref<GlobalTransform>>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<RigPalettes>,
) {
    let dirty_rigs: HashSet<Entity> = changed.iter().map(|j| j.0).collect();
    for root in dirty_rigs {
        let Ok(rig) = rigs.get(root) else {
            continue; // allocation failed at spawn (bind-pose fallback) or already torn down
        };
        let Some(ibp) = ibps.get(&rig.ibp) else {
            continue;
        };
        // The holder's translation is the origin, so the rows come out rig-sized; any origin is
        // correct and only precision cares, so a holder mid-teardown falls back to the map's.
        let origin = rebase_origin(
            worlds
                .get(root)
                .map(|g| g.translation())
                .unwrap_or_default(),
        );
        palettes.write_rig(rig, ibp, origin, &worlds);
    }
}

/// The render-world mirror: `Arc` bumps and the frame's dirty ranges.
#[derive(Resource, Clone, ExtractResource)]
struct RigPaletteExtract {
    rows: Arc<Vec<[f32; 4]>>,
    table: Arc<Vec<u32>>,
    origins: Arc<Vec<[f32; 4]>>,
    dirty: Arc<Vec<(u32, u32, bool)>>,
    table_generation: u64,
    origin_generation: u64,
    origin_mirror_generation: u64,
}

impl Default for RigPaletteExtract {
    fn default() -> Self {
        Self {
            rows: Arc::new(Vec::new()),
            table: Arc::new(Vec::new()),
            origins: Arc::new(Vec::new()),
            dirty: Arc::new(Vec::new()),
            // != RigPalettes' initial 0, so the first publish uploads even an empty table.
            table_generation: u64::MAX,
            origin_generation: u64::MAX,
            origin_mirror_generation: u64::MAX,
        }
    }
}

/// Main world, after the compute: hand the frame's dirty ranges and the shared rows to extraction.
fn publish_rig_palettes(mut palettes: ResMut<RigPalettes>, mut out: ResMut<RigPaletteExtract>) {
    if palettes.dirty.is_empty()
        && out.table_generation == palettes.table_generation
        && out.origin_generation == palettes.origin_generation
    {
        return;
    }
    let p = palettes.as_mut();
    if rig_cost_enabled() {
        eprintln!(
            "[rig-cost] ranges={} rows={} copies={} copy_ms={:.3}",
            p.dirty.len(),
            p.cost_rows,
            p.cost_copies,
            p.cost_copy_us / 1000.0
        );
        (p.cost_copies, p.cost_copy_us, p.cost_rows) = (0, 0.0, 0);
    }
    out.rows = Arc::clone(&p.rows);
    out.table = Arc::clone(&p.table);
    out.origins = Arc::clone(&p.origins);
    out.dirty = Arc::new(std::mem::take(&mut p.dirty));
    out.table_generation = p.table_generation;
    out.origin_generation = p.origin_generation;
    out.origin_mirror_generation = p.origin_mirror_generation;
}

/// The glue and portrait booths' studio light buffers, which mirror the palette regions.
#[derive(Resource, Clone, Default, ExtractResource)]
pub struct RigPaletteMirrors(
    pub std::collections::HashMap<&'static str, bevy::render::render_resource::Buffer>,
);

/// What the last upload put on the GPU; `dirty` is the published `Arc`'s pointer.
#[derive(Default)]
struct UploadedGenerations {
    table: Option<u64>,
    dirty: u64,
    origin: Option<u64>,
    origin_mirror: Option<u64>,
}

/// Render world (`PrepareResources`): write the dirty rows, and on change the slot and origin
/// tables, into the shared buffer and every mirror.
fn upload_rig_palettes(
    queue: Res<RenderQueue>,
    shared: Option<Res<crate::lighting::SharedLightBuffer>>,
    mirrors: Option<Res<RigPaletteMirrors>>,
    data: Option<Res<RigPaletteExtract>>,
    mut last: Local<UploadedGenerations>,
) {
    let Some(data) = data else { return };
    // The extract clones every frame, so gate on content.
    let dirty_ptr = Arc::as_ptr(&data.dirty) as u64;
    let table_new = last.table != Some(data.table_generation);
    let dirty_new = last.dirty != dirty_ptr && !data.dirty.is_empty();
    // The 32 KB origin table is written whole; the mirrors gate on their own generation.
    let origin_new = last.origin != Some(data.origin_generation) && !data.origins.is_empty();
    let origin_mirror_new =
        last.origin_mirror != Some(data.origin_mirror_generation) && !data.origins.is_empty();
    if !table_new && !dirty_new && !origin_new && !origin_mirror_new {
        return;
    }
    *last = UploadedGenerations {
        table: Some(data.table_generation),
        dirty: dirty_ptr,
        origin: Some(data.origin_generation),
        origin_mirror: Some(data.origin_mirror_generation),
    };
    let cost_t0 = rig_cost_enabled().then(std::time::Instant::now);
    let mut cost_calls = 0u32;
    let mut cost_bytes = 0u64;
    // Coalesce first: each `write_buffer` call costs far more than its bytes.
    let all = coalesce_ranges(data.dirty.iter().map(|&(b, l, _)| (b, l)).collect());
    let mirrored_only = coalesce_ranges(
        data.dirty
            .iter()
            .filter(|&&(_, _, m)| m)
            .map(|&(b, l, _)| (b, l))
            .collect(),
    );
    // Every target takes the slot table; the booth mirrors take only the mirrored ranges.
    let targets = shared
        .iter()
        .map(|s| (&s.0, false))
        .chain(mirrors.iter().flat_map(|m| m.0.values().map(|b| (b, true))));
    for (buffer, mirror_only) in targets {
        if table_new && !data.table.is_empty() {
            queue.write_buffer(
                buffer,
                rig_table_region_offset(),
                bytemuck::cast_slice(&data.table),
            );
        }
        if if mirror_only {
            origin_mirror_new
        } else {
            origin_new
        } {
            queue.write_buffer(
                buffer,
                rig_origin_region_offset(),
                bytemuck::cast_slice(&data.origins),
            );
            cost_calls += 1;
            cost_bytes += rig_origin_region_bytes();
        }
        if dirty_new {
            let ranges = if mirror_only { &mirrored_only } else { &all };
            for &(base, len) in ranges {
                let rows = &data.rows[3 * base as usize..3 * (base + len) as usize];
                queue.write_buffer(
                    buffer,
                    palette_region_offset() + base as u64 * BONE_BYTES,
                    bytemuck::cast_slice(rows),
                );
                cost_calls += 1;
                cost_bytes += len as u64 * BONE_BYTES;
            }
        }
    }
    if let Some(t0) = cost_t0 {
        eprintln!(
            "[rig-upload] calls={cost_calls} kb={} ms={:.3}",
            cost_bytes / 1024,
            t0.elapsed().as_secs_f32() * 1000.0
        );
    }
}

/// The widest gap, in bones, two dirty runs may leave and still upload as one `write_buffer`: a
/// call's fixed cost (a staging buffer each) is about what re-sending this many live rows costs.
const COALESCE_GAP_BONES: u32 = 256;

/// Merge `(base, len)` bone ranges that overlap or lie within [`COALESCE_GAP_BONES`] into runs.
fn coalesce_ranges(ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    coalesce_ranges_with_gap(ranges, COALESCE_GAP_BONES)
}

fn coalesce_ranges_with_gap(mut ranges: Vec<(u32, u32)>, gap: u32) -> Vec<(u32, u32)> {
    ranges.sort_unstable_by_key(|r| r.0);
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (base, len) in ranges {
        if let Some(last) = out.last_mut() {
            if base <= last.0 + last.1 + gap {
                last.1 = (base + len).max(last.0 + last.1) - last.0;
                continue;
            }
        }
        out.push((base, len));
    }
    out
}

/// `WOW_RIG_CENSUS=<secs>` (unparseable: 5): a periodic line of who holds the palette's slots, by
/// lane, with a bones-per-slot histogram, denials, and the prop-probe table's occupancy: probes
/// and rigs split the same 30 `MeshTag` payload bits, so a re-split must read both.
fn rig_census_every() -> Option<f32> {
    static EVERY: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("WOW_RIG_CENSUS")
            .ok()
            .map(|v| v.parse().unwrap_or(5.0))
    })
}

/// The `[rig-census]` printer ([`rig_census_every`]).
#[allow(clippy::type_complexity)] // one census's full lane classification
fn census_rig_palettes(
    time: Res<Time<bevy::time::Real>>,
    rigs: Query<(
        &RigSkin,
        Option<&crate::doodad_anim::DoodadAnimHost>,
        Has<crate::world_unit::WorldUnit>,
    )>,
    probes: Res<crate::lighting::PropProbes>,
    mut palettes: ResMut<RigPalettes>,
    mut next: Local<f32>,
) {
    let Some(every) = rig_census_every() else {
        return;
    };
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + every;
    // Lanes, `(slots, bones)` each: doodad hosts split by their draw gate, units, everything else.
    let (mut doodad, mut doodad_parked, mut unit, mut other) =
        ((0u32, 0u32), (0u32, 0u32), (0u32, 0u32), (0u32, 0u32));
    let mut hist = [0u32; 6]; // bones/slot: ≤2, 3–4, 5–8, 9–16, 17–32, 33+
    for (rig, host, is_unit) in &rigs {
        let b = rig.bones();
        let lane = match (host, is_unit) {
            (Some(h), _) if h.active => &mut doodad,
            (Some(_), _) => &mut doodad_parked,
            (None, true) => &mut unit,
            (None, false) => &mut other,
        };
        lane.0 += 1;
        lane.1 += b;
        hist[match b {
            0..=2 => 0,
            3..=4 => 1,
            5..=8 => 2,
            9..=16 => 3,
            17..=32 => 4,
            _ => 5,
        }] += 1;
    }
    let denied = std::mem::take(&mut palettes.denied);
    let (s, b, ps, pb) = palettes.occupancy();
    let (p_live, p_peak) = probes.occupancy();
    eprintln!(
        "[rig-census] slots={s}/{} bones={b}/{MAX_PALETTE_BONES} peak={ps}/{pb} | \
         doodad={}({}) parked={}({}) unit={}({}) other={}({}) | \
         bones/slot ≤2:{} 3-4:{} 5-8:{} 9-16:{} 17-32:{} 33+:{} | \
         denied +{denied} | probes {p_live}/{p_peak} of {}",
        MAX_RIG_SLOTS - 1,
        doodad.0,
        doodad.1,
        doodad_parked.0,
        doodad_parked.1,
        unit.0,
        unit.1,
        other.0,
        other.1,
        hist[0],
        hist[1],
        hist[2],
        hist[3],
        hist[4],
        hist[5],
        crate::lighting::MAX_PROP_PROBES,
    );
}

pub fn plugin(app: &mut App) {
    app.init_resource::<RigPalettes>()
        .init_resource::<RigPaletteExtract>()
        .init_resource::<RigPaletteMirrors>()
        .add_plugins(ExtractResourcePlugin::<RigPaletteExtract>::default())
        .add_plugins(ExtractResourcePlugin::<RigPaletteMirrors>::default())
        .add_systems(Update, census_rig_palettes)
        .add_systems(
            PostUpdate,
            (
                compute_rig_palettes,
                crate::rig_rider::write_rig_riders,
                publish_rig_palettes,
            )
                .chain()
                // After the last joint-world writer: propagation, then the billboard joint pass,
                // which rewrites joint `GlobalTransform`s in place.
                .after(crate::billboard::BillboardPlace),
        );
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(
            Render,
            upload_rig_palettes.in_set(RenderSystems::PrepareResources),
        );
    }
}

/// Spawn one entity per bone of a model's rest skeleton, each with a [`RigJoint`] to `holder`,
/// the palette's rig root; `root` takes the parentless joints, and differs for an attached model.
pub fn spawn_joints(
    commands: &mut Commands,
    root: Entity,
    holder: Entity,
    skeleton: &benilla_assets::ModelSkeleton,
) -> Vec<Entity> {
    let joints: Vec<Entity> = skeleton
        .joints
        .iter()
        .map(|j| {
            commands
                // Visibility too: held items and spell effects hang under joints, and a gap in the
                // chain warns in Bevy and leaves them visible under a hidden unit. A joint itself
                // renders nothing (`crate::vis_chain`).
                .spawn((
                    Transform::from_translation(j.local_translation),
                    Visibility::default(),
                    RigJoint(holder),
                ))
                .vis_chain_only()
                .id()
        })
        .collect();
    for (i, j) in skeleton.joints.iter().enumerate() {
        let parent = usize::try_from(j.parent)
            .ok()
            .and_then(|p| joints.get(p).copied())
            .unwrap_or(root);
        commands.entity(parent).add_child(joints[i]);
    }
    joints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_coalesce_roundtrip() {
        let mut p = RigPalettes::default();
        let (s1, b1) = p.alloc(10).unwrap();
        let (s2, b2) = p.alloc(20).unwrap();
        let (s3, b3) = p.alloc(30).unwrap();
        assert!(s1 >= 1, "slot 0 is the no-rig sentinel");
        assert_eq!((b1, b2, b3), (0, 10, 30));
        p.free(s2);
        assert_eq!(p.occupancy().0, 2);
        // An exact fit reuses the freed middle range.
        let (s4, b4) = p.alloc(20).unwrap();
        assert_eq!(b4, 10);
        p.free(s1);
        p.free(s4);
        p.free(s3);
        assert_eq!(p.free_ranges, vec![(0, MAX_PALETTE_BONES as u32)]);
        assert_eq!(p.occupancy().0, 0);
        assert_eq!(p.occupancy().2, 3, "peak slots");
        assert_eq!(p.occupancy().3, 60, "peak bones");
    }

    #[test]
    fn freed_rows_read_zero_not_stale() {
        let mut p = RigPalettes::default();
        let (slot, base) = p.alloc(2).unwrap();
        Arc::make_mut(&mut p.rows)[3 * base as usize] = [1.0; 4];
        p.free(slot);
        assert_eq!(p.rows[3 * base as usize], [0.0; 4]);
        // The free is a dirty range too, so the GPU copy zeroes as well.
        assert!(p.dirty.contains(&(base, 2, false)));
    }

    /// The copy-on-write clone copies only the live prefix; a free while shared still zeroes.
    #[test]
    fn a_shared_arc_write_copies_only_the_live_prefix() {
        let mut p = RigPalettes::default();
        let (_s1, b1) = p.alloc(2).unwrap();
        let (s2, b2) = p.alloc(3).unwrap();
        for r in 3 * b1 as usize..3 * (b1 + 2) as usize {
            Arc::make_mut(&mut p.rows)[r] = [1.0; 4];
        }
        for r in 3 * b2 as usize..3 * (b2 + 3) as usize {
            Arc::make_mut(&mut p.rows)[r] = [2.0; 4];
        }
        assert_eq!(p.bone_watermark(), 5, "first-fit packs the content low");
        // The extract's held reference, as at every frame's first write.
        let held = p.rows.clone();
        let wm = p.bone_watermark();
        let rows = rows_make_mut(
            &mut p.rows,
            &mut p.spare,
            wm,
            &mut p.cost_copies,
            &mut p.cost_copy_us,
        );
        rows[0] = [9.0; 4];
        assert_eq!(p.cost_copies, 1, "the shared write copied");
        assert_eq!(
            p.rows[3 * b2 as usize],
            [2.0; 4],
            "the neighbour survived the prefix copy"
        );
        assert_eq!(held[0], [1.0; 4], "the held (extracted) side is untouched");
        // Free the top rig while shared: the zeroing must land inside the copied prefix.
        let held2 = p.rows.clone();
        p.free(s2);
        assert_eq!(
            p.rows[3 * b2 as usize],
            [0.0; 4],
            "freed rows zeroed through the COW"
        );
        assert_eq!(
            held2[3 * b2 as usize],
            [2.0; 4],
            "the held side keeps the old frame"
        );
        assert_eq!(
            p.bone_watermark(),
            2,
            "the tail grew back down to the live rig"
        );
        // A fresh shared write after the shrink still carries the survivor whole.
        let _held3 = p.rows.clone();
        let wm = p.bone_watermark();
        let rows = rows_make_mut(
            &mut p.rows,
            &mut p.spare,
            wm,
            &mut p.cost_copies,
            &mut p.cost_copy_us,
        );
        rows[1] = [7.0; 4];
        assert_eq!(
            p.rows[0], [9.0; 4],
            "the live rig's rows survive every bounded copy"
        );
    }

    #[test]
    fn alloc_fails_gracefully_at_capacity() {
        let mut p = RigPalettes::default();
        let (big, _) = p.alloc(MAX_PALETTE_BONES as u32).unwrap();
        assert!(p.alloc(1).is_none(), "no bones left");
        p.free(big);
        // Slot exhaustion, separately from bone exhaustion.
        let taken: Vec<u16> = (0..MAX_RIG_SLOTS - 1)
            .map(|_| p.alloc(1).unwrap().0)
            .collect();
        assert!(p.alloc(1).is_none(), "no slots left");
        for s in taken {
            p.free(s);
        }
        assert!(p.alloc(1).is_some());
    }

    #[test]
    fn a_rider_frame_fills_every_row_and_re_writing_it_is_free() {
        let mut p = RigPalettes::default();
        let skin = RigSkin::allocate_bones(&mut p, 4, Handle::default()).unwrap();
        let origin = Vec3::new(-9464.31, 62.17, 56.91);
        let frame = Affine3A::from_rotation_translation(
            Quat::from_rotation_y(0.7),
            Vec3::new(0.3, 1.4, -0.2),
        );
        p.dirty.clear();
        p.write_rider(skin.slot, frame, origin);
        assert_eq!(p.dirty.len(), 1, "the first write dirties the range");
        assert_eq!(p.dirty[0].1, 4, "and dirties ALL of it, not just row 0");

        let pal = p.world_palette(skin.slot, 4).unwrap();
        let want = Mat4::from(Affine3A::from_translation(origin) * frame);
        for (b, m) in pal.iter().enumerate() {
            assert!(m.abs_diff_eq(want, 1e-2), "row {b}: {m:?} vs {want:?}");
        }
        // And the unsummed pair the vertex stage actually reads.
        let (o, t) = p.rider_placement(skin.slot).unwrap();
        assert_eq!(o, origin);
        assert!(t.abs_diff_eq(Vec3::from(frame.translation), 1e-6));

        p.dirty.clear();
        p.write_rider(skin.slot, frame, origin);
        assert!(
            p.dirty.is_empty(),
            "an unchanged rider frame must not dirty its range"
        );
        // A sub-epsilon move still lands: bit equality, not an epsilon.
        p.write_rider(
            skin.slot,
            Affine3A::from_translation(Vec3::new(0.3, 1.4, -0.2 + 1.0e-6)) * frame,
            origin,
        );
        assert_eq!(p.dirty.len(), 1, "a sub-millimetre move is still a move");
    }

    #[test]
    fn world_palette_reconstructs_the_affine_including_the_rig_origin() {
        let mut p = RigPalettes::default();
        let (slot, base) = p.alloc(1).unwrap();
        // A 90° turn about Y and a (1, 2, 3) offset, in a rig far from the map origin.
        let origin = Vec3::new(-9464.31, 62.17, 56.91);
        let m = Mat4::from_rotation_translation(
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(1.0, 2.0, 3.0),
        );
        let a = Affine3A::from_mat4(m);
        p.set_origin(slot, origin);
        let rows = Arc::make_mut(&mut p.rows);
        let (m3, t) = (a.matrix3, a.translation);
        let r = 3 * base as usize;
        rows[r] = [m3.x_axis.x, m3.y_axis.x, m3.z_axis.x, t.x];
        rows[r + 1] = [m3.x_axis.y, m3.y_axis.y, m3.z_axis.y, t.y];
        rows[r + 2] = [m3.x_axis.z, m3.y_axis.z, m3.z_axis.z, t.z];
        let want = Mat4::from_translation(origin) * m;
        let pal = p.world_palette(slot, 1).unwrap();
        assert!(pal[0].abs_diff_eq(want, 1e-2), "{:?} vs {want:?}", pal[0]);
        let q = Vec3::new(0.5, -1.0, 2.0);
        assert!(pal[0]
            .transform_point3(q)
            .abs_diff_eq(want.transform_point3(q), 1e-2));
        // The origin is load-bearing here, not a rounding-sized detail.
        assert!(
            p.world_palette(slot, 1).unwrap()[0]
                .w_axis
                .truncate()
                .distance(origin + Vec3::new(1.0, 2.0, 3.0))
                < 1e-2
        );
    }

    /// A slot recycles: the freed rig's origin must not ride into the next unit that claims it.
    #[test]
    fn freeing_a_slot_clears_its_origin() {
        let mut p = RigPalettes::default();
        let (slot, _) = p.alloc(4).unwrap();
        p.set_origin(slot, Vec3::new(-9464.31, 62.17, 56.91));
        assert_ne!(p.origins[slot as usize], [0.0; 4]);
        p.free(slot);
        assert_eq!(p.origins[slot as usize], [0.0; 4]);
    }

    #[test]
    fn dirty_ranges_coalesce_into_runs() {
        // Adjacent and overlapping merge, a hole splits, input order does not matter.
        assert_eq!(
            coalesce_ranges_with_gap(vec![(70, 5), (0, 10), (10, 20), (25, 10), (40, 5)], 0),
            vec![(0, 35), (40, 5), (70, 5)]
        );
        assert_eq!(coalesce_ranges(Vec::new()), Vec::new());
    }

    #[test]
    fn dirty_ranges_bridge_small_gaps_but_not_large_ones() {
        // A parked rig between two live ones is bridged; a wider gap still splits.
        assert_eq!(
            coalesce_ranges_with_gap(vec![(0, 10), (14, 6), (30, 5)], 4),
            vec![(0, 20), (30, 5)]
        );
        // The shipped tolerance bridges a gap of exactly its size and no more.
        let g = COALESCE_GAP_BONES;
        assert_eq!(
            coalesce_ranges(vec![(0, 10), (10 + g, 5)]),
            vec![(0, 15 + g)]
        );
        assert_eq!(
            coalesce_ranges(vec![(0, 10), (11 + g, 5)]),
            vec![(0, 10), (11 + g, 5)]
        );
    }

    #[test]
    fn region_layout_is_consistent() {
        // `wow_model.wgsl` mirrors this layout, and wgpu checks the bound size against its struct
        // at draw time, so a mismatch fails there.
        assert_eq!(
            crate::instance_tint::region_offset() - rig_table_region_offset(),
            (MAX_RIG_SLOTS * 4) as u64,
            "the tint table follows the slot table"
        );
        assert_eq!(
            rig_origin_region_offset() - crate::instance_tint::region_offset(),
            crate::instance_tint::region_bytes(),
            "the rig-origin table follows the tint table"
        );
        assert_eq!(
            crate::mat_anim_table::region_offset() - rig_origin_region_offset(),
            rig_origin_region_bytes(),
            "the mat-anim table follows the origin table (decision 1381)"
        );
        assert_eq!(
            crate::straddle::region_offset() - crate::mat_anim_table::region_offset(),
            crate::mat_anim_table::region_bytes(),
            "the straddle clip table follows the mat-anim table (decision 2188)"
        );
        assert_eq!(
            palette_region_offset() - crate::straddle::region_offset(),
            crate::straddle::region_bytes(),
            "the palette rows follow the straddle clip table"
        );
        assert_eq!(
            palette_regions_bytes(),
            (MAX_RIG_SLOTS * 4) as u64
                + crate::instance_tint::region_bytes()
                + rig_origin_region_bytes()
                + crate::mat_anim_table::region_bytes()
                + crate::straddle::region_bytes()
                + (MAX_PALETTE_BONES as u64) * BONE_BYTES
        );
        // One `vec4` a slot: the shader declares `array<vec4<f32>, 2048>`.
        assert_eq!(rig_origin_region_bytes(), (MAX_RIG_SLOTS * 16) as u64);
        // One word a slot: the shader declares `array<u32, 2048>` for both tables.
        assert_eq!(
            crate::instance_tint::region_bytes(),
            (MAX_RIG_SLOTS * 4) as u64
        );
    }
}
